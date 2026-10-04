use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::apply_patch;
use crate::apply_patch::convert_apply_patch_to_protocol;
use crate::function_tool::FunctionCallError;
use crate::hook_runtime::PreToolUseHookResult;
use crate::hook_runtime::record_additional_contexts;
use crate::hook_runtime::run_post_tool_use_hooks;
use crate::hook_runtime::run_pre_tool_use_hooks;
use crate::safety::PatchPolicyMatcher;
use crate::safety::PatchSandboxRoute;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ApplyPatchToolOutput;
use crate::tools::context::ExecCommandToolOutput;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch_spec::create_apply_patch_freeform_tool;
use crate::tools::handlers::file_system_sandbox_policy_context_for_cwd;
use crate::tools::handlers::implicit_granted_permissions;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::handlers::unified_exec::ExecCommandArgs;
use crate::tools::handlers::unified_exec::bash_post_tool_use_payload;
use crate::tools::handlers::unified_exec::get_command;
use crate::tools::handlers::unified_exec::shell_mode_for_environment;
use crate::tools::handlers::updated_hook_command;
use crate::tools::hook_names::HookToolName;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PostToolUsePayload;
use crate::tools::registry::PreToolUsePayload;
use crate::tools::registry::ToolArgumentDiffConsumer;
use crate::tools::registry::ToolExecutor;
use crate::tools::runtimes::apply_patch::ApplyPatchRequest;
use crate::tools::runtimes::apply_patch::ApplyPatchRuntime;
use crate::tools::sandboxing::ToolCtx;
use crate::unified_exec::ExecCommandRequest;
use crate::unified_exec::UnifiedExecContext;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcessManager;
use crate::windows_sandbox::windows_sandbox_level_for_legacy_checks;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::ApplyPatchFileChange;
use codex_apply_patch::ApplyPatchFileUpdateMode;
use codex_apply_patch::Hunk;
use codex_apply_patch::StreamingPatchParser;
use codex_exec_server::ExecutorFileSystem;
use codex_features::Feature;
use codex_hooks::PostToolUseOutcome;
use codex_protocol::items::apply_patch_check_item_id;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::PatchApplyUpdatedEvent;
use codex_sandboxing::policy_transforms::effective_file_system_sandbox_policy;
use codex_sandboxing::policy_transforms::merge_permission_profiles;
use codex_sandboxing::policy_transforms::normalize_additional_permissions;
use codex_sandboxing::policy_transforms::normalize_additional_permissions_with_context;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_output_truncation::approx_token_count;
use codex_utils_path_uri::PathUri;

const APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL: Duration = Duration::from_millis(500);

fn apply_patch_file_update_mode(turn: &TurnContext) -> ApplyPatchFileUpdateMode {
    if turn
        .config
        .features
        .enabled(Feature::ApplyPatchPreserveLineEndings)
    {
        ApplyPatchFileUpdateMode::PreserveLineEndings
    } else {
        ApplyPatchFileUpdateMode::NormalizeToLf
    }
}

/// Handles freeform `apply_patch` requests and routes verified patches to the
/// selected environment filesystem.
#[derive(Default)]
pub struct ApplyPatchHandler {
    multi_environment: bool,
}

impl ApplyPatchHandler {
    pub(crate) fn new(multi_environment: bool) -> Self {
        Self { multi_environment }
    }
}

#[derive(Default)]
struct ApplyPatchArgumentDiffConsumer {
    parser: StreamingPatchParser,
    last_sent_at: Option<Instant>,
    pending: Option<PatchApplyUpdatedEvent>,
}

impl ToolArgumentDiffConsumer for ApplyPatchArgumentDiffConsumer {
    fn consume_diff(
        &mut self,
        turn: &TurnContext,
        call_id: String,
        diff: &str,
    ) -> Option<EventMsg> {
        if !turn
            .config
            .features
            .enabled(Feature::ApplyPatchStreamingEvents)
        {
            return None;
        }

        self.push_delta(call_id, diff)
            .map(EventMsg::PatchApplyUpdated)
    }

    fn finish(&mut self) -> Result<Option<EventMsg>, FunctionCallError> {
        self.finish_update_on_complete()
            .map(|event| event.map(EventMsg::PatchApplyUpdated))
    }
}

impl ApplyPatchArgumentDiffConsumer {
    fn push_delta(&mut self, call_id: String, delta: &str) -> Option<PatchApplyUpdatedEvent> {
        let hunks = self.parser.push_delta(delta).ok()?;
        if hunks.is_empty() {
            return None;
        }
        let changes = convert_apply_patch_hunks_to_protocol(&hunks);
        let event = PatchApplyUpdatedEvent { call_id, changes };
        let now = Instant::now();
        match self.last_sent_at {
            Some(last_sent_at)
                if now.duration_since(last_sent_at) < APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL =>
            {
                self.pending = Some(event);
                None
            }
            Some(_) | None => {
                self.pending = None;
                self.last_sent_at = Some(now);
                Some(event)
            }
        }
    }

    fn finish_update_on_complete(
        &mut self,
    ) -> Result<Option<PatchApplyUpdatedEvent>, FunctionCallError> {
        self.parser.finish().map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to parse apply_patch: {err}"))
        })?;

        let event = self.pending.take();
        if event.is_some() {
            self.last_sent_at = Some(Instant::now());
        }
        Ok(event)
    }
}

fn convert_apply_patch_hunks_to_protocol(hunks: &[Hunk]) -> HashMap<PathBuf, FileChange> {
    hunks
        .iter()
        .map(|hunk| {
            let path = hunk_source_path(hunk).to_path_buf();
            let change = match hunk {
                Hunk::AddFile { contents, .. } => FileChange::Add {
                    content: contents.clone(),
                },
                Hunk::DeleteFile { .. } => FileChange::Delete {
                    content: String::new(),
                },
                Hunk::UpdateFile {
                    chunks, move_path, ..
                } => FileChange::Update {
                    unified_diff: format_update_chunks_for_progress(chunks),
                    move_path: move_path.clone(),
                },
            };
            (path, change)
        })
        .collect()
}

fn hunk_source_path(hunk: &Hunk) -> &Path {
    match hunk {
        Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } | Hunk::UpdateFile { path, .. } => {
            path
        }
    }
}

fn format_update_chunks_for_progress(chunks: &[codex_apply_patch::UpdateFileChunk]) -> String {
    let mut unified_diff = String::new();
    for chunk in chunks {
        match &chunk.change_context {
            Some(context) => {
                unified_diff.push_str("@@ ");
                unified_diff.push_str(context);
                unified_diff.push('\n');
            }
            None => {
                unified_diff.push_str("@@");
                unified_diff.push('\n');
            }
        }
        for line in &chunk.old_lines {
            unified_diff.push('-');
            unified_diff.push_str(line);
            unified_diff.push('\n');
        }
        for line in &chunk.new_lines {
            unified_diff.push('+');
            unified_diff.push_str(line);
            unified_diff.push('\n');
        }
        if chunk.is_end_of_file {
            unified_diff.push_str("*** End of File");
            unified_diff.push('\n');
        }
    }
    unified_diff
}

fn file_paths_for_action(action: &ApplyPatchAction) -> Vec<PathUri> {
    let mut keys = Vec::new();
    for (path, change) in action.changes() {
        keys.push(path.clone());

        if let ApplyPatchFileChange::Update { move_path, .. } = change
            && let Some(dest) = move_path
        {
            keys.push(dest.clone());
        }
    }

    keys
}

fn write_permissions_for_paths(
    file_paths: &[PathUri],
    matching: &PatchPolicyMatcher<'_>,
) -> io::Result<Option<AdditionalPermissionProfile>> {
    let sandbox_route = matching.sandbox_route;
    let context = &matching.context;
    let mut write_paths = Vec::new();
    for path in file_paths {
        // Skip already-writable targets before deriving parent permissions.
        // Otherwise, a writable directory could grant access to its parent.
        if matching.can_write_path(path)? {
            continue;
        }
        let parent = path
            .parent()
            .or_else(|| match sandbox_route {
                PatchSandboxRoute::Platform(_) => {
                    // Host path rules can recover parents of opaque local paths.
                    // `to_abs_path` verifies that the target round-trips losslessly.
                    path.to_abs_path().ok()?.parent().map(PathUri::from)
                }
                PatchSandboxRoute::ExecutorManaged => None,
            })
            .unwrap_or_else(|| path.clone());
        if !matching.can_write_path(&parent)? {
            write_paths.push(parent);
        }
    }
    write_paths.sort_by_key(PathUri::to_string);
    write_paths.dedup();

    if write_paths.is_empty() {
        return Ok(None);
    }
    let permissions = AdditionalPermissionProfile {
        file_system: Some(FileSystemPermissions::from_read_write_path_uris(
            Some(vec![]),
            Some(write_paths),
        )),
        ..Default::default()
    };

    Ok(match sandbox_route {
        PatchSandboxRoute::Platform(_) => normalize_additional_permissions(permissions).ok(),
        PatchSandboxRoute::ExecutorManaged => {
            normalize_additional_permissions_with_context(permissions, context).ok()
        }
    })
}

/// Extracts the raw patch text used as the command-shaped hook input for apply_patch.
fn apply_patch_payload_command(payload: &ToolPayload) -> Option<String> {
    match payload {
        ToolPayload::Custom { input } => Some(input.clone()),
        _ => None,
    }
}

impl ToolExecutor<ToolInvocation> for ApplyPatchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("apply_patch")
    }

    fn spec(&self) -> ToolSpec {
        create_apply_patch_freeform_tool(self.multi_environment)
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl ApplyPatchHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            step_context,
            cancellation_token,
            tracker,
            call_id,
            tool_name,
            payload,
            ..
        } = invocation;

        let ToolPayload::Custom { input: patch_input } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "apply_patch handler received unsupported payload".to_string(),
            ));
        };
        let args = match codex_apply_patch::parse_patch(&patch_input) {
            Ok(args) => args,
            Err(parse_error) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "apply_patch verification failed: {parse_error}"
                )));
            }
        };
        let selected_environment_id =
            require_environment_id(args.environment_id.as_deref(), self.multi_environment)?;

        // Verify the parsed patch against the selected environment filesystem.
        let Some(turn_environment) = resolve_tool_environment(
            &step_context.environments,
            selected_environment_id.as_deref(),
        )?
        else {
            return Err(FunctionCallError::RespondToModel(
                "apply_patch is unavailable in this session".to_string(),
            ));
        };
        let fs = turn_environment.environment.get_filesystem();
        let sandbox = turn_environment.sandbox_context(/*additional_permissions*/ None);
        match codex_apply_patch::verify_apply_patch_args_with_mode(
            args,
            turn_environment.cwd(),
            apply_patch_file_update_mode(&turn),
            fs.as_ref(),
            Some(&sandbox),
        )
        .await
        {
            codex_apply_patch::MaybeApplyPatchVerified::Body(changes) => {
                let tool_ctx = ToolCtx {
                    session,
                    step_context: Arc::clone(&step_context),
                    cancellation_token,
                    call_id,
                    tool_name,
                };
                let content = execute_verified_patch(
                    changes,
                    turn_environment.clone(),
                    Some(&tracker),
                    tool_ctx,
                )
                .await?;
                Ok(boxed_tool_output(ApplyPatchToolOutput::from_text(content)))
            }
            codex_apply_patch::MaybeApplyPatchVerified::CorrectnessError(parse_error) => {
                Err(FunctionCallError::RespondToModel(format!(
                    "apply_patch verification failed: {parse_error}"
                )))
            }
            codex_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
                tracing::trace!("Failed to parse apply_patch input, {error:?}");
                Err(FunctionCallError::RespondToModel(
                    "apply_patch handler received invalid patch input".to_string(),
                ))
            }
            codex_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => {
                Err(FunctionCallError::RespondToModel(
                    "apply_patch handler received non-apply_patch input".to_string(),
                ))
            }
        }
    }
}

impl CoreToolRuntime for ApplyPatchHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Custom { .. })
    }

    fn create_diff_consumer(&self) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        Some(Box::<ApplyPatchArgumentDiffConsumer>::default())
    }

    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        apply_patch_payload_command(&invocation.payload).map(|command| PreToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_input: serde_json::json!({ "command": command }),
        })
    }

    fn with_updated_hook_input(
        &self,
        mut invocation: ToolInvocation,
        updated_input: serde_json::Value,
    ) -> Result<ToolInvocation, FunctionCallError> {
        let patch = updated_hook_command(&updated_input)?;
        invocation.payload = match invocation.payload {
            ToolPayload::Custom { .. } => ToolPayload::Custom {
                input: patch.to_string(),
            },
            payload => payload,
        };
        Ok(invocation)
    }

    fn post_tool_use_payload(
        &self,
        invocation: &ToolInvocation,
        result: &dyn crate::tools::context::ToolOutput,
    ) -> Option<PostToolUsePayload> {
        let tool_response =
            result.post_tool_use_response(&invocation.call_id, &invocation.payload)?;
        Some(PostToolUsePayload {
            tool_name: HookToolName::apply_patch(),
            tool_use_id: invocation.call_id.clone(),
            tool_input: serde_json::json!({
                "command": apply_patch_payload_command(&invocation.payload)?,
            }),
            tool_response,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn intercept_apply_patch(
    command: &[String],
    cwd: &PathUri,
    fs: &dyn ExecutorFileSystem,
    turn_environment: TurnEnvironment,
    session: Arc<Session>,
    step_context: Arc<StepContext>,
    cancellation_token: CancellationToken,
    tracker: Option<&SharedTurnDiffTracker>,
    call_id: &str,
    tool_name: &str,
) -> Result<Option<FunctionToolOutput>, FunctionCallError> {
    let turn = &step_context.turn;
    let sandbox = turn_environment.sandbox_context(/*additional_permissions*/ None);
    match codex_apply_patch::maybe_parse_apply_patch_verified_with_mode(
        command,
        cwd,
        apply_patch_file_update_mode(turn),
        fs,
        Some(&sandbox),
    )
    .await
    {
        codex_apply_patch::MaybeApplyPatchVerified::Body(changes) => {
            let tool_ctx = ToolCtx {
                session,
                step_context,
                cancellation_token,
                call_id: call_id.to_string(),
                tool_name: ToolName::plain(tool_name),
            };
            let content =
                execute_verified_patch(changes, turn_environment, tracker, tool_ctx).await?;
            Ok(Some(FunctionToolOutput::from_text(content, Some(true))))
        }
        codex_apply_patch::MaybeApplyPatchVerified::CorrectnessError(parse_error) => {
            Err(FunctionCallError::RespondToModel(format!(
                "apply_patch verification failed: {parse_error}"
            )))
        }
        codex_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
            tracing::trace!("Failed to parse apply_patch input, {error:?}");
            Ok(None)
        }
        codex_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => Ok(None),
    }
}

async fn execute_verified_patch(
    mut action: ApplyPatchAction,
    turn_environment: TurnEnvironment,
    tracker: Option<&SharedTurnDiffTracker>,
    tool_ctx: ToolCtx,
) -> Result<String, FunctionCallError> {
    let cwd = action.cwd.clone();
    let verify = action
        .verify_command
        .take()
        .map(|command| (command, turn_environment.clone()));
    let sandbox_context = turn_environment.sandbox_context(/*additional_permissions*/ None);
    let policy_context = file_system_sandbox_policy_context_for_cwd(&sandbox_context, &cwd);
    let sandbox_route = if turn_environment.environment.is_remote() {
        PatchSandboxRoute::ExecutorManaged
    } else {
        PatchSandboxRoute::Platform(windows_sandbox_level_for_legacy_checks(
            turn_environment.config().windows_sandbox_type,
            turn_environment.config().windows_sandbox_level,
        ))
    };
    let environment_id = turn_environment.selection.environment_id.as_str();
    let file_paths = file_paths_for_action(&action);
    let granted_permissions = merge_permission_profiles(
        tool_ctx
            .session
            .granted_session_permissions(environment_id)
            .await
            .as_ref(),
        tool_ctx
            .session
            .granted_turn_permissions(environment_id)
            .await
            .as_ref(),
    );
    let base_file_system_sandbox_policy = turn_environment
        .permission_profile()
        .file_system_sandbox_policy();
    let file_system_sandbox_policy = effective_file_system_sandbox_policy(
        &base_file_system_sandbox_policy,
        granted_permissions.as_ref(),
    );
    let matching = sandbox_route
        .prepare_matching(&file_system_sandbox_policy, &policy_context)
        .map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "failed to prepare patch permissions: {error}"
            ))
        })?;
    let additional_permissions =
        write_permissions_for_paths(&file_paths, &matching).map_err(|error| {
            FunctionCallError::RespondToModel(format!("failed to check patch permissions: {error}"))
        })?;
    let effective_additional_permissions = apply_granted_turn_permissions(
        tool_ctx.session.as_ref(),
        &turn_environment,
        &cwd,
        crate::sandboxing::SandboxPermissions::UseDefault,
        additional_permissions,
    )
    .await;
    let apply = apply_patch::prepare_apply_patch(
        &tool_ctx.step_context,
        &turn_environment,
        &matching,
        action,
    )?;
    let changes = convert_apply_patch_to_protocol(&apply.action);
    let emitter = ToolEmitter::apply_patch_for_environment(
        changes.clone(),
        apply.auto_approved,
        turn_environment.selection.environment_id.clone(),
    );
    let event_ctx = ToolEventCtx::new(
        tool_ctx.session.as_ref(),
        tool_ctx.step_context.turn.as_ref(),
        &tool_ctx.step_context.settings.model_info,
        &tool_ctx.call_id,
        tracker,
    );
    emitter.begin(event_ctx).await;

    let request = ApplyPatchRequest {
        turn_environment,
        action: apply.action,
        file_paths,
        changes: Arc::new(changes),
        exec_approval_requirement: apply.exec_approval_requirement,
        additional_permissions: effective_additional_permissions.additional_permissions,
        permissions_preapproved: effective_additional_permissions.permissions_preapproved,
    };
    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = ApplyPatchRuntime::new();
    let result = orchestrator
        .run(&mut runtime, &request, &tool_ctx)
        .await
        .map(|result| result.output);
    let (result, delta) = match result {
        Ok(output) => (Ok(output.exec_output), Some(output.delta)),
        Err(error) => (Err(error), Some(runtime.committed_delta().clone())),
    };
    let event_ctx = ToolEventCtx::new(
        tool_ctx.session.as_ref(),
        tool_ctx.step_context.turn.as_ref(),
        &tool_ctx.step_context.settings.model_info,
        &tool_ctx.call_id,
        tracker,
    );
    let then_run = match (&result, verify) {
        (Ok(output), Some((command, environment))) if output.exit_code == 0 => {
            Some(run_then_run_command(command, &cwd, &environment, &tool_ctx).await)
        }
        _ => None,
    };
    let mut content = emitter.finish(event_ctx, result, delta.as_ref()).await?;
    if let Some(then_run) = then_run {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(&then_run);
    }
    Ok(content)
}

/// Longest a patch's `*** Then Run:` command may run before it is killed when
/// its line declares no limit.
const THEN_RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest limit a `*** Then Run: (timeout_ms: N) <command>` line may declare.
const THEN_RUN_MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// Splits an optional leading `(timeout_ms: N)` off a `*** Then Run:` command
/// and returns the limit the command runs under with the command itself.
///
/// A declared limit is clamped to between [`THEN_RUN_TIMEOUT`] and
/// [`THEN_RUN_MAX_TIMEOUT`]; a command without one, or with a malformed one,
/// runs as written under [`THEN_RUN_TIMEOUT`].
fn split_then_run_timeout(command: &str) -> (Duration, &str) {
    let declared = command
        .strip_prefix("(timeout_ms:")
        .and_then(|rest| rest.split_once(')'))
        .and_then(|(millis, rest)| {
            let millis = millis.trim().parse::<u64>().ok()?;
            let rest = rest.trim_start();
            (!rest.is_empty()).then_some((millis, rest))
        });
    match declared {
        Some((millis, rest)) => (
            Duration::from_millis(millis).clamp(THEN_RUN_TIMEOUT, THEN_RUN_MAX_TIMEOUT),
            rest,
        ),
        None => (THEN_RUN_TIMEOUT, command),
    }
}

/// Runs a patch's `*** Then Run:` command after the patch applied and returns
/// the section appended to the model's view of the `apply_patch` result.
///
/// The command goes through the lifecycle an `exec_command` call with the same
/// command gets: Bash `PreToolUse` hooks (which may block or rewrite it), the
/// unified exec runtime with its exec policy, approval, sandbox, and command
/// item events, then Bash `PostToolUse` hooks on its result. Its command item
/// uses [`apply_patch_check_item_id`], so clients can tie it to the patch; a
/// blocking `PostToolUse` hook or hook feedback replaces the command's output
/// in the model's view, as it replaces an `exec_command` result.
async fn run_then_run_command(
    command: String,
    cwd: &PathUri,
    turn_environment: &TurnEnvironment,
    tool_ctx: &ToolCtx,
) -> String {
    let (timeout, command) = split_then_run_timeout(&command);
    let command = command.to_string();
    let hook_result = run_pre_tool_use_hooks(
        &tool_ctx.session,
        tool_ctx.step_context.as_ref(),
        tool_ctx.call_id.clone(),
        &HookToolName::bash(),
        &serde_json::json!({ "command": command }),
    )
    .await;
    let command = match hook_result {
        PreToolUseHookResult::Blocked(message) => {
            return format!("Then Run: {command}\nNot run: {message}");
        }
        PreToolUseHookResult::Continue {
            updated_input: Some(updated_input),
        } => match updated_hook_command(&updated_input) {
            Ok(updated) => updated.to_string(),
            Err(err) => return format!("Then Run: {command}\nNot run: {err}"),
        },
        PreToolUseHookResult::Continue {
            updated_input: None,
        } => command,
    };
    let output =
        match exec_then_run_command(&command, timeout, cwd, turn_environment, tool_ctx).await {
            Ok(output) => output,
            Err(UnifiedExecError::SandboxDenied {
                output,
                original_token_count,
                output_omitted_bytes,
                ..
            }) => {
                let text = output.aggregated_output.text;
                ExecCommandToolOutput {
                    event_call_id: apply_patch_check_item_id(&tool_ctx.call_id),
                    chunk_id: String::new(),
                    wall_time: output.duration,
                    original_token_count: Some(
                        original_token_count.unwrap_or_else(|| approx_token_count(&text)),
                    ),
                    raw_output: text.into_bytes(),
                    truncation_policy: tool_ctx
                        .step_context
                        .settings
                        .model_info
                        .truncation_policy
                        .into(),
                    max_output_tokens: None,
                    process_id: None,
                    exit_code: Some(output.exit_code),
                    output_omitted_bytes,
                    hook_command: Some(command.clone()),
                }
            }
            Err(err) => return format!("Then Run: {command}\nFailed to run: {err}"),
        };
    let hook_payload = ToolPayload::Function {
        arguments: serde_json::json!({ "cmd": command }).to_string(),
    };
    let post_tool_use_outcome =
        match bash_post_tool_use_payload(tool_ctx.call_id.clone(), &hook_payload, &output) {
            Some(payload) => Some(
                run_post_tool_use_hooks(
                    &tool_ctx.session,
                    tool_ctx.step_context.as_ref(),
                    payload.tool_use_id,
                    payload.tool_name.name().to_string(),
                    payload.tool_name.matcher_aliases().to_vec(),
                    payload.tool_input,
                    payload.tool_response,
                )
                .await,
            ),
            None => None,
        };
    if let Some(outcome) = &post_tool_use_outcome {
        record_additional_contexts(
            &tool_ctx.session,
            &tool_ctx.step_context.turn,
            outcome.additional_contexts.clone(),
        )
        .await;
    }
    let body = match post_tool_use_outcome {
        Some(outcome) if outcome.should_block => format!(
            "Result blocked by PostToolUse hook: {}",
            outcome
                .feedback_message
                .unwrap_or_else(|| "PostToolUse hook blocked the tool result".to_string())
        ),
        Some(PostToolUseOutcome {
            feedback_message: Some(feedback_message),
            ..
        }) => feedback_message,
        Some(_) | None => {
            let exit_code = output
                .exit_code
                .map_or_else(|| "unknown".to_string(), |code| code.to_string());
            let limit = if output.wall_time >= timeout {
                format!(" (killed at the {} second limit)", timeout.as_secs())
            } else {
                String::new()
            };
            format!(
                "Exit code: {exit_code}{limit}\nOutput:\n{}",
                output.model_visible_output()
            )
        }
    };
    format!("Then Run: {command}\n{body}")
}

/// Runs `command` to completion within `timeout` the way a one-shot
/// `exec_command` call with only `cmd` set runs it: session shell, default
/// sandbox permissions plus any granted to the turn, no TTY.
async fn exec_then_run_command(
    command: &str,
    timeout: Duration,
    cwd: &PathUri,
    turn_environment: &TurnEnvironment,
    tool_ctx: &ToolCtx,
) -> Result<ExecCommandToolOutput, UnifiedExecError> {
    let session = &tool_ctx.session;
    let turn = &tool_ctx.step_context.turn;
    let item_id = apply_patch_check_item_id(&tool_ctx.call_id);
    let args: ExecCommandArgs = parse_arguments(&serde_json::json!({ "cmd": command }).to_string())
        .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
    let shell_mode = shell_mode_for_environment(
        &turn.unified_exec_shell_mode,
        turn_environment.environment.as_ref(),
    );
    let shell = turn_environment
        .shell
        .clone()
        .map(Arc::new)
        .unwrap_or_else(|| session.user_shell());
    let resolved_command = get_command(
        &args,
        shell,
        &shell_mode,
        turn_environment.config().allow_login_shell,
    )
    .map_err(UnifiedExecError::create_process)?;
    let effective_permissions = apply_granted_turn_permissions(
        session.as_ref(),
        turn_environment,
        cwd,
        crate::sandboxing::SandboxPermissions::UseDefault,
        /*additional_permissions*/ None,
    )
    .await;
    let additional_permissions = implicit_granted_permissions(
        crate::sandboxing::SandboxPermissions::UseDefault,
        /*additional_permissions*/ None,
        &effective_permissions,
    );
    let native_cwd = cwd.to_abs_path().ok();
    crate::maybe_emit_implicit_skill_invocation(
        session.as_ref(),
        turn.as_ref(),
        command,
        cwd,
        native_cwd.as_ref(),
        &turn_environment.selection.environment_id,
    )
    .await;
    let file_system = turn_environment.environment.get_filesystem();
    crate::tools::lifecycle::notify_command_start(
        session.as_ref(),
        turn.as_ref(),
        &item_id,
        &resolved_command.command,
        cwd,
        file_system.as_ref(),
    )
    .await;
    let process_id = session
        .services
        .unified_exec_manager
        .allocate_process_id()
        .await;
    let request = ExecCommandRequest {
        command: resolved_command.command,
        shell_type: resolved_command.shell_type,
        hook_command: command.to_string(),
        process_id,
        yield_time_ms: 0,
        max_output_tokens: None,
        cwd: cwd.clone(),
        sandbox_cwd: turn_environment.cwd().clone(),
        turn_environment: turn_environment.clone(),
        shell_mode,
        network: turn.network.clone(),
        tty: false,
        sandbox_permissions: effective_permissions.sandbox_permissions,
        additional_permissions,
        additional_permissions_preapproved: effective_permissions.permissions_preapproved,
        justification: None,
        prefix_rule: None,
    };
    let context = UnifiedExecContext::new(
        Arc::clone(session),
        Arc::clone(&tool_ctx.step_context),
        tool_ctx.cancellation_token.clone(),
        item_id,
    );
    UnifiedExecProcessManager::exec_command_to_completion(request, &context, timeout).await
}

fn require_environment_id(
    parsed_environment_id: Option<&str>,
    allow_environment_id: bool,
) -> Result<Option<String>, FunctionCallError> {
    match parsed_environment_id {
        Some(_) if !allow_environment_id => Err(FunctionCallError::RespondToModel(
            "apply_patch environment selection is unavailable for this turn".to_string(),
        )),
        Some(environment_id) => Ok(Some(environment_id.to_string())),
        None => Ok(None),
    }
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;

use codex_tools::FreeformTool;
use codex_tools::FreeformToolFormat;
use codex_tools::ToolSpec;

const APPLY_PATCH_LARK_GRAMMAR: &str = include_str!("../../../assets/tools/apply_patch.lark");

const APPLY_PATCH_DESCRIPTION: &str = "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON. To check the change in the same call, put one `*** Then Run: <single-line shell command>` line right before `*** End Patch`: after every file is written, the command runs in the patch's working directory the way `exec_command` would run it (same sandbox and approvals, at most 30 seconds unless the line declares more as `*** Then Run: (timeout_ms: <N>) <command>`, up to 600000, so a full test suite or a slow script belongs here with the time it needs), and its exit code and output are appended to this tool's result. A check that needs more than one line is a script file the same patch adds, under /tmp when it is not part of the change, which the `*** Then Run:` command runs. A patch that fails to apply runs nothing.";

/// Returns a custom tool that can be used to edit files. Well-suited for GPT-5 models
/// https://platform.openai.com/docs/guides/function-calling#custom-tools
pub fn create_apply_patch_freeform_tool(include_environment_id: bool) -> ToolSpec {
    let definition = if include_environment_id {
        APPLY_PATCH_LARK_GRAMMAR.replace(
            "start: begin_patch hunk+ then_run? end_patch",
            "start: begin_patch environment_id? hunk+ then_run? end_patch\nenvironment_id: \"*** Environment ID: \" filename LF",
        )
    } else {
        APPLY_PATCH_LARK_GRAMMAR.to_string()
    };
    ToolSpec::Freeform(FreeformTool {
        name: "apply_patch".to_string(),
        description: APPLY_PATCH_DESCRIPTION.to_string(),
        defer_loading: None,
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition,
        },
    })
}

#[cfg(test)]
#[path = "apply_patch_spec_tests.rs"]
mod tests;

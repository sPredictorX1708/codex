//! Tells a root agent whose current turn comes from `codex exec` how an unattended run ends.
//! The state follows the app-server client that started the turn, not the thread's saved origin,
//! so a thread resumed from another frontend gains or retires the guidance on its next turn.
//! Forked subagents drop the inherited fragment and snapshot, because their work is reviewed by
//! the parent agent rather than by whoever launched the run.

use super::CODEX_EXEC_CLIENT_NAME;
use super::PreviousSectionState;
use super::WorldStateSection;
use crate::context::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

const UNATTENDED_RUN_INSTRUCTIONS: &str = "This turn runs unattended through `codex exec`. Whoever launched the run reviews the resulting workspace changes with git after it ends. One `apply_patch` call can add or update several files: put every file that one change touches, including its tests and docs, into a single call (each file once, with all of its hunks under one header), then run the checks once. Once the checks for your change pass, finish with your final answer: do not spend further tool calls re-inspecting your own edits with `git diff` or `git status`, and leave untracked artifacts that running tests or builds created, such as bytecode caches, in place.";
const REMOVAL_NOTICE: &str = "The previously provided unattended-run instructions no longer apply.";

/// Whether the client that started the current turn is `codex exec`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnattendedRunState {
    unattended: bool,
}

impl UnattendedRunState {
    pub(crate) fn new(app_server_client_name: Option<&str>) -> Self {
        Self {
            unattended: app_server_client_name == Some(CODEX_EXEC_CLIENT_NAME),
        }
    }
}

/// The developer fragment carrying the unattended-run guidance or its removal notice.
pub(crate) struct UnattendedRunInstructions {
    text: &'static str,
}

impl ContextualUserFragment for UnattendedRunInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("unattended_run.instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<unattended_run>", "</unattended_run>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.text)
    }
}

impl WorldStateSection for UnattendedRunState {
    const ID: &'static str = "unattended_run";
    type Snapshot = bool;

    fn snapshot(&self) -> Self::Snapshot {
        self.unattended
    }

    fn matches_legacy_fragment(role: &str, text: &str) -> bool {
        role == "developer" && UnattendedRunInstructions::matches_text(text)
    }

    fn has_retained_fragment_matcher() -> bool {
        true
    }

    fn matches_retained_fragment(role: &str, text: &str) -> bool {
        Self::matches_legacy_fragment(role, text)
    }

    fn render_diff(
        &self,
        previous: PreviousSectionState<'_, Self::Snapshot>,
    ) -> Option<Box<dyn ContextualUserFragment>> {
        let text = match (self.unattended, previous) {
            (true, PreviousSectionState::Known(true))
            | (false, PreviousSectionState::Absent | PreviousSectionState::Known(false)) => {
                return None;
            }
            (true, _) => UNATTENDED_RUN_INSTRUCTIONS,
            (false, _) => REMOVAL_NOTICE,
        };
        Some(Box::new(UnattendedRunInstructions { text }))
    }
}

#[cfg(test)]
#[path = "unattended_run_tests.rs"]
mod tests;

//! Tells a root agent whose current turn comes from `codex exec` that nobody reads commentary.
//! The model instructions ask for an opening commentary message and frequent progress updates;
//! under `codex exec` those messages have no reader and only lengthen the turn.
//!
//! The state follows the app-server client that started the turn, not the thread's saved
//! `SessionSource`, because resuming a thread keeps its original source. A thread started by
//! `codex exec` and resumed from an interactive frontend retires the guidance on its next turn,
//! and a later `codex exec` resume brings it back. Forked subagents drop the inherited fragment
//! and snapshot, because they never render this section and so could not retire it.

use super::PreviousSectionState;
use super::WorldStateSection;
use crate::context::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

/// App-server client name that `codex exec` registers for its in-process server.
pub const CODEX_EXEC_CLIENT_NAME: &str = "codex_exec";

const HEADLESS_SESSION_INSTRUCTIONS: &str = "This session runs unattended through `codex exec`. Nobody reads the `commentary` channel while you work, and no user message or answer can arrive before the turn ends. This replaces the intermediate commentary guidance: do not send an opening commentary message or progress updates. Call tools directly, and put everything the user needs in the final answer.";
const REMOVAL_NOTICE: &str =
    "The previously provided headless-session instructions no longer apply.";

/// Whether the client that started the current turn is `codex exec`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeadlessSessionState {
    headless: bool,
}

impl HeadlessSessionState {
    pub(crate) fn new(app_server_client_name: Option<&str>) -> Self {
        Self {
            headless: app_server_client_name == Some(CODEX_EXEC_CLIENT_NAME),
        }
    }
}

/// The developer fragment carrying the headless-session guidance or its removal notice.
pub(crate) struct HeadlessSessionInstructions {
    text: &'static str,
}

impl ContextualUserFragment for HeadlessSessionInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("headless_session.instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<headless_session>", "</headless_session>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.text)
    }
}

impl WorldStateSection for HeadlessSessionState {
    const ID: &'static str = "headless_session";
    type Snapshot = bool;

    fn snapshot(&self) -> Self::Snapshot {
        self.headless
    }

    fn matches_legacy_fragment(role: &str, text: &str) -> bool {
        role == "developer" && HeadlessSessionInstructions::matches_text(text)
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
        let text = match (self.headless, previous) {
            (true, PreviousSectionState::Known(true))
            | (false, PreviousSectionState::Absent | PreviousSectionState::Known(false)) => {
                return None;
            }
            (true, _) => HEADLESS_SESSION_INSTRUCTIONS,
            (false, _) => REMOVAL_NOTICE,
        };
        Some(Box::new(HeadlessSessionInstructions { text }))
    }
}

#[cfg(test)]
#[path = "headless_session_tests.rs"]
mod tests;

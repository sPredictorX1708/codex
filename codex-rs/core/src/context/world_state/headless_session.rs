//! Tells a `codex exec` root agent that nobody reads commentary while it works.
//! The model instructions ask for an opening commentary message and frequent progress updates;
//! under `codex exec` those messages have no reader and only lengthen the turn.

use super::PreviousSectionState;
use super::WorldStateSection;
use crate::context::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

const HEADLESS_SESSION_INSTRUCTIONS: &str = "This session runs unattended through `codex exec`. Nobody reads the `commentary` channel while you work, and no user message or answer can arrive before the turn ends. This replaces the intermediate commentary guidance: do not send an opening commentary message or progress updates. Call tools directly, and put everything the user needs in the final answer.";
const REMOVAL_NOTICE: &str =
    "The previously provided headless-session instructions no longer apply.";

/// Whether the session runs without an interactive reader of intermediate commentary.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeadlessSessionState {
    headless: bool,
}

impl HeadlessSessionState {
    pub(crate) fn new(headless: bool) -> Self {
        Self { headless }
    }
}

struct HeadlessSessionInstructions {
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

    fn should_persist(&self) -> bool {
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
        let previously_headless = match previous {
            PreviousSectionState::Absent => false,
            PreviousSectionState::Unknown => return None,
            PreviousSectionState::Known(previous) => *previous,
        };
        let text = match (self.headless, previously_headless) {
            (true, false) => HEADLESS_SESSION_INSTRUCTIONS,
            (false, true) => REMOVAL_NOTICE,
            (true, true) | (false, false) => return None,
        };
        Some(Box::new(HeadlessSessionInstructions { text }))
    }
}

#[cfg(test)]
#[path = "headless_session_tests.rs"]
mod tests;

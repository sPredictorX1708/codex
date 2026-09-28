use super::*;
use crate::context::world_state::WorldState;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

fn instructions(text: &'static str) -> ResponseItem {
    ContextualUserFragment::into(HeadlessSessionInstructions { text })
}

#[test]
fn headless_instructions_are_sent_once_and_retired_when_the_session_turns_interactive() {
    let mut history = Vec::new();
    let mut previous = None;

    for (headless, expected) in [
        (false, None),
        (true, Some(HEADLESS_SESSION_INSTRUCTIONS)),
        (true, None),
        (false, Some(REMOVAL_NOTICE)),
        (false, None),
    ] {
        let mut world_state = WorldState::default();
        world_state.add_section(HeadlessSessionState::new(headless));
        let updates = world_state
            .render_history_diff(previous.as_ref(), &history)
            .into_iter()
            .map(ContextualUserFragment::into_boxed_response_item)
            .collect::<Vec<_>>();
        assert_eq!(
            updates,
            expected.map(instructions).into_iter().collect::<Vec<_>>()
        );
        history.extend(updates);
        previous = Some(world_state.snapshot());
    }
}

#[test]
fn headless_instructions_are_restored_when_compaction_drops_them() {
    let mut world_state = WorldState::default();
    world_state.add_section(HeadlessSessionState::new(/*headless*/ true));
    let snapshot = world_state.snapshot();

    assert_eq!(
        world_state
            .render_history_diff(Some(&snapshot), &[])
            .into_iter()
            .map(ContextualUserFragment::into_boxed_response_item)
            .collect::<Vec<_>>(),
        vec![instructions(HEADLESS_SESSION_INSTRUCTIONS)]
    );
    assert!(
        world_state
            .render_history_diff(
                Some(&snapshot),
                &[instructions(HEADLESS_SESSION_INSTRUCTIONS)]
            )
            .is_empty()
    );
}

#[test]
fn interactive_sessions_persist_no_headless_state() {
    let mut world_state = WorldState::default();
    world_state.add_section(HeadlessSessionState::new(/*headless*/ false));

    assert_eq!(world_state.snapshot(), WorldState::default().snapshot());
}

#[test]
fn snapshots() {
    let mut world_state = WorldState::default();
    world_state.add_section(HeadlessSessionState::new(/*headless*/ true));
    let fresh = world_state.render_full();
    let mut interactive = WorldState::default();
    interactive.add_section(HeadlessSessionState::new(/*headless*/ false));
    let retired = interactive.render_history_diff(
        Some(&world_state.snapshot()),
        &[instructions(HEADLESS_SESSION_INSTRUCTIONS)],
    );

    insta::assert_snapshot!(
        fresh
            .iter()
            .chain(&retired)
            .map(|fragment| format!("(role - {})\n{}", fragment.role(), fragment.render()))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

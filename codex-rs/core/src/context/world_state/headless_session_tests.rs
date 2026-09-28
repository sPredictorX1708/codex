use super::*;
use crate::context::ContextualUserFragment;
use crate::context::world_state::PreviousSectionState;
use crate::context::world_state::WorldState;
use crate::context::world_state::test_support::render_section_cases;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

fn fragment(text: &'static str) -> ResponseItem {
    ContextualUserFragment::into(HeadlessSessionInstructions { text })
}

#[test]
fn snapshots() {
    use PreviousSectionState::Absent;
    use PreviousSectionState::Known;
    use PreviousSectionState::Unknown;

    let interactive = HeadlessSessionState::new(Some("codex-tui"));
    let headless = HeadlessSessionState::new(Some(CODEX_EXEC_CLIENT_NAME));

    insta::assert_snapshot!(render_section_cases(&[
        (Absent, Known(&interactive)),
        (Absent, Known(&headless)),
        (Known(&interactive), Known(&interactive)),
        (Known(&interactive), Known(&headless)),
        (Known(&headless), Known(&headless)),
        (Known(&headless), Known(&interactive)),
        (Unknown, Known(&interactive)),
        (Unknown, Known(&headless)),
    ]));
}

#[test]
fn only_the_exec_client_is_headless() {
    assert_eq!(
        [
            HeadlessSessionState::new(Some(CODEX_EXEC_CLIENT_NAME)).snapshot(),
            HeadlessSessionState::new(Some("codex-tui")).snapshot(),
            HeadlessSessionState::new(Some("vscode")).snapshot(),
            HeadlessSessionState::new(/*app_server_client_name*/ None).snapshot(),
        ],
        [true, false, false, false]
    );
}

#[test]
fn guidance_is_sent_once_retired_and_restored_as_the_client_changes() {
    let mut history = Vec::new();
    let mut previous = None;

    for (client, expected) in [
        (
            Some(CODEX_EXEC_CLIENT_NAME),
            Some(HEADLESS_SESSION_INSTRUCTIONS),
        ),
        (Some(CODEX_EXEC_CLIENT_NAME), None),
        (Some("codex-tui"), Some(REMOVAL_NOTICE)),
        (Some("codex-tui"), None),
        (
            Some(CODEX_EXEC_CLIENT_NAME),
            Some(HEADLESS_SESSION_INSTRUCTIONS),
        ),
    ] {
        let mut world_state = WorldState::default();
        world_state.add_section(HeadlessSessionState::new(client));
        let updates = world_state
            .render_history_diff(previous.as_ref(), &history)
            .into_iter()
            .map(ContextualUserFragment::into_boxed_response_item)
            .collect::<Vec<_>>();
        assert_eq!(
            updates,
            expected.map(fragment).into_iter().collect::<Vec<_>>(),
            "client {client:?}"
        );
        history.extend(updates);
        previous = Some(world_state.snapshot());
    }
}

#[test]
fn persisted_guidance_is_restored_only_when_missing_from_history() {
    let mut world_state = WorldState::default();
    world_state.add_section(HeadlessSessionState::new(Some(CODEX_EXEC_CLIENT_NAME)));
    let snapshot = world_state.snapshot();

    assert_eq!(
        world_state
            .render_history_diff(Some(&snapshot), &[])
            .into_iter()
            .map(ContextualUserFragment::into_boxed_response_item)
            .collect::<Vec<_>>(),
        vec![fragment(HEADLESS_SESSION_INSTRUCTIONS)]
    );
    assert!(
        world_state
            .render_history_diff(Some(&snapshot), &[fragment(HEADLESS_SESSION_INSTRUCTIONS)])
            .is_empty()
    );
}

#[test]
fn interactive_turns_after_compaction_send_no_removal_notice() {
    let mut headless = WorldState::default();
    headless.add_section(HeadlessSessionState::new(Some(CODEX_EXEC_CLIENT_NAME)));
    let mut interactive = WorldState::default();
    interactive.add_section(HeadlessSessionState::new(Some("codex-tui")));

    assert!(
        interactive
            .render_history_diff(Some(&headless.snapshot()), &[])
            .is_empty()
    );
}

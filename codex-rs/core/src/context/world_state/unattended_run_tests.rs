use super::*;
use crate::context::ContextualUserFragment;
use crate::context::world_state::PreviousSectionState;
use crate::context::world_state::test_support::render_section_cases;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

#[test]
fn snapshots() {
    use PreviousSectionState::Absent;
    use PreviousSectionState::Known;
    use PreviousSectionState::Unknown;

    let attended = UnattendedRunState::new(Some("codex-tui"));
    let unattended = UnattendedRunState::new(Some(CODEX_EXEC_CLIENT_NAME));

    insta::assert_snapshot!(render_section_cases(&[
        (Absent, Known(&attended)),
        (Absent, Known(&unattended)),
        (Known(&attended), Known(&attended)),
        (Known(&attended), Known(&unattended)),
        (Known(&unattended), Known(&unattended)),
        (Known(&unattended), Known(&attended)),
        (Unknown, Known(&attended)),
        (Unknown, Known(&unattended)),
    ]));
}

#[test]
fn only_the_exec_client_is_unattended() {
    assert_eq!(
        [
            UnattendedRunState::new(Some(CODEX_EXEC_CLIENT_NAME)).snapshot(),
            UnattendedRunState::new(Some("codex-tui")).snapshot(),
            UnattendedRunState::new(Some("vscode")).snapshot(),
            UnattendedRunState::new(/*app_server_client_name*/ None).snapshot(),
        ],
        [true, false, false, false]
    );
}

#[test]
fn persisted_guidance_is_restored_only_when_missing_from_history() {
    let mut world_state = super::super::WorldState::default();
    world_state.add_section(UnattendedRunState::new(Some(CODEX_EXEC_CLIENT_NAME)));
    let snapshot = world_state.snapshot();
    let retained: ResponseItem = ContextualUserFragment::into(UnattendedRunInstructions {
        text: UNATTENDED_RUN_INSTRUCTIONS,
    });

    assert_eq!(
        world_state.render_history_diff(Some(&snapshot), &[]).len(),
        1
    );
    assert!(
        world_state
            .render_history_diff(Some(&snapshot), &[retained])
            .is_empty()
    );
}

#[test]
fn attended_state_is_persisted_so_a_later_exec_turn_reactivates_guidance() {
    let mut attended = super::super::WorldState::default();
    attended.add_section(UnattendedRunState::new(Some("codex-tui")));
    let removal: ResponseItem = ContextualUserFragment::into(UnattendedRunInstructions {
        text: REMOVAL_NOTICE,
    });
    let attended_snapshot = attended.snapshot();
    let mut unattended = super::super::WorldState::default();
    unattended.add_section(UnattendedRunState::new(Some(CODEX_EXEC_CLIENT_NAME)));

    assert!(
        attended
            .render_history_diff(Some(&attended_snapshot), std::slice::from_ref(&removal))
            .is_empty()
    );
    let rendered = unattended
        .render_history_diff(Some(&attended_snapshot), &[removal])
        .into_iter()
        .map(|fragment| fragment.render())
        .collect::<Vec<_>>();
    assert_eq!(
        rendered,
        vec![format!(
            "<unattended_run>\n{UNATTENDED_RUN_INSTRUCTIONS}\n</unattended_run>"
        )]
    );
}

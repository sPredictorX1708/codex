use super::*;
use pretty_assertions::assert_eq;

#[test_case::test_case(false; "denied")]
#[test_case::test_case(true; "interrupted")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_approval_never_runs_unapproved_command(interrupt: bool) -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_sandbox!(Ok(()));
    skip_if_target_windows!(Ok(()), "uses POSIX commands");
    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config.approvals_reviewer = codex_config::types::ApprovalsReviewer::User;
    });
    let test = builder.build_with_auto_env(&server).await?;
    let marker = test.config.cwd.join("unapproved-completion");
    let args = json!({
        "cmd": "printf unexpected > unapproved-completion",
        "timeout_ms": 2000,
        "sandbox_permissions": "require_escalated",
        "justification": "Exercise completion approval.",
    });
    mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-approval"),
                ev_function_call("completion-approval", "exec_command", &args.to_string()),
                ev_completed("resp-approval"),
            ]),
            sse(vec![
                ev_assistant_message("msg-denied", "denied"),
                ev_completed("resp-denied"),
            ]),
        ]
        .into_iter()
        .take(if interrupt { 1 } else { 2 })
        .collect(),
    )
    .await;
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "request approval".into(),
                text_elements: vec![],
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                approval_policy: Some(AskForApproval::OnRequest),
                sandbox_policy: Some(SandboxPolicy::ReadOnly {
                    network_access: false,
                }),
                ..Default::default()
            }),
        )
        .await?;
    let approval = wait_for_event_match(&test.codex, |event| match event {
        EventMsg::ExecApprovalRequest(approval) => Some(approval.clone()),
        _ => None,
    })
    .await;
    if interrupt {
        test.codex.submit(Op::Interrupt).await?;
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnAborted(_))
        })
        .await;
        test.codex
            .submit(Op::ExecApproval {
                id: approval.effective_approval_id(),
                turn_id: Some(approval.turn_id),
                decision: ReviewDecision::Approved,
            })
            .await?;
    } else {
        test.codex
            .submit(Op::ExecApproval {
                id: approval.effective_approval_id(),
                turn_id: Some(approval.turn_id),
                decision: ReviewDecision::Denied {
                    rejection: "Denied by the completion fixture.".into(),
                },
            })
            .await?;
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!marker.as_path().exists());
    Ok(())
}

#[test_case::test_case(false, 0; "success")]
#[test_case::test_case(false, 7; "nonzero_exit")]
#[test_case::test_case(true, 0; "tty_rejected")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_completion_returns_terminal_result(tty: bool, exit_code: i32) -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_sandbox!(Ok(()));
    skip_if_target_windows!(Ok(()), "uses POSIX commands");
    let server = start_mock_server().await;
    let mut builder = test_codex();
    let test = builder.build_with_auto_env(&server).await?;
    let marker = test.config.cwd.join("completion-marker");
    let call_id = "explicit-completion";
    let args = json!({
        "cmd": format!("printf started > completion-marker; sleep 0.4; printf completed; exit {exit_code}"),
        "tty": tty,
        "yield_time_ms": 250,
        "timeout_ms": 2000,
    });
    let request_log = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(call_id, "exec_command", &serde_json::to_string(&args)?),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_assistant_message("msg-1", "done"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    submit_unified_exec_turn(&test, "run once", PermissionProfile::Disabled).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let requests = request_log.requests();
    assert_eq!(requests.len(), 2);
    let body = requests[1].body_json();
    let output = body["input"]
        .as_array()
        .expect("request input array")
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .expect("completion tool output");
    let text = extract_output_text(output).expect("completion output text");
    if tty {
        assert!(text.contains("timeout_ms requires tty=false"));
        assert!(!marker.as_path().exists());
    } else {
        let parsed = parse_unified_exec_output(text)?;
        assert_eq!(
            (parsed.exit_code, parsed.process_id, parsed.output.trim()),
            (Some(exit_code), None, "completed")
        );
    }
    Ok(())
}

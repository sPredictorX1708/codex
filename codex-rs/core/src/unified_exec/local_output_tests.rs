use super::process::NoopSpawnLifecycle;
use super::process::UnifiedExecProcess;
use codex_sandboxing::SandboxType;
use pretty_assertions::assert_eq;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

#[tokio::test]
#[allow(
    clippy::await_holding_invalid_type,
    reason = "The regression requires stalling the collector while its input queues fill."
)]
async fn local_output_survives_a_stalled_collector() -> anyhow::Result<()> {
    let (writer_tx, _writer_rx) = mpsc::channel(1);
    let (_driver_tx, driver_rx) = broadcast::channel(1);
    let (exit_tx, exit_rx) = oneshot::channel();
    let (stdout_tx, stdout_rx) = mpsc::channel(1024);
    let (stderr_tx, stderr_rx) = mpsc::channel(1024);
    let mut spawned = codex_utils_pty::spawn_from_driver(codex_utils_pty::ProcessDriver {
        writer_tx,
        stdout_rx: driver_rx,
        stderr_rx: None,
        exit_rx,
        terminator: None,
        writer_handle: None,
        resizer: None,
        #[cfg(windows)]
        tty: false,
    });
    spawned.stdout_rx = stdout_rx;
    spawned.stderr_rx = stderr_rx;
    let process =
        UnifiedExecProcess::from_spawned(spawned, SandboxType::None, Box::new(NoopSpawnLifecycle))
            .await?;
    let handles = process.output_handles();
    let guard = handles.output_buffer.lock().await;
    let mut expected = Vec::new();
    for i in 0..768 {
        for (stream, tx) in [("out", &stdout_tx), ("err", &stderr_tx)] {
            let line = format!("{stream}-{i:04}-π\n");
            tx.try_send(line.as_bytes().to_vec())?;
            expected.push(line.trim_end().to_owned());
        }
    }
    drop((stdout_tx, stderr_tx));
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    drop(guard);
    timeout(Duration::from_secs(2), async {
        while !handles
            .output_closed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let mut guard = handles.output_buffer.lock().await;
    let pending = std::mem::take(&mut guard.pending).to_bytes();
    let retained = guard.transcript.to_bytes();
    assert_eq!(retained, pending);
    let text = String::from_utf8(retained)?;
    let mut lines: Vec<_> = text.lines().map(str::to_owned).collect();
    expected.sort();
    lines.sort();
    assert_eq!(lines, expected);
    exit_tx.send(0).expect("exit receiver");
    Ok(())
}

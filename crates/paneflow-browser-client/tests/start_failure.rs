#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use paneflow_browser_client::{BrowserClient, ClientEvent, HostConfig, TabStatus};
use paneflow_browser_proto::{CommandError, ProfileName};

const DEADLINE: Duration = Duration::from_secs(60);

fn failing_host(dir: &std::path::Path, release: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("failing-host.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nwhile [ ! -e '{}' ]; do sleep 0.05; done\necho 'the sandbox refused to start' >&2\nexit 7\n",
            release.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[test]
fn a_host_that_fails_to_start_never_blocks_the_caller_and_the_tab_shows_why() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let mut config = HostConfig::for_home(dir.path().join("home"));
    config.binary = failing_host(dir.path(), &release);
    let (client, events) = BrowserClient::new(config);

    let (tab, ticket) = client.open_tab(ProfileName::new("w").unwrap(), "http://127.0.0.1:1/");
    assert!(ticket.try_take().is_none());
    assert_eq!(client.tab_status(tab), Some(TabStatus::Starting));
    assert!(matches!(
        events.recv_timeout(Duration::from_millis(0)),
        Err(RecvTimeoutError::Timeout)
    ));

    std::fs::write(&release, b"go").unwrap();
    let reason = match events.recv_timeout(DEADLINE).unwrap() {
        ClientEvent::HostFailed { reason } => reason,
        other => panic!("expected a start failure, got {other:?}"),
    };
    assert!(reason.contains("the sandbox refused to start"), "{reason}");
    assert!(reason.contains("exit status: 7"), "{reason}");
    assert_eq!(
        ticket.wait(DEADLINE),
        Err(CommandError::HostUnavailable {
            reason: reason.clone()
        })
    );
    assert_eq!(client.tab_status(tab), Some(TabStatus::Failed { reason }));
    assert_eq!(client.host_pid(), None);
}

#[test]
fn a_missing_host_binary_reports_that_the_engine_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = HostConfig::for_home(dir.path().join("home"));
    config.binary = dir.path().join("absent").join("paneflow-browser-host");
    let (client, events) = BrowserClient::new(config);
    let (tab, ticket) = client.open_tab(ProfileName::new("w").unwrap(), "http://127.0.0.1:1/");
    let Err(CommandError::HostUnavailable { reason }) = ticket.wait(DEADLINE) else {
        panic!("expected the engine to be unavailable");
    };
    assert!(reason.contains("not installed"), "{reason}");
    assert!(matches!(
        events.recv_timeout(DEADLINE).unwrap(),
        ClientEvent::HostFailed { .. }
    ));
    assert!(matches!(
        client.tab_status(tab),
        Some(TabStatus::Failed { .. })
    ));
}

#[test]
fn creating_a_client_starts_no_host() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let mut config = HostConfig::for_home(dir.path().join("home"));
    config.binary = failing_host(dir.path(), &release);
    let (client, events) = BrowserClient::new(config);
    assert_eq!(client.host_pid(), None);
    assert!(client.tabs().is_empty());
    assert!(client.shutdown().try_take().is_some());
    assert!(matches!(
        events.recv_timeout(Duration::from_millis(200)),
        Err(RecvTimeoutError::Timeout)
    ));
}

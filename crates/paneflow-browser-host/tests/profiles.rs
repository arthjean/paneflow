#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{DEADLINE, FixtureServer, Harness, config};
use paneflow_browser_client::{ClientEvent, TabStatus};
use paneflow_browser_proto::{CommandError, Reply};
use serde_json::json;

fn cookies(harness: &Harness, tab: paneflow_browser_proto::TabId) -> String {
    harness
        .evaluate(tab, "document.cookie")
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn two_workspaces_never_read_each_others_cookies() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let first = harness.open("workspace-one", &server.url("/test/cookie?account=one"));
    let second = harness.open("workspace-two", &server.url("/test/cookie?account=two"));
    harness.wait_loaded(first, "/test/cookie");
    harness.wait_loaded(second, "/test/cookie");
    assert_eq!(cookies(&harness, first), "account=one");
    assert_eq!(cookies(&harness, second), "account=two");

    let third = harness.open("workspace-three", &server.url("/test/cookie"));
    harness.wait_loaded(third, "/test/cookie");
    assert_eq!(cookies(&harness, third), "");
    harness.shutdown();
}

#[test]
fn a_profile_lives_under_the_paneflow_home_and_survives_a_restart() {
    let server = FixtureServer::start();
    let home = tempfile::tempdir().unwrap();
    {
        let mut harness = Harness::sharing(config(home.path()));
        let tab = harness.open("kept", &server.url("/test/cookie?session=alive"));
        harness.wait_loaded(tab, "/test/cookie");
        assert_eq!(cookies(&harness, tab), "session=alive");
        assert_eq!(
            harness.client.close_tab(tab).wait(DEADLINE),
            Ok(Reply::Done)
        );
        harness.wait_for("the tab to close", |event| {
            matches!(
                event,
                ClientEvent::Browser(paneflow_browser_proto::Event::Closed { .. })
            )
        });
        harness.shutdown();
    }

    let profiles = paneflow_home::browser_profiles_dir_in(home.path());
    let profile = profiles.join("kept");
    assert!(profile.is_dir());
    assert!(profile.starts_with(home.path()));
    assert_eq!(
        std::fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for foreign in [dirs::config_dir(), dirs::cache_dir()]
        .into_iter()
        .flatten()
    {
        assert!(!profile.starts_with(&foreign), "{}", foreign.display());
    }

    let mut harness = Harness::sharing(config(home.path()));
    let tab = harness.open("kept", &server.url("/test/cookie"));
    harness.wait_loaded(tab, "/test/cookie");
    assert_eq!(cookies(&harness, tab), "session=alive");
    harness.shutdown();
}

#[test]
fn a_second_host_on_a_locked_profile_directory_is_refused_naming_the_lock() {
    let server = FixtureServer::start();
    let home = tempfile::tempdir().unwrap();
    let mut first = Harness::sharing(config(home.path()));
    let tab = first.open("shared", &server.url("/form.html"));
    first.wait_loaded(tab, "/form.html");

    let mut second = Harness::sharing(config(home.path()));
    let (refused, ticket) = second
        .client
        .open_tab(Harness::profile("shared"), server.url("/form.html"));
    let error = ticket.wait(DEADLINE).unwrap_err();
    let lock = paneflow_home::browser_profiles_dir_in(home.path()).join("paneflow-host.lock");
    let CommandError::HostUnavailable { reason } = error else {
        panic!("expected the engine to be unavailable, got {error:?}");
    };
    assert!(
        reason.contains("locked by another browser host"),
        "{reason}"
    );
    assert!(reason.contains(&lock.display().to_string()), "{reason}");
    assert!(matches!(
        second.client.tab_status(refused),
        Some(TabStatus::Failed { reason: shown }) if shown == reason
    ));
    second.wait_for("the start failure", |event| {
        matches!(event, ClientEvent::HostFailed { .. })
    });

    assert_eq!(first.evaluate(tab, "1"), json!(1));
    first.shutdown();
}

const INSTALLED_BROWSER_PATHS: &[&str] = &[
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/microsoft-edge",
    ".mozilla",
];

const SYSTEM_BROWSER_PATHS: &[&str] = &[
    "/usr/share/chromium",
    "/usr/share/google-chrome",
    "/opt/google/chrome",
    "/etc/opt/chrome",
    "/etc/chromium",
];

fn plant_installed_browser(user_home: &Path) {
    for relative in INSTALLED_BROWSER_PATHS {
        let profile = user_home.join(relative).join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("Cookies"), b"planted").unwrap();
    }
}

#[test]
fn a_fresh_profile_is_empty_and_reads_no_installed_browser() {
    let server = FixtureServer::start();
    let home = tempfile::tempdir().unwrap();
    let mut config = config(home.path());
    let user_home = home.path().join("user");
    plant_installed_browser(&user_home);

    let trace = home.path().join("trace.log");
    let wrapper = home.path().join("traced-host.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec strace -D -f -qq -e trace=%file -o '{}' '{}' \"$@\"\n",
            trace.display(),
            common::host_binary().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    config.binary = wrapper;

    let mut harness = Harness::sharing(config);
    let tab = harness.open("fresh", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");
    let reply = harness
        .client
        .devtools(
            tab,
            "Network.getCookies",
            json!({"urls": [server.url("/")]}),
        )
        .wait(DEADLINE);
    let Ok(Reply::DevTools(result)) = reply else {
        panic!("{reply:?}");
    };
    assert_eq!(result["cookies"], json!([]));
    assert_eq!(cookies(&harness, tab), "");
    harness.shutdown();

    let log = std::fs::read_to_string(&trace).unwrap();
    assert!(
        log.contains("paneflow-browser-host"),
        "the trace recorded nothing"
    );
    let profile_dir = paneflow_home::browser_profiles_dir_in(home.path()).join("fresh");
    assert!(
        log.contains(&format!("\"{}", profile_dir.display())),
        "the trace does not show the engine creating the profile"
    );
    assert!(
        log.contains("--type=renderer") || log.contains("icudtl.dat"),
        "the trace missed the engine's own reads"
    );
    let mut forbidden: Vec<String> = INSTALLED_BROWSER_PATHS
        .iter()
        .map(|relative| user_home.join(relative).display().to_string())
        .collect();
    forbidden.extend(SYSTEM_BROWSER_PATHS.iter().map(|path| path.to_string()));
    for line in log.lines() {
        for path in &forbidden {
            assert!(
                !line.contains(&format!("\"{path}")),
                "the host touched {path}: {line}"
            );
        }
    }
}

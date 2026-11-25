#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{FixtureServer, Harness, config, is_alive, process_tree};
use serde_json::json;

use paneflow_browser_client::{ClientEvent, HOST_STOPPED_MESSAGE, TabStatus};
use paneflow_browser_proto::{
    ConsoleLevel, ENDPOINT_ARG, Event, ExitReason, Hello, HostMessage, PROFILES_ARG,
    PROTOCOL_VERSION, READY_LINE, Refusal, Reply, WindowMessage, read_message, write_message,
};

fn hosts_carrying(marker: &str) -> Vec<u32> {
    let binary = common::host_binary();
    let entry = format!("PANEFLOW_TEST_MARKER={marker}");
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|dir| dir.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == binary))
        .filter(|pid| common::environment(*pid).contains(&entry))
        .filter(|pid| !common::command_line(*pid).contains("--type="))
        .collect()
}

#[test]
fn the_host_loads_a_fixture_reports_its_console_and_a_kill_marks_every_tab_crashed() {
    let server = FixtureServer::start();
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().display().to_string();
    let mut config = config(home.path());
    config.window_environment.push((
        OsString::from("PANEFLOW_TEST_MARKER"),
        OsString::from(&marker),
    ));
    let mut harness = Harness::with_config(home, config);

    assert_eq!(harness.client.host_pid(), None);
    assert!(hosts_carrying(&marker).is_empty());

    let mut terminal = Command::new("sleep").arg("600").spawn().unwrap();

    let app = harness.open("workspace-a", &server.url("/app.html"));
    let form = harness.open("workspace-a", &server.url("/form.html"));
    let first_pid = harness.host_pid();
    assert_eq!(hosts_carrying(&marker), vec![first_pid]);
    assert_eq!(harness.wait_loaded(app, "/app.html"), 200);
    assert_eq!(harness.wait_loaded(form, "/form.html"), 200);
    let console = harness.wait_for("the planted console error", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::Console { tab, level: ConsoleLevel::Error, text, .. })
                if *tab == app && text.contains("TypeError")
        )
    });
    assert!(matches!(
        console,
        ClientEvent::Browser(Event::Console { line: 14, .. })
    ));

    let tree = process_tree(first_pid);
    for pid in &tree {
        assert!(
            !common::command_line(*pid).contains("remote-debugging"),
            "{}",
            common::command_line(*pid)
        );
    }
    let listening = common::listening_tcp_inodes();
    for pid in &tree {
        let open = common::socket_inodes(*pid);
        assert!(
            open.is_disjoint(&listening),
            "process {pid} of the host tree listens on TCP"
        );
    }

    unsafe {
        libc::kill(first_pid as i32, libc::SIGKILL);
    }
    for tab in [app, form] {
        harness.wait_for(&format!("tab {} crashed", tab.0), |event| {
            matches!(
                event,
                ClientEvent::Browser(Event::Crashed { tab: crashed, reason })
                    if *crashed == tab && reason.starts_with(HOST_STOPPED_MESSAGE)
            )
        });
        assert!(matches!(
            harness.client.tab_status(tab),
            Some(TabStatus::Crashed { .. })
        ));
    }
    harness.wait_for("the host crash", |event| {
        matches!(event, ClientEvent::HostCrashed { .. })
    });
    assert!(matches!(terminal.try_wait(), Ok(None)));
    assert!(is_alive(std::process::id()));

    assert_eq!(
        harness.client.reload(app).wait(common::DEADLINE),
        Ok(Reply::Done)
    );
    let second_pid = harness.host_pid();
    assert_ne!(first_pid, second_pid);
    assert_eq!(harness.wait_loaded(app, "/app.html"), 200);

    assert_eq!(
        harness.client.reload(form).wait(common::DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.host_pid(), second_pid);
    assert_eq!(harness.wait_loaded(form, "/form.html"), 200);
    assert_eq!(
        harness.evaluate(form, "document.title"),
        json!("Checkout form")
    );

    harness.shutdown();
    let _ = terminal.kill();
    let _ = terminal.wait();
}

#[test]
fn the_host_exits_after_its_idle_period_once_the_last_tab_closes() {
    let server = FixtureServer::start();
    let home = tempfile::tempdir().unwrap();
    let mut config = config(home.path());
    config.idle_exit = Duration::from_secs(2);
    let mut harness = Harness::with_config(home, config);

    let tab = harness.open("idle", &server.url("/app.html"));
    let pid = harness.host_pid();
    harness.wait_loaded(tab, "/app.html");
    assert_eq!(
        harness.client.close_tab(tab).wait(common::DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the tab to close", |event| {
        matches!(event, ClientEvent::Browser(Event::Closed { tab: closed }) if *closed == tab)
    });
    harness.wait_for("an idle exit", |event| {
        matches!(
            event,
            ClientEvent::HostExited {
                reason: ExitReason::Idle
            }
        )
    });
    assert_eq!(harness.client.host_pid(), None);
    assert!(!is_alive(pid));
}

#[test]
fn a_host_started_from_a_pane_carries_no_pane_context() {
    let server = FixtureServer::start();
    let home = tempfile::tempdir().unwrap();
    let mut config = config(home.path());
    for key in paneflow_home::PANE_CONTEXT_ENV {
        config
            .window_environment
            .push((OsString::from(key), OsString::from("from-the-pane")));
    }
    config
        .window_environment
        .push((OsString::from("PANEFLOW_TEST_KEPT"), OsString::from("kept")));
    let mut harness = Harness::with_config(home, config);

    let tab = harness.open("pane", &server.url("/app.html"));
    harness.wait_loaded(tab, "/app.html");
    let environment = common::environment(harness.host_pid());
    assert!(
        environment
            .iter()
            .any(|entry| entry == "PANEFLOW_TEST_KEPT=kept")
    );
    for key in paneflow_home::PANE_CONTEXT_ENV {
        assert!(
            !environment
                .iter()
                .any(|entry| entry.starts_with(&format!("{key}="))),
            "the host inherited {key}"
        );
    }
    harness.shutdown();
}

struct DirectHost {
    child: std::process::Child,
    endpoint: std::path::PathBuf,
    _home: tempfile::TempDir,
}

fn start_direct_host() -> DirectHost {
    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_home::browser_endpoint_path(home.path(), std::process::id(), 9000);
    assert!(endpoint.starts_with(paneflow_home::runtime_dir()));
    let mut child = Command::new(common::host_binary())
        .arg(format!("{ENDPOINT_ARG}={}", endpoint.display()))
        .arg(format!(
            "{PROFILES_ARG}={}",
            paneflow_home::browser_profiles_dir_in(home.path()).display()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), READY_LINE);
    DirectHost {
        child,
        endpoint,
        _home: home,
    }
}

#[test]
fn the_host_accepts_only_its_parent_process() {
    let mut host = start_direct_host();

    let foreign = Command::new("python3")
        .arg("-c")
        .arg(
            "import socket, sys\n\
             s = socket.socket(socket.AF_UNIX)\n\
             s.connect(sys.argv[1])\n\
             s.settimeout(30)\n\
             try:\n    data = s.recv(1)\nexcept Exception as error:\n    print('error', error); sys.exit(0)\n\
             print('closed' if data == b'' else 'answered')\n",
        )
        .arg(&host.endpoint)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&foreign.stdout).trim(), "closed");

    let mut stream = UnixStream::connect(&host.endpoint).unwrap();
    write_message(&mut stream, &WindowMessage::Hello(Hello::current())).unwrap();
    let answer: HostMessage = read_message(&mut stream).unwrap();
    assert!(matches!(
        answer,
        HostMessage::Welcome(welcome) if welcome.protocol_version == PROTOCOL_VERSION
    ));
    stream.flush().unwrap();
    drop(stream);
    assert!(host.child.wait().unwrap().success());
}

#[test]
fn a_window_of_another_protocol_version_is_refused_naming_both_versions() {
    let mut host = start_direct_host();
    let mut stream = UnixStream::connect(&host.endpoint).unwrap();
    let mut hello = Hello::current();
    hello.protocol_version = PROTOCOL_VERSION + 41;
    write_message(&mut stream, &WindowMessage::Hello(hello)).unwrap();
    let answer: HostMessage = read_message(&mut stream).unwrap();
    let HostMessage::Refused(refusal) = answer else {
        panic!("expected a refusal, got {answer:?}");
    };
    assert_eq!(
        refusal,
        Refusal::VersionMismatch {
            window: PROTOCOL_VERSION + 41,
            host: PROTOCOL_VERSION
        }
    );
    let message = refusal.to_string();
    assert!(message.contains(&format!("version {}", PROTOCOL_VERSION + 41)));
    assert!(message.contains(&format!("version {PROTOCOL_VERSION}")));
    assert!(!host.child.wait().unwrap().success());
}

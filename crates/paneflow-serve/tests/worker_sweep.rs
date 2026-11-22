#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_config::schema::SessionId;
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const SETTLE: Duration = Duration::from_secs(30);

fn connect(home: &Path) -> HostControl {
    HostControl::connect(
        &paneflow_home::serve_endpoint_path(home),
        "worker-sweep-test",
    )
    .expect("the worker answers")
}

fn create(host: &mut HostClient) -> SessionId {
    host.create(&CreateSession {
        session: None,
        workspace: None,
        cwd: Some(std::env::temp_dir().display().to_string()),
        shell: Some("/bin/sh".to_string()),
        args: Vec::new(),
        env: Default::default(),
        cols: Some(80),
        rows: Some(24),
        title: None,
        appearance: None,
        cell: None,
    })
    .expect("a hosted session")
    .manifest
    .session
}

fn listed(host: &mut HostClient) -> Vec<(SessionId, bool)> {
    host.list(None)
        .unwrap()
        .into_iter()
        .map(|row| (row.session, row.live))
        .collect()
}

fn live(host: &mut HostClient, session: &SessionId) -> bool {
    listed(host)
        .iter()
        .any(|(known, live)| known == session && *live)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_worker_sweeps_what_no_surface_holds_and_stops_live_orphans_only_when_asked() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("a worker");
    let mut host = HostClient::connect(&core_endpoint, &ClientHello::local("sweep-test")).unwrap();

    let mut cli = connect(home.path());
    let cwd = tempfile::tempdir().unwrap();
    let opened = cli
        .request("workspace.up", json!({"panes": [{"cwd": cwd.path()}]}))
        .expect("a headless pane");
    let surface_id = opened["surface_ids"][0].as_u64().unwrap();
    let view = cli.request("layout.view", json!({})).unwrap();
    let bound: SessionId = serde_json::from_value(
        view["view"]["state"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|ws| ws["tabs"].as_array().unwrap().iter())
            .map(|tab| tab["root"].to_string())
            .find(|root| root.contains(&format!("\"id\":{surface_id}")))
            .and_then(|root| {
                let root: Value = serde_json::from_str(&root).unwrap();
                root["surfaces"][0]["definition"]["session"]
                    .as_str()
                    .map(str::to_owned)
            })
            .map(Value::String)
            .expect("the pane has a session"),
    )
    .unwrap();

    let orphan = create(&mut host);
    let kept = create(&mut host);
    let ended = create(&mut host);
    let generation = host.inspect(&ended).unwrap().manifest.generation;
    host.stop(&ended, Some(generation)).unwrap();
    let deadline = Instant::now() + SETTLE;
    while live(&mut host, &ended) {
        assert!(Instant::now() < deadline, "the stopped session never ended");
        std::thread::sleep(Duration::from_millis(20));
    }

    let unattached = cli
        .request("layout.sweep", json!({"live": true}))
        .expect_err("a sweep needs an attached client");
    assert!(unattached.contains("layout.attach"), "{unattached}");

    let mut window = connect(home.path());
    window
        .request("layout.attach", json!({"client": "window"}))
        .unwrap();
    let routine = window
        .request("layout.sweep", json!({"live": false}))
        .unwrap();
    assert_eq!(strings(&routine["forgotten"]), vec![ended.to_string()]);
    assert!(
        strings(&routine["stopped"]).is_empty(),
        "a routine sweep stops nothing"
    );
    assert!(live(&mut host, &orphan));

    let restored = window
        .request(
            "layout.sweep",
            json!({"live": true, "keep": [kept.to_string()]}),
        )
        .unwrap();
    assert_eq!(strings(&restored["stopped"]), vec![orphan.to_string()]);
    let after = listed(&mut host);
    assert!(
        after.iter().all(|(session, _)| *session != orphan),
        "a stopped orphan leaves no record"
    );
    assert!(
        live(&mut host, &bound),
        "the surface's session keeps running"
    );
    assert!(
        live(&mut host, &kept),
        "a session the client keeps keeps running"
    );

    for session in [&bound, &kept] {
        let generation = host.inspect(session).unwrap().manifest.generation;
        let _ = host.stop(session, Some(generation));
    }
    drop(host);
    worker.stop();
    core_server.stop().unwrap();
}

#[test]
fn a_session_the_worker_started_is_swept_once_a_rewritten_layout_drops_it() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("a worker");
    let mut host = HostClient::connect(&core_endpoint, &ClientHello::local("sweep-test")).unwrap();
    let session_file = paneflow_home::session_path_in(home.path());
    let saved_with = |session: &SessionId| {
        let deadline = Instant::now() + SETTLE;
        loop {
            let text = std::fs::read_to_string(&session_file).unwrap_or_default();
            if text.contains(session.as_str()) {
                return text;
            }
            assert!(Instant::now() < deadline, "the layout is never written");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let new_session = |host: &mut HostClient, known: &mut Vec<SessionId>| {
        let fresh: Vec<SessionId> = listed(host)
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| !known.contains(id))
            .collect();
        assert_eq!(fresh.len(), 1, "the worker started one terminal");
        known.push(fresh[0].clone());
        fresh[0].clone()
    };

    let mut cli = connect(home.path());
    let cwd = tempfile::tempdir().unwrap();
    let mut known = Vec::new();
    cli.request("workspace.up", json!({"panes": [{"cwd": cwd.path()}]}))
        .expect("a headless pane");
    let first = new_session(&mut host, &mut known);
    let before_second = saved_with(&first);
    cli.request("workspace.up", json!({"panes": [{"cwd": cwd.path()}]}))
        .expect("a second headless pane");
    let dropped = new_session(&mut host, &mut known);
    saved_with(&dropped);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&session_file, before_second).unwrap();
    cli.request("workspace.select", json!({"index": 0}))
        .expect("a structural call reads the rewritten layout");
    let view = cli.request("layout.view", json!({})).unwrap().to_string();
    assert!(
        !view.contains(dropped.as_str()) && view.contains(first.as_str()),
        "the worker reads the rewritten layout: {view}"
    );

    let mut window = connect(home.path());
    window
        .request("layout.attach", json!({"client": "window"}))
        .unwrap();
    let swept = window
        .request("layout.sweep", json!({"live": true}))
        .unwrap();
    assert_eq!(strings(&swept["stopped"]), vec![dropped.to_string()]);
    assert!(
        live(&mut host, &first),
        "the session the layout holds runs on"
    );

    let generation = host.inspect(&first).unwrap().manifest.generation;
    let _ = host.stop(&first, Some(generation));
    drop(host);
    worker.stop();
    core_server.stop().unwrap();
}

#[test]
fn only_the_sweep_after_a_load_a_reread_or_an_adoption_may_stop_live_sessions() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let worker = paneflow_serve::open(home.path()).expect("a worker");
    let mut window = connect(home.path());
    window
        .request("layout.attach", json!({"client": "window"}))
        .unwrap();
    let unlisted = window
        .request("layout.sweep", json!({"live": true}))
        .expect_err("no host lists the sessions");
    assert!(unlisted.contains("terminal host"), "{unlisted}");

    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the host serves its endpoint");
    let mut host = HostClient::connect(&core_endpoint, &ClientHello::local("sweep-test")).unwrap();

    let first = create(&mut host);
    let after_load = window
        .request("layout.sweep", json!({"live": true}))
        .unwrap();
    assert_eq!(
        strings(&after_load["stopped"]),
        vec![first.to_string()],
        "the mark set at load survived the sweep that could not list"
    );

    let second = create(&mut host);
    let ended = create(&mut host);
    let generation = host.inspect(&ended).unwrap().manifest.generation;
    host.stop(&ended, Some(generation)).unwrap();
    let deadline = Instant::now() + SETTLE;
    while live(&mut host, &ended) {
        assert!(Instant::now() < deadline, "the stopped session never ended");
        std::thread::sleep(Duration::from_millis(20));
    }
    let not_due = window
        .request("layout.sweep", json!({"live": true}))
        .unwrap();
    assert!(
        strings(&not_due["stopped"]).is_empty(),
        "a live sweep with no restore behind it stops nothing"
    );
    assert_eq!(strings(&not_due["forgotten"]), vec![ended.to_string()]);
    assert!(live(&mut host, &second));

    let view = window.request("layout.view", json!({})).unwrap();
    let adopted = window
        .request(
            "layout.adopt",
            json!({"view": view["view"], "acknowledged": view["revision"]}),
        )
        .unwrap();
    assert_eq!(adopted["accepted"], true, "{adopted}");
    let after_adopt = window
        .request("layout.sweep", json!({"live": true}))
        .unwrap();
    assert_eq!(strings(&after_adopt["stopped"]), vec![second.to_string()]);

    let third = create(&mut host);
    let session_file = paneflow_home::session_path_in(home.path());
    let deadline = Instant::now() + SETTLE;
    while !session_file.exists() {
        assert!(Instant::now() < deadline, "the layout is never written");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = std::fs::read_to_string(&session_file).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&session_file, format!("{text}\n")).unwrap();
    drop(window);
    let mut window = connect(home.path());
    window
        .request("layout.attach", json!({"client": "window"}))
        .unwrap();
    let after_reread = window
        .request("layout.sweep", json!({"live": true}))
        .unwrap();
    assert_eq!(strings(&after_reread["stopped"]), vec![third.to_string()]);

    drop(host);
    worker.stop();
    core_server.stop().unwrap();
}

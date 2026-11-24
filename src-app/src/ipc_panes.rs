use std::path::Path;
use std::time::Duration;

use paneflow_host::client::HostClient;
use paneflow_host::pane_endpoint::{PaneEndpoints, PaneIdentity, ServeStream};
use paneflow_host::protocol::ClientHello;

const RECONCILE_INTERVAL: Duration = Duration::from_millis(500);

const PANES_CLIENT: &str = "paneflow-window-panes";

type SandboxRequests = std::collections::BTreeMap<String, &'static str>;

static SANDBOXED: std::sync::RwLock<SandboxRequests> =
    std::sync::RwLock::new(std::collections::BTreeMap::new());

pub(crate) fn is_sandboxed(session: &str) -> bool {
    sandbox_request_of(session).is_some()
}

pub(crate) fn sandbox_request_of(session: &str) -> Option<&'static str> {
    SANDBOXED
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session)
        .copied()
}

fn remember_sandboxed(requests: SandboxRequests) {
    let mut known = SANDBOXED
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *known != requests {
        *known = requests;
    }
}

const SPAWNS_ENV_INHERITED: &[&str] = &["surface.split", "workspace.up"];

const SPAWNS_REFUSED_IN_SANDBOX: &[&str] = &[
    "workspace.create",
    "workspace.restore_layout",
    "client.dispatch_action",
];

fn insert_sandbox_request(env: &mut serde_json::Value, request: &str) {
    if !env.is_object() {
        *env = serde_json::Value::Object(serde_json::Map::new());
    }
    let key = paneflow_config::schema::SANDBOX_REQUEST_ENV;
    let stronger = env[key] == paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE;
    if !stronger {
        env[key] = serde_json::Value::String(request.to_string());
    }
}

pub(crate) fn confine_spawn(
    caller: &paneflow_api::Principal,
    method: &str,
    params: &mut serde_json::Value,
) -> Result<(), String> {
    let paneflow_api::Principal::Pane { session, .. } = caller else {
        return Ok(());
    };
    let Some(request) = sandbox_request_of(session) else {
        return Ok(());
    };
    if SPAWNS_REFUSED_IN_SANDBOX.contains(&method) {
        return Err(format!(
            "{method} is not available to a sandboxed pane: the window cannot keep what it starts in the sandbox; use surface.split or workspace.up"
        ));
    }
    if !SPAWNS_ENV_INHERITED.contains(&method) {
        return Ok(());
    }
    let malformed = || format!("{method} from a sandboxed pane needs an object of parameters");
    if !params.is_object() {
        return Err(malformed());
    }
    if method == "workspace.up" {
        let panes = params
            .get_mut("panes")
            .and_then(serde_json::Value::as_array_mut)
            .filter(|panes| panes.iter().all(serde_json::Value::is_object))
            .ok_or_else(malformed)?;
        for pane in panes.iter_mut() {
            insert_sandbox_request(&mut pane["env"], request);
        }
    } else {
        insert_sandbox_request(&mut params["env"], request);
    }
    Ok(())
}

pub(crate) fn start(serve: ServeStream) {
    let Some(home) = paneflow_home::paneflow_home() else {
        return;
    };
    let host = paneflow_home::host_endpoint_path(&home);
    let spawned = std::thread::Builder::new()
        .name("paneflow-ipc-panes".into())
        .spawn(move || reconcile_loop(&home, &host, &serve));
    if let Err(error) = spawned {
        log::warn!("IPC: cannot start the sandboxed pane endpoints: {error}");
    }
}

fn sandboxed_rows(client: &mut HostClient) -> Result<Vec<(PaneIdentity, &'static str)>, String> {
    let rows = client.list(None).map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|row| row.sandboxed && row.live)
        .map(|row| {
            let request = paneflow_config::schema::ProfileSandbox {
                unix_scope: row.sandbox_unix_scope,
            }
            .request_value();
            (
                PaneIdentity {
                    session: row.session.to_string(),
                    workspace: row.workspace.map(|workspace| workspace.to_string()),
                },
                request,
            )
        })
        .collect())
}

pub(crate) fn reconcile(
    endpoints: &PaneEndpoints,
    home: &Path,
    wanted: &[PaneIdentity],
    serve: &ServeStream,
) {
    endpoints.retain(|session| wanted.iter().any(|pane| pane.session == session));
    for pane in wanted {
        if endpoints.is_open(&pane.session) {
            continue;
        }
        if let Err(error) =
            endpoints.ensure(home, paneflow_home::PaneServer::Window, pane.clone(), serve)
        {
            log::debug!(
                "IPC: cannot open the window endpoint of sandboxed pane {}: {error}",
                pane.session
            );
        }
    }
}

fn reconcile_loop(home: &Path, host: &Path, serve: &ServeStream) {
    let endpoints = PaneEndpoints::default();
    let mut client: Option<HostClient> = None;
    loop {
        if client.is_none() {
            client = HostClient::connect(host, &ClientHello::control(PANES_CLIENT)).ok();
        }
        if let Some(connected) = client.as_mut() {
            match sandboxed_rows(connected) {
                Ok(rows) => {
                    remember_sandboxed(
                        rows.iter()
                            .map(|(pane, request)| (pane.session.clone(), *request))
                            .collect(),
                    );
                    let wanted: Vec<PaneIdentity> =
                        rows.into_iter().map(|(pane, _)| pane).collect();
                    reconcile(&endpoints, home, &wanted, serve);
                }
                Err(error) => {
                    log::debug!("IPC: cannot list the sandboxed panes: {error}");
                    client = None;
                }
            }
        }
        std::thread::sleep(RECONCILE_INTERVAL);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::io::{BufRead as _, Write as _};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn window_server(home: &Path) -> ServeStream {
        let (tx, _rx) = smol::channel::bounded(4);
        crate::ipc::pane_connection_server(
            tx,
            crate::ipc_events::EventBus::new(),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(paneflow_host::peer::PeerClassifier::new(
                None,
                Some(home.to_path_buf()),
                "window-panes-test",
            )),
            (
                Arc::new(paneflow_host::policy::PolicySource::new(
                    paneflow_home::config_path_in(home),
                    paneflow_api::policy::Switches::from_environment(),
                )),
                crate::ipc_grants::WindowGrants::start(None),
            ),
        )
    }

    fn whoami(endpoint: &Path) -> serde_json::Value {
        let stream = std::os::unix::net::UnixStream::connect(endpoint).expect("connects");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut writer = stream.try_clone().unwrap();
        writeln!(
            writer,
            r#"{{"jsonrpc":"2.0","id":1,"method":"system.whoami","params":{{}}}}"#
        )
        .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(stream)
            .read_line(&mut line)
            .unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["result"].clone()
    }

    #[test]
    fn a_window_pane_endpoint_answers_as_its_pane_and_closes_with_it() {
        let home = tempfile::Builder::new().prefix("pfp").tempdir().unwrap();
        let pane = PaneIdentity {
            session: "0b5f3c1e-7d1a-4f0e-9a4b-2c8d6e1f3a5b".into(),
            workspace: Some("w-1".into()),
        };
        paneflow_home::create_private_dir_all(&paneflow_home::pane_dir_in(
            home.path(),
            &pane.session,
        ))
        .unwrap();
        let endpoints = PaneEndpoints::default();
        let serve = window_server(home.path());
        reconcile(&endpoints, home.path(), std::slice::from_ref(&pane), &serve);
        let endpoint = paneflow_home::pane_endpoint_path(
            home.path(),
            &pane.session,
            paneflow_home::PaneServer::Window,
        );
        assert_eq!(
            whoami(&endpoint),
            serde_json::json!({"principal": "pane", "session": pane.session, "workspace": "w-1"})
        );
        reconcile(&endpoints, home.path(), &[], &serve);
        assert!(!endpoint.exists());
        assert!(!endpoints.is_open(&pane.session));
    }

    #[test]
    fn a_window_that_starts_after_the_pane_opens_its_endpoint_from_the_host_list() {
        let home = tempfile::Builder::new().prefix("pfp").tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            r#"{"shell_integration": false}"#,
        )
        .unwrap();
        let tools = tempfile::tempdir().unwrap();
        let helper = tools.path().join("paneflow-host");
        std::fs::write(
            &helper,
            "#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let host = paneflow_host::SessionHost::open(home.path(), &endpoint).unwrap();
        host.use_sandbox_helper(helper);
        let server =
            paneflow_host::ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).unwrap();
        let created = host.create(paneflow_host::CreateSession {
            cwd: Some(tools.path().display().to_string()),
            shell: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 60".into()],
            sandbox: true,
            sandbox_unix_scope: true,
            ..paneflow_host::CreateSession::default()
        });
        match created {
            Ok(summary) => {
                let session = summary.manifest.session.to_string();
                let mut client =
                    HostClient::connect(&endpoint, &ClientHello::control(PANES_CLIENT)).unwrap();
                let rows = sandboxed_rows(&mut client).unwrap();
                assert_eq!(
                    rows,
                    vec![(
                        PaneIdentity {
                            session: session.clone(),
                            workspace: None
                        },
                        paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE
                    )]
                );
                let wanted: Vec<PaneIdentity> = rows.into_iter().map(|(pane, _)| pane).collect();
                let endpoints = PaneEndpoints::default();
                reconcile(
                    &endpoints,
                    home.path(),
                    &wanted,
                    &window_server(home.path()),
                );
                let window = paneflow_home::pane_endpoint_path(
                    home.path(),
                    &session,
                    paneflow_home::PaneServer::Window,
                );
                assert_eq!(whoami(&window)["principal"], "pane");
                assert_eq!(whoami(&window)["session"], session.as_str());
            }
            Err(paneflow_host::HostError::SpawnFailed { reason, .. }) => {
                assert!(
                    std::env::var_os("PANEFLOW_SANDBOX_REQUIRED")
                        .is_none_or(|required| required != "1"),
                    "PANEFLOW_SANDBOX_REQUIRED=1 and the sandbox did not start: {reason}"
                );
                assert!(reason.contains("bwrap"), "{reason}");
            }
            Err(other) => panic!("unexpected launch error: {other}"),
        }
        let _ = host.request_shutdown(true);
        server.stop().unwrap();
    }

    #[test]
    fn a_sandboxed_caller_passes_its_sandbox_to_what_it_spawns_through_the_window() {
        let session = "5d1e0c4a-2b6f-4c8e-9a1d-3f7b2e6c8a40";
        let other = "6e2f1d5b-3c7a-4d9f-8b2e-4a8c3f7d9b51";
        remember_sandboxed(
            [
                (
                    session.to_string(),
                    paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW,
                ),
                (
                    other.to_string(),
                    paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW,
                ),
            ]
            .into_iter()
            .collect(),
        );
        let caller = paneflow_api::Principal::Pane {
            session: session.to_string(),
            workspace: None,
        };
        let key = paneflow_config::schema::SANDBOX_REQUEST_ENV;
        let mut split = serde_json::json!({"direction": "vertical"});
        confine_spawn(&caller, "surface.split", &mut split).unwrap();
        assert_eq!(
            split["env"][key],
            paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW
        );

        let mut stricter = serde_json::json!({"env": {
            key: paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE
        }});
        confine_spawn(&caller, "surface.split", &mut stricter).unwrap();
        assert_eq!(
            stricter["env"][key],
            paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE,
            "a caller may ask for a stricter sandbox, never a weaker one"
        );

        let mut downgrade = serde_json::json!({"env": {key: "none"}});
        confine_spawn(&caller, "surface.split", &mut downgrade).unwrap();
        assert_eq!(
            downgrade["env"][key],
            paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW
        );

        let mut up = serde_json::json!({"panes": [{"command": "a"}, {"env": {"X": "1"}}]});
        confine_spawn(&caller, "workspace.up", &mut up).unwrap();
        for pane in up["panes"].as_array().unwrap() {
            assert_eq!(
                pane["env"][key],
                paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW
            );
        }
        assert_eq!(up["panes"][1]["env"]["X"], "1");

        for method in [
            "workspace.create",
            "workspace.restore_layout",
            "client.dispatch_action",
        ] {
            let refused = confine_spawn(&caller, method, &mut serde_json::json!({})).unwrap_err();
            assert!(refused.contains(method), "{refused}");
        }

        confine_spawn(&caller, "workspace.up", &mut serde_json::json!({})).unwrap_err();
        confine_spawn(
            &caller,
            "workspace.up",
            &mut serde_json::json!({"panes": [1]}),
        )
        .unwrap_err();
        confine_spawn(&caller, "surface.split", &mut serde_json::json!([])).unwrap_err();

        let outside = paneflow_api::Principal::Pane {
            session: "7f3a2e6c-4d8b-4e0a-9c3f-5b9d4a8e0c62".to_string(),
            workspace: None,
        };
        let mut untouched = serde_json::json!({"direction": "vertical"});
        confine_spawn(&outside, "surface.split", &mut untouched).unwrap();
        confine_spawn(&outside, "workspace.create", &mut serde_json::json!({})).unwrap();
        assert!(untouched.get("env").is_none());
    }
}

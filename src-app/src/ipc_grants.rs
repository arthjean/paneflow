use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use paneflow_api::methods::grant::GRANT_CHANGED;
use paneflow_api::{Policy, Principal};
use paneflow_host::pane_grants::GrantMirror;
use paneflow_serve::controller::{Controller, EventFrame, EventRequest, EventStream};
use serde_json::{Value, json};

use crate::app::ipc_handler::Admission;

const RETRY: Duration = Duration::from_secs(2);
const EVENT_SLICE: Duration = Duration::from_secs(5);

pub(crate) struct WindowGrants {
    mirror: Arc<GrantMirror>,
    home: Option<PathBuf>,
}

fn read_set(endpoint: &Path) -> Option<paneflow_api::grants::GrantSet> {
    let mut controller = Controller::connect(endpoint).ok()?;
    let listed = controller.call("grant.list", json!({})).ok()?;
    serde_json::from_value(listed).ok()
}

fn sync(mirror: &GrantMirror, endpoint: &Path) {
    match read_set(endpoint) {
        Some(set) => {
            mirror.install(set);
        }
        None => log::debug!("paneflow: cannot read the worker's grant set yet"),
    }
}

fn follow(mirror: &GrantMirror, endpoint: &Path) {
    let request = EventRequest {
        types: vec![GRANT_CHANGED.to_string()],
        ..Default::default()
    };
    loop {
        let mut stream = match EventStream::open(endpoint, request.clone()) {
            Ok(stream) => stream,
            Err(_) => {
                std::thread::sleep(RETRY);
                continue;
            }
        };
        loop {
            match stream.next(EVENT_SLICE) {
                EventFrame::Following { .. } => sync(mirror, endpoint),
                EventFrame::Event(event) => {
                    if let Some(revision) = event["data"]["revision"].as_u64() {
                        mirror.announce(revision);
                    }
                    sync(mirror, endpoint);
                }
                EventFrame::Idle => {}
                EventFrame::Disconnected(_) => break,
            }
        }
    }
}

static FOLLOWED_HOMES: Mutex<Vec<(PathBuf, Arc<GrantMirror>)>> = Mutex::new(Vec::new());

fn followed_mirror(home: &Path) -> Arc<GrantMirror> {
    let mut followed = FOLLOWED_HOMES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some((_, mirror)) = followed.iter().find(|(known, _)| known == home) {
        return Arc::clone(mirror);
    }
    let mirror = Arc::new(GrantMirror::for_home(home));
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let follower = Arc::clone(&mirror);
    match std::thread::Builder::new()
        .name("paneflow-grant-follow".into())
        .spawn(move || follow(&follower, &endpoint))
    {
        Ok(_) => followed.push((home.to_path_buf(), Arc::clone(&mirror))),
        Err(error) => log::warn!("paneflow: cannot follow the worker's grants: {error}"),
    }
    mirror
}

impl WindowGrants {
    pub(crate) fn start(home: Option<PathBuf>) -> Arc<Self> {
        let mirror = match home.as_deref() {
            Some(home) => followed_mirror(home),
            None => Arc::new(GrantMirror::new(None)),
        };
        Arc::new(Self { mirror, home })
    }

    #[cfg(test)]
    pub(crate) fn detached(mirror: Arc<GrantMirror>, home: Option<PathBuf>) -> Self {
        Self { mirror, home }
    }

    fn generation_of(&self, principal: &Principal, holds: bool) -> Option<u64> {
        let Principal::Pane { session, .. } = principal else {
            return None;
        };
        if !holds {
            return None;
        }
        paneflow_host::pane_grants::manifest_generation(self.home.as_deref()?, session)
    }

    pub(crate) fn admit(
        &self,
        principal: &Principal,
        method: &str,
        params: &mut Value,
        policy: &Policy,
    ) -> Admission {
        let snapshot = self.mirror.snapshot();
        let holds = matches!(principal, Principal::Pane { session, .. }
            if snapshot.grants.iter().any(|grant| grant.holder.session == *session));
        let generation = self.generation_of(principal, holds);
        let view = snapshot.view(paneflow_host::manifest::now_ms(), generation);
        crate::app::ipc_handler::admit_with(principal, method, params, policy, &view)
    }
}

#[cfg(test)]
pub(crate) fn detached_grants() -> WindowGrants {
    WindowGrants::detached(Arc::new(GrantMirror::new(None)), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::grants::{GrantHolder, GrantOrigin, GrantScope, GrantSet, PaneGrant, Right};
    use std::time::Instant;

    fn pane(session: &str, workspace: Option<&str>) -> Principal {
        Principal::Pane {
            session: session.to_string(),
            workspace: workspace.map(str::to_string),
        }
    }

    fn write_grant(session: &str, generation: u64) -> PaneGrant {
        PaneGrant {
            id: "g-1".into(),
            holder: GrantHolder {
                session: session.to_string(),
                generation,
                workspace: None,
            },
            rights: vec![Right::Write],
            scope: GrantScope::Workspace,
            expires_at_ms: None,
            delegated_from: None,
            origin: GrantOrigin::Prompt,
            granted_at_ms: 0,
        }
    }

    fn real_session(home: &Path) -> (paneflow_config::schema::SessionId, Option<String>) {
        let host =
            paneflow_host::host::SessionHost::open(home, Path::new("ipc-grants-test")).unwrap();
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q".to_string(), "/D".to_string()]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::new());
        let created = host
            .create(paneflow_host::host::CreateSession {
                shell: Some(shell.to_string()),
                args,
                workspace: Some(paneflow_config::schema::WorkspaceId::new()),
                cwd: Some(home.display().to_string()),
                ..Default::default()
            })
            .unwrap();
        let workspace = created.manifest.workspace.map(|w| w.to_string());
        let _ = host.request_shutdown(true);
        (created.manifest.session, workspace)
    }

    fn refusal_reason(admission: &Admission) -> Option<String> {
        match admission {
            Admission::Refused { data, .. } => data["reason"].as_str().map(str::to_string),
            Admission::Allowed(_) => None,
        }
    }

    #[test]
    fn a_window_holding_an_older_set_refuses_a_request_that_needs_a_grant_and_answers_the_rest() {
        let mirror = Arc::new(GrantMirror::new(None));
        let grants = WindowGrants::detached(Arc::clone(&mirror), None);
        mirror.announce(5);
        let caller = pane("s-1", Some("w-1"));
        let refused = grants.admit(
            &caller,
            "surface.send_text",
            &mut json!({"text": "x"}),
            &Policy::default(),
        );
        assert_eq!(
            refusal_reason(&refused).as_deref(),
            Some("grants_unsynced"),
            "{refused:?}"
        );
        let Admission::Refused { code, message, .. } = refused else {
            panic!("refused");
        };
        assert_eq!(code, -32601);
        assert_eq!(message, "Grants are being synchronized; retry the call");
        let listed = grants.admit(&caller, "surface.list", &mut json!({}), &Policy::default());
        assert!(matches!(listed, Admission::Allowed(_)), "{listed:?}");
        mirror.install(GrantSet {
            revision: 5,
            grants: Vec::new(),
        });
        let closed = grants.admit(
            &caller,
            "surface.send_text",
            &mut json!({"text": "x"}),
            &Policy::default(),
        );
        let Admission::Allowed(gates) = closed else {
            panic!("a closed gate is decided on the app thread: {closed:?}");
        };
        assert!(!gates.scripting && !gates.stale_grants);
    }

    #[test]
    fn a_grant_of_the_pane_at_its_current_generation_opens_its_write_gate_in_the_window() {
        let home = tempfile::tempdir().unwrap();
        let (session, workspace) = real_session(home.path());
        let mirror = Arc::new(GrantMirror::new(None));
        let grants = WindowGrants::detached(Arc::clone(&mirror), Some(home.path().to_path_buf()));
        let caller = pane(session.as_str(), workspace.as_deref());
        for (generation, opens) in [(1, true), (2, false)] {
            mirror.install(GrantSet {
                revision: generation,
                grants: vec![write_grant(session.as_str(), generation)],
            });
            let admitted = grants.admit(
                &caller,
                "surface.send_text",
                &mut json!({"text": "x"}),
                &Policy::default(),
            );
            let Admission::Allowed(gates) = admitted else {
                panic!("admitted: {admitted:?}");
            };
            assert_eq!(gates.scripting, opens, "generation {generation}");
            assert!(!gates.reach);
        }
    }

    #[test]
    fn an_instance_write_grant_reaches_every_workspace_in_the_window_without_opening_a_spawn() {
        let home = tempfile::tempdir().unwrap();
        let (session, workspace) = real_session(home.path());
        let mirror = Arc::new(GrantMirror::new(None));
        let grants = WindowGrants::detached(Arc::clone(&mirror), Some(home.path().to_path_buf()));
        let caller = pane(session.as_str(), workspace.as_deref());
        let everywhere = |scope| {
            mirror.install(GrantSet {
                revision: 1,
                grants: vec![PaneGrant {
                    scope,
                    ..write_grant(session.as_str(), 1)
                }],
            });
            let admitted = grants.admit(
                &caller,
                "surface.send_text",
                &mut json!({"text": "x", "scope": "all"}),
                &Policy::default(),
            );
            let Admission::Allowed(gates) = admitted else {
                panic!("admitted: {admitted:?}");
            };
            assert!(gates.scripting && !gates.orchestration);
            gates.reach
        };
        assert!(!everywhere(GrantScope::Workspace));
        assert!(everywhere(GrantScope::Instance));
    }

    #[test]
    fn the_window_reads_the_grant_set_after_each_grant_changed_event() {
        let home = tempfile::tempdir().unwrap();
        let running = paneflow_serve::open(home.path()).unwrap();
        let grants = WindowGrants::start(Some(home.path().to_path_buf()));
        let worker = running.worker();
        let (_, change) = worker.pane_grants.create(
            paneflow_serve::pane_grants::NewGrant {
                holder: GrantHolder {
                    session: "s-1".into(),
                    generation: 1,
                    workspace: None,
                },
                rights: vec![Right::Write],
                scope: GrantScope::Workspace,
                expires_at_ms: None,
                delegated_from: None,
                origin: GrantOrigin::Prompt,
            },
            paneflow_host::manifest::now_ms(),
        );
        let revision = change.revision;
        worker.publish_grant_change(change);
        let deadline = Instant::now() + Duration::from_secs(60);
        while grants.mirror.held_revision() < revision {
            assert!(
                Instant::now() < deadline,
                "the window never read revision {revision}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        running.stop();
    }

    #[test]
    fn windows_beyond_the_worker_connection_cap_share_one_grant_follower_and_the_worker_still_answers()
     {
        let home = tempfile::tempdir().unwrap();
        let running = paneflow_serve::open(home.path()).unwrap();
        let windows: Vec<Arc<WindowGrants>> = (0..paneflow_serve::server::MAX_CONNECTIONS + 32)
            .map(|_| WindowGrants::start(Some(home.path().to_path_buf())))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline
            && windows
                .iter()
                .any(|window| window.mirror.held_revision() == 0)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let answered = paneflow_ipc_client::host_control::HostControl::connect(
            &paneflow_home::serve_endpoint_path(home.path()),
            "grant-windows",
        )
        .map(drop);
        assert_eq!(answered, Ok(()), "the worker answers another client");
        let unsynced = windows
            .iter()
            .filter(|window| window.mirror.held_revision() == 0)
            .count();
        assert_eq!(unsynced, 0, "every window read the grant set");
        assert_eq!(
            running.worker().events.follower_count(),
            1,
            "one follower serves every window of the process"
        );
        running.stop();
    }
}

use std::sync::{Arc, Mutex};

use gpui::App;
use paneflow_config::schema::{SessionId, WorkspaceId};

pub(crate) const FAKE_HOST_ENDPOINT: &str = "fake-host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostRequest {
    Create {
        session: SessionId,
    },
    Reattach {
        session: SessionId,
    },
    Hold {
        session: SessionId,
    },
    Stop {
        session: SessionId,
    },
    Assign {
        session: SessionId,
        workspace: WorkspaceId,
    },
}

impl HostRequest {
    pub(crate) fn session(&self) -> &SessionId {
        match self {
            Self::Create { session }
            | Self::Reattach { session }
            | Self::Hold { session }
            | Self::Stop { session }
            | Self::Assign { session, .. } => session,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct FakeHost {
    requests: Arc<Mutex<Vec<HostRequest>>>,
    spawned_surfaces: Arc<Mutex<std::collections::HashMap<SessionId, u64>>>,
}

impl gpui::Global for FakeHost {}

impl FakeHost {
    pub(crate) fn take(&self) -> Vec<HostRequest> {
        self.requests
            .lock()
            .map(|mut requests| std::mem::take(&mut *requests))
            .unwrap_or_default()
    }

    pub(crate) fn spawned_with_surface(&self, session: SessionId, surface: u64) {
        if let Ok(mut spawned) = self.spawned_surfaces.lock() {
            spawned.insert(session, surface);
        }
    }

    pub(crate) fn spawned_surface_ids(
        &self,
        sessions: &[SessionId],
    ) -> std::collections::HashMap<SessionId, u64> {
        let Ok(spawned) = self.spawned_surfaces.lock() else {
            return std::collections::HashMap::new();
        };
        sessions
            .iter()
            .filter_map(|session| spawned.get(session).map(|id| (session.clone(), *id)))
            .collect()
    }

    fn push(&self, request: HostRequest) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(request);
        }
    }
}

pub(crate) fn installed(cx: &App) -> bool {
    cx.has_global::<FakeHost>()
}

pub(crate) fn record(cx: &App, request: HostRequest) -> bool {
    match cx.try_global::<FakeHost>() {
        Some(host) => {
            host.push(request);
            true
        }
        None => false,
    }
}

pub(crate) fn hosted_session() -> crate::terminal::host_link::HostedSession {
    crate::terminal::host_link::HostedSession {
        endpoint: std::path::PathBuf::from(FAKE_HOST_ENDPOINT),
        generation: paneflow_config::schema::SessionGeneration::FIRST,
    }
}

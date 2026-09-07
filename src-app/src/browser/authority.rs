use std::path::PathBuf;

use gpui::App;
use paneflow_browser_protocol::{
    Availability, BrowserError, CONTRACT_VERSION, Command, Controller, Envelope, Event,
    MAX_BROWSERS_PER_SESSION, MAX_LIVE_BROWSERS, OperationId, Owner, SessionId, WorkspaceId,
};

use super::profile::{self, ProfileError, ProfileStore};

pub struct BrowserAuthority {
    availability: Availability,
    controller: Controller,
    profiles: Result<ProfileStore, ProfileError>,
    host_binary: Option<PathBuf>,
    runtime_root: Option<PathBuf>,
    next_operation: u64,
}

impl gpui::Global for BrowserAuthority {}

impl BrowserAuthority {
    pub fn install(cx: &mut App) {
        cx.set_global(Self::detect());
    }

    fn detect() -> Self {
        let target = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
        let runtime_root = if cfg!(target_os = "linux") {
            std::env::var_os(super::RUNTIME_ENV).map(PathBuf::from)
        } else {
            None
        };
        let host_binary = runtime_root.as_ref().and_then(|_| {
            std::env::var_os(super::HOST_ENV)
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(|dir| dir.join("paneflow-browser-host")))
                })
        });
        let development = runtime_root.is_some() && host_binary.is_some();
        let profiles = if development {
            match profile::default_root() {
                Some(root) => ProfileStore::open(root),
                None => Err(ProfileError::Inaccessible(
                    "no writable data directory".to_string(),
                )),
            }
        } else {
            Err(ProfileError::Inaccessible(
                "browser runtime not configured".to_string(),
            ))
        };
        if let Err(error) = &profiles
            && development
        {
            log::warn!("browser: profile root unavailable: {error:?}");
        }
        Self {
            availability: if development {
                Availability::Development
            } else {
                Availability::Absent
            },
            controller: Controller::new(target, development),
            profiles,
            host_binary,
            runtime_root,
            next_operation: 1,
        }
    }

    #[cfg(test)]
    pub fn for_test(root: PathBuf) -> Self {
        Self {
            availability: Availability::Development,
            controller: Controller::new("test".to_string(), true),
            profiles: ProfileStore::open(root),
            host_binary: None,
            runtime_root: None,
            next_operation: 1,
        }
    }

    pub fn available(cx: &App) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|authority| authority.availability != Availability::Absent)
    }

    pub fn profiles(&self) -> Result<&ProfileStore, ProfileError> {
        self.profiles.as_ref().map_err(Clone::clone)
    }

    pub fn host_paths(&self) -> Option<(PathBuf, PathBuf)> {
        Some((self.host_binary.clone()?, self.runtime_root.clone()?))
    }

    pub fn scope(workspace_id: u64, tab_id: u64) -> Owner {
        Owner {
            workspace: WorkspaceId::try_from(format!("ws-{workspace_id}"))
                .unwrap_or_else(|_| unreachable!("numeric identities are ASCII")),
            session: SessionId::try_from(format!("tab-{tab_id}"))
                .unwrap_or_else(|_| unreachable!("numeric identities are ASCII")),
        }
    }

    pub fn dispatch(&mut self, scope: &Owner, command: Command) -> Result<Event, BrowserError> {
        let operation = OperationId::try_from(format!("ui-{}", self.next_operation))
            .map_err(|_| BrowserError::LimitReached)?;
        self.next_operation = self.next_operation.wrapping_add(1).max(1);
        self.controller
            .dispatch(
                scope,
                Envelope {
                    version: CONTRACT_VERSION,
                    operation,
                    command,
                },
            )
            .result
    }
}

pub fn refusal_message(error: BrowserError) -> String {
    match error {
        BrowserError::LimitReached => {
            format!("Browser tab limit of {MAX_BROWSERS_PER_SESSION} reached")
        }
        BrowserError::Busy => {
            format!("Put a browser tab to sleep to continue ({MAX_LIVE_BROWSERS} live pages)")
        }
        BrowserError::InvalidUrl | BrowserError::TooLarge => {
            "This address is not supported".to_string()
        }
        BrowserError::Unavailable => "The browser is unavailable".to_string(),
        BrowserError::AccessDenied | BrowserError::UnknownIdentity => {
            "This page no longer belongs to an open session".to_string()
        }
        BrowserError::StaleGeneration => "The document has changed".to_string(),
        BrowserError::IncompatibleVersion => "Repair the browser installation".to_string(),
        BrowserError::InvalidMessage | BrowserError::InvalidFrame => {
            "The browser refused this action".to_string()
        }
    }
}

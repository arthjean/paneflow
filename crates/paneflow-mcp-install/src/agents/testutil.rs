//! Shared test double for the orchestration tests in `cli` and `api`.
//!
//! `Mock` implements [`AgentConfigWriter`] with injected outcomes and a
//! presence flag, so the engine can be exercised across every branch
//! without touching the filesystem or PATH (and without ever shelling out
//! to a real `claude`/`codex` on the developer's machine).

use std::cell::Cell;
use std::path::Path;

use anyhow::Result;

use crate::agents::{AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::detect::Presence;

pub(crate) struct Mock {
    id: &'static str,
    present: bool,
    install: Cell<Option<Result<InstallOutcome>>>,
    uninstall: Cell<Option<Result<UninstallOutcome>>>,
    status: Cell<Option<Result<StatusOutcome>>>,
}

impl Mock {
    pub(crate) fn present(id: &'static str) -> Self {
        Self {
            id,
            present: true,
            install: Cell::new(Some(Ok(InstallOutcome::Installed))),
            uninstall: Cell::new(Some(Ok(UninstallOutcome::Removed))),
            status: Cell::new(Some(Ok(StatusOutcome::Installed { path: "/p".into() }))),
        }
    }

    pub(crate) fn absent(id: &'static str) -> Self {
        let m = Self::present(id);
        Self {
            present: false,
            ..m
        }
    }

    pub(crate) fn with_install(self, r: Result<InstallOutcome>) -> Self {
        self.install.set(Some(r));
        self
    }
}

impl AgentConfigWriter for Mock {
    fn id(&self) -> &'static str {
        self.id
    }
    fn label(&self) -> &'static str {
        self.id
    }
    fn presence(&self) -> Presence {
        if self.present {
            Presence::Present
        } else {
            Presence::Absent
        }
    }
    fn install(&self, _bridge: &Path) -> Result<InstallOutcome> {
        self.install
            .take()
            .unwrap_or(Ok(InstallOutcome::AlreadyCurrent))
    }
    fn uninstall(&self) -> Result<UninstallOutcome> {
        self.uninstall
            .take()
            .unwrap_or(Ok(UninstallOutcome::NothingToRemove))
    }
    fn status(&self, _bridge: Option<&Path>) -> Result<StatusOutcome> {
        self.status
            .take()
            .unwrap_or(Ok(StatusOutcome::NotInstalled))
    }
}

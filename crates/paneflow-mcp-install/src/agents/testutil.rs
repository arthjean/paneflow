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

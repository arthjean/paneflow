use std::path::PathBuf;
use std::time::Duration;

use paneflow_browser_client::BrowserClient;
use paneflow_browser_proto::{CommandError, OriginDecision, ProfileName, Reply, TabId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    pub binary: PathBuf,
    pub installed: bool,
    pub running: Option<RunningHost>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningHost {
    pub pid: u32,
    pub engine: String,
    pub sandbox: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TabCommand {
    Navigate(String),
    Reload,
    Close,
    DevTools {
        method: String,
        params: serde_json::Value,
    },
    Decide {
        origin: String,
        decision: OriginDecision,
    },
}

pub trait Engine: Send + Sync + 'static {
    fn status(&self) -> EngineStatus;

    fn open_tab(
        &self,
        profile: ProfileName,
        url: &str,
        deadline: Duration,
    ) -> (TabId, Result<Reply, CommandError>);

    fn send(
        &self,
        tab: TabId,
        command: TabCommand,
        deadline: Duration,
    ) -> Result<Reply, CommandError>;
}

impl Engine for BrowserClient {
    fn status(&self) -> EngineStatus {
        let binary = self.binary().to_path_buf();
        EngineStatus {
            installed: binary.is_file(),
            binary,
            running: self.host_info().map(|info| RunningHost {
                pid: info.pid,
                engine: info.engine,
                sandbox: info.sandbox,
            }),
        }
    }

    fn open_tab(
        &self,
        profile: ProfileName,
        url: &str,
        deadline: Duration,
    ) -> (TabId, Result<Reply, CommandError>) {
        let (tab, ticket) = BrowserClient::open_tab(self, profile, url);
        (tab, ticket.wait(deadline))
    }

    fn send(
        &self,
        tab: TabId,
        command: TabCommand,
        deadline: Duration,
    ) -> Result<Reply, CommandError> {
        let ticket = match command {
            TabCommand::Navigate(url) => self.navigate(tab, url),
            TabCommand::Reload => self.reload(tab),
            TabCommand::Close => self.close_tab(tab),
            TabCommand::DevTools { method, params } => self.devtools(tab, method, params),
            TabCommand::Decide { origin, decision } => self.decide_origin(tab, origin, decision),
        };
        ticket.wait(deadline)
    }
}

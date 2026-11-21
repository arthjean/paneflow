use std::path::Path;
use std::sync::Arc;

use paneflow_config::schema::SessionId;
use paneflow_host::CreateSession;
use paneflow_serve::launcher::{LaunchError, LaunchSupport, SessionLink, SessionStarter};

struct AcceptingStarter;

struct AcceptingLink;

struct RefusingStarter;

struct RefusingLink;

pub const REFUSAL: &str = "the pty pool is exhausted";

impl SessionStarter for RefusingStarter {
    fn connect(&self) -> Result<Box<dyn SessionLink>, LaunchError> {
        Ok(Box::new(RefusingLink))
    }
}

impl SessionLink for RefusingLink {
    fn exists(&mut self, _: &SessionId) -> Result<bool, LaunchError> {
        Ok(false)
    }

    fn create(&mut self, _: &CreateSession) -> Result<(), LaunchError> {
        Err(LaunchError::Refused(REFUSAL.to_string()))
    }
}

impl SessionStarter for AcceptingStarter {
    fn connect(&self) -> Result<Box<dyn SessionLink>, LaunchError> {
        Ok(Box::new(AcceptingLink))
    }
}

impl SessionLink for AcceptingLink {
    fn exists(&mut self, _: &SessionId) -> Result<bool, LaunchError> {
        Ok(false)
    }

    fn create(&mut self, _: &CreateSession) -> Result<(), LaunchError> {
        Ok(())
    }
}

fn open_with_starter(
    home: &Path,
    starter: Arc<dyn SessionStarter>,
) -> paneflow_serve::worker::RunningWorker {
    paneflow_serve::open_with(
        home,
        paneflow_serve::Launching {
            support: LaunchSupport::default(),
            starter: Some(starter),
        },
    )
    .expect("the worker opens")
}

pub fn open_starting_terminals(home: &Path) -> paneflow_serve::worker::RunningWorker {
    open_with_starter(home, Arc::new(AcceptingStarter))
}

#[allow(dead_code, reason = "only the worktree tests need a host that refuses")]
pub fn open_refusing_terminals(home: &Path) -> paneflow_serve::worker::RunningWorker {
    open_with_starter(home, Arc::new(RefusingStarter))
}

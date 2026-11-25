use paneflow_api::methods::browser::TabOwner;
use paneflow_browser_proto::{ProfileName, TabId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteChoice {
    AllowOnce,
    AllowAlways,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRequest {
    pub tab: TabId,
    pub owner: TabOwner,
    pub workspace: ProfileName,
    pub origin: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluateRequest {
    pub tab: TabId,
    pub owner: TabOwner,
    pub workspace: ProfileName,
    pub origin: String,
    pub script: String,
}

pub trait Approvals: Send + Sync + 'static {
    fn site(&self, request: &SiteRequest) -> Result<SiteChoice, String>;

    fn evaluate(&self, request: &EvaluateRequest) -> Result<bool, String>;
}

pub struct Refuse;

impl Approvals for Refuse {
    fn site(&self, _request: &SiteRequest) -> Result<SiteChoice, String> {
        Err("no Paneflow window can ask the user".to_string())
    }

    fn evaluate(&self, _request: &EvaluateRequest) -> Result<bool, String> {
        Err("no Paneflow window can ask the user".to_string())
    }
}

use paneflow_api::methods::browser::{TabOwner, TabState};
use paneflow_browser_proto::surface::{Cursor, DialogKind, FileMode, Rect};
use paneflow_browser_proto::{NavigationRefusal, Permission, ProfileName, TabId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDialog {
    pub dialog: u64,
    pub kind: DialogKind,
    pub origin: String,
    pub message: String,
    pub default_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChooser {
    pub request: u64,
    pub mode: FileMode,
    pub title: String,
    pub accept: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceEvent {
    Changed(TabId),
    Closed(TabId),
    Cursor {
        tab: TabId,
        cursor: Cursor,
    },
    ImeCaret {
        tab: TabId,
        caret: Rect,
    },
    Dialog {
        tab: TabId,
        dialog: PageDialog,
    },
    FileChooser {
        tab: TabId,
        chooser: FileChooser,
    },
    Opened {
        tab: TabId,
        opener: TabId,
    },
    Annotation {
        tab: TabId,
        context: i64,
        payload: String,
    },
    Host,
}

pub const ANNOTATION_BINDING: &str = "__paneflowAnnotate";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedPermission {
    pub origin: String,
    pub permission: Permission,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldSite {
    pub origin: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabView {
    pub tab: TabId,
    pub workspace: ProfileName,
    pub url: String,
    pub title: Option<String>,
    pub state: TabState,
    pub owner: TabOwner,
    pub loading: bool,
    pub load_progress: u16,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub held: Option<HeldSite>,
    pub refusal: Option<NavigationRefusal>,
    pub failure: Option<String>,
    pub crash: Option<String>,
    pub presentation_failure: Option<String>,
    pub taken_over_from: Option<String>,
    pub blocked: Vec<BlockedPermission>,
}

impl TabView {
    pub fn agent_session(&self) -> Option<&str> {
        match &self.owner {
            TabOwner::Agent { session, .. } => Some(session),
            TabOwner::Human | TabOwner::Unclaimed => None,
        }
    }
}

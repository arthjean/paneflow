use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{AppContext, Context, Entity, Pixels, Point};

use crate::PaneFlowApp;
use crate::diff::ReviewSubject;
use crate::layout::LayoutTree;
use crate::pane::Pane;
use crate::widgets::text_input::TextInput;

mod events;
mod grid;
mod menu;
mod mode;
mod rail;
mod session;

pub(crate) use session::surface_for_subject;

pub(crate) const MAX_REVIEW_PANES: usize = 6;
pub(crate) const REVIEW_WORKSPACES_RAIL_WIDTH: f32 = 220.;
pub(crate) const REVIEW_CHANGES_RAIL_WIDTH: f32 = crate::app::constants::SIDEBAR_WIDTH;

#[derive(Clone)]
pub(crate) struct ReviewRailMenu {
    pub(crate) subject: ReviewSubject,
    pub(crate) position: Point<Pixels>,
}

pub(crate) struct ReviewState {
    pub(crate) layout: Option<LayoutTree>,
    pub(crate) saved_layout: Option<LayoutTree>,
    pub(crate) active_pane: Option<Entity<Pane>>,
    pub(crate) collapsed: HashSet<PathBuf>,
    pub(crate) rail_menu: Option<ReviewRailMenu>,
    pub(crate) base_picker_open: bool,
    pub(crate) base_filter: Entity<TextInput>,
    pub(crate) selected_file: Option<String>,
    pub(crate) files_tree: bool,
    pub(crate) collapsed_dirs: HashSet<String>,
    pub(crate) file_filter: Entity<TextInput>,
}

impl ReviewState {
    pub(crate) fn new(cx: &mut Context<PaneFlowApp>) -> Self {
        let file_filter = cx.new(|cx| TextInput::new("", "Filter files…", cx));
        cx.observe(&file_filter, |_, _, cx| cx.notify()).detach();
        let base_filter = cx.new(|cx| TextInput::new("", "Base branch or ref", cx));
        cx.observe(&base_filter, |_, _, cx| cx.notify()).detach();
        Self {
            layout: None,
            saved_layout: None,
            active_pane: None,
            collapsed: HashSet::new(),
            rail_menu: None,
            base_picker_open: false,
            base_filter,
            selected_file: None,
            files_tree: false,
            collapsed_dirs: HashSet::new(),
            file_filter,
        }
    }

    pub(crate) fn full_layout(&self) -> Option<&LayoutTree> {
        self.saved_layout.as_ref().or(self.layout.as_ref())
    }

    pub(crate) fn is_zoomed(&self) -> bool {
        self.saved_layout.is_some()
    }

    pub(crate) fn dismiss_popovers(&mut self) {
        self.rail_menu = None;
        self.base_picker_open = false;
    }
}

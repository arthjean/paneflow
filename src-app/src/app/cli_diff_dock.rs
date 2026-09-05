use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled,
    Window, div, px,
};

use crate::PaneFlowApp;
use crate::app::diff_dock::{DIFF_DOCK_PANEL_MIN_WIDTH, DiffDockData, DiffDockTab};

const PANE_GRID_RESERVED_WIDTH: f32 =
    crate::layout::MIN_PANE_SIZE + 2. * crate::layout::PANE_GUTTER_PX;

fn diff_dock_fit(preferred: f32, available: f32) -> (f32, f32) {
    let max = (available - PANE_GRID_RESERVED_WIDTH - crate::layout::PANE_GUTTER_PX)
        .max(DIFF_DOCK_PANEL_MIN_WIDTH);
    (preferred.min(max), max)
}

pub(crate) struct DiffDockSlot {
    open: bool,
    picker: bool,
    picked: bool,
    tabs: Vec<DiffDockTab>,
    active_tab: usize,
    data: Option<DiffDockData>,
}

impl DiffDockSlot {
    fn is_idle(&self) -> bool {
        !self.open && !self.picked && self.tabs.is_empty()
    }
}

impl PaneFlowApp {
    pub(crate) fn sync_diff_dock_session(&mut self, cx: &mut Context<Self>) {
        let active = self.active_session_id();
        if self.diff_dock.owner == active {
            return;
        }
        let previous = self.diff_dock.owner;
        self.diff_dock.owner = active;
        self.park_live_diff_dock(previous, cx);
        self.prune_parked_diff_docks();
        self.restore_diff_dock(active, cx);
    }

    pub(crate) fn active_session_id(&self) -> Option<u64> {
        self.active_workspace().map(|ws| ws.active_tab().id)
    }

    pub(crate) fn prune_parked_diff_docks(&mut self) {
        let workspaces = &self.workspaces;
        self.diff_dock.parked.retain(|id, _| {
            workspaces
                .iter()
                .flat_map(|ws| ws.tabs())
                .any(|tab| tab.id == *id)
        });
    }

    fn park_live_diff_dock(&mut self, owner: Option<u64>, cx: &mut Context<Self>) {
        let slot = DiffDockSlot {
            open: self.diff_dock.open,
            picker: self.diff_dock.picker,
            picked: self.diff_dock.picked,
            tabs: std::mem::take(&mut self.diff_dock.diff_tabs),
            active_tab: std::mem::replace(&mut self.diff_dock.diff_active_tab, 0),
            data: self.diff_dock.data.take(),
        };
        self.close_diff_dock_panel(cx);
        self.diff_dock.picker = false;
        self.diff_dock.picked = false;
        self.diff_dock.diff_tab_close_armed = None;
        self.diff_dock.diff_options_menu_open = false;
        self.diff_dock.diff_options_submenu = None;
        self.diff_dock.diff_new_tab_menu_open = false;
        self.diff_dock.diff_branch_menu = None;

        let owner = owner.filter(|id| {
            self.workspaces
                .iter()
                .flat_map(|ws| ws.tabs())
                .any(|tab| tab.id == *id)
        });
        match owner {
            None => drop(slot),
            Some(id) if slot.is_idle() => {
                self.diff_dock.parked.remove(&id);
            }
            Some(id) => {
                self.diff_dock.parked.insert(id, slot);
            }
        }
    }

    fn restore_diff_dock(&mut self, session_id: Option<u64>, cx: &mut Context<Self>) {
        let Some(slot) = session_id.and_then(|id| self.diff_dock.parked.remove(&id)) else {
            return;
        };
        self.diff_dock.picker = slot.picker;
        self.diff_dock.picked = slot.picked;
        self.diff_dock.diff_tabs = slot.tabs;
        self.diff_dock.diff_active_tab = slot.active_tab;
        let cwd = slot
            .data
            .as_ref()
            .map(|data| data.cwd.clone())
            .filter(|cwd| !cwd.is_empty());
        self.diff_dock.data = slot.data;
        if slot.open {
            let cwd = cwd.or_else(|| self.active_checkout()).unwrap_or_default();
            self.open_diff_dock_panel(cwd, cx);
        }
    }

    pub(crate) fn toggle_cli_diff_dock(&mut self, cwd: String, cx: &mut Context<Self>) {
        let cwd = cwd.trim().to_string();
        let showing = self.diff_dock.open
            && self
                .diff_dock
                .data
                .as_ref()
                .is_some_and(|data| data.cwd == cwd);
        if showing {
            self.close_diff_dock_panel(cx);
        } else {
            self.diff_dock.picker = !self.diff_dock.picked;
            self.open_diff_dock_panel(cwd, cx);
        }
    }

    pub(crate) fn diff_dock_visible(&self) -> bool {
        self.diff_dock.open
            && self.settings_section.is_none()
            && matches!(self.mode, paneflow_config::schema::AppMode::Cli)
    }

    pub(crate) fn wrap_cli_diff_dock(
        &mut self,
        body: AnyElement,
        available_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.sync_files_sidebar_session(cx);
        let files_width = self.rendered_files_sidebar_width(window, cx);
        if !self.diff_dock_visible() {
            return body;
        }
        let ui = crate::theme::ui_colors();
        let (width, max_width) = diff_dock_fit(self.diff_dock.width, available_width);
        div()
            .size_full()
            .flex()
            .flex_row()
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _w, cx| {
                if this.diff_dock.h_scroll_drag.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_diff_dock_h_scrollbar(event.position.x, cx);
                    } else {
                        this.end_diff_dock_h_scrollbar_drag(cx);
                    }
                } else if this.diff_dock.resize.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_diff_dock_resize(f32::from(event.position.x), cx);
                    } else {
                        this.end_diff_dock_resize(cx);
                    }
                } else {
                    this.update_diff_dock_hover(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _w, cx| {
                    this.end_diff_dock_h_scrollbar_drag(cx);
                    this.end_diff_dock_resize(cx);
                }),
            )
            .child(div().flex_1().min_w_0().h_full().child(body))
            .child(
                div()
                    .flex_none()
                    .h_full()
                    .flex()
                    .flex_col()
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .child(self.render_diff_dock_panel(width, max_width, files_width, ui, cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(open: bool, picked: bool, tabs: usize) -> DiffDockSlot {
        DiffDockSlot {
            open,
            picker: false,
            picked,
            tabs: (0..tabs).map(|_| DiffDockTab::Changes).collect(),
            active_tab: 0,
            data: None,
        }
    }

    #[test]
    fn a_wide_panel_leaves_the_preferred_dock_width_alone() {
        assert_eq!(diff_dock_fit(880., 1920.).0, 880.);
    }

    #[test]
    fn opening_a_right_rail_shrinks_the_dock_instead_of_clipping_it() {
        let available = 970.;
        let (width, max) = diff_dock_fit(880., available);
        assert!(width < 880., "the dock must give ground: {width}");
        assert_eq!(width, max, "a clamped dock renders at its ceiling");
        assert!(
            width + PANE_GRID_RESERVED_WIDTH + crate::layout::PANE_GUTTER_PX <= available,
            "the dock still overflows the panel: {width}"
        );
    }

    #[test]
    fn a_panel_too_narrow_for_the_floor_stops_at_the_floor() {
        assert_eq!(
            diff_dock_fit(880., 200.),
            (DIFF_DOCK_PANEL_MIN_WIDTH, DIFF_DOCK_PANEL_MIN_WIDTH)
        );
    }

    #[test]
    fn a_session_that_never_opened_the_dock_parks_nothing() {
        assert!(slot(false, false, 0).is_idle());
    }

    #[test]
    fn a_dock_worth_restoring_is_parked() {
        assert!(!slot(true, false, 1).is_idle(), "an open dock must survive");
        assert!(
            !slot(false, true, 1).is_idle(),
            "an answered picker must not ask again"
        );
        assert!(
            !slot(false, false, 1).is_idle(),
            "a terminal / file tab must not be dropped"
        );
    }
}

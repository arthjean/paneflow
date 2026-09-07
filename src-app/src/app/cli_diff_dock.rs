use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled,
    Window, canvas, div, px,
};

use crate::PaneFlowApp;
use crate::app::diff_dock::code::controls::{EditorDisplay, editor_display, set_editor_display};
use crate::app::diff_dock::{DIFF_DOCK_PANEL_MIN_WIDTH, DiffDockData, DiffDockTab};

const PANE_GRID_RESERVED_WIDTH: f32 =
    crate::layout::MIN_PANE_SIZE + 2. * crate::layout::PANE_GUTTER_PX;

fn diff_dock_fit(preferred: f32, available: f32) -> (f32, f32) {
    let max = (available - PANE_GRID_RESERVED_WIDTH - crate::layout::PANE_GUTTER_PX)
        .max(DIFF_DOCK_PANEL_MIN_WIDTH);
    (preferred.min(max), max)
}

fn diff_dock_maximized_width(available: f32) -> f32 {
    (available - 2. * crate::layout::PANE_GUTTER_PX).max(DIFF_DOCK_PANEL_MIN_WIDTH)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PaneGridLayout {
    Flex,
    Clipped { visible: f32, full: f32 },
    Hidden,
}

impl PaneGridLayout {
    fn dock_left_gutter(self) -> f32 {
        let gutter = crate::layout::PANE_GUTTER_PX;
        match self {
            Self::Flex => 0.,
            Self::Clipped { visible, full } => gutter * (1. - visible / full.max(1.)).clamp(0., 1.),
            Self::Hidden => gutter,
        }
    }
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
    pub(crate) fn is_idle(&self) -> bool {
        !self.open && !self.picked && self.tabs.is_empty()
    }

    pub(crate) fn with_browsers(
        views: Vec<gpui::Entity<crate::browser::view::BrowserView>>,
    ) -> Self {
        Self {
            open: false,
            picker: false,
            picked: true,
            tabs: views.into_iter().map(DiffDockTab::Browser).collect(),
            active_tab: 0,
            data: None,
        }
    }

    pub(crate) fn with_browser(view: gpui::Entity<crate::browser::view::BrowserView>) -> Self {
        Self::with_browsers(vec![view])
    }

    pub(crate) fn set_active_tab(&mut self, index: usize) {
        self.active_tab = index.min(self.tabs.len().saturating_sub(1));
    }

    pub(crate) fn push_browser(&mut self, view: gpui::Entity<crate::browser::view::BrowserView>) {
        self.tabs.push(DiffDockTab::Browser(view));
        self.picked = true;
    }

    pub(crate) fn take_browsers(&mut self) -> Vec<gpui::Entity<crate::browser::view::BrowserView>> {
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for tab in std::mem::take(&mut self.tabs) {
            match tab {
                DiffDockTab::Browser(view) => taken.push(view),
                other => kept.push(other),
            }
        }
        self.tabs = kept;
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        taken
    }

    pub(crate) fn browser_descriptors(
        &self,
        cx: &gpui::App,
    ) -> Vec<paneflow_config::schema::BrowserDescriptor> {
        crate::app::browser_dock::browser_descriptors(&self.tabs, self.active_tab, cx)
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
        self.hide_diff_dock_browsers(cx);
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
            self.diff_dock.reveal_animation = None;
        }
    }

    pub(crate) fn toggle_cli_diff_dock(&mut self, cwd: String, cx: &mut Context<Self>) {
        let cwd = cwd.trim().to_string();
        let showing = self.diff_dock.open
            && self
                .diff_dock
                .data
                .as_ref()
                .is_none_or(|data| data.cwd == cwd);
        if showing {
            self.close_diff_dock_panel(cx);
        } else {
            self.diff_dock.picker = !self.diff_dock.picked;
            self.open_diff_dock_panel(cwd, cx);
        }
    }

    pub(crate) fn handle_toggle_diff_dock_maximize(
        &mut self,
        _: &crate::ToggleDiffDockMaximize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.diff_dock_visible() {
            return;
        }
        self.toggle_diff_dock_maximize(window, cx);
    }

    pub(crate) fn toggle_diff_dock_maximize(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_dock.resize = None;
        let now = std::time::Instant::now();
        let full = self.diff_dock.pane_grid_width.get();
        let from = match self.diff_dock.maximize_animation {
            Some(animation) => animation.width_at(now),
            None if self.diff_dock.maximized.is_some() => 0.,
            None => full,
        };
        let to = match self.diff_dock.maximized.take() {
            Some(previous_focus) => {
                match previous_focus {
                    Some(focus) => window.focus(&focus, cx),
                    None => {
                        if let Some(ws) = self.workspaces.get(self.active_idx) {
                            ws.focus_first(window, cx);
                        }
                    }
                }
                full
            }
            None => {
                self.diff_dock.maximized = Some(window.focused(cx));
                self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
                0.
            }
        };
        self.diff_dock.maximize_animation = if !crate::ui_primitives::reduce_motion()
            && (from - to).abs() > crate::PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA
        {
            Some(crate::SidebarWidthAnimation {
                from_width: from,
                to_width: to,
                started_at: now,
            })
        } else {
            None
        };
        cx.notify();
    }

    pub(crate) fn apply_editor_display(&mut self, cx: &mut Context<Self>) {
        let display = EditorDisplay::from_config(&self.cached_config.editor);
        if display == editor_display() {
            return;
        }
        set_editor_display(display);
        let live = self.diff_dock.diff_tabs.iter();
        let parked = self
            .diff_dock
            .parked
            .values()
            .flat_map(|slot| slot.tabs.iter());
        for tab in live.chain(parked) {
            if let DiffDockTab::File(view) = tab {
                view.update(cx, |_, cx| cx.notify());
            }
        }
    }

    pub(crate) fn diff_dock_fills_panel(&self) -> bool {
        self.diff_dock.maximized.is_some() || self.diff_dock.maximize_animation.is_some()
    }

    fn rendered_dock_reveal(&mut self, window: &mut Window) -> Option<f32> {
        let animation = self.diff_dock.reveal_animation?;
        let now = std::time::Instant::now();
        if animation.is_finished(now) {
            self.diff_dock.reveal_animation = None;
            return None;
        }
        window.request_animation_frame();
        Some(animation.width_at(now))
    }

    fn rendered_pane_grid_layout(&mut self, window: &mut Window) -> PaneGridLayout {
        let now = std::time::Instant::now();
        if let Some(animation) = self.diff_dock.maximize_animation {
            if animation.is_finished(now) {
                self.diff_dock.maximize_animation = None;
            } else {
                window.request_animation_frame();
                return PaneGridLayout::Clipped {
                    visible: animation.width_at(now),
                    full: animation.from_width.max(animation.to_width),
                };
            }
        }
        if self.diff_dock.maximized.is_some() {
            PaneGridLayout::Hidden
        } else {
            PaneGridLayout::Flex
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
        let grid = self.rendered_pane_grid_layout(window);
        let fills_panel = grid != PaneGridLayout::Flex;
        let (width, max_width) = if fills_panel {
            let width = diff_dock_maximized_width(available_width);
            (width, width)
        } else {
            diff_dock_fit(self.diff_dock.width, available_width)
        };
        let measured_grid_width = self.diff_dock.pane_grid_width.clone();
        let reveal = if fills_panel {
            None
        } else {
            self.rendered_dock_reveal(window)
        };
        let dock_column_width = width + crate::layout::PANE_GUTTER_PX;
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
            .map(|row| match grid {
                PaneGridLayout::Flex => row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    measured_grid_width.set(f32::from(bounds.size.width))
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .child(body),
                ),
                PaneGridLayout::Clipped { visible, full } => row.child(
                    div()
                        .flex_none()
                        .w(px(visible))
                        .h_full()
                        .overflow_hidden()
                        .child(div().w(px(full)).h_full().child(body)),
                ),
                PaneGridLayout::Hidden => row,
            })
            .child({
                let dock = div()
                    .map(|dock| {
                        if fills_panel {
                            dock.flex_1().min_w_0()
                        } else {
                            dock.flex_none()
                        }
                    })
                    .pl(px(grid.dock_left_gutter()))
                    .h_full()
                    .flex()
                    .flex_col()
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .child(self.render_diff_dock_panel(width, max_width, files_width, ui, cx));
                match reveal {
                    Some(progress) => div()
                        .relative()
                        .flex_none()
                        .h_full()
                        .w(px(dock_column_width * progress))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(dock_column_width))
                                .child(dock),
                        )
                        .into_any_element(),
                    None => dock.into_any_element(),
                }
            })
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
    fn a_maximized_dock_spans_the_panel_minus_the_gutters() {
        let available = 1920.;
        assert_eq!(
            diff_dock_maximized_width(available),
            available - 2. * crate::layout::PANE_GUTTER_PX
        );
        assert!(diff_dock_maximized_width(available) > diff_dock_fit(880., available).1);
    }

    #[test]
    fn the_dock_gutter_grows_as_the_pane_grid_is_clipped_away() {
        let gutter = crate::layout::PANE_GUTTER_PX;
        assert_eq!(PaneGridLayout::Flex.dock_left_gutter(), 0.);
        assert_eq!(PaneGridLayout::Hidden.dock_left_gutter(), gutter);
        let half = PaneGridLayout::Clipped {
            visible: 400.,
            full: 800.,
        };
        assert!((half.dock_left_gutter() - gutter / 2.).abs() < 1e-3);
        let gone = PaneGridLayout::Clipped {
            visible: 0.,
            full: 800.,
        };
        assert_eq!(gone.dock_left_gutter(), gutter);
    }

    #[test]
    fn a_maximized_dock_never_drops_below_the_floor() {
        assert_eq!(diff_dock_maximized_width(200.), DIFF_DOCK_PANEL_MIN_WIDTH);
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

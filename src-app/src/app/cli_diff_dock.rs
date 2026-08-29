//! Right-docked git diff for the CLI cockpit.
//!
//! The trailing `layout-sidebar-right` button of a pane header toggles the side
//! dock ([`crate::app::diff_dock`]) on that pane's *workspace folder*. The
//! dock hosts three surfaces - the working-tree diff against `HEAD`, a shell,
//! and open files - and asks which one the first time a workspace opens it
//! (`diff_dock::surface_picker`). This module owns the CLI plumbing only - the
//! toggle, the per-workspace attachment, and the dock host; the panel itself is
//! rendered once by [`crate::app::diff_dock`].
//!
//! The dock is *detached per session* (workspace tab): it belongs to the tab it
//! was opened in, so switching tab - or workspace, which switches tab too -
//! parks it and brings up whatever the incoming session last had: nothing at
//! all, until that session opens the dock itself and answers the picker. A
//! sibling tab of the same folder is a different session and starts clean.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, div,
    px,
};

use crate::PaneFlowApp;
use crate::app::diff_dock::{DIFF_DOCK_PANEL_MIN_WIDTH, DiffDockData, DiffDockTab};

/// Width the pane grid keeps beside the dock: one minimum pane plus the gutter
/// on each of its sides, so a dock wider than the panel gives ground instead of
/// pushing its own right edge past the clip.
const PANE_GRID_RESERVED_WIDTH: f32 =
    crate::layout::MIN_PANE_SIZE + 2. * crate::layout::PANE_GUTTER_PX;

/// How the dock fits inside `available` px of main panel: `(render, max)`.
///
/// The stored width is a *preference*, not a layout fact: opening a right rail
/// (Files, Sessions) narrows the panel under a dock that was sized for the wide
/// one. Clamping at render rather than writing the preference back means the
/// dock returns to its full width when the rail closes. The ceiling travels
/// with it so a resize drag started under a rail cannot store a width the panel
/// could not show.
fn diff_dock_fit(preferred: f32, available: f32) -> (f32, f32) {
    let max = (available - PANE_GRID_RESERVED_WIDTH - crate::layout::PANE_GUTTER_PX)
        .max(DIFF_DOCK_PANEL_MIN_WIDTH);
    (preferred.min(max), max)
}

/// The dock state one session owns, parked while another session is on screen.
///
/// Only what makes the dock *this session's* dock is carried: whether it is
/// open, which surface was picked, the tabs it accumulated and the last diff
/// snapshot (kept so returning repaints from warm rows instead of flashing a
/// loader while git re-runs). Width, split/unified layout and the fold state
/// stay app-global - they are preferences about how a diff reads, not facts
/// about which session is on screen.
pub(crate) struct DiffDockSlot {
    open: bool,
    picker: bool,
    picked: bool,
    tabs: Vec<DiffDockTab>,
    active_tab: usize,
    data: Option<DiffDockData>,
}

impl DiffDockSlot {
    /// Nothing worth keeping: the session never opened the dock (or was left at
    /// its birth state), so parking it would only grow the map with slots
    /// indistinguishable from a fresh one.
    fn is_idle(&self) -> bool {
        !self.open && !self.picked && self.tabs.len() <= 1
    }
}

impl PaneFlowApp {
    /// Keep the live dock attached to the session on screen.
    ///
    /// Called once per frame from [`Self::wrap_cli_diff_dock`] rather than from
    /// each of the places the visible tab moves (sidebar click, `Ctrl+1..9`,
    /// tab switch / create / close / reorder, cross-workspace tab move,
    /// workspace create / close / restore, IPC `workspace.select`, Settings):
    /// the dock follows one fact - which session is visible - so it reconciles
    /// against that fact instead of asking every caller to remember it.
    fn sync_diff_dock_session(&mut self, cx: &mut Context<Self>) {
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

    /// Id of the session (workspace tab) the cockpit is showing. Tab ids come
    /// from one process-global counter, so a tab id is unique across every
    /// workspace and needs no workspace half to key on.
    pub(crate) fn active_session_id(&self) -> Option<u64> {
        self.active_workspace().map(|ws| ws.active_tab().id)
    }

    /// Drop the slots of sessions that no longer exist. A tab closed while its
    /// dock was parked never comes back, so its slot has to go with it or the
    /// terminals and documents it holds stay alive with nothing on screen.
    pub(crate) fn prune_parked_diff_docks(&mut self) {
        let workspaces = &self.workspaces;
        self.diff_dock.parked.retain(|id, _| {
            workspaces
                .iter()
                .flat_map(|ws| ws.tabs())
                .any(|tab| tab.id == *id)
        });
    }

    /// Move the live dock fields into `owner`'s slot and reset them to the
    /// state a session that has never opened the dock sees.
    fn park_live_diff_dock(&mut self, owner: Option<u64>, cx: &mut Context<Self>) {
        let slot = DiffDockSlot {
            open: self.diff_dock.open,
            picker: self.diff_dock.picker,
            picked: self.diff_dock.picked,
            tabs: std::mem::replace(&mut self.diff_dock.diff_tabs, vec![DiffDockTab::Changes]),
            active_tab: std::mem::replace(&mut self.diff_dock.diff_active_tab, 0),
            data: self.diff_dock.data.take(),
        };
        // Everything the parked dock left behind: the closer already drops the
        // snapshot state (folds, scroll, horizontal offsets) and the live
        // drags, and the menus below describe a strip that is no longer here.
        self.close_diff_dock_panel(cx);
        self.diff_dock.picker = false;
        self.diff_dock.picked = false;
        self.diff_dock.diff_tab_close_armed = None;
        self.diff_dock.diff_options_menu_open = false;
        self.diff_dock.diff_layout_submenu_open = false;
        self.diff_dock.diff_new_tab_menu_open = false;
        self.diff_dock.diff_branch_menu = None;

        let owner = owner.filter(|id| {
            self.workspaces
                .iter()
                .flat_map(|ws| ws.tabs())
                .any(|tab| tab.id == *id)
        });
        match owner {
            // The owner is gone (its tab was just closed): dropping the slot
            // here is that session's dock teardown.
            None => drop(slot),
            Some(id) if slot.is_idle() => {
                self.diff_dock.parked.remove(&id);
            }
            Some(id) => {
                self.diff_dock.parked.insert(id, slot);
            }
        }
    }

    /// Bring `session_id`'s parked dock back. A session with no slot keeps the
    /// closed dock the parking reset left, which is the whole point: opening a
    /// dock in one session must not open one in the next.
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
            // Reopening on the snapshot's own folder, not the workspace root:
            // the two are the same today, and asking the data keeps the warm
            // snapshot valid if a dock is ever opened on a subfolder.
            let cwd = cwd
                .or_else(|| self.active_workspace().map(|ws| ws.cwd.clone()))
                .unwrap_or_default();
            self.open_diff_dock_panel(cwd, cx);
        }
    }

    /// Pane-header button handler: close the dock when it already shows this
    /// folder, otherwise (re)open it there.
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
            // The button opens the dock, not the diff: until this session has
            // said once what it wants in it, the dock comes up on its surface
            // picker. Afterwards it restores whatever tab was last active there.
            self.diff_dock.picker = !self.diff_dock.picked;
            self.open_diff_dock_panel(cwd, cx);
        }
    }

    /// Whether the dock is actually on screen.
    ///
    /// `open` alone is not enough: the flag survives a mode switch
    /// and a trip through Settings, both of which unmount the dock in
    /// [`Self::wrap_cli_diff_dock`]. Anything that acts *on* the dock without
    /// putting it back on screen has to ask this instead, or it mutates a strip
    /// nobody can see.
    pub(crate) fn diff_dock_visible(&self) -> bool {
        self.diff_dock.open
            && self.settings_section.is_none()
            && matches!(self.mode, paneflow_config::schema::AppMode::Cli)
    }

    /// Dock the diff panel to the right of the CLI pane grid when it is open.
    /// The resize / horizontal-scrollbar drags are captured on this wrapper (a
    /// full-height surface) so a drag keeps tracking once the cursor outruns its
    /// handle and crosses into the panes beside it.
    pub(crate) fn wrap_cli_diff_dock(
        &mut self,
        body: AnyElement,
        available_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Before the visibility test, not after: a session whose parked dock is
        // open has to be swapped in to *become* visible.
        self.sync_diff_dock_session(cx);
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
            // The pane grid already pads its own right edge, so the dock only
            // has to reproduce the other three gutters to sit on the same
            // margins as the cards it docks beside.
            .child(
                div()
                    .flex_none()
                    .h_full()
                    .flex()
                    .flex_col()
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .child(self.render_diff_dock_panel(width, max_width, ui, cx)),
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
        // 1920px of panel: the dock has no reason to give ground, and the
        // preference is what the user dragged it to.
        assert_eq!(diff_dock_fit(880., 1920.).0, 880.);
    }

    #[test]
    fn opening_a_right_rail_shrinks_the_dock_instead_of_clipping_it() {
        // A 300px rail over a 1280px window leaves ~970px of panel. The dock
        // must fit inside it with the pane grid's reserve, not overflow the
        // panel's clip by the difference.
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
        // Past this point something has to be clipped; the dock stays readable
        // rather than collapsing to a sliver.
        assert_eq!(
            diff_dock_fit(880., 200.),
            (DIFF_DOCK_PANEL_MIN_WIDTH, DIFF_DOCK_PANEL_MIN_WIDTH)
        );
    }

    #[test]
    fn a_session_that_never_opened_the_dock_parks_nothing() {
        // The birth state. Parking it would make "this session has a slot"
        // stop meaning "this session has a dock", and every tab merely visited
        // once would grow the map.
        assert!(slot(false, false, 1).is_idle());
    }

    #[test]
    fn a_dock_worth_restoring_is_parked() {
        // Open, or answered, or carrying tabs: each on its own is state the
        // session must find again when it comes back.
        assert!(!slot(true, false, 1).is_idle(), "an open dock must survive");
        assert!(
            !slot(false, true, 1).is_idle(),
            "an answered picker must not ask again"
        );
        assert!(
            !slot(false, false, 2).is_idle(),
            "a terminal / file tab must not be dropped"
        );
    }
}

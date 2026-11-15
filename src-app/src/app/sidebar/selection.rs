use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt, Corners, Entity, Hsla, Pixels, Point};

use super::*;
use crate::app::close_policy::{CloseIntent, CloseTarget, close_target_tabs};
use crate::app::diff_dock::code::view::CodeView;
use crate::app::menu_icons as icon;
use crate::app::unsaved_dialog::UnsavedContinuation;
use crate::settings::components::{
    ModalKey, confirmation_list, confirmation_warning, destructive_button, modal_backdrop,
    modal_card, modal_footer, modal_header, modal_key, secondary_button,
};
use crate::ui_primitives::action_menu::{ActionMenu, MenuItem};
use crate::ui_primitives::squircle::{squircle_corners_border, squircle_corners_fill};
use crate::ui_primitives::{ease_out_cubic, lerp_color};

const SELECTION_FADE_SECONDS: f32 = 0.15;
const SELECTION_LEAVE: Duration = Duration::from_millis(180);
const SELECTION_REMOVAL_DELAY: Duration = Duration::from_millis(200);
const SELECTION_FILL_ALPHA: f32 = 0.16;
const SELECTION_DIALOG_WIDTH: Pixels = px(460.);
const SELECTION_DIALOG_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;
const SELECTION_DIALOG_MAX_ROWS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SidebarPick {
    Workspace(u64),
    Tab(TabKey),
}

impl SidebarPick {
    fn workspace_id(self) -> u64 {
        match self {
            Self::Workspace(id) => id,
            Self::Tab(key) => key.workspace_id,
        }
    }
}

fn picks_cover(picks: &[SidebarPick], pick: SidebarPick) -> bool {
    picks.contains(&pick)
        || matches!(pick, SidebarPick::Tab(key)
            if picks.contains(&SidebarPick::Workspace(key.workspace_id)))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SidebarSelection {
    picks: Vec<SidebarPick>,
    anchor: Option<SidebarPick>,
}

impl SidebarSelection {
    pub(super) fn is_empty(&self) -> bool {
        self.picks.is_empty()
    }

    pub(super) fn picks(&self) -> &[SidebarPick] {
        &self.picks
    }

    pub(super) fn covers(&self, pick: SidebarPick) -> bool {
        picks_cover(&self.picks, pick)
    }

    fn clear(&mut self) -> bool {
        self.anchor = None;
        let had_picks = !self.picks.is_empty();
        self.picks.clear();
        had_picks
    }

    fn toggle(&mut self, pick: SidebarPick, tree: &[(u64, Vec<u64>)]) {
        let held = self.picks.iter().position(|held| *held == pick);
        match (pick, held) {
            (_, Some(at)) => {
                self.picks.remove(at);
            }
            (SidebarPick::Workspace(id), None) => {
                self.picks.retain(|held| held.workspace_id() != id);
                self.picks.push(pick);
            }
            (SidebarPick::Tab(key), None) => {
                let workspace = SidebarPick::Workspace(key.workspace_id);
                match self.picks.iter().position(|held| *held == workspace) {
                    Some(at) => {
                        self.picks.remove(at);
                        let siblings = tree
                            .iter()
                            .find(|(id, _)| *id == key.workspace_id)
                            .map_or(&[][..], |(_, tabs)| tabs.as_slice());
                        self.picks.extend(
                            siblings.iter().filter(|tab_id| **tab_id != key.tab_id).map(
                                |&tab_id| {
                                    SidebarPick::Tab(TabKey {
                                        workspace_id: key.workspace_id,
                                        tab_id,
                                    })
                                },
                            ),
                        );
                    }
                    None => self.picks.push(pick),
                }
            }
        }
        self.anchor = Some(pick);
    }

    fn select_range(
        &mut self,
        fallback_anchor: Option<SidebarPick>,
        target: SidebarPick,
        rows: &[SidebarPick],
    ) {
        let Some(end) = rows.iter().position(|row| *row == target) else {
            return;
        };
        let anchor = self
            .anchor
            .or(fallback_anchor)
            .filter(|anchor| rows.contains(anchor))
            .unwrap_or(target);
        let start = rows.iter().position(|row| *row == anchor).unwrap_or(end);
        let span = &rows[start.min(end)..=start.max(end)];
        let whole: Vec<u64> = span
            .iter()
            .filter_map(|row| match row {
                SidebarPick::Workspace(id) => Some(*id),
                SidebarPick::Tab(_) => None,
            })
            .filter(|id| {
                let endpoint = SidebarPick::Workspace(*id);
                endpoint == anchor
                    || endpoint == target
                    || rows
                        .iter()
                        .filter(
                            |row| matches!(row, SidebarPick::Tab(key) if key.workspace_id == *id),
                        )
                        .all(|row| span.contains(row))
            })
            .collect();
        self.picks = span
            .iter()
            .copied()
            .filter(|row| match row {
                SidebarPick::Workspace(id) => whole.contains(id),
                SidebarPick::Tab(key) => !whole.contains(&key.workspace_id),
            })
            .collect();
        self.anchor = Some(anchor);
    }

    fn retain_live(&mut self, tree: &[(u64, Vec<u64>)], folders_shown: bool) {
        let live = |pick: SidebarPick| match pick {
            SidebarPick::Workspace(id) => folders_shown && tree.iter().any(|(ws, _)| *ws == id),
            SidebarPick::Tab(key) => tree
                .iter()
                .any(|(ws, tabs)| *ws == key.workspace_id && tabs.contains(&key.tab_id)),
        };
        self.picks.retain(|pick| live(*pick));
        if self.anchor.is_some_and(|anchor| !live(anchor)) {
            self.anchor = None;
        }
    }
}

fn selection_counts(picks: &[SidebarPick]) -> (usize, usize) {
    let workspaces = picks
        .iter()
        .filter(|pick| matches!(pick, SidebarPick::Workspace(_)))
        .count();
    (workspaces, picks.len() - workspaces)
}

fn selection_summary(workspaces: usize, tabs: usize) -> String {
    let workspaces_part = crate::app::plural(workspaces, "workspace", "workspaces");
    let tabs_part = crate::app::plural(tabs, "tab", "tabs");
    match (workspaces, tabs) {
        (_, 0) => workspaces_part,
        (0, _) => tabs_part,
        _ => format!("{workspaces_part} and {tabs_part}"),
    }
}

fn stopped_sessions_summary(sessions: usize) -> String {
    match sessions {
        0 => "Nothing is running in them.".to_string(),
        1 => "1 session will stop.".to_string(),
        count => format!("{count} sessions will stop."),
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Fade {
    on: bool,
    from: f32,
    started: Option<Instant>,
}

impl Fade {
    fn value_at(&self, now: Instant, seconds: f32) -> f32 {
        let to = if self.on { 1. } else { 0. };
        let Some(started) = self.started else {
            return to;
        };
        let progress = (now.duration_since(started).as_secs_f32() / seconds).min(1.);
        self.from + (to - self.from) * ease_out_cubic(progress)
    }

    fn is_animating(&self, now: Instant, seconds: f32) -> bool {
        self.started
            .is_some_and(|started| now.duration_since(started).as_secs_f32() < seconds)
    }

    fn retarget(&mut self, on: bool, now: Instant, seconds: f32) {
        if self.on != on {
            self.from = self.value_at(now, seconds);
            self.on = on;
            self.started = (!crate::ui_primitives::reduce_motion()).then_some(now);
        }
        if !self.is_animating(now, seconds) {
            self.started = None;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct SelectionShape {
    pub(super) picked: f32,
    above: f32,
    below: f32,
}

impl SelectionShape {
    fn corners(self) -> Corners<Pixels> {
        let radius = rail_row_radius();
        let top = radius * (1. - self.above);
        let bottom = radius * (1. - self.below);
        Corners {
            top_left: top,
            top_right: top,
            bottom_right: bottom,
            bottom_left: bottom,
        }
    }

    fn joined(self) -> bool {
        self.above > 0. || self.below > 0.
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SelectionSeam {
    above: SidebarPick,
    below: SidebarPick,
    inset: f32,
}

pub(super) fn sidebar_seam_inset(
    above: SidebarRow,
    below: SidebarRow,
    indent_guide: bool,
) -> Option<f32> {
    let tab_inset = if indent_guide {
        SIDEBAR_FOLDER_SLOT_WIDTH + SIDEBAR_TITLE_ROW_GAP
    } else {
        0.
    };
    match (above, below) {
        (SidebarRow::Tab(..), SidebarRow::Folder(_)) => None,
        (SidebarRow::Folder(_), SidebarRow::Folder(_)) => Some(0.),
        (SidebarRow::Folder(_), SidebarRow::Tab(..)) => (!indent_guide).then_some(0.),
        (SidebarRow::Tab(..), SidebarRow::Tab(..)) => Some(tab_inset),
    }
}

#[derive(Default)]
pub(crate) struct SidebarSelectionMotion {
    rows: HashMap<SidebarPick, Fade>,
    neighbors: HashMap<SidebarPick, (Option<SidebarPick>, Option<SidebarPick>)>,
}

impl SidebarSelectionMotion {
    fn join(&mut self, seams: &[SelectionSeam]) {
        self.neighbors.clear();
        for seam in seams {
            self.neighbors.entry(seam.above).or_default().1 = Some(seam.below);
            self.neighbors.entry(seam.below).or_default().0 = Some(seam.above);
        }
    }

    fn row(&mut self, pick: SidebarPick, on: bool, now: Instant) -> f32 {
        if !on && !self.rows.contains_key(&pick) {
            return 0.;
        }
        let fade = self.rows.entry(pick).or_default();
        fade.retarget(on, now, SELECTION_FADE_SECONDS);
        let value = fade.value_at(now, SELECTION_FADE_SECONDS);
        if !on && fade.started.is_none() {
            self.rows.remove(&pick);
        }
        value
    }

    fn is_animating(&self, now: Instant) -> bool {
        self.rows
            .values()
            .any(|fade| fade.is_animating(now, SELECTION_FADE_SECONDS))
    }

    fn forget_unless(&mut self, shown: impl Fn(SidebarPick) -> bool, now: Instant) {
        self.rows
            .retain(|pick, fade| shown(*pick) || fade.is_animating(now, SELECTION_FADE_SECONDS));
    }
}

struct SelectionCloseDialog {
    picks: Vec<SidebarPick>,
    focused: bool,
    return_focus: crate::FocusReturn,
}

pub(crate) struct SidebarSelectionState {
    selection: SidebarSelection,
    motion: std::cell::RefCell<SidebarSelectionMotion>,
    leaving: Vec<SidebarPick>,
    cursor_ring_hidden: bool,
    menu: Option<Point<Pixels>>,
    dialog: Option<SelectionCloseDialog>,
    dialog_focus: gpui::FocusHandle,
}

impl SidebarSelectionState {
    pub(crate) fn new(dialog_focus: gpui::FocusHandle) -> Self {
        Self {
            selection: SidebarSelection::default(),
            motion: Default::default(),
            leaving: Vec::new(),
            cursor_ring_hidden: false,
            menu: None,
            dialog: None,
            dialog_focus,
        }
    }

    pub(super) fn reveal_cursor_ring(&mut self) {
        self.cursor_ring_hidden = false;
    }

    pub(crate) fn menu(&self) -> Option<Point<Pixels>> {
        self.menu.filter(|_| !self.selection.is_empty())
    }

    pub(crate) fn close_menu(&mut self) {
        self.menu = None;
    }

    pub(crate) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.selection.is_empty()
    }
}

#[derive(Clone)]
pub(crate) enum SelectionMenuAction {
    Close,
    Clear,
}

fn selection_tint(ui: crate::theme::UiColors, amount: f32) -> Hsla {
    with_alpha(ui.accent, SELECTION_FILL_ALPHA * amount)
}

pub(super) fn sidebar_row_skin(
    element: gpui::Stateful<gpui::Div>,
    group: SharedString,
    shape: SelectionShape,
    resting: Option<Hsla>,
    hovered: Option<Hsla>,
) -> gpui::Stateful<gpui::Div> {
    if !shape.joined() {
        return squircle_skin(element, group, rail_row_radius(), resting, hovered);
    }
    let corners = shape.corners();
    let mut element = element.relative().group(group.clone());
    if let Some(resting) = resting {
        element = element.child(squircle_corners_fill(corners, resting));
    }
    if let Some(hovered) = hovered {
        element = element.child(
            div()
                .absolute()
                .inset_0()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(squircle_corners_fill(corners, hovered)),
        );
    }
    element
}

pub(super) fn sidebar_selection_fill(
    ui: crate::theme::UiColors,
    shape: SelectionShape,
) -> AnyElement {
    squircle_corners_fill(shape.corners(), selection_tint(ui, shape.picked))
}

pub(super) fn sidebar_row_title_color(ui: crate::theme::UiColors, picked: f32) -> gpui::Hsla {
    lerp_color(ui.muted, ui.text, picked)
}

fn sidebar_seam_bridge(ui: crate::theme::UiColors, inset: f32, joined: f32) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(SIDEBAR_ROW_MARGIN_X + inset))
        .right(px(SIDEBAR_ROW_MARGIN_X))
        .bg(selection_tint(ui, joined))
}

pub(super) fn sidebar_leaving_row(tab_id: u64, row: gpui::Div, spacing: f32) -> AnyElement {
    let height = spacing + SIDEBAR_ROW_PADDING_Y * 2. + SIDEBAR_ROW_LINE_HEIGHT;
    row.overflow_hidden()
        .with_animation(
            SharedString::from(format!("sidebar-leaving-row-{tab_id}")),
            Animation::new(SELECTION_LEAVE).with_easing(ease_out_cubic),
            move |row, delta| row.opacity(1. - delta).h(px(height * (1. - delta))),
        )
        .into_any_element()
}

impl PaneFlowApp {
    fn sidebar_tree(&self) -> Vec<(u64, Vec<u64>)> {
        sidebar_motion_topology(&self.workspaces, &self.sidebar_display_order())
    }

    pub(super) fn sidebar_pick_for(&self, row: SidebarRow) -> Option<SidebarPick> {
        match row {
            SidebarRow::Folder(ws_idx) => self
                .workspaces
                .get(ws_idx)
                .map(|ws| SidebarPick::Workspace(ws.id)),
            SidebarRow::Tab(ws_idx, tab_idx) => self
                .workspaces
                .get(ws_idx)
                .and_then(|ws| TabKey::of(ws, tab_idx))
                .map(SidebarPick::Tab),
        }
    }

    fn sidebar_visible_picks(&self, cx: &gpui::App) -> Vec<SidebarPick> {
        let query = self
            .sidebar_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        self.sidebar_rows(&query, cx)
            .into_iter()
            .filter_map(|row| self.sidebar_pick_for(row))
            .collect()
    }

    pub(crate) fn prune_sidebar_selection(&mut self) {
        if self.sidebar_selection.selection.is_empty() {
            return;
        }
        let tree = self.sidebar_tree();
        let folders_shown = !self.grouped_by_status();
        self.sidebar_selection
            .selection
            .retain_live(&tree, folders_shown);
    }

    fn sidebar_pick_progress(&self, pick: SidebarPick) -> f32 {
        let state = &self.sidebar_selection;
        let on = state.selection.covers(pick) || picks_cover(&state.leaving, pick);
        state.motion.borrow_mut().row(pick, on, Instant::now())
    }

    pub(super) fn sidebar_pick_shape(&self, pick: SidebarPick) -> SelectionShape {
        let picked = self.sidebar_pick_progress(pick);
        if picked <= 0. {
            return SelectionShape::default();
        }
        let (above, below) = self
            .sidebar_selection
            .motion
            .borrow()
            .neighbors
            .get(&pick)
            .copied()
            .unwrap_or_default();
        let joined = |neighbor: Option<SidebarPick>| {
            neighbor.map_or(0., |neighbor| {
                picked.min(self.sidebar_pick_progress(neighbor))
            })
        };
        SelectionShape {
            picked,
            above: joined(above),
            below: joined(below),
        }
    }

    pub(super) fn sidebar_selection_seam(
        &self,
        above: SidebarRow,
        below: SidebarRow,
        inset: Option<f32>,
    ) -> Option<SelectionSeam> {
        Some(SelectionSeam {
            above: self.sidebar_pick_for(above)?,
            below: self.sidebar_pick_for(below)?,
            inset: inset?,
        })
    }

    pub(super) fn join_sidebar_selection(&self, seams: &[Option<SelectionSeam>]) {
        let state = &self.sidebar_selection;
        let mut motion = state.motion.borrow_mut();
        if state.selection.is_empty() && state.leaving.is_empty() && motion.rows.is_empty() {
            motion.neighbors.clear();
            return;
        }
        let seams: Vec<SelectionSeam> = seams.iter().flatten().copied().collect();
        motion.join(&seams);
    }

    pub(super) fn render_sidebar_seam(
        &self,
        seam: Option<SelectionSeam>,
        ui: crate::theme::UiColors,
    ) -> Option<gpui::Div> {
        let seam = seam?;
        let joined = self
            .sidebar_pick_progress(seam.above)
            .min(self.sidebar_pick_progress(seam.below));
        (joined > 0.).then(|| sidebar_seam_bridge(ui, seam.inset, joined))
    }

    pub(super) fn sidebar_cursor_ring_for(
        &self,
        shape: SelectionShape,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        (!self.sidebar_selection.cursor_ring_hidden).then(|| {
            squircle_corners_border(shape.corners(), px(1.), ui.text.opacity(0.35))
                .into_any_element()
        })
    }

    pub(super) fn sidebar_row_leaving(&self, row: SidebarRow) -> bool {
        let leaving = &self.sidebar_selection.leaving;
        !leaving.is_empty()
            && self
                .sidebar_pick_for(row)
                .is_some_and(|pick| picks_cover(leaving, pick))
    }

    pub(crate) fn clear_sidebar_selection(&mut self, cx: &mut Context<Self>) -> bool {
        self.sidebar_selection.menu = None;
        let cleared = self.sidebar_selection.selection.clear();
        if cleared {
            cx.notify();
        }
        cleared
    }

    pub(super) fn sidebar_selection_click(
        &mut self,
        row: SidebarRow,
        modifiers: gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let toggles = modifiers.secondary(cx);
        if !toggles && !modifiers.shift {
            self.clear_sidebar_selection(cx);
            return false;
        }
        let Some(pick) = self.sidebar_pick_for(row) else {
            return false;
        };
        self.commit_rename(cx);
        self.dismiss_transient_surfaces();
        if toggles {
            let tree = self.sidebar_tree();
            self.sidebar_selection.selection.toggle(pick, &tree);
        } else {
            let active = self
                .shown_workspace_idx()
                .map(|ws_idx| SidebarRow::Tab(ws_idx, self.workspaces[ws_idx].active_tab_idx()));
            self.select_sidebar_range(active, row, cx);
        }
        self.place_sidebar_cursor(row, window, cx);
        self.sidebar_selection.cursor_ring_hidden = true;
        cx.notify();
        true
    }

    pub(super) fn select_sidebar_range(
        &mut self,
        fallback_anchor: Option<SidebarRow>,
        target: SidebarRow,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.sidebar_pick_for(target) else {
            return;
        };
        let rows = self.sidebar_visible_picks(cx);
        let fallback_anchor = fallback_anchor.and_then(|row| self.sidebar_pick_for(row));
        self.sidebar_selection
            .selection
            .select_range(fallback_anchor, target, &rows);
        cx.notify();
    }

    pub(super) fn open_sidebar_selection_menu(
        &mut self,
        row: SidebarRow,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let selected = self
            .sidebar_pick_for(row)
            .is_some_and(|pick| self.sidebar_selection.selection.covers(pick));
        if !selected {
            self.clear_sidebar_selection(cx);
            return false;
        }
        self.commit_rename(cx);
        self.dismiss_transient_surfaces();
        self.place_sidebar_cursor(row, window, cx);
        self.sidebar_selection.cursor_ring_hidden = true;
        self.sidebar_selection.menu = Some(position);
        cx.notify();
        true
    }

    pub(crate) fn render_sidebar_selection_menu(
        &self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (workspaces, tabs) = selection_counts(self.sidebar_selection.selection.picks());
        let menu = ActionMenu::new()
            .item(
                MenuItem::new(
                    format!("Close {}", selection_summary(workspaces, tabs)),
                    SelectionMenuAction::Close,
                )
                .icon(icon::CLOSE),
            )
            .separator()
            .item(MenuItem::new("Clear selection", SelectionMenuAction::Clear).mnemonic('c'));
        let handlers = self.menu_handlers(
            cx,
            |this, action: &SelectionMenuAction, window, cx| {
                this.sidebar_selection.menu = None;
                match action {
                    SelectionMenuAction::Close => this.request_close_sidebar_selection(window, cx),
                    SelectionMenuAction::Clear => {
                        this.clear_sidebar_selection(cx);
                    }
                }
            },
            |this| this.sidebar_selection.menu = None,
        );
        context_menu::render_context_menu(
            "sidebar-selection-menu",
            menu,
            position,
            handlers,
            window,
            cx,
        )
    }

    pub(super) fn advance_sidebar_selection_motion(&self, window: &mut Window) {
        let state = &self.sidebar_selection;
        let now = Instant::now();
        let mut motion = state.motion.borrow_mut();
        motion.forget_unless(
            |pick| state.selection.covers(pick) || picks_cover(&state.leaving, pick),
            now,
        );
        if motion.is_animating(now) {
            window.request_animation_frame();
        }
    }

    fn sidebar_pick_target(&self, pick: SidebarPick) -> Option<CloseTarget> {
        match pick {
            SidebarPick::Workspace(id) => self
                .workspaces
                .iter()
                .position(|ws| ws.id == id)
                .map(CloseTarget::Workspace),
            SidebarPick::Tab(key) => key
                .resolve(&self.workspaces)
                .map(|(ws_idx, tab_idx)| CloseTarget::Tab { ws_idx, tab_idx }),
        }
    }

    pub(crate) fn sidebar_pick_tabs(&self, picks: &[SidebarPick]) -> Vec<u64> {
        picks
            .iter()
            .filter_map(|pick| self.sidebar_pick_target(*pick))
            .flat_map(|target| close_target_tabs(&self.workspaces, &target))
            .collect()
    }

    fn sidebar_pick_row(&self, pick: SidebarPick) -> Option<(SharedString, SharedString)> {
        match pick {
            SidebarPick::Workspace(id) => self
                .workspaces
                .iter()
                .find(|ws| ws.id == id)
                .map(|ws| (SharedString::from(ws.title.clone()), "workspace".into())),
            SidebarPick::Tab(key) => {
                let (ws_idx, tab_idx) = key.resolve(&self.workspaces)?;
                let ws = &self.workspaces[ws_idx];
                let title = format!(
                    "{} \u{203a} {}",
                    ws.title,
                    tab_display_title(&ws.tabs()[tab_idx], tab_idx)
                );
                Some((SharedString::from(title), "tab".into()))
            }
        }
    }

    pub(crate) fn request_close_sidebar_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_selection.menu = None;
        if !self.sidebar_selection.leaving.is_empty() || self.sidebar_selection.dialog.is_some() {
            return;
        }
        self.prune_sidebar_selection();
        let picks = self.sidebar_selection.selection.picks().to_vec();
        match picks.len() {
            0 => {}
            1 => self.close_sidebar_picks(picks, window, cx),
            _ => {
                self.dismiss_transient_surfaces();
                self.sidebar_selection.dialog = Some(SelectionCloseDialog {
                    picks,
                    focused: false,
                    return_focus: crate::FocusReturn::default(),
                });
                cx.notify();
            }
        }
    }

    fn close_sidebar_picks(
        &mut self,
        picks: Vec<SidebarPick>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unsaved: Vec<Entity<CodeView>> = picks
            .iter()
            .filter_map(|pick| self.sidebar_pick_target(*pick))
            .flat_map(|target| self.unsaved_views_for_close(&target, cx))
            .collect();
        if self.ask_about_unsaved(
            unsaved,
            UnsavedContinuation::CloseSelection(picks.clone()),
            cx,
        ) {
            return;
        }
        self.close_sidebar_picks_checked(picks, Vec::new(), window, cx);
    }

    pub(crate) fn close_sidebar_picks_checked(
        &mut self,
        picks: Vec<SidebarPick>,
        discard: Vec<Entity<CodeView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for view in discard {
            view.update(cx, |view, _| view.discard_unsaved());
        }
        self.sidebar_selection.selection.clear();
        if crate::ui_primitives::reduce_motion() {
            self.remove_sidebar_picks(&picks, window, cx);
            return;
        }
        self.sidebar_selection.leaving = picks.clone();
        cx.notify();
        let window_handle = window.window_handle();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                cx.background_executor()
                    .timer(SELECTION_REMOVAL_DELAY)
                    .await;
                let _ = window_handle.update(cx, |_, window, cx| {
                    let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.remove_sidebar_picks(&picks, window, cx);
                    });
                });
            },
        )
        .detach();
    }

    fn remove_sidebar_picks(
        &mut self,
        picks: &[SidebarPick],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_selection.leaving.clear();
        let mut closed_tabs = 0;
        let mut closed_workspaces = 0;
        for pick in picks {
            if let SidebarPick::Tab(key) = pick
                && let Some((ws_idx, tab_idx)) = key.resolve(&self.workspaces)
            {
                self.perform_close(
                    CloseTarget::Tab { ws_idx, tab_idx },
                    CloseIntent::Hold,
                    Some(&mut *window),
                    cx,
                );
                closed_tabs += 1;
            }
        }
        for pick in picks {
            if let SidebarPick::Workspace(id) = pick
                && let Some(idx) = self.workspaces.iter().position(|ws| ws.id == *id)
            {
                self.perform_close(
                    CloseTarget::Workspace(idx),
                    CloseIntent::Hold,
                    Some(&mut *window),
                    cx,
                );
                closed_workspaces += 1;
            }
        }
        if closed_tabs + closed_workspaces > 1 {
            self.show_bulk_closed_toast(
                format!(
                    "{} closed",
                    selection_summary(closed_workspaces, closed_tabs)
                ),
                cx,
            );
        }
        cx.notify();
    }

    fn dismiss_selection_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.sidebar_selection.dialog.take() {
            self.return_focus(&dialog.return_focus, window, cx);
            cx.notify();
        }
    }

    fn confirm_selection_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.sidebar_selection.dialog.take() else {
            return;
        };
        self.return_focus(&dialog.return_focus, window, cx);
        let picks: Vec<SidebarPick> = dialog
            .picks
            .into_iter()
            .filter(|pick| self.sidebar_pick_target(*pick).is_some())
            .collect();
        if !picks.is_empty() {
            self.close_sidebar_picks(picks, window, cx);
        }
        cx.notify();
    }

    fn handle_selection_close_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_selection.dialog.is_none() {
            return;
        }
        match modal_key(event) {
            Some(ModalKey::Dismiss) => self.dismiss_selection_close_dialog(window, cx),
            Some(ModalKey::Confirm) => self.confirm_selection_close(window, cx),
            None => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_selection_close_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(dialog) = self.sidebar_selection.dialog.as_mut() else {
            return div().into_any_element();
        };
        if !dialog.focused {
            dialog.focused = true;
            dialog.return_focus.capture_once(window, cx);
            self.sidebar_selection.dialog_focus.focus(window, cx);
        }
        let Some(dialog) = self.sidebar_selection.dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let live: Vec<SidebarPick> = dialog
            .picks
            .iter()
            .copied()
            .filter(|pick| self.sidebar_pick_target(*pick).is_some())
            .collect();
        let (workspaces, tabs) = selection_counts(&live);
        let sessions: usize = live
            .iter()
            .filter_map(|pick| self.sidebar_pick_target(*pick))
            .map(|target| self.close_target_terminals(&target, cx).len())
            .sum();

        let header = modal_header(
            ui,
            format!("Close {}?", selection_summary(workspaces, tabs)),
            stopped_sessions_summary(sessions),
        );
        let list = confirmation_list(
            ui,
            live.iter().filter_map(|pick| self.sidebar_pick_row(*pick)),
            SELECTION_DIALOG_MAX_ROWS,
        );
        let explanation = confirmation_warning(
            ui,
            "Closing stops the sessions running in everything listed above. Files, branches \
             and worktrees on disk are kept.",
        );
        let footer = modal_footer()
            .child(secondary_button(
                "selection-close-cancel",
                "Cancel",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.dismiss_selection_close_dialog(window, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(
                destructive_button("selection-close-confirm", "Close selection").on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.confirm_selection_close(window, cx);
                        cx.stop_propagation();
                    }),
                ),
            );

        let card = modal_card(
            "selection-close-dialog",
            SELECTION_DIALOG_WIDTH,
            SELECTION_DIALOG_RADIUS,
            ui,
            div()
                .child(header)
                .child(list)
                .child(explanation)
                .child(footer),
        )
        .track_focus(&self.sidebar_selection.dialog_focus)
        .on_key_down(cx.listener(Self::handle_selection_close_key_down));

        modal_backdrop(
            "selection-close-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.dismiss_selection_close_dialog(window, cx);
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: u64) -> SidebarPick {
        SidebarPick::Workspace(id)
    }

    fn tab(workspace_id: u64, tab_id: u64) -> SidebarPick {
        SidebarPick::Tab(TabKey {
            workspace_id,
            tab_id,
        })
    }

    fn tree() -> Vec<(u64, Vec<u64>)> {
        vec![(1, vec![10, 11, 12]), (2, vec![20, 21])]
    }

    fn rows() -> Vec<SidebarPick> {
        vec![
            workspace(1),
            tab(1, 10),
            tab(1, 11),
            tab(1, 12),
            workspace(2),
            tab(2, 20),
            tab(2, 21),
        ]
    }

    #[test]
    fn a_toggle_adds_a_row_then_takes_it_back() {
        let mut selection = SidebarSelection::default();
        selection.toggle(tab(1, 10), &tree());
        selection.toggle(workspace(2), &tree());
        assert_eq!(selection.picks(), [tab(1, 10), workspace(2)]);
        assert!(selection.covers(tab(2, 21)), "a workspace covers its tabs");
        assert!(!selection.covers(tab(1, 11)));

        selection.toggle(tab(1, 10), &tree());
        assert_eq!(selection.picks(), [workspace(2)]);
        selection.toggle(workspace(2), &tree());
        assert!(selection.is_empty());
    }

    #[test]
    fn selecting_a_workspace_absorbs_the_tabs_already_picked_in_it() {
        let mut selection = SidebarSelection::default();
        selection.toggle(tab(1, 10), &tree());
        selection.toggle(tab(2, 20), &tree());
        selection.toggle(workspace(1), &tree());
        assert_eq!(selection.picks(), [tab(2, 20), workspace(1)]);
    }

    #[test]
    fn toggling_a_tab_of_a_selected_workspace_keeps_its_siblings_and_drops_the_workspace() {
        let mut selection = SidebarSelection::default();
        selection.toggle(workspace(1), &tree());
        selection.toggle(tab(1, 11), &tree());
        assert_eq!(selection.picks(), [tab(1, 10), tab(1, 12)]);
        assert!(!selection.covers(workspace(1)));
        assert!(!selection.covers(tab(1, 11)));
    }

    #[test]
    fn a_range_runs_from_the_last_toggled_row_and_replaces_the_selection() {
        let mut selection = SidebarSelection::default();
        selection.toggle(tab(2, 21), &tree());
        selection.toggle(tab(1, 10), &tree());
        selection.select_range(None, tab(1, 12), &rows());
        assert_eq!(selection.picks(), [tab(1, 10), tab(1, 11), tab(1, 12)]);

        selection.select_range(None, tab(1, 11), &rows());
        assert_eq!(
            selection.picks(),
            [tab(1, 10), tab(1, 11)],
            "the anchor holds while the range shrinks"
        );
    }

    #[test]
    fn a_range_without_an_anchor_starts_at_the_fallback_and_else_at_its_target() {
        let mut selection = SidebarSelection::default();
        selection.select_range(Some(tab(1, 11)), tab(1, 12), &rows());
        assert_eq!(selection.picks(), [tab(1, 11), tab(1, 12)]);

        let mut alone = SidebarSelection::default();
        alone.select_range(Some(tab(9, 90)), tab(2, 20), &rows());
        assert_eq!(alone.picks(), [tab(2, 20)]);
    }

    #[test]
    fn a_range_takes_a_whole_workspace_only_when_every_shown_tab_of_it_is_inside() {
        let mut selection = SidebarSelection::default();
        selection.toggle(tab(1, 11), &tree());
        selection.select_range(None, tab(2, 20), &rows());
        assert_eq!(
            selection.picks(),
            [tab(1, 11), tab(1, 12), tab(2, 20)],
            "crossing a folder row must not select the tabs below the range"
        );

        selection.select_range(None, tab(2, 21), &rows());
        assert_eq!(selection.picks(), [tab(1, 11), tab(1, 12), workspace(2)]);
    }

    #[test]
    fn a_folded_workspace_and_a_clicked_folder_row_count_as_whole() {
        let folded = [workspace(1), workspace(2), tab(2, 20), tab(2, 21)];
        let mut selection = SidebarSelection::default();
        selection.toggle(tab(2, 20), &tree());
        selection.select_range(None, workspace(1), &folded);
        assert_eq!(
            selection.picks(),
            [workspace(1), tab(2, 20)],
            "the folded workspace has no tab row outside the range, the open one does"
        );

        let mut clicked = SidebarSelection::default();
        clicked.select_range(None, workspace(1), &rows());
        assert_eq!(clicked.picks(), [workspace(1)]);
    }

    #[test]
    fn rows_that_left_the_rail_leave_the_selection() {
        let mut selection = SidebarSelection::default();
        selection.toggle(workspace(1), &tree());
        selection.toggle(tab(2, 20), &tree());
        selection.toggle(tab(2, 21), &tree());

        selection.retain_live(&[(2, vec![21])], true);
        assert_eq!(selection.picks(), [tab(2, 21)]);

        let mut grouped = SidebarSelection::default();
        grouped.toggle(workspace(1), &tree());
        grouped.toggle(tab(2, 20), &tree());
        grouped.retain_live(&tree(), false);
        assert_eq!(
            grouped.picks(),
            [tab(2, 20)],
            "grouped by status, no folder row is left to show a workspace pick"
        );
    }

    #[test]
    fn clearing_reports_whether_anything_was_selected() {
        let mut selection = SidebarSelection::default();
        assert!(!selection.clear());
        selection.toggle(tab(1, 10), &tree());
        assert!(selection.clear());
        assert!(selection.is_empty());
    }

    #[test]
    fn the_summary_names_each_kind_it_counts() {
        assert_eq!(
            selection_counts(&[workspace(1), tab(2, 20), tab(2, 21)]),
            (1, 2)
        );
        assert_eq!(selection_summary(1, 0), "1 workspace");
        assert_eq!(selection_summary(0, 3), "3 tabs");
        assert_eq!(selection_summary(2, 1), "2 workspaces and 1 tab");
        assert_eq!(stopped_sessions_summary(0), "Nothing is running in them.");
        assert_eq!(stopped_sessions_summary(1), "1 session will stop.");
        assert_eq!(stopped_sessions_summary(4), "4 sessions will stop.");
    }

    #[test]
    fn a_row_fades_in_when_selected_and_out_when_released() {
        let now = Instant::now();
        let mut motion = SidebarSelectionMotion::default();
        assert_eq!(motion.row(tab(1, 10), false, now), 0.);
        assert!(motion.rows.is_empty(), "an unselected row keeps no state");

        assert_eq!(motion.row(tab(1, 10), true, now), 0.);
        assert!(motion.is_animating(now));
        let midway = now + Duration::from_millis(75);
        let rising = motion.row(tab(1, 10), true, midway);
        assert!(rising > 0. && rising < 1.);

        let released = motion.row(tab(1, 10), false, midway);
        assert_eq!(released, rising, "a release retargets from where it stands");
        let settled = midway + Duration::from_millis(150);
        assert_eq!(motion.row(tab(1, 10), false, settled), 0.);
        assert!(motion.rows.is_empty());
        assert!(!motion.is_animating(settled));
    }

    #[test]
    fn rows_join_within_a_workspace_and_between_folded_ones_never_across_a_group_gap() {
        let (folder, tab) = (SidebarRow::Folder(0), SidebarRow::Tab(0, 0));
        assert_eq!(sidebar_seam_inset(folder, folder, false), Some(0.));
        assert_eq!(sidebar_seam_inset(folder, tab, false), Some(0.));
        assert_eq!(sidebar_seam_inset(tab, tab, false), Some(0.));
        assert_eq!(sidebar_seam_inset(tab, folder, false), None);

        assert_eq!(
            sidebar_seam_inset(folder, tab, true),
            None,
            "an indented tab row is narrower than its folder row"
        );
        assert_eq!(
            sidebar_seam_inset(tab, tab, true),
            Some(SIDEBAR_FOLDER_SLOT_WIDTH + SIDEBAR_TITLE_ROW_GAP)
        );
    }

    #[test]
    fn a_joined_edge_loses_its_corners_as_the_join_settles() {
        let radius = rail_row_radius();
        let alone = SelectionShape {
            picked: 1.,
            ..Default::default()
        };
        assert!(!alone.joined());
        assert_eq!(alone.corners(), Corners::all(radius));

        let top_of_a_run = SelectionShape {
            picked: 1.,
            above: 0.,
            below: 1.,
        };
        let corners = top_of_a_run.corners();
        assert_eq!((corners.top_left, corners.top_right), (radius, radius));
        assert_eq!(
            (corners.bottom_left, corners.bottom_right),
            (px(0.), px(0.))
        );

        let joining = SelectionShape {
            picked: 1.,
            above: 0.5,
            below: 0.,
        };
        assert_eq!(joining.corners().top_left, radius * 0.5);
    }

    #[test]
    fn seams_tell_each_row_which_neighbors_it_can_join() {
        let mut motion = SidebarSelectionMotion::default();
        motion.join(&[
            SelectionSeam {
                above: workspace(1),
                below: tab(1, 10),
                inset: 0.,
            },
            SelectionSeam {
                above: tab(1, 10),
                below: tab(1, 11),
                inset: 0.,
            },
        ]);
        assert_eq!(motion.neighbors[&workspace(1)], (None, Some(tab(1, 10))));
        assert_eq!(
            motion.neighbors[&tab(1, 10)],
            (Some(workspace(1)), Some(tab(1, 11)))
        );
        assert_eq!(motion.neighbors[&tab(1, 11)], (Some(tab(1, 10)), None));

        motion.join(&[]);
        assert!(motion.neighbors.is_empty());
    }

    #[test]
    fn rows_gone_from_the_selection_are_forgotten_once_they_rest() {
        let now = Instant::now();
        let mut motion = SidebarSelectionMotion::default();
        motion.row(tab(1, 10), true, now);
        motion.row(tab(1, 11), true, now);
        let rested = now + Duration::from_millis(200);
        motion.row(tab(1, 10), true, rested);
        motion.row(tab(1, 11), true, rested);

        motion.forget_unless(|pick| pick == tab(1, 10), rested);
        assert_eq!(motion.rows.len(), 1);
        assert!(motion.rows.contains_key(&tab(1, 10)));
    }
}

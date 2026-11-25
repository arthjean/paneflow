mod branch;
mod changes_tree;
pub(crate) mod code;
mod file_chrome;
mod git;
mod model;
mod new_tab_menu;
mod options_menu;
pub(crate) mod preview;
mod render;
#[cfg(test)]
pub(crate) use render::render_diff_header_icon_button;
mod revert;
mod surface_picker;
mod tab_menu;
mod tabs;

pub(crate) use branch::DiffBranchMenuState;
pub(crate) use file_chrome::CrumbMenu;
pub(crate) use model::{
    DIFF_DOCK_PANEL_MIN_WIDTH, DIFF_DOCK_PANEL_WIDTH, DiffDockData, DiffDockHScrollDrag,
    DiffDockTab, DiffHover,
};
pub(crate) use tab_menu::DiffTabMenu;
pub(crate) use tabs::{ClosedFileTabs, FilePositions};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, ScrollEvent, ScrollHandle, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px,
};

use self::branch::render_diff_branch_chip;
use self::git::{build_diff_dock, colorize_diff_dock};
use self::model::{DIFF_DOCK_PANEL_MAX_WIDTH, DiffChrome};
use self::render::{
    diff_panel_centered, dock_card_background, dock_card_border, dock_card_fill,
    render_diff_error_banner, render_diff_file_header, render_diff_files_toolbar,
    render_diff_resize_handle, render_diff_tab_strip,
};
use self::surface_picker::{render_diff_picker_header, render_diff_surface_picker};
use crate::PaneFlowApp;
use crate::diff::{
    DiffBody, DiffElement, DiffOptions, H_SCROLLBAR_TRACK_HEIGHT, HScrollbarSegment, RowKind,
    SplitRow, discard_expanded_folds_for_path, file_at_row, h_offset_index, h_offset_len,
    h_scrollbar_click_offset, h_scrollbar_segments, palette, row_at_offset, set_file_side_offset,
    split_right_side_at_x,
};

type DiffFileToggle = std::rc::Rc<dyn Fn(&gpui::SharedString, &mut Window, &mut gpui::App)>;

fn diff_file_toggle(app: gpui::WeakEntity<PaneFlowApp>) -> DiffFileToggle {
    std::rc::Rc::new(move |path, _window, cx| {
        let _ = app.update(cx, |this, cx| {
            this.toggle_diff_file_collapsed(path.to_string(), cx)
        });
    })
}

impl PaneFlowApp {
    pub(crate) fn open_diff_dock_panel(&mut self, cwd: String, cx: &mut Context<Self>) {
        let cwd = cwd.trim().to_string();
        self.diff_dock.owner = self.active_session_id();
        let split = self.diff_dock.split;
        let has_current_snapshot = self.diff_dock.data.as_ref().is_some_and(|data| {
            data.cwd == cwd
                && !data.loading
                && data.error.is_none()
                && data.has_mode(split)
                && data.options == self.diff_dock.diff_options.for_cached_rows()
                && data.theme_generation == crate::theme::theme_generation()
        });
        if !self.diff_dock.open {
            let now = std::time::Instant::now();
            let from = self
                .diff_dock
                .reveal_animation
                .map_or(0., |animation| animation.width_at(now));
            self.diff_dock.reveal_animation =
                (!crate::ui_primitives::reduce_motion()).then_some(crate::SidebarWidthAnimation {
                    from_width: from,
                    to_width: 1.,
                    started_at: now,
                });
        }
        self.diff_dock.open = true;
        if has_current_snapshot {
            cx.notify();
        } else {
            self.refresh_diff_dock(cwd, cx);
        }
    }

    pub(crate) fn dismiss_diff_dock_panel(&mut self, cx: &mut Context<Self>) {
        if !self.diff_dock_visible()
            || self.diff_dock_fills_panel()
            || crate::ui_primitives::reduce_motion()
        {
            self.close_diff_dock_panel(cx);
            return;
        }
        let now = std::time::Instant::now();
        let from = self
            .diff_dock
            .reveal_animation
            .map_or(1., |animation| animation.width_at(now));
        let animation = crate::SidebarWidthAnimation {
            from_width: from,
            to_width: 0.,
            started_at: now,
        };
        self.diff_dock.open = false;
        self.diff_dock.reveal_animation = Some(animation);
        self.diff_dock.resize = None;
        self.diff_dock.h_scroll_drag = None;
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::at(animation.finishes_at()).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let still_closing = !app.diff_dock.open
                            && app
                                .diff_dock
                                .reveal_animation
                                .is_some_and(|current| current.started_at == animation.started_at);
                        if still_closing {
                            app.close_diff_dock_panel(cx);
                        }
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn close_diff_dock_panel(&mut self, cx: &mut Context<Self>) {
        self.diff_dock.open = false;
        self.diff_dock.data = None;
        self.clear_diff_dock_snapshot_state();
        self.diff_dock.maximized = None;
        self.diff_dock.maximize_animation = None;
        self.diff_dock.reveal_animation = None;
        self.diff_dock.resize = None;
        self.diff_dock.h_scroll_drag = None;
        cx.notify();
    }

    pub(crate) fn refresh_diff_dock(&mut self, cwd: String, cx: &mut Context<Self>) {
        let cwd = cwd.trim().to_string();
        let generation = self.diff_dock.generation.wrapping_add(1);
        self.diff_dock.generation = generation;
        let previous_fingerprint = self
            .diff_dock
            .data
            .as_ref()
            .filter(|data| data.cwd == cwd)
            .map(|data| data.fingerprint)
            .unwrap_or(0);
        let cwd_changed = self
            .diff_dock
            .data
            .as_ref()
            .is_some_and(|data| data.cwd != cwd);
        if cwd_changed {
            self.clear_diff_dock_snapshot_state();
        }
        if cwd.is_empty() {
            self.clear_diff_dock_snapshot_state();
            self.diff_dock.data = Some(DiffDockData::message(
                cwd,
                "No folder is linked to this thread.".to_string(),
            ));
            cx.notify();
            return;
        }
        match self
            .diff_dock
            .data
            .as_mut()
            .filter(|data| data.cwd == cwd && data.has_rows())
        {
            Some(data) => {
                data.loading = true;
                data.error = None;
            }
            None => {
                let mut loading = DiffDockData::loading(cwd.clone());
                loading.fingerprint = previous_fingerprint;
                self.diff_dock.data = Some(loading);
            }
        }
        cx.notify();

        self.spawn_diff_dock_build(cwd, generation, cx);
    }

    pub(crate) fn set_diff_dock_options(&mut self, options: DiffOptions, cx: &mut Context<Self>) {
        if self.diff_dock.diff_options == options {
            return;
        }
        let comparison_changed = self.diff_dock.diff_options.whitespace != options.whitespace;
        self.diff_dock.diff_options = options;
        if !comparison_changed {
            cx.notify();
            return;
        }
        let cwd = self
            .diff_dock
            .data
            .as_ref()
            .map(|data| data.cwd.clone())
            .filter(|cwd| !cwd.trim().is_empty());
        match cwd {
            Some(cwd) => self.refresh_diff_dock(cwd, cx),
            None => cx.notify(),
        }
    }

    fn clear_diff_dock_snapshot_state(&mut self) {
        self.diff_dock.collapsed.clear();
        self.diff_dock.expanded_folds.clear();
        self.diff_dock.scroll = ScrollHandle::new();
        self.diff_dock.vertical_scrollbar.cancel_drag();
        self.diff_dock.h_offsets = std::rc::Rc::new(Vec::new());
        self.diff_dock.hover = None;
        self.diff_dock.header_hover = None;
    }

    fn reload_file_tab_bases(&mut self, cx: &mut Context<Self>) {
        for tab in self.diff_dock.diff_tabs.clone() {
            if let DiffDockTab::File(view) = tab {
                view.update(cx, |view, cx| view.reload_base(cx));
            }
        }
    }

    pub(crate) fn refresh_diff_dock_if_open_for_cwd(
        &mut self,
        cwd: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let should_refresh = self.diff_dock.open
            && self
                .diff_dock
                .data
                .as_ref()
                .is_some_and(|data| data.cwd == cwd && !data.loading);
        if should_refresh {
            self.refresh_diff_dock(cwd.to_string(), cx);
        }
        should_refresh
    }

    fn spawn_diff_dock_build(&mut self, cwd: String, generation: u64, cx: &mut Context<Self>) {
        let theme = crate::theme::active_theme();
        let theme_generation = crate::theme::theme_generation();
        let options = self.diff_dock.diff_options;
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = smol::unblock({
                    let cwd = cwd.clone();
                    move || build_diff_dock(&cwd, theme, theme_generation, options)
                })
                .await;
                let pending = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        if !app.diff_dock_build_is_current(&cwd, generation) {
                            return None;
                        }
                        let collapsed = app.diff_dock.collapsed.clone();
                        let expanded = app.diff_dock.expanded_folds.clone();
                        let mut pending = None;
                        match result {
                            Ok(built) => {
                                pending = built
                                    .syntax_pending
                                    .then(|| (built.files_full.clone(), built.shown_files));
                                let stats = crate::workspace::GitDiffStats {
                                    files_changed: built.file_count,
                                    insertions: built.added as usize,
                                    deletions: built.removed as usize,
                                    count_capped: built.file_count_capped,
                                    unavailable: false,
                                };
                                app.apply_git_stats_for_cwd(&cwd, stats);
                                let head_changed = app
                                    .diff_dock
                                    .data
                                    .as_ref()
                                    .filter(|data| data.has_rows())
                                    .is_some_and(|data| data.head_sha != built.head_sha);
                                let reset_snapshot_state =
                                    app.diff_dock.data.as_ref().is_some_and(|data| {
                                        data.fingerprint != 0
                                            && data.fingerprint != built.fingerprint
                                    });
                                if reset_snapshot_state {
                                    app.clear_diff_dock_snapshot_state();
                                } else {
                                    app.diff_dock.h_offsets = std::rc::Rc::new(Vec::new());
                                }
                                let collapsed = if reset_snapshot_state {
                                    std::collections::HashSet::new()
                                } else {
                                    collapsed
                                };
                                let expanded = if reset_snapshot_state {
                                    std::collections::HashSet::new()
                                } else {
                                    expanded
                                };
                                if let Some(data) = app.diff_dock.data.as_mut() {
                                    data.apply_built(built, &collapsed, &expanded);
                                }
                                app.diff_dock.hover = None;
                                app.diff_dock.header_hover = None;
                                if head_changed {
                                    app.reload_file_tab_bases(cx);
                                }
                            }
                            Err(err) => {
                                match app.diff_dock.data.as_mut().filter(|data| data.has_rows()) {
                                    Some(data) => {
                                        data.loading = false;
                                        data.error = Some(err);
                                    }
                                    None => {
                                        app.diff_dock.data =
                                            Some(DiffDockData::message(cwd.clone(), err));
                                    }
                                }
                            }
                        }
                        cx.notify();
                        pending
                    })
                });
                let Ok(Some((files, shown_files))) = pending else {
                    return;
                };
                let colored =
                    smol::unblock(move || colorize_diff_dock(&files, shown_files, &theme)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        if !app.diff_dock_build_is_current(&cwd, generation) {
                            return;
                        }
                        let collapsed = app.diff_dock.collapsed.clone();
                        let expanded = app.diff_dock.expanded_folds.clone();
                        if let Some(data) = app.diff_dock.data.as_mut() {
                            data.apply_colored(colored, &collapsed, &expanded);
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    fn diff_dock_build_is_current(&self, cwd: &str, generation: u64) -> bool {
        self.diff_dock.generation == generation
            && self
                .diff_dock
                .data
                .as_ref()
                .is_some_and(|data| data.cwd == cwd)
    }

    fn recompute_diff_dock_display(&mut self) {
        let collapsed = self.diff_dock.collapsed.clone();
        let expanded = self.diff_dock.expanded_folds.clone();
        if let Some(data) = self.diff_dock.data.as_mut() {
            data.recompute(&collapsed, &expanded);
        }
    }

    fn refresh_diff_dock_if_theme_changed(&mut self, cx: &mut Context<Self>) {
        let current_theme_generation = crate::theme::theme_generation();
        let Some(data) = self.diff_dock.data.as_ref() else {
            return;
        };
        if data.loading
            || data.error.is_some()
            || data.theme_generation == current_theme_generation
            || data.cwd.trim().is_empty()
        {
            return;
        }
        self.refresh_diff_dock(data.cwd.clone(), cx);
    }

    pub(crate) fn toggle_diff_file_collapsed(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.diff_dock.collapsed.remove(&path) {
            discard_expanded_folds_for_path(&mut self.diff_dock.expanded_folds, &path);
            self.diff_dock.collapsed.insert(path);
        }
        self.recompute_diff_dock_display();
        cx.notify();
    }

    pub(crate) fn set_all_diff_collapsed(
        &mut self,
        paths: &[String],
        collapse: bool,
        cx: &mut Context<Self>,
    ) {
        if collapse {
            self.diff_dock.collapsed.extend(paths.iter().cloned());
            self.diff_dock.expanded_folds.clear();
        } else {
            self.diff_dock.collapsed.clear();
        }
        self.recompute_diff_dock_display();
        cx.notify();
    }

    pub(crate) fn set_diff_dock_split(&mut self, split: bool, cx: &mut Context<Self>) {
        if self.diff_dock.split == split {
            return;
        }
        self.diff_dock.split = split;
        self.diff_dock.h_scroll_drag = None;
        self.diff_dock.hover = None;
        self.diff_dock.header_hover = None;
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_diff_dock_panel(
        &mut self,
        width: f32,
        max_width: f32,
        files_progress: f32,
        frame: crate::layout::CardFrame,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.diff_dock.picker || self.diff_dock.diff_tabs.is_empty() {
            return self.render_diff_dock_picker(width, max_width, frame, ui, cx);
        }
        self.refresh_diff_dock_if_theme_changed(cx);
        let data = self.diff_dock.data.clone();
        let cwd = data.as_ref().map(|d| d.cwd.clone()).unwrap_or_default();
        let active = self
            .diff_dock
            .diff_active_tab
            .min(self.diff_dock.diff_tabs.len().saturating_sub(1));
        let tabs = self.diff_dock.diff_tabs.clone();
        let maximized = self.diff_dock.maximized.is_some();
        let fills_panel = self.diff_dock_fills_panel();
        let card_background = self.diff_dock_card_background(ui);
        let in_title_bar = self.diff_dock.title_bar_zone_left.is_some();
        let header = (!in_title_bar).then(|| {
            render_diff_tab_strip(
                &tabs,
                active,
                self.diff_dock.diff_tab_close_armed.map(|(index, _)| index),
                self.diff_dock.diff_new_tab_menu_open,
                !self.diff_dock.closed_files.is_empty(),
                maximized,
                false,
                frame,
                card_background,
                &self.diff_dock.tab_scroll,
                &self.diff_dock.tab_strip_width,
                ui,
                cx,
            )
        });
        let (toolbar, body) = match tabs.get(active) {
            Some(DiffDockTab::Terminal(terminal)) => (None, terminal.clone().into_any_element()),
            Some(DiffDockTab::PendingFile) => (
                Some(render_diff_file_header(
                    self.render_dock_crumbs(None, ui, cx),
                    None,
                    None,
                    None,
                    ui,
                )),
                self.render_files_sidebar_fill(),
            ),
            Some(DiffDockTab::File(view)) => {
                let (path, position, editing) = {
                    let view = view.read(cx);
                    let editing = view.shows_text_position();
                    (
                        view.path().to_path_buf(),
                        editing.then(|| view.cursor_line_column()),
                        editing,
                    )
                };
                let mode_toggle = view.update(cx, |view, cx| view.render_mode_toggle(ui, cx));
                let controls = editing.then(|| view.read(cx).controls.clone().into_any_element());
                let controls = match (mode_toggle, controls) {
                    (None, controls) => controls,
                    (Some(toggle), controls) => Some(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .child(toggle)
                            .children(controls)
                            .into_any_element(),
                    ),
                };
                let tree_toggle = file_chrome::render_tree_toggle(self.files_sidebar_open, ui, cx);
                (
                    Some(render_diff_file_header(
                        self.render_dock_crumbs(Some(&path), ui, cx),
                        position,
                        controls,
                        Some(tree_toggle),
                        ui,
                    )),
                    view.clone().into_any_element(),
                )
            }
            Some(DiffDockTab::Changes) => {
                let toolbar = self.render_diff_toolbar(&cwd, &data, width, ui, cx);
                let diff = self.render_diff_dock_body(&data, ui, cx);
                let tree = self.render_diff_changes_tree(&data, width, ui, cx);
                (
                    Some(toolbar),
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .child(diff),
                        )
                        .children(tree)
                        .into_any_element(),
                )
            }
            None => (
                None,
                render_diff_surface_picker(
                    self.diff_dock.picker_highlight,
                    &self.diff_dock.picker_focus,
                    ui,
                    cx,
                ),
            ),
        };

        let inset = f32::from(crate::app::constants::DIFF_DOCK_CONTENT_INSET);
        let tree_width =
            super::files_sidebar::files_tree_fit(self.files_tree_width, width - 2. * inset);
        let tree_beside = matches!(tabs.get(active), Some(DiffDockTab::File(_)));
        let tree = (tree_beside && files_progress > 0.)
            .then(|| self.render_files_tree_column(tree_width, files_progress, ui, window, cx));
        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .overflow_hidden()
            .pl(crate::app::constants::DIFF_DOCK_CONTENT_INSET)
            .pr(crate::app::constants::DIFF_DOCK_CONTENT_INSET)
            .pb(crate::app::constants::DIFF_DOCK_CONTENT_INSET)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .children(tree);

        div()
            .relative()
            .map(|panel| {
                if fills_panel {
                    panel.w_full()
                } else {
                    panel.w(px(width))
                }
            })
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .child(dock_card_fill(
                frame,
                card_background,
                (!in_title_bar).then_some(ui.border),
            ))
            .when(!fills_panel, |panel| {
                panel.child(render_diff_resize_handle(width, max_width, frame, ui, cx))
            })
            .children(header)
            .children(toolbar)
            .child(body)
            .when(!in_title_bar, |panel| {
                panel.child(dock_card_border(frame, card_background, ui))
            })
            .into_any_element()
    }

    pub(crate) fn render_diff_dock_title_strip(
        &mut self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.diff_dock.picker || self.diff_dock.diff_tabs.is_empty() {
            return None;
        }
        let active = self
            .diff_dock
            .diff_active_tab
            .min(self.diff_dock.diff_tabs.len().saturating_sub(1));
        let tabs = self.diff_dock.diff_tabs.clone();
        Some(render_diff_tab_strip(
            &tabs,
            active,
            self.diff_dock.diff_tab_close_armed.map(|(index, _)| index),
            self.diff_dock.diff_new_tab_menu_open,
            !self.diff_dock.closed_files.is_empty(),
            self.diff_dock.maximized.is_some(),
            true,
            crate::layout::CardFrame::Flush(gpui::Edges::default()),
            self.diff_dock_card_background(ui),
            &self.diff_dock.tab_scroll,
            &self.diff_dock.tab_strip_width,
            ui,
            cx,
        ))
    }

    pub(crate) fn diff_dock_card_background(&self, ui: crate::theme::UiColors) -> gpui::Hsla {
        dock_card_background(&crate::theme::active_theme(), ui, self.backdrop)
    }

    fn render_diff_dock_picker(
        &mut self,
        width: f32,
        max_width: f32,
        frame: crate::layout::CardFrame,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fills_panel = self.diff_dock_fills_panel();
        let card_background = self.diff_dock_card_background(ui);
        div()
            .relative()
            .map(|panel| {
                if fills_panel {
                    panel.w_full()
                } else {
                    panel.w(px(width))
                }
            })
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .child(dock_card_fill(
                frame,
                card_background,
                self.diff_dock
                    .title_bar_zone_left
                    .is_none()
                    .then_some(ui.border),
            ))
            .when(!fills_panel, |panel| {
                panel.child(render_diff_resize_handle(width, max_width, frame, ui, cx))
            })
            .child(render_diff_picker_header())
            .child(render_diff_surface_picker(
                self.diff_dock.picker_highlight,
                &self.diff_dock.picker_focus,
                ui,
                cx,
            ))
            .when(self.diff_dock.title_bar_zone_left.is_none(), |panel| {
                panel.child(dock_card_border(frame, card_background, ui))
            })
            .into_any_element()
    }

    pub(crate) fn drag_diff_dock_resize(&mut self, cursor_x: f32, cx: &mut Context<Self>) {
        if let Some((anchor_x, anchor_w, max_w)) = self.diff_dock.resize {
            let delta = anchor_x - cursor_x;
            let ceiling = max_w.clamp(DIFF_DOCK_PANEL_MIN_WIDTH, DIFF_DOCK_PANEL_MAX_WIDTH);
            self.diff_dock.width = (anchor_w + delta).clamp(DIFF_DOCK_PANEL_MIN_WIDTH, ceiling);
            cx.notify();
        }
    }

    pub(crate) fn end_diff_dock_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if self.diff_dock.resize.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    fn render_diff_toolbar(
        &mut self,
        cwd: &str,
        data: &Option<DiffDockData>,
        width: f32,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chip = self.diff_branch_for_cwd(cwd).map(|(branch, uncommitted)| {
            render_diff_branch_chip(
                cwd.to_string(),
                branch,
                uncommitted,
                self.diff_dock.diff_branch_menu.as_ref(),
                ui,
                cx,
            )
        });
        let chrome = DiffChrome {
            data,
            cwd: cwd.to_string(),
            split: self.diff_dock.split,
            options: self.diff_dock.diff_options,
            options_open: self.diff_dock.diff_options_menu_open,
            collapsed: &self.diff_dock.collapsed,
            tree_open: self.diff_dock.changes_tree_open,
            width,
        };
        render_diff_files_toolbar(&chrome, chip, ui, cx)
    }

    fn render_diff_dock_body(
        &mut self,
        data: &Option<DiffDockData>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(data) = data else {
            return diff_panel_centered(
                "icons/dock/git-compare.svg",
                "Open the panel to see changes.",
                ui,
            );
        };
        let split = self.diff_dock.split;
        let has_rows = data.has_mode(split);
        if data.loading && !has_rows {
            return diff_panel_centered("icons/dock/loader-circle.svg", "Loading changes…", ui);
        }
        let banner = match &data.error {
            Some(error) if has_rows => Some(render_diff_error_banner(error, ui)),
            Some(error) => return diff_panel_centered("icons/dock/alert-02.svg", error, ui),
            None => None,
        };
        if data.file_count == 0 && banner.is_none() {
            return diff_panel_centered(
                "icons/dock/checkmark-circle-02.svg",
                "No uncommitted changes.",
                ui,
            );
        }

        let file_count = if split {
            data.disp_split_spans.len()
        } else {
            data.disp_unified_spans.len()
        };
        let needed_offsets = h_offset_len(file_count, split);
        if split {
            if self.diff_dock.h_offsets.len() != needed_offsets {
                std::rc::Rc::make_mut(&mut self.diff_dock.h_offsets).resize(needed_offsets, 0.0);
            }
        } else if self.diff_dock.h_offsets.len() < needed_offsets {
            std::rc::Rc::make_mut(&mut self.diff_dock.h_offsets).resize(needed_offsets, 0.0);
        }
        let h_offsets = self.diff_dock.h_offsets.clone();

        let body = if split {
            DiffBody::Split {
                rows: data.disp_split.clone(),
                offsets: data.disp_split_offsets.clone(),
                max_line_no: data.disp_split_max_no,
                spans: data.disp_split_spans.clone(),
                h_offsets: h_offsets.clone(),
            }
        } else {
            DiffBody::Unified {
                rows: data.disp_unified.clone(),
                offsets: data.disp_unified_offsets.clone(),
                max_line_no: data.disp_unified_max_no,
                spans: data.disp_unified_spans.clone(),
                h_offsets,
            }
        };
        let pal = palette(ui).on_material(self.backdrop.is_blended());
        let scroll = self.diff_dock.scroll.clone();
        let chip_row = self
            .diff_dock
            .hover
            .as_ref()
            .filter(|hover| hover.split == split)
            .map(|hover| hover.chip_row);

        let mut element = div()
            .id("diff-dock-scroll")
            .min_w_0()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_click(cx.listener(|this, ev: &ClickEvent, _w, cx| {
                this.handle_diff_dock_body_click(ev, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    let split = this.diff_dock.split;
                    if this.handle_diff_dock_h_scrollbar_mouse_down(ev.position, split, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                this.apply_diff_dock_wheel(ev, window, cx);
            }))
            .child(
                DiffElement::new(body, pal)
                    .with_file_toggle(diff_file_toggle(cx.entity().downgrade()))
                    .with_highlight(self.diff_dock.diff_options.highlight)
                    .with_revert_chip(chip_row)
                    .with_hovered_header(self.diff_dock.header_hover),
            );
        element.style().restrict_scroll_to_axis = Some(true);

        div()
            .id("diff-dock-body")
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .children(banner)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(element)
                    .child(self.diff_dock.vertical_scrollbar.render(&scroll, cx)),
            )
            .into_any_element()
    }

    fn apply_diff_dock_wheel(
        &mut self,
        ev: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = ev.delta.pixel_delta(window.line_height());
        let dx = f32::from(delta.x);
        if dx != 0.0 {
            let split = self.diff_dock.split;
            let bounds = self.diff_dock.scroll.bounds();
            if ev.position.x < bounds.left()
                || ev.position.x > bounds.right()
                || ev.position.y < bounds.top()
                || ev.position.y > bounds.bottom()
            {
                return;
            }
            let width = f32::from(bounds.size.width);
            let content_y =
                f32::from(ev.position.y - bounds.top() - self.diff_dock.scroll.offset().y).max(0.0);
            if let Some(file_idx) = self.diff_dock_file_at_content_y(content_y, split) {
                let spans = self.diff_dock_spans(split);
                let local_x = f32::from(ev.position.x - bounds.left());
                let right = split && split_right_side_at_x(local_x, width);
                let offset_idx = h_offset_index(spans.len(), file_idx, split, right);
                let cur = self
                    .diff_dock
                    .h_offsets
                    .get(offset_idx)
                    .copied()
                    .unwrap_or(0.0);
                set_file_side_offset(
                    std::rc::Rc::make_mut(&mut self.diff_dock.h_offsets),
                    &spans,
                    file_idx,
                    right,
                    cur - dx,
                    split,
                    width,
                );
            }
            cx.notify();
        }
    }

    fn diff_dock_scrollbar_segments(&self, split: bool) -> Option<Vec<HScrollbarSegment>> {
        let data = self.diff_dock.data.as_ref()?;
        let bounds = self.diff_dock.scroll.bounds();
        let viewport_h = f32::from(bounds.size.height);
        let panel_width = f32::from(bounds.size.width);
        if viewport_h <= 0.0 || panel_width <= 0.0 {
            return None;
        }

        let visible_top = f32::from(-self.diff_dock.scroll.offset().y).max(0.0);
        let visible_bottom = visible_top + viewport_h;
        let (offsets, spans) = if split {
            (&data.disp_split_offsets, &data.disp_split_spans)
        } else {
            (&data.disp_unified_offsets, &data.disp_unified_spans)
        };
        Some(h_scrollbar_segments(
            spans,
            offsets,
            self.diff_dock.h_offsets.as_ref(),
            split,
            panel_width,
            visible_top,
            visible_bottom,
        ))
    }

    fn diff_dock_h_scrollbar_local_point(&self, point: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.diff_dock.scroll.bounds();
        if point.x < bounds.left()
            || point.x > bounds.right()
            || point.y < bounds.top()
            || point.y > bounds.bottom()
        {
            return None;
        }
        Some((
            f32::from(point.x - bounds.left()),
            f32::from(point.y - bounds.top() - self.diff_dock.scroll.offset().y).max(0.0),
        ))
    }

    fn handle_diff_dock_h_scrollbar_mouse_down(
        &mut self,
        point: Point<Pixels>,
        split: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((x, y)) = self.diff_dock_h_scrollbar_local_point(point) else {
            return false;
        };
        let Some(segments) = self.diff_dock_scrollbar_segments(split) else {
            return false;
        };
        let Some(segment) = segments.iter().find(|segment| {
            x >= segment.x
                && x <= segment.x + segment.width
                && y >= segment.y
                && y <= segment.y + H_SCROLLBAR_TRACK_HEIGHT
        }) else {
            return false;
        };

        let thumb_left = segment.x + segment.thumb_x;
        let thumb_right = thumb_left + segment.thumb_width;
        let target = if x >= thumb_left && x <= thumb_right {
            segment.offset
        } else {
            h_scrollbar_click_offset(&segments, x, y)
                .map(|(_, offset)| offset)
                .unwrap_or(segment.offset)
        }
        .clamp(0.0, segment.max_scroll);

        self.set_diff_dock_h_offset(segment.offset_idx, target);
        self.diff_dock.h_scroll_drag = Some(DiffDockHScrollDrag {
            offset_idx: segment.offset_idx,
            start_mouse_x: point.x,
            start_offset: target,
            max_scroll: segment.max_scroll,
            track_width: segment.width,
            thumb_width: segment.thumb_width,
        });
        cx.notify();
        true
    }

    fn handle_diff_dock_h_scrollbar_click(
        &mut self,
        point: Point<Pixels>,
        split: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((x, y)) = self.diff_dock_h_scrollbar_local_point(point) else {
            return false;
        };
        let Some(segments) = self.diff_dock_scrollbar_segments(split) else {
            return false;
        };
        let Some(segment) = segments.iter().find(|segment| {
            x >= segment.x
                && x <= segment.x + segment.width
                && y >= segment.y
                && y <= segment.y + H_SCROLLBAR_TRACK_HEIGHT
        }) else {
            return false;
        };

        let thumb_left = segment.x + segment.thumb_x;
        let thumb_right = thumb_left + segment.thumb_width;
        if x >= thumb_left && x <= thumb_right {
            return true;
        }

        let target = h_scrollbar_click_offset(&segments, x, y)
            .map(|(_, offset)| offset)
            .unwrap_or(segment.offset)
            .clamp(0.0, segment.max_scroll);
        self.set_diff_dock_h_offset(segment.offset_idx, target);
        cx.notify();
        true
    }

    fn set_diff_dock_h_offset(&mut self, offset_idx: usize, value: f32) {
        let offsets = std::rc::Rc::make_mut(&mut self.diff_dock.h_offsets);
        if offsets.len() <= offset_idx {
            offsets.resize(offset_idx + 1, 0.0);
        }
        if let Some(slot) = offsets.get_mut(offset_idx) {
            *slot = value;
        }
    }

    pub(crate) fn drag_diff_dock_h_scrollbar(&mut self, mouse_x: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.diff_dock.h_scroll_drag else {
            return;
        };

        let track_range = (drag.track_width - drag.thumb_width).max(1.0);
        let delta = f32::from(mouse_x - drag.start_mouse_x);
        let next =
            (drag.start_offset + delta * drag.max_scroll / track_range).clamp(0.0, drag.max_scroll);
        if self
            .diff_dock
            .h_offsets
            .get(drag.offset_idx)
            .is_none_or(|current| (*current - next).abs() > 0.1)
        {
            self.set_diff_dock_h_offset(drag.offset_idx, next);
            cx.notify();
        }
    }

    pub(crate) fn end_diff_dock_h_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        if self.diff_dock.h_scroll_drag.take().is_some() {
            cx.notify();
        }
    }

    fn diff_dock_file_at_content_y(&self, content_y: f32, split: bool) -> Option<usize> {
        let data = self.diff_dock.data.as_ref()?;
        let (offsets, spans) = if split {
            (&data.disp_split_offsets, &data.disp_split_spans)
        } else {
            (&data.disp_unified_offsets, &data.disp_unified_spans)
        };
        let row = row_at_offset(offsets, content_y)?;
        file_at_row(spans, row)
    }

    fn diff_dock_spans(&self, split: bool) -> std::rc::Rc<Vec<crate::diff::FileSpan>> {
        self.diff_dock
            .data
            .as_ref()
            .map(|d| {
                if split {
                    d.disp_split_spans.clone()
                } else {
                    d.disp_unified_spans.clone()
                }
            })
            .unwrap_or_default()
    }

    fn handle_diff_dock_body_click(&mut self, ev: &ClickEvent, cx: &mut Context<Self>) {
        let split = self.diff_dock.split;
        if self.handle_diff_dock_h_scrollbar_click(ev.position(), split, cx)
            || self.handle_diff_dock_revert_click(ev.position(), cx)
        {
            return;
        }

        let row = {
            let Some(data) = self.diff_dock.data.as_ref() else {
                return;
            };
            let bounds = self.diff_dock.scroll.bounds();
            let y = ev.position().y;
            if y < bounds.top() || y > bounds.bottom() {
                return;
            }
            let target = f32::from(y - bounds.top() - self.diff_dock.scroll.offset().y).max(0.0);
            let offsets = if split {
                &data.disp_split_offsets
            } else {
                &data.disp_unified_offsets
            };
            let Some(row) = row_at_offset(offsets, target) else {
                return;
            };
            row
        };

        let fold_key = {
            let Some(data) = self.diff_dock.data.as_ref() else {
                return;
            };
            if split {
                match data.disp_split.get(row) {
                    Some(SplitRow::Fold(fold)) => Some(fold.key.to_string()),
                    _ => None,
                }
            } else {
                data.disp_unified
                    .get(row)
                    .filter(|r| r.kind == RowKind::Fold)
                    .and_then(|r| r.fold_key.as_ref())
                    .map(|key| key.to_string())
            }
        };
        if let Some(key) = fold_key {
            if !self.diff_dock.expanded_folds.remove(&key) {
                self.diff_dock.expanded_folds.insert(key);
            }
            self.recompute_diff_dock_display();
            cx.notify();
            return;
        }

        let path = {
            let Some(data) = self.diff_dock.data.as_ref() else {
                return;
            };
            let anchors = if split {
                &data.disp_anchors_split
            } else {
                &data.disp_anchors_unified
            };
            anchors
                .iter()
                .find(|(_, i)| *i == row)
                .map(|(p, _)| p.clone())
        };
        let Some(path) = path else {
            return;
        };
        self.toggle_diff_file_collapsed(path, cx);
    }
}

#[cfg(all(test, target_os = "linux"))]
impl PaneFlowApp {
    /// Opens the unified Changes tab on `cwd` at `width`, settled: no
    /// reveal animation and no checkout picker.
    pub(crate) fn open_diff_dock_for_test(
        &mut self,
        cwd: String,
        width: f32,
        cx: &mut Context<Self>,
    ) {
        self.diff_dock.width = width;
        self.diff_dock.picked = true;
        self.diff_dock.picker = false;
        self.diff_dock.split = false;
        self.open_diff_changes_tab(cx);
        self.open_diff_dock_panel(cwd, cx);
        self.diff_dock.reveal_animation = None;
    }

    /// The dock's build outcome once it has landed: its file paths, or the
    /// error it shows.
    pub(crate) fn diff_dock_outcome_for_test(&self) -> Option<Result<Vec<String>, String>> {
        let data = self.diff_dock.data.as_ref()?;
        if data.loading {
            return None;
        }
        Some(match &data.error {
            Some(error) => Err(error.clone()),
            None => Ok(data.paths.clone()),
        })
    }

    /// Collapses every file but `open`, then scrolls so that `top`'s header
    /// sits `past` pixels above the top of the dock body.
    pub(crate) fn frame_diff_dock_for_test(
        &mut self,
        open: &str,
        top: &str,
        past: f32,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let paths = self
            .diff_dock
            .data
            .as_ref()
            .ok_or("no dock data")?
            .paths
            .clone();
        if !paths.iter().any(|path| path == open) {
            return Err(format!("{open} is not in the diff"));
        }
        self.diff_dock.collapsed = paths.into_iter().filter(|path| path != open).collect();
        self.diff_dock.expanded_folds.clear();
        self.recompute_diff_dock_display();
        let data = self.diff_dock.data.as_ref().ok_or("no dock data")?;
        let row = data
            .disp_anchors_unified
            .iter()
            .find(|(path, _)| path == top)
            .map(|(_, row)| *row)
            .ok_or_else(|| format!("{top} has no header row"))?;
        let y = data.disp_unified_offsets[row] + past;
        self.diff_dock
            .scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(-y)));
        cx.notify();
        Ok(())
    }
}

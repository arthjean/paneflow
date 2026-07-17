//! US-014 (prd-git-diff-mode-2026-Q3.md): Multi-project scope - a **repo tab
//! bar** above a single, full-height [`DiffView`] for the selected repo.
//!
//! Each open repo is a tab; the selected repo's `DiffView` fills the whole area
//! (its own worktree columns side by side, its own internal scroll), so two
//! repos never compete for vertical space and there is no inner/outer scroll
//! fight. Repo views mount lazily and then stay cached; switching tabs suspends
//! the outgoing view's watchers while retaining loaded rows for instant return.
//! The base ref chosen in one repo is carried to the next tab (shared comparison
//! base across repos).

use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, div, prelude::*, px,
};

use super::DiffWorktree;
use super::scope::RepoGroup;
use super::view::{DiffView, FileListState};
use crate::ui_primitives::AnimatedHoverExt;

struct Group {
    repo_root: PathBuf,
    repo_name: String,
    /// Seed kept so the `DiffView` can be (re)mounted on select without
    /// re-collecting from the app.
    worktrees: Vec<DiffWorktree>,
    /// Lazy + warm: created on first selection, then retained with watchers
    /// suspended while the repo tab is not selected.
    view: Option<gpui::Entity<DiffView>>,
}

/// Hosts the per-repo diff tabs for the Multi-project scope.
pub struct MultiRepoDiffView {
    groups: Vec<Group>,
    /// Index of the repo whose `DiffView` is mounted + shown.
    selected: usize,
    /// Shared comparison base carried across tabs: when the user picks a base in
    /// one repo, switching to another seeds it with the same base. `None` until
    /// a repo resolves/sets one.
    base_ref: Option<String>,
    /// Scope breadcrumb fragment PUSHED by `render_diff_main` every frame and
    /// consumed by the next `render` (push-only contract, same as
    /// `DiffView::scope_slot`). Mounted at the left of the repo-tab strip so
    /// Multi-project also has a single chrome row.
    pub scope_slot: Option<gpui::AnyElement>,
}

impl MultiRepoDiffView {
    /// Build from the repo groups (US-014). The first repo is selected (and
    /// mounted) by default; the rest mount on demand when their tab is clicked.
    pub fn new(groups: Vec<RepoGroup>, cx: &mut Context<Self>) -> Self {
        let groups: Vec<Group> = groups
            .into_iter()
            .map(|g| Group {
                repo_root: g.repo_root,
                repo_name: g.repo_name,
                worktrees: g.worktrees,
                view: None,
            })
            .collect();
        let mut this = Self {
            groups,
            selected: 0,
            base_ref: None,
            scope_slot: None,
        };
        this.mount_selected(cx);
        this
    }

    /// Mount the selected repo's `DiffView` if not already, seeding it with the
    /// shared base ref so cross-repo comparison stays on one base.
    fn mount_selected(&mut self, cx: &mut Context<Self>) {
        let base = self.base_ref.clone();
        if let Some(g) = self.groups.get_mut(self.selected)
            && g.view.is_none()
        {
            let root = g.repo_root.clone();
            let wts = g.worktrees.clone();
            g.view = Some(cx.new(|cx| DiffView::with_base(root, wts, base, cx)));
        }
    }

    fn select(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx == self.selected || idx >= self.groups.len() {
            return;
        }
        // Carry the outgoing repo's base forward so the next tab opens on it.
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = &g.view
        {
            self.base_ref = Some(view.read(cx).base_ref().to_string());
        }
        // Keep the outgoing entity warm, but drop its watcher handles so hidden
        // repos cannot trigger diff rebuilds.
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = g.view.clone()
        {
            view.update(cx, |v, cx| v.suspend(cx));
        }
        self.selected = idx;
        self.mount_selected(cx);
        let base = self.base_ref.clone();
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = g.view.clone()
        {
            view.update(cx, |v, cx| v.resume_with_base(base, cx));
        }
        cx.notify();
    }

    /// US-016 warm-resume passthrough: suspend all cached child `DiffView`s so
    /// the Multi-project host releases watchers when the diff surface is hidden,
    /// while retaining loaded rows for instant warm resume.
    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        for group in &self.groups {
            if let Some(view) = group.view.clone() {
                view.update(cx, |v, cx| v.suspend(cx));
            }
        }
    }

    /// US-016 warm-resume passthrough: re-arm + revalidate the mounted child.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = g.view.clone()
        {
            view.update(cx, |v, cx| v.resume(cx));
        }
    }

    /// Per-branch changed-file lists of the selected repo's `DiffView`, for the
    /// multi-branch sidebar (one section per worktree column of that repo).
    pub fn active_column_file_lists(
        &self,
        cx: &App,
    ) -> Vec<(String, usize, PathBuf, FileListState)> {
        self.groups
            .get(self.selected)
            .and_then(|g| g.view.as_ref())
            .map(|v| v.read(cx).column_file_lists())
            .unwrap_or_default()
    }

    /// Selected column index of the selected repo's `DiffView` (active-branch
    /// highlight in the sidebar).
    pub fn active_selected_column(&self, cx: &App) -> usize {
        self.groups
            .get(self.selected)
            .and_then(|g| g.view.as_ref())
            .map(|v| v.read(cx).selected_column())
            .unwrap_or(0)
    }

    /// Select column `col_idx` of the selected repo's `DiffView` and scroll its
    /// body to `path` (sidebar file click in a multi-branch section).
    pub fn active_select_and_jump(
        &self,
        col_idx: usize,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = g.view.clone()
        {
            view.update(cx, |v, cx| v.select_and_jump(col_idx, path, window, cx));
        }
    }
}

impl Render for MultiRepoDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();

        // Single chrome row (Codex redesign): scope breadcrumb (host slot) at
        // the left, then the repo tabs. No own background and no border - the
        // strip sits directly on the panel (`ui.base`).
        let scope_slot = self.scope_slot.take();
        let mut tabs = div()
            .id("multi-diff-tabs")
            .flex_none()
            .h(px(36.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .px(px(10.))
            .overflow_x_scroll()
            .when_some(scope_slot, |d, slot| {
                d.child(slot).child(
                    gpui::svg()
                        .size(px(13.))
                        .flex_none()
                        .path("icons/chevron-right.svg")
                        .text_color(ui.muted),
                )
            });

        for (i, g) in self.groups.iter().enumerate() {
            let active = i == self.selected;
            let resting_bg = ui.subtle.opacity(0.0);
            tabs = tabs.child(
                // Flat browser-style tab: accent underline + content-bg + bold
                // when active; muted + transparent (border blends into the bar)
                // otherwise. The 2px bottom border is always present so the row
                // height does not jump between states. Repo name only - no git
                // icon, no worktree-count badge (kept deliberately minimal).
                div()
                    .id(SharedString::from(format!("multi-diff-tab-{i}")))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .px(px(12.))
                    .border_b_2()
                    .border_color(if active {
                        ui.accent
                    } else {
                        gpui::transparent_black()
                    })
                    .bg(resting_bg)
                    .animated_hover_bg(resting_bg, ui.subtle)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.select(i, cx);
                    }))
                    .child(
                        div()
                            .text_size(crate::ui_primitives::BODY_EMPHASIS)
                            .font_weight(FontWeight::NORMAL)
                            .text_color(if active { ui.text } else { ui.muted })
                            .child(g.repo_name.clone()),
                    ),
            );
        }

        let body: AnyElement = self
            .groups
            .get(self.selected)
            .and_then(|g| g.view.clone())
            .map(|v| v.into_any_element())
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_color(ui.muted)
                            .text_size(crate::ui_primitives::BODY_EMPHASIS)
                            .child("No repository selected"),
                    )
                    .into_any_element()
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.base)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
    }
}

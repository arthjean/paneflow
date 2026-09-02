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
    worktrees: Vec<DiffWorktree>,
    view: Option<gpui::Entity<DiffView>>,
}

pub struct MultiRepoDiffView {
    groups: Vec<Group>,
    selected: usize,
    base_ref: Option<String>,
    pub scope_slot: Option<gpui::AnyElement>,
}

impl MultiRepoDiffView {
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
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = &g.view
        {
            self.base_ref = Some(view.read(cx).base_ref().to_string());
        }
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

    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        for group in &self.groups {
            if let Some(view) = group.view.clone() {
                view.update(cx, |v, cx| v.suspend(cx));
            }
        }
    }

    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.groups.get(self.selected)
            && let Some(view) = g.view.clone()
        {
            view.update(cx, |v, cx| v.resume(cx));
        }
    }

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

    pub fn active_selected_column(&self, cx: &App) -> usize {
        self.groups
            .get(self.selected)
            .and_then(|g| g.view.as_ref())
            .map(|v| v.read(cx).selected_column())
            .unwrap_or(0)
    }

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

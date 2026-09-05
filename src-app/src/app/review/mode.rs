use gpui::{AnyElement, App, Context, IntoElement, ParentElement, Styled, Window, div, px};
use paneflow_config::schema::AppMode;

use super::{REVIEW_CHANGES_RAIL_WIDTH, REVIEW_WORKSPACES_RAIL_WIDTH};
use crate::{OpenDiffView, PaneFlowApp};

impl PaneFlowApp {
    pub(crate) fn handle_open_diff_view(
        &mut self,
        _: &OpenDiffView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            AppMode::Diff => self.enter_cli_mode(window, cx),
            AppMode::Cli => self.enter_diff_mode(window, cx),
        }
    }

    pub(crate) fn enter_diff_mode(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == AppMode::Diff {
            return;
        }
        self.mode = AppMode::Diff;
        self.review_resume_all(cx);
        self.review_refresh_worktree_listings(cx);
        if self.review.layout.is_none()
            && let Some(subject) = self.review_default_subject()
        {
            self.review_show_subject(subject, cx);
        } else if let Some(pane) = self.review_active_pane() {
            self.pending_pane_focus = Some(pane);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn enter_cli_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == AppMode::Cli {
            return;
        }
        self.review.dismiss_popovers();
        self.review_suspend_all(cx);
        self.mode = AppMode::Cli;
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn review_refresh_worktree_listings(&mut self, cx: &mut Context<Self>) {
        let mut seen = std::collections::HashSet::new();
        for ws_idx in 0..self.workspaces.len() {
            let Some(root) = self.workspaces[ws_idx].repo_root.clone() else {
                continue;
            };
            if seen.insert(root) {
                self.spawn_worktree_listing(ws_idx, cx);
            }
        }
    }

    pub(crate) fn review_rails_width() -> f32 {
        REVIEW_WORKSPACES_RAIL_WIDTH + REVIEW_CHANGES_RAIL_WIDTH
    }

    pub(crate) fn render_review_rails(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .h_full()
            .w(px(Self::review_rails_width()))
            .flex_shrink_0()
            .child(self.render_review_workspaces_rail(window, cx))
            .child(self.render_review_changes_rail(window, cx))
            .into_any_element()
    }

    pub(crate) fn render_review_main(
        &mut self,
        left_gutter: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        self.review_track_focus(window, cx);
        let Some(root) = self.review.layout.as_ref() else {
            return div()
                .flex()
                .size_full()
                .child(crate::ui_primitives::panel_empty_state(
                    ui,
                    Some("icons/git-branch.svg"),
                    Some("Choose a branch".into()),
                    "Pick a branch or worktree in the Workspaces rail to read its diff, or drag one here.",
                    false,
                ))
                .into_any_element();
        };
        let app_weak = cx.weak_entity();
        let on_resize_end = std::rc::Rc::new(move |cx: &mut App| {
            let _ = app_weak.update(cx, |app, cx| app.save_session(cx));
        });
        root.sync_unfocused_dim(window, cx);
        div()
            .flex()
            .size_full()
            .pl(px(left_gutter))
            .pr(px(crate::layout::PANE_GUTTER_PX))
            .pt(px(crate::layout::PANE_GUTTER_PX))
            .pb(px(crate::layout::PANE_GUTTER_PX))
            .child(root.render_with_preview(window, cx, Some(on_resize_end), None))
            .into_any_element()
    }
}

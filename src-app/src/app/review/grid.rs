use std::path::Path;

use gpui::{App, AppContext, Context, Entity, Focusable, Window};

use super::MAX_REVIEW_PANES;
use crate::PaneFlowApp;
use crate::diff::{DiffView, DiffWorktree, ReviewSubject};
use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::{Pane, PaneSurface};
use crate::pane_drag::DropEdge;

pub(crate) fn pane_view(pane: &Entity<Pane>, cx: &App) -> Option<Entity<DiffView>> {
    match &pane.read(cx).surface {
        PaneSurface::Diff(view) => Some(view.clone()),
        _ => None,
    }
}

pub(crate) fn pane_subject(pane: &Entity<Pane>, cx: &App) -> Option<ReviewSubject> {
    pane_view(pane, cx).map(|view| view.read(cx).subject())
}

impl PaneFlowApp {
    pub(crate) fn review_contains_pane(&self, pane: &Entity<Pane>) -> bool {
        self.review
            .layout
            .as_ref()
            .is_some_and(|root| root.contains_leaf(pane))
            || self
                .review
                .saved_layout
                .as_ref()
                .is_some_and(|saved| saved.contains_leaf(pane))
    }

    pub(crate) fn review_leaves(&self) -> Vec<Entity<Pane>> {
        self.review
            .full_layout()
            .map(LayoutTree::collect_leaves)
            .unwrap_or_default()
    }

    pub(crate) fn review_grid_subjects(&self, cx: &App) -> Vec<ReviewSubject> {
        self.review_leaves()
            .iter()
            .filter_map(|pane| pane_subject(pane, cx))
            .collect()
    }

    pub(crate) fn review_active_pane(&self) -> Option<Entity<Pane>> {
        self.review
            .active_pane
            .as_ref()
            .filter(|pane| self.review_contains_pane(pane))
            .cloned()
            .or_else(|| self.review.layout.as_ref().and_then(LayoutTree::first_leaf))
    }

    pub(crate) fn review_focused_view(&self, cx: &App) -> Option<Entity<DiffView>> {
        self.review_active_pane()
            .and_then(|pane| pane_view(&pane, cx))
    }

    pub(crate) fn review_focused_subject(&self, cx: &App) -> Option<ReviewSubject> {
        self.review_focused_view(cx)
            .map(|view| view.read(cx).subject())
    }

    pub(crate) fn review_track_focus(&mut self, window: &Window, cx: &App) {
        let focused = self
            .review
            .layout
            .as_ref()
            .and_then(|root| root.focused_pane(window, cx));
        if let Some(pane) = focused
            && self.review.active_pane.as_ref() != Some(&pane)
        {
            self.review.active_pane = Some(pane);
            self.review.selected_file = None;
            self.review.dismiss_popovers();
        }
    }

    fn review_pane_for_subject(&self, subject: &ReviewSubject, cx: &App) -> Option<Entity<Pane>> {
        self.review_leaves().into_iter().find(|pane| {
            pane_subject(pane, cx).is_some_and(|current| current.same_worktree(subject))
        })
    }

    pub(crate) fn review_can_add_pane(&self) -> bool {
        self.review
            .full_layout()
            .is_none_or(|root| root.leaf_count() < MAX_REVIEW_PANES)
    }

    pub(crate) fn review_new_pane(
        &mut self,
        subject: ReviewSubject,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let workspace_id = subject.worktree.workspace_id.unwrap_or(0);
        let view = cx.new(|cx| DiffView::new(subject, cx));
        self.create_pane_with_existing_surface(PaneSurface::Diff(view), workspace_id, cx)
    }

    fn review_set_pane_subject(
        &mut self,
        pane: &Entity<Pane>,
        subject: ReviewSubject,
        cx: &mut Context<Self>,
    ) {
        if let Some(old) = pane_view(pane, cx) {
            if old.read(cx).subject().same_worktree(&subject) {
                return;
            }
            old.update(cx, |view, cx| view.suspend(cx));
        }
        let workspace_id = subject.worktree.workspace_id.unwrap_or(0);
        let view = cx.new(|cx| DiffView::new(subject, cx));
        pane.update(cx, |pane, cx| {
            pane.surface = PaneSurface::Diff(view);
            pane.workspace_id = workspace_id;
            cx.notify();
        });
        self.review.selected_file = None;
    }

    pub(crate) fn review_show_subject(&mut self, subject: ReviewSubject, cx: &mut Context<Self>) {
        self.review.dismiss_popovers();
        if let Some(pane) = self.review_pane_for_subject(&subject, cx) {
            self.review.active_pane = Some(pane.clone());
            self.pending_pane_focus = Some(pane);
            cx.notify();
            return;
        }
        match self.review_active_pane() {
            Some(pane) => {
                self.review_set_pane_subject(&pane, subject, cx);
                self.review.active_pane = Some(pane.clone());
                self.pending_pane_focus = Some(pane);
            }
            None => {
                let pane = self.review_new_pane(subject, cx);
                self.review.layout = Some(LayoutTree::Leaf(pane.clone()));
                self.review.active_pane = Some(pane.clone());
                self.pending_pane_focus = Some(pane);
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn review_drop_subject(
        &mut self,
        target: Entity<Pane>,
        edge: Option<DropEdge>,
        subject: ReviewSubject,
        cx: &mut Context<Self>,
    ) {
        if !self.review_contains_pane(&target) {
            return;
        }
        self.review.dismiss_popovers();
        match edge {
            None => {
                self.review_set_pane_subject(&target, subject, cx);
                self.review.active_pane = Some(target.clone());
                self.pending_pane_focus = Some(target);
            }
            Some(edge) => {
                if self.review.is_zoomed() {
                    self.show_toast("Unzoom before splitting panes", cx);
                    return;
                }
                if !self.review_can_add_pane() {
                    self.show_toast(
                        format!("Maximum diff pane count reached ({MAX_REVIEW_PANES})"),
                        cx,
                    );
                    return;
                }
                let new_pane = self.review_new_pane(subject, cx);
                let inserted = self.review.layout.as_mut().is_some_and(|root| {
                    crate::app::event_handlers::split_pane_at_edge(
                        root,
                        &target,
                        edge,
                        new_pane.clone(),
                    )
                });
                if !inserted {
                    return;
                }
                self.review.active_pane = Some(new_pane.clone());
                self.pending_pane_focus = Some(new_pane);
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn review_split_pane(
        &mut self,
        pane: Entity<Pane>,
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.review.is_zoomed() {
            return Err("Unzoom before splitting panes".to_string());
        }
        if !self.review_can_add_pane() {
            return Err(format!(
                "Maximum diff pane count reached ({MAX_REVIEW_PANES})"
            ));
        }
        let Some(subject) = pane_subject(&pane, cx) else {
            return Err("That pane no longer exists".to_string());
        };
        let new_pane = self.review_new_pane(subject, cx);
        let inserted = self
            .review
            .layout
            .as_mut()
            .is_some_and(|root| root.split_at_pane(&pane, direction, new_pane.clone()));
        if !inserted {
            return Err("That pane no longer exists".to_string());
        }
        self.review.active_pane = Some(new_pane.clone());
        self.pending_pane_focus = Some(new_pane);
        self.save_session(cx);
        cx.notify();
        Ok(())
    }

    pub(crate) fn review_close_pane(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        if !self.review_contains_pane(&pane) {
            return;
        }
        if self.review.is_zoomed() {
            self.review_exit_zoom(cx);
        }
        let Some(root) = self.review.layout.take() else {
            return;
        };
        let (remaining, removed) = root.remove_pane(&pane);
        self.review.layout = remaining;
        if removed && let Some(view) = pane_view(&pane, cx) {
            view.update(cx, |view, cx| view.suspend(cx));
        }
        if self.review.active_pane.as_ref() == Some(&pane) {
            self.review.active_pane = self.review.layout.as_ref().and_then(LayoutTree::first_leaf);
            self.pending_pane_focus = self.review.active_pane.clone();
            self.review.selected_file = None;
        }
        self.review.dismiss_popovers();
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn review_toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_zoomed() {
            if let Some(pane) = self.review_exit_zoom(cx) {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
        } else {
            let Some(root) = self.review.layout.as_ref() else {
                return;
            };
            if root.leaf_count() <= 1 {
                return;
            }
            let Some(focused) = root
                .focused_pane(window, cx)
                .or_else(|| self.review_active_pane())
            else {
                return;
            };
            focused.update(cx, |pane, _| pane.zoomed = true);
            self.review.saved_layout = self.review.layout.take();
            self.review.layout = Some(LayoutTree::Leaf(focused.clone()));
            focused.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn review_exit_zoom(&mut self, cx: &mut App) -> Option<Entity<Pane>> {
        let zoomed = self.review.layout.as_ref().and_then(LayoutTree::first_leaf);
        let saved = self.review.saved_layout.take()?;
        self.review.layout = Some(saved);
        if let Some(pane) = &zoomed {
            pane.update(cx, |pane, _| pane.zoomed = false);
        }
        zoomed
    }

    pub(crate) fn review_suspend_all(&mut self, cx: &mut App) {
        for pane in self.review_leaves() {
            if let Some(view) = pane_view(&pane, cx) {
                view.update(cx, |view, cx| view.suspend(cx));
            }
        }
    }

    pub(crate) fn review_resume_all(&mut self, cx: &mut App) {
        for pane in self.review_leaves() {
            if let Some(view) = pane_view(&pane, cx) {
                view.update(cx, |view, cx| view.resume(cx));
            }
        }
    }

    pub(crate) fn review_prune_after_workspace_change(&mut self, cx: &mut Context<Self>) {
        let open: std::collections::HashSet<std::path::PathBuf> = self
            .workspaces
            .iter()
            .filter_map(|ws| ws.repo_root.clone())
            .collect();
        let stale: Vec<Entity<Pane>> = self
            .review_leaves()
            .into_iter()
            .filter(|pane| {
                pane_subject(pane, cx).is_none_or(|subject| !open.contains(&subject.repo_root))
            })
            .collect();
        for pane in stale {
            self.review_close_pane(pane, cx);
        }
        self.review.collapsed.retain(|root| open.contains(root));
    }

    pub(crate) fn review_forget_worktree(
        &mut self,
        repo_root: &Path,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        let stale: Vec<Entity<Pane>> = self
            .review_leaves()
            .into_iter()
            .filter(|pane| {
                pane_subject(pane, cx).is_some_and(|subject| {
                    subject.repo_root == repo_root && subject.worktree.path == path
                })
            })
            .collect();
        for pane in stale {
            self.review_close_pane(pane, cx);
        }
    }

    pub(crate) fn review_subject_for_workspace(
        &self,
        ws: &crate::workspace::Workspace,
    ) -> Option<ReviewSubject> {
        let repo_root = ws.repo_root.clone()?;
        let tab = ws.active_tab();
        let (path, branch) = match tab.worktree.as_ref() {
            Some(worktree) => (
                worktree.clone(),
                self.tab_checkout_git(tab)
                    .map(|git| git.branch.clone())
                    .unwrap_or_default(),
            ),
            None => (ws.worktree_root.clone(), ws.git_branch.clone()),
        };
        Some(ReviewSubject {
            repo_root,
            worktree: DiffWorktree {
                path,
                branch,
                workspace_id: Some(ws.id),
            },
        })
    }

    pub(crate) fn review_default_subject(&self) -> Option<ReviewSubject> {
        self.active_workspace()
            .and_then(|ws| self.review_subject_for_workspace(ws))
            .or_else(|| {
                self.workspaces
                    .iter()
                    .find_map(|ws| self.review_subject_for_workspace(ws))
            })
    }
}

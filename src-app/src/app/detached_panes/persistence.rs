use std::collections::HashSet;

use gpui::{App, Bounds, Context, Window, point, px, size};
use paneflow_config::schema::{
    DetachedPaneLocation, DetachedPaneSession, DetachedReviewSubject, MAX_DETACHED_PANE_WINDOWS,
};

use crate::{PaneFlowApp, layout::LayoutTree};

impl PaneFlowApp {
    pub(crate) fn serialize_detached_panes(&self, cx: &App) -> Vec<DetachedPaneSession> {
        let mut saved = Vec::new();
        for (workspace, ws) in self.workspaces.iter().enumerate() {
            for (tab, entry) in ws.tabs().iter().enumerate() {
                if let Some(root) = entry.saved_layout.as_ref().or(entry.root.as_ref()) {
                    collect_detached(
                        root,
                        |leaf| DetachedPaneLocation::Cli {
                            workspace,
                            tab,
                            leaf,
                        },
                        cx,
                        &mut saved,
                    );
                }
            }
        }
        if let Some(root) = self
            .review
            .saved_layout
            .as_ref()
            .or(self.review.layout.as_ref())
        {
            collect_detached(
                root,
                |leaf| DetachedPaneLocation::Review { leaf },
                cx,
                &mut saved,
            );
        }
        saved.truncate(MAX_DETACHED_PANE_WINDOWS);
        saved
    }

    pub(crate) fn restore_detached_panes(&mut self, source: &mut Window, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.pending_detached_panes);
        let mut restored = HashSet::new();
        for saved in pending.into_iter().take(MAX_DETACHED_PANE_WINDOWS) {
            if !saved.has_valid_bounds() || !restored.insert(saved.location) {
                continue;
            }
            let (root, leaf) = match saved.location {
                DetachedPaneLocation::Cli {
                    workspace,
                    tab,
                    leaf,
                } => (
                    self.workspaces
                        .get(workspace)
                        .and_then(|ws| ws.tabs().get(tab))
                        .and_then(|tab| tab.saved_layout.as_ref().or(tab.root.as_ref())),
                    leaf,
                ),
                DetachedPaneLocation::Review { leaf } => (
                    self.review
                        .saved_layout
                        .as_ref()
                        .or(self.review.layout.as_ref()),
                    leaf,
                ),
            };
            let Some(root) = root else {
                continue;
            };
            let panes = root.collect_leaves();
            let pane = match saved.location {
                DetachedPaneLocation::Review { .. } => {
                    saved.review_subject.as_ref().and_then(|subject| {
                        review_leaf_for_subject(
                            subject,
                            panes.iter().map(|pane| review_subject(pane.read(cx), cx)),
                        )
                        .and_then(|index| panes.get(index).cloned())
                    })
                }
                DetachedPaneLocation::Cli { .. } => {
                    if saved.layout_leaf_count != Some(panes.len()) {
                        continue;
                    }
                    panes.get(leaf).cloned()
                }
            };
            let Some(pane) = pane else {
                continue;
            };
            let bounds = Bounds::new(
                point(px(saved.x), px(saved.y)),
                size(px(saved.width), px(saved.height)),
            );
            if let Err(error) = self.detach_pane(pane, source, Some(bounds), cx) {
                log::warn!("Could not restore detached pane: {error:#}");
            }
        }
    }
}

fn collect_detached(
    root: &LayoutTree,
    location: impl Fn(usize) -> DetachedPaneLocation,
    cx: &App,
    saved: &mut Vec<DetachedPaneSession>,
) {
    let panes = root.collect_leaves();
    for (leaf, pane) in panes.iter().enumerate() {
        if let Some(placement) = pane.read(cx).detached {
            saved.push(DetachedPaneSession {
                location: location(leaf),
                review_subject: review_subject(pane.read(cx), cx),
                layout_leaf_count: Some(panes.len()),
                x: f32::from(placement.bounds.origin.x),
                y: f32::from(placement.bounds.origin.y),
                width: f32::from(placement.bounds.size.width),
                height: f32::from(placement.bounds.size.height),
            });
        }
    }
}

fn review_subject(pane: &crate::pane::Pane, cx: &App) -> Option<DetachedReviewSubject> {
    pane.surfaces().iter().find_map(|surface| {
        if let crate::pane::PaneSurface::Diff(diff) = surface {
            let subject = diff.read(cx).subject();
            Some(DetachedReviewSubject {
                repo_root: subject.repo_root.to_string_lossy().into_owned(),
                worktree: subject.worktree.path.to_string_lossy().into_owned(),
            })
        } else {
            None
        }
    })
}

fn review_leaf_for_subject(
    wanted: &DetachedReviewSubject,
    subjects: impl Iterator<Item = Option<DetachedReviewSubject>>,
) -> Option<usize> {
    let mut matching = subjects
        .enumerate()
        .filter_map(|(index, subject)| (subject.as_ref() == Some(wanted)).then_some(index));
    let first = matching.next()?;
    matching.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(worktree: &str) -> DetachedReviewSubject {
        DetachedReviewSubject {
            repo_root: "repo".into(),
            worktree: worktree.into(),
        }
    }

    #[test]
    fn restoring_review_window_follows_subject_after_missing_worktree_is_pruned() {
        let wanted = subject("kept");
        assert_eq!(
            review_leaf_for_subject(&wanted, [Some(subject("kept"))].into_iter()),
            Some(0)
        );
        assert_eq!(
            review_leaf_for_subject(&subject("removed"), [Some(subject("kept"))].into_iter()),
            None
        );
        assert_eq!(
            review_leaf_for_subject(
                &wanted,
                [Some(subject("kept")), Some(subject("kept"))].into_iter()
            ),
            None
        );
    }
}

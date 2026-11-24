use std::collections::HashSet;

use gpui::{App, Bounds, Context, Entity, Window, point, px, size};
use paneflow_config::schema::{
    DetachedPaneLocation, DetachedPaneSession, MAX_DETACHED_PANE_WINDOWS,
};

use crate::pane::Pane;
use crate::{PaneFlowApp, layout::LayoutTree};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingPlacement {
    pub(crate) saved: DetachedPaneSession,
    pub(crate) from_window_state: bool,
}

pub(crate) fn pending_placements(
    window_state: Vec<crate::window_state::PersistedDetached>,
    legacy: Vec<DetachedPaneSession>,
) -> Vec<PendingPlacement> {
    let mut placements: Vec<PendingPlacement> = window_state
        .into_iter()
        .map(|placed| PendingPlacement {
            saved: DetachedPaneSession {
                location: DetachedPaneLocation {
                    workspace: usize::MAX,
                    tab: usize::MAX,
                    leaf: usize::MAX,
                },
                layout_leaf_count: None,
                pane: Some(placed.pane),
                x: placed.x,
                y: placed.y,
                width: placed.width,
                height: placed.height,
            },
            from_window_state: true,
        })
        .collect();
    let known: HashSet<Option<u64>> = placements.iter().map(|placed| placed.saved.pane).collect();
    placements.extend(
        legacy
            .into_iter()
            .filter(|saved| saved.pane.is_none() || !known.contains(&saved.pane))
            .map(|saved| PendingPlacement {
                saved,
                from_window_state: false,
            }),
    );
    placements
}

impl PaneFlowApp {
    pub(crate) fn serialize_detached_panes(&self, cx: &App) -> Vec<DetachedPaneSession> {
        let mut saved = Vec::new();
        for (workspace, ws) in self.workspaces.iter().enumerate() {
            for (tab, entry) in ws.tabs().iter().enumerate() {
                if let Some(root) = entry.saved_layout.as_ref().or(entry.root.as_ref()) {
                    self.collect_detached(root, workspace, tab, cx, &mut saved);
                }
            }
        }
        saved.truncate(MAX_DETACHED_PANE_WINDOWS);
        saved
    }

    fn collect_detached(
        &self,
        root: &LayoutTree,
        workspace: usize,
        tab: usize,
        cx: &App,
        saved: &mut Vec<DetachedPaneSession>,
    ) {
        let panes = root.collect_leaves();
        for (leaf, pane) in panes.iter().enumerate() {
            if let Some(placement) = pane.read(cx).detached {
                saved.push(DetachedPaneSession {
                    pane: self.layout_pane_id(pane).map(|id| id.0),
                    location: DetachedPaneLocation {
                        workspace,
                        tab,
                        leaf,
                    },
                    layout_leaf_count: Some(panes.len()),
                    x: f32::from(placement.bounds.origin.x),
                    y: f32::from(placement.bounds.origin.y),
                    width: f32::from(placement.bounds.size.width),
                    height: f32::from(placement.bounds.size.height),
                });
            }
        }
    }

    pub(crate) fn restore_detached_panes(&mut self, source: &mut Window, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.pending_detached_panes);
        if pending.is_empty() {
            return;
        }
        self.adopt_layout(cx);
        let mut restored = HashSet::new();
        let mut dropped = 0usize;
        for PendingPlacement {
            saved,
            from_window_state,
        } in pending.into_iter().take(MAX_DETACHED_PANE_WINDOWS)
        {
            if !saved.has_valid_bounds() {
                dropped += usize::from(!from_window_state);
                continue;
            }
            let Some(pane) = self.detached_pane_of(&saved) else {
                dropped += usize::from(!from_window_state);
                continue;
            };
            if !restored.insert(pane.entity_id()) {
                continue;
            }
            let bounds = Bounds::new(
                point(px(saved.x), px(saved.y)),
                size(px(saved.width), px(saved.height)),
            );
            if let Err(error) = self.detach_pane(pane, source, Some(bounds), cx) {
                log::warn!("Could not restore detached pane: {error:#}");
            }
        }
        if dropped > 0 {
            log::warn!(
                "session restore: {dropped} detached pane placement(s) name no pane and were dropped"
            );
        }
    }

    fn detached_pane_of(&self, saved: &DetachedPaneSession) -> Option<Entity<Pane>> {
        if let Some(pane) = saved
            .pane
            .and_then(|id| self.layout_pane(paneflow_layout::PaneId(id)))
        {
            return Some(pane);
        }
        let DetachedPaneLocation {
            workspace,
            tab,
            leaf,
        } = saved.location;
        let root = self
            .workspaces
            .get(workspace)
            .and_then(|ws| ws.tabs().get(tab))
            .and_then(|tab| tab.saved_layout.as_ref().or(tab.root.as_ref()))?;
        let panes = root.collect_leaves();
        if saved.layout_leaf_count != Some(panes.len()) {
            return None;
        }
        panes.get(leaf).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(pane: Option<u64>) -> DetachedPaneSession {
        DetachedPaneSession {
            location: DetachedPaneLocation {
                workspace: 0,
                tab: 0,
                leaf: 1,
            },
            layout_leaf_count: Some(2),
            pane,
            x: 1.,
            y: 2.,
            width: 300.,
            height: 200.,
        }
    }

    #[test]
    fn window_state_placements_come_first_and_a_legacy_copy_of_the_same_pane_is_dropped() {
        let placed = crate::window_state::PersistedDetached {
            pane: 7,
            x: 5.,
            y: 6.,
            width: 700.,
            height: 500.,
        };

        let pending = pending_placements(vec![placed], vec![legacy(Some(7)), legacy(None)]);

        assert_eq!(pending.len(), 2);
        assert!(pending[0].from_window_state);
        assert_eq!(pending[0].saved.pane, Some(7));
        assert_eq!(pending[0].saved.width, 700.);
        assert_eq!(pending[0].saved.layout_leaf_count, None);
        assert!(!pending[1].from_window_state);
        assert_eq!(pending[1].saved.pane, None);
    }

    #[test]
    fn a_session_file_of_the_previous_release_feeds_its_placements_once() {
        let pending = pending_placements(Vec::new(), vec![legacy(Some(3))]);
        assert_eq!(
            pending,
            vec![PendingPlacement {
                saved: legacy(Some(3)),
                from_window_state: false,
            }]
        );
    }
}

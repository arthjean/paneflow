use std::collections::HashSet;

use gpui::{App, Bounds, Context, Entity, Window, point, px, size};
use paneflow_config::schema::{
    DetachedPaneLocation, DetachedPaneSession, MAX_DETACHED_PANE_WINDOWS,
};

use crate::pane::Pane;
use crate::{PaneFlowApp, layout::LayoutTree};

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
        for saved in pending.into_iter().take(MAX_DETACHED_PANE_WINDOWS) {
            if !saved.has_valid_bounds() {
                dropped += 1;
                continue;
            }
            let Some(pane) = self.detached_pane_of(&saved) else {
                dropped += 1;
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

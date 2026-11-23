use gpui::Context;
use paneflow_layout::{Op, Snapshot, SurfaceId};
use serde_json::Value;

use super::client::{Incoming, LayoutLink, Outgoing, Request};
use crate::PaneFlowApp;
use crate::layout::MAX_PANES;

const DRAIN_MAX_PER_TICK: usize = 256;
pub(crate) const LAYOUT_REFRESHED: &str = "The layout was refreshed from the background process";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Local,
    Attaching,
    Live,
    Away,
}

pub(crate) struct LayoutSync {
    link: Option<LayoutLink>,
    phase: Phase,
    generation: u64,
    epoch: Option<String>,
    seq: u64,
    pending: u64,
    acked_revision: u64,
    pub(crate) file_revision: u64,
    without_layout: bool,
    diverged: bool,
    mistrusted: bool,
    view_seq: Option<u64>,
    lease_seq: Option<u64>,
    adopt_seq: Option<u64>,
    revision_at_adopt: u64,
    seen_revision: u64,
    seen_unrevisioned: bool,
}

impl Default for LayoutSync {
    fn default() -> Self {
        Self {
            link: None,
            phase: Phase::Local,
            generation: 0,
            epoch: None,
            seq: 0,
            pending: 0,
            acked_revision: 0,
            file_revision: 0,
            without_layout: false,
            diverged: false,
            mistrusted: false,
            view_seq: None,
            lease_seq: None,
            adopt_seq: None,
            revision_at_adopt: 0,
            seen_revision: 0,
            seen_unrevisioned: false,
        }
    }
}

impl LayoutSync {
    pub(crate) fn is_live(&self) -> bool {
        matches!(self.phase, Phase::Live | Phase::Attaching)
    }

    pub(crate) fn has_link(&self) -> bool {
        self.link.is_some()
    }

    pub(crate) fn is_drained(&self) -> bool {
        self.pending == 0
    }

    pub(crate) fn worker_lacks_layout(&self) -> bool {
        self.without_layout
    }

    #[cfg(test)]
    pub(crate) fn sent(&self) -> u64 {
        self.seq
    }

    #[cfg(test)]
    pub(crate) fn is_settled(&self) -> bool {
        self.phase == Phase::Live
            && self.pending == 0
            && !self.diverged
            && self.view_seq.is_none()
            && self.lease_seq.is_none()
            && self.adopt_seq.is_none()
    }

    pub(crate) fn attach_link(&mut self, link: LayoutLink) {
        self.link = Some(link);
    }

    pub(crate) fn launch_route(&self) -> Option<crate::terminal::host_link::LaunchRoute> {
        if self.phase != Phase::Live {
            return None;
        }
        self.link.as_ref().map(LayoutLink::launch_route)
    }

    pub(crate) fn sweeper(&self) -> Option<super::client::Sweeper> {
        if self.phase != Phase::Live {
            return None;
        }
        self.link.as_ref().map(LayoutLink::sweeper)
    }

    fn push(&mut self, request: Request) -> Option<u64> {
        let link = self.link.as_ref()?;
        self.seq += 1;
        self.pending += 1;
        link.send(Outgoing {
            generation: self.generation,
            seq: self.seq,
            request,
        });
        Some(self.seq)
    }

    fn settle(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    pub(crate) fn record_op(&mut self, op: &Op, ids_from: u64, revision: u64) {
        if self.phase != Phase::Live {
            return;
        }
        match serde_json::to_value(op) {
            Ok(op) => {
                self.push(Request::Op {
                    op,
                    ids_from,
                    expected: revision,
                });
            }
            Err(error) => {
                log::warn!("paneflow: a layout op did not encode: {error}");
                self.diverged = true;
                self.mistrusted = true;
            }
        }
    }

    pub(crate) fn record_sync(&mut self, snapshot: &Snapshot, before: u64) {
        if self.phase != Phase::Live {
            return;
        }
        if let Ok(view) = serde_json::to_value(snapshot) {
            self.push(Request::Adopt {
                view,
                acknowledged: before,
                reconnect: false,
            });
        }
    }

    pub(crate) fn wants_lease(&self, remaining: Option<u64>) -> bool {
        self.phase == Phase::Live
            && self.lease_seq.is_none()
            && remaining.is_some_and(|left| left < super::client::LEASE_REFILL_BELOW)
    }

    pub(crate) fn request_lease(&mut self) {
        self.lease_seq = self.push(Request::Lease);
    }

    fn note_changed(&mut self, revision: Option<u64>) {
        if self.phase != Phase::Live {
            return;
        }
        match revision {
            Some(revision) => self.seen_revision = self.seen_revision.max(revision),
            None => self.seen_unrevisioned = true,
        }
    }

    fn absorb_changes(&mut self, model_revision: u64) {
        if self.phase != Phase::Live || self.pending != 0 {
            return;
        }
        if self.seen_unrevisioned || self.seen_revision > model_revision {
            self.diverged = true;
        }
        self.seen_unrevisioned = false;
    }

    fn acknowledged_for_reconnect(&self, epoch: &str) -> u64 {
        match &self.epoch {
            Some(previous) if previous == epoch => self.acked_revision,
            _ => self.acked_revision.max(self.file_revision),
        }
    }
}

pub(crate) fn refusal_message(op: &Value, reason: &str) -> Option<String> {
    let kind = op["op"].as_str().unwrap_or_default();
    let message = match (kind, reason) {
        ("split", "zoomed") => "Unzoom before splitting panes".to_string(),
        ("split", "pane_limit" | "ids_exhausted") => {
            format!("Maximum pane count reached ({MAX_PANES})")
        }
        ("split", _) => "That pane no longer exists".to_string(),
        ("undo_close", "zoomed") => "Unzoom before restoring a closed pane".to_string(),
        ("undo_close", "pane_limit" | "ids_exhausted") => {
            format!("Maximum pane count reached ({MAX_PANES})")
        }
        ("undo_close", "tab_limit") => "Tab limit reached for this workspace".to_string(),
        ("undo_close", _) => "No closed pane to restore".to_string(),
        _ => return None,
    };
    Some(message)
}

impl PaneFlowApp {
    pub(crate) fn start_layout_client(&mut self) {
        if self.layout.sync.has_link() || self.session_restore_failed {
            return;
        }
        let Some(endpoint) = paneflow_home::serve_endpoint_path_for_current_home() else {
            return;
        };
        if let Some(link) = super::client::LayoutLink::start(endpoint, Some(self.app_wake.clone()))
        {
            self.layout.sync.attach_link(link);
        }
    }

    pub(crate) fn note_layout_file_revision(&mut self, revision: u64) {
        self.layout.sync.file_revision = revision;
        if revision > self.layout.model.revision() {
            self.layout.model.set_revision(revision);
        }
    }

    pub(crate) fn unbind_layout_sessions(&mut self) {
        let mut state = self.layout.model.state().clone();
        for pane in state
            .workspaces
            .iter_mut()
            .flat_map(|ws| ws.tabs.iter_mut())
            .filter_map(|tab| tab.root.as_mut())
            .flat_map(paneflow_layout::Node::panes_mut)
        {
            for surface in pane
                .surfaces
                .iter_mut()
                .filter(|surface| surface.kind() == paneflow_layout::SurfaceKind::Terminal)
            {
                surface.definition.session = None;
                surface.definition.agent = None;
            }
        }
        self.layout.resync_recorded(state);
    }

    pub(crate) fn layout_is_attached(&self) -> bool {
        self.layout.sync.is_live()
    }

    pub(crate) fn layout_sync_drained(&self) -> bool {
        !self.layout.sync.is_live() || self.layout.sync.is_drained()
    }

    pub(crate) fn process_layout_client(&mut self, cx: &mut Context<Self>) {
        let mut received = Vec::new();
        if let Some(link) = self.layout.sync.link.as_ref() {
            while received.len() < DRAIN_MAX_PER_TICK {
                let Some(incoming) = link.try_recv() else {
                    break;
                };
                received.push(incoming);
            }
        }
        if received.len() == DRAIN_MAX_PER_TICK {
            self.app_wake.notify();
        }
        for incoming in received {
            self.handle_layout_incoming(incoming, cx);
        }
        self.converge_layout();
    }

    fn handle_layout_incoming(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
        match incoming {
            Incoming::Unavailable => {
                self.layout.sync.without_layout = false;
                if self.layout.sync.phase != Phase::Local {
                    self.layout.sync.phase = Phase::Away;
                }
            }
            Incoming::WithoutLayout => {
                if !self.layout.sync.without_layout {
                    log::info!(
                        "layout: the background process predates layout.authority, the window keeps the layout"
                    );
                }
                self.layout.sync.without_layout = true;
                if self.layout.sync.phase != Phase::Local {
                    self.layout.sync.phase = Phase::Away;
                }
            }
            Incoming::Disconnected => {
                if self.layout.sync.phase != Phase::Local {
                    self.layout.sync.phase = Phase::Away;
                }
                self.layout.sync.pending = 0;
                self.layout.sync.view_seq = None;
                self.layout.sync.lease_seq = None;
                self.layout.sync.adopt_seq = None;
                self.layout.sync.seen_revision = 0;
                self.layout.sync.seen_unrevisioned = false;
            }
            Incoming::Attached { generation, epoch } => self.layout_attached(generation, epoch),
            Incoming::Adopted {
                seq,
                accepted,
                revision,
                view,
                reconnect,
            } => {
                self.layout.sync.settle();
                let startup = self.layout.sync.adopt_seq == Some(seq);
                if startup {
                    self.layout.sync.adopt_seq = None;
                    self.layout.sync.phase = Phase::Live;
                    if self.owned_sessions_sweep_waits() {
                        self.refresh_owned_sessions(cx);
                    }
                }
                if accepted {
                    self.layout.sync.acked_revision = revision;
                    if !startup
                        && self.layout.sync.pending == 0
                        && revision != self.layout.model.revision()
                    {
                        self.layout.sync.diverged = true;
                        self.layout.sync.mistrusted = true;
                    }
                    if startup {
                        let changed_since =
                            self.layout.model.revision() != self.layout.sync.revision_at_adopt;
                        self.layout.model.set_revision(revision);
                        if changed_since {
                            let snapshot = self.layout.model.snapshot();
                            self.layout.sync.record_sync(&snapshot, revision);
                            self.layout.model.set_revision(revision + 1);
                        }
                    }
                } else if let Some(view) = view {
                    self.layout.sync.acked_revision = revision;
                    self.reconcile_layout(*view, cx);
                    if reconnect {
                        self.show_toast(LAYOUT_REFRESHED, cx);
                    }
                }
            }
            Incoming::Acked {
                revision, expected, ..
            } => {
                self.layout.sync.settle();
                self.layout.sync.acked_revision = revision;
                if revision != expected {
                    self.layout.sync.diverged = true;
                    self.layout.sync.mistrusted = true;
                }
            }
            Incoming::Refused { reason, op, .. } => {
                self.layout.sync.settle();
                self.layout.sync.diverged = true;
                self.layout.sync.mistrusted = true;
                log::debug!("layout: the worker refused {}: {reason}", op["op"]);
                if let Some(message) = refusal_message(&op, &reason) {
                    self.show_toast(message, cx);
                }
            }
            Incoming::Leased { seq, block } => {
                self.layout.sync.settle();
                if self.layout.sync.lease_seq == Some(seq) {
                    self.layout.sync.lease_seq = None;
                }
                self.layout.model.ids().grant(block);
            }
            Incoming::Viewed {
                seq,
                revision,
                view,
            } => {
                self.layout.sync.settle();
                if self.layout.sync.view_seq == Some(seq) {
                    self.layout.sync.view_seq = None;
                }
                if seq == self.layout.sync.seq {
                    self.layout.sync.diverged = false;
                    self.layout.sync.acked_revision = revision;
                    match view {
                        Some(view) if view.state != *self.layout.model.state() => {
                            self.reconcile_layout(*view, cx);
                        }
                        Some(view) => {
                            self.layout.model.restore(*view);
                        }
                        None => {}
                    }
                }
            }
            Incoming::Changed { revision } => self.layout.sync.note_changed(revision),
        }
    }

    fn layout_attached(&mut self, generation: u64, epoch: String) {
        let sync = &mut self.layout.sync;
        let reconnect = sync.epoch.is_some();
        let new_epoch = sync.epoch.as_deref() != Some(epoch.as_str());
        let acknowledged = sync.acknowledged_for_reconnect(&epoch);
        sync.generation = generation;
        sync.pending = 0;
        sync.view_seq = None;
        sync.lease_seq = None;
        sync.diverged = false;
        sync.mistrusted = false;
        sync.seen_revision = 0;
        sync.seen_unrevisioned = false;
        sync.epoch = Some(epoch);
        sync.without_layout = false;
        sync.phase = Phase::Attaching;
        if new_epoch {
            self.layout.model.ids().await_lease();
        }
        let snapshot = self.layout.model.snapshot();
        let Ok(view) = serde_json::to_value(&snapshot) else {
            return;
        };
        let sync = &mut self.layout.sync;
        sync.revision_at_adopt = snapshot.revision;
        sync.adopt_seq = sync.push(Request::Adopt {
            view,
            acknowledged,
            reconnect,
        });
        if new_epoch || !self.layout.model.ids().is_leased() {
            let sync = &mut self.layout.sync;
            sync.lease_seq = sync.push(Request::Lease);
        }
    }

    fn converge_layout(&mut self) {
        let revision = self.layout.model.revision();
        let sync = &mut self.layout.sync;
        sync.absorb_changes(revision);
        if sync.phase == Phase::Live
            && sync.diverged
            && sync.pending == 0
            && sync.view_seq.is_none()
        {
            let unless_revision = (!sync.mistrusted).then(|| self.layout.model.revision());
            let sync = &mut self.layout.sync;
            sync.mistrusted = false;
            sync.view_seq = sync.push(Request::View { unless_revision });
        }
        let remaining = self.layout.model.ids().remaining();
        if self.layout.sync.wants_lease(remaining) {
            self.layout.sync.request_lease();
        }
    }

    pub(crate) fn reconcile_layout(&mut self, view: Snapshot, cx: &mut Context<Self>) {
        let focused_before = model_focus(self.layout.model.state());
        self.layout.model.restore(view);
        let missing: Vec<(SurfaceId, Option<paneflow_config::schema::SessionId>)> = self
            .layout
            .model
            .state()
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .filter_map(|tab| tab.root.as_ref())
            .flat_map(paneflow_layout::Node::panes)
            .flat_map(|pane| pane.surfaces.iter())
            .filter(|surface| self.layout.surface(surface.id).is_none())
            .map(|surface| (surface.id, surface.session().cloned()))
            .collect();
        for (surface, session) in &missing {
            match session {
                Some(session) => self.reattach_layout_surface(*surface, session.clone(), cx),
                None => self.spawn_layout_surface(*surface, cx),
            }
        }
        self.project_layout(cx);
        self.layout.prune();
        let focused_after = model_focus(self.layout.model.state());
        if focused_after != focused_before
            && let Some(pane) = focused_after.and_then(|(_, pane)| self.layout.pane(pane))
        {
            self.pending_pane_focus = Some(pane);
        }
        cx.notify();
    }
}

fn model_focus(
    state: &paneflow_layout::State,
) -> Option<(paneflow_layout::WorkspaceId, paneflow_layout::PaneId)> {
    let ws = state.workspaces.get(state.active_workspace)?;
    Some((ws.id, ws.active_tab()?.focused?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> LayoutSync {
        LayoutSync {
            phase: Phase::Live,
            ..LayoutSync::default()
        }
    }

    #[test]
    fn a_foreign_change_seen_while_a_request_is_in_flight_is_fetched_once_it_is_answered() {
        let mut sync = live();
        sync.pending = 1;
        sync.note_changed(Some(12));
        sync.absorb_changes(11);
        assert!(!sync.diverged, "the answer in flight may still explain it");

        sync.settle();
        sync.absorb_changes(11);
        assert!(
            sync.diverged,
            "revision 12 was announced and the model is at 11"
        );
    }

    #[test]
    fn a_change_the_window_already_holds_asks_for_nothing() {
        let mut sync = live();
        sync.note_changed(Some(11));
        sync.absorb_changes(11);
        assert!(!sync.diverged);

        sync.note_changed(None);
        sync.absorb_changes(11);
        assert!(sync.diverged, "an event without a revision is checked");
        sync.diverged = false;
        sync.absorb_changes(11);
        assert!(!sync.diverged, "and checked once");
    }

    #[test]
    fn a_change_announced_while_the_worker_is_away_is_left_to_the_adopt() {
        let mut sync = LayoutSync {
            phase: Phase::Away,
            ..LayoutSync::default()
        };
        sync.note_changed(Some(40));
        sync.phase = Phase::Live;
        sync.absorb_changes(11);
        assert!(!sync.diverged);
    }
}

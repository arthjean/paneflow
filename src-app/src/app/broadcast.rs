//! Broadcast Groups (EP-001, CLI Cockpit).
//!
//! US-002: named groups of panes sharing a colored left-edge stripe - the
//! explicit, visible target of a Composer broadcast (US-003). Groups are
//! session-volatile by design (Technical Considerations: persistence is a
//! future extension), live on `PaneFlowApp` (single-thread GPUI state,
//! no Arc/Mutex) and reference panes by `EntityId`. A pane belongs to at
//! most ONE group (v1). The picker reuses the theme-picker modal scaffold
//! (dedicated focus handle + manual key handler + deferred backdrop).

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::pane::Pane;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

/// Hard cap on simultaneously defined groups - one per `UiColors` stripe
/// slot (`group_1..group_8`). Creating a 9th is refused with an explicit
/// message (US-002 AC3).
pub(crate) const MAX_GROUPS: usize = 8;

/// A named broadcast group. Generic over the member id so the membership
/// logic is unit-testable without constructing GPUI entities (`EntityId`
/// has no public constructor); production code uses the default.
pub(crate) struct BroadcastGroup<Id = gpui::EntityId> {
    pub(crate) name: String,
    /// Index into the eight `UiColors::group_*` stripe slots.
    pub(crate) color_idx: usize,
    /// Member panes. May hold ids of closed panes between syncs - readers
    /// always intersect with the live leaves (US-002 AC4: a closed pane
    /// disappears silently; an empty group stays valid).
    pub(crate) members: Vec<Id>,
}

/// All broadcast state owned by `PaneFlowApp`. Volatile - none of it is
/// persisted to session.json in v1.
#[derive(Default)]
pub(crate) struct BroadcastState {
    pub(crate) groups: Vec<BroadcastGroup>,
    /// Index of the active group (the target of member toggles and of the
    /// Composer's broadcast mode). `None` until the user picks one.
    pub(crate) active: Option<usize>,
    /// US-003: per-terminal queued prompt, keyed by surface id (terminal
    /// `EntityId::as_u64`). One slot per pane, latest-wins - a new
    /// broadcast to the same busy pane REPLACES the buffer. Flushed
    /// (prefill only, never submitted - FR-02) when the mapped session
    /// leaves `Thinking`; dropped silently if the terminal disappears
    /// first.
    pub(crate) pending: HashMap<u64, String>,
}

/// Toggle `pane` in `groups[active]`. A pane belongs to at most one group
/// (US-002 AC5), so joining the active group removes it from any other.
/// Returns `true` when the pane is a member of the active group after the
/// call.
pub(crate) fn toggle_member<Id: PartialEq + Copy>(
    groups: &mut [BroadcastGroup<Id>],
    active: usize,
    pane: Id,
) -> bool {
    let was_in_active = groups
        .get(active)
        .is_some_and(|g| g.members.contains(&pane));
    for g in groups.iter_mut() {
        g.members.retain(|m| *m != pane);
    }
    if was_in_active {
        false
    } else if let Some(g) = groups.get_mut(active) {
        g.members.push(pane);
        true
    } else {
        false
    }
}

/// First stripe slot not used by an existing group, so deleting and
/// re-creating groups recycles colors deterministically. Falls back to a
/// modulo wrap when all 8 are taken (callers gate creation on
/// [`MAX_GROUPS`] before asking for a color).
pub(crate) fn next_free_color<Id>(groups: &[BroadcastGroup<Id>]) -> usize {
    (0..MAX_GROUPS)
        .find(|i| !groups.iter().any(|g| g.color_idx == *i))
        .unwrap_or(groups.len() % MAX_GROUPS)
}

/// FR-02: a prompt may only be DELIVERED (pre-filled) into a pane whose
/// mapped session is not generating. A pane with no agent session is
/// always safe (US-001: the gate never requires an agent - bare shells
/// receive the prefill identically).
///
/// EP-004 US-011: `Stalled` blocks too - a stalled session is a `Thinking`
/// session that went silent, i.e. the agent may STILL be mid-generation
/// (the false-positive case the PRD documents). Treating it as safe would
/// reopen the exact stdin-corruption window FR-02 exists to close. The
/// buffer flushes on the next hook event, which also clears `Stalled`.
pub(crate) fn state_blocks_delivery(state: &crate::ai_types::AgentState) -> bool {
    matches!(
        state,
        crate::ai_types::AgentState::Thinking | crate::ai_types::AgentState::Stalled
    )
}

/// Validation shared by create + rename so the picker shows one error
/// vocabulary. Pure for unit tests.
pub(crate) fn validate_group_name<Id>(
    groups: &[BroadcastGroup<Id>],
    name: &str,
    renaming: Option<usize>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Group name is empty".to_string());
    }
    // Security review: names render in picker rows, the Composer group
    // label, and toasts - bound them so a runaway input can't inflate
    // every re-push and repaint.
    if name.chars().count() > 64 {
        return Err("Group name is too long (max 64 characters)".to_string());
    }
    if renaming.is_none() && groups.len() >= MAX_GROUPS {
        return Err("Limit of 8 groups reached".to_string());
    }
    if groups
        .iter()
        .enumerate()
        .any(|(i, g)| Some(i) != renaming && g.name == name)
    {
        return Err(format!("Group \"{name}\" already exists"));
    }
    Ok(())
}

impl PaneFlowApp {
    /// The pane a cockpit gesture targets: the focused leaf, or the first
    /// leaf of the active workspace as a fallback so the shortcut still
    /// works right after a workspace switch (no leaf focused yet).
    pub(crate) fn focused_or_first_pane(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<gpui::Entity<Pane>> {
        let root = self.active_workspace()?.root.as_ref()?;
        root.focused_pane(window, cx).or_else(|| root.first_leaf())
    }

    /// Live `Entity<Pane>` handles of the active group's members, in tree
    /// order across all workspaces. Stale ids (closed panes) are skipped -
    /// US-002 AC4.
    pub(crate) fn live_active_group_members(&self, _cx: &Context<Self>) -> Vec<gpui::Entity<Pane>> {
        let Some(group) = self
            .broadcast
            .active
            .and_then(|i| self.broadcast.groups.get(i))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for ws in &self.workspaces {
            if let Some(root) = &ws.root {
                for pane in root.collect_leaves() {
                    if group.members.contains(&pane.entity_id()) {
                        out.push(pane);
                    }
                }
            }
        }
        out
    }

    /// Push group membership down into the panes as a stripe color index
    /// and prune members whose pane no longer exists. Mirrors
    /// `sync_attention`: recomputed idempotently from the group truth.
    pub(crate) fn sync_broadcast_stripes(&mut self, cx: &mut Context<Self>) {
        let mut live: HashSet<gpui::EntityId> = HashSet::new();
        let mut leaves: Vec<gpui::Entity<Pane>> = Vec::new();
        for ws in &self.workspaces {
            if let Some(root) = &ws.root {
                for pane in root.collect_leaves() {
                    live.insert(pane.entity_id());
                    leaves.push(pane);
                }
            }
        }
        for g in &mut self.broadcast.groups {
            g.members.retain(|m| live.contains(m));
        }
        let mut color_of: HashMap<gpui::EntityId, usize> = HashMap::new();
        for g in &self.broadcast.groups {
            for m in &g.members {
                color_of.insert(*m, g.color_idx);
            }
        }
        for pane in leaves {
            let color = color_of.get(&pane.entity_id()).copied();
            pane.update(cx, |p, cx| p.set_broadcast_stripe(color, cx));
        }
    }

    // -- Actions ----------------------------------------------------------

    pub(crate) fn handle_toggle_broadcast_member(
        &mut self,
        _: &crate::ToggleBroadcastMember,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.mode, paneflow_config::schema::AppMode::Cli) {
            return;
        }
        let Some(pane) = self.focused_or_first_pane(window, cx) else {
            return;
        };
        match self.broadcast.active {
            Some(active) if active < self.broadcast.groups.len() => {
                toggle_member(&mut self.broadcast.groups, active, pane.entity_id());
                self.sync_broadcast_stripes(cx);
                self.refresh_composer_slot(cx);
                cx.notify();
            }
            // No (valid) active group yet: route to the picker, whose empty
            // state proposes creation (US-002 AC6) - never a silent no-op.
            _ => self.open_broadcast_picker(window, cx),
        }
    }

    pub(crate) fn handle_open_broadcast_groups(
        &mut self,
        _: &crate::OpenBroadcastGroups,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.mode, paneflow_config::schema::AppMode::Cli) {
            return;
        }
        if self.broadcast_picker_open {
            self.close_broadcast_picker(cx);
        } else {
            self.open_broadcast_picker(window, cx);
        }
    }

    // -- Picker (theme-picker scaffold) ------------------------------------

    pub(crate) fn open_broadcast_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.broadcast_picker_open = true;
        self.broadcast_picker_query.clear();
        self.broadcast_picker_selected = self.broadcast.active.unwrap_or(0);
        self.broadcast_picker_renaming = None;
        self.broadcast_picker_error = None;
        self.broadcast_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_broadcast_picker(&mut self, cx: &mut Context<Self>) {
        self.broadcast_picker_open = false;
        self.broadcast_picker_query.clear();
        self.broadcast_picker_renaming = None;
        self.broadcast_picker_error = None;
        cx.notify();
    }

    fn create_broadcast_group(&mut self, name: &str, cx: &mut Context<Self>) {
        match validate_group_name(&self.broadcast.groups, name, None) {
            Ok(()) => {
                let color_idx = next_free_color(&self.broadcast.groups);
                self.broadcast.groups.push(BroadcastGroup {
                    name: name.trim().to_string(),
                    color_idx,
                    members: Vec::new(),
                });
                self.broadcast.active = Some(self.broadcast.groups.len() - 1);
                self.close_broadcast_picker(cx);
            }
            Err(e) => {
                self.broadcast_picker_error = Some(e);
                cx.notify();
            }
        }
    }

    fn commit_broadcast_rename(&mut self, idx: usize, name: &str, cx: &mut Context<Self>) {
        match validate_group_name(&self.broadcast.groups, name, Some(idx)) {
            Ok(()) => {
                if let Some(g) = self.broadcast.groups.get_mut(idx) {
                    g.name = name.trim().to_string();
                }
                self.broadcast_picker_renaming = None;
                self.broadcast_picker_query.clear();
                self.broadcast_picker_error = None;
                self.refresh_composer_slot(cx);
                cx.notify();
            }
            Err(e) => {
                self.broadcast_picker_error = Some(e);
                cx.notify();
            }
        }
    }

    fn delete_broadcast_group(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.broadcast.groups.len() {
            return;
        }
        self.broadcast.groups.remove(idx);
        self.broadcast.active = match self.broadcast.active {
            Some(a) if a == idx => None,
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        self.broadcast_picker_renaming = None;
        self.broadcast_picker_selected = self
            .broadcast_picker_selected
            .min(self.broadcast.groups.len().saturating_sub(1));
        self.broadcast_picker_error = None;
        self.sync_broadcast_stripes(cx);
        self.refresh_composer_slot(cx);
        cx.notify();
    }

    pub(crate) fn handle_broadcast_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let len = self.broadcast.groups.len();
        match key {
            "escape" => {
                if self.broadcast_picker_renaming.is_some() {
                    self.broadcast_picker_renaming = None;
                    self.broadcast_picker_query.clear();
                    self.broadcast_picker_error = None;
                    cx.notify();
                } else {
                    self.close_broadcast_picker(cx);
                }
            }
            "enter" => {
                if let Some(idx) = self.broadcast_picker_renaming {
                    let name = self.broadcast_picker_query.clone();
                    self.commit_broadcast_rename(idx, &name, cx);
                } else if !self.broadcast_picker_query.trim().is_empty() {
                    let name = self.broadcast_picker_query.clone();
                    self.create_broadcast_group(&name, cx);
                } else if len > 0 {
                    let idx = self.broadcast_picker_selected.min(len - 1);
                    self.broadcast.active = Some(idx);
                    self.refresh_composer_slot(cx);
                    self.close_broadcast_picker(cx);
                }
            }
            "up" => {
                if len > 0 && self.broadcast_picker_selected > 0 {
                    self.broadcast_picker_selected -= 1;
                    cx.notify();
                }
            }
            "down" => {
                if len > 0 && self.broadcast_picker_selected + 1 < len {
                    self.broadcast_picker_selected += 1;
                    cx.notify();
                }
            }
            "backspace" => {
                if self.broadcast_picker_query.pop().is_some() {
                    self.broadcast_picker_error = None;
                    cx.notify();
                }
            }
            _ => {
                if let Some(ch) = &event.keystroke.key_char
                    && !ch.is_empty()
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.alt
                {
                    self.broadcast_picker_query.push_str(ch);
                    self.broadcast_picker_error = None;
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn render_broadcast_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let renaming = self.broadcast_picker_renaming;

        // Live member counts so a closed pane never inflates a row (AC4).
        let mut live: HashSet<gpui::EntityId> = HashSet::new();
        for ws in &self.workspaces {
            if let Some(root) = &ws.root {
                for pane in root.collect_leaves() {
                    live.insert(pane.entity_id());
                }
            }
        }

        let placeholder = if renaming.is_some() {
            "Rename group…"
        } else {
            "Type a name + Enter to create…"
        };
        let query_text: SharedString = if self.broadcast_picker_query.is_empty() {
            placeholder.into()
        } else {
            format!("{}|", self.broadcast_picker_query).into()
        };
        let query_color = if self.broadcast_picker_query.is_empty() {
            ui.muted
        } else {
            ui.text
        };

        let mut card = div()
            .id("broadcast-picker")
            .occlude()
            .track_focus(&self.broadcast_picker_focus)
            .on_key_down(cx.listener(Self::handle_broadcast_picker_key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_broadcast_picker(cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .w(px(420.))
            .flex()
            .flex_col()
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .rounded(px(8.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .text_size(px(13.))
                    .text_color(query_color)
                    .border_b_1()
                    .border_color(ui.border)
                    .child(query_text),
            );

        if let Some(err) = &self.broadcast_picker_error {
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(6.))
                    .text_size(px(11.))
                    .text_color(ui.vc_deleted)
                    .child(err.clone()),
            );
        }

        if self.broadcast.groups.is_empty() {
            // US-002 AC6: explicit empty state proposing creation.
            card = card.child(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("No broadcast groups yet - type a name and press Enter to create one"),
            );
        } else {
            for (idx, group) in self.broadcast.groups.iter().enumerate() {
                let is_selected = idx == self.broadcast_picker_selected;
                let is_active = self.broadcast.active == Some(idx);
                let member_count = group.members.iter().filter(|m| live.contains(m)).count();
                let stable_id = group.color_idx;
                let resting_background = if is_selected {
                    ui.subtle
                } else {
                    ui.subtle.opacity(0.0)
                };

                let rename_btn = div()
                    .id(SharedString::from(format!("broadcast-rename-{stable_id}")))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .animated_hover(move |style, delta| {
                        style
                            .bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta))
                            .text_color(lerp_color(ui.muted, ui.text, delta));
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.broadcast_picker_renaming = Some(idx);
                        this.broadcast_picker_query = this
                            .broadcast
                            .groups
                            .get(idx)
                            .map(|g| g.name.clone())
                            .unwrap_or_default();
                        this.broadcast_picker_error = None;
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .child("Rename");

                let delete_btn = div()
                    .id(SharedString::from(format!("broadcast-delete-{stable_id}")))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .animated_hover(move |style, delta| {
                        style
                            .bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta))
                            .text_color(lerp_color(ui.muted, ui.vc_deleted, delta));
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.delete_broadcast_group(idx, cx);
                        cx.stop_propagation();
                    }))
                    .child("Delete");

                card = card.child(
                    div()
                        .id(SharedString::from(format!(
                            "broadcast-group-row-{stable_id}"
                        )))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .px(px(14.))
                        .py(px(6.))
                        .text_size(px(13.))
                        .bg(resting_background)
                        .text_color(if is_active { ui.accent } else { ui.text })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.broadcast.active = Some(idx);
                            this.refresh_composer_slot(cx);
                            this.close_broadcast_picker(cx);
                            cx.stop_propagation();
                        }))
                        .child(
                            div()
                                .flex_none()
                                .w(px(10.))
                                .h(px(10.))
                                .rounded_full()
                                .bg(ui.group_color(group.color_idx)),
                        )
                        .child(div().flex_1().child(group.name.clone()))
                        .child(div().text_size(px(11.)).text_color(ui.muted).child(format!(
                            "{member_count} pane{}",
                            if member_count == 1 { "" } else { "s" }
                        )))
                        .when(is_active, |d| {
                            d.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(ui.accent)
                                    .child("active"),
                            )
                        })
                        .animated_hover_element(move |row, delta| {
                            row.style()
                                .bg(lerp_color(resting_background, ui.subtle, delta));
                            let placeholder = div()
                                .flex()
                                .flex_row()
                                .gap(px(8.))
                                .invisible()
                                .child(
                                    div()
                                        .px(px(6.))
                                        .py(px(2.))
                                        .text_size(px(10.))
                                        .child("Rename"),
                                )
                                .child(
                                    div()
                                        .px(px(6.))
                                        .py(px(2.))
                                        .text_size(px(10.))
                                        .child("Delete"),
                                );
                            let actions = div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .flex()
                                .flex_row()
                                .gap(px(8.))
                                .opacity(delta)
                                .when(delta <= f32::EPSILON, |actions| actions.hidden())
                                .child(rename_btn)
                                .child(delete_btn);
                            let actions_slot = div()
                                .relative()
                                .flex_none()
                                .child(placeholder)
                                .child(actions)
                                .into_any_element();
                            row.extend([actions_slot]);
                        }),
                );
            }
        }

        card = card.child(
            div()
                .px(px(14.))
                .py(px(8.))
                .border_t_1()
                .border_color(ui.border)
                .text_size(px(10.))
                .text_color(ui.muted)
                .child("Enter selects the active group · typing a name creates one"),
        );

        deferred(
            div()
                .id("broadcast-picker-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(96.))
                .bg(gpui::hsla(0., 0., 0., 0.4))
                .child(card),
        )
        .with_priority(6)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_types::AgentState;

    fn group(name: &str, color_idx: usize, members: &[u64]) -> BroadcastGroup<u64> {
        BroadcastGroup {
            name: name.to_string(),
            color_idx,
            members: members.to_vec(),
        }
    }

    #[test]
    fn toggle_adds_then_removes() {
        let mut groups = vec![group("a", 0, &[])];
        assert!(toggle_member(&mut groups, 0, 7));
        assert_eq!(groups[0].members, vec![7]);
        assert!(!toggle_member(&mut groups, 0, 7));
        assert!(groups[0].members.is_empty());
    }

    #[test]
    fn toggle_moves_between_groups_one_group_per_pane() {
        // US-002 AC5: a pane belongs to at most one group - joining the
        // active group removes it from any other.
        let mut groups = vec![group("a", 0, &[7]), group("b", 1, &[])];
        assert!(toggle_member(&mut groups, 1, 7));
        assert!(groups[0].members.is_empty());
        assert_eq!(groups[1].members, vec![7]);
    }

    #[test]
    fn toggle_out_of_range_active_is_noop() {
        let mut groups = vec![group("a", 0, &[7])];
        assert!(!toggle_member(&mut groups, 5, 9));
        assert_eq!(groups[0].members, vec![7]);
    }

    #[test]
    fn next_free_color_recycles_freed_slot() {
        let groups: Vec<BroadcastGroup<u64>> =
            vec![group("a", 0, &[]), group("c", 2, &[]), group("d", 3, &[])];
        // Slot 1 was freed (or never used) - it must be reused before 4.
        assert_eq!(next_free_color(&groups), 1);
    }

    #[test]
    fn validate_rejects_ninth_group_with_explicit_message() {
        // US-002 AC3: at 8 existing groups the 9th create is refused.
        let groups: Vec<BroadcastGroup<u64>> = (0..MAX_GROUPS)
            .map(|i| group(&format!("g{i}"), i, &[]))
            .collect();
        let err = validate_group_name(&groups, "ninth", None).unwrap_err();
        assert_eq!(err, "Limit of 8 groups reached");
        // Renaming an existing group at the cap stays allowed.
        assert!(validate_group_name(&groups, "renamed", Some(0)).is_ok());
    }

    #[test]
    fn validate_rejects_empty_and_duplicate_names() {
        let groups = vec![group("alpha", 0, &[])];
        assert!(validate_group_name(&groups, "  ", None).is_err());
        assert!(validate_group_name(&groups, "alpha", None).is_err());
        // Renaming a group to its own current name is a no-op, not an error.
        assert!(validate_group_name(&groups, "alpha", Some(0)).is_ok());
        assert!(validate_group_name(&groups, "beta", None).is_ok());
    }

    #[test]
    fn validate_rejects_oversized_names() {
        // Security review: names render in picker rows / Composer label /
        // toasts - bounded at 64 chars (counted in chars, not bytes).
        let groups: Vec<BroadcastGroup<u64>> = Vec::new();
        assert!(validate_group_name(&groups, &"x".repeat(64), None).is_ok());
        assert!(validate_group_name(&groups, &"x".repeat(65), None).is_err());
        assert!(validate_group_name(&groups, &"é".repeat(64), None).is_ok());
    }

    #[test]
    fn only_generating_states_block_delivery() {
        // FR-02: WaitingForInput / Finished / Errored (and absent sessions,
        // handled by the caller) are safe prefill targets. Thinking blocks;
        // Stalled blocks too (EP-004 US-011: a stalled agent may still be
        // mid-generation - its stdin is NOT known-safe).
        assert!(state_blocks_delivery(&AgentState::Thinking));
        assert!(state_blocks_delivery(&AgentState::Stalled));
        assert!(!state_blocks_delivery(&AgentState::WaitingForInput));
        assert!(!state_blocks_delivery(&AgentState::Finished));
        // An Errored agent's process is gone - the pane is a bare shell.
        assert!(!state_blocks_delivery(&AgentState::Errored));
    }
}

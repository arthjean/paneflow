use std::collections::HashMap;

use gpui::{Context, Window};
use paneflow_config::schema::{
    DEFAULT_SPACE_NAME, MAX_SPACES, PaneFlowConfig, SpaceConfig, SpaceId, validate_space_name,
};

use crate::PaneFlowApp;
use crate::workspace::Workspace;

pub(crate) type SpaceMemory = HashMap<Option<SpaceId>, u64>;

pub(crate) fn in_space(workspace: &Workspace, space: Option<&SpaceId>) -> bool {
    workspace.space.as_ref() == space
}

pub(crate) fn shown_index(
    workspaces: &[Workspace],
    active_idx: usize,
    active_space: Option<&SpaceId>,
) -> Option<usize> {
    workspaces
        .get(active_idx)
        .filter(|workspace| in_space(workspace, active_space))
        .map(|_| active_idx)
}

pub(crate) fn first_in_space(workspaces: &[Workspace], space: Option<&SpaceId>) -> Option<usize> {
    PaneFlowApp::compute_display_order(workspaces, space)
        .first()
        .copied()
}

pub(crate) fn enter_space(
    workspaces: &[Workspace],
    active_idx: usize,
    active_space: &mut Option<SpaceId>,
    memory: &mut SpaceMemory,
    target: Option<SpaceId>,
) -> Option<usize> {
    if let Some(shown) = shown_index(workspaces, active_idx, active_space.as_ref()) {
        memory.insert(active_space.clone(), workspaces[shown].id);
    }
    *active_space = target;
    memory
        .get(active_space)
        .and_then(|id| workspaces.iter().position(|workspace| workspace.id == *id))
        .filter(|&idx| in_space(&workspaces[idx], active_space.as_ref()))
        .or_else(|| first_in_space(workspaces, active_space.as_ref()))
}

pub(crate) fn point_at(
    workspaces: &[Workspace],
    active_idx: &mut usize,
    active_space: &mut Option<SpaceId>,
    memory: &mut SpaceMemory,
    idx: usize,
) -> bool {
    let shown_before = shown_index(workspaces, *active_idx, active_space.as_ref());
    if workspaces[idx].space != *active_space {
        enter_space(
            workspaces,
            *active_idx,
            active_space,
            memory,
            workspaces[idx].space.clone(),
        );
    }
    *active_idx = idx;
    shown_before != Some(idx)
}

pub(crate) fn active_idx_after_removal(
    remaining: &[Workspace],
    active_idx: usize,
    removed: usize,
    active_space: Option<&SpaceId>,
) -> usize {
    if remaining.is_empty() {
        return 0;
    }
    if active_idx != removed {
        return if active_idx > removed {
            active_idx - 1
        } else {
            active_idx
        };
    }
    let split = removed.min(remaining.len());
    remaining[split..]
        .iter()
        .position(|workspace| in_space(workspace, active_space))
        .map(|offset| split + offset)
        .or_else(|| {
            remaining[..split]
                .iter()
                .rposition(|workspace| in_space(workspace, active_space))
        })
        .unwrap_or(split.min(remaining.len() - 1))
}

pub(crate) fn is_declared(config: &PaneFlowConfig, space: Option<&SpaceId>) -> bool {
    space.is_none_or(|id| config.space(id).is_some())
}

pub(crate) fn release_undeclared_spaces(
    workspaces: &mut [Workspace],
    active_space: &mut Option<SpaceId>,
    memory: &mut SpaceMemory,
    config: &PaneFlowConfig,
) -> usize {
    let declared = |space: &Option<SpaceId>| is_declared(config, space.as_ref());
    let mut released = 0;
    for workspace in workspaces.iter_mut() {
        if !declared(&workspace.space) {
            workspace.space = None;
            released += 1;
        }
    }
    if !declared(active_space) {
        *active_space = None;
        released += 1;
    }
    memory.retain(|space, _| declared(space));
    released
}

pub(crate) fn space_label<'a>(config: &'a PaneFlowConfig, space: Option<&SpaceId>) -> &'a str {
    space
        .and_then(|id| config.space(id))
        .map_or(DEFAULT_SPACE_NAME, |space| space.name.as_str())
}

pub(crate) fn declared_space_label<'a>(
    config: &'a PaneFlowConfig,
    workspace: &Workspace,
) -> Option<&'a str> {
    (!config.spaces.is_empty()).then(|| space_label(config, workspace.space.as_ref()))
}

pub(crate) fn reorder_in_space(
    workspaces: &mut Vec<Workspace>,
    active_idx: &mut usize,
    from: usize,
    to: usize,
) -> bool {
    if from >= workspaces.len() {
        return false;
    }
    let space = workspaces[from].space.clone();
    let mut naive: Vec<usize> = (0..workspaces.len()).collect();
    let moved = naive.remove(from);
    naive.insert(to.min(naive.len()), moved);
    let mut members = naive
        .into_iter()
        .filter(|&idx| workspaces[idx].space == space);
    let order: Vec<usize> = (0..workspaces.len())
        .map(|idx| {
            if workspaces[idx].space == space {
                members.next().unwrap_or(idx)
            } else {
                idx
            }
        })
        .collect();
    if order.iter().enumerate().all(|(slot, &idx)| slot == idx) {
        return false;
    }
    let shown = workspaces.get(*active_idx).map(|workspace| workspace.id);
    let mut slots: Vec<Option<Workspace>> =
        std::mem::take(workspaces).into_iter().map(Some).collect();
    *workspaces = order
        .into_iter()
        .filter_map(|idx| slots[idx].take())
        .collect();
    if let Some(id) = shown {
        *active_idx = workspaces
            .iter()
            .position(|workspace| workspace.id == id)
            .unwrap_or(0);
    }
    true
}

pub(crate) const SPACE_LIMIT_REASON: &str = "9 spaces maximum";

pub(crate) fn next_space_name(config: &PaneFlowConfig) -> String {
    let taken = || config.spaces.iter().map(|space| space.name.as_str());
    (1..)
        .map(|n| format!("Space {n}"))
        .find(|name| validate_space_name(name, taken()).is_ok())
        .unwrap_or_default()
}

pub(crate) fn space_order(config: &PaneFlowConfig) -> Vec<Option<SpaceId>> {
    std::iter::once(None)
        .chain(config.spaces.iter().map(|space| Some(space.id.clone())))
        .collect()
}

pub(crate) fn cycled_space(
    config: &PaneFlowConfig,
    active: Option<&SpaceId>,
    forward: bool,
) -> Option<Option<SpaceId>> {
    if config.spaces.is_empty() {
        return None;
    }
    let order = space_order(config);
    let current = order
        .iter()
        .position(|space| space.as_ref() == active)
        .unwrap_or(0);
    let next = if forward {
        (current + 1) % order.len()
    } else {
        (current + order.len() - 1) % order.len()
    };
    order.into_iter().nth(next)
}

pub(crate) fn display_neighbor(order: &[usize], idx: usize) -> Option<usize> {
    let position = order.iter().position(|&shown| shown == idx)?;
    order
        .get(position + 1)
        .or_else(|| {
            position
                .checked_sub(1)
                .and_then(|previous| order.get(previous))
        })
        .copied()
}

pub(crate) fn restored_active_idx(
    workspaces: &[Workspace],
    active_workspace: usize,
    active_space: Option<&SpaceId>,
) -> usize {
    let saved = active_workspace.min(workspaces.len().saturating_sub(1));
    shown_index(workspaces, saved, active_space)
        .or_else(|| first_in_space(workspaces, active_space))
        .unwrap_or(saved)
}

impl PaneFlowApp {
    pub(crate) fn shown_workspace_idx(&self) -> Option<usize> {
        shown_index(
            &self.workspaces,
            self.active_idx,
            self.active_space.as_ref(),
        )
    }

    pub(crate) fn point_at_workspace(&mut self, idx: usize) -> bool {
        point_at(
            &self.workspaces,
            &mut self.active_idx,
            &mut self.active_space,
            &mut self.space_last_shown,
            idx,
        )
    }

    pub(crate) fn add_workspace(&mut self, mut workspace: Workspace) -> usize {
        workspace.space = self.active_space.clone();
        self.workspaces.push(workspace);
        self.workspaces.len() - 1
    }

    pub(crate) fn activate_space(
        &mut self,
        space: Option<SpaceId>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if space == self.active_space || !is_declared(&self.cached_config, space.as_ref()) {
            return false;
        }
        let shown_before = self.shown_workspace_idx();
        let target = enter_space(
            &self.workspaces,
            self.active_idx,
            &mut self.active_space,
            &mut self.space_last_shown,
            space,
        );
        self.sidebar_cursor = target.map(|idx| crate::app::sidebar::keyboard::SidebarCursor {
            workspace_id: self.workspaces[idx].id,
            tab_id: None,
        });
        match (target, window) {
            (Some(idx), window) => {
                let probed = match window {
                    Some(window) => self.activate_workspace_at(
                        idx,
                        crate::app::workspace_ops::WorkspaceFocusTarget::FirstPane,
                        window,
                        cx,
                    ),
                    None => self.activate_workspace_without_window(idx, cx),
                };
                if !probed && shown_before != Some(idx) {
                    self.probe_workspace_git_now(idx, cx);
                }
            }
            (None, window) => self.show_empty_space(window, cx),
        }
        true
    }

    fn show_empty_space(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.dismiss_transient_surfaces();
        if let Some(window) = window {
            window.focus(&self.welcome_focus, cx);
        }
        self.sync_files_sidebar_session(cx);
        if self.agent_sessions.sessions_sidebar_open {
            self.close_sessions_sidebar(cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn persist_spaces(&mut self, spaces: Vec<SpaceConfig>, cx: &mut Context<Self>) {
        let value = if spaces.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::to_value(spaces).unwrap_or(serde_json::Value::Null)
        };
        self.persist_setting(false, "spaces", value, cx);
        self.reconcile_spaces_with_config(cx);
    }

    pub(crate) fn add_space(&mut self, cx: &mut Context<Self>) -> Option<SpaceId> {
        if self.cached_config.spaces.len() >= MAX_SPACES {
            self.show_toast(SPACE_LIMIT_REASON, cx);
            return None;
        }
        let id = SpaceId::new();
        let mut spaces = self.cached_config.spaces.clone();
        spaces.push(SpaceConfig {
            id: id.clone(),
            name: next_space_name(&self.cached_config),
            color: None,
        });
        self.persist_spaces(spaces, cx);
        self.cached_config.space(&id).map(|space| space.id.clone())
    }

    pub(crate) fn new_space_in_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.add_space(cx) {
            self.open_settings_at(crate::SettingsSection::Spaces, window, cx);
            self.edit_space_name(id, window, cx);
        }
    }

    pub(crate) fn cycle_space(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = cycled_space(&self.cached_config, self.active_space.as_ref(), forward)
        {
            self.activate_space(target, Some(window), cx);
        }
    }

    pub(crate) fn move_workspace_to_space(
        &mut self,
        idx: usize,
        target: Option<SpaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspaces.get(idx).is_none_or(|ws| ws.space == target)
            || !is_declared(&self.cached_config, target.as_ref())
        {
            return;
        }
        let shown = self.shown_workspace_idx() == Some(idx);
        let order = Self::compute_display_order(&self.workspaces, self.active_space.as_ref());
        let neighbor = display_neighbor(&order, idx);
        self.workspaces[idx].space = target.clone();
        self.workspace_menu_open = None;
        if shown {
            match neighbor {
                Some(neighbor) => {
                    self.activate_workspace_at(
                        neighbor,
                        crate::app::workspace_ops::WorkspaceFocusTarget::FirstPane,
                        window,
                        cx,
                    );
                }
                None => self.show_empty_space(Some(window), cx),
            }
        }
        let moved = &self.workspaces[idx];
        let title = if moved.title.is_empty() {
            moved.cwd.clone()
        } else {
            moved.title.clone()
        };
        let message = format!(
            "Moved {title} to {}",
            space_label(&self.cached_config, target.as_ref())
        );
        self.push_toast(
            message,
            vec![crate::app::notifications::ToastAction::ShowSpace(target)],
            crate::app::constants::TOAST_HOLD_MS * 3,
            cx,
        );
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn move_workspace_to_new_space(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.add_space(cx) else {
            return;
        };
        self.move_workspace_to_space(idx, Some(id.clone()), window, cx);
        self.open_settings_at(crate::SettingsSection::Spaces, window, cx);
        self.edit_space_name(id, window, cx);
    }

    pub(crate) fn reconcile_spaces_with_config(&mut self, cx: &mut Context<Self>) {
        let removed_active = self
            .active_space
            .as_ref()
            .is_some_and(|id| self.cached_config.space(id).is_none());
        if removed_active {
            self.activate_space(None, None, cx);
        }
        let released = release_undeclared_spaces(
            &mut self.workspaces,
            &mut self.active_space,
            &mut self.space_last_shown,
            &self.cached_config,
        );
        if released > 0 {
            log::info!("spaces: {released} workspace(s) moved to the default space");
            self.save_session(cx);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(n: u8) -> SpaceId {
        SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap()
    }

    fn workspace(id: u64, space: Option<SpaceId>) -> Workspace {
        let mut workspace =
            Workspace::empty_with_cwd_and_id(id, format!("ws{id}"), std::path::PathBuf::new());
        workspace.space = space;
        workspace
    }

    fn mixed() -> Vec<Workspace> {
        vec![
            workspace(10, Some(space(1))),
            workspace(11, None),
            workspace(12, Some(space(1))),
            workspace(13, Some(space(2))),
            workspace(14, None),
        ]
    }

    #[test]
    fn only_a_workspace_of_the_active_space_is_shown() {
        let workspaces = mixed();
        assert_eq!(shown_index(&workspaces, 0, Some(&space(1))), Some(0));
        assert_eq!(shown_index(&workspaces, 0, None), None);
        assert_eq!(shown_index(&workspaces, 1, None), Some(1));
        assert_eq!(shown_index(&workspaces, 9, None), None);
    }

    struct View {
        active_idx: usize,
        active_space: Option<SpaceId>,
        memory: SpaceMemory,
    }

    impl View {
        fn new(active_idx: usize, active_space: Option<SpaceId>) -> Self {
            Self {
                active_idx,
                active_space,
                memory: SpaceMemory::new(),
            }
        }

        fn switch(&mut self, workspaces: &[Workspace], target: Option<SpaceId>) -> Option<usize> {
            let entry = enter_space(
                workspaces,
                self.active_idx,
                &mut self.active_space,
                &mut self.memory,
                target,
            );
            if let Some(idx) = entry {
                self.active_idx = idx;
            }
            entry
        }

        fn activate(&mut self, workspaces: &[Workspace], idx: usize) -> bool {
            point_at(
                workspaces,
                &mut self.active_idx,
                &mut self.active_space,
                &mut self.memory,
                idx,
            )
        }

        fn shown(&self, workspaces: &[Workspace]) -> Option<usize> {
            shown_index(workspaces, self.active_idx, self.active_space.as_ref())
        }
    }

    #[test]
    fn entering_a_space_shows_its_remembered_workspace_then_its_first() {
        let workspaces = mixed();
        let mut view = View::new(2, Some(space(1)));

        assert_eq!(
            view.switch(&workspaces, None),
            Some(1),
            "first default workspace in display order"
        );
        assert_eq!(view.memory.get(&Some(space(1))), Some(&12));

        view.active_idx = 4;
        assert_eq!(
            view.switch(&workspaces, Some(space(1))),
            Some(2),
            "the remembered workspace comes back"
        );
        assert_eq!(view.memory.get(&None), Some(&14));
        assert_eq!(view.switch(&workspaces, None), Some(4));
        assert!(view.memory.len() <= 10);
    }

    #[test]
    fn entering_a_space_leaves_the_index_to_the_caller() {
        let workspaces = mixed();
        let mut active_space = Some(space(1));
        let target = enter_space(
            &workspaces,
            0,
            &mut active_space,
            &mut SpaceMemory::new(),
            Some(space(2)),
        );
        assert_eq!(target, Some(3));
        assert_eq!(
            shown_index(&workspaces, 0, active_space.as_ref()),
            None,
            "until the caller activates the target, the old workspace is hidden"
        );
    }

    #[test]
    fn entering_an_empty_space_shows_nothing_and_keeps_the_index_in_bounds() {
        let workspaces = mixed();
        let mut view = View::new(0, Some(space(1)));
        assert_eq!(view.switch(&workspaces, Some(space(3))), None);
        assert_eq!(view.active_idx, 0);
        assert_eq!(view.shown(&workspaces), None);
    }

    #[test]
    fn a_remembered_workspace_that_closed_or_moved_falls_back_to_the_first() {
        let mut workspaces = mixed();
        let mut view = View::new(3, Some(space(2)));
        view.memory = SpaceMemory::from([(Some(space(1)), 12), (None, 99)]);

        assert_eq!(
            view.switch(&workspaces, None),
            Some(1),
            "a closed remembered workspace is skipped"
        );

        workspaces[2].space = Some(space(2));
        assert_eq!(
            view.switch(&workspaces, Some(space(1))),
            Some(0),
            "a moved remembered workspace is skipped"
        );
    }

    #[test]
    fn activating_a_workspace_of_another_space_switches_to_that_space() {
        let workspaces = mixed();
        let mut view = View::new(0, Some(space(1)));

        assert!(view.activate(&workspaces, 3));
        assert_eq!(view.active_space, Some(space(2)));
        assert_eq!(view.shown(&workspaces), Some(3));
        assert_eq!(
            view.memory.get(&Some(space(1))),
            Some(&10),
            "the space left behind remembers its workspace"
        );

        assert!(view.activate(&workspaces, 4));
        assert_eq!(view.active_space, None);
        assert_eq!(view.shown(&workspaces), Some(4));

        assert!(!view.activate(&workspaces, 4), "already shown");
        assert!(view.activate(&workspaces, 1));
        assert_eq!(view.active_space, None, "same space, no switch");
    }

    #[test]
    fn activating_from_an_empty_space_switches_and_reports_a_change() {
        let workspaces = mixed();
        let mut view = View::new(1, Some(space(3)));
        assert_eq!(view.shown(&workspaces), None);
        assert!(view.activate(&workspaces, 1));
        assert_eq!(view.active_space, None);
        assert_eq!(view.shown(&workspaces), Some(1));
    }

    #[test]
    fn every_activation_path_leaves_the_shown_workspace_in_the_active_space() {
        let workspaces = mixed();
        for start in 0..workspaces.len() {
            for target in 0..workspaces.len() {
                let mut view = View::new(start, workspaces[start].space.clone());
                view.activate(&workspaces, target);
                let shown = view.shown(&workspaces).expect("a workspace shows");
                assert!(in_space(&workspaces[shown], view.active_space.as_ref()));
                assert_eq!(shown, target);
            }
        }
    }

    #[test]
    fn closing_the_shown_workspace_shows_its_nearest_neighbor_in_the_space() {
        let mut workspaces = mixed();
        workspaces.remove(2);
        assert_eq!(
            active_idx_after_removal(&workspaces, 2, 2, Some(&space(1))),
            0,
            "no following workspace of the space, so the preceding one"
        );

        let mut workspaces = mixed();
        workspaces.remove(1);
        assert_eq!(
            active_idx_after_removal(&workspaces, 1, 1, None),
            3,
            "the next default workspace, skipping the other spaces"
        );

        let mut workspaces = mixed();
        workspaces.remove(3);
        let after = active_idx_after_removal(&workspaces, 3, 3, Some(&space(2)));
        assert!(after < workspaces.len());
        assert_eq!(
            shown_index(&workspaces, after, Some(&space(2))),
            None,
            "the space is now empty"
        );
    }

    #[test]
    fn closing_another_workspace_keeps_the_shown_one() {
        let mut workspaces = mixed();
        workspaces.remove(0);
        assert_eq!(active_idx_after_removal(&workspaces, 3, 0, None), 2);
        assert_eq!(active_idx_after_removal(&[], 0, 0, None), 0);
    }

    #[test]
    fn without_spaces_closing_matches_the_previous_index_rule() {
        for len in 1..5usize {
            for removed in 0..len {
                for active in 0..len {
                    let mut workspaces: Vec<_> =
                        (0..len as u64).map(|id| workspace(id, None)).collect();
                    workspaces.remove(removed);
                    let previous = if workspaces.is_empty() {
                        0
                    } else if active >= workspaces.len() {
                        workspaces.len() - 1
                    } else if active > removed {
                        active - 1
                    } else {
                        active
                    };
                    assert_eq!(
                        active_idx_after_removal(&workspaces, active, removed, None),
                        previous,
                        "len {len} removed {removed} active {active}"
                    );
                }
            }
        }
    }

    #[test]
    fn undeclared_spaces_fall_back_to_the_default_space() {
        let mut workspaces = mixed();
        let mut active_space = Some(space(2));
        let mut memory = SpaceMemory::from([(Some(space(2)), 13), (Some(space(1)), 10)]);
        let config = PaneFlowConfig {
            spaces: vec![paneflow_config::schema::SpaceConfig {
                id: space(1),
                name: "Work".to_string(),
                color: None,
            }],
            ..Default::default()
        };
        let released =
            release_undeclared_spaces(&mut workspaces, &mut active_space, &mut memory, &config);
        assert_eq!(released, 2, "one workspace and the active space");
        assert_eq!(active_space, None);
        assert_eq!(workspaces[3].space, None);
        assert_eq!(workspaces[0].space, Some(space(1)));
        assert!(!memory.contains_key(&Some(space(2))));
    }

    #[test]
    fn a_restored_active_workspace_outside_the_active_space_yields_its_first() {
        let workspaces = mixed();
        assert_eq!(restored_active_idx(&workspaces, 2, Some(&space(1))), 2);
        assert_eq!(restored_active_idx(&workspaces, 3, None), 1);
        assert_eq!(restored_active_idx(&workspaces, 99, Some(&space(2))), 3);
        let empty_space = restored_active_idx(&workspaces, 0, Some(&space(3)));
        assert_eq!(shown_index(&workspaces, empty_space, Some(&space(3))), None);
        assert_eq!(restored_active_idx(&[], 4, None), 0);
    }

    fn ids(workspaces: &[Workspace]) -> Vec<u64> {
        workspaces.iter().map(|workspace| workspace.id).collect()
    }

    #[test]
    fn reordering_inside_a_space_leaves_other_spaces_in_place() {
        let mut workspaces = mixed();
        let mut active_idx = 3;
        assert!(reorder_in_space(&mut workspaces, &mut active_idx, 0, 2));
        assert_eq!(ids(&workspaces), vec![12, 11, 10, 13, 14]);
        assert_eq!(active_idx, 3, "the shown workspace keeps its identity");

        let mut workspaces = mixed();
        let mut active_idx = 0;
        assert!(reorder_in_space(&mut workspaces, &mut active_idx, 4, 0));
        assert_eq!(ids(&workspaces), vec![10, 14, 12, 13, 11]);
        assert_eq!(workspaces[active_idx].id, 10);
    }

    #[test]
    fn the_trailing_drop_slot_moves_a_workspace_last_in_its_space() {
        let mut workspaces = mixed();
        let mut active_idx = 1;
        let len = workspaces.len();
        assert!(reorder_in_space(
            &mut workspaces,
            &mut active_idx,
            1,
            len - 1
        ));
        assert_eq!(ids(&workspaces), vec![10, 14, 12, 13, 11]);
        assert_eq!(workspaces[active_idx].id, 11);
    }

    #[test]
    fn without_spaces_reordering_matches_a_plain_move() {
        let mut workspaces: Vec<Workspace> = (0..4).map(|id| workspace(id, None)).collect();
        let mut active_idx = 2;
        assert!(reorder_in_space(&mut workspaces, &mut active_idx, 0, 3));
        assert_eq!(ids(&workspaces), vec![1, 2, 3, 0]);
        assert_eq!(active_idx, 1);
        assert!(!reorder_in_space(&mut workspaces, &mut active_idx, 1, 1));
        assert!(!reorder_in_space(&mut workspaces, &mut active_idx, 9, 0));
    }

    #[test]
    fn the_default_space_is_named_default_and_others_by_their_config_name() {
        let mut config = PaneFlowConfig::default();
        config.spaces.push(paneflow_config::schema::SpaceConfig {
            id: space(1),
            name: "Work".to_string(),
            color: None,
        });
        assert_eq!(space_label(&config, None), "Default");
        assert_eq!(space_label(&config, Some(&space(1))), "Work");
        assert_eq!(space_label(&config, Some(&space(2))), "Default");
    }

    fn config_with(names: &[&str]) -> PaneFlowConfig {
        let mut config = PaneFlowConfig::default();
        for (n, name) in names.iter().enumerate() {
            config.spaces.push(SpaceConfig {
                id: space(n as u8 + 1),
                name: name.to_string(),
                color: None,
            });
        }
        config
    }

    #[test]
    fn space_shortcuts_cycle_through_default_and_wrap_around() {
        let config = config_with(&["Work", "Perso"]);
        assert_eq!(cycled_space(&config, None, true), Some(Some(space(1))));
        assert_eq!(
            cycled_space(&config, Some(&space(1)), true),
            Some(Some(space(2)))
        );
        assert_eq!(cycled_space(&config, Some(&space(2)), true), Some(None));
        assert_eq!(cycled_space(&config, None, false), Some(Some(space(2))));
        assert_eq!(cycled_space(&config, Some(&space(1)), false), Some(None));
    }

    #[test]
    fn space_shortcuts_do_nothing_without_spaces() {
        let config = PaneFlowConfig::default();
        assert_eq!(cycled_space(&config, None, true), None);
        assert_eq!(cycled_space(&config, None, false), None);
    }

    #[test]
    fn a_removed_space_is_no_longer_a_switch_target() {
        let config = config_with(&["Work"]);
        assert!(is_declared(&config, None));
        assert!(is_declared(&config, Some(&space(1))));
        assert!(
            !is_declared(&config, Some(&space(2))),
            "a Show toast that outlives its space must not activate it"
        );
    }

    #[test]
    fn a_new_space_takes_the_first_free_number() {
        assert_eq!(next_space_name(&PaneFlowConfig::default()), "Space 1");
        assert_eq!(
            next_space_name(&config_with(&["Space 1", "space 2", "Space 4"])),
            "Space 3"
        );
    }

    #[test]
    fn moving_the_shown_workspace_shows_its_neighbor_in_the_space() {
        let workspaces = mixed();
        let order = PaneFlowApp::compute_display_order(&workspaces, Some(&space(1)));
        assert_eq!(order, vec![0, 2]);
        assert_eq!(display_neighbor(&order, 0), Some(2));
        assert_eq!(display_neighbor(&order, 2), Some(0));
        assert_eq!(
            display_neighbor(&[3], 3),
            None,
            "the last one leaves the space empty"
        );
        assert_eq!(display_neighbor(&order, 4), None);
    }

    #[test]
    fn the_shown_workspace_is_only_reached_through_the_filtered_accessors() {
        use std::path::{Path, PathBuf};

        let needles = [
            ["workspaces[self.", "active_idx]"].concat(),
            ["workspaces.get(self.", "active_idx)"].concat(),
            ["workspaces.get_mut(self.", "active_idx)"].concat(),
            ["workspaces.get(app.", "active_idx)"].concat(),
            ["workspaces.get_mut(app.", "active_idx)"].concat(),
        ];
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let compact: String = std::fs::read_to_string(&path)
                    .unwrap()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                for needle in &needles {
                    if compact.contains(needle.as_str()) {
                        violations.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "read the shown workspace through active_workspace() or shown_workspace_idx():\n{}",
            violations.join("\n")
        );
    }
}

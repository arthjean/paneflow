use crate::PaneFlowApp;

pub(crate) fn completion_was_seen(
    visible: Option<&std::collections::HashSet<u64>>,
    surface_id: Option<u64>,
) -> bool {
    match surface_id {
        Some(id) => visible.is_some_and(|visible| visible.contains(&id)),
        None => visible.is_some(),
    }
}

impl PaneFlowApp {
    pub(crate) fn surfaces_under_user_eye(
        &self,
        workspace_id: u64,
        cx: &gpui::App,
    ) -> Option<std::collections::HashSet<u64>> {
        if !crate::agents::notifications::window_active() {
            return None;
        }
        let ws = self.workspaces.iter().find(|ws| ws.id == workspace_id)?;
        let main_visible = self.settings_section.is_none()
            && self
                .workspaces
                .get(self.active_idx)
                .is_some_and(|active| active.id == workspace_id)
            && cx.windows().into_iter().any(|window| {
                window.downcast::<PaneFlowApp>().is_some()
                    && crate::agents::notifications::is_window_active(window.window_id())
            });
        let mut visible = std::collections::HashSet::new();
        for pane in ws.collect_panes() {
            let state = pane.read(cx);
            let shown = match state.detached {
                Some(placement) => {
                    crate::agents::notifications::is_window_active(placement.window.window_id())
                }
                None => {
                    main_visible
                        && ws
                            .active_tab()
                            .root
                            .as_ref()
                            .is_some_and(|root| root.contains_leaf(&pane))
                }
            };
            if shown && let Some(terminal) = state.active_terminal_opt() {
                visible.insert(terminal.entity_id().as_u64());
            }
        }
        (!visible.is_empty()).then_some(visible)
    }

    pub(super) fn session_is_seen(&self, workspace_id: u64, key: u32, cx: &gpui::App) -> bool {
        let surface = self
            .workspaces
            .iter()
            .find(|ws| ws.id == workspace_id)
            .and_then(|ws| ws.agent_sessions.get(&key))
            .and_then(|session| session.surface_id);
        completion_was_seen(
            self.surfaces_under_user_eye(workspace_id, cx).as_ref(),
            surface,
        )
    }

    pub(super) fn workspace_id_for_surface(&self, surface_id: u64, cx: &gpui::App) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|ws| {
                ws.collect_panes().iter().any(|pane| {
                    pane.read(cx)
                        .terminals()
                        .any(|terminal| terminal.entity_id().as_u64() == surface_id)
                })
            })
            .map(|ws| ws.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_only_seen_when_its_own_pane_is_the_one_on_screen() {
        let watched = std::collections::HashSet::from([7u64]);

        assert!(completion_was_seen(Some(&watched), Some(7)));
        assert!(!completion_was_seen(Some(&watched), Some(8)));
        assert!(!completion_was_seen(None, Some(7)));
    }

    #[test]
    fn an_unresolved_surface_falls_back_to_its_workspace() {
        let watched = std::collections::HashSet::from([7u64]);
        assert!(completion_was_seen(Some(&watched), None));
        assert!(completion_was_seen(
            Some(&std::collections::HashSet::new()),
            None
        ));
        assert!(!completion_was_seen(None, None));
    }
}

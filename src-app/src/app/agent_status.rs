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

pub(crate) fn pane_on_screen(
    detached_window_active: Option<bool>,
    main_visible: bool,
    in_active_tab: bool,
) -> bool {
    match detached_window_active {
        Some(active) => active,
        None => main_visible && in_active_tab,
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
            let shown = pane_on_screen(
                state.detached.map(|placement| {
                    crate::agents::notifications::is_window_active(placement.window.window_id())
                }),
                main_visible,
                ws.active_tab()
                    .root
                    .as_ref()
                    .is_some_and(|root| root.contains_leaf(&pane)),
            );
            if shown && let Some(terminal) = state.active_terminal_opt() {
                visible.insert(terminal.entity_id().as_u64());
            }
        }
        (!visible.is_empty()).then_some(visible)
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
    fn a_detached_pane_is_on_screen_only_while_its_own_window_is_active() {
        assert!(!pane_on_screen(Some(false), true, true));
        assert!(pane_on_screen(Some(true), false, false));
        assert!(pane_on_screen(None, true, true));
        assert!(!pane_on_screen(None, true, false));
        assert!(!pane_on_screen(None, false, true));
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

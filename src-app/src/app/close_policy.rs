use gpui::{Context, Entity, Window};

use crate::PaneFlowApp;
use crate::app::diff_dock::code::view::CodeView;
use crate::app::hosted_sessions::{pane_terminals, tab_terminals};
use crate::app::unsaved_dialog::UnsavedContinuation;
use crate::app::workspace_ops::settle_closed_sessions;
use crate::pane::Pane;
use crate::terminal::TerminalView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseIntent {
    Stop,
    Hold,
}

#[derive(Clone)]
pub(crate) enum CloseTarget {
    Pane(Entity<Pane>),
    FocusedPane(Entity<Pane>),
    Surface {
        pane: Entity<Pane>,
        terminal: Entity<TerminalView>,
    },
    Tab {
        ws_idx: usize,
        tab_idx: usize,
    },
    Workspace(usize),
    DiffTerminal(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsavedRefusal(pub(crate) Vec<String>);

impl UnsavedRefusal {
    pub(crate) fn message(&self) -> String {
        format!(
            "{}; save or discard them in Paneflow first",
            crate::app::unsaved_dialog::unsaved_close_error(&self.0).unwrap_or_default()
        )
    }
}

pub(crate) const STALE_CLOSE_MESSAGE: &str =
    "Nothing was closed: the tabs changed while the dialog was open";

pub(crate) fn close_target_tabs(
    workspaces: &[crate::workspace::Workspace],
    target: &CloseTarget,
) -> Vec<u64> {
    match target {
        CloseTarget::Tab { ws_idx, tab_idx } => workspaces
            .get(*ws_idx)
            .and_then(|ws| ws.tabs().get(*tab_idx))
            .map(|tab| vec![tab.id])
            .unwrap_or_default(),
        CloseTarget::Workspace(idx) => workspaces
            .get(*idx)
            .map(|ws| ws.tabs().iter().map(|tab| tab.id).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn discard_unsaved(views: Vec<Entity<CodeView>>, cx: &mut gpui::App) {
    for view in views {
        view.update(cx, |view, _| view.discard_unsaved());
    }
}

impl PaneFlowApp {
    pub(crate) fn close_target_terminals(
        &self,
        target: &CloseTarget,
        cx: &gpui::App,
    ) -> Vec<Entity<TerminalView>> {
        match target {
            CloseTarget::Pane(pane) | CloseTarget::FocusedPane(pane) => pane_terminals(pane, cx),
            CloseTarget::Surface { terminal, .. } => vec![terminal.clone()],
            CloseTarget::Tab { ws_idx, tab_idx } => self
                .workspaces
                .get(*ws_idx)
                .and_then(|ws| ws.tabs().get(*tab_idx))
                .map(|tab| tab_terminals(tab, cx))
                .unwrap_or_default(),
            CloseTarget::Workspace(idx) => self
                .workspaces
                .get(*idx)
                .map(|ws| {
                    ws.collect_panes()
                        .iter()
                        .flat_map(|pane| pane_terminals(pane, cx))
                        .collect()
                })
                .unwrap_or_default(),
            CloseTarget::DiffTerminal(index) => match self.diff_dock.diff_tabs.get(*index) {
                Some(crate::app::diff_dock::DiffDockTab::Terminal(terminal)) => {
                    vec![terminal.clone()]
                }
                _ => Vec::new(),
            },
        }
    }

    pub(crate) fn close_refusal(
        &self,
        target: &CloseTarget,
        cx: &gpui::App,
    ) -> Option<UnsavedRefusal> {
        let unsaved = self.unsaved_views_for_close(target, cx);
        if unsaved.is_empty() {
            return None;
        }
        let names = crate::app::unsaved_dialog::file_rows(&unsaved, cx)
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect();
        Some(UnsavedRefusal(names))
    }

    pub(crate) fn close_without_prompt(
        &mut self,
        target: CloseTarget,
        cx: &mut Context<Self>,
    ) -> Result<(), UnsavedRefusal> {
        if let Some(refusal) = self.close_refusal(&target, cx) {
            return Err(refusal);
        }
        self.perform_close(target, CloseIntent::Hold, None, cx);
        Ok(())
    }

    pub(crate) fn request_close(
        &mut self,
        target: CloseTarget,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.unsaved_dialog.is_some() {
            return;
        }
        let unsaved = self.unsaved_views_for_close(&target, cx);
        if self.ask_about_unsaved(unsaved, UnsavedContinuation::Close(target.clone()), cx) {
            return;
        }
        self.request_close_checked(target, Vec::new(), window, cx);
    }

    pub(crate) fn request_close_checked(
        &mut self,
        target: CloseTarget,
        discard: Vec<Entity<CodeView>>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        discard_unsaved(discard, cx);
        self.perform_close(target, CloseIntent::Hold, window, cx);
    }

    pub(crate) fn perform_close(
        &mut self,
        target: CloseTarget,
        intent: CloseIntent,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let terminals = self.close_target_terminals(&target, cx);
        if intent == CloseIntent::Stop {
            self.stop_terminals(terminals.clone(), cx);
        }
        match target {
            CloseTarget::Pane(pane) => self.remove_pane_from_layout(pane, cx),
            CloseTarget::FocusedPane(pane) => {
                if let Some(window) = window {
                    self.remove_focused_pane(pane, window, cx);
                }
            }
            CloseTarget::Surface { pane, terminal } => {
                pane.update(cx, |pane, cx| pane.remove_surface(&terminal, cx));
                self.save_session(cx);
                cx.notify();
            }
            CloseTarget::Tab { ws_idx, tab_idx } => {
                if let Some(window) = window {
                    self.remove_workspace_tab(ws_idx, tab_idx, window, cx);
                }
            }
            CloseTarget::Workspace(idx) => self.remove_workspace(idx, window, cx),
            CloseTarget::DiffTerminal(index) => {
                self.remove_diff_tab(index, cx);
                if let Some(window) = window {
                    self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
                }
            }
        }
        match intent {
            CloseIntent::Stop => {
                let closed = terminals
                    .iter()
                    .map(|terminal| terminal.read(cx).terminal.session_id.clone())
                    .collect();
                settle_closed_sessions(&mut self.closed_panes, &closed, None);
            }
            CloseIntent::Hold => self.hold_closed_sessions(terminals, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_close_target_resolves_to_other_tabs_once_the_indices_shift(cx: &mut gpui::TestAppContext) {
        use crate::workspace::{Tab, Workspace};
        use gpui::AppContext as _;

        let cx = cx.add_empty_window();
        let mut workspaces: Vec<Workspace> = (1..=2)
            .map(|id| {
                let terminal = cx.new(|cx| TerminalView::display_only_for_test(id, cx));
                let pane = cx.new(|cx| Pane::new(terminal, id, cx));
                Workspace::with_id(id, format!("ws{id}"), pane)
            })
            .collect();
        assert!(workspaces[0].open_tab(Tab::new("second", None)));
        let tab = CloseTarget::Tab {
            ws_idx: 0,
            tab_idx: 1,
        };
        let workspace = CloseTarget::Workspace(1);
        let tab_seen = close_target_tabs(&workspaces, &tab);
        let workspace_seen = close_target_tabs(&workspaces, &workspace);
        assert_eq!(tab_seen.len(), 1);
        assert_eq!(workspace_seen.len(), 1);

        workspaces[0].close_tab(0);
        assert_ne!(
            close_target_tabs(&workspaces, &tab),
            tab_seen,
            "a closed neighbor shifts the tab index to another tab"
        );

        workspaces.remove(0);
        assert_ne!(
            close_target_tabs(&workspaces, &workspace),
            workspace_seen,
            "a removed workspace shifts the workspace index"
        );
    }
}

use gpui::{Context, KeyDownEvent, Window};

use crate::app::drag::TabDrag;
use crate::{FocusWorkspacesSidebar, PaneFlowApp};

use super::{SidebarRow, tab_display_title};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidebarCursor {
    pub(crate) workspace_id: u64,
    pub(crate) tab_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorMove {
    Previous,
    Next,
    First,
    Last,
}

fn moved_cursor_index(current: Option<usize>, len: usize, step: CursorMove) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (step, current) {
        (CursorMove::First, _) | (CursorMove::Next, None) => 0,
        (CursorMove::Last, _) | (CursorMove::Previous, None) => len - 1,
        (CursorMove::Previous, Some(index)) => index.saturating_sub(1),
        (CursorMove::Next, Some(index)) => (index + 1).min(len - 1),
    })
}

impl PaneFlowApp {
    pub(crate) fn handle_focus_workspaces_sidebar(
        &mut self,
        _: &FocusWorkspacesSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.primary_sidebar_visible {
            self.toggle_primary_sidebar(cx);
        }
        if self.sidebar_cursor_index(cx).is_none() {
            self.sidebar_cursor = self
                .workspaces
                .get(self.active_idx)
                .map(|ws| SidebarCursor {
                    workspace_id: ws.id,
                    tab_id: None,
                });
        }
        self.sidebar_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn sidebar_cursor_matches(&self, row: SidebarRow) -> bool {
        self.sidebar_cursor
            .is_some_and(|cursor| self.sidebar_cursor_for(row) == Some(cursor))
    }

    pub(super) fn place_sidebar_cursor(
        &mut self,
        row: SidebarRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_cursor = self.sidebar_cursor_for(row);
        self.sidebar_focus.focus(window, cx);
    }

    fn sidebar_cursor_for(&self, row: SidebarRow) -> Option<SidebarCursor> {
        match row {
            SidebarRow::Folder(ws_idx) => self.workspaces.get(ws_idx).map(|ws| SidebarCursor {
                workspace_id: ws.id,
                tab_id: None,
            }),
            SidebarRow::Tab(ws_idx, tab_idx) => {
                let ws = self.workspaces.get(ws_idx)?;
                ws.tabs().get(tab_idx).map(|tab| SidebarCursor {
                    workspace_id: ws.id,
                    tab_id: Some(tab.id),
                })
            }
        }
    }

    fn sidebar_nav_rows(&self, cx: &Context<Self>) -> Vec<SidebarRow> {
        let query = self
            .sidebar_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        self.sidebar_rows(&query)
    }

    fn sidebar_cursor_index(&self, cx: &Context<Self>) -> Option<usize> {
        let cursor = self.sidebar_cursor?;
        self.sidebar_nav_rows(cx)
            .into_iter()
            .position(|row| self.sidebar_cursor_for(row) == Some(cursor))
    }

    fn move_sidebar_cursor(&mut self, step: CursorMove, cx: &mut Context<Self>) {
        let rows = self.sidebar_nav_rows(cx);
        let current = self.sidebar_cursor_index(cx);
        if let Some(index) = moved_cursor_index(current, rows.len(), step) {
            self.sidebar_cursor = self.sidebar_cursor_for(rows[index]);
            self.sidebar_scroll.scroll_to_item(index * 2 + 1);
        }
        cx.notify();
    }

    fn sidebar_cursor_row(&self, cx: &Context<Self>) -> Option<SidebarRow> {
        let index = self.sidebar_cursor_index(cx)?;
        self.sidebar_nav_rows(cx).get(index).copied()
    }

    fn return_focus_to_active_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get(self.active_idx) {
            ws.focus_first(window, cx);
        }
        cx.notify();
    }

    fn sidebar_menu_open(&self) -> bool {
        self.workspace_menu_open.is_some()
            || self.tab_menu_open.is_some()
            || self.session_menu_open.is_some()
            || self.sidebar_customize_menu_open
    }

    fn move_cursor_row(&mut self, row: SidebarRow, down: bool, cx: &mut Context<Self>) {
        match row {
            SidebarRow::Tab(ws_idx, tab_idx) => {
                let Some(ws) = self.workspaces.get(ws_idx) else {
                    return;
                };
                let target = if down {
                    tab_idx + 1
                } else {
                    tab_idx.saturating_sub(1)
                };
                if target >= ws.tab_count() || target == tab_idx {
                    return;
                }
                let tab = &ws.tabs()[tab_idx];
                let drag = TabDrag {
                    workspace_id: ws.id,
                    tab_id: tab.id,
                    title: tab_display_title(tab, tab_idx).into(),
                };
                self.reorder_workspace_tab(&drag, ws_idx, target, cx);
            }
            SidebarRow::Folder(ws_idx) => {
                let order = Self::compute_display_order(&self.workspaces);
                let Some(position) = order.iter().position(|&index| index == ws_idx) else {
                    return;
                };
                let neighbor = if down {
                    order.get(position + 1)
                } else {
                    position
                        .checked_sub(1)
                        .and_then(|previous| order.get(previous))
                };
                if let Some(&neighbor) = neighbor {
                    let id = self.workspaces[ws_idx].id;
                    self.reorder_workspace(id, neighbor, cx);
                }
            }
        }
        if let Some(index) = self.sidebar_cursor_index(cx) {
            self.sidebar_scroll.scroll_to_item(index * 2 + 1);
        }
        cx.notify();
    }

    pub(super) fn handle_sidebar_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sidebar_focus.is_focused(window) || self.renaming_tab.is_some() {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.function {
            return;
        }
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            if self.sidebar_menu_open() {
                self.dismiss_transient_surfaces();
            }
            self.return_focus_to_active_pane(window, cx);
            cx.stop_propagation();
            return;
        }
        if modifiers.alt {
            let down = match key {
                "up" => false,
                "down" => true,
                _ => return,
            };
            if let Some(row) = self.sidebar_cursor_row(cx) {
                self.move_cursor_row(row, down, cx);
            }
            cx.stop_propagation();
            return;
        }
        let step = match key {
            "up" => Some(CursorMove::Previous),
            "down" => Some(CursorMove::Next),
            "home" => Some(CursorMove::First),
            "end" => Some(CursorMove::Last),
            _ => None,
        };
        if let Some(step) = step {
            self.dismiss_transient_surfaces();
            self.move_sidebar_cursor(step, cx);
            cx.stop_propagation();
            return;
        }
        let Some(row) = self.sidebar_cursor_row(cx) else {
            return;
        };
        match (key, row) {
            ("enter", SidebarRow::Folder(ws_idx)) => {
                self.select_workspace(ws_idx, window, cx);
            }
            ("enter", SidebarRow::Tab(ws_idx, tab_idx)) => {
                self.select_workspace_tab(ws_idx, tab_idx, window, cx);
            }
            ("space", SidebarRow::Folder(ws_idx)) => {
                self.toggle_workspace_expanded(ws_idx, cx);
            }
            ("right", SidebarRow::Folder(ws_idx)) => {
                if self.workspaces[ws_idx].sidebar_expanded {
                    self.move_sidebar_cursor(CursorMove::Next, cx);
                } else {
                    self.toggle_workspace_expanded(ws_idx, cx);
                }
            }
            ("left", SidebarRow::Folder(ws_idx)) => {
                if self.workspaces[ws_idx].sidebar_expanded {
                    self.toggle_workspace_expanded(ws_idx, cx);
                }
            }
            ("left", SidebarRow::Tab(ws_idx, _)) => {
                self.sidebar_cursor = self.sidebar_cursor_for(SidebarRow::Folder(ws_idx));
                cx.notify();
            }
            ("f2", SidebarRow::Tab(ws_idx, tab_idx)) => {
                self.begin_tab_rename(ws_idx, tab_idx, cx);
            }
            ("delete", SidebarRow::Tab(ws_idx, tab_idx)) => {
                self.move_sidebar_cursor(CursorMove::Previous, cx);
                self.close_workspace_tab(ws_idx, tab_idx, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorMove, moved_cursor_index};

    #[test]
    fn cursor_moves_clamp_at_both_ends() {
        assert_eq!(
            moved_cursor_index(Some(0), 3, CursorMove::Previous),
            Some(0)
        );
        assert_eq!(moved_cursor_index(Some(2), 3, CursorMove::Next), Some(2));
        assert_eq!(moved_cursor_index(Some(1), 3, CursorMove::Next), Some(2));
    }

    #[test]
    fn cursor_without_position_enters_from_the_matching_end() {
        assert_eq!(moved_cursor_index(None, 4, CursorMove::Next), Some(0));
        assert_eq!(moved_cursor_index(None, 4, CursorMove::Previous), Some(3));
        assert_eq!(moved_cursor_index(Some(1), 4, CursorMove::Last), Some(3));
        assert_eq!(moved_cursor_index(Some(3), 4, CursorMove::First), Some(0));
    }

    #[test]
    fn cursor_has_no_position_in_an_empty_list() {
        assert_eq!(moved_cursor_index(Some(0), 0, CursorMove::Next), None);
    }
}

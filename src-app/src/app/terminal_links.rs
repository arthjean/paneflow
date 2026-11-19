use std::path::{Path, PathBuf};

use gpui::{AnyWindowHandle, Context, Entity, Window};
use paneflow_config::schema::OpenLinksInConfig;

use crate::PaneFlowApp;
use crate::app::agent_send::file_mention;
use crate::app::diff_dock::code::view::FileReveal;
use crate::terminal::TerminalView;

pub(crate) const FILE_NOT_FOUND: &str = "File not found";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    Dock,
    ExternalEditor,
}

pub(crate) fn link_target(setting: OpenLinksInConfig, alternate: bool) -> LinkTarget {
    match (setting, alternate) {
        (OpenLinksInConfig::Dock, false) | (OpenLinksInConfig::ExternalEditor, true) => {
            LinkTarget::Dock
        }
        (OpenLinksInConfig::Dock, true) | (OpenLinksInConfig::ExternalEditor, false) => {
            LinkTarget::ExternalEditor
        }
    }
}

pub(crate) fn link_reveal(line: Option<u32>, col: Option<u32>) -> Option<FileReveal> {
    line.map(|line| FileReveal {
        line: line as usize,
        column: col.map(|col| col as usize),
        end_line: None,
    })
}

pub(crate) struct CodeLink {
    pub(crate) path: PathBuf,
    pub(crate) line: Option<u32>,
    pub(crate) col: Option<u32>,
    pub(crate) alternate: bool,
    pub(crate) window: AnyWindowHandle,
}

impl PaneFlowApp {
    fn terminal_root(&self, terminal: &Entity<TerminalView>, cx: &gpui::App) -> PathBuf {
        self.workspace_idx_for_terminal(terminal, cx)
            .and_then(|idx| self.workspaces.get(idx))
            .or_else(|| self.active_workspace())
            .map(|workspace| PathBuf::from(&workspace.cwd))
            .unwrap_or_default()
    }

    pub(crate) fn follow_code_link(
        &mut self,
        terminal: &Entity<TerminalView>,
        link: CodeLink,
        cx: &mut Context<Self>,
    ) {
        let CodeLink {
            path,
            line,
            col,
            alternate,
            window,
        } = link;
        let setting = self
            .cached_config
            .terminal
            .as_ref()
            .map(|terminal| terminal.resolved_open_links_in())
            .unwrap_or_default();
        if link_target(setting, alternate) == LinkTarget::ExternalEditor {
            let preference = self.cached_config.external_editor.clone();
            cx.background_executor()
                .spawn(async move {
                    crate::editor::open_at_location(&path, line, col, preference.as_deref());
                })
                .detach();
            return;
        }
        let root = self.terminal_root(terminal, cx);
        cx.spawn(async move |this, cx| {
            let probe = path.clone();
            let exists = cx
                .background_executor()
                .spawn(async move { probe.is_file() })
                .await;
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    if exists {
                        app.open_code_link_in_dock(path, root, link_reveal(line, col), window, cx);
                    } else {
                        app.show_toast(FILE_NOT_FOUND, cx);
                    }
                });
            });
        })
        .detach();
    }

    pub(crate) fn open_code_link_in_dock(
        &mut self,
        path: PathBuf,
        root: PathBuf,
        reveal: Option<FileReveal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section.is_some() {
            self.close_settings(cx);
        }
        if !self.diff_dock.open {
            self.open_diff_dock_panel(root.to_string_lossy().into_owned(), cx);
        }
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        self.open_diff_file_tab_at(path, None, reveal, window, cx);
    }

    pub(crate) fn pick_code_link(
        &mut self,
        query: String,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let host = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = host.update(cx, |app, cx| {
                    app.open_file_finder_with(&query, window, cx);
                });
            });
        });
    }

    pub(crate) fn tree_file_dropped(
        &mut self,
        terminal: Entity<TerminalView>,
        path: PathBuf,
        root: PathBuf,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let host = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = host.update(cx, |app, cx| {
                    app.insert_dropped_file(&terminal, &root, &path, window, cx);
                });
            });
        });
    }

    fn insert_dropped_file(
        &mut self,
        terminal: &Entity<TerminalView>,
        root: &Path,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let surface_id = terminal.entity_id().as_u64();
        let agent = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.agent_sessions.values())
            .find(|session| session.surface_id == Some(surface_id))
            .map(|session| session.tool);
        match agent {
            Some(tool) => {
                let text = file_mention(tool, root, path);
                self.prefill_terminal(terminal, &text, None, window, cx);
            }
            None => {
                let outcome =
                    terminal.update(cx, |terminal, cx| terminal.prefill_path(path, window, cx));
                self.terminal_prefill_settled(terminal, None, outcome.map(|_| ()), window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_picks_the_target_and_the_alternate_gesture_inverts_it() {
        assert_eq!(
            link_target(OpenLinksInConfig::Dock, false),
            LinkTarget::Dock
        );
        assert_eq!(
            link_target(OpenLinksInConfig::Dock, true),
            LinkTarget::ExternalEditor
        );
        assert_eq!(
            link_target(OpenLinksInConfig::ExternalEditor, false),
            LinkTarget::ExternalEditor
        );
        assert_eq!(
            link_target(OpenLinksInConfig::ExternalEditor, true),
            LinkTarget::Dock
        );
        assert_eq!(OpenLinksInConfig::default(), OpenLinksInConfig::Dock);
    }

    #[test]
    fn a_link_reveals_its_line_and_column_and_a_bare_path_reveals_nothing() {
        assert_eq!(
            link_reveal(Some(12), Some(4)),
            Some(FileReveal {
                line: 12,
                column: Some(4),
                end_line: None
            })
        );
        assert_eq!(link_reveal(Some(12), None), Some(FileReveal::line(12)));
        assert_eq!(link_reveal(None, Some(3)), None);
    }
}

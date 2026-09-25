use crate::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SettingsSection {
    General,
    Appearance,
    Shortcuts,
    Terminal,
    Agents,
    McpServers,
    Workspaces,
    Worktrees,
}

impl SettingsSection {
    pub(crate) fn owns_its_scroll(self) -> bool {
        matches!(self, SettingsSection::Shortcuts)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ThemeMode {
    Light,
    Dark,
    System,
}

impl ThemeMode {
    pub(crate) fn from_config(mode: Option<&str>, theme_name: Option<&str>) -> Self {
        match mode.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            Some("system") => Self::System,
            _ => Self::from_theme_name(theme_name.unwrap_or(crate::theme::DEFAULT_THEME)),
        }
    }

    pub(crate) fn from_theme_name(name: &str) -> Self {
        if crate::theme::theme_name_is_light(name) {
            Self::Light
        } else {
            Self::Dark
        }
    }

    pub(crate) fn as_config_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    pub(crate) fn resolved_theme_name(
        self,
        preset: &crate::theme::ThemePreset,
        appearance: gpui::WindowAppearance,
    ) -> &'static str {
        preset.variant(self.is_light(appearance))
    }

    fn is_light(self, appearance: gpui::WindowAppearance) -> bool {
        match self {
            Self::Light => true,
            Self::Dark => false,
            Self::System => Self::appearance_is_light(appearance),
        }
    }

    pub(crate) fn appearance_is_light(appearance: gpui::WindowAppearance) -> bool {
        matches!(
            appearance,
            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TerminalDropdown {
    CursorShape,
    CursorColor,
    FontWeight,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GeneralDropdown {
    Editor,
    Shell,
    OnQuit,
    EndedSessions,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WorkspaceTemplateDropdown {
    Layout,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkspaceContextMenu {
    pub(crate) idx: usize,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone)]
pub(crate) struct SessionContextMenu {
    pub(crate) scope: crate::app::hosted_sessions::SessionRowScope,
    pub(crate) session: Box<crate::app::hosted_sessions::OwnedSession>,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone, Copy)]
pub(crate) struct TabContextMenu {
    pub(crate) ws_idx: usize,
    pub(crate) tab_idx: usize,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone)]
pub(crate) struct PaneContextMenu {
    pub(crate) pane: Entity<Pane>,
    pub(crate) position: Point<Pixels>,
}

#[derive(Clone)]
pub(crate) struct FilesContextMenu {
    pub(crate) root: std::path::PathBuf,
    pub(crate) path: std::path::PathBuf,
    pub(crate) position: Point<Pixels>,
}

pub(crate) enum ClosedSurfaceRecord {
    Terminal {
        cwd: Option<std::path::PathBuf>,
        replay: Option<Vec<u8>>,
        custom_name: Option<String>,
        font_size: Option<f32>,
    },
    Markdown {
        path: std::path::PathBuf,
    },
}

pub(crate) struct ClosedPaneRecord {
    pub(crate) surface: ClosedSurfaceRecord,
    pub(crate) workspace_idx: usize,
}

impl PaneFlowApp {
    pub(crate) fn create_pane(
        &mut self,
        terminal: Entity<TerminalView>,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        cx.subscribe(&terminal, Self::handle_terminal_event)
            .detach();
        let pane = cx.new(|cx| Pane::new(terminal, workspace_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        pane
    }

    pub(crate) fn create_pane_with_existing_surface(
        &mut self,
        surface: PaneSurface,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let pane = cx.new(|cx| Pane::new_with_surface(surface, workspace_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        pane
    }

    pub(crate) fn record_update_failure(
        &mut self,
        context: &str,
        err: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        log::error!("self-update/{context}: {err:#}");
        let tag = update::UpdateError::classify(err);
        self.emit_update_failure(&tag);
        self.self_update.self_update_status = update::SelfUpdateStatus::Errored;
        self.self_update.update_attempt_count =
            self.self_update.update_attempt_count.saturating_add(1);
        self.show_update_error_toast(&tag, cx);
        cx.notify();
    }
}

pub(crate) fn mount_paneflow_app(window: &mut Window, cx: &mut App) -> Entity<PaneFlowApp> {
    let view = cx.new(PaneFlowApp::new);
    startup_trace::mark("app_state_built");
    view.update(cx, |_, cx| {
        let weak = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if app.read(cx).settings_section != Some(SettingsSection::Shortcuts) {
                return;
            }
            let consumed = app.update(cx, |this, cx| {
                this.intercept_shortcut_keystroke(&event.keystroke, window, cx)
            });
            if consumed {
                cx.stop_propagation();
            }
        })
        .detach();
        let subscription = cx.observe_window_bounds(window, |this, window, cx| {
            crate::window_state::record_windowed_size(window);
            #[cfg(target_os = "linux")]
            crate::window_chrome::linux_backdrop::refresh_blur_region(window);
            if this.settings_section.is_some() {
                this.reset_settings_scroll();
                cx.notify();
                cx.on_next_frame(window, |this, _window, cx| {
                    if this.settings_section.is_some() {
                        cx.notify();
                    }
                });
            } else {
                cx.notify();
            }
        });
        subscription.detach();
    });
    window.on_window_should_close(cx, {
        let view = view.clone();
        move |_window, cx| {
            view.update(cx, |app, cx| app.request_quit(cx));
            false
        }
    });
    view.update(cx, |_, cx| {
        let subscription = cx.observe_window_activation(window, |_, window, cx| {
            crate::agents::notifications::set_window_active(
                window.window_handle().window_id(),
                window.is_window_active(),
            );
            #[cfg(target_os = "linux")]
            crate::window_chrome::linux_backdrop::refresh_blur_region(window);
            cx.notify();
        });
        subscription.detach();
    });
    crate::agents::notifications::set_window_active(
        window.window_handle().window_id(),
        window.is_window_active(),
    );
    let owner = view.downgrade();
    cx.on_window_closed(move |cx, id| {
        crate::agents::notifications::remove_window(id);
        let _ = owner.update(cx, |owner, cx| owner.detached_window_closed(id, cx));
    })
    .detach();

    view.update(cx, |app, cx| {
        app.sync_system_theme_from_window(window, cx);
        let subscription = cx.observe_window_appearance(window, |this, window, cx| {
            this.sync_system_theme_from_window(window, cx);
            cx.notify();
        });
        subscription.detach();
    });

    view.update(cx, |app, cx| {
        if let Some(ws) = app.workspaces.get(app.active_idx) {
            ws.focus_first(window, cx);
        }
        cx.on_next_frame(window, |app, window, cx| {
            app.restore_detached_panes(window, cx);
            app.prompt_session_recovery(window, cx);
        });
    });
    startup_trace::mark("app_mounted");
    view
}

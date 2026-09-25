use gpui::{AppContext, Context, KeyDownEvent, ScrollHandle, Window};

use crate::{PaneFlowApp, SettingsSection, config_writer, keybindings};

impl PaneFlowApp {
    pub(crate) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(SettingsSection::General, window, cx);
    }

    pub(crate) fn open_settings_at(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace_menu_open = None;
        self.settings_section = Some(section);
        self.reset_settings_scroll();
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        self.agent_profile_editor = None;
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.clear_settings_search(cx);
        if section == SettingsSection::Shortcuts {
            self.rebuild_shortcut_rows(cx);
        }
        self.refresh_mcp_status(cx);
        if section == SettingsSection::Agents {
            self.probe_agent_versions(cx);
            self.refresh_integration_status(cx);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn set_shortcut_capture(&mut self, active: bool, cx: &mut Context<Self>) {
        let changed = self.shortcut_capture_active != active;
        self.shortcut_capture_active = active;
        if active {
            self.shortcut_search_input.update(cx, |input, cx| {
                input.clear(cx);
            });
            self.cancel_shortcut_recording();
        } else if changed {
            self.rebuild_shortcut_rows(cx);
        }
    }

    pub(crate) fn clear_shortcut_filters(&mut self, cx: &mut Context<Self>) {
        self.shortcut_capture_active = false;
        self.shortcut_search_input.update(cx, |input, cx| {
            input.clear(cx);
        });
        self.rebuild_shortcut_rows(cx);
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_section = None;
        self.clear_shortcut_filters(cx);
        self.shortcut_reset_pending = false;
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        self.agent_profile_editor = None;
        self.clear_settings_search(cx);
        if self.recording_shortcut_idx.is_some() {
            self.cancel_shortcut_recording();
            let config = paneflow_config::loader::load_config();
            keybindings::apply_keybindings(cx, &config.shortcuts);
        }
    }

    pub(crate) fn cancel_shortcut_recording(&mut self) {
        self.recording_shortcut_idx = None;
        self.shortcut_conflict = None;
    }

    pub(crate) fn start_shortcut_recording(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.set_shortcut_capture(false, cx);
        self.shortcut_reset_pending = false;
        self.recording_shortcut_idx = Some(idx);
        self.shortcut_conflict = None;
    }

    pub(crate) fn reload_shortcuts(&mut self, cx: &mut Context<Self>) {
        let config = paneflow_config::loader::load_config();
        keybindings::apply_keybindings(cx, &config.shortcuts);
        self.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
        self.cancel_shortcut_recording();
        self.rebuild_shortcut_rows(cx);
    }

    pub(crate) fn reset_settings_scroll(&mut self) {
        self.settings_scroll = ScrollHandle::new();
        self.settings_drag = None;
    }

    fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_search_input.update(cx, |inp, cx| {
            inp.clear(cx);
        });
    }

    pub(crate) fn persist_setting(
        &mut self,
        nested: bool,
        key: &'static str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let default_shell_changed = !nested
            && key == "default_shell"
            && normalized_shell_setting(self.cached_config.default_shell.as_deref())
                != normalized_shell_setting(value.as_str());
        self.cached_config =
            config_writer::with_field(&self.cached_config, nested, key, value.clone());
        if crate::terminal::element::apply_font_config(&self.cached_config) {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if !nested
            && matches!(
                key,
                "windows_terminal_material" | "windows_chrome_material" | "macos_chrome_material"
            )
        {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if nested && terminal_key_repaints_open_terminals(key) {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if !nested && key == "reduce_motion" {
            crate::ui_primitives::set_reduce_motion(self.cached_config.reduce_motion_enabled());
        }
        if !nested && key == "editor" {
            self.apply_editor_display(cx);
        }
        if !nested && key == "worktrees" {
            crate::workspace::worktree::set_worktrees_root(self.cached_config.worktrees.dir_path());
        }
        if default_shell_changed {
            self.handle_default_shell_changed(cx);
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok = smol::unblock(move || {
                if nested {
                    config_writer::save_terminal_field_checked(key, value)
                } else {
                    config_writer::save_config_value_checked(key, value)
                }
            })
            .await;
            if !ok {
                log::warn!(
                    "settings: failed to persist {key}; choice is in-memory only this session"
                );
                let _ = this.update(cx, |this, cx| {
                    this.show_toast(format!("Could not save setting: {key}"), cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn handle_default_shell_changed(&mut self, cx: &mut Context<Self>) {
        self.show_toast("Shell updated. New terminals will use it.", cx);
    }

    pub(crate) fn persist_agent_panel_setting(
        &mut self,
        key: &'static str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.cached_config =
            config_writer::with_agent_panel_field(&self.cached_config, key, value.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok =
                smol::unblock(move || config_writer::save_agent_panel_field_checked(key, value))
                    .await;
            if !ok {
                log::warn!(
                    "settings: failed to persist agent_panel.{key}; choice is in-memory only this session"
                );
                let _ = this.update(cx, |this, cx| {
                    this.show_toast(format!("Could not save agent panel setting: {key}"), cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn handle_settings_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.font_dropdown_open {
            let key = event.keystroke.key.as_str();
            match key {
                "escape" => {
                    self.font_dropdown_open = false;
                    self.font_search.clear();
                    cx.notify();
                }
                "backspace" => {
                    self.font_search.pop();
                    cx.notify();
                }
                _ => {
                    if let Some(ch) = &event.keystroke.key_char
                        && !ch.is_empty()
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                    {
                        self.font_search.push_str(ch);
                        cx.notify();
                    }
                }
            }
            return;
        }

        if event.keystroke.key == "escape" && self.recording_shortcut_idx.is_none() {
            if self.terminal_dropdown.is_some() {
                self.terminal_dropdown = None;
            } else if self.general_dropdown.is_some() {
                self.general_dropdown = None;
            } else if self.workspace_template_dropdown.is_some() {
                self.workspace_template_dropdown = None;
            } else if self.agent_profile_editor.is_some() {
                self.close_agent_profile_editor(cx);
            } else {
                self.close_settings(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn intercept_shortcut_keystroke(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.settings_section != Some(SettingsSection::Shortcuts) {
            return false;
        }
        if keybindings::is_bare_modifier(keystroke) {
            return false;
        }

        if self.recording_shortcut_idx.is_some() {
            self.handle_shortcut_recording(keystroke, window, cx);
            cx.notify();
            return true;
        }

        if !self.shortcut_capture_active {
            return false;
        }

        if keystroke.key == "escape" {
            self.set_shortcut_capture(false, cx);
            cx.notify();
            return true;
        }

        let formatted = keybindings::format_keystroke(&keystroke.unparse());
        self.shortcut_search_input.update(cx, |input, cx| {
            input.set_value(formatted, cx);
        });
        cx.notify();
        true
    }

    pub(crate) fn handle_shortcut_recording(
        &mut self,
        keystroke: &gpui::Keystroke,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(idx) = self.recording_shortcut_idx else {
            return;
        };

        if keybindings::is_bare_modifier(keystroke) {
            return;
        }

        if keystroke.key == "escape" {
            self.cancel_shortcut_recording();
            cx.notify();
            return;
        }

        let Some(action_name) = self.effective_shortcuts.get(idx).map(|e| e.action_name) else {
            self.cancel_shortcut_recording();
            cx.notify();
            return;
        };

        let unassigns = matches!(keystroke.key.as_str(), "backspace" | "delete")
            && !keystroke.modifiers.modified();
        if unassigns {
            if !config_writer::unassign_shortcut(action_name) {
                self.cancel_shortcut_recording();
                self.show_toast("Could not save shortcut", cx);
                cx.notify();
                return;
            }
            self.reload_shortcuts(cx);
            cx.notify();
            return;
        }

        let new_key = keystroke.unparse();
        let confirmed = self
            .shortcut_conflict
            .as_ref()
            .is_some_and(|conflict| keybindings::keystrokes_conflict(&conflict.key, &new_key));
        if !confirmed {
            let owner = self
                .effective_shortcuts
                .iter()
                .enumerate()
                .filter(|(other_idx, _)| *other_idx != idx)
                .find(|(_, entry)| {
                    entry
                        .raw_key
                        .as_deref()
                        .is_some_and(|raw| keybindings::keystrokes_conflict(raw, &new_key))
                });
            if let Some((_, owner)) = owner {
                self.shortcut_conflict = Some(crate::settings::tabs::shortcuts::ShortcutConflict {
                    label: keybindings::format_keystroke(&new_key),
                    key: new_key,
                    owner: owner.description.clone(),
                });
                cx.notify();
                return;
            }
        }

        if !config_writer::save_shortcut_checked(&new_key, action_name) {
            self.cancel_shortcut_recording();
            self.show_toast("Could not save shortcut", cx);
            cx.notify();
            return;
        }

        self.reload_shortcuts(cx);
        cx.notify();
    }

    pub(crate) fn process_config_changes(&mut self, cx: &mut Context<Self>) {
        let new_config = self
            .pending_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(config) = new_config {
            crate::terminal::element::apply_font_config(&config);
            let default_shell_changed =
                super::settings::normalized_shell_setting(
                    self.cached_config.default_shell.as_deref(),
                ) != super::settings::normalized_shell_setting(config.default_shell.as_deref());
            let theme_mode = crate::ThemeMode::from_config(
                config.theme_mode.as_deref(),
                config.theme.as_deref(),
            );
            keybindings::apply_keybindings(cx, &config.shortcuts);
            self.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
            crate::theme::set_active_theme(config.theme.as_deref());
            self.reconcile_telemetry_consent(&config, cx);
            crate::workspace::worktree::set_worktrees_root(config.worktrees.dir_path());
            self.cached_config = config;
            self.theme_mode = theme_mode;
            crate::ui_primitives::set_reduce_motion(self.cached_config.reduce_motion_enabled());
            self.apply_editor_display(cx);
            if default_shell_changed {
                self.handle_default_shell_changed(cx);
            }
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
            cx.notify();
        }

        if self
            .theme_changed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            cx.notify();
        }

        crate::theme::publish_theme_generation(cx);
    }

    fn reconcile_telemetry_consent(
        &mut self,
        config: &paneflow_config::schema::PaneFlowConfig,
        cx: &mut Context<Self>,
    ) {
        let new_enabled = config.telemetry.as_ref().and_then(|t| t.enabled);
        let decision = reconcile_telemetry(self.telemetry_enabled_last, new_enabled);
        if !decision.rebuild {
            return;
        }

        let consent = crate::telemetry::client::TelemetryConsent::from_config(new_enabled);
        let (api_key, host) = super::telemetry_events::posthog_endpoint();
        let deactivating_telemetry = std::sync::Arc::clone(&self.telemetry);
        deactivating_telemetry.disable();
        cx.background_spawn(async move {
            smol::unblock(move || deactivating_telemetry.deactivate()).await;
        })
        .detach();
        let (telemetry_client, _) =
            crate::telemetry::client::TelemetryClient::from_consent(consent, api_key, host, || {
                (crate::telemetry::id::telemetry_id(), false)
            });
        let telemetry = std::sync::Arc::new(telemetry_client);
        self.telemetry = std::sync::Arc::clone(&telemetry);
        Self::spawn_telemetry_flusher(telemetry, cx);

        if decision.reenabled {
            self.telemetry
                .capture(crate::telemetry::event::TelemetryEvent::telemetry_reenabled());
        }

        self.telemetry_enabled_last = new_enabled;

        if let Some(msg) = decision.toast_msg {
            self.show_toast(msg, cx);
        }
    }
}

pub(crate) fn normalized_shell_setting(shell: Option<&str>) -> &str {
    shell.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("")
}

fn terminal_key_repaints_open_terminals(key: &str) -> bool {
    matches!(
        key,
        "integrated_glyphs" | "color_emoji" | "cursor_color" | "minimum_contrast"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TelemetryReconciliation {
    pub rebuild: bool,
    pub reenabled: bool,
    pub toast_msg: Option<&'static str>,
}

pub(crate) fn reconcile_telemetry(old: Option<bool>, new: Option<bool>) -> TelemetryReconciliation {
    if old == new {
        return TelemetryReconciliation {
            rebuild: false,
            reenabled: false,
            toast_msg: None,
        };
    }
    let toast_msg = Some(match new {
        Some(true) => "Télémétrie activée",
        Some(false) => "Télémétrie désactivée",
        None => "Télémétrie : la demande réapparaîtra au prochain lancement",
    });
    TelemetryReconciliation {
        rebuild: true,
        reenabled: old == Some(false) && new == Some(true),
        toast_msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hot_reloading_terminal_key_reaches_the_open_terminals() {
        for key in [
            "integrated_glyphs",
            "color_emoji",
            "cursor_color",
            "minimum_contrast",
        ] {
            assert!(terminal_key_repaints_open_terminals(key), "{key}");
        }
        assert!(!terminal_key_repaints_open_terminals("cursor_shape"));
        assert!(!terminal_key_repaints_open_terminals("scrollback_lines"));
    }

    #[test]
    fn the_settings_ladder_rewrites_the_resolved_threshold_through_the_config_writer() {
        let config = paneflow_config::schema::PaneFlowConfig::default();
        let off = config_writer::with_field(
            &config,
            true,
            "minimum_contrast",
            crate::settings::tabs::terminal::minimum_contrast_setting(1),
        );
        let terminal = off.terminal.clone().unwrap_or_default();
        assert_eq!(terminal.resolved_minimum_contrast(), 0.0);

        let auto = config_writer::with_field(
            &off,
            true,
            "minimum_contrast",
            crate::settings::tabs::terminal::minimum_contrast_setting(0),
        );
        assert_eq!(
            auto.terminal
                .clone()
                .unwrap_or_default()
                .resolved_minimum_contrast(),
            paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST
        );
    }

    #[test]
    fn identical_state_is_a_noop() {
        for state in [None, Some(false), Some(true)] {
            let r = reconcile_telemetry(state, state);
            assert!(!r.rebuild, "no rebuild for identical {state:?}");
            assert!(!r.reenabled);
            assert!(r.toast_msg.is_none());
        }
    }

    #[test]
    fn none_to_some_true_rebuilds_but_does_not_flag_reenabled() {
        let r = reconcile_telemetry(None, Some(true));
        assert!(r.rebuild);
        assert!(
            !r.reenabled,
            "first-ever consent (None → true) is not a re-enable"
        );
        assert_eq!(r.toast_msg, Some("Télémétrie activée"));
    }

    #[test]
    fn none_to_some_false_rebuilds() {
        let r = reconcile_telemetry(None, Some(false));
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(r.toast_msg, Some("Télémétrie désactivée"));
    }

    #[test]
    fn some_false_to_some_true_flags_reenabled() {
        let r = reconcile_telemetry(Some(false), Some(true));
        assert!(r.rebuild);
        assert!(
            r.reenabled,
            "opted-out → opted-in is the only transition that emits telemetry_reenabled"
        );
        assert_eq!(r.toast_msg, Some("Télémétrie activée"));
    }

    #[test]
    fn some_true_to_some_false_rebuilds_no_reenabled() {
        let r = reconcile_telemetry(Some(true), Some(false));
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(r.toast_msg, Some("Télémétrie désactivée"));
    }

    #[test]
    fn some_true_to_none_rebuilds() {
        let r = reconcile_telemetry(Some(true), None);
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(
            r.toast_msg,
            Some("Télémétrie : la demande réapparaîtra au prochain lancement")
        );
    }

    #[test]
    fn some_false_to_none_rebuilds_no_reenabled() {
        let r = reconcile_telemetry(Some(false), None);
        assert!(r.rebuild);
        assert!(!r.reenabled);
        assert_eq!(
            r.toast_msg,
            Some("Télémétrie : la demande réapparaîtra au prochain lancement")
        );
    }
}

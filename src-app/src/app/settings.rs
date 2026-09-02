use gpui::{Context, KeyDownEvent, ScrollHandle, Window};

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
        self.profile_menu_open = None;
        self.settings_section = Some(section);
        self.reset_settings_scroll();
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.clear_settings_search(cx);
        if section == SettingsSection::Shortcuts {
            self.rebuild_shortcut_rows(cx);
        }
        self.refresh_mcp_status(cx);
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
            self.recording_shortcut_idx = None;
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
        self.profile_menu_open = None;
        self.clear_shortcut_filters(cx);
        self.collapsed_shortcut_groups.clear();
        self.shortcut_reset_pending = false;
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        self.clear_settings_search(cx);
        if self.recording_shortcut_idx.is_some() {
            self.recording_shortcut_idx = None;
            let config = paneflow_config::loader::load_config();
            keybindings::apply_keybindings(cx, &config.shortcuts);
        }
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
        if nested && matches!(key, "integrated_glyphs" | "color_emoji" | "cursor_color") {
            for ws in &self.workspaces {
                ws.propagate_config(&self.cached_config, cx);
            }
        }
        if !nested && key == "reduce_motion" {
            crate::ui_primitives::set_reduce_motion(self.cached_config.reduce_motion_enabled());
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
            self.recording_shortcut_idx = None;
            cx.notify();
            return;
        }

        let Some(action_name) = self.effective_shortcuts.get(idx).map(|e| e.action_name) else {
            self.recording_shortcut_idx = None;
            cx.notify();
            return;
        };

        let new_key = keystroke.unparse();
        if !config_writer::save_shortcut_checked(&new_key, action_name) {
            self.recording_shortcut_idx = None;
            self.show_toast("Could not save shortcut", cx);
            cx.notify();
            return;
        }

        let config = paneflow_config::loader::load_config();
        keybindings::apply_keybindings(cx, &config.shortcuts);
        self.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
        self.recording_shortcut_idx = None;
        self.rebuild_shortcut_rows(cx);
        cx.notify();
    }
}

fn normalized_shell_setting(shell: Option<&str>) -> &str {
    shell.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("")
}

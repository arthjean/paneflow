//! Settings lifecycle + persistence + key handlers for `PaneFlowApp`.
//!
//! The settings *UI* - the Codex-style nav rail, the content panel, and the
//! per-section bodies - lives in `crate::settings` (`chrome` + `tabs::*`). This
//! module owns the glue on `PaneFlowApp`:
//! - [`PaneFlowApp::open_settings_window`] / [`PaneFlowApp::close_settings`] -
//!   toggle the embedded settings (set/clear `settings_section`).
//! - [`PaneFlowApp::persist_setting`] - the shared cache-mutate + repaint +
//!   off-thread write used by every settings control.
//! - [`PaneFlowApp::handle_settings_key_down`] /
//!   [`PaneFlowApp::handle_shortcut_recording`] - key routing for the font-picker
//!   typeahead, Escape handling, and shortcut capture.

use gpui::{Context, KeyDownEvent, ScrollHandle, Window};

use crate::{PaneFlowApp, SettingsSection, config_writer, keybindings};

impl PaneFlowApp {
    /// Open the embedded settings (Codex-style). The Settings button and the
    /// title-bar / macOS menu route here; it sets `settings_section`, and
    /// `main.rs` then swaps the left rail for the settings nav and the content
    /// area for the section panel. The name is kept for call-site compatibility
    /// there is no separate settings *window* anymore.
    pub(crate) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(SettingsSection::General, window, cx);
    }

    /// Open Settings directly on `section`.
    ///
    /// EP-005 US-017: the preset palette's "Manage presets..." entry lands on
    /// the page that owns the selected preset's source, so the user does not
    /// have to find it again after seeing it in the palette.
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
        // Clear any stale nav search so the landing row is always visible (a
        // leftover query could filter the nav to a section that doesn't match
        // the displayed page).
        self.clear_settings_search(cx);
        // Warm the MCP bridge status off-thread so the MCP page can render its
        // button label without ever doing config I/O during a frame.
        self.refresh_mcp_status(cx);
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    /// Arm or disarm the Shortcuts page's key-capture mode.
    ///
    /// Disarming always drops the captured chord: leaving the mode with a
    /// filter still applied would strand the user on a filtered list whose
    /// cause is no longer visible on screen.
    pub(crate) fn set_shortcut_capture(&mut self, active: bool, cx: &mut Context<Self>) {
        self.shortcut_capture_active = active;
        if !active {
            self.shortcut_captured_key = None;
        }
        // A capture and a text query are two ways to ask the same question, so
        // arming one clears the other rather than intersecting them.
        if active {
            self.shortcut_search_input.update(cx, |input, cx| {
                input.set_value("", cx);
            });
        }
        self.recording_shortcut_idx = None;
    }

    /// Clear every Shortcuts-page filter (text query and captured chord).
    pub(crate) fn clear_shortcut_filters(&mut self, cx: &mut Context<Self>) {
        self.shortcut_capture_active = false;
        self.shortcut_captured_key = None;
        self.shortcut_search_input.update(cx, |input, cx| {
            input.set_value("", cx);
        });
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_section = None;
        self.profile_menu_open = None;
        self.clear_shortcut_filters(cx);
        self.collapsed_shortcut_groups.clear();
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

    /// Drop stale scroll geometry when the settings surface is remounted,
    /// changes page, or the window is resized. GPUI repopulates the handle from
    /// the next `track_scroll` layout pass.
    pub(crate) fn reset_settings_scroll(&mut self) {
        self.settings_scroll = ScrollHandle::new();
        self.settings_drag = None;
    }

    /// Reset the nav search box. Shared by open/close so a reopened settings
    /// page always shows the full, unfiltered section list.
    fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_search_input.update(cx, |inp, cx| {
            inp.clear(cx);
        });
    }

    /// Apply a settings-control change. Mutates the render cache in memory for
    /// instant feedback, repaints, then persists the field to disk off the GPUI
    /// main thread (`smol::unblock`). `nested` routes into the `terminal` block;
    /// a `Null` value clears the field.
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

    /// The shell only binds when a PTY spawns, so a live terminal keeps the
    /// one it was started with. Say so rather than restarting anything under
    /// the user: a running session is work in progress.
    pub(crate) fn handle_default_shell_changed(&mut self, cx: &mut Context<Self>) {
        self.show_toast("Shell updated. New terminals will use it.", cx);
    }

    /// Apply an Agents-panel-scoped settings change. This keeps
    /// `agent_panel` writes as narrow read-modify-writes so profile settings
    /// and future sibling fields survive notification toggles.
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Font dropdown typeahead (Terminal page).
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

        // Shortcuts-page key capture: the pressed chord becomes the filter
        // rather than a rebind. Checked before the Escape branch below so
        // Escape leaves capture mode instead of closing the whole modal - a
        // user who armed capture expects Escape to disarm it first.
        if self.shortcut_capture_active && self.recording_shortcut_idx.is_none() {
            if event.keystroke.key == "escape" {
                self.set_shortcut_capture(false, cx);
                cx.notify();
                return;
            }
            // Bare modifiers are held on the way to a real chord, not a chord.
            if keybindings::is_bare_modifier(&event.keystroke) {
                return;
            }
            self.shortcut_captured_key =
                Some(keybindings::format_keystroke(&event.keystroke.to_string()));
            cx.notify();
            return;
        }

        // Escape (outside active shortcut recording): close an open Terminal-page
        // dropdown first, otherwise leave settings. During recording, Escape
        // falls through to `handle_shortcut_recording`, which cancels capture.
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
            return;
        }

        // Shortcut recording (only on the Shortcuts page).
        if self.settings_section == Some(SettingsSection::Shortcuts) {
            self.handle_shortcut_recording(event, window, cx);
        }
    }

    pub(crate) fn handle_shortcut_recording(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(idx) = self.recording_shortcut_idx else {
            return;
        };

        // Ignore bare modifier presses (Shift alone, Ctrl alone, etc.)
        if keybindings::is_bare_modifier(&event.keystroke) {
            return;
        }

        // Escape cancels recording.
        if event.keystroke.key == "escape" {
            self.recording_shortcut_idx = None;
            cx.notify();
            return;
        }

        // Resolve the action by the row's stable identity, NOT by indexing
        // `DEFAULTS` (the displayed list chains macOS-only defaults, skips
        // unbound rows, and appends user-only actions, so a positional index
        // would rebind the wrong action and corrupt `paneflow.json`).
        let Some(action_name) = self.effective_shortcuts.get(idx).map(|e| e.action_name) else {
            self.recording_shortcut_idx = None;
            cx.notify();
            return;
        };

        // Format keystroke to a GPUI string (e.g. "ctrl-shift-d") and save it.
        let new_key = event.keystroke.to_string();
        if !config_writer::save_shortcut_checked(&new_key, action_name) {
            self.recording_shortcut_idx = None;
            self.show_toast("Could not save shortcut", cx);
            cx.notify();
            return;
        }

        // Re-apply keybindings from the updated config.
        let config = paneflow_config::loader::load_config();
        keybindings::apply_keybindings(cx, &config.shortcuts);
        self.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
        self.recording_shortcut_idx = None;
        cx.notify();
    }
}

fn normalized_shell_setting(shell: Option<&str>) -> &str {
    shell.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("")
}

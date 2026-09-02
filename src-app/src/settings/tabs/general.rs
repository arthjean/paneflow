use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, IntoElement, MouseButton, ParentElement,
    SharedString, Styled, div, prelude::*, px,
};
use paneflow_config::schema::NotifyWhenAgentWaiting;
use serde_json::Value;

use crate::GeneralDropdown;
use crate::PaneFlowApp;
use crate::settings::components::{
    Logo, deferred_select_menu, hairline, render_logo, section_header, select_chevron, select_item,
    select_menu, select_trigger, setting_card, setting_text, toggle_row, toggle_row_with,
};

type SelectOption = (String, Option<Logo>, Value, bool);

impl PaneFlowApp {
    pub(crate) fn render_general_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let config = &self.cached_config;

        let editor_value = config
            .external_editor
            .clone()
            .unwrap_or_else(|| "auto".to_string());
        let editor_opts: Vec<SelectOption> = EDITOR_PRESETS
            .iter()
            .map(|(label, val)| {
                (
                    (*label).to_string(),
                    editor_icon(val),
                    Value::String((*val).to_string()),
                    editor_value == *val,
                )
            })
            .collect();
        let editor_label = editor_opts
            .iter()
            .find(|(_, _, _, selected)| *selected)
            .map(|(label, _, _, _)| label.clone())
            .unwrap_or_else(|| editor_value.clone());

        let editor_row = self.general_select_row(
            GeneralDropdown::Editor,
            "Default editor",
            "Default application for opening files and folders.",
            editor_label,
            editor_icon(&editor_value),
            editor_opts,
            "external_editor",
            ui,
            cx,
        );

        #[cfg(target_os = "windows")]
        let shells: Vec<(&str, String)> = vec![
            ("PowerShell", "pwsh.exe".to_string()),
            ("Windows PowerShell", "powershell.exe".to_string()),
            ("Command Prompt", "cmd.exe".to_string()),
            (
                "Git Bash",
                crate::terminal::shell::find_windows_git_bash()
                    .unwrap_or_else(|| "bash.exe".to_string()),
            ),
        ];
        #[cfg(not(target_os = "windows"))]
        let shells: Vec<(&str, String)> = vec![
            ("zsh", "/bin/zsh".to_string()),
            ("bash", "/bin/bash".to_string()),
            ("sh", "/bin/sh".to_string()),
            ("fish", "/usr/bin/fish".to_string()),
        ];

        let current_shell = config.default_shell.clone().unwrap_or_default();
        let shell_opts: Vec<SelectOption> = shells
            .iter()
            .map(|(label, val)| {
                (
                    (*label).to_string(),
                    None,
                    Value::String(val.clone()),
                    shell_preset_eq(&current_shell, val),
                )
            })
            .collect();
        let shell_label = shell_opts
            .iter()
            .find(|(_, _, _, selected)| *selected)
            .map(|(label, _, _, _)| label.clone())
            .unwrap_or_else(|| {
                if current_shell.is_empty() {
                    "System default".to_string()
                } else {
                    current_shell.clone()
                }
            });

        let shell_row = self.general_select_row(
            GeneralDropdown::Shell,
            "Shell in the integrated terminal",
            "Choose which shell opens in new integrated terminals. Existing terminals keep their shell until restarted.",
            shell_label,
            None,
            shell_opts,
            "default_shell",
            ui,
            cx,
        );

        let defaults_section = div()
            .flex()
            .flex_col()
            .child(section_header(ui, "Defaults"))
            .child(
                setting_card(ui)
                    .child(editor_row)
                    .child(hairline(ui))
                    .child(shell_row),
            );

        div()
            .flex()
            .flex_col()
            .child(self.render_permissions_section(ui, cx))
            .child(self.render_ai_access_section(ui, cx))
            .child(div().mt(px(24.)).child(defaults_section))
            .child(self.render_notifications_section(ui, cx))
    }

    fn render_notifications_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.cached_config.agent_panel.as_ref().is_some_and(|p| {
            p.resolved_notify_when_agent_waiting() != NotifyWhenAgentWaiting::Never
        });
        let target = if enabled {
            Value::String("Never".to_string())
        } else {
            Value::String("PrimaryScreen".to_string())
        };

        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .child(section_header(ui, "Notifications"))
            .child(setting_card(ui).child(toggle_row_with(
                "Native OS notifications",
                "Alert you when an agent needs attention or finishes while Paneflow is unfocused.",
                None,
                ui,
                div()
                    .id("row-native-notifications")
                    .flex_shrink_0()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_agent_panel_setting(
                            "notify_when_agent_waiting",
                            target.clone(),
                            cx,
                        );
                    }))
                    .child(crate::settings::components::toggle_pill(enabled, ui)),
            )))
            .into_any_element()
    }

    fn render_permissions_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bypass = self
            .cached_config
            .claude_code_bypass_permissions
            .unwrap_or(false);

        div()
            .flex()
            .flex_col()
            .child(section_header(ui, "Permissions"))
            .child(setting_card(ui).child(toggle_row(
                "row-claude-bypass",
                "Full access",
                "Claude Code edits any file and runs networked commands without \
                 asking. No protection against prompt injection.",
                None,
                bypass,
                "claude_code_bypass_permissions",
                ui,
                cx,
            )))
            .into_any_element()
    }

    fn render_ai_access_section(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unrestricted = self.cached_config.ai_unrestricted_enabled();
        let fence = self.cached_config.ai_injection_fence_enabled();

        let mut access_card = setting_card(ui).child(toggle_row(
            "row-ai-unrestricted",
            "AI free access",
            "Lets an agent auto-submit prompts to your other panes, without the \
             PANEFLOW_IPC_SCRIPTING gate. Every write is logged.",
            None,
            unrestricted,
            "ai_unrestricted",
            ui,
            cx,
        ));
        if unrestricted {
            access_card = access_card.child(hairline(ui)).child(toggle_row(
                "row-ai-injection-fence",
                "Injection fence",
                "Marks peer-pane output as untrusted when an agent reads it, so a \
                 malicious repo cannot hijack it.",
                None,
                fence,
                "ai_injection_fence",
                ui,
                cx,
            ));
            if !fence {
                access_card = access_card.child(hairline(ui)).child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(px(12.))
                        .text_color(gpui::rgb(0xE0_6C_75))
                        .child(
                            "Fence off: a malicious pane can silently redirect \
                             your agent.",
                        ),
                );
            }
        }

        div()
            .mt(px(24.))
            .flex()
            .flex_col()
            .child(section_header(ui, "AI access"))
            .child(access_card)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn general_select_row(
        &self,
        which: GeneralDropdown,
        title: &'static str,
        description: &'static str,
        current_label: String,
        current_icon: Option<Logo>,
        options: Vec<SelectOption>,
        config_key: &'static str,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = self.general_dropdown == Some(which);

        let mut value = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .flex_1()
            .min_w_0();
        if let Some(icon) = current_icon {
            value = value.child(render_logo(icon, ui));
        }
        value = value.child(
            div()
                .min_w_0()
                .text_size(px(12.))
                .text_color(ui.text)
                .truncate()
                .child(current_label),
        );

        let mut trigger =
            select_trigger(SharedString::from(format!("general-dd-{config_key}")), ui)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.general_dropdown = if is_open { None } else { Some(which) };
                        this.settings_focus.focus(window, cx);
                        cx.notify();
                    }),
                )
                .child(value)
                .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu(
                SharedString::from(format!("general-dd-list-{config_key}")),
                ui,
            )
            .on_mouse_down_out(cx.listener(move |this, _, _w, cx| {
                if this.general_dropdown == Some(which) {
                    this.general_dropdown = None;
                    cx.notify();
                }
            }));
            for (i, (label, icon, value, selected)) in options.into_iter().enumerate() {
                let value_for_click = value;
                let mut item = select_item((config_key, i), selected, ui)
                    .cursor(CursorStyle::Arrow)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.general_dropdown = None;
                        this.persist_setting(false, config_key, value_for_click.clone(), cx);
                    }));
                if let Some(icon) = icon {
                    item = item.child(render_logo(icon, ui));
                }
                item = item.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
                        .child(label),
                );
                menu = menu.child(item);
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(ui, title, description))
            .child(div().flex_shrink_0().child(trigger))
            .into_any_element()
    }
}

pub(crate) const EDITOR_PRESETS: &[(&str, &str)] = &[
    ("Auto-detect", "auto"),
    ("Zed", "zed"),
    ("Cursor", "cursor"),
    ("Windsurf", "windsurf"),
    ("VS Code", "code"),
    ("Visual Studio", "visual_studio"),
    ("System default", "system"),
];

pub(crate) fn editor_icon(value: &str) -> Option<Logo> {
    match value {
        "zed" => Some(("icons/editor-zed.png", true)),
        "code" => Some(("icons/editor-vscode.png", true)),
        "visual_studio" => Some(("icons/editor-visual-studio.png", true)),
        "cursor" => Some(("icons/editor-cursor.svg", false)),
        "windsurf" => Some(("icons/editor-windsurf.svg", false)),
        _ => None,
    }
}

fn shell_preset_eq(stored: &str, chip: &str) -> bool {
    fn has_separator(s: &str) -> bool {
        s.contains(['/', '\\'])
    }

    fn path_key(s: &str) -> String {
        s.replace('/', "\\").to_ascii_lowercase()
    }

    fn stem(s: &str) -> String {
        let base = s
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(s)
            .to_ascii_lowercase();
        base.trim_end_matches(".exe").to_string()
    }

    if stored.is_empty() {
        false
    } else if has_separator(stored) && has_separator(chip) {
        path_key(stored) == path_key(chip)
    } else {
        stem(stored) == stem(chip)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_preset_matches_bare_names_by_basename() {
        assert!(super::shell_preset_eq(
            "bash.exe",
            r"C:\Program Files\Git\bin\bash.exe"
        ));
        assert!(super::shell_preset_eq(
            r"C:\Program Files\Git\bin\bash.exe",
            "bash.exe"
        ));
    }

    #[test]
    fn shell_preset_does_not_label_explicit_wsl_bash_as_git_bash() {
        assert!(!super::shell_preset_eq(
            r"C:\Windows\System32\bash.exe",
            r"C:\Program Files\Git\bin\bash.exe"
        ));
    }
}

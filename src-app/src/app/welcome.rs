use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ObjectFit, ParentElement, SharedString, Stateful, Styled, Window, div, img, prelude::*, px,
    svg,
};

use crate::PaneFlowApp;
use crate::SettingsSection;
use crate::agent_launcher::AgentLaunch;
use crate::settings::components::select_item;
use crate::ui_primitives::squircle::squircle_fill;

const WELCOME_COLUMN_WIDTH: f32 = 460.0;
const WELCOME_TAGLINE: &str = "The cockpit for coding agents";
const AGENT_SUMMARY_NAMES: usize = 3;

fn welcome_row(
    id: &str,
    icon: &'static str,
    label: impl Into<SharedString>,
    shortcut: Option<String>,
    ui: crate::theme::UiColors,
) -> Stateful<gpui::Div> {
    select_item(SharedString::from(id.to_string()), false, ui)
        .w_full()
        .justify_between()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(
                    svg()
                        .size(px(14.))
                        .flex_none()
                        .path(icon)
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(ui.text)
                        .child(label.into()),
                ),
        )
        .when_some(shortcut, |row, key| {
            row.child(
                div()
                    .flex_none()
                    .pl(px(8.))
                    .text_size(px(11.))
                    .text_color(ui.muted)
                    .child(key),
            )
        })
}

fn welcome_section_header(label: &'static str, ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(4.))
        .pb(px(8.))
        .child(
            div()
                .flex_none()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.muted)
                .child(label),
        )
        .child(div().flex_1().h(px(1.)).bg(ui.border))
}

impl PaneFlowApp {
    pub(crate) fn installed_agent_summary(&self) -> String {
        let mut names: Vec<&str> = AgentLaunch::all(&self.cached_config)
            .iter()
            .filter(|launch| launch.is_installed())
            .map(|launch| launch.agent().display_name())
            .collect();
        names.dedup();
        let extra = names.len().saturating_sub(AGENT_SUMMARY_NAMES);
        names.truncate(AGENT_SUMMARY_NAMES);
        match (names.as_slice(), extra) {
            ([], _) => {
                "No agent CLI found. Settings, Agents lists what Paneflow looks for.".to_string()
            }
            ([only], 0) => format!("{only} found on this machine."),
            (shown, 0) => {
                let (last, head) = shown.split_last().unwrap_or((&"", &[]));
                format!("{} and {last} found on this machine.", head.join(", "))
            }
            (shown, more) => format!(
                "{} and {more} more found on this machine.",
                shown.join(", ")
            ),
        }
    }

    pub(crate) fn open_recent_workspace(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.recent_workspaces.get(idx).cloned() else {
            return;
        };
        if !entry.path.is_dir() {
            self.forget_recent_workspace(&entry.path, cx);
            self.show_toast("That folder is gone", cx);
            return;
        }
        self.open_workspace_folders(std::slice::from_ref(&entry.path), cx);
        let idx = self.active_idx;
        if idx < self.workspaces.len() {
            self.select_workspace(idx, window, cx);
        }
    }

    fn render_welcome_get_started(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let open_key = self
            .shortcut_for_action("new_workspace")
            .map(str::to_string);
        let palette_key = self
            .shortcut_for_action("open_command_palette")
            .map(str::to_string);

        div()
            .flex()
            .flex_col()
            .child(welcome_section_header("Get started", ui))
            .child(
                welcome_row(
                    "welcome-open-folder",
                    "icons/folder-open.svg",
                    "Open folder…",
                    open_key,
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.create_workspace_with_picker(window, cx);
                    cx.stop_propagation();
                })),
            )
            .child(
                welcome_row(
                    "welcome-clone-repo",
                    "icons/brand-github.svg",
                    "Clone repository…",
                    None,
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_clone_repo(window, cx);
                    cx.stop_propagation();
                })),
            )
            .child(
                welcome_row(
                    "welcome-command-palette",
                    "icons/list-details.svg",
                    "Open command palette",
                    palette_key,
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_command_palette(window, cx);
                    cx.stop_propagation();
                })),
            )
            .into_any_element()
    }

    fn render_welcome_recents(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let mut section = div()
            .flex()
            .flex_col()
            .child(welcome_section_header("Recent workspaces", ui));
        for (idx, entry) in self.recent_workspaces.iter().enumerate().take(5) {
            let shortcut = self
                .shortcut_for_action(&format!("select_workspace_{}", idx + 1))
                .map(str::to_string);
            section = section.child(
                welcome_row(
                    &format!("welcome-recent-{idx}"),
                    "icons/folder.svg",
                    entry.title.clone(),
                    shortcut,
                    ui,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_recent_workspace(idx, window, cx);
                    cx.stop_propagation();
                })),
            );
        }
        section.into_any_element()
    }

    fn render_welcome_configure(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        div()
            .flex()
            .flex_col()
            .child(welcome_section_header("Configure", ui))
            .child(
                welcome_row(
                    "welcome-agents",
                    "icons/pointer-2.svg",
                    "Agent profiles",
                    None,
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_settings_at(SettingsSection::Agents, window, cx);
                    cx.stop_propagation();
                })),
            )
            .child(
                welcome_row(
                    "welcome-shortcuts",
                    "icons/square-slash.svg",
                    "Keyboard shortcuts",
                    None,
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_settings_at(SettingsSection::Shortcuts, window, cx);
                    cx.stop_propagation();
                })),
            )
            .child(
                welcome_row(
                    "welcome-appearance",
                    "icons/shadow.svg",
                    "Appearance",
                    None,
                    ui,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_settings_at(SettingsSection::Appearance, window, cx);
                    cx.stop_propagation();
                })),
            )
            .into_any_element()
    }

    pub(crate) fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let has_recents = !self.recent_workspaces.is_empty();
        let headline = if has_recents {
            "Welcome back to Paneflow"
        } else {
            "Welcome to Paneflow"
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .pb(px(4.))
            .child(
                img("icons/paneflow.png")
                    .w(px(44.))
                    .h(px(44.))
                    .flex_none()
                    .object_fit(ObjectFit::Contain),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child(headline),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child(WELCOME_TAGLINE),
                    ),
            );

        let second_section = if has_recents {
            self.render_welcome_recents(cx)
        } else {
            self.render_welcome_configure(cx)
        };

        let column = div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(WELCOME_COLUMN_WIDTH))
                    .max_w_full()
                    .gap(px(20.))
                    .px(px(24.))
                    .py(px(16.))
                    .child(header)
                    .child(self.render_welcome_get_started(cx))
                    .child(second_section)
                    .child(
                        div()
                            .px(px(4.))
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child(self.installed_agent_summary()),
                    ),
            );

        div()
            .id("welcome")
            .track_focus(&self.welcome_focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .child(squircle_fill(
                crate::app::constants::PANE_CARD_RADIUS,
                crate::theme::active_theme().background,
            ))
            .child(column)
            .into_any_element()
    }
}

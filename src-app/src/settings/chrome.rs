use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Point, SharedString, Styled,
    Window, div, prelude::*, px, svg,
};

use crate::settings::search;
use crate::ui_primitives::squircle_skin;
use crate::widgets::scrollbar;
use crate::{PaneFlowApp, SettingsSection};

pub(crate) const SETTINGS_NAV_WIDTH: f32 = crate::SIDEBAR_WIDTH;
pub(super) const SETTINGS_NAV_ROW_MARGIN_X: f32 = 8.0;
pub(super) const SETTINGS_NAV_ROW_PADDING_X: f32 = 7.0;
const SETTINGS_NAV_GROUP_LABEL_INDENT: f32 = SETTINGS_NAV_ROW_MARGIN_X + SETTINGS_NAV_ROW_PADDING_X;
pub(super) const SETTINGS_NAV_ICON_SIZE: f32 = 15.0;
pub(super) const SETTINGS_NAV_ICON_BASELINE_NUDGE: f32 = 2.0;

struct NavItem {
    section: SettingsSection,
    label: &'static str,
    icon: &'static str,
    keywords: &'static [&'static str],
}

struct NavGroup {
    label: &'static str,
    items: &'static [NavItem],
}

const NAV_GROUPS: &[NavGroup] = &[
    NavGroup {
        label: "Personal",
        items: &[
            NavItem {
                section: SettingsSection::General,
                label: "General",
                icon: "icons/settings.svg",
                keywords: &[
                    "window",
                    "decorations",
                    "mode",
                    "shell",
                    "default shell",
                    "permissions",
                    "bypass",
                    "ai access",
                    "free access",
                    "injection fence",
                    "notifications",
                    "native",
                    "toast",
                    "bell",
                ],
            },
            NavItem {
                section: SettingsSection::Appearance,
                label: "Appearance",
                icon: "icons/shadow.svg",
                keywords: &["theme", "themes", "colors", "appearance"],
            },
            NavItem {
                section: SettingsSection::Shortcuts,
                label: "Keyboard Shortcuts",
                icon: "icons/square-slash.svg",
                keywords: &["keyboard", "shortcuts", "keys", "bindings", "hotkey"],
            },
        ],
    },
    NavGroup {
        label: "Terminal",
        items: &[
            NavItem {
                section: SettingsSection::Terminal,
                label: "Terminal",
                icon: "icons/terminal.svg",
                keywords: &["cursor", "font", "font family", "font size"],
            },
            NavItem {
                section: SettingsSection::Workspaces,
                label: "Workspaces",
                icon: "icons/layout-grid.svg",
                keywords: &[
                    "workspace",
                    "workspaces",
                    "project",
                    "layout",
                    "pane",
                    "panes",
                    "flow",
                    "toml",
                    "agent",
                    "command",
                ],
            },
            NavItem {
                section: SettingsSection::Worktrees,
                label: "Worktrees",
                icon: "icons/git-branch-sidebar.svg",
                keywords: &[
                    "worktree",
                    "worktrees",
                    "branch",
                    "checkout",
                    "directory",
                    "cleanup",
                    "remove",
                ],
            },
        ],
    },
    NavGroup {
        label: "Integrations",
        items: &[
            NavItem {
                section: SettingsSection::Agents,
                label: "Agents",
                icon: "icons/pointer-2.svg",
                keywords: &[
                    "ai",
                    "agent",
                    "agents",
                    "claude",
                    "codex",
                    "gemini",
                    "launcher",
                    "profile",
                    "permissions",
                ],
            },
            NavItem {
                section: SettingsSection::McpServers,
                label: "Plugins",
                icon: "icons/at-sign.svg",
                keywords: &[
                    "mcp",
                    "bridge",
                    "server",
                    "integration",
                    "plugin",
                    "plugins",
                ],
            },
        ],
    },
];

fn nav_item_matches(item: &NavItem, query: &str) -> bool {
    query.is_empty()
        || item.label.to_lowercase().contains(query)
        || item.keywords.iter().any(|keyword| keyword.contains(query))
        || search::section_matches(item.section, query)
}

fn nav_section_matches(section: SettingsSection, query: &str) -> bool {
    NAV_GROUPS
        .iter()
        .flat_map(|group| group.items)
        .any(|item| item.section == section && nav_item_matches(item, query))
}

fn first_matching_section(query: &str) -> Option<SettingsSection> {
    NAV_GROUPS
        .iter()
        .flat_map(|group| group.items)
        .find(|item| nav_item_matches(item, query))
        .map(|item| item.section)
}

pub(crate) fn section_title(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::General => "General",
        SettingsSection::Appearance => "Appearance",
        SettingsSection::Shortcuts => "Keyboard Shortcuts",
        SettingsSection::Terminal => "Terminal",
        SettingsSection::Agents => "Agents",
        SettingsSection::McpServers => "Plugins",
        SettingsSection::Workspaces => "Workspaces",
        SettingsSection::Worktrees => "Worktrees",
    }
}

impl PaneFlowApp {
    pub(crate) fn render_settings_nav(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let active = self.settings_section.unwrap_or(SettingsSection::General);
        let query = search::normalize(&self.settings_search_input.read(cx).value());
        let row_background = crate::app::constants::sidebar_tab_hover_background();

        let search = self.render_settings_search(ui, window, cx);

        let mut list = div()
            .id("settings-nav-list")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(px(4.))
            .pb(px(8.));

        let mut any_match = false;
        for group in NAV_GROUPS {
            let items: Vec<&NavItem> = group
                .items
                .iter()
                .filter(|it| nav_item_matches(it, &query))
                .collect();
            if items.is_empty() {
                continue;
            }
            any_match = true;
            list = list.child(
                div()
                    .mt(px(8.))
                    .pl(px(SETTINGS_NAV_GROUP_LABEL_INDENT))
                    .pr(px(8.))
                    .py(px(2.))
                    .child(crate::ui_primitives::section_eyebrow(group.label, ui)),
            );
            for it in items {
                let section = it.section;
                let is_active = section == active;
                let row = squircle_skin(
                    div()
                        .id(SharedString::from(format!("settings-nav-{}", it.label)))
                        .mx(px(SETTINGS_NAV_ROW_MARGIN_X))
                        .px(px(SETTINGS_NAV_ROW_PADDING_X))
                        .py(px(6.))
                        .min_h(px(32.))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.)),
                    SharedString::from(format!("settings-nav-{}-group", it.label)),
                    px(9.),
                    is_active.then_some(row_background),
                    (!is_active).then_some(row_background),
                )
                .child(
                    svg()
                        .size(px(SETTINGS_NAV_ICON_SIZE))
                        .flex_none()
                        .relative()
                        .top(px(SETTINGS_NAV_ICON_BASELINE_NUDGE))
                        .path(it.icon)
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .text_color(ui.text)
                        .truncate()
                        .child(crate::ui_primitives::highlight_matches(
                            it.label.to_string(),
                            &query,
                        )),
                );
                let row = if is_active {
                    row.into_any_element()
                } else {
                    row.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.select_settings_section(section, window, cx);
                    }))
                    .into_any_element()
                };
                list = list.child(row);
            }
        }

        if !any_match {
            list = list.child(
                squircle_skin(
                    div()
                        .id("settings-nav-empty")
                        .mx(px(8.))
                        .my(px(8.))
                        .px(px(8.))
                        .py(px(10.)),
                    "settings-nav-empty-group",
                    px(9.),
                    Some(ui.subtle),
                    None,
                )
                .text_size(px(12.))
                .text_color(ui.muted)
                .child("No matching settings"),
            );
        }

        div()
            .id("settings-nav")
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .w(px(SETTINGS_NAV_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .child(self.render_settings_nav_header(ui, cx))
            .child(div().mx(px(8.)).mt(px(4.)).child(search))
            .child(list)
    }

    fn render_settings_search(
        &self,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.settings_search_input.read(cx).focus_handle.clone();
        let has_query = !self.settings_search_input.read(cx).value().is_empty();
        crate::ui_primitives::filter_field(
            "settings-search",
            "settings-search-clear",
            ui,
            crate::ui_primitives::FilterFieldStyle::sidebar(
                crate::ui_primitives::FilterFieldGlyph::Search,
            ),
            focus.is_focused(window),
            has_query,
            true,
            None,
            self.settings_search_input.clone(),
            cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.settings_search_input
                    .update(cx, |input, cx| input.clear(cx));
                let focus = this.settings_search_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }),
        )
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
            if ev.keystroke.key == "escape" {
                if this.settings_search_input.read(cx).value().is_empty() {
                    this.close_settings(cx);
                } else {
                    this.settings_search_input.update(cx, |inp, cx| {
                        inp.clear(cx);
                    });
                }
                cx.notify();
                cx.stop_propagation();
            }
        }))
        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
            if this
                .settings_search_input
                .read(cx)
                .focus_handle
                .is_focused(window)
            {
                window.blur();
                cx.notify();
            }
        }))
        .into_any_element()
    }

    pub(crate) fn render_settings_content_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let section = self.settings_section.unwrap_or(SettingsSection::General);

        let ipc_banner = self.ipc_status.is_disabled().then(|| {
            use crate::widgets::callout::{Callout, CalloutIcon, CalloutSeverity};
            div().pb(px(16.)).child(
                Callout::new(CalloutSeverity::Warning, "IPC offline")
                    .icon(CalloutIcon::TriangleAlert)
                    .description("External clients (paneflow-ai-hook) will not connect.")
                    .render(),
            )
        });

        let title = div()
            .pb(px(20.))
            .text_size(px(26.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(section_title(section));

        let heading = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(title)
            .when_some(ipc_banner, |d, b| d.child(b))
            .into_any_element();

        let shell = div()
            .id("settings-panel")
            .track_focus(&self.settings_focus)
            .on_key_down(cx.listener(Self::handle_settings_key_down))
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0();

        if section.owns_its_scroll() {
            return shell.child(self.render_shortcuts_page(heading, cx));
        }

        let raw_query = search::normalize(&self.settings_search_input.read(cx).value());
        let page_query = if search::section_matches(section, &raw_query) {
            raw_query
        } else {
            String::new()
        };
        if search::begin_frame(
            page_query,
            self.settings_search_motion.clone(),
            std::time::Instant::now(),
        ) {
            window.request_animation_frame();
        }
        let body = match section {
            SettingsSection::General => self.render_general_content(cx).into_any_element(),
            SettingsSection::Appearance => self.render_appearance_content(cx).into_any_element(),
            SettingsSection::Terminal => self.render_terminal_content(cx).into_any_element(),
            SettingsSection::Agents => self.render_agents_content(cx).into_any_element(),
            SettingsSection::McpServers => self.render_mcp_servers_content(cx).into_any_element(),
            SettingsSection::Workspaces => self.render_workspaces_content(cx).into_any_element(),
            SettingsSection::Worktrees => self.render_worktrees_content(cx).into_any_element(),
            SettingsSection::Shortcuts => gpui::Empty.into_any_element(),
        };
        search::end_frame();

        let column = div()
            .flex()
            .flex_col()
            .child(heading)
            .child(body)
            .into_any_element();

        shell.child(self.render_settings_scroll(column, cx))
    }

    pub(crate) fn settings_reading_column(&self) -> gpui::Div {
        settings_column().pt(px(28.))
    }
}

pub(crate) const SETTINGS_COLUMN_MAX_WIDTH: gpui::Pixels = px(700.);

pub(crate) const SETTINGS_COLUMN_PADDING: gpui::Pixels = px(28.);

pub(crate) fn settings_column() -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .max_w(SETTINGS_COLUMN_MAX_WIDTH)
        .mx_auto()
        .px(SETTINGS_COLUMN_PADDING)
}

impl PaneFlowApp {
    fn render_settings_scroll(
        &self,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let inner = div()
            .id("settings-content")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .min_h_0()
            .pr(scrollbar::SCROLLBAR_GUTTER)
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .flex()
            .flex_col()
            .items_start()
            .child(
                self.settings_reading_column()
                    .flex_none()
                    .pb(px(72.))
                    .child(content),
            );

        let bar = scrollbar::render(
            &self.settings_scroll,
            crate::theme::ui_colors(),
            None,
            "settings-scrollbar-track",
            "settings-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(off) =
                    scrollbar::track_click_offset(&this.settings_scroll, ev.position.y)
                {
                    this.settings_scroll.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.settings_drag =
                    Some(scrollbar::begin_drag(&this.settings_scroll, ev.position.y));
                cx.stop_propagation();
            }),
        );

        div()
            .id("settings-content-wrapper")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(drag) = this.settings_drag
                    && let Some(off) =
                        scrollbar::drag_offset(&this.settings_scroll, &drag, ev.position.y)
                {
                    this.settings_scroll.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let drag = this.settings_drag.take();
                    if scrollbar::end_drag(&this.settings_scroll, drag) {
                        cx.notify();
                    }
                }),
            )
            .child(inner)
            .when_some(bar, |d, sb| d.child(sb))
    }

    pub(crate) fn follow_settings_search(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.settings_section else {
            return;
        };
        let query = search::normalize(&self.settings_search_input.read(cx).value());
        if query.is_empty() || nav_section_matches(current, &query) {
            return;
        }
        if let Some(section) = first_matching_section(&query) {
            self.enter_settings_section(section, cx);
        }
    }

    pub(crate) fn select_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.enter_settings_section(section, cx);
        self.settings_focus.focus(window, cx);
    }

    fn enter_settings_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.settings_section = Some(section);
        self.settings_search_motion.borrow_mut().reset();
        self.reset_settings_scroll();
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.theme_dropdown_open = false;
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        if self.recording_shortcut_idx.is_some() {
            self.recording_shortcut_idx = None;
            let config = paneflow_config::loader::load_config();
            crate::keybindings::apply_keybindings(cx, &config.shortcuts);
        }
        self.shortcut_reset_pending = false;
        self.clear_shortcut_filters(cx);
        if section == SettingsSection::Shortcuts {
            self.rebuild_shortcut_rows(cx);
        }
        if section == SettingsSection::McpServers {
            self.refresh_mcp_status(cx);
        }
        if section == SettingsSection::Agents {
            self.probe_agent_versions(cx);
            self.refresh_integration_status(cx);
        }
        if section == SettingsSection::Workspaces {
            self.sync_workspace_template_inputs(cx);
        }
        cx.notify();
    }
}

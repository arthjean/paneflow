use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Role, SharedString, Stateful, Styled, Window, div,
    prelude::*, px, svg,
};

use crate::keybindings::ShortcutEntry;
use crate::settings::search;
use crate::settings::tabs::shortcuts::any_shortcut_matches;
use crate::ui_primitives::AccessibleControlExt as _;
use crate::ui_primitives::squircle_skin;
use crate::{PaneFlowApp, SettingsSection};

pub(crate) const SETTINGS_NAV_WIDTH: f32 = 300.;
pub(super) const SETTINGS_NAV_ROW_MARGIN_X: f32 = 8.0;
pub(super) const SETTINGS_NAV_ROW_PADDING_X: f32 = 7.0;
const SETTINGS_NAV_GROUP_LABEL_INDENT: f32 = SETTINGS_NAV_ROW_MARGIN_X + SETTINGS_NAV_ROW_PADDING_X;
const SETTINGS_NAV_ICON_SIZE: f32 = 15.0;
pub(super) const SETTINGS_NAV_ROW_GAP: f32 = crate::app::sidebar::SIDEBAR_TITLE_ROW_GAP;

pub(super) fn settings_nav_icon(path: &'static str, ui: crate::theme::UiColors) -> gpui::Div {
    div()
        .flex_none()
        .w(px(crate::app::sidebar::SIDEBAR_FOLDER_SLOT_WIDTH))
        .h(px(crate::app::sidebar::SIDEBAR_ROW_LINE_HEIGHT))
        .pt(px(crate::app::sidebar::sidebar_glyph_top(
            SETTINGS_NAV_ICON_SIZE,
        )))
        .flex()
        .justify_center()
        .child(
            svg()
                .size(px(SETTINGS_NAV_ICON_SIZE))
                .flex_none()
                .path(path)
                .text_color(ui.muted),
        )
}

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
                icon: "icons/settings/settings-02.svg",
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
                icon: "icons/settings/paint-board.svg",
                keywords: &["theme", "themes", "colors", "appearance"],
            },
            NavItem {
                section: SettingsSection::Shortcuts,
                label: "Keyboard Shortcuts",
                icon: "icons/settings/keyboard.svg",
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
                icon: "icons/settings/computer-terminal-02.svg",
                keywords: &["cursor", "font", "font family", "font size"],
            },
            NavItem {
                section: SettingsSection::Editor,
                label: "Editor",
                icon: "icons/settings/pencil-edit-02.svg",
                keywords: &[
                    "editor",
                    "semantic",
                    "highlighting",
                    "language server",
                    "lsp",
                    "rust-analyzer",
                    "zls",
                    "permissions",
                ],
            },
            NavItem {
                section: SettingsSection::Workspaces,
                label: "Workspaces",
                icon: "icons/settings/dashboard-square-01.svg",
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
                section: SettingsSection::Spaces,
                label: "Spaces",
                icon: "icons/settings/layers-01.svg",
                keywords: &["space", "spaces", "context", "work", "personal", "group"],
            },
            NavItem {
                section: SettingsSection::Worktrees,
                label: "Worktrees",
                icon: "icons/settings/git-branch.svg",
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
                icon: "icons/settings/ai-magic.svg",
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
                section: SettingsSection::Usage,
                label: "Usage",
                icon: "icons/settings/chart-bar-line.svg",
                keywords: &[
                    "usage",
                    "limits",
                    "plan",
                    "quota",
                    "subscription",
                    "tokens",
                    "activity",
                ],
            },
            NavItem {
                section: SettingsSection::McpServers,
                label: "Plugins",
                icon: "icons/settings/puzzle.svg",
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

fn nav_item_named(item: &NavItem, query: &str) -> bool {
    item.label.to_lowercase().contains(query)
        || item.keywords.iter().any(|keyword| keyword.contains(query))
}

fn nav_item_matches(item: &NavItem, query: &str, shortcuts: &[ShortcutEntry]) -> bool {
    query.is_empty()
        || nav_item_named(item, query)
        || search::section_matches(item.section, query)
        || (item.section == SettingsSection::Shortcuts && any_shortcut_matches(shortcuts, query))
}

pub(crate) fn shortcuts_named_by(query: &str) -> bool {
    !query.is_empty()
        && NAV_GROUPS
            .iter()
            .flat_map(|group| group.items)
            .any(|item| item.section == SettingsSection::Shortcuts && nav_item_named(item, query))
}

fn nav_section_matches(section: SettingsSection, query: &str, shortcuts: &[ShortcutEntry]) -> bool {
    NAV_GROUPS
        .iter()
        .flat_map(|group| group.items)
        .any(|item| item.section == section && nav_item_matches(item, query, shortcuts))
}

fn first_matching_section(query: &str, shortcuts: &[ShortcutEntry]) -> Option<SettingsSection> {
    NAV_GROUPS
        .iter()
        .flat_map(|group| group.items)
        .find(|item| nav_item_matches(item, query, shortcuts))
        .map(|item| item.section)
}

pub(crate) fn section_title(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::General => "General",
        SettingsSection::Appearance => "Appearance",
        SettingsSection::Shortcuts => "Keyboard Shortcuts",
        SettingsSection::Terminal => "Terminal",
        SettingsSection::Editor => "Editor",
        SettingsSection::Agents => "Agents",
        SettingsSection::Usage => "Usage",
        SettingsSection::McpServers => "Plugins",
        SettingsSection::Workspaces => "Workspaces",
        SettingsSection::Spaces => "Spaces",
        SettingsSection::Worktrees => "Worktrees",
    }
}

const SETTINGS_NAV_LIST_LABEL: &str = "Settings pages";
const SETTINGS_NAV_EMPTY: &str = "No matching settings";

struct NavTab {
    item: &'static NavItem,
    selected: bool,
    position: usize,
    count: usize,
}

struct NavTabGroup {
    label: &'static str,
    tabs: Vec<NavTab>,
}

fn visible_nav_tabs(
    query: &str,
    shortcuts: &[ShortcutEntry],
    active: SettingsSection,
) -> Vec<NavTabGroup> {
    let matching: Vec<(&'static NavGroup, Vec<&'static NavItem>)> = NAV_GROUPS
        .iter()
        .map(|group| {
            let items = group
                .items
                .iter()
                .filter(|item| nav_item_matches(item, query, shortcuts))
                .collect::<Vec<_>>();
            (group, items)
        })
        .filter(|(_, items)| !items.is_empty())
        .collect();
    let count = matching.iter().map(|(_, items)| items.len()).sum();
    let mut position = 0;
    matching
        .into_iter()
        .map(|(group, items)| NavTabGroup {
            label: group.label,
            tabs: items
                .into_iter()
                .map(|item| {
                    position += 1;
                    NavTab {
                        item,
                        selected: item.section == active,
                        position,
                        count,
                    }
                })
                .collect(),
        })
        .collect()
}

fn settings_nav_tab_list() -> Stateful<Div> {
    div()
        .id("settings-nav-list")
        .role(Role::TabList)
        .aria_label(SETTINGS_NAV_LIST_LABEL)
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_x_hidden()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pt(px(4.))
        .pb(px(8.))
}

fn settings_nav_tab(
    tab: &NavTab,
    query: &str,
    ui: crate::theme::UiColors,
    row_background: Hsla,
    on_press: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = tab.item.label;
    let row = squircle_skin(
        div()
            .id(SharedString::from(format!("settings-nav-{label}")))
            .mx(px(SETTINGS_NAV_ROW_MARGIN_X))
            .px(px(SETTINGS_NAV_ROW_PADDING_X))
            .py(px(6.))
            .min_h(px(32.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SETTINGS_NAV_ROW_GAP)),
        SharedString::from(format!("settings-nav-{label}-group")),
        crate::app::constants::rail_row_radius(),
        tab.selected.then_some(row_background),
        (!tab.selected).then_some(row_background),
    )
    .role(Role::Tab)
    .aria_label(label)
    .aria_selected(tab.selected)
    .aria_position_in_set(tab.position)
    .aria_size_of_set(tab.count)
    .child(settings_nav_icon(tab.item.icon, ui))
    .child(
        div()
            .flex_1()
            .min_w_0()
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(ui.text)
            .truncate()
            .child(crate::ui_primitives::highlight_matches(
                label.to_string(),
                query,
            )),
    );
    if tab.selected {
        row
    } else {
        row.on_activate(on_press)
    }
}

fn settings_nav_empty(ui: crate::theme::UiColors) -> Stateful<Div> {
    squircle_skin(
        div()
            .id("settings-nav-empty")
            .mx(px(8.))
            .my(px(8.))
            .px(px(8.))
            .py(px(10.)),
        "settings-nav-empty-group",
        crate::app::constants::rail_row_radius(),
        Some(ui.subtle),
        None,
    )
    .role(Role::Label)
    .aria_label(SETTINGS_NAV_EMPTY)
    .text_size(px(12.))
    .text_color(ui.muted)
    .child(SETTINGS_NAV_EMPTY)
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

        let mut list = settings_nav_tab_list();

        let groups = visible_nav_tabs(&query, &self.effective_shortcuts, active);
        for group in &groups {
            list = list.child(
                div()
                    .mt(px(8.))
                    .pl(px(SETTINGS_NAV_GROUP_LABEL_INDENT))
                    .pr(px(8.))
                    .py(px(2.))
                    .child(crate::ui_primitives::section_eyebrow(group.label, ui)),
            );
            for tab in &group.tabs {
                let section = tab.item.section;
                list = list.child(settings_nav_tab(
                    tab,
                    &query,
                    ui,
                    row_background,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.select_settings_section(section, window, cx);
                    }),
                ));
            }
        }

        if groups.is_empty() {
            list = list.child(settings_nav_empty(ui));
        }

        div()
            .id("settings-nav")
            .font_family("Geist")
            .text_size(px(14.))
            .w(px(SETTINGS_NAV_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                self.backdrop.is_blended(),
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
            "Search settings",
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
                window.blur(cx);
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

        let title_text = div()
            .text_size(px(26.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(section_title(section));
        let title = if section == SettingsSection::Usage {
            self.render_usage_heading(title_text, window, cx)
        } else if section == SettingsSection::Agents {
            self.render_agents_heading(title_text, cx)
        } else if section == SettingsSection::Spaces {
            self.render_spaces_heading(title_text, cx)
        } else if section == SettingsSection::Workspaces {
            self.render_workspaces_heading(title_text, cx)
        } else if section == SettingsSection::Shortcuts {
            self.render_shortcuts_heading(title_text, cx)
        } else if section == SettingsSection::Worktrees {
            self.render_worktrees_heading(title_text)
        } else if section == SettingsSection::Terminal {
            self.render_terminal_heading(title_text)
        } else if section == SettingsSection::Editor {
            self.render_editor_heading(title_text)
        } else if section == SettingsSection::Appearance {
            self.render_appearance_heading(title_text)
        } else if section == SettingsSection::General {
            self.render_general_heading(title_text)
        } else if section == SettingsSection::McpServers {
            self.render_mcp_heading(title_text)
        } else {
            title_text.pb(px(20.)).into_any_element()
        };

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
            return shell.child(self.render_shortcuts_page(heading, window, cx));
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
            SettingsSection::Appearance => self
                .render_appearance_content(window, cx)
                .into_any_element(),
            SettingsSection::Terminal => self.render_terminal_content(cx).into_any_element(),
            SettingsSection::Editor => self.render_editor_content(cx).into_any_element(),
            SettingsSection::Agents => self.render_agents_content(cx),
            SettingsSection::Usage => self.render_usage_content(window, cx),
            SettingsSection::McpServers => self.render_mcp_servers_content(cx).into_any_element(),
            SettingsSection::Workspaces => self.render_workspaces_content(cx).into_any_element(),
            SettingsSection::Spaces => self.render_spaces_content(cx).into_any_element(),
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

        shell.child(self.render_settings_scroll(column, window))
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
    fn render_settings_scroll(&self, content: AnyElement, window: &Window) -> impl IntoElement {
        self.settings_glide.settle(&self.settings_scroll, window);
        let inner = div()
            .id("settings-content")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .min_h_0()
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

        div()
            .id("settings-content-wrapper")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(inner)
            .child(
                self.settings_glide
                    .wheel_layer(self.settings_scroll.clone()),
            )
    }

    pub(crate) fn follow_settings_search(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.settings_section else {
            return;
        };
        let query = search::normalize(&self.settings_search_input.read(cx).value());
        if query.is_empty() || nav_section_matches(current, &query, &self.effective_shortcuts) {
            return;
        }
        if let Some(section) = first_matching_section(&query, &self.effective_shortcuts) {
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
        self.workspace_template_delete_armed = false;
        self.workspace_template_notice = None;
        self.agent_settings_dialog = None;
        self.agent_profile_menu = None;
        self.agent_profile_editor = None;
        self.agent_account_namer = None;
        self.agent_update_menu_open = None;
        if self.recording_shortcut.is_some() {
            self.recording_shortcut = None;
            crate::keybindings::apply_keybindings(cx, &self.cached_config.shortcuts);
        }
        self.shortcut_cursor = None;
        self.reset_shortcut_search(cx);
        if section == SettingsSection::Shortcuts {
            self.seed_shortcut_search(cx);
            self.rebuild_shortcut_rows(cx);
        }
        if section == SettingsSection::Worktrees {
            self.refresh_worktree_listings(cx);
        }
        if section == SettingsSection::McpServers {
            self.refresh_mcp_status(cx);
        }
        if section == SettingsSection::Agents {
            self.probe_agent_versions(cx);
            self.refresh_integration_status(cx);
            self.check_agent_accounts(cx);
        }
        if section == SettingsSection::Spaces {
            self.refresh_space_integration_status(cx);
            self.refresh_integration_status(cx);
            self.check_agent_accounts(cx);
        }
        if section == SettingsSection::Usage {
            self.open_usage_page(cx);
        }
        if section == SettingsSection::Workspaces {
            self.sync_workspace_template_inputs(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NAV_GROUPS, NavTab, NavTabGroup, SETTINGS_NAV_EMPTY, SETTINGS_NAV_ICON_SIZE,
        SETTINGS_NAV_LIST_LABEL, first_matching_section, nav_section_matches, settings_nav_empty,
        settings_nav_tab, settings_nav_tab_list, shortcuts_named_by, visible_nav_tabs,
    };
    use crate::SettingsSection;
    use crate::keybindings::{ShortcutEntry, ShortcutGroup};
    use gpui::{
        ClickEvent, InteractiveElement, IntoElement, Modifiers, ParentElement, Role, Styled,
        accesskit::{Action, Node},
        div, px, size,
    };

    fn exposed<E: gpui::Element>(element: &E) -> (Role, Node) {
        let role = element
            .a11y_role()
            .expect("the element exposes an accessibility role");
        let mut node = Node::new(role);
        element.write_a11y_info(&mut node);
        (role, node)
    }

    fn tabs_of(groups: &[NavTabGroup]) -> Vec<&NavTab> {
        groups.iter().flat_map(|group| &group.tabs).collect()
    }

    fn exposed_tab(tab: &NavTab) -> (Role, Node) {
        exposed(&settings_nav_tab(
            tab,
            "",
            crate::theme::ui_colors(),
            crate::app::constants::sidebar_tab_hover_background(),
            |_, _, _| {},
        ))
    }

    #[test]
    fn the_settings_nav_is_a_tab_list_named_for_the_settings_pages() {
        let (role, node) = exposed(&settings_nav_tab_list());
        assert_eq!(role, Role::TabList);
        assert_eq!(node.label(), Some(SETTINGS_NAV_LIST_LABEL));
    }

    #[test]
    fn every_settings_nav_item_is_a_tab_named_after_its_page_and_selected_when_open() {
        let open = SettingsSection::Usage;
        let groups = visible_nav_tabs("", &[], open);
        let tabs = tabs_of(&groups);
        let every_item = NAV_GROUPS.iter().map(|group| group.items.len()).sum();
        assert_eq!(tabs.len(), every_item);
        for (index, tab) in tabs.iter().enumerate() {
            let (role, node) = exposed_tab(tab);
            let label = tab.item.label;
            let selected = tab.item.section == open;
            assert_eq!(role, Role::Tab, "{label}");
            assert_eq!(node.label(), Some(label));
            assert_eq!(node.is_selected(), Some(selected), "{label}");
            assert_eq!(node.position_in_set(), Some(index + 1), "{label}");
            assert_eq!(node.size_of_set(), Some(every_item), "{label}");
            assert_eq!(
                node.supports_action(Action::Click),
                !selected,
                "{label}: a press opens any page but the one already open"
            );
        }
    }

    #[test]
    fn a_settings_search_keeps_only_the_matching_tabs_and_numbers_them_among_themselves() {
        for query in ["usage", "work", "theme", "keyboard"] {
            let groups = visible_nav_tabs(query, &[], SettingsSection::General);
            let tabs = tabs_of(&groups);
            assert!(!tabs.is_empty(), "{query}");
            assert!(groups.iter().all(|group| !group.tabs.is_empty()), "{query}");
            for (index, tab) in tabs.iter().enumerate() {
                let (_, node) = exposed_tab(tab);
                assert_eq!(node.position_in_set(), Some(index + 1), "{query}");
                assert_eq!(node.size_of_set(), Some(tabs.len()), "{query}");
                assert_eq!(
                    node.is_selected(),
                    Some(tab.item.section == SettingsSection::General),
                    "{query}"
                );
            }
        }
        let usage = visible_nav_tabs("usage", &[], SettingsSection::Usage);
        assert!(
            tabs_of(&usage)
                .iter()
                .any(|tab| tab.item.section == SettingsSection::Usage && tab.selected)
        );
    }

    #[test]
    fn a_search_that_matches_no_page_says_so_to_assistive_technology() {
        assert!(visible_nav_tabs("qqqzzz", &[], SettingsSection::General).is_empty());
        let (role, node) = exposed(&settings_nav_empty(crate::theme::ui_colors()));
        assert_eq!(role, Role::Label);
        assert_eq!(node.label(), Some(SETTINGS_NAV_EMPTY));
    }

    struct NavHarness {
        open: SettingsSection,
    }

    impl gpui::Render for NavHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ui = crate::theme::ui_colors();
            let background = crate::app::constants::sidebar_tab_hover_background();
            let mut list = settings_nav_tab_list();
            for group in visible_nav_tabs("", &[], self.open) {
                for tab in &group.tabs {
                    let section = tab.item.section;
                    let label = tab.item.label;
                    list = list.child(div().debug_selector(move || format!("nav-{label}")).child(
                        settings_nav_tab(
                            tab,
                            "",
                            ui,
                            background,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.open = section;
                                cx.notify();
                            }),
                        ),
                    ));
                }
            }
            div().size_full().flex().flex_col().child(list)
        }
    }

    #[gpui::test]
    fn pressing_a_settings_nav_tab_opens_its_page(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| NavHarness {
            open: SettingsSection::General,
        });
        cx.simulate_resize(size(px(320.), px(900.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let usage = cx
            .debug_bounds("nav-Usage")
            .expect("the Usage tab is painted");
        cx.simulate_click(usage.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |harness, _| harness.open),
            SettingsSection::Usage
        );
    }

    fn shortcut(description: &str, key: &str) -> ShortcutEntry {
        ShortcutEntry {
            key: key.to_string(),
            raw_key: None,
            extra_raw_keys: Vec::new(),
            customized: false,
            description: description.to_string(),
            action_name: "probe",
            group: ShortcutGroup::Panes,
            search_key: key.to_lowercase(),
        }
    }

    #[test]
    fn the_settings_search_finds_a_shortcut_by_its_name() {
        let shortcuts = [shortcut("Zoom the focused pane", "Ctrl+Shift+Enter")];
        assert!(nav_section_matches(
            SettingsSection::Shortcuts,
            "zoom the focused",
            &shortcuts
        ));
        assert_eq!(
            first_matching_section("zoom the focused", &shortcuts),
            Some(SettingsSection::Shortcuts)
        );
        assert!(!nav_section_matches(
            SettingsSection::Shortcuts,
            "zoom the focused",
            &[]
        ));
    }

    #[test]
    fn naming_the_page_shows_every_shortcut() {
        assert!(shortcuts_named_by("keyboard"));
        assert!(shortcuts_named_by("hotkey"));
        assert!(!shortcuts_named_by("split"));
        assert!(!shortcuts_named_by(""));
    }

    #[test]
    fn a_nav_icon_sits_between_the_cap_and_x_height_axes_of_its_label() {
        use crate::app::sidebar::{
            GEIST_CAP_HEIGHT_EM, GEIST_X_HEIGHT_EM, sidebar_glyph_top, sidebar_text_ink_axis,
        };

        let top = sidebar_glyph_top(SETTINGS_NAV_ICON_SIZE);
        assert_eq!(top.fract(), 0., "the glyph lands on a whole pixel");
        let center = top + SETTINGS_NAV_ICON_SIZE / 2.;
        let capitals = sidebar_text_ink_axis(14., GEIST_CAP_HEIGHT_EM);
        let lowercase = sidebar_text_ink_axis(14., GEIST_X_HEIGHT_EM);
        assert!(
            capitals <= center && center <= lowercase,
            "glyph center {center} must sit between the label axes {capitals} and {lowercase}"
        );
    }

    #[test]
    fn every_nav_item_has_an_embedded_icon() {
        for item in NAV_GROUPS.iter().flat_map(|group| group.items) {
            assert!(
                crate::assets::Assets::get(item.icon).is_some(),
                "{} points at the missing icon {}",
                item.label,
                item.icon
            );
        }
    }
}

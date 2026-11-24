use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Role, StatefulInteractiveElement, Styled, deferred, div, prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::ui_primitives::squircle_skin;

use crate::ui_primitives::AccessibleControlExt as _;
use paneflow_config::schema::{SidebarGroupBy, SidebarShow};

#[derive(Clone, Copy)]
pub(super) struct CustomizeMenuState {
    pub open: bool,
    pub show: SidebarShow,
    pub group_by: SidebarGroupBy,
    pub all_expanded: Option<bool>,
}

#[derive(Clone, Copy)]
pub(super) enum CustomizeAction {
    Show(SidebarShowLine),
    GroupBy(SidebarGroupBy),
    SetAllExpanded(bool),
}

impl PaneFlowApp {
    pub(crate) fn set_sidebar_group_by(
        &mut self,
        group_by: SidebarGroupBy,
        cx: &mut Context<Self>,
    ) {
        if self.cached_config.resolved_sidebar_group_by() != group_by {
            self.persist_setting(
                false,
                "sidebar_group_by",
                serde_json::json!(group_by.wire_str()),
                cx,
            );
            self.agents_display_menu_open = false;
            self.sidebar_new_tab_menu_open = false;
            self.sidebar_scroll.set_offset(gpui::Point::default());
        }
        cx.notify();
    }

    fn toggle_sidebar_show(&mut self, line: SidebarShowLine, cx: &mut Context<Self>) {
        if line.is_agent_row_detail() {
            self.agents_display_menu_open = true;
        } else {
            self.sidebar_customize_menu_open = true;
        }

        let value = sidebar_show_value(line.toggled(self.cached_config.sidebar_show));
        self.persist_setting(false, "sidebar_show", value, cx);
        self.refresh_pull_requests(cx);
        cx.notify();
    }

    fn run_customize_action(&mut self, action: CustomizeAction, cx: &mut Context<Self>) {
        match action {
            CustomizeAction::Show(line) => self.toggle_sidebar_show(line, cx),
            CustomizeAction::GroupBy(group_by) => {
                self.sidebar_customize_menu_open = true;
                self.set_sidebar_group_by(group_by, cx);
            }
            CustomizeAction::SetAllExpanded(expanded) => {
                self.set_all_workspaces_expanded(expanded, cx);
            }
        }
    }

    pub(super) fn customize_handlers(
        cx: &mut Context<Self>,
        dismiss: impl Fn(&mut Self) + 'static,
    ) -> MenuHandlers<CustomizeAction> {
        MenuHandlers {
            activate: Rc::new(cx.listener(|this, action: &CustomizeAction, _, cx| {
                this.run_customize_action(*action, cx);
            })),
            dismiss: Rc::new(cx.listener(move |this, _: &(), _, cx| {
                dismiss(this);
                cx.notify();
            })),
        }
    }

    fn set_all_workspaces_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.sidebar_customize_menu_open = true;
        let space = self.active_space.clone();
        for ws in self
            .workspaces
            .iter_mut()
            .filter(|ws| crate::app::spaces::in_space(ws, space.as_ref()))
        {
            ws.sidebar_expanded = expanded;
        }
        self.save_session(cx);
        cx.notify();
    }
}

pub(crate) fn sidebar_show_value(show: SidebarShow) -> serde_json::Value {
    serde_json::json!({
        "branch": show.branch_enabled(),
        "diffstat": show.diffstat_enabled(),
        "pr": show.pr_enabled(),
        "status": show.status_enabled(),
        "indent_guide": show.indent_guide_enabled(),
        "agents": show.agents_enabled(),
        "usage": show.usage_enabled(),
        "agent_logo": show.agent_logo_enabled(),
        "agent_workspace": show.agent_workspace_enabled(),
        "shimmer": show.shimmer_enabled(),
        "agent_shimmer": show.agent_shimmer_enabled(),
        "agent_all_spaces": show.agent_all_spaces_enabled(),
    })
}

#[derive(Clone, Copy)]
pub(super) enum SidebarShowLine {
    Branch,
    Diffstat,
    Pr,
    Status,
    IndentGuide,
    Agents,
    Usage,
    AgentLogo,
    AgentWorkspace,
    Shimmer,
    AgentShimmer,
    AgentAllSpaces,
}

impl SidebarShowLine {
    fn applies_to(self, group_by: SidebarGroupBy) -> bool {
        group_by == SidebarGroupBy::Folder || !matches!(self, Self::Agents | Self::IndentGuide)
    }

    fn is_agent_row_detail(self) -> bool {
        matches!(
            self,
            Self::AgentLogo | Self::AgentWorkspace | Self::AgentShimmer | Self::AgentAllSpaces
        )
    }

    fn toggled(self, mut show: SidebarShow) -> SidebarShow {
        match self {
            Self::Branch => show.branch = Some(!show.branch_enabled()),
            Self::Diffstat => show.diffstat = Some(!show.diffstat_enabled()),
            Self::Pr => show.pr = Some(!show.pr_enabled()),
            Self::Status => show.status = Some(!show.status_enabled()),
            Self::IndentGuide => show.indent_guide = Some(!show.indent_guide_enabled()),
            Self::Agents => show.agents = Some(!show.agents_enabled()),
            Self::Usage => show.usage = Some(!show.usage_enabled()),
            Self::AgentLogo => show.agent_logo = Some(!show.agent_logo_enabled()),
            Self::AgentWorkspace => {
                show.agent_workspace = Some(!show.agent_workspace_enabled());
            }
            Self::Shimmer => show.shimmer = Some(!show.shimmer_enabled()),
            Self::AgentShimmer => show.agent_shimmer = Some(!show.agent_shimmer_enabled()),
            Self::AgentAllSpaces => {
                show.agent_all_spaces = Some(!show.agent_all_spaces_enabled());
            }
        }
        show
    }

    fn checked(self, show: SidebarShow) -> bool {
        match self {
            Self::Branch => show.branch_enabled(),
            Self::Diffstat => show.diffstat_enabled(),
            Self::Pr => show.pr_enabled(),
            Self::Status => show.status_enabled(),
            Self::IndentGuide => show.indent_guide_enabled(),
            Self::Agents => show.agents_enabled(),
            Self::Usage => show.usage_enabled(),
            Self::AgentLogo => show.agent_logo_enabled(),
            Self::AgentWorkspace => show.agent_workspace_enabled(),
            Self::Shimmer => show.shimmer_enabled(),
            Self::AgentShimmer => show.agent_shimmer_enabled(),
            Self::AgentAllSpaces => show.agent_all_spaces_enabled(),
        }
    }
}

pub(super) const SHIMMER_ICON: &str = "icons/palette/sparkles.svg";
pub(crate) const STATUS_ICON: &str = "icons/palette/activity-01.svg";
pub(crate) const INDENT_GUIDE_ICON: &str = "icons/palette/list-tree.svg";
pub(crate) const AGENTS_ICON: &str = "icons/palette/bot.svg";

const SHOW_SUBMENU_LINES: [(&str, &str, SidebarShowLine); 6] = [
    (
        "Branch",
        "icons/git-branch-sidebar.svg",
        SidebarShowLine::Branch,
    ),
    (
        "Diffstat",
        "icons/palette/plus-minus-01.svg",
        SidebarShowLine::Diffstat,
    ),
    ("PR", "icons/git-pull-request.svg", SidebarShowLine::Pr),
    ("Status", STATUS_ICON, SidebarShowLine::Status),
    ("Shimmer", SHIMMER_ICON, SidebarShowLine::Shimmer),
    (
        "Indent guide",
        INDENT_GUIDE_ICON,
        SidebarShowLine::IndentGuide,
    ),
];

const SHOW_SUBMENU_SECTIONS: [(&str, &str, SidebarShowLine); 2] = [
    ("Agents", AGENTS_ICON, SidebarShowLine::Agents),
    (
        "Usage",
        "icons/palette/limitation.svg",
        SidebarShowLine::Usage,
    ),
];

pub(super) fn render_customize_sidebar_button(
    state: CustomizeMenuState,
    icon_size: f32,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let open = state.open;
    let hover = crate::app::constants::sidebar_tab_hover_background();

    squircle_skin(
        div()
            .id("sidebar-customize")
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center(),
        "sidebar-customize-group",
        px(7.),
        open.then_some(hover),
        (!open).then_some(hover),
    )
    .role(Role::Button)
    .aria_label("Customize sidebar")
    .delayed_tooltip(crate::ui_primitives::text_tooltip("Customize sidebar"))
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.sidebar_customize_menu_open = !open;
        cx.notify();
    }))
    .child(
        svg()
            .size(px(icon_size))
            .flex_none()
            .path("icons/filter-2.svg")
            .text_color(ui.muted),
    )
    .when(open, |trigger| trigger.child(render_menu(state, ui, cx)))
    .into_any_element()
}

fn render_menu(
    state: CustomizeMenuState,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = ActionMenu::new()
        .item(
            MenuItem::submenu("Show", show_submenu(state.show, state.group_by))
                .id("sidebar-customize-show")
                .icon(icon::SHOW)
                .mnemonic('s'),
        )
        .item(
            MenuItem::submenu("Group by", group_by_submenu(state.group_by))
                .id("sidebar-customize-group-by")
                .icon(icon::GROUP_BY)
                .detail(state.group_by.label())
                .mnemonic('g'),
        )
        .separator()
        .items(state.all_expanded.map(|all_expanded| {
            let (label, glyph, key) = if all_expanded {
                ("Collapse all", icon::COLLAPSE_ALL, 'c')
            } else {
                ("Expand all", icon::EXPAND_ALL, 'e')
            };
            MenuItem::new(label, CustomizeAction::SetAllExpanded(!all_expanded))
                .id("sidebar-customize-expand-all")
                .icon(glyph)
                .mnemonic(key)
                .keep_open()
        }));
    let handlers =
        PaneFlowApp::customize_handlers(cx, |this| this.sidebar_customize_menu_open = false);

    deferred(crate::ui_primitives::menu_reveal(
        "sidebar-customize-menu-reveal",
        div()
            .absolute()
            .top(px(32.))
            .left(px(0.))
            .occlude()
            .child(menu.element("sidebar-customize-menu", handlers, ui)),
    ))
    .with_priority(3)
    .into_any_element()
}

type ShowSubmenuLine = (&'static str, &'static str, SidebarShowLine);

fn show_submenu_lines(
    lines: &'static [ShowSubmenuLine],
    group_by: SidebarGroupBy,
) -> impl Iterator<Item = ShowSubmenuLine> {
    lines
        .iter()
        .copied()
        .filter(move |(_, _, line)| line.applies_to(group_by))
}

fn show_submenu(show: SidebarShow, group_by: SidebarGroupBy) -> ActionMenu<CustomizeAction> {
    let option = |(label, glyph, line): ShowSubmenuLine| show_option(label, glyph, line, show);
    ActionMenu::new()
        .items(show_submenu_lines(&SHOW_SUBMENU_LINES, group_by).map(option))
        .separator()
        .items(show_submenu_lines(&SHOW_SUBMENU_SECTIONS, group_by).map(option))
}

fn group_by_submenu(current: SidebarGroupBy) -> ActionMenu<CustomizeAction> {
    ActionMenu::new().items(SidebarGroupBy::ALL.iter().map(|&group_by| {
        MenuItem::new(group_by.label(), CustomizeAction::GroupBy(group_by))
            .id(group_by_option_id(group_by))
            .icon(group_by_icon(group_by))
            .radio(group_by == current)
            .mnemonic(group_by_mnemonic(group_by))
            .keep_open()
    }))
}

fn group_by_icon(group_by: SidebarGroupBy) -> &'static str {
    match group_by {
        SidebarGroupBy::Folder => "icons/folder.svg",
        SidebarGroupBy::Status => STATUS_ICON,
    }
}

fn group_by_option_id(group_by: SidebarGroupBy) -> &'static str {
    match group_by {
        SidebarGroupBy::Folder => "sidebar-group-by-folder",
        SidebarGroupBy::Status => "sidebar-group-by-status",
    }
}

fn group_by_mnemonic(group_by: SidebarGroupBy) -> char {
    match group_by {
        SidebarGroupBy::Folder => 'f',
        SidebarGroupBy::Status => 's',
    }
}

fn show_line_id(line: SidebarShowLine) -> &'static str {
    match line {
        SidebarShowLine::Branch => "sidebar-show-branch",
        SidebarShowLine::Diffstat => "sidebar-show-diffstat",
        SidebarShowLine::Pr => "sidebar-show-pr",
        SidebarShowLine::Status => "sidebar-show-status",
        SidebarShowLine::IndentGuide => "sidebar-show-indent-guide",
        SidebarShowLine::Agents => "sidebar-show-agents",
        SidebarShowLine::Usage => "sidebar-show-usage",
        SidebarShowLine::AgentLogo => "agents-show-logo",
        SidebarShowLine::AgentWorkspace => "agents-show-workspace",
        SidebarShowLine::Shimmer => "sidebar-show-shimmer",
        SidebarShowLine::AgentShimmer => "agents-show-shimmer",
        SidebarShowLine::AgentAllSpaces => "agents-show-all-spaces",
    }
}

fn show_line_mnemonic(line: SidebarShowLine) -> char {
    match line {
        SidebarShowLine::Branch => 'b',
        SidebarShowLine::Diffstat => 'd',
        SidebarShowLine::Pr => 'p',
        SidebarShowLine::Status => 's',
        SidebarShowLine::IndentGuide => 'i',
        SidebarShowLine::Agents => 'a',
        SidebarShowLine::Usage => 'u',
        SidebarShowLine::AgentLogo => 'l',
        SidebarShowLine::AgentWorkspace => 'w',
        SidebarShowLine::Shimmer | SidebarShowLine::AgentShimmer => 'h',
        SidebarShowLine::AgentAllSpaces => 'a',
    }
}

pub(super) fn show_option(
    label: &'static str,
    glyph: &'static str,
    line: SidebarShowLine,
    show: SidebarShow,
) -> MenuItem<CustomizeAction> {
    MenuItem::new(label, CustomizeAction::Show(line))
        .id(show_line_id(line))
        .icon(glyph)
        .toggle(line.checked(show))
        .mnemonic(show_line_mnemonic(line))
        .keep_open()
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::PaneFlowConfig;

    #[test]
    fn the_show_object_carries_every_line_including_agents() {
        let value = sidebar_show_value(SidebarShow::default());
        assert_eq!(
            value,
            serde_json::json!({
                "branch": false,
                "diffstat": false,
                "pr": false,
                "status": true,
                "indent_guide": false,
                "agents": true,
                "usage": true,
                "agent_logo": true,
                "agent_workspace": true,
                "shimmer": true,
                "agent_shimmer": true,
                "agent_all_spaces": false,
            })
        );
    }

    #[test]
    fn the_show_submenu_lists_status_and_its_shimmer_between_pr_and_the_indent_guide() {
        assert_eq!(
            SHOW_SUBMENU_LINES.map(|(label, _, _)| label),
            [
                "Branch",
                "Diffstat",
                "PR",
                "Status",
                "Shimmer",
                "Indent guide"
            ]
        );
        assert_eq!(
            SHOW_SUBMENU_SECTIONS.map(|(label, _, _)| label),
            ["Agents", "Usage"]
        );
        let (_, icon, line) = SHOW_SUBMENU_LINES[3];
        assert_eq!(icon, "icons/palette/activity-01.svg");
        assert!(matches!(line, SidebarShowLine::Status));
        for (label, icon, _) in SHOW_SUBMENU_LINES.into_iter().chain(SHOW_SUBMENU_SECTIONS) {
            assert!(
                crate::assets::Assets::get(icon).is_some(),
                "{label} points at the missing icon {icon}"
            );
        }
    }

    #[test]
    fn grouping_by_status_drops_the_agents_section_and_the_indent_guide_from_show() {
        let labels = |lines: &'static [ShowSubmenuLine], group_by| {
            show_submenu_lines(lines, group_by)
                .map(|(label, _, _)| label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(&SHOW_SUBMENU_LINES, SidebarGroupBy::Folder).len(),
            SHOW_SUBMENU_LINES.len()
        );
        assert_eq!(
            labels(&SHOW_SUBMENU_SECTIONS, SidebarGroupBy::Folder),
            ["Agents", "Usage"]
        );
        assert_eq!(
            labels(&SHOW_SUBMENU_LINES, SidebarGroupBy::Status),
            ["Branch", "Diffstat", "PR", "Status", "Shimmer"]
        );
        assert_eq!(
            labels(&SHOW_SUBMENU_SECTIONS, SidebarGroupBy::Status),
            ["Usage"]
        );
    }

    #[test]
    fn every_grouping_has_its_own_option_and_an_embedded_icon() {
        let ids: Vec<_> = SidebarGroupBy::ALL
            .iter()
            .map(|&group_by| group_by_option_id(group_by))
            .collect();
        assert_eq!(ids, ["sidebar-group-by-folder", "sidebar-group-by-status"]);
        for &group_by in SidebarGroupBy::ALL {
            assert!(
                crate::assets::Assets::get(group_by_icon(group_by)).is_some(),
                "{} points at a missing icon",
                group_by.label()
            );
        }
    }

    #[test]
    fn choosing_a_grouping_writes_only_its_key_through_the_config_writer() {
        let config: PaneFlowConfig = serde_json::from_str(
            r#"{"theme": "One Dark", "sidebar_show": {"pr": true, "agents": false}}"#,
        )
        .expect("loads");
        assert_eq!(config.resolved_sidebar_group_by(), SidebarGroupBy::Folder);

        let by_status = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_group_by",
            serde_json::json!(SidebarGroupBy::Status.wire_str()),
        )
        .expect("written");
        assert_eq!(
            by_status.resolved_sidebar_group_by(),
            SidebarGroupBy::Status
        );
        assert_eq!(by_status.theme.as_deref(), Some("One Dark"));
        assert_eq!(by_status.sidebar_show, config.sidebar_show);

        let by_folder = crate::config_writer::with_field(
            &by_status,
            false,
            "sidebar_group_by",
            serde_json::json!(SidebarGroupBy::Folder.wire_str()),
        )
        .expect("written");
        assert_eq!(
            by_folder.resolved_sidebar_group_by(),
            SidebarGroupBy::Folder
        );
    }

    #[test]
    fn the_status_check_follows_the_loaded_config() {
        let line = SidebarShowLine::Status;
        assert!(line.checked(SidebarShow::default()));
        let hidden: PaneFlowConfig =
            serde_json::from_str(r#"{"sidebar_show": {"status": false}}"#).expect("loads");
        assert!(!line.checked(hidden.sidebar_show));
        let invalid: PaneFlowConfig =
            serde_json::from_str(r#"{"sidebar_show": {"status": "yes"}}"#).expect("loads");
        assert!(line.checked(invalid.sidebar_show));
    }

    #[test]
    fn clicking_status_writes_only_that_key_through_the_config_writer() {
        let config: PaneFlowConfig = serde_json::from_str(
            r#"{"theme": "One Dark", "sidebar_show": {"pr": true, "agents": false, "agent_logo": false}}"#,
        )
        .expect("loads");
        let before = config.sidebar_show;
        assert!(before.status_enabled());

        let hidden = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(SidebarShowLine::Status.toggled(before)),
        )
        .expect("written");
        assert!(!hidden.sidebar_show.status_enabled());
        assert_eq!(hidden.theme.as_deref(), Some("One Dark"));
        let mut every_other_key = sidebar_show_value(hidden.sidebar_show);
        every_other_key["status"] = serde_json::json!(true);
        assert_eq!(every_other_key, sidebar_show_value(before));

        let shown = crate::config_writer::with_field(
            &hidden,
            false,
            "sidebar_show",
            sidebar_show_value(SidebarShowLine::Status.toggled(hidden.sidebar_show)),
        )
        .expect("written");
        assert!(shown.sidebar_show.status_enabled());
    }

    #[test]
    fn toggling_another_line_keeps_a_hidden_tree_status() {
        let show = SidebarShow {
            status: Some(false),
            ..Default::default()
        };
        for line in [
            SidebarShowLine::Branch,
            SidebarShowLine::Pr,
            SidebarShowLine::Agents,
            SidebarShowLine::AgentLogo,
        ] {
            let next = crate::config_writer::with_field(
                &PaneFlowConfig::default(),
                false,
                "sidebar_show",
                sidebar_show_value(line.toggled(show)),
            )
            .expect("written");
            assert!(!next.sidebar_show.status_enabled());
        }
    }

    #[test]
    fn hiding_the_agents_section_round_trips_through_the_config_writer() {
        let config = PaneFlowConfig::default();
        assert!(config.sidebar_show.agents_enabled());
        let mut show = config.sidebar_show;
        show.agents = Some(!show.agents_enabled());

        let next = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(show),
        )
        .expect("written");

        assert!(!next.sidebar_show.agents_enabled());
        assert_eq!(next.sidebar_show.indent_guide, Some(false));
    }

    #[test]
    fn hiding_an_agent_row_detail_round_trips_through_the_config_writer() {
        let config = PaneFlowConfig::default();
        let mut show = config.sidebar_show;
        show.agent_workspace = Some(false);

        let next = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(show),
        )
        .expect("written");

        assert!(!next.sidebar_show.agent_workspace_enabled());
        assert!(next.sidebar_show.agent_logo_enabled());
        assert!(next.sidebar_show.agents_enabled());
    }

    #[test]
    fn each_shimmer_switch_writes_its_own_key_and_reopens_its_own_menu() {
        let config = PaneFlowConfig::default();
        assert!(SidebarShowLine::Shimmer.checked(config.sidebar_show));
        assert!(SidebarShowLine::AgentShimmer.checked(config.sidebar_show));
        assert!(!SidebarShowLine::Shimmer.is_agent_row_detail());
        assert!(SidebarShowLine::AgentShimmer.is_agent_row_detail());

        let tree_still = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(SidebarShowLine::Shimmer.toggled(config.sidebar_show)),
        )
        .expect("written");
        assert!(!tree_still.sidebar_show.shimmer_enabled());
        assert!(tree_still.sidebar_show.agent_shimmer_enabled());
        assert!(tree_still.sidebar_show.status_enabled());

        let both_still = crate::config_writer::with_field(
            &tree_still,
            false,
            "sidebar_show",
            sidebar_show_value(SidebarShowLine::AgentShimmer.toggled(tree_still.sidebar_show)),
        )
        .expect("written");
        assert!(!both_still.sidebar_show.shimmer_enabled());
        assert!(!both_still.sidebar_show.agent_shimmer_enabled());
        assert!(crate::assets::Assets::get(SHIMMER_ICON).is_some());
    }

    #[test]
    fn showing_every_space_writes_its_own_key_and_reopens_the_agent_rows_menu() {
        let config = PaneFlowConfig::default();
        let line = SidebarShowLine::AgentAllSpaces;
        assert!(!line.checked(config.sidebar_show));
        assert!(line.is_agent_row_detail());

        let everywhere = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(line.toggled(config.sidebar_show)),
        )
        .expect("written");
        assert!(everywhere.sidebar_show.agent_all_spaces_enabled());
        assert!(line.checked(everywhere.sidebar_show));
        let mut every_other_key = sidebar_show_value(everywhere.sidebar_show);
        every_other_key["agent_all_spaces"] = serde_json::json!(false);
        assert_eq!(every_other_key, sidebar_show_value(config.sidebar_show));

        let active_only = crate::config_writer::with_field(
            &everywhere,
            false,
            "sidebar_show",
            sidebar_show_value(line.toggled(everywhere.sidebar_show)),
        )
        .expect("written");
        assert!(!active_only.sidebar_show.agent_all_spaces_enabled());
    }

    #[test]
    fn hiding_the_usage_capsule_round_trips_through_the_config_writer() {
        let config = PaneFlowConfig::default();
        assert!(config.sidebar_show.usage_enabled());
        let mut show = config.sidebar_show;
        show.usage = Some(false);

        let next = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(show),
        )
        .expect("written");

        assert!(!next.sidebar_show.usage_enabled());
        assert!(next.sidebar_show.agents_enabled());
    }
}

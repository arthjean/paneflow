use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, deferred, div,
    prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_panel, menu_row};
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::squircle_skin;

use paneflow_config::schema::SidebarShow;

#[derive(Clone, Copy)]
pub(super) struct CustomizeMenuState {
    pub open: bool,
    pub submenu_open: bool,
    pub show: SidebarShow,
    pub all_expanded: Option<bool>,
}

const MENU_WIDTH: f32 = 208.0;
const SUBMENU_WIDTH: f32 = 156.0;

impl PaneFlowApp {
    pub(crate) fn close_sidebar_customize_menu(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_customize_menu_open {
            self.sidebar_customize_menu_open = false;
            self.sidebar_show_submenu_open = false;
            cx.notify();
        }
    }

    fn toggle_sidebar_show(&mut self, line: SidebarShowLine, cx: &mut Context<Self>) {
        if line.is_agent_row_detail() {
            self.agents_display_menu_open = true;
        } else {
            self.sidebar_customize_menu_open = true;
            self.sidebar_show_submenu_open = true;
        }

        let value = sidebar_show_value(line.toggled(self.cached_config.sidebar_show));
        self.persist_setting(false, "sidebar_show", value, cx);
        self.refresh_pull_requests(cx);
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
}

impl SidebarShowLine {
    fn is_agent_row_detail(self) -> bool {
        matches!(
            self,
            Self::AgentLogo | Self::AgentWorkspace | Self::AgentShimmer
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
    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.sidebar_customize_menu_open = !open;
        if open {
            this.sidebar_show_submenu_open = false;
        }
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
    let menu = menu_panel(div().id("sidebar-customize-menu"), ui)
        .font_family("Geist")
        .w(px(MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                this.close_sidebar_customize_menu(cx);
            }),
        )
        .child(render_show_row(state, ui, cx))
        .when_some(state.all_expanded, |menu, all_expanded| {
            menu.child(menu_divider(ui))
                .child(render_expand_all_row(all_expanded, ui, cx))
        });

    deferred(crate::ui_primitives::menu_reveal(
        "sidebar-customize-menu-reveal",
        div()
            .absolute()
            .top(px(32.))
            .left(px(0.))
            .occlude()
            .child(menu),
    ))
    .with_priority(3)
    .into_any_element()
}

impl PaneFlowApp {
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

fn render_expand_all_row(
    all_expanded: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    menu_row("sidebar-customize-expand-all", false, ui)
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.set_all_workspaces_expanded(!all_expanded, cx);
        }))
        .child(menu_label(
            if all_expanded {
                "Collapse all"
            } else {
                "Expand all"
            },
            ui,
        ))
        .into_any_element()
}

fn render_show_row(
    state: CustomizeMenuState,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let submenu_open = state.submenu_open;
    menu_row("sidebar-customize-show", submenu_open, ui)
        .relative()
        .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.sidebar_show_submenu_open = !this.sidebar_show_submenu_open;
            cx.notify();
        }))
        .child(menu_label("Show", ui))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/chevron-right.svg")
                .text_color(ui.muted),
        )
        .when(submenu_open, |row| {
            row.child(render_show_submenu(state.show, ui, cx))
        })
        .into_any_element()
}

fn render_show_submenu(
    show: SidebarShow,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = menu_panel(div().id("sidebar-show-submenu"), ui)
        .font_family("Geist")
        .w(px(SUBMENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(SHOW_SUBMENU_LINES.map(|(label, icon, line)| {
            render_show_option(label, icon, line, line.checked(show), ui, cx)
        }))
        .child(menu_divider(ui))
        .children(SHOW_SUBMENU_SECTIONS.map(|(label, icon, line)| {
            render_show_option(label, icon, line, line.checked(show), ui, cx)
        }));

    deferred(crate::ui_primitives::menu_reveal(
        "sidebar-show-submenu-reveal",
        div()
            .absolute()
            .top(px(-5.))
            .left(px(MENU_WIDTH - 12.))
            .occlude()
            .child(menu),
    ))
    .with_priority(4)
    .into_any_element()
}

pub(super) fn render_show_option(
    label: &'static str,
    icon: &'static str,
    line: SidebarShowLine,
    checked: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let id = match line {
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
    };

    menu_row(id, false, ui)
        .role(Role::MenuItemCheckBox)
        .aria_label(label)
        .aria_toggled(if checked {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.toggle_sidebar_show(line, cx);
        }))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(ui.muted),
        )
        .child(menu_label(label, ui))
        .child(div().w(px(14.)).flex_none().child(if checked {
            svg()
                .size(px(13.))
                .path("icons/check.svg")
                .text_color(ui.text)
                .into_any_element()
        } else {
            div().size(px(13.)).into_any_element()
        }))
        .into_any_element()
}

fn menu_divider(ui: crate::theme::UiColors) -> gpui::Div {
    div()
        .mx(px(6.))
        .my(px(4.))
        .h(px(1.))
        .bg(crate::settings::components::menu_divider_color(ui))
}

pub(super) fn menu_label(label: impl Into<SharedString>, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(crate::ui_primitives::BODY)
        .text_color(ui.text)
        .child(Into::<SharedString>::into(label))
        .into_any_element()
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

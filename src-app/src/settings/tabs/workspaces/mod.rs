mod canvas;
mod editor;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};
use paneflow_config::schema::{
    CommandDefinition, CommandTarget, SurfaceDefinition, WorkspaceDefinition,
};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::app::workspace_ops::templates::*;
use crate::settings::components::{KeyboardStopExt, settings_breadcrumb};
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::Block;
use crate::settings::tabs::spaces::{HintTone, hint_line};
use crate::ui_primitives::AccessibleControlExt;

const BLOCK: &str = "Workspace templates";
const INTRO: &str = "Each template opens a project with its agents, dev servers, and shells \
     already laid out.";
const WELCOME_TITLE: &str = "Open your whole setup in one click";
const WELCOME_BODY: &str = "Save the panes you start on every project, with their agents and \
     commands, and launch them together next time.";
const WELCOME_TEXT_WIDTH: f32 = 380.;
const DEFAULT_SERVER_COMMAND: &str = "bun dev";
const ROW_LOGO: f32 = 14.;
const ROW_LOGO_LIMIT: usize = 3;
const CHIP_GLYPH_WIDTH: f32 = 16.;
const CHIP_GLYPH_HEIGHT: f32 = 12.;
const CHIP_PADDING_X: f32 = 12.;
const CHIP_HEIGHT: f32 = 32.;
const CHIP_RADIUS: f32 = 8.;
const ADD_ICON: &str = "icons/settings/add-01.svg";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateNoticeAt {
    Identity,
    Pane,
    Launch,
}

#[derive(Clone, Debug)]
pub(crate) struct TemplateNotice {
    pub(crate) at: TemplateNoticeAt,
    pub(crate) message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplatePreset {
    AgentAndServer,
    TwoAgents,
    Blank,
}

impl TemplatePreset {
    const ALL: [TemplatePreset; 3] = [Self::AgentAndServer, Self::TwoAgents, Self::Blank];

    fn label(self) -> &'static str {
        match self {
            Self::AgentAndServer => "Agent, server, shell",
            Self::TwoAgents => "Two agents",
            Self::Blank => "Blank",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::AgentAndServer => "agent-server",
            Self::TwoAgents => "two-agents",
            Self::Blank => "blank",
        }
    }

    fn layout(self) -> &'static str {
        match self {
            Self::AgentAndServer => "main_vertical",
            Self::TwoAgents | Self::Blank => "even_h",
        }
    }

    fn panes(self, agents: &[TerminalAgent]) -> Vec<SurfaceDefinition> {
        let agent_pane = |agent: Option<&TerminalAgent>, focus: bool| match agent {
            Some(agent) => SurfaceDefinition {
                surface_type: Some("terminal".to_string()),
                name: Some(agent.display_name().to_string()),
                agent: Some(agent.tag().to_string()),
                focus: focus.then_some(true),
                ..Default::default()
            },
            None => SurfaceDefinition {
                surface_type: Some("terminal".to_string()),
                name: Some("Shell".to_string()),
                focus: focus.then_some(true),
                ..Default::default()
            },
        };
        match self {
            Self::AgentAndServer => vec![
                agent_pane(agents.first(), true),
                SurfaceDefinition {
                    surface_type: Some("terminal".to_string()),
                    name: Some("Dev server".to_string()),
                    command: Some(DEFAULT_SERVER_COMMAND.to_string()),
                    ..Default::default()
                },
                agent_pane(None, false),
            ],
            Self::TwoAgents => vec![
                agent_pane(agents.first(), true),
                agent_pane(agents.get(1).or(agents.first()), false),
            ],
            Self::Blank => vec![agent_pane(None, true)],
        }
    }
}

impl PaneFlowApp {
    pub(crate) fn render_workspaces_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        if self.workspace_template_detail_open
            && let Some(idx) = self.selected_workspace_template_index()
        {
            return self.render_workspace_template_detail(idx, ui, ink, cx);
        }

        let templates = self.workspace_template_indices();
        if templates.is_empty() {
            return Block::new(BLOCK)
                .child(self.render_workspaces_welcome(ui, ink, cx))
                .finish();
        }
        let mut list = kit::group(ink);
        let rows = templates.len() + 1;
        for (position, idx) in templates.into_iter().enumerate() {
            list = list
                .child(self.render_workspace_template_row(
                    idx,
                    kit::Place::at(position, rows),
                    ui,
                    ink,
                    cx,
                ))
                .child(kit::rule(ink));
        }
        list = list.child(
            kit::link_row(
                "workspace-template-create",
                ink,
                Some(kit::disc(ink, ADD_ICON)),
                Text::new("New template"),
                kit::Place::Last,
            )
            .on_activate(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.create_workspace_template(TemplatePreset::Blank, cx);
                let input = this.workspace_template_name_input.clone();
                input.update(cx, |input, cx| input.select_all_text(cx));
                let focus = input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            })),
        );
        Block::new(BLOCK).child(list).finish()
    }

    pub(crate) fn render_workspaces_heading(
        &self,
        title: gpui::Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        if self.workspace_template_detail_open
            && let Some(idx) = self.selected_workspace_template_index()
        {
            let (name, launchable) = self
                .cached_config
                .commands
                .get(idx)
                .and_then(|command| {
                    let workspace = command.workspace()?;
                    Some((
                        template_title(command, workspace),
                        !template_surfaces(workspace).is_empty(),
                    ))
                })
                .unwrap_or_default();
            let launch = kit::button_with(
                "workspace-template-launch",
                "Launch",
                Tone::Primary,
                Size::Medium,
                false,
                launchable,
                ink,
            )
            .when(launchable, |button| {
                button.on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.launch_workspace_template(idx, window, cx);
                }))
            });
            return div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .px(px(kit::TEXT_INSET))
                .pb(px(kit::SECTION_GAP))
                .child(settings_breadcrumb(
                    "workspaces-breadcrumb",
                    "Workspaces",
                    "Back to all templates",
                    name,
                    ui,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.close_workspace_template_detail(window, cx);
                    }),
                ))
                .child(launch)
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(ink, INTRO))
            .into_any_element()
    }

    fn render_workspace_template_row(
        &self,
        idx: usize,
        place: kit::Place,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(command) = self.cached_config.commands.get(idx) else {
            return div().into_any_element();
        };
        let Some(workspace) = command.workspace() else {
            return div().into_any_element();
        };
        let title = template_title(command, workspace);
        let meta = template_meta(workspace.cwd.as_deref(), template_surfaces(workspace).len());
        let agents = canvas::template_agents(workspace);
        let mut text = Text::new(title.clone()).line(kit::line(ink, meta));
        if !agents.is_empty() {
            let mut logos = div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.));
            for agent in agents.into_iter().take(ROW_LOGO_LIMIT) {
                logos = logos.child(canvas::agent_logo(agent, ROW_LOGO, ui));
            }
            text = text.badge(logos);
        }
        kit::link_row(
            ("workspace-template-row", idx),
            ink,
            Some(canvas::layout_mark(workspace, ink)),
            text,
            place,
        )
        .aria_label(format!("Open template {title}"))
        .on_activate(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.open_workspace_template(idx, cx);
        }))
        .into_any_element()
    }

    fn render_workspaces_welcome(
        &self,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agents = TerminalAgent::visible(&self.cached_config);
        let mut presets = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_center()
            .gap(px(8.));
        for preset in TemplatePreset::ALL {
            let mark = if preset == TemplatePreset::Blank {
                canvas::glyph(ADD_ICON, 14., ink.primary)
            } else {
                canvas::preset_glyph(
                    preset.layout(),
                    preset.panes(&agents).len(),
                    CHIP_GLYPH_WIDTH,
                    CHIP_GLYPH_HEIGHT,
                    ink.secondary,
                )
            };
            presets = presets.child(template_chip(preset, mark, ink).on_activate(cx.listener(
                move |this, _: &ClickEvent, _window, cx| {
                    this.create_workspace_template(preset, cx);
                },
            )));
        }
        kit::group(ink)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(px(32.))
                    .pt(px(32.))
                    .pb(px(28.))
                    .child(canvas::welcome_preview(agents.first().copied(), ui))
                    .child(kit::headline(ink, WELCOME_TITLE).pt(px(20.)))
                    .child(
                        kit::page_description(ink, WELCOME_BODY)
                            .pt(px(6.))
                            .max_w(px(WELCOME_TEXT_WIDTH))
                            .text_center(),
                    )
                    .child(div().pt(px(20.)).child(presets)),
            )
            .into_any_element()
    }

    fn create_workspace_template(&mut self, preset: TemplatePreset, cx: &mut Context<Self>) {
        let cwd = self
            .active_workspace()
            .map(|ws| ws.cwd.clone())
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_default();
        let base = std::path::Path::new(&cwd)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "New template".to_string());
        let name = unique_template_name(&self.cached_config.commands, &base);
        let agents = TerminalAgent::visible(&self.cached_config);
        let layout_preset = preset.layout().to_string();
        let command = CommandDefinition {
            name: name.clone(),
            description: Some(format!("Workspace template for {cwd}")),
            keywords: vec!["workspace".to_string()],
            target: CommandTarget::Workspace {
                workspace: WorkspaceDefinition {
                    name: Some(name),
                    cwd: Some(cwd),
                    layout_preset: Some(layout_preset.clone()),
                    color: None,
                    layout: build_layout_from_surfaces(&layout_preset, preset.panes(&agents)),
                },
            },
        };
        let mut commands = self.cached_config.commands.clone();
        commands.push(command);
        let idx = commands.len() - 1;
        self.persist_workspace_commands(commands, cx);
        self.open_workspace_template(idx, cx);
    }

    pub(crate) fn open_workspace_template(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.workspace_template_selected = Some(idx);
        self.workspace_template_detail_open = true;
        self.workspace_template_selected_pane = 0;
        self.workspace_template_notice = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_delete_armed = false;
        self.reset_settings_scroll();
        self.sync_workspace_template_inputs(cx);
        cx.notify();
    }

    pub(crate) fn close_workspace_template_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_workspace_template_if_editing(window, cx);
        self.workspace_template_detail_open = false;
        self.workspace_template_dropdown = None;
        self.workspace_template_notice = None;
        self.workspace_template_delete_armed = false;
        self.settings_focus.focus(window, cx);
        self.reset_settings_scroll();
        cx.notify();
    }

    pub(crate) fn step_back_from_workspace_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_template_delete_armed {
            self.workspace_template_delete_armed = false;
            cx.notify();
            return;
        }
        self.close_workspace_template_detail(window, cx);
    }

    fn duplicate_workspace_template(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(mut copy) = self.cached_config.commands.get(idx).cloned() else {
            return;
        };
        let name =
            unique_template_name(&self.cached_config.commands, &format!("{} copy", copy.name));
        copy.name = name.clone();
        if let Some(workspace) = copy.workspace_mut() {
            workspace.name = Some(name);
        }
        let mut commands = self.cached_config.commands.clone();
        commands.push(copy);
        let idx = commands.len() - 1;
        self.persist_workspace_commands(commands, cx);
        self.open_workspace_template(idx, cx);
    }

    fn delete_workspace_template(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if idx >= self.cached_config.commands.len() {
            return;
        }
        let mut commands = self.cached_config.commands.clone();
        commands.remove(idx);
        self.workspace_template_selected = None;
        self.workspace_template_selected_pane = 0;
        self.persist_workspace_commands(commands, cx);
        self.workspace_template_delete_armed = false;
        self.workspace_template_detail_open = false;
        self.workspace_template_notice = None;
        self.settings_focus.focus(window, cx);
        self.reset_settings_scroll();
        self.sync_workspace_template_inputs(cx);
        cx.notify();
    }

    pub(crate) fn launch_workspace_template(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_workspace_template_fields(cx);
        let commands = self.cached_config.commands.clone();
        let params = match self.workspace_up_params(&commands, idx) {
            Ok(params) => params,
            Err(message) => {
                self.set_template_notice(TemplateNoticeAt::Launch, sentence(&message), cx);
                return;
            }
        };
        let Some(project) = commands
            .get(idx)
            .and_then(CommandDefinition::workspace)
            .and_then(|workspace| workspace.cwd.as_deref())
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_string)
        else {
            self.set_template_notice(
                TemplateNoticeAt::Launch,
                "Choose a project folder first.".to_string(),
                cx,
            );
            return;
        };
        let path = std::path::PathBuf::from(&project);
        cx.spawn_in(window, async move |this: gpui::WeakEntity<Self>, cx| {
            let state = crate::fs_probe::probe_dirs(
                vec![path],
                crate::app::recents::RECENT_PROBE_TIMEOUT,
                std::path::Path::is_dir,
            )
            .await
            .pop()
            .map(|(_, state)| state);
            let _ = this.update_in(cx, |app, window, cx| match state {
                Some(crate::fs_probe::DirProbe::Dir) => {
                    app.start_template_workspace(&params, window, cx)
                }
                Some(crate::fs_probe::DirProbe::Missing) => app.set_template_notice(
                    TemplateNoticeAt::Launch,
                    "The project folder no longer exists. Choose another one.".to_string(),
                    cx,
                ),
                _ => app.set_template_notice(
                    TemplateNoticeAt::Launch,
                    "The project folder did not respond. Try again.".to_string(),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn start_template_workspace(
        &mut self,
        params: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launched = match self.launch_workspace_up_for_user(params, cx) {
            Ok(launched) => launched,
            Err(message) => {
                self.set_template_notice(TemplateNoticeAt::Launch, sentence(&message), cx);
                return;
            }
        };
        cx.spawn_in(window, async move |this: gpui::WeakEntity<Self>, cx| {
            let outcome = launched.await;
            let _ = this.update_in(cx, |app, window, cx| match outcome {
                Ok(idx) => {
                    app.close_settings(cx);
                    app.select_workspace(idx, window, cx);
                }
                Err(message) => {
                    app.set_template_notice(TemplateNoticeAt::Launch, sentence(&message), cx);
                }
            });
        })
        .detach();
    }

    fn set_template_notice(
        &mut self,
        at: TemplateNoticeAt,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.workspace_template_notice = Some(TemplateNotice { at, message });
        cx.notify();
    }

    fn clear_template_notice(&mut self, at: TemplateNoticeAt) {
        if self
            .workspace_template_notice
            .as_ref()
            .is_some_and(|notice| notice.at == at)
        {
            self.workspace_template_notice = None;
        }
    }

    fn template_notice(&self, at: TemplateNoticeAt) -> Option<String> {
        self.workspace_template_notice
            .as_ref()
            .filter(|notice| notice.at == at)
            .map(|notice| notice.message.clone())
    }

    fn persist_workspace_commands(
        &mut self,
        commands: Vec<CommandDefinition>,
        cx: &mut Context<Self>,
    ) {
        self.cached_config =
            crate::config_writer::with_commands(&self.cached_config, commands.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok =
                smol::unblock(move || crate::config_writer::save_commands_checked(commands)).await;
            if ok {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                log::warn!("settings: failed to persist workspace templates");
                this.show_toast("Workspace templates could not be saved", cx);
            });
        })
        .detach();
    }

    fn workspace_template_indices(&self) -> Vec<usize> {
        self.cached_config
            .commands
            .iter()
            .enumerate()
            .filter_map(|(idx, command)| command.workspace().is_some().then_some(idx))
            .collect()
    }

    fn selected_workspace_template_index(&self) -> Option<usize> {
        if let Some(idx) = self.workspace_template_selected
            && self
                .cached_config
                .commands
                .get(idx)
                .and_then(CommandDefinition::workspace)
                .is_some()
        {
            return Some(idx);
        }
        self.cached_config
            .commands
            .iter()
            .position(|command| command.workspace().is_some())
    }
}

fn template_chip(preset: TemplatePreset, mark: AnyElement, ink: Ink) -> gpui::Stateful<gpui::Div> {
    let id = SharedString::from(format!("workspace-template-preset-{}", preset.slug()));
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .h(px(CHIP_HEIGHT))
        .pl(px(CHIP_PADDING_X - 2.))
        .pr(px(CHIP_PADDING_X))
        .rounded(px(CHIP_RADIUS))
        .bg(ink.well)
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(ink.well_hover))
        .active(move |style| style.bg(ink.well_pressed))
        .accessible_control(
            gpui::accesskit::Role::Button,
            format!("Create a template: {}", preset.label()),
        )
        .keyboard_stop(px(CHIP_RADIUS))
        .child(mark)
        .child(
            div()
                .text_size(px(kit::SIZE_MD))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink.primary)
                .child(preset.label()),
        )
}

fn template_title(command: &CommandDefinition, workspace: &WorkspaceDefinition) -> String {
    workspace
        .name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(command.name.as_str())
        .to_string()
}

fn pane_count_label(panes: usize) -> String {
    match panes {
        0 => "No panes yet".to_string(),
        1 => "1 pane".to_string(),
        n => format!("{n} panes"),
    }
}

fn template_meta(cwd: Option<&str>, panes: usize) -> String {
    let panes = pane_count_label(panes);
    match cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        Some(cwd) => format!(
            "{} · {panes}",
            crate::settings::tabs::worktrees::display_path(std::path::Path::new(cwd))
        ),
        None => format!("No project folder · {panes}"),
    }
}

fn unique_template_name(commands: &[CommandDefinition], base: &str) -> String {
    let taken = |name: &str| commands.iter().any(|command| command.name == name);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_string())
}

fn sentence(message: &str) -> String {
    let message = message.trim().trim_end_matches('.');
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(name: &str) -> CommandDefinition {
        CommandDefinition {
            name: name.to_string(),
            description: None,
            keywords: Vec::new(),
            target: CommandTarget::Workspace {
                workspace: WorkspaceDefinition {
                    name: Some(name.to_string()),
                    cwd: None,
                    layout_preset: None,
                    color: None,
                    layout: None,
                },
            },
        }
    }

    #[test]
    fn a_new_template_name_never_collides_with_an_existing_one() {
        let commands = vec![template("paneflow"), template("paneflow 2")];
        assert_eq!(unique_template_name(&commands, "paneflow"), "paneflow 3");
        assert_eq!(unique_template_name(&commands, "site"), "site");
    }

    #[test]
    fn a_template_summary_counts_its_panes_in_plain_words() {
        assert_eq!(template_meta(None, 0), "No project folder · No panes yet");
        assert_eq!(template_meta(Some("/srv/app"), 1), "/srv/app · 1 pane");
        assert_eq!(template_meta(Some("/srv/app"), 3), "/srv/app · 3 panes");
    }

    #[test]
    fn launch_errors_read_as_sentences() {
        assert_eq!(
            sentence("add at least one pane before running"),
            "Add at least one pane before running."
        );
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn every_preset_starts_with_its_first_pane_focused() {
        for preset in TemplatePreset::ALL {
            let panes = preset.panes(&[]);
            assert!(!panes.is_empty(), "{}", preset.label());
            assert_eq!(panes[0].focus, Some(true), "{}", preset.label());
        }
    }
}

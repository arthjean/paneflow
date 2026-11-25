mod catalog;
pub(crate) mod matcher;

use std::ops::Range;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, Role, ScrollStrategy, SharedString, Stateful,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, deferred, div, px, svg,
    uniform_list,
};

use crate::PaneFlowApp;
use crate::pane::PaneSurface;
use crate::settings::components::{MENU_MAX_HEIGHT, select_item_shaped, with_alpha};
use crate::ui_primitives::interface_style::with_rounded_shape;
use crate::ui_primitives::quick_pick::{
    self, FIELD_GAP, FIELD_INSET, ICON_GAP, ICON_SIZE, LABEL_SIZE, ROW_GAP, ROW_HEIGHT, ROW_RADIUS,
    ROW_SPACING, ROW_TEXT_INSET, highlighted_label,
};
use crate::ui_primitives::squircle::squircle_fill;
use crate::ui_primitives::{FilterFieldStyle, filter_field};

use crate::ui_primitives::AccessibleControlExt as _;
use catalog::{Apply, COMMANDS, Command, Kind, Needs, ScopeValue};
pub(crate) use catalog::{Scope, Section};

const PALETTE_WIDTH: f32 = 560.;
const PALETTE_PLACEHOLDER: &str = "Search commands…";
const PALETTE_LABEL: &str = "Command palette";
const PALETTE_LIST_LABEL: &str = "Commands";
const PALETTE_EMPTY: &str = "No matching command";
const CHIP_HEIGHT: f32 = 24.;
const CHIP_RADIUS: gpui::Pixels = px(6.);
const SHORTCUT_SIZE: f32 = 13.;
const PALETTE_TOP_MARGIN: f32 = 96.;
const LUA_CATEGORY_ICON: &str = "icons/palette/source-code.svg";
const LUA_ACTION_ICON: &str = "icons/palette/command-line.svg";
const CONFIG_COMMAND_ICON: &str = "icons/palette/computer-terminal-01.svg";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PaletteContext {
    terminal: bool,
    terminal_search: bool,
    conversation_fork: bool,
    conversation_resume_offer: bool,
    conversation_failure: bool,
    conversation_notice: bool,
    write_request: bool,
    markdown: bool,
    markdown_search: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trailing {
    None,
    Chevron,
    Check,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    Command(usize),
    Value(usize),
    Section(Section),
    LuaCategory(usize),
    LuaAction(usize),
    ConfigCommand(usize),
}

struct PaletteRow {
    source: Source,
    icon: Option<&'static str>,
    label: String,
    highlights: Vec<Range<usize>>,
    value: Option<String>,
    shortcut: Option<String>,
    trailing: Trailing,
}

impl PaneFlowApp {
    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompts.shown.is_some() {
            return;
        }
        if self.command_palette_open {
            self.close_command_palette(window, cx);
            return;
        }
        let context = self.capture_palette_context(window, cx);
        let restore = window.focused(cx);
        self.dismiss_transient_surfaces();
        self.command_palette_open = true;
        self.command_palette_scope = None;
        self.command_palette_section = None;
        self.command_palette_lua_category = None;
        self.command_palette_context = context;
        self.command_palette_restore_focus = restore;
        self.command_palette_selected = 0;
        self.command_palette_scroll = UniformListScrollHandle::new();
        self.reset_palette_input(cx);
        let focus = self.command_palette_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command_palette_open {
            return;
        }
        self.command_palette_open = false;
        self.command_palette_scope = None;
        self.command_palette_section = None;
        self.command_palette_lua_category = None;
        self.command_palette_selected = 0;
        self.reset_palette_input(cx);
        if let Some(handle) = self.command_palette_restore_focus.take() {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    pub(crate) fn handle_open_command_palette(
        &mut self,
        _: &crate::OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(window, cx);
    }

    fn reset_palette_input(&mut self, cx: &mut Context<Self>) {
        let placeholder = match (
            self.command_palette_scope,
            self.command_palette_section,
            self.command_palette_lua_category.as_deref(),
        ) {
            (Some(scope), _, _) => scope.placeholder(),
            (None, Some(section), _) => section.placeholder(),
            (None, None, Some(category)) => format!("Search {}…", category.to_lowercase()),
            (None, None, None) => PALETTE_PLACEHOLDER.to_string(),
        };
        self.command_palette_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder(placeholder, cx);
        });
        self.command_palette_query_seen.clear();
    }

    fn palette_query(&self, cx: &App) -> String {
        self.command_palette_input.read(cx).value()
    }

    fn capture_palette_context(&self, window: &Window, cx: &App) -> PaletteContext {
        let Some(pane) = self
            .active_workspace()
            .and_then(|workspace| workspace.active_tab().root.as_ref())
            .and_then(|root| root.focused_pane(window, cx))
        else {
            return PaletteContext::default();
        };
        let pane = pane.read(cx);
        match pane.surfaces().get(pane.active_surface_idx()) {
            Some(PaneSurface::Terminal(view)) => {
                let view = view.read(cx);
                let banner = view.conversation_banner();
                PaletteContext {
                    terminal: true,
                    terminal_search: view.search_active(),
                    conversation_fork: view.can_fork_conversation(),
                    conversation_resume_offer: matches!(
                        banner,
                        Some(crate::terminal::view::conversation::ConversationBanner::OfferResume)
                    ),
                    conversation_failure: matches!(
                        banner,
                        Some(
                            crate::terminal::view::conversation::ConversationBanner::ResumeFailed { .. }
                        )
                    ),
                    conversation_notice: banner.is_some(),
                    write_request: view.write_request().is_some(),
                    ..PaletteContext::default()
                }
            }
            Some(PaneSurface::Markdown(view)) => PaletteContext {
                markdown: true,
                markdown_search: view.read(cx).search_active(),
                ..PaletteContext::default()
            },
            None => PaletteContext::default(),
        }
    }

    fn section_is_available(&self, section: Section) -> bool {
        section
            .commands()
            .any(|(_, command)| self.command_is_available(command))
    }

    fn command_is_available(&self, command: &Command) -> bool {
        match command.needs {
            Needs::Always => true,
            Needs::Terminal => self.command_palette_context.terminal,
            Needs::TerminalSearch => self.command_palette_context.terminal_search,
            Needs::ForkableConversation => self.command_palette_context.conversation_fork,
            Needs::ConversationResumeOffer => {
                self.command_palette_context.conversation_resume_offer
            }
            Needs::ConversationFailure => self.command_palette_context.conversation_failure,
            Needs::ConversationNotice => self.command_palette_context.conversation_notice,
            Needs::WriteRequest => self.command_palette_context.write_request,
            Needs::Markdown => self.command_palette_context.markdown,
            Needs::MarkdownSearch => self.command_palette_context.markdown_search,
            Needs::Workspace => self.active_workspace().is_some(),
            Needs::GitRepo => self
                .active_workspace()
                .is_some_and(|workspace| workspace.is_git_repo),
            Needs::ManyWorkspaces => self.workspaces.len() > 1,
            Needs::Spaces => !self.cached_config.spaces.is_empty(),
            Needs::ManyTabs => self
                .active_workspace()
                .is_some_and(|workspace| workspace.tab_count() > 1),
        }
    }

    fn command_palette_rows(&self, cx: &App) -> Vec<PaletteRow> {
        let query = self.palette_query(cx);
        let query = query.trim();
        let mut ranked: Vec<(matcher::Rank, PaletteRow)> = Vec::new();

        match self.command_palette_scope {
            Some(scope) => {
                for (idx, value) in scope.values(self).into_iter().enumerate() {
                    let Some(matched) = matcher::match_entry(&value.label, None, "", query) else {
                        continue;
                    };
                    ranked.push((
                        matched.rank,
                        PaletteRow {
                            source: Source::Value(idx),
                            icon: None,
                            label: value.label,
                            highlights: matched.highlights,
                            value: None,
                            shortcut: None,
                            trailing: if value.current {
                                Trailing::Check
                            } else {
                                Trailing::None
                            },
                        },
                    ));
                }
            }
            None if self.command_palette_lua_category.is_some() => {
                self.push_lua_action_rows(
                    self.command_palette_lua_category.as_deref(),
                    query,
                    &mut ranked,
                );
            }
            None => {
                let section = self.command_palette_section;
                let browsing = section.is_none() && query.is_empty();
                for (idx, command) in COMMANDS.iter().enumerate() {
                    let listed = match section {
                        Some(section) => command.section == section,
                        None => !browsing || command.needs.is_contextual(),
                    };
                    if !listed || !self.command_is_available(command) {
                        continue;
                    }
                    let value = match command.kind {
                        Kind::Scope(scope) => scope.current(self),
                        _ => None,
                    };
                    let Some(matched) = matcher::match_entry(
                        command.label,
                        value.as_deref(),
                        command.keywords,
                        query,
                    ) else {
                        continue;
                    };
                    let trailing = match command.kind {
                        Kind::Scope(_) => Trailing::Chevron,
                        Kind::Toggle { read, .. } if read(self) => Trailing::Check,
                        _ => Trailing::None,
                    };
                    ranked.push((
                        matched.rank,
                        PaletteRow {
                            source: Source::Command(idx),
                            icon: command.icon,
                            label: command.label.to_string(),
                            highlights: matched.highlights,
                            value,
                            shortcut: match command.kind {
                                Kind::Action(name) => {
                                    self.shortcut_for_action(name).map(str::to_string)
                                }
                                _ => None,
                            },
                            trailing,
                        },
                    ));
                }
                if section.is_none() && !browsing {
                    self.push_lua_action_rows(None, query, &mut ranked);
                    self.push_config_command_rows(query, &mut ranked);
                }
                if browsing {
                    for (idx, category) in self.lua_config.categories().into_iter().enumerate() {
                        ranked.push((
                            matcher::Rank::LabelWordStart,
                            PaletteRow {
                                source: Source::LuaCategory(idx),
                                icon: Some(LUA_CATEGORY_ICON),
                                label: category,
                                highlights: Vec::new(),
                                value: None,
                                shortcut: None,
                                trailing: Trailing::Chevron,
                            },
                        ));
                    }
                    for section in Section::ALL {
                        if !self.section_is_available(section) {
                            continue;
                        }
                        ranked.push((
                            matcher::Rank::LabelWordStart,
                            PaletteRow {
                                source: Source::Section(section),
                                icon: Some(section.icon()),
                                label: section.title().to_string(),
                                highlights: Vec::new(),
                                value: None,
                                shortcut: None,
                                trailing: Trailing::Chevron,
                            },
                        ));
                    }
                }
            }
        }

        if !query.is_empty()
            && let Some(best) = ranked
                .iter()
                .enumerate()
                .min_by_key(|(position, (rank, _))| (*rank, *position))
                .map(|(position, _)| position)
        {
            let promoted = ranked.remove(best);
            ranked.insert(0, promoted);
        }

        ranked.into_iter().map(|(_, row)| row).collect()
    }

    fn push_lua_action_rows(
        &self,
        category: Option<&str>,
        query: &str,
        ranked: &mut Vec<(matcher::Rank, PaletteRow)>,
    ) {
        for (idx, action) in self.lua_config.actions.iter().enumerate() {
            if category.is_some_and(|category| category != action.category) {
                continue;
            }
            let keywords = std::iter::once(action.name.as_str())
                .chain(action.description.as_deref())
                .chain(action.keywords.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            let value = category.is_none().then(|| action.category.clone());
            let Some(matched) =
                matcher::match_entry(&action.title, value.as_deref(), &keywords, query)
            else {
                continue;
            };
            ranked.push((
                matched.rank,
                PaletteRow {
                    source: Source::LuaAction(idx),
                    icon: Some(LUA_ACTION_ICON),
                    label: action.title.clone(),
                    highlights: matched.highlights,
                    value,
                    shortcut: None,
                    trailing: Trailing::None,
                },
            ));
        }
    }

    fn push_config_command_rows(&self, query: &str, ranked: &mut Vec<(matcher::Rank, PaletteRow)>) {
        if self.shown_workspace_idx().is_none() {
            return;
        }
        for (idx, command) in self.cached_config.commands.iter().enumerate() {
            let Some(shell) = command.shell_command() else {
                continue;
            };
            let value = command
                .description
                .as_deref()
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(str::to_string);
            let keywords = command
                .keywords
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(shell))
                .collect::<Vec<_>>()
                .join(" ");
            let Some(matched) =
                matcher::match_entry(&command.name, value.as_deref(), &keywords, query)
            else {
                continue;
            };
            ranked.push((
                matched.rank,
                PaletteRow {
                    source: Source::ConfigCommand(idx),
                    icon: Some(CONFIG_COMMAND_ICON),
                    label: command.name.clone(),
                    highlights: matched.highlights,
                    value,
                    shortcut: None,
                    trailing: Trailing::None,
                },
            ));
        }
    }

    fn run_config_palette_command(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((name, shell)) = self.cached_config.commands.get(idx).and_then(|command| {
            command
                .shell_command()
                .map(|shell| (command.name.clone(), shell.to_string()))
        }) else {
            return;
        };
        self.close_command_palette(window, cx);
        let Some(ws_idx) = self.shown_workspace_idx() else {
            return;
        };
        self.open_tab_with_surface(
            ws_idx,
            name,
            paneflow_config::schema::TerminalSurfaceProfile::Normal,
            crate::app::workspace_ops::SurfaceLaunch {
                command: Some(shell),
                env: None,
                cwd: None,
            },
            window,
            cx,
        );
    }

    fn enter_lua_category(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(category) = self.lua_config.categories().into_iter().nth(idx) else {
            return;
        };
        self.command_palette_lua_category = Some(category);
        self.command_palette_section = None;
        self.command_palette_scope = None;
        self.reset_palette_input(cx);
        self.command_palette_select(0, cx);
    }

    fn leave_lua_category(&mut self, cx: &mut Context<Self>) {
        let Some(category) = self.command_palette_lua_category.take() else {
            return;
        };
        self.reset_palette_input(cx);
        let position = self
            .lua_config
            .categories()
            .iter()
            .position(|known| *known == category);
        match position {
            Some(idx) => self.select_palette_row(Source::LuaCategory(idx), cx),
            None => self.command_palette_select(0, cx),
        }
    }

    fn run_lua_palette_action(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.lua_config.actions.get(idx) else {
            return;
        };
        let name = action.name.clone();
        self.close_command_palette(window, cx);
        self.invoke_lua_action(name, None, "palette", window, cx);
    }

    fn command_palette_select(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.command_palette_selected = idx;
        self.command_palette_scroll
            .scroll_to_item(idx, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn preview_palette_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.command_palette_scope else {
            return;
        };
        let Some(Source::Value(source)) = self
            .command_palette_rows(cx)
            .get(self.command_palette_selected)
            .map(|row| row.source)
        else {
            return;
        };
        let Some(value) = scope.values(self).into_iter().nth(source) else {
            return;
        };
        if value.current || !value.apply.applies_in_place() {
            return;
        }
        self.apply_scope_value(value, window, cx);
    }

    pub(crate) fn open_move_workspace_to_space_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cached_config.spaces.is_empty() || self.shown_workspace_idx().is_none() {
            return;
        }
        if !self.command_palette_open {
            self.open_command_palette(window, cx);
        }
        if !self.command_palette_open {
            return;
        }
        self.command_palette_section = None;
        self.enter_palette_scope(Scope::MoveToSpace, cx);
    }

    fn enter_palette_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.command_palette_section = Some(section);
        self.command_palette_scope = None;
        self.reset_palette_input(cx);
        self.command_palette_select(0, cx);
    }

    fn leave_palette_section(&mut self, cx: &mut Context<Self>) {
        let Some(section) = self.command_palette_section.take() else {
            return;
        };
        self.reset_palette_input(cx);
        self.select_palette_row(Source::Section(section), cx);
    }

    fn select_palette_row(&mut self, source: Source, cx: &mut Context<Self>) {
        let selected = self
            .command_palette_rows(cx)
            .iter()
            .position(|row| row.source == source)
            .unwrap_or(0);
        self.command_palette_select(selected, cx);
    }

    fn leave_palette_level(&mut self, cx: &mut Context<Self>) -> bool {
        if self.command_palette_scope.is_some() {
            self.leave_palette_scope(cx);
            true
        } else if self.command_palette_section.is_some() {
            self.leave_palette_section(cx);
            true
        } else if self.command_palette_lua_category.is_some() {
            self.leave_lua_category(cx);
            true
        } else {
            false
        }
    }

    fn enter_palette_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.command_palette_scope = Some(scope);
        self.reset_palette_input(cx);
        let selected = scope
            .values(self)
            .iter()
            .position(|value| value.current)
            .unwrap_or(0);
        self.command_palette_select(selected, cx);
    }

    fn leave_palette_scope(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.command_palette_scope.take() else {
            return;
        };
        self.reset_palette_input(cx);
        let owner = COMMANDS
            .iter()
            .position(|command| matches!(command.kind, Kind::Scope(other) if other == scope));
        match owner {
            Some(owner) => self.select_palette_row(Source::Command(owner), cx),
            None => self.command_palette_select(0, cx),
        }
    }

    fn apply_scope_value(
        &mut self,
        value: ScopeValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match value.apply {
            Apply::Setting { key, nested, value } => self.persist_setting(nested, key, value, cx),
            Apply::Theme(idx) => {
                if let Some(preset) = crate::theme::PRESETS.get(idx) {
                    self.apply_theme_preset(preset, window, cx);
                }
            }
            Apply::Mode(mode) => {
                self.theme_mode = mode;
                let preset = self.current_theme_preset();
                self.apply_theme_preset(preset, window, cx);
            }
            Apply::BackgroundStyle(style) => self.apply_background_style(style, cx),
            Apply::Workspace(idx) => self.select_workspace(idx, window, cx),
            Apply::Space(space) => {
                self.activate_space(space, Some(window), cx);
            }
            Apply::MoveToSpace(space) => {
                if let Some(idx) = self.shown_workspace_idx() {
                    self.move_workspace_to_space(idx, space, window, cx);
                }
            }
            Apply::MoveToNewSpace => {
                if let Some(idx) = self.shown_workspace_idx() {
                    self.move_workspace_to_new_space(idx, window, cx);
                }
            }
            Apply::Tab(idx) => {
                if let Some(workspace) = self.shown_workspace_idx() {
                    self.select_workspace_tab(workspace, idx, window, cx);
                }
            }
            Apply::Action(name) => {
                if let Some(action) = crate::keybindings::action_for_name(name) {
                    window.dispatch_action(action, cx);
                }
            }
            Apply::Settings(section) => self.open_settings_at(section, window, cx),
        }
    }

    fn command_palette_run(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.command_palette_rows(cx).get(idx).map(|row| row.source) else {
            return;
        };
        self.command_palette_select(idx, cx);

        let command = match (source, self.command_palette_scope) {
            (Source::Value(value), Some(scope)) => {
                let Some(value) = scope.values(self).into_iter().nth(value) else {
                    return;
                };
                if value.apply.applies_in_place() {
                    self.apply_scope_value(value, window, cx);
                    return;
                }
                self.close_command_palette(window, cx);
                self.apply_scope_value(value, window, cx);
                return;
            }
            (Source::Section(section), _) => {
                self.enter_palette_section(section, cx);
                return;
            }
            (Source::LuaCategory(idx), _) => {
                self.enter_lua_category(idx, cx);
                return;
            }
            (Source::LuaAction(idx), _) => {
                self.run_lua_palette_action(idx, window, cx);
                return;
            }
            (Source::ConfigCommand(idx), _) => {
                self.run_config_palette_command(idx, window, cx);
                return;
            }
            (Source::Command(command), _) => command,
            (Source::Value(_), None) => return,
        };

        let Some(command) = COMMANDS.get(command) else {
            return;
        };
        match command.kind {
            Kind::Scope(scope) => self.enter_palette_scope(scope, cx),
            Kind::Action(name) => {
                let action = crate::keybindings::action_for_name(name);
                self.close_command_palette(window, cx);
                if let Some(action) = action {
                    window.dispatch_action(action, cx);
                }
            }
            Kind::Run(run) => {
                self.close_command_palette(window, cx);
                run(self, window, cx);
            }
            Kind::Toggle { read, write } => {
                let next = !read(self);
                write(self, next, cx);
            }
        }
    }

    fn handle_command_palette_capture_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key != "backspace" || !self.palette_query(cx).is_empty() {
            return;
        }
        if self.leave_palette_level(cx) {
            cx.stop_propagation();
        }
    }

    pub(crate) fn handle_command_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.command_palette_rows(cx).len();
        let selected = self.command_palette_selected;
        match event.keystroke.key.as_str() {
            "escape" => {
                if !self.leave_palette_level(cx) {
                    self.close_command_palette(window, cx);
                }
                cx.stop_propagation();
            }
            "enter" => {
                if selected < len {
                    self.command_palette_run(selected, window, cx);
                }
                cx.stop_propagation();
            }
            "up" => {
                if selected > 0 {
                    self.command_palette_select(selected - 1, cx);
                    self.preview_palette_value(window, cx);
                }
                cx.stop_propagation();
            }
            "down" => {
                if selected + 1 < len {
                    self.command_palette_select(selected + 1, cx);
                    self.preview_palette_value(window, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn render_palette_field(
        &self,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.command_palette_input.read(cx).focus_handle.clone();
        let focused = focus.is_focused(window);
        let has_query = !self.palette_query(cx).is_empty();
        let style = FilterFieldStyle::palette();
        let chip_inset = (style.height - px(CHIP_HEIGHT)) / 2.;
        let chip = match (self.command_palette_scope, self.command_palette_section) {
            (Some(scope), _) => Some(scope.title()),
            (None, Some(section)) => Some(section.title()),
            (None, None) => None,
        };
        let prefix = chip.map(|title| {
            div()
                .relative()
                .flex_none()
                .h(px(CHIP_HEIGHT))
                .px(px(8.))
                .flex()
                .items_center()
                .child(squircle_fill(CHIP_RADIUS, with_alpha(ui.text, 0.18)))
                .child(
                    div()
                        .relative()
                        .text_size(px(LABEL_SIZE))
                        .text_color(ui.text)
                        .child(SharedString::from(title)),
                )
                .into_any_element()
        });
        let prefix_shown = prefix.is_some();

        filter_field(
            "command-palette-field",
            "command-palette-field-clear",
            "Search the command palette",
            ui,
            style,
            focused,
            has_query,
            true,
            prefix,
            self.command_palette_input.clone(),
            cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.command_palette_input
                    .update(cx, |input, cx| input.clear(cx));
                let focus = this.command_palette_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }),
        )
        .when(prefix_shown, |field| field.pl(chip_inset))
        .flex_none()
        .mx(px(FIELD_INSET))
        .mb(px(FIELD_GAP))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .into_any_element()
    }

    fn render_palette_row(
        &self,
        idx: usize,
        row: &PaletteRow,
        count: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        palette_option(
            idx,
            row,
            count,
            idx == self.command_palette_selected,
            ui,
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.command_palette_run(idx, window, cx);
                cx.stop_propagation();
            }),
        )
        .into_any_element()
    }

    fn command_palette_list_label(&self) -> SharedString {
        match (
            self.command_palette_scope,
            self.command_palette_section,
            self.command_palette_lua_category.as_deref(),
        ) {
            (Some(scope), _, _) => scope.title().into(),
            (None, Some(section), _) => section.title().into(),
            (None, None, Some(category)) => SharedString::from(category.to_string()),
            (None, None, None) => PALETTE_LIST_LABEL.into(),
        }
    }

    pub(crate) fn render_command_palette(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        with_rounded_shape(|| self.build_command_palette(window, cx))
    }

    fn build_command_palette(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let rows = Rc::new(self.command_palette_rows(cx));

        let list = if rows.is_empty() {
            quick_pick::empty_notice("command-palette-empty", PALETTE_EMPTY, ui).into_any_element()
        } else {
            let items = rows.clone();
            let count = rows.len();
            quick_pick::list_box("command-palette-options", self.command_palette_list_label())
                .child(
                    uniform_list(
                        "command-palette-list",
                        count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            range
                                .filter_map(|idx| {
                                    let row = items.get(idx)?;
                                    Some(
                                        div().w_full().flex().flex_col().pb(px(ROW_SPACING)).child(
                                            with_rounded_shape(|| {
                                                this.render_palette_row(idx, row, count, ui, cx)
                                            }),
                                        ),
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .w_full()
                    .h(list_height(count))
                    .track_scroll(&self.command_palette_scroll),
                )
                .into_any_element()
        };

        let panel = div()
            .id("command-palette")
            .role(Role::Group)
            .aria_label(PALETTE_LABEL)
            .occlude()
            .capture_key_down(cx.listener(Self::handle_command_palette_capture_key_down))
            .on_key_down(cx.listener(Self::handle_command_palette_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_command_palette(window, cx);
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let panel = quick_pick::shell(
            panel,
            px(PALETTE_WIDTH),
            ui,
            self.render_palette_field(ui, window, cx),
            list,
        );

        deferred(
            div()
                .id("command-palette-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(PALETTE_TOP_MARGIN))
                .child(crate::ui_primitives::menu_reveal(
                    "command-palette-reveal",
                    panel,
                )),
        )
        .with_priority(7)
        .into_any_element()
    }
}

fn list_height(rows: usize) -> Pixels {
    px(rows as f32 * (ROW_HEIGHT + ROW_SPACING)).min(MENU_MAX_HEIGHT)
}

impl PaletteRow {
    fn checked(&self) -> Option<bool> {
        let checkable = match self.source {
            Source::Value(_) => true,
            Source::Command(idx) => COMMANDS
                .get(idx)
                .is_some_and(|command| matches!(command.kind, Kind::Toggle { .. })),
            Source::Section(_)
            | Source::LuaCategory(_)
            | Source::LuaAction(_)
            | Source::ConfigCommand(_) => false,
        };
        checkable.then_some(self.trailing == Trailing::Check)
    }

    fn pick_option(&self, idx: usize, count: usize, selected: bool) -> quick_pick::PickOption {
        quick_pick::PickOption::new(self.label.clone(), idx, count)
            .selected(selected)
            .description(self.value.clone())
            .shortcut(self.shortcut.clone())
            .checked(self.checked())
    }
}

fn palette_option(
    idx: usize,
    row: &PaletteRow,
    count: usize,
    selected: bool,
    ui: crate::theme::UiColors,
    on_press: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let colors = quick_pick::row_colors(selected, ui);
    let label_color = colors.label;
    let muted_color = colors.muted;

    let mut label = div()
        .relative()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(ICON_GAP));
    if let Some(icon) = row.icon {
        label = label.child(
            svg()
                .flex_none()
                .size(px(ICON_SIZE))
                .path(icon)
                .text_color(with_alpha(label_color, 0.8)),
        );
    }
    let label = label.child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(label_color)
            .child(highlighted_label(&row.label, &row.highlights)),
    );

    let element = select_item_shaped(
        SharedString::from(format!("command-palette-row-{idx}")),
        selected,
        ui,
        colors.fill,
        ROW_RADIUS,
    )
    .h(px(ROW_HEIGHT))
    .px(px(ROW_TEXT_INSET))
    .gap(px(ROW_GAP))
    .text_size(px(LABEL_SIZE))
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_activate(on_press)
    .child(label);
    let mut element = quick_pick::expose_option(element, row.pick_option(idx, count, selected));

    if let Some(value) = &row.value {
        element = element.child(
            div()
                .relative()
                .flex_none()
                .max_w(px(200.))
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(muted_color)
                .child(SharedString::from(value.clone())),
        );
    }

    if let Some(shortcut) = &row.shortcut {
        element = element.child(
            div()
                .relative()
                .flex_none()
                .text_size(px(SHORTCUT_SIZE))
                .text_color(muted_color)
                .child(SharedString::from(shortcut.clone())),
        );
    }

    match row.trailing {
        Trailing::Chevron => element.child(
            svg()
                .relative()
                .flex_none()
                .size(px(12.))
                .path("icons/chevron-right.svg")
                .text_color(muted_color),
        ),
        Trailing::Check => element.child(
            svg()
                .relative()
                .flex_none()
                .size(px(13.))
                .path("icons/check.svg")
                .text_color(label_color),
        ),
        Trailing::None => element,
    }
}

#[cfg(test)]
impl PaneFlowApp {
    pub(crate) fn command_palette_entries(&self, cx: &App) -> Vec<(String, Option<String>)> {
        self.command_palette_rows(cx)
            .into_iter()
            .map(|row| (row.label, row.value))
            .collect()
    }

    pub(crate) fn command_palette_filter(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.to_string();
        self.command_palette_input
            .update(cx, |input, cx| input.set_value(query, cx));
    }

    pub(crate) fn command_palette_choose(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let idx = self
            .command_palette_rows(cx)
            .iter()
            .position(|row| row.label == label)
            .unwrap_or_else(|| panic!("no palette row labelled {label}"));
        self.command_palette_run(idx, window, cx);
    }

    pub(crate) fn command_palette_is_open(&self) -> bool {
        self.command_palette_open
    }

    pub(crate) fn command_palette_exposed_options(&self, cx: &App) -> Vec<gpui::accesskit::Node> {
        let rows = self.command_palette_rows(cx);
        let count = rows.len();
        rows.iter()
            .enumerate()
            .map(|(idx, row)| {
                exposed(&palette_option(
                    idx,
                    row,
                    count,
                    idx == self.command_palette_selected,
                    crate::theme::ui_colors(),
                    |_, _, _| {},
                ))
            })
            .collect()
    }
}

#[cfg(test)]
fn exposed<E: gpui::Element>(element: &E) -> gpui::accesskit::Node {
    let role = element
        .a11y_role()
        .expect("the element exposes an accessibility role");
    let mut node = gpui::accesskit::Node::new(role);
    element.write_a11y_info(&mut node);
    node
}

#[cfg(test)]
mod tests {
    use super::{
        COMMANDS, Kind, PALETTE_EMPTY, PALETTE_LABEL, PaletteRow, Source, Trailing, exposed,
        palette_option,
    };
    use crate::ui_primitives::quick_pick;
    use gpui::accesskit::{Action, Role, Toggled};

    fn row(source: Source, label: &str, trailing: Trailing) -> PaletteRow {
        PaletteRow {
            source,
            icon: None,
            label: label.to_string(),
            highlights: Vec::new(),
            value: None,
            shortcut: None,
            trailing,
        }
    }

    fn toggle_command() -> usize {
        COMMANDS
            .iter()
            .position(|command| matches!(command.kind, Kind::Toggle { .. }))
            .expect("the catalog holds a toggle")
    }

    fn scope_command() -> usize {
        COMMANDS
            .iter()
            .position(|command| matches!(command.kind, Kind::Scope(_)))
            .expect("the catalog holds a scope")
    }

    fn action_command() -> usize {
        COMMANDS
            .iter()
            .position(|command| matches!(command.kind, Kind::Action(_)))
            .expect("the catalog holds an action")
    }

    fn options(rows: &[PaletteRow], selected: usize) -> Vec<gpui::accesskit::Node> {
        rows.iter()
            .enumerate()
            .map(|(idx, row)| {
                exposed(&palette_option(
                    idx,
                    row,
                    rows.len(),
                    idx == selected,
                    crate::theme::ui_colors(),
                    |_, _, _| {},
                ))
            })
            .collect()
    }

    #[test]
    fn every_palette_row_is_an_option_named_numbered_and_pressable() {
        let mut shortcut = row(
            Source::Command(action_command()),
            "Split right",
            Trailing::None,
        );
        shortcut.shortcut = Some("⌘D".to_string());
        let mut scope = row(Source::Command(scope_command()), "Theme", Trailing::Chevron);
        scope.value = Some("Paneflow Dark".to_string());
        let rows = vec![
            shortcut,
            scope,
            row(
                Source::Command(toggle_command()),
                "Word wrap",
                Trailing::Check,
            ),
            row(
                Source::Command(toggle_command()),
                "Line numbers",
                Trailing::None,
            ),
            row(Source::Value(0), "Light", Trailing::None),
            row(Source::Value(1), "Dark", Trailing::Check),
            row(Source::LuaAction(0), "Deploy", Trailing::None),
        ];
        let nodes = options(&rows, 1);
        for (idx, (row, node)) in rows.iter().zip(&nodes).enumerate() {
            let label = row.label.as_str();
            assert_eq!(node.role(), Role::ListBoxOption, "{label}");
            assert_eq!(node.label(), Some(label));
            assert_eq!(node.is_selected(), Some(idx == 1), "{label}");
            assert_eq!(node.position_in_set(), Some(idx + 1), "{label}");
            assert_eq!(node.size_of_set(), Some(rows.len()), "{label}");
            assert!(node.supports_action(Action::Click), "{label}");
        }
        assert_eq!(nodes[0].keyboard_shortcut(), Some("⌘D"));
        assert_eq!(nodes[1].description(), Some("Paneflow Dark"));
        let checked: Vec<Option<Toggled>> = nodes.iter().map(|node| node.toggled()).collect();
        assert_eq!(
            checked,
            [
                None,
                None,
                Some(Toggled::True),
                Some(Toggled::False),
                Some(Toggled::False),
                Some(Toggled::True),
                None,
            ]
        );
    }

    #[test]
    fn a_filtered_palette_numbers_its_options_among_the_rows_it_keeps() {
        let kept = vec![
            row(
                Source::Command(action_command()),
                "Close pane",
                Trailing::None,
            ),
            row(Source::LuaAction(2), "Close all", Trailing::None),
        ];
        let nodes = options(&kept, 0);
        let exposed: Vec<_> = nodes
            .iter()
            .map(|node| {
                (
                    node.label(),
                    node.position_in_set(),
                    node.size_of_set(),
                    node.is_selected(),
                )
            })
            .collect();
        assert_eq!(
            exposed,
            [
                (Some("Close pane"), Some(1), Some(2), Some(true)),
                (Some("Close all"), Some(2), Some(2), Some(false)),
            ]
        );
    }

    #[test]
    fn the_palette_lists_its_options_in_a_named_list_box_and_says_when_nothing_matches() {
        let list = exposed(&quick_pick::list_box("command-palette-options", "Commands"));
        assert_eq!(list.role(), Role::ListBox);
        assert_eq!(list.label(), Some("Commands"));
        let empty = exposed(&quick_pick::empty_notice(
            "command-palette-empty",
            PALETTE_EMPTY,
            crate::theme::ui_colors(),
        ));
        assert_eq!(empty.role(), Role::Label);
        assert_eq!(empty.label(), Some(PALETTE_EMPTY));
        assert!(!PALETTE_LABEL.is_empty());
    }
}

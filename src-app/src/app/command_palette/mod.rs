mod catalog;
mod matcher;

use std::ops::Range;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Window, deferred, div, px, svg,
};

use crate::PaneFlowApp;
use crate::pane::PaneSurface;
use crate::settings::components::{
    MENU_MAX_HEIGHT, MENU_PADDING, MENU_ROW_GAP, MENU_ROW_HEIGHT, menu_panel, select_item_tinted,
    with_alpha,
};
use crate::ui_primitives::squircle::squircle_fill;
use crate::ui_primitives::{FilterFieldStyle, filter_field};

pub(crate) use catalog::Scope;
use catalog::{Apply, COMMANDS, Command, Kind, Needs, ScopeValue};

const PALETTE_MIN_WIDTH: f32 = 420.;
const PALETTE_MAX_WIDTH: f32 = 640.;
const PALETTE_PLACEHOLDER: &str = "Search commands…";
const FIELD_GAP_BELOW: f32 = 6.;
const CHIP_HEIGHT: f32 = 24.;
const CHIP_RADIUS: gpui::Pixels = px(6.);
const ROW_TEXT_INSET: f32 = 8.;
const LABEL_SIZE: f32 = 12.;
const SHORTCUT_SIZE: f32 = 11.;
const PALETTE_TOP_MARGIN: f32 = 96.;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PaletteContext {
    terminal: bool,
    terminal_search: bool,
    markdown: bool,
    markdown_search: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trailing {
    None,
    Chevron,
    Check,
}

struct PaletteRow {
    source: usize,
    label: String,
    highlights: Vec<Range<usize>>,
    value: Option<String>,
    shortcut: Option<String>,
    trailing: Trailing,
}

fn text_width(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * 0.55
}

impl PaneFlowApp {
    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette_open {
            self.close_command_palette(window, cx);
            return;
        }
        let context = self.capture_palette_context(window, cx);
        let restore = window.focused(cx);
        self.dismiss_transient_surfaces();
        self.command_palette_open = true;
        self.command_palette_scope = None;
        self.command_palette_context = context;
        self.command_palette_restore_focus = restore;
        self.command_palette_selected = 0;
        self.command_palette_scroll = gpui::ScrollHandle::new();
        self.reset_palette_input(None, cx);
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
        self.command_palette_selected = 0;
        self.reset_palette_input(None, cx);
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

    fn reset_palette_input(&mut self, scope: Option<Scope>, cx: &mut Context<Self>) {
        let placeholder = match scope {
            Some(scope) => scope.placeholder(),
            None => PALETTE_PLACEHOLDER.to_string(),
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
            Some(PaneSurface::Terminal(view)) => PaletteContext {
                terminal: true,
                terminal_search: view.read(cx).search_active(),
                ..PaletteContext::default()
            },
            Some(PaneSurface::Markdown(view)) => PaletteContext {
                markdown: true,
                markdown_search: view.read(cx).search_active(),
                ..PaletteContext::default()
            },
            None => PaletteContext::default(),
        }
    }

    fn command_is_available(&self, command: &Command) -> bool {
        match command.needs {
            Needs::Always => true,
            Needs::Terminal => self.command_palette_context.terminal,
            Needs::TerminalSearch => self.command_palette_context.terminal_search,
            Needs::Markdown => self.command_palette_context.markdown,
            Needs::MarkdownSearch => self.command_palette_context.markdown_search,
            Needs::Workspace => self.active_workspace().is_some(),
            Needs::GitRepo => self
                .active_workspace()
                .is_some_and(|workspace| workspace.is_git_repo),
            Needs::ManyWorkspaces => self.workspaces.len() > 1,
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
                            source: idx,
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
            None => {
                for (idx, command) in COMMANDS.iter().enumerate() {
                    if !self.command_is_available(command) {
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
                            source: idx,
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

    fn command_palette_select(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.command_palette_selected = idx;
        self.command_palette_scroll.scroll_to_item(idx);
        cx.notify();
    }

    fn enter_palette_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.command_palette_scope = Some(scope);
        self.reset_palette_input(Some(scope), cx);
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
        self.reset_palette_input(None, cx);
        let owner = COMMANDS
            .iter()
            .position(|command| matches!(command.kind, Kind::Scope(other) if other == scope));
        let selected = owner
            .and_then(|owner| {
                self.command_palette_rows(cx)
                    .iter()
                    .position(|row| row.source == owner)
            })
            .unwrap_or(0);
        self.command_palette_select(selected, cx);
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
            Apply::Workspace(idx) => self.select_workspace(idx, window, cx),
            Apply::Tab(idx) => {
                let workspace = self.active_idx;
                self.select_workspace_tab(workspace, idx, window, cx);
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

        if let Some(scope) = self.command_palette_scope {
            let Some(value) = scope.values(self).into_iter().nth(source) else {
                return;
            };
            self.close_command_palette(window, cx);
            self.apply_scope_value(value, window, cx);
            return;
        }

        let Some(command) = COMMANDS.get(source) else {
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
                self.close_command_palette(window, cx);
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
        if event.keystroke.key != "backspace"
            || self.command_palette_scope.is_none()
            || !self.palette_query(cx).is_empty()
        {
            return;
        }
        self.leave_palette_scope(cx);
        cx.stop_propagation();
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
                if self.command_palette_scope.is_some() {
                    self.leave_palette_scope(cx);
                } else {
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
                }
                cx.stop_propagation();
            }
            "down" => {
                if selected + 1 < len {
                    self.command_palette_select(selected + 1, cx);
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
        let prefix = self.command_palette_scope.map(|scope| {
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
                        .child(SharedString::from(scope.title())),
                )
                .into_any_element()
        });

        filter_field(
            "command-palette-field",
            "command-palette-field-clear",
            ui,
            FilterFieldStyle::palette(),
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
        .w_full()
        .flex_none()
        .mb(px(FIELD_GAP_BELOW))
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
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = idx == self.command_palette_selected;
        let label_color = if selected {
            crate::theme::on_selection_color()
        } else {
            ui.text
        };
        let muted_color = if selected {
            crate::theme::on_selection_muted()
        } else {
            ui.muted
        };

        let mut element = select_item_tinted(
            SharedString::from(format!("command-palette-row-{idx}")),
            selected,
            ui,
            crate::theme::selection_color(),
        )
        .h(MENU_ROW_HEIGHT)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.command_palette_run(idx, window, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_color(label_color)
                .child(highlighted_label(&row.label, &row.highlights)),
        );

        if let Some(value) = &row.value {
            element = element.child(
                div()
                    .relative()
                    .flex_none()
                    .max_w(px(200.))
                    .overflow_x_hidden()
                    .whitespace_nowrap()
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
            Trailing::Chevron => {
                element = element.child(
                    svg()
                        .relative()
                        .flex_none()
                        .size(px(12.))
                        .path("icons/chevron-right.svg")
                        .text_color(muted_color),
                );
            }
            Trailing::Check => {
                element = element.child(
                    svg()
                        .relative()
                        .flex_none()
                        .size(px(13.))
                        .path("icons/check.svg")
                        .text_color(label_color),
                );
            }
            Trailing::None => {}
        }

        element.into_any_element()
    }

    pub(crate) fn render_command_palette(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let rows = self.command_palette_rows(cx);
        let width = palette_width(&rows);

        let mut list = div()
            .id("command-palette-list")
            .flex()
            .flex_col()
            .gap(MENU_ROW_GAP)
            .max_h(MENU_MAX_HEIGHT)
            .overflow_y_scroll()
            .track_scroll(&self.command_palette_scroll);

        if rows.is_empty() {
            list = list.child(
                div()
                    .h(MENU_ROW_HEIGHT)
                    .px(px(ROW_TEXT_INSET))
                    .flex()
                    .items_center()
                    .text_size(px(LABEL_SIZE))
                    .text_color(ui.muted)
                    .child("No matching command"),
            );
        } else {
            for (idx, row) in rows.iter().enumerate() {
                list = list.child(self.render_palette_row(idx, row, ui, cx));
            }
        }

        let panel = menu_panel(div().id("command-palette"), ui)
            .w(px(width))
            .occlude()
            .capture_key_down(cx.listener(Self::handle_command_palette_capture_key_down))
            .on_key_down(cx.listener(Self::handle_command_palette_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_command_palette(window, cx);
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.render_palette_field(ui, window, cx))
            .child(list);

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

fn palette_width(rows: &[PaletteRow]) -> f32 {
    let mut widest: f32 = 0.;
    for row in rows {
        let mut width = text_width(&row.label, LABEL_SIZE);
        if let Some(value) = &row.value {
            width += 8. + text_width(value, LABEL_SIZE).min(200.);
        }
        if let Some(shortcut) = &row.shortcut {
            width += 8. + text_width(shortcut, SHORTCUT_SIZE);
        }
        if row.trailing != Trailing::None {
            width += 8. + 13.;
        }
        widest = widest.max(width + 2. * ROW_TEXT_INSET);
    }
    (widest + 2. * f32::from(MENU_PADDING)).clamp(PALETTE_MIN_WIDTH, PALETTE_MAX_WIDTH)
}

fn highlighted_label(label: &str, highlights: &[Range<usize>]) -> StyledText {
    StyledText::new(label.to_string()).with_highlights(highlights.iter().map(|range| {
        (
            range.clone(),
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        label: &str,
        value: Option<&str>,
        shortcut: Option<&str>,
        trailing: Trailing,
    ) -> PaletteRow {
        PaletteRow {
            source: 0,
            label: label.to_string(),
            highlights: Vec::new(),
            value: value.map(str::to_string),
            shortcut: shortcut.map(str::to_string),
            trailing,
        }
    }

    #[test]
    fn the_width_fits_the_content_between_the_clamps() {
        let narrow = palette_width(&[row("New tab", None, None, Trailing::None)]);
        assert_eq!(narrow, PALETTE_MIN_WIDTH);

        let wide = palette_width(&[row(
            "A command label long enough to push this palette well past its maximum width whatever the row metrics are",
            Some("with a current value"),
            Some("Ctrl+Shift+Alt+X"),
            Trailing::Chevron,
        )]);
        assert_eq!(wide, PALETTE_MAX_WIDTH);
    }

    #[test]
    fn the_width_grows_with_the_widest_row() {
        let base = "Maximize or restore the Changes dock of this workspace right now";
        let short = palette_width(&[row(base, None, None, Trailing::None)]);
        let long = palette_width(&[
            row(base, None, None, Trailing::None),
            row(base, Some("a current value"), None, Trailing::Chevron),
        ]);
        assert!(
            short > PALETTE_MIN_WIDTH,
            "the base row must clear the clamp"
        );
        assert!(long > short);
    }
}

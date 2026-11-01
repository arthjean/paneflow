use std::rc::Rc;

use chrono::{Days, NaiveDate};
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Role, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, accesskit::Toggled, div, prelude::FluentBuilder,
    px, relative, rgb, svg,
};
use paneflow_agent_config::usage::model::UsageLevel;

use super::activity::{
    ActivityInputs, ActivityView, LEVEL_MIX, Metric, NO_ACTIVITY, NO_TOKEN_DATA, Register, WEEKS,
    activity_view, step_day,
};
use super::model::{AccountBlock, BlockBody, UsageInputs, UsageView, WindowRow, usage_view};
use super::render::{focus_ring, pace_line, runtime_logo, tabular_numerals};
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, menu_row, select_menu_surface, setting_card, with_alpha,
};
use crate::settings::search::{self, Block};
use crate::theme::UiColors;
use crate::ui_primitives::ring::{RingSize, concentric_rings, level_color, ring};
use crate::ui_primitives::{TooltipDelayExt, lerp_color, section_eyebrow, text_tooltip};
use crate::{PaneFlowApp, SettingsSection};

pub(crate) const INTRO: &str = "Plan limits come from Claude Code and Codex while they run in Paneflow. Activity is counted by Paneflow on this computer.";
pub(crate) const NO_ACCOUNT: &str =
    "No Claude Code or Codex account found. Install an agent in Settings › Agents.";
const OPEN_AGENTS_LABEL: &str = "Open agent settings";
pub(crate) const IMPORT_LABEL: &str = "Import past activity";
pub(crate) const TRANSCRIPT_NOTE: &str = "Claude Code keeps transcripts for 30 days by default.";
const SCALE_NOTE: &str = "One scale for all agents";
const TILE_GRID_MIN_WIDTH: f32 = 560.;
const TILE_GAP: f32 = 10.;
const TILE_LOGO: f32 = 22.;
const SWATCH: f32 = 8.;
const SMALL_TEXT: f32 = 11.5;
const CELL: f32 = 8.;
const CELL_GAP: f32 = 2.;
const CELL_RADIUS: f32 = 2.;
const DAY_LABEL_WIDTH: f32 = 28.;
const MONTH_ROW_HEIGHT: f32 = 14.;
const INNER_SWATCH_ALPHA: f32 = 0.72;
const CLAUDE_HEAT: u32 = 0xD9_77_57;
const DAY_LABELS: [&str; 7] = ["Mon", "", "Wed", "", "Fri", "", ""];

pub(crate) fn tile_columns(page_width: f32) -> usize {
    if page_width < TILE_GRID_MIN_WIDTH {
        1
    } else {
        2
    }
}

pub(crate) fn page_width(viewport_width: f32) -> f32 {
    let column = (viewport_width
        - crate::settings::chrome::SETTINGS_NAV_WIDTH
        - f32::from(crate::widgets::scrollbar::SCROLLBAR_GUTTER))
    .min(f32::from(
        crate::settings::chrome::SETTINGS_COLUMN_MAX_WIDTH,
    ));
    column - 2. * f32::from(crate::settings::chrome::SETTINGS_COLUMN_PADDING)
}

const FAR_RIGHT: f32 = -100_000.;

pub(super) fn scroll_to_today(scroll: &ScrollHandle) {
    scroll.set_offset(gpui::point(px(FAR_RIGHT), px(0.)));
}

fn reveal_day(scroll: &ScrollHandle, view: &ActivityView, day: NaiveDate) {
    let Some(index) = view.day_index(day) else {
        return;
    };
    let width = f32::from(scroll.bounds().size.width);
    if width <= DAY_LABEL_WIDTH + CELL {
        return;
    }
    let left = DAY_LABEL_WIDTH + (index / 7) as f32 * (CELL + CELL_GAP);
    let shown = -f32::from(scroll.offset().x);
    let target = if left - DAY_LABEL_WIDTH < shown {
        left - DAY_LABEL_WIDTH
    } else if left + CELL + crate::pane::TAB_FADE_WIDTH > shown + width {
        left + CELL + crate::pane::TAB_FADE_WIDTH - width
    } else {
        return;
    };
    scroll.set_offset(gpui::point(px(-target.max(0.)), scroll.offset().y));
}

#[derive(Clone, Copy)]
enum UsageButton {
    Refresh,
    OpenAgents,
    Import,
}

pub(crate) fn import_progress_label(files: u64) -> String {
    let unit = if files == 1 { "file" } else { "files" };
    format!("Importing past activity\u{2026} {files} {unit}")
}

pub(crate) fn import_limit_label(days: u64) -> String {
    format!("Imported the last {days} days")
}

pub(crate) struct CellHover {
    pub(crate) day: NaiveDate,
    pub(crate) hovered: bool,
}

pub(crate) type HoverListener = Rc<dyn Fn(&CellHover, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
pub(crate) struct GridColors {
    pub(crate) ramp: [Hsla; 5],
    pub(crate) muted: Hsla,
    pub(crate) focus: Hsla,
}

impl GridColors {
    pub(crate) fn for_agent(slug: &str, ui: UiColors) -> Self {
        let heat: Hsla = if slug
            == paneflow_agent_config::usage::accounts::UsageAgent::ClaudeCode.runtime_slug()
        {
            rgb(CLAUDE_HEAT).into()
        } else {
            ui.text
        };
        Self {
            ramp: [
                ui.subtle,
                lerp_color(ui.subtle, heat, LEVEL_MIX[0]),
                lerp_color(ui.subtle, heat, LEVEL_MIX[1]),
                lerp_color(ui.subtle, heat, LEVEL_MIX[2]),
                lerp_color(ui.subtle, heat, LEVEL_MIX[3]),
            ],
            muted: ui.muted,
            focus: with_alpha(ui.text, 0.7),
        }
    }
}

pub(crate) fn register_grid(
    index: usize,
    view: &ActivityView,
    register: &Register,
    cursor: Option<NaiveDate>,
    colors: GridColors,
    on_hover: &HoverListener,
) -> Stateful<Div> {
    let mut months = div()
        .relative()
        .h(px(MONTH_ROW_HEIGHT))
        .ml(px(DAY_LABEL_WIDTH));
    for (column, name) in &view.months {
        months = months.child(
            div()
                .absolute()
                .top_0()
                .left(px(*column as f32 * (CELL + CELL_GAP)))
                .text_size(px(10.5))
                .line_height(px(MONTH_ROW_HEIGHT))
                .whitespace_nowrap()
                .text_color(colors.muted)
                .child(*name),
        );
    }
    let mut grid = div()
        .id(("usage-register", index))
        .flex()
        .flex_col()
        .gap(px(CELL_GAP))
        .child(months);
    for (row, label) in DAY_LABELS.iter().enumerate() {
        let mut line = div()
            .id(("usage-register-row", index * 7 + row))
            .role(Role::Row)
            .flex()
            .flex_row()
            .gap(px(CELL_GAP))
            .h(px(CELL))
            .child(
                div()
                    .w(px(DAY_LABEL_WIDTH - CELL_GAP))
                    .flex_none()
                    .text_size(px(10.5))
                    .line_height(px(CELL))
                    .whitespace_nowrap()
                    .text_color(colors.muted)
                    .child(*label),
            );
        for column in 0..WEEKS {
            let Some(day) = view
                .first_day
                .checked_add_days(Days::new((column * 7 + row) as u64))
            else {
                continue;
            };
            let Some(offset) = view.day_index(day) else {
                line = line.child(div().flex_none().size(px(CELL)));
                continue;
            };
            let value = register.values.get(offset).copied().unwrap_or(0);
            let level = usize::from(register.levels.get(offset).copied().unwrap_or(0)).min(4);
            let listener = Rc::clone(on_hover);
            line = line.child(
                div()
                    .id(("usage-cell", index * 512 + offset))
                    .relative()
                    .flex_none()
                    .size(px(CELL))
                    .rounded(px(CELL_RADIUS))
                    .bg(colors.ramp[level])
                    .role(Role::GridCell)
                    .aria_label(view.cell_name(day, value))
                    .on_hover(move |hovered, window, cx| {
                        listener(
                            &CellHover {
                                day,
                                hovered: *hovered,
                            },
                            window,
                            cx,
                        );
                    })
                    .when(cursor == Some(day), |cell| {
                        cell.aria_active_descendant().child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .left(px(-2.))
                                .size(px(CELL + 4.))
                                .rounded(px(CELL_RADIUS + 1.5))
                                .border(px(1.5))
                                .border_color(colors.focus),
                        )
                    }),
            );
        }
        grid = grid.child(line);
    }
    grid
}

fn eyebrow_row(label: &'static str, ui: UiColors) -> Div {
    div()
        .flex()
        .flex_row()
        .items_end()
        .gap(px(10.))
        .px(px(2.))
        .pt(px(24.))
        .pb(px(8.))
        .child(section_eyebrow(label, ui))
}

fn muted_text(text: impl Into<SharedString>, ui: UiColors) -> Div {
    div()
        .text_size(px(SMALL_TEXT))
        .line_height(relative(1.4))
        .text_color(ui.muted)
        .child(text.into())
}

fn tile_rings(
    slug: &str,
    outer: Option<&WindowRow>,
    inner: Option<&WindowRow>,
    ui: UiColors,
) -> AnyElement {
    let gauge = match (outer, inner) {
        (Some(outer), Some(inner)) => concentric_rings(
            RingSize::Large,
            (outer.fraction, outer.level),
            (inner.fraction, inner.level),
            ui,
        ),
        (Some(only), None) | (None, Some(only)) => {
            ring(RingSize::Large, only.fraction, only.level, ui)
        }
        (None, None) => concentric_rings(
            RingSize::Large,
            (0., UsageLevel::Normal),
            (0., UsageLevel::Normal),
            ui,
        ),
    };
    div()
        .relative()
        .flex_none()
        .size(px(RingSize::Large.diameter()))
        .child(gauge)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(runtime_logo(slug, TILE_LOGO, ui)),
        )
        .into_any_element()
}

fn legend_row(id: usize, row: &WindowRow, inner: bool, ui: UiColors) -> AnyElement {
    let color = level_color(row.level, ui);
    let swatch = if inner {
        with_alpha(color, INNER_SWATCH_ALPHA)
    } else {
        color
    };
    let reset = if row.reset_note.is_some() {
        String::new()
    } else {
        row.reset
            .strip_prefix("Resets ")
            .unwrap_or(&row.reset)
            .to_string()
    };
    div()
        .id(("usage-tile-meter", id))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .text_size(px(SMALL_TEXT))
        .font_features(tabular_numerals())
        .role(Role::Meter)
        .aria_label(format!("{} {}, {}", row.label, row.percent, row.reset))
        .delayed_tooltip(text_tooltip(row.reset.clone()))
        .child(div().flex_none().size(px(SWATCH)).rounded_full().bg(swatch))
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_color(ui.muted)
                .child(row.label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(ui.text)
                .child(row.percent.clone()),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_color(ui.muted)
                .child(reset),
        )
        .into_any_element()
}

pub(crate) fn render_tile(index: usize, block: &AccountBlock, ui: UiColors) -> AnyElement {
    let slug = block.agent.runtime_slug();
    let label: SharedString = block.label.clone().into();
    let name = div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .min_w_0()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(block.agent.display_name()),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(11.))
                .line_height(relative(1.35))
                .text_color(ui.muted)
                .child(label),
        );
    let mut text = div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .flex_1()
        .min_w_0()
        .child(name);
    let rings = match &block.body {
        BlockBody::Windows(rows) => {
            let inner = rows.iter().find(|row| row.label == "Session");
            let outer = rows.iter().find(|row| row.label != "Session");
            for (position, (row, is_inner)) in outer
                .map(|row| (row, false))
                .into_iter()
                .chain(inner.map(|row| (row, outer.is_some())))
                .enumerate()
            {
                text = text.child(legend_row(index * 4 + position, row, is_inner, ui));
            }
            for row in rows {
                if let Some(note) = &row.reset_note {
                    text = text.child(muted_text(note.clone(), ui));
                }
                if let Some(pace) = &row.pace {
                    text = text.child(pace_line(pace, ui));
                }
            }
            tile_rings(slug, outer, inner, ui)
        }
        BlockBody::Message(message) => {
            text = text.child(muted_text(message.clone(), ui));
            tile_rings(slug, None, None, ui)
        }
    };
    setting_card(ui)
        .id(("usage-tile", index))
        .flex_1()
        .min_w_0()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(16.))
        .py(px(14.))
        .role(Role::Group)
        .aria_label(format!("{}, {}", block.agent.display_name(), block.label))
        .child(rings)
        .child(text)
        .into_any_element()
}

fn tile_grid(view: &UsageView, columns: usize, ui: UiColors) -> AnyElement {
    let tiles: Vec<AnyElement> = view
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| render_tile(index, block, ui))
        .collect();
    let mut grid = div().flex().flex_col().gap(px(TILE_GAP));
    let mut tiles = tiles.into_iter().peekable();
    while tiles.peek().is_some() {
        let mut line = div().flex().flex_row().gap(px(TILE_GAP));
        for _ in 0..columns {
            line = match tiles.next() {
                Some(tile) => line.child(tile),
                None => line.child(div().flex_1().min_w_0()),
            };
        }
        grid = grid.child(line);
    }
    grid.into_any_element()
}

fn metric_pill(metric: Metric, selected: bool, group_focused: bool, ui: UiColors) -> Stateful<Div> {
    let selected_bg = with_alpha(ui.text, 0.10);
    let hover_bg = if selected {
        selected_bg
    } else {
        with_alpha(ui.text, 0.05)
    };
    div()
        .id(SharedString::from(format!(
            "usage-metric-{}",
            metric.label()
        )))
        .h(px(28.))
        .px(px(11.))
        .flex()
        .items_center()
        .rounded_full()
        .when(selected, |pill| pill.bg(selected_bg))
        .when(selected && group_focused, |pill| {
            pill.aria_active_descendant()
        })
        .hover(move |style| style.bg(hover_bg))
        .cursor_pointer()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if selected { ui.text } else { ui.muted })
        .role(Role::RadioButton)
        .aria_label(metric.label())
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .child(metric.label())
}

impl PaneFlowApp {
    pub(crate) fn usage_page_view(&self) -> Option<UsageView> {
        let active_space = self
            .active_space
            .as_ref()
            .and_then(|id| self.cached_config.space(id))
            .map(|space| space.name.as_str());
        usage_view(&UsageInputs {
            inventory: &self.usage.inventory,
            plans: &self.usage.plans,
            active_space,
            home: self.usage.home.as_deref(),
            now_ms: super::now_ms(),
            utc_offset_secs: self.usage.utc_offset_secs,
        })
    }

    pub(crate) fn usage_activity_view(&self) -> ActivityView {
        activity_view(&ActivityInputs {
            ledger: &self.usage.ledger,
            inventory: &self.usage.inventory,
            home: self.usage.home.as_deref(),
            today: super::day_at(super::now_ms(), self.usage.utc_offset_secs),
            metric: self.usage.metric,
        })
    }

    pub(crate) fn render_usage_heading(
        &self,
        title: impl IntoElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let updated = self.usage_page_view().and_then(|view| view.updated);
        let focused = self.usage.refresh_focus.is_focused(window);
        let refresh = crate::settings::components::soft_icon_button(
            "usage-refresh",
            "Refresh",
            "icons/refresh.svg",
            ui,
        )
        .relative()
        .flex_none()
        .track_focus(&self.usage.refresh_focus)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            this.handle_usage_button_key(UsageButton::Refresh, event, window, cx);
        }))
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.refresh_usage_page(cx);
        }))
        .when(focused, |button| {
            button.child(focus_ring(SETTINGS_CONTROL_CORNER_RADIUS, ui))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pb(px(4.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .child(title)
                    .child(div().flex_1())
                    .when_some(updated, |row, updated| {
                        row.child(
                            div()
                                .text_size(crate::ui_primitives::LABEL_SM)
                                .font_features(tabular_numerals())
                                .text_color(ui.muted)
                                .child(updated),
                        )
                    })
                    .child(refresh),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(relative(1.45))
                    .text_color(ui.muted)
                    .child(INTRO),
            )
            .into_any_element()
    }

    pub(crate) fn render_usage_content(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let columns = tile_columns(page_width(f32::from(window.viewport_size().width)));
        let limits = match self.usage_page_view() {
            Some(view) => tile_grid(&view, columns, ui),
            None => self.render_no_account_card(ui, window, cx),
        };
        let has_activity = !self.usage_activity_view().is_empty();
        let limits_copy = &search::USAGE_PLAN_LIMITS;
        let activity_copy = &search::USAGE_ACTIVITY;
        div()
            .flex()
            .flex_col()
            .child(
                Block::new("Plan limits")
                    .child(eyebrow_row("Plan limits", ui))
                    .region(
                        "usage-plan-limits",
                        &[limits_copy.title, limits_copy.description],
                        limits,
                    )
                    .finish(),
            )
            .child(
                Block::new("Activity by agent")
                    .child(
                        eyebrow_row("Activity by agent", ui).when(has_activity, |row| {
                            row.child(div().flex_1())
                                .child(self.render_metric_control(window, cx))
                        }),
                    )
                    .region(
                        "usage-activity",
                        &[activity_copy.title, activity_copy.description],
                        self.render_activity_card(window, cx),
                    )
                    .child(self.render_import_row(window, cx))
                    .finish(),
            )
            .into_any_element()
    }

    fn render_no_account_card(
        &self,
        ui: UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.usage.open_agents_focus.is_focused(window);
        setting_card(ui)
            .p(crate::settings::components::MENU_PADDING)
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .line_height(relative(1.45))
                    .text_color(ui.muted)
                    .child(NO_ACCOUNT),
            )
            .child(
                menu_row("usage-open-agents", false, ui)
                    .relative()
                    .track_focus(&self.usage.open_agents_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.handle_usage_button_key(UsageButton::OpenAgents, event, window, cx);
                    }))
                    .when(focused, |row| {
                        row.child(focus_ring(crate::ui_primitives::ROW_RADIUS, ui))
                    })
                    .role(Role::Button)
                    .aria_label(OPEN_AGENTS_LABEL)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.select_settings_section(SettingsSection::Agents, window, cx);
                    }))
                    .child(
                        svg()
                            .size(px(14.))
                            .flex_none()
                            .path("icons/settings/ai-magic.svg")
                            .text_color(ui.muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::ui_primitives::BODY)
                            .text_color(ui.text)
                            .child(OPEN_AGENTS_LABEL),
                    )
                    .child(
                        svg()
                            .size(px(12.))
                            .flex_none()
                            .path("icons/chevron-right.svg")
                            .text_color(ui.muted),
                    ),
            )
            .into_any_element()
    }

    fn render_import_row(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let import = &self.usage.import;
        let status = if import.running {
            Some(import_progress_label(import.files))
        } else {
            import.limited_days.map(import_limit_label)
        };
        let focused = self.usage.import_focus.is_focused(window);
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .pt(px(8.))
            .text_size(px(SMALL_TEXT))
            .line_height(relative(1.45))
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.))
                    .text_color(ui.muted)
                    .child(TRANSCRIPT_NOTE),
            )
            .when_some(status, |row, status| {
                row.child(
                    div()
                        .id("usage-import-status")
                        .flex_none()
                        .font_features(tabular_numerals())
                        .text_color(ui.muted)
                        .role(Role::Status)
                        .child(status),
                )
            })
            .when(!import.running, |row| {
                row.child(
                    crate::settings::components::ghost_icon_button(
                        "usage-import",
                        IMPORT_LABEL,
                        "icons/download.svg",
                        ui,
                    )
                    .relative()
                    .track_focus(&self.usage.import_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.handle_usage_button_key(UsageButton::Import, event, window, cx);
                    }))
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.import_past_activity(cx);
                    }))
                    .when(focused, |button| {
                        button.child(focus_ring(crate::ui_primitives::ROW_RADIUS, ui))
                    }),
                )
            })
            .into_any_element()
    }

    fn render_metric_control(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let focused = self.usage.metric_focus.is_focused(window);
        let mut control = div()
            .id("usage-metric")
            .relative()
            .flex()
            .flex_row()
            .gap(px(2.))
            .rounded_full()
            .track_focus(&self.usage.metric_focus)
            .role(Role::RadioGroup)
            .aria_label("Metric")
            .on_key_down(cx.listener(Self::handle_metric_key));
        for metric in Metric::ALL {
            let selected = metric == self.usage.metric;
            control = control.child(metric_pill(metric, selected, focused, ui).on_click(
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.usage.metric = metric;
                    cx.notify();
                }),
            ));
        }
        control
            .when(focused, |control| control.child(focus_ring(px(14.), ui)))
            .into_any_element()
    }

    fn render_activity_card(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let view = self.usage_activity_view();
        if view.is_empty() {
            return setting_card(ui)
                .px(px(16.))
                .py(px(14.))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(relative(1.45))
                        .text_color(ui.muted)
                        .child(NO_ACTIVITY),
                )
                .into_any_element();
        }
        let on_hover: HoverListener =
            Rc::new(cx.listener(|this, hover: &CellHover, _window, cx| {
                let next = if hover.hovered {
                    Some(hover.day)
                } else if this.usage.hover_day == Some(hover.day) {
                    None
                } else {
                    return;
                };
                if this.usage.hover_day != next {
                    this.usage.hover_day = next;
                    cx.notify();
                }
            }));
        let mut focused_day = None;
        let mut card = setting_card(ui)
            .px(px(16.))
            .pt(px(14.))
            .pb(px(12.))
            .gap(px(10.));
        for (index, register) in view.registers.iter().enumerate() {
            let header = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(7.))
                .min_w_0()
                .child(runtime_logo(&register.slug, 14., ui))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui.text)
                        .child(register.agent_name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .line_height(relative(1.35))
                        .text_color(ui.muted)
                        .child(register.label.clone()),
                )
                .when(register.has_data, |header| {
                    header.child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .font_features(tabular_numerals())
                            .text_color(ui.muted)
                            .child(register.total.clone()),
                    )
                });
            let body =
                if register.has_data {
                    let handle = self.usage.register_focus.get(index);
                    let focused = handle.is_some_and(|handle| handle.is_focused(window));
                    let cursor = focused.then_some(match self.usage.register_cursor {
                        Some((at, day)) if at == index => day,
                        _ => view.today,
                    });
                    if cursor.is_some() {
                        focused_day = cursor;
                    }
                    let mut grid = register_grid(
                        index,
                        &view,
                        register,
                        cursor,
                        GridColors::for_agent(&register.slug, ui),
                        &on_hover,
                    )
                    .role(Role::Grid)
                    .aria_label(view.grid_label(register))
                    .on_key_down(cx.listener(
                        move |this, event: &KeyDownEvent, window, cx| {
                            this.handle_register_key(index, event, window, cx);
                        },
                    ));
                    if let Some(handle) = handle {
                        grid = grid.track_focus(handle);
                    }
                    let scroll = self.usage.register_scroll.get(index).cloned();
                    let mut viewport = div()
                        .id(("usage-register-scroll", index))
                        .overflow_x_scroll()
                        .child(grid);
                    if let Some(scroll) = &scroll {
                        viewport = viewport.track_scroll(scroll);
                    }
                    div()
                        .relative()
                        .child(viewport)
                        .when_some(scroll, |frame, scroll| {
                            frame.child(div().absolute().inset_0().child(
                                crate::pane::tab_strip_fades(scroll, select_menu_surface(ui)),
                            ))
                        })
                        .into_any_element()
                } else {
                    muted_text(NO_TOKEN_DATA, ui).into_any_element()
                };
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(index > 0, |register| {
                        register
                            .pt(px(10.))
                            .border_t_1()
                            .border_color(with_alpha(ui.border, 0.7))
                    })
                    .child(header)
                    .child(body),
            );
        }
        let detail_day = self.usage.hover_day.or(focused_day).unwrap_or(view.today);
        card.child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .text_size(px(SMALL_TEXT))
                .font_features(tabular_numerals())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(220.))
                        .text_color(ui.text)
                        .child(view.detail(detail_day)),
                )
                .child(div().flex_none().text_color(ui.muted).child(SCALE_NOTE)),
        )
        .into_any_element()
    }

    fn handle_metric_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if key == "tab" {
            self.step_usage_focus(!event.keystroke.modifiers.shift, window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.modified() {
            return;
        }
        let forward = match key {
            "right" | "down" => true,
            "left" | "up" => false,
            _ => return,
        };
        self.usage.metric = self.usage.metric.step(forward);
        cx.notify();
        cx.stop_propagation();
    }

    fn handle_register_key(
        &mut self,
        index: usize,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if key == "tab" {
            self.step_usage_focus(!event.keystroke.modifiers.shift, window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.modified()
            || !matches!(key, "left" | "right" | "up" | "down" | "home" | "end")
        {
            return;
        }
        let view = self.usage_activity_view();
        let current = match self.usage.register_cursor {
            Some((at, day)) if at == index => day,
            _ => view.today,
        };
        let day = step_day(&view, current, key);
        self.usage.register_cursor = Some((index, day));
        self.usage.hover_day = None;
        if let Some(scroll) = self.usage.register_scroll.get(index) {
            reveal_day(scroll, &view, day);
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn handle_usage_button_key(
        &mut self,
        button: UsageButton,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "tab" => self.step_usage_focus(!event.keystroke.modifiers.shift, window, cx),
            "enter" | "space" if !event.keystroke.modifiers.modified() => match button {
                UsageButton::Refresh => self.refresh_usage_page(cx),
                UsageButton::OpenAgents => {
                    self.select_settings_section(SettingsSection::Agents, window, cx);
                }
                UsageButton::Import => self.import_past_activity(cx),
            },
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn step_usage_focus(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.usage_activity_view();
        let mut stops = vec![self.usage.refresh_focus.clone()];
        if self.usage_page_view().is_none() {
            stops.push(self.usage.open_agents_focus.clone());
        }
        if !view.is_empty() {
            stops.push(self.usage.metric_focus.clone());
        }
        stops.extend(
            view.registers
                .iter()
                .zip(&self.usage.register_focus)
                .filter(|(register, _)| register.has_data)
                .map(|(_, handle)| handle.clone()),
        );
        if !self.usage.import.running {
            stops.push(self.usage.import_focus.clone());
        }
        let current = stops.iter().position(|handle| handle.is_focused(window));
        let next = match (current, forward) {
            (None, true) => Some(0),
            (None, false) => stops.len().checked_sub(1),
            (Some(at), true) => (at + 1 < stops.len()).then_some(at + 1),
            (Some(at), false) => at.checked_sub(1),
        };
        match next.and_then(|at| stops.get(at)) {
            Some(handle) => {
                if let Some(entered) = self
                    .usage
                    .register_focus
                    .iter()
                    .position(|candidate| candidate == handle)
                    && self.usage.register_cursor.map(|(at, _)| at) != Some(entered)
                {
                    self.usage.register_cursor = Some((entered, view.today));
                    if let Some(scroll) = self.usage.register_scroll.get(entered) {
                        reveal_day(scroll, &view, view.today);
                    }
                }
                window.focus(handle, cx);
            }
            None => window.focus(&self.settings_focus, cx),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Instant;

    use gpui::{AvailableSpace, point, size};
    use paneflow_serve::usage::ledger::{DayTally, LedgerAccount, LedgerDays};

    use super::*;

    #[test]
    fn the_tile_grid_drops_to_one_column_under_560_px() {
        assert_eq!(tile_columns(644.), 2);
        assert_eq!(tile_columns(560.), 2);
        assert_eq!(tile_columns(559.), 1);
        assert_eq!(tile_columns(page_width(1920.)), 2);
        assert_eq!(tile_columns(page_width(800.)), 1);
    }

    fn year_of_activity(agents: &[(&str, &str)], today: NaiveDate) -> LedgerDays {
        let mut ledger = LedgerDays::new();
        for (index, (agent, dir)) in agents.iter().enumerate() {
            let mut days = BTreeMap::new();
            for offset in 0..380_u64 {
                let Some(day) = today.checked_sub_days(Days::new(offset)) else {
                    continue;
                };
                let turns = (offset * 7 + index as u64 * 3) % 23;
                days.insert(
                    day,
                    DayTally {
                        turns,
                        active_ms: turns * 90_000,
                        tokens_in: turns * 1_200,
                        tokens_out: turns * 300,
                    },
                );
            }
            ledger.insert(
                LedgerAccount {
                    agent: (*agent).into(),
                    config_dir: (*dir).into(),
                },
                days,
            );
        }
        ledger
    }

    #[gpui::test]
    fn four_registers_render_within_a_frame(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let today = NaiveDate::from_ymd_opt(2026, 8, 18).unwrap();
        let ledger = year_of_activity(
            &[
                ("claude-code", "/home/a/.claude"),
                ("claude-code", "/home/a/.claude-work"),
                ("codex", "/home/a/.codex"),
                ("gemini", ""),
            ],
            today,
        );
        let ui = cx.update(|_, _| crate::theme::ui_colors());
        let mut timings = Vec::new();
        for metric in Metric::ALL {
            let started = Instant::now();
            cx.draw(
                point(px(0.), px(0.)),
                size(
                    AvailableSpace::Definite(px(644.)),
                    AvailableSpace::MinContent,
                ),
                |_, _| {
                    let view = activity_view(&ActivityInputs {
                        ledger: &ledger,
                        inventory: &[],
                        home: None,
                        today,
                        metric,
                    });
                    assert_eq!(view.registers.len(), 4);
                    let on_hover: HoverListener = Rc::new(|_, _, _| {});
                    let mut column = div().flex().flex_col();
                    for (index, register) in view.registers.iter().enumerate() {
                        column = column.child(register_grid(
                            index,
                            &view,
                            register,
                            (index == 0).then_some(today),
                            GridColors::for_agent(&register.slug, ui),
                            &on_hover,
                        ));
                    }
                    column
                },
            );
            timings.push(started.elapsed());
        }
        let worst = timings.iter().max().copied().unwrap_or_default();
        eprintln!("usage page: four registers built, laid out and painted in {timings:?}");
        if !cfg!(debug_assertions) {
            assert!(
                worst.as_secs_f64() * 1000. < 16.,
                "four registers took {worst:?} in a release build"
            );
        }
    }
}

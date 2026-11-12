use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseUpEvent, ParentElement, Role, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, deferred, div, prelude::FluentBuilder, px, relative, rgb, svg,
};
use paneflow_agent_config::runtime_catalog::runtime_by_slug;
use paneflow_agent_config::usage::accounts::UsageAgent;
use paneflow_agent_config::usage::model::UsageLevel;

use super::model::{
    AccountBlock, BlockBody, CapsuleView, Pace, UsageView, WindowRow, bubble_list_max_height,
    capsule_shows_percent,
};
use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;

use crate::settings::components::{menu_divider_color, menu_row, menu_surface_filled, with_alpha};
use crate::theme::UiColors;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::ring::{RingSize, level_color, ring};
use crate::ui_primitives::squircle::squircle_border;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin, text_tooltip};

const BUBBLE_WIDTH: f32 = 284.;
const BUBBLE_BOTTOM: f32 = 52.;
const CAPSULE_HEIGHT: f32 = 36.;
const CAPSULE_RADIUS: f32 = 12.;
const METER_LABEL_WIDTH: f32 = 52.;
const METER_PERCENT_WIDTH: f32 = 34.;
const METER_BAR_HEIGHT: f32 = 4.;
const LOGO_SIZE: f32 = 15.;
const SMALL_TEXT: f32 = 11.5;
const FOCUS_RING_ALPHA: f32 = 0.35;
const TRACK_ALPHA: f32 = 0.13;
const OPEN_SETTINGS_LABEL: &str = "Open usage settings";

impl PaneFlowApp {
    pub(crate) fn render_usage_capsule(
        &self,
        view: &UsageView,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let open = self.usage.bubble_open;
        let focused = self.usage.capsule_focus.is_focused(window);
        let footer_width =
            crate::app::constants::SIDEBAR_WIDTH - 2. * crate::app::sidebar::SIDEBAR_ROW_MARGIN_X;
        let (value, level, percent) = match &view.capsule {
            CapsuleView::Binding {
                fraction,
                level,
                percent,
                ..
            } => (*fraction, *level, Some(percent.clone())),
            CapsuleView::NoData => (0., UsageLevel::Normal, None),
        };
        let percent = percent.filter(|_| capsule_shows_percent(footer_width));
        let ring_only = percent.is_none();
        squircle_skin(
            div()
                .id("sidebar-usage-capsule")
                .flex_none()
                .h(px(CAPSULE_HEIGHT))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(7.))
                .when(ring_only, |capsule| {
                    capsule.w(px(CAPSULE_HEIGHT)).justify_center()
                })
                .when(!ring_only, |capsule| capsule.pl(px(9.)).pr(px(11.))),
            "sidebar-usage-capsule-group",
            px(CAPSULE_RADIUS),
            open.then(crate::app::constants::sidebar_tab_active_background),
            (!open).then(crate::app::constants::sidebar_tab_hover_background),
        )
        .track_focus(&self.usage.capsule_focus)
        .role(Role::Button)
        .aria_label(view.capsule.accessible_name())
        .aria_expanded(open)
        .delayed_tooltip(text_tooltip(view.capsule.tooltip()))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            if open {
                this.close_usage_bubble(window, cx);
            } else {
                this.open_usage_bubble(window, cx);
            }
        }))
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key == "tab" && event.keystroke.modifiers.shift {
                let focus = this.sidebar_filter_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                cx.stop_propagation();
            }
        }))
        .child(ring(RingSize::Small, value, level, ui))
        .when(focused, |capsule| {
            capsule.child(focus_ring(px(CAPSULE_RADIUS), ui))
        })
        .when_some(percent, |capsule, percent| {
            capsule.child(
                div()
                    .font_family("Geist")
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(tabular_numerals())
                    .text_color(ui.text)
                    .child(percent),
            )
        })
        .into_any_element()
    }

    pub(crate) fn render_usage_bubble(
        &self,
        view: &UsageView,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let cursor = self.usage.cursor;
        let settings_stop = view.blocks.len();
        let list_max_height = bubble_list_max_height(f32::from(window.viewport_size().height));
        let mut accounts = div()
            .id("usage-bubble-accounts")
            .flex()
            .flex_col()
            .min_h_0()
            .max_h(px(list_max_height))
            .overflow_y_scroll();
        for (index, block) in view.blocks.iter().enumerate() {
            if index > 0 {
                accounts = accounts.child(bubble_divider(ui));
            }
            accounts = accounts.child(render_block(index, block, cursor == Some(index), ui));
        }
        let open_settings = menu_row("usage-open-settings", false, ui)
            .relative()
            .role(Role::Button)
            .aria_label(OPEN_SETTINGS_LABEL)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.open_usage_settings(window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .child(OPEN_SETTINGS_LABEL),
            )
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/chevron-right.svg")
                    .text_color(ui.muted),
            )
            .when(cursor == Some(settings_stop), |row| {
                row.aria_active_descendant()
                    .child(focus_ring(ROW_RADIUS, ui))
            });
        let header = div()
            .flex()
            .flex_row()
            .items_baseline()
            .justify_between()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .child(
                div()
                    .text_size(crate::ui_primitives::LABEL_SM)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.muted)
                    .child("Plan usage"),
            )
            .when_some(view.updated.clone(), |header, updated| {
                header.child(
                    div()
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .font_features(tabular_numerals())
                        .text_color(ui.muted)
                        .child(updated),
                )
            });
        let fill = crate::app::constants::sidebar_menu_background(
            self.cached_config.interface_style(),
            ui,
        );
        let top = self.usage.bubble_top();
        let bubble = menu_surface_filled(div().id("usage-bubble"), fill, ui)
            .child(
                canvas(
                    move |bounds, _, _| top.set(Some(bounds.origin.y)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .font_family("Geist")
            .w(px(BUBBLE_WIDTH))
            .flex()
            .flex_col()
            .p(crate::settings::components::MENU_PADDING)
            .track_focus(&self.usage.bubble_focus)
            .role(Role::Dialog)
            .aria_label("Plan usage")
            .on_key_down(cx.listener(Self::handle_usage_bubble_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    this.close_usage_bubble(window, cx);
                }),
            )
            .child(header)
            .child(accounts)
            .child(bubble_divider(ui))
            .child(open_settings);
        deferred(crate::ui_primitives::menu_reveal(
            "usage-bubble-reveal",
            div()
                .absolute()
                .bottom(px(BUBBLE_BOTTOM))
                .left(px(crate::app::sidebar::SIDEBAR_ROW_MARGIN_X))
                .occlude()
                .child(bubble),
        ))
        .with_priority(3)
        .into_any_element()
    }
}

fn bubble_divider(ui: UiColors) -> gpui::Div {
    div()
        .flex_none()
        .mx(px(6.))
        .my(px(4.))
        .h(px(1.))
        .bg(menu_divider_color(ui))
}

fn render_block(index: usize, block: &AccountBlock, focused: bool, ui: UiColors) -> AnyElement {
    let label_text = div()
        .min_w_0()
        .text_size(px(SMALL_TEXT))
        .text_color(ui.muted);
    let label_text = if focused {
        label_text.line_height(relative(1.4))
    } else {
        label_text
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
    };
    let label: SharedString = block.label.clone().into();
    let identity = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .child(agent_logo(block.agent, ui))
        .child(
            div()
                .flex_none()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(block.agent.display_name()),
        )
        .child(
            div()
                .id(("usage-account-label", index))
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .delayed_tooltip(text_tooltip(label.clone()))
                .child(label_text.child(label)),
        );
    let body = match &block.body {
        BlockBody::Message(text) => div()
            .text_size(px(SMALL_TEXT))
            .line_height(relative(1.4))
            .text_color(ui.muted)
            .child(text.clone())
            .into_any_element(),
        BlockBody::Windows(rows) => {
            let mut meters = div().flex().flex_col().gap(px(3.));
            for (window, row) in rows.iter().enumerate() {
                meters = meters.child(render_meter(index, window, row, focused, ui));
            }
            meters.into_any_element()
        }
    };
    div()
        .id(("usage-account", index))
        .relative()
        .flex()
        .flex_col()
        .gap(px(7.))
        .px(px(10.))
        .pt(px(8.))
        .pb(px(10.))
        .role(Role::Group)
        .aria_label(format!("{}, {}", block.agent.display_name(), block.label))
        .child(identity)
        .child(body)
        .when(focused, |block| {
            block
                .aria_active_descendant()
                .child(focus_ring(ROW_RADIUS, ui))
        })
        .into_any_element()
}

fn render_meter(
    block: usize,
    window: usize,
    row: &WindowRow,
    focused: bool,
    ui: UiColors,
) -> AnyElement {
    let color = level_color(row.level, ui);
    let fill = (row.fraction > 0.).then(|| {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(relative(row.fraction as f32))
            .min_w(px(METER_BAR_HEIGHT))
            .rounded_full()
            .bg(color)
    });
    let meter = div()
        .id(("usage-meter", block * 4 + window))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .min_h(px(18.))
        .text_size(px(SMALL_TEXT))
        .font_features(tabular_numerals())
        .role(Role::Meter)
        .aria_label(format!("{} {}, {}", row.label, row.percent, row.reset))
        .delayed_tooltip(text_tooltip(row.reset.clone()))
        .child(
            div()
                .w(px(METER_LABEL_WIDTH))
                .flex_none()
                .whitespace_nowrap()
                .text_color(ui.muted)
                .child(row.label),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(METER_BAR_HEIGHT))
                .rounded_full()
                .bg(with_alpha(ui.text, TRACK_ALPHA))
                .children(fill),
        )
        .child(
            div()
                .w(px(METER_PERCENT_WIDTH))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(ui.text)
                .child(row.percent.clone()),
        );
    let mut stack = div().flex().flex_col().gap(px(4.)).child(meter);
    match &row.reset_note {
        Some(note) => stack = stack.child(muted_line(note.clone(), ui)),
        None if focused => stack = stack.child(muted_line(row.reset.clone(), ui)),
        None => {}
    }
    if let Some(pace) = &row.pace {
        stack = stack.child(pace_line(pace, ui));
    }
    stack.into_any_element()
}

fn muted_line(text: String, ui: UiColors) -> AnyElement {
    div()
        .text_size(px(SMALL_TEXT))
        .line_height(relative(1.4))
        .text_color(ui.muted)
        .child(text)
        .into_any_element()
}

pub(super) fn pace_line(pace: &Pace, ui: UiColors) -> AnyElement {
    let (icon, icon_color, text_color, text) = match pace {
        Pace::ReachesLimit(text) => (
            "icons/triangle-alert.svg",
            level_color(UsageLevel::Warn, ui),
            ui.text,
            text,
        ),
        Pace::OnTrack(text) => ("icons/check.svg", ui.muted, ui.muted, text),
    };
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(7.))
        .text_size(px(SMALL_TEXT))
        .line_height(relative(1.4))
        .text_color(text_color)
        .child(
            svg()
                .mt(px(1.))
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(icon_color),
        )
        .child(div().flex_1().min_w_0().child(text.clone()))
        .into_any_element()
}

fn agent_logo(agent: UsageAgent, ui: UiColors) -> AnyElement {
    runtime_logo(agent.runtime_slug(), LOGO_SIZE, ui)
}

pub(super) fn runtime_logo(slug: &str, size: f32, ui: UiColors) -> AnyElement {
    let Some(terminal) =
        runtime_by_slug(slug).and_then(|runtime| TerminalAgent::from_runtime_id(runtime.id))
    else {
        return div().size(px(size)).flex_none().into_any_element();
    };
    let tint: Hsla = terminal
        .accent()
        .map_or(ui.text, |accent| rgb(accent).into());
    crate::settings::components::render_logo(
        terminal.icon_path(),
        terminal.icon_multicolor(),
        px(size),
        tint,
    )
}

pub(super) fn focus_ring(radius: gpui::Pixels, ui: UiColors) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .child(squircle_border(
            radius,
            px(1.5),
            with_alpha(ui.text, FOCUS_RING_ALPHA),
        ))
        .into_any_element()
}

pub(super) fn tabular_numerals() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

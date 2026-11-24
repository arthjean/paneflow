use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseUpEvent, ParentElement, Role, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, deferred, div, prelude::FluentBuilder, px, relative, rgb, svg,
};
use paneflow_agent_config::runtime_catalog::runtime_by_slug;
use paneflow_agent_config::usage::model::UsageLevel;

use super::model::{
    AccountBlock, BlockBody, CapsuleView, Pace, UsageView, WindowRow, bubble_list_max_height,
    capsule_shows_percent,
};
use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;

use crate::settings::components::{MENU_RADIUS, menu_padding, menu_surface_filled, with_alpha};
use crate::settings::kit::{self, Ink, MeterTone, Text};
use crate::settings::private_text;
use crate::theme::UiColors;
use crate::ui_primitives::AccessibleControlExt as _;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::ring::{RingSize, level_color, ring};
use crate::ui_primitives::squircle::squircle_border;
use crate::ui_primitives::{squircle_skin, text_tooltip};

const BUBBLE_WIDTH: f32 = 284.;
const BUBBLE_BOTTOM: f32 = 52.;
const CAPSULE_HEIGHT: f32 = 36.;
const CAPSULE_RADIUS: f32 = 12.;
const CONTENT_INSET: f32 = 10.;
const HEADER_TOP: f32 = 8.;
const HEADER_BOTTOM: f32 = 2.;
const ROW_PADDING_Y: f32 = 8.;
const AVATAR_SIZE: f32 = 24.;
const LEAD_GAP: f32 = 8.;
const BODY_TOP: f32 = 4.;
const METER_STACK_GAP: f32 = 4.;
const METER_NOTE_GAP: f32 = 2.;
const DIVIDER_MARGIN: f32 = 4.;
const CHEVRON_SIZE: f32 = 12.;
const OPEN_SETTINGS_GROUP: &str = "usage-open-settings-row";
const SMALL_TEXT: f32 = 11.5;
const FOCUS_RING_ALPHA: f32 = 0.35;
const FOCUS_RING_WIDTH: f32 = 1.5;
const OPEN_SETTINGS_LABEL: &str = "Open usage settings";
const IN_USE_LABEL: &str = "In use";

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
        let footer_width = self.sidebar_width() - 2. * crate::app::sidebar::SIDEBAR_ROW_MARGIN_X;
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
        .on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
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
        crate::ui_primitives::interface_style::with_surface_shape(|| {
            self.render_styled_usage_bubble(view, window, cx)
        })
    }

    fn render_styled_usage_bubble(
        &self,
        view: &UsageView,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
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
                accounts = accounts.child(kit::rule(ink));
            }
            accounts = accounts.child(self.render_usage_account(
                index,
                block,
                cursor == Some(index),
                ink,
                cx,
            ));
        }
        let inner_radius = nested_row_radius();
        let open_settings = div()
            .id("usage-open-settings")
            .group(OPEN_SETTINGS_GROUP)
            .relative()
            .flex_none()
            .h(crate::ui_primitives::action_menu::ROW_HEIGHT)
            .px(px(CONTENT_INSET))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(inner_radius)
                    .bg(ink.hover)
                    .invisible()
                    .group_hover(OPEN_SETTINGS_GROUP, |style| style.visible()),
            )
            .role(Role::Button)
            .aria_label(OPEN_SETTINGS_LABEL)
            .on_activate(cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.open_usage_settings(window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(kit::SIZE_MD))
                    .line_height(px(kit::LINE_MD))
                    .text_color(ink.primary)
                    .child(OPEN_SETTINGS_LABEL),
            )
            .child(
                svg()
                    .size(px(CHEVRON_SIZE))
                    .flex_none()
                    .path("icons/chevron-right.svg")
                    .text_color(ink.tertiary),
            )
            .when(cursor == Some(settings_stop), |row| {
                row.aria_active_descendant()
                    .child(nested_focus_ring(inner_radius, ui))
            });
        let header = div()
            .flex()
            .flex_row()
            .items_baseline()
            .justify_between()
            .gap(px(8.))
            .px(px(CONTENT_INSET))
            .pt(px(HEADER_TOP))
            .pb(px(HEADER_BOTTOM))
            .child(
                div()
                    .text_size(px(kit::SIZE_MD))
                    .line_height(px(kit::LINE_MD))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ink.primary)
                    .child("Plan usage"),
            )
            .when_some(view.updated.clone(), |header, updated| {
                header.child(
                    kit::line(ink, updated)
                        .flex_none()
                        .whitespace_nowrap()
                        .font_features(tabular_numerals()),
                )
            });
        let fill = crate::app::constants::sidebar_menu_background(
            self.cached_config.resolved_background_style(),
            ui,
        );
        let margin = crate::app::sidebar::SIDEBAR_ROW_MARGIN_X;
        let bubble_left = (self.sidebar_width() - margin - BUBBLE_WIDTH).max(margin);
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
            .p(menu_padding())
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
            .child(
                div()
                    .flex_none()
                    .py(px(DIVIDER_MARGIN))
                    .child(kit::rule(ink)),
            )
            .child(open_settings);
        deferred(crate::ui_primitives::menu_reveal(
            "usage-bubble-reveal",
            div()
                .absolute()
                .bottom(px(BUBBLE_BOTTOM))
                .left(px(bubble_left))
                .occlude()
                .child(bubble),
        ))
        .with_priority(3)
        .into_any_element()
    }

    fn render_usage_account(
        &self,
        index: usize,
        block: &AccountBlock,
        focused: bool,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let detail = account_detail(block);
        let private = private_text::is_private(&block.label);
        let hidden = private && !self.private_reveals.is_shown(&block.key);
        let title = if private {
            let key = block.key.clone();
            private_text::private_text(
                &block.key,
                &block.label,
                &self.private_reveals,
                ink.primary,
                ink.accent,
            )
            .on_activate(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.private_reveals.toggle(&key);
                cx.notify();
            }))
            .into_any_element()
        } else {
            let label: SharedString = block.label.clone().into();
            div()
                .id(("usage-account-label", index))
                .min_w_0()
                .when(!focused, |title| title.truncate())
                .delayed_tooltip(text_tooltip(label.clone()))
                .child(label)
                .into_any_element()
        };
        let mut text = Text::new(block.label.clone())
            .drawn_as(title)
            .line(kit::line(ink, detail.clone()));
        if block.in_use {
            text = text.badge(kit::badge(ink, IN_USE_LABEL));
        }
        let body = match &block.body {
            BlockBody::Message(message) => kit::line(ink, message.clone()).into_any_element(),
            BlockBody::Windows(rows) => {
                let mut meters = div().flex().flex_col().gap(px(METER_STACK_GAP));
                for (window, row) in rows.iter().enumerate() {
                    meters = meters.child(render_meter(index, window, row, focused, ink));
                }
                meters.into_any_element()
            }
        };
        let text = text.line(div().pt(px(BODY_TOP)).child(body));
        let name = if hidden {
            detail
        } else {
            format!("{detail}, {}", block.label)
        };
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(LEAD_GAP))
            .px(px(CONTENT_INSET))
            .py(px(ROW_PADDING_Y))
            .child(kit::avatar_of(&block.label, AVATAR_SIZE))
            .child(text.render(ink))
            .id(("usage-account", index))
            .relative()
            .role(Role::Group)
            .aria_label(name)
            .when(focused, |row| {
                row.aria_active_descendant()
                    .child(nested_focus_ring(nested_row_radius(), ui))
            })
            .into_any_element()
    }
}

fn account_detail(block: &AccountBlock) -> String {
    let agent = block.agent.display_name();
    match &block.plan {
        Some(plan) => format!("{agent} · {plan}"),
        None => agent.to_string(),
    }
}

fn nested_row_radius() -> gpui::Pixels {
    crate::ui_primitives::interface_style::frame_corner(MENU_RADIUS) - menu_padding()
}

fn nested_focus_ring(radius: gpui::Pixels, ui: UiColors) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .rounded(radius)
        .border(px(FOCUS_RING_WIDTH))
        .border_color(with_alpha(ui.text, FOCUS_RING_ALPHA))
        .into_any_element()
}

fn render_meter(
    block: usize,
    window: usize,
    row: &WindowRow,
    focused: bool,
    ink: Ink,
) -> AnyElement {
    let meter = div()
        .id(("usage-meter", block * 4 + window))
        .font_features(tabular_numerals())
        .role(Role::Meter)
        .aria_label(format!("{} {}, {}", row.label, row.percent, row.reset))
        .delayed_tooltip(text_tooltip(row.reset.clone()))
        .child(kit::meter(
            ink,
            row.label,
            row.fraction,
            row.percent.clone(),
            MeterTone::of(row.level),
        ));
    let mut stack = div().flex().flex_col().gap(px(METER_NOTE_GAP)).child(meter);
    match &row.reset_note {
        Some(note) => stack = stack.child(kit::line(ink, note.clone())),
        None if focused => stack = stack.child(kit::line(ink, row.reset.clone())),
        None => {}
    }
    if let Some(pace) = row
        .pace
        .as_ref()
        .filter(|pace| !matches!(pace, Pace::OnTrack(_)))
    {
        stack = stack.child(kit_pace_line(pace, ink));
    }
    stack.into_any_element()
}

fn kit_pace_line(pace: &Pace, ink: Ink) -> AnyElement {
    let (alert, text_color, text) = match pace {
        Pace::Reached(text) => (Some(ink.danger), ink.primary, text),
        Pace::ReachesLimit(text) => (Some(ink.warning), ink.primary, text),
        Pace::OnTrack(text) => (None, ink.tertiary, text),
    };
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(6.))
        .text_size(px(kit::SIZE_SM))
        .line_height(px(kit::LINE_SM))
        .text_color(text_color)
        .when_some(alert, |line, color| {
            line.child(
                svg()
                    .mt(px(2.))
                    .size(px(12.))
                    .flex_none()
                    .path("icons/triangle-alert.svg")
                    .text_color(color),
            )
        })
        .child(div().flex_1().min_w_0().child(text.clone()))
        .into_any_element()
}

pub(super) fn pace_line(pace: &Pace, ui: UiColors) -> AnyElement {
    let (alert, text_color, text) = match pace {
        Pace::Reached(text) => (Some(level_color(UsageLevel::Critical, ui)), ui.text, text),
        Pace::ReachesLimit(text) => (Some(level_color(UsageLevel::Warn, ui)), ui.text, text),
        Pace::OnTrack(text) => (None, ui.muted, text),
    };
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(6.))
        .text_size(px(SMALL_TEXT))
        .line_height(relative(1.4))
        .text_color(text_color)
        .when_some(alert, |line, color| {
            line.child(
                svg()
                    .mt(px(2.))
                    .size(px(12.))
                    .flex_none()
                    .path("icons/triangle-alert.svg")
                    .text_color(color),
            )
        })
        .child(div().flex_1().min_w_0().child(text.clone()))
        .into_any_element()
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

pub(crate) fn focus_ring(radius: gpui::Pixels, ui: UiColors) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .child(squircle_border(
            radius,
            px(FOCUS_RING_WIDTH),
            with_alpha(ui.text, FOCUS_RING_ALPHA),
        ))
        .into_any_element()
}

pub(super) fn tabular_numerals() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Rgba, SharedString, StatefulInteractiveElement, Styled, div, prelude::*, px,
    svg,
};
use serde_json::{Value, json};

use paneflow_config::schema::{CursorShapeConfig, MinimumContrast, TerminalConfig};

use crate::settings::components::{
    deferred_select_menu, select_chevron, select_menu, select_option, select_trigger,
};
use crate::settings::kit::{self, Ink, Text};
use crate::settings::search::{self, Block, SearchCard};
use crate::ui_primitives::AccessibleControlExt;

use crate::{PaneFlowApp, TerminalDropdown};

const TERMINAL_INTRO: &str = "How text and the cursor look in every terminal pane.";
const ADD_ICON: &str = "icons/settings/add-01.svg";
const REMOVE_ICON: &str = "icons/settings/remove-01.svg";
const TICK_ICON: &str = "icons/settings/tick-02.svg";
const CHEVRON_DOWN: &str = "icons/settings/arrow-down-01.svg";
const CHEVRON_UP: &str = "icons/settings/arrow-up-01.svg";
const CHEVRON: f32 = 14.;
const SWATCH: f32 = 32.;
const SWATCH_RING: f32 = 2.;
const SWATCH_RING_GAP: f32 = 2.;
const SWATCH_RADIUS: f32 = 6.;
const SWATCH_GAP: f32 = 4.;
const SWATCH_PREVIEW: f32 = 14.;
const SCHEME_HEIGHT: f32 = 28.;
const SCHEME_PADDING_X: f32 = 10.;
const SCHEME_RADIUS: f32 = 6.;

const CURSOR_SHAPES: [(&str, CursorShapeConfig); 6] = [
    ("Vintage (_▂)", CursorShapeConfig::Vintage),
    ("Bar (|)", CursorShapeConfig::Beam),
    ("Underline (_)", CursorShapeConfig::Underline),
    ("Double underline (‿)", CursorShapeConfig::DoubleUnderline),
    ("Filled box (█)", CursorShapeConfig::Block),
    ("Empty box (□)", CursorShapeConfig::Hollow),
];

const FONT_WEIGHT_OPTIONS: [(&str, &str); 11] = [
    ("Thin", "thin"),
    ("Extra-light", "extra_light"),
    ("Light", "light"),
    ("Semi light", "semi_light"),
    ("Normal", "normal"),
    ("Medium", "medium"),
    ("Semi-bold", "semi_bold"),
    ("Bold", "bold"),
    ("Extra-bold", "extra_bold"),
    ("Black", "black"),
    ("Extra-black", "extra_black"),
];

const CURSOR_COLOR_SWATCHES: [u32; 16] = [
    0x007aff, 0x0a84ff, 0x5aa6ff, 0x57d5c4, 0x57d992, 0xffd166, 0xff6f6a, 0xc79bff, 0x3f4451,
    0xf0f3f7, 0x4c6fff, 0x315ecf, 0x40c878, 0xf89850, 0xf87878, 0xd8d0d0,
];

pub(crate) const MINIMUM_CONTRAST_STEPS: [(&str, Option<f32>); 6] = [
    ("Auto", None),
    ("Off", Some(0.0)),
    ("45", Some(45.0)),
    ("60", Some(60.0)),
    ("75", Some(75.0)),
    ("90", Some(90.0)),
];

pub(crate) fn minimum_contrast_step(terminal: &TerminalConfig) -> usize {
    match terminal.minimum_contrast() {
        MinimumContrast::Automatic | MinimumContrast::Rejected(_) => 0,
        MinimumContrast::Explicit(lc) => MINIMUM_CONTRAST_STEPS
            .iter()
            .enumerate()
            .skip(1)
            .min_by(|(_, (_, left)), (_, (_, right))| {
                let left = left.map_or(f32::INFINITY, |step| (step - lc).abs());
                let right = right.map_or(f32::INFINITY, |step| (step - lc).abs());
                left.total_cmp(&right)
            })
            .map_or(0, |(index, _)| index),
    }
}

pub(crate) fn minimum_contrast_setting(step: usize) -> Value {
    MINIMUM_CONTRAST_STEPS
        .get(step)
        .and_then(|(_, lc)| *lc)
        .map_or(Value::Null, |lc| json!(lc))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FontPick<'a> {
    Default,
    Named(&'a str),
}

pub(crate) struct FontMatches<'a> {
    pub(crate) default_label: String,
    pub(crate) default_matches: bool,
    pub(crate) names: Vec<&'a str>,
}

impl<'a> FontMatches<'a> {
    pub(crate) fn new(names: &'a [String], default_font: &str, search: &str) -> Self {
        let search = search.to_lowercase();
        let default_label = format!("PaneFlow default - {default_font}");
        let default_matches = search.is_empty() || default_label.to_lowercase().contains(&search);
        let names = names
            .iter()
            .map(String::as_str)
            .filter(|name| {
                *name != default_font
                    && (search.is_empty() || name.to_lowercase().contains(&search))
            })
            .collect();
        Self {
            default_label,
            default_matches,
            names,
        }
    }

    pub(crate) fn first(&self) -> Option<FontPick<'a>> {
        if self.default_matches {
            Some(FontPick::Default)
        } else {
            self.names.first().map(|name| FontPick::Named(name))
        }
    }
}

pub(crate) fn font_search_accepts(key_char: &str) -> bool {
    !key_char.is_empty() && !key_char.chars().any(char::is_control)
}

fn hex_string_from_u32(hex: u32) -> String {
    format!("#{hex:06X}")
}

fn hsla_from_u32(hex: u32) -> Hsla {
    Hsla::from(gpui::rgb(hex))
}

fn hex_string_from_hsla(color: Hsla) -> String {
    let rgba = Rgba::from(color);
    let channel = |value: f32| -> u8 { (value.clamp(0.0, 1.0) * 255.0).round() as u8 };
    format!(
        "#{:02X}{:02X}{:02X}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b)
    )
}

impl PaneFlowApp {
    pub(crate) fn render_terminal_heading(&self, title: gpui::Div) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(
                Ink::of(crate::theme::ui_colors()),
                TERMINAL_INTRO,
            ))
            .into_any_element()
    }

    pub(crate) fn render_terminal_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let config = &self.cached_config;
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        let terminal = config.terminal.clone().unwrap_or_default();

        let shape = terminal.cursor_shape.unwrap_or_default();
        let integrated_glyphs = terminal.resolved_integrated_glyphs();
        let color_emoji = terminal.resolved_color_emoji();
        let scrollbar = terminal.resolved_scrollbar_visible();
        let configured_cursor_color = terminal.normalized_cursor_color();
        let theme_cursor_hex = hex_string_from_hsla(crate::theme::active_theme().cursor);
        let cursor_color_hex = configured_cursor_color
            .clone()
            .unwrap_or_else(|| theme_cursor_hex.clone());
        let cursor_uses_theme = configured_cursor_color.is_none();
        let current_font =
            crate::terminal::element::resolve_font_family(config.font_family.as_deref());
        let font_weight_key =
            crate::terminal::element::normalize_font_weight_key(config.font_weight.as_deref());
        let font_size = config
            .font_size
            .unwrap_or(crate::terminal::element::DEFAULT_FONT_SIZE) as f64;
        let line_height = config
            .line_height
            .unwrap_or(crate::terminal::element::DEFAULT_LINE_HEIGHT)
            as f64;
        let cell_width = config
            .cell_width
            .unwrap_or(crate::terminal::element::DEFAULT_CELL_WIDTH)
            as f64;

        let shape_label = CURSOR_SHAPES
            .iter()
            .find_map(|(label, value)| (*value == shape).then_some(*label))
            .unwrap_or("Filled box (█)");
        let shape_opts: Vec<(String, Value, bool)> = CURSOR_SHAPES
            .iter()
            .map(|(label, value)| {
                (
                    (*label).to_string(),
                    serde_json::to_value(value).unwrap_or(Value::Null),
                    *value == shape,
                )
            })
            .collect();
        let font_weight_label = FONT_WEIGHT_OPTIONS
            .iter()
            .find_map(|(label, key)| (*key == font_weight_key).then_some(*label))
            .unwrap_or("Normal");
        let font_weight_opts: Vec<(String, Value, bool)> = FONT_WEIGHT_OPTIONS
            .iter()
            .map(|(label, key)| ((*label).to_string(), json!(*key), *key == font_weight_key))
            .collect();

        let font_card = SearchCard::new()
            .row(
                &search::FONT_FAMILY,
                self.terminal_font_family_row(current_font, ink, ui, cx),
            )
            .row(
                &search::FONT_SIZE,
                self.terminal_number_row(
                    "term-font-size",
                    &search::FONT_SIZE,
                    NumberStep {
                        value: font_size,
                        min: 8.0,
                        max: 32.0,
                        step: 1.0,
                        decimals: 0,
                    },
                    "font_size",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::FONT_WEIGHT,
                self.terminal_enum_row(
                    TerminalDropdown::FontWeight,
                    &search::FONT_WEIGHT,
                    font_weight_label.to_string(),
                    font_weight_opts,
                    "font_weight",
                    false,
                    ink,
                    ui,
                    cx,
                ),
            )
            .row(
                &search::LINE_HEIGHT,
                self.terminal_number_row(
                    "term-line-height",
                    &search::LINE_HEIGHT,
                    NumberStep {
                        value: line_height,
                        min: 1.0,
                        max: 2.5,
                        step: 0.1,
                        decimals: 1,
                    },
                    "line_height",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::CELL_WIDTH,
                self.terminal_number_row(
                    "term-cell-width",
                    &search::CELL_WIDTH,
                    NumberStep {
                        value: cell_width,
                        min: 0.3,
                        max: 2.0,
                        step: 0.1,
                        decimals: 1,
                    },
                    "cell_width",
                    ink,
                    cx,
                ),
            );

        let cursor_card = SearchCard::new()
            .row(
                &search::CURSOR_SHAPE,
                self.terminal_enum_row(
                    TerminalDropdown::CursorShape,
                    &search::CURSOR_SHAPE,
                    shape_label.to_string(),
                    shape_opts,
                    "cursor_shape",
                    true,
                    ink,
                    ui,
                    cx,
                ),
            )
            .row(
                &search::CURSOR_COLOR,
                self.terminal_cursor_color_row(
                    cursor_color_hex,
                    cursor_uses_theme,
                    theme_cursor_hex,
                    ink,
                    cx,
                ),
            );

        let rendering_card = SearchCard::new()
            .row(
                &search::INTEGRATED_GLYPHS,
                self.terminal_toggle_row(
                    "term-integrated-glyphs",
                    &search::INTEGRATED_GLYPHS,
                    integrated_glyphs,
                    "integrated_glyphs",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::COLOR_EMOJI,
                self.terminal_toggle_row(
                    "term-color-emoji",
                    &search::COLOR_EMOJI,
                    color_emoji,
                    "color_emoji",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::SCROLLBAR,
                self.terminal_toggle_row(
                    "term-scrollbar",
                    &search::SCROLLBAR,
                    scrollbar,
                    "scrollbar",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::MINIMUM_CONTRAST,
                self.terminal_minimum_contrast_row(minimum_contrast_step(&terminal), ink, cx),
            );

        let mut first = true;
        div()
            .flex()
            .flex_col()
            .child(terminal_section(
                "Font",
                font_card,
                kit::section_gap(&mut first),
                ink,
            ))
            .child(terminal_section(
                "Cursor",
                cursor_card,
                kit::section_gap(&mut first),
                ink,
            ))
            .child(terminal_section(
                "Rendering",
                rendering_card,
                kit::section_gap(&mut first),
                ink,
            ))
    }

    fn terminal_font_family_row(
        &self,
        current_font: String,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let default_font = crate::terminal::element::resolve_font_family(None);
        let trigger_label = if self.font_dropdown_open {
            if self.font_search.is_empty() {
                "Search fonts…".to_string()
            } else {
                format!("{}|", self.font_search)
            }
        } else {
            current_font.clone()
        };
        let trigger_label_color = if self.font_dropdown_open && self.font_search.is_empty() {
            ink.tertiary
        } else {
            ink.primary
        };

        let font_open = self.font_dropdown_open;
        let mut trigger = select_trigger(
            "terminal-font-family-trigger",
            search::FONT_FAMILY.title,
            font_open,
            ui,
        )
        .on_press(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.terminal_dropdown = None;
            this.font_dropdown_open = !font_open;
            this.font_search.clear();
            if this.font_dropdown_open && this.mono_font_names.is_empty() {
                cx.spawn(async move |this, cx| {
                    let fonts = smol::unblock(crate::fonts::load_mono_fonts).await;
                    let _ = this.update(cx, |this, cx| {
                        this.mono_font_names = fonts;
                        cx.notify();
                    });
                })
                .detach();
            }
            this.settings_focus.focus(window, cx);
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(kit::SIZE_SM))
                .text_color(trigger_label_color)
                .truncate()
                .child(trigger_label),
        )
        .child(select_chevron(ui));

        if self.font_dropdown_open {
            let FontMatches {
                default_label,
                default_matches,
                names: filtered,
            } = FontMatches::new(&self.mono_font_names, &default_font, &self.font_search);

            let mut menu = select_menu("terminal-font-dropdown", ui).on_mouse_down_out(
                cx.listener(|this, _, _w, cx| {
                    if this.font_dropdown_open {
                        this.font_dropdown_open = false;
                        this.font_search.clear();
                        cx.notify();
                    }
                }),
            );

            if default_matches {
                menu = menu.option(
                    select_option(
                        ("terminal-font-default", 0usize),
                        default_label.clone(),
                        current_font == default_font,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ink.primary)
                            .child(default_label),
                    ),
                    cx.listener(|this, _: &(), _w, cx| {
                        this.font_dropdown_open = false;
                        this.font_search.clear();
                        this.persist_setting(false, "font_family", Value::Null, cx);
                    }),
                );
            }

            for (i, name) in filtered.iter().enumerate() {
                let name_owned = (*name).to_string();
                let is_current = *name == current_font;
                menu = menu.option(
                    select_option(("terminal-font", i), name.to_string(), is_current, ui)
                        .cursor(CursorStyle::Arrow)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ink.primary)
                                .child(name.to_string()),
                        ),
                    cx.listener(move |this, _: &(), _w, cx| {
                        this.font_dropdown_open = false;
                        this.font_search.clear();
                        this.persist_setting(
                            false,
                            "font_family",
                            Value::String(name_owned.clone()),
                            cx,
                        );
                    }),
                );
            }

            if !default_matches && filtered.is_empty() {
                menu = menu.child(
                    div()
                        .px(px(8.))
                        .py(px(8.))
                        .text_size(px(kit::SIZE_SM))
                        .text_color(ink.tertiary)
                        .child("No matching fonts"),
                );
            }

            trigger = trigger.child(deferred_select_menu(menu));
        }

        kit::row(
            ink,
            None,
            setting_copy(&search::FONT_FAMILY, ink),
            Some(trigger.into_any_element()),
        )
        .into_any_element()
    }

    fn terminal_cursor_color_row(
        &self,
        current_hex: String,
        uses_theme: bool,
        theme_hex: String,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = self.terminal_dropdown == Some(TerminalDropdown::CursorColor);
        let current_color = crate::terminal::view::hsla_from_hex_color(&current_hex)
            .unwrap_or_else(|| hsla_from_u32(0x007aff));
        let theme_color = crate::terminal::view::hsla_from_hex_color(&theme_hex)
            .unwrap_or_else(|| hsla_from_u32(0x007aff));

        let summary = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_none()
                    .size(px(SWATCH_PREVIEW))
                    .rounded(px(4.))
                    .bg(current_color),
            )
            .child(
                div()
                    .font_family(kit::mono_family())
                    .text_size(px(kit::SIZE_SM))
                    .line_height(px(kit::LINE_SM))
                    .text_color(ink.primary)
                    .child(if uses_theme {
                        "Color scheme".to_string()
                    } else {
                        current_hex.clone()
                    }),
            )
            .child(
                svg()
                    .size(px(CHEVRON))
                    .flex_none()
                    .path(if is_open { CHEVRON_UP } else { CHEVRON_DOWN })
                    .text_color(ink.tertiary),
            );

        let head = div()
            .id("term-cursor-color-row")
            .accessible_control(gpui::accesskit::Role::ComboBox, search::CURSOR_COLOR.title)
            .aria_expanded(is_open)
            .cursor(CursorStyle::PointingHand)
            .on_press(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.font_dropdown_open = false;
                this.font_search.clear();
                this.terminal_dropdown = if is_open {
                    None
                } else {
                    Some(TerminalDropdown::CursorColor)
                };
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
            .child(kit::row(
                ink,
                None,
                setting_copy(&search::CURSOR_COLOR, ink),
                Some(summary.into_any_element()),
            ));

        let mut row = div().flex().flex_col().child(head);
        if !is_open {
            return row.into_any_element();
        }

        let mut swatches = div().flex().flex_row().flex_wrap().gap(px(SWATCH_GAP));
        for (index, &hex) in CURSOR_COLOR_SWATCHES.iter().enumerate() {
            let hex_string = hex_string_from_u32(hex);
            let selected = !uses_theme && hex_string == current_hex;
            let value = hex_string.clone();
            swatches = swatches.child(
                div()
                    .id(("term-cursor-color", index))
                    .flex_none()
                    .size(px(SWATCH))
                    .p(px(SWATCH_RING_GAP))
                    .rounded(px(SWATCH_RADIUS + SWATCH_RING_GAP + SWATCH_RING))
                    .border_2()
                    .border_color(if selected {
                        ink.primary
                    } else {
                        gpui::transparent_black()
                    })
                    .cursor(CursorStyle::PointingHand)
                    .when(!selected, |swatch| {
                        swatch.hover(move |style| style.border_color(ink.tertiary))
                    })
                    .accessible_control(gpui::accesskit::Role::RadioButton, hex_string.clone())
                    .aria_toggled(if selected {
                        gpui::accesskit::Toggled::True
                    } else {
                        gpui::accesskit::Toggled::False
                    })
                    .on_activate(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(
                            true,
                            "cursor_color",
                            Value::String(value.clone()),
                            cx,
                        );
                    }))
                    .child(
                        div()
                            .size_full()
                            .rounded(px(SWATCH_RADIUS))
                            .bg(hsla_from_u32(hex)),
                    ),
            );
        }

        let scheme = div()
            .id("term-cursor-color-theme")
            .flex_none()
            .h(px(SCHEME_HEIGHT))
            .pl(px(SCHEME_PADDING_X - 2.))
            .pr(px(SCHEME_PADDING_X))
            .rounded(px(SCHEME_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .bg(ink.well)
            .cursor(CursorStyle::PointingHand)
            .hover(move |style| style.bg(ink.well_hover))
            .active(move |style| style.bg(ink.well_pressed))
            .accessible_control(gpui::accesskit::Role::Button, "Use color scheme color")
            .on_activate(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.persist_setting(true, "cursor_color", Value::Null, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .size(px(SWATCH_PREVIEW))
                    .rounded(px(4.))
                    .bg(theme_color),
            )
            .child(
                div()
                    .text_size(px(kit::SIZE_SM))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ink.primary)
                    .child("Use color scheme color"),
            )
            .when(uses_theme, |button| {
                button.child(
                    svg()
                        .size(px(CHEVRON))
                        .flex_none()
                        .path(TICK_ICON)
                        .text_color(ink.primary),
                )
            });

        row = row.child(kit::rule(ink)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .px(px(kit::ROW_INSET))
                .py(px(14.))
                .child(swatches)
                .child(div().flex().child(scheme)),
        );
        row.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_enum_row(
        &self,
        which: TerminalDropdown,
        copy: &search::SettingCopy,
        current_label: String,
        options: Vec<(String, Value, bool)>,
        config_key: &'static str,
        nested: bool,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = self.terminal_dropdown == Some(which);

        let mut trigger = select_trigger(
            SharedString::from(format!("term-dd-{config_key}")),
            copy.title,
            is_open,
            ui,
        )
        .on_press(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.font_dropdown_open = false;
            this.font_search.clear();
            this.terminal_dropdown = if is_open { None } else { Some(which) };
            this.settings_focus.focus(window, cx);
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(kit::SIZE_SM))
                .text_color(ink.primary)
                .truncate()
                .child(current_label),
        )
        .child(select_chevron(ui));

        if is_open {
            let mut menu =
                select_menu(SharedString::from(format!("term-dd-list-{config_key}")), ui)
                    .on_mouse_down_out(cx.listener(move |this, _, _w, cx| {
                        if this.terminal_dropdown == Some(which) {
                            this.terminal_dropdown = None;
                            cx.notify();
                        }
                    }));
            for (i, (label, value, selected)) in options.into_iter().enumerate() {
                let item = select_option((config_key, i), label.clone(), selected, ui)
                    .cursor(CursorStyle::Arrow)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ink.primary)
                            .child(label),
                    );
                menu = menu.option(
                    item,
                    cx.listener(move |this, _: &(), _w, cx| {
                        this.terminal_dropdown = None;
                        this.persist_setting(nested, config_key, value.clone(), cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        kit::row(
            ink,
            None,
            setting_copy(copy, ink),
            Some(trigger.into_any_element()),
        )
        .into_any_element()
    }

    fn terminal_toggle_row(
        &self,
        id: &'static str,
        copy: &'static search::SettingCopy,
        current: bool,
        config_key: &'static str,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        kit::row(
            ink,
            None,
            setting_copy(copy, ink),
            Some(
                kit::switch(id, copy.title, current, ink)
                    .on_activate(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(true, config_key, Value::Bool(!current), cx);
                    }))
                    .into_any_element(),
            ),
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_number_row(
        &self,
        id: &'static str,
        copy: &'static search::SettingCopy,
        number: NumberStep,
        config_key: &'static str,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let NumberStep {
            value,
            min,
            max,
            step,
            decimals,
        } = number;
        let factor = 10f64.powi(decimals as i32);
        let round = move |v: f64| (v.clamp(min, max) * factor).round() / factor;
        let can_decrease = value > min + f64::EPSILON;
        let can_increase = value < max - f64::EPSILON;
        let decrease = kit::step_button(
            SharedString::from(format!("{id}-dec")),
            format!("Decrease {}", copy.title.to_lowercase()),
            REMOVE_ICON,
            can_decrease,
            ink,
        )
        .when(can_decrease, |button| {
            button.on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.persist_setting(false, config_key, json!(round(value - step)), cx);
            }))
        });
        let increase = kit::step_button(
            SharedString::from(format!("{id}-inc")),
            format!("Increase {}", copy.title.to_lowercase()),
            ADD_ICON,
            can_increase,
            ink,
        )
        .when(can_increase, |button| {
            button.on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.persist_setting(false, config_key, json!(round(value + step)), cx);
            }))
        });
        kit::row(
            ink,
            None,
            setting_copy(copy, ink),
            Some(
                kit::stepper(ink, format!("{value:.decimals$}"), decrease, increase)
                    .into_any_element(),
            ),
        )
        .into_any_element()
    }

    fn terminal_minimum_contrast_row(
        &self,
        step: usize,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let last = MINIMUM_CONTRAST_STEPS.len() - 1;
        let can_decrease = step > 0;
        let can_increase = step < last;
        let label = MINIMUM_CONTRAST_STEPS
            .get(step)
            .map_or_else(String::new, |(label, _)| (*label).to_string());
        let previous = minimum_contrast_setting(step.saturating_sub(1));
        let next = minimum_contrast_setting((step + 1).min(last));
        let decrease = kit::step_button(
            "term-minimum-contrast-dec",
            "Lower the minimum contrast",
            REMOVE_ICON,
            can_decrease,
            ink,
        )
        .when(can_decrease, |button| {
            button.on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.persist_setting(true, "minimum_contrast", previous.clone(), cx);
            }))
        });
        let increase = kit::step_button(
            "term-minimum-contrast-inc",
            "Raise the minimum contrast",
            ADD_ICON,
            can_increase,
            ink,
        )
        .when(can_increase, |button| {
            button.on_activate(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.persist_setting(true, "minimum_contrast", next.clone(), cx);
            }))
        });
        kit::row(
            ink,
            None,
            setting_copy(&search::MINIMUM_CONTRAST, ink),
            Some(kit::stepper(ink, label, decrease, increase).into_any_element()),
        )
        .into_any_element()
    }
}

#[derive(Clone, Copy)]
struct NumberStep {
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    decimals: usize,
}

fn setting_copy(copy: &search::SettingCopy, ink: Ink) -> Text {
    Text::new(copy.title).line(kit::line(ink, copy.description))
}

fn terminal_section(title: &'static str, card: SearchCard, top_gap: f32, ink: Ink) -> AnyElement {
    let (matched, rows) = card.into_rows();
    Block::new(title)
        .top_gap(top_gap)
        .found(matched)
        .child(kit::section(ink, title, None, kit::search_group(ink, rows)))
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_search_ignores_the_newline_and_tab_macos_sends() {
        assert!(!font_search_accepts("\n"));
        assert!(!font_search_accepts("\r"));
        assert!(!font_search_accepts("\t"));
        assert!(!font_search_accepts(""));
        assert!(font_search_accepts("J"));
        assert!(font_search_accepts(" "));
    }

    #[test]
    fn enter_picks_the_first_font_the_filter_shows() {
        let names = vec![
            "Fira Code".to_string(),
            "JetBrains Mono".to_string(),
            "JuliaMono".to_string(),
        ];
        let matches = FontMatches::new(&names, "JetBrains Mono", "");
        assert_eq!(matches.first(), Some(FontPick::Default));
        assert_eq!(matches.names, ["Fira Code", "JuliaMono"]);

        let matches = FontMatches::new(&names, "JetBrains Mono", "ju");
        assert_eq!(matches.first(), Some(FontPick::Named("JuliaMono")));

        let matches = FontMatches::new(&names, "JetBrains Mono", "jetbrains");
        assert_eq!(matches.first(), Some(FontPick::Default));

        let matches = FontMatches::new(&names, "JetBrains Mono", "zzz");
        assert_eq!(matches.first(), None);
    }

    fn with_contrast(minimum_contrast: Option<f32>) -> TerminalConfig {
        TerminalConfig {
            minimum_contrast,
            ..TerminalConfig::default()
        }
    }

    #[test]
    fn the_ladder_starts_on_auto_when_the_key_is_unset_or_unusable() {
        assert_eq!(minimum_contrast_step(&with_contrast(None)), 0);
        assert_eq!(minimum_contrast_step(&with_contrast(Some(f32::NAN))), 0);
        assert_eq!(minimum_contrast_step(&with_contrast(Some(-5.0))), 0);
        assert_eq!(minimum_contrast_setting(0), Value::Null);
    }

    #[test]
    fn an_explicit_value_lands_on_the_nearest_step_without_rewriting_it() {
        assert_eq!(minimum_contrast_step(&with_contrast(Some(0.0))), 1);
        assert_eq!(minimum_contrast_step(&with_contrast(Some(72.5))), 4);
        assert_eq!(MINIMUM_CONTRAST_STEPS[4].0, "75");
        assert_eq!(minimum_contrast_step(&with_contrast(Some(60.0))), 3);
        assert_eq!(minimum_contrast_step(&with_contrast(Some(120.0))), 5);
        assert_eq!(with_contrast(Some(72.5)).minimum_contrast().lc(), 72.5);
    }

    #[test]
    fn every_step_persists_the_value_the_row_advertises() {
        let persisted: Vec<Value> = (0..MINIMUM_CONTRAST_STEPS.len())
            .map(minimum_contrast_setting)
            .collect();
        assert_eq!(
            persisted,
            vec![
                Value::Null,
                json!(0.0),
                json!(45.0),
                json!(60.0),
                json!(75.0),
                json!(90.0)
            ]
        );
        for (index, value) in persisted.iter().enumerate() {
            let config: TerminalConfig = serde_json::from_value(json!({
                "minimum_contrast": value.clone(),
            }))
            .expect("terminal config round trip");
            assert_eq!(minimum_contrast_step(&config), index);
        }
    }

    #[test]
    fn turning_the_correction_off_resolves_to_zero_while_auto_resolves_to_sixty() {
        let off: TerminalConfig =
            serde_json::from_value(json!({ "minimum_contrast": minimum_contrast_setting(1) }))
                .expect("terminal config round trip");
        assert_eq!(off.resolved_minimum_contrast(), 0.0);
        assert_eq!(
            with_contrast(None).resolved_minimum_contrast(),
            TerminalConfig::DEFAULT_MINIMUM_CONTRAST
        );
    }
}

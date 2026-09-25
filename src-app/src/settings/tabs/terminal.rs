use gpui::{
    ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Rgba, SharedString, Styled, div, prelude::*, px,
};
use serde_json::{Value, json};

use paneflow_config::schema::{CursorShapeConfig, MinimumContrast, TerminalConfig};

use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, deferred_select_menu, hairline, menu_row, section_header,
    select_chevron, select_menu, select_trigger_with_hover, setting_text, toggle_pill,
};
use crate::settings::search::{self, Block, SearchCard};
use crate::ui_primitives::AnimatedHoverExt;

use crate::{PaneFlowApp, TerminalDropdown};

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

fn hex_string_from_u32(hex: u32) -> String {
    format!("#{hex:06X}")
}

fn hsla_from_u32(hex: u32) -> Hsla {
    Hsla::from(gpui::rgb(hex))
}

fn lighter_control_hover(base: Hsla) -> Hsla {
    Hsla {
        l: (base.l + 0.045).min(1.0),
        ..base
    }
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
    pub(crate) fn render_terminal_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let config = &self.cached_config;
        let ui = crate::theme::ui_colors();
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

        let shape_label = match shape {
            CursorShapeConfig::Vintage => "Vintage (_▂)",
            CursorShapeConfig::Block => "Filled box (█)",
            CursorShapeConfig::Beam => "Bar (|)",
            CursorShapeConfig::Underline => "Underline (_)",
            CursorShapeConfig::DoubleUnderline => "Double underline (‿)",
            CursorShapeConfig::Hollow => "Empty box (□)",
        };
        let font_weight_label = FONT_WEIGHT_OPTIONS
            .iter()
            .find_map(|(label, key)| (*key == font_weight_key).then_some(*label))
            .unwrap_or("Normal");

        let shape_opts: Vec<(String, Value, bool)> = vec![
            (
                "Vintage (_▂)".into(),
                json!("vintage"),
                shape == CursorShapeConfig::Vintage,
            ),
            (
                "Bar (|)".into(),
                json!("beam"),
                shape == CursorShapeConfig::Beam,
            ),
            (
                "Underline (_)".into(),
                json!("underline"),
                shape == CursorShapeConfig::Underline,
            ),
            (
                "Double underline (‿)".into(),
                json!("double_underline"),
                shape == CursorShapeConfig::DoubleUnderline,
            ),
            (
                "Filled box (█)".into(),
                json!("block"),
                shape == CursorShapeConfig::Block,
            ),
            (
                "Empty box (□)".into(),
                json!("hollow"),
                shape == CursorShapeConfig::Hollow,
            ),
        ];
        let font_weight_opts: Vec<(String, Value, bool)> = FONT_WEIGHT_OPTIONS
            .iter()
            .map(|(label, key)| ((*label).to_string(), json!(*key), *key == font_weight_key))
            .collect();

        let cursor_card = SearchCard::new(ui)
            .row(
                &search::CURSOR_SHAPE,
                self.terminal_enum_row(
                    TerminalDropdown::CursorShape,
                    search::CURSOR_SHAPE.title,
                    search::CURSOR_SHAPE.description,
                    shape_label.to_string(),
                    shape_opts,
                    "cursor_shape",
                    true,
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
                    ui,
                    cx,
                ),
            );

        let display_card = SearchCard::new(ui)
            .row(
                &search::FONT_FAMILY,
                self.terminal_font_family_row(current_font, ui, cx),
            )
            .row(
                &search::FONT_SIZE,
                self.settings_stepper_row(
                    "term-font-size",
                    search::FONT_SIZE.title,
                    search::FONT_SIZE.description,
                    font_size,
                    8.0,
                    32.0,
                    1.0,
                    0,
                    "font_size",
                    ui,
                    cx,
                ),
            )
            .row(
                &search::LINE_HEIGHT,
                self.settings_stepper_row(
                    "term-line-height",
                    search::LINE_HEIGHT.title,
                    search::LINE_HEIGHT.description,
                    line_height,
                    1.0,
                    2.5,
                    0.1,
                    1,
                    "line_height",
                    ui,
                    cx,
                ),
            )
            .row(
                &search::CELL_WIDTH,
                self.settings_stepper_row(
                    "term-cell-width",
                    search::CELL_WIDTH.title,
                    search::CELL_WIDTH.description,
                    cell_width,
                    0.3,
                    2.0,
                    0.1,
                    1,
                    "cell_width",
                    ui,
                    cx,
                ),
            )
            .row(
                &search::FONT_WEIGHT,
                self.terminal_enum_row(
                    TerminalDropdown::FontWeight,
                    search::FONT_WEIGHT.title,
                    search::FONT_WEIGHT.description,
                    font_weight_label.to_string(),
                    font_weight_opts,
                    "font_weight",
                    false,
                    ui,
                    cx,
                ),
            )
            .row(
                &search::INTEGRATED_GLYPHS,
                self.terminal_toggle_row(
                    "term-integrated-glyphs",
                    search::INTEGRATED_GLYPHS.title,
                    search::INTEGRATED_GLYPHS.description,
                    integrated_glyphs,
                    "integrated_glyphs",
                    true,
                    ui,
                    cx,
                ),
            )
            .row(
                &search::COLOR_EMOJI,
                self.terminal_toggle_row(
                    "term-color-emoji",
                    search::COLOR_EMOJI.title,
                    search::COLOR_EMOJI.description,
                    color_emoji,
                    "color_emoji",
                    true,
                    ui,
                    cx,
                ),
            )
            .row(
                &search::SCROLLBAR,
                self.terminal_toggle_row(
                    "term-scrollbar",
                    search::SCROLLBAR.title,
                    search::SCROLLBAR.description,
                    scrollbar,
                    "scrollbar",
                    true,
                    ui,
                    cx,
                ),
            )
            .row(
                &search::MINIMUM_CONTRAST,
                self.terminal_minimum_contrast_row(minimum_contrast_step(&terminal), ui, cx),
            );

        let content = div()
            .flex()
            .flex_col()
            .child(
                Block::new("Cursor")
                    .gap(20.)
                    .child(section_header(ui, "Cursor"))
                    .card(cursor_card)
                    .finish(),
            )
            .child(
                Block::new("Display")
                    .top_gap(20.)
                    .gap(20.)
                    .child(section_header(ui, "Display"))
                    .card(display_card)
                    .finish(),
            );

        #[cfg(target_os = "windows")]
        let content = {
            let material_card = SearchCard::new(ui).row(
                &search::ACRYLIC_MATERIAL,
                self.terminal_toggle_row(
                    "term-windows-terminal-material",
                    search::ACRYLIC_MATERIAL.title,
                    search::ACRYLIC_MATERIAL.description,
                    config.windows_terminal_material_enabled(),
                    "windows_terminal_material",
                    false,
                    ui,
                    cx,
                ),
            );

            content.child(
                Block::new("Window")
                    .top_gap(20.)
                    .gap(20.)
                    .child(section_header(ui, "Window"))
                    .card(material_card)
                    .finish(),
            )
        };

        content
    }

    fn terminal_font_family_row(
        &self,
        current_font: String,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
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
            ui.muted
        } else {
            ui.text
        };

        let font_open = self.font_dropdown_open;
        let trigger_hover_bg = lighter_control_hover(ui.subtle);
        let mut trigger =
            select_trigger_with_hover("terminal-font-family-trigger", ui, trigger_hover_bg)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
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
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(trigger_label_color)
                        .truncate()
                        .child(trigger_label),
                )
                .child(select_chevron(ui));

        if self.font_dropdown_open {
            let search = self.font_search.to_lowercase();
            let default_label = format!("PaneFlow default - {default_font}");
            let default_matches =
                search.is_empty() || default_label.to_lowercase().contains(&search);
            let filtered: Vec<&String> = self
                .mono_font_names
                .iter()
                .filter(|name| {
                    name.as_str() != default_font.as_str()
                        && (search.is_empty() || name.to_lowercase().contains(&search))
                })
                .collect();

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
                menu = menu.child(
                    menu_row(
                        ("terminal-font-default", 0usize),
                        current_font == default_font,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.font_dropdown_open = false;
                        this.font_search.clear();
                        this.persist_setting(false, "font_family", Value::Null, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ui.text)
                            .child(default_label),
                    ),
                );
            }

            for (i, name) in filtered.iter().enumerate() {
                let name_owned = (*name).clone();
                let is_current = **name == current_font;
                menu = menu.child(
                    menu_row(("terminal-font", i), is_current, ui)
                        .cursor(CursorStyle::Arrow)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.font_dropdown_open = false;
                            this.font_search.clear();
                            this.persist_setting(
                                false,
                                "font_family",
                                Value::String(name_owned.clone()),
                                cx,
                            );
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child((*name).clone()),
                        ),
                );
            }

            if !default_matches && filtered.is_empty() {
                menu = menu.child(
                    div()
                        .px(px(8.))
                        .py(px(8.))
                        .text_size(px(12.))
                        .text_color(ui.muted)
                        .child("No matching fonts"),
                );
            }

            trigger = trigger.child(deferred_select_menu(menu));
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(
                ui,
                search::FONT_FAMILY.title,
                search::FONT_FAMILY.description,
            ))
            .child(div().flex_shrink_0().child(trigger))
            .into_any_element()
    }

    fn terminal_cursor_color_row(
        &self,
        current_hex: String,
        uses_theme: bool,
        theme_hex: String,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.terminal_dropdown == Some(TerminalDropdown::CursorColor);
        let current_color = crate::terminal::view::hsla_from_hex_color(&current_hex)
            .unwrap_or_else(|| hsla_from_u32(0x007aff));
        let theme_color = crate::terminal::view::hsla_from_hex_color(&theme_hex)
            .unwrap_or_else(|| hsla_from_u32(0x007aff));

        let top = div()
            .id("term-cursor-color-row")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
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
                }),
            )
            .child(setting_text(
                ui,
                search::CURSOR_COLOR.title,
                search::CURSOR_COLOR.description,
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(12.))
                            .h(px(12.))
                            .rounded(px(3.))
                            .bg(current_color),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(ui.text)
                            .child(current_hex.clone()),
                    )
                    .child(select_chevron(ui)),
            );

        let mut row = div().flex().flex_col().child(top);

        if is_open {
            let mut swatch_grid = div().flex().flex_col().gap(px(4.));
            for (row_idx, chunk) in CURSOR_COLOR_SWATCHES.chunks(4).enumerate() {
                let mut swatch_row = div().flex().flex_row().gap(px(4.));
                for (col_idx, &hex) in chunk.iter().enumerate() {
                    let hex_string = hex_string_from_u32(hex);
                    let selected = !uses_theme && hex_string == current_hex;
                    let resting_opacity = if selected { 1.0 } else { 0.92 };
                    let value = hex_string.clone();
                    swatch_row = swatch_row.child(
                        div()
                            .id(SharedString::from(format!(
                                "term-cursor-color-{row_idx}-{col_idx}"
                            )))
                            .w(px(32.))
                            .h(px(32.))
                            .rounded(px(6.))
                            .bg(hsla_from_u32(hex))
                            .opacity(resting_opacity)
                            .animated_hover(move |style, delta| {
                                style.opacity(resting_opacity + (1.0 - resting_opacity) * delta);
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.persist_setting(
                                    true,
                                    "cursor_color",
                                    Value::String(value.clone()),
                                    cx,
                                );
                            })),
                    );
                }
                swatch_grid = swatch_grid.child(swatch_row);
            }

            let scheme_bg = if uses_theme {
                Hsla::from(gpui::rgb(0x2fd7f2))
            } else {
                ui.subtle
            };
            let scheme_text = if uses_theme { gpui::black() } else { ui.text };
            let scheme_hover_bg = if uses_theme {
                scheme_bg
            } else {
                lighter_control_hover(ui.subtle)
            };
            let controls = div().flex().flex_col().gap(px(8.)).child(
                div()
                    .id("term-cursor-color-theme")
                    .h(px(32.))
                    .min_w(px(200.))
                    .px(px(10.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .bg(scheme_bg)
                    .animated_hover_bg(scheme_bg, scheme_hover_bg)
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(true, "cursor_color", Value::Null, cx);
                    }))
                    .child(div().w(px(16.)).h(px(16.)).rounded(px(4.)).bg(theme_color))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(scheme_text)
                            .child("Use color scheme color"),
                    ),
            );

            row = row.child(hairline(ui)).child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(16.))
                    .p(px(12.))
                    .child(swatch_grid)
                    .child(controls),
            );
        }

        row.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_enum_row(
        &self,
        which: TerminalDropdown,
        title: &'static str,
        description: &'static str,
        current_label: String,
        options: Vec<(String, Value, bool)>,
        config_key: &'static str,
        nested: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.terminal_dropdown == Some(which);

        let trigger_hover_bg = lighter_control_hover(ui.subtle);
        let mut trigger = select_trigger_with_hover(
            SharedString::from(format!("term-dd-{config_key}")),
            ui,
            trigger_hover_bg,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.font_dropdown_open = false;
                this.font_search.clear();
                this.terminal_dropdown = if is_open { None } else { Some(which) };
                this.settings_focus.focus(window, cx);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(ui.text)
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
                let value_for_click = value;
                let item = menu_row((config_key, i), selected, ui)
                    .cursor(CursorStyle::Arrow)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.terminal_dropdown = None;
                        this.persist_setting(nested, config_key, value_for_click.clone(), cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ui.text)
                            .child(label),
                    );
                menu = menu.child(item);
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(ui, title, description))
            .child(div().flex_shrink_0().child(trigger))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal_toggle_row(
        &self,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        current: bool,
        config_key: &'static str,
        nested: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let target_value = !current;

        div()
            .id(SharedString::from(format!("{id}-row")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(ui, title, description))
            .child(
                div()
                    .id(SharedString::from(id))
                    .flex_shrink_0()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(nested, config_key, Value::Bool(target_value), cx);
                    }))
                    .child(toggle_pill(current, ui)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settings_stepper_row(
        &self,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        decimals: usize,
        config_key: &'static str,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let factor = 10f64.powi(decimals as i32);
        let round = move |v: f64| (v.clamp(min, max) * factor).round() / factor;
        let at_min = value <= min + f64::EPSILON;
        let at_max = value >= max - f64::EPSILON;

        let dec = cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.persist_setting(false, config_key, json!(round(value - step)), cx);
        });
        let inc = cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.persist_setting(false, config_key, json!(round(value + step)), cx);
        });

        stepper_frame(
            ui,
            title,
            description,
            format!("{value:.decimals$}"),
            stepper_button(format!("{id}-dec"), "−", at_min, ui)
                .when(!at_min, move |b| b.on_click(dec)),
            stepper_button(format!("{id}-inc"), "+", at_max, ui)
                .when(!at_max, move |b| b.on_click(inc)),
        )
    }

    fn terminal_minimum_contrast_row(
        &self,
        step: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = "term-minimum-contrast";
        let at_min = step == 0;
        let at_max = step + 1 >= MINIMUM_CONTRAST_STEPS.len();
        let label = MINIMUM_CONTRAST_STEPS
            .get(step)
            .map_or_else(String::new, |(label, _)| (*label).to_string());

        let previous = minimum_contrast_setting(step.saturating_sub(1));
        let next = minimum_contrast_setting((step + 1).min(MINIMUM_CONTRAST_STEPS.len() - 1));
        let dec = cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.persist_setting(true, "minimum_contrast", previous.clone(), cx);
        });
        let inc = cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.persist_setting(true, "minimum_contrast", next.clone(), cx);
        });

        stepper_frame(
            ui,
            search::MINIMUM_CONTRAST.title,
            search::MINIMUM_CONTRAST.description,
            label,
            stepper_button(format!("{id}-dec"), "−", at_min, ui)
                .when(!at_min, move |b| b.on_click(dec)),
            stepper_button(format!("{id}-inc"), "+", at_max, ui)
                .when(!at_max, move |b| b.on_click(inc)),
        )
        .into_any_element()
    }
}

fn stepper_button(
    id: String,
    glyph: &'static str,
    disabled: bool,
    ui: crate::theme::UiColors,
) -> crate::ui_primitives::AnimatedHover {
    let hover_bg = if disabled {
        ui.subtle
    } else {
        lighter_control_hover(ui.subtle)
    };
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .justify_center()
        .w(px(24.))
        .h(px(24.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .text_size(px(15.))
        .text_color(if disabled { ui.muted } else { ui.text })
        .animated_hover_bg(ui.subtle, hover_bg)
        .child(glyph)
}

fn stepper_frame(
    ui: crate::theme::UiColors,
    title: &'static str,
    description: &'static str,
    label: String,
    decrement: crate::ui_primitives::AnimatedHover,
    increment: crate::ui_primitives::AnimatedHover,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(12.))
        .py(px(10.))
        .child(setting_text(ui, title, description))
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(decrement)
                .child(
                    div()
                        .w(px(48.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(ui.text)
                        .child(label),
                )
                .child(increment),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

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

//! "Terminal" settings tab (US-016) - the small set of terminal preferences
//! worth keeping in the primary Settings UI: cursor shape, font
//! family, font size, font weight, line height, cell width, and Windows terminal
//! material.
//!
//! Controls map to config like so:
//! - **cursor_shape** -> enum/preset dropdown persisted into the `terminal`
//!   block via [`config_writer::save_terminal_field`].
//! - **cursor_color** -> swatch picker persisted into the `terminal` block via
//!   [`config_writer::save_terminal_field`].
//! - **font_family** -> searchable monospace-font dropdown, persisted as a
//!   top-level field via [`config_writer::save_config_value`].
//! - **font_size / line_height / cell_width** -> `−`/`+` steppers that clamp
//!   by construction, persisted as top-level fields via
//!   [`config_writer::save_config_value`].
//! - **font_weight** -> preset dropdown persisted as a top-level field via
//!   [`config_writer::save_config_value`].
//! - **integrated_glyphs** -> toggle persisted into the `terminal` block via
//!   [`config_writer::save_terminal_field`].
//! - **color_emoji** -> toggle persisted into the `terminal` block via
//!   [`config_writer::save_terminal_field`].
//! - **windows_terminal_material** -> Windows-only top-level toggle persisted
//!   via [`config_writer::save_config_value`].
//!
//! Other advanced terminal knobs remain supported in `paneflow.json`, but are
//! intentionally not mirrored here to keep Settings focused.

use gpui::{
    ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Rgba, SharedString, Styled, div, prelude::*, px,
};
use serde_json::{Value, json};

use paneflow_config::schema::{CursorShapeConfig, normalize_hex_color};

use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, deferred_select_menu, hairline, section_header, select_chevron,
    select_item, select_menu, select_trigger_with_hover, setting_card, setting_text, toggle_pill,
};

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
        // US-016: read the cached config (no per-frame `load_config()`).
        let config = &self.cached_config;
        let ui = crate::theme::ui_colors();
        let terminal = config.terminal.clone().unwrap_or_default();

        // ── current values ──────────────────────────────────────────────
        let shape = terminal.cursor_shape.unwrap_or_default();
        let integrated_glyphs = terminal.resolved_integrated_glyphs();
        let color_emoji = terminal.resolved_color_emoji();
        let configured_cursor_color = terminal
            .cursor_color
            .as_deref()
            .and_then(normalize_hex_color);
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

        // ── Cursor ──────────────────────────────────────────────────────
        let cursor_card = setting_card(ui)
            .child(self.terminal_enum_row(
                TerminalDropdown::CursorShape,
                "Cursor shape",
                "Default shape before an application overrides it. Takes effect on the next new terminal.",
                shape_label.to_string(),
                shape_opts,
                "cursor_shape",
                true,
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.terminal_cursor_color_row(
                cursor_color_hex,
                cursor_uses_theme,
                theme_cursor_hex,
                ui,
                cx,
            ));

        // ── Display ─────────────────────────────────────────────────────
        let display_card = setting_card(ui)
            .child(self.terminal_font_family_row(current_font, ui, cx))
            .child(hairline(ui))
            .child(self.terminal_stepper_row(
                "term-font-size",
                "Font size",
                "Terminal font size in points (8-32). Hot-reloads.",
                font_size,
                8.0,
                32.0,
                1.0,
                0,
                "font_size",
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.terminal_stepper_row(
                "term-line-height",
                "Line height",
                "Terminal line-height multiplier (1.0-2.5). Hot-reloads.",
                line_height,
                1.0,
                2.5,
                0.1,
                1,
                "line_height",
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.terminal_stepper_row(
                "term-cell-width",
                "Cell width",
                "Terminal cell-width multiplier (0.3-2.0). Hot-reloads.",
                cell_width,
                0.3,
                2.0,
                0.1,
                1,
                "cell_width",
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.terminal_enum_row(
                TerminalDropdown::FontWeight,
                "Font weight",
                "Controls terminal stroke thickness. Hot-reloads.",
                font_weight_label.to_string(),
                font_weight_opts,
                "font_weight",
                false,
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.terminal_toggle_row(
                "term-integrated-glyphs",
                "Integrated glyphs",
                "Draw block elements with Paneflow's built-in renderer instead of the font glyph.",
                integrated_glyphs,
                "integrated_glyphs",
                true,
                ui,
                cx,
            ))
            .child(hairline(ui))
            .child(self.terminal_toggle_row(
                "term-color-emoji",
                "Color emoji",
                "Render emoji in color when the platform font stack supports it.",
                color_emoji,
                "color_emoji",
                true,
                ui,
                cx,
            ));

        let content = div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(section_header(ui, "Cursor"))
            .child(cursor_card)
            .child(section_header(ui, "Display"))
            .child(display_card);

        #[cfg(target_os = "windows")]
        let content = {
            let material_card = setting_card(ui).child(self.terminal_toggle_row(
                "term-windows-terminal-material",
                "Enable acrylic material",
                "Applies a translucent texture behind the terminal window.",
                config.windows_terminal_material_enabled(),
                "windows_terminal_material",
                false,
                ui,
                cx,
            ));

            content
                .child(section_header(ui, "Window"))
                .child(material_card)
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
                    select_item(
                        ("terminal-font-default", 0usize),
                        current_font == default_font,
                        ui,
                    )
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
                    select_item(("terminal-font", i), is_current, ui)
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
                "Font family",
                "Choose the monospace font used by every terminal. Hot-reloads.",
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
            .cursor(CursorStyle::PointingHand)
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
                "Cursor color",
                "Overrides the cursor color from the active color scheme.",
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
                            .opacity(if selected { 1.0 } else { 0.92 })
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.opacity(1.0))
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
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |s| s.bg(scheme_hover_bg))
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

    /// One settings row: label/description on the left, a Codex-style select on
    /// the right (shared `components::select_*` primitives). `options` are
    /// `(label, json_value_to_write, is_selected)`. `nested` routes the write to
    /// the `terminal` block vs. a top-level key.
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

        // Decide open/close from the render-time `is_open` snapshot, not the live
        // state: the menu's `on_mouse_down_out` fires on the same press and may
        // have already cleared it, so a live toggle would re-open (see general.rs).
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
                let item = select_item((config_key, i), selected, ui)
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
                    .cursor(CursorStyle::PointingHand)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(nested, config_key, Value::Bool(target_value), cx);
                    }))
                    .child(toggle_pill(current, ui)),
            )
            .into_any_element()
    }

    /// A `−`/`+` numeric stepper row for a top-level float field. The value is
    /// clamped to `[min, max]` on every step and rounded to `decimals` places
    /// before being written, so it can never go out of range and never writes
    /// a float-precision-noisy value.
    #[allow(clippy::too_many_arguments)]
    fn terminal_stepper_row(
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
            // US-016: cache-mutate + notify + off-thread persist.
            this.persist_setting(false, config_key, json!(round(value - step)), cx);
        });
        let inc = cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.persist_setting(false, config_key, json!(round(value + step)), cx);
        });

        let button = |btn_id: String, glyph: &'static str, disabled: bool| {
            let hover_bg = lighter_control_hover(ui.subtle);
            div()
                .id(SharedString::from(btn_id))
                .flex()
                .items_center()
                .justify_center()
                .w(px(24.))
                .h(px(24.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.subtle)
                .text_size(px(15.))
                .text_color(if disabled { ui.muted } else { ui.text })
                .when(!disabled, |d| {
                    d.cursor(CursorStyle::PointingHand)
                        .hover(move |s| s.bg(hover_bg))
                })
                .child(glyph)
        };

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
                    .child(
                        button(format!("{id}-dec"), "−", at_min)
                            .when(!at_min, move |b| b.on_click(dec)),
                    )
                    .child(
                        div()
                            .w(px(48.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(ui.text)
                            .child(format!("{value:.decimals$}")),
                    )
                    .child(
                        button(format!("{id}-inc"), "+", at_max)
                            .when(!at_max, move |b| b.on_click(inc)),
                    ),
            )
    }
}

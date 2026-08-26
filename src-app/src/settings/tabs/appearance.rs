//! "Appearance" settings tab - theme mode, presets, and appearance preferences.

use gpui::{
    ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, Styled, div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{
    deferred_select_menu, secondary_button, section_header, section_header_with_action,
    select_chevron, select_item, select_menu, select_trigger, setting_card, setting_text,
    toggle_pill, with_alpha,
};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

/// Height of the three Light/Dark/System mockup tiles. Sized so a 700px-wide
/// settings column (three `flex_1` tiles, 12px gutters) keeps a ~1.55:1 tile.
const THEME_MODE_TILE_HEIGHT: f32 = 134.;
const THEME_MODE_TILE_RADIUS: f32 = 10.;
/// Ring width of a mode tile (`border_2`). The mockup inside rounds its own
/// full-bleed fills to the tile radius minus this, because GPUI's
/// `overflow_hidden` masks children to a plain *rectangle*: a corner radius on
/// the frame styles the frame's own quad only, and an opaque child painted
/// edge-to-edge squares the corners straight back off. Visible on the light
/// halves, where the fill contrasts with the page; the dark tile only hid it.
const THEME_MODE_TILE_BORDER: f32 = 2.;
const THEME_PREVIEW_CORNER_RADIUS: f32 = 8.;

/// Preview row geometry, mirroring `DiffElement`'s split-cell layout: a 4px
/// hunk bar, 4px of pad, then the line-number gutter. The panel floors its
/// gutter at 36px so five-digit line numbers fit; a five-line sample needs one
/// digit, so the preview narrows it and keeps the rest identical.
const PREVIEW_BAR_W: f32 = 4.;
const PREVIEW_BAR_PAD: f32 = 4.;
const PREVIEW_GUTTER_W: f32 = 24.;
/// Right padding inside the gutter (number -> code gap), `element.rs::NUM_GAP`.
const PREVIEW_NUM_GAP: f32 = 6.;
/// Rows 2..4 of the five-line sample: the changed block.
const PREVIEW_CHANGED: std::ops::RangeInclusive<usize> = 1..=3;

/// Which slice of the window mockup a tile paints. `System` splits down the
/// middle: a light `Left` half against a dark `Right` half, the way Codex
/// renders its system tile.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MockupHalf {
    Full,
    Left,
    Right,
}

impl PaneFlowApp {
    pub(crate) fn render_appearance_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let reset_btn = secondary_button(
            "reset-theme",
            "Reset to default",
            ui,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.reset_theme_selection(cx);
            }),
        );
        let header = section_header_with_action(ui, "Theme", reset_btn);
        let preset = self.current_theme_preset();

        // Light / Dark / System as full-bleed window mockups instead of a
        // segmented control: the tile shows what the mode does to the chrome,
        // painted with *this preset's* two variants.
        let modes: [(crate::ThemeMode, &str, &str); 3] = [
            (crate::ThemeMode::System, "System", "theme-mode-system"),
            (crate::ThemeMode::Light, "Light", "theme-mode-light"),
            (crate::ThemeMode::Dark, "Dark", "theme-mode-dark"),
        ];
        let mut mode_row = div().flex().flex_row().w_full().gap(px(12.));
        for (mode, label, id) in modes {
            mode_row = mode_row.child(self.render_theme_mode_tile(mode, label, id, preset, ui, cx));
        }

        let preset_row = self.render_theme_preset_select(preset, ui, cx);

        let reduce_motion = self.cached_config.reduce_motion_enabled();
        let motion_row = div()
            .id("row-reduce-motion")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(
                ui,
                "Reduce motion",
                "Settle hover transitions and the sidebar slide instantly instead of animating them.",
            ))
            .child(
                div()
                    .id("reduce-motion-toggle")
                    .flex_shrink_0()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(
                            false,
                            "reduce_motion",
                            serde_json::Value::Bool(!reduce_motion),
                            cx,
                        );
                    }))
                    .child(toggle_pill(reduce_motion, ui)),
            );

        let content = div()
            .flex()
            .flex_col()
            .child(header)
            .child(mode_row)
            .child(div().h(px(14.)).flex_none())
            .child(render_theme_diff_preview(ui))
            .child(div().h(px(12.)).flex_none())
            .child(setting_card(ui).child(preset_row))
            .child(div().h(px(18.)).flex_none())
            .child(section_header(ui, "Preferences"))
            .child(setting_card(ui).child(motion_row));

        #[cfg(target_os = "windows")]
        let content = {
            let chrome_material = self.cached_config.cockpit_chrome_material_enabled();
            let chrome_material_row = div()
                .id("row-windows-chrome-material")
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .px(px(12.))
                .py(px(10.))
                .child(setting_text(
                    ui,
                    "Chrome material",
                    "Let Mica show through the navigation card.",
                ))
                .child(
                    div()
                        .id("windows-chrome-material-toggle")
                        .flex_shrink_0()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.persist_setting(
                                false,
                                "windows_chrome_material",
                                serde_json::Value::Bool(!chrome_material),
                                cx,
                            );
                        }))
                        .child(crate::settings::components::toggle_pill(
                            chrome_material,
                            ui,
                        )),
                );

            let windows_card = setting_card(ui).child(chrome_material_row);

            content
                .child(div().h(px(18.)).flex_none())
                .child(crate::settings::components::section_header(ui, "Windows"))
                .child(windows_card)
        };

        #[cfg(target_os = "macos")]
        let content = {
            let sidebar_material = self.cached_config.macos_chrome_material_enabled();
            let sidebar_material_row = div()
                .id("row-macos-chrome-material")
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .px(px(12.))
                .py(px(10.))
                .child(setting_text(
                    ui,
                    "Sidebar transparency",
                    "Show the native macOS Sidebar material in the navigation card.",
                ))
                .child(
                    div()
                        .id("macos-chrome-material-toggle")
                        .flex_shrink_0()
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.persist_setting(
                                false,
                                "macos_chrome_material",
                                serde_json::Value::Bool(!sidebar_material),
                                cx,
                            );
                        }))
                        .child(crate::settings::components::toggle_pill(
                            sidebar_material,
                            ui,
                        )),
                );

            let macos_card = setting_card(ui).child(sidebar_material_row);

            content
                .child(div().h(px(18.)).flex_none())
                .child(crate::settings::components::section_header(ui, "macOS"))
                .child(macos_card)
        };

        content
    }

    /// One Light/Dark/System tile: a full-bleed window mockup framed by a 2px
    /// ring. The ring is always painted (transparent-ish at rest) so selecting
    /// a mode never reflows the row.
    fn render_theme_mode_tile(
        &self,
        mode: crate::ThemeMode,
        label: &'static str,
        id: &'static str,
        preset: &crate::theme::ThemePreset,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = self.theme_mode == mode;
        let resting = if is_active {
            with_alpha(ui.text, 0.85)
        } else {
            with_alpha(ui.text, 0.12)
        };
        let hovered = if is_active {
            with_alpha(ui.text, 0.85)
        } else {
            with_alpha(ui.text, 0.32)
        };

        let light = theme_preview_palette(preset.light);
        let dark = theme_preview_palette(preset.dark);
        let mockup = match mode {
            crate::ThemeMode::Light => {
                theme_mode_mockup(light, MockupHalf::Full).into_any_element()
            }
            crate::ThemeMode::Dark => theme_mode_mockup(dark, MockupHalf::Full).into_any_element(),
            crate::ThemeMode::System => div()
                .size_full()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(theme_mode_mockup(light, MockupHalf::Left)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(theme_mode_mockup(dark, MockupHalf::Right)),
                )
                .into_any_element(),
        };

        let frame = div()
            .id(id)
            .w_full()
            .h(px(THEME_MODE_TILE_HEIGHT))
            .rounded(px(THEME_MODE_TILE_RADIUS))
            .overflow_hidden()
            .border_2()
            .border_color(resting)
            .animated_hover(move |style, delta| {
                style.border_color(lerp_color(resting, hovered, delta));
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.apply_theme_mode(mode, window, cx);
            }))
            .child(mockup);

        div()
            .flex_1()
            .min_w(px(120.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(frame)
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_size(crate::ui_primitives::BODY)
                    .font_weight(if is_active {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if is_active { ui.text } else { ui.muted })
                    .child(label),
            )
    }

    /// The preset select. Replaces the old preset tile grid: the mode tiles
    /// above already carry the visual weight, so the identity picker is a
    /// one-line control listing every [`crate::theme::ThemePreset`]. Picking a
    /// preset keeps the current Light/Dark/System mode - the two axes are
    /// orthogonal, so the swatch previews the variant that mode resolves to.
    fn render_theme_preset_select(
        &self,
        current: &crate::theme::ThemePreset,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.theme_dropdown_open;
        let is_light = self.theme_mode == crate::ThemeMode::Light
            || (self.theme_mode == crate::ThemeMode::System
                && crate::theme::theme_name_is_light(&self.current_theme_name()));
        let current_name = current.name;

        // Decide open/close from the render-time snapshot, not the live state:
        // the menu's `on_mouse_down_out` fires on this same press and may have
        // already cleared it, so a live toggle would re-open the menu.
        let mut trigger = select_trigger("theme-preset-select", ui)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.theme_dropdown_open = !is_open;
                    this.settings_focus.focus(window, cx);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .flex_1()
                    .min_w_0()
                    .child(theme_swatch(current.variant(is_light)))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(crate::ui_primitives::BODY)
                            .text_color(ui.text)
                            .truncate()
                            .child(SharedString::from(current_name.to_string())),
                    ),
            )
            .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu("theme-preset-list", ui).on_mouse_down_out(cx.listener(
                |this, _, _w, cx| {
                    if this.theme_dropdown_open {
                        this.theme_dropdown_open = false;
                        cx.notify();
                    }
                },
            ));
            for (idx, preset) in crate::theme::PRESETS.iter().enumerate() {
                let is_current = preset.name == current_name;
                menu = menu.child(
                    select_item(("theme-preset", idx), is_current, ui)
                        .cursor(CursorStyle::Arrow)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.theme_dropdown_open = false;
                            this.apply_theme_preset(&crate::theme::PRESETS[idx], window, cx);
                        }))
                        .child(theme_swatch(preset.variant(is_light)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child(preset.name),
                        )
                        .child(
                            svg()
                                .size(px(13.))
                                .flex_none()
                                .path("icons/check.svg")
                                .text_color(if is_current {
                                    ui.text
                                } else {
                                    with_alpha(ui.text, 0.0)
                                }),
                        ),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(
                ui,
                "Preset",
                "Palette applied to the terminal grid and the app chrome.",
            ))
            .child(div().flex_shrink_0().child(trigger))
            .into_any_element()
    }

    /// Apply a Light/Dark/System selection from the Themes page.
    pub(crate) fn apply_theme_mode(
        &mut self,
        mode: crate::ThemeMode,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let name = mode.resolved_theme_name(self.current_theme_preset(), window.appearance());
        self.persist_theme_selection(mode, name, cx);
    }

    pub(crate) fn sync_system_theme_from_window(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_mode != crate::ThemeMode::System {
            return;
        }
        let name = self
            .theme_mode
            .resolved_theme_name(self.current_theme_preset(), window.appearance());
        if self.cached_config.theme.as_deref() == Some(name) {
            return;
        }
        self.persist_theme_selection(crate::ThemeMode::System, name, cx);
    }
}

#[derive(Clone, Copy)]
struct ThemePreview {
    base: Hsla,
    surface: Hsla,
    overlay: Hsla,
    border: Hsla,
    text: Hsla,
    accent: Hsla,
}

fn theme_preview_palette(name: &str) -> ThemePreview {
    let theme = crate::theme::theme_by_name(name).unwrap_or_else(crate::theme::paneflow_dark);
    let ui = crate::theme::ui_colors_with(&theme);
    ThemePreview {
        base: ui.base,
        surface: ui.surface,
        overlay: ui.overlay,
        border: ui.border,
        text: ui.text,
        accent: ui.accent,
    }
}

/// The theme's own base + accent as a 20px "Aa" chip, so a row of theme names
/// reads as a row of palettes. Painted from `ui_colors_with`, not from a
/// hand-kept table, so a new entry in `THEMES` needs no change here.
fn theme_swatch(name: &str) -> impl IntoElement {
    let p = theme_preview_palette(name);
    div()
        .w(px(20.))
        .h(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(p.base)
        .border_1()
        .border_color(with_alpha(p.text, 0.16))
        .text_size(px(10.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(p.accent)
        .child("Aa")
}

/// A rounded placeholder bar inside a mockup (nav item, text run).
fn mock_bar(width: f32, height: f32, color: Hsla) -> gpui::Div {
    div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .rounded_full()
        .bg(color)
}

/// The window mockup painted inside a Light/Dark/System tile: a rail on the
/// left, a raised panel on the right that bleeds off the bottom edge. `half`
/// selects the slice, so the System tile can butt a light left half against a
/// dark right half and read as one continuous window.
fn theme_mode_mockup(p: ThemePreview, half: MockupHalf) -> impl IntoElement {
    let show_rail = half != MockupHalf::Right;
    // Inner radius: concentric with the ring rather than equal to it.
    let radius = px(THEME_MODE_TILE_RADIUS - THEME_MODE_TILE_BORDER);
    let mut root = div().size_full().flex().flex_row().bg(p.base);
    // A System half owns only the outer corners of its own side; the seam edge
    // stays square so the two halves meet as one continuous window.
    root = match half {
        MockupHalf::Full => root.rounded(radius),
        MockupHalf::Left => root.rounded_l(radius),
        MockupHalf::Right => root.rounded_r(radius),
    };

    if show_rail {
        root = root.child(
            div()
                .w(px(40.))
                .flex_none()
                .h_full()
                .bg(p.surface)
                // The rail is opaque and sits on the tile's left edge, so it
                // has to carry the same corners the root just took.
                .rounded_l(radius)
                .pt(px(16.))
                .px(px(9.))
                .flex()
                .flex_col()
                .gap(px(7.))
                .child(mock_bar(22., 5., with_alpha(p.text, 0.34)))
                .child(mock_bar(15., 4., with_alpha(p.text, 0.18)))
                .child(mock_bar(18., 4., with_alpha(p.text, 0.18)))
                .child(mock_bar(12., 4., with_alpha(p.text, 0.18))),
        );
    }

    // The panel keeps a gutter on the outer edges only: the seam edge runs
    // flush so the two System halves meet without a visible gap.
    let (pad_left, pad_right) = match half {
        MockupHalf::Full => (10., 12.),
        MockupHalf::Left => (10., 0.),
        MockupHalf::Right => (0., 12.),
    };

    root.child(
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pt(px(16.))
            .pl(px(pad_left))
            .pr(px(pad_right))
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .bg(p.overlay)
                    .border_1()
                    .border_color(with_alpha(p.border, 0.9))
                    .when(half != MockupHalf::Right, |d| d.rounded_tl(px(7.)))
                    .when(half != MockupHalf::Left, |d| d.rounded_tr(px(7.)))
                    .p(px(11.))
                    .flex()
                    .flex_col()
                    .gap(px(7.))
                    .child(mock_bar(
                        if half == MockupHalf::Right { 74. } else { 52. },
                        6.,
                        with_alpha(p.text, 0.30),
                    ))
                    .child(mock_bar(96., 5., with_alpha(p.text, 0.16)))
                    .child(mock_bar(
                        if half == MockupHalf::Right { 60. } else { 78. },
                        5.,
                        with_alpha(p.text, 0.16),
                    ))
                    .child(mock_bar(44., 5., p.accent))
                    .child(mock_bar(88., 5., with_alpha(p.text, 0.16))),
            ),
    )
}

/// Which syntax slot paints one token of the preview snippet. The sample is
/// hand-tokenized instead of run through `diff::highlighter`, so repainting the
/// settings page costs no tree-sitter parse.
#[derive(Clone, Copy)]
enum PreviewTok {
    Keyword,
    Constant,
    Type,
    Property,
    String,
    Number,
    Punctuation,
    Operator,
}

impl PreviewTok {
    fn color(self, s: &crate::theme::SyntaxPalette) -> Hsla {
        match self {
            Self::Keyword => s.keyword,
            Self::Constant => s.constant,
            Self::Type => s.r#type,
            Self::Property => s.property,
            Self::String => s.string,
            Self::Number => s.number,
            Self::Punctuation => s.punctuation,
            Self::Operator => s.operator,
        }
    }
}

type PreviewLine = &'static [(&'static str, PreviewTok)];

/// The sample: a Rust const the theme itself could plausibly be written as,
/// edited on three lines so both diff sides carry a full hunk.
const PREVIEW_HEAD: PreviewLine = &[
    ("const ", PreviewTok::Keyword),
    ("THEME_PREVIEW", PreviewTok::Constant),
    (": ", PreviewTok::Punctuation),
    ("Theme", PreviewTok::Type),
    (" = ", PreviewTok::Operator),
    ("Theme", PreviewTok::Type),
    (" {", PreviewTok::Punctuation),
];

const PREVIEW_TAIL: PreviewLine = &[("};", PreviewTok::Punctuation)];

const PREVIEW_OLD: [PreviewLine; 3] = [
    &[
        ("    surface", PreviewTok::Property),
        (": ", PreviewTok::Punctuation),
        ("\"sidebar\"", PreviewTok::String),
        (",", PreviewTok::Punctuation),
    ],
    &[
        ("    accent", PreviewTok::Property),
        (": ", PreviewTok::Punctuation),
        ("\"#2563eb\"", PreviewTok::String),
        (",", PreviewTok::Punctuation),
    ],
    &[
        ("    contrast", PreviewTok::Property),
        (": ", PreviewTok::Punctuation),
        ("42", PreviewTok::Number),
        (",", PreviewTok::Punctuation),
    ],
];

const PREVIEW_NEW: [PreviewLine; 3] = [
    &[
        ("    surface", PreviewTok::Property),
        (": ", PreviewTok::Punctuation),
        ("\"sidebar-elevated\"", PreviewTok::String),
        (",", PreviewTok::Punctuation),
    ],
    &[
        ("    accent", PreviewTok::Property),
        (": ", PreviewTok::Punctuation),
        ("\"#0ea5e9\"", PreviewTok::String),
        (",", PreviewTok::Punctuation),
    ],
    &[
        ("    contrast", PreviewTok::Property),
        (": ", PreviewTok::Punctuation),
        ("68", PreviewTok::Number),
        (",", PreviewTok::Punctuation),
    ],
];

/// Live sample under the mode tiles, painted as the split diff the Review dock
/// renders: same `RowPalette`, same 18px rows at 12px mono, same bar + gutter
/// geometry. Every color comes from the active theme, so switching a mode or a
/// preset repaints it in place - this is the surface the theme's syntax palette
/// and diff washes actually drive.
fn render_theme_diff_preview(ui: crate::theme::UiColors) -> impl IntoElement {
    let p = crate::diff::palette(ui);
    let syntax = crate::theme::active_theme().syntax;
    // The diff surface is always drawn in the default mono family (see
    // `DiffElement::new`), never the terminal's configured font.
    let font = SharedString::from(crate::terminal::element::resolve_font_family(None));

    div()
        .w_full()
        .rounded(px(THEME_PREVIEW_CORNER_RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(ui.border)
        .bg(p.context_bg)
        .p(px(6.))
        .flex()
        .flex_row()
        // The seam is body background showing between the two washes, the way
        // the split view separates its halves.
        .gap(px(6.))
        .font_family(font)
        .text_size(px(12.))
        .child(preview_diff_column(false, p, syntax))
        .child(preview_diff_column(true, p, syntax))
}

/// One half of the split preview: `added` paints the new side (green wash,
/// added bar), otherwise the base side (red wash, deleted bar).
fn preview_diff_column(
    added: bool,
    p: crate::diff::RowPalette,
    syntax: crate::theme::SyntaxPalette,
) -> impl IntoElement {
    let (wash, gutter_bg, number, bar) = if added {
        (p.add_bg, p.add_gutter_bg, p.gutter_add, p.add_bar)
    } else {
        (p.del_bg, p.del_gutter_bg, p.gutter_del, p.del_bar)
    };
    let body = if added { PREVIEW_NEW } else { PREVIEW_OLD };

    let mut column = div().flex_1().min_w_0().flex().flex_col().overflow_hidden();

    for (idx, line) in std::iter::once(PREVIEW_HEAD)
        .chain(body)
        .chain(std::iter::once(PREVIEW_TAIL))
        .enumerate()
    {
        let changed = PREVIEW_CHANGED.contains(&idx);

        let mut code = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .flex()
            .flex_row()
            .items_center();
        for (text, tok) in line {
            code = code.child(
                div()
                    .flex_none()
                    .text_color(tok.color(&syntax))
                    .child(*text),
            );
        }

        column = column.child(
            div()
                .w_full()
                .h(px(crate::diff::ROW_HEIGHT))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .whitespace_nowrap()
                .bg(if changed { wash } else { p.context_bg })
                .child(
                    // Gutter rail: the tint spans the bar lane too, and the bar
                    // is painted over it - `element.rs` paint order.
                    div()
                        .w(px(PREVIEW_BAR_W + PREVIEW_BAR_PAD + PREVIEW_GUTTER_W))
                        .h_full()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .bg(if changed { gutter_bg } else { p.gutter_bg })
                        .child(
                            div()
                                .w(px(PREVIEW_BAR_W))
                                .h_full()
                                .flex_none()
                                .when(changed, |d| d.bg(bar)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .pr(px(PREVIEW_NUM_GAP))
                                .text_right()
                                .text_color(if changed { number } else { p.muted })
                                .child(format!("{}", idx + 1)),
                        ),
                )
                .child(code),
        );
    }

    column
}

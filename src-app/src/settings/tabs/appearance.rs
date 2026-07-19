//! "Themes" settings tab - light, dark, and system theme selection.

use gpui::{
    ClickEvent, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::app::theme_picker::is_default_theme_name;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, card_colors, secondary_button, section_header,
    section_header_with_action, setting_card, setting_text, with_alpha,
};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

const PRESET_THEME_NAMES: [&str; 3] = ["Vercel", "Claude", "Cursor"];
const THEME_PREVIEW_CORNER_RADIUS: f32 = 7.;
const THEME_PREVIEW_EDGE_FILL_RADIUS: f32 = 6.;

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
        let header = section_header_with_action(ui, "Default theme", reset_btn);
        let current_theme = self.current_theme_name();
        let default_theme_active = is_default_theme_name(current_theme.as_str());

        // Theme mode selector (Light / Dark / System).
        let modes: [(crate::ThemeMode, &str, &str, &str); 3] = [
            (
                crate::ThemeMode::Light,
                "Light",
                "icons/sun.svg",
                "theme-mode-light",
            ),
            (
                crate::ThemeMode::Dark,
                "Dark",
                "icons/moon.svg",
                "theme-mode-dark",
            ),
            (
                crate::ThemeMode::System,
                "System",
                "icons/device-desktop.svg",
                "theme-mode-system",
            ),
        ];
        let (theme_card_bg, _) = card_colors();
        let theme_mode_hover_bg = lerp_color(theme_card_bg, ui.text, 0.06);
        let mut mode_switch = div().flex().flex_row().items_center().gap(px(2.));
        for (mode, label, icon, id) in modes {
            let is_active = default_theme_active && self.theme_mode == mode;
            let fg = if is_active { ui.text } else { ui.muted };
            let seg = div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.))
                // Generous radius approximating Codex's Apple-style corner
                // smoothing (GPUI draws circular-arc corners - no true squircle).
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .child(svg().size(px(14.)).flex_none().path(icon).text_color(fg))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(fg)
                        .child(label),
                );
            let seg = if is_active {
                seg.bg(crate::app::constants::sidebar_tab_active_background())
                    .into_any_element()
            } else {
                seg.bg(theme_card_bg)
                    .animated_hover_bg(theme_card_bg, theme_mode_hover_bg)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.apply_theme_mode(mode, window, cx);
                    }))
                    .into_any_element()
            };
            mode_switch = mode_switch.child(seg);
        }

        let theme_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(
                ui,
                "Default theme",
                "Light, dark, or system when no preset is active",
            ))
            .child(div().flex_shrink_0().child(mode_switch));
        // Shared Codex-style card look (white/#f2f2f2 in light, #232323/#303030
        // in dark, generous Apple-approximating radius) - see `setting_card`.
        let theme_card = setting_card(ui).child(theme_row);

        let mut presets_grid = div().flex().flex_row().flex_wrap().gap(px(10.));
        for (idx, name) in PRESET_THEME_NAMES.iter().enumerate() {
            presets_grid = presets_grid.child(self.render_theme_preset_tile(
                idx,
                name,
                current_theme.as_str(),
                ui,
                cx,
            ));
        }

        let content = div()
            .flex()
            .flex_col()
            .child(header)
            .child(theme_card)
            .child(div().h(px(18.)).flex_none())
            .child(section_header(ui, "Presets"))
            .child(presets_grid);

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

    fn render_theme_preset_tile(
        &self,
        idx: usize,
        name: &'static str,
        current_name: &str,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_current = name.eq_ignore_ascii_case(current_name);
        let preview = theme_preview_palette(name);
        let name_owned = name.to_string();
        let bg = if is_current {
            with_alpha(ui.text, 0.08)
        } else {
            with_alpha(ui.text, 0.03)
        };
        let hover_bg = if is_current {
            with_alpha(ui.text, 0.12)
        } else {
            with_alpha(ui.text, 0.08)
        };
        let border = with_alpha(ui.text, 0.10);

        div()
            .id(SharedString::from(format!("theme-preset-{idx}")))
            .flex_1()
            .min_w(px(190.))
            .h(px(132.))
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(10.))
            .px(px(12.))
            .py(px(12.))
            .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
            .border_1()
            .border_color(border)
            .bg(bg)
            .animated_hover_bg(bg, hover_bg)
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.apply_theme_by_name(&name_owned, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(render_theme_logo(name, preview))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(crate::ui_primitives::BODY)
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(ui.text)
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_size(crate::ui_primitives::LABEL_SM)
                                            .text_color(ui.muted)
                                            .child(theme_preset_description(name)),
                                    ),
                            ),
                    )
                    .child(
                        div().w(px(16.)).h(px(16.)).flex().items_center().child(
                            svg()
                                .size(px(14.))
                                .path("icons/check.svg")
                                .text_color(if is_current {
                                    ui.text
                                } else {
                                    with_alpha(ui.text, 0.0)
                                }),
                        ),
                    ),
            )
            .child(render_theme_preview_panel(preview))
    }

    /// Apply a Light/Dark/System selection from the Themes page.
    pub(crate) fn apply_theme_mode(
        &mut self,
        mode: crate::ThemeMode,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let name = mode.resolved_theme_name(window.appearance());
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
        let name = self.theme_mode.resolved_theme_name(window.appearance());
        if self.cached_config.theme.as_deref() == Some(name) {
            return;
        }
        self.persist_theme_selection(crate::ThemeMode::System, name, cx);
    }
}

fn theme_preset_description(name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("Vercel") {
        "Monochrome dark"
    } else if name.eq_ignore_ascii_case("Claude") {
        "Claude Desktop"
    } else if name.eq_ignore_ascii_case("Cursor") {
        "Cursor IDE"
    } else {
        "Default dark"
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
    let theme = crate::theme::theme_by_name(name).unwrap_or_else(crate::theme::one_dark);
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

fn theme_logo_path(name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("Vercel") {
        "icons/vercel.svg"
    } else if name.eq_ignore_ascii_case("Claude") {
        "icons/claude-color.svg"
    } else if name.eq_ignore_ascii_case("Cursor") {
        "agents/cursor.svg"
    } else {
        "icons/palette.svg"
    }
}

fn render_theme_logo(name: &str, preview: ThemePreview) -> impl IntoElement {
    div()
        .w(px(34.))
        .h(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(preview.base)
        .border_1()
        .border_color(with_alpha(preview.text, 0.16))
        .child(
            svg()
                .size(px(18.))
                .path(theme_logo_path(name))
                .text_color(preview.accent),
        )
}

fn render_theme_preview_panel(preview: ThemePreview) -> impl IntoElement {
    let outer_radius = px(THEME_PREVIEW_CORNER_RADIUS);
    let edge_fill_radius = px(THEME_PREVIEW_EDGE_FILL_RADIUS);

    div()
        .w_full()
        .h(px(44.))
        .rounded(outer_radius)
        .overflow_hidden()
        .border_1()
        .border_color(with_alpha(preview.text, 0.12))
        .bg(preview.base)
        .child(
            div()
                .flex()
                .flex_row()
                .h_full()
                .child(
                    div()
                        .w(px(34.))
                        .h_full()
                        .bg(preview.surface)
                        .rounded_tl(edge_fill_radius)
                        .rounded_bl(edge_fill_radius)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .px(px(6.))
                                .py(px(7.))
                                .child(
                                    div()
                                        .w(px(16.))
                                        .h(px(4.))
                                        .rounded_full()
                                        .bg(with_alpha(preview.text, 0.26)),
                                )
                                .child(
                                    div()
                                        .w(px(10.))
                                        .h(px(4.))
                                        .rounded_full()
                                        .bg(with_alpha(preview.text, 0.16)),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .bg(preview.base)
                        .p(px(7.))
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(div().w_full().h(px(7.)).rounded(px(3.)).bg(preview.overlay))
                        .child(div().w(px(58.)).h(px(4.)).rounded_full().bg(preview.accent))
                        .child(
                            div()
                                .w(px(82.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(with_alpha(preview.text, 0.18)),
                        ),
                )
                .child(
                    div()
                        .w(px(6.))
                        .h_full()
                        .rounded_tr(edge_fill_radius)
                        .rounded_br(edge_fill_radius)
                        .bg(with_alpha(preview.border, 0.95)),
                ),
        )
}

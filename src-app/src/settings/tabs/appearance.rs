//! "Themes" settings tab - light, dark, and system theme selection.

use gpui::{
    ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, ParentElement, Styled, div,
    prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, secondary_button, section_header_with_action, setting_card,
    setting_text,
};

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
        let mut mode_switch = div().flex().flex_row().items_center().gap(px(2.));
        for (mode, label, icon, id) in modes {
            let is_active = self.theme_mode == mode;
            let fg = if is_active { ui.text } else { ui.muted };
            let mut seg = div()
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
            if is_active {
                seg = seg.bg(crate::app::constants::sidebar_tab_active_background());
            } else {
                seg = seg
                    .cursor(CursorStyle::PointingHand)
                    .hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.apply_theme_mode(mode, window, cx);
                    }));
            }
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
                "Theme",
                "Use the light, dark, or system mode",
            ))
            .child(div().flex_shrink_0().child(mode_switch));
        // Shared Codex-style card look (white/#f2f2f2 in light, #232323/#303030
        // in dark, generous Apple-approximating radius) - see `setting_card`.
        let theme_card = setting_card(ui).child(theme_row);

        let content = div().flex().flex_col().child(header).child(theme_card);

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
                    "Let Mica or blur show through sidebars and the title bar.",
                ))
                .child(
                    div()
                        .id("windows-chrome-material-toggle")
                        .flex_shrink_0()
                        .cursor(CursorStyle::PointingHand)
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

        content
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

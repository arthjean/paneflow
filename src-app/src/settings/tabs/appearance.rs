use gpui::{
    ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, div, img, prelude::*, px, svg,
};

use paneflow_config::schema::{AppIcon, BackgroundStyle, InterfaceStyle};

use paneflow_config::schema::WorkspaceIcon;

use crate::GeneralDropdown;
use crate::PaneFlowApp;
use crate::settings::components::{
    deferred_select_menu, select_chevron, select_menu, select_option, select_trigger, with_alpha,
};
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::{self, Block, SearchCard};
use crate::ui_primitives::{AccessibleControlExt, AnimatedHover, AnimatedHoverExt, lerp_color};

const THEME_MODE_TILE_HEIGHT: f32 = 134.;
const THEME_MODE_TILE_RADIUS: f32 = 10.;
const THEME_MODE_TILE_BORDER: f32 = 2.;
const THEME_PREVIEW_CORNER_RADIUS: f32 = 8.;
const THEME_PREVIEW_GAP: f32 = 12.;
const APPEARANCE_INTRO: &str = "The theme, interface style, icons and motion of the whole app.";

const APP_ICON_TILE: f32 = 44.;
const APP_ICON_RING_GAP: f32 = 2.;
const APP_ICON_RING_RADIUS: f32 = 14.;
const APP_ICON_TRIGGER_ICON: f32 = 20.;
const APP_ICON_TRIGGER_CHEVRON: f32 = 12.;
const APP_ICON_CELL_WIDTH: f32 = 76.;
const APP_ICON_CELL_LABEL_GAP: f32 = 6.;
const APP_ICON_GRID_ROW_GAP: f32 = 14.;
const APP_ICON_GALLERY_PADDING: f32 = 12.;
const APP_ICON_GALLERY_RADIUS: f32 = 10.;
const APP_ICON_GALLERY_DROP: f32 = 6.;
const APP_ICON_PICKER_SECONDS: f32 = 0.2;

const PREVIEW_BAR_W: f32 = 4.;
const PREVIEW_BAR_PAD: f32 = 4.;
const PREVIEW_GUTTER_W: f32 = 24.;
const PREVIEW_NUM_GAP: f32 = 6.;
const PREVIEW_CHANGED: std::ops::RangeInclusive<usize> = 1..=3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MockupHalf {
    Full,
    Left,
    Right,
}

impl PaneFlowApp {
    pub(crate) fn render_appearance_heading(&self, title: gpui::Div) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(
                Ink::of(crate::theme::ui_colors()),
                APPEARANCE_INTRO,
            ))
            .into_any_element()
    }

    pub(crate) fn render_appearance_content(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        let reset = kit::button(
            "reset-theme",
            "Reset to default",
            Tone::Ghost,
            Size::Small,
            ink,
        )
        .on_activate(cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.reset_theme_selection(cx);
        }));
        let preset = self.current_theme_preset();

        let modes: [(crate::ThemeMode, &str, &str); 3] = [
            (crate::ThemeMode::System, "System", "theme-mode-system"),
            (crate::ThemeMode::Light, "Light", "theme-mode-light"),
            (crate::ThemeMode::Dark, "Dark", "theme-mode-dark"),
        ];
        let mut mode_row = div().flex().flex_row().w_full().gap(px(12.));
        for (mode, label, id) in modes {
            mode_row =
                mode_row.child(self.render_theme_mode_tile(mode, label, id, preset, ink, cx));
        }

        let (theme_matched, theme_rows) = SearchCard::new()
            .row(
                &search::THEME_PRESET,
                self.render_theme_preset_select(preset, ink, ui, cx),
            )
            .into_rows();
        let (interface_matched, interface_rows) = SearchCard::new()
            .row(
                &search::INTERFACE_STYLE,
                self.render_interface_style_select(ink, ui, cx),
            )
            .row(
                &search::BACKGROUND_STYLE,
                self.render_background_style_select(ink, ui, cx),
            )
            .row(
                &search::WORKSPACE_ICON,
                self.render_workspace_icon_select(window.scale_factor(), ink, ui, cx),
            )
            .row(
                &search::APP_ICON,
                self.render_app_icon_row(window, ink, ui, cx),
            )
            .into_rows();

        let reduce_motion = self.cached_config.reduce_motion_enabled();
        let comfortable_density = self.cached_config.comfortable_density_enabled();
        let (preferences_matched, preferences_rows) = SearchCard::new()
            .row(
                &search::REDUCE_MOTION,
                self.appearance_toggle_row(
                    "reduce-motion-toggle",
                    &search::REDUCE_MOTION,
                    reduce_motion,
                    "reduce_motion",
                    ink,
                    cx,
                ),
            )
            .row(
                &search::COMFORTABLE_DENSITY,
                self.appearance_toggle_row(
                    "comfortable-density-toggle",
                    &search::COMFORTABLE_DENSITY,
                    comfortable_density,
                    "comfortable_density",
                    ink,
                    cx,
                ),
            )
            .into_rows();

        let mut first = true;
        let theme_block = Block::new("Theme")
            .top_gap(kit::section_gap(&mut first))
            .found(theme_matched)
            .child(kit::section_head_with(ink, "Theme", None, reset))
            .child(div().h(px(kit::SECTION_HEAD_GAP)).flex_none())
            .region("theme-mode", search::THEME_MODES, mode_row)
            .child(div().h(px(THEME_PREVIEW_GAP)).flex_none())
            .child(render_theme_diff_preview(ui, ink))
            .child(div().h(px(THEME_PREVIEW_GAP)).flex_none())
            .child(kit::search_group(ink, theme_rows))
            .finish();
        let interface_block = Block::new("Interface")
            .top_gap(kit::section_gap(&mut first))
            .found(interface_matched)
            .child(kit::section(
                ink,
                "Interface",
                None,
                kit::search_group(ink, interface_rows),
            ))
            .finish();
        let preferences_block = Block::new("Preferences")
            .top_gap(kit::section_gap(&mut first))
            .found(preferences_matched)
            .child(kit::section(
                ink,
                "Preferences",
                None,
                kit::search_group(ink, preferences_rows),
            ))
            .finish();
        div()
            .flex()
            .flex_col()
            .child(theme_block)
            .child(interface_block)
            .child(preferences_block)
    }

    fn appearance_toggle_row(
        &self,
        id: &'static str,
        copy: &'static search::SettingCopy,
        on: bool,
        config_key: &'static str,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        kit::row(
            ink,
            None,
            setting_copy(copy, ink),
            Some(
                kit::switch(id, copy.title, on, ink)
                    .on_activate(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.persist_setting(false, config_key, serde_json::Value::Bool(!on), cx);
                    }))
                    .into_any_element(),
            ),
        )
        .into_any_element()
    }

    fn render_theme_mode_tile(
        &self,
        mode: crate::ThemeMode,
        label: &'static str,
        id: &'static str,
        preset: &crate::theme::ThemePreset,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = self.theme_mode == mode;
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
            .border_color(if is_active { ink.primary } else { ink.edge })
            .cursor(CursorStyle::PointingHand)
            .when(!is_active, |frame| {
                frame.hover(move |style| style.border_color(ink.tertiary))
            })
            .accessible_control(gpui::accesskit::Role::RadioButton, label)
            .aria_toggled(if is_active {
                gpui::accesskit::Toggled::True
            } else {
                gpui::accesskit::Toggled::False
            })
            .on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
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
                    .text_size(px(kit::SIZE_MD))
                    .line_height(px(kit::LINE_MD))
                    .font_weight(if is_active {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if is_active {
                        ink.primary
                    } else {
                        ink.secondary
                    })
                    .child(label),
            )
    }

    fn render_theme_preset_select(
        &self,
        current: &crate::theme::ThemePreset,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.theme_dropdown_open;
        let is_light = self.theme_mode == crate::ThemeMode::Light
            || (self.theme_mode == crate::ThemeMode::System
                && crate::theme::theme_name_is_light(&self.current_theme_name()));
        let current_name = current.name;

        let mut trigger = select_trigger("theme-preset-select", "Theme", is_open, ui)
            .on_press(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.theme_dropdown_open = !is_open;
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
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
                menu = menu.option(
                    select_option(("theme-preset", idx), preset.name, is_current, ui)
                        .cursor(CursorStyle::Arrow)
                        .child(theme_swatch(preset.variant(is_light)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child(preset.name),
                        ),
                    cx.listener(move |this, _: &(), window, cx| {
                        this.theme_dropdown_open = false;
                        this.apply_theme_preset(&crate::theme::PRESETS[idx], window, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        kit::row(
            ink,
            None,
            setting_copy(&search::THEME_PRESET, ink),
            Some(trigger.into_any_element()),
        )
        .into_any_element()
    }

    fn render_interface_style_select(
        &self,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.general_dropdown == Some(GeneralDropdown::InterfaceStyle);
        let current = self.cached_config.resolved_interface_style();

        let mut trigger = select_trigger(
            "interface-style-select",
            search::INTERFACE_STYLE.title,
            is_open,
            ui,
        )
        .on_press(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.general_dropdown = if is_open {
                None
            } else {
                Some(GeneralDropdown::InterfaceStyle)
            };
            this.settings_focus.focus(window, cx);
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.text)
                .truncate()
                .child(current.label()),
        )
        .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu("interface-style-list", ui).on_mouse_down_out(cx.listener(
                |this, _, _w, cx| {
                    if this.general_dropdown == Some(GeneralDropdown::InterfaceStyle) {
                        this.general_dropdown = None;
                        cx.notify();
                    }
                },
            ));
            for (idx, &style) in InterfaceStyle::ALL.iter().enumerate() {
                menu = menu.option(
                    select_option(
                        ("interface-style", idx),
                        style.label(),
                        current == style,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ui.text)
                            .child(style.label()),
                    ),
                    cx.listener(move |this, _: &(), _window, cx| {
                        this.general_dropdown = None;
                        this.apply_interface_style(style, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        kit::row(
            ink,
            None,
            setting_copy(&search::INTERFACE_STYLE, ink),
            Some(trigger.into_any_element()),
        )
        .into_any_element()
    }

    pub(crate) fn apply_interface_style(&mut self, style: InterfaceStyle, cx: &mut Context<Self>) {
        self.persist_setting(
            false,
            "interface_style",
            serde_json::Value::String(style.wire_str().to_string()),
            cx,
        );
    }

    fn render_workspace_icon_select(
        &self,
        scale_factor: f32,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.general_dropdown == Some(GeneralDropdown::WorkspaceIcon);
        let current = self.cached_config.resolved_workspace_icon();
        let preview = move |icon: WorkspaceIcon| {
            let glyphs = crate::app::sidebar::folder_glyphs(icon, scale_factor);
            div()
                .flex_none()
                .size(px(crate::app::sidebar::SIDEBAR_FOLDER_SLOT_WIDTH))
                .flex()
                .items_center()
                .justify_center()
                .child(glyphs.render(glyphs.closed, ui))
        };

        let mut trigger = select_trigger(
            "workspace-icon-select",
            search::WORKSPACE_ICON.title,
            is_open,
            ui,
        )
        .on_press(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.general_dropdown = if is_open {
                None
            } else {
                Some(GeneralDropdown::WorkspaceIcon)
            };
            this.settings_focus.focus(window, cx);
            cx.notify();
        }))
        .child(preview(current))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.text)
                .truncate()
                .child(current.label()),
        )
        .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu("workspace-icon-list", ui).on_mouse_down_out(cx.listener(
                |this, _, _w, cx| {
                    if this.general_dropdown == Some(GeneralDropdown::WorkspaceIcon) {
                        this.general_dropdown = None;
                        cx.notify();
                    }
                },
            ));
            for (idx, &icon) in WorkspaceIcon::ALL.iter().enumerate() {
                menu = menu.option(
                    select_option(("workspace-icon", idx), icon.label(), current == icon, ui)
                        .cursor(CursorStyle::Arrow)
                        .child(preview(icon))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child(icon.label()),
                        ),
                    cx.listener(move |this, _: &(), _window, cx| {
                        this.general_dropdown = None;
                        this.apply_workspace_icon(icon, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        kit::row(
            ink,
            None,
            setting_copy(&search::WORKSPACE_ICON, ink),
            Some(trigger.into_any_element()),
        )
        .into_any_element()
    }

    pub(crate) fn apply_workspace_icon(&mut self, icon: WorkspaceIcon, cx: &mut Context<Self>) {
        self.persist_setting(
            false,
            "workspace_icon",
            serde_json::Value::String(icon.wire_str().to_string()),
            cx,
        );
    }

    fn render_app_icon_row(
        &self,
        window: &gpui::Window,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let scale_factor = window.scale_factor();
        let current = self.cached_config.resolved_app_icon();
        let now = std::time::Instant::now();
        if self.app_icon_picker.is_animating(now) {
            window.request_animation_frame();
        }
        let amount = self.app_icon_picker.amount(now);
        let open = self.app_icon_picker.is_open();

        let chevron = svg()
            .size(px(APP_ICON_TRIGGER_CHEVRON))
            .flex_none()
            .path("icons/settings/arrow-down-01.svg")
            .text_color(ink.secondary)
            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                std::f32::consts::PI * amount,
            )));
        let trigger = select_trigger("app-icon-select", search::APP_ICON.title, open, ui)
            .on_press(cx.listener(|this, _, _window, cx| {
                this.app_icon_picker.toggle(std::time::Instant::now());
                cx.notify();
            }))
            .child(
                img(crate::app_icon::preview_asset(current, scale_factor))
                    .size(px(APP_ICON_TRIGGER_ICON))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .truncate()
                    .child(current.label()),
            )
            .child(chevron);

        let header = kit::row(
            ink,
            None,
            setting_copy(&search::APP_ICON, ink),
            Some(trigger.into_any_element()),
        );

        let gallery = (amount > 0.).then(|| {
            let mut grid = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_y(px(APP_ICON_GRID_ROW_GAP));
            for (idx, &icon) in AppIcon::ALL.iter().enumerate() {
                let is_active = current == icon;
                let tile = app_icon_tile(idx, icon, is_active, scale_factor, ink).on_activate(
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.apply_app_icon(icon, cx);
                    }),
                );
                grid = grid.child(app_icon_cell(tile, icon, is_active, ink));
            }
            let measure_into = self.app_icon_picker.height.clone();
            let natural = div()
                .relative()
                .top(px(-APP_ICON_GALLERY_DROP * (1. - amount)))
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .px(px(kit::ROW_INSET))
                .pb(px(kit::ROW_INSET))
                .child(
                    div()
                        .w_full()
                        .p(px(APP_ICON_GALLERY_PADDING))
                        .rounded(px(APP_ICON_GALLERY_RADIUS))
                        .bg(ink.hover)
                        .child(grid),
                )
                .child(
                    gpui::canvas(
                        move |bounds, _, _| {
                            measure_into.set(Some(f32::from(bounds.size.height)));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );
            let measured = self.app_icon_picker.height.get();
            div()
                .id("app-icon-gallery")
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .overflow_hidden()
                .opacity(amount)
                .when(amount < 1., |wrapper| {
                    wrapper.h(px(measured.unwrap_or(0.) * amount))
                })
                .child(natural)
        });

        div()
            .id("row-app-icon")
            .flex()
            .flex_col()
            .child(header)
            .children(gallery)
            .into_any_element()
    }

    pub(crate) fn apply_app_icon(&mut self, icon: AppIcon, cx: &mut Context<Self>) {
        self.persist_setting(
            false,
            "app_icon",
            serde_json::Value::String(icon.wire_str().to_string()),
            cx,
        );
    }

    fn render_background_style_select(
        &self,
        ink: Ink,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_open = self.general_dropdown == Some(GeneralDropdown::BackgroundStyle);
        let current = self.cached_config.resolved_background_style();

        let mut trigger =
            select_trigger("background-style-select", "Background style", is_open, ui)
                .on_press(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.general_dropdown = if is_open {
                        None
                    } else {
                        Some(GeneralDropdown::BackgroundStyle)
                    };
                    this.settings_focus.focus(window, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(crate::ui_primitives::BODY)
                        .text_color(ui.text)
                        .truncate()
                        .child(current.label()),
                )
                .child(select_chevron(ui));

        if is_open {
            let mut menu = select_menu("background-style-list", ui).on_mouse_down_out(cx.listener(
                |this, _, _w, cx| {
                    if this.general_dropdown == Some(GeneralDropdown::BackgroundStyle) {
                        this.general_dropdown = None;
                        cx.notify();
                    }
                },
            ));
            for (idx, &style) in BackgroundStyle::ALL.iter().enumerate() {
                let is_current = current == style;
                menu = menu.option(
                    select_option(("background-style", idx), style.label(), is_current, ui)
                        .cursor(CursorStyle::Arrow)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.text)
                                .child(style.label()),
                        ),
                    cx.listener(move |this, _: &(), _window, cx| {
                        this.general_dropdown = None;
                        this.apply_background_style(style, cx);
                    }),
                );
            }
            trigger = trigger.child(deferred_select_menu(menu));
        }

        kit::row(
            ink,
            None,
            setting_copy(&search::BACKGROUND_STYLE, ink),
            Some(trigger.into_any_element()),
        )
        .into_any_element()
    }

    pub(crate) fn apply_background_style(
        &mut self,
        style: BackgroundStyle,
        cx: &mut Context<Self>,
    ) {
        self.persist_background_style(style, cx);
    }

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
        self.sync_system_theme(window.appearance(), cx);
    }

    pub(crate) fn sync_system_theme(
        &mut self,
        appearance: gpui::WindowAppearance,
        cx: &mut Context<Self>,
    ) {
        if self.theme_mode != crate::ThemeMode::System {
            return;
        }
        let name = self
            .theme_mode
            .resolved_theme_name(self.current_theme_preset(), appearance);
        if self.cached_config.theme.as_deref() == Some(name) {
            return;
        }
        self.persist_theme_selection(crate::ThemeMode::System, name, cx);
    }
}

fn setting_copy(copy: &search::SettingCopy, ink: Ink) -> Text {
    Text::new(copy.title).line(kit::line(ink, copy.description))
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

const SWATCH_STROKE: f32 = 2.;

fn theme_swatch(name: &str) -> impl IntoElement {
    let theme = crate::theme::theme_by_name(name).unwrap_or_else(crate::theme::paneflow_dark);
    let base = gpui::Rgba::from(crate::theme::ui_colors_with(&theme).base);
    let code = theme.syntax;
    let line = |indent: f32, tokens: &[(f32, Hsla)]| {
        div()
            .flex()
            .flex_row()
            .gap(px(SWATCH_STROKE))
            .pl(px(indent))
            .children(
                tokens
                    .iter()
                    .map(|&(width, color)| mock_bar(width, SWATCH_STROKE, color)),
            )
    };
    let glyph = div()
        .flex()
        .flex_col()
        .gap(px(SWATCH_STROKE))
        .child(line(0., &[(4., code.keyword), (6., code.function)]))
        .child(line(2., &[(4., code.property), (5., code.string)]))
        .child(line(0., &[(9., code.comment)]))
        .into_any_element();
    let (background, ring) = crate::pane::tinted_badge_colors(base);
    crate::pane::badge_tile(background, ring, glyph)
}

fn mock_bar(width: f32, height: f32, color: Hsla) -> gpui::Div {
    div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .rounded_full()
        .bg(color)
}

fn theme_mode_mockup(p: ThemePreview, half: MockupHalf) -> impl IntoElement {
    let show_rail = half != MockupHalf::Right;
    let radius = px(THEME_MODE_TILE_RADIUS - THEME_MODE_TILE_BORDER);
    let mut root = div().size_full().flex().flex_row().bg(p.base);
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

fn render_theme_diff_preview(ui: crate::theme::UiColors, ink: Ink) -> impl IntoElement {
    let p = crate::diff::palette(ui);
    let syntax = crate::theme::active_theme().syntax;
    let font = SharedString::from(crate::terminal::element::resolve_font_family(None));

    div()
        .w_full()
        .rounded(px(THEME_PREVIEW_CORNER_RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(ink.edge)
        .bg(p.context_bg)
        .p(px(6.))
        .flex()
        .flex_row()
        .gap(px(6.))
        .font_family(font)
        .text_size(px(12.))
        .child(preview_diff_column(false, p, syntax))
        .child(preview_diff_column(true, p, syntax))
}

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

fn app_icon_tile(
    idx: usize,
    icon: AppIcon,
    is_active: bool,
    scale_factor: f32,
    ink: Ink,
) -> AnimatedHover {
    let resting = if is_active {
        ink.primary
    } else {
        with_alpha(ink.primary, 0.)
    };
    let hovered = if is_active { ink.primary } else { ink.tertiary };
    div()
        .id(("app-icon", idx))
        .flex_none()
        .p(px(APP_ICON_RING_GAP))
        .rounded(px(APP_ICON_RING_RADIUS))
        .border_2()
        .border_color(resting)
        .focusable()
        .accessible_control(gpui::accesskit::Role::Button, icon.label())
        .animated_hover(move |style, delta| {
            style.border_color(lerp_color(resting, hovered, delta));
        })
        .child(
            img(crate::app_icon::preview_asset(icon, scale_factor))
                .size(px(APP_ICON_TILE))
                .flex_none(),
        )
}

fn app_icon_cell(tile: AnimatedHover, icon: AppIcon, is_active: bool, ink: Ink) -> gpui::Div {
    div()
        .w(px(APP_ICON_CELL_WIDTH))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(APP_ICON_CELL_LABEL_GAP))
        .child(tile)
        .child(
            div()
                .w_full()
                .text_center()
                .truncate()
                .text_size(px(kit::SIZE_SM))
                .line_height(px(kit::LINE_SM))
                .text_color(if is_active {
                    ink.primary
                } else {
                    ink.secondary
                })
                .when(is_active, |label| {
                    label.font_weight(gpui::FontWeight::MEDIUM)
                })
                .child(icon.label()),
        )
}

#[derive(Default)]
pub(crate) struct AppIconPicker {
    open: bool,
    from: f32,
    started_at: Option<std::time::Instant>,
    height: std::rc::Rc<std::cell::Cell<Option<f32>>>,
}

impl AppIconPicker {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    fn progress(&self, now: std::time::Instant) -> Option<f32> {
        self.started_at.map(|started_at| {
            (now.saturating_duration_since(started_at).as_secs_f32() / APP_ICON_PICKER_SECONDS)
                .clamp(0., 1.)
        })
    }

    pub(crate) fn is_animating(&self, now: std::time::Instant) -> bool {
        self.progress(now).is_some_and(|progress| progress < 1.)
    }

    pub(crate) fn amount(&self, now: std::time::Instant) -> f32 {
        let to = if self.open { 1. } else { 0. };
        match self.progress(now) {
            Some(progress) => {
                self.from + (to - self.from) * crate::ui_primitives::ease_out_cubic(progress)
            }
            None => to,
        }
    }

    pub(crate) fn toggle(&mut self, now: std::time::Instant) {
        self.from = self.amount(now);
        self.open = !self.open;
        self.started_at = (!crate::ui_primitives::reduce_motion()).then_some(now);
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.from = 0.;
        self.started_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_icon_tile_builds_with_a_single_tooltip() {
        for (idx, &icon) in AppIcon::ALL.iter().enumerate() {
            let _ = app_icon_tile(idx, icon, idx == 0, 1.0, Ink::of(crate::theme::ui_colors()));
        }
    }

    #[test]
    fn app_icon_picker_starts_folded() {
        let picker = AppIconPicker::default();
        let now = std::time::Instant::now();
        assert!(!picker.is_open());
        assert_eq!(picker.amount(now), 0.);
        assert!(!picker.is_animating(now));
    }

    #[test]
    fn app_icon_picker_unfolds_and_settles_open() {
        let mut picker = AppIconPicker::default();
        let start = std::time::Instant::now();
        picker.toggle(start);
        let midway = start + std::time::Duration::from_millis(100);
        let settled = start + std::time::Duration::from_secs(1);
        assert!(picker.is_open());
        assert!(picker.amount(midway) > 0. && picker.amount(midway) < 1.);
        assert!(picker.is_animating(midway));
        assert_eq!(picker.amount(settled), 1.);
        assert!(!picker.is_animating(settled));
    }

    #[test]
    fn app_icon_picker_folds_back_from_where_it_was() {
        let mut picker = AppIconPicker::default();
        let start = std::time::Instant::now();
        picker.toggle(start);
        let midway = start + std::time::Duration::from_millis(60);
        let reached = picker.amount(midway);
        picker.toggle(midway);
        assert!((picker.amount(midway) - reached).abs() < f32::EPSILON);
        let settled = midway + std::time::Duration::from_secs(1);
        assert_eq!(picker.amount(settled), 0.);
    }

    #[test]
    fn closing_the_app_icon_picker_folds_it_at_once() {
        let mut picker = AppIconPicker::default();
        let start = std::time::Instant::now();
        picker.toggle(start);
        picker.close();
        assert!(!picker.is_open());
        assert_eq!(picker.amount(start), 0.);
    }
}

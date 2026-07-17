//! Bottom-of-sidebar mode tabs + Settings popover. The CLI, Review, and
//! Agents sidebars share one persistent mode switch, with Settings kept as
//! a compact utility button at the end of the row.
//!
//! The popover state (`PaneFlowApp::sidebar_actions_menu_open`) is
//! shared because only one sidebar is rendered at a time (mode toggle
//! swaps the whole sidebar tree).

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, CursorStyle, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString, Styled,
    Transformation, div, percentage, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{select_item, select_menu_surface, with_alpha};
use crate::window_chrome::title_bar::{SelfUpdatePillState, SystemPackageKind, UpdatePillKind};

const SIDEBAR_UPDATE_SHIMMER_MS: u64 = 2600;
const SIDEBAR_UPDATE_IRIS_COLORS: [u32; 5] = [0x2f6fff, 0x1da8ff, 0x8ea7ff, 0xb68cff, 0xf2f7ff];

impl PaneFlowApp {
    /// Update CTA banner at the bottom of the sidebar, above the Settings
    /// trigger. Replaces the title-bar update pill in the cockpit modes
    /// (Cli/Agents), where the title bar is a rail-confined overlay with no
    /// room for pills. Same states, labels, icons, and dismiss rules as the
    /// title-bar pill (`title_bar.rs`); same mouse-DOWN dispatch (Wayland
    /// focus-stealing prevention silently drops the first on_click after a
    /// cold start - see the title-bar pill comment for the full story).
    /// `None` when no update is available.
    pub(crate) fn render_sidebar_update_banner(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let info = self.update_pill_info()?;
        let ui = crate::theme::ui_colors();

        let (label, busy, system_hint): (String, bool, bool) = match info.kind {
            UpdatePillKind::InApp(state) => match state {
                SelfUpdatePillState::Idle => (format!("v{} available", info.version), false, false),
                SelfUpdatePillState::Downloading => ("Downloading update…".into(), true, false),
                SelfUpdatePillState::Installing => ("Installing update…".into(), true, false),
                SelfUpdatePillState::ReadyToRestart => ("Restart Paneflow".into(), false, false),
                SelfUpdatePillState::Errored => ("Update failed".into(), false, false),
            },
            UpdatePillKind::SystemManaged(kind) => {
                let label = match kind {
                    SystemPackageKind::RpmOstree => "Update via rpm-ostree".to_string(),
                    SystemPackageKind::Other => "Update via package manager".to_string(),
                };
                (label, false, true)
            }
        };
        let is_ready_to_restart = matches!(
            info.kind,
            UpdatePillKind::InApp(SelfUpdatePillState::ReadyToRestart)
        );
        let dismissable = matches!(
            info.kind,
            UpdatePillKind::InApp(SelfUpdatePillState::Idle | SelfUpdatePillState::Errored)
                | UpdatePillKind::SystemManaged(_)
        );
        let label_element = if matches!(info.kind, UpdatePillKind::InApp(SelfUpdatePillState::Idle))
        {
            render_update_available_label(&format!("v{}", info.version), ui)
        } else {
            render_update_plain_label(&label, ui)
        };

        let leading_icon: AnyElement = if busy {
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/loader-circle.svg")
                .text_color(ui.muted)
                .with_animation(
                    "sidebar-update-spinner",
                    Animation::new(Duration::from_secs(1)).repeat(),
                    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                )
                .into_any_element()
        } else {
            svg()
                .size(px(14.))
                .flex_none()
                .path(if system_hint {
                    "icons/tool.svg"
                } else if is_ready_to_restart {
                    "icons/refresh.svg"
                } else {
                    "icons/download.svg"
                })
                .text_color(ui.muted)
                .into_any_element()
        };

        let mut banner = div()
            .id("sidebar-update-banner")
            .mx(px(6.))
            .mb(px(2.))
            .h(px(30.))
            .px(px(8.))
            .rounded(crate::app::constants::SIDEBAR_TAB_CORNER_RADIUS)
            .bg(crate::app::constants::sidebar_tab_active_background())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(leading_icon)
            .child(label_element);

        if dismissable {
            let muted = ui.muted;
            let text = ui.text;
            banner = banner.child(
                div()
                    .id("sidebar-update-dismiss")
                    .px(px(4.))
                    .text_color(muted)
                    .text_size(px(13.))
                    .font_weight(FontWeight::BOLD)
                    .hover(move |s| s.text_color(text))
                    // stop_propagation on BOTH mouse-down and click so the
                    // press never reaches the banner's StartSelfUpdate
                    // dispatch - hitting × must not start the update it
                    // just dismissed.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.handle_dismiss_update(&crate::DismissUpdate, window, cx);
                    }))
                    .child("×"),
            );
        }

        if busy {
            banner = banner.opacity(0.7);
        } else {
            banner = banner
                .when(system_hint, |d| d.opacity(0.8))
                .hover(move |s| {
                    let s = s.bg(crate::app::constants::sidebar_tab_active_background());
                    if system_hint { s.opacity(1.0) } else { s }
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.handle_start_self_update(&crate::StartSelfUpdate, window, cx);
                    }),
                );
        }

        Some(banner.into_any_element())
    }

    /// "IPC offline" notice at the bottom of the sidebar - the cockpit home
    /// of the title-bar IPC pill (same rail-confinement story as the update
    /// banner). Purely informational, like the original pill: no click
    /// handler. `None` while the IPC server is up.
    pub(crate) fn render_sidebar_ipc_banner(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.ipc_status.state() != crate::ipc::IpcState::Disabled {
            return None;
        }
        let ui = crate::theme::ui_colors();
        Some(
            div()
                .id("sidebar-ipc-banner")
                .mx(px(6.))
                .mb(px(2.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(ui.border)
                .bg(ui.subtle)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(
                    svg()
                        .size(px(14.))
                        .flex_none()
                        .path("icons/triangle-alert.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(ui.text)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child("IPC offline"),
                )
                .into_any_element(),
        )
    }

    /// Render the bottom footer: persistent interface tabs plus a compact
    /// Settings trigger. The mode switch stays visible after selection so the
    /// footer reads as primary navigation, while Settings remains an upward
    /// popover scoped to the current sidebar mode.
    pub(crate) fn render_sidebar_settings_footer(
        &self,
        items: Vec<SidebarMenuItem>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use paneflow_config::schema::AppMode;

        let ui = crate::theme::ui_colors();
        let settings_open = self.agents_view.sidebar_actions_menu_open;
        let mode = self.mode;

        let settings_trigger = div()
            .id("sidebar-settings-trigger")
            .flex_none()
            .h(px(30.))
            .w(px(30.))
            .rounded(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .when(settings_open, |d| {
                d.bg(crate::app::constants::sidebar_tab_active_background())
            })
            .hover(|s| s.bg(crate::app::constants::sidebar_tab_active_background()))
            .tooltip(move |_window, cx| {
                cx.new(|_| crate::app::sidebar::SidebarTooltip {
                    label: "Settings".into(),
                })
                .into()
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.agents_view.sidebar_actions_menu_open =
                    !this.agents_view.sidebar_actions_menu_open;
                this.agents_view.sidebar_mode_picker_open = false;
                cx.notify();
            }))
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path("icons/settings.svg")
                    .text_color(ui.muted),
            );

        let settings_popover: Option<AnyElement> = if settings_open {
            // Vertical menu opening upward from the trigger. Mirrors the
            // Settings "Shell" select menu (`components::select_menu`) that the
            // title-bar "Files" / "Help" dropdowns also use: the same elevated
            // surface, hairline border at 0.6 alpha, soft shadow, 10px radius
            // and 4px padding, so every app menu reads as one consistent menu
            // language. The container is open-coded (not `select_menu`) because
            // this popover stretches to the sidebar width via left/right, which
            // would fight `select_menu`'s fixed 200-280px clamp.
            let mut menu = div()
                .id("sidebar-settings-popover")
                .absolute()
                .left(px(6.))
                .right(px(6.))
                .bottom(px(44.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(4.))
                .rounded(px(10.))
                .bg(select_menu_surface(ui))
                .border_1()
                .border_color(with_alpha(ui.border, 0.6))
                // Click anywhere outside the popover (or its trigger)
                // dismisses it. Same pattern as `profile_menu.rs`.
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.agents_view.sidebar_actions_menu_open {
                        this.agents_view.sidebar_actions_menu_open = false;
                        cx.notify();
                    }
                }));
            for item in items {
                menu = menu.child(render_menu_item(item, ui, cx));
            }
            Some(menu.into_any_element())
        } else {
            None
        };

        type Activate = Box<dyn Fn(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>)>;
        let mode_button = |id: &'static str,
                           label: &'static str,
                           icon: &'static str,
                           is_active: bool,
                           activate: Activate| {
            // Equal-width compact segments keep the three primary surfaces
            // visible without letting the Settings utility reclaim the row.
            let fg = if is_active { ui.text } else { ui.muted };
            let mut button = div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .min_w_0()
                .px(px(2.))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .rounded(px(8.))
                .child(svg().size(px(13.)).flex_none().path(icon).text_color(fg))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(11.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(fg)
                        .truncate()
                        .child(label),
                );
            // Same interaction tint as the workspace cards / thread tabs.
            // Active and hover deliberately match in dark mode.
            if is_active {
                button = button.bg(crate::app::constants::sidebar_tab_active_background());
            } else {
                button = button
                    .hover(|s| s.bg(crate::app::constants::sidebar_tab_active_background()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        activate(this, window, cx);
                        this.agents_view.sidebar_actions_menu_open = false;
                        this.agents_view.sidebar_mode_picker_open = false;
                        cx.notify();
                    }));
            }
            button.into_any_element()
        };

        let footer_row: AnyElement = div()
            .id("sidebar-mode-tabs")
            .mx(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .child(mode_button(
                "sidebar-mode-cli",
                "CLI",
                "icons/terminal.svg",
                matches!(mode, AppMode::Cli),
                Box::new(|this, window, cx| this.enter_cli_mode(window, cx)),
            ))
            .child(mode_button(
                "sidebar-mode-diff",
                "Review",
                "icons/git-pull-request.svg",
                matches!(mode, AppMode::Diff),
                Box::new(|this, _window, cx| this.enter_diff_mode(cx)),
            ))
            .child(mode_button(
                "sidebar-mode-agents",
                "Agents",
                "icons/sparkles.svg",
                matches!(mode, AppMode::Agents),
                Box::new(|this, _window, cx| this.enter_agents_mode(cx)),
            ))
            .child(settings_trigger)
            .into_any_element();

        let mut footer = div().relative().flex_none().pt(px(6.)).pb(px(8.));
        if let Some(popover) = settings_popover {
            footer = footer.child(popover);
        }
        // Cockpit homes of the old title-bar pills, right above the Settings
        // trigger, shared by Cli + Agents: the "IPC offline" notice first,
        // then the update CTA banner.
        if let Some(banner) = self.render_sidebar_ipc_banner(cx) {
            footer = footer.child(banner);
        }
        if let Some(banner) = self.render_sidebar_update_banner(cx) {
            footer = footer.child(banner);
        }
        footer.child(footer_row).into_any_element()
    }

    /// Compatibility slot for existing sidebar call sites. The mode picker now
    /// lives in the shared footer above.
    pub(crate) fn render_mode_toggle(&self, _cx: &mut Context<Self>) -> AnyElement {
        div().into_any_element()
    }
}

pub(crate) type SidebarMenuAction =
    Box<dyn Fn(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>) + 'static>;

/// A single action row in the Settings popover.
pub(crate) struct SidebarMenuItem {
    pub id: SharedString,
    pub icon: &'static str,
    pub label: SharedString,
    pub on_click: SidebarMenuAction,
}

fn render_menu_item(
    item: SidebarMenuItem,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let handler = item.on_click;
    select_item(item.id, false, ui)
        .cursor(CursorStyle::Arrow)
        .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
            handler(this, w, cx);
            this.agents_view.sidebar_actions_menu_open = false;
            cx.notify();
        }))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(item.icon)
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(ui.text)
                .truncate()
                .child(item.label),
        )
        .into_any_element()
}

fn render_update_available_label(version: &str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .truncate()
        .flex()
        .flex_row()
        .child(render_update_shimmer_text(
            version,
            gpui::Hsla::from(gpui::rgb(SIDEBAR_UPDATE_IRIS_COLORS[2])).opacity(0.88),
        ))
        .child(div().text_color(ui.text).child(" available"))
        .into_any_element()
}

fn render_update_plain_label(label: &str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(ui.text)
        .truncate()
        .child(label.to_string())
        .into_any_element()
}

fn render_update_shimmer_text(label: &str, base_color: gpui::Hsla) -> AnyElement {
    let letter_count = label.chars().count() as f32;

    div()
        .flex()
        .flex_row()
        .children(label.chars().enumerate().map(|(index, ch)| {
            div()
                .text_color(base_color)
                .child(ch.to_string())
                .with_animation(
                    SharedString::from(format!("sidebar-update-shimmer-letter-{index}")),
                    Animation::new(Duration::from_millis(SIDEBAR_UPDATE_SHIMMER_MS)).repeat(),
                    move |letter, delta| {
                        letter.text_color(update_shimmer_color(
                            base_color,
                            index,
                            letter_count,
                            delta,
                        ))
                    },
                )
                .into_any_element()
        }))
        .into_any_element()
}

fn update_shimmer_color(
    base_color: gpui::Hsla,
    index: usize,
    letter_count: f32,
    delta: f32,
) -> gpui::Hsla {
    let width = letter_count.max(1.);
    let letter_phase = index as f32 / width;
    let phase = (delta + letter_phase * 0.32).fract();
    let iris_color = update_iris_color_at(phase);
    let highlight = ((phase * std::f32::consts::TAU).sin() + 1.) * 0.5;
    let hue = lerp(base_color.h, iris_color.h, 0.92);
    let saturation = lerp(base_color.s, iris_color.s, 0.9);
    let lightness = (lerp(base_color.l, iris_color.l, 0.9) + highlight * 0.035).min(0.94);
    let alpha = lerp(base_color.a, 0.98, 0.86);

    gpui::hsla(hue, saturation, lightness, alpha)
}

fn update_iris_color_at(phase: f32) -> gpui::Hsla {
    let palette_len = SIDEBAR_UPDATE_IRIS_COLORS.len();
    let scaled = phase * palette_len as f32;
    let start = scaled.floor() as usize % palette_len;
    let end = (start + 1) % palette_len;
    let amount = scaled.fract();
    let a = gpui::Hsla::from(gpui::rgb(SIDEBAR_UPDATE_IRIS_COLORS[start]));
    let b = gpui::Hsla::from(gpui::rgb(SIDEBAR_UPDATE_IRIS_COLORS[end]));

    gpui::hsla(
        lerp(a.h, b.h, amount),
        lerp(a.s, b.s, amount),
        lerp(a.l, b.l, amount),
        lerp(a.a, b.a, amount),
    )
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

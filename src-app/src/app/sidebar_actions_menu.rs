//! Bottom-of-sidebar mode tabs + Settings button. The CLI and Review
//! sidebars share one persistent mode switch, with Settings kept as
//! a compact utility button at the end of the row that opens the settings
//! surface directly - no intermediate menu.

use crate::ui_primitives::{ROW_RADIUS, TooltipDelayExt, squircle_skin};
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, SharedString, Styled, Transformation, div, percentage,
    prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};
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
                    .animated_hover(move |style, delta| {
                        style.text_color(lerp_color(muted, text, delta));
                    })
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

        let banner = if busy {
            banner.opacity(0.7).into_any_element()
        } else {
            let resting_opacity = if system_hint { 0.8 } else { 1.0 };
            banner
                .opacity(resting_opacity)
                .animated_hover(move |style, delta| {
                    style.opacity(lerp(resting_opacity, 1.0, delta));
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.handle_start_self_update(&crate::StartSelfUpdate, window, cx);
                    }),
                )
                .into_any_element()
        };

        Some(banner)
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
    /// footer reads as primary navigation, while Settings opens the settings
    /// surface in one click.
    pub(crate) fn render_sidebar_settings_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        use paneflow_config::schema::AppMode;

        let ui = crate::theme::ui_colors();
        let mode = self.mode;

        // Skinned exactly like a workspace card: the trigger rests on the
        // active tint while the settings surface is up, and lifts to the hover
        // tint otherwise, both on the rail's continuous corner.
        let active_bg = crate::app::constants::sidebar_tab_active_background();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let settings_open = self.settings_section.is_some();
        let settings_trigger = squircle_skin(
            div()
                .id("sidebar-settings-trigger")
                .flex_none()
                .h(px(30.))
                .w(px(30.))
                .flex()
                .items_center()
                .justify_center(),
            "sidebar-settings-trigger-group",
            ROW_RADIUS,
            settings_open.then_some(active_bg),
            (!settings_open).then_some(hover_bg),
        )
        .delayed_tooltip(move |_window, cx| {
            cx.new(|_| crate::app::sidebar::SidebarTooltip {
                label: "Settings".into(),
            })
            .into()
        })
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.open_settings_window(window, cx);
        }))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/settings.svg")
                .text_color(ui.muted),
        );

        type Activate = Box<dyn Fn(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>)>;
        let mode_button = |id: &'static str,
                           label: &'static str,
                           icon: &'static str,
                           is_active: bool,
                           activate: Activate| {
            // Equal-width compact segments keep both primary surfaces visible
            // without letting the Settings utility reclaim the row.
            let fg = if is_active { ui.text } else { ui.muted };
            // Same grammar as a workspace card: exactly one segment rests
            // filled - the current mode - and the others are pure hover
            // affordances one tint step below it.
            let button = squircle_skin(
                div()
                    .id(id)
                    .flex_1()
                    .h(px(30.))
                    .min_w_0()
                    .px(px(2.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(3.)),
                format!("{id}-group"),
                ROW_RADIUS,
                is_active.then_some(active_bg),
                (!is_active).then_some(hover_bg),
            )
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
            if is_active {
                button.into_any_element()
            } else {
                button
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        activate(this, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
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
            .child(settings_trigger)
            .into_any_element();

        let mut footer = div().relative().flex_none().pt(px(6.)).pb(px(8.));
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

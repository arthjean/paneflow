use gpui::{
    ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div, prelude::*, px,
};

use paneflow_mcp_install::{InstallKind, OverallState, StatusKind};

use crate::PaneFlowApp;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, hairline, section_header, setting_card, setting_text,
    with_alpha,
};
use crate::ui_primitives::AnimatedHoverExt;

impl PaneFlowApp {
    pub(crate) fn render_mcp_servers_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();

        let state = self
            .mcp_status
            .as_deref()
            .map(paneflow_mcp_install::overall_state);

        let (label, enabled): (SharedString, bool) = if self.mcp_busy {
            ("Installing…".into(), false)
        } else {
            match state {
                None => ("Checking…".into(), false),
                Some(OverallState::NoAgents) => ("No agents detected".into(), false),
                Some(OverallState::AllInstalled) => ("Reinstall".into(), true),
                Some(OverallState::NeedsRepair) => ("Repair".into(), true),
                Some(OverallState::NeedsInstall) => ("Install MCP bridge".into(), true),
            }
        };

        let button_bg = if enabled { ui.accent } else { ui.subtle };
        let button_hover_bg = if enabled {
            with_alpha(ui.accent, 0.85)
        } else {
            button_bg
        };
        let button = div()
            .id("mcp-install-btn")
            .flex_shrink_0()
            .px(px(12.))
            .py(px(6.))
            .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .bg(button_bg)
            .text_color(if enabled { gpui::white() } else { ui.muted })
            .animated_hover_bg(button_bg, button_hover_bg)
            .child(label)
            .when(enabled, |button| {
                button.on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.start_mcp_install(cx);
                }))
            });

        let header_row = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .child(setting_text(
                ui,
                "Read your panes from your agents",
                "Registers the bundled paneflow-mcp bridge with every detected CLI \
                 agent (Claude Code, Codex, Gemini, opencode) so they can read other \
                 panes' output. Idempotent, backed up, and only touches the paneflow \
                 entry. Re-run after an update if a path goes stale.",
            ))
            .child(button);

        let mut card = setting_card(ui).child(header_row);

        let recap_lines = self.mcp_recap_lines();
        if let Some(error) = self.mcp_install_error() {
            card = card.child(hairline(ui)).child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .text_color(danger_color())
                    .child(error),
            );
        }
        for (line, is_error) in recap_lines {
            card = card.child(hairline(ui)).child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(if is_error { danger_color() } else { ui.muted })
                    .child(line),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(section_header(ui, "MCP bridge"))
            .child(card)
    }

    fn mcp_install_error(&self) -> Option<SharedString> {
        match &self.mcp_install {
            Some(Err(msg)) => Some(SharedString::from(msg.clone())),
            _ => None,
        }
    }

    fn mcp_recap_lines(&self) -> Vec<(SharedString, bool)> {
        if let Some(Ok(results)) = &self.mcp_install {
            return results
                .iter()
                .map(|r| {
                    let (state, err) = match &r.kind {
                        InstallKind::Installed => ("installed", false),
                        InstallKind::Updated => ("updated", false),
                        InstallKind::AlreadyCurrent => ("already up to date", false),
                        InstallKind::SkippedAbsent => ("not detected", false),
                        InstallKind::Error(e) => {
                            return (format!("{}: error - {e}", r.label).into(), true);
                        }
                    };
                    (format!("{}: {state}", r.label).into(), err)
                })
                .collect();
        }
        match &self.mcp_status {
            Some(statuses) => statuses
                .iter()
                .map(|r| {
                    let (state, err) = match &r.kind {
                        StatusKind::NotDetected => ("not detected", false),
                        StatusKind::Installed { .. } => ("installed", false),
                        StatusKind::Stale { .. } => ("stale path - click Repair", false),
                        StatusKind::NeedsRepair { .. } => ("needs repair - click Repair", false),
                        StatusKind::NotInstalled => ("not installed", false),
                        StatusKind::Error(e) => {
                            return (format!("{}: error - {e}", r.label).into(), true);
                        }
                    };
                    (format!("{}: {state}", r.label).into(), err)
                })
                .collect(),
            None => Vec::new(),
        }
    }

    pub(crate) fn refresh_mcp_status(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = smol::unblock(|| {
                let bridge = crate::runtime_paths::bridge_binary_path();
                paneflow_mcp_install::status_all(bridge.as_deref())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.mcp_status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    fn start_mcp_install(&mut self, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (install, status) = smol::unblock(|| {
                let bridge = match crate::ai_hooks::extract::ensure_bridge_extracted() {
                    Ok(p) => Some(p),
                    Err(e) => {
                        log::warn!(
                            "settings: MCP bridge extraction failed ({e:#}); install may refuse"
                        );
                        crate::runtime_paths::bridge_binary_path()
                    }
                };
                let install = paneflow_mcp_install::install_all(bridge.as_deref());
                let status = paneflow_mcp_install::status_all(bridge.as_deref());
                (install, status)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.mcp_busy = false;
                this.mcp_install = Some(install);
                this.mcp_status = Some(status);
                cx.notify();
            });
        })
        .detach();
    }
}

fn danger_color() -> gpui::Hsla {
    gpui::rgb(0xE0_6C_75).into()
}

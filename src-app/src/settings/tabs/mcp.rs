use gpui::{
    AnyElement, ClickEvent, Context, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::*, px,
};

use paneflow_mcp_install::{InstallKind, OverallState, StatusKind};

use crate::PaneFlowApp;
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::{self, Block, SearchCard};
use crate::ui_primitives::AccessibleControlExt as _;

const PLUGINS_INTRO: &str = "Connect the coding agents on this computer to Paneflow.";
const AGENTS_HINT: &str = "Where each detected agent stands with the bridge.";

struct AgentLine {
    label: SharedString,
    state: SharedString,
    is_error: bool,
}

impl PaneFlowApp {
    pub(crate) fn render_mcp_heading(&self, title: gpui::Div) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(
                Ink::of(crate::theme::ui_colors()),
                PLUGINS_INTRO,
            ))
            .into_any_element()
    }

    pub(crate) fn render_mcp_servers_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);

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
        let tone = match state {
            Some(OverallState::AllInstalled) => Tone::Secondary,
            _ => Tone::Primary,
        };
        let button = kit::button_with(
            "mcp-install-btn",
            label,
            tone,
            Size::Small,
            self.mcp_busy,
            enabled || self.mcp_busy,
            ink,
        )
        .when(enabled, |button| {
            button.on_activate(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.start_mcp_install(cx);
            }))
        });

        let mut card = SearchCard::new().row(
            &search::MCP_BRIDGE,
            kit::row(
                ink,
                None,
                Text::new(search::MCP_BRIDGE.title)
                    .line(kit::line(ink, search::MCP_BRIDGE.description)),
                Some(button.into_any_element()),
            ),
        );
        if let Some(error) = self.mcp_install_error() {
            card = card.fixed(kit::row(
                ink,
                None,
                Text::new("The bridge could not be installed")
                    .danger(ink)
                    .line(kit::line(ink, error)),
                None,
            ));
        }
        let (matched, rows) = card.into_rows();

        let agents = self.mcp_agent_lines();
        let agents_section = (!agents.is_empty()).then(|| {
            kit::section(
                ink,
                "Agents",
                Some(AGENTS_HINT),
                kit::separated(
                    ink,
                    agents.into_iter().map(|line| {
                        kit::row(
                            ink,
                            None,
                            Text::new(line.label),
                            Some(
                                kit::line_in(
                                    if line.is_error {
                                        ink.danger
                                    } else {
                                        ink.secondary
                                    },
                                    line.state,
                                )
                                .text_right()
                                .into_any_element(),
                            ),
                        )
                        .into_any_element()
                    }),
                ),
            )
        });

        let mut block = Block::new("MCP bridge")
            .gap(kit::SECTION_GAP)
            .found(matched)
            .child(kit::section(
                ink,
                "MCP bridge",
                None,
                kit::search_group(ink, rows),
            ));
        if let Some(agents) = agents_section {
            block = block.child(agents);
        }
        block.finish()
    }

    fn mcp_install_error(&self) -> Option<SharedString> {
        match &self.mcp_install {
            Some(Err(msg)) => Some(SharedString::from(msg.clone())),
            _ => None,
        }
    }

    fn mcp_agent_lines(&self) -> Vec<AgentLine> {
        let line = |label: &str, state: String, is_error: bool| AgentLine {
            label: SharedString::from(label.to_string()),
            state: SharedString::from(state),
            is_error,
        };
        if let Some(Ok(results)) = &self.mcp_install {
            return results
                .iter()
                .map(|r| match &r.kind {
                    InstallKind::Installed => line(&r.label, "Installed".into(), false),
                    InstallKind::Updated => line(&r.label, "Updated".into(), false),
                    InstallKind::AlreadyCurrent => line(&r.label, "Up to date".into(), false),
                    InstallKind::SkippedAbsent => line(&r.label, "Not detected".into(), false),
                    InstallKind::Error(e) => line(&r.label, format!("Error: {e}"), true),
                })
                .collect();
        }
        match &self.mcp_status {
            Some(statuses) => statuses
                .iter()
                .map(|r| match &r.kind {
                    StatusKind::NotDetected => line(&r.label, "Not detected".into(), false),
                    StatusKind::Installed { .. } => line(&r.label, "Installed".into(), false),
                    StatusKind::Stale { .. } => {
                        line(&r.label, "Stale path, click Repair".into(), false)
                    }
                    StatusKind::NeedsRepair { .. } => {
                        line(&r.label, "Needs repair, click Repair".into(), false)
                    }
                    StatusKind::DisabledByUser { .. } => {
                        line(&r.label, "Disabled in the agent config".into(), false)
                    }
                    StatusKind::NotInstalled => line(&r.label, "Not installed".into(), false),
                    StatusKind::Error(e) => line(&r.label, format!("Error: {e}"), true),
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
                let install = paneflow_mcp_install::install_all(bridge.as_deref(), false);
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

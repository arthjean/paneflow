use gpui::{
    AnyElement, Context, Hsla, IntoElement, ParentElement, SharedString, Styled, div, img, px, rgb,
    svg,
};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::settings::components::{hairline, section_header, setting_card, toggle_row};

struct AgentToggleRow {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    agent: TerminalAgent,
    config_key: &'static str,
}

const AGENT_TOGGLE_ROWS: &[AgentToggleRow] = &[
    AgentToggleRow {
        id: "row-claude-visible",
        title: "Claude Code",
        description: "Show the Claude Code launcher button in every tab bar.",
        agent: TerminalAgent::ClaudeCode,
        config_key: "claude_code_button_visible",
    },
    AgentToggleRow {
        id: "row-codex-visible",
        title: "Codex",
        description: "Show the Codex launcher button in every tab bar.",
        agent: TerminalAgent::Codex,
        config_key: "codex_button_visible",
    },
    AgentToggleRow {
        id: "row-opencode-visible",
        title: "Opencode",
        description: "Show the Opencode launcher button in every tab bar.",
        agent: TerminalAgent::OpenCode,
        config_key: "opencode_button_visible",
    },
    AgentToggleRow {
        id: "row-pi-visible",
        title: "Pi",
        description: "Show the Pi launcher button in every tab bar.",
        agent: TerminalAgent::Pi,
        config_key: "pi_button_visible",
    },
    AgentToggleRow {
        id: "row-hermes-agent-visible",
        title: "Hermes Agent",
        description: "Show the Hermes Agent launcher button in every tab bar.",
        agent: TerminalAgent::Hermes,
        config_key: "hermes_agent_button_visible",
    },
    AgentToggleRow {
        id: "row-grok-visible",
        title: "Grok",
        description: "Show the Grok launcher button in every tab bar.",
        agent: TerminalAgent::Grok,
        config_key: "grok_button_visible",
    },
    AgentToggleRow {
        id: "row-amp-visible",
        title: "Amp",
        description: "Show the Amp launcher button in every tab bar.",
        agent: TerminalAgent::Amp,
        config_key: "amp_button_visible",
    },
    AgentToggleRow {
        id: "row-cursor-visible",
        title: "Cursor",
        description: "Show the Cursor launcher button in every tab bar.",
        agent: TerminalAgent::Cursor,
        config_key: "cursor_button_visible",
    },
    AgentToggleRow {
        id: "row-gemini-visible",
        title: "Gemini",
        description: "Show the Gemini launcher button in every tab bar.",
        agent: TerminalAgent::Gemini,
        config_key: "gemini_button_visible",
    },
    AgentToggleRow {
        id: "row-kiro-visible",
        title: "Kiro",
        description: "Show the Kiro launcher button in every tab bar.",
        agent: TerminalAgent::Kiro,
        config_key: "kiro_button_visible",
    },
    AgentToggleRow {
        id: "row-antigravity-visible",
        title: "Antigravity",
        description: "Show the Antigravity launcher button in every tab bar.",
        agent: TerminalAgent::Antigravity,
        config_key: "antigravity_button_visible",
    },
    AgentToggleRow {
        id: "row-copilot-visible",
        title: "Copilot",
        description: "Show the Copilot launcher button in every tab bar.",
        agent: TerminalAgent::Copilot,
        config_key: "copilot_button_visible",
    },
    AgentToggleRow {
        id: "row-codebuddy-visible",
        title: "CodeBuddy",
        description: "Show the CodeBuddy launcher button in every tab bar.",
        agent: TerminalAgent::CodeBuddy,
        config_key: "codebuddy_button_visible",
    },
    AgentToggleRow {
        id: "row-factory-visible",
        title: "Factory",
        description: "Show the Factory launcher button in every tab bar.",
        agent: TerminalAgent::Factory,
        config_key: "factory_button_visible",
    },
    AgentToggleRow {
        id: "row-qoder-visible",
        title: "Qoder",
        description: "Show the Qoder launcher button in every tab bar.",
        agent: TerminalAgent::Qoder,
        config_key: "qoder_button_visible",
    },
    AgentToggleRow {
        id: "row-openclaw-visible",
        title: "Openclaw",
        description: "Show the Openclaw launcher button in every tab bar.",
        agent: TerminalAgent::Openclaw,
        config_key: "openclaw_button_visible",
    },
];

impl PaneFlowApp {
    pub(crate) fn render_ai_agent_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let config = &self.cached_config;
        let ui = crate::theme::ui_colors();

        let mut buttons_card = setting_card(ui);
        for (idx, row) in AGENT_TOGGLE_ROWS.iter().enumerate() {
            if idx > 0 {
                buttons_card = buttons_card.child(hairline(ui));
            }
            buttons_card = buttons_card.child(toggle_row(
                row.id,
                row.title,
                row.description,
                Some(agent_icon_el(row.agent, ui)),
                row.agent.is_visible(config),
                row.config_key,
                ui,
                cx,
            ));
        }

        let buttons_section = div()
            .flex()
            .flex_col()
            .child(section_header(ui, "Tab bar buttons"))
            .child(buttons_card);

        div()
            .flex()
            .flex_col()
            .child(buttons_section)
            .child(div().h(px(180.)).flex_none())
    }
}

fn agent_icon_el(agent: TerminalAgent, ui: crate::theme::UiColors) -> AnyElement {
    let path = SharedString::from(agent.icon_path());
    if agent.icon_multicolor() {
        img(path).size(px(18.)).flex_none().into_any_element()
    } else {
        let tint: Hsla = agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text);
        svg()
            .size(px(18.))
            .flex_none()
            .path(path)
            .text_color(tint)
            .into_any_element()
    }
}

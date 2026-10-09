use gpui::{Context, Entity, SharedString};
use paneflow_host::program_status::DeclaredStatus;

use crate::PaneFlowApp;
use crate::terminal::TerminalView;

const CHIP_MAX_CHARS: usize = 48;

const LINE_MAX_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclaredState {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl DeclaredState {
    pub(crate) fn of(status: &DeclaredStatus) -> Option<Self> {
        match status.state.as_str() {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredChip {
    Fallback,
    Hidden,
    Show { label: SharedString, error: bool },
}

pub(crate) fn declared_chip(status: Option<&DeclaredStatus>) -> DeclaredChip {
    let Some((status, state)) =
        status.and_then(|status| Some((status, DeclaredState::of(status)?)))
    else {
        return DeclaredChip::Fallback;
    };
    let Some(base) = state_label(state, status.progress, status.kind.as_deref()) else {
        return DeclaredChip::Hidden;
    };
    let label = match one_line(&status.message).or_else(|| one_line(&status.title)) {
        Some(detail) => format!("{base} · {detail}"),
        None => base,
    };
    DeclaredChip::Show {
        label: truncate_chars(&label, CHIP_MAX_CHARS).into(),
        error: state == DeclaredState::Error,
    }
}

fn state_label(state: DeclaredState, progress: Option<u8>, kind: Option<&str>) -> Option<String> {
    match state {
        DeclaredState::Idle => None,
        DeclaredState::Working => Some(match progress {
            Some(percent) => format!("working {}%", percent.min(100)),
            None => "working".to_string(),
        }),
        DeclaredState::Blocked => Some(match kind_label(kind) {
            Some(kind) => format!("blocked: {kind}"),
            None => "blocked".to_string(),
        }),
        DeclaredState::Done => Some("done".to_string()),
        DeclaredState::Error => Some("error".to_string()),
    }
}

fn kind_label(kind: Option<&str>) -> Option<&'static str> {
    match kind? {
        "permission" => Some("permission"),
        "question" => Some("question"),
        "auth" => Some("auth"),
        _ => None,
    }
}

fn is_hidden_control(c: char) -> bool {
    matches!(c,
        '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{206A}'..='\u{206F}'
    )
}

pub(crate) fn one_line(raw: &str) -> Option<String> {
    let visible: String = raw.chars().filter(|c| !is_hidden_control(*c)).collect();
    let line: String = visible
        .split(['\n', '\r'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(LINE_MAX_CHARS)
        .collect();
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_string())
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(max.saturating_sub(1)).collect();
    kept.truncate(kept.trim_end().len());
    kept.push('…');
    kept
}

impl PaneFlowApp {
    pub(crate) fn refresh_declared_status(&mut self, cx: &mut Context<Self>) {
        for ws_idx in 0..self.workspaces.len() {
            for pane in self.workspaces[ws_idx].collect_panes() {
                let terminals: Vec<Entity<TerminalView>> =
                    pane.read(cx).terminals().cloned().collect();
                let mut pane_changed = false;
                for terminal in terminals {
                    let row = self.host_agents.row(&terminal.read(cx).terminal.session_id);
                    let declared = row.and_then(|row| row.declared_status.clone());
                    if terminal.read(cx).terminal.declared_status != declared {
                        terminal.update(cx, |view, _| view.terminal.declared_status = declared);
                        pane_changed = true;
                    }
                }
                if pane_changed {
                    pane.update(cx, |_, cx| cx.notify());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &str) -> DeclaredStatus {
        DeclaredStatus {
            state: state.to_string(),
            kind: None,
            progress: None,
            app: String::new(),
            title: String::new(),
            message: String::new(),
        }
    }

    type LabelCase = (
        DeclaredState,
        Option<u8>,
        Option<&'static str>,
        Option<&'static str>,
    );

    fn shown(chip: DeclaredChip) -> (String, bool) {
        match chip {
            DeclaredChip::Show { label, error } => (label.to_string(), error),
            other => panic!("expected a shown chip, got {other:?}"),
        }
    }

    #[test]
    fn every_declared_state_maps_to_its_own_label() {
        let table: &[LabelCase] = &[
            (DeclaredState::Working, None, None, Some("working")),
            (DeclaredState::Working, Some(40), None, Some("working 40%")),
            (
                DeclaredState::Working,
                Some(250),
                None,
                Some("working 100%"),
            ),
            (DeclaredState::Blocked, None, None, Some("blocked")),
            (
                DeclaredState::Blocked,
                None,
                Some("permission"),
                Some("blocked: permission"),
            ),
            (
                DeclaredState::Blocked,
                None,
                Some("question"),
                Some("blocked: question"),
            ),
            (
                DeclaredState::Blocked,
                None,
                Some("auth"),
                Some("blocked: auth"),
            ),
            (
                DeclaredState::Blocked,
                None,
                Some("telepathy"),
                Some("blocked"),
            ),
            (DeclaredState::Done, None, None, Some("done")),
            (DeclaredState::Error, None, None, Some("error")),
            (DeclaredState::Idle, None, None, None),
        ];
        for (state, progress, kind, expected) in table {
            assert_eq!(
                state_label(*state, *progress, *kind).as_deref(),
                *expected,
                "{state:?} {progress:?} {kind:?}"
            );
        }
    }

    #[test]
    fn every_wire_state_parses_and_an_unknown_one_does_not() {
        for (wire, state) in [
            ("idle", Some(DeclaredState::Idle)),
            ("working", Some(DeclaredState::Working)),
            ("blocked", Some(DeclaredState::Blocked)),
            ("done", Some(DeclaredState::Done)),
            ("error", Some(DeclaredState::Error)),
            ("clear", None),
            ("", None),
            ("WORKING", None),
        ] {
            assert_eq!(DeclaredState::of(&status(wire)), state, "{wire:?}");
        }
    }

    #[test]
    fn the_chip_names_the_state_and_why_it_is_blocked() {
        let mut blocked = status("blocked");
        blocked.kind = Some("permission".to_string());
        blocked.message = "Apply the plan?".to_string();
        assert_eq!(
            shown(declared_chip(Some(&blocked))),
            ("blocked: permission · Apply the plan?".to_string(), false)
        );

        let mut failed = status("error");
        failed.title = "cargo test".to_string();
        assert_eq!(
            shown(declared_chip(Some(&failed))),
            ("error · cargo test".to_string(), true)
        );

        let mut long = status("working");
        long.message = "x".repeat(200);
        let (label, _) = shown(declared_chip(Some(&long)));
        assert_eq!(label.chars().count(), CHIP_MAX_CHARS);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn a_session_without_a_declared_status_keeps_the_progress_chip() {
        assert_eq!(declared_chip(None), DeclaredChip::Fallback);
    }

    #[test]
    fn an_absent_or_unknown_state_falls_back_to_the_progress_chip() {
        for wire in ["", "paused", "clear"] {
            assert_eq!(
                declared_chip(Some(&status(wire))),
                DeclaredChip::Fallback,
                "{wire:?}"
            );
        }
        let absent: DeclaredStatus =
            serde_json::from_value(serde_json::json!({"message": "no state"})).unwrap();
        assert_eq!(declared_chip(Some(&absent)), DeclaredChip::Fallback);
    }

    #[test]
    fn a_declared_idle_hides_the_progress_chip_instead_of_falling_back() {
        assert_eq!(declared_chip(Some(&status("idle"))), DeclaredChip::Hidden);
    }

    #[test]
    fn displayed_text_drops_every_hidden_control_range_then_keeps_one_line() {
        let ranges: &[(u32, u32)] = &[
            (0x202A, 0x202E),
            (0x2066, 0x2069),
            (0x200B, 0x200F),
            (0x2028, 0x2028),
            (0x2029, 0x2029),
            (0x206A, 0x206F),
        ];
        for (start, end) in ranges {
            for code in *start..=*end {
                let hidden = char::from_u32(code).unwrap();
                assert_eq!(
                    one_line(&format!("Ap{hidden}ply")).as_deref(),
                    Some("Apply"),
                    "U+{code:04X}"
                );
            }
        }
        assert_eq!(
            one_line("first line\nsecond line").as_deref(),
            Some("first line")
        );
        assert_eq!(one_line("first\r\nsecond").as_deref(), Some("first"));
        assert_eq!(one_line("\u{1b}[31mred").as_deref(), Some("[31mred"));
        assert_eq!(one_line("  \u{202E}  "), None);
        assert_eq!(
            one_line(&"é".repeat(LINE_MAX_CHARS + 10)).map(|line| line.chars().count()),
            Some(LINE_MAX_CHARS)
        );
    }
}

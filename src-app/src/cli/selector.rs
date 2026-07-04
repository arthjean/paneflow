//! Target selector for the `paneflow` CLI (US-003).
//!
//! Resolves a `<target>` argument to a concrete `surface_id` by querying
//! `surface.list` once and filtering client-side, so `read`/`search`/`send`
//! address a pane uniformly by its id, its name, the process running in it
//! (`cmdline:<substr>`), or its working directory (`cwd:<path>`) - with one
//! place that produces the ambiguity / no-match errors.
//!
//! Cross-platform note: `cmdline:` matches the `cmd` field of `surface.list`,
//! which is the full foreground argv on Linux but only the executable basename
//! on macOS/Windows (see `pty_session::foreground_command`). `cwd:` and `name`
//! are the portable selectors when argv is unavailable (US-015 documents this).

use paneflow_ipc_client::IpcTransport;
use serde::Deserialize;
use serde_json::json;

use super::CliError;

/// One entry of the `surface.list` response. Lenient by design: every field
/// but `surface_id` is optional so a future server field can't break parsing.
#[derive(Debug, Deserialize)]
pub struct Surface {
    pub surface_id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub cmd: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Selector<'a> {
    Id(u64),
    Name(&'a str),
    Cmdline(&'a str),
    Cwd(&'a str),
}

fn parse_selector(raw: &str) -> Selector<'_> {
    if let Some(rest) = raw.strip_prefix("cmdline:") {
        return Selector::Cmdline(rest);
    }
    if let Some(rest) = raw.strip_prefix("cwd:") {
        return Selector::Cwd(rest);
    }
    if let Ok(id) = raw.parse::<u64>() {
        return Selector::Id(id);
    }
    Selector::Name(raw)
}

/// Fetch terminal surfaces via `surface.list`.
pub fn fetch_surfaces(client: &impl IpcTransport) -> Result<Vec<Surface>, CliError> {
    let result = client
        .call("surface.list", json!({}))
        .map_err(CliError::runtime)?;
    let surfaces = result.get("surfaces").cloned().unwrap_or_else(|| json!([]));
    serde_json::from_value(surfaces)
        .map_err(|e| CliError::runtime(format!("malformed surface.list response: {e}")))
}

/// Resolve a `<target>` string to a `surface_id` against the live surface list.
pub fn resolve_target(client: &impl IpcTransport, target: &str) -> Result<u64, CliError> {
    let surfaces = fetch_surfaces(client)?;
    resolve(parse_selector(target), &surfaces)
}

/// All `surface_id`s matching the selector (for `wait --any/--all`, US-014).
/// Errors only when nothing matches; never errors on ambiguity (the caller
/// wants the whole set).
pub fn resolve_all(client: &impl IpcTransport, target: &str) -> Result<Vec<u64>, CliError> {
    let surfaces = fetch_surfaces(client)?;
    let ids: Vec<u64> = matches_for(&parse_selector(target), &surfaces)
        .iter()
        .map(|s| s.surface_id)
        .collect();
    if ids.is_empty() {
        return Err(CliError::target(format!(
            "no pane matches target '{target}'"
        )));
    }
    Ok(ids)
}

fn resolve(selector: Selector<'_>, surfaces: &[Surface]) -> Result<u64, CliError> {
    let label = selector_label(&selector);
    pick_unique(&matches_for(&selector, surfaces), &label)
}

/// Every surface matching a selector. Shared by the unique-resolution path
/// (`resolve` -> `pick_unique`) and the all-matches path (`resolve_all`).
fn matches_for<'s>(selector: &Selector<'_>, surfaces: &'s [Surface]) -> Vec<&'s Surface> {
    match selector {
        Selector::Id(id) => surfaces.iter().filter(|s| s.surface_id == *id).collect(),
        Selector::Name(name) => matches_by_name(name, surfaces),
        Selector::Cmdline(sub) => {
            let needle = sub.to_lowercase();
            surfaces
                .iter()
                .filter(|s| {
                    s.cmd
                        .as_deref()
                        .is_some_and(|c| c.to_lowercase().contains(&needle))
                })
                .collect()
        }
        Selector::Cwd(path) => surfaces
            .iter()
            .filter(|s| {
                s.cwd
                    .as_deref()
                    .is_some_and(|c| c == *path || c.starts_with(*path))
            })
            .collect(),
    }
}

fn matches_by_name<'s>(name: &str, surfaces: &'s [Surface]) -> Vec<&'s Surface> {
    // Exact (case-insensitive) wins over prefix, so a pane named "claude" stays
    // reachable even when "claude-2" exists.
    let exact: Vec<&Surface> = surfaces
        .iter()
        .filter(|s| {
            s.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let lower = name.to_lowercase();
    surfaces
        .iter()
        .filter(|s| {
            s.name
                .as_deref()
                .is_some_and(|n| n.to_lowercase().starts_with(&lower))
        })
        .collect()
}

fn selector_label(selector: &Selector<'_>) -> String {
    match selector {
        Selector::Id(id) => id.to_string(),
        Selector::Name(name) => (*name).to_string(),
        Selector::Cmdline(sub) => format!("cmdline:{sub}"),
        Selector::Cwd(path) => format!("cwd:{path}"),
    }
}

/// Exactly-one or a dedicated target error. An ambiguous match lists the
/// candidates (id + name) rather than silently picking one (US-003 AC).
fn pick_unique(matches: &[&Surface], target: &str) -> Result<u64, CliError> {
    match matches {
        [] => Err(CliError::target(format!(
            "no pane matches target '{target}'"
        ))),
        [one] => Ok(one.surface_id),
        many => {
            let listed = many
                .iter()
                .map(|s| format!("{}({})", s.surface_id, s.name.as_deref().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::target(format!(
                "ambiguous target '{target}'; matches: {listed}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: u64, name: &str, cmd: &str, cwd: &str) -> Surface {
        Surface {
            surface_id: id,
            name: Some(name.to_string()),
            cmd: Some(cmd.to_string()),
            cwd: Some(cwd.to_string()),
        }
    }

    fn fixtures() -> Vec<Surface> {
        vec![
            surface(12, "claude", "claude --resume", "/home/a/proj-backend"),
            surface(18, "claude-2", "claude", "/home/a/proj-frontend"),
            surface(20, "cargo-run", "cargo run", "/home/a/proj-backend"),
        ]
    }

    #[test]
    fn parses_each_selector_kind() {
        assert_eq!(parse_selector("12"), Selector::Id(12));
        assert_eq!(parse_selector("cargo-run"), Selector::Name("cargo-run"));
        assert_eq!(
            parse_selector("cmdline:claude"),
            Selector::Cmdline("claude")
        );
        assert_eq!(parse_selector("cwd:/home/a"), Selector::Cwd("/home/a"));
    }

    #[test]
    fn id_resolves_when_present() {
        assert_eq!(resolve(Selector::Id(20), &fixtures()).unwrap(), 20);
    }

    #[test]
    fn id_missing_is_target_error() {
        let err = resolve(Selector::Id(99), &fixtures()).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("99"), "got: {}", err.message);
    }

    #[test]
    fn exact_name_wins_over_prefix() {
        // "claude" is also a prefix of "claude-2", but the exact match resolves.
        assert_eq!(resolve(Selector::Name("claude"), &fixtures()).unwrap(), 12);
    }

    #[test]
    fn unique_prefix_resolves() {
        assert_eq!(resolve(Selector::Name("cargo"), &fixtures()).unwrap(), 20);
    }

    #[test]
    fn ambiguous_cmdline_lists_candidates() {
        let err = resolve(Selector::Cmdline("claude"), &fixtures()).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("ambiguous"), "got: {}", err.message);
        assert!(err.message.contains("12") && err.message.contains("18"));
    }

    #[test]
    fn cwd_prefix_can_be_ambiguous() {
        // Two panes under /home/a/proj-backend.
        let err = resolve(Selector::Cwd("/home/a/proj-backend"), &fixtures()).unwrap_err();
        assert!(err.message.contains("ambiguous"), "got: {}", err.message);
    }

    #[test]
    fn no_match_is_target_error() {
        let err = resolve(Selector::Name("nope"), &fixtures()).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("no pane matches"));
    }

    #[test]
    fn matches_for_returns_every_match() {
        // The all-matches path (resolve_all / wait --any|--all) keeps every
        // candidate rather than erroring on ambiguity.
        let surfaces = fixtures();
        let m = matches_for(&Selector::Cmdline("claude"), &surfaces);
        let ids: Vec<u64> = m.iter().map(|s| s.surface_id).collect();
        assert_eq!(ids, vec![12, 18]);
    }

    #[test]
    fn cmdline_matches_basename_only_cmd() {
        // macOS/Windows expose `cmd` as the executable basename (no argv), so a
        // `cmdline:` selector must still match it (US-015 cross-platform).
        let surfaces = vec![surface(5, "agent", "claude", "/tmp")];
        assert_eq!(resolve(Selector::Cmdline("claude"), &surfaces).unwrap(), 5);
    }
}

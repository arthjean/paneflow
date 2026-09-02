use paneflow_ipc_client::IpcTransport;
use serde::Deserialize;
use serde_json::json;

use super::CliError;

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

fn parse_selector(raw: &str) -> Result<Selector<'_>, CliError> {
    if let Some(rest) = raw.strip_prefix("cmdline:") {
        if rest.is_empty() {
            return Err(CliError::target(
                "cmdline: selector needs a non-empty substring",
            ));
        }
        return Ok(Selector::Cmdline(rest));
    }
    if let Some(rest) = raw.strip_prefix("cwd:") {
        if rest.is_empty() {
            return Err(CliError::target("cwd: selector needs a non-empty path"));
        }
        return Ok(Selector::Cwd(rest));
    }
    if let Ok(id) = raw.parse::<u64>() {
        return Ok(Selector::Id(id));
    }
    Ok(Selector::Name(raw))
}

pub fn fetch_surfaces(client: &impl IpcTransport) -> Result<Vec<Surface>, CliError> {
    let result = client
        .call("surface.list", json!({}))
        .map_err(CliError::runtime)?;
    let surfaces = result.get("surfaces").cloned().unwrap_or_else(|| json!([]));
    serde_json::from_value(surfaces)
        .map_err(|e| CliError::runtime(format!("malformed surface.list response: {e}")))
}

pub fn resolve_target(client: &impl IpcTransport, target: &str) -> Result<u64, CliError> {
    let surfaces = fetch_surfaces(client)?;
    resolve(parse_selector(target)?, &surfaces)
}

pub fn resolve_all(client: &impl IpcTransport, target: &str) -> Result<Vec<u64>, CliError> {
    let surfaces = fetch_surfaces(client)?;
    let selector = parse_selector(target)?;
    let ids: Vec<u64> = matches_for(&selector, &surfaces)
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
                    .is_some_and(|c| cwd_selector_matches(path, c))
            })
            .collect(),
    }
}

fn cwd_selector_matches(selector: &str, cwd: &str) -> bool {
    let selector = normalize_path_selector(selector);
    let cwd = normalize_path_selector(cwd);
    cwd == selector
        || cwd
            .strip_prefix(&selector)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn normalize_path_selector(raw: &str) -> String {
    let mut normalized = raw.replace('\\', "/");
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    if cfg!(windows) {
        normalized.make_ascii_lowercase();
    }
    normalized
}

fn matches_by_name<'s>(name: &str, surfaces: &'s [Surface]) -> Vec<&'s Surface> {
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
        assert_eq!(parse_selector("12").unwrap(), Selector::Id(12));
        assert_eq!(
            parse_selector("cargo-run").unwrap(),
            Selector::Name("cargo-run")
        );
        assert_eq!(
            parse_selector("cmdline:claude").unwrap(),
            Selector::Cmdline("claude")
        );
        assert_eq!(
            parse_selector("cwd:/home/a").unwrap(),
            Selector::Cwd("/home/a")
        );
    }

    #[test]
    fn empty_prefixed_selectors_are_rejected() {
        assert_eq!(
            parse_selector("cmdline:").unwrap_err().code,
            super::super::EXIT_TARGET
        );
        assert_eq!(
            parse_selector("cwd:").unwrap_err().code,
            super::super::EXIT_TARGET
        );
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
        let err = resolve(Selector::Cwd("/home/a/proj-backend"), &fixtures()).unwrap_err();
        assert!(err.message.contains("ambiguous"), "got: {}", err.message);
    }

    #[test]
    fn cwd_selector_matches_path_components_not_sibling_prefixes() {
        let surfaces = vec![
            surface(1, "repo", "sh", "/home/a/repo"),
            surface(2, "child", "sh", "/home/a/repo/api"),
            surface(3, "sibling", "sh", "/home/a/repo2"),
        ];
        let ids: Vec<u64> = matches_for(&Selector::Cwd("/home/a/repo"), &surfaces)
            .iter()
            .map(|s| s.surface_id)
            .collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn cwd_selector_normalizes_separator_variants() {
        assert!(cwd_selector_matches(r"C:\dev\repo", "C:/dev/repo/api"));
    }

    #[test]
    fn no_match_is_target_error() {
        let err = resolve(Selector::Name("nope"), &fixtures()).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("no pane matches"));
    }

    #[test]
    fn matches_for_returns_every_match() {
        let surfaces = fixtures();
        let m = matches_for(&Selector::Cmdline("claude"), &surfaces);
        let ids: Vec<u64> = m.iter().map(|s| s.surface_id).collect();
        assert_eq!(ids, vec![12, 18]);
    }

    #[test]
    fn cmdline_matches_basename_only_cmd() {
        let surfaces = vec![surface(5, "agent", "claude", "/tmp")];
        assert_eq!(resolve(Selector::Cmdline("claude"), &surfaces).unwrap(), 5);
    }
}

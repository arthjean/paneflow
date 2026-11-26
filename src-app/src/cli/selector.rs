use std::time::{Duration, Instant};

use paneflow_ipc_client::IpcTransport;
use serde::Deserialize;
use serde_json::json;

use super::CliError;

const DRAW_POLL: Duration = Duration::from_millis(100);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Workspace,
    All,
}

impl Reach {
    pub const fn from_scope_all(all: bool) -> Self {
        if all { Self::All } else { Self::Workspace }
    }

    pub fn list_params(self) -> serde_json::Value {
        match self {
            Self::Workspace => json!({}),
            Self::All => json!({"scope": "all"}),
        }
    }
}

pub struct Listing {
    pub surfaces: Vec<Surface>,
    pub scoped: bool,
    pub reach: Reach,
}

pub fn listing_is_scoped(result: &serde_json::Value) -> bool {
    result
        .get("scope_workspace_id")
        .is_some_and(|scope| !scope.is_null())
        || result.get("scope_workspace").is_some()
}

pub fn fetch_surfaces(client: &impl IpcTransport, reach: Reach) -> Result<Listing, CliError> {
    let result = client
        .call("surface.list", reach.list_params())
        .map_err(CliError::runtime)?;
    let scoped = listing_is_scoped(&result);
    let surfaces = result.get("surfaces").cloned().unwrap_or_else(|| json!([]));
    let surfaces = serde_json::from_value(surfaces)
        .map_err(|e| CliError::runtime(format!("malformed surface.list response: {e}")))?;
    Ok(Listing {
        surfaces,
        scoped,
        reach,
    })
}

pub fn resolve_target(client: &impl IpcTransport, target: &str) -> Result<u64, CliError> {
    let selector = parse_selector(target)?;
    let listing = fetch_surfaces(client, Reach::All)?;
    let listing = if listing.scoped && matches_for(&selector, &listing.surfaces).is_empty() {
        match layout_rows(client) {
            Some(rows) if !matches_for(&selector, &rows).is_empty() => listing.widened_to(rows),
            _ => listing,
        }
    } else {
        listing
    };
    resolve(selector, &listing)
}

pub fn resolve_target_in(
    client: &impl IpcTransport,
    target: &str,
    reach: Reach,
) -> Result<u64, CliError> {
    let listing = fetch_surfaces(client, reach)?;
    resolve(parse_selector(target)?, &listing)
}

pub fn resolve_all(
    client: &impl IpcTransport,
    target: &str,
    reach: Reach,
) -> Result<Vec<u64>, CliError> {
    let listing = fetch_surfaces(client, reach)?;
    every_match(&parse_selector(target)?, &listing, target)
}

pub fn resolve_target_once_drawn(
    client: &impl IpcTransport,
    target: &str,
    deadline: Instant,
) -> Result<Option<u64>, CliError> {
    let selector = parse_selector(target)?;
    match surfaces_once_drawn(client, &selector, deadline)? {
        Some(listing) => resolve(selector, &listing).map(Some),
        None => Ok(None),
    }
}

pub fn resolve_all_once_drawn(
    client: &impl IpcTransport,
    target: &str,
    deadline: Instant,
) -> Result<Option<Vec<u64>>, CliError> {
    let selector = parse_selector(target)?;
    let Some(listing) = surfaces_once_drawn(client, &selector, deadline)? else {
        return Ok(None);
    };
    every_match(&selector, &listing, target).map(Some)
}

fn every_match(
    selector: &Selector<'_>,
    listing: &Listing,
    target: &str,
) -> Result<Vec<u64>, CliError> {
    let ids: Vec<u64> = matches_for(selector, &listing.surfaces)
        .iter()
        .map(|s| s.surface_id)
        .collect();
    if ids.is_empty() {
        return Err(no_match(target, listing));
    }
    Ok(ids)
}

impl Listing {
    fn widened_to(self, surfaces: Vec<Surface>) -> Self {
        Self {
            surfaces,
            scoped: false,
            reach: self.reach,
        }
    }
}

fn no_match(target: &str, listing: &Listing) -> CliError {
    match (listing.scoped, listing.reach) {
        (false, _) => CliError::target(format!("no pane matches target '{target}'")),
        (true, Reach::Workspace) => CliError::target(format!(
            "no pane of the calling pane's workspace matches target '{target}'; \
             panes of other workspaces need --scope all with ipc.orchestration set to true in paneflow.json"
        )),
        (true, Reach::All) => CliError::target(format!(
            "no pane of the calling pane's workspace matches target '{target}'; \
             panes of other workspaces need ipc.orchestration set to true in paneflow.json"
        )),
    }
}

fn surfaces_once_drawn(
    client: &impl IpcTransport,
    selector: &Selector<'_>,
    deadline: Instant,
) -> Result<Option<Listing>, CliError> {
    loop {
        let surfaces = fetch_surfaces(client, Reach::All)?;
        if !matches_for(selector, &surfaces.surfaces).is_empty() {
            return Ok(Some(surfaces));
        }
        let Some(rows) = layout_rows(client).filter(|rows| !matches_for(selector, rows).is_empty())
        else {
            return Ok(Some(surfaces));
        };
        if surfaces.scoped && drawn_outside_the_listing(client, selector, &rows) {
            return Ok(Some(surfaces.widened_to(rows)));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(DRAW_POLL);
    }
}

fn drawn_outside_the_listing(
    client: &impl IpcTransport,
    selector: &Selector<'_>,
    rows: &[Surface],
) -> bool {
    matches_for(selector, rows).iter().all(|surface| {
        client
            .call(
                "surface.status",
                json!({ "surface_id": surface.surface_id }),
            )
            .is_ok_and(|status| status.get("error").is_none())
    })
}

fn layout_rows(client: &impl IpcTransport) -> Option<Vec<Surface>> {
    let view = client.call("layout.view", json!({})).ok()?;
    Some(
        super::headless::worker_layout_of(&view["view"])
            .surfaces
            .into_iter()
            .map(|surface| Surface {
                surface_id: surface.surface_id,
                name: surface.name,
                cwd: None,
                cmd: None,
            })
            .collect(),
    )
}

fn resolve(selector: Selector<'_>, listing: &Listing) -> Result<u64, CliError> {
    let label = selector_label(&selector);
    pick_unique(&matches_for(&selector, &listing.surfaces), &label)
        .map_err(|error| error.unwrap_or_else(|| no_match(&label, listing)))
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

fn pick_unique(matches: &[&Surface], target: &str) -> Result<u64, Option<CliError>> {
    match matches {
        [] => Err(None),
        [one] => Ok(one.surface_id),
        many => {
            let listed = many
                .iter()
                .map(|s| format!("{}({})", s.surface_id, s.name.as_deref().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join(", ");
            Err(Some(CliError::target(format!(
                "ambiguous target '{target}'; matches: {listed}"
            ))))
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

    fn listing(surfaces: Vec<Surface>, scoped: bool) -> Listing {
        Listing {
            surfaces,
            scoped,
            reach: Reach::Workspace,
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
        assert_eq!(
            resolve(Selector::Id(20), &listing(fixtures(), false)).unwrap(),
            20
        );
    }

    #[test]
    fn id_missing_is_target_error() {
        let err = resolve(Selector::Id(99), &listing(fixtures(), false)).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("99"), "got: {}", err.message);
    }

    #[test]
    fn exact_name_wins_over_prefix() {
        assert_eq!(
            resolve(Selector::Name("claude"), &listing(fixtures(), false)).unwrap(),
            12
        );
    }

    #[test]
    fn unique_prefix_resolves() {
        assert_eq!(
            resolve(Selector::Name("cargo"), &listing(fixtures(), false)).unwrap(),
            20
        );
    }

    #[test]
    fn ambiguous_cmdline_lists_candidates() {
        let err = resolve(Selector::Cmdline("claude"), &listing(fixtures(), false)).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("ambiguous"), "got: {}", err.message);
        assert!(err.message.contains("12") && err.message.contains("18"));
    }

    #[test]
    fn cwd_prefix_can_be_ambiguous() {
        let err = resolve(
            Selector::Cwd("/home/a/proj-backend"),
            &listing(fixtures(), false),
        )
        .unwrap_err();
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
        let err = resolve(Selector::Name("nope"), &listing(fixtures(), false)).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(err.message.contains("no pane matches"));
    }

    #[test]
    fn a_miss_in_a_scoped_listing_names_the_way_to_other_workspaces() {
        let err = resolve(Selector::Name("nope"), &listing(fixtures(), true)).unwrap_err();
        assert_eq!(err.code, super::super::EXIT_TARGET);
        assert!(
            err.message.contains("calling pane's workspace"),
            "{}",
            err.message
        );
        assert!(err.message.contains("--scope all"), "{}", err.message);
        assert!(err.message.contains("ipc.orchestration"), "{}", err.message);
        let err =
            every_match(&Selector::Name("nope"), &listing(fixtures(), true), "nope").unwrap_err();
        assert!(err.message.contains("--scope all"), "{}", err.message);
    }

    #[test]
    fn a_listing_is_scoped_when_the_window_or_the_host_says_so() {
        assert!(listing_is_scoped(&json!({"scope_workspace_id": 4})));
        assert!(listing_is_scoped(&json!({"scope_workspace": null})));
        assert!(!listing_is_scoped(&json!({"scope_workspace_id": null})));
        assert!(!listing_is_scoped(&json!({"surfaces": []})));
        assert_eq!(Reach::All.list_params(), json!({"scope": "all"}));
        assert_eq!(Reach::Workspace.list_params(), json!({}));
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
        assert_eq!(
            resolve(Selector::Cmdline("claude"), &listing(surfaces, false)).unwrap(),
            5
        );
    }

    struct ScopedWindow {
        drawn: Vec<u64>,
    }

    impl IpcTransport for ScopedWindow {
        fn call(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, String> {
            let pane = |id: u64, name: &str| {
                json!({"kind": "pane", "id": id + 100, "surfaces": [
                    {"id": id, "definition": {"surface_type": "terminal", "custom_name": name}}
                ]})
            };
            match method {
                "surface.list" => Ok(json!({
                    "scope_workspace_id": 1,
                    "surfaces": [{"surface_id": 4, "name": "own"}],
                })),
                "layout.view" => Ok(json!({"view": {"state": {
                    "active_workspace": 0,
                    "workspaces": [
                        {"id": 1, "title": "alpha", "tabs": [{"id": 10, "root": pane(4, "own")}]},
                        {"id": 2, "title": "beta", "tabs": [
                            {"id": 20, "root": {"kind": "split", "children": [
                                {"node": pane(9, "foreign")},
                                {"node": pane(11, "pending")},
                            ]}}
                        ]},
                    ],
                }}})),
                "surface.status" => {
                    let id = params["surface_id"].as_u64().unwrap_or_default();
                    if self.drawn.contains(&id) {
                        Ok(json!({"surface_id": id}))
                    } else {
                        Ok(json!({"error": "surface not found"}))
                    }
                }
                other => Err(format!("unexpected {other}")),
            }
        }
    }

    #[test]
    fn an_unscoped_verb_still_resolves_a_pane_of_another_workspace_by_id_or_name() {
        let window = ScopedWindow { drawn: vec![4, 9] };
        assert_eq!(resolve_target(&window, "4").unwrap(), 4);
        assert_eq!(resolve_target(&window, "9").unwrap(), 9);
        assert_eq!(resolve_target(&window, "foreign").unwrap(), 9);
        let err = resolve_target(&window, "cwd:/elsewhere").unwrap_err();
        assert!(err.message.contains("ipc.orchestration"), "{}", err.message);
        assert!(!err.message.contains("--scope all"), "{}", err.message);
        let err = resolve_target_in(&window, "9", Reach::Workspace).unwrap_err();
        assert!(err.message.contains("--scope all"), "{}", err.message);
    }

    #[test]
    fn a_wait_on_a_drawn_pane_of_another_workspace_resolves_at_once_and_an_undrawn_one_waits() {
        let window = ScopedWindow { drawn: vec![4, 9] };
        let soon = Instant::now() + Duration::from_secs(30);
        assert_eq!(
            resolve_target_once_drawn(&window, "foreign", soon).unwrap(),
            Some(9)
        );
        assert_eq!(
            resolve_all_once_drawn(&window, "9", soon).unwrap(),
            Some(vec![9])
        );
        assert_eq!(
            resolve_target_once_drawn(&window, "pending", Instant::now()).unwrap(),
            None
        );
    }
}

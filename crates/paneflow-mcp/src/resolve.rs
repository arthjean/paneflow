//! Name → surface_id resolution for the bridge tools (US-009).
//!
//! The Paneflow IPC server resolves a surface name by exact match only. The
//! bridge does better: exact → case-insensitive → unique prefix, with
//! ambiguity and no-match errors that list the candidates so the agent can
//! re-target. All pure and unit-tested.

use serde_json::Value;

/// Minimal view of a surface entry from `surface.list` - only what name
/// resolution needs.
#[derive(Debug, Clone)]
pub struct SurfaceRef {
    pub surface_id: u64,
    pub name: String,
}

/// Parse one `surfaces[]` entry from a `surface.list` result. Returns `None`
/// for entries missing the required `surface_id` / `name`.
pub fn surface_ref_from_json(v: &Value) -> Option<SurfaceRef> {
    Some(SurfaceRef {
        surface_id: v.get("surface_id").and_then(Value::as_u64)?,
        name: v.get("name").and_then(Value::as_str)?.to_string(),
    })
}

/// Resolve a name query to a single `surface_id`, in precedence order:
/// exact → case-insensitive exact → unique case-insensitive prefix.
/// Ambiguity or no match yields a candidate-listing error.
pub fn resolve_target(surfaces: &[SurfaceRef], query: &str) -> Result<u64, String> {
    if query.trim().is_empty() {
        return Err("surface name must not be empty".to_string());
    }

    let exact: Vec<&SurfaceRef> = surfaces.iter().filter(|s| s.name == query).collect();
    match exact.as_slice() {
        [one] => return Ok(one.surface_id),
        [] => {}
        many => return Err(ambiguous(many, query)),
    }

    let ci: Vec<&SurfaceRef> = surfaces
        .iter()
        .filter(|s| s.name.eq_ignore_ascii_case(query))
        .collect();
    match ci.as_slice() {
        [one] => return Ok(one.surface_id),
        [] => {}
        many => return Err(ambiguous(many, query)),
    }

    let q = query.to_ascii_lowercase();
    let prefix: Vec<&SurfaceRef> = surfaces
        .iter()
        .filter(|s| s.name.to_ascii_lowercase().starts_with(&q))
        .collect();
    match prefix.as_slice() {
        [one] => Ok(one.surface_id),
        [] => Err(no_match(surfaces, query)),
        many => Err(ambiguous(many, query)),
    }
}

fn ambiguous(candidates: &[&SurfaceRef], query: &str) -> String {
    let list = candidates
        .iter()
        .map(|s| format!("{} (surface_id {})", s.name, s.surface_id))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "surface name '{query}' is ambiguous across {} surfaces: [{list}]; pass a more specific name or a numeric surface_id",
        candidates.len()
    )
}

fn no_match(surfaces: &[SurfaceRef], query: &str) -> String {
    let available = surfaces
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("no surface matches '{query}'; available: [{available}]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(id: u64, name: &str) -> SurfaceRef {
        SurfaceRef {
            surface_id: id,
            name: name.to_string(),
        }
    }

    #[test]
    fn exact_unique_match() {
        let set = [s(1, "cargo-run"), s(2, "vite")];
        assert_eq!(resolve_target(&set, "vite"), Ok(2));
    }

    #[test]
    fn case_insensitive_match() {
        let set = [s(1, "cargo-run"), s(2, "Vite")];
        assert_eq!(resolve_target(&set, "vite"), Ok(2));
    }

    #[test]
    fn unique_prefix_match() {
        let set = [s(1, "cargo-run@paneflow"), s(2, "vite")];
        assert_eq!(resolve_target(&set, "cargo"), Ok(1));
    }

    #[test]
    fn ambiguous_prefix_lists_candidates() {
        let set = [s(1, "cargo-run@paneflow"), s(2, "cargo-run@web")];
        let err = resolve_target(&set, "cargo").expect_err("ambiguous");
        assert!(err.contains("ambiguous"), "got: {err}");
        assert!(err.contains("surface_id 1"));
        assert!(err.contains("surface_id 2"));
    }

    #[test]
    fn exact_wins_over_prefix_siblings() {
        // "cargo-run" exactly matches one entry even though it's also a prefix
        // of "cargo-run-2"; the exact match must short-circuit.
        let set = [s(1, "cargo-run"), s(2, "cargo-run-2")];
        assert_eq!(resolve_target(&set, "cargo-run"), Ok(1));
    }

    #[test]
    fn no_match_lists_available() {
        let set = [s(1, "cargo-run"), s(2, "vite")];
        let err = resolve_target(&set, "nope").expect_err("no match");
        assert!(err.contains("no surface matches"), "got: {err}");
        assert!(err.contains("cargo-run"));
        assert!(err.contains("vite"));
    }

    #[test]
    fn empty_query_is_error() {
        let set = [s(1, "cargo-run")];
        let err = resolve_target(&set, "").expect_err("empty");
        assert!(err.contains("must not be empty"), "got: {err}");
        let err = resolve_target(&set, "   ").expect_err("blank");
        assert!(err.contains("must not be empty"), "got: {err}");
    }

    #[test]
    fn surface_ref_from_json_parses_full_entry() {
        let v = json!({
            "surface_id": 42u64,
            "name": "cargo-run",
            "title": "cargo run",
            "cwd": "/home/a/paneflow",
            "cmd": "cargo run",
            "workspace": 0
        });
        let r = surface_ref_from_json(&v).expect("some");
        assert_eq!(r.surface_id, 42);
        assert_eq!(r.name, "cargo-run");
    }

    #[test]
    fn surface_ref_from_json_rejects_missing_fields() {
        assert!(surface_ref_from_json(&json!({"name": "x"})).is_none());
        assert!(surface_ref_from_json(&json!({"surface_id": 1u64})).is_none());
    }
}

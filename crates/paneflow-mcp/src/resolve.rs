#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceRef {
    pub surface_id: u64,
    pub name: String,
}

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
}

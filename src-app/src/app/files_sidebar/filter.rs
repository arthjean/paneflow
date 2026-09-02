use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::app::files_tree::{self, FileNode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FilterRow<'a> {
    pub node: &'a FileNode,
    pub rel: String,
    pub highlight: Option<Range<usize>>,
}

pub(super) fn filter_rows<'a>(
    root: &Path,
    children: &'a HashMap<PathBuf, Vec<FileNode>>,
    lowered_needle: &str,
) -> Vec<FilterRow<'a>> {
    if lowered_needle.is_empty() {
        return Vec::new();
    }
    let root_str = root.to_string_lossy();
    let mut out: Vec<FilterRow<'a>> = Vec::new();
    let mut buf = String::new();
    for (dir, listing) in children {
        if listing.is_empty() {
            continue;
        }
        buf.clear();
        let dir_str = dir.to_string_lossy();
        match dir_str.strip_prefix(root_str.as_ref()) {
            Some(rest) => buf.push_str(rest.trim_start_matches(std::path::is_separator)),
            None => buf.push_str(&files_tree::workspace_relative_path(root, dir)),
        }
        if !buf.is_empty() {
            buf.push(std::path::MAIN_SEPARATOR);
        }
        let dir_len = buf.len();
        for node in listing {
            if node.is_dir {
                continue;
            }
            buf.truncate(dir_len);
            match node.path.file_name().map(|name| name.to_string_lossy()) {
                Some(name) => buf.push_str(&name),
                None => continue,
            }
            let Some(highlight) = find_ignore_case(&buf, lowered_needle) else {
                continue;
            };
            out.push(FilterRow {
                node,
                rel: buf.clone(),
                highlight: Some(highlight),
            });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

fn match_positions(haystack: &str, lowered_needle: &str) -> Option<(usize, usize)> {
    if lowered_needle.is_empty() {
        return None;
    }
    let mut lowered = String::with_capacity(haystack.len());
    let mut map: Vec<(usize, usize)> = Vec::with_capacity(haystack.len());
    for (orig_idx, ch) in haystack.char_indices() {
        map.push((lowered.len(), orig_idx));
        for lc in ch.to_lowercase() {
            lowered.push(lc);
        }
    }
    map.push((lowered.len(), haystack.len()));

    let lo_start = lowered.find(lowered_needle)?;
    let lo_end = lo_start + lowered_needle.len();

    let start = map
        .binary_search_by_key(&lo_start, |&(lo, _)| lo)
        .ok()
        .map(|i| map[i].1)?;
    let end = map
        .binary_search_by_key(&lo_end, |&(lo, _)| lo)
        .ok()
        .map(|i| map[i].1)?;
    Some((start, end))
}

fn find_ignore_case(haystack: &str, lowered_needle: &str) -> Option<Range<usize>> {
    if lowered_needle.is_empty() {
        return None;
    }
    if !haystack.is_ascii() || !lowered_needle.is_ascii() {
        return match_positions(haystack, lowered_needle).map(|(start, end)| start..end);
    }
    let hay = haystack.as_bytes();
    let needle = lowered_needle.as_bytes();
    if needle.len() > hay.len() {
        return None;
    }
    let first = needle[0];
    let last_start = hay.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        let offset = hay[i..=last_start]
            .iter()
            .position(|byte| byte.to_ascii_lowercase() == first)?;
        let start = i + offset;
        if hay[start..start + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(start..start + needle.len());
        }
        i = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn match_positions_finds_substring_byte_range() {
        assert_eq!(match_positions("Refactor sidebar", "side"), Some((9, 13)));
        assert_eq!(match_positions("Bug Fix", "bug"), Some((0, 3)));
        assert_eq!(match_positions("anything", "xyz"), None);
        assert_eq!(match_positions("anything", ""), None);
        assert_eq!(match_positions("ab", "abcdef"), None);
    }

    #[test]
    fn match_positions_slice_is_safe_to_index() {
        let title = "Refactor sidebar";
        let (s, e) = match_positions(title, "side").expect("match");
        assert_eq!(&title[..s], "Refactor ");
        assert_eq!(&title[s..e], "side");
        assert_eq!(&title[e..], "bar");
    }

    #[test]
    fn match_positions_maps_non_ascii_offsets_to_original() {
        let title = "Café au lait";
        let (s, e) = match_positions(title, "fé").expect("match");
        assert_eq!(&title[s..e], "fé", "range must slice the original cleanly");
        assert_eq!(&title[..s], "Ca");

        let title2 = "Éclair";
        let (s2, e2) = match_positions(title2, "é").expect("match");
        assert_eq!(
            &title2[s2..e2],
            "É",
            "lowered 'é' maps back to original 'É'"
        );
        assert_eq!(s2, 0);

        let title3 = "straße";
        let (s3, e3) = match_positions(title3, "ße").expect("match");
        assert_eq!(&title3[s3..e3], "ße");
    }
    use super::*;
    use std::collections::HashSet;

    fn file(path: PathBuf) -> FileNode {
        FileNode {
            path,
            is_dir: false,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        }
    }

    fn dir(path: PathBuf) -> FileNode {
        FileNode {
            path,
            is_dir: true,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        }
    }

    fn fixture() -> (PathBuf, HashMap<PathBuf, Vec<FileNode>>) {
        let root = PathBuf::from("/w");
        let src = root.join("src");
        let mut children = HashMap::new();
        children.insert(
            root.clone(),
            vec![dir(src.clone()), file(root.join("README.md"))],
        );
        children.insert(
            src.clone(),
            vec![file(src.join("Widget.rs")), file(src.join("main.rs"))],
        );
        (root, children)
    }

    #[test]
    fn matches_on_the_relative_path_not_only_the_name() {
        let (root, children) = fixture();
        let rows = filter_rows(&root, &children, "src");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.rel.contains("src")));
    }

    #[test]
    fn matching_is_case_insensitive_both_ways() {
        let (root, children) = fixture();
        let rows = filter_rows(&root, &children, "widget");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].rel.ends_with("Widget.rs"));
        assert_eq!(filter_rows(&root, &children, "readme").len(), 1);
    }

    #[test]
    fn directories_are_excluded_and_results_are_sorted() {
        let (root, children) = fixture();
        let rows = filter_rows(&root, &children, "e");
        assert!(rows.iter().all(|r| !r.node.is_dir));
        let rels: Vec<&str> = rows.iter().map(|r| r.rel.as_str()).collect();
        let mut sorted = rels.clone();
        sorted.sort_unstable();
        assert_eq!(rels, sorted);
    }

    #[test]
    fn spans_collapsed_directories_so_it_differs_from_flatten_visible() {
        let (root, children) = fixture();
        let expanded: HashSet<PathBuf> = HashSet::from([root.clone()]);
        let visible = files_tree::flatten_visible(&root, &expanded, &children);
        assert!(
            !visible.iter().any(|row| row.node.path.ends_with("main.rs")),
            "main.rs lives in a collapsed directory, so the tree must not show it"
        );
        let rows = filter_rows(&root, &children, "main.rs");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn filtering_does_not_touch_the_fold_state() {
        let (root, children) = fixture();
        let expanded: HashSet<PathBuf> = HashSet::from([root.clone(), root.join("src")]);
        let before = files_tree::flatten_visible(&root, &expanded, &children);

        let _ = filter_rows(&root, &children, "rs");
        let _ = filter_rows(&root, &children, "");
        let _ = filter_rows(&root, &children, "nothing-matches-this");

        let after = files_tree::flatten_visible(&root, &expanded, &children);
        assert_eq!(before, after);
        assert_eq!(expanded.len(), 2);
    }

    #[test]
    fn empty_needle_yields_nothing() {
        let (root, children) = fixture();
        assert!(filter_rows(&root, &children, "").is_empty());
    }

    #[test]
    fn no_match_yields_an_empty_vector() {
        let (root, children) = fixture();
        assert!(filter_rows(&root, &children, "zzz").is_empty());
    }

    #[test]
    fn highlight_range_slices_the_relative_path_cleanly() {
        let (root, children) = fixture();
        let rows = filter_rows(&root, &children, "widget");
        let row = &rows[0];
        let range = row.highlight.clone().expect("a hit must carry a range");
        assert_eq!(&row.rel[range.clone()], "Widget");
        assert!(row.rel.is_char_boundary(range.start));
        assert!(row.rel.is_char_boundary(range.end));
    }

    #[test]
    fn non_ascii_paths_match_and_highlight_safely() {
        let root = PathBuf::from("/w");
        let mut children = HashMap::new();
        children.insert(root.clone(), vec![file(root.join("Étude.rs"))]);
        let rows = filter_rows(&root, &children, "étude");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        let range = row.highlight.clone().expect("a hit must carry a range");
        assert!(row.rel.is_char_boundary(range.start));
        assert!(row.rel.is_char_boundary(range.end));
    }

    #[test]
    fn fifty_thousand_entries_filter_under_the_frame_budget() {
        let root = PathBuf::from("/w");
        let mut children: HashMap<PathBuf, Vec<FileNode>> = HashMap::new();
        for d in 0..500 {
            let dir_path = root.join(format!("crate_{d}")).join("src");
            let listing = (0..100)
                .map(|f| file(dir_path.join(format!("module_{f}.rs"))))
                .collect();
            children.insert(dir_path, listing);
        }
        let total: usize = children.values().map(Vec::len).sum();
        assert_eq!(total, 50_000);

        let start = std::time::Instant::now();
        let rows = filter_rows(&root, &children, "module_42.rs");
        let elapsed = start.elapsed();

        assert_eq!(rows.len(), 500);
        let budget_ms = if cfg!(debug_assertions) { 64 } else { 16 };
        assert!(
            elapsed < std::time::Duration::from_millis(budget_ms),
            "filtering 50 000 entries took {:.2}ms, over the {budget_ms}ms budget",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

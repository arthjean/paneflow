//! Sidebar search / filter logic for US-012 of
//! `tasks/prd-agents-view.md`.
//!
//! Lives in its own submodule so the matching rules stay testable
//! without spinning up GPUI: every public function here takes plain
//! data (`&str`, `&[Thread]`, ...) and returns a boolean / index. The
//! render path imports these helpers and only deals with element
//! emission.

use crate::project::{AgentsTarget, Project, Thread};

/// Case-insensitive substring match. `lowered_needle` MUST already be
/// `to_lowercase()`-ed by the caller -- on a workspace with N projects
/// and P threads/project the previous "lowercase inside" form burned
/// N*P needle allocations per keystroke (audit P1-4). Empty needle
/// matches everything; the caller short-circuits before this is
/// called, but the behaviour is documented anyway for symmetry.
#[inline]
pub(crate) fn matches(haystack: &str, lowered_needle: &str) -> bool {
    if lowered_needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(lowered_needle)
}

/// Should this project appear at all under the given filter?
///
/// Rule per AC #2: "a project header is visible if any of its threads
/// match OR if its own title matches". Empty filter -> always visible.
pub(crate) fn project_visible(project: &Project, lowered_needle: &str) -> bool {
    if lowered_needle.is_empty() {
        return true;
    }
    if matches(&project.title, lowered_needle) {
        return true;
    }
    project
        .threads
        .iter()
        .any(|t| matches(&t.title, lowered_needle))
}

/// Should this thread row appear under the given filter?
///
/// Rule per AC #2: "a thread row is visible if its title contains the
/// input (case-insensitive substring)". Empty filter -> always
/// visible. When the project itself matches but no thread does, the
/// caller still wants to render the children so the user can drill
/// in; [`thread_visible_in_project`] folds both signals.
pub(crate) fn thread_visible_in_project(
    thread: &Thread,
    project: &Project,
    lowered_needle: &str,
) -> bool {
    if lowered_needle.is_empty() {
        return true;
    }
    if matches(&thread.title, lowered_needle) {
        return true;
    }
    // Surface every thread when the project title is the match: the
    // user typed the project name, they want to see what is inside.
    matches(&project.title, lowered_needle)
}

/// US-009: should this free chat row appear under the given filter?
/// A chat has no parent project, so the rule is simply "the chat title
/// contains the needle". Empty needle -> always visible.
pub(crate) fn chat_visible(chat: &Thread, lowered_needle: &str) -> bool {
    if lowered_needle.is_empty() {
        return true;
    }
    matches(&chat.title, lowered_needle)
}

/// First target whose row matches, in the same order the sidebar renders:
/// pinned rows, project rows, then free chats. Used by Enter in the search
/// field to jump to the first visible hit.
pub(crate) fn first_matching_target(
    projects: &[Project],
    chats: &[Thread],
    lowered_needle: &str,
) -> Option<AgentsTarget> {
    if lowered_needle.is_empty() {
        return None;
    }
    for (project_idx, project) in projects.iter().enumerate() {
        for thread_idx in (0..project.threads.len()).rev() {
            let thread = &project.threads[thread_idx];
            if thread.pinned && thread_visible_in_project(thread, project, lowered_needle) {
                return Some(AgentsTarget::Thread {
                    project_idx,
                    thread_idx,
                });
            }
        }
    }
    for chat_idx in (0..chats.len()).rev() {
        let chat = &chats[chat_idx];
        if chat.pinned && chat_visible(chat, lowered_needle) {
            return Some(AgentsTarget::Chat { chat_idx });
        }
    }
    for (p_idx, project) in projects.iter().enumerate() {
        for t_idx in (0..project.threads.len()).rev() {
            let thread = &project.threads[t_idx];
            if thread_visible_in_project(thread, project, lowered_needle) {
                return Some(AgentsTarget::Thread {
                    project_idx: p_idx,
                    thread_idx: t_idx,
                });
            }
        }
    }
    for chat_idx in (0..chats.len()).rev() {
        if chat_visible(&chats[chat_idx], lowered_needle) {
            return Some(AgentsTarget::Chat { chat_idx });
        }
    }
    None
}

/// Are there ZERO matching projects/threads/chats? Used by the render path
/// to swap the list for the empty-state row from AC #7.
///
/// US-009: extended to the free `chats` source - the rail shows the
/// "no matches" hint only when NOTHING across all three sections
/// (Pinned/Projects/Chats) hits.
pub(crate) fn nothing_matches(
    projects: &[Project],
    chats: &[Thread],
    lowered_needle: &str,
) -> bool {
    if lowered_needle.is_empty() {
        return false;
    }
    !projects.iter().any(|p| project_visible(p, lowered_needle))
        && !chats.iter().any(|c| chat_visible(c, lowered_needle))
}

/// US-021: byte-range of the first case-insensitive substring hit of
/// `lowered_needle` inside `haystack`, suitable for splitting a string
/// into `[before, match, after]` for highlight rendering. Returns
/// `None` when the needle is empty, longer than the haystack, or
/// doesn't match.
///
/// `lowered_needle` MUST already be `to_lowercase()`-ed by the caller.
/// The match preserves the haystack's original byte boundaries so the
/// caller can slice safely (`&haystack[..start]`, `&haystack[start..end]`,
/// `&haystack[end..]`). The lowered haystack/needle are only used to
/// locate the hit -- the slices returned point into the original.
pub(crate) fn match_positions(haystack: &str, lowered_needle: &str) -> Option<(usize, usize)> {
    if lowered_needle.is_empty() {
        return None;
    }
    // U-012: `to_lowercase()` can change byte length and even char count for
    // non-ASCII text (İ→i̇ is 1→2 chars, ß→ss is 1→2 bytes), so locating the
    // hit in the lowered string and transferring that byte offset to the
    // ORIGINAL drifts on non-ASCII titles (the old form fell back to "no
    // highlight" via char-boundary guards). Build the lowered haystack while
    // recording, at each original char start, the (lowered_offset,
    // original_offset) pair, then map the hit back to a valid original
    // boundary. For ASCII this is identical to the original byte indices.
    let mut lowered = String::with_capacity(haystack.len());
    let mut map: Vec<(usize, usize)> = Vec::with_capacity(haystack.len());
    for (orig_idx, ch) in haystack.char_indices() {
        map.push((lowered.len(), orig_idx));
        for lc in ch.to_lowercase() {
            lowered.push(lc);
        }
    }
    // Sentinel so a match ending exactly at end-of-string maps cleanly.
    map.push((lowered.len(), haystack.len()));

    let lo_start = lowered.find(lowered_needle)?;
    let lo_end = lo_start + lowered_needle.len();

    // Map lowered byte offsets back to original byte offsets. A hit that
    // begins or ends in the MIDDLE of a lowered multi-byte expansion (e.g.
    // inside the "ss" a lowered ß produced) has no clean original boundary -
    // render no highlight rather than slice mid-codepoint. `map` is sorted by
    // lowered offset, so binary-search it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::TerminalAgent;
    use crate::project::Project;

    fn project_with_threads(title: &str, titles: &[&str]) -> Project {
        let mut p = Project::new(title, "/tmp");
        for t in titles {
            p.threads.push(crate::project::Thread::new_terminal(
                *t,
                "/tmp",
                Some(TerminalAgent::ClaudeCode),
            ));
        }
        p
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches("anything", ""));
        let p = project_with_threads("Paneflow", &["A", "B"]);
        assert!(project_visible(&p, ""));
        assert!(thread_visible_in_project(&p.threads[0], &p, ""));
    }

    #[test]
    fn matches_is_case_insensitive() {
        // Callers must pre-lower the needle (US-010 contract): the
        // haystack is lowered here, the needle is taken as-is.
        assert!(matches("Paneflow", "pane"));
        assert!(matches("paneflow", "flow"));
        assert!(!matches("paneflow", "xyz"));
    }

    #[test]
    fn project_visible_when_thread_matches() {
        let p = project_with_threads("Other", &["Bug fix", "Refactor"]);
        // Project title does not contain "bug", but its first thread
        // does -> the header must still show so the matched thread
        // can be reached.
        assert!(project_visible(&p, "bug"));
    }

    #[test]
    fn thread_surfaces_when_only_project_title_matches() {
        // User typed the project title -> they want to see all
        // children, not the empty headers-only collapse.
        let p = project_with_threads("Paneflow", &["Bug fix", "Refactor"]);
        // "paneflow" matches project only.
        assert!(thread_visible_in_project(&p.threads[0], &p, "paneflow"));
        assert!(thread_visible_in_project(&p.threads[1], &p, "paneflow"));
    }

    #[test]
    fn first_matching_target_returns_the_first_visible_hit() {
        let projects = vec![
            project_with_threads("Alpha", &["nope", "nope"]),
            project_with_threads("Beta", &["nope", "MATCH", "MATCH"]),
            project_with_threads("Gamma", &["MATCH"]),
        ];
        assert_eq!(
            first_matching_target(&projects, &[], "match"),
            Some(AgentsTarget::Thread {
                project_idx: 1,
                thread_idx: 2
            })
        );
    }

    #[test]
    fn first_matching_target_considers_chats() {
        let projects = vec![project_with_threads("Alpha", &["one", "two"])];
        let chats = vec![crate::project::Thread::new_terminal(
            "scratch session",
            "/home/me",
            None,
        )];
        assert_eq!(
            first_matching_target(&projects, &chats, "scratch"),
            Some(AgentsTarget::Chat { chat_idx: 0 })
        );
    }

    #[test]
    fn match_positions_finds_substring_byte_range() {
        // Simple ASCII match.
        assert_eq!(match_positions("Refactor sidebar", "side"), Some((9, 13)));
        // Case-insensitive: needle is already lowered (US-010
        // contract), haystack mixed.
        assert_eq!(match_positions("Bug Fix", "bug"), Some((0, 3)));
        // No match returns None.
        assert_eq!(match_positions("anything", "xyz"), None);
        // Empty needle returns None (caller short-circuits but the
        // contract is "no highlight to render").
        assert_eq!(match_positions("anything", ""), None);
        // Needle longer than haystack: None.
        assert_eq!(match_positions("ab", "abcdef"), None);
    }

    #[test]
    fn match_positions_slice_is_safe_to_index() {
        // The returned byte range must always be a valid UTF-8 slice
        // boundary in the original haystack, so the render path can
        // safely split into [before, match, after] without panicking.
        let title = "Refactor sidebar";
        let (s, e) = match_positions(title, "side").expect("match");
        assert_eq!(&title[..s], "Refactor ");
        assert_eq!(&title[s..e], "side");
        assert_eq!(&title[e..], "bar");
    }

    #[test]
    fn match_positions_maps_non_ascii_offsets_to_original() {
        // U-012: the hit's byte range must index the ORIGINAL string, even
        // when `to_lowercase()` changed byte lengths before the match.
        // "Café" - the needle "fé" follows the multi-byte 'é' position.
        let title = "Café au lait";
        let (s, e) = match_positions(title, "fé").expect("match");
        assert_eq!(&title[s..e], "fé", "range must slice the original cleanly");
        assert_eq!(&title[..s], "Ca");

        // A leading uppercase multi-byte char: needle "é" against "Éclair".
        let title2 = "Éclair";
        let (s2, e2) = match_positions(title2, "é").expect("match");
        assert_eq!(
            &title2[s2..e2],
            "É",
            "lowered 'é' maps back to original 'É'"
        );
        assert_eq!(s2, 0);

        // German ß lowercases to itself (already lowercase) - a plain
        // multi-byte match still slices the original safely.
        let title3 = "straße";
        let (s3, e3) = match_positions(title3, "ße").expect("match");
        assert_eq!(&title3[s3..e3], "ße");
    }

    #[test]
    fn nothing_matches_when_no_project_or_thread_hits() {
        let projects = vec![project_with_threads("Alpha", &["one", "two"])];
        assert!(!nothing_matches(&projects, &[], ""));
        assert!(!nothing_matches(&projects, &[], "one"));
        assert!(nothing_matches(&projects, &[], "xyzzy"));
    }

    #[test]
    fn chat_visible_matches_chat_title_only() {
        let chat = crate::project::Thread::new_terminal("Refactor auth", "/home/me", None);
        // Empty needle -> always visible.
        assert!(chat_visible(&chat, ""));
        // Case-insensitive substring on the title (needle pre-lowered).
        assert!(chat_visible(&chat, "auth"));
        assert!(chat_visible(&chat, "refactor"));
        assert!(!chat_visible(&chat, "xyz"));
    }

    #[test]
    fn nothing_matches_considers_chats_too() {
        // US-009: a hit that lives ONLY in a chat must keep the rail from
        // collapsing to the "no matches" hint.
        let projects = vec![project_with_threads("Alpha", &["one", "two"])];
        let chats = vec![crate::project::Thread::new_terminal(
            "scratch session",
            "/home/me",
            None,
        )];
        // Needle matches the chat, not any project/thread.
        assert!(!nothing_matches(&projects, &chats, "scratch"));
        // Needle matches nothing in either source.
        assert!(nothing_matches(&projects, &chats, "xyzzy"));
    }

    #[test]
    fn filter_completes_in_under_50ms_at_50_projects_x_100_threads() {
        // PRD AC #5: "Filter results render in under 50ms for 5 000
        // total threads across 50 projects". The matcher itself runs
        // way under that budget -- this test asserts the algorithmic
        // bound holds, not the GPUI render time (which would be
        // verified manually with dev tools).
        let projects: Vec<Project> = (0..50)
            .map(|p| {
                let titles: Vec<String> = (0..100).map(|t| format!("Thread {p}-{t}")).collect();
                let titles_ref: Vec<&str> = titles.iter().map(String::as_str).collect();
                project_with_threads(&format!("Project {p}"), &titles_ref)
            })
            .collect();
        let total_threads: usize = projects.iter().map(|p| p.threads.len()).sum();
        assert_eq!(total_threads, 5_000);

        let start = std::time::Instant::now();
        let _ = nothing_matches(&projects, &[], "10");
        let _ = first_matching_target(&projects, &[], "10");
        let elapsed = start.elapsed();
        // Generous bound -- the substring matcher should hit
        // sub-millisecond in practice. CI noise & debug builds can
        // push this up; 50 ms is the PRD budget.
        assert!(
            elapsed.as_millis() < 50,
            "filter pass took {} ms, budget is 50",
            elapsed.as_millis()
        );
    }
}

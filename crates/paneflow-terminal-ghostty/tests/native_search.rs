#![cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    )
))]

use paneflow_terminal_ghostty::{
    DisplayTerminal, NativeSearchSnapshot, TerminalAppearance, WindowSize,
};

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn terminal(cols: usize, rows: usize, history: usize) -> DisplayTerminal {
    let mut terminal = DisplayTerminal::new(
        WindowSize::new(cols, rows, 8, 16).unwrap(),
        history,
        TerminalAppearance::default(),
    )
    .unwrap();
    terminal
        .set_scrollback_max_bytes(Some(history.saturating_mul(1024)))
        .unwrap();
    terminal
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture search must fail immediately"
)]
fn complete(terminal: &mut DisplayTerminal) -> NativeSearchSnapshot {
    for _ in 0..10_000 {
        let snapshot = terminal
            .search_step_with_rail_refresh(true)
            .unwrap()
            .unwrap();
        if snapshot.complete {
            return snapshot;
        }
    }
    unreachable!("native search did not complete")
}

#[test]
fn deferred_rail_preserves_live_results_and_refreshes_without_more_output() {
    let mut terminal = terminal(80, 8, 10_000);
    terminal
        .feed(format!("marker\r\n{}", "filler\r\n".repeat(20)).as_bytes())
        .unwrap();
    terminal.set_search_query("marker").unwrap();
    let before = complete(&mut terminal);
    terminal.feed(b"another line\r\n").unwrap();
    let deferred = terminal
        .search_step_with_rail_refresh(false)
        .unwrap()
        .unwrap();
    assert!(deferred.rail_pending);
    assert_eq!(deferred.total_matches, before.total_matches);
    assert!(std::sync::Arc::ptr_eq(
        &deferred.rail_offsets,
        &before.rail_offsets
    ));
    let after = complete(&mut terminal);
    assert!(!after.rail_pending);
    assert_ne!(after.rail_offsets, before.rail_offsets);
    assert_eq!(after.total_matches, before.total_matches);
}

#[test]
fn finds_deep_history_and_tracks_selection_through_output() {
    let mut terminal = terminal(80, 8, 30_000);
    terminal.feed(b"old-marker\r\n").unwrap();
    for _ in 0..20_000 {
        terminal.feed(b"filler\r\n").unwrap();
    }
    assert_eq!(terminal.snapshot().unwrap().history_size, 19_994);
    terminal.set_search_query("OLD-marker").unwrap();
    let before = complete(&mut terminal);
    assert_eq!(before.total_matches, 1);
    assert!(before.selected_match.as_ref().unwrap().start.line < -19_000);
    assert!(
        before
            .viewport_matches
            .contains(before.selected_match.as_ref().unwrap())
    );
    assert_eq!(before.selected, Some(0));
    terminal.feed(b"old-marker\r\n").unwrap();
    let after = complete(&mut terminal);
    assert_eq!(after.total_matches, 2);
    assert_eq!(after.selected, Some(0));
    assert_eq!(
        after.selected_match.as_ref().unwrap().start.line,
        before.selected_match.as_ref().unwrap().start.line - 1
    );
    terminal.search_select(true).unwrap();
    assert_eq!(complete(&mut terminal).selected, Some(1));
}

#[test]
fn finds_soft_wrapped_text_before_and_after_resize() {
    let mut terminal = terminal(10, 4, 100);
    terminal.feed(b"prefix-long-needle-suffix").unwrap();
    terminal.set_search_query("long-needle").unwrap();
    let before = complete(&mut terminal);
    assert_eq!(before.total_matches, 1);
    assert!(before.viewport_matches[0].start.line < before.viewport_matches[0].end.line);
    terminal
        .resize(WindowSize::new(30, 4, 8, 16).unwrap())
        .unwrap();
    let after = complete(&mut terminal);
    assert_eq!(after.total_matches, 1);
    assert_eq!(
        after.viewport_matches[0].start.line,
        after.viewport_matches[0].end.line
    );
}

#[test]
fn pruning_removes_old_matches_without_reselecting() {
    let mut terminal = terminal(20, 3, 10);
    terminal.feed(b"marker\r\n").unwrap();
    assert_eq!(terminal.snapshot().unwrap().history_size, 0);
    terminal.set_search_query("marker").unwrap();
    assert_eq!(complete(&mut terminal).selected, Some(0));
    for _ in 0..20_000 {
        terminal.feed(b"filler\r\n").unwrap();
    }
    let retained = terminal.snapshot().unwrap().history_size;
    assert!(
        retained < 19_998,
        "fixture did not prune history: {retained}"
    );
    let snapshot = complete(&mut terminal);
    assert_eq!(snapshot.total_matches, 0);
    assert!(snapshot.viewport_matches.is_empty());
    assert!(snapshot.rail_offsets.is_empty());
    assert_eq!(snapshot.selected, None);
}

#[test]
fn alternate_screen_restores_primary_results() {
    let mut terminal = terminal(20, 4, 100);
    terminal.feed(b"marker primary\r\n").unwrap();
    terminal.set_search_query("marker").unwrap();
    let primary = complete(&mut terminal);
    assert_eq!(primary.total_matches, 1);
    terminal
        .feed(b"\x1b[?1049hmarker alternate\r\nmarker")
        .unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 2);
    terminal.feed(b"\x1b[?1049l").unwrap();
    assert_eq!(complete(&mut terminal), primary);
}

#[test]
fn clearing_and_replacing_queries_releases_search_state() {
    let mut terminal = terminal(20, 4, 100);
    terminal.feed("Hello Éclair".as_bytes()).unwrap();
    terminal.set_search_query("HELLO").unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 1);
    terminal.set_search_query("éclair").unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 0);
    terminal.set_search_query("Éclair").unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 1);
    terminal.clear_search();
    assert!(
        terminal
            .search_step_with_rail_refresh(true)
            .unwrap()
            .is_none()
    );
    terminal.set_search_query("Hello").unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 1);
    terminal.set_search_query("").unwrap();
    assert!(
        terminal
            .search_step_with_rail_refresh(true)
            .unwrap()
            .is_none()
    );
    terminal.set_search_query("Hello").unwrap();
    complete(&mut terminal);
}

#[test]
fn native_search_total_stays_constant_across_viewport_scroll() {
    let mut terminal = terminal(80, 8, 1_000);
    for row in 0..300 {
        terminal
            .feed(if row % 100 == 0 {
                b"smoke\r\n"
            } else {
                b"filler\r\n"
            })
            .unwrap();
    }
    terminal.set_search_query("smoke").unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 3);
    for scroll in [
        paneflow_terminal_ghostty::Scroll::Bottom,
        paneflow_terminal_ghostty::Scroll::Delta(150),
    ] {
        terminal.scroll(scroll);
        assert_eq!(complete(&mut terminal).total_matches, 3);
    }
    terminal.scroll_to_viewport_row(0).unwrap();
    assert_eq!(complete(&mut terminal).total_matches, 3);
}

#[test]
fn native_search_total_tracks_alternate_screen_redraws() {
    let mut terminal = terminal(80, 8, 1_000);
    terminal.feed(b"\x1b[?1049h").unwrap();
    terminal.set_search_query("smoke").unwrap();
    for count in [3, 1, 2] {
        terminal.feed(b"\x1b[2J\x1b[H").unwrap();
        for _ in 0..count {
            terminal.feed(b"smoke\r\n").unwrap();
        }
        assert_eq!(complete(&mut terminal).total_matches, count);
        assert_eq!(terminal.snapshot().unwrap().history_size, 0);
    }
}

#[test]
fn navigation_reuses_rail_and_cells_within_the_same_viewport() {
    let mut terminal = terminal(80, 12, 3_000);
    terminal
        .feed("marker\r\n".repeat(2_000).as_bytes())
        .unwrap();
    terminal.set_search_query("marker").unwrap();
    let before = complete(&mut terminal);
    let cells = terminal.snapshot().unwrap();
    assert_eq!(before.total_matches, 2_000);
    assert!(before.viewport_matches.len() <= 12);
    assert_eq!(before.rail_offsets.len(), 2_000);

    terminal.search_select(true).unwrap();
    let after = complete(&mut terminal);
    let unchanged = terminal.snapshot().unwrap();
    assert_eq!(after.selected, Some(1_998));
    assert!(std::sync::Arc::ptr_eq(
        &before.rail_offsets,
        &after.rail_offsets
    ));
    assert_eq!(cells.display_offset, unchanged.display_offset);
    assert!(unchanged.dirty_rows.iter().all(|dirty| !dirty));
    assert!(std::sync::Arc::ptr_eq(&cells.cells, &unchanged.cells));
}

#[test]
fn repeated_navigation_moves_one_match_and_keeps_the_selection_visible() {
    let mut terminal = terminal(80, 8, 1_000);
    terminal.feed("marker\r\n".repeat(100).as_bytes()).unwrap();
    terminal.set_search_query("marker").unwrap();
    let initial = complete(&mut terminal);
    let mut expected = 99;
    for previous in std::iter::repeat_n(true, 110).chain(std::iter::repeat_n(false, 110)) {
        terminal.search_select(previous).unwrap();
        expected = if previous {
            (expected + 99) % 100
        } else {
            (expected + 1) % 100
        };
        let search = complete(&mut terminal);
        let content = terminal.snapshot().unwrap();
        assert_eq!(search.total_matches, 100);
        assert_eq!(search.selected, Some(expected));
        assert!(search.viewport_matches.len() <= 8);
        assert!(std::sync::Arc::ptr_eq(
            &initial.rail_offsets,
            &search.rail_offsets
        ));
        let selected = search.selected_match.unwrap();
        let row = i64::from(selected.start.line) + content.display_offset as i64;
        assert!((0..8).contains(&row));
        assert!(search.viewport_matches.contains(&selected));
    }
}

#[test]
fn rail_refreshes_when_output_moves_matches_without_changing_the_total() {
    let mut terminal = terminal(80, 8, 100);
    terminal.feed(b"\x1b[?1049hmarker\r\n\r\nmarker").unwrap();
    terminal.set_search_query("marker").unwrap();
    let before = complete(&mut terminal);
    terminal
        .feed(b"\x1b[2J\x1b[H\r\nmarker\r\n\r\nmarker")
        .unwrap();
    let after = complete(&mut terminal);
    assert_eq!(before.total_matches, after.total_matches);
    assert_ne!(before.rail_offsets, after.rail_offsets);
    assert_eq!(&*after.rail_offsets, &[4, 6]);
}

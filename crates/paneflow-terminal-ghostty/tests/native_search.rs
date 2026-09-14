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
        let snapshot = terminal.search_step().unwrap().unwrap();
        if snapshot.complete {
            return snapshot;
        }
    }
    unreachable!("native search did not complete")
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
    assert_eq!(before.matches.len(), 1);
    assert!(before.matches[0].start.line < -19_000);
    assert_eq!(before.selected, Some(0));
    terminal.feed(b"old-marker\r\n").unwrap();
    let after = complete(&mut terminal);
    assert_eq!(after.matches.len(), 2);
    assert_eq!(after.selected, Some(1));
    assert_eq!(
        after.matches[1].start.line,
        before.matches[0].start.line - 1
    );
    terminal.search_select(true).unwrap();
    assert_eq!(complete(&mut terminal).selected, Some(0));
}

#[test]
fn finds_soft_wrapped_text_before_and_after_resize() {
    let mut terminal = terminal(10, 4, 100);
    terminal.feed(b"prefix-long-needle-suffix").unwrap();
    terminal.set_search_query("long-needle").unwrap();
    let before = complete(&mut terminal);
    assert_eq!(before.matches.len(), 1);
    assert!(before.matches[0].start.line < before.matches[0].end.line);
    terminal
        .resize(WindowSize::new(30, 4, 8, 16).unwrap())
        .unwrap();
    let after = complete(&mut terminal);
    assert_eq!(after.matches.len(), 1);
    assert_eq!(after.matches[0].start.line, after.matches[0].end.line);
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
    assert!(snapshot.matches.is_empty());
    assert_eq!(snapshot.selected, None);
}

#[test]
fn alternate_screen_restores_primary_results() {
    let mut terminal = terminal(20, 4, 100);
    terminal.feed(b"marker primary\r\n").unwrap();
    terminal.set_search_query("marker").unwrap();
    let primary = complete(&mut terminal);
    assert_eq!(primary.matches.len(), 1);
    terminal
        .feed(b"\x1b[?1049hmarker alternate\r\nmarker")
        .unwrap();
    assert_eq!(complete(&mut terminal).matches.len(), 2);
    terminal.feed(b"\x1b[?1049l").unwrap();
    assert_eq!(complete(&mut terminal), primary);
}

#[test]
fn clearing_and_replacing_queries_releases_search_state() {
    let mut terminal = terminal(20, 4, 100);
    terminal.feed("Hello Éclair".as_bytes()).unwrap();
    terminal.set_search_query("HELLO").unwrap();
    assert_eq!(complete(&mut terminal).matches.len(), 1);
    terminal.set_search_query("éclair").unwrap();
    assert!(complete(&mut terminal).matches.is_empty());
    terminal.set_search_query("Éclair").unwrap();
    assert_eq!(complete(&mut terminal).matches.len(), 1);
    terminal.clear_search();
    assert!(terminal.search_step().unwrap().is_none());
    terminal.set_search_query("Hello").unwrap();
    assert_eq!(complete(&mut terminal).matches.len(), 1);
    terminal.set_search_query("").unwrap();
    assert!(terminal.search_step().unwrap().is_none());
    terminal.set_search_query("Hello").unwrap();
    complete(&mut terminal);
}

#[test]
fn one_shot_search_uses_native_wrapping_and_preserves_order() {
    let mut terminal = terminal(10, 4, 100);
    terminal.feed(b"prefix-long-needle\r\nlong-needle").unwrap();
    let matches = terminal.search("long-needle", false).unwrap().matches;
    assert_eq!(matches.len(), 2);
    assert!(matches[0].start.line < matches[1].start.line);
    assert!(terminal.search_step().unwrap().is_none());
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
    assert_eq!(complete(&mut terminal).matches.len(), 3);
    for scroll in [
        paneflow_terminal_ghostty::Scroll::Bottom,
        paneflow_terminal_ghostty::Scroll::Delta(150),
        paneflow_terminal_ghostty::Scroll::Top,
    ] {
        terminal.scroll(scroll);
        assert_eq!(complete(&mut terminal).matches.len(), 3);
    }
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
        assert_eq!(complete(&mut terminal).matches.len(), count);
        assert_eq!(terminal.snapshot().unwrap().history_size, 0);
    }
}

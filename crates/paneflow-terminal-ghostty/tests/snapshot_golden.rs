#![cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    )
))]

use paneflow_terminal_ghostty::{Content, DisplayTerminal, Scroll, TerminalAppearance, WindowSize};

#[allow(dead_code, reason = "the corpus module is shared with the app suite")]
#[path = "../../../src-app/src/terminal/bench_corpus.rs"]
mod bench_corpus;

const PUBLISHED_CORPUS_DIGEST: u64 = 4_802_858_128_671_786_933;
const PRE_ROW_REUSE_CONTENT_DIGEST: u64 = 11_315_952_540_898_940_023;

fn fnv1a(digest: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *digest ^= u64::from(*byte);
        *digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn fold(digest: &mut u64, content: &Content, with_dirty_rows: bool) {
    if with_dirty_rows {
        fnv1a(digest, format!("{:?}", content.dirty_rows).as_bytes());
    }
    let published = format!(
        "{:?}|{:?}|{:?}|{}x{}|{}|{}",
        content.cells,
        content.cursor,
        content.selection,
        content.cols,
        content.rows,
        content.display_offset,
        content.history_size,
    )
    .replace(", underline_color: Default", "");
    fnv1a(digest, published.as_bytes());
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn corpus_digest(with_dirty_rows: bool, configure: impl FnOnce(&mut DisplayTerminal)) -> u64 {
    let mut terminal = DisplayTerminal::new(
        WindowSize::new(80, 24, 8, 16).unwrap(),
        10_000,
        TerminalAppearance::default(),
    )
    .unwrap();
    configure(&mut terminal);
    let mut digest = 0xcbf2_9ce4_8422_2325;
    for (index, stream) in bench_corpus::deterministic_streams().iter().enumerate() {
        terminal.feed(stream).unwrap();
        terminal.feed(b"\x1b[0m\r\n").unwrap();
        if index % 7 == 0 {
            fold(&mut digest, &terminal.snapshot().unwrap(), with_dirty_rows);
        }
    }
    for scroll in [
        Scroll::Delta(5),
        Scroll::Delta(1),
        Scroll::Delta(-3),
        Scroll::Bottom,
    ] {
        terminal.scroll(scroll).unwrap();
        fold(&mut digest, &terminal.snapshot().unwrap(), with_dirty_rows);
    }
    digest
}

#[test]
fn the_published_corpus_is_unchanged_without_overscan() {
    assert_eq!(corpus_digest(true, |_| {}), PUBLISHED_CORPUS_DIGEST);
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn an_explicit_zero_overscan_publishes_the_same_corpus() {
    assert_eq!(
        corpus_digest(true, |terminal| terminal.set_overscan(0, 0).unwrap()),
        PUBLISHED_CORPUS_DIGEST
    );
}

fn assert_clean_rows_match_their_source(previous: &Content, next: &Content, frame: usize) {
    let continuous = previous.revision.wrapping_add(1) == next.revision
        && previous.cols == next.cols
        && previous.rows == next.rows
        && next.dirty_rows.len() == next.rows;
    if !continuous {
        return;
    }
    let cols = next.cols;
    for (row, dirty) in next.dirty_rows.iter().enumerate() {
        let Some(source) = row
            .checked_add_signed(next.row_shift)
            .filter(|source| *source < next.rows)
        else {
            continue;
        };
        if *dirty {
            continue;
        }
        let fresh: Vec<_> = next.cells[row * cols..(row + 1) * cols]
            .iter()
            .map(|cell| {
                (
                    cell.character,
                    cell.zerowidth.clone(),
                    cell.foreground,
                    cell.background,
                    cell.flags,
                    cell.wide,
                    cell.hyperlink,
                )
            })
            .collect();
        let kept: Vec<_> = previous.cells[source * cols..(source + 1) * cols]
            .iter()
            .map(|cell| {
                (
                    cell.character,
                    cell.zerowidth.clone(),
                    cell.foreground,
                    cell.background,
                    cell.flags,
                    cell.wide,
                    cell.hyperlink,
                )
            })
            .collect();
        assert_eq!(
            fresh, kept,
            "frame {frame}: row {row} changed but was reported clean (source row {source}, shift {})",
            next.row_shift
        );
    }
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn every_row_reported_clean_keeps_the_content_of_its_source_row() {
    let mut terminal = DisplayTerminal::new(
        WindowSize::new(80, 24, 8, 16).unwrap(),
        10_000,
        TerminalAppearance::default(),
    )
    .unwrap();
    let mut previous = terminal.snapshot().unwrap();
    let mut frame = 0usize;
    let mut clean_rows = 0usize;
    for stream in bench_corpus::deterministic_streams() {
        terminal.feed(&stream).unwrap();
        terminal.feed(b"\x1b[0m\r\n").unwrap();
        let next = terminal.snapshot().unwrap();
        assert_clean_rows_match_their_source(&previous, &next, frame);
        clean_rows += next.dirty_rows.iter().filter(|dirty| !**dirty).count();
        previous = next;
        frame += 1;
    }
    for scroll in [
        Scroll::Delta(5),
        Scroll::Delta(1),
        Scroll::Delta(-3),
        Scroll::Bottom,
    ] {
        terminal.scroll(scroll).unwrap();
        let next = terminal.snapshot().unwrap();
        assert_clean_rows_match_their_source(&previous, &next, frame);
        clean_rows += next.dirty_rows.iter().filter(|dirty| !**dirty).count();
        previous = next;
        frame += 1;
    }
    assert!(clean_rows > 0, "the corpus must exercise clean rows");
}

#[test]
fn reusing_shifted_rows_publishes_the_same_content_as_a_full_copy() {
    assert_eq!(corpus_digest(false, |_| {}), PRE_ROW_REUSE_CONTENT_DIGEST);
}

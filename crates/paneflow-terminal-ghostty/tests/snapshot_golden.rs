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

const PRE_OVERSCAN_DIGEST: u64 = 15_384_815_173_529_891_403;

fn fnv1a(digest: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *digest ^= u64::from(*byte);
        *digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn fold(digest: &mut u64, content: &Content) {
    let published = format!(
        "{:?}|{:?}|{:?}|{:?}|{}x{}|{}|{}",
        content.cells,
        content.dirty_rows,
        content.cursor,
        content.selection,
        content.cols,
        content.rows,
        content.display_offset,
        content.history_size,
    );
    fnv1a(digest, published.as_bytes());
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn corpus_digest(configure: impl FnOnce(&mut DisplayTerminal)) -> u64 {
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
            fold(&mut digest, &terminal.snapshot().unwrap());
        }
    }
    for scroll in [
        Scroll::Delta(5),
        Scroll::Delta(1),
        Scroll::Delta(-3),
        Scroll::Bottom,
    ] {
        terminal.scroll(scroll);
        fold(&mut digest, &terminal.snapshot().unwrap());
    }
    digest
}

#[test]
fn the_published_corpus_is_unchanged_without_overscan() {
    assert_eq!(corpus_digest(|_| {}), PRE_OVERSCAN_DIGEST);
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
#[test]
fn an_explicit_zero_overscan_publishes_the_same_corpus() {
    assert_eq!(
        corpus_digest(|terminal| terminal.set_overscan(0, 0).unwrap()),
        PRE_OVERSCAN_DIGEST
    );
}

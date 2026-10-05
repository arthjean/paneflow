use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group, main};
use paneflow_terminal_ghostty::{Content, DisplayTerminal, TerminalAppearance, WindowSize};

#[allow(dead_code, reason = "the corpus module is shared with the app suite")]
#[path = "../../../src-app/src/terminal/bench_corpus.rs"]
mod bench_corpus;

const CORPUS_BYTES: usize = 1024 * 1024;
const SCROLLBACK_LINES: usize = 10_000;

fn mebibyte_corpus() -> (DisplayTerminal, Vec<u8>) {
    let size = WindowSize::new(220, 60, 8, 16).expect("valid benchmark grid");
    let terminal = DisplayTerminal::new(size, SCROLLBACK_LINES, TerminalAppearance::default())
        .expect("libghostty must initialize");
    let streams = bench_corpus::deterministic_streams();
    let mut corpus = Vec::with_capacity(CORPUS_BYTES + 4096);
    while corpus.len() < CORPUS_BYTES {
        for stream in &streams {
            corpus.extend_from_slice(stream);
            corpus.extend_from_slice(b"\x1b[0m\r\n");
        }
    }
    corpus.truncate(CORPUS_BYTES);
    (terminal, corpus)
}

#[library_benchmark]
#[bench::mebibyte_220x60(setup = mebibyte_corpus)]
fn parse_and_convert((mut terminal, corpus): (DisplayTerminal, Vec<u8>)) -> Content {
    terminal
        .feed(black_box(&corpus))
        .expect("corpus must parse");
    black_box(terminal.snapshot().expect("snapshot must succeed"))
}

library_benchmark_group!(name = terminal; benchmarks = parse_and_convert);

main!(library_benchmark_groups = terminal);

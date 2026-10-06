use std::hint::black_box;
use std::path::{MAIN_SEPARATOR_STR, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime};

use super::index::{self, Builder, GLOBAL_CAP, Index};
use super::listing::{self, Cancel, Request};
use crate::bench_harness::{Metric, live_bytes, measure, refuse_debug_profile, results_table};

const WORDS: [&str; 48] = [
    "src", "app", "core", "terminal", "path", "picker", "main", "lib", "mod", "test", "tests",
    "utils", "config", "render", "view", "model", "index", "listing", "fuzzy", "history", "assets",
    "icons", "docs", "build", "scripts", "native", "bench", "crates", "home", "layout", "theme",
    "input", "session", "agent", "worker", "server", "client", "shared", "types", "error",
    "events", "state", "store", "cache", "search", "widgets", "ui", "web",
];
const EXTENSIONS: [&str; 8] = ["rs", "ts", "md", "json", "toml", "tsx", "txt", "png"];
const QUERIES: [&str; 5] = ["a", "main", "picker", "srcmodrs", "qzx"];

fn corpus(count: usize) -> Vec<(String, bool)> {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut entries = Vec::with_capacity(count);
    let mut chain: Vec<String> = Vec::new();
    while entries.len() < count {
        let roll = next();
        let word = WORDS[(roll >> 8) as usize % WORDS.len()];
        let other = WORDS[(roll >> 16) as usize % WORDS.len()];
        match roll % 10 {
            0 if !chain.is_empty() => {
                chain.pop();
            }
            1 | 2 if chain.len() < 8 => {
                let name = format!("{word}_{other}{}", (roll >> 24) % 7);
                chain.push(name);
                entries.push((chain.join(MAIN_SEPARATOR_STR), true));
            }
            _ => {
                let extension = EXTENSIONS[(roll >> 32) as usize % EXTENSIONS.len()];
                let name = format!("{word}{other}{}.{extension}", (roll >> 40) % 13);
                let mut path = chain.join(MAIN_SEPARATOR_STR);
                if !path.is_empty() {
                    path.push_str(MAIN_SEPARATOR_STR);
                }
                path.push_str(&name);
                entries.push((path, false));
            }
        }
    }
    entries
}

fn build(entries: &[(String, bool)]) -> Index {
    let mut builder = Builder::new(PathBuf::from("bench-root"), GLOBAL_CAP.max(entries.len()));
    for (relative, is_dir) in entries {
        let depth = relative.split(MAIN_SEPARATOR_STR).count();
        builder.push(
            depth,
            relative.rsplit(MAIN_SEPARATOR_STR).next(),
            *is_dir,
            false,
        );
    }
    builder.finish(SystemTime::now())
}

fn leak(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn search_scenarios(metrics: &mut Vec<Metric>, count: usize, label: &str) {
    let entries = corpus(count);
    let before = live_bytes();
    let index = Arc::new(build(&entries));
    let retained = live_bytes() - before;
    drop(entries);
    println!(
        "index_{label}: {} entries, {retained} live bytes",
        index.len()
    );
    let mut stored = Vec::new();
    let started = Instant::now();
    index.encode(&mut stored).expect("encodable");
    let encoded = started.elapsed();
    let started = Instant::now();
    let decoded = Index::decode(&mut stored.as_slice()).expect("decodable");
    println!(
        "store_{label}: {} bytes, encode {encoded:?}, decode {:?}, {} entries back",
        stored.len(),
        started.elapsed(),
        decoded.len()
    );
    for query in QUERIES {
        let request = Request {
            query: query.to_owned(),
            base: Some(PathBuf::from("bench-root")),
            home: None,
            wsl: None,
            local: Some(index.clone()),
            globals: Vec::new(),
            visited: Vec::new(),
            recents: Arc::from([]),
            pending: false,
        };
        metrics.push(measure(
            leak(format!("search_{label}_{query}")),
            "one keystroke over the synthetic corpus",
            2,
            10,
            || {
                black_box(listing::compute(&request, &Cancel::never()));
            },
        ));
    }
}

#[test]
#[ignore = "path picker benchmark: cargo test --release -p paneflow-app --bin paneflow path_picker_benchmark -- --ignored --nocapture"]
fn path_picker_benchmark() {
    refuse_debug_profile();
    let mut metrics = Vec::new();
    search_scenarios(&mut metrics, 100_000, "100k");
    search_scenarios(&mut metrics, 1_000_000, "1m");
    let sealed: Vec<PathBuf> = dirs::home_dir()
        .map(|home| index::platform_data_dirs(&home))
        .unwrap_or_default();
    let walks = std::env::var("PANEFLOW_PICKER_BENCH_WALK").unwrap_or_default();
    for root in walks.split(';').filter(|root| !root.is_empty()) {
        let started = Instant::now();
        let before = live_bytes();
        let walked = index::walk(
            &PathBuf::from(root),
            GLOBAL_CAP,
            &sealed,
            &AtomicBool::new(false),
        )
        .expect("not cancelled");
        println!(
            "walk {root}: {} entries in {:?}, {} live bytes, complete {}",
            walked.len(),
            started.elapsed(),
            live_bytes() - before,
            walked.complete()
        );
    }
    println!(
        "{}",
        results_table(&metrics, "Path picker walk, no baseline.")
    );
}

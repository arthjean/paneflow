use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::app::files_git::GitStatuses;
use crate::app::files_tree::{FileNode, FilesTreeState, compare_nodes};
use crate::bench_harness::{Metric, measure, process_cpu_time, publish, refuse_debug_profile};
use crate::terminal::project_files::ProjectFiles;

use super::projection::FilesProjection;
use super::search::rank_hits;

pub(crate) const TREE_CORPUS_SEED: u64 = 0x7472_6565_0000_0001;
const CHANGES: usize = 1_000;

struct Corpus {
    tree: FilesTreeState,
    expanded: HashSet<PathBuf>,
    files: ProjectFiles,
}

fn file_name(index: usize) -> String {
    let extension = ["rs", "ts", "md", "json", "toml"][index % 5];
    format!("file_{index:06}.{extension}")
}

fn node(path: PathBuf) -> FileNode {
    FileNode {
        path,
        is_dir: false,
        is_ignored: false,
        is_hidden: false,
        size: 1_024,
    }
}

fn flat_corpus(entries: usize) -> Corpus {
    let root = PathBuf::from("bench-root");
    let mut listing: Vec<FileNode> = (0..entries)
        .map(|index| node(root.join(file_name(index))))
        .collect();
    listing.sort_by(compare_nodes);
    let mut tree = FilesTreeState::root_shell(root.clone());
    tree.children.insert(root.clone(), listing);
    let expanded = HashSet::from([root.clone()]);
    let files = ProjectFiles {
        root,
        paths: (0..entries).map(file_name).collect(),
        complete: true,
    };
    Corpus {
        tree,
        expanded,
        files,
    }
}

fn apply_changes(base: &FilesTreeState) -> FilesTreeState {
    let mut tree = base.clone();
    let root = tree.root.clone();
    if let Some(listing) = tree.children.get_mut(&root) {
        let stride = (listing.len() / (CHANGES / 2)).max(1);
        let mut removed = 0;
        let mut index = listing.len();
        while removed < CHANGES / 2 && index >= stride {
            index -= stride;
            listing.remove(index);
            removed += 1;
        }
        listing
            .extend((0..CHANGES / 2).map(|added| node(root.join(format!("added_{added:04}.rs")))));
        listing.sort_by(compare_nodes);
    }
    tree
}

fn scenarios(metrics: &mut Vec<Metric>, corpus: &Corpus, names: [&'static str; 3], iters: usize) {
    let git = GitStatuses::default();
    metrics.push(measure(
        names[0],
        "the rows of a flat expanded folder rebuilt from its listing, as the projection does after a folder is expanded",
        1,
        iters,
        || {
            std::hint::black_box(FilesProjection::build(
                &corpus.tree,
                &corpus.expanded,
                None,
                &git,
            ));
        },
    ));
    metrics.push(measure(
        names[1],
        "1,000 entries of that folder changed on disk (500 removed, 500 added): the snapshot clone the worker sends, the re-sorted listing and the rebuilt rows",
        1,
        iters,
        || {
            let tree = apply_changes(&corpus.tree);
            std::hint::black_box(FilesProjection::build(
                &tree,
                &corpus.expanded,
                None,
                &git,
            ));
        },
    ));
    metrics.push(measure(
        names[2],
        "a project search over every file of the folder, ranked and capped at 200 results, as the tree search runs it after the typing pause",
        1,
        iters,
        || {
            std::hint::black_box(rank_hits(&corpus.files, "file_0424"));
        },
    ));
}

#[test]
#[ignore = "benchmark: run through scripts/bench-tree.sh"]
fn tree_pipeline_benchmark() {
    refuse_debug_profile();
    let small = flat_corpus(50_000);
    let large = flat_corpus(300_000);
    let mut metrics = Vec::new();
    let started = Instant::now();
    let cpu_before = process_cpu_time();
    scenarios(
        &mut metrics,
        &small,
        [
            "rows_after_expand_50k",
            "apply_1000_changes_50k",
            "search_50k",
        ],
        20,
    );
    scenarios(
        &mut metrics,
        &large,
        [
            "rows_after_expand_300k",
            "apply_1000_changes_300k",
            "search_300k",
        ],
        8,
    );
    let cpu_share = (process_cpu_time() - cpu_before).as_secs_f64()
        / started.elapsed().as_secs_f64().max(f64::EPSILON);
    println!("PANEFLOW_BENCH_NOTE cpu share over the timed scenarios: {cpu_share:.2}");
    publish(
        "paneflow-tree-bench",
        "tree",
        TREE_CORPUS_SEED,
        &metrics,
        Some(cpu_share),
    );
}

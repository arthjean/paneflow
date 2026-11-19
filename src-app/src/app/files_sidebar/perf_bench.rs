use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::app::files_git::GitStatuses;
use crate::app::files_tree::{FileNode, compare_nodes};
use crate::bench_harness::{
    Metric, SegmentTimer, measure, measure_segments, process_cpu_time, publish,
    refuse_debug_profile,
};
use crate::terminal::project_files::ProjectFiles;

use super::model::{DirChange, ListingDiff, TreeModel};
use super::projection::FileRow;
use super::search::rank_hits;

pub(crate) const TREE_CORPUS_SEED: u64 = 0x7472_6565_0000_0001;
const CHANGES: usize = 1_000;
const WINDOW: usize = 64;

struct Corpus {
    root: PathBuf,
    folder: PathBuf,
    listing: Arc<[FileNode]>,
    files: ProjectFiles,
}

fn file_name(index: usize) -> String {
    let extension = ["rs", "ts", "md", "json", "toml"][index % 5];
    format!("file_{index:06}.{extension}")
}

fn node(path: PathBuf, is_dir: bool) -> FileNode {
    FileNode {
        path,
        is_dir,
        is_ignored: false,
        is_hidden: false,
        size: if is_dir { 0 } else { 1_024 },
    }
}

fn flat_corpus(entries: usize) -> Corpus {
    let root = PathBuf::from("bench-root");
    let folder = root.join("flat");
    let mut listing: Vec<FileNode> = (0..entries)
        .map(|index| node(folder.join(file_name(index)), false))
        .collect();
    listing.sort_by(compare_nodes);
    let files = ProjectFiles {
        root: root.clone(),
        paths: (0..entries).map(file_name).collect(),
        complete: true,
    };
    Corpus {
        root,
        folder,
        listing: listing.into(),
        files,
    }
}

fn listing_change(
    dir: &std::path::Path,
    nodes: Arc<[FileNode]>,
    diff: Option<ListingDiff>,
) -> DirChange {
    DirChange::Listing {
        dir: dir.to_path_buf(),
        nodes,
        diff,
        unreadable: false,
    }
}

fn collapsed_model(corpus: &Corpus) -> TreeModel {
    let mut model = TreeModel::new(corpus.root.clone(), Vec::new());
    model.apply(vec![listing_change(
        &corpus.root,
        vec![node(corpus.folder.clone(), true)].into(),
        None,
    )]);
    model
}

fn window(model: &TreeModel, start: usize, git: &GitStatuses) -> Vec<FileRow> {
    model
        .rows(start..start + WINDOW)
        .into_iter()
        .map(|row| FileRow::from_ref(row, git))
        .collect()
}

fn changed_listing(base: &[FileNode], folder: &std::path::Path) -> Vec<FileNode> {
    let mut listing = base.to_vec();
    let stride = (listing.len() / (CHANGES / 2)).max(1);
    let mut removed = 0;
    let mut index = listing.len();
    while removed < CHANGES / 2 && index >= stride {
        index -= stride;
        listing.remove(index);
        removed += 1;
    }
    listing.extend(
        (0..CHANGES / 2).map(|added| node(folder.join(format!("added_{added:04}.rs")), false)),
    );
    listing
}

fn scenarios(metrics: &mut Vec<Metric>, corpus: &Corpus, names: [&'static str; 3], iters: usize) {
    let git = GitStatuses::default();
    let middle = corpus.listing.len() / 2;
    let mut model = collapsed_model(corpus);
    metrics.push(measure_segments(
        names[0],
        "a collapsed flat folder expanded: its listing installed with one visible count per entry, the folder opened, and the 64 rows of a scrolled window built; no row outside the window is built",
        1,
        iters,
        |timer: &mut SegmentTimer| {
            let listing = corpus.listing.clone();
            let rows = timer.time(|| {
                model.apply(vec![listing_change(&corpus.folder, listing, None)]);
                model.set_expanded(&corpus.folder, true);
                window(&model, middle, &git)
            });
            assert_eq!(rows.len(), WINDOW);
            std::hint::black_box(rows);
            model.set_expanded(&corpus.folder, false);
            model.apply(vec![DirChange::Dropped(corpus.folder.clone())]);
        },
    ));
    let mut model = collapsed_model(corpus);
    model.apply(vec![listing_change(
        &corpus.folder,
        corpus.listing.clone(),
        None,
    )]);
    model.set_expanded(&corpus.folder, true);
    metrics.push(measure_segments(
        names[1],
        "1,000 entries of that expanded folder changed on disk (500 removed, 500 added): the re-read listing sorted, diffed against the previous one by the worker, applied to the visible counts of the changed entries only, and the 64 rows of a scrolled window rebuilt",
        1,
        iters,
        |timer: &mut SegmentTimer| {
            let mut fresh = changed_listing(&corpus.listing, &corpus.folder);
            let rows = timer.time(|| {
                fresh.sort_by(compare_nodes);
                let diff = ListingDiff::between(&corpus.listing, &fresh);
                let nodes: Arc<[FileNode]> = fresh.into();
                model.apply(vec![listing_change(&corpus.folder, nodes.clone(), Some(diff))]);
                (window(&model, middle, &git), nodes)
            });
            let (rows, nodes) = rows;
            assert_eq!(rows.len(), WINDOW);
            assert_eq!(model.row_count(), corpus.listing.len() + 1);
            std::hint::black_box(rows);
            let back = ListingDiff::between(&nodes, &corpus.listing);
            model.apply(vec![listing_change(&corpus.folder, corpus.listing.clone(), Some(back))]);
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

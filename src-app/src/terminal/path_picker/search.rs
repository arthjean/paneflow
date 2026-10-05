use std::cmp::Ordering;
use std::ops::Range;
use std::sync::atomic::{self, AtomicUsize};

use super::fuzzy::{self, Pattern};
use super::index::{Index, Located};

const CHUNK: u32 = 16_384;
const MAX_THREADS: usize = 8;
const PARALLEL_FROM: usize = 32_768;
const FOLDER_SLOTS: usize = 64;
pub(super) const LOCAL_BONUS: i32 = 32;
pub(super) const RECENT_BONUS: i32 = 24;

pub(super) struct Source<'a> {
    pub(super) index: &'a Index,
    pub(super) range: Range<u32>,
    pub(super) skip: Vec<Range<u32>>,
    pub(super) from: Located,
    pub(super) min_depth: usize,
    pub(super) local: Option<Range<u32>>,
    pub(super) show_hidden: bool,
    pub(super) recents: Vec<RecentTarget>,
}

pub(super) struct RecentTarget {
    pub(super) name: String,
    pub(super) relative: String,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Hit {
    pub(super) score: i32,
    pub(super) len: u32,
    pub(super) source: u16,
    pub(super) node: u32,
    pub(super) recent: bool,
}

impl Hit {
    pub(super) fn rank(&self, other: &Self) -> Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.len.cmp(&other.len))
            .then_with(|| self.source.cmp(&other.source))
            .then_with(|| self.node.cmp(&other.node))
    }
}

pub(super) struct Scan {
    pub(super) hits: Vec<Hit>,
}

struct Worker {
    hits: Vec<Hit>,
    floor: i32,
    positions: Vec<usize>,
    text: String,
    chain: Vec<u32>,
    folders_of: usize,
    folders: [(u32, usize); FOLDER_SLOTS],
}

pub(super) fn scan(
    sources: &[Source<'_>],
    pattern: &Pattern,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Scan> {
    let work = chunks(sources);
    let total: usize = work.iter().map(|(_, range)| range.len()).sum();
    let threads = if total < PARALLEL_FROM {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(MAX_THREADS)
            .min(work.len())
    };
    let next = AtomicUsize::new(0);
    let run = || -> Option<Worker> {
        let mut worker = Worker {
            hits: Vec::new(),
            floor: i32::MIN,
            positions: Vec::new(),
            text: String::new(),
            chain: Vec::new(),
            folders_of: usize::MAX,
            folders: [(u32::MAX, 0); FOLDER_SLOTS],
        };
        loop {
            let item = next.fetch_add(1, atomic::Ordering::Relaxed);
            let Some((source, range)) = work.get(item) else {
                return Some(worker);
            };
            if cancelled() {
                return None;
            }
            scan_range(&mut worker, sources, *source, range.clone(), pattern, limit);
        }
    };
    let workers: Vec<Option<Worker>> = if threads <= 1 {
        vec![run()]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads).map(|_| scope.spawn(run)).collect();
            handles
                .into_iter()
                .map(|handle| handle.join().ok().flatten())
                .collect()
        })
    };
    let mut hits = Vec::new();
    for worker in workers {
        hits.extend(worker?.hits);
    }
    keep_best(&mut hits, limit);
    hits.sort_by(Hit::rank);
    Some(Scan { hits })
}

fn chunks(sources: &[Source<'_>]) -> Vec<(usize, Range<u32>)> {
    let mut work = Vec::new();
    for (nth, source) in sources.iter().enumerate() {
        let mut start = source.range.start;
        let mut skips: Vec<&Range<u32>> = source.skip.iter().collect();
        skips.sort_by_key(|skip| skip.start);
        let mut pieces = Vec::new();
        for skip in skips {
            if skip.start > start {
                pieces.push(start..skip.start.min(source.range.end));
            }
            start = start.max(skip.end);
        }
        if start < source.range.end {
            pieces.push(start..source.range.end);
        }
        for piece in pieces {
            let mut at = piece.start;
            while at < piece.end {
                let end = piece.end.min(at.saturating_add(CHUNK));
                work.push((nth, at..end));
                at = end;
            }
        }
    }
    work
}

fn scan_range(
    worker: &mut Worker,
    sources: &[Source<'_>],
    nth: usize,
    range: Range<u32>,
    pattern: &Pattern,
    limit: usize,
) {
    let source = &sources[nth];
    let index = source.index;
    for node in range {
        if pattern.excluded_by(index.mask(node))
            || index.depth(node) < source.min_depth
            || (!source.show_hidden && index.is_hidden(node))
        {
            continue;
        }
        let name = index.name(node);
        let mut written = false;
        let (score, len) = match fuzzy::score_name(name, pattern, &mut worker.positions) {
            Some(score) => (score, index.path_len(node, source.from)),
            None => {
                let above = folder_progress(worker, nth, source, node, pattern);
                if !fuzzy::is_complete(pattern, fuzzy::advance(name, pattern, above)) {
                    continue;
                }
                index.write_path(node, source.from, &mut worker.text, &mut worker.chain);
                written = true;
                match fuzzy::score_path(&worker.text, pattern, &mut worker.positions) {
                    Some(score) => (score, worker.text.len()),
                    None => continue,
                }
            }
        };
        let mut recent = false;
        for target in source.recents.iter().filter(|target| target.name == name) {
            if !written {
                index.write_path(node, source.from, &mut worker.text, &mut worker.chain);
                written = true;
            }
            if worker.text == target.relative {
                recent = true;
            }
        }
        let local = source
            .local
            .as_ref()
            .is_some_and(|local| local.contains(&node));
        let score =
            score + if local { LOCAL_BONUS } else { 0 } + if recent { RECENT_BONUS } else { 0 };
        if score < worker.floor {
            continue;
        }
        worker.hits.push(Hit {
            score,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            source: nth as u16,
            node,
            recent,
        });
        if worker.hits.len() >= limit.saturating_mul(4).max(1024) {
            keep_best(&mut worker.hits, limit);
            if worker.hits.len() == limit {
                worker.floor = worker
                    .hits
                    .iter()
                    .map(|hit| hit.score)
                    .min()
                    .unwrap_or(i32::MIN);
            }
        }
    }
}

fn folder_progress(
    worker: &mut Worker,
    nth: usize,
    source: &Source<'_>,
    node: u32,
    pattern: &Pattern,
) -> usize {
    let stop = match source.from {
        Located::Root => None,
        Located::Node(ancestor) => Some(ancestor),
    };
    let Some(parent) = source
        .index
        .parent(node)
        .filter(|parent| Some(*parent) != stop)
    else {
        return 0;
    };
    if worker.folders_of != nth {
        worker.folders_of = nth;
        worker.folders = [(u32::MAX, 0); FOLDER_SLOTS];
    }
    let slot = parent as usize % FOLDER_SLOTS;
    let (known, matched) = worker.folders[slot];
    if known == parent {
        return matched;
    }
    source
        .index
        .write_path(parent, source.from, &mut worker.text, &mut worker.chain);
    let matched = fuzzy::advance(&worker.text, pattern, 0);
    worker.folders[slot] = (parent, matched);
    matched
}

fn keep_best(hits: &mut Vec<Hit>, limit: usize) {
    if hits.len() > limit {
        hits.select_nth_unstable_by(limit, Hit::rank);
        hits.truncate(limit);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::*;
    use crate::terminal::path_picker::index::{Builder, GLOBAL_CAP};

    fn wide(files: usize) -> Index {
        let mut builder = Builder::new(PathBuf::from("root"), GLOBAL_CAP);
        builder.push(1, Some("group"), true, false);
        for nth in 0..files {
            builder.push(2, Some(&format!("file{nth}.txt")), false, false);
        }
        builder.push(1, Some("needle.rs"), false, false);
        builder.finish(SystemTime::now())
    }

    fn source(index: &Index, skip: Vec<Range<u32>>) -> Source<'_> {
        Source {
            index,
            range: index.subtree(Located::Root),
            skip,
            from: Located::Root,
            min_depth: 1,
            local: None,
            show_hidden: true,
            recents: Vec::new(),
        }
    }

    #[test]
    fn a_parallel_scan_keeps_the_best_hits_in_rank_order() {
        let index = wide(3 * PARALLEL_FROM);
        let found = scan(
            &[source(&index, Vec::new())],
            &Pattern::new("needle"),
            5,
            &|| false,
        )
        .expect("not cancelled");
        assert_eq!(found.hits.len(), 1);
        assert_eq!(index.name(found.hits[0].node), "needle.rs");
        let found = scan(
            &[source(&index, Vec::new())],
            &Pattern::new("file1"),
            10,
            &|| false,
        )
        .expect("not cancelled");
        assert_eq!(found.hits.len(), 10);
        assert!(
            found
                .hits
                .windows(2)
                .all(|pair| pair[0].rank(&pair[1]).is_le())
        );
        assert_eq!(index.name(found.hits[0].node), "file1.txt");
    }

    #[test]
    fn a_skipped_range_is_never_scanned() {
        let index = wide(10);
        let group = index.subtree(Located::Node(0));
        let found = scan(
            &[source(&index, vec![group])],
            &Pattern::new("txt"),
            50,
            &|| false,
        )
        .expect("not cancelled");
        assert!(found.hits.is_empty());
    }

    #[test]
    fn a_cancelled_scan_yields_nothing() {
        let index = wide(10);
        assert!(
            scan(
                &[source(&index, Vec::new())],
                &Pattern::new("f"),
                5,
                &|| true
            )
            .is_none()
        );
    }
}

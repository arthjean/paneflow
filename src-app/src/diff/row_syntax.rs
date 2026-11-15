use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use gpui::Hsla;

use super::git::FileDiff;
use super::highlighter::{grammar_for_ext, highlight_cap, highlight_lines};
use super::rows::{FileRowCache, file_ext};
use super::syntax::DiffSyntax;
use crate::theme::SyntaxPalette;

pub(crate) type LineRuns = Vec<(Range<usize>, Hsla)>;

pub(crate) type SideRuns = Arc<Vec<LineRuns>>;

const STORE_TEXT_BYTES: usize = 16 * 1024 * 1024;

const MAX_SYNTAX_WORKERS: usize = 8;

const SECOND_DIGEST_SALT: u8 = 0xA5;

#[derive(Clone, PartialEq, Eq, Hash)]
struct SideKey {
    ext: String,
    len: usize,
    digest: (u64, u64),
}

impl SideKey {
    fn of(ext: &str, text: &str) -> Self {
        let mut first = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut first);
        let mut second = std::collections::hash_map::DefaultHasher::new();
        SECOND_DIGEST_SALT.hash(&mut second);
        text.hash(&mut second);
        Self {
            ext: ext.to_string(),
            len: text.len(),
            digest: (first.finish(), second.finish()),
        }
    }
}

struct StoredSide {
    runs: SideRuns,
    used: u64,
}

#[derive(Default)]
struct SyntaxStore {
    palette: Option<SyntaxPalette>,
    clock: u64,
    bytes: usize,
    sides: HashMap<SideKey, StoredSide>,
}

impl SyntaxStore {
    fn get(&mut self, palette: SyntaxPalette, key: &SideKey) -> Option<SideRuns> {
        if self.palette != Some(palette) {
            return None;
        }
        self.clock += 1;
        let clock = self.clock;
        self.sides.get_mut(key).map(|side| {
            side.used = clock;
            side.runs.clone()
        })
    }

    fn insert(&mut self, palette: SyntaxPalette, key: SideKey, runs: SideRuns, capacity: usize) {
        if self.palette != Some(palette) {
            self.sides.clear();
            self.bytes = 0;
            self.palette = Some(palette);
        }
        if key.len > capacity {
            return;
        }
        self.clock += 1;
        let added = key.len;
        let stored = StoredSide {
            runs,
            used: self.clock,
        };
        if self.sides.insert(key, stored).is_none() {
            self.bytes += added;
        }
        while self.bytes > capacity {
            let Some(oldest) = self
                .sides
                .iter()
                .min_by_key(|(_, side)| side.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.sides.remove(&oldest);
            self.bytes = self.bytes.saturating_sub(oldest.len);
        }
    }
}

fn store() -> MutexGuard<'static, SyntaxStore> {
    static STORE: OnceLock<Mutex<SyntaxStore>> = OnceLock::new();
    STORE
        .get_or_init(|| Mutex::new(SyntaxStore::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn side_key(file: &FileDiff, ext: &str, text: &str) -> Option<SideKey> {
    let colored = !file.is_binary
        && !text.is_empty()
        && text.len() <= highlight_cap(ext)
        && grammar_for_ext(ext).is_some();
    colored.then(|| SideKey::of(ext, text))
}

fn sides(file: &FileDiff) -> [&str; 2] {
    [file.base_text.as_str(), file.new_text.as_str()]
}

pub(crate) struct CachedRowSyntax {
    pub(crate) caches: Vec<FileRowCache>,
    pub(crate) missing_bytes: Vec<usize>,
}

pub(crate) fn cached_file_row_caches(files: &[FileDiff], syntax: &DiffSyntax) -> CachedRowSyntax {
    let palette = syntax.palette();
    let mut caches = Vec::with_capacity(files.len());
    let mut missing_bytes = Vec::with_capacity(files.len());
    for file in files {
        let ext = file_ext(&file.path);
        let mut missing = 0usize;
        let [old, new] = sides(file).map(|text| {
            let Some(key) = side_key(file, &ext, text) else {
                return SideRuns::default();
            };
            store().get(palette, &key).unwrap_or_else(|| {
                missing += text.len();
                SideRuns::default()
            })
        });
        caches.push(FileRowCache::from_sides(old, new));
        missing_bytes.push(missing);
    }
    CachedRowSyntax {
        caches,
        missing_bytes,
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntaxWork {
    pub(crate) highlighted: usize,
    pub(crate) reused: usize,
}

struct SideJob<'a> {
    file: usize,
    new_side: bool,
    ext: String,
    text: &'a str,
    key: SideKey,
}

fn run_in_parallel<T: Send>(count: usize, job: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(MAX_SYNTAX_WORKERS)
        .min(count);
    if workers <= 1 {
        return (0..count).map(job).collect();
    }
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut finished = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= count {
                            break finished;
                        }
                        finished.push((index, job(index)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
            .collect()
    });
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, value)| value).collect()
}

pub(crate) fn highlighted_file_row_caches(
    files: &[FileDiff],
    shown_files: usize,
    syntax: &DiffSyntax,
) -> (Vec<FileRowCache>, SyntaxWork) {
    let palette = syntax.palette();
    let mut jobs = Vec::new();
    for (index, file) in files.iter().enumerate().take(shown_files) {
        let ext = file_ext(&file.path);
        for (new_side, text) in sides(file).into_iter().enumerate() {
            if let Some(key) = side_key(file, &ext, text) {
                jobs.push(SideJob {
                    file: index,
                    new_side: new_side == 1,
                    ext: ext.clone(),
                    text,
                    key,
                });
            }
        }
    }
    jobs.sort_by_key(|job| std::cmp::Reverse(job.text.len()));

    let results = run_in_parallel(jobs.len(), |index| {
        let job = &jobs[index];
        if let Some(runs) = store().get(palette, &job.key) {
            return (runs, true);
        }
        let runs: SideRuns = Arc::new(highlight_lines(job.text, &job.ext, syntax));
        store().insert(palette, job.key.clone(), runs.clone(), STORE_TEXT_BYTES);
        (runs, false)
    });

    let mut work = SyntaxWork::default();
    let mut per_file: Vec<[SideRuns; 2]> = files.iter().map(|_| Default::default()).collect();
    for (job, (runs, reused)) in jobs.iter().zip(results) {
        if reused {
            work.reused += 1;
        } else {
            work.highlighted += 1;
        }
        per_file[job.file][usize::from(job.new_side)] = runs;
    }
    let caches = per_file
        .into_iter()
        .map(|[old, new]| FileRowCache::from_sides(old, new))
        .collect();
    (caches, work)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::FileChange;

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&crate::theme::active_theme())
    }

    fn rust_file(path: &str, marker: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            change: FileChange::Modified,
            old_path: None,
            base_text: format!("fn {marker}_before() -> u32 {{ 1 }}\n"),
            new_text: format!("fn {marker}_after() -> u32 {{ 2 }}\n"),
            hunks: Vec::new(),
            is_binary: false,
        }
    }

    fn colored(cache: &FileRowCache) -> bool {
        let (old, new) = cache.sides();
        old.iter().chain(new.iter()).any(|line| !line.is_empty())
    }

    #[test]
    fn a_second_build_of_the_same_files_reuses_every_side() {
        let files = vec![
            rust_file("src/a.rs", "reuse_every_side_a"),
            rust_file("src/b.rs", "reuse_every_side_b"),
        ];
        let syntax = syntax();
        let (first, _) = highlighted_file_row_caches(&files, files.len(), &syntax);
        let (second, work) = highlighted_file_row_caches(&files, files.len(), &syntax);
        assert_eq!(
            work,
            SyntaxWork {
                highlighted: 0,
                reused: 4
            }
        );
        assert!(first.iter().all(colored));
        assert!(second.iter().all(colored));
    }

    #[test]
    fn only_the_changed_side_is_highlighted_again() {
        let mut files = vec![rust_file("src/a.rs", "changed_side_only")];
        let syntax = syntax();
        highlighted_file_row_caches(&files, 1, &syntax);
        files[0].new_text = "fn changed_side_only_rewritten() {}\n".to_string();
        let (_, work) = highlighted_file_row_caches(&files, 1, &syntax);
        assert_eq!(
            work,
            SyntaxWork {
                highlighted: 1,
                reused: 1
            }
        );
    }

    #[test]
    fn files_past_the_shown_count_are_never_highlighted() {
        let files = vec![
            rust_file("src/a.rs", "past_shown_count_a"),
            rust_file("src/b.rs", "past_shown_count_b"),
        ];
        let syntax = syntax();
        let (caches, work) = highlighted_file_row_caches(&files, 1, &syntax);
        assert_eq!(work.highlighted + work.reused, 2);
        assert!(colored(&caches[0]));
        assert!(!colored(&caches[1]));
        let cached = cached_file_row_caches(&files, &syntax);
        assert_eq!(cached.missing_bytes[0], 0);
        assert_eq!(
            cached.missing_bytes[1],
            files[1].base_text.len() + files[1].new_text.len()
        );
    }

    #[test]
    fn a_lookup_never_highlights_and_reports_what_is_missing() {
        let files = vec![rust_file("src/a.rs", "lookup_never_highlights")];
        let syntax = syntax();
        let cached = cached_file_row_caches(&files, &syntax);
        assert!(!colored(&cached.caches[0]));
        assert_eq!(
            cached.missing_bytes,
            vec![files[0].base_text.len() + files[0].new_text.len()]
        );
        highlighted_file_row_caches(&files, 1, &syntax);
        let cached = cached_file_row_caches(&files, &syntax);
        assert!(colored(&cached.caches[0]));
        assert_eq!(cached.missing_bytes, vec![0]);
    }

    #[test]
    fn binary_unknown_and_empty_sides_need_no_highlight() {
        let mut binary = rust_file("src/a.rs", "needs_no_highlight_binary");
        binary.is_binary = true;
        let mut unknown = rust_file("notes.unknownext", "needs_no_highlight_unknown");
        unknown.base_text.clear();
        let files = vec![binary, unknown];
        let syntax = syntax();
        let cached = cached_file_row_caches(&files, &syntax);
        assert_eq!(cached.missing_bytes, vec![0, 0]);
        let (_, work) = highlighted_file_row_caches(&files, files.len(), &syntax);
        assert_eq!(work, SyntaxWork::default());
    }

    #[test]
    fn the_store_drops_its_least_recently_used_side_past_its_capacity() {
        let mut store = SyntaxStore::default();
        let palette = syntax().palette();
        let runs = SideRuns::default();
        let first = SideKey::of("rs", "aaaa");
        let second = SideKey::of("rs", "bbbb");
        let third = SideKey::of("rs", "cccc");
        store.insert(palette, first.clone(), runs.clone(), 8);
        store.insert(palette, second.clone(), runs.clone(), 8);
        assert!(store.get(palette, &first).is_some());
        store.insert(palette, third.clone(), runs.clone(), 8);
        assert!(store.get(palette, &first).is_some());
        assert!(store.get(palette, &second).is_none());
        assert!(store.get(palette, &third).is_some());
        assert_eq!(store.bytes, 8);
    }

    #[test]
    fn a_side_larger_than_the_store_is_not_kept() {
        let mut store = SyntaxStore::default();
        let palette = syntax().palette();
        let key = SideKey::of("rs", "0123456789");
        store.insert(palette, key.clone(), SideRuns::default(), 4);
        assert!(store.get(palette, &key).is_none());
        assert_eq!(store.bytes, 0);
    }

    #[test]
    fn parallel_jobs_come_back_in_their_own_order() {
        let squares = run_in_parallel(64, |index| index * index);
        assert_eq!(
            squares,
            (0..64).map(|index| index * index).collect::<Vec<_>>()
        );
    }
}

use std::{
    collections::HashMap,
    ffi::OsString,
    num::NonZeroUsize,
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use bstr::{BStr, BString, ByteSlice};
use rayon::prelude::*;

const MAX_POOL_THREADS: usize = 8;

/// One entry of a directory listing, as [`std::fs::read_dir()`] returned it.
#[derive(Debug, Clone)]
pub struct ListedEntry {
    /// The name of the entry, precomposed if the listing was read with `precompose_unicode`.
    pub file_name: OsString,
    /// The type of the entry, without following symlinks.
    pub file_type: Option<std::fs::FileType>,
    /// The metadata the directory enumeration returned for the entry, without following symlinks.
    pub metadata: Option<std::fs::Metadata>,
}

/// The listing of one directory, in the order the file system returned it.
#[derive(Debug, Default)]
pub struct ListedDirectory {
    entries: Vec<ListedEntry>,
    by_name: HashMap<BString, usize>,
}

impl ListedDirectory {
    /// All entries of the directory, in the order the file system returned them.
    pub fn entries(&self) -> &[ListedEntry] {
        &self.entries
    }

    /// The entry named `file_name`, if the directory held it when it was read.
    pub fn get(&self, file_name: &BStr) -> Option<&ListedEntry> {
        self.by_name.get(file_name).map(|index| &self.entries[*index])
    }
}

/// Complete listings of a set of directories, read once and in parallel, to be reused by a
/// [walk](crate::walk_with_listing()) and by other passes over the same worktree.
///
/// A directory is absent if it could not be read completely, so a consumer falls back to reading it itself.
#[derive(Debug, Default)]
pub struct Listing {
    directories: HashMap<BString, ListedDirectory>,
}

impl Listing {
    /// Read the listing of each of `directories`, given relative to `worktree_root` with `/` separators
    /// and the empty path for the root itself.
    ///
    /// The directories are read on one thread pool that every listing of the process shares, so concurrent
    /// listings queue for the same threads instead of oversubscribing the machine, and a lone listing
    /// gets all of them. The pool holds half the available cores, at most eight, and is started on first use.
    pub fn read(
        worktree_root: &Path,
        directories: &[BString],
        precompose_unicode: bool,
        should_interrupt: Option<&AtomicBool>,
    ) -> Listing {
        let read = |rela_dir: &BString| {
            if should_interrupt.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return None;
            }
            read_directory(worktree_root, rela_dir.as_bstr(), precompose_unicode)
                .map(|directory| (rela_dir.clone(), directory))
        };
        let directories = match shared_pool() {
            Some(pool) if directories.len() > 1 => {
                pool.install(|| directories.par_iter().filter_map(read).collect::<Vec<_>>())
            }
            _ => directories.iter().filter_map(read).collect(),
        };
        Listing {
            directories: directories.into_iter().collect(),
        }
    }

    /// The number of threads in the pool that [`read()`](Self::read()) shares across the process,
    /// or 1 if it reads on the calling thread.
    pub fn pool_threads() -> usize {
        shared_pool().map_or(1, rayon::ThreadPool::current_num_threads)
    }

    /// The listing of `rela_dir`, relative to the worktree root with `/` separators.
    pub fn directory(&self, rela_dir: &BStr) -> Option<&ListedDirectory> {
        self.directories.get(rela_dir)
    }

    /// The number of directories that were read completely.
    pub fn len(&self) -> usize {
        self.directories.len()
    }

    /// Whether no directory was read completely.
    pub fn is_empty(&self) -> bool {
        self.directories.is_empty()
    }
}

fn shared_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .div_ceil(2)
            .min(MAX_POOL_THREADS);
        (threads > 1)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .thread_name(|index| format!("gix_dir::listing::{index}"))
                    .build()
                    .ok()
            })
            .flatten()
    })
    .as_ref()
}

fn read_directory(worktree_root: &Path, rela_dir: &BStr, precompose_unicode: bool) -> Option<ListedDirectory> {
    let path = if rela_dir.is_empty() {
        worktree_root.to_owned()
    } else {
        worktree_root.join(gix_path::from_bstr(rela_dir).ok()?)
    };
    let mut directory = ListedDirectory::default();
    for entry in std::fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        let metadata = entry.metadata().ok();
        let file_type = entry.file_type().ok();
        let file_name = gix_fs::read_dir::DirEntry::new(entry, precompose_unicode).file_name().into_owned();
        if let Ok(name) = gix_path::try_os_str_into_bstr(file_name.as_os_str().into()) {
            directory.by_name.insert(name.into_owned(), directory.entries.len());
        }
        directory.entries.push(ListedEntry {
            file_name,
            file_type,
            metadata,
        });
    }
    Some(directory)
}

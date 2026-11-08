use std::{
    collections::HashMap,
    ffi::OsString,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use bstr::{BStr, BString};
use rayon::prelude::*;

use crate::entry::Kind;

const MAX_POOL_THREADS: usize = 8;

/// A directory to list, and the names in it whose metadata the caller needs.
#[derive(Debug, Clone)]
pub struct ListRequest<'a> {
    /// The directory relative to the worktree root with `/` separators, empty for the root itself.
    pub directory: &'a BStr,
    /// The names whose metadata is wanted, sorted by their bytes.
    ///
    /// The Windows enumeration returns the metadata of every entry at no extra cost, so this only
    /// limits the per-entry `fstatat` calls on unix.
    pub stat_names: Vec<&'a BStr>,
}

/// One entry of a directory listing.
pub struct ListedEntry {
    /// The name of the entry, precomposed if the listing was read with `precompose_unicode`.
    pub file_name: OsString,
    #[cfg(not(unix))]
    name: Option<BString>,
    kind: Option<Kind>,
    #[cfg(unix)]
    stat: Option<rustix::fs::Stat>,
    #[cfg(not(unix))]
    metadata: Option<std::fs::Metadata>,
}

impl ListedEntry {
    /// The type of the entry, without following symlinks.
    pub fn kind(&self) -> Option<Kind> {
        self.kind
    }

    /// The metadata of the entry, without following symlinks, if the listing read it.
    ///
    /// On unix only the [requested names](ListRequest::stat_names) and entries of unknown type have it.
    pub fn metadata(&self) -> Option<gix_index::fs::Metadata> {
        #[cfg(unix)]
        {
            self.stat.map(Into::into)
        }
        #[cfg(windows)]
        {
            self.metadata.clone().map(Into::into)
        }
        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }

    fn has_metadata(&self) -> bool {
        #[cfg(unix)]
        {
            self.stat.is_some()
        }
        #[cfg(not(unix))]
        {
            self.metadata.is_some()
        }
    }
}

impl std::fmt::Debug for ListedEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListedEntry")
            .field("file_name", &self.file_name)
            .field("kind", &self.kind)
            .field("has_metadata", &self.has_metadata())
            .finish()
    }
}

/// The listing of one directory, in the order the file system returned it.
#[derive(Debug, Default)]
pub struct ListedDirectory {
    entries: Vec<ListedEntry>,
    by_name: Vec<usize>,
}

impl ListedDirectory {
    /// All entries of the directory, in the order the file system returned them.
    pub fn entries(&self) -> &[ListedEntry] {
        &self.entries
    }

    /// The entry named `file_name`, if the directory held it when it was read.
    pub fn get(&self, file_name: &BStr) -> Option<&ListedEntry> {
        self.by_name
            .binary_search_by(|index| name_of(&self.entries[*index]).cmp(&Some(file_name)))
            .ok()
            .map(|position| &self.entries[self.by_name[position]])
    }

    fn finish(mut self) -> Self {
        let mut by_name: Vec<usize> = (0..self.entries.len())
            .filter(|index| name_of(&self.entries[*index]).is_some())
            .collect();
        by_name.sort_unstable_by(|a, b| name_of(&self.entries[*a]).cmp(&name_of(&self.entries[*b])));
        self.by_name = by_name;
        self
    }
}

fn name_of(entry: &ListedEntry) -> Option<&BStr> {
    #[cfg(unix)]
    {
        use bstr::ByteSlice;
        use std::os::unix::ffi::OsStrExt;
        Some(entry.file_name.as_bytes().as_bstr())
    }
    #[cfg(not(unix))]
    {
        entry.name.as_ref().map(|name| name.as_ref())
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
    /// Read the listing of each of `requests`, whose directories are relative to `worktree_root`.
    ///
    /// The directories are read on one thread pool that every listing of the process shares, so concurrent
    /// listings queue for the same threads instead of oversubscribing the machine, and a lone listing
    /// gets all of them. The pool holds half the available cores, at most eight, and is started on first use.
    ///
    /// On Windows, the enumeration returns the metadata of every entry. On unix, a directory is opened
    /// without following a symlink in its last component, and each requested name is read with `fstatat`
    /// relative to the open directory, so the kernel never resolves the full path again.
    pub fn read(
        worktree_root: &Path,
        requests: &[ListRequest<'_>],
        precompose_unicode: bool,
        should_interrupt: Option<&AtomicBool>,
    ) -> Listing {
        let read = |request: &ListRequest<'_>| {
            if should_interrupt.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return None;
            }
            read_directory(worktree_root, request, precompose_unicode)
                .map(|directory| (request.directory.to_owned(), directory))
        };
        let directories = match shared_pool() {
            Some(pool) if requests.len() > 1 => {
                pool.install(|| requests.par_iter().filter_map(read).collect::<Vec<_>>())
            }
            _ => requests.iter().filter_map(read).collect(),
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

fn directory_path(worktree_root: &Path, rela_dir: &BStr) -> Option<PathBuf> {
    Some(if rela_dir.is_empty() {
        worktree_root.to_owned()
    } else {
        worktree_root.join(gix_path::from_bstr(rela_dir).ok()?)
    })
}

#[cfg(not(unix))]
fn read_directory(
    worktree_root: &Path,
    request: &ListRequest<'_>,
    precompose_unicode: bool,
) -> Option<ListedDirectory> {
    let path = directory_path(worktree_root, request.directory)?;
    let mut directory = ListedDirectory::default();
    for entry in std::fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        let metadata = entry.metadata().ok();
        let kind = entry.file_type().ok().map(Kind::from);
        let file_name = gix_fs::read_dir::DirEntry::new(entry, precompose_unicode)
            .file_name()
            .into_owned();
        let name = gix_path::try_os_str_into_bstr(file_name.as_os_str().into())
            .ok()
            .map(std::borrow::Cow::into_owned);
        directory.entries.push(ListedEntry {
            file_name,
            name,
            kind,
            metadata,
        });
    }
    Some(directory.finish())
}

#[cfg(unix)]
fn read_directory(
    worktree_root: &Path,
    request: &ListRequest<'_>,
    precompose_unicode: bool,
) -> Option<ListedDirectory> {
    use std::os::unix::ffi::OsStringExt;

    use bstr::ByteSlice;
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};

    let path = directory_path(worktree_root, request.directory)?;
    let fd = rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let mut dir = Dir::new(fd).ok()?;
    let mut read = Vec::new();
    while let Some(entry) = dir.read() {
        let entry = entry.ok()?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        read.push((name.to_owned(), entry.file_type()));
    }
    let dirfd = dir.fd().ok()?;
    let mut directory = ListedDirectory::default();
    for (raw_name, file_type) in read {
        let precomposed = precompose_unicode
            .then(|| {
                gix_utils::str::precompose_bstr(raw_name.as_bytes().as_bstr().into())
                    .into_owned()
            })
            .filter(|name| name.as_slice() != raw_name.as_bytes());
        let name = precomposed.as_ref().map_or(raw_name.as_bytes(), |name| name.as_slice());
        let requested = request.stat_names.binary_search(&name.as_bstr()).is_ok();
        let stat = (requested || file_type == FileType::Unknown)
            .then(|| rustix::fs::statat(dirfd, raw_name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW).ok())
            .flatten();
        let kind = match file_type {
            FileType::Unknown => stat.map(|stat| kind_of(FileType::from_raw_mode(stat.st_mode as _))),
            known => Some(kind_of(known)),
        };
        let file_name = OsString::from_vec(match precomposed {
            Some(name) => name.into(),
            None => raw_name.into_bytes(),
        });
        directory.entries.push(ListedEntry { file_name, kind, stat });
    }
    Some(directory.finish())
}

#[cfg(unix)]
fn kind_of(file_type: rustix::fs::FileType) -> Kind {
    use rustix::fs::FileType;
    match file_type {
        FileType::RegularFile => Kind::File,
        FileType::Directory => Kind::Directory,
        FileType::Symlink => Kind::Symlink,
        _ => Kind::Untrackable,
    }
}

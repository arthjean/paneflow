use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use gix::bstr::{BStr, BString, ByteSlice, ByteVec};
use gix::index::entry::Mode;
use gix::worktree::stack::state::ignore::Source;

use crate::{open, Error};

const DOT_GIT: &str = ".git";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub worktree: PathBuf,
    pub git_dirs: BTreeSet<PathBuf>,
    pub directories: BTreeSet<PathBuf>,
    pub ignored: BTreeSet<PathBuf>,
}

impl Coverage {
    pub fn scan(worktree: &Path, thread_limit: usize) -> Result<Self, Error> {
        let repo = open::discover(worktree, thread_limit)?;
        let workdir = repo.workdir().ok_or_else(|| Error::Bare {
            path: worktree.to_path_buf(),
        })?;
        let workdir = canonical(workdir, "resolve the worktree directory")?;
        let mut coverage = Coverage {
            worktree: workdir.clone(),
            ..Coverage::default()
        };
        scan_repository(&repo, &workdir, &workdir, thread_limit, &mut coverage)?;
        Ok(coverage)
    }

    pub fn is_ignored(&self, relative: &Path) -> bool {
        relative
            .ancestors()
            .skip(1)
            .any(|ancestor| self.ignored.contains(ancestor))
    }
}

fn canonical(path: &Path, action: &'static str) -> Result<PathBuf, Error> {
    std::fs::canonicalize(path).map_err(|error| Error::git(action, error))
}

fn scan_repository(
    repo: &gix::Repository,
    workdir: &Path,
    top: &Path,
    thread_limit: usize,
    coverage: &mut Coverage,
) -> Result<(), Error> {
    coverage
        .git_dirs
        .insert(canonical(repo.git_dir(), "resolve the git directory")?);
    coverage.git_dirs.insert(canonical(
        repo.common_dir(),
        "resolve the common git directory",
    )?);
    let index = repo
        .index_or_empty()
        .map_err(|error| Error::git("read the index", error))?;
    let mut tracked = BTreeSet::<BString>::new();
    let mut submodules = BTreeSet::<BString>::new();
    for entry in index.entries() {
        let path = entry.path(&index);
        if entry.mode == Mode::COMMIT {
            submodules.insert(path.to_owned());
        }
        let mut end = 0;
        while let Some(offset) = path[end..].find_byte(b'/') {
            end += offset;
            tracked.insert(path[..end].to_owned());
            end += 1;
        }
    }
    let mut excludes = repo
        .excludes(&index, None, Source::WorktreeThenIdMappingIfNotSkipped)
        .map_err(|error| Error::git("prepare the ignore rules", error))?;
    let relative_root = relative_to(top, workdir)?;
    let mut pending: Vec<(BString, bool)> = vec![(BString::default(), false)];
    while let Some((directory, inside_ignored)) = pending.pop() {
        let absolute = join(workdir, directory.as_bstr())?;
        coverage.directories.insert(absolute.clone());
        let entries = match std::fs::read_dir(&absolute) {
            Ok(entries) => entries,
            Err(error) if directory.is_empty() => {
                return Err(Error::git("list the worktree root", error));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                coverage.directories.remove(&absolute);
                continue;
            }
            Err(_) => continue,
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            if directory.is_empty() && name == DOT_GIT {
                continue;
            }
            let name = gix::path::os_str_into_bstr(&name)
                .map_err(|error| Error::git("encode a worktree directory name", error))?;
            let mut child = directory.clone();
            if !child.is_empty() {
                child.push(b'/');
            }
            child.push_str(name);
            if submodules.contains(&child) {
                let path = join(workdir, child.as_bstr())?;
                scan_submodule(&path, top, thread_limit, coverage)?;
                continue;
            }
            let mut excluded = || -> Result<bool, Error> {
                Ok(excludes
                    .at_entry(child.as_bstr(), Some(Mode::DIR))
                    .map_err(|error| Error::git("match the ignore rules", error))?
                    .is_excluded())
            };
            if tracked.contains(&child) {
                let ignored = inside_ignored || excluded()?;
                pending.push((child, ignored));
            } else if inside_ignored || excluded()? {
                coverage
                    .ignored
                    .insert(join(&relative_root, child.as_bstr())?);
            } else if !entry.path().join(DOT_GIT).exists() {
                pending.push((child, false));
            }
        }
    }
    Ok(())
}

fn scan_submodule(
    path: &Path,
    top: &Path,
    thread_limit: usize,
    coverage: &mut Coverage,
) -> Result<(), Error> {
    coverage.directories.insert(path.to_path_buf());
    if !path.join(DOT_GIT).exists() {
        return Ok(());
    }
    let Ok(repo) = open::submodule(path, thread_limit) else {
        return Ok(());
    };
    let Some(workdir) = repo.workdir() else {
        return Ok(());
    };
    let workdir = canonical(workdir, "resolve the submodule worktree")?;
    scan_repository(&repo, &workdir, top, thread_limit, coverage)
}

fn relative_to(top: &Path, workdir: &Path) -> Result<PathBuf, Error> {
    workdir
        .strip_prefix(top)
        .map(Path::to_path_buf)
        .map_err(|_| {
            Error::git(
                "locate a submodule in the worktree",
                format!("{} is outside {}", workdir.display(), top.display()),
            )
        })
}

fn join(base: &Path, relative: &BStr) -> Result<PathBuf, Error> {
    let mut path = base.to_path_buf();
    for component in relative
        .split_str("/")
        .filter(|component| !component.is_empty())
    {
        let component = gix::path::from_byte_slice(component)
            .map_err(|error| Error::git("decode a worktree path", error))?;
        path.push(component);
    }
    Ok(path)
}

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gix::bstr::ByteSlice;

use crate::snapshot::Worktree;

const IGNORE_FILE: &[u8] = b".gitignore";
const ATTRIBUTES_FILE: &[u8] = b".gitattributes";
const GITMODULES: &str = ".gitmodules";
const TRAILER_BYTES: i64 = 32;
const CHECKSUM_BYTES: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Input {
    Config,
    Excludes,
    Attributes,
    IgnoreFile,
    AttributesFile,
    Gitmodules,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stamp {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
        unix: Option<(u64, i64, i64)>,
    },
}

pub(crate) fn stamp(path: &Path) -> Stamp {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Stamp::Missing;
    };
    Stamp::Present {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        unix: unix_identity(&metadata),
    }
}

#[cfg(unix)]
fn unix_identity(metadata: &std::fs::Metadata) -> Option<(u64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.ino(), metadata.ctime(), metadata.ctime_nsec()))
}

#[cfg(not(unix))]
fn unix_identity(_: &std::fs::Metadata) -> Option<(u64, i64, i64)> {
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexStamp {
    file: Stamp,
    trailer: Vec<u8>,
}

impl IndexStamp {
    pub(crate) fn read(path: &Path) -> Self {
        let file = stamp(path);
        let trailer = trailer(path).unwrap_or_default();
        Self { file, trailer }
    }

    pub(crate) fn vouches_for_content(&self) -> bool {
        self.trailer.len() >= CHECKSUM_BYTES
            && self.trailer[self.trailer.len() - CHECKSUM_BYTES..]
                .iter()
                .any(|byte| *byte != 0)
    }
}

fn trailer(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let back = TRAILER_BYTES.min(i64::try_from(len).unwrap_or(i64::MAX));
    file.seek(SeekFrom::End(-back))?;
    let mut trailer = Vec::new();
    file.read_to_end(&mut trailer)?;
    Ok(trailer)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Inputs {
    config: Vec<u8>,
    files: Vec<(Input, PathBuf, Stamp)>,
}

impl Inputs {
    pub(crate) fn read(
        repo: &gix::Repository,
        config: Vec<u8>,
        index: &gix::index::State,
        worktree: &Worktree,
    ) -> Self {
        let files = sources(repo, index, worktree)
            .into_iter()
            .map(|(input, path)| {
                let stamp = stamp(&path);
                (input, path, stamp)
            })
            .collect();
        Self { config, files }
    }

    pub(crate) fn changed(&self, config: &[u8]) -> Option<Input> {
        if self.config != config {
            return Some(Input::Config);
        }
        self.files
            .iter()
            .find(|(_, path, recorded)| stamp(path) != *recorded)
            .map(|(input, _, _)| *input)
    }
}

pub(crate) fn input_of_name(name: &[u8]) -> Option<Input> {
    if name == IGNORE_FILE {
        Some(Input::IgnoreFile)
    } else if name == ATTRIBUTES_FILE {
        Some(Input::AttributesFile)
    } else if name == GITMODULES.as_bytes() {
        Some(Input::Gitmodules)
    } else {
        None
    }
}

fn sources(
    repo: &gix::Repository,
    index: &gix::index::State,
    worktree: &Worktree,
) -> BTreeSet<(Input, PathBuf)> {
    let mut sources = BTreeSet::new();
    let git_dirs = [repo.git_dir(), repo.common_dir()];
    for git_dir in git_dirs {
        sources.insert((Input::Excludes, git_dir.join("info").join("exclude")));
        sources.insert((Input::Attributes, git_dir.join("info").join("attributes")));
    }
    let config = repo.config_snapshot();
    for (input, key, default) in [
        (Input::Excludes, "core.excludesFile", "ignore"),
        (Input::Attributes, "core.attributesFile", "attributes"),
    ] {
        if let Ok(Some(path)) = config.trusted_path(key) {
            sources.insert((input, path));
        }
        if let Some(path) = gix::path::env::xdg_config(default, &mut environment) {
            sources.insert((input, path));
        }
    }
    let Some(workdir) = repo.workdir() else {
        return sources;
    };
    sources.insert((Input::Gitmodules, workdir.join(GITMODULES)));
    let tracked = index
        .entries()
        .iter()
        .map(|entry| entry.path(index).as_bytes());
    let untracked = worktree.untracked.iter().map(Vec::as_slice);
    for path in tracked.chain(untracked) {
        let name = path.rsplit_str("/").next().unwrap_or(path);
        let input = if name == IGNORE_FILE {
            Input::IgnoreFile
        } else if name == ATTRIBUTES_FILE {
            Input::AttributesFile
        } else {
            continue;
        };
        if let Ok(relative) = gix::path::from_byte_slice(path) {
            sources.insert((input, workdir.join(relative)));
        }
    }
    sources
}

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| {
        (name == "HOME")
            .then(gix::path::env::home_dir)
            .flatten()
            .map(PathBuf::into_os_string)
    })
}

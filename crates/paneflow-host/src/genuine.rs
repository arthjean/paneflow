use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use paneflow_peer::PeerIdentity;

pub const WINDOW_EXECUTABLE_FILE_NAME: &str = if cfg!(windows) {
    "paneflow.exe"
} else {
    "paneflow"
};

const COMPARE_CHUNK_BYTES: usize = 256 * 1024;

const MAX_REMEMBERED_IMAGES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Spawner,
    SamePath,
    SameImage,
    Foreign,
    Unresolved,
}

impl Provenance {
    pub const fn is_genuine(self) -> bool {
        matches!(self, Self::Spawner | Self::SamePath | Self::SameImage)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Spawner => "the process that started this server",
            Self::SamePath => "a Paneflow executable of this installation",
            Self::SameImage => {
                "a byte-identical copy of a Paneflow executable of this installation"
            }
            Self::Foreign => "an executable that is not Paneflow's",
            Self::Unresolved => "an executable that could not be resolved",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ImageStamp {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl ImageStamp {
    fn of(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Debug)]
pub struct GenuineCallers {
    binaries: Mutex<Vec<PathBuf>>,
    spawner: Option<(u32, u64)>,
    images: Mutex<HashMap<ImageStamp, bool>>,
    refusals_logged: Mutex<HashSet<String>>,
}

impl GenuineCallers {
    pub fn of_this_installation() -> Self {
        let own = std::env::current_exe().ok();
        let beside = own
            .as_deref()
            .and_then(Path::parent)
            .map(|dir| dir.join(WINDOW_EXECUTABLE_FILE_NAME));
        Self::of(
            own.into_iter().chain(beside),
            paneflow_peer::parent_process(),
        )
    }

    pub(crate) fn of(
        binaries: impl IntoIterator<Item = PathBuf>,
        spawner: Option<(u32, u64)>,
    ) -> Self {
        let mut canonical: Vec<PathBuf> = Vec::new();
        for path in binaries {
            if let Ok(path) = std::fs::canonicalize(&path)
                && path.is_file()
                && !canonical.contains(&path)
            {
                canonical.push(path);
            }
        }
        Self {
            binaries: Mutex::new(canonical),
            spawner,
            images: Mutex::new(HashMap::new()),
            refusals_logged: Mutex::new(HashSet::new()),
        }
    }

    pub fn binaries(&self) -> Vec<PathBuf> {
        self.lock_binaries().clone()
    }

    pub fn vouch_for_executable(&self, executable: &Path) -> io::Result<()> {
        let canonical = std::fs::canonicalize(executable)?;
        if !canonical.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a file", canonical.display()),
            ));
        }
        let mut binaries = self.lock_binaries();
        if !binaries.contains(&canonical) {
            binaries.push(canonical);
        }
        drop(binaries);
        self.images
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        Ok(())
    }

    fn lock_binaries(&self) -> std::sync::MutexGuard<'_, Vec<PathBuf>> {
        self.binaries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn provenance(&self, caller: Option<&PeerIdentity>) -> Provenance {
        let Some(caller) = caller else {
            return Provenance::Unresolved;
        };
        if self.spawner == Some((caller.pid(), caller.started_at())) {
            return Provenance::Spawner;
        }
        self.provenance_of_executable(caller.executable())
    }

    pub(crate) fn provenance_of_executable(&self, executable: Option<&Path>) -> Provenance {
        let Some(executable) = executable else {
            return Provenance::Unresolved;
        };
        let Ok(resolved) = std::fs::canonicalize(executable) else {
            return Provenance::Unresolved;
        };
        let binaries = self.binaries();
        if binaries.contains(&resolved) {
            return Provenance::SamePath;
        }
        let Ok(stamp) = ImageStamp::of(&resolved) else {
            return Provenance::Unresolved;
        };
        if let Some(&same) = self
            .images
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&stamp)
        {
            return if same {
                Provenance::SameImage
            } else {
                Provenance::Foreign
            };
        }
        let same = binaries
            .iter()
            .any(|binary| same_contents(binary, &resolved, stamp.len).unwrap_or(false));
        let mut images = self.images.lock().unwrap_or_else(PoisonError::into_inner);
        if images.len() >= MAX_REMEMBERED_IMAGES {
            images.clear();
        }
        images.insert(stamp, same);
        if same {
            Provenance::SameImage
        } else {
            Provenance::Foreign
        }
    }

    pub fn vouches_for(&self, server: &str, claim: &str, caller: Option<&PeerIdentity>) -> bool {
        let provenance = self.provenance(caller);
        if !provenance.is_genuine() {
            self.log_refusal_once(server, claim, caller, provenance);
        }
        provenance.is_genuine()
    }

    fn log_refusal_once(
        &self,
        server: &str,
        claim: &str,
        caller: Option<&PeerIdentity>,
        provenance: Provenance,
    ) {
        let executable = caller.and_then(PeerIdentity::executable).map_or_else(
            || "an unknown executable".to_string(),
            |path| path.display().to_string(),
        );
        let key = format!("{claim} {executable}");
        let mut logged = self
            .refusals_logged
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if logged.len() >= MAX_REMEMBERED_IMAGES || !logged.insert(key) {
            return;
        }
        log::warn!(
            "paneflow {server}: a caller running {executable} claimed the {claim} principal; it is {}, so it is treated as external",
            provenance.label()
        );
    }
}

fn same_contents(reference: &Path, candidate: &Path, candidate_len: u64) -> io::Result<bool> {
    let mut reference = File::open(reference)?;
    if reference.metadata()?.len() != candidate_len {
        return Ok(false);
    }
    let mut candidate = File::open(candidate)?;
    let mut left = vec![0u8; COMPARE_CHUNK_BYTES];
    let mut right = vec![0u8; COMPARE_CHUNK_BYTES];
    loop {
        let read = fill(&mut reference, &mut left)?;
        if fill(&mut candidate, &mut right[..read.max(1)])? != read {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
        if left[..read] != right[..read] {
            return Ok(false);
        }
    }
}

fn fill(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn the_executable_at_an_expected_path_is_genuine_and_another_one_is_foreign() {
        let dir = tempfile::tempdir().unwrap();
        let window = write(dir.path(), "paneflow", b"the window image");
        let other = write(dir.path(), "impostor", b"another image of other length");
        let callers = GenuineCallers::of([window.clone()], None);
        assert_eq!(
            callers.provenance_of_executable(Some(&window)),
            Provenance::SamePath
        );
        assert_eq!(
            callers.provenance_of_executable(Some(&other)),
            Provenance::Foreign
        );
    }

    #[test]
    fn an_executable_that_cannot_be_resolved_is_never_genuine() {
        let dir = tempfile::tempdir().unwrap();
        let window = write(dir.path(), "paneflow", b"the window image");
        let callers = GenuineCallers::of([window], None);
        assert_eq!(
            callers.provenance_of_executable(None),
            Provenance::Unresolved
        );
        assert_eq!(
            callers.provenance_of_executable(Some(&dir.path().join("gone"))),
            Provenance::Unresolved
        );
        assert!(!callers.provenance(None).is_genuine());
    }

    #[test]
    fn a_byte_identical_copy_elsewhere_is_genuine_and_a_same_length_edit_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let image: Vec<u8> = (0..COMPARE_CHUNK_BYTES * 2 + 17)
            .map(|index| (index % 251) as u8)
            .collect();
        let window = write(dir.path(), "paneflow", &image);
        let elsewhere = tempfile::tempdir().unwrap();
        let copy = write(elsewhere.path(), "paneflow", &image);
        let mut edited = image.clone();
        let last = edited.len() - 1;
        edited[last] ^= 0xff;
        let tampered = write(elsewhere.path(), "tampered", &edited);
        let callers = GenuineCallers::of([window], None);
        assert_eq!(
            callers.provenance_of_executable(Some(&copy)),
            Provenance::SameImage
        );
        assert_eq!(
            callers.provenance_of_executable(Some(&copy)),
            Provenance::SameImage
        );
        assert_eq!(
            callers.provenance_of_executable(Some(&tampered)),
            Provenance::Foreign
        );
    }

    #[test]
    fn an_expected_binary_that_does_not_exist_vouches_for_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let other = write(dir.path(), "impostor", b"");
        let callers = GenuineCallers::of([dir.path().join("paneflow")], None);
        assert!(callers.binaries().is_empty());
        assert_eq!(
            callers.provenance_of_executable(Some(&other)),
            Provenance::Foreign
        );
    }

    #[test]
    fn this_installation_names_its_own_executable_and_the_window_beside_it() {
        let callers = GenuineCallers::of_this_installation();
        let own = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert!(
            callers.binaries().contains(&own),
            "{:?}",
            callers.binaries()
        );
        assert_eq!(
            callers.provenance_of_executable(Some(&own)),
            Provenance::SamePath
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_spawner_is_recognized_by_its_process_identity_whatever_its_executable() {
        use std::os::fd::AsFd;
        let (client, _server) = std::os::unix::net::UnixStream::pair().unwrap();
        let read = paneflow_peer::read_unix_peer(client.as_fd());
        let me = read.identity().unwrap();
        let callers = GenuineCallers::of(Vec::<PathBuf>::new(), Some((me.pid(), me.started_at())));
        assert_eq!(callers.provenance(Some(me)), Provenance::Spawner);
        let reused =
            GenuineCallers::of(Vec::<PathBuf>::new(), Some((me.pid(), me.started_at() + 1)));
        assert_eq!(reused.provenance(Some(me)), Provenance::Foreign);
    }

    #[test]
    fn an_executable_the_embedding_process_vouches_for_becomes_genuine() {
        let dir = tempfile::tempdir().unwrap();
        let window = write(dir.path(), "paneflow", b"the window image");
        let other = write(dir.path(), "built-window", b"a window built elsewhere");
        let callers = GenuineCallers::of([window], None);
        assert_eq!(
            callers.provenance_of_executable(Some(&other)),
            Provenance::Foreign
        );
        callers.vouch_for_executable(&other).unwrap();
        assert_eq!(
            callers.provenance_of_executable(Some(&other)),
            Provenance::SamePath
        );
        assert!(
            callers
                .vouch_for_executable(&dir.path().join("missing"))
                .is_err()
        );
    }
}

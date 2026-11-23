use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_TRUST_FILE_BYTES: u64 = 1024 * 1024;
const TRUST_FILE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrustEntry {
    pub server: String,
    pub root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct TrustFile {
    version: u32,
    allowed: Vec<TrustEntry>,
}

#[derive(Clone, Debug)]
pub struct TrustStore {
    path: PathBuf,
    allowed: BTreeSet<TrustEntry>,
}

impl TrustStore {
    pub fn load(path: &Path) -> Self {
        let allowed = match paneflow_home::read_regular_string_capped(path, MAX_TRUST_FILE_BYTES) {
            Ok(text) => match serde_json::from_str::<TrustFile>(&text) {
                Ok(file) => file.allowed.into_iter().collect(),
                Err(error) => {
                    log::warn!(
                        "ignoring the unreadable language server trust file {}: {error}",
                        path.display()
                    );
                    BTreeSet::new()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => {
                log::warn!(
                    "ignoring the language server trust file {}: {error}",
                    path.display()
                );
                BTreeSet::new()
            }
        };
        Self {
            path: path.to_path_buf(),
            allowed,
        }
    }

    pub fn is_allowed(&self, server: &str, canonical_root: &Path) -> bool {
        self.allowed.contains(&TrustEntry {
            server: server.to_string(),
            root: canonical_root.to_path_buf(),
        })
    }

    pub fn allow(&mut self, server: &str, canonical_root: &Path) -> io::Result<()> {
        self.allowed.insert(TrustEntry {
            server: server.to_string(),
            root: canonical_root.to_path_buf(),
        });
        self.save()
    }

    pub fn forget_all(&mut self) -> io::Result<()> {
        self.allowed.clear();
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let file = TrustFile {
            version: TRUST_FILE_VERSION,
            allowed: self.allowed.iter().cloned().collect(),
        };
        let mut bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        bytes.push(b'\n');
        paneflow_home::write_atomically(&self.path, &bytes)
    }
}

pub fn canonical_root(path: &Path) -> io::Result<PathBuf> {
    Ok(without_verbatim_prefix(std::fs::canonicalize(path)?))
}

fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        if let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_allowed_pair_survives_a_reload_and_other_pairs_stay_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("language-server-trust.json");
        let root = canonical_root(dir.path()).unwrap();
        let mut store = TrustStore::load(&path);
        assert!(!store.is_allowed("rust-analyzer", &root));
        store.allow("rust-analyzer", &root).unwrap();
        let reloaded = TrustStore::load(&path);
        assert!(reloaded.is_allowed("rust-analyzer", &root));
        assert!(!reloaded.is_allowed("zls", &root));
    }

    #[test]
    fn an_unparseable_file_reads_as_empty_and_stays_unchanged_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("language-server-trust.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = TrustStore::load(&path);
        assert!(!store.is_allowed("rust-analyzer", dir.path()));
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[test]
    fn forgetting_empties_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("language-server-trust.json");
        let mut store = TrustStore::load(&path);
        store.allow("zls", dir.path()).unwrap();
        store.forget_all().unwrap();
        assert!(!TrustStore::load(&path).is_allowed("zls", dir.path()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["allowed"],
            serde_json::json!([])
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_canonicalizes_to_the_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            canonical_root(&link).unwrap(),
            canonical_root(&real).unwrap()
        );
    }
}

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(test)]
use paneflow_browser_protocol::BrowserId;
use paneflow_browser_protocol::ProfileId;

const MANIFEST: &str = include_str!("../../../native/browser/manifest.toml");
const PROFILE_SCHEMA: u32 = 1;
const MARKER: &str = "profile.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    InUse,
    Corrupted(String),
    Inaccessible(String),
    NeedsNewerRuntime,
}

impl ProfileError {
    pub fn message(&self) -> String {
        match self {
            Self::InUse => "This profile is already used by another Paneflow instance".to_string(),
            Self::Corrupted(_) => "Cannot open the browser data".to_string(),
            Self::Inaccessible(detail) => format!("Browser data is inaccessible: {detail}"),
            Self::NeedsNewerRuntime => "This data needs a newer browser version".to_string(),
        }
    }
}

pub struct ProfileStore {
    root: PathBuf,
    _lock: File,
}

pub fn engine_version() -> &'static str {
    MANIFEST
        .lines()
        .find_map(|line| line.strip_prefix("cef_version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or("unknown")
}

pub fn allocate_profile_id() -> ProfileId {
    let id = format!("p-{}", uuid::Uuid::new_v4().simple());
    ProfileId::try_from(id).unwrap_or_else(|_| unreachable!("uuid simple form is alphanumeric"))
}

pub fn default_root() -> Option<PathBuf> {
    crate::runtime_paths::data_dir().map(|dir| dir.join("browser"))
}

impl ProfileStore {
    pub fn open(root: PathBuf) -> Result<Self, ProfileError> {
        private_directory(&root)?;
        private_directory(&root.join("profiles"))?;
        let lock = acquire_lock(&root.join("owner.lock"))?;
        Ok(Self { root, _lock: lock })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    pub fn profile_dir(&self, profile: &ProfileId) -> Result<PathBuf, ProfileError> {
        let dir = self.root.join("profiles").join(profile.as_str());
        prepare_profile_dir(&dir)?;
        Ok(dir)
    }

    #[cfg(test)]
    pub fn page_dir(
        &self,
        profile: &ProfileId,
        browser: &BrowserId,
    ) -> Result<PathBuf, ProfileError> {
        let dir = self
            .profile_dir(profile)?
            .join("pages")
            .join(browser.as_str());
        private_directory(&dir)?;
        Ok(dir)
    }

    pub fn has_data(&self, profile: &ProfileId) -> bool {
        self.root
            .join("profiles")
            .join(profile.as_str())
            .join(MARKER)
            .is_file()
    }

    pub fn erase_profile(&self, profile: &ProfileId) -> Result<Option<PathBuf>, ProfileError> {
        let dir = self.root.join("profiles").join(profile.as_str());
        if fs::symlink_metadata(&dir).is_err() {
            return Ok(None);
        }
        let erased = self.root.join("erased");
        private_directory(&erased)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let target = erased.join(format!("{}-{nanos}", profile.as_str()));
        fs::rename(&dir, &target).map_err(|error| {
            ProfileError::Inaccessible(format!("cannot move {}: {error}", dir.display()))
        })?;
        Ok(Some(target))
    }
}

pub(super) fn prepare_profile_dir(dir: &Path) -> Result<(), ProfileError> {
    private_directory(dir)?;
    check_marker(&dir.join(MARKER))
}

fn acquire_lock(path: &Path) -> Result<File, ProfileError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        #[cfg(windows)]
        Err(error) if error.raw_os_error() == Some(32) => return Err(ProfileError::InUse),
        Err(error) => {
            return Err(ProfileError::Inaccessible(format!(
                "cannot open {}: {error}",
                path.display()
            )));
        }
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
                ProfileError::InUse
            } else {
                ProfileError::Inaccessible(format!("cannot lock {}: {error}", path.display()))
            });
        }
    }
    Ok(file)
}

fn private_directory(dir: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(ProfileError::Corrupted(format!(
                    "{} is not a directory",
                    dir.display()
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(
                        |error| {
                            ProfileError::Inaccessible(format!(
                                "cannot restrict {}: {error}",
                                dir.display()
                            ))
                        },
                    )?;
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir).map_err(|error| {
                ProfileError::Inaccessible(format!("cannot create {}: {error}", dir.display()))
            })
        }
        Err(error) => Err(ProfileError::Inaccessible(format!(
            "cannot inspect {}: {error}",
            dir.display()
        ))),
    }
}

fn check_marker(path: &Path) -> Result<(), ProfileError> {
    match File::open(path) {
        Ok(mut file) => {
            let mut text = String::new();
            let mut limited = (&mut file).take(4096);
            limited
                .read_to_string(&mut text)
                .map_err(|error| ProfileError::Corrupted(format!("{}: {error}", path.display())))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| ProfileError::Corrupted(format!("{}: {error}", path.display())))?;
            let schema = value
                .get("schema")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| ProfileError::Corrupted(format!("{}: no schema", path.display())))?;
            if schema > u64::from(PROFILE_SCHEMA) {
                return Err(ProfileError::NeedsNewerRuntime);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let marker =
                serde_json::json!({ "schema": PROFILE_SCHEMA, "engine": engine_version() });
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path).map_err(|error| {
                ProfileError::Inaccessible(format!("cannot create {}: {error}", path.display()))
            })?;
            file.write_all(marker.to_string().as_bytes())
                .map_err(|error| {
                    ProfileError::Inaccessible(format!("cannot write {}: {error}", path.display()))
                })
        }
        Err(error) => Err(ProfileError::Inaccessible(format!(
            "cannot read {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paneflow-browser-profiles-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn profile(id: &str) -> ProfileId {
        ProfileId::try_from(id.to_string()).unwrap()
    }

    #[test]
    fn a_second_holder_of_the_root_is_refused_until_the_owner_releases_it() {
        let root = scratch("lock");
        let first = ProfileStore::open(root.clone()).unwrap();
        assert_eq!(
            ProfileStore::open(root.clone()).err(),
            Some(ProfileError::InUse)
        );
        drop(first);
        assert!(ProfileStore::open(root.clone()).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn profiles_are_private_directories_with_a_versioned_marker() {
        let root = scratch("private");
        let store = ProfileStore::open(root.clone()).unwrap();
        let dir = store.profile_dir(&profile("p-one")).unwrap();
        assert!(dir.starts_with(root.join("profiles")));
        let marker: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join(MARKER)).unwrap()).unwrap();
        assert_eq!(marker["schema"], PROFILE_SCHEMA);
        assert_eq!(marker["engine"], engine_version());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&root, &dir] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        assert!(store.has_data(&profile("p-one")));
        assert!(!store.has_data(&profile("p-two")));
        let page = store
            .page_dir(
                &profile("p-one"),
                &BrowserId::try_from("b1".to_string()).unwrap(),
            )
            .unwrap();
        assert_eq!(page, dir.join("pages").join("b1"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_corrupted_or_newer_marker_is_refused_without_being_rewritten() {
        let root = scratch("corrupt");
        let store = ProfileStore::open(root.clone()).unwrap();
        let dir = root.join("profiles").join("p-bad");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MARKER), b"{not json").unwrap();
        assert!(matches!(
            store.profile_dir(&profile("p-bad")),
            Err(ProfileError::Corrupted(_))
        ));
        assert_eq!(fs::read(dir.join(MARKER)).unwrap(), b"{not json");
        fs::write(dir.join(MARKER), r#"{"schema": 99}"#).unwrap();
        assert_eq!(
            store.profile_dir(&profile("p-bad")).err(),
            Some(ProfileError::NeedsNewerRuntime)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn erasing_is_explicit_and_moves_the_data_out_of_the_profile_root() {
        let root = scratch("erase");
        let store = ProfileStore::open(root.clone()).unwrap();
        let dir = store.profile_dir(&profile("p-gone")).unwrap();
        fs::write(dir.join("Cookies"), b"secret").unwrap();
        let moved = store.erase_profile(&profile("p-gone")).unwrap().unwrap();
        assert!(!dir.exists());
        assert!(moved.starts_with(root.join("erased")));
        assert!(moved.join("Cookies").is_file());
        assert!(!store.has_data(&profile("p-gone")));
        assert_eq!(store.erase_profile(&profile("p-gone")).unwrap(), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn allocated_profile_ids_are_valid_identities_and_unique() {
        let first = allocate_profile_id();
        let second = allocate_profile_id();
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("p-"));
        assert_eq!(first.as_str().len(), 34);
    }
}

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::limits::MAX_MARKDOWN_STATE_SIZE_BYTES;

#[derive(Debug, Serialize, Deserialize)]
pub struct MarkdownState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub offsets: HashMap<String, f32>,
}

const CURRENT_VERSION: u32 = 1;

fn default_version() -> u32 {
    CURRENT_VERSION
}

impl Default for MarkdownState {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            offsets: HashMap::new(),
        }
    }
}

impl MarkdownState {
    pub fn lookup_offset(&self, path: &Path) -> Option<f32> {
        let key = key_for_path(path);
        self.offsets.get(&key).copied()
    }

    pub fn record_offset(&mut self, path: &Path, offset_y: f32) {
        if !offset_y.is_finite() {
            return;
        }
        let key = key_for_path(path);
        self.offsets.insert(key, offset_y);
    }
}

fn key_for_path(path: &Path) -> String {
    normalized_state_path(path).to_string_lossy().into_owned()
}

fn normalized_state_path(path: &Path) -> PathBuf {
    if std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return absolutize_lexical(path);
    }
    path.canonicalize()
        .unwrap_or_else(|_| absolutize_lexical(path))
}

fn absolutize_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    lexical_normalize(absolute)
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn shared() -> &'static Mutex<MarkdownState> {
    static SHARED: OnceLock<Mutex<MarkdownState>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(load()))
}

pub fn lookup_offset_for(path: &Path) -> Option<f32> {
    shared().lock().ok()?.lookup_offset(path)
}

pub fn save_offset_for(path: &Path, offset_y: f32) -> std::io::Result<()> {
    let mut guard = match shared().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.record_offset(path, offset_y);
    save(&guard)
}

pub fn state_file_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "markdown_state-dev.json"
    } else {
        "markdown_state.json"
    };
    dirs::cache_dir().map(|dir| dir.join(crate::runtime_paths::APP_SUBDIR).join(filename))
}

pub fn load() -> MarkdownState {
    let Some(path) = state_file_path() else {
        return MarkdownState::default();
    };
    load_from_path(&path)
}

fn load_from_path(path: &Path) -> MarkdownState {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return MarkdownState::default(),
    };
    if !meta.is_file() {
        log::warn!(
            "markdown_state.json: {} is not a regular file; resetting",
            path.display()
        );
        return MarkdownState::default();
    }
    if meta.len() > MAX_MARKDOWN_STATE_SIZE_BYTES {
        log::warn!(
            "markdown_state.json: {} exceeds {} bytes; resetting",
            path.display(),
            MAX_MARKDOWN_STATE_SIZE_BYTES
        );
        return MarkdownState::default();
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return MarkdownState::default(),
    };
    match serde_json::from_slice::<MarkdownState>(&bytes) {
        Ok(state) => state,
        Err(e) => {
            log::warn!("markdown_state.json: parse failed ({}); resetting", e);
            MarkdownState::default()
        }
    }
}

pub fn save(state: &MarkdownState) -> std::io::Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = temp_path_for(&path);
    if let Err(e) = std::fs::write(&tmp, &json) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!(
            "markdown_state.json: rename failed ({}); leaving prior state",
            e
        );
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "markdown_state.json".to_string());
    parent.join(format!(".{filename}.tmp.{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_returns_none_for_lookup() {
        let s = MarkdownState::default();
        assert!(s.lookup_offset(Path::new("/x.md")).is_none());
    }

    #[test]
    fn record_then_lookup_roundtrips() {
        let mut s = MarkdownState::default();
        s.record_offset(Path::new("/foo/bar.md"), 1234.5);
        assert_eq!(s.lookup_offset(Path::new("/foo/bar.md")), Some(1234.5));
    }

    #[test]
    fn lookup_uses_normalized_existing_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("create subdir");
        let path = sub.join("doc.md");
        std::fs::write(&path, "# title\n").expect("write doc");

        let mut s = MarkdownState::default();
        s.record_offset(&path, 321.0);
        let alias = sub.join(".").join("doc.md");

        assert_eq!(s.lookup_offset(&alias), Some(321.0));
    }

    #[test]
    fn record_overwrites_previous_offset_for_same_path() {
        let mut s = MarkdownState::default();
        s.record_offset(Path::new("/foo.md"), 100.0);
        s.record_offset(Path::new("/foo.md"), 200.0);
        assert_eq!(s.lookup_offset(Path::new("/foo.md")), Some(200.0));
    }

    #[test]
    fn json_roundtrip_preserves_offsets() {
        let mut s = MarkdownState::default();
        s.record_offset(Path::new("/a.md"), 10.0);
        s.record_offset(Path::new("/b.md"), 42.5);
        let serialized = serde_json::to_string(&s).expect("ser");
        let restored: MarkdownState = serde_json::from_str(&serialized).expect("de");
        assert_eq!(restored.lookup_offset(Path::new("/a.md")), Some(10.0));
        assert_eq!(restored.lookup_offset(Path::new("/b.md")), Some(42.5));
        assert_eq!(restored.version, 1);
    }

    #[test]
    fn missing_version_falls_back_to_default() {
        let json = r#"{ "offsets": { "/x.md": 5.0 } }"#;
        let restored: MarkdownState = serde_json::from_str(json).expect("de");
        assert_eq!(restored.version, 1);
        assert_eq!(restored.offsets.get("/x.md"), Some(&5.0));
    }

    #[test]
    fn corrupt_input_does_not_panic() {
        let res: Result<MarkdownState, _> = serde_json::from_str("{ malformed");
        assert!(res.is_err());
    }

    #[test]
    fn load_rejects_oversized_state_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("markdown_state.json");
        std::fs::write(
            &path,
            vec![b' '; (crate::limits::MAX_MARKDOWN_STATE_SIZE_BYTES + 1) as usize],
        )
        .expect("write oversized cache");

        let state = load_from_path(&path);
        assert!(state.offsets.is_empty());
        assert_eq!(state.version, 1);
    }

    #[test]
    fn load_rejects_non_file_state_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = load_from_path(dir.path());
        assert!(state.offsets.is_empty());
        assert_eq!(state.version, 1);
    }
}

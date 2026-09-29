use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};

use crate::limits::MAX_MARKDOWN_STATE_SIZE_BYTES;

#[derive(Debug, Serialize, Deserialize)]
pub struct MarkdownState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub offsets: HashMap<String, f32>,
    #[serde(default)]
    pub recent: Vec<String>,
}

const CURRENT_VERSION: u32 = 1;

pub(crate) const MAX_MARKDOWN_STATE_ENTRIES: usize = 1000;

const MAX_MARKDOWN_STATE_READ_BYTES: u64 = 16 * MAX_MARKDOWN_STATE_SIZE_BYTES;

fn default_version() -> u32 {
    CURRENT_VERSION
}

impl Default for MarkdownState {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            offsets: HashMap::new(),
            recent: Vec::new(),
        }
    }
}

impl MarkdownState {
    #[cfg(test)]
    pub fn lookup_offset(&self, path: &Path) -> Option<f32> {
        let key = key_for_path(path);
        self.offsets.get(&key).copied()
    }

    pub fn record_offset(&mut self, path: &Path, offset_y: f32) {
        if !offset_y.is_finite() {
            return;
        }
        let key = key_for_path(path);
        self.offsets.insert(key.clone(), offset_y);
        self.touch(&key);
        self.evict_to(MAX_MARKDOWN_STATE_ENTRIES);
    }

    fn touch(&mut self, key: &str) {
        self.recent.retain(|recent| recent != key);
        self.recent.push(key.to_string());
    }

    fn evict_to(&mut self, max: usize) {
        let mut seen = HashSet::new();
        let mut recent: Vec<String> = self
            .recent
            .drain(..)
            .rev()
            .filter(|key| self.offsets.contains_key(key) && seen.insert(key.clone()))
            .collect();
        recent.reverse();
        self.recent = recent;
        let excess = self.offsets.len().saturating_sub(max);
        if excess == 0 {
            return;
        }
        let mut untracked: Vec<String> = self
            .offsets
            .keys()
            .filter(|key| !seen.contains(*key))
            .cloned()
            .collect();
        untracked.sort();
        let evicted: Vec<String> = untracked
            .into_iter()
            .chain(self.recent.iter().cloned())
            .take(excess)
            .collect();
        for key in &evicted {
            self.offsets.remove(key);
        }
        self.recent.retain(|key| self.offsets.contains_key(key));
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
    let key = key_for_path(path);
    let mut guard = shared().lock().unwrap_or_else(PoisonError::into_inner);
    let offset = guard.offsets.get(&key).copied()?;
    guard.touch(&key);
    Some(offset)
}

pub fn save_offset_for(path: &Path, offset_y: f32) -> std::io::Result<()> {
    let mut guard = shared().lock().unwrap_or_else(PoisonError::into_inner);
    guard.record_offset(path, offset_y);
    save(&guard)
}

pub fn state_file_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "markdown_state-dev.json"
    } else {
        "markdown_state.json"
    };
    crate::runtime_paths::cache_dir().map(|dir| dir.join(filename))
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
    if meta.len() > MAX_MARKDOWN_STATE_READ_BYTES {
        log::warn!(
            "markdown_state.json: {} exceeds {} bytes; resetting",
            path.display(),
            MAX_MARKDOWN_STATE_READ_BYTES
        );
        return MarkdownState::default();
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return MarkdownState::default(),
    };
    let mut state = match serde_json::from_slice::<MarkdownState>(&bytes) {
        Ok(state) => state,
        Err(e) => {
            log::warn!("markdown_state.json: parse failed ({}); resetting", e);
            return MarkdownState::default();
        }
    };
    let stored = state.offsets.len();
    state.evict_to(MAX_MARKDOWN_STATE_ENTRIES);
    if meta.len() > MAX_MARKDOWN_STATE_SIZE_BYTES || stored > state.offsets.len() {
        log::warn!(
            "markdown_state.json: {} held {stored} entries in {} bytes; kept the {} most recent",
            path.display(),
            meta.len(),
            state.offsets.len()
        );
    }
    state
}

pub fn save(state: &MarkdownState) -> std::io::Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };
    save_to(&path, state)
}

fn save_to(path: &Path, state: &MarkdownState) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    paneflow_home::write_atomically(path, json.as_bytes()).inspect_err(|e| {
        log::warn!("markdown_state.json: write failed ({e}); leaving prior state");
    })
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
    fn an_oversized_state_keeps_its_most_recent_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("markdown_state.json");
        let padding = "p".repeat(800);
        let total = MAX_MARKDOWN_STATE_ENTRIES + 500;
        let keys: Vec<String> = (0..total)
            .map(|i| format!("/docs/{padding}/{i:05}.md"))
            .collect();
        let legacy_key = "/docs/legacy-untracked.md".to_string();
        let mut state = MarkdownState::default();
        for (i, key) in keys.iter().enumerate() {
            state.offsets.insert(key.clone(), i as f32);
            state.recent.push(key.clone());
        }
        state.offsets.insert(legacy_key.clone(), 1.0);
        let json = serde_json::to_vec(&state).expect("ser");
        assert!(json.len() as u64 > crate::limits::MAX_MARKDOWN_STATE_SIZE_BYTES);
        std::fs::write(&path, json).expect("write oversized state");

        let loaded = load_from_path(&path);

        assert_eq!(loaded.offsets.len(), MAX_MARKDOWN_STATE_ENTRIES);
        assert!(!loaded.offsets.contains_key(&legacy_key));
        assert!(!loaded.offsets.contains_key(&keys[499]));
        assert_eq!(loaded.offsets.get(&keys[500]), Some(&500.0));
        assert_eq!(
            loaded.offsets.get(&keys[total - 1]),
            Some(&((total - 1) as f32))
        );
        assert_eq!(loaded.recent.len(), MAX_MARKDOWN_STATE_ENTRIES);
    }

    #[test]
    fn recording_past_the_cap_evicts_the_least_recently_used_entry() {
        let mut s = MarkdownState::default();
        for i in 0..MAX_MARKDOWN_STATE_ENTRIES {
            s.record_offset(Path::new(&format!("/lru/{i}.md")), i as f32);
        }
        let first = key_for_path(Path::new("/lru/0.md"));
        s.touch(&first);
        s.record_offset(Path::new("/lru/new.md"), 1.0);

        assert_eq!(s.offsets.len(), MAX_MARKDOWN_STATE_ENTRIES);
        assert_eq!(s.lookup_offset(Path::new("/lru/0.md")), Some(0.0));
        assert_eq!(s.lookup_offset(Path::new("/lru/1.md")), None);
        assert_eq!(s.lookup_offset(Path::new("/lru/new.md")), Some(1.0));
    }

    #[test]
    fn load_rejects_non_file_state_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = load_from_path(dir.path());
        assert!(state.offsets.is_empty());
        assert_eq!(state.version, 1);
    }

    fn link_to(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
    }

    fn is_link(path: &std::path::Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    #[test]
    fn saving_through_a_symlink_keeps_the_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").expect("seed");
        let link = dir.path().join("markdown_state.json");
        link_to(&target, &link);
        let mut state = MarkdownState::default();
        state.record_offset(Path::new("/notes.md"), 42.0);

        save_to(&link, &state).expect("save");

        assert!(is_link(&link));
        assert_eq!(
            load_from_path(&target).lookup_offset(Path::new("/notes.md")),
            Some(42.0)
        );
    }
}

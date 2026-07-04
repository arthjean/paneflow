//! Per-file markdown scroll-position persistence (US-022 AC).
//!
//! On `MarkdownView::open(path)`, the view consults `MarkdownState::load_offset`
//! to restore the user's last viewing offset. While the user scrolls, the
//! view debounces writes and calls `MarkdownState::save_offset(path, offset)`.
//!
//! Storage: `dirs::cache_dir()/paneflow/markdown_state.json`, written
//! atomically via `write to .tmp + rename` (mirrors `app/session.rs`). The
//! file maps absolute path → vertical scroll offset in CSS pixels. Schema is
//! versioned so a future field addition can be additive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::limits::MAX_MARKDOWN_STATE_SIZE_BYTES;

/// On-disk schema. Serialized as `markdown_state.json`. New fields must be
/// `#[serde(default)]` so older files still load.
#[derive(Debug, Serialize, Deserialize)]
pub struct MarkdownState {
    /// Schema version. Incremented when a non-additive change lands. Loaders
    /// tolerate unknown values and fall back to default state. Initialised
    /// to `CURRENT_VERSION` rather than `u32::default()` (0) so freshly
    /// created files round-trip with a non-zero version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Absolute path → last vertical scroll offset (CSS pixels).
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
    /// Look up a previously-saved scroll offset for `path`. Returns `None` if
    /// the path was never opened, or the cache is missing/corrupt.
    pub fn lookup_offset(&self, path: &Path) -> Option<f32> {
        let key = path.to_str()?;
        self.offsets.get(key).copied()
    }

    /// Update the offset for `path`. Caller is responsible for calling `save`
    /// to persist the change to disk. Non-finite values (NaN, ±Inf) are
    /// silently dropped - they would round-trip through JSON as `null` or
    /// crash GPUI's layout when applied via `set_offset`.
    pub fn record_offset(&mut self, path: &Path, offset_y: f32) {
        if !offset_y.is_finite() {
            return;
        }
        if let Some(key) = path.to_str() {
            self.offsets.insert(key.to_string(), offset_y);
        }
    }
}

/// Process-wide shared state. All `MarkdownView` panes serialize their
/// scroll-position writes through this mutex so two concurrent persist
/// tasks cannot lose updates via a load → modify → save race. Initialised
/// lazily on first access from the on-disk file (or empty default).
fn shared() -> &'static Mutex<MarkdownState> {
    static SHARED: OnceLock<Mutex<MarkdownState>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(load()))
}

/// Public entry point used by `MarkdownView::open` to look up a previously
/// saved offset for `path`.
pub fn lookup_offset_for(path: &Path) -> Option<f32> {
    shared().lock().ok()?.lookup_offset(path)
}

/// Public entry point used by `MarkdownView`'s persistence task. Records
/// the new offset in the shared state and flushes the whole map to disk
/// atomically. The flush happens under the mutex so concurrent writers
/// observe a consistent on-disk file.
pub fn save_offset_for(path: &Path, offset_y: f32) -> std::io::Result<()> {
    let mut guard = match shared().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.record_offset(path, offset_y);
    save(&guard)
}

/// Resolve the on-disk JSON path. Returns `None` when `dirs::cache_dir`
/// can't determine a cache directory (extremely rare; e.g. exotic targets).
/// Debug builds use a `-dev` suffix so dev/release runs don't share state.
/// The subdir itself is also `-dev`-suffixed via
/// [`crate::runtime_paths::APP_SUBDIR`] so dev runs never write into the
/// installed Paneflow's cache namespace -- belt + suspenders, the file
/// suffix alone is kept for backward compatibility with already-deployed
/// release caches.
pub fn state_file_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "markdown_state-dev.json"
    } else {
        "markdown_state.json"
    };
    dirs::cache_dir().map(|dir| dir.join(crate::runtime_paths::APP_SUBDIR).join(filename))
}

/// Load the state file from disk. A missing or corrupt file returns the
/// default empty state - the cache is not load-bearing, so the view always
/// renders something.
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

/// Persist `state` atomically: write to a `.tmp` sibling, then rename. Mirrors
/// the convention used by `app/session.rs::save_session`. Returns `Err` only
/// for path/serde failures; rename failures are logged and the temp is
/// cleaned up so a partial state file is never left in place.
pub fn save(state: &MarkdownState) -> std::io::Result<()> {
    let Some(path) = state_file_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
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
        // A pre-version file (or hand-edited) should still load - the
        // `default = "default_version"` serde attribute provides v1.
        let json = r#"{ "offsets": { "/x.md": 5.0 } }"#;
        let restored: MarkdownState = serde_json::from_str(json).expect("de");
        assert_eq!(restored.version, 1);
        assert_eq!(restored.lookup_offset(Path::new("/x.md")), Some(5.0));
    }

    #[test]
    fn corrupt_input_does_not_panic() {
        // Direct deserialize returns Err; the `load()` wrapper would silently
        // fall back to default state. We cannot exercise `load()` here
        // without poking the user's cache dir, so test the parser directly.
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

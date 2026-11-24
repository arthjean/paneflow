use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use paneflow_api::grants::{GrantSet, GrantView, PaneGrant};
use paneflow_config::schema::SessionId;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug, Default)]
struct MirrorState {
    held: Option<GrantSet>,
    announced: u64,
    published: Option<(Option<Stamp>, u64)>,
}

#[derive(Debug)]
pub struct GrantMirror {
    published_file: Option<PathBuf>,
    state: Mutex<MirrorState>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorSnapshot {
    pub grants: Vec<PaneGrant>,
    pub synced: bool,
}

impl MirrorSnapshot {
    pub fn view(&self, now_ms: u64, generation: Option<u64>) -> GrantView<'_> {
        GrantView {
            grants: &self.grants,
            now_ms,
            generation,
            synced: self.synced,
        }
    }
}

#[derive(Deserialize)]
struct PublishedRevision {
    revision: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().map(|metadata| Stamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

pub fn published_revision(path: &Path) -> u64 {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PublishedRevision>(&bytes).ok())
        .map_or(0, |published| published.revision)
}

impl GrantMirror {
    pub fn new(published_file: Option<PathBuf>) -> Self {
        Self {
            published_file,
            state: Mutex::new(MirrorState::default()),
        }
    }

    pub fn for_home(home: &Path) -> Self {
        Self::new(Some(paneflow_home::serve_grants_path_in(home)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MirrorState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn install(&self, set: GrantSet) -> bool {
        let mut state = self.lock();
        if state
            .held
            .as_ref()
            .is_some_and(|held| held.revision > set.revision)
        {
            return false;
        }
        state.announced = state.announced.max(set.revision);
        state.held = Some(set);
        true
    }

    pub fn announce(&self, revision: u64) {
        let mut state = self.lock();
        state.announced = state.announced.max(revision);
    }

    pub fn held_revision(&self) -> u64 {
        self.lock().held.as_ref().map_or(0, |held| held.revision)
    }

    fn published(&self, state: &mut MirrorState) -> u64 {
        let Some(path) = self.published_file.as_deref() else {
            return 0;
        };
        let stamp = stamp_of(path);
        match state.published {
            Some((seen, revision)) if seen == stamp => revision,
            _ => {
                let revision = stamp.map_or(0, |_| published_revision(path));
                state.published = Some((stamp, revision));
                revision
            }
        }
    }

    pub fn snapshot(&self) -> MirrorSnapshot {
        let mut state = self.lock();
        let latest = state.announced.max(self.published(&mut state));
        let held = state.held.as_ref().map_or(0, |held| held.revision);
        MirrorSnapshot {
            grants: state
                .held
                .as_ref()
                .map(|held| held.grants.clone())
                .unwrap_or_default(),
            synced: held >= latest,
        }
    }
}

pub fn manifest_generation(home: &Path, session: &str) -> Option<u64> {
    let session = SessionId::parse(session).ok()?;
    crate::manifest::read_manifest(&crate::manifest::manifest_path(home, &session))
        .ok()
        .map(|manifest| manifest.generation.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::grants::{GrantHolder, GrantOrigin, GrantScope, Right};

    fn grant(id: &str) -> PaneGrant {
        PaneGrant {
            id: id.into(),
            holder: GrantHolder {
                session: "s-1".into(),
                generation: 1,
                workspace: None,
            },
            rights: vec![Right::Write],
            scope: GrantScope::Workspace,
            expires_at_ms: None,
            delegated_from: None,
            origin: GrantOrigin::Prompt,
            granted_at_ms: 0,
        }
    }

    #[test]
    fn a_mirror_is_stale_until_it_holds_the_announced_revision() {
        let mirror = GrantMirror::new(None);
        assert!(mirror.snapshot().synced);
        mirror.announce(3);
        assert!(!mirror.snapshot().synced);
        assert!(mirror.install(GrantSet {
            revision: 3,
            grants: vec![grant("g-1")],
        }));
        let snapshot = mirror.snapshot();
        assert!(snapshot.synced);
        assert_eq!(snapshot.grants.len(), 1);
        assert!(!mirror.install(GrantSet {
            revision: 2,
            grants: Vec::new(),
        }));
        assert_eq!(mirror.held_revision(), 3);
    }

    #[test]
    fn a_mirror_is_stale_while_the_published_file_names_a_newer_revision() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("grants.json");
        let mirror = GrantMirror::new(Some(path.clone()));
        assert!(mirror.snapshot().synced);
        std::fs::write(&path, r#"{"revision": 7, "grants": []}"#).expect("written");
        assert!(!mirror.snapshot().synced);
        mirror.install(GrantSet {
            revision: 7,
            grants: Vec::new(),
        });
        assert!(mirror.snapshot().synced);
    }
}

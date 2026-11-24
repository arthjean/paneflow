use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use paneflow_api::grants::{
    GrantHolder, GrantOrigin, GrantScope, GrantSet, MAX_GRANTS, MAX_GRANTS_PER_HOLDER, PaneGrant,
    Right,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    revision: u64,
    grants: Vec<PaneGrant>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    spawned_by: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    revoked_profiles: Vec<(GrantHolder, GrantOrigin)>,
}

#[derive(Debug, Default)]
struct StoreState {
    revision: u64,
    grants: Vec<PaneGrant>,
    spawned_by: BTreeMap<String, String>,
    revoked_profiles: Vec<(GrantHolder, GrantOrigin)>,
    issued: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub revision: u64,
    pub revoked: Vec<String>,
    pub set: GrantSet,
}

#[derive(Debug)]
pub struct PaneGrantStore {
    path: PathBuf,
    state: Mutex<StoreState>,
}

pub struct NewGrant {
    pub holder: GrantHolder,
    pub rights: Vec<Right>,
    pub scope: GrantScope,
    pub expires_at_ms: Option<u64>,
    pub delegated_from: Option<String>,
    pub origin: GrantOrigin,
}

fn read_file(path: &Path) -> Result<Option<GrantFile>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn descendants(grants: &[PaneGrant], roots: BTreeSet<String>) -> BTreeSet<String> {
    let mut gone = roots;
    loop {
        let before = gone.len();
        for grant in grants {
            if grant
                .delegated_from
                .as_ref()
                .is_some_and(|parent| gone.contains(parent))
            {
                gone.insert(grant.id.clone());
            }
        }
        if gone.len() == before {
            return gone;
        }
    }
}

impl StoreState {
    fn drop_with_delegations(&mut self, roots: BTreeSet<String>) -> Vec<String> {
        if roots.is_empty() {
            return Vec::new();
        }
        let gone = descendants(&self.grants, roots);
        let mut revoked = Vec::new();
        self.grants.retain(|grant| {
            let drop = gone.contains(&grant.id);
            if drop {
                revoked.push(grant.id.clone());
            }
            !drop
        });
        revoked
    }

    fn has_room_for(&self, holder: &GrantHolder) -> bool {
        self.grants.len() < MAX_GRANTS
            && self
                .grants
                .iter()
                .filter(|grant| grant.holder.session == holder.session)
                .count()
                < MAX_GRANTS_PER_HOLDER
    }

    fn set(&self) -> GrantSet {
        GrantSet {
            revision: self.revision,
            grants: self.grants.clone(),
        }
    }

    fn file(&self) -> GrantFile {
        GrantFile {
            revision: self.revision,
            grants: self.grants.clone(),
            spawned_by: self.spawned_by.clone(),
            revoked_profiles: self.revoked_profiles.clone(),
        }
    }
}

impl PaneGrantStore {
    pub fn open(home: &Path, now_ms: u64) -> Self {
        let path = paneflow_home::serve_grants_path_in(home);
        let mut state = StoreState::default();
        match read_file(&path) {
            Ok(Some(file)) => {
                state.revision = file.revision;
                state.grants = file.grants;
                state.spawned_by = file.spawned_by;
                state.revoked_profiles = file.revoked_profiles;
            }
            Ok(None) => {}
            Err(cause) => {
                log::warn!("paneflow-serve: starting with no grant: {cause}");
            }
        }
        state.revision = state.revision.max(now_ms);
        let expired = state
            .grants
            .iter()
            .filter(|grant| !grant.is_live_at(now_ms))
            .map(|grant| grant.id.clone())
            .collect();
        state.drop_with_delegations(expired);
        let store = Self {
            path,
            state: Mutex::new(state),
        };
        store.persist(&store.lock());
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoreState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self, state: &StoreState) {
        let Ok(bytes) = serde_json::to_vec_pretty(&state.file()) else {
            return;
        };
        if let Err(error) = paneflow_host::manifest::write_atomically(&self.path, &bytes) {
            log::warn!(
                "paneflow-serve: cannot write {}: {error}",
                self.path.display()
            );
        }
    }

    fn commit(&self, state: &mut StoreState, revoked: Vec<String>) -> Change {
        state.revision += 1;
        self.persist(state);
        Change {
            revision: state.revision,
            revoked,
            set: state.set(),
        }
    }

    pub fn set(&self) -> GrantSet {
        self.lock().set()
    }

    pub fn get(&self, id: &str) -> Option<PaneGrant> {
        self.lock()
            .grants
            .iter()
            .find(|grant| grant.id == id)
            .cloned()
    }

    pub fn has_room_for(&self, holder: &GrantHolder) -> bool {
        self.lock().has_room_for(holder)
    }

    pub fn issue(&self, grant: NewGrant, now_ms: u64) -> Option<(PaneGrant, Change)> {
        let mut state = self.lock();
        if !state.has_room_for(&grant.holder) {
            return None;
        }
        Some(self.store(&mut state, grant, now_ms))
    }

    pub fn create(&self, grant: NewGrant, now_ms: u64) -> (PaneGrant, Change) {
        let mut state = self.lock();
        self.store(&mut state, grant, now_ms)
    }

    fn store(&self, state: &mut StoreState, grant: NewGrant, now_ms: u64) -> (PaneGrant, Change) {
        state.issued += 1;
        let created = PaneGrant {
            id: format!("g-{}-{}", state.revision + 1, state.issued),
            holder: grant.holder,
            rights: paneflow_api::grants::normalized_rights(&grant.rights),
            scope: grant.scope,
            expires_at_ms: grant.expires_at_ms,
            delegated_from: grant.delegated_from,
            origin: grant.origin,
            granted_at_ms: now_ms,
        };
        state.grants.push(created.clone());
        let change = self.commit(state, Vec::new());
        (created, change)
    }

    pub fn revoke(&self, id: &str) -> Option<Change> {
        let mut state = self.lock();
        if !state.grants.iter().any(|grant| grant.id == id) {
            return None;
        }
        let profile_grants: Vec<(GrantHolder, GrantOrigin)> = state
            .grants
            .iter()
            .filter(|grant| grant.id == id && matches!(grant.origin, GrantOrigin::Profile { .. }))
            .map(|grant| (grant.holder.clone(), grant.origin.clone()))
            .collect();
        state.revoked_profiles.extend(profile_grants);
        let revoked = state.drop_with_delegations(BTreeSet::from([id.to_string()]));
        Some(self.commit(&mut state, revoked))
    }

    pub fn reconcile(
        &self,
        now_ms: u64,
        generation_of: impl Fn(&str) -> Option<u64>,
        wanted: Vec<NewGrant>,
    ) -> Option<Change> {
        let mut state = self.lock();
        let stale: BTreeSet<String> = state
            .grants
            .iter()
            .filter(|grant| {
                !grant.is_live_at(now_ms)
                    || generation_of(&grant.holder.session) != Some(grant.holder.generation)
            })
            .map(|grant| grant.id.clone())
            .collect();
        let revoked = state.drop_with_delegations(stale);
        let live_sessions: BTreeSet<String> = state
            .spawned_by
            .keys()
            .filter(|child| generation_of(child).is_some())
            .cloned()
            .collect();
        let spawns_before = state.spawned_by.len() + state.revoked_profiles.len();
        state
            .spawned_by
            .retain(|child, _| live_sessions.contains(child));
        state
            .revoked_profiles
            .retain(|(holder, _)| generation_of(&holder.session) == Some(holder.generation));
        let mut added = false;
        for grant in wanted {
            let rights = paneflow_api::grants::normalized_rights(&grant.rights);
            let declined = state
                .revoked_profiles
                .iter()
                .any(|(holder, origin)| *holder == grant.holder && *origin == grant.origin);
            let present = declined
                || state.grants.iter().any(|held| {
                    held.holder == grant.holder
                        && held.origin == grant.origin
                        && held.rights == rights
                        && held.scope == grant.scope
                });
            if present || !state.has_room_for(&grant.holder) {
                continue;
            }
            state.issued += 1;
            let id = format!("g-{}-{}", state.revision + 1, state.issued);
            state.grants.push(PaneGrant {
                id,
                holder: grant.holder,
                rights,
                scope: grant.scope,
                expires_at_ms: grant.expires_at_ms,
                delegated_from: grant.delegated_from,
                origin: grant.origin,
                granted_at_ms: now_ms,
            });
            added = true;
        }
        if revoked.is_empty()
            && !added
            && spawns_before == state.spawned_by.len() + state.revoked_profiles.len()
        {
            return None;
        }
        if revoked.is_empty() && !added {
            self.persist(&state);
            return None;
        }
        Some(self.commit(&mut state, revoked))
    }

    pub fn record_spawn(&self, parent: &str, child: &str) {
        let mut state = self.lock();
        state
            .spawned_by
            .insert(child.to_string(), parent.to_string());
        self.persist(&state);
    }

    pub fn spawned_by(&self, child: &str) -> Option<String> {
        self.lock().spawned_by.get(child).cloned()
    }

    pub fn held_by(&self, session: &str, generation: Option<u64>, now_ms: u64) -> Vec<PaneGrant> {
        self.lock()
            .grants
            .iter()
            .filter(|grant| grant.is_held_by(session, generation) && grant.is_live_at(now_ms))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(session: &str, generation: u64) -> GrantHolder {
        GrantHolder {
            session: session.into(),
            generation,
            workspace: Some("w-1".into()),
        }
    }

    fn new_grant(session: &str, expires_at_ms: Option<u64>) -> NewGrant {
        NewGrant {
            holder: holder(session, 1),
            rights: vec![Right::Write],
            scope: GrantScope::Workspace,
            expires_at_ms,
            delegated_from: None,
            origin: GrantOrigin::Prompt,
        }
    }

    #[test]
    fn a_stored_grant_records_its_holder_rights_scope_expiry_and_parent() {
        let home = tempfile::tempdir().expect("a home");
        let store = PaneGrantStore::open(home.path(), 10);
        let (parent, _) = store.create(new_grant("s-1", Some(500)), 10);
        let (child, change) = store.create(
            NewGrant {
                delegated_from: Some(parent.id.clone()),
                origin: GrantOrigin::Delegation,
                ..new_grant("s-2", Some(400))
            },
            20,
        );
        assert_eq!(child.holder, holder("s-2", 1));
        assert_eq!(child.rights, vec![Right::Write]);
        assert_eq!(child.scope, GrantScope::Workspace);
        assert_eq!(child.expires_at_ms, Some(400));
        assert_eq!(child.delegated_from.as_deref(), Some(parent.id.as_str()));
        assert_eq!(change.set.grants.len(), 2);
        let revoked = store.revoke(&parent.id).expect("revoked");
        assert_eq!(revoked.revoked, vec![parent.id, child.id]);
        assert!(store.set().grants.is_empty());
    }

    #[test]
    fn a_full_store_issues_no_grant_and_its_set_fits_one_host_frame() {
        let home = tempfile::tempdir().expect("a home");
        let store = PaneGrantStore::open(home.path(), 1_763_900_000_000);
        let sessions: Vec<String> = (0..MAX_GRANTS / MAX_GRANTS_PER_HOLDER + 1)
            .map(|_| paneflow_config::schema::SessionId::new().to_string())
            .collect();
        let workspace = paneflow_config::schema::WorkspaceId::new().to_string();
        let wanted = |session: &str, parent: Option<String>| NewGrant {
            holder: GrantHolder {
                session: session.to_string(),
                generation: u64::MAX,
                workspace: Some(workspace.clone()),
            },
            rights: Right::ALL.to_vec(),
            scope: GrantScope::Workspace,
            expires_at_ms: Some(u64::MAX),
            delegated_from: parent,
            origin: GrantOrigin::Profile {
                profile: "a-long-agent-profile-name".into(),
            },
        };
        let mut parent = None;
        for session in &sessions[..MAX_GRANTS / MAX_GRANTS_PER_HOLDER] {
            for _ in 0..MAX_GRANTS_PER_HOLDER {
                let (grant, _) = store
                    .issue(wanted(session, parent.clone()), u64::MAX)
                    .expect("room under both limits");
                parent = Some(grant.id);
            }
            assert!(store.issue(wanted(session, None), 0).is_none());
        }
        assert_eq!(store.set().grants.len(), MAX_GRANTS);
        let last = sessions.last().expect("a session");
        assert!(store.issue(wanted(last, None), 0).is_none());
        let frame = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": u64::MAX,
            "method": "host.grants.set",
            "params": store.set(),
        }))
        .expect("a frame");
        assert!(
            frame.len() < paneflow_ipc_client::host_control::MAX_CONTROL_FRAME_BYTES,
            "{} bytes",
            frame.len()
        );
    }

    #[test]
    fn a_restart_restores_live_grants_and_drops_expired_ones_with_their_delegations() {
        let home = tempfile::tempdir().expect("a home");
        let store = PaneGrantStore::open(home.path(), 10);
        let (lasting, _) = store.create(new_grant("s-1", None), 10);
        let (short, _) = store.create(new_grant("s-2", Some(50)), 10);
        store.create(
            NewGrant {
                delegated_from: Some(short.id.clone()),
                ..new_grant("s-3", None)
            },
            10,
        );
        let revision = store.set().revision;
        drop(store);
        let reopened = PaneGrantStore::open(home.path(), 60);
        let ids: Vec<String> = reopened.set().grants.into_iter().map(|g| g.id).collect();
        assert_eq!(ids, vec![lasting.id]);
        assert!(reopened.set().revision >= revision);
    }

    #[test]
    fn an_unparseable_grant_file_starts_the_store_empty() {
        let home = tempfile::tempdir().expect("a home");
        let path = paneflow_home::serve_grants_path_in(home.path());
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("serve dir");
        std::fs::write(&path, "{not json").expect("written");
        let store = PaneGrantStore::open(home.path(), 10);
        assert!(store.set().grants.is_empty());
    }

    #[test]
    fn reconciling_drops_grants_of_ended_or_restarted_holders() {
        let home = tempfile::tempdir().expect("a home");
        let store = PaneGrantStore::open(home.path(), 10);
        store.create(new_grant("s-ended", None), 10);
        store.create(new_grant("s-restarted", None), 10);
        let (kept, _) = store.create(new_grant("s-live", None), 10);
        let change = store
            .reconcile(
                20,
                |session| match session {
                    "s-live" => Some(1),
                    "s-restarted" => Some(2),
                    _ => None,
                },
                Vec::new(),
            )
            .expect("a change");
        assert_eq!(change.revoked.len(), 2);
        assert_eq!(change.set.grants, vec![kept]);
        assert!(store.reconcile(20, |_| Some(1), Vec::new()).is_none());
    }
}

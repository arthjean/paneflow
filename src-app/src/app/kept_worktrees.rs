use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{AppContext as _, Context};

use crate::PaneFlowApp;
use crate::workspace::worktree::ManagedWorktree;

const LEDGER_CAP: u64 = 1 << 20;

static LEDGER_WRITES: Mutex<u64> = Mutex::new(0);

static LEDGER_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Ledger {
    #[serde(default)]
    worktrees: Vec<LedgerEntry>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LedgerEntry {
    path: String,
    repo_root: String,
    branch: String,
    teardown: String,
}

pub(crate) fn read_ledger(path: &Path) -> Vec<ManagedWorktree> {
    let raw = match paneflow_home::read_regular_string_capped(path, LEDGER_CAP) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            log::warn!("kept worktrees: cannot read {}: {error}", path.display());
            return Vec::new();
        }
    };
    let ledger: Ledger = match serde_json::from_str(&raw) {
        Ok(ledger) => ledger,
        Err(error) => {
            log::warn!("kept worktrees: {} is not valid: {error}", path.display());
            return Vec::new();
        }
    };
    let mut kept: Vec<ManagedWorktree> = Vec::with_capacity(ledger.worktrees.len());
    for entry in ledger.worktrees {
        let Some(worktree) = crate::workspace::worktree::managed_worktree_from_record(
            &entry.path,
            &entry.repo_root,
            &entry.branch,
            &entry.teardown,
        ) else {
            continue;
        };
        if kept.iter().all(|known| known.path != worktree.path) {
            kept.push(worktree);
        }
    }
    kept
}

pub(crate) fn write_ledger(path: &Path, kept: &[ManagedWorktree]) -> std::io::Result<()> {
    if kept.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let ledger = Ledger {
        worktrees: kept
            .iter()
            .map(|wt| LedgerEntry {
                path: wt.path.to_string_lossy().into_owned(),
                repo_root: wt.repo_root.to_string_lossy().into_owned(),
                branch: wt.branch.clone(),
                teardown: wt.teardown.as_str().to_string(),
            })
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&ledger).map_err(std::io::Error::other)?;
    paneflow_home::write_atomically(path, &json)
}

fn persist(generation: u64, kept: Vec<ManagedWorktree>) {
    let Some(path) = paneflow_home::kept_worktrees_path() else {
        return;
    };
    let mut written = LEDGER_WRITES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if generation <= *written {
        return;
    }
    if let Err(error) = write_ledger(&path, &kept) {
        log::warn!("kept worktrees: cannot write {}: {error}", path.display());
        return;
    }
    *written = generation;
}

impl PaneFlowApp {
    pub(crate) fn spawn_kept_worktrees_load(cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let loaded = smol::unblock(|| {
                    paneflow_home::kept_worktrees_path()
                        .map(|path| read_ledger(&path))
                        .unwrap_or_default()
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.keep_unlisted_worktrees(loaded, cx);
                        app.persist_kept_worktrees(cx);
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn keep_unlisted_worktrees(
        &mut self,
        kept: Vec<ManagedWorktree>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for wt in kept {
            let listed = self
                .workspaces
                .iter()
                .flat_map(|ws| ws.managed_worktrees.iter())
                .chain(self.kept_worktrees.iter())
                .any(|known| known.path == wt.path);
            if !listed {
                self.kept_worktrees.push(wt);
                changed = true;
            }
        }
        if changed {
            self.persist_kept_worktrees(cx);
            cx.notify();
        }
    }

    pub(crate) fn forget_kept_paths(&mut self, removed: &[PathBuf], cx: &mut Context<Self>) {
        let before = self.kept_worktrees.len();
        self.kept_worktrees.retain(|wt| !removed.contains(&wt.path));
        if self.kept_worktrees.len() != before {
            self.persist_kept_worktrees(cx);
        }
    }

    fn persist_kept_worktrees(&mut self, cx: &mut Context<Self>) {
        let generation = LEDGER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let kept = self.kept_worktrees.clone();
        cx.background_spawn(smol::unblock(move || persist(generation, kept)))
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::worktree::TeardownPolicy;

    fn worktree(repo: &Path, branch: &str) -> ManagedWorktree {
        ManagedWorktree {
            path: crate::workspace::worktree::create_branch_checkout(repo, branch, None)
                .expect("the worktree is created")
                .path,
            repo_root: repo.to_path_buf(),
            branch: branch.to_string(),
            teardown: TeardownPolicy::Auto,
        }
    }

    #[test]
    fn the_ledger_round_trips_and_drops_what_is_no_longer_a_paneflow_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _root = crate::workspace::worktree::test_support::scoped_root(tmp.path().join("wt"));
        let repo = tmp.path().join("repo");
        paneflow_git::fixture::committed_repo(&repo, &[("README.md", "init\n")]);
        let alive = worktree(&repo, "ledger/alive");
        let gone = worktree(&repo, "ledger/gone");
        let ledger = tmp.path().join("kept-worktrees.json");
        write_ledger(&ledger, &[alive.clone(), gone.clone()]).expect("the ledger is written");
        crate::workspace::worktree::remove_worktree(&repo, &gone.path)
            .expect("the worktree is removed");

        assert_eq!(read_ledger(&ledger), vec![alive]);

        write_ledger(&ledger, &[]).expect("an empty ledger is removed");
        assert!(!ledger.exists());
    }
}

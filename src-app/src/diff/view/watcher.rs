use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::Either;
use notify::event::ModifyKind;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::agent_sessions::SessionMeta;

use super::{DiffView, REFRESH_COOLDOWN, REFRESH_DEBOUNCE};

const WATCH_IGNORE_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    ".jj",
    ".hg",
    ".svn",
    "dist",
    "build",
    ".next",
    ".cache",
    ".venv",
    "venv",
    "vendor",
];

enum Revalidation {
    Unchanged,
    Changed,
    Attribution(Vec<SessionMeta>),
}

pub(super) fn event_relevant(res: &notify::Result<Event>) -> bool {
    let Ok(event) = res else {
        return false;
    };
    match event.kind {
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)) => return false,
        _ => {}
    }
    event.paths.iter().any(|path| !is_noise_path(path))
}

fn component_eq(component: &OsStr, expected: &str) -> bool {
    if cfg!(target_os = "windows") {
        component.to_string_lossy().eq_ignore_ascii_case(expected)
    } else {
        component == OsStr::new(expected)
    }
}

fn has_component(components: &[&OsStr], expected: &str) -> bool {
    components.iter().any(|part| component_eq(part, expected))
}

fn has_component_pair(components: &[&OsStr], first: &str, second: &str) -> bool {
    components
        .windows(2)
        .any(|pair| component_eq(pair[0], first) && component_eq(pair[1], second))
}

fn ignored_watch_dir(name: &OsStr) -> bool {
    WATCH_IGNORE_DIRS
        .iter()
        .any(|expected| component_eq(name, expected))
}

fn is_noise_path(path: &Path) -> bool {
    let components: Vec<&OsStr> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();

    has_component(&components, "target")
        || has_component(&components, "node_modules")
        || has_component_pair(&components, ".git", "objects")
        || has_component_pair(&components, ".git", "logs")
        || has_component_pair(&components, ".git", "index.lock")
        || ["FETCH_HEAD", "ORIG_HEAD", "COMMIT_EDITMSG", "MERGE_HEAD"]
            .iter()
            .any(|name| has_component_pair(&components, ".git", name))
        || path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(super::super::git::is_skipped_name)
}

pub(super) fn build(
    tx: mpsc::UnboundedSender<notify::Result<Event>>,
    worktree: PathBuf,
    repo_root: PathBuf,
) -> Option<RecommendedWatcher> {
    let mut watcher = match RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            let _ = tx.unbounded_send(res);
        },
        Config::default(),
    ) {
        Ok(watcher) => watcher,
        Err(e) => {
            log::warn!("diff watcher: failed to create: {e}");
            return None;
        }
    };

    let mut targets: Vec<(PathBuf, RecursiveMode)> = Vec::new();
    targets.push((worktree.clone(), RecursiveMode::NonRecursive));
    if let Ok(entries) = std::fs::read_dir(&worktree) {
        for entry in entries.flatten() {
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let path = entry.path();
            let ignored = path.file_name().is_some_and(ignored_watch_dir);
            if !ignored {
                targets.push((path, RecursiveMode::Recursive));
            }
        }
    }

    let git_common = repo_root.join(".git");
    if git_common.is_dir() {
        targets.push((
            git_common.join("refs").join("heads"),
            RecursiveMode::Recursive,
        ));
        targets.push((git_common.join("packed-refs"), RecursiveMode::NonRecursive));
        targets.push((git_common.join("HEAD"), RecursiveMode::NonRecursive));
    }

    let mut registered = 0usize;
    for (path, mode) in &targets {
        match watcher.watch(path, *mode) {
            Ok(()) => registered += 1,
            Err(e) => log::debug!("diff watcher: skip {}: {e}", path.display()),
        }
    }
    log::debug!(
        "diff: watcher registered {registered}/{} paths for {}",
        targets.len(),
        worktree.display()
    );
    Some(watcher)
}

impl DiffView {
    pub(super) fn start_watchers(&mut self, cx: &mut gpui::Context<Self>) {
        let worktree = self.column.path.clone();
        let repo_root = self.repo_root.clone();
        let epoch = self.watch_epoch;
        let (tx, mut rx) = mpsc::unbounded::<notify::Result<Event>>();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                log::debug!("diff: start_watchers building watcher off-thread");
                let watcher = smol::unblock(move || build(tx, worktree, repo_root)).await;
                let Some(watcher) = watcher else {
                    log::warn!("diff: watcher build returned None");
                    return;
                };
                let installed = cx.update(|cx| {
                    this.update(cx, |view: &mut Self, _| {
                        if view.watch_epoch != epoch {
                            return false;
                        }
                        view._watchers.push(watcher);
                        true
                    })
                    .unwrap_or(false)
                });
                if !installed {
                    log::debug!("diff: watcher build superseded (epoch advanced) - dropped");
                    return;
                }

                let mut relevant_events = 0u64;
                while let Some(result) = rx.next().await {
                    if !event_relevant(&result) {
                        continue;
                    }
                    relevant_events += 1;
                    if let Ok(event) = &result {
                        log::debug!(
                            "diff: watcher relevant event #{relevant_events} ({:?} {:?}) -> debounce",
                            event.kind,
                            event.paths.first()
                        );
                    }
                    let deadline = Instant::now() + REFRESH_DEBOUNCE;
                    loop {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        match futures::future::select(rx.next(), smol::Timer::after(remaining)).await
                        {
                            Either::Left((Some(_), _)) => {}
                            Either::Left((None, _)) => return,
                            Either::Right(_) => break,
                        }
                    }
                    let alive = cx.update(|cx| {
                        this.update(cx, |view: &mut Self, cx| {
                            if view.watch_epoch != epoch {
                                return false;
                            }
                            view.revalidate(cx);
                            true
                        })
                        .unwrap_or(false)
                    });
                    if !alive {
                        break;
                    }
                    let cooldown = Instant::now() + REFRESH_COOLDOWN;
                    loop {
                        let remaining = cooldown.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        match futures::future::select(rx.next(), smol::Timer::after(remaining)).await
                        {
                            Either::Left((Some(_), _)) => {}
                            Either::Left((None, _)) => return,
                            Either::Right(_) => break,
                        }
                    }
                }
            },
        )
        .detach();
    }

    pub fn suspend(&mut self, _cx: &mut gpui::Context<Self>) {
        if self.suspended {
            return;
        }
        self.suspended = true;
        self.watch_epoch = self.watch_epoch.wrapping_add(1);
        self._watchers.clear();
    }

    pub fn resume(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.suspended {
            return;
        }
        self.suspended = false;
        if !self.bootstrapped {
            return;
        }
        self.start_watchers(cx);
        if !self.base_ref.is_empty() {
            self.revalidate(cx);
        }
    }

    fn revalidate(&mut self, cx: &mut gpui::Context<Self>) {
        let base = self.base_ref.clone();
        let path = self.column.path.clone();
        let branch = self.column.branch.clone();
        let generation = self.column.generation;
        let stored = self.column.fingerprint.clone();
        cx.spawn(async move |this, cx| {
            let outcome = smol::unblock(move || {
                let fresh = super::super::git::column_fingerprint(&path, &base);
                if stored.as_ref() != Some(&fresh) {
                    return Revalidation::Changed;
                }
                let cwd = path.to_string_lossy();
                let sessions = crate::agent_sessions::attribution_for_column(&cwd, &branch);
                if sessions.is_empty() {
                    Revalidation::Unchanged
                } else {
                    Revalidation::Attribution(sessions)
                }
            })
            .await;
            if matches!(outcome, Revalidation::Unchanged) {
                return;
            }
            let _ = cx.update(|cx| {
                this.update(cx, |view: &mut Self, cx| {
                    if view.suspended {
                        return;
                    }
                    match outcome {
                        Revalidation::Changed => view.start_loading(cx),
                        Revalidation::Attribution(sessions) => {
                            if view.column.generation == generation {
                                view.column.attribution = sessions;
                                cx.notify();
                            }
                        }
                        Revalidation::Unchanged => {}
                    }
                })
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(path: PathBuf) -> notify::Result<Event> {
        Ok(Event {
            kind: EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            paths: vec![path],
            attrs: Default::default(),
        })
    }

    #[test]
    fn ignores_noise_directories_using_native_components() {
        assert!(!event_relevant(&event(
            ["repo", "target", "debug", "paneflow"].iter().collect()
        )));
        assert!(!event_relevant(&event(
            ["repo", "node_modules", "pkg", "index.js"].iter().collect()
        )));
        assert!(!event_relevant(&event(
            ["repo", ".git", "objects", "ab", "hash"].iter().collect()
        )));
    }

    #[test]
    fn ignores_git_transient_files_and_lockfiles() {
        assert!(!event_relevant(&event(
            ["repo", ".git", "FETCH_HEAD"].iter().collect()
        )));
        assert!(!event_relevant(&event(
            ["repo", "Cargo.lock"].iter().collect()
        )));
    }

    #[test]
    fn accepts_source_and_ref_changes() {
        assert!(event_relevant(&event(
            ["repo", "src", "main.rs"].iter().collect()
        )));
        assert!(event_relevant(&event(
            ["repo", ".git", "refs", "heads", "main"].iter().collect()
        )));
    }
}

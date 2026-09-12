use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Context};

use crate::PaneFlowApp;

pub(crate) const MAX_RECENT_WORKSPACES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RecentWorkspace {
    pub(crate) path: PathBuf,
    pub(crate) title: String,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct RecentsFile {
    #[serde(default)]
    workspaces: Vec<RecentWorkspace>,
}

fn recents_path() -> Option<PathBuf> {
    paneflow_home::recents_path()
}

fn title_for(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn read_from_disk(path: &Path) -> Vec<RecentWorkspace> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<RecentsFile>(&raw) else {
        log::warn!(
            "recents: {} is not valid JSON, starting empty",
            path.display()
        );
        return Vec::new();
    };
    parsed.workspaces
}

fn write_to_disk(path: &Path, workspaces: &[RecentWorkspace]) {
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!("recents: could not create {}: {error}", parent.display());
        return;
    }
    let file = RecentsFile {
        workspaces: workspaces.to_vec(),
    };
    match serde_json::to_string_pretty(&file) {
        Ok(json) => {
            if let Err(error) = std::fs::write(path, json) {
                log::warn!("recents: could not write {}: {error}", path.display());
            }
        }
        Err(error) => log::warn!("recents: could not serialize: {error}"),
    }
}

pub(crate) fn load_pruned() -> Vec<RecentWorkspace> {
    let Some(path) = recents_path() else {
        return Vec::new();
    };
    let stored = read_from_disk(&path);
    let mut kept = Vec::with_capacity(stored.len());
    for entry in stored {
        if kept.len() >= MAX_RECENT_WORKSPACES {
            break;
        }
        if !entry.path.is_dir() {
            continue;
        }
        if kept
            .iter()
            .any(|kept: &RecentWorkspace| kept.path == entry.path)
        {
            continue;
        }
        kept.push(entry);
    }
    kept
}

pub(crate) fn promote(workspaces: &mut Vec<RecentWorkspace>, paths: &[PathBuf]) -> bool {
    let before = workspaces.clone();
    for path in paths.iter().rev() {
        if path.as_os_str().is_empty() {
            continue;
        }
        let entry = RecentWorkspace {
            path: path.clone(),
            title: title_for(path),
        };
        workspaces.retain(|kept| kept.path != entry.path);
        workspaces.insert(0, entry);
    }
    workspaces.truncate(MAX_RECENT_WORKSPACES);
    *workspaces != before
}

pub(crate) fn restored_session_paths(
    workspaces: &[crate::workspace::Workspace],
    active_idx: usize,
) -> Vec<PathBuf> {
    let mut ordered: Vec<PathBuf> = Vec::with_capacity(workspaces.len());
    if let Some(active) = workspaces.get(active_idx).filter(|ws| !ws.cwd.is_empty()) {
        ordered.push(PathBuf::from(&active.cwd));
    }
    for (idx, ws) in workspaces.iter().enumerate() {
        if idx == active_idx || ws.cwd.is_empty() {
            continue;
        }
        ordered.push(PathBuf::from(&ws.cwd));
    }
    ordered
}

impl PaneFlowApp {
    pub(crate) fn record_recent_workspaces(&mut self, paths: &[PathBuf], cx: &App) {
        if paths.is_empty() || !promote(&mut self.recent_workspaces, paths) {
            return;
        }
        self.persist_recent_workspaces(cx);
    }

    pub(crate) fn forget_recent_workspace(&mut self, path: &Path, cx: &mut Context<Self>) {
        let before = self.recent_workspaces.len();
        self.recent_workspaces.retain(|entry| entry.path != path);
        if self.recent_workspaces.len() == before {
            return;
        }
        self.persist_recent_workspaces(cx);
        cx.notify();
    }

    fn persist_recent_workspaces(&self, cx: &App) {
        let Some(path) = recents_path() else {
            return;
        };
        let workspaces = self.recent_workspaces.clone();
        cx.background_spawn(async move {
            smol::unblock(move || write_to_disk(&path, &workspaces)).await;
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_moves_a_known_path_back_to_the_front() {
        let mut recents = vec![
            RecentWorkspace {
                path: PathBuf::from("/a"),
                title: "a".into(),
            },
            RecentWorkspace {
                path: PathBuf::from("/b"),
                title: "b".into(),
            },
        ];
        assert!(promote(&mut recents, &[PathBuf::from("/b")]));
        assert_eq!(recents[0].path, PathBuf::from("/b"));
        assert_eq!(recents.len(), 2);
    }

    #[test]
    fn promote_reports_no_change_when_the_front_entry_is_reopened() {
        let mut recents = vec![RecentWorkspace {
            path: PathBuf::from("/a"),
            title: "a".into(),
        }];
        assert!(!promote(&mut recents, &[PathBuf::from("/a")]));
    }

    #[test]
    fn promote_keeps_the_newest_entries_within_the_cap() {
        let mut recents = Vec::new();
        let paths: Vec<PathBuf> = (0..MAX_RECENT_WORKSPACES + 3)
            .map(|i| PathBuf::from(format!("/ws{i}")))
            .collect();
        for path in &paths {
            promote(&mut recents, std::slice::from_ref(path));
        }
        assert_eq!(recents.len(), MAX_RECENT_WORKSPACES);
        assert_eq!(recents[0].path, paths[paths.len() - 1]);
    }

    #[test]
    fn promote_orders_a_multi_folder_open_left_to_right() {
        let mut recents = Vec::new();
        promote(
            &mut recents,
            &[PathBuf::from("/first"), PathBuf::from("/second")],
        );
        assert_eq!(recents[0].path, PathBuf::from("/first"));
        assert_eq!(recents[1].path, PathBuf::from("/second"));
    }
}

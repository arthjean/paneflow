use crate::diff::{DiffScope, DiffView, DiffViewEvent, DiffWorktree, RepoGroup};
use crate::{OpenDiffView, PaneFlowApp};
use gpui::{
    AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div, px,
};
use paneflow_config::schema::AppMode;
use std::path::{Path, PathBuf};

const DIFF_VIEW_CACHE_CAP: usize = 6;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DiffViewKey {
    repo_root: PathBuf,
    scope: DiffScope,
    worktrees_hash: u64,
}

impl DiffViewKey {
    fn new(repo_root: &Path, scope: DiffScope, worktrees: &[DiffWorktree]) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        let mut paths: Vec<String> = worktrees
            .iter()
            .map(|w| w.path.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        paths.hash(&mut h);
        Self {
            repo_root: repo_root.to_path_buf(),
            scope,
            worktrees_hash: h.finish(),
        }
    }
}

fn filter_chosen(
    worktrees: Vec<DiffWorktree>,
    chosen: Option<&std::collections::HashSet<String>>,
) -> Vec<DiffWorktree> {
    match chosen {
        Some(set) if !set.is_empty() => worktrees
            .into_iter()
            .filter(|w| set.contains(&w.path.to_string_lossy().into_owned()))
            .collect(),
        _ => worktrees,
    }
}

fn multiproject_signature(groups: &[RepoGroup]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut entries: Vec<(String, Vec<String>)> = groups
        .iter()
        .map(|g| {
            let mut worktrees: Vec<String> = g
                .worktrees
                .iter()
                .map(|w| w.path.to_string_lossy().into_owned())
                .collect();
            worktrees.sort();
            (norm_path(&g.repo_root), worktrees)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut h);
    h.finish()
}

pub(crate) const DIFF_SIDEBAR_WIDTH: f32 = crate::SIDEBAR_WIDTH;

impl PaneFlowApp {
    pub(crate) fn handle_open_diff_view(
        &mut self,
        _: &OpenDiffView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            AppMode::Diff => self.enter_cli_mode(window, cx),
            AppMode::Cli => self.enter_diff_mode(cx),
        }
    }

    pub(crate) fn enter_diff_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = AppMode::Diff;
        self.rebuild_diff_view(cx);
        self.save_session(cx);
    }

    pub(crate) fn rebuild_diff_view(&mut self, cx: &mut Context<Self>) {
        self.park_displayed_diff(cx);
        self.prune_diff_cache();

        let repo_root = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.clone());

        match self.diff_mode.diff_scope {
            DiffScope::MultiProject => {
                self.diff_mode.diff_view_key = None;
                let groups = self.collect_multiproject_groups();
                if groups.is_empty() {
                    self.diff_mode.multi_diff_view_retained = None;
                    cx.notify();
                    return;
                }
                let sig = multiproject_signature(&groups);
                if let Some((retained_sig, view)) = self.diff_mode.multi_diff_view_retained.clone()
                    && retained_sig == sig
                {
                    view.update(cx, |v, cx| v.resume(cx));
                    self.diff_mode.multi_diff_view = Some(view);
                    cx.notify();
                    return;
                }
                let view = cx.new(|cx| crate::diff::MultiRepoDiffView::new(groups, cx));
                self.diff_mode.multi_diff_view_retained = Some((sig, view.clone()));
                self.diff_mode.multi_diff_view = Some(view);
            }
            DiffScope::Project => {
                let Some(root) = repo_root else {
                    self.diff_mode.diff_view_key = None;
                    cx.notify();
                    return;
                };
                let worktrees = self.collect_project_worktrees();
                let key = DiffViewKey::new(&root, DiffScope::Project, &worktrees);
                let (view, _miss) = self.mount_or_resume_diff(key, root, worktrees, cx);
                self.diff_mode.diff_view = Some(view);
            }
            DiffScope::Worktree => {
                let Some(root) = repo_root else {
                    self.diff_mode.diff_view_key = None;
                    cx.notify();
                    return;
                };
                let chosen = self.diff_mode.diff_chosen_worktrees.get(&root).cloned();
                let open = filter_chosen(self.collect_diff_worktrees(&root), chosen.as_ref());
                let key = DiffViewKey::new(&root, DiffScope::Worktree, &open);
                let (view, miss) = self.mount_or_resume_diff(key, root.clone(), open.clone(), cx);
                self.diff_mode.diff_view = Some(view);
                if self.diff_mode.diff_available_repo.as_deref() != Some(root.as_path()) {
                    self.refresh_diff_available_worktrees(root.clone(), cx);
                }
                if miss {
                    self.spawn_worktree_discovery(root, open, chosen, cx);
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn park_displayed_diff(&mut self, cx: &mut Context<Self>) {
        if let Some(dv) = self.diff_mode.diff_view.take() {
            dv.update(cx, |v, cx| v.suspend(cx));
        }
        if let Some(mv) = self.diff_mode.multi_diff_view.take() {
            mv.update(cx, |v, cx| v.suspend(cx));
        }
    }

    fn mount_or_resume_diff(
        &mut self,
        key: DiffViewKey,
        root: PathBuf,
        worktrees: Vec<DiffWorktree>,
        cx: &mut Context<Self>,
    ) -> (Entity<crate::diff::DiffView>, bool) {
        if let Some(view) = self.diff_mode.diff_view_cache.get(&key).cloned() {
            view.update(cx, |v, cx| v.resume(cx));
            self.diff_mode.diff_view_key = Some(key);
            return (view, false);
        }
        let view = cx.new(|cx| crate::diff::DiffView::new(root, worktrees, cx));
        if self.diff_mode.diff_scope == DiffScope::Worktree {
            view.update(cx, |v, _| v.set_close_removes(true));
            cx.subscribe(&view, Self::handle_diff_view_event).detach();
        }
        self.diff_mode
            .diff_view_cache
            .insert(key.clone(), view.clone());
        self.diff_mode.diff_view_key = Some(key.clone());
        self.evict_diff_cache_if_needed(&key);
        (view, true)
    }

    fn prune_diff_cache(&mut self) {
        let open: std::collections::HashSet<PathBuf> = self
            .workspaces
            .iter()
            .filter_map(|ws| ws.repo_root.clone())
            .collect();
        self.diff_mode
            .diff_view_cache
            .retain(|k, _| open.contains(&k.repo_root));
        if open.is_empty() {
            self.diff_mode.multi_diff_view_retained = None;
        }
    }

    fn evict_diff_cache_if_needed(&mut self, keep: &DiffViewKey) {
        if self.diff_mode.diff_view_cache.len() <= DIFF_VIEW_CACHE_CAP {
            return;
        }
        let victims: Vec<DiffViewKey> = self
            .diff_mode
            .diff_view_cache
            .keys()
            .filter(|k| *k != keep)
            .cloned()
            .collect();
        for k in victims {
            if self.diff_mode.diff_view_cache.len() <= DIFF_VIEW_CACHE_CAP {
                break;
            }
            self.diff_mode.diff_view_cache.remove(&k);
        }
    }

    pub(crate) fn invalidate_worktree_diff_cache(
        &mut self,
        repo_root: &Path,
        cx: &mut Context<Self>,
    ) {
        let stale: Vec<DiffViewKey> = self
            .diff_mode
            .diff_view_cache
            .keys()
            .filter(|key| key.scope == DiffScope::Worktree && key.repo_root == repo_root)
            .cloned()
            .collect();
        if stale.is_empty() {
            return;
        }
        let was_displayed = self
            .diff_mode
            .diff_view_key
            .as_ref()
            .is_some_and(|key| stale.contains(key));
        for key in stale {
            self.diff_mode.diff_view_cache.remove(&key);
        }
        if was_displayed && self.mode == AppMode::Diff {
            self.rebuild_diff_view(cx);
        }
    }

    fn spawn_worktree_discovery(
        &mut self,
        root: std::path::PathBuf,
        open: Vec<crate::diff::DiffWorktree>,
        chosen: Option<std::collections::HashSet<String>>,
        cx: &mut Context<Self>,
    ) {
        self.diff_mode.diff_discovering = true;
        self.diff_mode.diff_discovering_root = Some(root.clone());
        let requested_root = root.clone();
        cx.spawn(async move |this, cx| {
            let discovered =
                smol::unblock(move || crate::diff::list_repo_worktrees(&requested_root)).await;
            let mut seen: std::collections::HashSet<String> =
                open.iter().map(|w| norm_path(&w.path)).collect();
            let mut new_cols = Vec::new();
            for (path, branch) in discovered {
                if let Some(set) = &chosen
                    && !set.contains(&path.to_string_lossy().into_owned())
                {
                    continue;
                }
                if seen.insert(norm_path(&path)) {
                    new_cols.push(crate::diff::DiffWorktree {
                        path,
                        branch,
                        workspace_id: None,
                    });
                }
            }
            let _ = cx.update(|cx| {
                this.update(cx, |app, cx| {
                    let owns_discovery =
                        app.diff_mode.diff_discovering_root.as_deref() == Some(root.as_path());
                    let still_current_worktree_view = app.mode == AppMode::Diff
                        && app.diff_mode.diff_scope == crate::diff::DiffScope::Worktree
                        && app.diff_mode.diff_view_key.as_ref().is_some_and(|key| {
                            key.repo_root == root && key.scope == crate::diff::DiffScope::Worktree
                        });
                    if owns_discovery {
                        app.diff_mode.diff_discovering = false;
                        app.diff_mode.diff_discovering_root = None;
                    }
                    if !new_cols.is_empty()
                        && still_current_worktree_view
                        && owns_discovery
                        && let Some(dv) = app.diff_mode.diff_view.clone()
                    {
                        dv.update(cx, |v, cx| v.add_columns(new_cols, cx));
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(crate) fn refresh_diff_available_worktrees(
        &mut self,
        root: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.diff_mode.diff_available_repo = Some(root.clone());
        let requested_root = root.clone();
        cx.spawn(async move |this, cx| {
            let wts =
                smol::unblock(move || crate::diff::list_repo_worktrees(&requested_root)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |app, cx| {
                    if app.diff_mode.diff_available_repo.as_deref() != Some(root.as_path()) {
                        return;
                    }
                    app.diff_mode.diff_available_worktrees = wts
                        .into_iter()
                        .map(|(path, branch)| crate::diff::DiffWorktree {
                            path,
                            branch,
                            workspace_id: None,
                        })
                        .collect();
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(crate) fn toggle_chosen_worktree(
        &mut self,
        root: std::path::PathBuf,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let all: std::collections::HashSet<String> = self
            .diff_mode
            .diff_available_worktrees
            .iter()
            .map(|w| w.path.to_string_lossy().into_owned())
            .collect();
        let set = self
            .diff_mode
            .diff_chosen_worktrees
            .entry(root.clone())
            .or_insert(all);
        if !set.remove(&path) {
            set.insert(path);
        }
        let now_empty = set.is_empty();
        if now_empty {
            self.diff_mode.diff_chosen_worktrees.remove(&root);
        }
        self.rebuild_diff_view(cx);
    }

    pub(crate) fn diff_worktree_is_chosen(&self, root: &std::path::Path, path: &str) -> bool {
        match self.diff_mode.diff_chosen_worktrees.get(root) {
            Some(set) => set.contains(path),
            None => true,
        }
    }

    pub(crate) fn handle_diff_view_event(
        &mut self,
        _view: Entity<DiffView>,
        event: &DiffViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            DiffViewEvent::CloseColumn { path } => {
                self.deselect_diff_worktree(path.to_string_lossy().into_owned(), cx);
            }
        }
    }

    fn deselect_diff_worktree(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(root) = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.clone())
        else {
            return;
        };
        let shown: std::collections::HashSet<String> = self
            .diff_mode
            .diff_view
            .as_ref()
            .map(|v| {
                v.read(cx)
                    .column_paths()
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if shown.len() <= 1 {
            return;
        }
        let set = self
            .diff_mode
            .diff_chosen_worktrees
            .entry(root)
            .or_insert(shown);
        set.remove(&path);
        self.rebuild_diff_view(cx);
    }

    pub(crate) fn enter_cli_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == AppMode::Cli {
            return;
        }
        self.park_displayed_diff(cx);
        self.mode = AppMode::Cli;
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn render_diff_main(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::diff::DiffScope;
        let ui = crate::theme::ui_colors();
        let breadcrumb = self.render_scope_header(cx);
        let empty = |msg: &'static str, breadcrumb: AnyElement| {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(36.))
                        .px(px(10.))
                        .child(breadcrumb),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().text_color(ui.muted).text_size(px(13.)).child(msg)),
                )
                .into_any_element()
        };
        let body = match self.diff_mode.diff_scope {
            DiffScope::MultiProject => match self.diff_mode.multi_diff_view.clone() {
                Some(v) => {
                    v.update(cx, |mv, _| mv.scope_slot = Some(breadcrumb));
                    v.into_any_element()
                }
                None => empty("No open projects with a git repository", breadcrumb),
            },
            _ => match self.diff_mode.diff_view.clone() {
                Some(v) => {
                    v.update(cx, |dv, _| dv.scope_slot = Some(breadcrumb));
                    v.into_any_element()
                }
                None => empty("No git repository in the active workspace", breadcrumb),
            },
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

fn norm_path(p: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = resolved.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        s.to_lowercase()
    } else {
        s
    }
}

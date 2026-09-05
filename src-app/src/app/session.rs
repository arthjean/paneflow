use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity};
use paneflow_config::schema::{LayoutNode, TabTitleSource};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::launch_cwd;
use crate::layout::{LayoutTree, MAX_PANES};
use crate::limits::MAX_SESSION_SIZE_BYTES;
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::{MAX_TABS_PER_WORKSPACE, MAX_WORKSPACES, Tab, Workspace, next_workspace_id};

const MAX_CORRUPTION_BACKUPS: usize = 5;

const SAVE_DEBOUNCE_MS: u64 = 150;

static SESSION_WRITE_LOCK: Mutex<()> = Mutex::new(());
static SESSION_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static SESSION_CORRUPTION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn session_write_guard() -> MutexGuard<'static, ()> {
    SESSION_WRITE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone)]
pub(crate) struct SessionCorruptionInfo {
    pub(crate) error_category: &'static str,
    pub(crate) file_size: u64,
    pub(crate) file_age_seconds: Option<u64>,
    pub(crate) backup_path: Option<PathBuf>,
}

impl PaneFlowApp {
    fn build_session_state(&self, cx: &App) -> paneflow_config::schema::SessionState {
        paneflow_config::schema::SessionState {
            version: paneflow_config::schema::SESSION_SCHEMA_VERSION,
            active_workspace: self.active_idx,
            workspaces: self
                .workspaces
                .iter()
                .map(|ws| paneflow_config::schema::WorkspaceSession {
                    title: ws.title.clone(),
                    cwd: ws.cwd.clone(),
                    tabs: ws.serialize_tabs_without_scrollback(cx),
                    active_tab: ws.active_tab_idx(),
                    legacy_layout: None,
                    legacy_empty: false,
                    custom_buttons: ws.custom_buttons.clone(),
                    expanded_paths: persisted_expanded_paths(&ws.cwd, &ws.files_expanded),
                    managed_worktrees: ws
                        .managed_worktrees
                        .iter()
                        .map(|wt| paneflow_config::schema::ManagedWorktreeDef {
                            path: wt.path.to_string_lossy().into_owned(),
                            repo_root: wt.repo_root.to_string_lossy().into_owned(),
                            branch: wt.branch.clone(),
                            teardown: wt.teardown.as_str().to_string(),
                        })
                        .collect(),
                    sidebar_collapsed: !ws.sidebar_expanded,
                })
                .collect(),
            mode: self.mode,
            review_layout: self.serialize_review_layout(cx),
            review_collapsed: self.serialize_review_collapsed(),
        }
    }

    pub(crate) fn save_session(&self, cx: &App) {
        let state = self.build_session_state(cx);
        let Some(path) = paneflow_config::loader::session_path() else {
            return;
        };

        let seq = self
            .save_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let save_seq = std::sync::Arc::clone(&self.save_seq);

        cx.background_spawn(async move {
            smol::Timer::after(std::time::Duration::from_millis(SAVE_DEBOUNCE_MS)).await;
            if save_seq.load(std::sync::atomic::Ordering::SeqCst) != seq {
                return;
            }
            smol::unblock(move || {
                write_session_json_if_current(&path, &state, &save_seq, seq);
            })
            .await;
        })
        .detach();
    }

    pub(crate) fn save_session_blocking(&self, cx: &App) {
        crate::window_state::save();
        self.save_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let state = self.build_session_state(cx);
        let Some(path) = paneflow_config::loader::session_path() else {
            return;
        };
        write_session_json(&path, &state);
    }

    pub(crate) fn load_session() -> (
        Option<paneflow_config::schema::SessionState>,
        Option<SessionCorruptionInfo>,
    ) {
        let Some(path) = paneflow_config::loader::session_path() else {
            return (None, None);
        };
        Self::load_session_at(&path)
    }

    pub(crate) fn load_session_at(
        path: &Path,
    ) -> (
        Option<paneflow_config::schema::SessionState>,
        Option<SessionCorruptionInfo>,
    ) {
        let bytes = match read_session_capped(path) {
            Ok(SessionRead::Data(d)) => d,
            Ok(SessionRead::Missing) => return (None, None),
            Ok(SessionRead::Rejected(category)) => {
                return (None, Some(session_corruption_info(path, category, None)));
            }
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("session load: read failed at {}: {e}", path.display());
                }
                return (None, Some(session_corruption_info(path, "io", None)));
            }
        };

        let data = match String::from_utf8(bytes) {
            Ok(data) => data,
            Err(err) => {
                log::warn!(
                    "session load: invalid UTF-8 at {}; falling back to empty session",
                    path.display()
                );
                let bytes = err.into_bytes();
                let backup_path = write_corruption_backup(path, &bytes).unwrap_or_else(|e| {
                    log::warn!(
                        "session load: backup write failed at {}: {e}",
                        path.display()
                    );
                    None
                });

                return (
                    None,
                    Some(session_corruption_info(path, "data", backup_path)),
                );
            }
        };

        match serde_json::from_str::<paneflow_config::schema::SessionState>(&data) {
            Ok(state) if state.version == paneflow_config::schema::SESSION_SCHEMA_VERSION => {
                (Some(state), None)
            }
            Ok(mut state)
                if state.version == paneflow_config::schema::SESSION_SCHEMA_VERSION_V1 =>
            {
                log::info!(
                    "session load: migrating schema v{} to v{} at {}",
                    paneflow_config::schema::SESSION_SCHEMA_VERSION_V1,
                    paneflow_config::schema::SESSION_SCHEMA_VERSION,
                    path.display()
                );
                paneflow_config::schema::migrate_session_v1(&mut state);
                (Some(state), None)
            }
            Ok(state) => {
                log::warn!(
                    "session load: unsupported schema version {} at {} (expected {}); falling back to empty session",
                    state.version,
                    path.display(),
                    paneflow_config::schema::SESSION_SCHEMA_VERSION
                );
                let backup_path =
                    write_corruption_backup(path, data.as_bytes()).unwrap_or_else(|e| {
                        log::warn!(
                            "session load: backup write failed at {}: {e}",
                            path.display()
                        );
                        None
                    });
                (
                    None,
                    Some(session_corruption_info(
                        path,
                        "unsupported_version",
                        backup_path,
                    )),
                )
            }
            Err(parse_err) => {
                log::warn!(
                    "session load: parse failed at {} ({}); falling back to empty session",
                    path.display(),
                    parse_err
                );

                let backup_path =
                    write_corruption_backup(path, data.as_bytes()).unwrap_or_else(|e| {
                        log::warn!(
                            "session load: backup write failed at {}: {e}",
                            path.display()
                        );
                        None
                    });

                (
                    None,
                    Some(session_corruption_info(
                        path,
                        serde_category_tag(&parse_err),
                        backup_path,
                    )),
                )
            }
        }
    }

    pub(crate) fn restore_workspaces(
        session: &paneflow_config::schema::SessionState,
        cx: &mut Context<Self>,
    ) -> (Vec<Workspace>, usize) {
        let mut workspaces = Vec::new();

        if session.workspaces.len() > MAX_WORKSPACES {
            log::warn!(
                "session restore: {} workspaces exceeds MAX_WORKSPACES ({MAX_WORKSPACES}); restoring the first {MAX_WORKSPACES}",
                session.workspaces.len()
            );
        }
        for ws_session in session.workspaces.iter().take(MAX_WORKSPACES) {
            let mut cwd = restored_workspace_cwd(&ws_session.cwd);
            let mut title = ws_session.title.clone();
            if should_repair_restored_root_terminal(&title, &cwd) {
                let repaired_cwd = launch_cwd::implicit_launch_cwd();
                log::info!(
                    "session restore: repairing legacy default workspace at filesystem root"
                );
                title = launch_cwd::title_for_cwd_or(&repaired_cwd, title);
                cwd = repaired_cwd;
            }
            let ws_id = next_workspace_id();

            if ws_session.tabs.len() > MAX_TABS_PER_WORKSPACE {
                log::warn!(
                    "session restore: workspace \"{title}\" holds {} tabs, restoring the first {MAX_TABS_PER_WORKSPACE}",
                    ws_session.tabs.len()
                );
            }
            let mut tabs = Vec::new();
            for tab_session in ws_session.tabs.iter().take(MAX_TABS_PER_WORKSPACE) {
                let restored_layout = tab_session
                    .layout
                    .clone()
                    .map(without_persisted_scrollback)
                    .map(canonicalize_persisted_layout);
                let root = restored_layout.map(|layout| {
                    let mut pane_deque: VecDeque<Entity<Pane>> = VecDeque::new();
                    let ws_cwd = cwd.clone();
                    LayoutTree::from_layout_node(&layout, &mut pane_deque, &mut |node| {
                        let surfaces = match node {
                            LayoutNode::Pane { surfaces } => surfaces.as_slice(),
                            _ => &[],
                        };
                        Self::spawn_pane_from_surfaces(ws_id, surfaces, &ws_cwd, cx)
                    })
                });
                tabs.push(Tab::restored(
                    tab_session.title.clone(),
                    restored_tab_title_source(tab_session, &ws_session.custom_buttons),
                    root,
                    restored_tab_worktree(tab_session.worktree.as_deref()),
                ));
            }
            let mut workspace =
                Workspace::restored_with_id(ws_id, title.clone(), cwd, tabs, ws_session.active_tab);

            workspace.custom_buttons = ws_session.custom_buttons.clone();
            workspace.sidebar_expanded = !ws_session.sidebar_collapsed;
            workspace.managed_worktrees = ws_session
                .managed_worktrees
                .iter()
                .filter_map(rehydrate_managed_worktree)
                .collect();
            workspace.files_expanded = ws_session
                .expanded_paths
                .iter()
                .filter_map(|rel| rehydrate_expanded_path(&workspace.cwd, rel))
                .collect();
            Self::spawn_initial_git_stats(ws_id, workspace.cwd.clone(), cx);
            workspaces.push(workspace);
        }

        let mut prune_roots: Vec<std::path::PathBuf> = workspaces
            .iter()
            .flat_map(|ws| ws.managed_worktrees.iter().map(|wt| wt.repo_root.clone()))
            .collect();
        prune_roots.sort();
        prune_roots.dedup();
        if !prune_roots.is_empty() {
            cx.spawn(async move |_this, _cx: &mut gpui::AsyncApp| {
                smol::unblock(move || {
                    for root in prune_roots {
                        if let Err(e) = crate::workspace::worktree::prune(&root) {
                            log::debug!("worktree prune skipped for {}: {e}", root.display());
                        }
                    }
                })
                .await;
            })
            .detach();
        }

        let active_idx = session
            .active_workspace
            .min(workspaces.len().saturating_sub(1));
        (workspaces, active_idx)
    }

    fn build_restored_surface(
        workspace_id: u64,
        surface: &paneflow_config::schema::SurfaceDefinition,
        fallback_cwd: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Option<crate::pane::PaneSurface> {
        use std::path::PathBuf;

        if surface.surface_type.as_deref() == Some("markdown") {
            let path = surface.path.as_ref().map(PathBuf::from)?;
            let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
                crate::markdown::MarkdownView::open(path, cx)
            });
            return Some(crate::pane::PaneSurface::Markdown(markdown));
        }

        let cwd = resolved_surface_cwd(surface.cwd.as_deref(), fallback_cwd);

        let surface_env = surface.env.clone();
        let t = cx.new(|cx| {
            TerminalView::with_cwd_and_env(workspace_id, Some(cwd), None, surface_env, cx)
        });
        if let Some(ref scrollback) = surface.scrollback {
            t.read(cx).restore_scrollback(scrollback);
        }
        if let Some(ref custom) = surface.custom_name {
            t.update(cx, |view, _cx| {
                view.terminal.custom_name = Some(custom.clone());
            });
        }
        if let Some(agent) = surface
            .agent
            .as_deref()
            .and_then(crate::agent_launcher::TerminalAgent::from_tag)
        {
            t.update(cx, |view, _cx| {
                view.terminal.detected_agent = Some(agent);
                view.terminal.agent_confirmed = false;
            });
        }
        if let Some(size) = surface
            .font_size
            .and_then(crate::terminal::element::sanitize_font_override)
        {
            t.update(cx, |view, _cx| {
                view.terminal.font_size_override = Some(size);
            });
        }
        cx.subscribe(&t, Self::handle_terminal_event).detach();
        Some(crate::pane::PaneSurface::Terminal(t))
    }

    pub(crate) fn spawn_pane_from_surfaces(
        workspace_id: u64,
        surfaces: &[paneflow_config::schema::SurfaceDefinition],
        fallback_cwd: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        let mut built: Option<crate::pane::PaneSurface> = None;
        for i in restore_candidate_order(surfaces) {
            built = Self::build_restored_surface(workspace_id, &surfaces[i], fallback_cwd, cx);
            if built.is_some() {
                break;
            }
        }

        let Some(surface) = built else {
            if !surfaces.is_empty() {
                log::error!(
                    "spawn_pane_from_surfaces: no restorable surface built; using fallback"
                );
            }
            let t = cx.new(|cx| {
                TerminalView::with_cwd(workspace_id, Some(fallback_cwd.to_path_buf()), None, cx)
            });
            cx.subscribe(&t, Self::handle_terminal_event).detach();
            let pane = cx.new(|cx| Pane::new(t, workspace_id, cx));
            cx.subscribe(&pane, Self::handle_pane_event).detach();
            return pane;
        };
        let pane = cx.new(|cx| Pane::new_with_surface(surface, workspace_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        pane
    }
}

fn restore_candidate_order(surfaces: &[paneflow_config::schema::SurfaceDefinition]) -> Vec<usize> {
    if surfaces.is_empty() {
        return Vec::new();
    }
    let focused = surfaces
        .iter()
        .position(|s| s.focus == Some(true))
        .unwrap_or(0);
    std::iter::once(focused)
        .chain((0..surfaces.len()).filter(|&i| i != focused))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum SessionRead {
    Data(Vec<u8>),
    Missing,
    Rejected(&'static str),
}

fn read_session_capped(path: &Path) -> std::io::Result<SessionRead> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SessionRead::Missing),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        log::warn!(
            "session load: {} is not a regular file; starting empty",
            path.display()
        );
        return Ok(SessionRead::Rejected("non_regular"));
    }
    if meta.len() > MAX_SESSION_SIZE_BYTES {
        log::warn!(
            "session load: {} is {} bytes (> {MAX_SESSION_SIZE_BYTES} cap); starting empty",
            path.display(),
            meta.len()
        );
        return Ok(SessionRead::Rejected("oversize"));
    }
    let mut data = Vec::new();
    file.take(MAX_SESSION_SIZE_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > MAX_SESSION_SIZE_BYTES {
        log::warn!(
            "session load: {} exceeded the {MAX_SESSION_SIZE_BYTES} cap during read; starting empty",
            path.display()
        );
        return Ok(SessionRead::Rejected("oversize"));
    }
    Ok(SessionRead::Data(data))
}

fn session_corruption_info(
    path: &Path,
    error_category: &'static str,
    backup_path: Option<PathBuf>,
) -> SessionCorruptionInfo {
    let metadata = std::fs::metadata(path).ok();
    let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
    let file_age_seconds = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|mt| SystemTime::now().duration_since(mt).ok())
        .map(|d| d.as_secs());

    SessionCorruptionInfo {
        error_category,
        file_size,
        file_age_seconds,
        backup_path,
    }
}

fn restored_workspace_cwd(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_dir() {
        return path;
    }
    let fallback = launch_cwd::implicit_launch_cwd();
    log::warn!(
        "session restore: workspace cwd {} is not a directory; falling back to {}",
        path.display(),
        fallback.display()
    );
    fallback
}

fn resolved_surface_cwd(raw: Option<&str>, fallback_cwd: &Path) -> PathBuf {
    let Some(raw) = raw else {
        return fallback_cwd.to_path_buf();
    };
    let path = PathBuf::from(raw);
    if path.is_dir() {
        return path;
    }
    log::warn!(
        "session restore: surface cwd {} is not a directory; falling back to {}",
        path.display(),
        fallback_cwd.display()
    );
    fallback_cwd.to_path_buf()
}

fn without_persisted_scrollback(mut layout: LayoutNode) -> LayoutNode {
    fn clear(node: &mut LayoutNode) {
        match node {
            LayoutNode::Pane { surfaces } => {
                for surface in surfaces {
                    surface.scrollback = None;
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    clear(child);
                }
            }
        }
    }

    clear(&mut layout);
    layout
}

fn canonicalize_persisted_layout(mut layout: LayoutNode) -> LayoutNode {
    paneflow_config::schema::validate_layout(&mut layout);
    debug_assert!(layout.leaf_count() <= MAX_PANES);
    layout
}

fn restored_tab_title_source(
    tab: &paneflow_config::schema::TabSession,
    custom_buttons: &[paneflow_config::schema::ButtonCommand],
) -> TabTitleSource {
    if let Some(stated) = tab.title_source {
        return stated;
    }
    if is_app_written_tab_title(&tab.title)
        || custom_buttons
            .iter()
            .any(|button| button.name.trim() == tab.title.trim())
    {
        return TabTitleSource::Preset;
    }
    TabTitleSource::User
}

fn is_app_written_tab_title(title: &str) -> bool {
    let title = title.trim();
    title.is_empty()
        || title == crate::app::pane_palette::PALETTE_TAB_TITLE
        || title == SHELL_PRESET_LABEL
        || TerminalAgent::ALL
            .iter()
            .any(|agent| agent.display_name() == title)
}

const SHELL_PRESET_LABEL: &str = "Terminal";

fn should_repair_restored_root_terminal(title: &str, cwd: &Path) -> bool {
    is_numbered_terminal_title(title) && launch_cwd::is_filesystem_root(cwd)
}

fn is_numbered_terminal_title(title: &str) -> bool {
    let Some(number) = title.strip_prefix("Terminal ") else {
        return false;
    };
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
}

fn rehydrate_expanded_path(cwd: &str, rel: &str) -> Option<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        log::warn!(
            "session restore: dropping expanded_path with traversal/absolute component: {rel:?}"
        );
        return None;
    }
    let base = PathBuf::from(cwd);
    let abs = base.join(rel_path);
    if !abs.starts_with(&base) {
        log::warn!("session restore: dropping expanded_path escaping workspace root: {rel:?}");
        return None;
    }
    Some(abs)
}

fn rehydrate_managed_worktree(
    def: &paneflow_config::schema::ManagedWorktreeDef,
) -> Option<crate::workspace::worktree::ManagedWorktree> {
    crate::workspace::worktree::managed_worktree_from_record(
        &def.path,
        &def.repo_root,
        &def.branch,
        &def.teardown,
    )
}

fn restored_tab_worktree(path: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(path.filter(|p| !p.is_empty())?);
    path.is_dir().then_some(path)
}

fn persisted_expanded_paths(cwd: &str, expanded: &[PathBuf]) -> Vec<String> {
    let mut paths: Vec<String> = expanded
        .iter()
        .filter_map(|p| p.strip_prefix(cwd).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

fn write_session_json(path: &Path, state: &paneflow_config::schema::SessionState) {
    let _guard = session_write_guard();
    write_session_json_inner(path, state);
}

fn write_session_json_if_current(
    path: &Path,
    state: &paneflow_config::schema::SessionState,
    save_seq: &AtomicU64,
    seq: u64,
) {
    let _guard = session_write_guard();
    if save_seq.load(Ordering::SeqCst) != seq {
        return;
    }
    write_session_json_inner(path, state);
}

fn write_session_json_inner(path: &Path, state: &paneflow_config::schema::SessionState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            let tmp_path = session_tmp_path(path);
            match std::fs::write(&tmp_path, &json) {
                Ok(()) => {
                    if let Err(e) = std::fs::rename(&tmp_path, path) {
                        log::warn!("session save rename failed: {e}");
                        let _ = std::fs::remove_file(&tmp_path);
                    }
                }
                Err(e) => {
                    log::warn!("session save failed: {e}");
                    let _ = std::fs::remove_file(&tmp_path);
                }
            }
        }
        Err(e) => log::warn!("session serialize failed: {e}"),
    }
}

fn session_tmp_path(path: &Path) -> PathBuf {
    let seq = SESSION_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let Some(parent) = path.parent() else {
        return path.with_extension(format!("json.tmp.{}.{}", std::process::id(), seq));
    };
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session.json");
    parent.join(format!(".{file_name}.tmp.{}.{}", std::process::id(), seq))
}

fn serde_category_tag(err: &serde_json::Error) -> &'static str {
    match err.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
}

fn write_corruption_backup(
    session_path: &Path,
    contents: &[u8],
) -> std::io::Result<Option<PathBuf>> {
    let parent = match session_path.parent() {
        Some(p) => p,
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session path has no parent",
            ));
        }
    };
    std::fs::create_dir_all(parent)?;

    let ts = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos(),
        Err(_) => return Ok(None),
    };
    let seq = SESSION_CORRUPTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = session_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session.json");
    let backup = parent.join(format!(
        "{stem}.corrupted-{ts}-{}-{seq}",
        std::process::id()
    ));
    std::fs::write(&backup, contents)?;

    rotate_corruption_backups(parent, stem);
    Ok(Some(backup))
}

fn rotate_corruption_backups(dir: &Path, stem: &str) {
    let prefix = format!("{stem}.corrupted-");
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(it) => it
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
            })
            .collect(),
        Err(_) => return,
    };

    if backups.len() <= MAX_CORRUPTION_BACKUPS {
        return;
    }

    backups.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(&prefix))
            .and_then(corruption_backup_timestamp)
            .unwrap_or(u128::MAX)
    });

    let drop_count = backups.len() - MAX_CORRUPTION_BACKUPS;
    for old in backups.into_iter().take(drop_count) {
        if let Err(e) = std::fs::remove_file(&old) {
            log::warn!(
                "session backup rotation: could not remove {}: {e}",
                old.display()
            );
        }
    }
}

fn corruption_backup_timestamp(suffix: &str) -> Option<u128> {
    suffix.split('-').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform_root() -> PathBuf {
        std::env::current_dir()
            .ok()
            .and_then(|path| path.ancestors().last().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string()))
    }

    fn legacy_tab(title: &str) -> paneflow_config::schema::TabSession {
        serde_json::from_value(serde_json::json!({ "title": title }))
            .expect("a title-only tab is a valid pre-auto-naming snapshot")
    }

    #[test]
    fn a_legacy_preset_label_is_handed_back_to_auto_naming() {
        for title in [
            "",
            "   ",
            "Claude Code",
            "OpenCode",
            "Codex",
            "Terminal",
            "New pane",
            "Qoder",
        ] {
            assert_eq!(
                restored_tab_title_source(&legacy_tab(title), &[]),
                TabTitleSource::Preset,
                "{title:?} is a label Paneflow writes, not a name a human chose"
            );
        }
    }

    #[test]
    fn a_legacy_title_that_is_not_app_written_stays_user_owned() {
        for title in ["sprint 3", "fix the flaky test", "claude code review"] {
            assert_eq!(
                restored_tab_title_source(&legacy_tab(title), &[]),
                TabTitleSource::User,
                "{title:?} looks like a name someone typed"
            );
        }
    }

    #[test]
    fn a_legacy_custom_button_label_is_handed_back_to_auto_naming() {
        let buttons = vec![paneflow_config::schema::ButtonCommand {
            id: "b1".to_string(),
            name: "Run dev server".to_string(),
            icon: "icons/rocket.svg".to_string(),
            command: "bun dev".to_string(),
        }];
        assert_eq!(
            restored_tab_title_source(&legacy_tab("Run dev server"), &buttons),
            TabTitleSource::Preset
        );
        assert_eq!(
            restored_tab_title_source(&legacy_tab("Run dev server"), &[]),
            TabTitleSource::User,
            "the button belongs to its own workspace, not to every workspace"
        );
    }

    #[test]
    fn an_explicit_provenance_is_taken_at_face_value() {
        let user_named_after_a_preset: paneflow_config::schema::TabSession =
            serde_json::from_value(serde_json::json!({
                "title": "Claude Code",
                "title_source": "user",
            }))
            .expect("valid tab snapshot");
        assert_eq!(
            restored_tab_title_source(&user_named_after_a_preset, &[]),
            TabTitleSource::User,
            "someone who types \"Claude Code\" over an auto name meant it"
        );

        let auto: paneflow_config::schema::TabSession = serde_json::from_value(
            serde_json::json!({ "title": "sprint 3", "title_source": "preset" }),
        )
        .expect("valid tab snapshot");
        assert_eq!(
            restored_tab_title_source(&auto, &[]),
            TabTitleSource::Preset
        );
    }

    #[test]
    fn restored_root_terminal_repair_only_targets_numbered_default_titles() {
        let root = platform_root();
        assert!(should_repair_restored_root_terminal("Terminal 1", &root));
        assert!(should_repair_restored_root_terminal("Terminal 12", &root));
        assert!(!should_repair_restored_root_terminal("Terminal", &root));
        assert!(!should_repair_restored_root_terminal("Root shell", &root));
    }

    #[test]
    fn restored_root_terminal_repair_ignores_non_root_cwd() {
        let mut cwd = platform_root();
        cwd.push("project");

        assert!(!should_repair_restored_root_terminal("Terminal 1", &cwd));
    }

    #[test]
    fn rehydrate_expanded_path_keeps_inside_root_and_drops_escapes() {
        assert_eq!(
            rehydrate_expanded_path("/home/u/proj", "src/app"),
            Some(PathBuf::from("/home/u/proj/src/app"))
        );
        assert_eq!(rehydrate_expanded_path("/home/u/proj", "../../etc"), None);
        assert_eq!(rehydrate_expanded_path("/home/u/proj", "/etc/passwd"), None);
        assert_eq!(rehydrate_expanded_path("/home/u/proj", "a/../../b"), None);
    }

    #[test]
    fn persisted_expanded_paths_are_workspace_relative_and_sorted() {
        let root = PathBuf::from("project");
        let cwd = root.to_string_lossy().into_owned();
        let paths = vec![
            root.join("src").join("z"),
            PathBuf::from("outside"),
            root.join("src").join("a"),
        ];

        let expected_a = PathBuf::from("src")
            .join("a")
            .to_string_lossy()
            .into_owned();
        let expected_z = PathBuf::from("src")
            .join("z")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            persisted_expanded_paths(&cwd, &paths),
            vec![expected_a, expected_z]
        );
    }

    #[test]
    fn session_restore_discards_legacy_scrollback_recursively() {
        let surface = |scrollback: &str| paneflow_config::schema::SurfaceDefinition {
            scrollback: Some(scrollback.to_string()),
            ..Default::default()
        };
        let layout = LayoutNode::Split {
            direction: "horizontal".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![surface("old pane")],
                },
                LayoutNode::Split {
                    direction: "vertical".to_string(),
                    ratio: None,
                    ratios: None,
                    children: vec![LayoutNode::Pane {
                        surfaces: vec![surface("old nested pane")],
                    }],
                },
            ],
        };

        let cleaned = without_persisted_scrollback(layout);
        let mut pending = vec![&cleaned];
        while let Some(node) = pending.pop() {
            match node {
                LayoutNode::Pane { surfaces } => {
                    assert!(surfaces.iter().all(|surface| surface.scrollback.is_none()));
                }
                LayoutNode::Split { children, .. } => pending.extend(children),
            }
        }
    }

    #[test]
    fn canonicalize_persisted_layout_sanitizes_deep_layout() {
        use paneflow_config::schema::LayoutNode;
        let small = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
        let sanitized_small = canonicalize_persisted_layout(small.clone());
        assert_eq!(sanitized_small, small);

        let mut deep = LayoutNode::Pane {
            surfaces: vec![Default::default()],
        };
        for _ in 0..60 {
            deep = LayoutNode::Split {
                direction: "vertical".to_string(),
                ratio: None,
                ratios: None,
                children: vec![
                    deep,
                    LayoutNode::Pane {
                        surfaces: vec![Default::default()],
                    },
                ],
            };
        }
        let sanitized = canonicalize_persisted_layout(deep);
        assert!(sanitized.leaf_count() <= MAX_PANES);
    }

    #[test]
    fn read_session_capped_reads_small_file_and_rejects_non_regular() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("session.json");
        std::fs::write(&path, "{\"ok\":true}").expect("seed");
        assert_eq!(
            read_session_capped(&path).expect("io ok"),
            SessionRead::Data(b"{\"ok\":true}".to_vec())
        );
        assert!(matches!(
            read_session_capped(tmp.path()),
            Ok(SessionRead::Rejected("non_regular")) | Err(_)
        ));
    }

    #[test]
    fn oversized_session_returns_corruption_info_without_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        let file = std::fs::File::create(&session_path).expect("seed");
        file.set_len(MAX_SESSION_SIZE_BYTES + 1)
            .expect("sparse oversize file");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);

        assert!(state.is_none());
        let info = info.expect("oversize rejection emits diagnostics");
        assert_eq!(info.error_category, "oversize");
        assert_eq!(info.file_size, MAX_SESSION_SIZE_BYTES + 1);
        assert!(
            info.backup_path.is_none(),
            "do not copy huge rejected files"
        );
    }

    #[test]
    fn unsupported_session_version_returns_corruption_info_and_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        let contents = r#"{
            "version": 999,
            "active_workspace": 0,
            "workspaces": []
        }"#;
        std::fs::write(&session_path, contents).expect("seed unsupported session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);

        assert!(state.is_none());
        let info = info.expect("unsupported version emits diagnostics");
        assert_eq!(info.error_category, "unsupported_version");
        let backup = info.backup_path.expect("backup path populated");
        assert_eq!(
            std::fs::read_to_string(backup).expect("backup readable"),
            contents
        );
    }

    #[test]
    fn v1_session_is_migrated_not_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        let contents = r#"{
            "version": 1,
            "active_workspace": 0,
            "workspaces": [
                {
                    "title": "paneflow",
                    "cwd": "/tmp",
                    "layout": {
                        "type": "pane",
                        "surfaces": [
                            { "surface_type": "terminal", "name": "zsh" },
                            { "surface_type": "terminal", "name": "claude", "focus": true }
                        ]
                    }
                }
            ]
        }"#;
        std::fs::write(&session_path, contents).expect("seed v1 session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);

        assert!(info.is_none(), "a v1 file is not corruption");
        let state = state.expect("v1 session restores");
        assert_eq!(
            state.version,
            paneflow_config::schema::SESSION_SCHEMA_VERSION
        );
        let ws = &state.workspaces[0];
        assert_eq!(ws.tabs.len(), 2, "the stacked surface becomes a second tab");
        assert!(ws.legacy_layout.is_none(), "the v1 key is drained");
        assert_eq!(
            ws.tabs[1].title, "zsh",
            "promoted tab keeps the surface name"
        );
        assert!(
            !session_path.with_extension("json.corrupted").exists(),
            "no corruption backup for a supported version"
        );
    }

    #[test]
    fn corruption_backup_names_do_not_collide_within_same_second() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");

        let first = write_corruption_backup(&session_path, b"first")
            .expect("first backup")
            .expect("first path");
        let second = write_corruption_backup(&session_path, b"second")
            .expect("second backup")
            .expect("second path");

        assert_ne!(first, second, "backups must not overwrite each other");
        assert_eq!(std::fs::read(&first).expect("first readable"), b"first");
        assert_eq!(std::fs::read(&second).expect("second readable"), b"second");
    }

    #[test]
    fn restored_cwd_helpers_fall_back_for_missing_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let valid = tmp.path().to_path_buf();
        let missing = tmp.path().join("missing");
        let valid_str = valid.to_string_lossy().into_owned();
        let missing_str = missing.to_string_lossy().into_owned();

        assert_eq!(
            restored_workspace_cwd(&valid_str),
            valid,
            "existing workspace cwd is preserved"
        );

        let workspace_fallback = restored_workspace_cwd(&missing_str);
        assert!(
            workspace_fallback.is_dir(),
            "missing workspace cwd falls back to a live directory"
        );
        assert_ne!(workspace_fallback, missing);

        let surface_fallback = tmp.path().join("fallback");
        std::fs::create_dir_all(&surface_fallback).expect("fallback dir");
        assert_eq!(
            resolved_surface_cwd(Some(&missing_str), &surface_fallback),
            surface_fallback.clone(),
            "missing surface cwd falls back to workspace cwd"
        );
        assert_eq!(
            resolved_surface_cwd(None, &surface_fallback),
            surface_fallback,
            "absent surface cwd falls back to workspace cwd"
        );
    }

    #[test]
    fn malformed_json_returns_corruption_info_and_writes_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        std::fs::write(&session_path, "{").expect("seed broken session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);
        assert!(state.is_none(), "fallback to empty session expected");

        let info = info.expect("corruption info expected");
        assert_eq!(info.error_category, "eof", "trailing brace = EOF bucket");
        assert_eq!(info.file_size, 1, "single byte file");
        let backup = info.backup_path.expect("backup path populated");
        assert!(backup.exists(), "backup file actually on disk");
        assert!(
            backup
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("session.json.corrupted-")),
            "backup name format honoured"
        );
        let backup_contents = std::fs::read_to_string(&backup).expect("backup is readable");
        assert_eq!(backup_contents, "{", "backup preserves original bytes");
    }

    #[test]
    fn invalid_utf8_returns_corruption_info_and_writes_backup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");
        std::fs::write(&session_path, [0xff, 0xfe, b'{']).expect("seed broken session");

        let (state, info) = PaneFlowApp::load_session_at(&session_path);
        assert!(state.is_none(), "fallback to empty session expected");

        let info = info.expect("corruption info expected");
        assert_eq!(info.error_category, "data");
        let backup = info.backup_path.expect("backup path populated");
        let backup_contents = std::fs::read(&backup).expect("backup is readable");
        assert_eq!(backup_contents, vec![0xff, 0xfe, b'{']);
    }

    #[test]
    fn missing_file_yields_no_state_no_corruption() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nonexistent.json");

        let (state, info) = PaneFlowApp::load_session_at(&path);
        assert!(state.is_none());
        assert!(info.is_none(), "missing file is not corruption");
    }

    #[test]
    fn corruption_backup_rotation_caps_at_five() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_path = tmp.path().join("session.json");

        for ts in 1000..1007u64 {
            let p = tmp.path().join(format!("session.json.corrupted-{ts}"));
            std::fs::write(&p, format!("backup{ts}")).expect("seed backup");
        }

        rotate_corruption_backups(tmp.path(), "session.json");

        let mut surviving: Vec<u64> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("session.json.corrupted-"))
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .collect();
        surviving.sort_unstable();
        assert_eq!(
            surviving,
            vec![1002, 1003, 1004, 1005, 1006],
            "5 newest survive, 2 oldest deleted"
        );

        std::fs::write(&session_path, "{").expect("seed");
        assert!(session_path.exists());
    }

    #[test]
    fn save_seq_burst_coalesces_to_a_single_write() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

        let save_seq = AtomicU64::new(0);
        let captured: Vec<u64> = (0..20).map(|_| save_seq.fetch_add(1, SeqCst) + 1).collect();

        let latest = save_seq.load(SeqCst);
        let survivors = captured.iter().filter(|&&s| s == latest).count();
        assert_eq!(survivors, 1, "a 20-save burst coalesces to one write");
        assert_eq!(
            captured.last().copied(),
            Some(latest),
            "the most-recent snapshot is the survivor"
        );
    }

    #[test]
    fn deferred_save_skips_write_when_superseded_before_write() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

        let save_seq = AtomicU64::new(0);
        let deferred = save_seq.fetch_add(1, SeqCst) + 1;
        assert_eq!(
            save_seq.load(SeqCst),
            deferred,
            "deferred is latest pre-drain"
        );

        save_seq.fetch_add(1, SeqCst);

        assert_ne!(
            save_seq.load(SeqCst),
            deferred,
            "deferred write must be skipped after a quit-time bump"
        );
    }

    #[test]
    fn restored_managed_worktree_must_match_paneflow_worktree_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        let branch = "feat/session-hardening";
        let owned_path = crate::workspace::worktree::worktree_dir(&repo_root, branch);
        std::fs::create_dir_all(&owned_path).expect("owned worktree dir");
        std::fs::write(
            crate::workspace::worktree::owner_marker_path(&owned_path),
            "owner=paneflow\n",
        )
        .expect("owner marker");
        let valid = paneflow_config::schema::ManagedWorktreeDef {
            path: owned_path.to_string_lossy().into_owned(),
            repo_root: repo_root.to_string_lossy().into_owned(),
            branch: branch.to_string(),
            teardown: "auto".to_string(),
        };

        let restored = rehydrate_managed_worktree(&valid).expect("valid owned worktree restores");
        assert_eq!(restored.path, owned_path);
        assert_eq!(
            restored.teardown,
            crate::workspace::worktree::TeardownPolicy::Auto
        );

        let outside = paneflow_config::schema::ManagedWorktreeDef {
            path: tmp.path().join("external").to_string_lossy().into_owned(),
            ..valid.clone()
        };
        assert!(
            rehydrate_managed_worktree(&outside).is_none(),
            "a restored worktree path outside Paneflow's generated dir is dropped"
        );

        let unknown_policy = paneflow_config::schema::ManagedWorktreeDef {
            teardown: "delete".to_string(),
            ..valid
        };
        let restored =
            rehydrate_managed_worktree(&unknown_policy).expect("shape-valid worktree restores");
        assert_eq!(
            restored.teardown,
            crate::workspace::worktree::TeardownPolicy::Keep,
            "unknown restored policy must not become auto-remove"
        );
    }

    #[test]
    fn restore_candidate_order_puts_the_focused_surface_first() {
        use paneflow_config::schema::SurfaceDefinition;

        let surface = |focus: Option<bool>| SurfaceDefinition {
            focus,
            ..Default::default()
        };

        assert!(super::restore_candidate_order(&[]).is_empty());

        assert_eq!(
            super::restore_candidate_order(&[surface(None), surface(Some(false))]),
            vec![0, 1]
        );

        assert_eq!(
            super::restore_candidate_order(&[surface(None), surface(None), surface(Some(true))]),
            vec![2, 0, 1]
        );
    }
}

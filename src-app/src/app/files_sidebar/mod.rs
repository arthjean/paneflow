//! Docked Files right sidebar (PRD `prd-files-tree-sidebar-2026-Q3`, EP-001).
//!
//! Mirrors the agent-sessions sidebar (`sessions_sidebar.rs`): a
//! `flex_shrink_0` child of the root `flex_row`, toggled by the
//! `toggle_files_sidebar` action (`secondary-alt-f`), mutually exclusive with
//! the sessions sidebar (one right column). The pane header carries no Files
//! button: the tree is keyboard/command-driven only. Renders a lazily-expanded,
//! folders-first tree of the active workspace's `cwd`. Since US-019 of
//! `prd-file-editor-2026-Q3` every file opens in the diff dock's editor, and
//! markdown is no longer the exception: a `.md` row reads as source there like
//! any other file rather than opening a rendered pane of its own. Only
//! editor-refused files (binary or over `MAX_FILE_BYTES`) stay muted;
//! gitignored/hidden entries are filtered out before rendering. Rows carry no
//! drag: the EP-003 markdown drag-to-pane is gone, so a click is the sidebar's
//! only gesture and the dock editor its only destination.
//!
//! This module holds the state mutations (open/close, re-root, expand/collapse,
//! open-in-dock) + the container render; the header/body/row rendering lives in
//! `view.rs`, the type-to-filter matcher in `filter.rs`, and the pure tree model
//! + fs helpers in `files_tree.rs`.

mod context_menu;
mod filter;
mod keyboard;
mod row;
mod view;
mod watch;

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, Window,
    div, px,
};

use crate::app::files_tree::{self, FilesTreeState};
use crate::{PaneFlowApp, ToggleFilesSidebar};

/// Fixed sidebar width - matches the sessions sidebar (a resizable width is
/// deferred per the PRD non-goals).
pub(crate) const FILES_SIDEBAR_WIDTH: f32 = 300.;
pub(super) const SIDEBAR_WIDTH: Pixels = px(FILES_SIDEBAR_WIDTH);
/// Tree geometry, measured off the Codex file tree so the two read as the same
/// widget: 28px rows, an 18px indent step, one 14px leading slot (chevron for a
/// directory, language icon for a file) and a 12px gap before the name.
pub(super) const ROW_HEIGHT: Pixels = px(28.);
/// Per-depth indentation added to the row's left padding.
pub(super) const INDENT_STEP: f32 = 18.;
/// Width of the single leading slot. A directory fills it with its chevron, a
/// file with its language icon; both therefore start on the same pixel.
pub(super) const ROW_SLOT: f32 = 14.;
/// Gap between that slot and the name.
pub(super) const ROW_GAP: f32 = 12.;
/// Extra opacity knock-down for gitignored / hidden rows (US-004 second tier).
pub(super) const DIMMED_OPACITY: f32 = 0.55;

impl PaneFlowApp {
    /// Whether the surface that hosts the Files rail is on screen, whatever
    /// `files_sidebar_open` says.
    ///
    /// The open flag alone is not enough, exactly like the diff dock's own
    /// (see [`Self::diff_dock_visible`]): it survives a mode switch and a trip
    /// through Settings. The tree belongs to the CLI cockpit - its rows open
    /// into a pane or into the dock's editor, and neither exists on the
    /// full-screen Review surface - so `render` unmounts the rail off the
    /// cockpit and the surviving flag is what brings the same tree back on
    /// return. Also gates the toggle, so the chord cannot flip a rail the user
    /// cannot see.
    pub(crate) fn files_sidebar_host_visible(&self) -> bool {
        self.settings_section.is_none()
            && matches!(self.mode, paneflow_config::schema::AppMode::Cli)
    }

    /// Toggle the Files sidebar. Opening resolves the active workspace's `cwd`
    /// to the tree root, reads + auto-expands it, and closes the sessions
    /// sidebar (mutual exclusion). Re-clicking closes and releases the tree.
    pub(crate) fn handle_toggle_files_sidebar(
        &mut self,
        _: &ToggleFilesSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Inert off the cockpit: the rail is unmounted there, so a toggle would
        // only flip a flag the user cannot see.
        if !self.files_sidebar_host_visible() {
            return;
        }
        if !self.files_sidebar_open {
            self.files_surface_id = self
                .workspaces
                .get(self.active_idx)
                .and_then(|ws| ws.active_tab().root.as_ref())
                .and_then(|root| root.focused_pane(window, cx))
                .and_then(|pane| pane.read(cx).active_terminal_opt())
                .map(|terminal| terminal.entity_id().as_u64());
        }
        self.toggle_files_sidebar(cx);
        if self.files_sidebar_open {
            self.files_focus.focus(window, cx);
        }
    }

    pub(crate) fn toggle_files_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.files_sidebar_open {
            self.close_files_sidebar(cx);
            return;
        }
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        let root = PathBuf::from(&ws.cwd);
        // US-007: restore this workspace's expansion (held on the Workspace,
        // so it survives a previous close within the session and a restart).
        let persisted = ws.files_expanded.clone();

        // Mutual exclusion: only one right column is ever visible.
        if self.agent_sessions.sessions_sidebar_open
            || self.agent_sessions.sessions_sidebar_animation.is_some()
        {
            self.close_sessions_sidebar_immediate(cx);
        }
        // Floating dropdowns would paint over the docked panel.
        self.dismiss_transient_surfaces();

        self.set_files_sidebar_open(true, cx);
        self.files_tree_scroll = gpui::ScrollHandle::new();
        self.files_selected = 0;
        // US-020: a stale needle from a previous open would hide the tree the
        // user just asked for.
        self.files_filter_input
            .update(cx, |input, cx| input.clear(cx));
        // US-018: hydrate the tree + install non-recursive watches OFF the
        // render thread. A root shell paints this frame; `sync_files_expansion`
        // runs (and reconciles stale persisted paths back into `session.json`)
        // once hydration lands.
        self.spawn_files_hydration(root, persisted, cx);
    }

    /// Close the sidebar and release the per-open tree cache + watcher. The
    /// per-workspace expansion lives on the `Workspace`, so it is NOT reset
    /// here (US-007) - reopening restores it.
    pub(crate) fn close_files_sidebar(&mut self, cx: &mut Context<Self>) {
        // US-005: drop the watch + its channel while closed.
        self.files_watcher = None;
        self.files_event_rx = None;
        // Close any open row context menu so it can't outlive the tree.
        self.files_menu_open = None;
        self.set_files_sidebar_open(false, cx);
    }

    fn files_sidebar_width_at(&self, now: std::time::Instant) -> f32 {
        if let Some(animation) = self.files_sidebar_animation {
            animation.width_at(now)
        } else if self.files_sidebar_open {
            FILES_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    pub(crate) fn rendered_files_sidebar_width(&mut self, window: &mut Window) -> f32 {
        let now = std::time::Instant::now();
        if let Some(animation) = self.files_sidebar_animation {
            if animation.is_finished(now) {
                self.files_sidebar_animation = None;
                if !self.files_sidebar_open {
                    self.clear_files_sidebar_state();
                }
                animation.to_width
            } else {
                window.request_animation_frame();
                animation.width_at(now)
            }
        } else if self.files_sidebar_open {
            FILES_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    fn set_files_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.files_sidebar_width_at(now);
        self.files_sidebar_open = open;
        // The rail is one app-level surface, but wanting it belongs to the
        // session looking at it. Recording that here - the single funnel every
        // open and close goes through - is what keeps a sibling tab from
        // inheriting a tree it never asked for.
        if let Some(ws) = self.active_workspace_mut() {
            ws.active_tab_mut().files_sidebar_open = open;
        }
        let to_width = if open { FILES_SIDEBAR_WIDTH } else { 0. };

        self.files_sidebar_animation =
            if (from_width - to_width).abs() > crate::PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA {
                Some(crate::SidebarWidthAnimation {
                    from_width,
                    to_width,
                    started_at: now,
                })
            } else {
                None
            };

        if !open && self.files_sidebar_animation.is_none() {
            self.clear_files_sidebar_state();
        }
        cx.notify();
    }

    fn clear_files_sidebar_state(&mut self) {
        self.files_tree = FilesTreeState::default();
        self.files_watcher = None;
        self.files_event_rx = None;
        self.files_menu_open = None;
        self.files_surface_id = None;
        self.files_selected = 0;
    }

    /// Reconcile the live rail with the session (workspace tab) on screen.
    ///
    /// Two things can be stale after a session change: whether the rail should
    /// be up at all (the visible tab's own flag) and, when both sessions want
    /// it, which `cwd` it is rooted on. Idempotent and cheap on the steady
    /// path, so `render` can call it every frame - which is what makes this
    /// correct without every tab mutation (switch, close, reorder, cross-
    /// workspace move) having to remember the rail exists.
    pub(crate) fn sync_files_sidebar_session(&mut self, cx: &mut Context<Self>) {
        let wanted = self
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().files_sidebar_open);
        if wanted != self.files_sidebar_open {
            // Opening hydrates from the active workspace's `cwd`, so the
            // re-root below has nothing left to do on this path.
            if wanted {
                self.toggle_files_sidebar(cx);
            } else {
                self.close_files_sidebar(cx);
            }
            return;
        }
        self.reroot_files_tree(cx);
    }

    /// Re-root the tree on the active workspace's `cwd` when it changed while
    /// the sidebar is open (US-002 workspace-switch). No-op when closed or when
    /// the root is unchanged. Restores the new workspace's expansion (US-007)
    /// and re-targets the watcher (US-005).
    fn reroot_files_tree(&mut self, cx: &mut Context<Self>) {
        if !self.files_sidebar_open {
            return;
        }
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        // Borrowed compare: this runs on the render path every frame the rail
        // is up, and the owning `PathBuf` is only worth building on a miss.
        if self.files_tree.root == *Path::new(&ws.cwd) {
            return;
        }
        let root = PathBuf::from(&ws.cwd);
        let persisted = ws.files_expanded.clone();
        // US-018: re-root off the render thread.
        self.spawn_files_hydration(root, persisted, cx);
    }

    /// Expand or collapse a directory. First expand reads its listing (lazy,
    /// cached thereafter); when the live watcher is unavailable (US-006), every
    /// expand re-reads so manual navigation stays current without push updates.
    /// Reads are synchronous on the interaction (not the render path) per the
    /// PRD's "start synchronous" decision. Mirrors the expansion into the
    /// workspace + persists it (US-007).
    fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.files_tree.expanded.contains(path) {
            self.files_tree.expanded.remove(path);
            self.unwatch_files_dir(path);
        } else {
            self.files_tree.expanded.insert(path.to_path_buf());
            self.watch_files_dir(path);
            let stale =
                self.files_watcher.is_none() || !self.files_tree.children.contains_key(path);
            if stale {
                let listing = files_tree::read_dir_sorted(&self.files_tree.root, path);
                self.files_tree.children.insert(path.to_path_buf(), listing);
            }
        }
        self.sync_files_expansion();
        self.clamp_files_selection();
        self.save_session(cx);
        cx.notify();
    }

    /// US-019: open a file in the diff dock's editor. Every file goes here,
    /// markdown included - a `.md` row opens as source, not as a preview.
    ///
    /// The dock is the editor's only host, so a click from the sidebar has to
    /// put it on screen first. `wrap_cli_diff_dock` only mounts the panel when
    /// all three of its conditions hold, and the Files sidebar is a layout
    /// child of the root row - reachable from every mode and from behind
    /// Settings via the global `toggle_files_sidebar` chord - so satisfying
    /// `open` alone would leave the click opening a tab nobody can
    /// see. Settings is dismissed and the app returns to Cli mode before the
    /// tab is pushed.
    ///
    /// `open_diff_file_tab` owns the rest of the lifecycle: a file already open
    /// activates its tab instead of being duplicated, and a file the editor
    /// refuses (binary, too large) surfaces the US-003 error inside the tab.
    pub(crate) fn open_file_in_diff_dock(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section.is_some() {
            self.close_settings(cx);
        }
        // Idempotent when already in Cli mode; otherwise it parks the diff host
        // and hands focus to the active pane, which `open_diff_file_tab` then
        // takes back for the editor.
        self.enter_cli_mode(window, cx);
        if !self.diff_dock.open {
            let cwd = self.files_tree.root.to_string_lossy().into_owned();
            self.open_diff_dock_panel(cwd, cx);
        }
        // Opening a document *is* an answer to the dock's surface picker: the
        // dock must come up on the file, both now and the next time this
        // workspace toggles it from a pane header.
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        self.open_diff_file_tab(path, window, cx);
    }

    /// Render the docked Files sidebar. Only called when `files_sidebar_open`.
    pub(crate) fn render_files_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        div()
            .id("files-sidebar")
            .flex()
            .flex_col()
            .w(SIDEBAR_WIDTH)
            .flex_shrink_0()
            .h_full()
            .track_focus(&self.files_focus)
            .on_key_down(cx.listener(Self::handle_files_sidebar_key_down))
            // Match the app's other navigation rails: optional native material
            // on Windows, platform default on macOS, and a light/dark tint on Linux.
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .child(self.files_sidebar_header(ui, cx))
            .child(self.files_sidebar_body(ui, cx))
            .into_any_element()
    }
}

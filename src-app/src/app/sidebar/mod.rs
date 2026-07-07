//! Sidebar rendering for `PaneFlowApp`: workspace cards, action buttons,
//! notification dropdown, and the context-menu row helpers (in the
//! [`context_menu`] submodule).
//!
//! Extracted from `main.rs` per US-025 of the src-app refactor PRD - pure
//! code-motion, behaviour unchanged. Toast utilities and sidebar-adjacent
//! types (`WorkspaceContextMenu`, `WorkspaceDrag`, `WorkspaceDragPreview`)
//! remain in `main.rs` because they cross module boundaries.

pub(crate) mod context_menu;

use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, ClickEvent, Context, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, Styled,
    Transformation, Window, div, percentage, prelude::*, px, rgb, svg,
};

use crate::{
    PaneFlowApp, SIDEBAR_WIDTH, WorkspaceContextMenu, WorkspaceDrag, WorkspaceDragPreview,
    ai_types, workspace::Workspace,
};

/// US-048: memoized result of the sidebar's sibling-worktree grouping. The
/// `order` is a list of indices into `PaneFlowApp::workspaces`; `signature` is
/// the cheap content hash it was computed for (`None` until the first render).
/// Stored behind a `RefCell` on `PaneFlowApp` because `render_sidebar` borrows
/// `&self`.
#[derive(Default)]
pub(crate) struct SidebarOrderCache {
    signature: Option<u64>,
    order: Vec<usize>,
}

/// Debug-only render budget guard for the CLI sidebar. Mirrors the Agents
/// sidebar canary so projection or card regressions show up during profiling
/// without adding user-facing log noise.
struct SidebarRenderTimeCanary {
    start: std::time::Instant,
    workspace_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarDiffSummary {
    None,
    Lines { insertions: usize, deletions: usize },
    Files { files_changed: usize },
}

impl SidebarDiffSummary {
    fn is_visible(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl SidebarRenderTimeCanary {
    fn new(workspace_count: usize) -> Self {
        Self {
            start: std::time::Instant::now(),
            workspace_count,
        }
    }
}

impl Drop for SidebarRenderTimeCanary {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > std::time::Duration::from_millis(16) {
            tracing::debug!(
                target: "paneflow_app::sidebar",
                "render_sidebar exceeded 16ms frame budget: {:.2}ms across {} workspaces",
                elapsed.as_secs_f64() * 1000.0,
                self.workspace_count
            );
        }
    }
}

/// Collapse a `home`-rooted absolute path to a `~`-prefixed display string.
///
/// US-040: uses [`std::path::Path::strip_prefix`] (component-boundary match,
/// OS-native separator) instead of a raw `str::starts_with` + byte slice. The
/// old form false-positived on a partial component (`/home/arth` vs
/// `/home/arthur`) and assumed `/` separators. Returns `cwd` verbatim when it
/// isn't under `home` (or `home` is empty), so a Windows casing mismatch
/// degrades to the full path rather than a wrong collapse.
fn collapse_home(cwd: &str, home: &str) -> String {
    if home.is_empty() {
        return cwd.to_string();
    }
    match std::path::Path::new(cwd).strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => cwd.to_string(),
    }
}

fn visible_service_ports(
    active_ports: &[u16],
    service_labels: &std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
) -> Vec<u16> {
    active_ports
        .iter()
        .copied()
        .filter(|port| service_labels.contains_key(port))
        .collect()
}

fn sidebar_diff_summary(stats: &crate::workspace::GitDiffStats) -> SidebarDiffSummary {
    if stats.insertions > 0 || stats.deletions > 0 {
        SidebarDiffSummary::Lines {
            insertions: stats.insertions,
            deletions: stats.deletions,
        }
    } else if stats.files_changed > 0 {
        SidebarDiffSummary::Files {
            files_changed: stats.files_changed,
        }
    } else {
        SidebarDiffSummary::None
    }
}

fn sidebar_file_change_label(files_changed: usize) -> String {
    format!("{files_changed} changed")
}

impl PaneFlowApp {
    /// Cheap content signature for the sidebar display order (US-048). Hashes
    /// the workspace count plus each `(id, repo_root)` in positional order, so
    /// it changes on create / close / reorder / repo-root change - exactly the
    /// inputs [`Self::compute_display_order`] reads. No allocation.
    fn sidebar_order_signature(workspaces: &[Workspace]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        workspaces.len().hash(&mut hasher);
        for ws in workspaces {
            ws.id.hash(&mut hasher);
            match &ws.repo_root {
                Some(root) => root.hash(&mut hasher),
                None => 0u8.hash(&mut hasher),
            }
        }
        hasher.finish()
    }

    /// Sibling-worktree grouping (US-002): workspaces sharing a `repo_root`
    /// render contiguously when ≥2 share it (group appears at the first
    /// member's position); a lone workspace keeps its original position.
    /// Returns indices into `workspaces`. Pure - memoized by the caller.
    fn compute_display_order(workspaces: &[Workspace]) -> Vec<usize> {
        let mut repo_members: std::collections::HashMap<&std::path::Path, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, ws) in workspaces.iter().enumerate() {
            if let Some(root) = &ws.repo_root {
                repo_members.entry(root.as_path()).or_default().push(i);
            }
        }
        let mut order: Vec<usize> = Vec::with_capacity(workspaces.len());
        let mut placed = vec![false; workspaces.len()];
        for (i, ws) in workspaces.iter().enumerate() {
            if placed[i] {
                continue;
            }
            if let Some(root) = &ws.repo_root
                && let Some(members) = repo_members.get(root.as_path())
                && members.len() >= 2
            {
                for &m in members {
                    order.push(m);
                    placed[m] = true;
                }
                continue;
            }
            order.push(i);
            placed[i] = true;
        }
        order
    }

    pub(crate) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let _render_canary = SidebarRenderTimeCanary::new(self.workspaces.len());
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let mut sidebar = div()
            .relative()
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            // Cockpit rail (#141414), matching the Agents sidebar. The
            // border-right is gone: the rail and the #181818 content gutter
            // separate by a luminance step, not a drawn divider (the OpenAI
            // surface system - separation by luminance, not borders).
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .flex()
            .flex_col();

        // All top-of-sidebar affordances (New workspace, Clear all,
        // Open Settings) moved into the bottom-of-sidebar Settings
        // popover. The top of the CLI sidebar is now empty -- see
        // `cli_menu_items` for the popover contents.
        // Workspace list - scrollable area. Wheel-scroll comes from
        // `overflow_y_scroll + track_scroll`; the visible scroll bar
        // is gone, so the list uses the full sidebar width without a
        // trailing gutter.
        let mut list = div()
            .id("workspace-list")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .flex()
            .flex_col()
            .gap(px(6.))
            .pb(px(8.));

        if self.workspaces.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(44.))
                            .h(px(44.))
                            .rounded(px(10.))
                            .bg(ui.subtle)
                            .child(
                                svg()
                                    .size(px(20.))
                                    .flex_none()
                                    .path("icons/folder_open.svg")
                                    .text_color(ui.muted),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text)
                                    .child("No workspaces yet"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui.muted)
                                    .child("Create one to get started"),
                            ),
                    )
                    .child(
                        div()
                            .id("empty-new-ws")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(12.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .bg(ui.text)
                            .text_color(ui.base)
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|s| s.opacity(0.85))
                            .on_click(cx.listener(|this, _: &ClickEvent, w, cx| {
                                this.create_workspace_with_picker(w, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .flex_none()
                                    .path("icons/plus.svg")
                                    .text_color(ui.base),
                            )
                            .child("New Workspace"),
                    ),
            );
            sidebar = sidebar.child(list);
            return sidebar;
        }

        // ── Sibling-worktree grouping (US-002), memoized (US-048) ──
        // The display order depends only on the workspace set/order and each
        // `repo_root`. `render_sidebar` runs on every app `notify()`, so the old
        // per-frame `HashMap` + `Vec` rebuild was pure waste - recompute only
        // when a cheap content signature changes. `idx` stays the workspace's
        // real index in `self.workspaces`, so selection/drag/rename are
        // unaffected by the display reordering.
        let signature = Self::sidebar_order_signature(&self.workspaces);
        if self.sidebar_order_cache.borrow().signature != Some(signature) {
            let order = Self::compute_display_order(&self.workspaces);
            let mut cache = self.sidebar_order_cache.borrow_mut();
            cache.order = order;
            cache.signature = Some(signature);
        }
        let order_cache = self.sidebar_order_cache.borrow();
        for &i in &order_cache.order {
            let ws = &self.workspaces[i];
            let is_active = i == self.active_idx;

            let title = ws.title.clone();
            // Format cwd as ~/... (collapse home dir)
            let cwd_display = collapse_home(&ws.cwd, &self.home_dir);

            let idx = i;
            let ws_id = ws.id;
            let ws_title: SharedString = ws.title.clone().into();

            let mut card = div()
                .id(SharedString::from(format!("ws-{i}")))
                .mx(px(8.))
                .px(px(10.))
                .py(px(8.))
                .rounded(crate::app::constants::WORKSPACE_CARD_CORNER_RADIUS)
                .cursor_pointer()
                // Quiet card (Codex/OpenAI sidebar row): transparent at rest,
                // with the same subtle translucent tint for selection and hover
                // in dark mode. The accent stays reserved for agent status.
                .when(is_active, |d| {
                    d.bg(crate::app::constants::sidebar_tab_active_background())
                })
                .when(!is_active, |d| {
                    d.hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
                })
                .on_drag(
                    WorkspaceDrag {
                        id: ws_id,
                        title: ws_title.clone(),
                    },
                    |drag, _offset, _window, cx| {
                        cx.new(|_| WorkspaceDragPreview {
                            title: drag.title.clone(),
                        })
                    },
                )
                .on_drop(cx.listener(move |this, drag: &WorkspaceDrag, _window, cx| {
                    this.reorder_workspace(drag.id, idx, cx);
                }))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    this.dismiss_transient_surfaces();
                    let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                    if is_double {
                        this.commit_rename(cx); // commit any previous rename
                        this.rename_text = this.workspaces[idx].title.clone();
                        this.renaming_idx = Some(idx);
                    } else {
                        this.commit_rename(cx);
                        this.select_workspace(idx, window, cx);
                    }
                    cx.notify();
                }))
                .on_aux_click(cx.listener(move |this, e: &ClickEvent, _window, cx| {
                    if e.is_right_click()
                        && let Some(position) = e.mouse_position()
                    {
                        this.commit_rename(cx);
                        this.dismiss_transient_surfaces();
                        this.workspace_menu_open = Some(WorkspaceContextMenu { idx, position });
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _window, cx| {
                    if this.renaming_idx != Some(idx) {
                        return;
                    }
                    let key = e.keystroke.key.as_str();
                    match key {
                        "enter" => {
                            this.commit_rename(cx);
                            cx.notify();
                        }
                        "escape" => {
                            this.renaming_idx = None;
                            this.rename_text.clear();
                            cx.notify();
                        }
                        "backspace" => {
                            this.rename_text.pop();
                            cx.notify();
                        }
                        _ => {
                            if let Some(ch) = &e.keystroke.key_char
                                && !ch.is_empty()
                                && !e.keystroke.modifiers.control
                                && !e.keystroke.modifiers.platform
                            {
                                this.rename_text.push_str(ch);
                                cx.notify();
                            }
                        }
                    }
                }))
                .flex()
                .flex_col()
                .gap(px(5.));

            // ── Row 1: Title ──
            let title_el = if self.renaming_idx == Some(i) {
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(ui.text)
                    .text_sm()
                    .font_weight(FontWeight::NORMAL)
                    .bg(ui.overlay)
                    .px_1()
                    .rounded_sm()
                    .child(format!("{}|", self.rename_text))
            } else {
                // Display title keeps its natural width so the
                // session pastille that follows sits flush against
                // the workspace name. No `flex_1 / min_w_0 / truncate`
                // here -- that combo causes GPUI to collapse the
                // label to "…" even with plenty of room (same bug
                // we hit on the branch label). Unusually long titles
                // will push the row width past the card; the user
                // can rename them.
                div()
                    .flex_none()
                    .text_color(ui.text)
                    .text_sm()
                    .font_weight(FontWeight::NORMAL)
                    .child(title)
            };

            // Shared projection for hook-backed rows and detected-but-unhooked
            // fallbacks. `Workspace::detected_agents` covers every known
            // `TerminalAgent`, so the title dot tooltip uses the real display
            // names instead of the old Claude/Codex-only helper.
            let agent_status =
                ai_types::workspace_agent_status(ws.agent_sessions.values(), &ws.detected_agents);
            let has_active_session = !agent_status.active_labels.is_empty();
            let session_tooltip = agent_session_tooltip(&agent_status.active_labels);

            let title_row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .min_w_0()
                .child(title_el)
                .when(has_active_session, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("ws-{i}-session-dot")))
                            .flex_none()
                            .w(px(7.))
                            .h(px(7.))
                            .rounded_full()
                            .bg(rgb(0x3B82F6)) // Tailwind blue-500
                            .tooltip({
                                let label = session_tooltip.clone();
                                move |_w, cx| {
                                    cx.new(|_| SidebarTooltip {
                                        label: label.clone(),
                                    })
                                    .into()
                                }
                            }),
                    )
                });

            card = card.child(title_row);

            // ── Meta line - branch, diff stats, and detected services, all on a
            // single compact muted line (Codex quiet-card: one airy meta row,
            // not a stack of three). `flex_wrap()` lets a long branch or extra
            // ports drop gracefully instead of truncating; the branch keeps its
            // natural width (GPUI's truncate + flex_shrink + min_w_0 combo
            // collapses the label to "…" even with room to spare).
            let has_branch = !ws.git_branch.is_empty();
            let diff_summary = sidebar_diff_summary(&ws.git_stats);
            let has_stats = diff_summary.is_visible();
            let display_ports = visible_service_ports(&ws.active_ports, &ws.service_labels);
            let has_ports = !display_ports.is_empty();
            if has_branch || has_stats || has_ports {
                let mut meta_row = div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .text_xs()
                    .text_color(ui.muted);

                if has_branch {
                    meta_row = meta_row
                        .child(
                            svg()
                                .size(px(11.))
                                .flex_none()
                                .path("icons/git-branch.svg")
                                .text_color(ui.muted),
                        )
                        .child(div().flex_none().child(ws.git_branch.clone()));
                }

                if has_branch && has_stats {
                    meta_row = meta_row.child(div().flex_none().text_color(ui.muted).child("·"));
                }

                if has_stats {
                    // Shared diff palette (Codex green/red on dark, theme vc_* on
                    // light) so the CLI sidebar diffstat matches the Diff/Review
                    // view and the Agents dock instead of inlining its own hex.
                    let diff = ui.diff_colors();
                    match diff_summary {
                        SidebarDiffSummary::Lines {
                            insertions,
                            deletions,
                        } => {
                            if insertions > 0 {
                                meta_row = meta_row.child(
                                    div()
                                        .flex_none()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(diff.added)
                                        .child(format!("+{insertions}")),
                                );
                            }
                            if deletions > 0 {
                                meta_row = meta_row.child(
                                    div()
                                        .flex_none()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(diff.deleted)
                                        .child(format!("-{deletions}")),
                                );
                            }
                        }
                        SidebarDiffSummary::Files { files_changed } => {
                            meta_row = meta_row.child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.muted)
                                    .child(sidebar_file_change_label(files_changed)),
                            );
                        }
                        SidebarDiffSummary::None => {}
                    }
                }

                // Separator before the ports, only when branch/diff preceded
                // them (a leading `·` would otherwise dangle).
                if (has_branch || has_stats) && has_ports {
                    meta_row = meta_row.child(div().flex_none().text_color(ui.muted).child("·"));
                }

                // Detected services. Frontend services are clickable tinted chips;
                // non-frontend ports render as plain muted `:port` text so the
                // chip ink stays meaningful (= clickable). Capped at 3 visible
                // to keep the card height predictable; overflow condenses to a
                // `+N` muted counter.
                if has_ports {
                    const PORTS_VISIBLE: usize = 3;
                    for (pi, port) in display_ports.iter().take(PORTS_VISIBLE).enumerate() {
                        let info = ws.service_labels.get(port);
                        let is_frontend = info.is_some_and(|i| i.is_frontend);
                        let label = if let Some(i) = info
                            && let Some(ref l) = i.label
                        {
                            format!("{l} :{port}")
                        } else {
                            format!(":{port}")
                        };

                        if is_frontend {
                            let url = info
                                .and_then(|i| i.url.clone())
                                .unwrap_or_else(|| format!("http://localhost:{port}"));
                            meta_row = meta_row.child(
                                div()
                                    .id(SharedString::from(format!("port-{idx}-{pi}")))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(ui.subtle)
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text)
                                    .cursor_pointer()
                                    .hover(|s| {
                                        let ui = crate::theme::ui_colors();
                                        s.bg(ui.surface)
                                    })
                                    // Open via Paneflow's URL launcher so
                                    // Windows browsers break away from the
                                    // kill-on-close Job Object.
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _w, cx| {
                                            if let Err(err) =
                                                crate::external_open::open_url(&url)
                                            {
                                                let msg = if err.kind()
                                                    == std::io::ErrorKind::NotFound
                                                {
                                                    "Could not open URL - install xdg-utils (Linux), or check your default browser".to_string()
                                                } else {
                                                    format!("Could not open URL: {err}")
                                                };
                                                log::warn!("sidebar: open URL failed: {err}");
                                                this.show_toast(msg, cx);
                                            }
                                        },
                                    ))
                                    .child(label),
                            );
                        } else {
                            meta_row = meta_row
                                .child(div().text_size(px(10.)).text_color(ui.muted).child(label));
                        }
                    }

                    if display_ports.len() > PORTS_VISIBLE {
                        meta_row = meta_row.child(
                            div()
                                .px(px(4.))
                                .text_size(px(10.))
                                .text_color(ui.muted)
                                .child(format!("+{}", display_ports.len() - PORTS_VISIBLE)),
                        );
                    }
                }

                card = card.child(meta_row);
            }

            // ── Row 4: AI tool status (one row per active tool). The projection
            // is shared with `fleet.list`; this render layer only maps status
            // specs to GPUI elements.
            for agg in &agent_status.hooked {
                card = card.child(render_workspace_agent_status_row(agg, ws.id, ui));
            }

            // Agents detected in the process tree (per-pane /proc scan) with
            // NO IPC session - the shim was bypassed (shell alias, rc file
            // rewriting PATH, absolute binary path) or the agent has no hook
            // support at all. Show an honest static "running" row instead of
            // nothing: the user sees the agent is alive without a fabricated
            // lifecycle state (no spinner - we genuinely don't know).
            for tool in agent_status.unhooked {
                let name = tool.display_name();
                card = card.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .text_xs()
                        .text_color(ui.muted)
                        .child(
                            div()
                                .w(px(11.))
                                .h(px(11.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().w(px(5.)).h(px(5.)).rounded_full().bg(ui.muted)),
                        )
                        .child(div().child(format!("{name} running"))),
                );
            }

            // Working directory moves into a hover tooltip so the card
            // height stays tight. Users who need the path still get it
            // on hover; the visible layout no longer carries a row
            // that duplicates information already implied by the title.
            let cwd_tooltip = SharedString::from(cwd_display);
            card = card.tooltip(move |_w, cx| {
                cx.new(|_| WorkspaceCwdTooltip {
                    path: cwd_tooltip.clone(),
                })
                .into()
            });

            list = list.child(card);
        }

        // Wrap list + scrollbar in a relative flex_1 container so the
        // overlay can absolutely-position itself over the list. Mouse
        // handlers stay on the wrapper to track drag even if the cursor
        // leaves the 6px-wide thumb.
        sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
        sidebar = sidebar.child(self.render_sidebar_settings_footer(self.cli_menu_items(), cx));
        sidebar = sidebar.child(self.render_mode_toggle(cx));
        sidebar
    }

    /// Items rendered inside the bottom Settings popover when in CLI
    /// mode. Order: creation actions first, destructive last, escape
    /// hatch to the Settings window.
    fn cli_menu_items(&self) -> Vec<crate::app::sidebar_actions_menu::SidebarMenuItem> {
        use crate::app::sidebar_actions_menu::SidebarMenuItem;
        let mut items = vec![SidebarMenuItem {
            id: "cli-menu-themes".into(),
            icon: "icons/palette.svg",
            label: "Themes".into(),
            on_click: Box::new(|app, w, cx| {
                app.open_theme_picker(w, cx);
            }),
        }];
        items.push(SidebarMenuItem {
            id: "cli-menu-about".into(),
            icon: "icons/info-circle.svg",
            label: "About Paneflow".into(),
            on_click: Box::new(|app, _w, cx| {
                app.show_about_dialog = true;
                cx.notify();
            }),
        });
        items.push(SidebarMenuItem {
            id: "cli-menu-open-settings".into(),
            icon: "icons/settings.svg",
            label: "Settings".into(),
            on_click: Box::new(|app, w, cx| {
                app.open_settings_window(w, cx);
            }),
        });
        items
    }

    pub(crate) fn sidebar_list_wrapper(
        &self,
        list: gpui::Stateful<gpui::Div>,
        _cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // The visible scroll bar was removed; wheel-scroll on the
        // inner `list` (driven by `overflow_y_scroll + track_scroll`)
        // is the only scrolling surface now. The wrapper still
        // exists so callers keep a stable insertion point if a
        // trailing affordance lands here later.
        div()
            .id("sidebar-list-wrapper")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .child(list)
    }
}

fn agent_session_tooltip(labels: &[String]) -> SharedString {
    match labels {
        [] => SharedString::default(),
        [one] => format!("{one} session active").into(),
        [a, b] => format!("{a} + {b} sessions active").into(),
        many => format!("{} active sessions", many.len()).into(),
    }
}

fn render_workspace_agent_status_row(
    agg: &ai_types::ToolAggregate,
    workspace_id: u64,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let extra = agg.extra_suffix();
    match &agg.dominant {
        ai_types::AgentState::Thinking => {
            let is_claude = matches!(agg.tool, crate::agent_launcher::TerminalAgent::ClaudeCode);
            let thinking_color: gpui::Hsla = if is_claude {
                ui.agent_claude
            } else {
                rgb(0xc4c4c4).into()
            };
            let glyph: AnyElement = svg()
                .size(px(11.))
                .flex_none()
                .path("icons/loader-circle.svg")
                .text_color(thinking_color)
                .with_animation(
                    SharedString::from(format!(
                        "sidebar-spinner-{}-{}",
                        workspace_id,
                        agg.tool.display_name()
                    )),
                    Animation::new(std::time::Duration::from_secs(1)).repeat(),
                    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                )
                .into_any_element();
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .text_xs()
                .text_color(thinking_color)
                .child(glyph)
                .child(div().child(format!("{} thinking…{}", agg.tool.display_name(), extra)))
                .into_any_element()
        }
        ai_types::AgentState::WaitingForInput => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .self_start()
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .bg(rgb(0xFBBF24))
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/bell.svg")
                    .text_color(ui.base),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.base)
                    .child(format!("{} needs input{}", agg.tool.display_name(), extra)),
            )
            .into_any_element(),
        ai_types::AgentState::Finished => {
            let done_color = rgb(0x00E08A);
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .text_xs()
                .text_color(done_color)
                .child(
                    svg()
                        .size(px(11.))
                        .flex_none()
                        .path("icons/check.svg")
                        .text_color(done_color),
                )
                .child(div().child(format!("{} done{}", agg.tool.display_name(), extra)))
                .into_any_element()
        }
        ai_types::AgentState::Errored => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .text_xs()
            .text_color(ui.agent_error)
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/x_circle.svg")
                    .text_color(ui.agent_error),
            )
            .child(div().child(format!("{} errored{}", agg.tool.display_name(), extra)))
            .into_any_element(),
        ai_types::AgentState::Stalled => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .text_xs()
            .text_color(ui.agent_stalled)
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/triangle-alert.svg")
                    .text_color(ui.agent_stalled),
            )
            .child(div().child(format!("{} stalled{}", agg.tool.display_name(), extra)))
            .into_any_element(),
    }
}

/// Lightweight tooltip body reused by sidebar affordances that just
/// need to show one short label. Mirrors the `WorkspaceCwdTooltip`
/// style minus the monospace font so prose reads naturally.
/// `pub(crate)`: the tab identity pill (EP-005, pane.rs) reuses it rather
/// than duplicating a fourth one-label tooltip body.
pub(crate) struct SidebarTooltip {
    pub(crate) label: SharedString,
}

impl Render for SidebarTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::active_theme();
        let ui = crate::theme::ui_colors();
        div()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(theme.title_bar_background)
            .border_1()
            .border_color(ui.border)
            .text_color(ui.text)
            .text_size(px(11.))
            .child(self.label.clone())
    }
}

/// Tooltip body for a workspace card. Surfaces the full cwd path so
/// it can stay off-screen on the card itself (the title is enough
/// signal at idle; the path is only relevant when the user needs to
/// distinguish two workspaces with similar titles or open a shell at
/// that exact location).
struct WorkspaceCwdTooltip {
    path: SharedString,
}

impl Render for WorkspaceCwdTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::active_theme();
        let ui = crate::theme::ui_colors();
        div()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(theme.title_bar_background)
            .border_1()
            .border_color(ui.border)
            .text_color(ui.text)
            .text_size(px(11.))
            .font_family("monospace")
            .child(self.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SidebarDiffSummary, collapse_home, sidebar_diff_summary, sidebar_file_change_label,
        visible_service_ports,
    };
    use crate::terminal::ServiceInfo;
    use crate::workspace::GitDiffStats;
    use std::collections::HashMap;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn collapses_nested_path_under_home() {
        assert_eq!(
            collapse_home("/home/arthur/dev/x", "/home/arthur"),
            "~/dev/x"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn exact_home_collapses_to_tilde() {
        assert_eq!(collapse_home("/home/arthur", "/home/arthur"), "~");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn partial_component_is_not_a_prefix() {
        // US-040 regression: `/home/arth` must NOT match `/home/arthur` - the
        // old `starts_with` + byte slice produced the bogus "~ur/proj".
        assert_eq!(
            collapse_home("/home/arthur/proj", "/home/arth"),
            "/home/arthur/proj"
        );
    }

    #[test]
    fn empty_home_returns_cwd_verbatim() {
        assert_eq!(collapse_home("/some/path", ""), "/some/path");
    }

    #[test]
    fn visible_service_ports_hide_unlabeled_ephemeral_ports() {
        let labels = HashMap::from([
            (
                3000,
                ServiceInfo {
                    port: 3000,
                    url: Some("http://localhost:3000".to_string()),
                    label: Some("Next.js".to_string()),
                    is_frontend: true,
                },
            ),
            (
                8000,
                ServiceInfo {
                    port: 8000,
                    url: Some("http://localhost:8000".to_string()),
                    label: Some("Fastify".to_string()),
                    is_frontend: false,
                },
            ),
        ]);

        assert_eq!(
            visible_service_ports(&[3000, 53154, 8000, 53155], &labels),
            vec![3000, 8000]
        );
    }

    #[test]
    fn sidebar_diff_summary_hides_zero_line_counts() {
        let binary_only = GitDiffStats {
            files_changed: 1,
            insertions: 0,
            deletions: 0,
        };
        assert_eq!(
            sidebar_diff_summary(&binary_only),
            SidebarDiffSummary::Files { files_changed: 1 }
        );

        let deletion_only = GitDiffStats {
            files_changed: 1,
            insertions: 0,
            deletions: 82,
        };
        assert_eq!(
            sidebar_diff_summary(&deletion_only),
            SidebarDiffSummary::Lines {
                insertions: 0,
                deletions: 82
            }
        );

        assert_eq!(
            sidebar_diff_summary(&GitDiffStats::default()),
            SidebarDiffSummary::None
        );
    }

    #[test]
    fn sidebar_file_change_label_is_compact_for_unmeasured_diffs() {
        assert_eq!(sidebar_file_change_label(1), "1 changed");
        assert_eq!(sidebar_file_change_label(2), "2 changed");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn cwd_outside_home_is_unchanged() {
        assert_eq!(collapse_home("/etc/hosts", "/home/arthur"), "/etc/hosts");
    }
}

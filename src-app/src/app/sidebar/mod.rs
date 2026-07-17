//! Sidebar rendering for `PaneFlowApp`: workspace rows, action buttons,
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
    Window, div, prelude::*, px, rgb, svg,
};

use crate::{
    PaneFlowApp, SIDEBAR_WIDTH, WorkspaceContextMenu, WorkspaceDrag, WorkspaceDragPreview,
    ai_types, workspace::Workspace,
};

/// Memoized sibling-worktree ordering. Group labels stay hidden, but sibling
/// worktrees remain contiguous as before the visual redesign.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarAgentState {
    NeedsInput,
    Errored,
    Stalled,
    Finished,
    Thinking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidebarAgentSummary {
    state: SidebarAgentState,
    count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidebarServiceSummary {
    primary: u16,
    overflow: usize,
}

const SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
const SIDEBAR_ROW_PADDING_X: f32 = 8.0;
const SIDEBAR_TITLE_ROW_GAP: f32 = 8.0;
const SIDEBAR_AGENT_STATUS_SLOT_WIDTH: f32 = 48.0;
const SIDEBAR_AGENT_ICON_SLOT_WIDTH: f32 = 20.0;
const SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH: f32 =
    SIDEBAR_WIDTH - SIDEBAR_ROW_MARGIN_X * 2.0 - SIDEBAR_ROW_PADDING_X * 2.0;

impl SidebarDiffSummary {
    fn is_visible(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl SidebarAgentSummary {
    fn slot_width(self) -> f32 {
        if self.state == SidebarAgentState::NeedsInput {
            SIDEBAR_AGENT_STATUS_SLOT_WIDTH
        } else if self.count > 1 {
            28.0
        } else {
            SIDEBAR_AGENT_ICON_SLOT_WIDTH
        }
    }

    fn tooltip_state(self) -> String {
        match self.state {
            SidebarAgentState::NeedsInput => {
                agent_status_sentence(self.count, "needs input", "need input")
            }
            SidebarAgentState::Errored => agent_status_sentence(self.count, "errored", "errored"),
            SidebarAgentState::Stalled => agent_status_sentence(self.count, "stalled", "stalled"),
            SidebarAgentState::Thinking => {
                agent_status_sentence(self.count, "thinking", "thinking")
            }
            SidebarAgentState::Finished => {
                "Agent finished · Click workspace to dismiss".to_string()
            }
        }
    }
}

fn agent_status_sentence(count: usize, singular_state: &str, plural_state: &str) -> String {
    if count == 1 {
        format!("1 agent {singular_state}")
    } else {
        format!("{count} agents {plural_state}")
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

fn sidebar_service_summary(
    active_ports: &[u16],
    service_labels: &std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
) -> Option<SidebarServiceSummary> {
    let visible = visible_service_ports(active_ports, service_labels);
    let primary = visible
        .iter()
        .copied()
        .find(|port| {
            service_labels
                .get(port)
                .is_some_and(|info| info.is_frontend)
        })
        .or_else(|| visible.first().copied())?;
    Some(SidebarServiceSummary {
        primary,
        overflow: visible.len().saturating_sub(1),
    })
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

fn sidebar_agent_summary<'a, I>(sessions: I, completion_unread: bool) -> Option<SidebarAgentSummary>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
{
    let mut counts = [0usize; 4];
    for session in sessions {
        let index = match session.state {
            ai_types::AgentState::WaitingForInput => 0,
            ai_types::AgentState::Errored => 1,
            ai_types::AgentState::Stalled => 2,
            ai_types::AgentState::Thinking => 3,
            ai_types::AgentState::Finished => continue,
        };
        counts[index] += 1;
    }

    let priority = [
        SidebarAgentState::NeedsInput,
        SidebarAgentState::Errored,
        SidebarAgentState::Stalled,
    ];
    for (state, count) in priority.into_iter().zip(counts[..3].iter().copied()) {
        if count > 0 {
            return Some(SidebarAgentSummary { state, count });
        }
    }

    if completion_unread {
        return Some(SidebarAgentSummary {
            state: SidebarAgentState::Finished,
            count: 1,
        });
    }

    (counts[3] > 0).then_some(SidebarAgentSummary {
        state: SidebarAgentState::Thinking,
        count: counts[3],
    })
}

fn sidebar_workspace_title_slot_width(summary: Option<SidebarAgentSummary>) -> f32 {
    let reserved = summary.map_or(0.0, |summary| SIDEBAR_TITLE_ROW_GAP + summary.slot_width());
    (SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH - reserved).max(0.0)
}

impl PaneFlowApp {
    fn begin_workspace_rename(&mut self, index: usize, cx: &gpui::App) {
        self.commit_rename(cx);
        if let Some(title) = self
            .workspaces
            .get(index)
            .map(|workspace| workspace.title.clone())
        {
            self.rename_text = title;
            self.renaming_idx = Some(index);
        }
    }

    fn sidebar_order_signature(workspaces: &[Workspace]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        workspaces.len().hash(&mut hasher);
        for workspace in workspaces {
            workspace.id.hash(&mut hasher);
            match &workspace.repo_root {
                Some(root) => root.hash(&mut hasher),
                None => 0u8.hash(&mut hasher),
            }
        }
        hasher.finish()
    }

    fn compute_display_order(workspaces: &[Workspace]) -> Vec<usize> {
        let mut repo_members: std::collections::HashMap<&std::path::Path, Vec<usize>> =
            std::collections::HashMap::new();
        for (index, workspace) in workspaces.iter().enumerate() {
            if let Some(root) = &workspace.repo_root {
                repo_members.entry(root.as_path()).or_default().push(index);
            }
        }

        let mut order = Vec::with_capacity(workspaces.len());
        let mut placed = vec![false; workspaces.len()];
        for (index, workspace) in workspaces.iter().enumerate() {
            if placed[index] {
                continue;
            }
            if let Some(root) = &workspace.repo_root
                && let Some(members) = repo_members.get(root.as_path())
                && members.len() >= 2
            {
                for &member in members {
                    order.push(member);
                    placed[member] = true;
                }
                continue;
            }
            order.push(index);
            placed[index] = true;
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

        let new_workspace_tooltip = self.shortcut_for_action("new_workspace").map_or_else(
            || "New workspace".to_string(),
            |key| format!("New workspace  {key}"),
        );
        sidebar = sidebar.child(
            div()
                .h(px(48.))
                .flex_none()
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .pl(px(8.))
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ui.text)
                        .child("Workspaces"),
                )
                .child(
                    div()
                        .id("sidebar-new-workspace")
                        .size(px(28.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|s| s.bg(crate::app::constants::sidebar_tab_active_background()))
                        .tooltip(move |_w, cx| {
                            cx.new(|_| SidebarTooltip {
                                label: new_workspace_tooltip.clone().into(),
                            })
                            .into()
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.create_workspace_with_picker(window, cx);
                        }))
                        .child(
                            svg()
                                .size(px(12.))
                                .flex_none()
                                .path("icons/plus.svg")
                                .text_color(ui.muted),
                        ),
                ),
        );

        // Workspace list - scrollable area. Wheel-scroll comes from
        // `overflow_y_scroll + track_scroll`; the visible scroll bar
        // is gone, so the list uses the full sidebar width without a
        // trailing gutter.
        let mut list = div()
            .id("workspace-list")
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .flex()
            .flex_col()
            .gap(px(4.))
            .pt(px(4.))
            .pb(px(8.));

        if self.workspaces.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child("Open a project folder"),
                    )
                    .child(
                        div()
                            .id("empty-new-ws")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .bg(ui.subtle)
                            .text_color(ui.text)
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .hover(|s| s.bg(crate::app::constants::sidebar_tab_active_background()))
                            .on_click(cx.listener(|this, _: &ClickEvent, w, cx| {
                                this.create_workspace_with_picker(w, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .flex_none()
                                    .path("icons/folder_open.svg")
                                    .text_color(ui.muted),
                            )
                            .child("Open folder"),
                    ),
            );
        }

        list = self.render_workspace_rows(list, ui, cx);
        sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
        sidebar = sidebar.child(self.render_sidebar_settings_footer(self.cli_menu_items(), cx));
        sidebar = sidebar.child(self.render_mode_toggle(cx));
        sidebar
    }

    fn render_workspace_rows(
        &self,
        mut list: gpui::Stateful<gpui::Div>,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let signature = Self::sidebar_order_signature(&self.workspaces);
        if self.sidebar_order_cache.borrow().signature != Some(signature) {
            let order = Self::compute_display_order(&self.workspaces);
            let mut cache = self.sidebar_order_cache.borrow_mut();
            cache.order = order;
            cache.signature = Some(signature);
        }
        let order_cache = self.sidebar_order_cache.borrow();
        for &i in &order_cache.order {
            list = list.child(self.render_workspace_row(i, ui, cx));
        }
        list
    }

    fn render_workspace_row(
        &self,
        i: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[i];
        let is_active = i == self.active_idx;

        let title = ws.title.clone();
        // Format cwd as ~/... (collapse home dir)
        let cwd_display = collapse_home(&ws.cwd, &self.home_dir);

        let idx = i;
        let ws_id = ws.id;
        let ws_title: SharedString = ws.title.clone().into();

        let mut row = div()
            .id(SharedString::from(format!("ws-{i}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .px(px(SIDEBAR_ROW_PADDING_X))
            .py(px(4.))
            .min_h(px(44.))
            .rounded(px(8.))
            .overflow_x_hidden()
            .when(is_active, |d| {
                d.bg(crate::app::constants::sidebar_tab_active_background())
            })
            .when(!is_active, |d| {
                d.hover(|s| s.bg(crate::app::constants::sidebar_tab_active_background()))
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
                if let Some(workspace) = this.workspaces.get_mut(idx) {
                    workspace.agent_completion_notification.acknowledge();
                }
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                if is_double {
                    this.begin_workspace_rename(idx, cx);
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
                let key = e.keystroke.key.as_str();
                if this.renaming_idx != Some(idx) {
                    if key == "f2" {
                        this.begin_workspace_rename(idx, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    return;
                }
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
            .gap(px(4.));

        // Row 1: title
        let agent_status =
            ai_types::workspace_agent_status(ws.agent_sessions.values(), &ws.detected_agents);
        let row_agent_status = sidebar_agent_summary(
            ws.agent_sessions.values(),
            ws.agent_completion_notification.is_unread(),
        );
        let title_slot_width = sidebar_workspace_title_slot_width(row_agent_status);

        let title_el = if self.renaming_idx == Some(i) {
            div()
                .w(px(title_slot_width))
                .max_w(px(title_slot_width))
                .min_w_0()
                .overflow_x_hidden()
                .text_color(ui.text)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .bg(ui.overlay)
                .px_1()
                .rounded_sm()
                .child(format!("{}|", self.rename_text))
        } else {
            div()
                .w(px(title_slot_width))
                .max_w(px(title_slot_width))
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(ui.text)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(title)
        };

        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .min_w_0()
            .overflow_x_hidden()
            .child(title_el);
        if let Some(row_agent_status) = row_agent_status {
            let status_tooltip = sidebar_agent_status_tooltip(row_agent_status, &agent_status);
            title_row = title_row.child(render_workspace_agent_summary(
                row_agent_status,
                ws.id,
                status_tooltip,
                ui,
            ));
        }

        row = row.child(title_row);

        if let Some(meta_row) = self.render_workspace_meta_row(idx, ws, ui, cx) {
            row = row.child(meta_row);
        }

        let cwd_tooltip = SharedString::from(cwd_display);
        row = row.tooltip(move |_w, cx| {
            cx.new(|_| WorkspaceCwdTooltip {
                path: cwd_tooltip.clone(),
            })
            .into()
        });
        row
    }

    fn render_workspace_meta_row(
        &self,
        idx: usize,
        ws: &Workspace,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Branch, diff, and service summary stay on one clipped line so a
        // workspace row keeps its compact 44px rhythm.
        let has_branch = !ws.git_branch.is_empty();
        let diff_summary = sidebar_diff_summary(&ws.git_stats);
        let has_stats = diff_summary.is_visible();
        let service_summary = sidebar_service_summary(&ws.active_ports, &ws.service_labels);
        let has_ports = service_summary.is_some();
        if has_branch || has_stats || has_ports {
            let mut meta_row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
                .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
                .h(px(14.))
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_xs()
                .text_color(ui.muted);

            if has_branch {
                let branch_width = match (has_stats, has_ports) {
                    (true, true) => 64.0,
                    (true, false) => 112.0,
                    (false, true) => 126.0,
                    (false, false) => SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH,
                };
                meta_row = meta_row.child(
                    div()
                        .min_w_0()
                        .max_w(px(branch_width))
                        .overflow_x_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            svg()
                                .size(px(10.))
                                .flex_none()
                                .path("icons/git-branch-sidebar.svg")
                                .text_color(ui.muted),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(ws.git_branch.clone()),
                        ),
                );
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

            if let Some(service) = service_summary {
                let port = service.primary;
                let info = ws.service_labels.get(&port);
                let is_frontend = info.is_some_and(|service| service.is_frontend);
                let service_name = info
                    .and_then(|service| service.label.clone())
                    .unwrap_or_else(|| "Local service".to_string());
                let service_tooltip: SharedString = format!("{service_name}  :{port}").into();

                if is_frontend {
                    let url = info
                        .and_then(|service| service.url.clone())
                        .unwrap_or_else(|| format!("http://localhost:{port}"));
                    meta_row = meta_row.child(
                        div()
                            .id(SharedString::from(format!("port-{idx}-{port}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(2.))
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.muted)
                            .hover(|style| style.text_color(crate::theme::ui_colors().text))
                            .tooltip({
                                let label = service_tooltip.clone();
                                move |_w, cx| {
                                    cx.new(|_| SidebarTooltip {
                                        label: label.clone(),
                                    })
                                    .into()
                                }
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.open_workspace_service_url(&url, cx);
                                cx.stop_propagation();
                            }))
                            .child(
                                svg()
                                    .size(px(10.))
                                    .flex_none()
                                    .path("icons/world.svg")
                                    .text_color(ui.muted),
                            )
                            .child(format!(":{port}")),
                    );
                } else {
                    meta_row = meta_row.child(
                        div()
                            .id(SharedString::from(format!("port-{idx}-{port}-info")))
                            .text_size(px(10.))
                            .text_color(ui.muted)
                            .tooltip({
                                let label = service_tooltip.clone();
                                move |_w, cx| {
                                    cx.new(|_| SidebarTooltip {
                                        label: label.clone(),
                                    })
                                    .into()
                                }
                            })
                            .child(format!(":{port}")),
                    );
                }

                if service.overflow > 0 {
                    let overflow = service.overflow;
                    meta_row = meta_row.child(
                        div()
                            .id(SharedString::from(format!("ports-{idx}-overflow")))
                            .flex_none()
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.muted)
                            .tooltip(move |_w, cx| {
                                cx.new(|_| SidebarTooltip {
                                    label: format!(
                                        "{overflow} more services · Right-click workspace to view"
                                    )
                                    .into(),
                                })
                                .into()
                            })
                            .child(format!("+{overflow}")),
                    );
                }
            }

            Some(meta_row.into_any_element())
        } else {
            None
        }
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

fn sidebar_agent_status_tooltip(
    summary: SidebarAgentSummary,
    status: &ai_types::WorkspaceAgentStatus,
) -> SharedString {
    let state = summary.tooltip_state();
    if summary.state == SidebarAgentState::Finished {
        return state.into();
    }

    let mut details: Vec<String> = status
        .hooked
        .iter()
        .map(|aggregate| {
            format!(
                "{}{}",
                aggregate.tool.display_name(),
                aggregate.extra_suffix()
            )
        })
        .chain(
            status
                .unhooked
                .iter()
                .map(|tool| format!("{} running", tool.display_name())),
        )
        .collect();
    for label in &status.active_labels {
        if !details.iter().any(|detail| detail.starts_with(label)) {
            details.push(label.clone());
        }
    }

    if details.is_empty() {
        state.into()
    } else {
        format!("{state} · {}", details.join(", ")).into()
    }
}

fn render_workspace_agent_summary(
    summary: SidebarAgentSummary,
    workspace_id: u64,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (color, glyph, label): (gpui::Hsla, AnyElement, Option<String>) = match summary.state {
        SidebarAgentState::NeedsInput => (
            rgb(0xFBBF24).into(),
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/bell.svg")
                .text_color(rgb(0xFBBF24))
                .into_any_element(),
            Some(if summary.count > 1 {
                format!("Input {}", summary.count)
            } else {
                "Input".to_string()
            }),
        ),
        SidebarAgentState::Errored => (
            ui.agent_error,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/x_circle.svg")
                .text_color(ui.agent_error)
                .into_any_element(),
            (summary.count > 1).then(|| summary.count.to_string()),
        ),
        SidebarAgentState::Stalled => (
            ui.agent_stalled,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/triangle-alert.svg")
                .text_color(ui.agent_stalled)
                .into_any_element(),
            (summary.count > 1).then(|| summary.count.to_string()),
        ),
        SidebarAgentState::Thinking => {
            let color = ui.muted;
            (
                color,
                render_comet_trail_loader(workspace_id, color),
                (summary.count > 1).then(|| summary.count.to_string()),
            )
        }
        SidebarAgentState::Finished => {
            let color: gpui::Hsla = rgb(0x83C3FF).into();
            (
                color,
                div()
                    .size(px(11.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(7.)).rounded_full().bg(color))
                    .into_any_element(),
                None,
            )
        }
    };

    div()
        .id(SharedString::from(format!(
            "workspace-agent-status-{workspace_id}"
        )))
        .w(px(summary.slot_width()))
        .h(px(20.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(3.))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(glyph)
        .when_some(label, |d, label| d.child(label))
        .into_any_element()
}

/// Compact GPUI adaptation of Dot Matrix's `Comet Trail` loader. The 3x3
/// perimeter leaves room for larger dots while keeping the native sidebar free
/// of a web runtime, glow, or accent color.
fn render_comet_trail_loader(workspace_id: u64, color: gpui::Hsla) -> AnyElement {
    static SYNC_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

    const MATRIX_SIZE: usize = 3;
    const DOT_SIZE: f32 = 3.0;
    const DOT_GAP: f32 = 1.0;
    const CYCLE_MS: u64 = 720;
    const PERIMETER: usize = 8;
    const BASE_OPACITY: f32 = 0.06;
    const TAIL_OPACITIES: [f32; 3] = [0.8144, 0.4864, 0.2568];

    div()
        .size(px(11.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(DOT_GAP))
        .with_animation(
            SharedString::from(format!("workspace-comet-trail-{workspace_id}")),
            Animation::new(std::time::Duration::from_millis(CYCLE_MS)).repeat(),
            move |loader, _delta| {
                let cycle_elapsed = SYNC_EPOCH
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_millis()
                    % u128::from(CYCLE_MS);
                let head = (cycle_elapsed * PERIMETER as u128 / u128::from(CYCLE_MS)) as usize;

                loader.children((0..MATRIX_SIZE).map(|row| {
                    div()
                        .h(px(DOT_SIZE))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .gap(px(DOT_GAP))
                        .children((0..MATRIX_SIZE).map(move |col| {
                            let order = match (row, col) {
                                (0, 0) => Some(0),
                                (0, 1) => Some(1),
                                (0, 2) => Some(2),
                                (1, 2) => Some(3),
                                (2, 2) => Some(4),
                                (2, 1) => Some(5),
                                (2, 0) => Some(6),
                                (1, 0) => Some(7),
                                _ => None,
                            };
                            let opacity = order.map_or_else(
                                || if head.is_multiple_of(2) { 0.1 } else { 0.18 },
                                |order| {
                                    let trail = (head + PERIMETER - order) % PERIMETER;
                                    TAIL_OPACITIES.get(trail).copied().unwrap_or(BASE_OPACITY)
                                },
                            );

                            div()
                                .size(px(DOT_SIZE))
                                .flex_none()
                                .rounded_full()
                                .bg(color.opacity(opacity))
                        }))
                }))
            },
        )
        .into_any_element()
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
        SIDEBAR_AGENT_ICON_SLOT_WIDTH, SIDEBAR_AGENT_STATUS_SLOT_WIDTH, SIDEBAR_TITLE_ROW_GAP,
        SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH, SidebarAgentState, SidebarAgentSummary,
        SidebarDiffSummary, SidebarServiceSummary, collapse_home, sidebar_agent_summary,
        sidebar_diff_summary, sidebar_file_change_label, sidebar_service_summary,
        sidebar_workspace_title_slot_width, visible_service_ports,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::terminal::ServiceInfo;
    use crate::workspace::GitDiffStats;
    use std::collections::HashMap;

    fn session(state: AgentState) -> AgentSession {
        AgentSession::new(TerminalAgent::ClaudeCode, state)
    }

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
    fn sidebar_service_summary_prefers_frontend_and_counts_overflow() {
        let labels = HashMap::from([
            (
                3000,
                ServiceInfo {
                    port: 3000,
                    url: Some("http://localhost:3000".to_string()),
                    label: Some("API".to_string()),
                    is_frontend: false,
                },
            ),
            (
                5173,
                ServiceInfo {
                    port: 5173,
                    url: Some("http://localhost:5173".to_string()),
                    label: Some("Vite".to_string()),
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
            sidebar_service_summary(&[3000, 53154, 5173, 8000], &labels),
            Some(SidebarServiceSummary {
                primary: 5173,
                overflow: 2,
            })
        );
    }

    #[test]
    fn sidebar_service_summary_falls_back_to_first_visible_service() {
        let labels = HashMap::from([
            (
                3000,
                ServiceInfo {
                    port: 3000,
                    url: None,
                    label: Some("API".to_string()),
                    is_frontend: false,
                },
            ),
            (
                8000,
                ServiceInfo {
                    port: 8000,
                    url: None,
                    label: Some("Worker".to_string()),
                    is_frontend: false,
                },
            ),
        ]);

        assert_eq!(
            sidebar_service_summary(&[3000, 8000], &labels),
            Some(SidebarServiceSummary {
                primary: 3000,
                overflow: 1,
            })
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

    #[test]
    fn sidebar_agent_summary_hides_idle_without_signal() {
        assert_eq!(sidebar_agent_summary(std::iter::empty(), false), None);
    }

    #[test]
    fn sidebar_agent_summary_counts_winning_needs_input_sessions() {
        let sessions = [
            session(AgentState::WaitingForInput),
            session(AgentState::Errored),
            session(AgentState::WaitingForInput),
        ];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 2
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_applies_sidebar_priority() {
        let cases = [
            (
                vec![AgentState::Finished, AgentState::Thinking],
                SidebarAgentState::Thinking,
            ),
            (
                vec![AgentState::Thinking, AgentState::Stalled],
                SidebarAgentState::Stalled,
            ),
            (
                vec![AgentState::Stalled, AgentState::Errored],
                SidebarAgentState::Errored,
            ),
            (
                vec![AgentState::Errored, AgentState::WaitingForInput],
                SidebarAgentState::NeedsInput,
            ),
        ];
        for (states, expected) in cases {
            let sessions: Vec<_> = states.into_iter().map(session).collect();
            assert_eq!(
                sidebar_agent_summary(sessions.iter(), false).map(|summary| summary.state),
                Some(expected)
            );
        }
    }

    #[test]
    fn sidebar_agent_summary_surfaces_unread_completion_without_live_session() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), true),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 1
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_hides_acknowledged_finished_session() {
        let sessions = [session(AgentState::Finished)];
        assert_eq!(sidebar_agent_summary(sessions.iter(), false), None);
    }

    #[test]
    fn sidebar_workspace_title_slot_width_reserves_agent_status() {
        assert_eq!(
            sidebar_workspace_title_slot_width(None),
            SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH
        );

        let needs_input = Some(SidebarAgentSummary {
            state: SidebarAgentState::NeedsInput,
            count: 1,
        });
        assert_eq!(
            sidebar_workspace_title_slot_width(needs_input),
            SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH
                - SIDEBAR_TITLE_ROW_GAP
                - SIDEBAR_AGENT_STATUS_SLOT_WIDTH
        );

        let unread_completion = Some(SidebarAgentSummary {
            state: SidebarAgentState::Finished,
            count: 1,
        });
        assert_eq!(
            sidebar_workspace_title_slot_width(unread_completion),
            SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH
                - SIDEBAR_TITLE_ROW_GAP
                - SIDEBAR_AGENT_ICON_SLOT_WIDTH
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn cwd_outside_home_is_unchanged() {
        assert_eq!(collapse_home("/etc/hosts", "/home/arthur"), "/etc/hosts");
    }
}

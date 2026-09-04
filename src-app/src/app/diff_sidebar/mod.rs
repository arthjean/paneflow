use crate::PaneFlowApp;
use crate::app::diff_view_actions::DIFF_SIDEBAR_WIDTH;
use crate::diff::{FileEntry, FileListState, aggregate_file_lists};
use crate::theme::UiColors;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, Styled, Window, div, prelude::*, px,
};
use std::collections::BTreeMap;

mod header;
mod rows;

#[derive(Default)]
struct DirNode {
    subdirs: BTreeMap<String, DirNode>,
    files: Vec<usize>,
}

const REVIEW_SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
const REVIEW_SIDEBAR_ROW_PADDING_X: f32 = 8.0;
const REVIEW_SIDEBAR_ROW_RADIUS: f32 = 8.0;
const REVIEW_SIDEBAR_LIST_GAP: f32 = 4.0;

impl PaneFlowApp {
    pub(crate) fn render_diff_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();

        div()
            .relative()
            .w(px(DIFF_SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .flex()
            .flex_col()
            .child(self.render_diff_files(ui, cx))
            .child(self.render_sidebar_settings_footer(cx))
            .into_any_element()
    }

    fn render_diff_files(&self, ui: UiColors, cx: &mut Context<Self>) -> AnyElement {
        let mounted = match self.diff_mode.diff_scope {
            crate::diff::DiffScope::MultiProject => self.diff_mode.multi_diff_view.is_some(),
            _ => self.diff_mode.diff_view.is_some(),
        };
        if !mounted {
            return crate::ui_primitives::panel_empty_state(
                ui,
                Some("icons/git-branch.svg"),
                Some("No Git repository".into()),
                "Open a workspace backed by a Git repo to review its changes here.",
                false,
            )
            .into_any_element();
        }

        let (lists, selected_col): (
            Vec<(String, usize, std::path::PathBuf, FileListState)>,
            usize,
        ) = match self.diff_mode.diff_scope {
            crate::diff::DiffScope::MultiProject => self
                .diff_mode
                .multi_diff_view
                .as_ref()
                .map(|v| {
                    let v = v.read(cx);
                    (v.active_column_file_lists(cx), v.active_selected_column(cx))
                })
                .unwrap_or_default(),
            _ => self
                .diff_mode
                .diff_view
                .as_ref()
                .map(|v| {
                    let v = v.read(cx);
                    (v.column_file_lists(), v.selected_column())
                })
                .unwrap_or_default(),
        };

        if lists.is_empty() {
            let branch = self
                .workspaces
                .get(self.active_idx)
                .map(|ws| ws.git_branch.clone())
                .filter(|b| !b.is_empty());
            let msg = if self.diff_mode.diff_discovering {
                "Discovering worktrees…".to_string()
            } else {
                match branch {
                    Some(b) => format!("Computing diff for {b}…"),
                    None => "Computing diff…".to_string(),
                }
            };
            return crate::ui_primitives::panel_empty_state(
                ui,
                Some("icons/loader-circle.svg"),
                None,
                msg,
                true,
            )
            .into_any_element();
        }

        let collapsed = self.diff_mode.diff_files_collapsed;

        let has_files = lists
            .iter()
            .any(|(_, _, _, st)| matches!(st, FileListState::Loaded(f) if !f.is_empty()));

        let (_, _, total_added, total_removed) = aggregate_file_lists(&lists);

        let filter_lc = self
            .diff_mode
            .diff_file_filter
            .read(cx)
            .value()
            .to_lowercase();
        let filtering = !filter_lc.is_empty();

        let header = self.render_diff_files_header(ui, collapsed, total_added, total_removed, cx);

        let filter_field = crate::ui_primitives::filter_pill_with_arrow_clear(
            "diff-files-filter",
            "diff-files-filter-clear",
            ui,
            self.diff_mode.diff_file_filter.clone(),
            filtering,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.diff_mode.diff_file_filter.update(cx, |inp, cx| {
                    inp.clear(cx);
                });
            }),
        )
        .mx(px(8.))
        .mt(px(4.))
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
            if ev.keystroke.key.as_str() == "escape" {
                this.diff_mode.diff_file_filter.update(cx, |inp, cx| {
                    inp.clear(cx);
                });
                cx.stop_propagation();
            }
        }));

        let body: Vec<AnyElement> = if collapsed {
            Vec::new()
        } else if lists.len() == 1 {
            match lists.first() {
                Some((_, col_idx, _, st)) => {
                    self.render_diff_file_rows(*col_idx, true, st, &filter_lc, ui, cx)
                }
                None => Vec::new(),
            }
        } else {
            lists
                .iter()
                .map(|(branch, col_idx, path, st)| {
                    self.render_diff_branch_section(
                        branch,
                        &path.to_string_lossy(),
                        *col_idx,
                        *col_idx == selected_col,
                        st,
                        &filter_lc,
                        ui,
                        cx,
                    )
                })
                .collect()
        };

        let mut container = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header);
        if !collapsed && has_files {
            container = container.child(filter_field);
        }
        container
            .child(
                div()
                    .id("diff-files-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(REVIEW_SIDEBAR_LIST_GAP))
                    .pt(px(REVIEW_SIDEBAR_LIST_GAP))
                    .pb(px(8.))
                    .children(body),
            )
            .into_any_element()
    }

    fn render_diff_file_rows(
        &self,
        col_idx: usize,
        is_active: bool,
        state: &FileListState,
        filter_lc: &str,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let filtering = !filter_lc.is_empty();
        let note = |msg: String| {
            div()
                .mx(px(REVIEW_SIDEBAR_ROW_MARGIN_X))
                .px(px(REVIEW_SIDEBAR_ROW_PADDING_X))
                .py(px(8.))
                .text_color(ui.muted)
                .text_size(crate::ui_primitives::BODY)
                .child(msg)
                .into_any_element()
        };
        match state {
            FileListState::Loading => vec![note("Computing diff…".into())],
            FileListState::Failed(e) => vec![note(e.clone())],
            FileListState::Loaded(files) => {
                let visible: Vec<&FileEntry> = files
                    .iter()
                    .filter(|e| !filtering || e.path.to_lowercase().contains(filter_lc))
                    .collect();
                if visible.is_empty() {
                    vec![note(
                        if filtering {
                            "No files match your filter"
                        } else {
                            "No changes"
                        }
                        .into(),
                    )]
                } else if self.diff_mode.diff_files_tree {
                    self.render_diff_file_tree(col_idx, is_active, &visible, ui, cx)
                } else {
                    visible
                        .iter()
                        .map(|&e| self.render_diff_file_row(e, col_idx, is_active, 0.0, ui, cx))
                        .collect()
                }
            }
        }
    }

    fn render_diff_file_tree(
        &self,
        col_idx: usize,
        is_active: bool,
        visible: &[&FileEntry],
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut root = DirNode::default();
        for (i, e) in visible.iter().enumerate() {
            let mut node = &mut root;
            let mut segs = e.path.split('/').peekable();
            while let Some(seg) = segs.next() {
                if segs.peek().is_none() {
                    node.files.push(i);
                } else {
                    node = node.subdirs.entry(seg.to_string()).or_default();
                }
            }
        }
        let mut out = Vec::new();
        self.render_dir_node(&root, "", 0, col_idx, is_active, visible, ui, cx, &mut out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn render_dir_node(
        &self,
        node: &DirNode,
        prefix: &str,
        depth: usize,
        col_idx: usize,
        is_active: bool,
        visible: &[&FileEntry],
        ui: UiColors,
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
    ) {
        const INDENT: f32 = 12.0;
        for (name, child) in &node.subdirs {
            let mut disp = name.clone();
            let mut full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let mut cur = child;
            while cur.files.is_empty() && cur.subdirs.len() == 1 {
                let Some((sn, sc)) = cur.subdirs.iter().next() else {
                    break;
                };
                disp = format!("{disp}/{sn}");
                full = format!("{full}/{sn}");
                cur = sc;
            }
            let key = format!("{col_idx}\u{0}{full}");
            let collapsed = self.diff_mode.diff_collapsed_dirs.contains(&key);
            out.push(self.render_dir_header_row(col_idx, &disp, &full, collapsed, depth, ui, cx));
            if !collapsed {
                self.render_dir_node(
                    cur,
                    &full,
                    depth + 1,
                    col_idx,
                    is_active,
                    visible,
                    ui,
                    cx,
                    out,
                );
            }
        }
        for &fi in &node.files {
            out.push(self.render_diff_file_row(
                visible[fi],
                col_idx,
                is_active,
                depth as f32 * INDENT,
                ui,
                cx,
            ));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_dir_header_row(
        &self,
        col_idx: usize,
        disp: &str,
        full: &str,
        collapsed: bool,
        depth: usize,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const INDENT: f32 = 12.0;
        let key = format!("{col_idx}\u{0}{full}");
        let hover_background = crate::app::constants::sidebar_tab_active_background();
        div()
            .id(SharedString::from(format!("diff-dir-{col_idx}-{full}")))
            .flex_none()
            .h(px(28.))
            .mx(px(REVIEW_SIDEBAR_ROW_MARGIN_X))
            .pl(px(REVIEW_SIDEBAR_ROW_PADDING_X + depth as f32 * INDENT))
            .pr(px(REVIEW_SIDEBAR_ROW_PADDING_X))
            .rounded(px(REVIEW_SIDEBAR_ROW_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .hover(|s| s.bg(hover_background))
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                if !this.diff_mode.diff_collapsed_dirs.remove(&key) {
                    this.diff_mode.diff_collapsed_dirs.insert(key.clone());
                }
                cx.notify();
            }))
            .child(
                gpui::svg()
                    .size(px(10.))
                    .flex_none()
                    .text_color(ui.muted)
                    .path(if collapsed {
                        "icons/chevron-right.svg"
                    } else {
                        "icons/chevron-down.svg"
                    }),
            )
            .child(
                gpui::svg()
                    .size(px(12.))
                    .flex_none()
                    .path(if collapsed {
                        "icons/folder.svg"
                    } else {
                        "icons/folder-open.svg"
                    })
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .child(disp.to_string()),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_diff_branch_section(
        &self,
        branch: &str,
        collapse_key: &str,
        col_idx: usize,
        is_active: bool,
        state: &FileListState,
        filter_lc: &str,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let section_collapsed = self
            .diff_mode
            .diff_collapsed_branches
            .contains(collapse_key);
        let (added, removed, count) = match state {
            FileListState::Loaded(files) => {
                let (a, r) = files
                    .iter()
                    .fold((0u32, 0u32), |(a, r), f| (a + f.added, r + f.removed));
                (a, r, files.len())
            }
            _ => (0, 0, 0),
        };
        let key_owned = collapse_key.to_string();
        let hover_background = crate::app::constants::sidebar_tab_active_background();
        let resting_background = if is_active {
            crate::app::constants::sidebar_tab_active_background()
        } else {
            hover_background.opacity(0.0)
        };
        let sub_header = div()
            .id(SharedString::from(format!("diff-branch-{col_idx}")))
            .flex_none()
            .h(px(28.))
            .mx(px(REVIEW_SIDEBAR_ROW_MARGIN_X))
            .px(px(REVIEW_SIDEBAR_ROW_PADDING_X))
            .rounded(px(REVIEW_SIDEBAR_ROW_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .bg(resting_background)
            .hover(|s| s.bg(hover_background))
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                if !this.diff_mode.diff_collapsed_branches.remove(&key_owned) {
                    this.diff_mode
                        .diff_collapsed_branches
                        .insert(key_owned.clone());
                }
                cx.notify();
            }))
            .child(
                gpui::svg()
                    .size(px(10.))
                    .flex_none()
                    .text_color(ui.muted)
                    .path(if section_collapsed {
                        "icons/chevron-right.svg"
                    } else {
                        "icons/chevron-down.svg"
                    }),
            )
            .child(
                gpui::svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/git-branch.svg")
                    .text_color(if is_active { ui.accent } else { ui.muted }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::ui_primitives::BODY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if is_active { ui.accent } else { ui.text })
                    .child(branch.to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui_primitives::LABEL_XS)
                    .text_color(ui.muted)
                    .child(format!("{count}")),
            )
            .when(added > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(crate::ui_primitives::LABEL_XS)
                        .text_color(ui.diff_colors().added)
                        .child(format!("+{added}")),
                )
            })
            .when(removed > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(crate::ui_primitives::LABEL_XS)
                        .text_color(ui.diff_colors().deleted)
                        .child(format!("-{removed}")),
                )
            });

        let mut section = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(REVIEW_SIDEBAR_LIST_GAP))
            .child(sub_header);
        if !section_collapsed {
            section = section
                .children(self.render_diff_file_rows(col_idx, is_active, state, filter_lc, ui, cx));
        }
        section.into_any_element()
    }
}

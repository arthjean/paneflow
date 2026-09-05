use crate::PaneFlowApp;
use crate::app::review::REVIEW_CHANGES_RAIL_WIDTH;
use crate::diff::{DiffView, FileEntry, FileListState};
use crate::theme::UiColors;
use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
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
    pub(crate) fn render_review_changes_rail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();

        div()
            .relative()
            .w(px(REVIEW_CHANGES_RAIL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .border_l_1()
            .border_color(ui.text.opacity(0.06))
            .flex()
            .flex_col()
            .child(self.render_diff_files(ui, window, cx))
            .into_any_element()
    }

    fn render_diff_files(
        &self,
        ui: UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.review_focused_view(cx) else {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.render_diff_files_header(ui, cx))
                .child(crate::ui_primitives::panel_empty_state(
                    ui,
                    Some("icons/git-branch.svg"),
                    Some("Choose a branch".into()),
                    "Pick a branch or worktree in the Workspaces rail to list its changes here.",
                    false,
                ))
                .into_any_element();
        };

        let state = view.read(cx).file_list();
        let has_files = matches!(&state, FileListState::Loaded(files) if !files.is_empty());

        let filter_lc = self.review.file_filter.read(cx).value().to_lowercase();
        let filtering = !filter_lc.is_empty();

        let header = self.render_diff_files_header(ui, cx);
        let controls = self.render_diff_controls(&view, ui, window, cx);

        let filter_field = crate::ui_primitives::filter_pill_with_arrow_clear(
            "diff-files-filter",
            "diff-files-filter-clear",
            ui,
            self.review.file_filter.clone(),
            filtering,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.review.file_filter.update(cx, |inp, cx| {
                    inp.clear(cx);
                });
            }),
        )
        .mx(px(8.))
        .mt(px(4.))
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
            if ev.keystroke.key.as_str() == "escape" {
                this.review.file_filter.update(cx, |inp, cx| {
                    inp.clear(cx);
                });
                cx.stop_propagation();
            }
        }));

        let body = self.render_diff_file_rows(&view, &state, &filter_lc, ui, cx);

        let mut container = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(controls);
        if has_files {
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
        view: &Entity<DiffView>,
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
                } else if self.review.files_tree {
                    self.render_diff_file_tree(view, &visible, ui, cx)
                } else {
                    visible
                        .iter()
                        .map(|&e| self.render_diff_file_row(view, e, 0.0, ui, cx))
                        .collect()
                }
            }
        }
    }

    fn render_diff_file_tree(
        &self,
        view: &Entity<DiffView>,
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
        self.render_dir_node(&root, "", 0, view, visible, ui, cx, &mut out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn render_dir_node(
        &self,
        node: &DirNode,
        prefix: &str,
        depth: usize,
        view: &Entity<DiffView>,
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
            let collapsed = self.review.collapsed_dirs.contains(&full);
            out.push(self.render_dir_header_row(&disp, &full, collapsed, depth, ui, cx));
            if !collapsed {
                self.render_dir_node(cur, &full, depth + 1, view, visible, ui, cx, out);
            }
        }
        for &fi in &node.files {
            out.push(self.render_diff_file_row(view, visible[fi], depth as f32 * INDENT, ui, cx));
        }
    }

    fn render_dir_header_row(
        &self,
        disp: &str,
        full: &str,
        collapsed: bool,
        depth: usize,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const INDENT: f32 = 12.0;
        let key = full.to_string();
        let hover_background = crate::app::constants::sidebar_tab_active_background();
        div()
            .id(SharedString::from(format!("diff-dir-{full}")))
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
                if !this.review.collapsed_dirs.remove(&key) {
                    this.review.collapsed_dirs.insert(key.clone());
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
}

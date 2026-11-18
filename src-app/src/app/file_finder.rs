use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, deferred, div, img, prelude::FluentBuilder, px, svg,
    uniform_list,
};

use crate::PaneFlowApp;
use crate::settings::components::{MENU_MAX_HEIGHT, select_item_shaped};
use crate::terminal::project_files::{
    PROJECT_FILES_CAP, ProjectFiles, rank_project_files, walk_project_files,
};
use crate::ui_primitives::interface_style::with_rounded_shape;
use crate::ui_primitives::quick_pick::{
    self, FIELD_GAP, FIELD_INSET, ICON_GAP, ICON_SIZE, LABEL_SIZE, ROW_GAP, ROW_HEIGHT, ROW_RADIUS,
    ROW_SPACING, ROW_TEXT_INSET, highlighted_label,
};
use crate::ui_primitives::{FilterFieldStyle, filter_field};

pub(crate) const FILE_FINDER_LIMIT: usize = 200;
const FINDER_WIDTH: f32 = 560.;
const FINDER_TOP_MARGIN: f32 = 96.;
const FINDER_CHROME: f32 = 64.;
const FINDER_BOTTOM_GAP: f32 = 16.;
const NO_MATCHES: &str = "No matching files.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenRecord {
    count: u32,
    last: Instant,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FileOpenHistory(HashMap<PathBuf, OpenRecord>);

impl FileOpenHistory {
    pub(crate) fn record(&mut self, path: &Path, at: Instant) {
        let record = self
            .0
            .entry(path.to_path_buf())
            .or_insert(OpenRecord { count: 0, last: at });
        record.count = record.count.saturating_add(1);
        record.last = at;
    }

    pub(crate) fn ranked(&self, root: &Path, now: Instant, limit: usize) -> Vec<PathBuf> {
        let mut entries: Vec<(&PathBuf, &OpenRecord)> = self
            .0
            .iter()
            .filter(|(path, _)| path.starts_with(root))
            .collect();
        entries.sort_by(|(left_path, left), (right_path, right)| {
            frecency(right, now)
                .cmp(&frecency(left, now))
                .then_with(|| right.last.cmp(&left.last))
                .then_with(|| left_path.cmp(right_path))
        });
        entries
            .into_iter()
            .take(limit)
            .map(|(path, _)| path.clone())
            .collect()
    }
}

fn frecency(record: &OpenRecord, now: Instant) -> u64 {
    let age = now.saturating_duration_since(record.last);
    let weight = if age < Duration::from_secs(60 * 60) {
        8
    } else if age < Duration::from_secs(24 * 60 * 60) {
        4
    } else if age < Duration::from_secs(7 * 24 * 60 * 60) {
        2
    } else {
        1
    };
    u64::from(record.count) * weight
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FinderHit {
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) highlights: Vec<Range<usize>>,
}

pub(crate) struct FileFinderState {
    root: PathBuf,
    files: Option<Arc<ProjectFiles>>,
    hits: Rc<Vec<FinderHit>>,
    selected: usize,
    query: String,
    generation: u64,
    searching: bool,
    return_focus: crate::FocusReturn,
    scroll: UniformListScrollHandle,
    cancelled: Arc<AtomicBool>,
}

impl Drop for FileFinderState {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn finder_label(root: &Path, path: &Path) -> String {
    crate::app::files_tree::workspace_relative_path(root, path)
}

pub(crate) fn finder_status(
    query: &str,
    files: Option<&ProjectFiles>,
    searching: bool,
    hits: usize,
) -> Option<String> {
    if !query.is_empty() && hits == 0 && !searching && files.is_some() {
        return Some(NO_MATCHES.to_string());
    }
    if files.is_some_and(|files| !files.complete) {
        return Some(format!(
            "Showing the first {} files.",
            group_thousands(PROJECT_FILES_CAP)
        ));
    }
    if !query.is_empty() && files.is_none() {
        return Some("Indexing files…".to_string());
    }
    None
}

pub(crate) fn finder_list_height(rows: usize, viewport_height: f32) -> f32 {
    let row = ROW_HEIGHT + ROW_SPACING;
    let room = (viewport_height - FINDER_TOP_MARGIN - FINDER_CHROME - FINDER_BOTTOM_GAP).max(row);
    (rows as f32 * row)
        .min(f32::from(MENU_MAX_HEIGHT))
        .min(room)
}

fn group_thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

impl PaneFlowApp {
    pub(crate) fn record_dock_file_open(&mut self, path: &Path) {
        self.dock_file_opens.record(path, Instant::now());
    }

    pub(crate) fn handle_open_file_finder(
        &mut self,
        _: &crate::OpenFileFinder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_finder(window, cx);
    }

    pub(crate) fn open_file_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_finder.is_some() {
            self.dismiss_file_finder(window, cx);
            return;
        }
        let Some(root) = self
            .active_workspace()
            .map(|ws| PathBuf::from(&ws.cwd))
            .filter(|root| !root.as_os_str().is_empty())
        else {
            return;
        };
        self.dismiss_transient_surfaces();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.file_finder = Some(FileFinderState {
            root: root.clone(),
            files: None,
            hits: Rc::new(Vec::new()),
            selected: 0,
            query: String::new(),
            generation: 0,
            searching: false,
            return_focus: crate::FocusReturn::capture(window, cx),
            scroll: UniformListScrollHandle::new(),
            cancelled: cancelled.clone(),
        });
        self.file_finder_input
            .update(cx, |input, cx| input.clear(cx));
        let focus = self.file_finder_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        self.refresh_file_finder_hits(cx);
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let walked = {
                    let root = root.clone();
                    smol::unblock(move || walk_project_files(&root, &cancelled)).await
                };
                let _ = this.update(cx, |app, cx| {
                    let Some(state) = app.file_finder.as_mut() else {
                        return;
                    };
                    if state.root != root {
                        return;
                    }
                    state.files = walked.map(Arc::new);
                    app.refresh_file_finder_hits(cx);
                });
            },
        )
        .detach();
        cx.notify();
    }

    pub(crate) fn file_finder_query_changed(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(state) = self.file_finder.as_mut() else {
            return;
        };
        if state.query == query {
            return;
        }
        state.query = query;
        state.selected = 0;
        state.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.refresh_file_finder_hits(cx);
    }

    fn refresh_file_finder_hits(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.file_finder.as_mut() else {
            return;
        };
        let history = self
            .dock_file_opens
            .ranked(&state.root, Instant::now(), FILE_FINDER_LIMIT);
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        if state.query.is_empty() {
            state.searching = false;
            state.hits = Rc::new(
                history
                    .into_iter()
                    .map(|path| FinderHit {
                        label: finder_label(&state.root, &path),
                        path,
                        highlights: Vec::new(),
                    })
                    .collect(),
            );
            cx.notify();
            return;
        }
        let Some(files) = state.files.clone() else {
            state.searching = true;
            cx.notify();
            return;
        };
        state.searching = true;
        let query = state.query.clone();
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let hits = smol::unblock(move || {
                    rank_project_files(&files.paths, &query, FILE_FINDER_LIMIT)
                        .into_iter()
                        .map(|found| FinderHit {
                            path: files.root.join(&files.paths[found.index]),
                            label: files.paths[found.index].clone(),
                            highlights: found.highlights,
                        })
                        .collect::<Vec<_>>()
                })
                .await;
                let _ = this.update(cx, |app, cx| {
                    let Some(state) = app.file_finder.as_mut() else {
                        return;
                    };
                    if state.generation != generation {
                        return;
                    }
                    state.searching = false;
                    state.hits = Rc::new(hits);
                    cx.notify();
                });
            },
        )
        .detach();
    }

    pub(crate) fn dismiss_file_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.file_finder.take() else {
            return;
        };
        self.file_finder_input
            .update(cx, |input, cx| input.clear(cx));
        self.return_focus(&state.return_focus, window, cx);
        cx.notify();
    }

    fn activate_file_finder_hit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((path, root)) = self.file_finder.as_ref().and_then(|state| {
            state
                .hits
                .get(index)
                .map(|hit| (hit.path.clone(), state.root.clone()))
        }) else {
            return;
        };
        self.file_finder = None;
        self.file_finder_input
            .update(cx, |input, cx| input.clear(cx));
        self.open_file_in_diff_dock(path, root, window, cx);
        cx.notify();
    }

    fn select_file_finder_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(state) = self.file_finder.as_mut() {
            state.selected = index;
            state.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn handle_file_finder_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((len, selected)) = self
            .file_finder
            .as_ref()
            .map(|state| (state.hits.len(), state.selected))
        else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_file_finder(window, cx),
            "enter" if selected < len => self.activate_file_finder_hit(selected, window, cx),
            "up" if selected > 0 => self.select_file_finder_hit(selected - 1, cx),
            "down" if selected + 1 < len => self.select_file_finder_hit(selected + 1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_file_finder_row(
        &self,
        index: usize,
        hit: &FinderHit,
        selected: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = quick_pick::row_colors(selected, ui);
        let name = hit
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let icon = crate::file_icons::language_icon_path(&name);
        let glyph = if icon.starts_with("icons/languages/") {
            img(icon).flex_none().size(px(ICON_SIZE)).into_any_element()
        } else {
            svg()
                .flex_none()
                .size(px(ICON_SIZE))
                .path(icon)
                .text_color(colors.muted)
                .into_any_element()
        };
        select_item_shaped(
            SharedString::from(format!("file-finder-row-{index}")),
            selected,
            ui,
            colors.fill,
            ROW_RADIUS,
        )
        .h(px(ROW_HEIGHT))
        .px(px(ROW_TEXT_INSET))
        .gap(px(ROW_GAP))
        .text_size(px(LABEL_SIZE))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.activate_file_finder_hit(index, window, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(ICON_GAP))
                .child(glyph)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .text_color(colors.label)
                        .child(highlighted_label(&hit.label, &hit.highlights)),
                ),
        )
        .into_any_element()
    }

    pub(crate) fn render_file_finder(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        with_rounded_shape(|| self.build_file_finder(window, cx))
    }

    fn build_file_finder(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let Some(state) = self.file_finder.as_ref() else {
            return div().into_any_element();
        };
        let focus = self.file_finder_input.read(cx).focus_handle.clone();
        let field = filter_field(
            "file-finder-field",
            "file-finder-field-clear",
            "Search files",
            ui,
            FilterFieldStyle::palette(),
            focus.is_focused(window),
            !state.query.is_empty(),
            true,
            None,
            self.file_finder_input.clone(),
            cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.file_finder_input
                    .update(cx, |input, cx| input.clear(cx));
                let focus = this.file_finder_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }),
        )
        .flex_none()
        .mx(px(FIELD_INSET))
        .mb(px(FIELD_GAP))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .into_any_element();

        let hits = state.hits.clone();
        let selected = state.selected.min(hits.len().saturating_sub(1));
        let status = finder_status(
            &state.query,
            state.files.as_deref(),
            state.searching,
            hits.len(),
        );
        let list = div()
            .flex()
            .flex_col()
            .when(!hits.is_empty(), |list| {
                let rows = hits.clone();
                list.child(
                    uniform_list(
                        "file-finder-list",
                        rows.len(),
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            range
                                .filter_map(|index| {
                                    let hit = rows.get(index)?;
                                    Some(
                                        div().w_full().flex().flex_col().pb(px(ROW_SPACING)).child(
                                            with_rounded_shape(|| {
                                                this.render_file_finder_row(
                                                    index,
                                                    hit,
                                                    index == selected,
                                                    ui,
                                                    cx,
                                                )
                                            }),
                                        ),
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .w_full()
                    .h(px(finder_list_height(
                        hits.len(),
                        f32::from(window.viewport_size().height),
                    )))
                    .track_scroll(&state.scroll),
                )
            })
            .children(status.map(|status| {
                div()
                    .h(px(ROW_HEIGHT))
                    .px(px(ROW_TEXT_INSET))
                    .flex()
                    .items_center()
                    .text_size(px(LABEL_SIZE))
                    .text_color(ui.muted)
                    .child(status)
            }))
            .into_any_element();

        let panel = div()
            .id("file-finder")
            .occlude()
            .on_key_down(cx.listener(Self::handle_file_finder_key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_file_finder(window, cx);
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let panel = quick_pick::shell(panel, px(FINDER_WIDTH), ui, field, list);

        deferred(
            div()
                .id("file-finder-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(FINDER_TOP_MARGIN))
                .child(crate::ui_primitives::menu_reveal(
                    "file-finder-reveal",
                    panel,
                )),
        )
        .with_priority(7)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_files_rank_by_how_often_and_how_lately_they_were_opened() {
        let start = Instant::now();
        let later = |hours: u64| start + Duration::from_secs(hours * 60 * 60);
        let mut history = FileOpenHistory::default();
        for _ in 0..3 {
            history.record(Path::new("/repo/old.rs"), start);
        }
        history.record(Path::new("/repo/fresh.rs"), later(48));
        history.record(Path::new("/repo/fresh.rs"), later(48));
        history.record(
            Path::new("/repo/once.rs"),
            later(48) - Duration::from_secs(30 * 60),
        );

        history.record(Path::new("/elsewhere/other.rs"), later(48));
        let root = Path::new("/repo");
        let ranked = history.ranked(root, later(48), FILE_FINDER_LIMIT);
        assert_eq!(
            ranked,
            [
                PathBuf::from("/repo/fresh.rs"),
                PathBuf::from("/repo/once.rs"),
                PathBuf::from("/repo/old.rs"),
            ],
            "only the active workspace's files are offered"
        );
        assert_eq!(history.ranked(root, later(48), 1).len(), 1);
    }

    #[test]
    fn the_list_never_runs_past_the_bottom_of_a_short_window() {
        let row = ROW_HEIGHT + ROW_SPACING;
        assert_eq!(finder_list_height(3, 1000.), 3. * row);
        assert_eq!(
            finder_list_height(200, 1000.),
            f32::from(MENU_MAX_HEIGHT),
            "a tall window keeps the palette's maximum"
        );
        let short = finder_list_height(200, 500.);
        assert!(short + FINDER_TOP_MARGIN + FINDER_CHROME <= 500.);
        assert_eq!(
            finder_list_height(200, 100.),
            row,
            "one row stays reachable"
        );
    }

    fn files(complete: bool) -> ProjectFiles {
        ProjectFiles {
            root: PathBuf::from("/repo"),
            paths: vec!["src/main.rs".into()],
            complete,
        }
    }

    #[test]
    fn the_finder_says_when_nothing_matches_and_when_the_walk_was_capped() {
        assert_eq!(
            finder_status("zzz", Some(&files(true)), false, 0).as_deref(),
            Some("No matching files.")
        );
        assert_eq!(finder_status("zzz", Some(&files(true)), true, 0), None);
        assert_eq!(finder_status("main", Some(&files(true)), false, 1), None);
        assert_eq!(
            finder_status("main", Some(&files(false)), false, 1).as_deref(),
            Some("Showing the first 100,000 files.")
        );
        assert_eq!(
            finder_status("main", None, true, 0).as_deref(),
            Some("Indexing files…")
        );
        assert_eq!(finder_status("", None, false, 0), None);
    }
}

mod change_markers;
mod disk_sync;
mod editing;
mod input_handler;
mod keymap;
mod motion;
mod pointer;
mod render;

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::Either;
use gpui::{
    Anchor, AnyElement, App, AppContext, AsyncApp, Bounds, ClickEvent, ClipboardItem, Context,
    CursorStyle, EntityInputHandler, FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement,
    IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, StyledText, UTF16Selection, WeakEntity, Window, actions,
    anchored, deferred, div, point, px, size,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use paneflow_textdiff::{Block, BlockKind, BlockTracker, ComparisonPolicy, split_lines};
use ropey::Rope;

type WatchBridge = Arc<Mutex<Option<mpsc::UnboundedSender<notify::Result<notify::Event>>>>>;
type WatchEvents = mpsc::UnboundedReceiver<notify::Result<notify::Event>>;

use super::base::{Base, spawn_base_load};
use super::controls::EditorControls;
use super::cursor::{self, CodeSelection};
use super::document::{CodeDocument, ReadOnlyReason, normalize_newlines};
use super::edit::{self, EditGroup, IndentUnit, TrackerWindow};
#[cfg(test)]
use super::element;
use super::element::{
    CODE_FONT_SIZE, CODE_ROW_HEIGHT, CodeCaret, CodeColors, CodeElement, CodeGeometry, CodeHitMap,
    CodeScroll, GutterMemo, autoscroll_step, code_font, reveal_h_offset, reveal_rows,
    visible_rows_at,
};
use super::highlight::{
    CodeHighlighter, DeferredParse, HIGHLIGHT_FRAME_BUDGET, HighlightOutcome, SYNC_PARSE_BUDGET,
    spawn_deferred_parse,
};
use super::load::{CodeLoadSlot, CodeLoadState, CodeOpen, spawn_code_load};
use super::markers::MARKER_COLUMN_W;
use super::navigation::NavigationState;
use super::save::{self, FileStamp};
use super::spawn_blocking_then;
use crate::diff::{DiffSyntax, highlight_lines, palette};
use crate::settings::components::menu_surface;
use crate::terminal::blink::{BlinkPhaseGlobal, CURSOR_BLINK_INTERVAL};
pub(crate) use keymap::*;
use render::*;

const INITIAL_HIGHLIGHT_ROWS: usize = 60;
const TOO_COMPLEX_BANNER: &str = "This file is too complex to color.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragGrain {
    Grapheme,
    Word,
    Line,
}

#[derive(Clone, Debug)]
struct TextDrag {
    grain: DragGrain,
    anchor: Range<usize>,
}

#[derive(Clone, Copy)]
struct ClickChain {
    at: Instant,
    position: Point<Pixels>,
    count: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum DiskState {
    #[default]
    InSync,
    Conflict,
    Deleted,
}

type PopupLine = (SharedString, Vec<(Range<usize>, Hsla)>);

struct MarkerPopup {
    block: Block,
    title: String,
    shown: Vec<PopupLine>,
    hidden: usize,
    base_text: String,
}

struct DiskDiff {
    rope: Rope,
    revision: u64,
}

impl DiskDiff {
    fn of(doc: &CodeDocument) -> Self {
        Self {
            rope: doc.text().clone(),
            revision: doc.revision(),
        }
    }
}

pub(crate) struct CodeView {
    pub(crate) controls: gpui::Entity<EditorControls>,
    pub(super) navigation: NavigationState,
    path: PathBuf,
    state: CodeLoadState,
    slot: CodeLoadSlot,
    focus: FocusHandle,
    scroll: CodeScroll,
    h_offset: f32,
    selection: CodeSelection,
    goal_column: usize,
    text_drag: Option<TextDrag>,
    click_chain: Option<ClickChain>,
    last_motion: Instant,
    blink_visible: bool,
    focused: bool,
    focus_subscriptions: Option<(gpui::WindowId, gpui::Subscription, gpui::Subscription)>,
    theme_generation: u64,
    geometry: Rc<Cell<CodeGeometry>>,
    gutter_memo: Rc<Cell<GutterMemo>>,
    hits: Rc<RefCell<CodeHitMap>>,
    element_id: SharedString,
    history: edit::UndoHistory,
    saved_mark: edit::HistoryMark,
    indent: IndentUnit,
    marked: Option<Range<usize>>,
    read_only_flash: Option<Instant>,
    stamp: Option<FileStamp>,
    disk: DiskState,
    save_error: Option<String>,
    saving: bool,
    base: Base,
    tracker: BlockTracker,
    tracker_generation: u64,
    hovered_marker: Option<usize>,
    popup: Option<MarkerPopup>,
    highlight_budget: Duration,
    _watcher: Option<RecommendedWatcher>,
    _watch_bridge: Option<WatchBridge>,
}

impl CodeView {
    pub(crate) fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self::with_state(path, CodeLoadState::Loading, None, cx);
        view.observe_blink(cx);
        view.start_load(cx);
        view
    }

    fn with_state(
        path: PathBuf,
        state: CodeLoadState,
        stamp: Option<FileStamp>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let controls = EditorControls::attach(focus.clone(), cx);
        Self {
            controls,
            navigation: NavigationState::default(),
            element_id: format!("code-view:{}", path.display()).into(),
            path,
            state,
            slot: CodeLoadSlot::new(),
            focus,
            scroll: CodeScroll::new(),
            h_offset: 0.0,
            selection: CodeSelection::default(),
            goal_column: 0,
            text_drag: None,
            click_chain: None,
            last_motion: Instant::now(),
            blink_visible: true,
            focused: false,
            focus_subscriptions: None,
            theme_generation: crate::theme::theme_generation(),
            geometry: Rc::new(Cell::new(CodeGeometry::default())),
            gutter_memo: Rc::new(Cell::new(GutterMemo::default())),
            hits: Rc::new(RefCell::new(CodeHitMap::default())),
            history: edit::UndoHistory::default(),
            saved_mark: edit::HistoryMark::default(),
            indent: IndentUnit::Spaces(4),
            marked: None,
            read_only_flash: None,
            stamp,
            disk: DiskState::default(),
            save_error: None,
            saving: false,
            base: Base::None,
            tracker: BlockTracker::inactive(),
            tracker_generation: 0,
            hovered_marker: None,
            popup: None,
            highlight_budget: HIGHLIGHT_FRAME_BUDGET,
            _watcher: None,
            _watch_bridge: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn ready_for_test(path: PathBuf, text: &str, cx: &mut Context<Self>) -> Self {
        let document = super::load::build_document(path.clone(), text, false);
        let mut highlighter = CodeHighlighter::new(
            &document,
            DiffSyntax::from_theme(&crate::theme::active_theme()),
        );
        highlighter.parse_initial_blocking(&document);
        let focus = cx.focus_handle();
        let controls = EditorControls::attach(focus.clone(), cx);
        Self {
            controls,
            navigation: NavigationState::default(),
            element_id: format!("code-view:{}", path.display()).into(),
            path,
            state: CodeLoadState::Ready(Box::new(super::load::LoadedCode {
                document,
                highlighter,
                indent: IndentUnit::Spaces(4),
                stamp: None,
            })),
            slot: CodeLoadSlot::new(),
            focus,
            scroll: CodeScroll::new(),
            h_offset: 0.0,
            selection: CodeSelection::default(),
            goal_column: 0,
            text_drag: None,
            click_chain: None,
            last_motion: Instant::now(),
            blink_visible: true,
            focused: false,
            focus_subscriptions: None,
            theme_generation: crate::theme::theme_generation(),
            geometry: Rc::new(Cell::new(CodeGeometry::default())),
            gutter_memo: Rc::new(Cell::new(GutterMemo::default())),
            hits: Rc::new(RefCell::new(CodeHitMap::default())),
            history: edit::UndoHistory::default(),
            saved_mark: edit::HistoryMark::default(),
            indent: IndentUnit::Spaces(4),
            marked: None,
            read_only_flash: None,
            stamp: None,
            disk: DiskState::default(),
            save_error: None,
            saving: false,
            base: Base::None,
            tracker: BlockTracker::inactive(),
            tracker_generation: 0,
            hovered_marker: None,
            popup: None,
            highlight_budget: HIGHLIGHT_FRAME_BUDGET,
            _watcher: None,
            _watch_bridge: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn visible_row_range(&self) -> Range<usize> {
        let Some(line_count) = self.state.document().map(CodeDocument::line_count) else {
            return 0..0;
        };
        visible_rows_at(
            self.scroll.rows(),
            self.scroll.viewport_height(),
            line_count,
        )
    }

    #[cfg(test)]
    pub(crate) fn scroll_rows(&self) -> f64 {
        self.scroll.rows()
    }

    #[cfg(test)]
    pub(crate) fn scroll_offset_y(&self) -> f32 {
        self.scroll.content_top()
    }

    #[cfg(test)]
    pub(crate) fn materialized_lines(&self) -> usize {
        self.hits.borrow().materialized_lines
    }

    #[cfg(test)]
    pub(crate) fn materialized_numbers(&self) -> usize {
        self.hits.borrow().materialized_numbers
    }

    #[cfg(test)]
    pub(crate) fn row_width(&self, row: usize) -> Option<f32> {
        let hits = self.hits.borrow();
        let index = row.checked_sub(hits.first_row)?;
        Some(f32::from(hits.lines.get(index)?.as_ref()?.width()))
    }

    #[cfg(test)]
    pub(crate) fn row_top(&self, row: usize) -> f32 {
        let hits = self.hits.borrow();
        hits.top_y + row.saturating_sub(hits.first_row) as f32 * CODE_ROW_HEIGHT
    }

    #[cfg(test)]
    pub(crate) fn stale_visible_rows(&self) -> usize {
        let rows = self.visible_row_range();
        self.state
            .highlighter()
            .map(|highlighter| highlighter.stale_rows_in(rows))
            .unwrap_or(0)
    }

    fn observe_blink(&mut self, cx: &mut Context<Self>) {
        let Some(global) = cx.try_global::<BlinkPhaseGlobal>() else {
            log::warn!("BlinkPhaseGlobal not installed - the code caret will not blink");
            return;
        };
        let phase = global.0.clone();
        cx.observe(&phase, |view: &mut Self, phase, cx: &mut Context<Self>| {
            let caret_visible = view.caret_is_visible();
            if view.apply_blink_phase(phase.read(cx).visible, caret_visible) {
                cx.notify();
            }
        })
        .detach();
    }

    fn ensure_focus_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_focus_state(self.focus.is_focused(window));
        let window_id = window.window_handle().window_id();
        if self.focus_subscriptions.as_ref().map(|binding| binding.0) == Some(window_id) {
            return;
        }
        if self.focus_subscriptions.take().is_some() {
            self.text_drag = None;
            self.click_chain = None;
            self.marked = None;
        }
        let focus = self.focus.clone();
        let focus_in = cx.on_focus(&focus, window, |view, _window, cx| {
            view.sync_focus_state(true);
            cx.notify();
        });
        let focus_out = cx.on_blur(&focus, window, |view, _window, cx| {
            view.sync_focus_state(false);
            cx.notify();
        });
        self.focus_subscriptions = Some((window_id, focus_in, focus_out));
    }

    fn sync_focus_state(&mut self, focused: bool) {
        if focused && !self.focused {
            self.last_motion = Instant::now();
            self.blink_visible = true;
        }
        self.focused = focused;
    }

    fn caret_is_visible(&self) -> bool {
        if !self.focused {
            return false;
        }
        let Some(doc) = self.state.document() else {
            return false;
        };
        let viewport_h = self.scroll.viewport_height();
        if viewport_h <= 0.0 {
            return false;
        }
        let content_top = self.scroll.content_top();
        let row = doc.byte_to_line(self.selection.cursor());
        Self::row_intersects_viewport(row, content_top, viewport_h)
    }

    fn row_intersects_viewport(row: usize, content_top: f32, viewport_h: f32) -> bool {
        let row_top = row as f32 * CODE_ROW_HEIGHT;
        row_top < content_top + viewport_h && row_top + CODE_ROW_HEIGHT > content_top
    }

    fn apply_blink_phase(&mut self, phase_visible: bool, caret_visible: bool) -> bool {
        if !self.focused || !caret_visible {
            return false;
        }
        let visible = self.last_motion.elapsed() < CURSOR_BLINK_INTERVAL || phase_visible;
        if visible == self.blink_visible {
            return false;
        }
        self.blink_visible = visible;
        true
    }

    pub(crate) fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.element_id = format!("code-view:{}", path.display()).into();
        self.path = path;
        self.state = CodeLoadState::Loading;
        self.h_offset = 0.0;
        self.selection = CodeSelection::default();
        self.goal_column = 0;
        self.text_drag = None;
        self.click_chain = None;
        self.gutter_memo.set(GutterMemo::default());
        self.geometry.set(CodeGeometry::default());
        *self.hits.borrow_mut() = CodeHitMap::default();
        self.scroll.reset_rows();
        self.history.clear();
        self.saved_mark = edit::HistoryMark::default();
        self.marked = None;
        self.read_only_flash = None;
        self.stamp = None;
        self.disk = DiskState::default();
        self.save_error = None;
        self.saving = false;
        self.base = Base::None;
        self.tracker = BlockTracker::inactive();
        self.tracker_generation = self.tracker_generation.wrapping_add(1);
        self.hovered_marker = None;
        self.popup = None;
        self._watcher = None;
        self._watch_bridge = None;
        self.start_load(cx);
        cx.notify();
    }

    fn start_load(&mut self, cx: &mut Context<Self>) {
        let generation = self.slot.begin();
        let syntax = DiffSyntax::from_theme(&crate::theme::active_theme());
        self.theme_generation = crate::theme::theme_generation();
        spawn_code_load(
            self.path.clone(),
            generation,
            syntax,
            cx,
            |view: &mut Self, generation, outcome: CodeOpen, cx| {
                if !view.slot.accept(generation) {
                    return;
                }
                match outcome {
                    Ok(loaded) => {
                        view.indent = loaded.indent;
                        view.stamp = loaded.stamp;
                        view.state = CodeLoadState::Ready(Box::new(loaded));
                        view.start_initial_parse(cx);
                        view.start_base_load(cx);
                    }
                    Err(err) => {
                        view.state = CodeLoadState::Failed(err);
                    }
                }
                view.start_watcher(cx);
                cx.notify();
            },
        );
    }

    fn start_initial_parse(&mut self, cx: &mut Context<Self>) {
        let Some((doc, hl)) = self.state.editable() else {
            return;
        };
        let Some(parse) = hl.initial_parse(doc) else {
            return;
        };
        spawn_deferred_parse(parse, cx, |view: &mut Self, parsed, cx| {
            if let Some((doc, hl)) = view.state.editable()
                && hl.apply_parsed(doc, parsed)
            {
                cx.notify();
            }
        });
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn document(&self) -> Option<&CodeDocument> {
        self.state.document()
    }

    pub(crate) fn highlighter(&self) -> Option<&CodeHighlighter> {
        self.state.highlighter()
    }

    #[cfg(test)]
    pub(crate) fn cursor(&self) -> usize {
        self.selection.cursor()
    }

    #[cfg(test)]
    pub(crate) fn cursor_row(&self) -> usize {
        self.document()
            .map(|doc| doc.byte_to_line(self.selection.cursor()))
            .unwrap_or(0)
    }

    pub(crate) fn cursor_line_column(&self) -> (usize, usize) {
        let Some(doc) = self.document() else {
            return (1, 1);
        };
        let offset = self.selection.cursor();
        (
            doc.byte_to_line(offset) + 1,
            cursor::goal_column(doc, offset) + 1,
        )
    }

    #[cfg(test)]
    pub(crate) fn selection(&self) -> Range<usize> {
        self.selection.range()
    }

    fn sync_scroll_line_count(&self) {
        if let Some(doc) = self.state.document() {
            self.scroll.set_line_count(doc.line_count());
        }
    }

    fn sync_theme(&mut self) {
        let generation = crate::theme::theme_generation();
        if generation == self.theme_generation {
            return;
        }
        self.theme_generation = generation;
        let syntax = DiffSyntax::from_theme(&crate::theme::active_theme());
        if let Some((_, hl)) = self.state.editable() {
            hl.set_syntax(syntax);
        }
    }

    fn fill_visible_highlights(&mut self, window: &mut Window) {
        let Some(line_count) = self.state.document().map(CodeDocument::line_count) else {
            return;
        };
        let viewport_h = self.scroll.viewport_height();
        let rows = if viewport_h > 0.0 {
            visible_rows_at(self.scroll.rows(), viewport_h, line_count)
        } else {
            0..INITIAL_HIGHLIGHT_ROWS.min(line_count)
        };
        let budget = self.highlight_budget;
        let minimap_rows = self
            .navigation
            .layout
            .get()
            .minimap
            .map(|_| super::minimap::visible_rows(line_count, &self.scroll));
        if let Some((doc, highlighter)) = self.state.editable() {
            let started = Instant::now();
            let mut stale = highlighter.fill_stale_rows(doc, rows, budget).any_stale();
            if let Some(rows) = minimap_rows {
                let remaining = budget.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    stale = true;
                } else {
                    stale |= highlighter
                        .fill_stale_rows(doc, rows, remaining)
                        .any_stale();
                }
            }
            if stale {
                window.request_animation_frame();
            }
        }
    }
}

impl Focusable for CodeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};

    use super::super::highlight::CodeHighlighter;
    use super::super::load::{LoadedCode, build_document};
    use super::*;

    pub(super) fn view<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        view_with_budget(cx, text, HIGHLIGHT_FRAME_BUDGET)
    }

    pub(super) fn view_with_budget<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
        budget: Duration,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        view_named(cx, "/nonexistent/paneflow-code.rs", text, budget)
    }

    pub(super) fn view_named<'a>(
        cx: &'a mut TestAppContext,
        name: &str,
        text: &str,
        highlight_budget: Duration,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        let path = PathBuf::from(name);
        let state = if text.is_empty() {
            CodeLoadState::Loading
        } else {
            let document = build_document(path.clone(), text, false);
            let mut highlighter = CodeHighlighter::new(
                &document,
                DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
            );
            highlighter.parse_initial_blocking(&document);
            CodeLoadState::Ready(Box::new(LoadedCode {
                document,
                highlighter,
                indent: IndentUnit::Spaces(4),
                stamp: None,
            }))
        };
        cx.add_window_view(move |_window, cx| {
            let mut view = CodeView::with_state(path, state, None, cx);
            view.highlight_budget = highlight_budget;
            view
        })
    }

    pub(super) fn rows_of_code(rows: usize) -> String {
        (0..rows).map(|row| format!("fn f{row}() {{}}\n")).collect()
    }

    pub(super) const VIEWPORT: Point<Pixels> = Point {
        x: px(800.),
        y: px(360.),
    };

    pub(super) fn scrolled<'a>(
        cx: &'a mut TestAppContext,
        name: &str,
        text: &str,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        let (view, cx) = view_named(cx, name, text, HIGHLIGHT_FRAME_BUDGET);
        cx.simulate_resize(size(VIEWPORT.x, VIEWPORT.y));
        cx.run_until_parked();
        let centre = point(VIEWPORT.x / 2., VIEWPORT.y / 2.);
        cx.simulate_mouse_move(centre, None, Modifiers::default());
        cx.run_until_parked();
        (view, cx)
    }

    pub(super) fn frame(view: &Entity<CodeView>, cx: &mut VisualTestContext) {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn moving_code_between_windows_rebinds_focus_without_losing_content(cx: &mut TestAppContext) {
        struct CodeHost(Option<Entity<CodeView>>);

        impl Render for CodeHost {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().children(self.0.clone())
            }
        }

        let view = cx.new(seeded_view(
            PathBuf::from("/nonexistent/moved.rs"),
            "preserved\n",
        ));
        let first_view = view.clone();
        let first_window = {
            let (host, first) = cx.add_window_view(move |_, _| CodeHost(Some(first_view)));
            first.simulate_resize(gpui::size(px(800.), px(600.)));
            first.run_until_parked();
            let handle = first.update(|window, cx| {
                view.read(cx).focus.clone().focus(window, cx);
                window.window_handle()
            });
            first.run_until_parked();
            assert!(view.read_with(first, |view, _| view.focused));
            host.update(first, |host, cx| {
                host.0 = None;
                cx.notify();
            });
            first.run_until_parked();
            handle
        };
        let second_view = view.clone();
        let (_, second) = cx.add_window_view(move |_, _| CodeHost(Some(second_view)));
        second.simulate_resize(gpui::size(px(800.), px(600.)));
        second.run_until_parked();
        second.update(|window, cx| view.read(cx).focus.clone().focus(window, cx));
        second.run_until_parked();
        second.update(|_, cx| {
            first_window
                .update(cx, |_, window, _| window.blur())
                .unwrap();
        });
        second.run_until_parked();
        view.read_with(second, |view, _| {
            assert!(view.focused);
            assert_ne!(
                view.focus_subscriptions.as_ref().unwrap().0,
                first_window.window_id()
            );
            assert_eq!(text_of(view), "preserved\n");
        });
        second.update(|window, _| window.blur());
        second.run_until_parked();
        assert!(!view.read_with(second, |view, _| view.focused));
    }
    #[gpui::test]
    fn blink_phase_is_ignored_while_the_view_is_unfocused(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo\n");

        view.update(cx, |view, _cx| {
            view.focused = false;
            view.last_motion = Instant::now() - CURSOR_BLINK_INTERVAL;
            view.blink_visible = true;
            assert!(!view.apply_blink_phase(false, true));
            assert!(view.blink_visible);
        });
    }

    #[gpui::test]
    fn blink_phase_is_ignored_while_the_caret_is_outside_the_viewport(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo\n");

        view.update(cx, |view, _cx| {
            view.focused = true;
            view.last_motion = Instant::now() - CURSOR_BLINK_INTERVAL;
            view.blink_visible = true;
            let caret_visible = CodeView::row_intersects_viewport(2, 0.0, CODE_ROW_HEIGHT * 2.0);
            assert!(!caret_visible);
            assert!(!view.apply_blink_phase(false, caret_visible));
            assert!(view.blink_visible);
        });
    }

    #[gpui::test]
    fn blink_phase_notifies_only_when_a_visible_caret_changes(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo\n");

        view.update(cx, |view, _cx| {
            view.focused = true;
            view.last_motion = Instant::now() - CURSOR_BLINK_INTERVAL;
            view.blink_visible = true;
            let caret_visible = CodeView::row_intersects_viewport(1, 0.0, CODE_ROW_HEIGHT * 2.0);
            assert!(caret_visible);
            assert!(view.apply_blink_phase(false, caret_visible));
            assert!(!view.blink_visible);
            assert!(!view.apply_blink_phase(false, caret_visible));
        });
    }

    #[gpui::test]
    fn returning_focus_makes_the_caret_visible_immediately(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo\n");

        view.update(cx, |view, _cx| {
            view.focused = false;
            view.last_motion = Instant::now() - CURSOR_BLINK_INTERVAL;
            view.blink_visible = false;
            view.sync_focus_state(true);
            assert!(view.focused);
            assert!(view.blink_visible);
            assert!(view.last_motion.elapsed() < CURSOR_BLINK_INTERVAL);
        });
    }

    pub(super) fn file_view<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
        watch: bool,
    ) -> (
        tempfile::TempDir,
        Entity<CodeView>,
        &'a mut VisualTestContext,
    ) {
        file_view_named(cx, "main.rs", text, watch)
    }

    pub(super) fn file_view_named<'a>(
        cx: &'a mut TestAppContext,
        name: &str,
        text: &str,
        watch: bool,
    ) -> (
        tempfile::TempDir,
        Entity<CodeView>,
        &'a mut VisualTestContext,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        std::fs::write(&path, text).expect("seed");
        let seeded = seeded_view(path, text);
        let (view, cx) = cx.add_window_view(move |_window, cx| {
            let mut view = seeded(cx);
            if watch {
                view.start_watcher(cx);
            }
            view
        });
        (dir, view, cx)
    }

    pub(super) fn seeded_view(
        path: PathBuf,
        text: &str,
    ) -> impl FnOnce(&mut Context<CodeView>) -> CodeView + use<> {
        let document = build_document(path.clone(), text, false);
        let mut highlighter = CodeHighlighter::new(
            &document,
            DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
        );
        highlighter.parse_initial_blocking(&document);
        let stamp = FileStamp::read(&path);
        let state = CodeLoadState::Ready(Box::new(LoadedCode {
            document,
            highlighter,
            indent: IndentUnit::Spaces(4),
            stamp,
        }));
        move |cx: &mut Context<CodeView>| CodeView::with_state(path, state, stamp, cx)
    }

    pub(super) fn text_of(view: &CodeView) -> String {
        view.document()
            .map(|doc| doc.slice_string(0..doc.len_bytes()))
            .unwrap_or_default()
    }

    #[gpui::test]
    fn opening_a_file_shows_its_text_first_and_colors_it_when_the_tree_lands(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn main() {\n    let value = 1;\n}\n").expect("seed");
        let opened = super::super::load::open_blocking(
            &path,
            DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
        )
        .expect("open");
        assert!(
            !opened.highlighter.has_tree(),
            "the blocking read hands the text over before any parse"
        );
        drop(opened);

        let spawn_path = path.clone();
        let (view, cx) = cx.add_window_view(move |_window, cx| CodeView::new(spawn_path, cx));
        cx.run_until_parked();

        view.update(cx, |view, _cx| {
            let (doc, highlighter) = view.state.editable().expect("the file loaded");
            assert_eq!(doc.line_count(), 4, "the text is there");
            assert!(
                highlighter.has_tree(),
                "the deferred initial parse landed and installed the tree"
            );
            assert!(
                !highlighter.is_too_complex(),
                "a three-line file is not too complex to color"
            );
            highlighter.requery_rows(doc, 0..doc.line_count());
            assert!(
                (0..doc.line_count()).any(|row| !highlighter.runs(row).is_empty()),
                "and the rows color from it"
            );
            let bridge = view._watch_bridge.take();
            if let Some(bridge) = bridge {
                *bridge.lock().expect("bridge lock") = None;
            }
            view._watcher = None;
        });
    }

    #[gpui::test]
    fn initial_parse_keeps_the_loaded_text_available_until_its_tree_arrives(
        cx: &mut TestAppContext,
    ) {
        let text = "fn main() {\n    let value = 1;\n}\n";
        let path = PathBuf::from("main.rs");
        let document = build_document(path.clone(), text, false);
        let highlighter = CodeHighlighter::new(
            &document,
            DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
        );
        let state = CodeLoadState::Ready(Box::new(LoadedCode {
            document,
            highlighter,
            indent: IndentUnit::Spaces(4),
            stamp: None,
        }));
        let (view, cx) =
            cx.add_window_view(move |_window, cx| CodeView::with_state(path, state, None, cx));

        view.update(cx, |view, cx| {
            view.start_initial_parse(cx);
            assert_eq!(text_of(view), text);
            assert!(!view.highlighter().expect("highlighter").has_tree());
        });
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), text);
            assert!(view.highlighter().expect("highlighter").has_tree());
        });
    }

    #[gpui::test]
    fn an_initial_parse_that_gives_up_greys_the_file_and_raises_its_banner(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = view(cx, "fn main() {\n    let value = 1;\n}\n");
        let ui = crate::theme::ui_colors();

        let before = view.update(cx, |view, cx| view.banners(ui, cx).len());

        view.update(cx, |view, _cx| {
            let (doc, highlighter) = view.state.editable().expect("ready");
            let mut fresh =
                CodeHighlighter::new(doc, DiffSyntax::from_theme(&crate::theme::paneflow_dark()));
            let expired = fresh
                .initial_parse(doc)
                .expect("a fresh highlighter defers its first parse")
                .with_timeout_for_test(Duration::ZERO);
            assert!(
                highlighter.apply_parsed(doc, expired.run()),
                "the expired parse is applied to the live highlighter"
            );
            assert!(highlighter.is_too_complex(), "the tab gave up on coloring");
            assert!(!highlighter.is_enabled(), "and the file stays grey");
        });

        let after = view.update(cx, |view, cx| view.banners(ui, cx).len());
        assert_eq!(
            after,
            before + 1,
            "giving up on the parse raises one banner under the file name"
        );
    }
}

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
use super::element::{
    CODE_FONT_SIZE, CODE_ROW_HEIGHT, CodeCaret, CodeColors, CodeElement, CodeGeometry, CodeHitMap,
    CodeScroll, GutterMemo, autoscroll_step, code_font, reveal_h_offset, reveal_rows,
    syntax_text_runs, visible_rows_at,
};
use super::highlight::{
    CodeHighlighter, DeferredParse, HIGHLIGHT_FRAME_BUDGET, HighlightOutcome, SYNC_PARSE_BUDGET,
    spawn_deferred_parse,
};
use super::load::{CodeLoadSlot, CodeLoadState, CodeOpen, spawn_code_load};
use super::markers::MARKER_COLUMN_W;
use super::navigation::NavigationState;
use super::save::{self, FileStamp};
use crate::diff::{DiffSyntax, highlight_lines, palette};
use crate::settings::components::menu_surface;
use crate::terminal::blink::{BlinkPhaseGlobal, CURSOR_BLINK_INTERVAL};

pub(crate) const CODE_KEY_CONTEXT: &str = "CodeEditor";

fn ops_descend_by_row(doc: &CodeDocument, ops: &[(Range<usize>, String)]) -> bool {
    ops.windows(2)
        .all(|pair| doc.byte_to_line(pair[1].0.end) < doc.byte_to_line(pair[0].0.start))
}

fn wheel_pixels(delta: &ScrollDelta, char_w: f32) -> Point<f32> {
    match delta {
        ScrollDelta::Pixels(pixels) => Point::new(f32::from(pixels.x), f32::from(pixels.y)),
        ScrollDelta::Lines(lines) => Point::new(lines.x * char_w, lines.y * CODE_ROW_HEIGHT),
    }
}

const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
const MULTI_CLICK_RADIUS: f32 = 2.0;

const DRAG_SCROLL_ROWS: f32 = 1.0;
const DRAG_SCROLL_COLUMNS: f32 = 3.0;

const READ_ONLY_FLASH: Duration = Duration::from_millis(600);

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const RELOAD_DIFF_ATTEMPTS: usize = 2;
const INITIAL_HIGHLIGHT_ROWS: usize = 60;
const TOO_COMPLEX_BANNER: &str = "This file is too complex to color.";

pub(crate) const TRACKER_DEBOUNCE: Duration = Duration::from_millis(150);
const TRACKER_POLICY: ComparisonPolicy = ComparisonPolicy::Default;

pub(crate) const POPUP_SHOWN_LINES: usize = 200;
const POPUP_VISIBLE_ROWS: f32 = 12.0;
const POPUP_MIN_W: f32 = 280.0;
const POPUP_MAX_W: f32 = 520.0;
const POPUP_MARGIN: f32 = 12.0;
const POPUP_HEADER_H: f32 = 28.0;
const POPUP_ACTIONS_H: f32 = 34.0;
const POPUP_FOOTER_H: f32 = 20.0;
const POPUP_PADDING: f32 = 6.0;

actions!(
    paneflow_code_editor,
    [
        CeLeft,
        CeRight,
        CeUp,
        CeDown,
        CeSelectLeft,
        CeSelectRight,
        CeSelectUp,
        CeSelectDown,
        CeWordLeft,
        CeWordRight,
        CeSelectWordLeft,
        CeSelectWordRight,
        CeHome,
        CeEnd,
        CeSelectHome,
        CeSelectEnd,
        CePageUp,
        CePageDown,
        CeSelectPageUp,
        CeSelectPageDown,
        CeDocStart,
        CeDocEnd,
        CeSelectDocStart,
        CeSelectDocEnd,
        CeSelectAll,
        CeBackspace,
        CeDelete,
        CeNewline,
        CeUndo,
        CeRedo,
        CeCopy,
        CeCut,
        CePaste,
        CeIndent,
        CeOutdent,
        CeSave,
        CeEscape,
    ]
);

pub(crate) fn register_keybindings(cx: &mut App) {
    let ctx = Some(CODE_KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", CeLeft, ctx),
        KeyBinding::new("right", CeRight, ctx),
        KeyBinding::new("up", CeUp, ctx),
        KeyBinding::new("down", CeDown, ctx),
        KeyBinding::new("shift-left", CeSelectLeft, ctx),
        KeyBinding::new("shift-right", CeSelectRight, ctx),
        KeyBinding::new("shift-up", CeSelectUp, ctx),
        KeyBinding::new("shift-down", CeSelectDown, ctx),
        KeyBinding::new("home", CeHome, ctx),
        KeyBinding::new("end", CeEnd, ctx),
        KeyBinding::new("shift-home", CeSelectHome, ctx),
        KeyBinding::new("shift-end", CeSelectEnd, ctx),
        KeyBinding::new("pageup", CePageUp, ctx),
        KeyBinding::new("pagedown", CePageDown, ctx),
        KeyBinding::new("shift-pageup", CeSelectPageUp, ctx),
        KeyBinding::new("shift-pagedown", CeSelectPageDown, ctx),
        KeyBinding::new("secondary-a", CeSelectAll, ctx),
        KeyBinding::new("backspace", CeBackspace, ctx),
        KeyBinding::new("delete", CeDelete, ctx),
        KeyBinding::new("enter", CeNewline, ctx),
        KeyBinding::new("tab", CeIndent, ctx),
        KeyBinding::new("shift-tab", CeOutdent, ctx),
        KeyBinding::new("secondary-z", CeUndo, ctx),
        KeyBinding::new("secondary-shift-z", CeRedo, ctx),
        KeyBinding::new("secondary-c", CeCopy, ctx),
        KeyBinding::new("secondary-x", CeCut, ctx),
        KeyBinding::new("secondary-v", CePaste, ctx),
        KeyBinding::new("secondary-s", CeSave, ctx),
        KeyBinding::new("escape", CeEscape, ctx),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("alt-left", CeWordLeft, ctx),
        KeyBinding::new("alt-right", CeWordRight, ctx),
        KeyBinding::new("alt-shift-left", CeSelectWordLeft, ctx),
        KeyBinding::new("alt-shift-right", CeSelectWordRight, ctx),
        KeyBinding::new("cmd-up", CeDocStart, ctx),
        KeyBinding::new("cmd-down", CeDocEnd, ctx),
        KeyBinding::new("cmd-shift-up", CeSelectDocStart, ctx),
        KeyBinding::new("cmd-shift-down", CeSelectDocEnd, ctx),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-left", CeWordLeft, ctx),
        KeyBinding::new("ctrl-right", CeWordRight, ctx),
        KeyBinding::new("ctrl-shift-left", CeSelectWordLeft, ctx),
        KeyBinding::new("ctrl-shift-right", CeSelectWordRight, ctx),
        KeyBinding::new("ctrl-home", CeDocStart, ctx),
        KeyBinding::new("ctrl-end", CeDocEnd, ctx),
        KeyBinding::new("ctrl-shift-home", CeSelectDocStart, ctx),
        KeyBinding::new("ctrl-shift-end", CeSelectDocEnd, ctx),
        KeyBinding::new("ctrl-y", CeRedo, ctx),
    ]);
}

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

async fn reload_from_disk(
    this: &WeakEntity<CodeView>,
    cx: &mut AsyncApp,
    stamp: Option<FileStamp>,
    text: Option<String>,
    force: bool,
) -> bool {
    let present = text.is_some();
    let begun = cx.update(|cx| {
        this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
            view.begin_disk_reload(stamp, present, force, cx)
        })
    });
    let Ok(begun) = begun else {
        return false;
    };
    let (Some(mut diff), Some(text)) = (begun, text) else {
        return true;
    };
    let text = Arc::new(text);
    for attempt in 0..RELOAD_DIFF_ATTEMPTS {
        let DiskDiff { rope, revision } = diff;
        let incoming = Arc::clone(&text);
        let splices = cx
            .background_spawn(async move { edit::disk_splices(&rope, &incoming) })
            .await;
        let retry = attempt + 1 < RELOAD_DIFF_ATTEMPTS;
        let finished = cx.update(|cx| {
            this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
                view.finish_disk_reload(revision, splices, retry, force, cx)
            })
        });
        let Ok(next) = finished else {
            return false;
        };
        match next {
            Some(again) => diff = again,
            None => return true,
        }
    }
    true
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
    focus_observers_installed: bool,
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
            focus_observers_installed: false,
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
            focus_observers_installed: false,
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
        if self.focus_observers_installed {
            return;
        }
        self.focus_observers_installed = true;
        let focus = self.focus.clone();
        cx.on_focus(&focus, window, |view, _window, cx| {
            view.sync_focus_state(true);
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus, window, |view, _window, cx| {
            view.sync_focus_state(false);
            cx.notify();
        })
        .detach();
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

    #[allow(dead_code)]
    pub(crate) fn cursor(&self) -> usize {
        self.selection.cursor()
    }

    #[allow(dead_code)]
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

    fn render_load_error(
        &self,
        message: String,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel =
            super::super::render::diff_panel_centered("icons/triangle-alert.svg", message, ui);
        if !self.state.is_retriable() {
            return panel;
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .pb(px(20.))
            .child(panel)
            .child(
                div()
                    .id("code-reload")
                    .flex_none()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ui.border)
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(ui.subtle))
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        let path = this.path.clone();
                        this.open(path, cx);
                    }))
                    .child("Reload"),
            )
            .into_any_element()
    }

    #[allow(dead_code)]
    pub(crate) fn selection(&self) -> Range<usize> {
        self.selection.range()
    }

    #[allow(dead_code)]
    pub(crate) fn set_cursor_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let row = row.min(doc.line_count().saturating_sub(1));
        let offset = doc.line_to_byte(row);
        self.place_caret(offset, false, cx);
    }

    fn place_caret(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        self.end_typing_group();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = cursor::clamp(doc, offset);
        let goal = cursor::goal_column(doc, offset);
        self.goal_column = goal;
        self.selection.apply(offset, extend);
        self.after_motion(cx);
    }

    fn move_rows(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        self.end_typing_group();
        let goal = self.goal_column;
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = cursor::vertical(doc, self.selection.cursor(), goal, delta);
        self.selection.apply(offset, extend);
        self.after_motion(cx);
    }

    fn after_motion(&mut self, cx: &mut Context<Self>) {
        self.last_motion = Instant::now();
        self.blink_visible = true;
        self.reveal_cursor();
        cx.notify();
    }

    fn page_rows(&self) -> usize {
        cursor::page_rows(self.scroll.viewport_height(), CODE_ROW_HEIGHT)
    }

    fn sync_scroll_line_count(&self) {
        if let Some(doc) = self.state.document() {
            self.scroll.set_line_count(doc.line_count());
        }
    }

    pub(crate) fn reveal_cursor(&mut self) {
        let viewport_h = self.scroll.viewport_height();
        let geometry = self.geometry.get();
        let h_offset = self.h_offset;
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = self.selection.cursor();
        let row = doc.byte_to_line(offset);
        let column = cursor::goal_column(doc, offset);
        self.scroll.set_line_count(doc.line_count());

        let target = reveal_rows(row, viewport_h, self.scroll.max_rows(), self.scroll.rows());
        self.scroll.set_rows(target);
        let caret_x = column as f32 * geometry.char_w;
        self.h_offset = reveal_h_offset(
            caret_x,
            geometry.text_viewport_w,
            geometry.max_h_offset,
            h_offset,
        );
    }

    fn sync_theme(&mut self) {
        let generation = crate::theme::theme_generation();
        if generation == self.theme_generation {
            return;
        }
        self.theme_generation = generation;
        let syntax = DiffSyntax::from_theme(&crate::theme::active_theme());
        if let Some((doc, hl)) = self.state.editable() {
            hl.set_syntax(doc, syntax);
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

    fn apply_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let bounds = self.scroll.bounds();
        if !bounds.contains(&ev.position) {
            return;
        }
        let geometry = self.geometry.get();
        let delta = wheel_pixels(&ev.delta, geometry.char_w);
        self.sync_scroll_line_count();
        let mut moved = self.scroll.scroll_by_pixels(-delta.y);
        if delta.x != 0.0 {
            let next = (self.h_offset - delta.x).clamp(0.0, geometry.max_h_offset);
            if next != self.h_offset {
                self.h_offset = next;
                moved = true;
            }
        }
        if moved {
            cx.notify();
        }
    }

    fn on_scrollbar_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) -> bool {
        let handled = self.navigation.mouse_down(
            ev.position,
            &self.scroll,
            &mut self.h_offset,
            self.geometry.get().max_h_offset,
        );
        if handled {
            cx.notify();
        }
        handled
    }

    pub(super) fn on_scrollbar_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.navigation.mouse_move(
            ev.position,
            ev.pressed_button == Some(MouseButton::Left),
            &self.scroll,
            &mut self.h_offset,
            self.geometry.get().max_h_offset,
        ) {
            cx.notify();
        }
    }

    pub(super) fn on_scrollbar_up(&mut self, _ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.navigation.drag.take().is_some() {
            cx.notify();
        }
    }

    fn offset_at(&self, position: Point<Pixels>) -> Option<usize> {
        let doc = self.state.document()?;
        Some(self.hits.borrow().offset_at(doc, position))
    }

    fn chain_click(&mut self, position: Point<Pixels>, now: Instant) -> u8 {
        let count = match self.click_chain {
            Some(prev)
                if now.duration_since(prev.at) <= MULTI_CLICK_INTERVAL
                    && f32::from(position.x - prev.position.x).abs() <= MULTI_CLICK_RADIUS
                    && f32::from(position.y - prev.position.y).abs() <= MULTI_CLICK_RADIUS =>
            {
                prev.count % 3 + 1
            }
            _ => 1,
        };
        self.click_chain = Some(ClickChain {
            at: now,
            position,
            count,
        });
        count
    }

    fn on_text_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.scroll.bounds().contains(&ev.position) {
            return false;
        }
        window.focus(&self.focus, cx);
        self.end_typing_group();
        let Some(offset) = self.offset_at(ev.position) else {
            return false;
        };
        let count = self.chain_click(ev.position, Instant::now());
        let Some(doc) = self.state.document() else {
            return false;
        };
        let (grain, range) = match count {
            2 => (DragGrain::Word, cursor::word_range_at(doc, offset)),
            3 => (DragGrain::Line, cursor::line_range_at(doc, offset)),
            _ => (DragGrain::Grapheme, offset..offset),
        };
        let goal = cursor::goal_column(doc, range.end);
        self.goal_column = goal;
        self.selection = CodeSelection {
            anchor: range.start,
            head: range.end,
        };
        self.text_drag = Some(TextDrag {
            grain,
            anchor: range,
        });
        self.after_motion(cx);
        true
    }

    fn on_text_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.text_drag.is_none() {
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            self.text_drag = None;
            cx.notify();
            return;
        }
        let scrolled = self.drag_autoscroll(ev.position);
        let Some(offset) = self.offset_at(ev.position) else {
            if scrolled {
                cx.notify();
            }
            return;
        };
        self.extend_drag_to(offset, cx);
    }

    fn extend_drag_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(drag) = self.text_drag.clone() else {
            return;
        };
        let Some(doc) = self.state.document() else {
            return;
        };
        let reach = match drag.grain {
            DragGrain::Grapheme => offset..offset,
            DragGrain::Word => cursor::word_range_at(doc, offset),
            DragGrain::Line => cursor::line_range_at(doc, offset),
        };
        let selection = if reach.start < drag.anchor.start {
            CodeSelection {
                anchor: drag.anchor.end,
                head: reach.start,
            }
        } else {
            CodeSelection {
                anchor: drag.anchor.start,
                head: reach.end.max(drag.anchor.end),
            }
        };
        let goal = cursor::goal_column(doc, selection.cursor());
        self.selection = selection;
        self.goal_column = goal;
        self.last_motion = Instant::now();
        self.blink_visible = true;
        cx.notify();
    }

    fn drag_autoscroll(&mut self, position: Point<Pixels>) -> bool {
        let bounds = self.scroll.bounds();
        let geometry = self.geometry.get();
        let mut moved = false;

        let dy = autoscroll_step(
            f32::from(position.y),
            f32::from(bounds.origin.y),
            f32::from(bounds.bottom()),
            DRAG_SCROLL_ROWS * CODE_ROW_HEIGHT,
        );
        if dy != 0.0 {
            self.sync_scroll_line_count();
            moved = self.scroll.scroll_by_pixels(dy);
        }

        let dx = autoscroll_step(
            f32::from(position.x),
            f32::from(bounds.origin.x),
            f32::from(bounds.right()),
            DRAG_SCROLL_COLUMNS * geometry.char_w,
        );
        if dx != 0.0 {
            let next = (self.h_offset + dx).clamp(0.0, geometry.max_h_offset);
            if next != self.h_offset {
                self.h_offset = next;
                moved = true;
            }
        }

        moved
    }

    fn on_text_up(&mut self, _ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.text_drag.take().is_some() {
            cx.notify();
        }
    }

    fn left(&mut self, _: &CeLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(-1, false, cx);
    }

    fn right(&mut self, _: &CeRight, _w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(1, false, cx);
    }

    fn select_left(&mut self, _: &CeSelectLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(-1, true, cx);
    }

    fn select_right(&mut self, _: &CeSelectRight, _w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(1, true, cx);
    }

    fn up(&mut self, _: &CeUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, false, cx);
    }

    fn down(&mut self, _: &CeDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(1, false, cx);
    }

    fn select_up(&mut self, _: &CeSelectUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, true, cx);
    }

    fn select_down(&mut self, _: &CeSelectDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(1, true, cx);
    }

    fn word_left(&mut self, _: &CeWordLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.word(-1, false, cx);
    }

    fn word_right(&mut self, _: &CeWordRight, _w: &mut Window, cx: &mut Context<Self>) {
        self.word(1, false, cx);
    }

    fn select_word_left(&mut self, _: &CeSelectWordLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.word(-1, true, cx);
    }

    fn select_word_right(
        &mut self,
        _: &CeSelectWordRight,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.word(1, true, cx);
    }

    fn home(&mut self, _: &CeHome, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(false, false, cx);
    }

    fn end(&mut self, _: &CeEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(true, false, cx);
    }

    fn select_home(&mut self, _: &CeSelectHome, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(false, true, cx);
    }

    fn select_end(&mut self, _: &CeSelectEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(true, true, cx);
    }

    fn page_up(&mut self, _: &CePageUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.page(-1, false, cx);
    }

    fn page_down(&mut self, _: &CePageDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.page(1, false, cx);
    }

    fn select_page_up(&mut self, _: &CeSelectPageUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.page(-1, true, cx);
    }

    fn select_page_down(&mut self, _: &CeSelectPageDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.page(1, true, cx);
    }

    fn doc_start(&mut self, _: &CeDocStart, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_edge(false, false, cx);
    }

    fn doc_end(&mut self, _: &CeDocEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_edge(true, false, cx);
    }

    fn select_doc_start(&mut self, _: &CeSelectDocStart, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_edge(false, true, cx);
    }

    fn select_doc_end(&mut self, _: &CeSelectDocEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_edge(true, true, cx);
    }

    fn select_all(&mut self, _: &CeSelectAll, _w: &mut Window, cx: &mut Context<Self>) {
        self.take_whole_document(cx);
    }

    fn horizontal(&mut self, direction: isize, extend: bool, cx: &mut Context<Self>) {
        let from = match (extend, self.selection.is_empty(), direction < 0) {
            (false, false, true) => self.selection.range().start,
            (false, false, false) => self.selection.range().end,
            _ => self.selection.cursor(),
        };
        let collapsing = !extend && !self.selection.is_empty();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if collapsing {
            from
        } else if direction < 0 {
            cursor::grapheme_left(doc, from)
        } else {
            cursor::grapheme_right(doc, from)
        };
        self.place_caret(offset, extend, cx);
    }

    fn word(&mut self, direction: isize, extend: bool, cx: &mut Context<Self>) {
        let from = self.selection.cursor();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if direction < 0 {
            cursor::word_left(doc, from)
        } else {
            cursor::word_right(doc, from)
        };
        self.place_caret(offset, extend, cx);
    }

    fn line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let from = self.selection.cursor();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if end {
            cursor::line_end(doc, from)
        } else {
            cursor::line_home(doc, from)
        };
        self.place_caret(offset, extend, cx);
    }

    fn page(&mut self, direction: isize, extend: bool, cx: &mut Context<Self>) {
        let rows = self.page_rows() as isize;
        self.move_rows(direction * rows, extend, cx);
    }

    fn doc_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if end { cursor::doc_end(doc) } else { 0 };
        self.place_caret(offset, extend, cx);
    }

    fn take_whole_document(&mut self, cx: &mut Context<Self>) {
        self.end_typing_group();
        let Some(doc) = self.state.document() else {
            return;
        };
        let end = cursor::doc_end(doc);
        let goal = cursor::goal_column(doc, end);
        self.selection = CodeSelection {
            anchor: 0,
            head: end,
        };
        self.goal_column = goal;
        self.after_motion(cx);
    }

    fn end_typing_group(&mut self) {
        self.history.close_group();
        self.marked = None;
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.history.mark() != self.saved_mark
    }

    #[allow(dead_code)]
    pub(crate) fn has_conflict(&self) -> bool {
        self.disk == DiskState::Conflict
    }

    fn splice_all(
        &mut self,
        ops: &[(Range<usize>, String)],
        after: CodeSelection,
        group: EditGroup,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            self.flash_read_only(cx);
            return false;
        }
        let before = self.selection;
        let now = Instant::now();
        let batched = self
            .state
            .document()
            .is_some_and(|doc| ops_descend_by_row(doc, ops));
        let mut records = Vec::with_capacity(ops.len());
        let mut edits = Vec::with_capacity(ops.len());
        let mut deferred: Option<DeferredParse> = None;
        let mut changes = Vec::with_capacity(ops.len() * 2);
        if let Some((doc, hl)) = self.state.editable() {
            for (range, text) in ops {
                let Some(applied) = edit::splice(doc, range.clone(), text) else {
                    continue;
                };
                if batched {
                    edits.push(applied.edit);
                } else if let HighlightOutcome::Deferred(parse) = hl.edit(doc, &applied.edit) {
                    deferred = Some(parse);
                }
                changes.extend(applied.windows);
                records.push(applied.record);
            }
            if batched
                && let Ok(HighlightOutcome::Deferred(parse)) =
                    hl.edit_batch(doc, &edits, SYNC_PARSE_BUDGET)
            {
                deferred = Some(parse);
            }
        }
        if records.is_empty() {
            return false;
        }
        self.history.push(records, before, after, group, now);
        self.note_changes(&changes, cx);
        self.finish_edit(after, deferred, cx);
        true
    }

    fn finish_edit(
        &mut self,
        after: CodeSelection,
        deferred: Option<DeferredParse>,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self.state.document() {
            self.selection = CodeSelection {
                anchor: cursor::clamp(doc, after.anchor),
                head: cursor::clamp(doc, after.head),
            };
            self.goal_column = cursor::goal_column(doc, self.selection.cursor());
        }
        if let Some(parse) = deferred {
            spawn_deferred_parse(parse, cx, |view: &mut Self, parsed, cx| {
                if let Some((doc, hl)) = view.state.editable()
                    && hl.apply_parsed(doc, parsed)
                {
                    cx.notify();
                }
            });
        }
        self.refresh_longest_line(cx);
        self.after_motion(cx);
    }

    fn refresh_longest_line(&mut self, cx: &mut Context<Self>) {
        let Some((text, revision)) = self
            .state
            .document()
            .and_then(CodeDocument::longest_line_snapshot)
        else {
            return;
        };
        let load_generation = self.slot.current();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            #[cfg(not(test))]
            let longest = smol::unblock(move || CodeDocument::measure_longest_line(&text)).await;
            #[cfg(test)]
            let longest = cx
                .background_spawn(async move { CodeDocument::measure_longest_line(&text) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if !view.slot.accept(load_generation) {
                        return;
                    }
                    let Some(doc) = view.state.document_mut() else {
                        return;
                    };
                    if doc.apply_longest_line_measurement(revision, longest) {
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn flash_read_only(&mut self, cx: &mut Context<Self>) {
        self.read_only_flash = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(READ_ONLY_FLASH).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if view
                        .read_only_flash
                        .is_some_and(|at| at.elapsed() >= READ_ONLY_FLASH)
                    {
                        view.read_only_flash = None;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn insert_text(&mut self, text: &str, group: EditGroup, cx: &mut Context<Self>) -> bool {
        let range = self.replacement_range();
        let inserted = normalize_newlines(text).into_owned();
        let caret = CodeSelection::at(range.start + inserted.len());
        self.splice_all(&[(range, inserted)], caret, group, cx)
    }

    fn replacement_range(&self) -> Range<usize> {
        match &self.marked {
            Some(marked) => marked.clone(),
            None => self.selection.range(),
        }
    }

    fn resolve_replacement(&self, range_utf16: Option<Range<usize>>) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        Some(match range_utf16 {
            Some(range) => {
                let start = doc.utf16_to_byte(range.start);
                start..doc.utf16_to_byte(range.end).max(start)
            }
            None => self.replacement_range(),
        })
    }

    fn delete_grapheme(&mut self, forward: bool, cx: &mut Context<Self>) {
        let selection = self.selection.range();
        let range = if !selection.is_empty() {
            selection
        } else {
            let Some(doc) = self.state.document() else {
                return;
            };
            let at = self.selection.cursor();
            if forward {
                at..cursor::grapheme_right(doc, at)
            } else {
                cursor::grapheme_left(doc, at)..at
            }
        };
        if range.is_empty() {
            return;
        }
        let caret = CodeSelection::at(range.start);
        self.splice_all(&[(range, String::new())], caret, EditGroup::Typing, cx);
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        let mut text = String::from("\n");
        if let Some(doc) = self.state.document() {
            let at = self.selection.range().start;
            let row = doc.byte_to_line(at);
            let start = doc.line_to_byte(row);
            if let Some(line) = doc.line_string(row) {
                let indent = edit::leading_indent(&line);
                let column = at.saturating_sub(start);
                text.push_str(&indent[..indent.len().min(column)]);
            }
        }
        self.insert_text(&text, EditGroup::Atomic, cx);
    }

    fn selected_rows(&self) -> Option<(usize, usize)> {
        let doc = self.state.document()?;
        let range = self.selection.range();
        let first = doc.byte_to_line(range.start);
        let last_byte = if range.end > range.start {
            range.end - 1
        } else {
            range.end
        };
        Some((first, doc.byte_to_line(last_byte).max(first)))
    }

    fn shift_lines(&mut self, outdent: bool, cx: &mut Context<Self>) {
        let Some((first, last)) = self.selected_rows() else {
            return;
        };
        if !outdent && self.selection.is_empty() {
            let unit = self.indent.as_str().into_owned();
            self.insert_text(&unit, EditGroup::Atomic, cx);
            return;
        }
        let unit = self.indent;
        let mut ops: Vec<(Range<usize>, String)> = Vec::new();
        let mut deltas: Vec<(usize, isize)> = Vec::new();
        {
            let Some(doc) = self.state.document() else {
                return;
            };
            for row in (first..=last).rev() {
                let start = doc.line_to_byte(row);
                let Some(line) = doc.line_string(row) else {
                    continue;
                };
                if outdent {
                    let width = edit::dedent_width(&line, unit);
                    if width == 0 {
                        continue;
                    }
                    ops.push((start..start + width, String::new()));
                    deltas.push((start, -(width as isize)));
                } else {
                    if line.trim_end_matches('\n').is_empty() {
                        continue;
                    }
                    let text = unit.as_str().into_owned();
                    let width = text.len() as isize;
                    ops.push((start..start, text));
                    deltas.push((start, width));
                }
            }
        }
        if ops.is_empty() {
            return;
        }
        let after = CodeSelection {
            anchor: shift_offset(self.selection.anchor, &deltas),
            head: shift_offset(self.selection.head, &deltas),
        };
        self.splice_all(&ops, after, EditGroup::Atomic, cx);
    }

    fn clip_range(&self) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        let selection = self.selection.range();
        if selection.is_empty() {
            Some(cursor::line_range_at(doc, selection.start))
        } else {
            Some(selection)
        }
    }

    fn copy_selection(&mut self, cut: bool, cx: &mut Context<Self>) {
        let Some(range) = self.clip_range() else {
            return;
        };
        if range.is_empty() {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        let text = doc.slice_string(range.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut {
            let caret = CodeSelection::at(range.start);
            self.splice_all(&[(range, String::new())], caret, EditGroup::Atomic, cx);
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        let text = edit::sanitize_paste(&text);
        if text.is_empty() {
            return;
        }
        self.end_typing_group();
        self.insert_text(&text, EditGroup::Atomic, cx);
    }

    fn time_travel(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            self.flash_read_only(cx);
            return;
        }
        self.marked = None;
        let mut deferred: Option<DeferredParse> = None;
        let mut restored = None;
        let mut changes = Vec::new();
        if let Some((doc, hl)) = self.state.editable() {
            let step = if redo {
                self.history.redo(doc)
            } else {
                self.history.undo(doc)
            };
            if let Some(step) = step {
                for change in &step.edits {
                    if let HighlightOutcome::Deferred(parse) = hl.edit(doc, &change.edit) {
                        deferred = Some(parse);
                    }
                }
                changes = step.edits.iter().map(|change| change.window).collect();
                restored = Some(step.selection);
            }
        }
        let Some(selection) = restored else {
            return;
        };
        self.note_changes(&changes, cx);
        self.finish_edit(selection, deferred, cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        if doc.is_read_only() {
            self.flash_read_only(cx);
            return;
        }
        if !self.is_dirty() && self.disk == DiskState::InSync {
            return;
        }
        self.history.close_group();
        let contents = doc.to_disk_string();
        let path = self.path.clone();
        let expected = self.stamp;
        let mark = self.history.mark();
        self.saving = true;
        self.save_error = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = cx
                .background_spawn(async move {
                    let current = FileStamp::read(&path);
                    let conflict = match (expected, current) {
                        (Some(expected), Some(current)) => expected.differs(&current),
                        (None, Some(_)) => true,
                        _ => false,
                    };
                    if conflict {
                        return Err(None);
                    }
                    save::save_blocking(&path, &contents).map_err(Some)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    view.finish_save(outcome, mark, cx);
                });
            });
        })
        .detach();
    }

    fn finish_save(
        &mut self,
        outcome: Result<FileStamp, Option<String>>,
        mark: edit::HistoryMark,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        match outcome {
            Ok(stamp) => {
                self.stamp = Some(stamp);
                self.saved_mark = mark;
                self.disk = DiskState::InSync;
                self.save_error = None;
            }
            Err(Some(message)) => {
                self.save_error = Some(message);
            }
            Err(None) => {
                self.disk = DiskState::Conflict;
            }
        }
        cx.notify();
    }

    fn start_watcher(&mut self, cx: &mut Context<Self>) {
        self._watcher = None;
        self._watch_bridge = None;
        let Some(parent) = self.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let Some(name) = self.path.file_name().map(|name| name.to_os_string()) else {
            return;
        };
        let generation = self.slot.current();
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let watched_parent = parent.clone();
            let outcome = cx
                .background_spawn(async move { create_file_watcher(parent) })
                .await;
            let (watcher, bridge, rx) = match outcome {
                Ok(parts) => parts,
                Err(err) => {
                    log::warn!(
                        "could not watch {} for changes: {err}",
                        watched_parent.display()
                    );
                    return;
                }
            };
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if !view.slot.accept(generation) {
                        return;
                    }
                    view._watcher = Some(watcher);
                    view._watch_bridge = Some(bridge);
                    view.spawn_reload_loop(path, name, rx, cx);
                });
            });
        })
        .detach();
    }

    fn spawn_reload_loop(
        &mut self,
        path: PathBuf,
        name: std::ffi::OsString,
        mut rx: WatchEvents,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Some(first) = rx.next().await {
                if !event_is_relevant(&first, &name) {
                    continue;
                }
                let deadline = Instant::now() + RELOAD_DEBOUNCE;
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let timer = cx.background_executor().timer(remaining);
                    match futures::future::select(rx.next(), timer).await {
                        Either::Left((Some(_), _)) => continue,
                        Either::Left((None, _)) => return,
                        Either::Right(_) => break,
                    }
                }
                let probe = path.clone();
                let (stamp, text) = cx
                    .background_spawn(async move {
                        (
                            FileStamp::read(&probe),
                            std::fs::read_to_string(&probe).ok(),
                        )
                    })
                    .await;
                if !reload_from_disk(&this, cx, stamp, text, false).await {
                    break;
                }
            }
        })
        .detach();
    }

    fn begin_disk_reload(
        &mut self,
        stamp: Option<FileStamp>,
        present: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Option<DiskDiff> {
        let Some(stamp) = stamp.filter(|_| present) else {
            if self.disk != DiskState::Deleted {
                self.disk = DiskState::Deleted;
                cx.notify();
            }
            return None;
        };
        if !force {
            if self.stamp == Some(stamp) && self.disk == DiskState::InSync {
                return None;
            }
            self.stamp = Some(stamp);
            if self.is_dirty() {
                self.disk = DiskState::Conflict;
                cx.notify();
                return None;
            }
        } else {
            self.stamp = Some(stamp);
        }
        self.disk = DiskState::InSync;
        self.state.document().map(DiskDiff::of)
    }

    fn finish_disk_reload(
        &mut self,
        revision: u64,
        splices: Vec<(Range<usize>, String)>,
        retry: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Option<DiskDiff> {
        let doc = self.state.document()?;
        if doc.revision() != revision {
            if retry {
                return Some(DiskDiff::of(doc));
            }
            self.disk = DiskState::Conflict;
            cx.notify();
            return None;
        }
        if !force && self.is_dirty() {
            self.disk = DiskState::Conflict;
            cx.notify();
            return None;
        }
        self.apply_disk_splices(&splices, cx);
        self.saved_mark = self.history.mark();
        None
    }

    fn apply_disk_splices(&mut self, ops: &[(Range<usize>, String)], cx: &mut Context<Self>) {
        if ops.is_empty() {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        let scroll_rows = self.scroll.rows();
        let after = edit::shift_selection_for_splices(self.selection, ops);
        let reason = doc.read_only_reason();
        if reason.is_some()
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_read_only(None);
        }
        let replaced = self.splice_all(ops, after, EditGroup::Atomic, cx);
        if let Some(reason) = reason
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_read_only(Some(reason));
        }
        if !replaced {
            return;
        }
        self.sync_scroll_line_count();
        self.scroll.set_rows(scroll_rows);
        self.popup = None;
        self.reset_tracker(cx);
        cx.notify();
    }

    fn resolve_keep_mine(&mut self, cx: &mut Context<Self>) {
        self.disk = DiskState::InSync;
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let stamp = cx
                .background_spawn(async move { FileStamp::read(&path) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    view.stamp = stamp;
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn resolve_reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let probe = path.clone();
            let (stamp, text) = cx
                .background_spawn(async move {
                    (
                        FileStamp::read(&probe),
                        std::fs::read_to_string(&probe).ok(),
                    )
                })
                .await;
            reload_from_disk(&this, cx, stamp, text, true).await;
        })
        .detach();
    }

    fn backspace(&mut self, _: &CeBackspace, _w: &mut Window, cx: &mut Context<Self>) {
        self.delete_grapheme(false, cx);
    }

    fn delete(&mut self, _: &CeDelete, _w: &mut Window, cx: &mut Context<Self>) {
        self.delete_grapheme(true, cx);
    }

    fn newline(&mut self, _: &CeNewline, _w: &mut Window, cx: &mut Context<Self>) {
        self.insert_newline(cx);
    }

    fn undo(&mut self, _: &CeUndo, _w: &mut Window, cx: &mut Context<Self>) {
        self.time_travel(false, cx);
    }

    fn redo(&mut self, _: &CeRedo, _w: &mut Window, cx: &mut Context<Self>) {
        self.time_travel(true, cx);
    }

    fn copy(&mut self, _: &CeCopy, _w: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(false, cx);
    }

    fn cut(&mut self, _: &CeCut, _w: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(true, cx);
    }

    fn paste_action(&mut self, _: &CePaste, _w: &mut Window, cx: &mut Context<Self>) {
        self.paste(cx);
    }

    fn indent(&mut self, _: &CeIndent, _w: &mut Window, cx: &mut Context<Self>) {
        self.shift_lines(false, cx);
    }

    fn outdent(&mut self, _: &CeOutdent, _w: &mut Window, cx: &mut Context<Self>) {
        self.shift_lines(true, cx);
    }

    fn save_action(&mut self, _: &CeSave, _w: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }

    fn escape(&mut self, _: &CeEscape, _w: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_some() {
            self.close_marker_popup(cx);
        } else {
            cx.propagate();
        }
    }

    fn banners(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        let row = || {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .text_xs()
                .border_b_1()
                .border_color(ui.border)
        };
        if let Some(reason) = self
            .state
            .document()
            .and_then(CodeDocument::read_only_reason)
        {
            let flashing = self.read_only_flash.is_some();
            out.push(
                row()
                    .bg(if flashing {
                        ui.vc_conflict.opacity(0.22)
                    } else {
                        ui.overlay
                    })
                    .text_color(if flashing { ui.text } else { ui.muted })
                    .child(read_only_text(reason))
                    .into_any_element(),
            );
        }
        match self.disk {
            DiskState::Conflict => out.push(
                row()
                    .bg(ui.vc_conflict.opacity(0.16))
                    .text_color(ui.text)
                    .child(div().flex_1().child(
                        "This file changed on disk while you were editing it. Nothing has been \
                         overwritten.",
                    ))
                    .child(conflict_button(
                        "code-conflict-keep",
                        "Keep mine",
                        ui,
                        cx.listener(|this, _: &MouseDownEvent, _w, cx| this.resolve_keep_mine(cx)),
                    ))
                    .child(conflict_button(
                        "code-conflict-reload",
                        "Reload from disk",
                        ui,
                        cx.listener(|this, _: &MouseDownEvent, _w, cx| this.resolve_reload(cx)),
                    ))
                    .into_any_element(),
            ),
            DiskState::Deleted => out.push(
                row()
                    .bg(ui.vc_conflict.opacity(0.16))
                    .text_color(ui.text)
                    .child("This file was deleted on disk. Saving recreates it.")
                    .into_any_element(),
            ),
            DiskState::InSync => {}
        }
        if let Some(message) = &self.save_error {
            out.push(
                row()
                    .bg(ui.vc_deleted.opacity(0.16))
                    .text_color(ui.text)
                    .child(format!("{message} Your edits are still here."))
                    .into_any_element(),
            );
        }
        if self
            .state
            .highlighter()
            .is_some_and(CodeHighlighter::is_too_complex)
        {
            out.push(
                row()
                    .bg(ui.overlay)
                    .text_color(ui.muted)
                    .child(TOO_COMPLEX_BANNER)
                    .into_any_element(),
            );
        }
        out
    }
}

fn shift_offset(offset: usize, deltas: &[(usize, isize)]) -> usize {
    let mut out = offset as isize;
    for (start, delta) in deltas {
        if *delta > 0 {
            if *start <= offset {
                out += delta;
            }
        } else if *start < offset {
            let removed = delta.unsigned_abs();
            out -= removed.min(offset - start) as isize;
        }
    }
    out.max(0) as usize
}

fn create_file_watcher(
    parent: PathBuf,
) -> Result<(RecommendedWatcher, WatchBridge, WatchEvents), String> {
    if !parent.is_dir() {
        return Err("the parent directory no longer exists".to_string());
    }
    let (tx, rx) = mpsc::unbounded::<notify::Result<notify::Event>>();
    let bridge: WatchBridge = Arc::new(Mutex::new(Some(tx)));
    let notify_side = Arc::clone(&bridge);
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            if let Ok(guard) = notify_side.lock()
                && let Some(tx) = guard.as_ref()
            {
                let _ = tx.unbounded_send(result);
            }
        },
        notify::Config::default(),
    )
    .map_err(|err| err.to_string())?;
    watcher
        .watch(&parent, RecursiveMode::NonRecursive)
        .map_err(|err| err.to_string())?;
    Ok((watcher, bridge, rx))
}

fn event_is_relevant(result: &notify::Result<notify::Event>, target: &std::ffi::OsStr) -> bool {
    match result {
        Ok(event) => event
            .paths
            .iter()
            .any(|path| path.file_name() == Some(target)),
        Err(_) => false,
    }
}

fn read_only_text(reason: ReadOnlyReason) -> String {
    format!(
        "{} Nothing you type is discarded - it simply is not applied.",
        reason.banner()
    )
}

fn conflict_button(
    id: &'static str,
    label: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(ui.border)
        .bg(ui.surface)
        .text_color(ui.text)
        .cursor_pointer()
        .hover(|style| style.bg(ui.overlay))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
        .into_any_element()
}

fn count_lines(text: &str) -> u32 {
    (text.bytes().filter(|byte| *byte == b'\n').count() + 1) as u32
}

pub(crate) fn base_block_text(base_lines: &[&str], range: &Range<u32>) -> String {
    let start = (range.start as usize).min(base_lines.len());
    let end = (range.end as usize).min(base_lines.len()).max(start);
    let mut text = base_lines[start..end].join("\n");
    if end > start && end < base_lines.len() {
        text.push('\n');
    }
    text
}

pub(crate) fn doc_line_range(doc: &CodeDocument, lines: &Range<u32>) -> Range<usize> {
    let line_count = doc.line_count();
    let start = (lines.start as usize).min(line_count);
    let end = (lines.end as usize).min(line_count).max(start);
    let byte_at = |line: usize| {
        if line < line_count {
            doc.line_to_byte(line)
        } else {
            doc.len_bytes()
        }
    };
    byte_at(start)..byte_at(end)
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        format!("{count} {word}s")
    }
}

pub(crate) fn popup_title(block: &Block) -> String {
    match block.kind() {
        BlockKind::Added => format!("Added {}", plural(block.lines.len(), "line")),
        BlockKind::Deleted => {
            let lines = plural(block.base_lines.len(), "line");
            if block.lines.start == 0 {
                format!("Deleted {lines} at the top")
            } else {
                format!("Deleted {lines} after {}", block.lines.start)
            }
        }
        BlockKind::Modified => {
            let first = block.lines.start + 1;
            let last = block.lines.end;
            if first == last {
                format!("Modified line {first}")
            } else {
                format!("Modified lines {first}-{last}")
            }
        }
    }
}

pub(crate) fn popup_width(editor_w: f32) -> f32 {
    let available = (editor_w - 2.0 * POPUP_MARGIN).max(0.0);
    available.min(POPUP_MAX_W).max(POPUP_MIN_W.min(available))
}

fn popup_height_estimate(kind: BlockKind, shown: usize, hidden: usize) -> f32 {
    let code = if kind == BlockKind::Added {
        0.0
    } else {
        (shown as f32).min(POPUP_VISIBLE_ROWS) * CODE_ROW_HEIGHT + 2.0 * POPUP_PADDING
    };
    let footer = if hidden > 0 { POPUP_FOOTER_H } else { 0.0 };
    POPUP_HEADER_H + code + footer + POPUP_ACTIONS_H + 2.0 * POPUP_PADDING
}

pub(crate) fn popup_anchor(
    row_top: f32,
    row_bottom: f32,
    popup_h: f32,
    viewport_bottom: f32,
) -> (Anchor, f32) {
    if row_bottom + popup_h > viewport_bottom && row_top - popup_h >= 0.0 {
        (Anchor::BottomLeft, row_top)
    } else {
        (Anchor::TopLeft, row_bottom)
    }
}

impl CodeView {
    fn start_base_load(&mut self, cx: &mut Context<Self>) {
        let generation = self.slot.current();
        spawn_base_load(
            self.path.clone(),
            generation,
            cx,
            |view: &mut Self, generation, base: Base, cx| {
                if !view.slot.accept(generation) {
                    return;
                }
                view.install_base(base, cx);
            },
        );
    }

    pub(crate) fn reload_base(&mut self, cx: &mut Context<Self>) {
        if self.state.document().is_none() {
            return;
        }
        self.start_base_load(cx);
    }

    fn install_base(&mut self, base: Base, cx: &mut Context<Self>) {
        let same_commit = matches!(
            (self.base.head_sha(), base.head_sha()),
            (Some(current), Some(next)) if current == next
        );
        self.base = base;
        if same_commit && self.tracker.is_active() {
            return;
        }
        self.popup = None;
        self.hovered_marker = None;
        self.reset_tracker(cx);
        cx.notify();
    }

    fn reset_tracker(&mut self, cx: &mut Context<Self>) {
        let doc_lines = self.state.document().map(CodeDocument::line_count);
        match (doc_lines, self.base.text()) {
            (Some(doc_lines), Some(text)) => {
                self.tracker = BlockTracker::fresh(doc_lines as u32, count_lines(text));
                self.schedule_tracker_refresh(cx);
            }
            _ => {
                self.tracker = BlockTracker::inactive();
                self.tracker_generation = self.tracker_generation.wrapping_add(1);
            }
        }
    }

    fn note_changes(&mut self, changes: &[TrackerWindow], cx: &mut Context<Self>) {
        if !self.tracker.is_active() {
            return;
        }
        for window in changes {
            self.tracker
                .range_changed(window.start_line, window.before_len, window.after_len);
        }
        self.schedule_tracker_refresh(cx);
    }

    fn schedule_tracker_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.tracker.is_active() || !self.tracker.is_dirty() {
            return;
        }
        self.tracker_generation = self.tracker_generation.wrapping_add(1);
        let generation = self.tracker_generation;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(TRACKER_DEBOUNCE).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if view.tracker_generation == generation {
                        view.refresh_tracker_now(cx);
                    }
                });
            });
        })
        .detach();
    }

    fn refresh_tracker_now(&mut self, cx: &mut Context<Self>) {
        if !self.tracker.is_active() || !self.tracker.is_dirty() {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        let Some(base) = self.base.text().cloned() else {
            return;
        };
        let base_sha = self.base.head_sha().map(str::to_string);
        let revision = doc.revision();
        let rope = doc.text().clone();
        let tracker = self.tracker.clone();
        let load_generation = self.slot.current();
        self.tracker_generation = self.tracker_generation.wrapping_add(1);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let compute = move || {
                let mut tracker = tracker;
                let text = rope.to_string();
                let doc_lines = split_lines(&text);
                let base_lines = split_lines(&base);
                tracker.refresh_dirty(&doc_lines, &base_lines, TRACKER_POLICY);
                tracker
            };
            #[cfg(not(test))]
            let tracker = smol::unblock(compute).await;
            #[cfg(test)]
            let tracker = cx.background_spawn(async move { compute() }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if !view.slot.accept(load_generation) || !view.tracker.is_active() {
                        return;
                    }
                    let current = view.state.document().map(CodeDocument::revision);
                    let same_base = view.base.head_sha() == base_sha.as_deref();
                    if same_base && current == Some(revision) {
                        view.tracker = tracker;
                        cx.notify();
                    } else {
                        view.schedule_tracker_refresh(cx);
                    }
                });
            });
        })
        .detach();
    }

    pub(crate) fn marker_blocks(&self) -> &[Block] {
        self.tracker.blocks()
    }

    pub(crate) fn hovered_marker(&self) -> Option<usize> {
        self.hovered_marker
            .filter(|index| *index < self.tracker.blocks().len())
    }

    fn on_marker_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.text_drag.is_some() || self.navigation.drag.is_some() {
            return;
        }
        let hit = self.hits.borrow().marker_at(ev.position);
        if hit != self.hovered_marker {
            self.hovered_marker = hit;
            cx.notify();
        }
    }

    fn on_marker_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.scroll.bounds().contains(&ev.position) {
            return false;
        }
        let Some(index) = self.hits.borrow().marker_at(ev.position) else {
            return false;
        };
        window.focus(&self.focus, cx);
        self.end_typing_group();
        self.open_marker_popup(index, cx);
        true
    }

    fn open_marker_popup(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(block) = self.tracker.blocks().get(index).cloned() else {
            return;
        };
        let Some(doc) = self.state.document() else {
            return;
        };
        let Some(base) = self.base.text() else {
            return;
        };
        let base_lines = split_lines(base);
        let start = (block.base_lines.start as usize).min(base_lines.len());
        let end = (block.base_lines.end as usize)
            .min(base_lines.len())
            .max(start);
        let block_lines = &base_lines[start..end];
        let shown_count = block_lines.len().min(POPUP_SHOWN_LINES);
        let shown_text = block_lines[..shown_count].join("\n");
        let syntax = DiffSyntax::from_theme(&crate::theme::active_theme());
        let runs = highlight_lines(&shown_text, doc.ext(), &syntax);
        let shown = block_lines[..shown_count]
            .iter()
            .zip(runs.into_iter().chain(std::iter::repeat_with(Vec::new)))
            .map(|(line, runs)| (SharedString::from(line.to_string()), runs))
            .collect();
        let popup = MarkerPopup {
            title: popup_title(&block),
            shown,
            hidden: block_lines.len() - shown_count,
            base_text: base_block_text(&base_lines, &block.base_lines),
            block,
        };
        self.popup = Some(popup);
        cx.notify();
    }

    fn close_marker_popup(&mut self, cx: &mut Context<Self>) {
        if self.popup.take().is_some() {
            cx.notify();
        }
    }

    fn copy_popup_base(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = &self.popup {
            cx.write_to_clipboard(ClipboardItem::new_string(popup.base_text.clone()));
        }
    }

    fn revert_from_popup(&mut self, cx: &mut Context<Self>) {
        let Some(popup) = self.popup.take() else {
            return;
        };
        cx.notify();
        self.revert_block(popup.block.lines.start as usize, cx);
    }

    pub(crate) fn revert_block(&mut self, line: usize, cx: &mut Context<Self>) -> bool {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            self.flash_read_only(cx);
            return false;
        }
        let Some((_, block)) = self.tracker.block_at(line as u32) else {
            log::debug!(
                "revert: no changed block at line {} of {}",
                line + 1,
                self.path.display()
            );
            return false;
        };
        let block = block.clone();
        let (range, replacement) = {
            let Some(doc) = self.state.document() else {
                return false;
            };
            let Some(base) = self.base.text() else {
                return false;
            };
            let base_lines = split_lines(base);
            (
                doc_line_range(doc, &block.lines),
                base_block_text(&base_lines, &block.base_lines),
            )
        };
        let caret = CodeSelection::at(range.start);
        self.end_typing_group();
        if !self.splice_all(&[(range, replacement)], caret, EditGroup::Atomic, cx) {
            return false;
        }
        self.refresh_tracker_now(cx);
        true
    }

    fn render_marker_popup(
        &self,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popup = self.popup.as_ref()?;
        let bounds = self.scroll.bounds();
        let (row_top, anchor_x) = {
            let hits = self.hits.borrow();
            (
                hits.row_top(popup.block.lines.start as usize),
                hits.marker_x + MARKER_COLUMN_W,
            )
        };
        let kind = popup.block.kind();
        let row_bottom = if kind == BlockKind::Deleted {
            row_top
        } else {
            row_top + CODE_ROW_HEIGHT
        };
        let width = popup_width(f32::from(bounds.size.width));
        let height = popup_height_estimate(kind, popup.shown.len(), popup.hidden);
        let (anchor, anchor_y) = popup_anchor(
            row_top,
            row_bottom,
            height,
            f32::from(window.viewport_size().height),
        );
        let font = code_font();

        let mut panel = menu_surface(div().id("code-marker-popup"), ui)
            .flex()
            .flex_col()
            .w(px(width))
            .p(px(POPUP_PADDING))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _w, cx| this.close_marker_popup(cx)),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(POPUP_HEADER_H))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_size(crate::ui_primitives::BODY)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child(SharedString::from(popup.title.clone())),
            );
        if kind != BlockKind::Added {
            let lines = popup.shown.iter().map(|(text, syntax)| {
                let runs = syntax_text_runs(text, syntax, &font, ui.text);
                div()
                    .flex_none()
                    .h(px(CODE_ROW_HEIGHT))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .font_family(font.family.clone())
                    .text_size(px(CODE_FONT_SIZE))
                    .child(StyledText::new(text.clone()).with_runs(runs))
                    .into_any_element()
            });
            panel = panel.child(
                div()
                    .id("code-marker-popup-lines")
                    .flex_none()
                    .max_h(px(
                        POPUP_VISIBLE_ROWS * CODE_ROW_HEIGHT + 2.0 * POPUP_PADDING
                    ))
                    .overflow_y_scroll()
                    .mx(px(2.))
                    .py(px(POPUP_PADDING))
                    .px(px(8.))
                    .rounded(px(6.))
                    .bg(ui.vc_deleted_background)
                    .flex()
                    .flex_col()
                    .children(lines),
            );
            if popup.hidden > 0 {
                panel = panel.child(
                    div()
                        .flex_none()
                        .h(px(POPUP_FOOTER_H))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .text_color(ui.muted)
                        .child(SharedString::from(format!(
                            "and {} more lines",
                            popup.hidden
                        ))),
                );
            }
        }
        let mut actions = div()
            .flex_none()
            .h(px(POPUP_ACTIONS_H))
            .px(px(4.))
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(6.))
            .text_size(crate::ui_primitives::BODY);
        if kind != BlockKind::Added {
            actions = actions.child(conflict_button(
                "code-marker-copy",
                "Copy",
                ui,
                cx.listener(|this, _: &MouseDownEvent, _w, cx| this.copy_popup_base(cx)),
            ));
        }
        actions = actions.child(conflict_button(
            "code-marker-revert",
            "Revert",
            ui,
            cx.listener(|this, _: &MouseDownEvent, _w, cx| this.revert_from_popup(cx)),
        ));
        panel = panel.child(actions);

        Some(
            deferred(
                anchored()
                    .anchor(anchor)
                    .position(point(px(anchor_x), px(anchor_y)))
                    .child(panel),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

impl EntityInputHandler for CodeView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let doc = self.state.document()?;
        let start = doc.utf16_to_byte(range_utf16.start);
        let end = doc.utf16_to_byte(range_utf16.end).max(start);
        *adjusted_range = Some(doc.byte_to_utf16(start)..doc.byte_to_utf16(end));
        Some(doc.slice_string(start..end))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let doc = self.state.document()?;
        let range = self.selection.range();
        Some(UTF16Selection {
            range: doc.byte_to_utf16(range.start)..doc.byte_to_utf16(range.end),
            reversed: self.selection.head < self.selection.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        let marked = self.marked.clone()?;
        Some(doc.byte_to_utf16(marked.start)..doc.byte_to_utf16(marked.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            self.history.close_group();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.resolve_replacement(range_utf16) else {
            return;
        };
        self.marked = None;
        let inserted = normalize_newlines(text).into_owned();
        let caret = CodeSelection::at(range.start + inserted.len());
        self.splice_all(&[(range, inserted)], caret, EditGroup::Typing, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.resolve_replacement(range_utf16) else {
            return;
        };
        let inserted = normalize_newlines(new_text).into_owned();
        let start = range.start;
        let end = start + inserted.len();
        let caret = CodeSelection::at(end);
        if !self.splice_all(&[(range, inserted)], caret, EditGroup::Typing, cx) {
            return;
        }
        self.marked = if start == end { None } else { Some(start..end) };
        if let Some(selected) = new_selected_range_utf16
            && let Some(doc) = self.state.document()
        {
            let base = doc.byte_to_utf16(start);
            let head = doc.utf16_to_byte(base + selected.end);
            let anchor = doc.utf16_to_byte(base + selected.start);
            self.selection = CodeSelection { anchor, head };
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry.get();
        let doc = self.state.document()?;
        let start = doc.utf16_to_byte(range_utf16.start);
        let row = doc.byte_to_line(start);
        let column = cursor::goal_column(doc, start);
        let hits = self.hits.borrow();
        let (x, y) = if hits.lines.is_empty() {
            (
                f32::from(element_bounds.origin.x),
                f32::from(element_bounds.origin.y),
            )
        } else {
            (
                hits.text_x + column as f32 * geometry.char_w,
                hits.top_y + row.saturating_sub(hits.first_row) as f32 * CODE_ROW_HEIGHT,
            )
        };
        Some(Bounds {
            origin: Point::new(px(x), px(y)),
            size: size(px(geometry.char_w.max(1.0)), px(CODE_ROW_HEIGHT)),
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let doc = self.state.document()?;
        let offset = self.hits.borrow().offset_at(doc, point);
        Some(doc.byte_to_utf16(offset))
    }
}

impl Focusable for CodeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_focus_observers(window, cx);
        self.sync_theme();
        self.fill_visible_highlights(window);
        let ui = crate::theme::ui_colors();

        let Some(doc) = self.state.document() else {
            return match self.state.error_message() {
                Some(message) => self.render_load_error(message, ui, cx),
                None => super::super::render::diff_panel_centered(
                    "icons/loader-circle.svg",
                    "Loading file…",
                    ui,
                ),
            };
        };

        self.scroll.set_line_count(doc.line_count());
        let banners = self.banners(ui, cx);
        let popup = self.render_marker_popup(ui, window, cx);
        let theme = crate::theme::active_theme();
        let focused = self.focus.is_focused(window);
        let element = CodeElement::new(
            cx.entity(),
            palette(ui),
            CodeColors {
                scrollbar_thumb: theme.scrollbar_thumb,
                cursor: theme.cursor,
                selection: theme.selection,
                selection_fg: theme.selection_foreground,
                marker_added: ui.vc_added,
                marker_modified: ui.vc_modified,
                marker_deleted: ui.vc_deleted,
            },
            self.scroll.clone(),
            self.h_offset,
            CodeCaret {
                cursor: self.selection.cursor(),
                selection: self.selection.range(),
                focused,
                visible: self.blink_visible,
                marked: self.marked.clone().unwrap_or(0..0),
            },
            self.geometry.clone(),
            self.gutter_memo.clone(),
            self.hits.clone(),
        );

        let host = div()
            .id(self.element_id.clone())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .line_height(px(CODE_ROW_HEIGHT))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered && this.navigation.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if this.on_scrollbar_down(ev, cx)
                        || this.on_marker_down(ev, window, cx)
                        || this.on_text_down(ev, window, cx)
                    {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                this.apply_wheel(ev, cx);
            }))
            .child(element);

        div()
            .id("code-view-body")
            .key_context(CODE_KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::select_page_up))
            .on_action(cx.listener(Self::select_page_down))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste_action))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::save_action))
            .on_action(cx.listener(Self::escape))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                this.on_scrollbar_move(ev, cx);
                this.on_text_move(ev, cx);
                this.on_marker_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _w, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _w, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                }),
            )
            .children(banners)
            .child(host)
            .children(popup)
            .into_any_element()
    }
}

#[cfg(test)]
impl CodeView {
    fn disk_changed(
        &mut self,
        stamp: Option<FileStamp>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let present = text.is_some();
        let text = text.unwrap_or_default();
        let Some(diff) = self.begin_disk_reload(stamp, present, false, cx) else {
            return;
        };
        let splices = edit::disk_splices(&diff.rope, &text);
        self.finish_disk_reload(diff.revision, splices, false, false, cx);
    }

    fn adopt_disk_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let splices = edit::disk_splices(doc.text(), text);
        self.apply_disk_splices(&splices, cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, TestAppContext, TouchPhase, VisualTestContext, point};

    use super::super::highlight::{CodeHighlighter, MAX_QUERY_ROWS};
    use super::super::load::{LoadedCode, build_document};
    use super::*;

    fn view<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        view_with_budget(cx, text, HIGHLIGHT_FRAME_BUDGET)
    }

    fn view_with_budget<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
        budget: Duration,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        view_named(cx, "/nonexistent/paneflow-code.rs", text, budget)
    }

    fn view_named<'a>(
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

    fn tracked_view<'a>(
        cx: &'a mut TestAppContext,
        base: &str,
        text: &str,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        let (view, cx) = view(cx, text);
        view.update(cx, |view, cx| {
            view.install_base(
                Base::Text {
                    text: base.into(),
                    head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
                },
                cx,
            );
        });
        settle_tracker(cx);
        (view, cx)
    }

    fn settle_tracker(cx: &mut VisualTestContext) {
        for _ in 0..4 {
            cx.executor().advance_clock(TRACKER_DEBOUNCE);
            cx.run_until_parked();
        }
    }

    fn blocks_of(view: &CodeView) -> Vec<(BlockKind, Range<u32>, Range<u32>)> {
        view.marker_blocks()
            .iter()
            .map(|block| (block.kind(), block.lines.clone(), block.base_lines.clone()))
            .collect()
    }

    fn rows_of_code(rows: usize) -> String {
        (0..rows).map(|row| format!("fn f{row}() {{}}\n")).collect()
    }

    const VIEWPORT: Point<Pixels> = Point {
        x: px(800.),
        y: px(360.),
    };

    fn scrolled<'a>(
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

    fn wheel(delta: ScrollDelta) -> ScrollWheelEvent {
        ScrollWheelEvent {
            position: point(VIEWPORT.x / 2., VIEWPORT.y / 2.),
            delta,
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }
    }

    fn notification_counter(
        view: &Entity<CodeView>,
        cx: &mut VisualTestContext,
    ) -> (Rc<Cell<usize>>, gpui::Subscription) {
        let count = Rc::new(Cell::new(0usize));
        let seen = count.clone();
        let subscription =
            cx.update(|_, cx| cx.observe(view, move |_, _| seen.set(seen.get() + 1)));
        (count, subscription)
    }

    #[gpui::test]
    fn a_wheel_notch_scrolls_three_rows(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/wheel.rs", &rows_of_code(500));

        cx.simulate_event(wheel(ScrollDelta::Lines(point(0.0, -3.0))));
        cx.run_until_parked();

        assert_eq!(
            view.read_with(cx, |view, _| view.scroll_offset_y()),
            3.0 * CODE_ROW_HEIGHT,
            "a notch must move exactly three rows"
        );
        assert_eq!(view.read_with(cx, |view, _| view.scroll_rows()), 3.0);
    }

    #[gpui::test]
    fn a_trackpad_delta_scrolls_its_exact_pixels(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/trackpad.rs", &rows_of_code(500));

        cx.simulate_event(wheel(ScrollDelta::Pixels(point(px(0.), px(-7.5)))));
        cx.run_until_parked();

        assert_eq!(view.read_with(cx, |view, _| view.scroll_offset_y()), 7.5);
    }

    #[gpui::test]
    fn a_horizontal_notch_moves_whole_columns(cx: &mut TestAppContext) {
        let mut text = "x".repeat(400);
        text.push('\n');
        text.push_str(&rows_of_code(200));
        let (view, cx) = scrolled(cx, "/nonexistent/wide.rs", &text);

        let char_w = view.read_with(cx, |view, _| view.geometry.get().char_w);
        assert!(char_w > 0.0, "the test text system must measure a column");

        cx.simulate_event(wheel(ScrollDelta::Lines(point(-1.0, 0.0))));
        cx.run_until_parked();

        let (h_offset, rows) = view.read_with(cx, |view, _| (view.h_offset, view.scroll_rows()));
        assert_eq!(h_offset, char_w, "one notch is one column, not one line");
        assert_eq!(rows, 0.0, "a horizontal notch must not scroll vertically");

        cx.simulate_event(wheel(ScrollDelta::Lines(point(-2.0, 0.0))));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.h_offset), 3.0 * char_w);
    }

    #[gpui::test]
    fn a_document_shorter_than_the_viewport_absorbs_the_notch(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/short.rs", &rows_of_code(3));
        let (notifications, _subscription) = notification_counter(&view, cx);

        cx.simulate_event(wheel(ScrollDelta::Lines(point(0.0, -3.0))));
        cx.run_until_parked();

        assert_eq!(view.read_with(cx, |view, _| view.scroll_offset_y()), 0.0);
        assert_eq!(
            notifications.get(),
            0,
            "an absorbed notch must not repaint the editor"
        );
    }

    #[gpui::test]
    fn notches_in_one_frame_coalesce_into_a_single_notification(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/coalesce.rs", &rows_of_code(500));
        let (notifications, _subscription) = notification_counter(&view, cx);

        cx.update(|window, cx| {
            for _ in 0..3 {
                window.dispatch_event(
                    gpui::PlatformInput::ScrollWheel(wheel(ScrollDelta::Lines(point(0.0, -3.0)))),
                    cx,
                );
            }
        });
        cx.run_until_parked();

        assert_eq!(
            view.read_with(cx, |view, _| view.scroll_rows()),
            9.0,
            "the three deltas must all land"
        );
        assert_eq!(
            notifications.get(),
            1,
            "three notches inside one frame are one repaint"
        );
    }

    #[gpui::test]
    fn the_last_row_of_a_huge_file_lands_on_the_viewport_floor(cx: &mut TestAppContext) {
        let line_count = 300_000usize;
        let text = rows_of_code(line_count);
        let (view, cx) = scrolled(cx, "/nonexistent/huge.rs", &text);

        view.update(cx, |view, cx| {
            view.scroll.set_rows(view.scroll.max_rows());
            cx.notify();
        });
        cx.run_until_parked();

        let (last, first, floor) = view.read_with(cx, |view, _| {
            let last = view.document().expect("a loaded document").line_count() - 1;
            (
                last,
                view.row_top(last),
                f32::from(view.scroll.bounds().bottom()),
            )
        });
        assert!(last >= line_count, "{last} must reach past {line_count}");
        assert!(
            (first + CODE_ROW_HEIGHT - floor).abs() < 1.0,
            "the last row must sit on the viewport floor, got {first} for a floor at {floor}"
        );

        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.row_top(last)),
            first,
            "two identical frames must place the last row identically"
        );

        view.update(cx, |view, cx| {
            view.scroll.set_rows(view.scroll.rows() - 1.0);
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.row_top(last)),
            first + CODE_ROW_HEIGHT,
            "one scrolled row must move the last row by exactly one row height"
        );
    }

    #[gpui::test]
    fn the_scrollbar_drives_the_owned_position(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/bar.rs", &rows_of_code(2_000));

        let thumb_h = view.update(cx, |view, cx| {
            let thumb = view
                .navigation
                .layout
                .get()
                .vertical
                .and_then(|track| track.thumb)
                .expect("an overflowing document");
            let bar_x = thumb.right() - px(3.);
            let below_thumb = thumb.bottom() + px(40.0);
            assert!(view.on_scrollbar_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(bar_x, below_thumb),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                },
                cx,
            ));
            f32::from(thumb.size.height)
        });
        cx.run_until_parked();
        let after_click = view.read_with(cx, |view, _| view.scroll_rows());
        assert!(after_click > 0.0, "a track click must move the position");

        view.update(cx, |view, cx| {
            let thumb = view
                .navigation
                .layout
                .get()
                .vertical
                .and_then(|track| track.thumb)
                .expect("an overflowing document");
            let bar_x = thumb.right() - px(3.);
            let thumb_y = thumb.origin.y + px(thumb_h / 2.0);
            view.on_scrollbar_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(bar_x, thumb_y),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                },
                cx,
            );
            view.on_scrollbar_move(
                &MouseMoveEvent {
                    position: point(bar_x, thumb_y + px(60.)),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                },
                cx,
            );
        });
        let after_drag = view.read_with(cx, |view, _| view.scroll_rows());
        assert!(
            after_drag > after_click,
            "dragging the thumb down must advance the position, {after_click} -> {after_drag}"
        );
    }

    #[gpui::test]
    fn an_external_reload_that_drops_lines_rebinds_the_position(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/reload.rs", &rows_of_code(500));

        view.update(cx, |view, cx| {
            view.scroll.set_rows(view.scroll.max_rows());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.scroll_rows()) > 400.0);

        view.update(cx, |view, cx| {
            view.adopt_disk_text(&rows_of_code(25), cx);
        });
        cx.run_until_parked();

        let (rows, max_rows, viewport_h) = view.read_with(cx, |view, _| {
            (
                view.scroll_rows(),
                view.scroll.max_rows(),
                view.scroll.viewport_height(),
            )
        });
        let line_count = view.read_with(cx, |view, _| {
            view.document().expect("a loaded document").line_count()
        });
        assert_eq!(
            max_rows,
            line_count as f64 - f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT)
        );
        assert_eq!(rows, max_rows, "the position must stop at the new end");
    }

    fn frame(view: &Entity<CodeView>, cx: &mut VisualTestContext) {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }

    fn scroll_to(view: &Entity<CodeView>, cx: &mut VisualTestContext, rows: f64) {
        view.update(cx, |view, cx| {
            view.scroll.set_rows(rows);
            cx.notify();
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_warm_frame_shapes_nothing_it_already_shaped(cx: &mut TestAppContext) {
        let text: String = (0..400)
            .map(|row| format!("row {row} of plain text\n"))
            .collect();
        let (view, cx) = scrolled(cx, "/nonexistent/warm.txt", &text);

        frame(&view, cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_lines()),
            0,
            "a warm frame must not build a single line string"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_numbers()),
            0,
            "a warm frame must not build a single number string"
        );

        scroll_to(&view, cx, 200.0);
        frame(&view, cx);
        scroll_to(&view, cx, 0.0);
        assert!(
            view.read_with(cx, |view, _| view.materialized_lines()) > 0,
            "rows the layout cache dropped must be shaped again"
        );
    }

    #[gpui::test]
    fn an_edit_only_reshapes_the_row_it_touched(cx: &mut TestAppContext) {
        let rows = 100;
        let text: String = (0..rows)
            .map(|row| format!("row {row} of plain text\n"))
            .collect();
        let (view, cx) = scrolled(cx, "/nonexistent/edit.txt", &text);

        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.materialized_lines()), 0);

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 0 };
            view.replace_text_in_range(None, "z", window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_lines()),
            1,
            "only the edited row may miss the layout cache"
        );
    }

    #[gpui::test]
    fn identical_rows_share_one_shaped_line(cx: &mut TestAppContext) {
        let text: String = (0..400)
            .map(|row| {
                if row % 2 == 0 {
                    "same line\n"
                } else {
                    "other line\n"
                }
            })
            .collect();
        let (view, cx) = scrolled(cx, "/nonexistent/twins.txt", &text);

        let (even, odd, twin) = view.read_with(cx, |view, _| {
            (view.row_width(0), view.row_width(1), view.row_width(2))
        });
        assert_eq!(even, twin, "identical rows must carry identical layouts");
        assert_ne!(even, odd, "the probe needs two measurably different texts");

        frame(&view, cx);
        scroll_to(&view, cx, 200.0);

        let visible = view.read_with(cx, |view, _| view.visible_row_range().len());
        assert!(visible > 4, "the probe needs more rows than distinct texts");
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_lines()),
            0,
            "{visible} rows of already shaped texts must all hit at their new indices"
        );
    }

    #[gpui::test]
    fn the_ime_reads_the_caret_from_the_painted_rows(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/ime.rs", "let alpha = 1;\nlet beta = 2;\n");

        let (caret, index) = view.update_in(cx, |view, window, cx| {
            let element_bounds = view.scroll.bounds();
            let caret = view
                .bounds_for_range(4..9, element_bounds, window, cx)
                .expect("a laid out row");
            let index = view
                .character_index_for_point(caret.origin, window, cx)
                .expect("a laid out row");
            (caret, index)
        });
        assert_eq!(index, 4, "the IME round trips the caret it was given");
        assert_eq!(f32::from(caret.size.height), CODE_ROW_HEIGHT);
        assert_eq!(
            f32::from(caret.origin.y),
            view.read_with(cx, |view, _| view.row_top(0)),
            "the IME caret sits on the painted row"
        );
    }

    #[gpui::test]
    fn a_starved_fill_schedules_the_next_frame_itself(cx: &mut TestAppContext) {
        let text = rows_of_code(500);
        let (view, cx) = view_with_budget(cx, &text, Duration::ZERO);
        cx.simulate_resize(size(px(800.), px(6_000.)));
        cx.run_until_parked();
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();

        let visible = view.read_with(cx, |view, _| view.visible_row_range());
        assert!(
            visible.len() > MAX_QUERY_ROWS,
            "the probe needs a viewport taller than one query slice, got {visible:?}"
        );

        let mut stale = view.read_with(cx, |view, _| view.stale_visible_rows());
        assert!(stale > 0, "a zero budget must leave visible rows stale");

        let mut frames = 0usize;
        while stale > 0 {
            assert_eq!(
                cx.update(|window, cx| window.simulate_next_frame(cx)),
                1,
                "a starved fill must schedule its own follow-up frame"
            );
            cx.run_until_parked();
            let left = view.read_with(cx, |view, _| view.stale_visible_rows());
            assert!(
                left < stale,
                "a follow-up frame must colour more rows, {stale} -> {left}"
            );
            stale = left;
            frames += 1;
            assert!(frames < 8, "the progressive fill must converge");
        }

        assert_eq!(
            cx.update(|window, cx| window.simulate_next_frame(cx)),
            0,
            "a fresh viewport must not schedule another frame"
        );
    }

    #[gpui::test]
    fn a_fresh_viewport_schedules_no_frame(cx: &mut TestAppContext) {
        let text = rows_of_code(40);
        let (view, cx) = view(cx, &text);
        cx.run_until_parked();

        assert_eq!(view.read_with(cx, |view, _| view.stale_visible_rows()), 0);
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    }

    #[gpui::test]
    fn a_loading_document_never_asks_for_a_frame(cx: &mut TestAppContext) {
        let (view, cx) = view_with_budget(cx, "", Duration::ZERO);
        cx.run_until_parked();

        let scheduled = cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            view.update(cx, |view, _cx| view.fill_visible_highlights(window));
            window.simulate_next_frame(cx)
        });
        assert_eq!(
            scheduled, 0,
            "while the document loads only the spinner may drive frames"
        );
    }

    #[gpui::test]
    fn a_file_past_the_highlight_cap_schedules_no_frame(cx: &mut TestAppContext) {
        let mut text = String::with_capacity(crate::diff::MAX_HIGHLIGHT_BYTES + 64);
        while text.len() <= crate::diff::MAX_HIGHLIGHT_BYTES {
            text.push_str("pub fn f() -> i32 { 1 }\n");
        }
        let (view, cx) = view_named(cx, "/nonexistent/huge.rs", &text, Duration::ZERO);
        cx.run_until_parked();

        assert_eq!(view.read_with(cx, |view, _| view.stale_visible_rows()), 0);
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
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

    #[gpui::test]
    fn a_move_without_the_left_button_ends_the_drag(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/drag.rs", &rows_of_code(500));

        view.update(cx, |view, cx| {
            let thumb = view
                .navigation
                .layout
                .get()
                .vertical
                .and_then(|track| track.thumb)
                .expect("an overflowing document");
            let position = thumb.center();
            let down = MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            };
            assert!(view.on_scrollbar_down(&down, cx));
            view.on_scrollbar_move(
                &MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                },
                cx,
            );
            assert!(view.navigation.drag.is_none());

            assert!(view.on_scrollbar_down(&down, cx));
            view.on_scrollbar_move(
                &MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                },
                cx,
            );
            assert!(view.navigation.drag.is_some());
        });
    }

    #[gpui::test]
    fn the_header_reads_the_caret_as_one_based_line_and_column(cx: &mut TestAppContext) {
        let (editor, cx) = view(cx, "let foo = 1;\nbb\nlast line");

        editor.update_in(cx, |view, window, cx| {
            assert_eq!(view.cursor_line_column(), (1, 1));

            view.right(&CeRight, window, cx);
            view.right(&CeRight, window, cx);
            view.right(&CeRight, window, cx);
            assert_eq!(view.cursor_line_column(), (1, 4));

            view.down(&CeDown, window, cx);
            assert_eq!(
                view.cursor_line_column().0,
                2,
                "the caret moved a line down"
            );

            view.doc_end(&CeDocEnd, window, cx);
            assert_eq!(view.cursor_line_column(), (3, 10), "end of `last line`");
        });

        let (loading, cx) = view(cx, "");
        loading.update(cx, |view, _cx| {
            assert!(view.document().is_none());
            assert_eq!(view.cursor_line_column(), (1, 1));
        });
    }

    #[gpui::test]
    fn the_navigation_actions_walk_the_document(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "let foo = 1;\nbb\nlast line");

        view.update_in(cx, |view, window, cx| {
            view.right(&CeRight, window, cx);
            assert_eq!(view.cursor(), 1);
            view.word_right(&CeWordRight, window, cx);
            assert_eq!(view.cursor(), 3, "end of `let`");
            view.end(&CeEnd, window, cx);
            assert_eq!(view.cursor(), 12);
            view.right(&CeRight, window, cx);
            assert_eq!(
                view.cursor(),
                13,
                "right at a row end steps to the next row"
            );
            view.home(&CeHome, window, cx);
            assert_eq!(view.cursor(), 13);
            view.doc_end(&CeDocEnd, window, cx);
            assert_eq!(view.cursor(), view.document().unwrap().len_bytes());
            view.doc_start(&CeDocStart, window, cx);
            assert_eq!(view.cursor(), 0);
            assert!(view.selection().is_empty(), "plain motion never selects");
        });
    }

    #[gpui::test]
    fn shift_extends_and_select_all_takes_the_document(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "abc\ndef");

        view.update_in(cx, |view, window, cx| {
            view.select_right(&CeSelectRight, window, cx);
            view.select_right(&CeSelectRight, window, cx);
            assert_eq!(view.selection(), 0..2);
            assert_eq!(view.cursor(), 2);

            view.left(&CeLeft, window, cx);
            assert_eq!(view.cursor(), 0, "collapses onto the near edge");
            assert!(view.selection().is_empty());

            view.select_all(&CeSelectAll, window, cx);
            assert_eq!(view.selection(), 0..7);
        });
    }

    #[gpui::test]
    fn vertical_motion_restores_the_goal_column(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "aaaaaaa\nbb\ncccccccc");

        view.update_in(cx, |view, window, cx| {
            view.place_caret(5, false, cx);
            view.down(&CeDown, window, cx);
            assert_eq!(view.cursor(), 10, "clamped to the short row");
            view.down(&CeDown, window, cx);
            assert_eq!(view.cursor(), 16, "the goal column comes back");
        });
    }

    #[gpui::test]
    fn the_caret_clamps_and_a_new_caret_clears_the_selection(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo");

        view.update(cx, |view, cx| {
            view.place_caret(9_999, false, cx);
            assert_eq!(view.cursor(), 7);

            view.take_whole_document(cx);
            assert_eq!(view.selection(), 0..7);
            let before = view.document().unwrap().len_bytes();
            view.place_caret(2, false, cx);
            assert!(view.selection().is_empty(), "the selection is gone");
            assert_eq!(
                view.document().unwrap().len_bytes(),
                before,
                "and the content is untouched"
            );
            assert_eq!(view.cursor_row(), 0, "the row follows the byte offset");
        });
    }

    #[gpui::test]
    fn multi_click_chains_then_resets(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "let foo = 1;\nnext");

        view.update(cx, |view, _cx| {
            let at = point(px(4.), px(4.));
            let now = Instant::now();
            assert_eq!(view.chain_click(at, now), 1);
            assert_eq!(view.chain_click(at, now), 2);
            assert_eq!(view.chain_click(at, now), 3);
            assert_eq!(view.chain_click(at, now), 1, "the chain wraps at three");

            assert_eq!(view.chain_click(at, now), 2);
            assert_eq!(
                view.chain_click(point(px(80.), px(4.)), now),
                1,
                "too far restarts it"
            );
            assert_eq!(
                view.chain_click(
                    point(px(80.), px(4.)),
                    now + MULTI_CLICK_INTERVAL + Duration::from_millis(1)
                ),
                1,
                "too late restarts it"
            );
        });
    }

    #[gpui::test]
    fn a_word_drag_extends_by_whole_words(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "alpha beta gamma");

        view.update(cx, |view, cx| {
            view.selection = CodeSelection {
                anchor: 6,
                head: 10,
            };
            view.text_drag = Some(TextDrag {
                grain: DragGrain::Word,
                anchor: 6..10,
            });
            view.extend_drag_to(2, cx);
            assert_eq!(view.selection(), 0..10, "backward, whole words");
            view.extend_drag_to(13, cx);
            assert_eq!(view.selection(), 6..16, "forward, whole words");
        });
    }

    fn file_view<'a>(
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

    fn file_view_named<'a>(
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

    fn seeded_view(
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

    fn text_of(view: &CodeView) -> String {
        view.document()
            .map(|doc| doc.slice_string(0..doc.len_bytes()))
            .unwrap_or_default()
    }

    #[gpui::test]
    fn typing_replaces_the_live_selection(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "hello world\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 5 };
            view.replace_text_in_range(None, "bye", window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "bye world\n");
            assert_eq!(view.cursor(), 3, "the caret lands past what was inserted");
            assert!(view.is_dirty(), "an edit marks the document dirty");
        });
    }

    #[gpui::test]
    async fn shortening_the_longest_line_refreshes_horizontal_extent_off_thread(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = view(cx, "the longest line\nshort\n");
        cx.executor().allow_parking();

        view.update(cx, |view, cx| {
            assert!(view.splice_all(
                &[(0..16, "tiny".to_string())],
                CodeSelection::at(4),
                EditGroup::Atomic,
                cx,
            ));
            assert_eq!(view.document().expect("document").longest_line_chars(), 16);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| {
                view.document().expect("document").longest_line_chars() == 5
            }) {
                break;
            }
            smol::Timer::after(Duration::from_millis(1)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(view.document().expect("document").longest_line_chars(), 5);
        });
    }

    #[gpui::test]
    fn backspace_removes_a_whole_composed_emoji(cx: &mut TestAppContext) {
        let emoji = "\u{1F44D}\u{1F3FD}";
        let (view, cx) = view(cx, &format!("ok{emoji}\n"));
        let end = 2 + emoji.len();

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(end);
            view.backspace(&CeBackspace, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(
                text_of(view),
                "ok\n",
                "the whole grapheme went in one press"
            );
        });
    }

    #[gpui::test]
    fn enter_repeats_the_row_indentation(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "fn main() {\n    let x = 1;\n}\n");
        let at = "fn main() {\n    let x = 1;".len();

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(at);
            view.newline(&CeNewline, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "fn main() {\n    let x = 1;\n    \n}\n");
            assert_eq!(view.cursor(), at + 5, "the caret sits past the new indent");
        });
    }

    #[gpui::test]
    async fn opening_a_file_shows_its_text_first_and_colors_it_when_the_tree_lands(
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
        cx.executor().allow_parking();
        for _ in 0..300 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| {
                view.state
                    .highlighter()
                    .is_some_and(CodeHighlighter::has_tree)
            }) {
                break;
            }
            smol::Timer::after(Duration::from_millis(10)).await;
        }

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

    #[gpui::test]
    fn a_keystroke_on_a_read_only_document_is_refused_visibly(cx: &mut TestAppContext) {
        let path = PathBuf::from("/nonexistent/paneflow-code.rs");
        let document = build_document(path.clone(), "locked\n", true);
        let mut highlighter = CodeHighlighter::new(
            &document,
            DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
        );
        highlighter.parse_initial_blocking(&document);
        let state = CodeLoadState::Ready(Box::new(LoadedCode {
            document,
            highlighter,
            indent: IndentUnit::Spaces(4),
            stamp: None,
        }));
        let (view, cx) =
            cx.add_window_view(move |_window, cx| CodeView::with_state(path, state, None, cx));

        view.update_in(cx, |view, window, cx| {
            view.replace_text_in_range(None, "x", window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "locked\n", "nothing was written");
            assert!(
                !view.is_dirty(),
                "a refused keystroke leaves no transaction"
            );
            assert!(
                view.read_only_flash.is_some(),
                "the refusal lights the banner up"
            );
        });
    }

    #[gpui::test]
    fn undo_on_a_read_only_document_is_refused_visibly(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");

        view.update(cx, |view, _cx| {
            view.state
                .document_mut()
                .expect("document")
                .set_read_only(Some(ReadOnlyReason::Permissions));
        });

        std::fs::write(&path, "one\ntwo\nthree\n").expect("external write");
        let stamp = FileStamp::read(&path);
        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some("one\ntwo\nthree\n".to_string()), cx);
            assert_eq!(text_of(view), "one\ntwo\nthree\n", "the reload landed");
            assert!(
                !view.is_dirty(),
                "a silent reload leaves the document clean"
            );
        });

        view.update_in(cx, |view, window, cx| {
            view.undo(&CeUndo, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "one\ntwo\nthree\n", "nothing was replayed");
            assert!(!view.is_dirty(), "and the document is still clean");
            assert!(view.read_only_flash.is_some(), "the refusal is visible");
        });
    }

    #[gpui::test]
    fn undo_keeps_the_highlighting_a_fresh_parse_would_give(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "fn main() {\n    let value = 1;\n}\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(16);
            view.replace_text_in_range(None, "xyz", window, cx);
            view.undo(&CeUndo, window, cx);
        });
        cx.run_until_parked();

        view.update(cx, |view, _cx| {
            let (doc, live) = view.state.editable().expect("document and highlighter");
            live.requery_rows(doc, 0..doc.line_count());
            let mut oracle =
                CodeHighlighter::new(doc, DiffSyntax::from_theme(&crate::theme::paneflow_dark()));
            oracle.parse_initial_blocking(doc);
            oracle.requery_rows(doc, 0..doc.line_count());
            assert!(live.is_enabled(), "the grammar is loaded");
            assert!(
                !oracle.runs(1).is_empty(),
                "the oracle colors something, so the comparison means something"
            );
            for row in 0..doc.line_count() {
                assert_eq!(
                    live.runs(row),
                    oracle.runs(row),
                    "row {row} kept its coloring across the undo"
                );
            }
        });
    }

    #[gpui::test]
    fn keystrokes_group_until_the_caret_moves(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "\n");

        view.update_in(cx, |view, window, cx| {
            for letter in ["a", "b", "c"] {
                view.replace_text_in_range(None, letter, window, cx);
            }
            view.left(&CeLeft, window, cx);
            view.right(&CeRight, window, cx);
            view.replace_text_in_range(None, "d", window, cx);
            assert_eq!(text_of(view), "abcd\n");

            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                "abc\n",
                "the post-move keystroke undid alone"
            );
            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                "\n",
                "the three grouped keystrokes undid together"
            );

            view.redo(&CeRedo, window, cx);
            assert_eq!(text_of(view), "abc\n", "redo replays the same grouping");
        });
    }

    #[gpui::test]
    fn undo_restores_the_selection_the_edit_replaced(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "hello world\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection {
                anchor: 6,
                head: 11,
            };
            view.replace_text_in_range(None, "there", window, cx);
            view.undo(&CeUndo, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "hello world\n");
            assert_eq!(view.selection(), 6..11, "the replaced selection came back");
        });
    }

    #[gpui::test]
    fn a_multi_line_paste_is_one_undo_step(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "start\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(6);
            cx.write_to_clipboard(ClipboardItem::new_string("one\r\ntwo\r\nthree".to_string()));
            view.paste_action(&CePaste, window, cx);
            assert_eq!(text_of(view), "start\none\ntwo\nthree");
            assert_eq!(
                view.cursor(),
                text_of(view).len(),
                "the caret is at the end"
            );

            view.undo(&CeUndo, window, cx);
            assert_eq!(text_of(view), "start\n", "the whole paste undid at once");
        });
    }

    #[gpui::test]
    fn a_paste_is_sanitized_before_insertion(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "\n");

        view.update_in(cx, |view, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(
                "let x = 1;\u{202E}\u{0007}\u{200B}".to_string(),
            ));
            view.paste_action(&CePaste, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "let x = 1;\n");
        });
    }

    #[gpui::test]
    fn copy_with_no_selection_takes_the_whole_row(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "first\nsecond\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(8);
            view.copy(&CeCopy, window, cx);
            let clipped = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            assert_eq!(clipped, "second\n");
            assert_eq!(text_of(view), "first\nsecond\n", "copy never mutates");
        });
    }

    #[gpui::test]
    fn tab_and_shift_tab_shift_every_touched_row(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo\nthree\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 8 };
            view.indent(&CeIndent, window, cx);
            assert_eq!(text_of(view), "    one\n    two\nthree\n");

            view.outdent(&CeOutdent, window, cx);
            assert_eq!(text_of(view), "one\ntwo\nthree\n");

            view.outdent(&CeOutdent, window, cx);
            assert_eq!(text_of(view), "one\ntwo\nthree\n");
        });
    }

    #[gpui::test]
    fn saving_writes_the_file_and_settles_the_dirty_mark(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "two\n", window, cx);
            assert!(view.is_dirty());
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\ntwo\n");
        view.update_in(cx, |view, window, cx| {
            assert!(!view.is_dirty(), "a landed save clears the dot");
            assert!(view.save_error.is_none());

            view.replace_text_in_range(None, "x", window, cx);
            assert!(view.is_dirty());
            view.undo(&CeUndo, window, cx);
            assert!(
                !view.is_dirty(),
                "undoing back to the saved state clears the dot again"
            );
        });
    }

    #[gpui::test]
    fn a_save_is_refused_when_the_file_changed_underneath(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, "written by someone else\n").expect("agent write");

        view.update_in(cx, |view, window, cx| {
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "written by someone else\n",
            "the refusal happened before the write"
        );
        view.update(cx, |view, _cx| {
            assert!(view.has_conflict(), "the user is asked to choose");
            assert!(view.is_dirty(), "the in-memory edits survived");
            assert_eq!(text_of(view), "one\nmine\n");
        });
    }

    #[gpui::test]
    async fn an_external_write_reloads_through_the_watcher(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", true);
        let path = dir.path().join("main.rs");
        cx.executor().allow_parking();
        cx.run_until_parked();

        std::fs::write(&path, "one\nAGENT\ntwo\n").expect("agent write");
        for _ in 0..300 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| text_of(view) == "one\nAGENT\ntwo\n") {
                break;
            }
            smol::Timer::after(Duration::from_millis(10)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(
                text_of(view),
                "one\nAGENT\ntwo\n",
                "the watched write reached the document through the background diff"
            );
            assert!(!view.is_dirty(), "a silent reload is the new saved state");
            assert_eq!(view.disk, DiskState::InSync);
            let bridge = view._watch_bridge.take().expect("bridged watcher");
            *bridge.lock().expect("bridge lock") = None;
            view._watcher = None;
        });
    }

    #[gpui::test]
    fn a_reload_that_races_an_edit_recomputes_once_then_conflicts(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            let diff = view
                .begin_disk_reload(stamp, true, false, cx)
                .expect("a clean document starts a diff");
            let splices = edit::disk_splices(&diff.rope, "ONE!\nTWO!\n");
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "x", window, cx);

            let again = view
                .finish_disk_reload(diff.revision, splices, true, false, cx)
                .expect("a stale revision buys exactly one recomputation");
            assert_eq!(
                text_of(view),
                "xone\ntwo\n",
                "splices computed against an older revision are refused"
            );
            assert!(!view.has_conflict(), "the first miss is not a conflict yet");

            let splices = edit::disk_splices(&again.rope, "ONE!\nTWO!\n");
            view.replace_text_in_range(None, "y", window, cx);
            assert!(
                view.finish_disk_reload(again.revision, splices, false, false, cx)
                    .is_none(),
                "the second stale delivery gives up"
            );
            assert!(
                view.has_conflict(),
                "a document that keeps moving is left to the user"
            );
            assert_eq!(text_of(view), "xyone\ntwo\n", "the user edits survived");
        });
    }

    #[gpui::test]
    fn a_reload_that_settles_on_a_dirty_document_keeps_the_user_text(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            let diff = view
                .begin_disk_reload(stamp, true, false, cx)
                .expect("a clean document starts a diff");
            let splices = edit::disk_splices(&diff.rope, "ONE!\nTWO!\n");
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "x", window, cx);

            let again = view
                .finish_disk_reload(diff.revision, splices, true, false, cx)
                .expect("a stale revision buys exactly one recomputation");
            let splices = edit::disk_splices(&again.rope, "ONE!\nTWO!\n");
            assert!(
                view.finish_disk_reload(again.revision, splices, false, false, cx)
                    .is_none(),
                "the recomputed diff reaches a document that stopped moving"
            );
            assert_eq!(
                text_of(view),
                "xone\ntwo\n",
                "a document the user touched during the diff is never overwritten"
            );
            assert!(view.is_dirty(), "the unsaved edit is still unsaved");
            assert!(
                view.has_conflict(),
                "the user resolves it like any other conflict"
            );
        });
    }

    #[gpui::test]
    fn a_forced_reload_still_overwrites_the_document_the_user_edited(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "x", window, cx);
            assert!(view.is_dirty(), "the fixture starts dirty");

            let diff = view
                .begin_disk_reload(stamp, true, true, cx)
                .expect("a forced reload ignores the dirty mark");
            let splices = edit::disk_splices(&diff.rope, "ONE!\nTWO!\n");
            assert!(
                view.finish_disk_reload(diff.revision, splices, false, true, cx)
                    .is_none()
            );
            assert_eq!(
                text_of(view),
                "ONE!\nTWO!\n",
                "discarding my changes is what the user asked for"
            );
            assert!(!view.is_dirty(), "and the reload is the new saved state");
            assert!(!view.has_conflict());
        });
    }

    #[gpui::test]
    async fn a_reload_whose_tab_closed_ends_without_a_panic(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "one\ntwo\n").expect("seed");
        let stamp = FileStamp::read(&path);
        let seeded = seeded_view(path, "one\n");
        let weak = cx.update(|cx| {
            let view = cx.new(seeded);
            let weak = view.downgrade();
            drop(view);
            weak
        });
        cx.run_until_parked();
        assert!(weak.upgrade().is_none(), "the tab is gone");

        let carried = cx
            .spawn(|mut cx| async move {
                reload_from_disk(&weak, &mut cx, stamp, Some("one\ntwo\n".to_string()), false).await
            })
            .await;
        assert!(
            !carried,
            "a reload delivered to a closed tab stops its loop instead of panicking"
        );
    }

    #[gpui::test]
    fn an_external_write_reloads_a_clean_document(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update(cx, |view, cx| {
            view.selection = CodeSelection::at(4);
            view.disk_changed(stamp, Some("ONE!\nTWO!\n".to_string()), cx);
        });

        view.update_in(cx, |view, window, cx| {
            assert_eq!(text_of(view), "ONE!\nTWO!\n");
            assert!(!view.has_conflict(), "a clean document reloads silently");
            assert!(!view.is_dirty(), "the reload is the new saved state");
            assert_eq!(
                view.cursor(),
                4,
                "the caret held: the line count is unchanged"
            );

            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                "one\ntwo\n",
                "Ctrl+Z recovers what was replaced"
            );
        });
    }

    #[gpui::test]
    fn a_multi_hunk_reload_reaches_the_highlighter_as_one_batch(cx: &mut TestAppContext) {
        let original = (0..40)
            .map(|row| format!("fn f{row:03}() {{}}\n"))
            .collect::<String>();
        let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
        lines[5] = "fn agent_a() {}".to_string();
        lines[25] = "fn agent_b() {}".to_string();
        let incoming = lines.join("\n") + "\n";
        let (dir, view, cx) = file_view(cx, &original, false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, &incoming).expect("agent write");
        let stamp = FileStamp::read(&path);

        let before = view.update(cx, |view, _cx| {
            let highlighter = view.highlighter().expect("highlighter");
            assert!(highlighter.is_enabled(), "the fixture must be colored");
            highlighter.generation()
        });

        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some(incoming.clone()), cx);
            assert_eq!(text_of(view), incoming, "both hunks landed");
            assert_eq!(
                view.highlighter().expect("highlighter").generation(),
                before + 1,
                "two hunks reach the highlighter as a single batched edit"
            );
        });
    }

    #[gpui::test]
    fn an_external_write_keeps_a_distant_caret_and_undoes_all_hunks_once(cx: &mut TestAppContext) {
        let original = (0..30)
            .map(|row| format!("line {row:03}\n"))
            .collect::<String>();
        let mut incoming_lines = original.lines().map(str::to_string).collect::<Vec<_>>();
        for (row, line) in incoming_lines.iter_mut().enumerate().take(8).skip(5) {
            *line = format!("agent changed line {row:03}");
        }
        incoming_lines[15] = "second distant hunk".to_string();
        let incoming = incoming_lines.join("\n") + "\n";
        let (dir, view, cx) = file_view_named(cx, "main.txt", &original, false);
        let path = dir.path().join("main.txt");
        std::fs::write(&path, &incoming).expect("agent write");
        let stamp = FileStamp::read(&path);
        let caret = original.find("line 025").expect("caret line") + 5;
        let expected = incoming.find("line 025").expect("shifted caret line") + 5;

        view.update(cx, |view, cx| {
            view.selection = CodeSelection::at(caret);
            view.disk_changed(stamp, Some(incoming.clone()), cx);
            assert_eq!(view.cursor(), expected);
            assert_eq!(text_of(view), incoming);
        });

        view.update_in(cx, |view, window, cx| {
            view.undo(&CeUndo, window, cx);
            assert_eq!(text_of(view), original, "every hunk shares one transaction");
            assert_eq!(view.cursor(), caret);
        });
    }

    #[gpui::test]
    fn an_identical_external_reload_pushes_no_transaction(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        view.update(cx, |view, cx| {
            let before = view.history.mark();
            view.adopt_disk_text("one\r\ntwo\r\n", cx);
            assert_eq!(view.history.mark(), before);
            assert_eq!(text_of(view), "one\ntwo\n");
        });
    }

    #[gpui::test]
    fn a_crlf_reload_preserves_the_document_line_ending(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "one\r\ntwo\r\n", false);
        view.update(cx, |view, cx| {
            view.adopt_disk_text("one\r\nTWO\r\n", cx);
            let doc = view.document().expect("document");
            assert_eq!(doc.to_disk_string(), "one\r\nTWO\r\n");
        });
    }

    #[gpui::test]
    fn a_read_only_reload_temporarily_unlocks_and_restores_the_document(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "old\n", false);
        view.update(cx, |view, cx| {
            view.state
                .document_mut()
                .expect("document")
                .set_read_only(Some(ReadOnlyReason::Permissions));
            view.adopt_disk_text("new content\n", cx);
            assert_eq!(text_of(view), "new content\n");
            assert_eq!(
                view.document().and_then(CodeDocument::read_only_reason),
                Some(ReadOnlyReason::Permissions)
            );
        });
    }

    #[gpui::test]
    fn a_whole_document_reload_remeasures_the_longest_line(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "this line starts longest\nx\n", false);
        view.update(cx, |view, cx| {
            view.adopt_disk_text("a\na much longer replacement line\n", cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert_eq!(
                view.document().expect("document").longest_line_chars(),
                "a much longer replacement line".len()
            );
        });
    }

    #[gpui::test]
    fn an_external_write_on_a_dirty_document_raises_a_conflict(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, "theirs\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some("theirs\n".to_string()), cx);
            assert!(view.has_conflict());
            assert_eq!(text_of(view), "one\nmine\n", "the buffer was not touched");
        });

        view.update(cx, |view, cx| view.resolve_keep_mine(cx));
        cx.executor().allow_parking();
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            assert!(!view.has_conflict());
            view.save_action(&CeSave, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\nmine\n");
    }

    #[gpui::test]
    fn a_deleted_file_is_flagged_and_saving_recreates_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        std::fs::remove_file(&path).expect("delete");

        view.update(cx, |view, cx| {
            view.disk_changed(None, None, cx);
            view.stamp = None;
            assert_eq!(view.disk, DiskState::Deleted);
        });

        view.update_in(cx, |view, window, cx| {
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\n");
        view.update(cx, |view, _cx| assert_eq!(view.disk, DiskState::InSync));
    }

    #[gpui::test]
    fn opening_a_real_file_registers_the_conflict_watcher(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "one\n", true);
        cx.executor().allow_parking();
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert!(view._watcher.is_some(), "the parent directory is watched");
            let bridge = view
                ._watch_bridge
                .take()
                .expect("the reload task is bridged to the watcher");
            *bridge.lock().expect("bridge lock") = None;
            view._watcher = None;
        });
    }

    #[gpui::test]
    async fn only_the_latest_rapid_open_keeps_its_watcher(cx: &mut TestAppContext) {
        let first = tempfile::tempdir().expect("first tempdir");
        let second = tempfile::tempdir().expect("second tempdir");
        let first_path = first.path().join("first.rs");
        let second_path = second.path().join("second.rs");
        std::fs::write(&first_path, "first\n").expect("first fixture");
        std::fs::write(&second_path, "second\n").expect("second fixture");
        let (view, cx) = view(cx, "seed\n");
        cx.executor().allow_parking();

        view.update(cx, |view, cx| {
            view.open(first_path, cx);
            view.open(second_path.clone(), cx);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| {
                view.document()
                    .and_then(|doc| doc.line_string(0))
                    .as_deref()
                    == Some("second")
                    && view._watcher.is_some()
            }) {
                break;
            }
            smol::Timer::after(Duration::from_millis(1)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(view.path(), second_path);
            assert_eq!(
                view.document()
                    .and_then(|doc| doc.line_string(0))
                    .as_deref(),
                Some("second")
            );
            assert!(view._watcher.is_some());
            if let Some(bridge) = view._watch_bridge.take() {
                *bridge.lock().expect("bridge lock") = None;
            }
            view._watcher = None;
        });
    }

    #[test]
    fn a_removed_parent_refuses_watcher_creation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().to_path_buf();
        drop(dir);
        assert!(create_file_watcher(parent).is_err());
    }

    #[gpui::test]
    fn a_tracked_base_yields_blocks_after_the_debounce(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "a\nX\nc\nd\ne\n");
        view.update(cx, |view, cx| {
            assert!(view.marker_blocks().is_empty());
            view.install_base(
                Base::Text {
                    text: "a\nb\nc\ne\n".into(),
                    head_sha: "deadbeef".to_string(),
                },
                cx,
            );
            assert!(view.tracker.is_dirty(), "the base installs one dirty block");
            assert!(
                view.marker_blocks().len() == 1,
                "nothing is diffed on the render thread"
            );
        });
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert!(view.tracker.is_dirty(), "the debounce has not elapsed yet");
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| {
            assert_eq!(
                blocks_of(view),
                vec![
                    (BlockKind::Modified, 1..2, 1..2),
                    (BlockKind::Added, 3..4, 3..3),
                ]
            );
            assert!(!view.tracker.is_dirty());
        });
    }

    #[gpui::test]
    fn an_untracked_or_absent_base_keeps_the_tracker_inactive(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "a\nb\n");
        view.update_in(cx, |view, window, cx| {
            view.install_base(Base::Untracked, cx);
            view.replace_text_in_range(None, "x", window, cx);
            assert!(!view.tracker.is_active());
            assert!(view.marker_blocks().is_empty());
            view.install_base(Base::None, cx);
            assert!(!view.tracker.is_active());
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| assert!(view.marker_blocks().is_empty()));
    }

    #[gpui::test]
    fn keystrokes_move_the_blocks_without_a_full_rediff(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let base: String = (0..200).map(|row| format!("line {row}\n")).collect();
        let (view, cx) = tracked_view(cx, &base, &base);
        view.update(cx, |view, _cx| {
            assert!(view.marker_blocks().is_empty());
            assert_eq!(
                view.tracker.stats().rediffs,
                0,
                "an equal document short-circuits"
            );
        });

        let at = base.find("line 100").expect("line 100") + 8;
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(at);
            for letter in ["x", "y", "z"] {
                view.replace_text_in_range(None, letter, window, cx);
            }
            assert_eq!(
                blocks_of(view),
                vec![(BlockKind::Modified, 100..101, 100..101)],
                "range_changed marks the typed line synchronously"
            );
            assert!(view.tracker.is_dirty());
            assert_eq!(view.tracker.stats().rediffs, 0);
        });
        settle_tracker(cx);
        view.update_in(cx, |view, window, cx| {
            assert_eq!(
                blocks_of(view),
                vec![(BlockKind::Modified, 100..101, 100..101)]
            );
            assert_eq!(
                view.tracker.stats().rediffs,
                1,
                "three grouped keystrokes, one diff"
            );
            assert_eq!(view.tracker.stats().full_rediffs, 0);

            let top = view.document().expect("document").line_to_byte(10);
            view.selection = CodeSelection::at(top);
            view.replace_text_in_range(None, "inserted\n", window, cx);
            assert_eq!(
                blocks_of(view),
                vec![
                    (BlockKind::Added, 10..11, 10..10),
                    (BlockKind::Modified, 101..102, 100..101),
                ],
                "blocks after the edit shift without a diff"
            );
            assert!(view.marker_blocks()[0].dirty);
            assert!(!view.marker_blocks()[1].dirty);
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| {
            assert_eq!(
                blocks_of(view),
                vec![
                    (BlockKind::Added, 10..11, 10..10),
                    (BlockKind::Modified, 101..102, 100..101),
                ]
            );
            assert_eq!(view.tracker.stats().full_rediffs, 0);
        });
    }

    #[gpui::test]
    fn a_stale_refresh_result_is_discarded_and_rescheduled(cx: &mut TestAppContext) {
        let (view, cx) = tracked_view(cx, "a\nb\nc\n", "a\nB\nc\n");
        view.update_in(cx, |view, window, cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Modified, 1..2, 1..2)]);
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "C", window, cx);
            view.refresh_tracker_now(cx);
            view.replace_text_in_range(None, "C", window, cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert!(
                view.tracker.is_dirty(),
                "the result computed before the second keystroke was discarded"
            );
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Modified, 1..3, 1..3)]);
            assert_eq!(text_of(view), "a\nB\nCCc\n");
        });
    }

    #[gpui::test]
    fn a_refresh_in_flight_when_the_base_changes_is_discarded(cx: &mut TestAppContext) {
        let (view, cx) = tracked_view(cx, "a\nb\nc\n", "a\nB\nc\n");
        view.update(cx, |view, cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Modified, 1..2, 1..2)]);
            let doc_lines = view.document().expect("document").line_count() as u32;
            view.tracker.reset(doc_lines, count_lines("a\nb\nc\n"));
            view.refresh_tracker_now(cx);
            view.install_base(
                Base::Text {
                    text: "a\nB\nc\n".into(),
                    head_sha: "cafebabe".to_string(),
                },
                cx,
            );
            assert!(
                view.tracker.is_dirty(),
                "the new base installs one dirty block"
            );
        });
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert!(
                view.tracker.is_dirty(),
                "the result computed against the previous base is discarded"
            );
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| {
            assert!(
                view.marker_blocks().is_empty(),
                "the document equals the new base, got {:?}",
                blocks_of(view)
            );
        });
    }

    #[gpui::test]
    fn an_external_reload_resets_the_tracker_and_closes_the_popup(cx: &mut TestAppContext) {
        let (view, cx) = tracked_view(cx, "a\nb\nc\n", "a\nB\nc\n");
        view.update(cx, |view, cx| {
            view.open_marker_popup(0, cx);
            assert!(view.popup.is_some());
            view.adopt_disk_text("a\nb\nc\nd\n", cx);
            assert!(view.popup.is_none(), "an agent write closes the popup");
            assert_eq!(view.marker_blocks().len(), 1);
            assert!(
                view.marker_blocks()[0].dirty,
                "one dirty block covers everything"
            );
            assert_eq!(view.marker_blocks()[0].lines, 0..5);
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Added, 3..4, 3..3)]);
        });
    }

    #[gpui::test]
    fn reverting_added_deleted_and_modified_blocks_restores_the_base(cx: &mut TestAppContext) {
        let base = "a\nb\nc\nd\n";
        let (view, cx) = tracked_view(cx, base, "a\nB\nc\nd\n");
        view.update(cx, |view, cx| {
            assert!(view.revert_block(1, cx), "a modified block is replaced");
            assert_eq!(text_of(view), base);
            assert!(view.is_dirty(), "the document is dirty after a revert");
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| assert!(view.marker_blocks().is_empty()));

        let (view, cx) = tracked_view(cx, base, "a\nb\nnew\nc\nd\n");
        view.update(cx, |view, cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Added, 2..3, 2..2)]);
            assert!(view.revert_block(2, cx), "an added block loses its lines");
            assert_eq!(text_of(view), base);
        });

        let (view, cx) = tracked_view(cx, base, "a\nd\n");
        view.update(cx, |view, cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Deleted, 1..1, 1..3)]);
            assert!(
                view.revert_block(1, cx),
                "a deleted block is reinserted at its boundary"
            );
            assert_eq!(text_of(view), base);
        });

        let (view, cx) = tracked_view(cx, base, "a\nB\nC\nd\n");
        view.update(cx, |view, cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Modified, 1..3, 1..3)]);
            assert!(view.revert_block(2, cx));
            assert_eq!(text_of(view), base);
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| assert!(view.marker_blocks().is_empty()));
    }

    #[gpui::test]
    fn two_reverts_on_adjacent_blocks_give_the_base_and_undo_brings_each_back(
        cx: &mut TestAppContext,
    ) {
        let base = "a\nb\nc\nd\ne\n";
        let edited = "a\nB\nc\nD\ne\n";
        let (view, cx) = tracked_view(cx, base, edited);
        let before = view.update(cx, |view, cx| {
            assert_eq!(
                blocks_of(view),
                vec![
                    (BlockKind::Modified, 1..2, 1..2),
                    (BlockKind::Modified, 3..4, 3..4),
                ]
            );
            let before = view.tracker.stats();
            assert!(view.revert_block(1, cx));
            assert_eq!(
                blocks_of(view),
                vec![
                    (BlockKind::Modified, 1..2, 1..2),
                    (BlockKind::Modified, 3..4, 3..4),
                ],
                "the next block is untouched by the revert"
            );
            assert!(
                view.marker_blocks()[0].dirty,
                "the reverted block awaits its diff"
            );
            assert!(!view.marker_blocks()[1].dirty);
            assert_eq!(view.tracker.stats().rediffs, before.rediffs);
            before
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert_eq!(
                blocks_of(view),
                vec![(BlockKind::Modified, 3..4, 3..4)],
                "the revert is diffed at once, without the keystroke debounce"
            );
            assert_eq!(view.tracker.stats().rediffs, before.rediffs + 1);
            assert_eq!(
                view.tracker.stats().full_rediffs,
                before.full_rediffs,
                "only the reverted window is diffed"
            );
            assert!(view.revert_block(3, cx));
            assert_eq!(text_of(view), base);
        });
        settle_tracker(cx);
        view.update_in(cx, |view, window, cx| {
            assert!(view.marker_blocks().is_empty());
            view.undo(&CeUndo, window, cx);
            assert_eq!(text_of(view), "a\nb\nc\nD\ne\n", "one Ctrl+Z, one revert");
            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                edited,
                "the second Ctrl+Z restores the rest exactly"
            );
        });
        settle_tracker(cx);
        view.update(cx, |view, _cx| {
            assert_eq!(
                blocks_of(view),
                vec![
                    (BlockKind::Modified, 1..2, 1..2),
                    (BlockKind::Modified, 3..4, 3..4),
                ],
                "the tracker finds both blocks again"
            );
        });
    }

    #[gpui::test]
    fn a_revert_on_a_read_only_document_or_a_plain_line_does_nothing(cx: &mut TestAppContext) {
        let (view, cx) = tracked_view(cx, "a\nb\nc\n", "a\nB\nc\n");
        view.update(cx, |view, cx| {
            assert!(!view.revert_block(0, cx), "line 0 has no block");
            assert_eq!(text_of(view), "a\nB\nc\n");
            assert!(view.read_only_flash.is_none());

            view.state
                .document_mut()
                .expect("document")
                .set_read_only(Some(ReadOnlyReason::Permissions));
            assert!(!view.revert_block(1, cx));
            assert_eq!(text_of(view), "a\nB\nc\n");
            assert!(view.read_only_flash.is_some(), "refused like a keystroke");
        });
    }

    #[gpui::test]
    fn the_popup_shows_the_base_text_copies_all_of_it_and_reverts(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let base: String = (0..250).map(|row| format!("base {row}\n")).collect();
        let doc = "top\n".to_string() + &base[base.find("base 245").expect("tail")..];
        let (view, cx) = tracked_view(cx, &base, &doc);
        view.update(cx, |view, cx| {
            assert_eq!(blocks_of(view), vec![(BlockKind::Modified, 0..1, 0..245)]);
            view.open_marker_popup(0, cx);
            let popup = view.popup.as_ref().expect("popup");
            assert_eq!(popup.title, "Modified line 1");
            assert_eq!(popup.shown.len(), POPUP_SHOWN_LINES);
            assert_eq!(popup.hidden, 45);
            assert_eq!(popup.shown[0].0.as_ref(), "base 0");
            assert!(popup.base_text.ends_with("base 244\n"));
            assert_eq!(popup.base_text.lines().count(), 245);

            view.copy_popup_base(cx);
            let clipped = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            assert_eq!(clipped.lines().count(), 245, "Copy takes the whole block");

            view.revert_from_popup(cx);
            assert!(view.popup.is_none(), "Revert closes the popup");
            assert_eq!(text_of(view), base);
        });
    }

    #[gpui::test]
    fn escape_closes_the_popup_and_hover_alone_never_opens_it(cx: &mut TestAppContext) {
        let (view, cx) = tracked_view(cx, "a\nb\nc\n", "a\nB\nc\n");
        view.update_in(cx, |view, window, cx| {
            *view.hits.borrow_mut() = CodeHitMap {
                marker_x: 10.0,
                markers: vec![super::super::element::MarkerHit {
                    index: 0,
                    y0: 100.0,
                    y1: 118.0,
                }],
                ..CodeHitMap::default()
            };
            let over = MouseMoveEvent {
                position: point(px(12.), px(105.)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            };
            view.on_marker_move(&over, cx);
            assert_eq!(view.hovered_marker(), Some(0), "hover widens the bar");
            assert!(view.popup.is_none(), "hover never opens the popup");

            let away = MouseMoveEvent {
                position: point(px(200.), px(105.)),
                ..over
            };
            view.on_marker_move(&away, cx);
            assert_eq!(view.hovered_marker(), None);

            view.open_marker_popup(0, cx);
            assert!(view.popup.is_some());
            view.escape(&CeEscape, window, cx);
            assert!(view.popup.is_none(), "Escape closes it without acting");
            assert_eq!(text_of(view), "a\nB\nc\n");
        });
    }

    #[test]
    fn popup_titles_name_the_block() {
        let block = |lines: Range<u32>, base_lines: Range<u32>| Block {
            lines,
            base_lines,
            dirty: false,
            too_big: false,
        };
        assert_eq!(popup_title(&block(11..15, 11..13)), "Modified lines 12-15");
        assert_eq!(popup_title(&block(4..5, 4..5)), "Modified line 5");
        assert_eq!(
            popup_title(&block(20..20, 19..22)),
            "Deleted 3 lines after 20"
        );
        assert_eq!(popup_title(&block(0..0, 0..1)), "Deleted 1 line at the top");
        assert_eq!(popup_title(&block(3..7, 3..3)), "Added 4 lines");
    }

    #[test]
    fn the_popup_width_is_bounded_by_the_editor_and_flips_when_short_of_room() {
        assert_eq!(popup_width(880.0), POPUP_MAX_W);
        assert_eq!(popup_width(400.0), 400.0 - 2.0 * POPUP_MARGIN);
        assert_eq!(popup_width(360.0), 336.0);
        assert_eq!(popup_width(200.0), 200.0 - 2.0 * POPUP_MARGIN);
        assert_eq!(
            popup_anchor(100.0, 118.0, 200.0, 600.0),
            (Anchor::TopLeft, 118.0)
        );
        assert_eq!(
            popup_anchor(500.0, 518.0, 200.0, 600.0),
            (Anchor::BottomLeft, 500.0)
        );
        assert_eq!(
            popup_anchor(50.0, 68.0, 200.0, 100.0),
            (Anchor::TopLeft, 68.0),
            "no room either side keeps the default below"
        );
    }

    #[test]
    fn base_block_text_and_doc_line_range_agree_on_terminators() {
        let base = ["a", "b", "c", ""];
        assert_eq!(base_block_text(&base, &(1..3)), "b\nc\n");
        assert_eq!(base_block_text(&base, &(2..4)), "c\n");
        assert_eq!(base_block_text(&base, &(1..1)), "");
        let unterminated = ["a", "b"];
        assert_eq!(base_block_text(&unterminated, &(1..2)), "b");

        let doc = build_document(PathBuf::from("/nonexistent/x.txt"), "a\nb\nc\n", false);
        assert_eq!(doc_line_range(&doc, &(1..3)), 2..6);
        assert_eq!(doc_line_range(&doc, &(3..4)), 6..6);
        assert_eq!(doc_line_range(&doc, &(1..1)), 2..2);
        let short = build_document(PathBuf::from("/nonexistent/y.txt"), "a\nb", false);
        assert_eq!(doc_line_range(&short, &(1..2)), 2..3);
    }
}

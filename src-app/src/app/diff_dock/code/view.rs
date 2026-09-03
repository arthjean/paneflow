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
    AnyElement, App, AppContext, AsyncApp, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle,
    EntityInputHandler, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, ScrollHandle, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
    UTF16Selection, WeakEntity, Window, actions, div, px, size,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

type WatchBridge = Arc<Mutex<Option<mpsc::UnboundedSender<notify::Result<notify::Event>>>>>;
type WatchEvents = mpsc::UnboundedReceiver<notify::Result<notify::Event>>;

use super::cursor::{self, CodeSelection};
use super::document::{CodeDocument, ReadOnlyReason, normalize_newlines};
use super::edit::{self, EditGroup, IndentUnit};
use super::element::{
    CODE_ROW_HEIGHT, CodeCaret, CodeColors, CodeElement, CodeGeometry, CodeHitMap, GutterMemo,
    autoscroll_step, reveal_h_offset, reveal_offset, visible_rows,
};
use super::highlight::{
    CodeHighlighter, DeferredParse, HIGHLIGHT_FRAME_BUDGET, HighlightOutcome, spawn_deferred_parse,
};
use super::load::{CodeLoadSlot, CodeLoadState, CodeOpen, spawn_code_load};
use super::save::{self, FileStamp};
use crate::diff::{DiffSyntax, palette};
use crate::terminal::blink::{BlinkPhaseGlobal, CURSOR_BLINK_INTERVAL};
use crate::widgets::scrollbar::{self, SCROLLBAR_GUTTER, ScrollDragState};

pub(crate) const CODE_KEY_CONTEXT: &str = "CodeEditor";

const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
const MULTI_CLICK_RADIUS: f32 = 2.0;

const DRAG_SCROLL_ROWS: f32 = 1.0;
const DRAG_SCROLL_COLUMNS: f32 = 3.0;

const READ_ONLY_FLASH: Duration = Duration::from_millis(600);

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const INITIAL_HIGHLIGHT_ROWS: usize = 60;

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

pub(crate) struct CodeView {
    path: PathBuf,
    state: CodeLoadState,
    slot: CodeLoadSlot,
    focus: FocusHandle,
    scroll: ScrollHandle,
    v_drag: Option<ScrollDragState>,
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
    _watcher: Option<RecommendedWatcher>,
    _watch_bridge: Option<WatchBridge>,
}

impl CodeView {
    pub(crate) fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            element_id: format!("code-view:{}", path.display()).into(),
            path,
            state: CodeLoadState::Loading,
            slot: CodeLoadSlot::new(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            v_drag: None,
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
            _watcher: None,
            _watch_bridge: None,
        };
        view.observe_blink(cx);
        view.start_load(cx);
        view
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
        let viewport_h = f32::from(self.scroll.bounds().size.height);
        if viewport_h <= 0.0 {
            return false;
        }
        let content_top = f32::from(-self.scroll.offset().y).max(0.0);
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
        self.scroll.set_offset(Point::new(px(0.), px(0.)));
        self.history.clear();
        self.saved_mark = edit::HistoryMark::default();
        self.marked = None;
        self.read_only_flash = None;
        self.stamp = None;
        self.disk = DiskState::default();
        self.save_error = None;
        self.saving = false;
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
        cursor::page_rows(f32::from(self.scroll.bounds().size.height), CODE_ROW_HEIGHT)
    }

    pub(crate) fn reveal_cursor(&mut self) {
        let viewport_h = f32::from(self.scroll.bounds().size.height);
        let geometry = self.geometry.get();
        let current = f32::from(self.scroll.offset().y);
        let h_offset = self.h_offset;
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = self.selection.cursor();
        let row = doc.byte_to_line(offset);
        let column = cursor::goal_column(doc, offset);
        let content_h = doc.line_count() as f32 * CODE_ROW_HEIGHT;

        let target = reveal_offset(row, viewport_h, content_h, current);
        if (target - current).abs() > f32::EPSILON {
            self.scroll.set_offset(Point::new(px(0.), px(target)));
        }
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

    fn fill_visible_highlights(&mut self, cx: &mut Context<Self>) {
        let Some(line_count) = self.state.document().map(CodeDocument::line_count) else {
            return;
        };
        let viewport_h = f32::from(self.scroll.bounds().size.height);
        let content_top = f32::from(-self.scroll.offset().y).max(0.0);
        let rows = if viewport_h > 0.0 {
            visible_rows(content_top, viewport_h, line_count)
        } else {
            0..INITIAL_HIGHLIGHT_ROWS.min(line_count)
        };
        if let Some((doc, highlighter)) = self.state.editable()
            && highlighter.fill_stale_rows(doc, rows, HIGHLIGHT_FRAME_BUDGET)
        {
            cx.notify();
        }
    }

    fn apply_wheel(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let dx = f32::from(ev.delta.pixel_delta(window.line_height()).x);
        if dx == 0.0 {
            return;
        }
        let bounds = self.scroll.bounds();
        if !bounds.contains(&ev.position) {
            return;
        }
        let max = self.geometry.get().max_h_offset;
        let next = (self.h_offset - dx).clamp(0.0, max);
        if next != self.h_offset {
            self.h_offset = next;
            cx.notify();
        }
    }

    fn over_v_scrollbar(&self, position: Point<gpui::Pixels>) -> bool {
        let bounds = self.scroll.bounds();
        position.x >= bounds.right() - SCROLLBAR_GUTTER && position.x <= bounds.right()
    }

    fn on_scrollbar_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) -> bool {
        if !self.over_v_scrollbar(ev.position) {
            return false;
        }
        let Some(m) = scrollbar::metrics(&self.scroll) else {
            return false;
        };
        let local_y = f32::from(ev.position.y - self.scroll.bounds().origin.y);
        if local_y >= m.thumb_top && local_y <= m.thumb_top + m.thumb_h {
            self.v_drag = Some(scrollbar::begin_drag(&self.scroll, ev.position.y));
        } else if let Some(offset) = scrollbar::track_click_offset(&self.scroll, ev.position.y) {
            self.scroll.set_offset(Point::new(px(0.), px(offset)));
            cx.notify();
        }
        true
    }

    fn on_scrollbar_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.v_drag else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.v_drag = None;
            cx.notify();
            return;
        }
        if let Some(offset) = scrollbar::drag_offset(&self.scroll, &drag, ev.position.y) {
            self.scroll.set_offset(Point::new(px(0.), px(offset)));
            cx.notify();
        }
    }

    fn on_scrollbar_up(&mut self, _ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.v_drag.take().is_some() {
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
            let max = f32::from(self.scroll.max_offset().y);
            let current = -f32::from(self.scroll.offset().y);
            let next = (current + dy).clamp(0.0, max);
            if next != current {
                self.scroll.set_offset(Point::new(px(0.), px(-next)));
                moved = true;
            }
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
        let mut records = Vec::with_capacity(ops.len());
        let mut deferred: Option<DeferredParse> = None;
        if let Some((doc, hl)) = self.state.editable() {
            for (range, text) in ops {
                let Some(applied) = edit::splice(doc, range.clone(), text) else {
                    continue;
                };
                for change in &applied.edits {
                    if let HighlightOutcome::Deferred(parse) = hl.edit(doc, change) {
                        deferred = Some(parse);
                    }
                }
                records.push(applied.record);
            }
        }
        if records.is_empty() {
            return false;
        }
        self.history.push(records, before, after, group, now);
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
        if let Some((doc, hl)) = self.state.editable() {
            let step = if redo {
                self.history.redo(doc)
            } else {
                self.history.undo(doc)
            };
            if let Some(step) = step {
                for change in &step.edits {
                    if let HighlightOutcome::Deferred(parse) = hl.edit(doc, change) {
                        deferred = Some(parse);
                    }
                }
                restored = Some(step.selection);
            }
        }
        let Some(selection) = restored else {
            return;
        };
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
                let updated = cx.update(|cx| {
                    this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                        view.disk_changed(stamp, text, cx);
                    })
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn disk_changed(
        &mut self,
        stamp: Option<FileStamp>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match (stamp, text) {
            (None, _) | (_, None) => {
                if self.disk != DiskState::Deleted {
                    self.disk = DiskState::Deleted;
                    cx.notify();
                }
            }
            (Some(stamp), Some(text)) => {
                if self.stamp == Some(stamp) && self.disk == DiskState::InSync {
                    return;
                }
                self.stamp = Some(stamp);
                if self.is_dirty() {
                    self.disk = DiskState::Conflict;
                    cx.notify();
                    return;
                }
                self.disk = DiskState::InSync;
                self.adopt_disk_text(&text, cx);
                self.saved_mark = self.history.mark();
            }
        }
    }

    fn adopt_disk_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let current = doc.text().to_string();
        let ops = edit::disk_splices(&current, text);
        if ops.is_empty() {
            return;
        }
        let scroll = self.scroll.offset();
        let after = edit::shift_selection_for_splices(self.selection, &ops);
        let reason = doc.read_only_reason();
        if reason.is_some()
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_read_only(None);
        }
        let replaced = self.splice_all(&ops, after, EditGroup::Atomic, cx);
        if let Some(reason) = reason
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_read_only(Some(reason));
        }
        if !replaced {
            return;
        }
        self.scroll.set_offset(scroll);
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
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    let Some(text) = text else {
                        view.disk = DiskState::Deleted;
                        cx.notify();
                        return;
                    };
                    view.stamp = stamp;
                    view.disk = DiskState::InSync;
                    view.adopt_disk_text(&text, cx);
                    view.saved_mark = view.history.mark();
                });
            });
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
        self.fill_visible_highlights(cx);
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

        let line_count = doc.line_count();
        let banners = self.banners(ui, cx);
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
            line_count,
            self.geometry.clone(),
            self.gutter_memo.clone(),
            self.hits.clone(),
        );

        let mut host = div()
            .id(self.element_id.clone())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if this.on_scrollbar_down(ev, cx) || this.on_text_down(ev, window, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| {
                this.apply_wheel(ev, window, cx);
            }))
            .child(element);
        host.style().restrict_scroll_to_axis = Some(true);

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
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                this.on_scrollbar_move(ev, cx);
                this.on_text_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _w, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                }),
            )
            .children(banners)
            .child(host)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};

    use super::super::highlight::CodeHighlighter;
    use super::super::load::{LoadedCode, build_document};
    use super::*;

    fn view<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        let path = PathBuf::from("/nonexistent/paneflow-code.rs");
        let state = if text.is_empty() {
            CodeLoadState::Loading
        } else {
            let document = build_document(path.clone(), text, false);
            let highlighter = CodeHighlighter::new(
                &document,
                DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
            );
            CodeLoadState::Ready(Box::new(LoadedCode {
                document,
                highlighter,
                indent: IndentUnit::Spaces(4),
                stamp: None,
            }))
        };
        cx.add_window_view(move |_window, cx| CodeView {
            element_id: "code-view:test".into(),
            path,
            state,
            slot: CodeLoadSlot::new(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            v_drag: None,
            h_offset: 0.0,
            selection: CodeSelection::default(),
            goal_column: 0,
            text_drag: None,
            click_chain: None,
            last_motion: Instant::now(),
            blink_visible: true,
            focused: false,
            focus_observers_installed: false,
            theme_generation: 0,
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
            _watcher: None,
            _watch_bridge: None,
        })
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
        let (view, cx) = view(cx, "");

        view.update(cx, |view, cx| {
            view.v_drag = Some(scrollbar::begin_drag(&view.scroll, px(40.)));

            view.on_scrollbar_move(
                &MouseMoveEvent {
                    position: point(px(10.), px(90.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                },
                cx,
            );
            assert!(view.v_drag.is_none());

            view.v_drag = Some(scrollbar::begin_drag(&view.scroll, px(40.)));
            view.on_scrollbar_move(
                &MouseMoveEvent {
                    position: point(px(10.), px(90.)),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                },
                cx,
            );
            assert!(view.v_drag.is_some());
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
        let document = build_document(path.clone(), text, false);
        let highlighter = CodeHighlighter::new(
            &document,
            DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
        );
        let stamp = FileStamp::read(&path);
        let state = CodeLoadState::Ready(Box::new(LoadedCode {
            document,
            highlighter,
            indent: IndentUnit::Spaces(4),
            stamp,
        }));
        let (view, cx) = {
            let path = path.clone();
            cx.add_window_view(move |_window, cx| {
                let mut view = CodeView {
                    element_id: "code-view:test".into(),
                    path,
                    state,
                    slot: CodeLoadSlot::new(),
                    focus: cx.focus_handle(),
                    scroll: ScrollHandle::new(),
                    v_drag: None,
                    h_offset: 0.0,
                    selection: CodeSelection::default(),
                    goal_column: 0,
                    text_drag: None,
                    click_chain: None,
                    last_motion: Instant::now(),
                    blink_visible: true,
                    focused: false,
                    focus_observers_installed: false,
                    theme_generation: 0,
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
                    _watcher: None,
                    _watch_bridge: None,
                };
                if watch {
                    view.start_watcher(cx);
                }
                view
            })
        };
        (dir, view, cx)
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
    fn a_keystroke_on_a_read_only_document_is_refused_visibly(cx: &mut TestAppContext) {
        let path = PathBuf::from("/nonexistent/paneflow-code.rs");
        let document = build_document(path.clone(), "locked\n", true);
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
        let (view, cx) = cx.add_window_view(move |_window, cx| CodeView {
            element_id: "code-view:test".into(),
            path,
            state,
            slot: CodeLoadSlot::new(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            v_drag: None,
            h_offset: 0.0,
            selection: CodeSelection::default(),
            goal_column: 0,
            text_drag: None,
            click_chain: None,
            last_motion: Instant::now(),
            blink_visible: true,
            focused: false,
            focus_observers_installed: false,
            theme_generation: 0,
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
            _watcher: None,
            _watch_bridge: None,
        });

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
}

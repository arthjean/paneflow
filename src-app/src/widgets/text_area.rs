use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ShapedLine, SharedString,
    Style, Styled, Task, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, hsla,
    point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::ime_caret::{ImeCaretReport, follow_ime_caret};
use super::text_input::{
    Backspace, Delete, DeleteToStart, DeleteWordLeft, DeleteWordRight, End, Home, Left, Redo,
    Right, SelectAll, SelectEnd, SelectHome, SelectLeft, SelectRight, SelectWordLeft,
    SelectWordRight, TextInputCopy, TextInputCut, TextInputPaste, Undo, WordLeft, WordRight,
};
use super::utf16::{byte_range_from_utf16, utf16_offset_from_byte};

actions!(
    text_area,
    [
        AreaUp,
        AreaDown,
        AreaSelectUp,
        AreaSelectDown,
        AreaNewline,
        AreaSubmit,
        AreaCancel,
    ]
);

pub(crate) const TEXT_AREA_CONTEXT: &str = "TextArea";
const KEY_CONTEXT: &str = "TextInput TextArea";
#[cfg(target_os = "macos")]
pub(crate) const SUBMIT_KEYSTROKE: &str = "cmd-enter";
#[cfg(not(target_os = "macos"))]
pub(crate) const SUBMIT_KEYSTROKE: &str = "ctrl-enter";
const CARET_BLINK_MS: u64 = 500;
const CARET_WIDTH: f32 = 1.;
const UNDO_LIMIT: usize = 200;

pub fn register_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", AreaUp, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new("down", AreaDown, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new("shift-up", AreaSelectUp, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new("shift-down", AreaSelectDown, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new("enter", AreaNewline, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new("shift-enter", AreaNewline, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new(SUBMIT_KEYSTROKE, AreaSubmit, Some(TEXT_AREA_CONTEXT)),
        KeyBinding::new("escape", AreaCancel, Some(TEXT_AREA_CONTEXT)),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextAreaEvent {
    Changed,
    Submit,
    Cancel,
}

struct Snapshot {
    content: String,
    cursor: usize,
    anchor: usize,
}

struct ShapedRow {
    line: usize,
    start: usize,
    shaped: ShapedLine,
}

pub(crate) struct TextArea {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    accessible_name: SharedString,
    rows: usize,
    cursor: usize,
    anchor: usize,
    marked: Option<Range<usize>>,
    goal_x: Option<Pixels>,
    scroll_top: usize,
    scroll_left: Pixels,
    shaped: Vec<ShapedRow>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    selecting: bool,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    blink_anchor: Instant,
    blink_task: Option<Task<()>>,
    ime_caret: ImeCaretReport,
}

impl EventEmitter<TextAreaEvent> for TextArea {}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    starts
}

fn line_range(text: &str, starts: &[usize], line: usize) -> Range<usize> {
    let start = starts[line.min(starts.len() - 1)];
    let end = starts
        .get(line + 1)
        .map_or(text.len(), |next| next.saturating_sub(1));
    start..end.max(start)
}

fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|start| *start <= offset).max(1) - 1
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(at, _)| (at < offset).then_some(at))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(at, _)| (at > offset).then_some(at))
        .unwrap_or(text.len())
}

fn previous_word(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .rev()
        .find(|(at, word)| *at < offset && !word.trim().is_empty())
        .map_or(0, |(at, _)| at)
}

fn next_word(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .find(|(at, word)| at + word.len() > offset && !word.trim().is_empty())
        .map_or(text.len(), |(at, word)| at + word.len())
}

fn visible_from(scroll_top: usize, caret_line: usize, rows: usize, line_count: usize) -> usize {
    let rows = rows.max(1);
    let last_top = line_count.saturating_sub(rows);
    let top = if caret_line < scroll_top {
        caret_line
    } else if caret_line >= scroll_top + rows {
        caret_line + 1 - rows
    } else {
        scroll_top
    };
    top.min(last_top)
}

fn caret_visible(since_activity: Duration) -> bool {
    (since_activity.as_millis() / u128::from(CARET_BLINK_MS)).is_multiple_of(2)
}

impl TextArea {
    pub(crate) fn new(
        placeholder: impl Into<SharedString>,
        accessible_name: impl Into<SharedString>,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            accessible_name: accessible_name.into(),
            rows: rows.max(1),
            cursor: 0,
            anchor: 0,
            marked: None,
            goal_x: None,
            scroll_top: 0,
            scroll_left: px(0.),
            shaped: Vec::new(),
            last_bounds: None,
            line_height: px(18.),
            selecting: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            blink_anchor: Instant::now(),
            blink_task: None,
            ime_caret: ImeCaretReport::default(),
        }
    }

    pub(crate) fn value(&self) -> &str {
        &self.content
    }

    pub(crate) fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }

    #[cfg(test)]
    pub(crate) fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.content = value.to_owned();
        self.cursor = self.content.len();
        self.anchor = self.cursor;
        self.marked = None;
        cx.emit(TextAreaEvent::Changed);
        cx.notify();
    }

    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.cursor = snapshot.cursor.min(self.content.len());
        self.anchor = snapshot.anchor.min(self.content.len());
        self.marked = None;
        self.touched(cx);
        cx.emit(TextAreaEvent::Changed);
    }

    fn touched(&mut self, cx: &mut Context<Self>) {
        self.blink_anchor = Instant::now();
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        self.cursor = offset.min(self.content.len());
        if !extend {
            self.anchor = self.cursor;
        }
        self.goal_x = None;
        self.touched(cx);
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.push_undo();
        self.content.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
        self.marked = None;
        self.goal_x = None;
        self.touched(cx);
        cx.emit(TextAreaEvent::Changed);
    }

    fn delete_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let range = if self.selection().is_empty() {
            offset.min(self.cursor)..offset.max(self.cursor)
        } else {
            self.selection()
        };
        if !range.is_empty() {
            self.replace(range, "", cx);
        }
    }

    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let starts = line_starts(&self.content);
        let line = line_of(&starts, self.cursor);
        let target = if down {
            if line + 1 >= starts.len() {
                self.move_to(self.content.len(), extend, cx);
                return;
            }
            line + 1
        } else {
            if line == 0 {
                self.move_to(0, extend, cx);
                return;
            }
            line - 1
        };
        let goal = self.goal_x.unwrap_or_else(|| self.caret_x(self.cursor));
        let range = line_range(&self.content, &starts, target);
        let local = self
            .shaped
            .iter()
            .find(|row| row.line == target)
            .map(|row| row.shaped.closest_index_for_x(goal))
            .unwrap_or_else(|| {
                let column = self.cursor - starts[line];
                let text = &self.content[range.clone()];
                let mut at = column.min(text.len());
                while !text.is_char_boundary(at) {
                    at -= 1;
                }
                at
            });
        self.cursor = range.start + local.min(range.len());
        if !extend {
            self.anchor = self.cursor;
        }
        self.goal_x = Some(goal);
        self.touched(cx);
    }

    fn caret_x(&self, offset: usize) -> Pixels {
        let starts = line_starts(&self.content);
        let line = line_of(&starts, offset);
        self.shaped
            .iter()
            .find(|row| row.line == line)
            .map_or(px(0.), |row| {
                row.shaped.x_for_index(offset.saturating_sub(row.start))
            })
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return self.content.len();
        };
        if self.shaped.is_empty() {
            return 0;
        }
        let row = ((position.y - bounds.top()) / self.line_height).floor();
        let row = (row.max(0.) as usize).min(self.shaped.len() - 1);
        let shaped = &self.shaped[row];
        shaped.start
            + shaped
                .shaped
                .closest_index_for_x(position.x - bounds.left() + self.scroll_left)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.selecting = true;
        let index = self.index_for_position(event.position);
        if event.click_count >= 2 {
            let starts = line_starts(&self.content);
            let range = line_range(&self.content, &starts, line_of(&starts, index));
            self.anchor = range.start;
            self.cursor = range.end;
            self.touched(cx);
        } else {
            self.move_to(index, event.modifiers.shift, cx);
        }
        cx.stop_propagation();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            let index = self.index_for_position(event.position);
            self.move_to(index, true, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn caret_shown(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.focus_handle.is_focused(window) || !window.is_window_active() {
            self.blink_task = None;
            return false;
        }
        if self.blink_task.is_none() {
            self.blink_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(CARET_BLINK_MS))
                        .await;
                    let focused = this.update_in(cx, |this, window, cx| {
                        let focused =
                            this.focus_handle.is_focused(window) && window.is_window_active();
                        if focused {
                            cx.notify();
                        } else {
                            this.blink_task = None;
                        }
                        focused
                    });
                    if !matches!(focused, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        caret_visible(self.blink_anchor.elapsed())
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        utf16_offset_from_byte(&self.content, range.start)
            ..utf16_offset_from_byte(&self.content, range.end)
    }

    fn replacement_range(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| byte_range_from_utf16(&self.content, range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection())
    }
}

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = byte_range_from_utf16(&self.content, &range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selection()),
            reversed: self.cursor < self.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16.as_ref());
        let text = new_text.replace("\r\n", "\n").replace('\r', "\n");
        self.replace(range, &text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16.as_ref());
        if self.marked.is_none() {
            self.push_undo();
        }
        self.content.replace_range(range.clone(), new_text);
        self.marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let selected = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| byte_range_from_utf16(new_text, range_utf16))
            .map(|inner| range.start + inner.start..range.start + inner.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.anchor = selected.start;
        self.cursor = selected.end;
        self.goal_x = None;
        self.touched(cx);
        cx.emit(TextAreaEvent::Changed);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = byte_range_from_utf16(&self.content, &range_utf16);
        let starts = line_starts(&self.content);
        let line = line_of(&starts, range.start);
        let row = self.shaped.iter().position(|row| row.line == line)?;
        let shaped = &self.shaped[row];
        let top = bounds.top() + self.line_height * row as f32;
        let left = bounds.left() - self.scroll_left;
        let end = range.end.min(shaped.start + shaped.shaped.len());
        Some(Bounds::from_corners(
            point(
                left + shaped.shaped.x_for_index(range.start - shaped.start),
                top,
            ),
            point(
                left + shaped.shaped.x_for_index(end.saturating_sub(shaped.start)),
                top + self.line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.last_bounds?
            .contains(&point)
            .then(|| utf16_offset_from_byte(&self.content, self.index_for_position(point)))
    }
}

struct AreaElement {
    area: Entity<TextArea>,
    caret_color: Hsla,
    selection_color: Hsla,
    placeholder_color: Hsla,
}

struct AreaPrepaint {
    rows: Vec<ShapedRow>,
    placeholder: Option<ShapedLine>,
    scroll_left: Pixels,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl Element for AreaElement {
    type RequestLayoutState = ();
    type PrepaintState = AreaPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let rows = self.area.read(cx).rows;
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let caret_shown = self
            .area
            .update(cx, |area, cx| area.caret_shown(window, cx));
        let line_height = window.line_height();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let area = self.area.read(cx);
        let starts = line_starts(&area.content);
        let caret_line = line_of(&starts, area.cursor);
        let scroll_top = visible_from(area.scroll_top, caret_line, area.rows, starts.len());
        let selection = area.selection();
        let marked = area.marked.clone();
        let run = |len: usize, color: Hsla, underline: bool| TextRun {
            len,
            font: style.font(),
            color,
            background_color: None,
            underline: underline.then_some(UnderlineStyle {
                color: Some(color),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        };

        let mut rows = Vec::new();
        for line in scroll_top..(scroll_top + area.rows).min(starts.len()) {
            let range = line_range(&area.content, &starts, line);
            let text = SharedString::from(area.content[range.clone()].to_owned());
            let mut runs = Vec::new();
            let marked_here = marked
                .as_ref()
                .map(|marked| marked.start.max(range.start)..marked.end.min(range.end))
                .filter(|marked| marked.start < marked.end);
            match marked_here {
                Some(marked) => {
                    runs.push(run(marked.start - range.start, style.color, false));
                    runs.push(run(marked.len(), style.color, true));
                    runs.push(run(range.end - marked.end, style.color, false));
                    runs.retain(|run| run.len > 0);
                }
                None => runs.push(run(text.len(), style.color, false)),
            }
            let shaped = window
                .text_system()
                .shape_line(text, font_size, &runs, None);
            rows.push(ShapedRow {
                line,
                start: range.start,
                shaped,
            });
        }

        let placeholder = area.content.is_empty().then(|| {
            let text = area.placeholder.clone();
            let runs = [run(text.len(), self.placeholder_color, false)];
            window
                .text_system()
                .shape_line(text, font_size, &runs, None)
        });

        let caret_width = px(CARET_WIDTH);
        let caret_row = rows.iter().position(|row| row.line == caret_line);
        let caret_x = caret_row.map_or(px(0.), |row| {
            rows[row]
                .shaped
                .x_for_index(area.cursor.saturating_sub(rows[row].start))
        });
        let widest = rows
            .iter()
            .map(|row| row.shaped.width)
            .fold(px(0.), Pixels::max);
        let max_scroll = (widest + caret_width - bounds.size.width).max(px(0.));
        let scroll_left = area
            .scroll_left
            .min(caret_x)
            .max(caret_x + caret_width - bounds.size.width)
            .clamp(px(0.), max_scroll);
        let origin = bounds.left() - scroll_left;

        let mut quads = Vec::new();
        if !selection.is_empty() {
            for (index, row) in rows.iter().enumerate() {
                let end = row.start + row.shaped.len();
                let from = selection.start.max(row.start);
                let to = selection.end.min(end);
                if from > to || selection.end < row.start || selection.start > end {
                    continue;
                }
                let top = bounds.top() + line_height * index as f32;
                let tail = if selection.end > end { px(6.) } else { px(0.) };
                quads.push(fill(
                    Bounds::from_corners(
                        point(origin + row.shaped.x_for_index(from - row.start), top),
                        point(
                            origin + row.shaped.x_for_index(to - row.start) + tail,
                            top + line_height,
                        ),
                    ),
                    self.selection_color,
                ));
            }
        }
        let cursor = (caret_shown && selection.is_empty())
            .then_some(caret_row)
            .flatten()
            .map(|row| {
                let caret_height = (font_size + px(2.)).min(line_height);
                let top =
                    bounds.top() + line_height * row as f32 + (line_height - caret_height) / 2.;
                fill(
                    Bounds::new(
                        point(origin + caret_x, top),
                        size(caret_width, caret_height),
                    ),
                    self.caret_color,
                )
            });
        self.area.update(cx, |area, _| {
            area.scroll_top = scroll_top;
            area.scroll_left = scroll_left;
            area.line_height = line_height;
        });
        AreaPrepaint {
            rows,
            placeholder,
            scroll_left,
            cursor,
            selection: quads,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.area.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.area.clone()),
            cx,
        );
        let line_height = window.line_height();
        let rows = std::mem::take(&mut prepaint.rows);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }
            if let Some(placeholder) = prepaint.placeholder.take() {
                placeholder
                    .paint(
                        bounds.origin,
                        line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            for (index, row) in rows.iter().enumerate() {
                row.shaped
                    .paint(
                        point(
                            bounds.left() - prepaint.scroll_left,
                            bounds.top() + line_height * index as f32,
                        ),
                        line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });
        self.area.update(cx, |area, _| {
            area.shaped = rows;
            area.last_bounds = Some(bounds);
        });
        follow_ime_caret(
            &self.area,
            |area| &area.ime_caret,
            &focus_handle,
            bounds,
            window,
            cx,
        );
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextArea {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let selection = hsla(ui.accent.h, ui.accent.s, ui.accent.l, 0.28);
        div()
            .id(("paneflow-text-area", cx.entity_id().as_u64()))
            .role(gpui::accesskit::Role::MultilineTextInput)
            .aria_label(self.accessible_name.clone())
            .aria_value(SharedString::from(self.content.clone()))
            .aria_placeholder(self.placeholder.clone())
            .w_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Left, _, cx| {
                let to = if this.selection().is_empty() {
                    previous_boundary(&this.content, this.cursor)
                } else {
                    this.selection().start
                };
                this.move_to(to, false, cx);
            }))
            .on_action(cx.listener(|this, _: &Right, _, cx| {
                let to = if this.selection().is_empty() {
                    next_boundary(&this.content, this.cursor)
                } else {
                    this.selection().end
                };
                this.move_to(to, false, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
                this.move_to(previous_boundary(&this.content, this.cursor), true, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| {
                this.move_to(next_boundary(&this.content, this.cursor), true, cx);
            }))
            .on_action(cx.listener(|this, _: &WordLeft, _, cx| {
                this.move_to(previous_word(&this.content, this.cursor), false, cx);
            }))
            .on_action(cx.listener(|this, _: &WordRight, _, cx| {
                this.move_to(next_word(&this.content, this.cursor), false, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectWordLeft, _, cx| {
                this.move_to(previous_word(&this.content, this.cursor), true, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectWordRight, _, cx| {
                this.move_to(next_word(&this.content, this.cursor), true, cx);
            }))
            .on_action(cx.listener(|this, _: &Home, _, cx| {
                let starts = line_starts(&this.content);
                this.move_to(starts[line_of(&starts, this.cursor)], false, cx);
            }))
            .on_action(cx.listener(|this, _: &End, _, cx| {
                let starts = line_starts(&this.content);
                let range = line_range(&this.content, &starts, line_of(&starts, this.cursor));
                this.move_to(range.end, false, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectHome, _, cx| {
                let starts = line_starts(&this.content);
                this.move_to(starts[line_of(&starts, this.cursor)], true, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectEnd, _, cx| {
                let starts = line_starts(&this.content);
                let range = line_range(&this.content, &starts, line_of(&starts, this.cursor));
                this.move_to(range.end, true, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.anchor = 0;
                this.cursor = this.content.len();
                this.touched(cx);
            }))
            .on_action(cx.listener(|this, _: &AreaUp, _, cx| this.vertical(false, false, cx)))
            .on_action(cx.listener(|this, _: &AreaDown, _, cx| this.vertical(true, false, cx)))
            .on_action(cx.listener(|this, _: &AreaSelectUp, _, cx| this.vertical(false, true, cx)))
            .on_action(cx.listener(|this, _: &AreaSelectDown, _, cx| this.vertical(true, true, cx)))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                this.delete_to(previous_boundary(&this.content, this.cursor), cx);
            }))
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                this.delete_to(next_boundary(&this.content, this.cursor), cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| {
                this.delete_to(previous_word(&this.content, this.cursor), cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteWordRight, _, cx| {
                this.delete_to(next_word(&this.content, this.cursor), cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteToStart, _, cx| {
                let starts = line_starts(&this.content);
                this.delete_to(starts[line_of(&starts, this.cursor)], cx);
            }))
            .on_action(cx.listener(|this, _: &AreaNewline, _, cx| {
                this.replace(this.selection(), "\n", cx);
            }))
            .on_action(cx.listener(|_, _: &AreaSubmit, _, cx| cx.emit(TextAreaEvent::Submit)))
            .on_action(cx.listener(|_, _: &AreaCancel, _, cx| cx.emit(TextAreaEvent::Cancel)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if let Some(previous) = this.undo_stack.pop() {
                    this.redo_stack.push(this.snapshot());
                    this.restore(previous, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if let Some(next) = this.redo_stack.pop() {
                    this.undo_stack.push(this.snapshot());
                    this.restore(next, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TextInputCopy, _, cx| {
                if !this.selection().is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        this.content[this.selection()].to_string(),
                    ));
                }
            }))
            .on_action(cx.listener(|this, _: &TextInputCut, _, cx| {
                if !this.selection().is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        this.content[this.selection()].to_string(),
                    ));
                    this.replace(this.selection(), "", cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TextInputPaste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    let text = text.replace("\r\n", "\n").replace('\r', "\n");
                    this.replace(this.selection(), &text, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(AreaElement {
                area: cx.entity(),
                caret_color: ui.text,
                selection_color: selection,
                placeholder_color: ui.muted,
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    struct Host(Entity<TextArea>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .text_size(px(13.))
                .line_height(px(18.))
                .child(self.0.clone())
        }
    }

    fn focused_area(cx: &mut TestAppContext) -> (Entity<TextArea>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::widgets::text_input::register_keybindings(cx);
            register_keybindings(cx);
        });
        let area = cx.new(|cx| TextArea::new("Comment", "Comment", 4, cx));
        let held = area.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Host(held));
        cx.update(|window, cx| area.read(cx).focus_handle.clone().focus(window, cx));
        cx.run_until_parked();
        (area, cx)
    }

    #[test]
    fn lines_are_located_by_their_byte_offsets() {
        let text = "ab\n\ncde";
        let starts = line_starts(text);
        assert_eq!(starts, [0, 3, 4]);
        assert_eq!(line_range(text, &starts, 0), 0..2);
        assert_eq!(line_range(text, &starts, 1), 3..3);
        assert_eq!(line_range(text, &starts, 2), 4..7);
        assert_eq!(line_of(&starts, 0), 0);
        assert_eq!(line_of(&starts, 2), 0);
        assert_eq!(line_of(&starts, 3), 1);
        assert_eq!(line_of(&starts, 7), 2);
    }

    #[test]
    fn the_window_of_rows_follows_the_caret_and_stops_at_the_last_screenful() {
        assert_eq!(visible_from(0, 2, 4, 10), 0);
        assert_eq!(visible_from(0, 4, 4, 10), 1);
        assert_eq!(visible_from(5, 2, 4, 10), 2);
        assert_eq!(visible_from(9, 9, 4, 10), 6);
        assert_eq!(visible_from(3, 0, 4, 2), 0);
    }

    #[gpui::test]
    fn enter_adds_a_line_and_typing_lands_on_it(cx: &mut TestAppContext) {
        let (area, cx) = focused_area(cx);
        cx.simulate_input("one");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("two");
        assert_eq!(
            area.read_with(cx, |area, _| area.value().to_owned()),
            "one\ntwo"
        );
        cx.simulate_keystrokes("up backspace");
        assert_eq!(
            area.read_with(cx, |area, _| area.value().to_owned()),
            "on\ntwo"
        );
    }

    #[gpui::test]
    fn submit_and_cancel_are_reported_without_changing_the_text(cx: &mut TestAppContext) {
        let (area, cx) = focused_area(cx);
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&area, move |_, event: &TextAreaEvent, _| {
                sink.borrow_mut().push(*event);
            })
        });
        cx.simulate_input("x");
        cx.simulate_keystrokes(SUBMIT_KEYSTROKE);
        cx.simulate_keystrokes("escape");
        assert_eq!(
            *seen.borrow(),
            [
                TextAreaEvent::Changed,
                TextAreaEvent::Submit,
                TextAreaEvent::Cancel
            ]
        );
        assert_eq!(area.read_with(cx, |area, _| area.value().to_owned()), "x");
    }

    #[gpui::test]
    fn undo_restores_the_text_before_the_last_edit(cx: &mut TestAppContext) {
        let (area, cx) = focused_area(cx);
        area.update(cx, |area, cx| area.set_value("kept", cx));
        cx.simulate_keystrokes("backspace");
        assert_eq!(area.read_with(cx, |area, _| area.value().to_owned()), "kep");
        area.update_in(cx, |area, window, cx| {
            window.dispatch_action(Box::new(Undo), cx);
            let _ = area;
        });
        cx.run_until_parked();
        assert_eq!(
            area.read_with(cx, |area, _| area.value().to_owned()),
            "kept"
        );
        assert!(!area.read_with(cx, |area, _| area.is_blank()));
    }
}

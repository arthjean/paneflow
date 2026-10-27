use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, Render, ShapedLine, SharedString, Style, Styled, Task, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, div, fill, hsla, point, prelude::*, px,
    relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::utf16::{byte_offset_from_utf16, byte_range_from_utf16, utf16_offset_from_byte};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToStart,
        Undo,
        Redo,
        ShowCharacterPalette,
        TextInputPaste,
        TextInputCut,
        TextInputCopy,
    ]
);

const CARET_BLINK_MS: u64 = 500;
const CARET_WIDTH: f32 = 2.;
const UNDO_COALESCE: Duration = Duration::from_millis(700);
const UNDO_LIMIT: usize = 200;

pub fn register_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("shift-backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("shift-home", SelectHome, Some("TextInput")),
        KeyBinding::new("shift-end", SelectEnd, Some("TextInput")),
    ]);

    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-c", TextInputCopy, Some("TextInput")),
        KeyBinding::new("cmd-v", TextInputPaste, Some("TextInput")),
        KeyBinding::new("cmd-x", TextInputCut, Some("TextInput")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextInput")),
        KeyBinding::new("cmd-z", Undo, Some("TextInput")),
        KeyBinding::new("shift-cmd-z", Redo, Some("TextInput")),
        KeyBinding::new("cmd-left", Home, Some("TextInput")),
        KeyBinding::new("cmd-right", End, Some("TextInput")),
        KeyBinding::new("shift-cmd-left", SelectHome, Some("TextInput")),
        KeyBinding::new("shift-cmd-right", SelectEnd, Some("TextInput")),
        KeyBinding::new("cmd-backspace", DeleteToStart, Some("TextInput")),
        KeyBinding::new("alt-left", WordLeft, Some("TextInput")),
        KeyBinding::new("alt-right", WordRight, Some("TextInput")),
        KeyBinding::new("shift-alt-left", SelectWordLeft, Some("TextInput")),
        KeyBinding::new("shift-alt-right", SelectWordRight, Some("TextInput")),
        KeyBinding::new("alt-backspace", DeleteWordLeft, Some("TextInput")),
        KeyBinding::new("alt-delete", DeleteWordRight, Some("TextInput")),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-c", TextInputCopy, Some("TextInput")),
        KeyBinding::new("ctrl-v", TextInputPaste, Some("TextInput")),
        KeyBinding::new("ctrl-x", TextInputCut, Some("TextInput")),
        KeyBinding::new("ctrl-z", Undo, Some("TextInput")),
        KeyBinding::new("shift-ctrl-z", Redo, Some("TextInput")),
        KeyBinding::new("ctrl-y", Redo, Some("TextInput")),
        KeyBinding::new("ctrl-left", WordLeft, Some("TextInput")),
        KeyBinding::new("ctrl-right", WordRight, Some("TextInput")),
        KeyBinding::new("shift-ctrl-left", SelectWordLeft, Some("TextInput")),
        KeyBinding::new("shift-ctrl-right", SelectWordRight, Some("TextInput")),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, Some("TextInput")),
        KeyBinding::new("ctrl-delete", DeleteWordRight, Some("TextInput")),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
}

struct EditSnapshot {
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

pub struct TextInput {
    pub focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    accessible_name: Option<SharedString>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_left: Pixels,
    is_selecting: bool,
    drag_anchor: Option<Range<usize>>,
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    last_edit: Option<(EditKind, usize, Instant)>,
    blink_anchor: Instant,
    blink_task: Option<Task<()>>,
}

impl TextInput {
    pub fn new(
        initial: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let content: SharedString = initial.into();
        let cursor = content.len();
        Self {
            focus_handle: cx.focus_handle(),
            content,
            placeholder: placeholder.into(),
            accessible_name: None,
            selected_range: cursor..cursor,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_left: px(0.),
            is_selecting: false,
            drag_anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: None,
            blink_anchor: Instant::now(),
            blink_task: None,
        }
    }

    pub fn with_accessible_name(mut self, name: impl Into<SharedString>) -> Self {
        self.accessible_name = Some(name.into());
        self
    }

    pub fn accessible_name(&self) -> SharedString {
        accessible_field_name(self.accessible_name.as_ref(), &self.placeholder)
    }

    pub fn value(&self) -> String {
        self.content.to_string()
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = value.into();
        let cursor = self.content.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.is_selecting = false;
        self.drag_anchor = None;
        self.last_layout = None;
        self.last_bounds = None;
        self.scroll_left = px(0.);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
        self.reset_blink();
        cx.notify();
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_value(SharedString::default(), cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.extend_selection(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.extend_selection(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to(
            self.previous_word_boundary(self.cursor_offset()),
            window,
            cx,
        );
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to(self.next_word_boundary(self.cursor_offset()), window, cx);
    }

    fn delete_to_start(&mut self, _: &DeleteToStart, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(0, window, cx);
    }

    fn delete_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            if self.cursor_offset() == offset {
                return;
            }
            self.extend_selection(offset, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(previous) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore(previous, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore(next, cx);
    }

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn restore(&mut self, snapshot: EditSnapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.last_edit = None;
        self.reset_blink();
        cx.notify();
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn record_edit(&mut self, range: &Range<usize>, new_text: &str) {
        let kind = if new_text.is_empty() {
            EditKind::Delete
        } else {
            EditKind::Insert
        };
        let coalescible = match kind {
            EditKind::Insert => range.is_empty() && new_text.chars().count() == 1,
            EditKind::Delete => self
                .content
                .get(range.clone())
                .is_some_and(|text| text.graphemes(true).count() == 1),
        };
        let mergeable = coalescible
            && match (kind, &self.last_edit) {
                (EditKind::Insert, Some((EditKind::Insert, at, when))) => {
                    range.start == *at
                        && !new_text.starts_with(char::is_whitespace)
                        && when.elapsed() < UNDO_COALESCE
                }
                (EditKind::Delete, Some((EditKind::Delete, at, when))) => {
                    range.end == *at && when.elapsed() < UNDO_COALESCE
                }
                _ => false,
            };
        if mergeable {
            self.redo_stack.clear();
        } else {
            self.push_undo();
        }
        let tail = match kind {
            EditKind::Insert => range.start + new_text.len(),
            EditKind::Delete => range.start,
        };
        self.last_edit = coalescible.then(|| (kind, tail, Instant::now()));
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        self.drag_anchor = None;
        let index = self.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            self.move_to(0, cx);
            self.select_to(self.content.len(), cx);
            self.drag_anchor = Some(0..self.content.len());
        } else if event.click_count == 2 {
            let word = word_range(&self.content, index);
            self.move_to(word.start, cx);
            self.select_to(word.end, cx);
            self.drag_anchor = Some(word);
        } else if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
        self.drag_anchor = None;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.drag_select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn drag_select_to(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(anchor) = self.drag_anchor.clone() else {
            self.select_to(index, cx);
            return;
        };
        let unit = word_range(&self.content, index);
        if unit.start < anchor.start {
            self.move_to(anchor.end, cx);
            self.select_to(unit.start, cx);
        } else {
            self.move_to(anchor.start, cx);
            self.select_to(unit.end.max(anchor.end), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &TextInputPaste, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_clipboard(window, cx);
    }

    pub fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let sanitized = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
            self.replace_text_in_range(None, &sanitized, window, cx);
        }
    }

    fn copy(&mut self, _: &TextInputCopy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    pub fn copy_selection(&self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &TextInputCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.last_edit = None;
        self.reset_blink();
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.scroll_left)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.last_edit = None;
        self.extend_selection(offset, cx);
    }

    fn extend_selection(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.reset_blink();
        cx.notify()
    }

    fn reset_blink(&mut self) {
        self.blink_anchor = Instant::now();
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

    fn offset_from_utf16(&self, offset: usize) -> usize {
        byte_offset_from_utf16(&self.content, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        utf16_offset_from_byte(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn replacement_range_from_utf16(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        match (self.marked_range.as_ref(), range_utf16) {
            (Some(marked_range), Some(range_utf16)) => {
                let marked_text = &self.content[marked_range.clone()];
                let relative = byte_range_from_utf16(marked_text, range_utf16);
                marked_range.start + relative.start..marked_range.start + relative.end
            }
            (_, Some(range_utf16)) => self.range_from_utf16(range_utf16),
            (Some(marked_range), None) => marked_range.clone(),
            (None, None) => self.selected_range.clone(),
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .rev()
            .find_map(|(idx, word)| (idx < offset && !word.trim().is_empty()).then_some(idx))
            .unwrap_or(0)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .find_map(|(idx, word)| {
                let end = idx + word.len();
                (end > offset && !word.trim().is_empty()).then_some(end)
            })
            .unwrap_or(self.content.len())
    }
}

fn word_range(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let probe = if offset == text.len() {
        text.grapheme_indices(true)
            .next_back()
            .map_or(offset, |(idx, _)| idx)
    } else {
        offset
    };
    text.split_word_bound_indices()
        .find(|(idx, word)| *idx <= probe && probe < idx + word.len())
        .map_or(offset..offset, |(idx, word)| idx..idx + word.len())
}

fn caret_visible(since_activity: Duration) -> bool {
    (since_activity.as_millis() / u128::from(CARET_BLINK_MS)).is_multiple_of(2)
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
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
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range_from_utf16(range_utf16.as_ref());
        if self.marked_range.is_none() {
            self.record_edit(&range, new_text);
        }

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.reset_blink();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range_from_utf16(range_utf16.as_ref());
        if self.marked_range.is_none() {
            self.push_undo();
            self.last_edit = None;
        }

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| byte_range_from_utf16(new_text, range_utf16))
            .map(|new_range| range.start + new_range.start..range.start + new_range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.reset_blink();

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let origin = bounds.left() - self.scroll_left;
        Some(Bounds::from_corners(
            point(origin + last_layout.x_for_index(range.start), bounds.top()),
            point(origin + last_layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        if last_layout.text != self.content {
            return None;
        }
        let utf8_index = last_layout.index_for_x(line_point.x + self.scroll_left)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
    caret_color: Hsla,
    selection_color: Hsla,
    placeholder_color: Hsla,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    scroll_left: Pixels,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
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
            .input
            .update(cx, |input, cx| input.caret_shown(window, cx));
        let input = self.input.read(cx);
        let previous_scroll = input.scroll_left;
        let content = input.content.clone();
        let is_empty = content.is_empty();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), self.placeholder_color)
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let caret_width = px(CARET_WIDTH);
        let scroll_left = if is_empty {
            px(0.)
        } else {
            let max_scroll = (line.width + caret_width - bounds.size.width).max(px(0.));
            previous_scroll
                .min(cursor_pos)
                .max(cursor_pos + caret_width - bounds.size.width)
                .clamp(px(0.), max_scroll)
        };
        self.input
            .update(cx, |input, _| input.scroll_left = scroll_left);
        let origin = bounds.left() - scroll_left;
        let caret_height = (font_size + px(2.)).min(bounds.size.height);
        let caret_top = bounds.top() + (bounds.size.height - caret_height) / 2.;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                caret_shown.then(|| {
                    fill(
                        Bounds::new(
                            point(origin + cursor_pos, caret_top),
                            size(caret_width, caret_height),
                        ),
                        self.caret_color,
                    )
                }),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            origin + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            origin + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    self.selection_color,
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            scroll_left,
            cursor,
            selection,
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
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().unwrap();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            line.paint(
                point(bounds.left() - prepaint.scroll_left, bounds.top()),
                window.line_height(),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let selection = hsla(ui.accent.h, ui.accent.s, ui.accent.l, 0.28);

        div()
            .id(("paneflow-text-input", cx.entity_id().as_u64()))
            .role(gpui::accesskit::Role::TextInput)
            .aria_label(self.accessible_name())
            .aria_value(self.content.clone())
            .aria_placeholder(self.placeholder.clone())
            .w_full()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement {
                input: cx.entity(),
                caret_color: gpui::rgb(crate::ui_primitives::FOCUS_BLUE).into(),
                selection_color: selection,
                placeholder_color: ui.muted,
            })
    }
}

pub(crate) fn accessible_field_name(
    explicit: Option<&SharedString>,
    placeholder: &SharedString,
) -> SharedString {
    explicit
        .filter(|name| !name.trim().is_empty())
        .or(Some(placeholder).filter(|name| !name.trim().is_empty()))
        .cloned()
        .unwrap_or_else(|| SharedString::from("Text field"))
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        input: Entity<TextInput>,
    }

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .pl(px(240.0))
                .pt(px(160.0))
                .w(px(900.0))
                .child(self.input.clone())
        }
    }

    fn painted_input<'a>(
        text: &'static str,
        placeholder: &'static str,
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<TextInput>, &'a mut gpui::VisualTestContext) {
        let (host, cx) = cx.add_window_view(move |_window, cx| Host {
            input: cx.new(|cx| TextInput::new(text, placeholder, cx)),
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let input = host.read_with(cx, |host, _| host.input.clone());
        (input, cx)
    }

    #[gpui::test]
    fn the_field_exposes_a_text_role_its_name_and_its_value(cx: &mut gpui::TestAppContext) {
        let (input, cx) = painted_input("hello", "Search settings", cx);
        let node = cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let element = input.render(window, cx).into_element();
                assert_eq!(element.a11y_role(), Some(gpui::accesskit::Role::TextInput));
                let mut node = gpui::accesskit::Node::new(gpui::accesskit::Role::TextInput);
                element.write_a11y_info(&mut node);
                node
            })
        });
        assert_eq!(node.label(), Some("Search settings"));
        assert_eq!(node.value(), Some("hello"));

        input.update(cx, |input, cx| input.set_value("world", cx));
        let (name, value) = cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let element = input.render(window, cx).into_element();
                let mut node = gpui::accesskit::Node::new(gpui::accesskit::Role::TextInput);
                element.write_a11y_info(&mut node);
                (
                    node.label().map(str::to_owned),
                    node.value().map(str::to_owned),
                )
            })
        });
        assert_eq!(name.as_deref(), Some("Search settings"));
        assert_eq!(value.as_deref(), Some("world"));
    }

    #[test]
    fn an_example_placeholder_yields_to_an_explicit_name() {
        let placeholder = SharedString::from("e.g. Dev Server");
        let explicit = SharedString::from("Name");
        assert_eq!(accessible_field_name(Some(&explicit), &placeholder), "Name");
        assert_eq!(accessible_field_name(None, &placeholder), "e.g. Dev Server");
        assert_eq!(
            accessible_field_name(None, &SharedString::default()),
            "Text field"
        );
    }

    #[gpui::test]
    fn points_on_a_field_far_from_the_window_origin_map_to_the_characters_under_them(
        cx: &mut gpui::TestAppContext,
    ) {
        let (input, cx) = painted_input("d\u{e9}j\u{e0} vu \u{e9}t\u{e9}", "", cx);
        let (bounds, third, ninth) = input.read_with(cx, |input, _| {
            let layout = input.last_layout.as_ref().expect("a painted line");
            let byte = |chars: usize| {
                input
                    .content
                    .char_indices()
                    .nth(chars)
                    .map(|(index, _)| index)
                    .expect("a character")
            };
            (
                input.last_bounds.expect("painted bounds"),
                layout.x_for_index(byte(2)),
                layout.x_for_index(byte(8)),
            )
        });
        assert!(bounds.left() > px(200.0), "{bounds:?}");
        let inside = |x: Pixels| point(bounds.left() + x + px(1.0), bounds.top() + px(2.0));

        let offsets = cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                (
                    input.character_index_for_point(inside(third), window, cx),
                    input.character_index_for_point(inside(ninth), window, cx),
                )
            })
        });

        assert_eq!(offsets, (Some(2), Some(8)));
    }

    #[gpui::test]
    fn an_empty_field_has_no_character_under_any_point(cx: &mut gpui::TestAppContext) {
        let (input, cx) = painted_input("", "Search", cx);
        let bounds = input.read_with(cx, |input, _| input.last_bounds.expect("painted bounds"));

        let index = cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.character_index_for_point(
                    point(bounds.left() + px(4.0), bounds.top() + px(2.0)),
                    window,
                    cx,
                )
            })
        });

        assert_eq!(index, None);
    }

    #[gpui::test]
    fn a_typed_burst_undoes_in_one_step_and_redoes_back(cx: &mut gpui::TestAppContext) {
        let (input, cx) = painted_input("", "", cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                for ch in ["d", "e", "v"] {
                    input.replace_text_in_range(None, ch, window, cx);
                }
                input.replace_text_in_range(None, " ", window, cx);
                input.replace_text_in_range(None, "x", window, cx);
                input.undo(&Undo, window, cx);
                assert_eq!(input.value(), "dev");
                input.undo(&Undo, window, cx);
                assert_eq!(input.value(), "");
                input.redo(&Redo, window, cx);
                assert_eq!(input.value(), "dev");
            })
        });
    }

    #[gpui::test]
    fn word_motion_and_deletion_stop_at_word_starts(cx: &mut gpui::TestAppContext) {
        let (input, cx) = painted_input("hello big world", "", cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert_eq!(input.previous_word_boundary(15), 10);
                assert_eq!(input.previous_word_boundary(10), 6);
                assert_eq!(input.next_word_boundary(0), 5);
                assert_eq!(input.next_word_boundary(5), 9);
                input.delete_word_left(&DeleteWordLeft, window, cx);
                assert_eq!(input.value(), "hello big ");
                input.undo(&Undo, window, cx);
                assert_eq!(input.value(), "hello big world");
            })
        });
    }

    #[test]
    fn a_double_click_selects_the_word_under_the_pointer_or_the_last_one() {
        assert_eq!(word_range("hello world", 2), 0..5);
        assert_eq!(word_range("hello world", 11), 6..11);
        assert_eq!(word_range("", 0), 0..0);
    }

    #[test]
    fn the_caret_stays_solid_for_half_a_period_after_activity() {
        assert!(caret_visible(Duration::from_millis(CARET_BLINK_MS - 1)));
        assert!(!caret_visible(Duration::from_millis(CARET_BLINK_MS)));
        assert!(caret_visible(Duration::from_millis(2 * CARET_BLINK_MS)));
    }
}

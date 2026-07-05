//! US-016 (prd-agents-view.md): a focused multi-line text area for
//! the Agents Composer.
//!
//! Design constraint: the existing single-line [`crate::widgets::text_input::TextInput`]
//! is a faithful port of GPUI's `examples/input.rs` with full native text
//! input. This module keeps the textarea smaller, but it still supports the
//! production editing surface the composer needs:
//!
//! - Stores `content: String` with `\n` separators.
//! - Stores cursor + selection anchor as byte offsets (UTF-8 safe via
//!   grapheme-aware navigation through `unicode-segmentation`).
//! - Shapes logical lines through GPUI, with soft-wrap aware hit testing.
//! - Supports native text input and IME composition through
//!   [`gpui::EntityInputHandler`].
//! - Supports click-to-position, drag selection, double-click word selection
//!   and triple-click line selection.
//! - Up/Down movement remains logical-line based, not visual-row based.
//!
//! The widget exposes `pub` callbacks for Enter (parent decides --
//! Composer maps Enter to send and Shift+Enter to insert `\n`) so the
//! same widget works as a Send-on-Enter input or as a free-typing
//! textarea.

// Two methods on the public surface (`is_empty`, `set_value`) are
// not exercised by US-016's Composer but are intentionally part of
// the widget API so other callers (settings flows, US-019's
// attachment composer, US-020's edit-message-fork) can reuse the
// area without a fork. Silence the dead-code warning until those
// land.
#![allow(dead_code)]

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, DispatchPhase, Element, ElementId,
    ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, Font, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, KeyBinding, LayoutId, Length,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, SharedString, Size, Style, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle,
    WeakEntity, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

/// Max delay between mouse clicks to count as a double / triple
/// click. Mirrors the OS default on Linux/macOS (400 ms ~= GTK's
/// `gtk-double-click-time`).
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// Max byte distance between two clicks to still register as a
/// multi-click. A small slop tolerates micro-movements of the mouse
/// between mouse-up and mouse-down on the same character.
const MULTI_CLICK_RADIUS: usize = 2;

actions!(
    paneflow_text_area,
    [
        TaBackspace,
        TaDelete,
        TaLeft,
        TaRight,
        TaUp,
        TaDown,
        TaSelectLeft,
        TaSelectRight,
        TaSelectUp,
        TaSelectDown,
        TaSelectAll,
        TaHome,
        TaEnd,
        TaInsertNewline,
        TaCopy,
        TaCut,
        TaPaste,
        TaSubmit,
        // US-106 (prd-agent-ui-refactor-2026-Q3.md): bypass the queue
        // and send the current draft immediately, interrupting any
        // in-flight turn. Bound to Ctrl+Shift+Enter (Cmd+Shift+Enter
        // on macOS). Composer routes this to `send_prompt_immediate`.
        TaSubmitImmediate,
        // US-019 (prd-agents-view.md): the Composer overlays popups
        // (`+`-menu, `@`-mention, `/`-slash) on top of the textarea.
        // Escape dismisses an open popup; if none is open, the action
        // is a no-op (no default behaviour to swallow). The AC
        // mandates that the trigger character (`@` / `/`) stays in
        // the buffer after dismissal, which falls out naturally
        // because Escape never inserts text.
        TaEscape,
    ]
);

/// Register key bindings for every [`TextArea`] in the app. Call
/// once at startup, after GPUI's App is created. Uses its own
/// `PaneflowTextArea` key context so it does not conflict with
/// `TextInput` bindings on the single-line widget.
pub fn register_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", TaBackspace, Some("PaneflowTextArea")),
        KeyBinding::new("delete", TaDelete, Some("PaneflowTextArea")),
        KeyBinding::new("left", TaLeft, Some("PaneflowTextArea")),
        KeyBinding::new("right", TaRight, Some("PaneflowTextArea")),
        KeyBinding::new("up", TaUp, Some("PaneflowTextArea")),
        KeyBinding::new("down", TaDown, Some("PaneflowTextArea")),
        KeyBinding::new("shift-left", TaSelectLeft, Some("PaneflowTextArea")),
        KeyBinding::new("shift-right", TaSelectRight, Some("PaneflowTextArea")),
        KeyBinding::new("shift-up", TaSelectUp, Some("PaneflowTextArea")),
        KeyBinding::new("shift-down", TaSelectDown, Some("PaneflowTextArea")),
        KeyBinding::new("home", TaHome, Some("PaneflowTextArea")),
        KeyBinding::new("end", TaEnd, Some("PaneflowTextArea")),
        // PRD AC #2: Shift+Enter for a literal newline; plain Enter
        // fires `TaSubmit` which the Composer interprets as "send".
        KeyBinding::new("enter", TaSubmit, Some("PaneflowTextArea")),
        KeyBinding::new("shift-enter", TaInsertNewline, Some("PaneflowTextArea")),
        // US-019: Escape dismisses the Composer's popups via the
        // registered `on_escape` callback (no-op when none is set).
        KeyBinding::new("escape", TaEscape, Some("PaneflowTextArea")),
        // EP-001 (cli-cockpit US-001): the Composer's explicit
        // deliver-then-submit gesture. `secondary` resolves to Cmd on macOS
        // and Ctrl elsewhere. Consumers that install no
        // `on_submit_immediate` callback (inline renames) are unaffected -
        // the action no-ops for them.
        KeyBinding::new(
            "secondary-enter",
            TaSubmitImmediate,
            Some("PaneflowTextArea"),
        ),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-a", TaSelectAll, Some("PaneflowTextArea")),
        KeyBinding::new("cmd-c", TaCopy, Some("PaneflowTextArea")),
        KeyBinding::new("cmd-v", TaPaste, Some("PaneflowTextArea")),
        KeyBinding::new("cmd-x", TaCut, Some("PaneflowTextArea")),
        // US-106: bypass the queue and send immediately.
        KeyBinding::new(
            "cmd-shift-enter",
            TaSubmitImmediate,
            Some("PaneflowTextArea"),
        ),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-a", TaSelectAll, Some("PaneflowTextArea")),
        KeyBinding::new("ctrl-c", TaCopy, Some("PaneflowTextArea")),
        KeyBinding::new("ctrl-v", TaPaste, Some("PaneflowTextArea")),
        // Linux/Windows convention: terminal apps swallow Ctrl+V so
        // `Ctrl+Shift+V` is the second-nature paste binding. We bind
        // both so it works regardless of muscle memory.
        KeyBinding::new("ctrl-shift-v", TaPaste, Some("PaneflowTextArea")),
        KeyBinding::new("ctrl-x", TaCut, Some("PaneflowTextArea")),
        // US-106: bypass the queue and send immediately.
        KeyBinding::new(
            "ctrl-shift-enter",
            TaSubmitImmediate,
            Some("PaneflowTextArea"),
        ),
    ]);
}

/// Callback fired when the user hits Enter (no shift). Receives the
/// current full content. Boxed so callers can store stateful
/// closures (e.g. `move |text| send_runtime.send_prompt(text)`).
type SubmitFn = Rc<RefCell<dyn FnMut(String, &mut Window, &mut App)>>;

/// US-019: callback fired after any mutation that changes the content
/// OR the cursor position. Used by the agents Composer to detect
/// `@` / `/` triggers and to update completion popup state.
type ChangeFn = Rc<RefCell<dyn FnMut(&str, usize, &mut Context<TextArea>)>>;

/// US-019: callback fired on Escape. Boxed for the same reason as
/// `SubmitFn`. The Composer's installer dismisses any open popup;
/// when no popup is open the closure is still invoked but is a
/// no-op, which is harmless.
type EscapeFn = Rc<RefCell<dyn FnMut(&mut Window, &mut App)>>;

/// US-106: callback fired on Ctrl+Shift+Enter (Cmd+Shift+Enter on
/// macOS). Same shape as [`SubmitFn`] - the Composer routes this to
/// `send_prompt_immediate`, which interrupts the current turn before
/// dispatching the new prompt.
type SubmitImmediateFn = Rc<RefCell<dyn FnMut(String, &mut Window, &mut App)>>;

/// Inline decoration anchored to a byte range in [`TextArea::content`].
///
/// US-108a of `tasks/prd-agent-ui-refactor-2026-Q3.md`. The
/// decoration shape is intentionally minimal in this first cut: a
/// byte range + a display label. The paint pass overlays a chip-
/// shaped rectangle on top of the underlying text bytes; the
/// underlying string is preserved verbatim so prompt-send
/// serialization (US-108b) can recover the literal `@path` token
/// and emit the appropriate `ContentBlock::ResourceLink`.
///
/// Decorations are invalidated lazily on any structural edit
/// (replace_selection / clear / set_value): the simplest correct
/// behavior given that the cursor model is still byte-offset based.
/// Atomic cursor + selection over decorations is the follow-up cut
/// captured in the US-108a notes.
#[derive(Debug, Clone)]
pub struct Decoration {
    pub byte_range: Range<usize>,
    pub label: SharedString,
}

pub struct TextArea {
    pub focus_handle: FocusHandle,
    content: String,
    /// Selected byte range. When empty, `start == end` is the cursor
    /// position.
    selected_range: Range<usize>,
    /// `true` when the selection grew leftward, so further
    /// shift-left extends the start; otherwise extends the end.
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// Byte offset where the current mouse drag (or shift-click)
    /// anchor lives. `Some` while the mouse is held down after a
    /// click; `None` between drags. Used by `extend_selection_to` so
    /// drag-select always grows from the original click position.
    drag_anchor: Option<usize>,
    /// `(when, where, count)` of the most recent left-button click.
    /// Lets `register_click` detect double / triple clicks by
    /// checking the time delta + byte proximity to the previous one.
    last_click: Option<(Instant, usize, u8)>,
    placeholder: SharedString,
    on_submit: Option<SubmitFn>,
    on_change: Option<ChangeFn>,
    on_escape: Option<EscapeFn>,
    on_submit_immediate: Option<SubmitImmediateFn>,
    last_bounds: Option<Bounds<Pixels>>,
    /// EP-002 (Launch Pad): when `true`, Enter fires `on_submit` even on an
    /// empty buffer (optional field in a form whose Enter confirms the whole
    /// form). Default `false` - every other consumer keeps the empty no-op.
    submit_on_empty: bool,
    /// Inline chip decorations (US-108a). Rendered as paint-pass
    /// overlays in the `TextAreaContent` element; the underlying
    /// `content` string still carries the literal bytes the
    /// decoration shadows.
    decorations: Vec<Decoration>,
}

impl TextArea {
    /// Build a new TextArea. `placeholder` shows in
    /// `ui.muted` while content is empty.
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            drag_anchor: None,
            last_click: None,
            placeholder: placeholder.into(),
            on_submit: None,
            on_change: None,
            on_escape: None,
            on_submit_immediate: None,
            last_bounds: None,
            submit_on_empty: false,
            decorations: Vec::new(),
        }
    }

    /// Opt into firing `on_submit` on an empty buffer (optional form field
    /// whose Enter confirms the whole form). See [`Self::submit_on_empty`].
    pub fn set_submit_on_empty(&mut self, value: bool) {
        self.submit_on_empty = value;
    }

    /// Register a chip decoration spanning `byte_range` with display
    /// label `label`. Overlapping decorations are rejected silently
    /// so callers can fire-and-forget. US-108a.
    pub fn insert_decoration(&mut self, byte_range: Range<usize>, label: impl Into<SharedString>) {
        if byte_range.start >= byte_range.end || byte_range.end > self.content.len() {
            return;
        }
        if self
            .decorations
            .iter()
            .any(|d| ranges_overlap(&d.byte_range, &byte_range))
        {
            return;
        }
        self.decorations.push(Decoration {
            byte_range,
            label: label.into(),
        });
    }

    /// Snapshot of the current decorations. Returned as a clone so
    /// callers can iterate without holding a borrow.
    pub fn decorations(&self) -> Vec<Decoration> {
        self.decorations.clone()
    }

    /// Remove every decoration. Called when the composer is reset
    /// (Send / draft delete) and as the lazy invalidation step on
    /// any text mutation.
    pub fn clear_decorations(&mut self) {
        self.decorations.clear();
    }

    // -----------------------------------------------------------------
    // Mouse-driven cursor + selection mutators
    // -----------------------------------------------------------------

    /// Collapse the selection to `offset` and arm the drag anchor so
    /// a subsequent mouse-move extends from this point.
    pub(crate) fn place_cursor_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        let clamped = clamp_to_grapheme(&self.content, offset);
        self.selected_range = clamped..clamped;
        self.selection_reversed = false;
        self.marked_range = None;
        self.drag_anchor = Some(clamped);
        cx.notify();
        self.fire_change(cx);
    }

    /// Extend selection from the current `drag_anchor` (or, if no
    /// anchor is set, the active cursor end - which is then promoted
    /// to the persistent anchor for any subsequent drag) to `offset`.
    /// Used by both shift+click and drag.
    pub(crate) fn extend_selection_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let clamped = clamp_to_grapheme(&self.content, offset);
        let anchor = match self.drag_anchor {
            Some(a) => a,
            None => {
                let a = self.cursor();
                self.drag_anchor = Some(a);
                a
            }
        };
        if clamped >= anchor {
            self.selected_range = anchor..clamped;
            self.selection_reversed = false;
        } else {
            self.selected_range = clamped..anchor;
            self.selection_reversed = true;
        }
        self.marked_range = None;
        cx.notify();
        self.fire_change(cx);
    }

    /// Double-click: snap the selection to the word under `offset`.
    /// Word boundaries follow the same heuristic as most editors:
    /// alphanumerics + underscore form a word, everything else is a
    /// boundary.
    pub(crate) fn select_word_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        let (start, end) = word_bounds(&self.content, offset);
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.drag_anchor = Some(start);
        cx.notify();
        self.fire_change(cx);
    }

    /// Triple-click: snap the selection to the full line under
    /// `offset` (excluding the trailing newline).
    pub(crate) fn select_line_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        let start = line_start(&self.content, offset);
        let end = line_end(&self.content, offset);
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.drag_anchor = Some(start);
        cx.notify();
        self.fire_change(cx);
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag_anchor = None;
    }

    /// Returns the click count for `offset`: 1 for a fresh click,
    /// 2 if it lands close enough in time + position to the last
    /// click, 3 if there was already a recent double-click. Caps at
    /// 3 - further clicks roll back to single.
    pub(crate) fn register_click(&mut self, offset: usize) -> u8 {
        let now = Instant::now();
        let count = match self.last_click {
            Some((when, prev_offset, count))
                if now.duration_since(when) < MULTI_CLICK_INTERVAL
                    && offset.abs_diff(prev_offset) <= MULTI_CLICK_RADIUS
                    && count < 3 =>
            {
                count + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, offset, count));
        count
    }

    /// Install a callback that fires on plain Enter. The parent
    /// typically captures the content + clears the area inside the
    /// closure.
    pub fn on_submit<F>(&mut self, f: F)
    where
        F: FnMut(String, &mut Window, &mut App) + 'static,
    {
        self.on_submit = Some(Rc::new(RefCell::new(f)));
    }

    /// US-019: install a callback that fires after every content
    /// mutation OR cursor movement. The Composer uses it to drive the
    /// `@` / `/` completion popups -- detecting trigger characters
    /// and re-running the file walk when the query changes.
    /// Receives `(content, cursor_byte_offset, cx)`.
    pub fn on_change<F>(&mut self, f: F)
    where
        F: FnMut(&str, usize, &mut Context<TextArea>) + 'static,
    {
        self.on_change = Some(Rc::new(RefCell::new(f)));
    }

    /// US-019: install an Escape callback. Bound via `KeyBinding::new
    /// ("escape", TaEscape, "PaneflowTextArea")` so it fires only
    /// while the textarea is focused.
    pub fn on_escape<F>(&mut self, f: F)
    where
        F: FnMut(&mut Window, &mut App) + 'static,
    {
        self.on_escape = Some(Rc::new(RefCell::new(f)));
    }

    /// US-106: install a callback that fires on Ctrl+Shift+Enter
    /// (Cmd+Shift+Enter on macOS). The Composer routes this to
    /// `send_prompt_immediate`, which cancels the in-flight turn before
    /// dispatching the new prompt. Mirrors [`Self::on_submit`].
    pub fn on_submit_immediate<F>(&mut self, f: F)
    where
        F: FnMut(String, &mut Window, &mut App) + 'static,
    {
        self.on_submit_immediate = Some(Rc::new(RefCell::new(f)));
    }

    /// US-019: current cursor byte offset. The Composer reads this to
    /// scan backwards for an active `@<query>` / `/<query>` token.
    pub fn cursor_offset(&self) -> usize {
        self.cursor()
    }

    /// US-019: replace the bytes in `range` with `replacement` and
    /// place the cursor at the end of the inserted text. Public so
    /// the Composer can splice a selected file path into the input
    /// after an `@`-mention pick.
    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        cx: &mut Context<Self>,
    ) {
        let start = clamp_to_grapheme(&self.content, range.start);
        let end = clamp_to_grapheme(&self.content, range.end.max(start));
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.marked_range = None;
        // US-032: `replace_selection` already ends with `fire_change` (the two
        // calls were sequential, not nested, so `try_borrow_mut` didn't guard
        // them). The duplicate re-fired `on_change`, re-triggering the Composer
        // `@`-mention file-walk / popup after a path was picked.
        self.replace_selection(replacement, cx);
    }

    pub fn value(&self) -> String {
        self.content.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.decorations.clear();
        cx.notify();
        self.fire_change(cx);
    }

    pub fn set_value(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.decorations.clear();
        cx.notify();
        self.fire_change(cx);
    }

    /// Select the full content. Used by callers (inline-rename flows) that
    /// open a TextArea pre-populated with a value the user is expected to
    /// replace - selecting it up front avoids a Ctrl+A round-trip.
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        Self::byte_offset_from_utf16_in_text(&self.content, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        let start = clamp_to_grapheme(&self.content, self.offset_from_utf16(range_utf16.start));
        let end = clamp_to_grapheme(&self.content, self.offset_from_utf16(range_utf16.end));
        start.min(end)..end.max(start)
    }

    fn byte_offset_from_utf16_in_text(text: &str, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in text.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn byte_range_from_utf16_in_text(text: &str, range_utf16: &Range<usize>) -> Range<usize> {
        Self::byte_offset_from_utf16_in_text(text, range_utf16.start)
            ..Self::byte_offset_from_utf16_in_text(text, range_utf16.end)
    }

    fn replacement_range_from_utf16(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        match (self.marked_range.as_ref(), range_utf16) {
            (Some(marked_range), Some(range_utf16)) => {
                let marked_text = &self.content[marked_range.clone()];
                let relative = Self::byte_range_from_utf16_in_text(marked_text, range_utf16);
                marked_range.start + relative.start..marked_range.start + relative.end
            }
            (_, Some(range_utf16)) => self.range_from_utf16(range_utf16),
            (Some(marked_range), None) => marked_range.clone(),
            (None, None) => self.selected_range.clone(),
        }
    }

    fn replace_range_inner(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        mark_inserted: bool,
        selected_range: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let start = clamp_to_grapheme(&self.content, range.start);
        let end = clamp_to_grapheme(&self.content, range.end.max(start));
        let range = start..end;
        self.invalidate_decorations_after_edit(&range, replacement.len());
        self.content.replace_range(range.clone(), replacement);
        let inserted = range.start..range.start + replacement.len();
        self.marked_range = (mark_inserted && !replacement.is_empty()).then_some(inserted.clone());
        let selected_range = selected_range.unwrap_or(inserted.end..inserted.end);
        let selected_start = clamp_to_grapheme(&self.content, selected_range.start);
        let selected_end = clamp_to_grapheme(&self.content, selected_range.end.max(selected_start));
        self.selected_range = selected_start..selected_end;
        self.selection_reversed = false;
        cx.notify();
        self.fire_change(cx);
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let clamped = clamp_to_grapheme(&self.content, offset);
        self.selected_range = clamped..clamped;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
        self.fire_change(cx);
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let clamped = clamp_to_grapheme(&self.content, offset);
        if self.selection_reversed {
            self.selected_range.start = clamped;
        } else {
            self.selected_range.end = clamped;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            let new = self.selected_range.end..self.selected_range.start;
            self.selected_range = new;
        }
        self.marked_range = None;
        cx.notify();
        self.fire_change(cx);
    }

    fn replace_selection(&mut self, replacement: &str, cx: &mut Context<Self>) {
        let range = self.selected_range.clone();
        // US-108a: drop any decoration that overlaps the edit and
        // shift every decoration after the edit by the byte delta
        // so chips beyond the edit stay anchored to the same token.
        self.invalidate_decorations_after_edit(&range, replacement.len());
        self.content.replace_range(range.clone(), replacement);
        let new_cursor = range.start + replacement.len();
        self.selected_range = new_cursor..new_cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
        self.fire_change(cx);
    }

    /// US-108a: rebuild the decoration list after an edit that
    /// removed bytes `range` and inserted `inserted_len` bytes in
    /// their place. Decorations strictly before the edit keep their
    /// range; decorations that overlap the edit are dropped (their
    /// underlying token no longer exists); decorations strictly
    /// after the edit shift by `delta = inserted_len - removed_len`.
    fn invalidate_decorations_after_edit(&mut self, range: &Range<usize>, inserted_len: usize) {
        if self.decorations.is_empty() {
            return;
        }
        let removed_len = range.end - range.start;
        let delta: isize = inserted_len as isize - removed_len as isize;
        self.decorations.retain_mut(|d| {
            if d.byte_range.end <= range.start {
                true
            } else if d.byte_range.start >= range.end {
                let new_start = (d.byte_range.start as isize + delta).max(0) as usize;
                let new_end = (d.byte_range.end as isize + delta).max(0) as usize;
                d.byte_range = new_start..new_end;
                true
            } else {
                false
            }
        });
    }

    /// US-019: dispatch to the registered `on_change` callback, if
    /// any. Borrows the closure mutably so re-entrant changes inside
    /// the callback are gracefully ignored (`try_borrow_mut` returns
    /// Err on the second call, which we discard).
    fn fire_change(&mut self, cx: &mut Context<Self>) {
        let Some(cb) = self.on_change.clone() else {
            return;
        };
        if let Ok(mut callback) = cb.try_borrow_mut() {
            callback(&self.content.clone(), self.cursor(), cx);
        }
    }

    // ---- Action handlers ----

    fn backspace(&mut self, _: &TaBackspace, _w: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            // US-123 AC #1: when the cursor sits flush against the
            // right edge of a chip decoration, the FIRST Backspace
            // selects the whole chip (visual selected state); the
            // SECOND Backspace then deletes it atomically -- the
            // existing `replace_selection("")` path drops the
            // decoration via `invalidate_decorations_after_edit`.
            if let Some(range) = self.decoration_ending_at(self.cursor()) {
                self.selected_range = range;
                cx.notify();
                return;
            }
            let prev = prev_grapheme(&self.content, self.cursor());
            if prev == self.cursor() {
                return;
            }
            self.selected_range = prev..self.cursor();
        }
        self.replace_selection("", cx);
    }

    fn delete(&mut self, _: &TaDelete, _w: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            // Symmetrical to backspace (AC #1): when the cursor is at
            // the LEFT edge of a chip, first Delete selects it; second
            // Delete removes it atomically.
            if let Some(range) = self.decoration_starting_at(self.cursor()) {
                self.selected_range = range;
                cx.notify();
                return;
            }
            let next = next_grapheme(&self.content, self.cursor());
            if next == self.cursor() {
                return;
            }
            self.selected_range = self.cursor()..next;
        }
        self.replace_selection("", cx);
    }

    fn left(&mut self, _: &TaLeft, _w: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = prev_grapheme(&self.content, self.cursor());
            // US-123 AC #2: chips are not character-traversable. When
            // the previous grapheme would land inside a decoration's
            // byte range, jump over the whole chip to its left edge.
            let target = self
                .decoration_containing(prev)
                .map(|r| r.start)
                .unwrap_or(prev);
            self.move_to(target, cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &TaRight, _w: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            // Symmetrical to `left`: when the cursor would step INTO
            // a decoration (or starts at its left edge), jump straight
            // to the chip's right edge.
            let here = self.cursor();
            if let Some(range) = self.decoration_containing(here) {
                self.move_to(range.end, cx);
                return;
            }
            let next = next_grapheme(&self.content, self.cursor());
            self.move_to(next, cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    /// US-123 helper: thin instance wrapper around the pure
    /// [`find_decoration_containing`] -- kept inline so existing
    /// callers stay terse.
    fn decoration_containing(&self, offset: usize) -> Option<Range<usize>> {
        find_decoration_containing(&self.decorations, offset)
    }

    fn decoration_ending_at(&self, offset: usize) -> Option<Range<usize>> {
        find_decoration_ending_at(&self.decorations, offset)
    }

    fn decoration_starting_at(&self, offset: usize) -> Option<Range<usize>> {
        find_decoration_starting_at(&self.decorations, offset)
    }

    /// US-035: if `offset` would land STRICTLY inside a chip decoration, snap
    /// to the chip boundary in the direction of travel. Chips are not
    /// character-traversable (US-123); `Home`/`End`/`Up`/`Down` went straight
    /// to `move_to(raw_offset)` and could park the cursor mid-chip, where a
    /// following delete drops the `@path` token. The chip boundaries
    /// themselves are valid stops, so only a strictly-interior offset snaps.
    fn snap_out_of_chip(&self, offset: usize, toward_start: bool) -> usize {
        match self.decoration_containing(offset) {
            Some(range) if offset > range.start => {
                if toward_start {
                    range.start
                } else {
                    range.end
                }
            }
            _ => offset,
        }
    }

    fn up(&mut self, _: &TaUp, _w: &mut Window, cx: &mut Context<Self>) {
        let target = offset_one_line_up(&self.content, self.cursor());
        self.move_to(self.snap_out_of_chip(target, true), cx);
    }

    fn down(&mut self, _: &TaDown, _w: &mut Window, cx: &mut Context<Self>) {
        let target = offset_one_line_down(&self.content, self.cursor());
        self.move_to(self.snap_out_of_chip(target, false), cx);
    }

    fn select_left(&mut self, _: &TaSelectLeft, _w: &mut Window, cx: &mut Context<Self>) {
        let prev = prev_grapheme(&self.content, self.cursor());
        self.select_to(prev, cx);
    }

    fn select_right(&mut self, _: &TaSelectRight, _w: &mut Window, cx: &mut Context<Self>) {
        let next = next_grapheme(&self.content, self.cursor());
        self.select_to(next, cx);
    }

    fn select_up(&mut self, _: &TaSelectUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.select_to(offset_one_line_up(&self.content, self.cursor()), cx);
    }

    fn select_down(&mut self, _: &TaSelectDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.select_to(offset_one_line_down(&self.content, self.cursor()), cx);
    }

    fn select_all(&mut self, _: &TaSelectAll, _w: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &TaHome, _w: &mut Window, cx: &mut Context<Self>) {
        let target = line_start(&self.content, self.cursor());
        self.move_to(self.snap_out_of_chip(target, true), cx);
    }

    fn end(&mut self, _: &TaEnd, _w: &mut Window, cx: &mut Context<Self>) {
        let target = line_end(&self.content, self.cursor());
        self.move_to(self.snap_out_of_chip(target, false), cx);
    }

    fn copy(&mut self, _: &TaCopy, _w: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &TaCut, _w: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_selection("", cx);
        }
    }

    fn paste(&mut self, _: &TaPaste, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text, cx);
        }
    }

    fn insert_newline(&mut self, _: &TaInsertNewline, _w: &mut Window, cx: &mut Context<Self>) {
        self.replace_selection("\n", cx);
    }

    fn escape(&mut self, _: &TaEscape, w: &mut Window, cx: &mut Context<Self>) {
        let Some(cb) = self.on_escape.clone() else {
            return;
        };
        if let Ok(mut callback) = cb.try_borrow_mut() {
            callback(w, cx);
        }
    }

    fn submit(&mut self, _: &TaSubmit, w: &mut Window, cx: &mut Context<Self>) {
        // PRD AC #2: Enter sends. AC #9 (unhappy path): empty submit is a
        // no-op - unless the consumer opted into empty submits (EP-002
        // Launch Pad: the prompt is OPTIONAL, so Enter in the empty field
        // must still confirm the form instead of being swallowed here).
        if !self.submit_on_empty && self.content.trim().is_empty() {
            return;
        }
        let Some(cb) = self.on_submit.clone() else {
            return;
        };
        let content = self.content.clone();
        // Fire synchronously so the parent (Composer) can clear the
        // area and dispatch the prompt in the same frame.
        if let Ok(mut callback) = cb.try_borrow_mut() {
            callback(content, w, cx);
        }
    }

    fn submit_immediate(&mut self, _: &TaSubmitImmediate, w: &mut Window, cx: &mut Context<Self>) {
        // US-106 AC #8: empty submit is still a no-op -- there is
        // nothing to send.
        if self.content.trim().is_empty() {
            return;
        }
        let Some(cb) = self.on_submit_immediate.clone() else {
            return;
        };
        let content = self.content.clone();
        if let Ok(mut callback) = cb.try_borrow_mut() {
            callback(content, w, cx);
        }
    }

    /// Type a literal character into the area. Routed from the
    /// element's `input_handler` in [`Render::render`].
    pub fn insert_char(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.replace_selection(text, cx);
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
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

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range_from_utf16(range_utf16.as_ref());
        self.replace_range_inner(range, new_text, false, None, cx);
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
        let selected_range = new_selected_range_utf16.as_ref().map(|range_utf16| {
            let relative = Self::byte_range_from_utf16_in_text(new_text, range_utf16);
            range.start + relative.start..range.start + relative.end
        });
        self.replace_range_inner(range, new_text, true, selected_range, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let offset = range.start.min(self.content.len());
        let row = self.content[..offset]
            .chars()
            .filter(|ch| *ch == '\n')
            .count();
        let line_start = line_start(&self.content, offset);
        let col = self.content[line_start..offset].chars().count();
        let x = element_bounds.left() + px(col as f32 * 7.0);
        let y = element_bounds.top() + px(row as f32 * 20.0);
        Some(Bounds::new(point(x, y), size(px(1.0), px(20.0))))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self
            .last_bounds
            .and_then(|bounds| bounds.localize(&point))
            .unwrap_or(point);
        let row = (local.y.as_f32() / 20.0).max(0.0).floor() as usize;
        let col = (local.x.as_f32() / 7.0).max(0.0).floor() as usize;
        let mut byte_offset = 0;
        for (idx, line) in self.content.split('\n').enumerate() {
            if idx == row {
                let local = line
                    .char_indices()
                    .nth(col)
                    .map(|(offset, _)| offset)
                    .unwrap_or(line.len());
                return Some(self.offset_to_utf16(byte_offset + local));
            }
            byte_offset += line.len() + 1;
        }
        Some(self.offset_to_utf16(self.content.len()))
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextArea {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let focused = self.focus_handle.is_focused(w);
        let content: SharedString = self.content.clone().into();
        let cursor = self.cursor();
        let sel = self.selected_range.clone();
        let marked_range = self.marked_range.clone();

        // Custom Element does the heavy lifting: shapes each line in
        // `prepaint`, paints text + cursor + selection, and registers
        // mouse listeners for click-to-position + drag-to-select +
        // double-click word / triple-click line. The outer div keeps
        // the key context, focus tracking and all keyboard actions
        // (Ctrl+C/V/X, Backspace, Delete, arrows, …).
        let content_view = TextAreaContent {
            entity: cx.weak_entity(),
            content,
            cursor,
            selected_range: sel,
            marked_range,
            focused,
            placeholder: self.placeholder.clone(),
            font_size: px(13.),
            line_height: px(20.),
            text_color: ui.text,
            muted_color: ui.muted,
            // Match the markdown selection background (markdown_style.rs:70)
            // - accent at 30% alpha keeps the glyphs readable beneath
            // the selection rect.
            selection_color: ui.accent.alpha(0.3),
            cursor_color: ui.accent,
            decorations: self.decorations.clone(),
            chip_bg: ui.subtle,
            chip_border: ui.border,
            // US-004: subtle accent tint for the "just inserted"
            // selected state. Alpha matches Zed's `Tinted` button
            // background opacity (~0.18 against the surface bg).
            chip_accent_bg: ui.accent.alpha(0.18),
            chip_accent_border: ui.accent,
        };

        div()
            .id("paneflow-text-area")
            .key_context("PaneflowTextArea")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::submit_immediate))
            .on_action(cx.listener(Self::escape))
            .text_size(px(13.))
            .text_color(ui.text)
            .min_h(px(20.))
            .child(content_view)
    }
}

// ---------------------------------------------------------------------------
// Custom Element: shaped-text rendering + mouse hit-testing
// ---------------------------------------------------------------------------

/// Per-frame snapshot of the [`TextArea`] state, packaged as a custom
/// GPUI `Element` so we can shape each line in `prepaint` (giving us
/// pixel-exact x positions for every character) and then hit-test
/// mouse clicks in `paint`. Plain `div` children don't expose the
/// shaped-text geometry, hence the dedicated element.
struct TextAreaContent {
    entity: WeakEntity<TextArea>,
    content: SharedString,
    cursor: usize,
    selected_range: Range<usize>,
    marked_range: Option<Range<usize>>,
    focused: bool,
    placeholder: SharedString,
    font_size: Pixels,
    line_height: Pixels,
    text_color: Hsla,
    muted_color: Hsla,
    selection_color: Hsla,
    cursor_color: Hsla,
    /// US-108a: decorations to paint as chips over the shaped text.
    decorations: Vec<Decoration>,
    /// US-108a: surface color for the chip fill. Read from the
    /// active theme at render time so the chip blends with the
    /// composer's card surface.
    chip_bg: Hsla,
    /// US-108a: border / outline color for the chip.
    chip_border: Hsla,
    /// US-004 (visual-parity): tinted fill used when the chip is
    /// "selected" (cursor immediately follows the chip's last byte).
    /// Mirrors Zed `ButtonStyle::Tinted(TintColor::Accent)` applied
    /// via `selected_style(..)` in `mention_crease.rs:103`.
    chip_accent_bg: Hsla,
    /// US-004 (visual-parity): accent border used in the selected
    /// state.
    chip_accent_border: Hsla,
}

impl IntoElement for TextAreaContent {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// Carried from `prepaint` into `paint` and into the mouse-listener
/// closures. The shaped lines are wrapped in `Arc` so we can share
/// them across the three `on_mouse_event` callbacks without cloning
/// the glyph layout buffers.
struct TextAreaPrepaint {
    lines: Arc<Vec<ShapedLineInfo>>,
    hitbox: Hitbox,
}

#[derive(Clone)]
struct ShapedLineInfo {
    /// Byte offset of the first character on this line within the
    /// full textarea content. Advances past every newline.
    byte_start: usize,
    /// Byte offset of the line's end (exclusive of the trailing
    /// newline). For an empty trailing line this equals `byte_start`.
    byte_end: usize,
    /// Top-left Y position where the wrapped line will paint.
    y_top: Pixels,
    /// Total visual height of this logical line after wrapping:
    /// `line_height * (1 + wrap_boundaries.len())`. Used by hit-test
    /// to find which logical line a click landed on when content
    /// soft-wraps across multiple visual rows.
    visual_height: Pixels,
    /// The wrapped, shaped line - owns the glyph layout + wrap
    /// boundaries and exposes `position_for_index` /
    /// `closest_index_for_position` for cursor placement and
    /// hit-testing across wrap boundaries.
    wrapped: Arc<WrappedLine>,
}

fn text_runs_for_segment(
    len: usize,
    byte_start: usize,
    marked_range: Option<&Range<usize>>,
    font: Font,
    color: Hsla,
) -> Vec<TextRun> {
    let base = TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let Some(marked_range) = marked_range else {
        return vec![base];
    };
    let start = marked_range.start.saturating_sub(byte_start).min(len);
    let end = marked_range.end.saturating_sub(byte_start).min(len);
    if start >= end {
        return vec![base];
    }

    let underline = UnderlineStyle {
        color: Some(color),
        thickness: px(1.0),
        wavy: false,
    };
    let mut runs = Vec::with_capacity(3);
    if start > 0 {
        runs.push(TextRun {
            len: start,
            ..base.clone()
        });
    }
    runs.push(TextRun {
        len: end - start,
        underline: Some(underline),
        ..base.clone()
    });
    if end < len {
        runs.push(TextRun {
            len: len - end,
            ..base
        });
    }
    runs
}

impl Element for TextAreaContent {
    type RequestLayoutState = ();
    type PrepaintState = TextAreaPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        // Height must account for soft-wrap rows, not just `\n`
        // separators, otherwise a long single line that wraps
        // visually overflows onto the toolbar below (the previous
        // `content.matches('\n').count()` heuristic ignored wrap
        // boundaries entirely -- only Ctrl+Enter grew the box).
        //
        // Use `request_measured_layout` so the closure receives the
        // actual available width from the layout engine and can
        // shape the text against it before reporting a height. The
        // closure runs during compute_layout (prepaint), which is
        // when the textarea's parent flex container knows how wide
        // it is.
        let content = self.content.clone();
        let font_size = self.font_size;
        let line_height = self.line_height;
        let text_color = self.text_color;
        let font = window.text_style().font();
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = Length::Auto;
        let layout_id = window.request_measured_layout(
            style,
            move |known_dimensions, available_space, window, _cx| {
                let wrap_width = known_dimensions.width.or(match available_space.width {
                    AvailableSpace::Definite(w) => Some(w),
                    _ => None,
                });
                let segments: Vec<&str> = content.split('\n').collect();
                let mut total_rows: usize = 0;
                for segment in &segments {
                    let len = segment.len();
                    let run = TextRun {
                        len,
                        font: font.clone(),
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let runs = [run];
                    let text: SharedString = (*segment).to_string().into();
                    let wrapped = window
                        .text_system()
                        .shape_text(text, font_size, &runs, wrap_width, None);
                    match wrapped {
                        Ok(lines) if !lines.is_empty() => {
                            total_rows += lines[0].wrap_boundaries().len() + 1;
                        }
                        _ => {
                            // Empty segment (trailing newline) still
                            // occupies one row for the caret.
                            total_rows += 1;
                        }
                    }
                }
                let total_rows = total_rows.max(1);
                let measured_height = px(line_height.as_f32() * total_rows as f32);
                Size {
                    width: wrap_width.unwrap_or(px(0.)),
                    height: measured_height,
                }
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> TextAreaPrepaint {
        let font = window.text_style().font();
        let segments: Vec<&str> = self.content.split('\n').collect();
        let last_idx = segments.len().saturating_sub(1);
        let mut lines = Vec::with_capacity(segments.len());
        let mut byte_offset = 0usize;
        let mut y = bounds.origin.y;
        // Wrap at the textarea's available width so long input lines
        // soft-wrap to a new visual row instead of running off the
        // right edge of the composer card.
        let wrap_width = Some(bounds.size.width);
        for (i, segment) in segments.iter().enumerate() {
            let len = segment.len();
            let byte_end = byte_offset + len;
            let runs = text_runs_for_segment(
                len,
                byte_offset,
                self.marked_range.as_ref(),
                font.clone(),
                self.text_color,
            );
            let text: SharedString = (*segment).to_string().into();
            let mut wrapped_lines = window
                .text_system()
                .shape_text(text, self.font_size, &runs, wrap_width, None)
                .unwrap_or_default();
            // `shape_text` may return multiple `WrappedLine`s when the
            // input contains newlines - we already split on `\n`, so
            // each segment shapes into exactly one `WrappedLine`. Take
            // it; skip empty / failed segments silently.
            if let Some(wrapped) = wrapped_lines.drain(..).next() {
                let wrap_rows = wrapped.wrap_boundaries().len() + 1;
                let visual_height = px(self.line_height.as_f32() * wrap_rows as f32);
                lines.push(ShapedLineInfo {
                    byte_start: byte_offset,
                    byte_end,
                    y_top: y,
                    visual_height,
                    wrapped: Arc::new(wrapped),
                });
                y += visual_height;
            } else {
                // Empty segment (e.g. trailing newline) still needs a
                // visual row so the caret can sit on it.
                lines.push(ShapedLineInfo {
                    byte_start: byte_offset,
                    byte_end,
                    y_top: y,
                    visual_height: self.line_height,
                    wrapped: Arc::new(WrappedLine::default()),
                });
                y += self.line_height;
            }
            byte_offset = byte_end + if i < last_idx { 1 } else { 0 };
        }
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        TextAreaPrepaint {
            lines: Arc::new(lines),
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut TextAreaPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(entity) = self.entity.upgrade() {
            let focus_handle = entity.read(cx).focus_handle.clone();
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, entity.clone()),
                cx,
            );
            entity.update(cx, |this, _cx| {
                this.last_bounds = Some(bounds);
            });
        }

        let content_empty = self.content.is_empty();

        // 1. Selection highlight - paint first so the glyphs draw on
        // top. Each logical line may span multiple visual rows after
        // wrapping; `paint_wrapped_selection` handles single-row and
        // multi-row cases.
        if !self.selected_range.is_empty() {
            for line in prepaint.lines.iter() {
                let Some((start_local, end_local)) =
                    sel_overlap_local(&self.selected_range, line.byte_start, line.byte_end)
                else {
                    continue;
                };
                paint_wrapped_selection(
                    &line.wrapped,
                    point(bounds.origin.x, line.y_top),
                    bounds.size.width,
                    self.line_height,
                    start_local,
                    end_local,
                    self.selection_color,
                    window,
                );
            }
        }

        // 2. Glyphs - placeholder for the empty state, otherwise the
        // wrapped lines we built in `prepaint`.
        if content_empty {
            let run = TextRun {
                len: self.placeholder.len(),
                font: window.text_style().font(),
                color: self.muted_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = [run];
            let mut placeholder_lines = window
                .text_system()
                .shape_text(
                    self.placeholder.clone(),
                    self.font_size,
                    &runs,
                    Some(bounds.size.width),
                    None,
                )
                .unwrap_or_default();
            if let Some(placeholder) = placeholder_lines.drain(..).next() {
                // Indent so the blinking caret doesn't overlap the
                // first placeholder glyph.
                let origin = if self.focused {
                    point(bounds.origin.x + px(4.0), bounds.origin.y)
                } else {
                    bounds.origin
                };
                let _ = placeholder.paint(
                    origin,
                    self.line_height,
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                );
            }
        } else {
            for line in prepaint.lines.iter() {
                let origin = point(bounds.origin.x, line.y_top);
                let _ = line.wrapped.paint(
                    origin,
                    self.line_height,
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                );
            }
        }

        // US-108a: decoration chip overlays. Each decoration paints
        // a filled, rounded rectangle behind a label that REPLACES
        // the underlying glyphs (the underlying text was already
        // drawn -- the chip sits on top, so the bare bytes only
        // peek through if rendering fails). The chip rect is
        // computed via `position_for_index` on the wrapped line:
        // start and end x positions delimit the chip's horizontal
        // span; the y is the line's `y_top` plus the position's
        // wrap-row offset.
        if !self.content.is_empty() {
            for deco in &self.decorations {
                let Some(line) = prepaint.lines.iter().find(|l| {
                    deco.byte_range.start >= l.byte_start && deco.byte_range.end <= l.byte_end
                }) else {
                    continue;
                };
                let local_start = deco.byte_range.start.saturating_sub(line.byte_start);
                let local_end = deco.byte_range.end.saturating_sub(line.byte_start);
                let Some(start_pos) = line
                    .wrapped
                    .position_for_index(local_start, self.line_height)
                else {
                    continue;
                };
                let Some(end_pos) = line.wrapped.position_for_index(local_end, self.line_height)
                else {
                    continue;
                };
                // Same-row chip only -- a chip that straddles a
                // soft-wrap boundary is too fiddly for this first
                // cut and would need a multi-segment paint. The
                // mention popover commits its insert as a single
                // contiguous run so soft-wrap inside a chip is rare.
                if start_pos.y != end_pos.y {
                    continue;
                }
                let chip_x = bounds.origin.x + start_pos.x - px(2.);
                // US-004: chip height = line_height - 1px (Zed
                // `mention_crease.rs:95-97`). Offset y by 0.5 so the
                // shorter chip sits visually centred within the line.
                let chip_h = self.line_height - px(1.);
                let chip_y = line.y_top + start_pos.y + px(0.5);
                let chip_w = (end_pos.x - start_pos.x) + px(4.);
                let chip_bounds = Bounds::new(point(chip_x, chip_y), size(chip_w, chip_h));
                // US-004 (visual-parity): "selected" state when the
                // cursor sits immediately after the chip's last byte
                // - mirrors Zed's `selected_style(ButtonStyle::Tinted
                // (TintColor::Accent))` after a fresh mention insert.
                // Moving the cursor away returns the chip to the
                // Outlined default styling.
                let is_selected = deco.byte_range.end == self.cursor;
                let (fill, border) = if is_selected {
                    (self.chip_accent_bg, self.chip_accent_border)
                } else {
                    (self.chip_bg, self.chip_border)
                };
                window.paint_quad(gpui::quad(
                    chip_bounds,
                    px(4.0),
                    fill,
                    px(1.0),
                    border,
                    gpui::BorderStyle::Solid,
                ));
                // Paint the label on top so the chip shows the
                // resolved display text even when the underlying
                // bytes differ (e.g. `@src/main.rs` -> "main.rs").
                let label_run = TextRun {
                    len: deco.label.len(),
                    font: window.text_style().font(),
                    color: self.text_color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let runs = [label_run];
                let mut shaped = window
                    .text_system()
                    .shape_text(
                        deco.label.clone(),
                        self.font_size,
                        &runs,
                        Some(chip_w),
                        None,
                    )
                    .unwrap_or_default();
                if let Some(label_line) = shaped.drain(..).next() {
                    let _ = label_line.paint(
                        point(chip_x + px(2.), chip_y),
                        self.line_height,
                        TextAlign::Left,
                        Some(chip_bounds),
                        window,
                        cx,
                    );
                }
            }
        }

        // 3. Caret. `position_for_index` accounts for wrap rows so
        // the caret jumps to the correct visual row even when the
        // user is mid-soft-wrapped-line.
        if self.focused {
            let (caret_x, caret_y) = if content_empty {
                (bounds.origin.x, bounds.origin.y)
            } else {
                let cursor_pos = self.cursor;
                let line = prepaint
                    .lines
                    .iter()
                    .find(|l| cursor_pos >= l.byte_start && cursor_pos <= l.byte_end)
                    .or_else(|| prepaint.lines.last());
                match line {
                    Some(line) => {
                        let local = cursor_pos.saturating_sub(line.byte_start);
                        match line.wrapped.position_for_index(local, self.line_height) {
                            Some(pos) => (bounds.origin.x + pos.x, line.y_top + pos.y),
                            None => (bounds.origin.x, line.y_top),
                        }
                    }
                    None => (bounds.origin.x, bounds.origin.y),
                }
            };
            let caret = Bounds::new(point(caret_x, caret_y), size(px(1.5), self.line_height));
            window.paint_quad(fill(caret, self.cursor_color));
        }

        // 4. Mouse listeners. Each closure clones what it needs
        // (entity, shaped lines, hitbox, geometry) so it stays valid
        // past the paint frame.
        let entity_down = self.entity.clone();
        let entity_move = self.entity.clone();
        let entity_up = self.entity.clone();
        let lines_down = prepaint.lines.clone();
        let lines_move = prepaint.lines.clone();
        let hitbox_down = prepaint.hitbox.clone();
        let line_height = self.line_height;
        let bounds_origin = bounds.origin;

        // Mouse down: focus + place caret. Shift extends, double-click
        // selects the word, triple-click the line.
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if ev.button != MouseButton::Left {
                return;
            }
            if !hitbox_down.is_hovered(w) {
                return;
            }
            let offset = hit_test(&lines_down, bounds_origin, line_height, ev.position);
            let shift = ev.modifiers.shift;
            entity_down
                .update(cx, |this, cx| {
                    this.focus_handle.focus(w, cx);
                    if shift {
                        this.extend_selection_to(offset, cx);
                    } else {
                        let count = this.register_click(offset);
                        match count {
                            1 => this.place_cursor_at(offset, cx),
                            2 => this.select_word_at(offset, cx),
                            _ => this.select_line_at(offset, cx),
                        }
                    }
                })
                .ok();
        });

        // Mouse move while dragging: extend selection. No hitbox
        // check so drags past the textarea bounds still grow the
        // selection - `hit_test` clamps to the nearest line / edge.
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if !ev.dragging() {
                return;
            }
            let offset = hit_test(&lines_move, bounds_origin, line_height, ev.position);
            entity_move
                .update(cx, |this, cx| {
                    if this.drag_anchor.is_some() {
                        this.extend_selection_to(offset, cx);
                    }
                })
                .ok();
        });

        // Mouse up: clear the drag anchor so the next mouse-down
        // starts a fresh selection rather than extending the prior
        // one.
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if ev.button != MouseButton::Left {
                return;
            }
            entity_up
                .update(cx, |this, _cx| {
                    this.end_drag();
                })
                .ok();
        });
    }
}

/// Convert a mouse position into the byte offset within the textarea
/// content. Walks the wrapped lines to find which logical line owns
/// the click's Y, then defers to `closest_index_for_position` which
/// is wrap-aware (returns the right byte even when the line spans
/// multiple visual rows).
fn hit_test(
    lines: &[ShapedLineInfo],
    origin: Point<Pixels>,
    line_height: Pixels,
    pos: Point<Pixels>,
) -> usize {
    if lines.is_empty() {
        return 0;
    }
    // Find which logical line owns this Y. Clamp out-of-range clicks
    // to the first / last line so drags past the edges still update
    // the selection.
    let line = lines
        .iter()
        .find(|l| pos.y >= l.y_top && pos.y < l.y_top + l.visual_height)
        .or_else(|| {
            if pos.y < lines[0].y_top {
                lines.first()
            } else {
                lines.last()
            }
        })
        .unwrap_or(&lines[0]);
    // Position relative to the line's top-left, used by the wrapped
    // layout to pick the right visual row + character offset.
    let local = point(
        (pos.x - origin.x).max(px(0.0)),
        (pos.y - line.y_top).max(px(0.0)),
    );
    let in_line = line
        .wrapped
        .closest_index_for_position(local, line_height)
        .unwrap_or_else(|e| e);
    line.byte_start + in_line.min(line.byte_end - line.byte_start)
}

/// Paint the selection highlight for the byte range
/// `[start_local, end_local)` within a wrapped line. Handles
/// single-row and multi-row cases by querying `position_for_index`
/// at both ends and walking the wrap boundaries in between.
#[allow(clippy::too_many_arguments)]
fn paint_wrapped_selection(
    wrapped: &WrappedLine,
    line_origin: Point<Pixels>,
    available_width: Pixels,
    line_height: Pixels,
    start_local: usize,
    end_local: usize,
    color: Hsla,
    window: &mut Window,
) {
    let Some(start_pos) = wrapped.position_for_index(start_local, line_height) else {
        return;
    };
    let Some(end_pos) = wrapped.position_for_index(end_local, line_height) else {
        return;
    };
    let start_abs = point(line_origin.x + start_pos.x, line_origin.y + start_pos.y);
    let end_abs = point(line_origin.x + end_pos.x, line_origin.y + end_pos.y);
    // Right edge of selectable area = the wrap width (where text
    // soft-wraps). For the last visual row of a logical line we use
    // the line's own content width so the highlight doesn't trail
    // past the last glyph.
    let row_right = line_origin.x + available_width;

    if (start_abs.y.as_f32() - end_abs.y.as_f32()).abs() < line_height.as_f32() * 0.5 {
        // Same visual row.
        let rect = Bounds::new(
            start_abs,
            size((end_abs.x - start_abs.x).max(px(1.0)), line_height),
        );
        window.paint_quad(fill(rect, color));
        return;
    }

    // Multi-row: first row → end of available width, middle rows →
    // full width, last row → from left to end_x.
    let first_row = Bounds::new(
        start_abs,
        size((row_right - start_abs.x).max(px(1.0)), line_height),
    );
    window.paint_quad(fill(first_row, color));

    let mut y = start_abs.y + line_height;
    while y + line_height * 0.5 < end_abs.y {
        let middle = Bounds::new(point(line_origin.x, y), size(available_width, line_height));
        window.paint_quad(fill(middle, color));
        y += line_height;
    }

    let last_row = Bounds::new(
        point(line_origin.x, end_abs.y),
        size((end_abs.x - line_origin.x).max(px(1.0)), line_height),
    );
    window.paint_quad(fill(last_row, color));
}

/// Intersection of `[sel.start, sel.end)` with the line span
/// `[line_start, line_end]`. Returns local-to-line byte offsets, or
/// `None` when the selection is empty or doesn't touch this line.
fn sel_overlap_local(
    sel: &Range<usize>,
    line_start: usize,
    line_end: usize,
) -> Option<(usize, usize)> {
    if sel.is_empty() {
        return None;
    }
    let a = sel.start.max(line_start);
    let b = sel.end.min(line_end);
    if a >= b {
        return None;
    }
    Some((a - line_start, b - line_start))
}

/// Word boundaries around `offset`. Alphanumerics + underscore form
/// a word; everything else is a boundary. Used by double-click word
/// selection.
/// US-108a: whether two byte ranges share any overlap.
fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// US-123: decoration whose byte range contains `offset`. Inclusive
/// on `start`, exclusive on `end` -- a cursor at `end` is "just past"
/// the chip, NOT inside. Pure (no `self`) so the boundary semantics
/// are unit-testable without constructing a `TextArea` (which needs
/// a GPUI `FocusHandle` -- only constructible inside `Context<Self>`).
fn find_decoration_containing(decorations: &[Decoration], offset: usize) -> Option<Range<usize>> {
    decorations
        .iter()
        .find(|d| d.byte_range.start <= offset && offset < d.byte_range.end)
        .map(|d| d.byte_range.clone())
}

/// US-123: decoration whose right edge sits at `offset` (cursor is
/// just past the chip). Used by backspace's first-press select.
fn find_decoration_ending_at(decorations: &[Decoration], offset: usize) -> Option<Range<usize>> {
    decorations
        .iter()
        .find(|d| d.byte_range.end == offset)
        .map(|d| d.byte_range.clone())
}

/// US-123: decoration whose left edge sits at `offset` (cursor is
/// just before the chip). Used by delete's first-press select.
fn find_decoration_starting_at(decorations: &[Decoration], offset: usize) -> Option<Range<usize>> {
    decorations
        .iter()
        .find(|d| d.byte_range.start == offset)
        .map(|d| d.byte_range.clone())
}

fn word_bounds(content: &str, offset: usize) -> (usize, usize) {
    let offset = clamp_to_grapheme(content, offset);
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = offset;
    while start > 0 {
        let prev = prev_grapheme(content, start);
        let is_word = content[prev..start]
            .chars()
            .next()
            .is_some_and(is_word_char);
        if !is_word {
            break;
        }
        start = prev;
    }
    let mut end = offset;
    while end < content.len() {
        let next = next_grapheme(content, end);
        let is_word = content[end..next].chars().next().is_some_and(is_word_char);
        if !is_word {
            break;
        }
        end = next;
    }
    (start, end)
}

// ---------------------------------------------------------------------------
// Grapheme / line navigation helpers (private)
// ---------------------------------------------------------------------------

fn prev_grapheme(s: &str, offset: usize) -> usize {
    s.grapheme_indices(true)
        .rev()
        .find_map(|(i, _)| if i < offset { Some(i) } else { None })
        .unwrap_or(0)
}

fn next_grapheme(s: &str, offset: usize) -> usize {
    let mut last = s.len();
    for (i, _) in s.grapheme_indices(true) {
        if i > offset {
            last = i;
            return last;
        }
    }
    last
}

fn clamp_to_grapheme(s: &str, offset: usize) -> usize {
    let off = offset.min(s.len());
    if off == 0 || off == s.len() {
        return off;
    }
    // Snap to the nearest grapheme boundary at or before `off`.
    s.grapheme_indices(true)
        .map(|(i, g)| (i, i + g.len()))
        .find_map(|(start, end)| {
            if start <= off && off < end {
                Some(start)
            } else {
                None
            }
        })
        .unwrap_or(off)
}

/// Byte offset of the `\n` that closes the line containing
/// `offset`, OR the end-of-string if no further `\n`. The cursor at
/// this offset still sits on the same logical line.
fn line_end(s: &str, offset: usize) -> usize {
    s[offset..]
        .find('\n')
        .map(|rel| offset + rel)
        .unwrap_or(s.len())
}

/// Byte offset of the character that starts the line containing
/// `offset`. Zero when there is no preceding `\n`.
fn line_start(s: &str, offset: usize) -> usize {
    s[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Move the cursor "one line up". Targets the same visual column
/// (computed in chars from the start of the current line). Returns
/// 0 when already on the first line.
fn offset_one_line_up(s: &str, offset: usize) -> usize {
    let cur_start = line_start(s, offset);
    if cur_start == 0 {
        return 0;
    }
    let col = s[cur_start..offset].chars().count();
    let prev_end = cur_start - 1; // index of `\n` that closes the previous line
    let prev_start = line_start(s, prev_end);
    let prev_line = &s[prev_start..prev_end];
    let mut col_iter = prev_line.char_indices();
    for _ in 0..col {
        if col_iter.next().is_none() {
            return prev_end;
        }
    }
    match col_iter.next() {
        Some((i, _)) => prev_start + i,
        None => prev_end,
    }
}

/// Move the cursor "one line down". Mirrors
/// [`offset_one_line_up`].
fn offset_one_line_down(s: &str, offset: usize) -> usize {
    let cur_start = line_start(s, offset);
    let cur_end = line_end(s, offset);
    if cur_end == s.len() {
        return s.len();
    }
    let col = s[cur_start..offset].chars().count();
    let next_start = cur_end + 1;
    let next_end = line_end(s, next_start);
    let next_line = &s[next_start..next_end];
    let mut col_iter = next_line.char_indices();
    for _ in 0..col {
        if col_iter.next().is_none() {
            return next_end;
        }
    }
    match col_iter.next() {
        Some((i, _)) => next_start + i,
        None => next_end,
    }
}

/// Walk `content` line-by-line, yielding each line and whether it
/// is closed by a trailing `\n`. Avoids the allocation of `lines()`'s
/// owned iterator AND keeps trailing-newline information so the
/// caller can advance the byte cursor correctly.
fn split_keeping_newlines(s: &str) -> impl Iterator<Item = LineSlice<'_>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            out.push(LineSlice {
                text: &s[start..i],
                has_trailing_newline: true,
            });
            start = i + 1;
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(LineSlice {
            text: &s[start..],
            has_trailing_newline: false,
        });
    }
    out.into_iter()
}

struct LineSlice<'a> {
    text: &'a str,
    has_trailing_newline: bool,
}

impl<'a> LineSlice<'a> {
    fn bytes_without_trailing_newline(&self) -> usize {
        self.text.len()
    }
    fn full_len(&self) -> usize {
        self.text.len() + if self.has_trailing_newline { 1 } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prev_grapheme_at_start_returns_zero() {
        assert_eq!(prev_grapheme("hello", 0), 0);
    }

    #[test]
    fn next_grapheme_past_end_returns_end() {
        assert_eq!(next_grapheme("hi", 100), 2);
    }

    #[test]
    fn clamp_to_grapheme_snaps_inside_multibyte_codepoint_to_start() {
        let s = "éa";

        assert_eq!(clamp_to_grapheme(s, 0), 0);
        assert_eq!(clamp_to_grapheme(s, 1), 0);
        assert_eq!(clamp_to_grapheme(s, 2), 2);
        assert_eq!(clamp_to_grapheme(s, 3), 3);
    }

    #[test]
    fn clamp_to_grapheme_snaps_inside_zwj_cluster_to_start() {
        let s = "a👨‍👩‍👧‍👦b";
        let cluster_start = "a".len();
        let cluster_end = s.len() - "b".len();

        for offset in cluster_start + 1..cluster_end {
            assert_eq!(clamp_to_grapheme(s, offset), cluster_start);
        }
        assert_eq!(clamp_to_grapheme(s, cluster_end), cluster_end);
    }

    #[test]
    fn line_end_handles_trailing_newline() {
        let s = "one\ntwo\n";
        assert_eq!(line_end(s, 0), 3);
        assert_eq!(line_end(s, 4), 7);
        assert_eq!(line_end(s, 8), 8);
    }

    #[test]
    fn line_start_handles_first_line() {
        let s = "one\ntwo";
        assert_eq!(line_start(s, 0), 0);
        assert_eq!(line_start(s, 2), 0);
        assert_eq!(line_start(s, 4), 4);
    }

    #[test]
    fn line_up_preserves_column() {
        // "abcde\nxy" -- cursor at offset 8 (end of "xy"), column 2.
        let s = "abcde\nxy";
        let up = offset_one_line_up(s, 8);
        // Up should land on offset 2 (column 2 of "abcde").
        assert_eq!(up, 2);
    }

    #[test]
    fn line_up_when_already_first_line_returns_zero() {
        let s = "single line";
        assert_eq!(offset_one_line_up(s, 5), 0);
    }

    #[test]
    fn line_down_preserves_column_or_clamps() {
        // "abcde\nxy" -- cursor at column 4 of line 0; line 1 has 2 chars.
        let s = "abcde\nxy";
        let down = offset_one_line_down(s, 4);
        // Column 4 on "xy" exceeds length, so clamps to end of "xy" -> 8.
        assert_eq!(down, 8);
    }

    #[test]
    fn split_keeping_newlines_handles_empty_lines() {
        let s = "a\n\nb";
        let lines: Vec<_> = split_keeping_newlines(s)
            .map(|l| (l.text.to_string(), l.has_trailing_newline))
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], ("a".to_string(), true));
        assert_eq!(lines[1], ("".to_string(), true));
        assert_eq!(lines[2], ("b".to_string(), false));
    }

    #[test]
    fn sel_overlap_returns_none_for_empty_selection() {
        let sel = 0..0;
        assert_eq!(sel_overlap_local(&sel, 0, 10), None);
    }

    #[test]
    fn sel_overlap_returns_intersection() {
        // `sel_overlap_local` returns offsets LOCAL to the line, so the
        // returned tuple is (sel_clamp - line_start, ...).
        let sel = 2..8;
        assert_eq!(sel_overlap_local(&sel, 0, 5), Some((2, 5)));
        assert_eq!(sel_overlap_local(&sel, 6, 10), Some((0, 2)));
    }

    #[test]
    fn ranges_overlap_basic() {
        assert!(ranges_overlap(&(0..5), &(3..7)));
        assert!(ranges_overlap(&(3..7), &(0..5)));
        assert!(!ranges_overlap(&(0..5), &(5..10)));
        assert!(!ranges_overlap(&(0..5), &(10..15)));
        assert!(ranges_overlap(&(0..10), &(3..6)));
    }

    // ----------------------------------------------------------------
    // US-123: atomic-cursor helpers. These tests cover the pure
    // boundary lookup functions; the cursor-movement integration is
    // exercised manually inside the running app (GPUI `Context<Self>`
    // cannot be constructed from a unit test, so the action handlers
    // themselves are not in scope here).
    // ----------------------------------------------------------------

    fn make_decoration(byte_range: Range<usize>, label: &str) -> Decoration {
        Decoration {
            byte_range,
            label: label.to_string().into(),
        }
    }

    #[test]
    fn decoration_containing_inclusive_start_exclusive_end() {
        let decos = vec![make_decoration(6..14, "file.rs")];
        // Offset just inside the left edge -> inside.
        assert!(find_decoration_containing(&decos, 6).is_some());
        assert!(find_decoration_containing(&decos, 10).is_some());
        assert!(find_decoration_containing(&decos, 13).is_some());
        // Right-edge offset is OUTSIDE -- cursor sitting at the end
        // of the chip is "just past" the decoration.
        assert!(find_decoration_containing(&decos, 14).is_none());
        // Outside the chip entirely.
        assert!(find_decoration_containing(&decos, 5).is_none());
    }

    #[test]
    fn decoration_ending_at_matches_right_edge_only() {
        let decos = vec![make_decoration(6..14, "file.rs")];
        // Cursor just past the chip -- backspace's first-press anchor.
        assert!(find_decoration_ending_at(&decos, 14).is_some());
        // Anywhere else -> None.
        assert!(find_decoration_ending_at(&decos, 13).is_none());
        assert!(find_decoration_ending_at(&decos, 15).is_none());
    }

    #[test]
    fn decoration_starting_at_matches_left_edge_only() {
        let decos = vec![make_decoration(6..14, "file.rs")];
        // Cursor at the chip's left edge -- delete's first-press anchor.
        assert!(find_decoration_starting_at(&decos, 6).is_some());
        assert!(find_decoration_starting_at(&decos, 5).is_none());
        assert!(find_decoration_starting_at(&decos, 7).is_none());
    }

    #[test]
    fn decoration_helpers_handle_multiple_chips() {
        // Two chips: one near the start, one near the end. Each helper
        // must locate the matching one independently.
        let decos = vec![make_decoration(0..2, "a"), make_decoration(12..14, "b")];
        assert!(find_decoration_containing(&decos, 0).is_some());
        assert!(find_decoration_containing(&decos, 13).is_some());
        assert!(find_decoration_containing(&decos, 5).is_none());
        assert_eq!(
            find_decoration_ending_at(&decos, 2).map(|r| r.start),
            Some(0)
        );
        assert_eq!(
            find_decoration_ending_at(&decos, 14).map(|r| r.start),
            Some(12)
        );
        assert_eq!(
            find_decoration_starting_at(&decos, 0).map(|r| r.end),
            Some(2)
        );
        assert_eq!(
            find_decoration_starting_at(&decos, 12).map(|r| r.end),
            Some(14)
        );
    }
}

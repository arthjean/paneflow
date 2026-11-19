use gpui::prelude::FluentBuilder;

use super::*;
use crate::app::agent_send::{RangeComment, SEND_TO_AGENT, range_label};
use crate::app::diff_dock::code::edit::TrackerWindow;
use crate::app::diff_dock::code::rows::RowSpan;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::widgets::text_area::{TextArea, TextAreaEvent};

pub(crate) const COMMENT_BLOCK_ROWS: u32 = 8;
const COMMENT_FIELD_ROWS: usize = 4;
pub(crate) const GUTTER_HOVER_DELAY: Duration = Duration::from_millis(200);
pub(crate) const COMMENT_BUTTON_TOOLTIP: &str = "Comment on this line (drag for a range)";
pub(crate) const COMMENT_SELECTION: &str = "Comment Selection";
pub(crate) const CANCEL_COMMENT: &str = "Cancel";
pub(crate) const COMMENT_KEY_CONTEXT: &str = "CodeComment";
const COMMENT_PLACEHOLDER: &str = "Tell the agent what to change";
const REPLACE_PROMPT: &str = "Replace the comment you are writing?";
const COMMENT_BUTTON_SIZE: f32 = 16.0;
const COMMENT_BUTTON_LEFT: f32 = 9.0;
const BLOCK_INSET_R: f32 = 28.0;

pub(super) struct CommentDraft {
    pub(super) lines: Range<usize>,
    pub(super) field: gpui::Entity<TextArea>,
    _events: gpui::Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GutterHover {
    pub(super) line: usize,
    pub(super) armed: bool,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CommentDrag {
    anchor: usize,
    head: usize,
}

impl CommentDrag {
    fn lines(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head) + 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorMenuAction {
    CommentSelection,
    Copy,
    Paste,
    SelectAll,
}

pub(crate) fn shift_comment_lines(
    lines: Range<usize>,
    window: TrackerWindow,
) -> Option<Range<usize>> {
    let start = window.start_line as usize;
    let before = window.before_len as usize;
    let after = window.after_len as usize;
    let changed_end = start + before;
    if changed_end <= lines.start {
        let shifted = lines.start + after - before.min(lines.start + after);
        return Some(shifted..shifted + lines.len());
    }
    if start >= lines.end {
        return Some(lines);
    }
    let new_start = lines.start.min(start);
    let new_end = if changed_end >= lines.end {
        start + after
    } else {
        lines.end + after - before.min(lines.end + after)
    };
    (new_end > new_start).then_some(new_start..new_end)
}

fn editor_menu(has_selection: bool) -> ActionMenu<EditorMenuAction> {
    let comment = if has_selection {
        MenuItem::new(COMMENT_SELECTION, EditorMenuAction::CommentSelection)
    } else {
        MenuItem::disabled(COMMENT_SELECTION)
    };
    let copy = if has_selection {
        MenuItem::new("Copy", EditorMenuAction::Copy)
    } else {
        MenuItem::disabled("Copy")
    };
    ActionMenu::new()
        .item(
            comment
                .icon(crate::app::menu_icons::TERMINAL)
                .mnemonic('m')
                .shortcut(Some(
                    crate::keybindings::format_keystroke(COMMENT_KEYSTROKE).into(),
                )),
        )
        .separator()
        .item(copy.icon(crate::app::menu_icons::COPY).mnemonic('c'))
        .item(MenuItem::new("Paste", EditorMenuAction::Paste).mnemonic('p'))
        .item(MenuItem::new("Select All", EditorMenuAction::SelectAll).mnemonic('a'))
}

impl CodeView {
    pub(crate) fn comment_lines(&self) -> Option<Range<usize>> {
        self.comment.as_ref().map(|draft| draft.lines.clone())
    }

    #[cfg(test)]
    pub(crate) fn comment_field(&self) -> Option<gpui::Entity<TextArea>> {
        self.comment.as_ref().map(|draft| draft.field.clone())
    }

    pub(super) fn comment_focused(&self, window: &Window, cx: &App) -> bool {
        self.comment
            .as_ref()
            .is_some_and(|draft| draft.field.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn comment_bands(&self, ui: crate::theme::UiColors) -> Vec<LineBand> {
        let color = ui.accent.opacity(0.14);
        self.comment_drag
            .map(CommentDrag::lines)
            .into_iter()
            .chain(self.comment_lines())
            .map(|lines| LineBand { lines, color })
            .collect()
    }

    pub(super) fn sync_comment_block(&mut self) {
        let line_count = self.state.document().map_or(0, CodeDocument::line_count);
        if self
            .comment
            .as_ref()
            .is_some_and(|draft| draft.lines.end > line_count || draft.lines.is_empty())
        {
            self.comment = None;
        }
        self.scroll
            .set_block(self.comment.as_ref().map(|draft| RowSpan {
                line: draft.lines.end - 1,
                wrap: 0,
                below: COMMENT_BLOCK_ROWS,
            }));
    }

    pub(super) fn shift_comment(&mut self, changes: &[TrackerWindow]) {
        let Some(draft) = self.comment.as_mut() else {
            return;
        };
        let mut lines = Some(draft.lines.clone());
        for window in changes {
            lines = lines.and_then(|lines| shift_comment_lines(lines, *window));
        }
        match lines {
            Some(lines) => draft.lines = lines,
            None => self.comment = None,
        }
        self.sync_comment_block();
    }

    pub(crate) fn open_comment(
        &mut self,
        lines: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let last = doc.line_count().saturating_sub(1);
        let lines = lines.start.min(last)..lines.end.clamp(lines.start.min(last) + 1, last + 1);
        let occupied = self
            .comment
            .as_ref()
            .is_some_and(|draft| !draft.field.read(cx).is_blank());
        if !occupied {
            self.start_comment(lines, window, cx);
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            REPLACE_PROMPT,
            Some("The text you typed in the open comment is discarded."),
            &[
                gpui::PromptButton::Ok("Replace".into()),
                gpui::PromptButton::Cancel("Keep".into()),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |view, window, cx| {
                view.start_comment(lines, window, cx);
            });
        })
        .detach();
    }

    fn start_comment(&mut self, lines: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| {
            TextArea::new(
                COMMENT_PLACEHOLDER,
                "Comment for the agent",
                COMMENT_FIELD_ROWS,
                cx,
            )
        });
        let events = cx.subscribe_in(
            &field,
            window,
            |view: &mut Self, _, event: &TextAreaEvent, window, cx| match event {
                TextAreaEvent::Changed => cx.notify(),
                TextAreaEvent::Submit => view.request_send_comment(window, cx),
                TextAreaEvent::Cancel => view.cancel_comment(window, cx),
            },
        );
        let focus = field.read(cx).focus_handle(cx);
        self.dismiss_reveal();
        self.comment_drag = None;
        self.gutter_hover = None;
        self.comment = Some(CommentDraft {
            lines,
            field,
            _events: events,
        });
        self.sync_comment_block();
        self.sync_row_map(cx);
        self.reveal_comment_block();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn reveal_comment_block(&mut self) {
        let Some(draft) = self.comment.as_ref() else {
            return;
        };
        let visible = self.scroll.visible_rows();
        if visible <= 0.0 {
            return;
        }
        let (top, bottom) = {
            let map = self.scroll.map();
            (
                map.line_top(draft.lines.end - 1) as f64,
                map.boundary_row(draft.lines.end) as f64,
            )
        };
        let rows = self.scroll.rows();
        if bottom > rows + visible {
            self.scroll.set_rows((bottom - visible).min(top));
        } else if top < rows {
            self.scroll.set_rows(top);
        }
    }

    pub(crate) fn cancel_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment.take().is_some() {
            self.sync_comment_block();
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    pub(crate) fn comment_delivered(&mut self, cx: &mut Context<Self>) {
        if self.comment.take().is_some() {
            self.sync_comment_block();
            cx.notify();
        }
    }

    pub(crate) fn range_comment(&self, cx: &App) -> Option<RangeComment> {
        let draft = self.comment.as_ref()?;
        let doc = self.state.document()?;
        let field = draft.field.read(cx);
        if field.is_blank() {
            return None;
        }
        let start = doc.line_to_byte(draft.lines.start);
        let end = if draft.lines.end >= doc.line_count() {
            doc.len_bytes()
        } else {
            doc.line_to_byte(draft.lines.end)
        };
        Some(RangeComment {
            path: self.path.clone(),
            first_line: draft.lines.start + 1,
            last_line: draft.lines.end,
            text: field.value().to_owned(),
            quoted: doc.slice_string(start..end),
        })
    }

    fn comment_anchor(&self) -> Point<Pixels> {
        let bounds = self.scroll.bounds();
        let top = self.comment_block_top().unwrap_or(0.0)
            + rows_to_px(f64::from(COMMENT_BLOCK_ROWS)) as f32;
        point(
            bounds.origin.x + px(self.geometry.get().gutter_w + 12.0),
            bounds.origin.y + px(top.clamp(0.0, f32::from(bounds.size.height))),
        )
    }

    pub(crate) fn request_send_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(comment) = self.range_comment(cx) else {
            return;
        };
        cx.emit(CodeViewEvent::SendComment {
            comment,
            position: self.comment_anchor(),
            window: window.window_handle(),
        });
    }

    fn comment_block_top(&self) -> Option<f32> {
        let draft = self.comment.as_ref()?;
        let map = self.scroll.map();
        let line = draft.lines.end - 1;
        let row = map.line_top(line) + map.text_rows(line);
        Some(rows_to_px(row as f64 - self.scroll.rows()) as f32)
    }

    fn gutter_line_at(&self, position: Point<Pixels>) -> Option<usize> {
        let bounds = self.scroll.bounds();
        if !bounds.contains(&position) || self.state.document().is_none() {
            return None;
        }
        let x = f32::from(position.x - bounds.origin.x);
        if x >= self.geometry.get().gutter_w {
            return None;
        }
        self.hits
            .borrow()
            .text_line_at(&self.scroll.map(), position)
    }

    fn any_line_at(&self, position: Point<Pixels>) -> Option<usize> {
        self.state.document()?;
        let bounds = self.scroll.bounds();
        let y = position.y.clamp(bounds.origin.y, bounds.bottom() - px(1.));
        self.hits
            .borrow()
            .text_line_at(&self.scroll.map(), point(position.x, y))
    }

    pub(super) fn on_comment_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(drag) = self.comment_drag {
            if ev.pressed_button != Some(MouseButton::Left) {
                self.comment_drag = None;
                cx.notify();
                return;
            }
            let scrolled = self.drag_autoscroll(ev.position);
            let head = self.any_line_at(ev.position).unwrap_or(drag.head);
            if head != drag.head || scrolled {
                self.comment_drag = Some(CommentDrag { head, ..drag });
                cx.notify();
            }
            return;
        }
        if self.text_drag.is_some() || self.navigation.drag.is_some() {
            return;
        }
        match (self.gutter_line_at(ev.position), self.gutter_hover) {
            (None, None) => {}
            (None, Some(_)) => {
                self.gutter_hover = None;
                cx.notify();
            }
            (Some(line), Some(hover)) => {
                if hover.line != line {
                    self.gutter_hover = Some(GutterHover { line, ..hover });
                    if hover.armed {
                        cx.notify();
                    }
                }
            }
            (Some(line), None) => {
                self.hover_generation = self.hover_generation.wrapping_add(1);
                let generation = self.hover_generation;
                self.gutter_hover = Some(GutterHover {
                    line,
                    armed: false,
                    generation,
                });
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor().timer(GUTTER_HOVER_DELAY).await;
                    let _ = this.update(cx, |view, cx| view.arm_gutter_hover(generation, cx));
                })
                .detach();
            }
        }
    }

    pub(super) fn arm_gutter_hover(&mut self, generation: u64, cx: &mut Context<Self>) {
        if let Some(hover) = self.gutter_hover.as_mut()
            && hover.generation == generation
            && !hover.armed
        {
            hover.armed = true;
            cx.notify();
        }
    }

    pub(super) fn leave_gutter(&mut self, cx: &mut Context<Self>) {
        if self.comment_drag.is_none() && self.gutter_hover.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn comment_button_line(&self) -> Option<usize> {
        if let Some(drag) = self.comment_drag {
            return Some(drag.anchor);
        }
        self.gutter_hover
            .filter(|hover| hover.armed)
            .map(|hover| hover.line)
    }

    pub(crate) fn begin_comment_drag(&mut self, line: usize, cx: &mut Context<Self>) {
        self.comment_drag = Some(CommentDrag {
            anchor: line,
            head: line,
        });
        cx.notify();
    }

    pub(super) fn on_comment_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.comment_drag.take() {
            self.open_comment(drag.lines(), window, cx);
            cx.notify();
        }
    }

    pub(crate) fn selection_lines(&self) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        let range = self.selection.range();
        if range.is_empty() {
            return None;
        }
        let first = doc.byte_to_line(range.start);
        let mut last = doc.byte_to_line(range.end);
        if last > first && doc.line_to_byte(last) == range.end {
            last -= 1;
        }
        Some(first..last + 1)
    }

    pub(crate) fn comment_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(lines) = self.selection_lines() {
            self.open_comment(lines, window, cx);
        }
    }

    pub(super) fn comment_at_caret(
        &mut self,
        _: &CeComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let caret = doc.byte_to_line(self.selection.head);
        let lines = self.selection_lines().unwrap_or(caret..caret + 1);
        self.open_comment(lines, window, cx);
    }

    pub(super) fn on_context_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.scroll.bounds().contains(&ev.position) {
            return;
        }
        window.focus(&self.focus, cx);
        self.editor_menu = Some(ev.position);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn render_editor_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let position = self.editor_menu?;
        let handlers = MenuHandlers {
            activate: Rc::new(cx.listener(
                |view: &mut Self, action: &EditorMenuAction, window, cx| {
                    view.editor_menu = None;
                    match action {
                        EditorMenuAction::CommentSelection => view.comment_selection(window, cx),
                        EditorMenuAction::Copy => view.copy(&CeCopy, window, cx),
                        EditorMenuAction::Paste => view.paste_action(&CePaste, window, cx),
                        EditorMenuAction::SelectAll => view.select_all(&CeSelectAll, window, cx),
                    }
                    cx.notify();
                },
            )),
            dismiss: Rc::new(cx.listener(|view: &mut Self, _: &(), _, cx| {
                view.editor_menu = None;
                cx.notify();
            })),
        };
        Some(
            crate::app::sidebar::context_menu::render_anchored_context_menu(
                SharedString::from("code-editor-menu"),
                editor_menu(self.selection_lines().is_some()),
                position,
                handlers,
                window,
                cx,
            ),
        )
    }

    pub(super) fn render_comment_button(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let line = self.comment_button_line()?;
        let top = {
            let map = self.scroll.map();
            rows_to_px(map.line_top(line) as f64 - self.scroll.rows()) as f32
        };
        Some(
            div()
                .id("code-comment-add")
                .debug_selector(|| "code-comment-add".into())
                .absolute()
                .left(px(COMMENT_BUTTON_LEFT))
                .top(px(top + (CODE_ROW_HEIGHT - COMMENT_BUTTON_SIZE) / 2.0))
                .size(px(COMMENT_BUTTON_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(ui.accent)
                .cursor(CursorStyle::PointingHand)
                .role(gpui::accesskit::Role::Button)
                .aria_label(COMMENT_BUTTON_TOOLTIP)
                .delayed_tooltip(crate::ui_primitives::text_tooltip(COMMENT_BUTTON_TOOLTIP))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _: &MouseDownEvent, _window, cx| {
                        view.begin_comment_drag(line, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    gpui::svg()
                        .size(px(10.))
                        .flex_none()
                        .path(crate::app::menu_icons::ADD)
                        .text_color(ui.base),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_comment_block(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.comment.as_ref()?;
        let top = self.comment_block_top()?;
        let height = rows_to_px(f64::from(COMMENT_BLOCK_ROWS)) as f32;
        let viewport_h = self.scroll.viewport_height();
        if top + height <= 0.0 || top >= viewport_h {
            return None;
        }
        let blank = draft.field.read(cx).is_blank();
        let label = range_label(draft.lines.start + 1, draft.lines.end);
        let send = div()
            .id("code-comment-send")
            .debug_selector(|| "code-comment-send".into())
            .role(gpui::accesskit::Role::Button)
            .aria_label(SEND_TO_AGENT)
            .flex_none()
            .h(px(24.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_size(crate::ui_primitives::BODY)
            .bg(ui.accent)
            .text_color(ui.base)
            .when(blank, |button| {
                button
                    .opacity(crate::ui_primitives::action_menu::DISABLED_OPACITY)
                    .cursor(CursorStyle::Arrow)
            })
            .when(!blank, |button| {
                button
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.opacity(0.9))
                    .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                        view.request_send_comment(window, cx);
                    }))
            })
            .child(SEND_TO_AGENT);
        let cancel = div()
            .id("code-comment-cancel")
            .debug_selector(|| "code-comment-cancel".into())
            .role(gpui::accesskit::Role::Button)
            .aria_label(CANCEL_COMMENT)
            .flex_none()
            .h(px(24.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(ui.border)
            .text_size(crate::ui_primitives::BODY)
            .text_color(ui.text)
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(ui.subtle))
            .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                view.cancel_comment(window, cx);
            }))
            .child(CANCEL_COMMENT);
        Some(
            div()
                .id("code-comment-block")
                .debug_selector(|| "code-comment-block".into())
                .absolute()
                .top(px(top))
                .left(px(self.geometry.get().gutter_w))
                .right(px(BLOCK_INSET_R))
                .h(px(height))
                .py(px(6.))
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll(cx.listener(|view, ev: &ScrollEvent, _window, cx| {
                    view.apply_wheel(ev, cx);
                }))
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(ui.border)
                        .bg(ui.overlay)
                        .font_family(crate::ui_primitives::action_menu::FONT_FAMILY)
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::ui_primitives::LABEL_SM)
                                .line_height(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui.muted)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .text_size(crate::ui_primitives::BODY)
                                .line_height(px(CODE_ROW_HEIGHT))
                                .text_color(ui.text)
                                .child(draft.field.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(px(8.))
                                .child(cancel)
                                .child(send),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::super::tests::*;
    use super::*;

    fn window(start: u32, before: u32, after: u32) -> TrackerWindow {
        TrackerWindow {
            start_line: start,
            before_len: before,
            after_len: after,
        }
    }

    #[test]
    fn a_change_above_the_range_moves_it_and_one_below_leaves_it() {
        assert_eq!(shift_comment_lines(10..13, window(2, 0, 3)), Some(13..16));
        assert_eq!(shift_comment_lines(10..13, window(2, 4, 1)), Some(7..10));
        assert_eq!(shift_comment_lines(10..13, window(13, 2, 5)), Some(10..13));
    }

    #[test]
    fn an_edit_inside_the_range_keeps_it_and_deleting_every_line_closes_it() {
        assert_eq!(shift_comment_lines(10..13, window(11, 1, 1)), Some(10..13));
        assert_eq!(shift_comment_lines(10..13, window(11, 1, 3)), Some(10..15));
        assert_eq!(shift_comment_lines(10..13, window(12, 1, 0)), Some(10..12));
        assert_eq!(shift_comment_lines(10..13, window(10, 3, 0)), None);
        assert_eq!(shift_comment_lines(10..13, window(8, 9, 0)), None);
        assert_eq!(shift_comment_lines(10..11, window(10, 1, 1)), Some(10..11));
    }

    #[test]
    fn the_editor_menu_offers_the_comment_only_with_a_selection() {
        let menu = editor_menu(false);
        assert!(!menu.enabled_at_for_test(0));
        let menu = editor_menu(true);
        assert!(menu.enabled_at_for_test(0));
        assert_eq!(menu.action_at(0), Some(EditorMenuAction::CommentSelection));
    }

    fn commenting(
        cx: &mut TestAppContext,
        rows: usize,
    ) -> (gpui::Entity<CodeView>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::widgets::text_input::register_keybindings(cx);
            crate::widgets::text_area::register_keybindings(cx);
            crate::app::diff_dock::code::view::register_keybindings(cx);
        });
        scrolled(cx, "/nonexistent/comment.rs", &rows_of_code(rows))
    }

    fn type_comment(view: &gpui::Entity<CodeView>, cx: &mut VisualTestContext, text: &str) {
        let field = view
            .read_with(cx, |view, _| view.comment_field())
            .expect("draft field");
        field.update(cx, |field, cx| field.set_value(text, cx));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_draft_reserves_rows_below_its_last_line_and_takes_the_focus(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 200);
        let before = view.read_with(cx, |view, _| view.scroll.map().total_rows());
        view.update_in(cx, |view, window, cx| view.open_comment(3..6, window, cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let map = view.scroll.map();
            assert_eq!(view.comment_lines(), Some(3..6));
            assert_eq!(map.total_rows(), before + COMMENT_BLOCK_ROWS as usize);
            assert_eq!(map.line_top(5), 5);
            assert_eq!(map.line_top(6), 6 + COMMENT_BLOCK_ROWS as usize);
        });
        assert!(cx.update(|window, cx| view.read(cx).comment_focused(window, cx)));
        assert!(cx.debug_bounds("code-comment-block").is_some());
        let block = cx.debug_bounds("code-comment-block").expect("block");
        assert_eq!(
            f32::from(block.size.height),
            rows_to_px(f64::from(COMMENT_BLOCK_ROWS)) as f32
        );
    }

    #[gpui::test]
    fn scrolling_past_a_draft_stays_continuous(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 200);
        view.update_in(cx, |view, window, cx| view.open_comment(3..4, window, cx));
        cx.run_until_parked();
        let top_before = cx.debug_bounds("code-comment-block").expect("block").top();
        view.update(cx, |view, cx| {
            view.scroll.scroll_by_pixels(CODE_ROW_HEIGHT * 2.0);
            cx.notify();
        });
        cx.run_until_parked();
        let top_after = cx.debug_bounds("code-comment-block").expect("block").top();
        assert_eq!(f32::from(top_before - top_after), CODE_ROW_HEIGHT * 2.0);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.row_top(4) - view.row_top(3),
                CODE_ROW_HEIGHT * (1 + COMMENT_BLOCK_ROWS) as f32
            );
        });
    }

    #[gpui::test]
    fn escape_and_cancel_close_the_draft_and_return_the_focus(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 50);
        let rows = view.read_with(cx, |view, _| view.scroll.map().total_rows());
        view.update_in(cx, |view, window, cx| view.open_comment(2..3, window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.comment_lines()), None);
        assert!(cx.update(|window, cx| view.read(cx).focus.is_focused(window)));
        assert_eq!(
            view.read_with(cx, |view, _| view.scroll.map().total_rows()),
            rows
        );

        view.update_in(cx, |view, window, cx| view.open_comment(2..3, window, cx));
        cx.run_until_parked();
        let cancel = cx.debug_bounds("code-comment-cancel").expect("cancel");
        cx.simulate_click(cancel.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.comment_lines()), None);
        assert!(cx.update(|window, cx| view.read(cx).focus.is_focused(window)));
    }

    #[gpui::test]
    fn typing_in_the_draft_never_edits_the_file(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 20);
        let before = view.read_with(cx, |view, _| text_of(view));
        view.update_in(cx, |view, window, cx| view.open_comment(1..2, window, cx));
        cx.run_until_parked();
        cx.simulate_input("rename");
        cx.simulate_keystrokes("enter tab backspace");
        cx.simulate_input("this");
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| text_of(view)), before);
        let typed = view
            .read_with(cx, |view, _| view.comment_field())
            .expect("field")
            .read_with(cx, |field, _| field.value().to_owned());
        assert_eq!(typed, "rename\nthis");
    }

    #[gpui::test]
    fn the_draft_follows_its_lines_and_closes_when_they_are_all_deleted(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 20);
        let rows = view.read_with(cx, |view, _| view.scroll.map().total_rows());
        view.update_in(cx, |view, window, cx| view.open_comment(5..7, window, cx));
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 0 };
            view.replace_text_in_range(None, "one\ntwo\n", window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(7..9)
        );
        view.update_in(cx, |view, window, cx| {
            let doc = view.document().expect("doc");
            let range = doc.line_to_byte(6)..doc.line_to_byte(10);
            view.selection = CodeSelection {
                anchor: range.start,
                head: range.end,
            };
            view.replace_text_in_range(None, "", window, cx);
        });
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.comment_lines()), None);
        assert_eq!(
            view.read_with(cx, |view, _| view.scroll.map().total_rows()),
            rows + 2 - 4
        );
    }

    #[gpui::test]
    fn the_gutter_button_appears_after_the_hover_delay_and_follows_the_line(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = commenting(cx, 50);
        let origin = view.read_with(cx, |view, _| view.scroll.bounds().origin);
        let gutter = |row: f32| {
            point(
                origin.x + px(20.),
                origin.y + px(CODE_ROW_HEIGHT * row + 9.),
            )
        };
        cx.simulate_mouse_move(gutter(2.), None, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_button_line()),
            None
        );
        assert!(cx.debug_bounds("code-comment-add").is_none());
        cx.executor().advance_clock(GUTTER_HOVER_DELAY);
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_button_line()),
            Some(2)
        );
        assert!(cx.debug_bounds("code-comment-add").is_some());
        cx.simulate_mouse_move(gutter(4.), None, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_button_line()),
            Some(4)
        );
        cx.simulate_mouse_move(
            point(origin.x + px(300.), origin.y + px(40.)),
            None,
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_button_line()),
            None
        );
    }

    #[gpui::test]
    fn a_click_on_the_button_comments_one_line_and_a_drag_comments_the_range(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = commenting(cx, 50);
        let origin = view.read_with(cx, |view, _| view.scroll.bounds().origin);
        let gutter = |row: f32| {
            point(
                origin.x + px(20.),
                origin.y + px(CODE_ROW_HEIGHT * row + 9.),
            )
        };
        cx.simulate_mouse_move(gutter(2.), None, Modifiers::default());
        cx.executor().advance_clock(GUTTER_HOVER_DELAY);
        cx.run_until_parked();
        let button = cx
            .debug_bounds("code-comment-add")
            .expect("button")
            .center();
        cx.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(2..3)
        );

        view.update_in(cx, |view, window, cx| view.cancel_comment(window, cx));
        cx.run_until_parked();
        cx.simulate_mouse_move(gutter(1.), None, Modifiers::default());
        cx.executor().advance_clock(GUTTER_HOVER_DELAY);
        cx.run_until_parked();
        let button = cx
            .debug_bounds("code-comment-add")
            .expect("button")
            .center();
        cx.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(gutter(4.), Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(gutter(4.), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(1..5)
        );
        let block = cx.debug_bounds("code-comment-block").expect("block");
        assert_eq!(
            f32::from(block.top() - origin.y),
            CODE_ROW_HEIGHT * 5.0,
            "the draft opens under the last line of the range"
        );
    }

    #[gpui::test]
    fn comment_selection_covers_the_selected_lines(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 30);
        view.update_in(cx, |view, window, cx| {
            let doc = view.document().expect("doc");
            view.selection = CodeSelection {
                anchor: doc.line_to_byte(11) + 2,
                head: doc.line_to_byte(15),
            };
            assert_eq!(view.selection_lines(), Some(11..15));
            view.comment_selection(window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(11..15)
        );
        view.update(cx, |view, _| {
            view.selection = CodeSelection { anchor: 4, head: 4 };
            assert_eq!(view.selection_lines(), None);
        });
    }

    #[gpui::test]
    fn the_comment_keystroke_comments_the_selection_or_the_caret_line(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 30);
        view.update_in(cx, |view, window, cx| {
            let at = view.document().expect("doc").line_to_byte(6) + 1;
            view.selection = CodeSelection {
                anchor: at,
                head: at,
            };
            window.focus(&view.focus, cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(COMMENT_KEYSTROKE);
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(6..7)
        );
    }

    #[gpui::test]
    fn sending_reports_the_range_the_text_and_the_quoted_lines(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 30);
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &CodeViewEvent, _| {
                if let CodeViewEvent::SendComment { comment, .. } = event {
                    sink.borrow_mut().push(comment.clone());
                }
            })
        });
        view.update_in(cx, |view, window, cx| view.open_comment(11..15, window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes(crate::widgets::text_area::SUBMIT_KEYSTROKE);
        cx.run_until_parked();
        assert!(events.borrow().is_empty(), "an empty draft sends nothing");
        type_comment(&view, cx, "   \n ");
        let send = cx.debug_bounds("code-comment-send").expect("send").center();
        cx.simulate_click(send, Modifiers::default());
        cx.run_until_parked();
        assert!(events.borrow().is_empty(), "a blank draft sends nothing");

        type_comment(&view, cx, "merge these");
        cx.simulate_keystrokes(crate::widgets::text_area::SUBMIT_KEYSTROKE);
        cx.run_until_parked();
        let send = cx.debug_bounds("code-comment-send").expect("send").center();
        cx.simulate_click(send, Modifiers::default());
        cx.run_until_parked();
        let sent = events.borrow().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
        assert_eq!(sent[0].first_line, 12);
        assert_eq!(sent[0].last_line, 15);
        assert_eq!(sent[0].text, "merge these");
        assert_eq!(
            sent[0].quoted,
            "fn f11() {}\nfn f12() {}\nfn f13() {}\nfn f14() {}\n"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(11..15),
            "the draft stays open until the terminal accepted the text"
        );
        view.update(cx, |view, cx| view.comment_delivered(cx));
        assert_eq!(view.read_with(cx, |view, _| view.comment_lines()), None);
    }

    #[gpui::test]
    fn a_second_draft_replaces_an_empty_one_and_asks_before_dropping_text(cx: &mut TestAppContext) {
        let (view, cx) = commenting(cx, 30);
        view.update_in(cx, |view, window, cx| view.open_comment(2..3, window, cx));
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| view.open_comment(8..9, window, cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(8..9)
        );
        assert!(!cx.has_pending_prompt());

        type_comment(&view, cx, "keep me");
        view.update_in(cx, |view, window, cx| view.open_comment(12..13, window, cx));
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Keep");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(8..9)
        );

        view.update_in(cx, |view, window, cx| view.open_comment(12..13, window, cx));
        cx.run_until_parked();
        cx.simulate_prompt_answer("Replace");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.comment_lines()),
            Some(12..13)
        );
        let text = view
            .read_with(cx, |view, _| view.comment_field())
            .expect("field")
            .read_with(cx, |field, _| field.value().to_owned());
        assert_eq!(text, "");
    }
}

use super::*;

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

impl CodeView {
    pub(super) fn start_base_load(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn reset_tracker(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn note_changes(&mut self, changes: &[TrackerWindow], cx: &mut Context<Self>) {
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
        super::spawn_blocking_then(
            cx,
            move || {
                let mut tracker = tracker;
                let text = rope.to_string();
                let doc_lines = split_lines(&text);
                let base_lines = split_lines(&base);
                tracker.refresh_dirty(&doc_lines, &base_lines, TRACKER_POLICY);
                tracker
            },
            move |view: &mut Self, tracker, cx| {
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
            },
        );
    }

    pub(crate) fn marker_blocks(&self) -> &[Block] {
        self.tracker.blocks()
    }

    pub(crate) fn hovered_marker(&self) -> Option<usize> {
        self.hovered_marker
            .filter(|index| *index < self.tracker.blocks().len())
    }

    pub(super) fn on_marker_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.text_drag.is_some() || self.navigation.drag.is_some() {
            return;
        }
        let hit = self.hits.borrow().marker_at(ev.position);
        if hit != self.hovered_marker {
            self.hovered_marker = hit;
            cx.notify();
        }
    }

    pub(super) fn on_marker_down(
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

    pub(super) fn close_marker_popup(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn render_marker_popup(
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
                let runs = crate::diff::text_runs(text, syntax, &font, ui.text);
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
                    .child(crate::ui_primitives::menu_reveal(
                        "code-marker-popup-reveal",
                        panel,
                    )),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
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

pub(crate) fn popup_title(block: &Block) -> String {
    match block.kind() {
        BlockKind::Added => format!(
            "Added {}",
            crate::app::plural(block.lines.len(), "line", "lines")
        ),
        BlockKind::Deleted => {
            let lines = crate::app::plural(block.base_lines.len(), "line", "lines");
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

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};

    use super::super::tests::*;
    use super::*;

    use crate::app::diff_dock::code::load::build_document;

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

    #[gpui::test]
    fn a_git_base_from_an_older_load_cannot_replace_the_current_base(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "current\n");
        view.update(cx, |view, cx| {
            view.path = PathBuf::from("relative-only.rs");
            view.base = Base::Untracked;
            view.start_base_load(cx);
            view.slot.begin();
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert_eq!(view.base, Base::Untracked);
            view.start_base_load(cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, _cx| assert_eq!(view.base, Base::None));
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

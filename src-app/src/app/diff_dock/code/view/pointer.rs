use super::*;

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

impl CodeView {
    pub(super) fn apply_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
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

    pub(crate) fn on_scrollbar_down(
        &mut self,
        ev: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
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

    pub(crate) fn on_scrollbar_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
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

    pub(crate) fn on_scrollbar_up(&mut self, _ev: &MouseUpEvent, cx: &mut Context<Self>) {
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

    pub(super) fn on_text_down(
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

    pub(super) fn on_text_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
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

    pub(super) fn on_text_up(&mut self, _ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.text_drag.take().is_some() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, TestAppContext, TouchPhase, VisualTestContext, point};

    use super::super::tests::*;
    use super::*;

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
}

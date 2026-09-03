use super::*;
use std::rc::Rc;

impl DiffView {
    pub(super) fn toggle_sync(&mut self, cx: &mut Context<Self>) {
        self.sync_scroll = !self.sync_scroll;
        cx.notify();
    }

    pub fn jump_to_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.effective_mode(window);
        let target = self
            .columns
            .get(self.selected_column)
            .filter(|c| c.visible)
            .and_then(|col| {
                let anchors = match mode {
                    ViewMode::Unified => &col.disp_anchors_unified,
                    ViewMode::Split => &col.disp_anchors_split,
                };
                let idx = anchors.iter().find(|(p, _)| p == path).map(|(_, i)| *i)?;
                let offsets = match mode {
                    ViewMode::Unified => &col.disp_unified_offsets,
                    ViewMode::Split => &col.disp_split_offsets,
                };
                let y = hit_test::row_top(offsets, idx);
                Some((col.el_scroll.clone(), y))
            });
        let Some((handle, y)) = target else {
            return;
        };
        let x = handle.offset().x;
        handle.set_offset(point(x, px(-y)));
        self.scroll_driver = self.selected_column;
        cx.notify();
    }
    pub(super) fn broadcast_scroll(&self, mode: ViewMode) {
        if !self.sync_scroll {
            return;
        }
        let driver = if self
            .columns
            .get(self.scroll_driver)
            .map(|c| c.visible)
            .unwrap_or(false)
        {
            self.scroll_driver
        } else {
            match self.columns.iter().position(|c| c.visible) {
                Some(i) => i,
                None => return,
            }
        };
        let Some(driver_col) = self.columns.get(driver) else {
            return;
        };
        let driver_y = f32::from(-driver_col.el_scroll.offset().y).max(0.0);
        let (top_file, intra) = self.file_at_offset(driver_col, mode, driver_y);
        for (i, col) in self.columns.iter().enumerate() {
            if i == driver || !col.visible {
                continue;
            }
            let target_y = match &top_file {
                Some(path) => self
                    .file_top_offset(col, mode, path)
                    .map(|fy| fy + intra)
                    .unwrap_or(driver_y),
                None => driver_y,
            };
            let cur = col.el_scroll.offset();
            if f32::from(-cur.y) != target_y {
                col.el_scroll.set_offset(point(cur.x, px(-target_y)));
            }
        }
    }

    pub(super) fn file_at_offset(
        &self,
        col: &Column,
        mode: ViewMode,
        y: f32,
    ) -> (Option<String>, f32) {
        let (offsets, anchors) = match mode {
            ViewMode::Unified => (&col.disp_unified_offsets, &col.disp_anchors_unified),
            ViewMode::Split => (&col.disp_split_offsets, &col.disp_anchors_split),
        };
        let row = offsets.partition_point(|&o| o <= y).saturating_sub(1);
        match anchors
            .partition_point(|(_, ri)| *ri <= row)
            .checked_sub(1)
            .and_then(|i| anchors.get(i))
        {
            Some((path, anchor_row)) => {
                let top = offsets.get(*anchor_row).copied().unwrap_or(0.0);
                (Some(path.clone()), (y - top).max(0.0))
            }
            None => (None, y),
        }
    }

    pub(super) fn file_top_offset(&self, col: &Column, mode: ViewMode, path: &str) -> Option<f32> {
        let (offsets, anchors) = match mode {
            ViewMode::Unified => (&col.disp_unified_offsets, &col.disp_anchors_unified),
            ViewMode::Split => (&col.disp_split_offsets, &col.disp_anchors_split),
        };
        let idx = anchors.iter().find(|(p, _)| p == path).map(|(_, i)| *i)?;
        offsets.get(idx).copied()
    }

    fn scrollbar_segments_for_column(
        &self,
        col_idx: usize,
        mode: ViewMode,
    ) -> Option<Vec<HScrollbarSegment>> {
        let col = self.columns.get(col_idx)?;
        let bounds = col.el_scroll.bounds();
        let viewport_h = f32::from(bounds.size.height);
        let panel_width = f32::from(bounds.size.width);
        if viewport_h <= 0.0 || panel_width <= 0.0 {
            return None;
        }

        let visible_top = f32::from(-col.el_scroll.offset().y).max(0.0);
        let visible_bottom = visible_top + viewport_h;
        let (offsets, spans) = match mode {
            ViewMode::Unified => (&col.disp_unified_offsets, &col.disp_unified_spans),
            ViewMode::Split => (&col.disp_split_offsets, &col.disp_split_spans),
        };
        Some(super::super::hscroll::h_scrollbar_segments(
            spans,
            offsets,
            &col.h_offsets,
            mode == ViewMode::Split,
            panel_width,
            visible_top,
            visible_bottom,
        ))
    }

    fn h_scrollbar_local_point(&self, col_idx: usize, point: Point<Pixels>) -> Option<(f32, f32)> {
        let col = self.columns.get(col_idx)?;
        let bounds = col.el_scroll.bounds();
        if point.x < bounds.left()
            || point.x > bounds.right()
            || point.y < bounds.top()
            || point.y > bounds.bottom()
        {
            return None;
        }
        Some((
            f32::from(point.x - bounds.left()),
            f32::from(point.y - bounds.top() - col.el_scroll.offset().y).max(0.0),
        ))
    }

    pub(super) fn handle_horizontal_scrollbar_mouse_down(
        &mut self,
        col_idx: usize,
        point: Point<Pixels>,
        mode: ViewMode,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((x, y)) = self.h_scrollbar_local_point(col_idx, point) else {
            return false;
        };
        let Some(segments) = self.scrollbar_segments_for_column(col_idx, mode) else {
            return false;
        };
        let Some(segment) = segments.iter().find(|segment| {
            x >= segment.x
                && x <= segment.x + segment.width
                && y >= segment.y
                && y <= segment.y + super::super::hscroll::H_SCROLLBAR_TRACK_HEIGHT
        }) else {
            return false;
        };

        let thumb_left = segment.x + segment.thumb_x;
        let thumb_right = thumb_left + segment.thumb_width;
        let target = if x >= thumb_left && x <= thumb_right {
            segment.offset
        } else {
            super::super::hscroll::h_scrollbar_click_offset(&segments, x, y)
                .map(|(_, offset)| offset)
                .unwrap_or(segment.offset)
        };

        if let Some(col) = self.columns.get_mut(col_idx) {
            let offsets = Rc::make_mut(&mut col.h_offsets);
            if offsets.len() <= segment.offset_idx {
                offsets.resize(segment.offset_idx + 1, 0.0);
            }
            offsets[segment.offset_idx] = target.clamp(0.0, segment.max_scroll);
        }

        self.h_scroll_drag = Some(DiffHScrollDrag {
            col_idx,
            offset_idx: segment.offset_idx,
            start_mouse_x: point.x,
            start_offset: target.clamp(0.0, segment.max_scroll),
            max_scroll: segment.max_scroll,
            track_width: segment.width,
            thumb_width: segment.thumb_width,
        });
        cx.notify();
        true
    }

    pub(super) fn handle_horizontal_scrollbar_click(
        &mut self,
        col_idx: usize,
        point: Point<Pixels>,
        mode: ViewMode,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((x, y)) = self.h_scrollbar_local_point(col_idx, point) else {
            return false;
        };
        let Some(segments) = self.scrollbar_segments_for_column(col_idx, mode) else {
            return false;
        };
        let Some(segment) = segments.iter().find(|segment| {
            x >= segment.x
                && x <= segment.x + segment.width
                && y >= segment.y
                && y <= segment.y + super::super::hscroll::H_SCROLLBAR_TRACK_HEIGHT
        }) else {
            return false;
        };

        let thumb_left = segment.x + segment.thumb_x;
        let thumb_right = thumb_left + segment.thumb_width;
        if x >= thumb_left && x <= thumb_right {
            return true;
        }

        let target = super::super::hscroll::h_scrollbar_click_offset(&segments, x, y)
            .map(|(_, offset)| offset)
            .unwrap_or(segment.offset);
        if let Some(col) = self.columns.get_mut(col_idx) {
            let offsets = Rc::make_mut(&mut col.h_offsets);
            if offsets.len() <= segment.offset_idx {
                offsets.resize(segment.offset_idx + 1, 0.0);
            }
            offsets[segment.offset_idx] = target;
        }
        cx.notify();
        true
    }

    pub(super) fn drag_horizontal_scrollbar(&mut self, mouse_x: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.h_scroll_drag else {
            return;
        };
        let Some(col) = self.columns.get_mut(drag.col_idx) else {
            self.h_scroll_drag = None;
            cx.notify();
            return;
        };

        let track_range = (drag.track_width - drag.thumb_width).max(1.0);
        let delta = f32::from(mouse_x - drag.start_mouse_x);
        let next =
            (drag.start_offset + delta * drag.max_scroll / track_range).clamp(0.0, drag.max_scroll);
        let offsets = Rc::make_mut(&mut col.h_offsets);
        if offsets.len() <= drag.offset_idx {
            offsets.resize(drag.offset_idx + 1, 0.0);
        }
        if (offsets[drag.offset_idx] - next).abs() > 0.1 {
            offsets[drag.offset_idx] = next;
            cx.notify();
        }
    }

    pub(super) fn end_horizontal_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        if self.h_scroll_drag.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn apply_horizontal_wheel(
        &mut self,
        col_idx: usize,
        ev: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = ev.delta.pixel_delta(window.line_height());
        let dx = f32::from(delta.x);
        if dx == 0.0 {
            return;
        }

        let mode = self.effective_mode(window);
        let Some(col) = self.columns.get_mut(col_idx) else {
            return;
        };
        let bounds = col.el_scroll.bounds();
        if ev.position.x < bounds.left()
            || ev.position.x > bounds.right()
            || ev.position.y < bounds.top()
            || ev.position.y > bounds.bottom()
        {
            return;
        }

        let content_y = f32::from(ev.position.y - bounds.top() - col.el_scroll.offset().y).max(0.0);
        let (offsets, spans) = match mode {
            ViewMode::Unified => (&col.disp_unified_offsets, &col.disp_unified_spans),
            ViewMode::Split => (&col.disp_split_offsets, &col.disp_split_spans),
        };
        let Some(row) = hit_test::row_at_offset(offsets, content_y) else {
            return;
        };
        let Some(file_idx) = super::super::hscroll::file_at_row(spans, row) else {
            return;
        };

        let panel_width = f32::from(bounds.size.width);
        let split = mode == ViewMode::Split;
        let local_x = f32::from(ev.position.x - bounds.left());
        let right = split && super::super::hscroll::split_right_side_at_x(local_x, panel_width);
        let offset_idx = super::super::hscroll::h_offset_index(spans.len(), file_idx, split, right);
        let current = col.h_offsets.get(offset_idx).copied().unwrap_or(0.0);
        super::super::hscroll::set_file_side_offset(
            Rc::make_mut(&mut col.h_offsets),
            spans,
            file_idx,
            right,
            current - dx,
            split,
            panel_width,
        );
        cx.notify();
    }
}

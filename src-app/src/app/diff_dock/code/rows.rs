use std::ops::Range;

pub(crate) const CODE_ROW_HEIGHT: f32 = 18.0;
pub(crate) const OVERDRAW_ROWS: usize = 1;

pub(crate) fn rows_to_px(rows: f64) -> f64 {
    rows * f64::from(CODE_ROW_HEIGHT)
}

pub(crate) fn px_to_rows(px: f64) -> f64 {
    px / f64::from(CODE_ROW_HEIGHT)
}

pub(crate) fn rows_in(viewport_h: f32) -> f64 {
    if viewport_h <= 0.0 {
        return 0.0;
    }
    px_to_rows(f64::from(viewport_h))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowSpan {
    pub(crate) line: usize,
    pub(crate) wrap: u32,
    pub(crate) below: u32,
}

impl RowSpan {
    fn extra(self) -> u64 {
        u64::from(self.wrap) + u64::from(self.below)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowSlot {
    Text { line: usize, segment: usize },
    Block { line: usize, row: usize },
}

impl RowSlot {
    #[cfg(test)]
    pub(crate) fn line(self) -> usize {
        match self {
            Self::Text { line, .. } | Self::Block { line, .. } => line,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RowMap {
    line_count: usize,
    spans: Vec<RowSpan>,
    extra_before: Vec<u64>,
}

impl RowMap {
    pub(crate) fn uniform(line_count: usize) -> Self {
        Self {
            line_count,
            spans: Vec::new(),
            extra_before: vec![0],
        }
    }

    pub(crate) fn from_spans(line_count: usize, spans: impl IntoIterator<Item = RowSpan>) -> Self {
        let mut merged: Vec<RowSpan> = Vec::new();
        let mut sorted: Vec<RowSpan> = spans
            .into_iter()
            .filter(|span| span.line < line_count && span.extra() > 0)
            .collect();
        sorted.sort_by_key(|span| span.line);
        for span in sorted {
            match merged.last_mut() {
                Some(last) if last.line == span.line => {
                    last.wrap = last.wrap.saturating_add(span.wrap);
                    last.below = last.below.saturating_add(span.below);
                }
                _ => merged.push(span),
            }
        }
        let mut map = Self {
            line_count,
            spans: merged,
            extra_before: Vec::new(),
        };
        map.rebuild_prefix();
        map
    }

    fn rebuild_prefix(&mut self) {
        self.extra_before.clear();
        self.extra_before.reserve(self.spans.len() + 1);
        let mut total = 0u64;
        self.extra_before.push(0);
        for span in &self.spans {
            total += span.extra();
            self.extra_before.push(total);
        }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.line_count
    }

    pub(crate) fn is_uniform(&self) -> bool {
        self.spans.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn spans(&self) -> &[RowSpan] {
        &self.spans
    }

    pub(crate) fn set_line_count(&mut self, line_count: usize) {
        if line_count == self.line_count {
            return;
        }
        self.line_count = line_count;
        if self
            .spans
            .last()
            .is_some_and(|span| span.line >= line_count)
        {
            self.spans.retain(|span| span.line < line_count);
            self.rebuild_prefix();
        }
    }

    fn extra_total(&self) -> u64 {
        self.extra_before.last().copied().unwrap_or(0)
    }

    pub(crate) fn total_rows(&self) -> usize {
        self.line_count + self.extra_total() as usize
    }

    fn span_of(&self, line: usize) -> Option<RowSpan> {
        if self.spans.is_empty() {
            return None;
        }
        self.spans
            .binary_search_by_key(&line, |span| span.line)
            .ok()
            .map(|index| self.spans[index])
    }

    fn extra_before_line(&self, line: usize) -> u64 {
        if self.spans.is_empty() {
            return 0;
        }
        let index = self.spans.partition_point(|span| span.line < line);
        self.extra_before[index]
    }

    pub(crate) fn boundary_row(&self, line: usize) -> usize {
        if line >= self.line_count {
            return self.total_rows();
        }
        line + self.extra_before_line(line) as usize
    }

    pub(crate) fn line_top(&self, line: usize) -> usize {
        self.boundary_row(line.min(self.line_count.saturating_sub(1)))
    }

    pub(crate) fn text_rows(&self, line: usize) -> usize {
        1 + self.span_of(line).map_or(0, |span| span.wrap as usize)
    }

    pub(crate) fn line_rows(&self, line: usize) -> usize {
        self.text_rows(line) + self.span_of(line).map_or(0, |span| span.below as usize)
    }

    pub(crate) fn line_at(&self, row: usize) -> usize {
        let last = self.line_count.saturating_sub(1);
        if self.spans.is_empty() {
            return row.min(last);
        }
        let (mut lo, mut hi) = (0usize, self.spans.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.span_top(mid) <= row {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let line = match lo.checked_sub(1) {
            None => row,
            Some(index) => {
                let span = self.spans[index];
                let top = self.span_top(index);
                let end = top + 1 + span.extra() as usize;
                if row < end {
                    span.line
                } else {
                    span.line + 1 + (row - end)
                }
            }
        };
        line.min(last)
    }

    fn span_top(&self, index: usize) -> usize {
        self.spans[index].line + self.extra_before[index] as usize
    }

    pub(crate) fn slot_at(&self, row: usize) -> RowSlot {
        let line = self.line_at(row);
        let offset = row.saturating_sub(self.line_top(line));
        let text = self.text_rows(line);
        if offset < text {
            RowSlot::Text {
                line,
                segment: offset,
            }
        } else {
            RowSlot::Block {
                line,
                row: (offset - text).min(self.line_rows(line) - text),
            }
        }
    }

    pub(crate) fn visible_rows(&self, scroll_rows: f64, viewport_h: f32) -> Range<usize> {
        let total = self.total_rows();
        if total == 0 || viewport_h <= 0.0 {
            return 0..0;
        }
        let top = scroll_rows.max(0.0);
        let first = (top as usize).saturating_sub(OVERDRAW_ROWS);
        let bottom = top + rows_in(viewport_h);
        let last = bottom as usize + 1 + OVERDRAW_ROWS;
        first.min(total)..last.min(total)
    }

    pub(crate) fn lines_in(&self, rows: Range<usize>) -> Range<usize> {
        if rows.is_empty() || self.line_count == 0 {
            return 0..0;
        }
        self.line_at(rows.start)..self.line_at(rows.end - 1) + 1
    }

    pub(crate) fn line_position(&self, rows: f64) -> f64 {
        if self.line_count == 0 {
            return 0.0;
        }
        let rows = if rows.is_finite() { rows.max(0.0) } else { 0.0 };
        if rows >= self.total_rows() as f64 {
            return self.line_count as f64;
        }
        if self.spans.is_empty() {
            return rows;
        }
        let line = self.line_at(rows as usize);
        let within = (rows - self.line_top(line) as f64) / self.line_rows(line) as f64;
        line as f64 + within.clamp(0.0, 1.0)
    }

    pub(crate) fn rows_at_line_position(&self, position: f64) -> f64 {
        let position = if position.is_finite() {
            position.max(0.0)
        } else {
            0.0
        };
        if position >= self.line_count as f64 {
            return self.total_rows() as f64;
        }
        if self.spans.is_empty() {
            return position;
        }
        let line = position as usize;
        let fraction = position - line as f64;
        self.line_top(line) as f64 + fraction * self.line_rows(line) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped() -> RowMap {
        RowMap::from_spans(
            10,
            [
                RowSpan {
                    line: 2,
                    wrap: 2,
                    below: 0,
                },
                RowSpan {
                    line: 5,
                    wrap: 0,
                    below: 3,
                },
                RowSpan {
                    line: 7,
                    wrap: 1,
                    below: 1,
                },
            ],
        )
    }

    #[test]
    fn a_uniform_map_is_the_identity() {
        let map = RowMap::uniform(1_000);
        assert!(map.is_uniform());
        assert_eq!(map.total_rows(), 1_000);
        for line in [0usize, 1, 499, 999] {
            assert_eq!(map.line_top(line), line);
            assert_eq!(map.line_at(line), line);
            assert_eq!(
                rows_to_px(map.line_top(line) as f64),
                f64::from(line as u32) * 18.0
            );
            assert_eq!(map.slot_at(line), RowSlot::Text { line, segment: 0 });
        }
        assert_eq!(map.line_position(42.5), 42.5);
        assert_eq!(map.rows_at_line_position(42.5), 42.5);
    }

    #[test]
    fn wrapped_lines_and_blocks_push_the_lines_below_them_down() {
        let map = wrapped();
        assert_eq!(map.total_rows(), 10 + 2 + 3 + 2);
        assert_eq!(map.line_top(2), 2);
        assert_eq!(map.text_rows(2), 3);
        assert_eq!(map.line_top(3), 5);
        assert_eq!(map.line_top(5), 7);
        assert_eq!(map.line_rows(5), 4);
        assert_eq!(map.line_top(6), 11);
        assert_eq!(map.line_top(7), 12);
        assert_eq!(map.line_top(8), 15);
        assert_eq!(map.line_top(9), 16);
        assert_eq!(map.boundary_row(10), 17);
    }

    #[test]
    fn every_row_maps_back_to_the_line_that_owns_it() {
        let map = wrapped();
        let owners: Vec<usize> = (0..map.total_rows()).map(|row| map.line_at(row)).collect();
        assert_eq!(
            owners,
            vec![0, 1, 2, 2, 2, 3, 4, 5, 5, 5, 5, 6, 7, 7, 7, 8, 9]
        );
        assert_eq!(
            map.slot_at(3),
            RowSlot::Text {
                line: 2,
                segment: 1
            }
        );
        assert_eq!(map.slot_at(8), RowSlot::Block { line: 5, row: 0 });
        assert_eq!(map.slot_at(14), RowSlot::Block { line: 7, row: 0 });
        for line in 0..10 {
            assert_eq!(map.line_at(map.line_top(line)), line);
        }
    }

    #[test]
    fn out_of_bounds_queries_land_on_the_first_or_last_line() {
        for map in [RowMap::uniform(0), RowMap::uniform(1), wrapped()] {
            let last = map.line_count().saturating_sub(1);
            assert_eq!(map.line_at(usize::MAX), last);
            assert_eq!(map.line_at(0), 0);
            assert_eq!(map.line_top(usize::MAX), map.line_top(last));
            assert_eq!(map.slot_at(usize::MAX).line(), last);
            assert_eq!(map.line_position(f64::NAN), 0.0);
            assert_eq!(map.line_position(-4.0), 0.0);
            assert_eq!(map.line_position(1e12), map.line_count() as f64);
            assert_eq!(map.rows_at_line_position(-1.0), 0.0);
            assert_eq!(map.rows_at_line_position(1e12), map.total_rows() as f64);
        }
        assert_eq!(RowMap::uniform(0).visible_rows(0.0, 400.0), 0..0);
        assert_eq!(RowMap::uniform(0).lines_in(0..4), 0..0);
        assert_eq!(RowMap::uniform(1).visible_rows(50.0, 400.0), 1..1);
    }

    #[test]
    fn a_line_position_round_trips_through_rows() {
        let map = wrapped();
        for position in [0.0, 2.0, 2.5, 5.25, 7.75, 9.0] {
            let rows = map.rows_at_line_position(position);
            assert!(
                (map.line_position(rows) - position).abs() < 1e-9,
                "{position}"
            );
        }
    }

    #[test]
    fn visible_rows_are_bounded_by_the_viewport_not_the_map() {
        let map = RowMap::from_spans(
            100_000,
            (0..100_000).step_by(3).map(|line| RowSpan {
                line,
                wrap: 4,
                below: 0,
            }),
        );
        let bound = (720.0 / CODE_ROW_HEIGHT) as usize + 1 + 2 * OVERDRAW_ROWS;
        let mut top = 0.0;
        while top < map.total_rows() as f64 {
            assert!(map.visible_rows(top, 720.0).len() <= bound);
            top += 997.3;
        }
        let lines = map.lines_in(map.visible_rows(5.0, 720.0));
        assert_eq!(lines.start, map.line_at(4));
    }

    #[test]
    fn dropping_lines_drops_the_spans_past_the_new_end() {
        let mut map = wrapped();
        map.set_line_count(6);
        assert_eq!(map.total_rows(), 6 + 2 + 3);
        assert_eq!(map.line_at(usize::MAX), 5);
        map.set_line_count(3);
        assert_eq!(map.total_rows(), 5);
    }

    #[test]
    fn duplicate_and_empty_spans_fold_into_one_entry() {
        let map = RowMap::from_spans(
            4,
            [
                RowSpan {
                    line: 1,
                    wrap: 1,
                    below: 0,
                },
                RowSpan {
                    line: 1,
                    wrap: 0,
                    below: 2,
                },
                RowSpan {
                    line: 2,
                    wrap: 0,
                    below: 0,
                },
                RowSpan {
                    line: 9,
                    wrap: 5,
                    below: 0,
                },
            ],
        );
        assert_eq!(
            map.spans(),
            &[RowSpan {
                line: 1,
                wrap: 1,
                below: 2
            }]
        );
        assert_eq!(map.total_rows(), 7);
    }
}

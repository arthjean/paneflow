use crate::iterable::{Changes, Range};
use crate::text::{expand_backward, expand_forward};

pub(crate) trait BoundaryShift {
    fn shift(
        &self,
        touch_left: bool,
        equal_forward: usize,
        equal_backward: usize,
        range1: Range,
        range2: Range,
    ) -> isize;
}

pub(crate) fn optimize_chunks<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    iterable: &Changes,
    shift: &impl BoundaryShift,
) -> Changes {
    let mut ranges: Vec<Range> = Vec::new();
    for range in iterable.unchanged() {
        ranges.push(range);
        process_last_ranges(data1, data2, &mut ranges, shift);
    }
    Changes::from_unchanged(&ranges, data1.len(), data2.len())
}

fn process_last_ranges<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    ranges: &mut Vec<Range>,
    shift: &impl BoundaryShift,
) {
    if ranges.len() < 2 {
        return;
    }
    let range1 = ranges[ranges.len() - 2];
    let range2 = ranges[ranges.len() - 1];
    if range1.end1 != range2.start1 && range1.end2 != range2.start2 {
        return;
    }

    let count1 = range1.end1 - range1.start1;
    let count2 = range2.end1 - range2.start1;

    let equal_forward = expand_forward(
        data1,
        data2,
        range1.end1,
        range1.end2,
        range1.end1 + count2,
        range1.end2 + count2,
    );
    let equal_backward = expand_backward(
        data1,
        data2,
        range2.start1 - count1,
        range2.start2 - count1,
        range2.start1,
        range2.start2,
    );

    if equal_forward == 0 && equal_backward == 0 {
        return;
    }

    if equal_forward == count2 {
        ranges.pop();
        ranges.pop();
        ranges.push(Range::new(
            range1.start1,
            range1.end1 + count2,
            range1.start2,
            range1.end2 + count2,
        ));
        process_last_ranges(data1, data2, ranges, shift);
        return;
    }

    if equal_backward == count1 {
        ranges.pop();
        ranges.pop();
        ranges.push(Range::new(
            range2.start1 - count1,
            range2.end1,
            range2.start2 - count1,
            range2.end2,
        ));
        process_last_ranges(data1, data2, ranges, shift);
        return;
    }

    let touch_left = range1.end1 == range2.start1;
    let delta = shift.shift(touch_left, equal_forward, equal_backward, range1, range2);
    if delta != 0 {
        ranges.pop();
        ranges.pop();
        ranges.push(Range::new(
            range1.start1,
            offset(range1.end1, delta),
            range1.start2,
            offset(range1.end2, delta),
        ));
        ranges.push(Range::new(
            offset(range2.start1, delta),
            range2.end1,
            offset(range2.start2, delta),
            range2.end2,
        ));
    }
}

fn offset(index: usize, delta: isize) -> usize {
    if delta >= 0 {
        index + delta as usize
    } else {
        index - delta.unsigned_abs()
    }
}

pub(crate) fn select<T>(left: bool, a: T, b: T) -> T {
    if left {
        a
    } else {
        b
    }
}

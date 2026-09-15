pub(crate) fn row_at_offset(offsets: &[f32], y: f32) -> Option<usize> {
    let pp = offsets.partition_point(|&o| o <= y);
    (1..offsets.len()).contains(&pp).then(|| pp - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFSETS: &[f32] = &[0.0, 10.0, 50.0, 68.0];

    #[test]
    fn row_at_offset_maps_each_band() {
        assert_eq!(row_at_offset(OFFSETS, 0.0), Some(0));
        assert_eq!(row_at_offset(OFFSETS, 9.9), Some(0));
        assert_eq!(row_at_offset(OFFSETS, 10.0), Some(1));
        assert_eq!(row_at_offset(OFFSETS, 49.9), Some(1));
        assert_eq!(row_at_offset(OFFSETS, 50.0), Some(2));
        assert_eq!(row_at_offset(OFFSETS, 67.9), Some(2));
    }

    #[test]
    fn row_at_offset_past_end_is_none() {
        assert_eq!(row_at_offset(OFFSETS, 68.0), None);
        assert_eq!(row_at_offset(OFFSETS, 1000.0), None);
    }
}

use std::ops::Range;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ImeSnapshot {
    pub start: u32,
    pub end: u32,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub bounds: Vec<[i32; 4]>,
}

impl ImeSnapshot {
    #[cfg(target_os = "linux")]
    pub fn valid(&self) -> bool {
        self.start != u32::MAX
            && self.end != u32::MAX
            && self.text.as_ref().is_none_or(|text| text.len() <= 65536)
            && self.bounds.len() <= 4096
            && self.bounds.iter().all(|r| r[2] >= 0 && r[3] >= 0)
    }
}

#[derive(Default)]
pub struct ImeState {
    pub selection: Option<Range<usize>>,
    pub marked: Option<Range<usize>>,
    pub bounds: Vec<[i32; 4]>,
    pub bounds_start: usize,
    text: String,
    text_start: usize,
}

impl ImeState {
    pub fn selection_changed(&mut self, snapshot: ImeSnapshot) {
        self.selection = Some(snapshot.start as usize..snapshot.end as usize);
        if self.marked.is_none() {
            self.text_start = snapshot.start.min(snapshot.end) as usize;
            self.text = snapshot.text.unwrap_or_default();
        }
    }

    pub fn composition_bounds(&mut self, snapshot: ImeSnapshot) {
        self.bounds_start = snapshot.start.min(snapshot.end) as usize;
        self.bounds = snapshot.bounds;
    }

    pub fn compose(&mut self, text: &str, selection: Option<Range<usize>>) -> Range<usize> {
        let length = text.encode_utf16().count();
        let start = self
            .marked
            .as_ref()
            .or(self.selection.as_ref())
            .map(|r| r.start.min(r.end))
            .unwrap_or(0);
        let selected = selection.unwrap_or(length..length);
        let selected = selected.start.min(length)..selected.end.min(length);
        self.marked = (!text.is_empty()).then_some(start..start + length);
        self.selection = Some(start + selected.start..start + selected.end);
        self.text_start = start;
        self.text = text.to_owned();
        selected
    }

    pub fn finish(&mut self) {
        self.marked = None;
        self.bounds.clear();
        self.text.clear();
    }

    pub fn text_for_range(&self, range: Range<usize>) -> Option<String> {
        let start = range.start.checked_sub(self.text_start)?;
        let end = range.end.checked_sub(self.text_start)?;
        let utf16: Vec<u16> = self.text.encode_utf16().collect();
        String::from_utf16(utf16.get(start..end)?).ok()
    }

    pub fn caret_bounds(&self, index: usize) -> Option<[i32; 4]> {
        let offset = index.checked_sub(self.bounds_start)?;
        if let Some(rect) = self.bounds.get(offset) {
            return Some(*rect);
        }
        if offset == self.bounds.len() {
            let [x, y, w, h] = *self.bounds.last()?;
            return Some([x.saturating_add(w), y, 1, h]);
        }
        None
    }
}

pub fn replacement_range(range: Option<Range<usize>>) -> Result<Option<[u32; 2]>, ()> {
    range
        .map(|range| {
            if range.start > range.end || range.end >= u32::MAX as usize {
                return Err(());
            }
            Ok([range.start as u32, range.end as u32])
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_preserves_utf16_offsets_and_rejects_half_surrogates() {
        let mut state = ImeState {
            selection: Some(7..10),
            ..Default::default()
        };
        state.compose("a😀b", Some(1..3));
        assert_eq!(state.marked, Some(7..11));
        assert_eq!(state.selection, Some(8..10));
        assert_eq!(state.text_for_range(8..10), Some("😀".into()));
        assert_eq!(state.text_for_range(8..9), None);
        assert_eq!(state.text_for_range(0..2), None);
        state.compose("日本", Some(2..2));
        assert_eq!(state.marked, Some(7..9));
        state.finish();
        assert_eq!(state.marked, None);
        assert_eq!(state.text_for_range(7..9), None);
    }

    #[test]
    fn candidate_anchor_uses_character_rect_and_end_caret() {
        let state = ImeState {
            bounds_start: 4,
            bounds: vec![[30, 40, 12, 20], [42, 40, 14, 20]],
            ..Default::default()
        };
        assert_eq!(state.caret_bounds(5), Some([42, 40, 14, 20]));
        assert_eq!(state.caret_bounds(6), Some([56, 40, 1, 20]));
        assert_eq!(state.caret_bounds(3), None);
    }
}

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

pub fn candidate_bounds(
    rect: [i32; 4],
    origin: gpui::Point<gpui::Pixels>,
    viewport: gpui::Size<gpui::Pixels>,
    geometry: super::page::Geometry,
) -> gpui::Bounds<gpui::Pixels> {
    let horizontal = f32::from(viewport.width) / geometry.width.max(1) as f32;
    let vertical = f32::from(viewport.height) / geometry.height.max(1) as f32;
    let [x, y, width, height] = rect;
    gpui::Bounds::new(
        origin
            + gpui::point(
                gpui::px(x as f32 * horizontal),
                gpui::px(y as f32 * vertical),
            ),
        gpui::size(
            gpui::px(width.max(1) as f32 * horizontal),
            gpui::px(height.max(1) as f32 * vertical),
        ),
    )
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
    fn candidate_rectangles_keep_their_page_position_at_every_windows_scale() {
        let origin = gpui::point(gpui::px(24.0), gpui::px(48.0));
        for scale_percent in [100_u32, 125, 150, 200] {
            let scale = scale_percent as f32 / 100.0;
            let logical_width = (1920.0 / scale).round();
            let logical_height = (1080.0 / scale).round();
            let geometry = crate::browser::page::Geometry {
                width: logical_width as u32,
                height: logical_height as u32,
                scale_percent,
            };
            let caret = [
                (logical_width * 0.25) as i32,
                (logical_height * 0.5) as i32,
                12,
                20,
            ];
            let bounds = candidate_bounds(
                caret,
                origin,
                gpui::size(gpui::px(logical_width), gpui::px(logical_height)),
                geometry,
            );
            let horizontal = (f32::from(bounds.origin.x) - 24.0) / logical_width;
            let vertical = (f32::from(bounds.origin.y) - 48.0) / logical_height;
            assert!((horizontal - 0.25).abs() < 0.001, "{scale_percent}");
            assert!((vertical - 0.5).abs() < 0.001, "{scale_percent}");
            assert_eq!(f32::from(bounds.size.width), 12.0);
            assert_eq!(f32::from(bounds.size.height), 20.0);
        }
    }

    #[test]
    fn a_stale_geometry_rescales_the_candidate_rect_onto_the_current_viewport() {
        let before = crate::browser::page::Geometry {
            width: 1920,
            height: 1080,
            scale_percent: 100,
        };
        let bounds = candidate_bounds(
            [960, 540, 12, 20],
            gpui::point(gpui::px(0.0), gpui::px(0.0)),
            gpui::size(gpui::px(960.0), gpui::px(540.0)),
            before,
        );
        assert_eq!(f32::from(bounds.origin.x), 480.0);
        assert_eq!(f32::from(bounds.origin.y), 270.0);
        assert_eq!(f32::from(bounds.size.width), 6.0);
        assert_eq!(f32::from(bounds.size.height), 10.0);
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

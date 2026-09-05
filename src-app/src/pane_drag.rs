use gpui::{
    Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px,
    svg,
};

use crate::agent_sessions::SessionAgent;
use crate::diff::ReviewSubject;

pub struct SessionDrag {
    pub agent: SessionAgent,
    pub session_id: String,
    pub cwd: String,
    pub title: SharedString,
    pub icon: SharedString,
}

#[derive(Clone)]
pub struct ReviewSubjectDrag {
    pub subject: ReviewSubject,
    pub title: SharedString,
}

#[derive(Clone)]
pub struct PaneDrag {
    pub pane_id: u64,
    pub title: SharedString,
    pub icon: SharedString,
}

pub struct DragPreview {
    pub title: SharedString,
    pub icon: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .shadow_lg()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ui.text)
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path(self.icon.clone())
                    .text_color(ui.muted),
            )
            .child(self.title.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropEdge {
    Up,
    Down,
    Left,
    Right,
}

impl DropEdge {
    pub fn to_split(self) -> (crate::layout::SplitDirection, bool) {
        match self {
            DropEdge::Up => (crate::layout::SplitDirection::Horizontal, true),
            DropEdge::Down => (crate::layout::SplitDirection::Horizontal, false),
            DropEdge::Left => (crate::layout::SplitDirection::Vertical, true),
            DropEdge::Right => (crate::layout::SplitDirection::Vertical, false),
        }
    }
}

pub const SPLIT_EDGE_BAND: f32 = 0.20;

pub fn compute_drop_edge(width: f32, height: f32, x: f32, y: f32, band: f32) -> Option<DropEdge> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let size = width.min(height) * band;
    let in_band = x < size || x > width - size || y < size || y > height - size;
    if !in_band {
        return None;
    }
    let candidates = [
        (DropEdge::Up, y),
        (DropEdge::Right, width - x),
        (DropEdge::Down, height - y),
        (DropEdge::Left, x),
    ];
    candidates
        .into_iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(edge, _)| edge)
}

pub fn split_rect(dir: Option<DropEdge>, width: f32, height: f32) -> (f32, f32, f32, f32) {
    let (hw, hh) = (width * 0.5, height * 0.5);
    match dir {
        None => (0.0, 0.0, width, height),
        Some(DropEdge::Up) => (0.0, 0.0, width, hh),
        Some(DropEdge::Down) => (0.0, hh, width, hh),
        Some(DropEdge::Left) => (0.0, 0.0, hw, height),
        Some(DropEdge::Right) => (hw, 0.0, hw, height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_edge_center_is_none() {
        assert_eq!(compute_drop_edge(1000., 800., 500., 400., 0.20), None);
    }

    #[test]
    fn drop_edge_picks_nearest_edge() {
        assert_eq!(
            compute_drop_edge(1000., 800., 40., 400., 0.20),
            Some(DropEdge::Left)
        );
        assert_eq!(
            compute_drop_edge(1000., 800., 960., 400., 0.20),
            Some(DropEdge::Right)
        );
        assert_eq!(
            compute_drop_edge(1000., 800., 500., 30., 0.20),
            Some(DropEdge::Up)
        );
        assert_eq!(
            compute_drop_edge(1000., 800., 500., 770., 0.20),
            Some(DropEdge::Down)
        );
    }

    #[test]
    fn drop_edge_non_square_uses_smaller_dimension() {
        assert_eq!(
            compute_drop_edge(200., 1000., 180., 500., 0.20),
            Some(DropEdge::Right)
        );
    }

    #[test]
    fn drop_edge_degenerate_bounds_is_none() {
        assert_eq!(compute_drop_edge(0., 0., 0., 0., 0.20), None);
    }

    #[test]
    fn split_rect_center_fills_pane() {
        assert_eq!(split_rect(None, 800., 600.), (0., 0., 800., 600.));
    }

    #[test]
    fn split_rect_edges_cover_correct_half() {
        assert_eq!(
            split_rect(Some(DropEdge::Up), 800., 600.),
            (0., 0., 800., 300.)
        );
        assert_eq!(
            split_rect(Some(DropEdge::Down), 800., 600.),
            (0., 300., 800., 300.)
        );
        assert_eq!(
            split_rect(Some(DropEdge::Left), 800., 600.),
            (0., 0., 400., 600.)
        );
        assert_eq!(
            split_rect(Some(DropEdge::Right), 800., 600.),
            (400., 0., 400., 600.)
        );
    }
}

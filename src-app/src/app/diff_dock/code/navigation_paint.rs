use gpui::{
    BorderStyle, Bounds, Corners, Edges, Hsla, Pixels, Window, fill, point, px, quad, size,
};
use paneflow_textdiff::BlockKind;

use super::navigation::{NavigationLayout, NavigationPart, NavigationState};
use super::view::CodeView;

pub(crate) fn paint(
    layout: NavigationLayout,
    state: &NavigationState,
    view: &CodeView,
    thumb_color: Hsla,
    window: &mut Window,
) {
    let ui = crate::theme::ui_colors();
    for part in [
        NavigationPart::Vertical,
        NavigationPart::Horizontal,
        NavigationPart::Minimap,
    ] {
        let Some(track) = layout.track(part) else {
            continue;
        };
        window.paint_layer(track.bounds, |window| {
            let vertical_edges = Edges {
                left: px(1.),
                ..Default::default()
            };
            if part != NavigationPart::Minimap {
                window.paint_quad(quad(
                    track.bounds,
                    Corners::default(),
                    Hsla::transparent_black(),
                    if part == NavigationPart::Vertical {
                        vertical_edges
                    } else {
                        Edges::default()
                    },
                    ui.border,
                    BorderStyle::Solid,
                ));
            }
            if part == NavigationPart::Vertical {
                paint_markers(view, track.bounds, window);
            }
            if let Some(thumb) = track.thumb {
                let color = if state.dragging(part) {
                    thumb_color.blend(ui.text.opacity(0.2))
                } else if state.hovered == Some(part) {
                    thumb_color.blend(ui.text.opacity(0.1))
                } else {
                    thumb_color
                };
                let edges = match part {
                    NavigationPart::Vertical => vertical_edges,
                    NavigationPart::Horizontal => Edges::default(),
                    NavigationPart::Minimap => Edges {
                        top: px(1.),
                        right: px(1.),
                        bottom: px(1.),
                        left: px(0.),
                    },
                };
                window.paint_quad(quad(
                    thumb,
                    Corners::default(),
                    if part == NavigationPart::Minimap {
                        color.opacity(0.5)
                    } else {
                        color
                    },
                    edges,
                    ui.border,
                    BorderStyle::Solid,
                ));
            }
        });
    }
}

fn paint_markers(view: &CodeView, bounds: Bounds<Pixels>, window: &mut Window) {
    let Some(doc) = view.document() else {
        return;
    };
    let ui = crate::theme::ui_colors();
    let unit = f32::from(bounds.size.height) / doc.line_count().max(1) as f32;
    for block in view.marker_blocks() {
        let color = match block.kind() {
            BlockKind::Added => ui.vc_added,
            BlockKind::Modified => ui.vc_modified,
            BlockKind::Deleted => ui.vc_deleted,
        };
        let top =
            px(block.lines.start as f32 * unit).min((bounds.size.height - px(5.)).max(px(0.)));
        let height = px(((block.lines.end - block.lines.start) as f32 * unit).max(5.));
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(1.), bounds.origin.y + top),
                size(px(4.), height.min(bounds.size.height - top)),
            ),
            color,
        ));
    }
}

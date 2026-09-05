use gpui::{Bounds, point, px, size};

use super::*;

#[test]
fn scrollbar_thumbs_reach_the_end_without_shrinking_below_twenty_five_pixels() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(15.), px(400.)));
    let track = scrollbar_track(bounds, 20.0, 100_000.0, 99_980.0, false);
    let thumb = track.thumb.expect("overflow");
    assert_eq!(thumb.size.height, px(25.));
    assert_eq!(thumb.bottom(), bounds.bottom());
    assert!(
        scrollbar_track(bounds, 20.0, 10.0, 0.0, false)
            .thumb
            .is_none()
    );
}

#[test]
fn horizontal_track_clicks_start_a_drag_and_reach_the_last_column() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(15.)));
    let mut navigation = NavigationState::default();
    navigation.layout.set(NavigationLayout {
        horizontal: Some(scrollbar_track(bounds, 400.0, 1_600.0, 0.0, true)),
        ..Default::default()
    });
    let scroll = CodeScroll::new();
    let mut offset = 0.0;
    assert!(navigation.mouse_down(point(px(200.), px(7.)), &scroll, &mut offset, 1_200.0));
    assert_eq!(offset, 600.0);
    assert!(navigation.dragging(NavigationPart::Horizontal));
    navigation.mouse_move(point(px(400.), px(7.)), true, &scroll, &mut offset, 1_200.0);
    assert_eq!(offset, 1_200.0);
    navigation.mouse_move(
        point(px(400.), px(7.)),
        false,
        &scroll,
        &mut offset,
        1_200.0,
    );
    assert!(navigation.drag.is_none());
}

#[test]
fn minimap_drag_tracks_document_progress_on_large_files() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(80.), px(600.)));
    let scroll = CodeScroll::new();
    scroll.set_metrics(bounds, 10_000);
    let track = minimap_track(bounds, 10_000, &scroll);
    let mut navigation = NavigationState::default();
    navigation.layout.set(NavigationLayout {
        minimap: Some(track),
        ..Default::default()
    });
    let mut horizontal = 0.0;
    let start = track.thumb.expect("overflow").center();
    assert!(navigation.mouse_down(start, &scroll, &mut horizontal, 0.0));
    navigation.mouse_move(
        point(start.x, start.y + px(100.)),
        true,
        &scroll,
        &mut horizontal,
        0.0,
    );
    assert!((scroll.rows() - 10_000.0 / 6.0).abs() < 0.01);
}

#[test]
fn minimap_click_centers_the_visible_code_and_short_files_have_no_thumb() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(80.), px(600.)));
    let scroll = CodeScroll::new();
    scroll.set_metrics(bounds, 100);
    let track = minimap_track(bounds, 100, &scroll);
    let mut navigation = NavigationState::default();
    navigation.layout.set(NavigationLayout {
        minimap: Some(track),
        ..Default::default()
    });
    let mut horizontal = 0.0;
    navigation.mouse_down(
        point(px(40.), px(50.0 * MINIMAP_LINE_HEIGHT)),
        &scroll,
        &mut horizontal,
        0.0,
    );
    assert!((scroll.rows() - (50.0 - scroll.visible_rows() / 2.0)).abs() < 0.01);
    scroll.set_metrics(bounds, 3);
    assert!(minimap_track(bounds, 3, &scroll).thumb.is_none());
    assert_eq!(minimap_width(100.0, 1.2, true), 0.0);
    assert_eq!(minimap_width(1_000.0, 1.2, true), 96.0);
}

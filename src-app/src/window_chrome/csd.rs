//! Shared CSD (Client-Side Decoration) utilities used by the main window and
//! settings window. Avoids duplicating resize-edge hit-testing and the default
//! window-button layout across multiple files.

use gpui::{
    AnyElement, App, Bounds, ClickEvent, CursorStyle, Decorations, HitboxBehavior, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point, ResizeEdge,
    SharedString, Size, Styled, Tiling, Window, WindowButton, WindowButtonLayout,
    WindowControlArea, WindowControls, canvas, div, point, prelude::*, px, size, svg,
};

/// Default button layout when the DE doesn't provide one.
pub fn default_button_layout() -> WindowButtonLayout {
    WindowButtonLayout {
        left: [None, None, None],
        right: [
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
            Some(WindowButton::Close),
        ],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClientDecorationGeometry {
    free_top: bool,
    free_bottom: bool,
    free_left: bool,
    free_right: bool,
    round_top_left: bool,
    round_top_right: bool,
    round_bottom_left: bool,
    round_bottom_right: bool,
    draw_shadow: bool,
}

impl ClientDecorationGeometry {
    fn from_tiling(tiling: Tiling) -> Self {
        let free_top = !tiling.top;
        let free_bottom = !tiling.bottom;
        let free_left = !tiling.left;
        let free_right = !tiling.right;
        Self {
            free_top,
            free_bottom,
            free_left,
            free_right,
            round_top_left: free_top && free_left,
            round_top_right: free_top && free_right,
            round_bottom_left: free_bottom && free_left,
            round_bottom_right: free_bottom && free_right,
            draw_shadow: !tiling.is_tiled(),
        }
    }
}

/// Wrap application content in a native-looking client-side decoration shell.
///
/// The outer inset belongs to the compositor shadow and resize hitbox, so it
/// must stay transparent. The themed application surface lives in the inner
/// rounded element and drops its radius, border, and padding edge-by-edge when
/// the compositor tiles or maximizes the window.
pub(crate) fn client_side_window_shell(
    content: impl IntoElement,
    window: &mut Window,
    background: Hsla,
    border_color: Hsla,
) -> impl IntoElement {
    let decorations = window.window_decorations();
    match decorations {
        Decorations::Client { .. } => window.set_client_inset(crate::RESIZE_BORDER),
        Decorations::Server => window.set_client_inset(px(0.0)),
    }

    let mut outer = div().id("window-backdrop").relative().size_full();
    let mut surface = div()
        .id("window-surface")
        .size_full()
        .cursor(CursorStyle::Arrow)
        .bg(background)
        .child(content);

    match decorations {
        Decorations::Server => outer.child(surface),
        Decorations::Client { tiling } => {
            let geometry = ClientDecorationGeometry::from_tiling(tiling);
            surface = surface
                .border_color(border_color)
                .when(geometry.round_top_left, |surface| {
                    surface.rounded_tl(crate::app::constants::WINDOW_CORNER_RADIUS)
                })
                .when(geometry.round_top_right, |surface| {
                    surface.rounded_tr(crate::app::constants::WINDOW_CORNER_RADIUS)
                })
                .when(geometry.round_bottom_left, |surface| {
                    surface.rounded_bl(crate::app::constants::WINDOW_CORNER_RADIUS)
                })
                .when(geometry.round_bottom_right, |surface| {
                    surface.rounded_br(crate::app::constants::WINDOW_CORNER_RADIUS)
                })
                .when(geometry.free_top, |surface| {
                    surface.border_t(crate::app::constants::WINDOW_BORDER_SIZE)
                })
                .when(geometry.free_bottom, |surface| {
                    surface.border_b(crate::app::constants::WINDOW_BORDER_SIZE)
                })
                .when(geometry.free_left, |surface| {
                    surface.border_l(crate::app::constants::WINDOW_BORDER_SIZE)
                })
                .when(geometry.free_right, |surface| {
                    surface.border_r(crate::app::constants::WINDOW_BORDER_SIZE)
                })
                .when(geometry.draw_shadow, |surface| {
                    surface.shadow(vec![
                        gpui::BoxShadow::new(px(0.0), px(0.0), gpui::hsla(0.0, 0.0, 0.0, 0.4))
                            .blur_radius(crate::RESIZE_BORDER / 2.0),
                    ])
                });

            outer = outer
                .bg(gpui::transparent_black())
                .when(geometry.free_top, |outer| outer.pt(crate::RESIZE_BORDER))
                .when(geometry.free_bottom, |outer| outer.pb(crate::RESIZE_BORDER))
                .when(geometry.free_left, |outer| outer.pl(crate::RESIZE_BORDER))
                .when(geometry.free_right, |outer| outer.pr(crate::RESIZE_BORDER))
                .on_mouse_move(move |event, window, _| {
                    let window_size = window.window_bounds().get_bounds().size;
                    if resize_edge(
                        event.position,
                        crate::RESIZE_BORDER * 2.0,
                        window_size,
                        tiling,
                    )
                    .is_some()
                    {
                        window.refresh();
                    }
                })
                .on_mouse_down(MouseButton::Left, move |event, window, _| {
                    let window_size = window.window_bounds().get_bounds().size;
                    if let Some(edge) =
                        resize_edge(event.position, crate::RESIZE_BORDER, window_size, tiling)
                    {
                        window.start_window_resize(edge);
                    }
                })
                .child(surface)
                .child(
                    canvas(
                        |_bounds, window, _| {
                            window.insert_hitbox(
                                Bounds::new(
                                    point(px(0.0), px(0.0)),
                                    window.window_bounds().get_bounds().size,
                                ),
                                HitboxBehavior::Normal,
                            )
                        },
                        move |_bounds, hitbox, window, _| {
                            let Some(edge) = resize_edge(
                                window.mouse_position(),
                                crate::RESIZE_BORDER,
                                window.window_bounds().get_bounds().size,
                                tiling,
                            ) else {
                                return;
                            };
                            window.set_cursor_style(
                                match edge {
                                    ResizeEdge::Top | ResizeEdge::Bottom => {
                                        CursorStyle::ResizeUpDown
                                    }
                                    ResizeEdge::Left | ResizeEdge::Right => {
                                        CursorStyle::ResizeLeftRight
                                    }
                                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                        CursorStyle::ResizeUpLeftDownRight
                                    }
                                    ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                        CursorStyle::ResizeUpRightDownLeft
                                    }
                                },
                                &hitbox,
                            );
                        },
                    )
                    .size_full()
                    .absolute(),
                );
            outer
        }
    }
}

/// Hit-test a mouse position against the CSD resize border.
///
/// Returns `Some(edge)` if the cursor is in a resize zone, respecting the
/// current tiling state (tiled edges are not resizable).
pub fn resize_edge(
    pos: Point<Pixels>,
    border: Pixels,
    window_size: Size<Pixels>,
    tiling: gpui::Tiling,
) -> Option<ResizeEdge> {
    let inner = Bounds::new(Point::default(), window_size).inset(border * 1.5);
    if inner.contains(&pos) {
        return None;
    }

    let corner = size(border * 1.5, border * 1.5);

    // Corners first (larger hit zone = 1.5× border)
    if !tiling.top && !tiling.left && Bounds::new(point(px(0.), px(0.)), corner).contains(&pos) {
        return Some(ResizeEdge::TopLeft);
    }
    if !tiling.top
        && !tiling.right
        && Bounds::new(point(window_size.width - corner.width, px(0.)), corner).contains(&pos)
    {
        return Some(ResizeEdge::TopRight);
    }
    if !tiling.bottom
        && !tiling.left
        && Bounds::new(point(px(0.), window_size.height - corner.height), corner).contains(&pos)
    {
        return Some(ResizeEdge::BottomLeft);
    }
    if !tiling.bottom
        && !tiling.right
        && Bounds::new(
            point(
                window_size.width - corner.width,
                window_size.height - corner.height,
            ),
            corner,
        )
        .contains(&pos)
    {
        return Some(ResizeEdge::BottomRight);
    }

    // Edges
    if !tiling.top && pos.y <= border {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y >= window_size.height - border {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x <= border {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x >= window_size.width - border {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

/// Render a group of window control buttons for one side (left or right).
///
/// Returns `None` if no buttons are active on this side (all slots are `None`
/// or all are filtered out by the compositor's supported controls).
///
/// `on_close` is invoked when the Close button is clicked, allowing each
/// caller (main title bar vs settings window) to dispatch its own close
/// semantics (event emission vs `window.remove_window()`).
pub(crate) fn render_button_group(
    side: &'static str,
    buttons: &[Option<WindowButton>; 3],
    is_maximized: bool,
    bar_height: Pixels,
    supported: &WindowControls,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> Option<AnyElement> {
    let children: Vec<AnyElement> = buttons
        .iter()
        .filter_map(|slot| *slot)
        .filter(|button| match button {
            WindowButton::Minimize => supported.minimize,
            WindowButton::Maximize => supported.maximize,
            WindowButton::Close => true,
        })
        .map(|button| {
            render_window_button(side, button, is_maximized, bar_height, on_close.clone())
        })
        .collect();

    if children.is_empty() {
        return None;
    }

    Some(
        div()
            .flex()
            .flex_row()
            .items_center()
            // Windows: full-height, flush, zero-gap cluster (native Win11
            // caption strip). Linux/macOS: compact pills with a 2px gap.
            .when(cfg!(target_os = "windows"), |d| d.h(bar_height))
            .when(!cfg!(target_os = "windows"), |d| d.gap(px(2.)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(children)
            .into_any_element(),
    )
}

/// Render a single window control button. Close button clicks dispatch to
/// the `on_close` callback; Min/Max call directly into `Window`.
pub(crate) fn render_window_button(
    side: &'static str,
    button: WindowButton,
    is_maximized: bool,
    bar_height: Pixels,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = match button {
        WindowButton::Minimize => "wc-minimize",
        WindowButton::Maximize => "wc-maximize",
        WindowButton::Close => "wc-close",
    };

    let icon_path = match (cfg!(target_os = "windows"), button, is_maximized) {
        (true, WindowButton::Minimize, _) => "icons/windows_minimize.svg",
        (true, WindowButton::Maximize, true) => "icons/windows_restore.svg",
        (true, WindowButton::Maximize, false) => "icons/windows_maximize.svg",
        (true, WindowButton::Close, _) => "icons/windows_close.svg",
        (false, WindowButton::Minimize, _) => "icons/generic_minimize.svg",
        (false, WindowButton::Maximize, true) => "icons/generic_restore.svg",
        (false, WindowButton::Maximize, false) => "icons/generic_maximize.svg",
        (false, WindowButton::Close, _) => "icons/generic_close.svg",
    };

    let control_area = match button {
        WindowButton::Minimize => WindowControlArea::Min,
        WindowButton::Maximize => WindowControlArea::Max,
        WindowButton::Close => WindowControlArea::Close,
    };

    let element_id = SharedString::from(format!("{id}-{side}"));
    let hover_group = SharedString::from(format!("{id}-{side}-hover"));
    let is_left = side == "l";

    // Windows: native Win11 caption buttons - 46px wide, full title-bar
    // height, square + flush, with the system hover palette (subtle white
    // overlay for min/max, #c42b1c red on close, #c84c3f when pressed). The
    // OS already hit-tests these regions as HT{MIN,MAX,CLOSE}, so snap
    // layouts and the actual minimize/maximize/close are system-handled
    // (gpui_windows events.rs) - only the pixels are ours, making them
    // indistinguishable from the OS-drawn ones. Linux/macOS keep the compact
    // chrome-themed pills.
    let is_windows = cfg!(target_os = "windows");
    let is_close = matches!(button, WindowButton::Close);
    let (button_width, button_height) = if is_windows {
        (px(46.), bar_height)
    } else if is_left {
        (px(22.), px(22.))
    } else {
        (px(28.), px(22.))
    };

    let btn = div()
        .id(element_id)
        .group(hover_group.clone())
        .window_control_area(control_area)
        .flex()
        .items_center()
        .justify_center()
        .w(button_width)
        .h(button_height)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_: &ClickEvent, window, cx| {
            cx.stop_propagation();
            match button {
                WindowButton::Minimize => window.minimize_window(),
                WindowButton::Maximize => window.zoom_window(),
                WindowButton::Close => on_close(window, cx),
            }
        });

    let btn = if is_windows {
        if is_close {
            btn.hover(|s| s.bg(gpui::rgb(0xc42b1c)))
                .active(|s| s.bg(gpui::rgb(0xc84c3f)))
        } else {
            btn.hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()))
                .active(|s| s.bg(crate::app::constants::sidebar_tab_active_background()))
        }
    } else {
        let hover_bg = crate::theme::ui_colors().subtle;
        btn.when(is_left, |s| s.rounded_full())
            .when(!is_left, |s| s.rounded_sm())
            .hover(move |s| s.bg(hover_bg))
            .active(move |s| s.bg(hover_bg))
    };

    btn.child({
        let ui = crate::theme::ui_colors();
        let icon = svg()
            .size(if is_windows { px(12.) } else { px(16.) })
            .flex_none()
            .path(icon_path)
            .text_color(ui.text);

        // Windows paints the close glyph white over its native red hover and
        // pressed states. All other states inherit the active theme's text
        // color, yielding dark controls in light mode and light controls in
        // dark mode across both the main and Settings title bars.
        icon.when(is_windows && is_close, |icon| {
            icon.group_hover(hover_group, |s| s.text_color(gpui::rgb(0xffffff)))
        })
    })
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::ClientDecorationGeometry;
    use gpui::Tiling;

    #[test]
    fn free_window_rounds_and_insets_every_edge() {
        assert_eq!(
            ClientDecorationGeometry::from_tiling(Tiling::default()),
            ClientDecorationGeometry {
                free_top: true,
                free_bottom: true,
                free_left: true,
                free_right: true,
                round_top_left: true,
                round_top_right: true,
                round_bottom_left: true,
                round_bottom_right: true,
                draw_shadow: true,
            }
        );
    }

    #[test]
    fn tiled_top_edge_loses_top_rounding_padding_and_shadow() {
        let geometry = ClientDecorationGeometry::from_tiling(Tiling {
            top: true,
            ..Tiling::default()
        });
        assert!(!geometry.free_top);
        assert!(!geometry.round_top_left);
        assert!(!geometry.round_top_right);
        assert!(geometry.round_bottom_left);
        assert!(geometry.round_bottom_right);
        assert!(!geometry.draw_shadow);
    }
}

//! Shared CSD (Client-Side Decoration) utilities used by the main window and
//! settings window. Avoids duplicating resize-edge hit-testing and the default
//! window-button layout across multiple files.

use gpui::{
    AnyElement, App, Bounds, ClickEvent, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, ResizeEdge, SharedString, Size, Styled, Window, WindowButton,
    WindowButtonLayout, WindowControlArea, WindowControls, div, point, prelude::*, px, size, svg,
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

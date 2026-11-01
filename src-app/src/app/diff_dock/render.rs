use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, SharedString, StatefulInteractiveElement,
    Styled, Window, div, img, prelude::FluentBuilder, px, svg,
};

use super::model::{DiffChrome, DiffDockTab};
use super::new_tab_menu::render_diff_new_tab_menu;
use super::options_menu::render_diff_options_button;
use crate::PaneFlowApp;
use crate::layout::CardFrame;
use crate::pane::CardShape;
use crate::settings::components::with_alpha;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::{AccessibleControlExt, AnimatedHoverExt, ROW_RADIUS, squircle_skin};

pub(super) fn dock_card_background(
    theme: &crate::theme::TerminalTheme,
    ui: crate::theme::UiColors,
    terminal_material_active: bool,
    material_veiled: bool,
) -> Hsla {
    if terminal_material_active {
        crate::pane::pane_card_background(theme, true, true, material_veiled)
    } else {
        ui.base
    }
}

pub(super) fn dock_card_fill(frame: CardFrame, background: Hsla) -> AnyElement {
    CardShape::from(frame).background(background)
}

pub(super) fn dock_card_border(
    frame: CardFrame,
    background: Hsla,
    ui: crate::theme::UiColors,
) -> AnyElement {
    CardShape::from(frame).border(background, ui.border, true)
}

pub(super) fn render_diff_resize_handle(
    width: f32,
    max_width: f32,
    frame: CardFrame,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let overlaps_the_grid = !matches!(frame, CardFrame::Card);
    div()
        .id("diff-dock-resize")
        .absolute()
        .left(px(-3.))
        .top_0()
        .bottom_0()
        .w(px(7.))
        .cursor(CursorStyle::ResizeLeftRight)
        .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.06))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _w, cx| {
                this.diff_dock.resize = Some((f32::from(event.position.x), width, max_width));
                if overlaps_the_grid {
                    cx.stop_propagation();
                }
                cx.notify();
            }),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_diff_tab_strip(
    tabs: &[DiffDockTab],
    active: usize,
    close_armed: Option<usize>,
    new_tab_menu_open: bool,
    maximized: bool,
    frame: CardFrame,
    card_background: Hsla,
    scroll: &gpui::ScrollHandle,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let chip_radius = CardShape::from(frame).tab_chip_radius();
    let mut chips = div()
        .id("diff-dock-tab-chips")
        .flex()
        .flex_row()
        .items_center()
        .h_full()
        .gap(px(3.))
        .overflow_x_scroll()
        .track_scroll(scroll);

    for (index, tab) in tabs.iter().enumerate() {
        chips = chips.child(render_diff_tab(
            tab,
            index,
            index == active,
            close_armed == Some(index),
            chip_radius,
            ui,
            cx,
        ));
    }

    let open = new_tab_menu_open;
    let rail_hover = crate::app::constants::sidebar_tab_hover_background();

    div()
        .h(px(44.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.))
        .px(px(6.))
        .border_b_1()
        .border_color(ui.border)
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(chips)
                .child(crate::pane::tab_strip_fades(
                    scroll.clone(),
                    card_background,
                )),
        )
        .child(
            squircle_skin(
                div()
                    .id("diff-dock-tab-new")
                    .flex_none()
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::PointingHand),
                "diff-dock-tab-new-group",
                ROW_RADIUS,
                open.then_some(rail_hover),
                Some(rail_hover),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.toggle_diff_new_tab_menu(!open, cx);
            }))
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path("icons/plus.svg")
                    .text_color(ui.muted),
            )
            .when(open, |trigger| {
                trigger.child(render_diff_new_tab_menu(ui, cx))
            }),
        )
        .child(render_diff_header_icon_button(
            "diff-dock-agent-sessions",
            "Agent sessions",
            "icons/sessions.svg",
            cx.listener(|this, _: &ClickEvent, window, cx| {
                if let Some(pane) = this.focused_or_first_pane(window, cx) {
                    pane.update(cx, |_, cx| {
                        cx.emit(crate::pane::PaneEvent::ToggleAgentSessions)
                    });
                }
            }),
            ui.muted,
        ))
        .child(render_diff_header_icon_button(
            "diff-dock-maximize",
            if maximized {
                "Restore panel"
            } else {
                "Maximize panel"
            },
            if maximized {
                "icons/minimize.svg"
            } else {
                "icons/maximize.svg"
            },
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.toggle_diff_dock_maximize(window, cx);
            }),
            ui.muted,
        ))
        .child(render_diff_dock_close_button(
            "diff-dock-close",
            frame,
            ui.muted,
            cx,
        ))
        .into_any_element()
}

fn render_diff_tab(
    tab: &DiffDockTab,
    index: usize,
    active: bool,
    close_armed: bool,
    chip_radius: Option<Pixels>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let file = match tab {
        DiffDockTab::File(view) => {
            let view = view.read(cx);
            let name = view
                .path()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".to_string());
            Some((
                crate::file_icons::language_icon_path(&name),
                truncate_tab_label(&name),
                view.path().display().to_string(),
                view.is_dirty(),
            ))
        }
        _ => None,
    };
    let (icon, label, tooltip) = match (tab, &file) {
        (DiffDockTab::Changes, _) => ("icons/plus-minus.svg", "Changes".to_string(), None),
        (DiffDockTab::Terminal(_), _) => ("icons/terminal.svg", "Terminal".to_string(), None),
        (DiffDockTab::PendingFile, _) => ("icons/file-text.svg", "Open a file".to_string(), None),
        (_, Some((icon, label, path, _))) => (*icon, label.clone(), Some(path.clone())),
        _ => ("icons/file-text.svg", "File".to_string(), None),
    };
    let tooltip = tooltip.unwrap_or_else(|| label.clone());
    let dirty = file.is_some_and(|(_, _, _, dirty)| dirty);
    let text = if active { ui.text } else { ui.muted };
    let group = SharedString::from(format!("diff-dock-tab-{index}-group"));
    let icon_element = match tab {
        DiffDockTab::Terminal(terminal) => {
            crate::pane::tab_badge(terminal.read(cx).terminal.detected_agent)
        }
        _ if icon_is_colored(icon) => file_icon_element(icon, px(16.), ui.muted),
        _ => crate::pane::unified_tab_icon(icon, ui.muted),
    };

    let glyph = px(crate::pane::UNIFIED_TAB_CLOSE_GLYPH_SIZE);
    let mark: AnyElement = if dirty && !close_armed {
        div()
            .relative()
            .flex_none()
            .size(glyph)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex_none()
                    .size(px(7.))
                    .rounded_full()
                    .bg(ui.vc_modified)
                    .group_hover(group.clone(), |style| style.invisible()),
            )
            .child(
                svg()
                    .absolute()
                    .inset_0()
                    .size(glyph)
                    .invisible()
                    .group_hover(group.clone(), |style| style.visible())
                    .path("icons/close.svg")
                    .text_color(ui.muted),
            )
            .into_any_element()
    } else {
        svg()
            .size(glyph)
            .flex_none()
            .path("icons/close.svg")
            .text_color(if close_armed { ui.vc_deleted } else { ui.muted })
            .into_any_element()
    };

    crate::pane::unified_tab_chip(
        SharedString::from(format!("diff-dock-tab-{index}")),
        group.clone(),
        active,
        chip_radius,
    )
    .min_w(px(64.))
    .max_w(px(208.))
    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
        this.select_diff_tab(index, cx);
        this.focus_diff_tab(index, window, cx);
    }))
    .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
    .child(icon_element)
    .child(
        crate::pane::unified_tab_label(text)
            .flex_initial()
            .child(label),
    )
    .child(
        crate::pane::tab_close_button(
            SharedString::from(format!("diff-dock-tab-close-{index}")),
            chip_radius.is_none(),
        )
        .when(!dirty && !close_armed, |button| {
            button
                .invisible()
                .group_hover(group.clone(), |style| style.visible())
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.request_close_diff_tab(index, window, cx);
            cx.stop_propagation();
        }))
        .child(mark),
    )
    .into_any_element()
}

fn icon_is_colored(icon: &str) -> bool {
    icon.starts_with("icons/languages/")
}

fn file_icon_element(icon: &'static str, size: Pixels, color: Hsla) -> AnyElement {
    if icon_is_colored(icon) {
        img(icon).size(size).flex_none().into_any_element()
    } else {
        svg()
            .size(size)
            .flex_none()
            .path(icon)
            .text_color(color)
            .into_any_element()
    }
}

const TAB_LABEL_MAX_CHARS: usize = 22;

fn truncate_tab_label(name: &str) -> String {
    if name.chars().count() <= TAB_LABEL_MAX_CHARS {
        return name.to_string();
    }
    let kept: String = name
        .chars()
        .skip(name.chars().count() - (TAB_LABEL_MAX_CHARS - 1))
        .collect();
    format!("…{kept}")
}

const FILE_HEADER_MAX_CHARS: usize = 64;

pub(super) fn diff_file_header_path(root: &str, path: &std::path::Path) -> String {
    let relative = if root.is_empty() {
        None
    } else {
        path.strip_prefix(std::path::Path::new(root)).ok()
    };
    let shown = relative
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let count = shown.chars().count();
    if count <= FILE_HEADER_MAX_CHARS {
        return shown;
    }
    let kept: String = shown
        .chars()
        .skip(count - (FILE_HEADER_MAX_CHARS - 1))
        .collect();
    format!("…{kept}")
}

pub(super) fn render_diff_file_header(
    root: &str,
    path: String,
    line: usize,
    column: usize,
    controls: AnyElement,
    tree_toggle: AnyElement,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
        .flex_none()
        .h(px(40.))
        .w_full()
        .flex()
        .items_center()
        .gap(px(6.))
        .pr(px(8.))
        .border_b_1()
        .border_color(ui.border)
        .child(super::file_chrome::render_file_breadcrumbs(root, &path, ui))
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.muted)
                .child(format!("Ln {line}, Col {column}")),
        )
        .child(controls)
        .child(tree_toggle)
        .into_any_element()
}

pub(crate) fn render_diff_header_icon_button(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    color: Hsla,
) -> gpui::Stateful<gpui::Div> {
    header_icon_button(id, label, icon, on_click, color, false)
}

pub(super) fn render_diff_dock_close_button(
    id: &'static str,
    frame: CardFrame,
    color: Hsla,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::Stateful<gpui::Div> {
    header_icon_button(
        id,
        "Close panel",
        "icons/layout-sidebar-right-collapse.svg",
        cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.dismiss_diff_dock_panel(cx);
        }),
        color,
        matches!(frame, CardFrame::Card),
    )
}

fn header_icon_button(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    color: Hsla,
    round: bool,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("{id}-group"));
    let hover = crate::app::constants::sidebar_tab_hover_background();
    let button = div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center();
    let button = if round {
        button.relative().group(group.clone()).child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(hover)
                .invisible()
                .group_hover(group, |style| style.visible()),
        )
    } else {
        squircle_skin(button, group, ROW_RADIUS, None, Some(hover))
    };
    button
        .accessible_control(gpui::accesskit::Role::Button, label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(svg().size(px(14.)).flex_none().path(icon).text_color(color))
}

pub(super) fn render_diff_files_toolbar(
    chrome: &DiffChrome<'_>,
    branch_chip: Option<AnyElement>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome.data.as_ref().filter(|d| d.has_rows());
    let diff = ui.diff_colors();

    let mut row = div()
        .flex_none()
        .h(px(36.))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .border_b_1()
        .border_color(ui.border)
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/file-text.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_none()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.text)
                .child("Uncommitted"),
        );

    if let Some(data) = loaded {
        row = row
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(diff.added)
                    .child(format!("+{}", data.added)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(diff.deleted)
                    .child(format!("-{}", data.removed)),
            );
    }

    if let Some(chip) = branch_chip {
        row = row.child(chip);
    }

    row.child(div().flex_1().min_w_0())
        .child(render_diff_options_button(chrome, ui, cx))
        .into_any_element()
}

pub(super) fn render_pending_file_body(ui: crate::theme::UiColors) -> AnyElement {
    crate::ui_primitives::panel_empty_state(
        ui,
        Some("icons/folder-open.svg"),
        Some("Open a file".into()),
        "Select a file in the workspace tree",
        false,
    )
    .into_any_element()
}

pub(super) fn render_diff_error_banner(error: &str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_none()
        .w_full()
        .min_h(px(28.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(4.))
        .border_b_1()
        .border_color(ui.border)
        .bg(with_alpha(ui.vc_deleted, 0.08))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/triangle-alert.svg")
                .text_color(ui.vc_deleted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(ui.text)
                .child(SharedString::from(error.to_string())),
        )
        .into_any_element()
}

pub(super) fn diff_panel_centered(
    icon: &'static str,
    label: impl Into<String>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    crate::ui_primitives::panel_empty_state(
        ui,
        Some(icon),
        None,
        label.into(),
        icon == "icons/loader-circle.svg",
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_dock_card_takes_the_terminal_card_color_under_a_material() {
        for theme in [
            crate::theme::paneflow_dark(),
            crate::theme::paneflow_light(),
        ] {
            let ui = crate::theme::ui_colors_with(&theme);
            assert_eq!(dock_card_background(&theme, ui, false, false), ui.base);
            assert_eq!(dock_card_background(&theme, ui, false, true), ui.base);
            for veiled in [false, true] {
                assert_eq!(
                    dock_card_background(&theme, ui, true, veiled),
                    crate::pane::pane_card_background(&theme, true, true, veiled)
                );
            }
        }
    }

    #[test]
    fn colored_language_icons_are_not_painted_as_masks() {
        for name in [
            "main.rs",
            "view.tsx",
            "Cargo.toml",
            "Dockerfile",
            "Makefile",
            "app.py",
            "logo.png",
            "LICENSE",
        ] {
            let icon = crate::file_icons::language_icon_path(name);
            assert!(
                icon_is_colored(icon),
                "{name} resolves to {icon}, which would be tinted flat"
            );
        }
        assert!(!icon_is_colored("icons/close.svg"));
    }

    #[test]
    fn a_long_tab_label_is_truncated_from_the_left() {
        let short = "main.rs";
        assert_eq!(truncate_tab_label(short), short);

        let long = "a_very_long_generated_module_name.rs";
        let cut = truncate_tab_label(long);
        assert_eq!(cut.chars().count(), TAB_LABEL_MAX_CHARS);
        assert!(cut.starts_with('…'));
        assert!(cut.ends_with(".rs"), "the tail must survive, got {cut}");

        let accented = "élément_très_long_généré_par_le_compilateur.rs";
        let cut = truncate_tab_label(accented);
        assert_eq!(cut.chars().count(), TAB_LABEL_MAX_CHARS);
        assert!(cut.ends_with(".rs"));
    }

    #[test]
    fn the_file_header_path_is_relative_and_elides_from_the_left() {
        assert_eq!(
            diff_file_header_path("/repo", Path::new("/repo/src/main.rs")),
            "src/main.rs"
        );
        assert_eq!(
            diff_file_header_path("/repo", Path::new("/etc/hosts")),
            "/etc/hosts"
        );
        assert_eq!(
            diff_file_header_path("", Path::new("/repo/src/main.rs")),
            "/repo/src/main.rs"
        );

        let deep = Path::new(
            "/repo/crates/paneflow-config/src/schema/very/deeply/nested/module/config.rs",
        );
        let shown = diff_file_header_path("/repo", deep);
        assert_eq!(shown.chars().count(), FILE_HEADER_MAX_CHARS);
        assert!(shown.starts_with('…'));
        assert!(
            shown.ends_with("config.rs"),
            "the file itself must survive the elision, got {shown}"
        );
    }
}

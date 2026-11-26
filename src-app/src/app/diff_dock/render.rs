use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, SharedString, StatefulInteractiveElement,
    Styled, Window, div, img, prelude::FluentBuilder, px, svg,
};

use super::model::{DiffChrome, DiffDockTab};
use super::new_tab_menu::render_diff_new_tab_menu;
use super::options_menu::render_diff_options_button;
use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::diff::{ComparisonPolicy, DiffOptions};
use crate::layout::CardFrame;
use crate::pane::CardShape;
use crate::settings::components::with_alpha;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::{AccessibleControlExt, AnimatedHoverExt, ROW_RADIUS, squircle_skin};

pub(super) fn dock_card_background(
    theme: &crate::theme::TerminalTheme,
    ui: crate::theme::UiColors,
    backdrop: crate::window_chrome::veil::Backdrop,
) -> Hsla {
    if backdrop.is_blended() {
        crate::pane::pane_card_background(theme, backdrop, true)
    } else {
        ui.base
    }
}

pub(super) fn dock_card_fill(
    frame: CardFrame,
    background: Hsla,
    border: Option<Hsla>,
) -> AnyElement {
    CardShape::from(frame).background(background, border)
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
    can_reopen: bool,
    maximized: bool,
    in_title_bar: bool,
    frame: CardFrame,
    card_background: Hsla,
    scroll: &gpui::ScrollHandle,
    strip_width: &std::rc::Rc<std::cell::Cell<f32>>,
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
    let divided = !in_title_bar && !matches!(tabs.get(active), Some(DiffDockTab::Changes));

    div()
        .map(|strip| {
            if in_title_bar {
                strip.h_full()
            } else {
                strip.h(px(44.))
            }
        })
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.))
        .px(px(6.))
        .when(divided, |strip| strip.border_b_1().border_color(ui.border))
        .child(
            div()
                .id("diff-dock-tab-strip")
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .on_aux_click(cx.listener(|this, event: &ClickEvent, _w, cx| {
                    if event.is_right_click()
                        && let Some(position) = event.mouse_position()
                    {
                        this.open_diff_tab_menu(None, position, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(chips)
                .child(crate::pane::tab_strip_fades(
                    scroll.clone(),
                    card_background,
                ))
                .child(crate::pane::keep_active_tab_in_view(
                    scroll.clone(),
                    active,
                    strip_width.clone(),
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
                    .path("icons/dock/add-01.svg")
                    .text_color(ui.muted),
            )
            .when(open, |trigger| {
                trigger.child(render_diff_new_tab_menu(ui, can_reopen, cx))
            }),
        )
        .child(render_diff_header_icon_button(
            "diff-dock-agent-sessions",
            "Agent sessions",
            "icons/dock/ai-chat-02.svg",
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
                "icons/dock/arrow-shrink.svg"
            } else {
                "icons/dock/arrow-expand.svg"
            },
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.toggle_diff_dock_maximize(window, cx);
            }),
            ui.muted,
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
                crate::file_icons::file_icon_path(&name),
                truncate_tab_label(&name),
                view.path().display().to_string(),
                view.is_dirty(),
            ))
        }
        _ => None,
    };
    let (icon, label, tooltip) = match (tab, &file) {
        (DiffDockTab::Changes, _) => ("icons/dock/git-compare.svg", "Changes".to_string(), None),
        (DiffDockTab::Terminal(_), _) => (
            "icons/dock/computer-terminal-01.svg",
            "Terminal".to_string(),
            None,
        ),
        (DiffDockTab::PendingFile, _) => ("icons/dock/folder-tree.svg", "Files".to_string(), None),
        (DiffDockTab::Browser(view), _) => {
            let title = view.read(cx).title();
            ("icons/world.svg", truncate_page_title(&title), Some(title))
        }
        (_, Some((icon, label, path, _))) => (*icon, label.clone(), Some(path.clone())),
        _ => ("icons/dock/file-code.svg", "File".to_string(), None),
    };
    let tooltip = tooltip.unwrap_or_else(|| label.clone());
    let closes_on_middle_click = matches!(
        tab,
        DiffDockTab::File(_) | DiffDockTab::PendingFile | DiffDockTab::Browser(_)
    );
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
                    .path("icons/dock/cancel-01.svg")
                    .text_color(ui.muted),
            )
            .into_any_element()
    } else {
        svg()
            .size(glyph)
            .flex_none()
            .path("icons/dock/cancel-01.svg")
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
    .on_aux_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
        if event.is_right_click() {
            if let Some(position) = event.mouse_position() {
                this.open_diff_tab_menu(Some(index), position, cx);
            }
            cx.stop_propagation();
        } else if event.is_middle_click() && closes_on_middle_click {
            this.request_close_diff_tab(index, window, cx);
            cx.stop_propagation();
        }
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
    crate::file_icons::is_file_icon_path(icon)
}

pub(super) fn file_icon_element(icon: &'static str, size: Pixels, color: Hsla) -> AnyElement {
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

fn truncate_page_title(title: &str) -> String {
    if title.chars().count() <= TAB_LABEL_MAX_CHARS {
        return title.to_string();
    }
    let head: String = title.chars().take(TAB_LABEL_MAX_CHARS - 1).collect();
    format!("{}…", head.trim_end())
}

pub(super) fn render_diff_file_header(
    crumbs: AnyElement,
    position: Option<(usize, usize)>,
    controls: Option<AnyElement>,
    tree_toggle: Option<AnyElement>,
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
        .child(crumbs)
        .children(position.map(|(line, column)| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.muted)
                .child(format!("Ln {line}, Col {column}"))
        }))
        .children(controls)
        .children(tree_toggle)
        .into_any_element()
}

pub(crate) fn render_diff_header_icon_button(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    color: Hsla,
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
    squircle_skin(button, group, ROW_RADIUS, None, Some(hover))
        .accessible_control(gpui::accesskit::Role::Button, label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(svg().size(px(14.)).flex_none().path(icon).text_color(color))
}

const TOOLBAR_HEIGHT: f32 = 28.0;
const TOOLBAR_GAP_BELOW: f32 = 8.0;
const TOOLBAR_CONTROL: f32 = 24.0;
const TOOLBAR_SEGMENT_HEIGHT: f32 = 20.0;
const TOOLBAR_ICON: f32 = 14.0;
const TOOLBAR_STAT_MIN_WIDTH: f32 = 460.0;
const LAYOUT_UNIFIED_ICON: &str = "icons/palette/layout-two-row.svg";
const LAYOUT_SPLIT_ICON: &str = icon::LAYOUT;
const CHANGES_TREE_ICON: &str = "icons/dock/folder-tree.svg";

fn toolbar_toggle(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    height: f32,
    pressed: bool,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let hover = crate::app::constants::sidebar_tab_hover_background();
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .w(px(TOOLBAR_CONTROL))
            .h(px(height))
            .flex()
            .items_center()
            .justify_center(),
        SharedString::from(format!("{id}-group")),
        ROW_RADIUS,
        pressed.then_some(hover),
        Some(hover),
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(on_click)
    .child(
        svg()
            .size(px(TOOLBAR_ICON))
            .flex_none()
            .path(icon)
            .text_color(if pressed { ui.text } else { ui.muted }),
    )
}

fn render_layout_segments(
    split: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::Stateful<gpui::Div> {
    let unified = toolbar_toggle(
        "diff-dock-layout-unified",
        "Unified diff view",
        LAYOUT_UNIFIED_ICON,
        TOOLBAR_SEGMENT_HEIGHT,
        !split,
        ui,
        cx.listener(|this, _: &ClickEvent, _w, cx| this.set_diff_dock_split(false, cx)),
    );
    let side_by_side = toolbar_toggle(
        "diff-dock-layout-split",
        "Split diff view",
        LAYOUT_SPLIT_ICON,
        TOOLBAR_SEGMENT_HEIGHT,
        split,
        ui,
        cx.listener(|this, _: &ClickEvent, _w, cx| this.set_diff_dock_split(true, cx)),
    );
    squircle_skin(
        div()
            .id("diff-dock-layout")
            .flex_none()
            .h(px(TOOLBAR_CONTROL))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .px(px(2.)),
        "diff-dock-layout-group",
        ROW_RADIUS,
        Some(with_alpha(ui.text, 0.05)),
        None,
    )
    .child(unified)
    .child(side_by_side)
}

pub(super) fn render_diff_files_toolbar(
    chrome: &DiffChrome<'_>,
    branch_chip: Option<AnyElement>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome.data.as_ref().filter(|d| d.has_rows());
    let files = loaded.filter(|d| d.file_count > 0);
    let diff = ui.diff_colors();

    let left = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(crate::ui_primitives::BODY)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.text)
                .child("Uncommitted"),
        )
        .children(branch_chip);

    let mut right = div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.));

    if let Some(data) = loaded.filter(|_| chrome.width >= TOOLBAR_STAT_MIN_WIDTH) {
        right = right.child(
            div()
                .flex_none()
                .mr(px(4.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .whitespace_nowrap()
                .text_size(crate::ui_primitives::LABEL_SM)
                .child(
                    div()
                        .text_color(diff.added)
                        .child(format!("+{}", data.added)),
                )
                .child(
                    div()
                        .text_color(diff.deleted)
                        .child(format!("-{}", data.removed)),
                ),
        );
    }

    let cwd = chrome.cwd.clone();
    right = right.child(toolbar_toggle(
        "diff-dock-refresh",
        "Refresh changes",
        icon::REFRESH,
        TOOLBAR_CONTROL,
        false,
        ui,
        cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.refresh_diff_dock(cwd.clone(), cx);
        }),
    ));

    if let Some(data) = files {
        let all_collapsed = data.all_collapsed(chrome.collapsed);
        let paths = data.paths();
        right = right.child(toolbar_toggle(
            "diff-dock-collapse-all",
            if all_collapsed {
                "Expand all files"
            } else {
                "Collapse all files"
            },
            if all_collapsed {
                icon::EXPAND_ALL
            } else {
                icon::COLLAPSE_ALL
            },
            TOOLBAR_CONTROL,
            false,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_all_diff_collapsed(&paths, !all_collapsed, cx);
            }),
        ));
    }

    right = right.child(render_layout_segments(chrome.split, ui, cx));

    let options = chrome.options;
    let hides_whitespace = options.whitespace == ComparisonPolicy::IgnoreWhitespaces;
    right = right.child(toolbar_toggle(
        "diff-dock-whitespace",
        if hides_whitespace {
            "Show whitespace changes"
        } else {
            "Hide whitespace changes"
        },
        icon::WHITESPACE,
        TOOLBAR_CONTROL,
        hides_whitespace,
        ui,
        cx.listener(move |this, _: &ClickEvent, _w, cx| {
            let whitespace = if hides_whitespace {
                ComparisonPolicy::Default
            } else {
                ComparisonPolicy::IgnoreWhitespaces
            };
            this.set_diff_dock_options(
                DiffOptions {
                    whitespace,
                    ..options
                },
                cx,
            );
        }),
    ));

    if files.is_some() {
        let tree_open = chrome.tree_open;
        right = right.child(toolbar_toggle(
            "diff-dock-changes-tree",
            if tree_open {
                "Hide file tree"
            } else {
                "Show file tree"
            },
            CHANGES_TREE_ICON,
            TOOLBAR_CONTROL,
            tree_open,
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_diff_changes_tree_open(!tree_open, cx);
            }),
        ));
    }

    div()
        .flex_none()
        .h(px(TOOLBAR_HEIGHT))
        .mb(px(TOOLBAR_GAP_BELOW))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .child(left)
        .child(right.child(render_diff_options_button(chrome, ui, cx)))
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
                .path("icons/dock/alert-02.svg")
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
        icon == "icons/dock/loader-circle.svg",
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_card_takes_the_terminal_card_color_under_a_material() {
        for theme in [
            crate::theme::paneflow_dark(),
            crate::theme::paneflow_light(),
        ] {
            use crate::window_chrome::veil::{Backdrop, BlurSupport};
            let ui = crate::theme::ui_colors_with(&theme);
            assert_eq!(dock_card_background(&theme, ui, Backdrop::THEMED), ui.base);
            for blur in [BlurSupport::Full, BlurSupport::None] {
                let blended = Backdrop::new(
                    paneflow_config::schema::BackgroundStyle::Blended,
                    true,
                    blur,
                );
                assert_eq!(
                    dock_card_background(&theme, ui, blended),
                    crate::pane::pane_card_background(&theme, blended, true)
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
            let icon = crate::file_icons::file_icon_path(name);
            assert!(
                icon_is_colored(icon),
                "{name} resolves to {icon}, which would be tinted flat"
            );
        }
        assert!(!icon_is_colored("icons/dock/cancel-01.svg"));
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
    fn a_long_page_title_keeps_its_start_like_a_pane_tab() {
        assert_eq!(truncate_page_title("Checkout form"), "Checkout form");
        let cut =
            truncate_page_title("A very long page title that keeps going well past any tab width");
        assert!(cut.starts_with("A very long page"), "got {cut}");
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= TAB_LABEL_MAX_CHARS);
    }
}

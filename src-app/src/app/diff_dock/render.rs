use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, Window, div, img, prelude::FluentBuilder, px, svg,
};

use super::model::{DiffChrome, DiffDockTab};
use super::new_tab_menu::render_diff_new_tab_menu;
use super::options_menu::render_diff_options_button;
use crate::PaneFlowApp;
use crate::settings::components::with_alpha;
use crate::ui_primitives::{AnimatedHoverExt, ROW_RADIUS, squircle_skin};

pub(super) fn render_diff_resize_handle(
    width: f32,
    max_width: f32,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
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
                cx.notify();
            }),
        )
        .into_any_element()
}

pub(super) fn render_diff_tab_strip(
    tabs: &[DiffDockTab],
    active: usize,
    close_armed: Option<usize>,
    new_tab_menu_open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let mut strip = div()
        .h(px(40.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .px(px(8.))
        .border_b_1()
        .border_color(ui.border);

    for (index, tab) in tabs.iter().enumerate() {
        strip = strip.child(render_diff_tab(
            tab,
            index,
            index == active,
            close_armed == Some(index),
            ui,
            cx,
        ));
    }

    let open = new_tab_menu_open;
    let rail_hover = crate::app::constants::sidebar_tab_hover_background();

    strip
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
        .child(div().flex_1().min_w_0())
        .child(render_diff_header_icon_button(
            "diff-dock-close",
            "icons/close.svg",
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.close_diff_dock_panel(cx);
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
                file_tab_icon(&name),
                truncate_tab_label(&name),
                view.is_dirty(),
            ))
        }
        _ => None,
    };
    let (icon, label) = match (tab, &file) {
        (DiffDockTab::Changes, _) => ("icons/plus-minus.svg", "Changes".to_string()),
        (DiffDockTab::Terminal(_), _) => ("icons/terminal.svg", "Terminal".to_string()),
        (DiffDockTab::PendingFile, _) => ("icons/file-text.svg", "Open a file".to_string()),
        (_, Some((icon, label, _))) => (*icon, label.clone()),
        _ => ("icons/file-text.svg", "File".to_string()),
    };
    let dirty = file.map(|(_, _, dirty)| dirty).unwrap_or(false);
    let rail_hover = crate::app::constants::sidebar_tab_hover_background();
    let (resting, hovered) = if active {
        (Some(rail_hover), None)
    } else {
        (None, Some(rail_hover))
    };
    let text = if active { ui.text } else { ui.muted };
    let group = SharedString::from(format!("diff-dock-tab-{index}-group"));

    let mut chip = squircle_skin(
        div()
            .id(SharedString::from(format!("diff-dock-tab-{index}")))
            .flex_none()
            .h(px(26.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .cursor(CursorStyle::PointingHand),
        group.clone(),
        ROW_RADIUS,
        resting,
        hovered,
    )
    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.select_diff_tab(index, cx);
    }))
    .child(file_icon_element(
        icon,
        px(13.),
        if active { ui.muted } else { text },
    ))
    .child(
        div()
            .flex_none()
            .whitespace_nowrap()
            .text_size(crate::ui_primitives::BODY)
            .font_weight(FontWeight::MEDIUM)
            .text_color(text)
            .child(label),
    );

    if matches!(
        tab,
        DiffDockTab::Terminal(_) | DiffDockTab::File(_) | DiffDockTab::PendingFile
    ) {
        let mark: AnyElement = if dirty && !close_armed {
            div()
                .relative()
                .flex_none()
                .size(px(11.))
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
                        .size(px(11.))
                        .invisible()
                        .group_hover(group.clone(), |style| style.visible())
                        .path("icons/close.svg")
                        .text_color(ui.muted),
                )
                .into_any_element()
        } else {
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/close.svg")
                .text_color(if close_armed { ui.vc_deleted } else { ui.muted })
                .into_any_element()
        };
        chip = chip.child(
            div()
                .id(SharedString::from(format!("diff-dock-tab-close-{index}")))
                .flex_none()
                .size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .animated_hover_bg(
                    gpui::transparent_black(),
                    crate::app::constants::sidebar_tab_active_background(),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.request_close_diff_tab(index, cx);
                }))
                .child(mark),
        );
    }

    chip.into_any_element()
}

pub(super) fn file_tab_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "dockerfile" => return "icons/languages/docker.svg",
        "makefile" => return "icons/languages/makefile.svg",
        _ => {}
    }
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    match ext {
        "rs" => "icons/languages/rust-small.svg",
        "ts" | "tsx" | "mts" | "cts" => "icons/languages/typescript.svg",
        "js" | "jsx" | "mjs" | "cjs" => "icons/languages/react.svg",
        "json" | "jsonc" => "icons/languages/json.svg",
        "toml" => "icons/languages/toml.svg",
        "md" | "markdown" | "mdx" => "icons/languages/markdown.svg",
        "py" | "pyi" => "icons/languages/python.svg",
        "go" => "icons/languages/go.svg",
        "rb" => "icons/languages/ruby.svg",
        "swift" => "icons/languages/swift.svg",
        "css" | "scss" | "sass" | "less" => "icons/languages/css.svg",
        "log" => "icons/languages/log.svg",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => "icons/languages/image.svg",
        "txt" | "text" => "icons/languages/text.svg",
        _ => "icons/file-text.svg",
    }
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
    icon: &'static str,
    path: String,
    line: usize,
    column: usize,
    ui: crate::theme::UiColors,
) -> AnyElement {
    div()
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
        .child(file_icon_element(icon, px(14.), ui.muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.text)
                .child(path),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.muted)
                .child(format!("Ln {line}, Col {column}")),
        )
        .into_any_element()
}

pub(super) fn render_diff_header_icon_button(
    id: &'static str,
    icon: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    color: Hsla,
) -> AnyElement {
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center(),
        SharedString::from(format!("{id}-group")),
        ROW_RADIUS,
        None,
        Some(crate::app::constants::sidebar_tab_hover_background()),
    )
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(on_click)
    .child(svg().size(px(14.)).flex_none().path(icon).text_color(color))
    .into_any_element()
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
    fn the_tab_icon_follows_the_extension_and_falls_back() {
        assert_eq!(file_tab_icon("main.rs"), "icons/languages/rust-small.svg");
        assert_eq!(file_tab_icon("view.TSX"), "icons/languages/typescript.svg");
        assert_eq!(file_tab_icon("Cargo.toml"), "icons/languages/toml.svg");
        assert_eq!(file_tab_icon("Dockerfile"), "icons/languages/docker.svg");
        assert_eq!(
            file_tab_icon("paneflow.schema.json"),
            "icons/languages/json.svg"
        );
        assert_eq!(file_tab_icon("LICENSE"), "icons/file-text.svg");
        assert_eq!(file_tab_icon("notes.xyz"), "icons/file-text.svg");
        assert_eq!(file_tab_icon(""), "icons/file-text.svg");
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
        ] {
            let icon = file_tab_icon(name);
            assert!(
                icon_is_colored(icon),
                "{name} resolves to {icon}, which would be tinted flat"
            );
        }
        assert!(!icon_is_colored(file_tab_icon("LICENSE")));
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

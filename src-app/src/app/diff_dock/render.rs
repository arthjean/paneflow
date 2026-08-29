//! Free render helpers for the diff dock chrome: the resize handle, the
//! toolbar toggle button, the tab strip, the files toolbar, and the
//! empty/loading/error placeholder. The body (the shared `DiffElement`) and the
//! panel orchestration live on `PaneFlowApp` in [`super`].

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

/// The thin, column-resize hit target straddling the panel's left border.
/// Captures the drag anchor `(cursor_x, width_at_grab, max_width)`; the resize math
/// runs in the CLI dock wrapper's `on_mouse_move` (a wide capture surface, so
/// the drag survives the cursor leaving the dock).
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
                // Anchor on the *rendered* width, not the stored preference:
                // while a right rail clamps the dock, a drag must continue from
                // the edge the cursor grabbed instead of jumping to a width
                // that is not on screen.
                this.diff_dock.resize = Some((f32::from(event.position.x), width, max_width));
                cx.notify();
            }),
        )
        .into_any_element()
}

/// The dock's tab strip: the permanent "Changes" diff tab, then one tab per
/// terminal opened from the trailing `+` (which opens the surface picker in
/// [`super::new_tab_menu`]). The dock's own close button is pinned right, so it
/// stays reachable from every tab.
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

    // Toggle off the render-time snapshot, not the live flag: the open menu's
    // `on_mouse_up_out` fires on this same release and has already cleared it,
    // so a live toggle would re-open the menu on every second press.
    let open = new_tab_menu_open;
    // Same skin as the rail's own `+`: 28 px box, `ROW_RADIUS` superellipse,
    // rail hover tint. While the picker is up the hover fill is pinned on as
    // the resting fill so the trigger stays lit.
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

/// One tab chip. The active one carries the raised fill and hairline; the rest
/// stay flat until hovered. Terminal tabs get a trailing close button; the
/// `Changes` tab is permanent and has none.
fn render_diff_tab(
    tab: &DiffDockTab,
    index: usize,
    active: bool,
    close_armed: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    // A file tab reads its chip straight off the open document, so the label,
    // the icon and the dirty dot can never describe a stale path.
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
        // Unreachable: `File` is the only remaining variant and it always
        // resolves `file` above. Kept total rather than panicking in a paint.
        _ => ("icons/file-text.svg", "File".to_string()),
    };
    let dirty = file.map(|(_, _, dirty)| dirty).unwrap_or(false);
    // The rail's row grammar, verbatim: exactly one chip rests filled (the
    // active one, which then has no hover step), every other stays flat and
    // takes the same fill on hover. No hairline - the rail marks selection with
    // material, not with a drawn border - and the same `ROW_RADIUS`
    // superellipse instead of GPUI's circular `rounded()`.
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
        // Cursor's grammar: a modified document trades the close glyph for a
        // dot at rest, the dot yields the slot back to the glyph while the
        // pointer is on the chip, and the control keeps its hit target through
        // both. Arming the close (the confirmation US-017 asks for) pins the
        // glyph in the deletion color, so the second press reads as
        // destructive.
        let mark: AnyElement = if dirty && !close_armed {
            // The two states share the slot and swap by visibility, the way
            // `squircle_skin` swaps its own fills: both are laid out every
            // frame, so the flip cannot disagree with itself between prepaint
            // and paint, and the chip never resizes under the pointer.
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
                // A control nested inside a filled row, like the rail's own
                // hover actions: it keeps a plain 6 px corner (a superellipse
                // this small resolves to a lozenge) and hovers one tint step
                // past the row it sits on, or it would be invisible.
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

/// Icon for a file tab, derived from the basename (US-017). Mirrors the diff
/// body's language mapping over the bundled `icons/languages` set, but falls
/// back to `icons/file-text.svg` rather than the diff's generic file glyph, so
/// an unknown extension still reads as "a document" in the strip.
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

/// Whether an icon asset carries its own colors.
///
/// The `icons/languages/` set is multi-fill artwork; everything else in the
/// bundle is a single-color glyph meant to be tinted by its caller.
fn icon_is_colored(icon: &str) -> bool {
    icon.starts_with("icons/languages/")
}

/// Paint a file icon on the side of the fence it belongs to.
///
/// A colored asset goes through `img()`; `svg()` would rasterize it to an alpha
/// mask and repaint the whole glyph in `color`, collapsing it to a solid blob.
/// The monochrome fallback still wants the tint, so it keeps the `svg()` path.
/// See `crate::file_icons` for the policy this enforces.
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

/// Longest tab label the strip shows before eliding. Past it the tail is kept
/// (the extension carries more signal than the head of a long basename).
const TAB_LABEL_MAX_CHARS: usize = 22;

/// Truncate a tab label to [`TAB_LABEL_MAX_CHARS`] on char boundaries, so a
/// multi-byte basename can never be sliced mid-codepoint (US-017).
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

/// Longest path the file header shows before eliding from the left. Beyond it
/// the tail (the file itself) is what survives.
const FILE_HEADER_MAX_CHARS: usize = 64;

/// The worktree-relative path of `path`, truncated from the left, plus the
/// separator normalization the header needs. `root` is the dock's diff cwd;
/// a file outside it keeps its absolute path, which is the honest label.
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

/// The header shown under the tab strip while a file tab is active (US-018):
/// the worktree-relative path on the left, the caret's line and column on the
/// right. Same 36 px height and hairline as [`render_diff_files_toolbar`], so
/// switching tabs never shifts the body by a pixel.
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

/// A dock-header control skinned exactly like the sidebar's rail actions: the
/// same 28 px box, the same continuous corner (`ROW_RADIUS` traced by
/// `squircle`, not a circular `rounded()`), and the same hover tint. The dock
/// chrome and the workspace rail are the same control family, so they share one
/// silhouette instead of drifting into two.
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

/// The summary row under the tab strip, shown with the `Changes` tab: the scope
/// ("Uncommitted" plus its +/- totals), the branch chip, then the overflow menu
/// pushed to the right edge.
pub(super) fn render_diff_files_toolbar(
    chrome: &DiffChrome<'_>,
    branch_chip: Option<AnyElement>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome
        .data
        .as_ref()
        .filter(|d| !d.loading && d.error.is_none());
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

/// The body of the placeholder `File` tab: what the dock is now for, while the
/// Files sidebar beside it supplies the document. Titled (unlike the diff's own
/// empty states) because it is an instruction, not a report on a folder.
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

    /// US-017: the chip's icon is derived from the extension, with
    /// `icons/file-text.svg` as the declared fallback.
    #[test]
    fn the_tab_icon_follows_the_extension_and_falls_back() {
        assert_eq!(file_tab_icon("main.rs"), "icons/languages/rust-small.svg");
        assert_eq!(file_tab_icon("view.TSX"), "icons/languages/typescript.svg");
        assert_eq!(file_tab_icon("Cargo.toml"), "icons/languages/toml.svg");
        // Extension-less well-known names are matched whole.
        assert_eq!(file_tab_icon("Dockerfile"), "icons/languages/docker.svg");
        // Multi-dot names use the last segment.
        assert_eq!(
            file_tab_icon("paneflow.schema.json"),
            "icons/languages/json.svg"
        );
        // Anything unrecognized, and anything with no extension at all.
        assert_eq!(file_tab_icon("LICENSE"), "icons/file-text.svg");
        assert_eq!(file_tab_icon("notes.xyz"), "icons/file-text.svg");
        assert_eq!(file_tab_icon(""), "icons/file-text.svg");
    }

    /// The `icons/languages/` assets ship their own `fill`, so every icon
    /// `file_tab_icon` can hand back must be routed to `img()`; only the
    /// monochrome fallback goes through the tinted `svg()` path. Painting a
    /// colored asset as an `svg()` mask flattens it to a solid blob.
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

    /// US-017: a long file name is truncated for the chip, keeping the tail
    /// (the part that identifies the file) and never splitting a character.
    #[test]
    fn a_long_tab_label_is_truncated_from_the_left() {
        let short = "main.rs";
        assert_eq!(truncate_tab_label(short), short);

        let long = "a_very_long_generated_module_name.rs";
        let cut = truncate_tab_label(long);
        assert_eq!(cut.chars().count(), TAB_LABEL_MAX_CHARS);
        assert!(cut.starts_with('…'));
        assert!(cut.ends_with(".rs"), "the tail must survive, got {cut}");

        // Multi-byte input must not panic or produce a broken boundary.
        let accented = "élément_très_long_généré_par_le_compilateur.rs";
        let cut = truncate_tab_label(accented);
        assert_eq!(cut.chars().count(), TAB_LABEL_MAX_CHARS);
        assert!(cut.ends_with(".rs"));
    }

    /// US-018: the header shows the worktree-relative path, elided from the
    /// left when it does not fit; a file outside the worktree keeps its
    /// absolute path rather than a misleading relative one.
    #[test]
    fn the_file_header_path_is_relative_and_elides_from_the_left() {
        assert_eq!(
            diff_file_header_path("/repo", Path::new("/repo/src/main.rs")),
            "src/main.rs"
        );
        // Outside the worktree, and with no worktree at all.
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

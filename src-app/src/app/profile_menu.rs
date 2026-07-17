//! Profile menu - opened by clicking the user avatar on the right of the
//! title bar. Mirrors Zed's user menu shape: a user-info header and an app
//! action list (Settings, Themes…, About). Sign Out will be added once auth
//! is wired.

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement, Styled, Window,
    deferred, div, px,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_divider_color, select_item, select_menu};

/// Approximate width of the profile menu - used to shift the menu left so
/// its right edge sits under the profile button (click anchor is inside the
/// button on the far right of the title bar).
const PROFILE_MENU_WIDTH: Pixels = px(220.);
// 200px matches the `select_menu` primitive's `min_w`, so the rendered width
// equals the value used to clamp the menu's left edge (no silent overflow).
const TITLE_BAR_FILES_MENU_WIDTH: Pixels = px(200.);
const TITLE_BAR_HELP_MENU_WIDTH: Pixels = px(220.);
const DOCUMENTATION_URL: &str = "https://paneflow.dev/docs";
const RELEASES_URL: &str = "https://paneflow.dev/releases";
const AUTOMATIONS_URL: &str = "https://paneflow.dev/docs/scripting";
const REVIEW_URL: &str = "https://paneflow.dev/docs/review";
const TROUBLESHOOTING_URL: &str = "https://paneflow.dev/docs/troubleshooting";
type TitleBarMenuClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static>;

impl PaneFlowApp {
    fn open_help_url(&mut self, url: &'static str, cx: &mut Context<Self>) {
        if let Err(err) = crate::external_open::open_url(url) {
            log::warn!("help menu: open URL failed: {err}");
            self.show_toast(format!("Could not open URL: {err}"), cx);
        }
    }

    pub(crate) fn render_title_bar_files_menu(
        &self,
        anchor: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let win_size = window.window_bounds().get_bounds().size;
        let desired_left = anchor.x - px(12.);
        let max_left = (win_size.width - TITLE_BAR_FILES_MENU_WIDTH - px(4.)).max(px(4.));
        let left = desired_left.clamp(px(4.), max_left);
        let top = anchor.y + px(4.);

        let menu_item = |id: &'static str, label: &'static str, on_click: TitleBarMenuClick| {
            select_item(id, false, ui)
                .cursor(CursorStyle::Arrow)
                .on_click(on_click)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
                        .child(label),
                )
        };

        let new_workspace = menu_item(
            "title-bar-files-new-workspace",
            "New Workspace",
            Box::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.title_bar_files_menu_open = None;
                this.create_workspace_with_picker(window, cx);
                cx.stop_propagation();
            })),
        );
        let settings = menu_item(
            "title-bar-files-settings",
            "Settings",
            Box::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.title_bar_files_menu_open = None;
                this.open_settings_window(window, cx);
                cx.stop_propagation();
            })),
        );

        deferred(
            select_menu("title-bar-files-menu", ui)
                .occlude()
                .absolute()
                .left(left)
                .top(top)
                .w(TITLE_BAR_FILES_MENU_WIDTH)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.title_bar_files_menu_open = None;
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(new_workspace)
                .child(settings),
        )
        .with_priority(4)
        .into_any_element()
    }

    pub(crate) fn render_title_bar_help_menu(
        &self,
        anchor: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let win_size = window.window_bounds().get_bounds().size;
        let desired_left = anchor.x - px(12.);
        let max_left = (win_size.width - TITLE_BAR_HELP_MENU_WIDTH - px(4.)).max(px(4.));
        let left = desired_left.clamp(px(4.), max_left);
        let top = anchor.y + px(4.);

        let menu_item = |id: &'static str, label: &'static str, on_click: TitleBarMenuClick| {
            select_item(id, false, ui)
                .cursor(CursorStyle::Arrow)
                .on_click(on_click)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
                        .child(label),
                )
        };

        let documentation = menu_item(
            "title-bar-help-documentation",
            "Paneflow Documentation",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.open_help_url(DOCUMENTATION_URL, cx);
                cx.stop_propagation();
            })),
        );
        let whats_new = menu_item(
            "title-bar-help-whats-new",
            "What's New",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.open_help_url(RELEASES_URL, cx);
                cx.stop_propagation();
            })),
        );
        let automations = menu_item(
            "title-bar-help-automations",
            "Automations",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.open_help_url(AUTOMATIONS_URL, cx);
                cx.stop_propagation();
            })),
        );
        let review = menu_item(
            "title-bar-help-review",
            "Review",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.open_help_url(REVIEW_URL, cx);
                cx.stop_propagation();
            })),
        );
        let troubleshooting = menu_item(
            "title-bar-help-troubleshooting",
            "Troubleshooting",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.open_help_url(TROUBLESHOOTING_URL, cx);
                cx.stop_propagation();
            })),
        );
        let about = menu_item(
            "title-bar-help-about",
            "About Paneflow",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.show_about_dialog = true;
                cx.notify();
                cx.stop_propagation();
            })),
        );

        deferred(
            select_menu("title-bar-help-menu", ui)
                .occlude()
                .absolute()
                .left(left)
                .top(top)
                .w(TITLE_BAR_HELP_MENU_WIDTH)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.title_bar_help_menu_open = None;
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(documentation)
                .child(whats_new)
                .child(automations)
                .child(review)
                .child(troubleshooting)
                .child(
                    div()
                        .mx(px(6.))
                        .my(px(4.))
                        .h(px(1.))
                        .bg(menu_divider_color(ui)),
                )
                .child(about),
        )
        .with_priority(4)
        .into_any_element()
    }

    pub(crate) fn render_profile_menu(
        &self,
        anchor: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let win_size = window.window_bounds().get_bounds().size;

        // Anchor the menu's top-right corner just below-left of the cursor so
        // the menu opens toward the free area (down-and-left of the profile
        // button). Then clamp both axes so the menu never spills past the
        // window edges - same flip-if-overflow pattern as the workspace
        // context menu in `main.rs`.
        let desired_left = anchor.x - PROFILE_MENU_WIDTH;
        let max_left = (win_size.width - PROFILE_MENU_WIDTH - px(4.)).max(px(4.));
        let left = desired_left.clamp(px(4.), max_left);

        // Estimated menu height: header (26) + sep (7) + 3 items × 24 (72)
        //                      + p(4) ×2 + border = ~115.
        // Rounded up to leave slack for font-metric variance.
        let menu_height = px(140.);
        let desired_top = anchor.y + px(4.);
        let top = if desired_top + menu_height > win_size.height {
            (desired_top - menu_height).max(px(4.))
        } else {
            desired_top
        };

        // ── User info header (placeholder until auth lands) ──
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(10.))
            .py(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child("Guest"),
            )
            .child(
                div()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(ui.subtle)
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child("Free"),
            );

        // ── App actions ──
        let settings_item = self.render_context_menu_item(
            SharedString::from("profile-menu-settings"),
            "Settings",
            None,
            ui,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.profile_menu_open = None;
                this.open_settings_window(window, cx);
                cx.stop_propagation();
            }),
        );

        let themes_item = self.render_context_menu_item(
            SharedString::from("profile-menu-themes"),
            "Themes…",
            None,
            ui,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.profile_menu_open = None;
                this.open_theme_picker(window, cx);
                cx.stop_propagation();
            }),
        );

        let about_item = self.render_context_menu_item(
            SharedString::from("profile-menu-about"),
            "About PaneFlow",
            None,
            ui,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.profile_menu_open = None;
                this.show_about_dialog = true;
                cx.notify();
                cx.stop_propagation();
            }),
        );

        deferred(
            div()
                .id("profile-menu")
                .occlude()
                .absolute()
                .left(left)
                .top(top)
                .w(PROFILE_MENU_WIDTH)
                .bg(ui.overlay)
                .border_1()
                .border_color(ui.border)
                .rounded(px(8.))
                .flex()
                .flex_col()
                .p(px(4.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.profile_menu_open = None;
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(header)
                .child(
                    div()
                        .mx(px(-4.))
                        .my(px(3.))
                        .h(px(1.))
                        .bg(menu_divider_color(ui)),
                )
                .child(settings_item)
                .child(themes_item)
                .child(about_item),
        )
        .with_priority(4)
        .into_any_element()
    }
}

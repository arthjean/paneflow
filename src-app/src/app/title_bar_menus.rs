use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, StatefulInteractiveElement, Styled, Window, deferred, div, px,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_divider_color, menu_row, select_menu};

const TITLE_BAR_FILES_MENU_WIDTH: Pixels = px(200.);
const TITLE_BAR_HELP_MENU_WIDTH: Pixels = px(220.);
pub(crate) const DOCUMENTATION_URL: &str = "https://paneflow.dev/docs";
pub(crate) const RELEASES_URL: &str = "https://paneflow.dev/releases";
pub(crate) const AUTOMATIONS_URL: &str = "https://paneflow.dev/docs/scripting";
pub(crate) const TROUBLESHOOTING_URL: &str = "https://paneflow.dev/docs/troubleshooting";
type TitleBarMenuClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static>;

impl PaneFlowApp {
    pub(crate) fn open_documentation(&mut self, cx: &mut Context<Self>) {
        self.open_help_url(DOCUMENTATION_URL, cx);
    }

    pub(crate) fn open_help_url(&mut self, url: &'static str, cx: &mut Context<Self>) {
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
            menu_row(id, false, ui)
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

        deferred(crate::ui_primitives::menu_reveal(
            "title-bar-files-menu-reveal",
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
        ))
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
            menu_row(id, false, ui)
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
        let check_for_updates = menu_item(
            "title-bar-help-check-for-updates",
            "Check for Updates…",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.request_update_check(cx);
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
        let troubleshooting = menu_item(
            "title-bar-help-troubleshooting",
            "Troubleshooting",
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.title_bar_help_menu_open = None;
                this.open_help_url(TROUBLESHOOTING_URL, cx);
                cx.stop_propagation();
            })),
        );
        let system_info = menu_item(
            "title-bar-help-system-info",
            "System Info…",
            Box::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.title_bar_help_menu_open = None;
                this.open_system_info_dialog(window, cx);
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

        deferred(crate::ui_primitives::menu_reveal(
            "title-bar-help-menu-reveal",
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
                .child(check_for_updates)
                .child(automations)
                .child(troubleshooting)
                .child(system_info)
                .child(
                    div()
                        .mx(px(6.))
                        .my(px(4.))
                        .h(px(1.))
                        .bg(menu_divider_color(ui)),
                )
                .child(about),
        ))
        .with_priority(4)
        .into_any_element()
    }
}

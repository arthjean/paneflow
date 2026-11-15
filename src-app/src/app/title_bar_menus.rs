use gpui::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, Pixels, Point, Styled, Window,
    deferred, px,
};

use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};

pub(crate) const DOCUMENTATION_URL: &str = "https://paneflow.dev/docs";
pub(crate) const RELEASES_URL: &str = "https://paneflow.dev/releases";
pub(crate) const AUTOMATIONS_URL: &str = "https://paneflow.dev/docs/scripting";
pub(crate) const TROUBLESHOOTING_URL: &str = "https://paneflow.dev/docs/troubleshooting";

#[derive(Clone, Copy)]
enum FilesMenuAction {
    NewWorkspace,
    Settings,
}

#[derive(Clone, Copy)]
enum HelpMenuAction {
    Documentation,
    WhatsNew,
    CheckForUpdates,
    Automations,
    Troubleshooting,
    SystemInfo,
    About,
}

fn anchored_menu<A: Clone + 'static>(
    id: &'static str,
    menu: ActionMenu<A>,
    anchor: Point<Pixels>,
    handlers: MenuHandlers<A>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let ui = crate::theme::ui_colors();
    let width = menu.size(window).width;
    let win_size = window.window_bounds().get_bounds().size;
    let desired_left = anchor.x - px(12.);
    let max_left = (win_size.width - width - px(4.)).max(px(4.));
    let left = desired_left.clamp(px(4.), max_left);
    let top = anchor.y + px(4.);
    let panel = menu
        .render(id, handlers, ui, window, cx)
        .occlude()
        .absolute()
        .left(left)
        .top(top);
    deferred(crate::ui_primitives::menu_reveal(
        (ElementId::Name(id.into()), "reveal"),
        panel,
    ))
    .with_priority(4)
    .into_any_element()
}

impl PaneFlowApp {
    pub(crate) fn open_documentation(&mut self, cx: &mut Context<Self>) {
        self.open_help_url(DOCUMENTATION_URL, cx);
    }

    pub(crate) fn open_help_url(&mut self, url: &str, cx: &mut Context<Self>) {
        self.open_external_url(url.to_string(), cx);
    }

    pub(crate) fn render_title_bar_files_menu(
        &self,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = ActionMenu::new()
            .item(
                MenuItem::new("New Workspace", FilesMenuAction::NewWorkspace)
                    .icon(icon::NEW_WORKSPACE)
                    .mnemonic('n'),
            )
            .item(
                MenuItem::new("Settings", FilesMenuAction::Settings)
                    .icon(icon::SETTINGS)
                    .mnemonic('s'),
            );
        let handlers = self.menu_handlers(
            cx,
            |this, action: &FilesMenuAction, window, cx| {
                this.title_bar_files_menu_open = None;
                match action {
                    FilesMenuAction::NewWorkspace => this.create_workspace_with_picker(window, cx),
                    FilesMenuAction::Settings => this.open_settings_window(window, cx),
                }
            },
            |this| this.title_bar_files_menu_open = None,
        );
        anchored_menu("title-bar-files-menu", menu, anchor, handlers, window, cx)
    }

    pub(crate) fn render_title_bar_help_menu(
        &self,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = ActionMenu::new()
            .item(
                MenuItem::new("Paneflow Documentation", HelpMenuAction::Documentation)
                    .icon(icon::DOCUMENTATION)
                    .mnemonic('d'),
            )
            .item(
                MenuItem::new("What's New", HelpMenuAction::WhatsNew)
                    .icon(icon::WHATS_NEW)
                    .mnemonic('w'),
            )
            .item(
                MenuItem::new("Check for Updates…", HelpMenuAction::CheckForUpdates)
                    .icon(icon::UPDATE)
                    .mnemonic('u'),
            )
            .item(
                MenuItem::new("Automations", HelpMenuAction::Automations)
                    .icon(icon::AUTOMATIONS)
                    .mnemonic('a'),
            )
            .item(
                MenuItem::new("Troubleshooting", HelpMenuAction::Troubleshooting)
                    .icon(icon::TROUBLESHOOTING)
                    .mnemonic('t'),
            )
            .item(
                MenuItem::new("System Info…", HelpMenuAction::SystemInfo)
                    .icon(icon::SYSTEM_INFO)
                    .mnemonic('i'),
            )
            .separator()
            .item(
                MenuItem::new("About Paneflow", HelpMenuAction::About)
                    .icon(icon::ABOUT)
                    .mnemonic('b'),
            );
        let handlers = self.menu_handlers(
            cx,
            |this, action: &HelpMenuAction, window, cx| {
                this.title_bar_help_menu_open = None;
                match action {
                    HelpMenuAction::Documentation => this.open_help_url(DOCUMENTATION_URL, cx),
                    HelpMenuAction::WhatsNew => this.open_help_url(RELEASES_URL, cx),
                    HelpMenuAction::CheckForUpdates => this.request_update_check(cx),
                    HelpMenuAction::Automations => this.open_help_url(AUTOMATIONS_URL, cx),
                    HelpMenuAction::Troubleshooting => {
                        this.open_help_url(TROUBLESHOOTING_URL, cx);
                    }
                    HelpMenuAction::SystemInfo => this.open_system_info_dialog(window, cx),
                    HelpMenuAction::About => this.open_about_dialog(window, cx),
                }
            },
            |this| this.title_bar_help_menu_open = None,
        );
        anchored_menu("title-bar-help-menu", menu, anchor, handlers, window, cx)
    }
}

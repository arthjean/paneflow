use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClipboardItem, Context, ElementId, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, Styled, Window, deferred, div, point, px,
};

use crate::app::files_tree;
use crate::app::menu_icons as icon;
use crate::pane::PaneSurface;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::{PaneContextMenu, PaneFlowApp, TabContextMenu, TabKey, WorkspaceContextMenu};

use super::SidebarRow;

pub(crate) struct EditorMenuEntry {
    pub(crate) label: &'static str,
    pub(crate) command: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) mnemonic: char,
}

static INSTALLED_EDITORS: std::sync::OnceLock<Vec<&'static EditorMenuEntry>> =
    std::sync::OnceLock::new();

pub(crate) fn detect_installed_editors() {
    INSTALLED_EDITORS.get_or_init(|| {
        EDITOR_MENU_ENTRIES
            .iter()
            .filter(|entry| crate::app::workspace_ops::find_editor_binary(entry.command).is_some())
            .collect()
    });
}

pub(crate) fn installed_editors() -> &'static [&'static EditorMenuEntry] {
    INSTALLED_EDITORS.get().map_or(&[], Vec::as_slice)
}

pub(crate) const EDITOR_MENU_ENTRIES: &[EditorMenuEntry] = &[
    EditorMenuEntry {
        label: "Open in Zed",
        command: "zed",
        icon: "icons/editor-zed.svg",
        mnemonic: 'z',
    },
    EditorMenuEntry {
        label: "Open in Cursor",
        command: "cursor",
        icon: "icons/editor-cursor.svg",
        mnemonic: 'c',
    },
    EditorMenuEntry {
        label: "Open in VS Code",
        command: "code",
        icon: icon::VSCODE,
        mnemonic: 'v',
    },
    EditorMenuEntry {
        label: "Open in WebStorm",
        command: "webstorm",
        icon: icon::WEBSTORM,
        mnemonic: 'w',
    },
];

const SPACE_DIGIT_MNEMONICS: &[char] = &['1', '2', '3', '4', '5', '6', '7', '8', '9'];

#[derive(Clone)]
pub(crate) enum WorkspaceMenuAction {
    RunWorkflow(usize),
    OpenService(String),
    OpenInEditor {
        command: &'static str,
        label: &'static str,
    },
    Reveal,
    CopyPath,
    CustomButtons,
    MoveToSpace(Option<paneflow_config::schema::SpaceId>),
    MoveToNewSpace,
    MarkRead,
    ToggleMute,
    Close,
}

#[derive(Clone)]
pub(crate) enum TabMenuAction {
    Rename,
    ResetName,
    Close,
    NewBranch,
    BindBranch(String),
    BindWorktree(PathBuf),
    CreateBranchHere(PathBuf),
    RemoveWorktree,
}

#[derive(Clone)]
pub(crate) enum PaneMenuAction {
    Copy { value: String, toast: &'static str },
    Fork,
    ToggleDetached,
    Close,
}

pub(crate) fn clamped_context_menu_position(
    position: gpui::Point<Pixels>,
    width: Pixels,
    height: Pixels,
    window: &Window,
) -> gpui::Point<Pixels> {
    let win_size = window.window_bounds().get_bounds().size;
    let x = if position.x + width > win_size.width {
        (position.x - width).max(px(0.))
    } else {
        position.x
    };
    let y = if position.y + height > win_size.height {
        (position.y - height).max(px(0.))
    } else {
        position.y
    };
    point(x, y)
}

pub(crate) fn render_context_menu<A: Clone + 'static>(
    id: impl Into<ElementId>,
    menu: ActionMenu<A>,
    position: gpui::Point<Pixels>,
    handlers: MenuHandlers<A>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: ElementId = id.into();
    let ui = crate::theme::ui_colors();
    let size = menu.size(window);
    let at = clamped_context_menu_position(position, size.width, size.height, window);
    let panel = menu
        .render(id.clone(), handlers, ui, window, cx)
        .occlude()
        .absolute()
        .left(at.x)
        .top(at.y);
    deferred(crate::ui_primitives::menu_reveal((id, "reveal"), panel))
        .priority(3)
        .into_any_element()
}

pub(crate) fn render_anchored_context_menu<A: Clone + 'static>(
    id: impl Into<ElementId>,
    menu: ActionMenu<A>,
    position: gpui::Point<Pixels>,
    handlers: MenuHandlers<A>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: ElementId = id.into();
    let ui = crate::theme::ui_colors();
    let size = menu.size(window);
    let at = clamped_context_menu_position(position, size.width, size.height, window);
    let panel = menu.render(id.clone(), handlers, ui, window, cx).occlude();
    deferred(
        gpui::anchored()
            .position(at)
            .child(crate::ui_primitives::menu_reveal((id, "reveal"), panel)),
    )
    .priority(3)
    .into_any_element()
}

impl PaneFlowApp {
    pub(crate) fn shortcut_for_action(&self, action_name: &str) -> Option<&str> {
        self.effective_shortcuts
            .iter()
            .find(|entry| entry.action_name == action_name && entry.key != "Unassigned")
            .map(|entry| entry.key.as_str())
    }

    pub(super) fn open_sidebar_row_menu(
        &mut self,
        row: SidebarRow,
        position: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_sidebar_selection_menu(row, position, window, cx) {
            return;
        }
        self.commit_rename(cx);
        self.dismiss_transient_surfaces();
        self.place_sidebar_cursor(row, window, cx);
        match row {
            SidebarRow::Folder(idx) => {
                self.workspace_menu_open = Some(WorkspaceContextMenu::at(idx, position));
            }
            SidebarRow::Tab(ws_idx, tab_idx) => {
                let Some(tab) = self
                    .workspaces
                    .get(ws_idx)
                    .and_then(|ws| TabKey::of(ws, tab_idx))
                else {
                    return;
                };
                self.tab_menu_open = Some(TabContextMenu { tab, position });
                self.spawn_worktree_listing(ws_idx, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn open_workspace_service_url(&mut self, url: &str, cx: &mut Context<Self>) {
        self.open_external_url(url.to_string(), cx);
    }

    pub(crate) fn menu_handlers<A: Clone + 'static>(
        &self,
        cx: &mut Context<Self>,
        activate: impl Fn(&mut Self, &A, &mut Window, &mut Context<Self>) + 'static,
        dismiss: impl Fn(&mut Self) + 'static,
    ) -> MenuHandlers<A> {
        MenuHandlers {
            activate: Rc::new(cx.listener(move |this, action: &A, window, cx| {
                activate(this, action, window, cx);
                cx.notify();
            })),
            dismiss: Rc::new(cx.listener(move |this, _: &(), _, cx| {
                dismiss(this);
                cx.notify();
            })),
        }
    }

    fn workspace_menu(&self, idx: usize) -> ActionMenu<WorkspaceMenuAction> {
        let Some(workspace) = self.workspaces.get(idx) else {
            return ActionMenu::new();
        };
        let services = workspace.active_ports.iter().filter_map(|port| {
            let info = workspace.service_labels.get(port)?;
            let name = info.label.as_deref().unwrap_or("Local service");
            Some(if info.is_frontend {
                let url = info
                    .url
                    .clone()
                    .unwrap_or_else(|| format!("http://localhost:{port}"));
                MenuItem::new(
                    format!("Open {name} :{port}"),
                    WorkspaceMenuAction::OpenService(url),
                )
                .icon(icon::GLOBE)
            } else {
                MenuItem::disabled(format!("{name} :{port}")).icon(icon::SERVER)
            })
        });
        let editors = EDITOR_MENU_ENTRIES.iter().map(|entry| {
            MenuItem::new(
                entry.label,
                WorkspaceMenuAction::OpenInEditor {
                    command: entry.command,
                    label: entry.label,
                },
            )
            .icon(entry.icon)
            .mnemonic(entry.mnemonic)
        });
        let mute = if workspace.muted {
            MenuItem::new("Unmute notifications", WorkspaceMenuAction::ToggleMute)
                .icon(icon::NOTIFICATION)
        } else {
            MenuItem::new("Mute notifications", WorkspaceMenuAction::ToggleMute)
                .icon(icon::NOTIFICATION_OFF)
        };
        let reveal_label = if cfg!(target_os = "windows") {
            "Open in File Explorer"
        } else {
            "Reveal in file manager"
        };
        let unread = workspace.agent_completion_notification.is_unread();
        ActionMenu::new()
            .items(self.workspace_template_for_workspace(idx).map(|template| {
                MenuItem::new("Run workflow", WorkspaceMenuAction::RunWorkflow(template))
                    .icon(icon::PLAY)
                    .mnemonic('r')
            }))
            .separator()
            .items(services)
            .separator()
            .items(editors)
            .item(
                MenuItem::new(reveal_label, WorkspaceMenuAction::Reveal)
                    .icon(icon::FOLDER_OPEN)
                    .mnemonic('e'),
            )
            .separator()
            .item(
                MenuItem::new("Copy path", WorkspaceMenuAction::CopyPath)
                    .icon(icon::COPY)
                    .mnemonic('p'),
            )
            .item(
                MenuItem::new("Manage custom buttons…", WorkspaceMenuAction::CustomButtons)
                    .icon(icon::PREFERENCES)
                    .mnemonic('b'),
            )
            .items((!self.cached_config.spaces.is_empty()).then(|| {
                MenuItem::submenu("Move to space", self.space_menu(idx))
                    .icon(icon::MOVE_TO_SPACE)
                    .mnemonic('s')
            }))
            .separator()
            .items(unread.then(|| {
                MenuItem::new("Mark as read", WorkspaceMenuAction::MarkRead)
                    .icon(icon::MARK_READ)
                    .mnemonic('a')
            }))
            .item(mute.mnemonic('m'))
            .separator()
            .item(MenuItem::new("Close workspace", WorkspaceMenuAction::Close).icon(icon::CLOSE))
    }

    fn space_menu(&self, idx: usize) -> ActionMenu<WorkspaceMenuAction> {
        let current = self.workspaces.get(idx).and_then(|ws| ws.space.clone());
        let digits = SPACE_DIGIT_MNEMONICS
            .iter()
            .copied()
            .map(Some)
            .chain(std::iter::repeat(None));
        let targets = crate::app::spaces::space_order(&self.cached_config)
            .into_iter()
            .filter(|space| *space != current)
            .zip(digits)
            .map(|(space, digit)| {
                let name = crate::app::spaces::space_label(&self.cached_config, space.as_ref())
                    .to_string();
                let item = MenuItem::new(name, WorkspaceMenuAction::MoveToSpace(space));
                match digit {
                    Some(digit) => item.mnemonic(digit),
                    None => item,
                }
            });
        let at_limit = self.cached_config.spaces.len() >= paneflow_config::schema::MAX_SPACES;
        let new_space = if at_limit {
            MenuItem::disabled(crate::app::spaces::SPACE_LIMIT_REASON)
        } else {
            MenuItem::new("New space…", WorkspaceMenuAction::MoveToNewSpace).mnemonic('n')
        };
        ActionMenu::new().items(targets).separator().item(new_space)
    }

    fn tab_menu(&self, key: TabKey) -> Option<ActionMenu<TabMenuAction>> {
        let (ws_idx, tab_idx) = key.resolve(&self.workspaces)?;
        let tab = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx));
        let can_reset_name = tab.is_some_and(|tab| tab.title_is_user_owned());
        let bound = tab.and_then(|tab| tab.worktree.clone());
        let root = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.clone())
            .unwrap_or_default();
        let listing = self.workspace_worktree_listing(ws_idx);
        let on_branch = match bound.as_ref() {
            Some(path) => listing
                .iter()
                .find(|entry| entry.path == *path)
                .and_then(|entry| entry.branch.clone()),
            None => Some(self.workspace_checkout_label(ws_idx)),
        };
        let mut checkouts: Vec<MenuItem<TabMenuAction>> = self
            .workspace_branches(ws_idx)
            .iter()
            .map(|branch| {
                let selected = on_branch.as_deref() == Some(branch.as_str());
                MenuItem::new(branch.clone(), TabMenuAction::BindBranch(branch.clone()))
                    .radio(selected)
            })
            .collect();
        checkouts.extend(
            listing
                .iter()
                .filter(|entry| entry.branch.is_none() && entry.path != root)
                .map(|entry| {
                    let label =
                        crate::workspace::worktree::checkout_label(None, &entry.path, &root);
                    let selected = bound.as_deref() == Some(entry.path.as_path());
                    MenuItem::new(label, TabMenuAction::BindWorktree(entry.path.clone()))
                        .radio(selected)
                }),
        );
        let show_checkouts = checkouts.len() > 1;
        let detached = self.tab_detached_checkout(ws_idx, tab_idx);
        let mut menu = ActionMenu::new()
            .item(
                MenuItem::new("Rename", TabMenuAction::Rename)
                    .icon(icon::RENAME)
                    .mnemonic('r'),
            )
            .items(can_reset_name.then(|| {
                MenuItem::new("Reset name", TabMenuAction::ResetName)
                    .icon(icon::RESET_NAME)
                    .mnemonic('n')
            }))
            .item(
                MenuItem::new("Close tab (stops its sessions)", TabMenuAction::Close)
                    .icon(icon::CLOSE),
            );
        if show_checkouts {
            menu = menu
                .separator()
                .heading("Branch")
                .item(
                    MenuItem::new("New branch…", TabMenuAction::NewBranch)
                        .icon(icon::ADD)
                        .mnemonic('b'),
                )
                .items(checkouts);
        }
        if let Some(path) = detached {
            menu = menu.separator().item(
                MenuItem::new("Create branch here…", TabMenuAction::CreateBranchHere(path))
                    .icon(icon::GIT_BRANCH)
                    .mnemonic('h'),
            );
        }
        if bound.is_some() {
            menu = menu.separator().item(
                MenuItem::new("Remove worktree", TabMenuAction::RemoveWorktree).icon(icon::TRASH),
            );
        }
        Some(menu)
    }

    fn run_workspace_menu_action(
        &mut self,
        idx: usize,
        action: &WorkspaceMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace_menu_open = None;
        match action {
            WorkspaceMenuAction::RunWorkflow(template) => {
                self.run_saved_workspace_template_for_workspace(idx, *template, cx);
            }
            WorkspaceMenuAction::OpenService(url) => self.open_workspace_service_url(url, cx),
            WorkspaceMenuAction::OpenInEditor { command, label } => {
                self.open_workspace_in_editor(idx, command, label, cx);
            }
            WorkspaceMenuAction::Reveal => self.reveal_workspace_in_file_manager(idx, cx),
            WorkspaceMenuAction::CopyPath => self.copy_workspace_path(idx, cx),
            WorkspaceMenuAction::CustomButtons => self.open_custom_buttons_modal(idx, window, cx),
            WorkspaceMenuAction::MoveToSpace(space) => {
                self.move_workspace_to_space(idx, space.clone(), window, cx);
            }
            WorkspaceMenuAction::MoveToNewSpace => {
                self.move_workspace_to_new_space(idx, window, cx);
            }
            WorkspaceMenuAction::MarkRead => self.mark_workspace_read(idx, cx),
            WorkspaceMenuAction::ToggleMute => self.toggle_workspace_muted(idx, cx),
            WorkspaceMenuAction::Close => self.close_workspace_at(idx, window, cx),
        }
    }

    fn run_tab_menu_action(
        &mut self,
        key: TabKey,
        action: &TabMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_menu_open = None;
        let Some((ws_idx, tab_idx)) = key.resolve(&self.workspaces) else {
            return;
        };
        match action {
            TabMenuAction::Rename => self.begin_tab_rename(ws_idx, tab_idx, cx),
            TabMenuAction::ResetName => self.reset_tab_name(ws_idx, tab_idx, cx),
            TabMenuAction::Close => self.close_workspace_tab(ws_idx, tab_idx, window, cx),
            TabMenuAction::NewBranch => self.open_new_branch_for_tab(ws_idx, tab_idx, window, cx),
            TabMenuAction::BindBranch(branch) => {
                self.bind_tab_to_branch(ws_idx, tab_idx, branch.clone(), cx);
            }
            TabMenuAction::BindWorktree(path) => {
                self.bind_tab_to_checkout(ws_idx, tab_idx, path.clone(), cx);
            }
            TabMenuAction::CreateBranchHere(path) => {
                self.open_branch_prompt(ws_idx, path.clone(), window, cx);
            }
            TabMenuAction::RemoveWorktree => self.remove_tab_worktree(ws_idx, tab_idx, cx),
        }
    }

    pub(crate) fn render_workspace_context_menu(
        &self,
        menu: WorkspaceContextMenu,
        _ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = menu.idx;
        let handlers = self.menu_handlers(
            cx,
            move |this, action, window, cx| this.run_workspace_menu_action(idx, action, window, cx),
            |this| this.workspace_menu_open = None,
        );
        render_context_menu(
            SharedString::from(format!("workspace-context-menu-{idx}")),
            self.workspace_menu(idx),
            menu.position,
            handlers,
            window,
            cx,
        )
    }

    pub(crate) fn render_tab_context_menu(
        &self,
        menu: TabContextMenu,
        _ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = menu.tab;
        let Some(actions) = self.tab_menu(key) else {
            return div().into_any_element();
        };
        let handlers = self.menu_handlers(
            cx,
            move |this, action, window, cx| this.run_tab_menu_action(key, action, window, cx),
            |this| this.tab_menu_open = None,
        );
        render_context_menu(
            SharedString::from(format!("tab-context-menu-{}", key.tab_id)),
            actions,
            menu.position,
            handlers,
            window,
            cx,
        )
    }

    fn pane_menu(&self, menu: &PaneContextMenu, cx: &App) -> Option<ActionMenu<PaneMenuAction>> {
        let source = menu.pane.upgrade()?;
        let pane = source.read(cx);
        let workspace_cwd: Option<PathBuf> = self
            .workspaces
            .iter()
            .find(|ws| ws.id == pane.workspace_id)
            .map(|ws| PathBuf::from(&ws.cwd));
        let surface_path = Self::surface_context_path(pane.surface(), cx);
        let full_path = surface_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let relative_path = surface_path.as_ref().map(|path| {
            workspace_cwd
                .as_ref()
                .map(|root| files_tree::workspace_relative_path(root, path))
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        });
        let can_fork = pane
            .surface()
            .as_terminal()
            .is_some_and(|terminal| terminal.read(cx).can_fork_conversation());
        let copy_path = match full_path {
            Some(value) => MenuItem::new(
                "Copy path",
                PaneMenuAction::Copy {
                    value,
                    toast: "Copied path",
                },
            ),
            None => MenuItem::disabled("Copy path unavailable"),
        };
        let copy_relative = match relative_path {
            Some(value) => MenuItem::new(
                "Copy relative path",
                PaneMenuAction::Copy {
                    value,
                    toast: "Copied relative path",
                },
            ),
            None => MenuItem::disabled("Copy relative path unavailable"),
        };
        let detach = if pane.is_detached() {
            MenuItem::new("Return to workspace", PaneMenuAction::ToggleDetached).icon(icon::DOCK)
        } else {
            MenuItem::new("Detach into window", PaneMenuAction::ToggleDetached).icon(icon::DETACH)
        };
        Some(
            ActionMenu::new()
                .item(copy_path.icon(icon::COPY).mnemonic('p'))
                .item(copy_relative.icon(icon::LINK).mnemonic('l'))
                .items(can_fork.then(|| {
                    MenuItem::new("Fork conversation", PaneMenuAction::Fork)
                        .icon(icon::FORK)
                        .mnemonic('f')
                        .tooltip(crate::app::conversation_restore::FORK_TOOLTIP)
                }))
                .separator()
                .item(detach.mnemonic('d'))
                .item(
                    MenuItem::new("Close pane (stops its sessions)", PaneMenuAction::Close)
                        .icon(icon::CLOSE),
                ),
        )
    }

    fn run_pane_menu_action(
        &mut self,
        pane: &gpui::WeakEntity<crate::pane::Pane>,
        action: &PaneMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_menu_open = None;
        let Some(source) = pane.upgrade() else {
            return;
        };
        match action {
            PaneMenuAction::Copy { value, toast } => {
                cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                self.show_toast(*toast, cx);
            }
            PaneMenuAction::Fork => self.fork_conversation_in(source, window, cx),
            PaneMenuAction::ToggleDetached => self.toggle_detached_pane(source, window, cx),
            PaneMenuAction::Close => {
                source.update(cx, |pane, pane_cx| pane.close(pane_cx));
                self.save_session(cx);
            }
        }
    }

    pub(crate) fn render_pane_context_menu(
        &self,
        menu: PaneContextMenu,
        _ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(actions) = self.pane_menu(&menu, cx) else {
            return div().into_any_element();
        };
        let pane = menu.pane.clone();
        let pane_id = pane.entity_id();
        let handlers = self.menu_handlers(
            cx,
            move |this, action, window, cx| this.run_pane_menu_action(&pane, action, window, cx),
            |this| this.pane_menu_open = None,
        );
        render_context_menu(
            SharedString::from(format!("pane-context-menu-{pane_id}")),
            actions,
            menu.position,
            handlers,
            window,
            cx,
        )
    }

    fn surface_context_path(surface: &PaneSurface, cx: &App) -> Option<PathBuf> {
        match surface {
            PaneSurface::Terminal(terminal) => terminal
                .read(cx)
                .terminal
                .current_cwd
                .as_ref()
                .filter(|cwd| !cwd.is_empty())
                .map(PathBuf::from),
            PaneSurface::Markdown(markdown) => Some(markdown.read(cx).path.clone()),
        }
    }
}

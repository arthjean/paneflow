use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Pixels, Point, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
    svg,
};

use crate::PaneFlowApp;
use crate::app::files_tree::{FileNode, node_name, read_dir_listing};
use crate::theme::UiColors;
use crate::ui_primitives::action_menu::{ActionMenu, MenuGlyph, MenuItem};
use crate::ui_primitives::{AccessibleControlExt, AnimatedHoverExt};

const CRUMB_MENU_MAX_ENTRIES: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Crumb {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
    pub(crate) is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CrumbListing {
    Loading,
    Ready(Vec<FileNode>),
    Missing,
}

#[derive(Clone, Debug)]
pub(crate) struct CrumbMenu {
    pub(crate) ignore_root: PathBuf,
    pub(crate) stack: Vec<PathBuf>,
    pub(crate) current: PathBuf,
    pub(crate) position: Point<Pixels>,
    pub(crate) listing: CrumbListing,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CrumbAction {
    Back,
    Enter(PathBuf),
    Open(PathBuf),
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

pub(crate) fn crumb_trail(root: &Path, file: Option<&Path>) -> Vec<Crumb> {
    let Some(file) = file else {
        return if root.as_os_str().is_empty() {
            Vec::new()
        } else {
            vec![Crumb {
                label: project_name(root),
                path: root.to_path_buf(),
                is_dir: true,
            }]
        };
    };
    let (base, folders) = match file.strip_prefix(root) {
        Ok(rel) if !root.as_os_str().is_empty() => (Some(root.to_path_buf()), rel.to_path_buf()),
        _ => (None, file.to_path_buf()),
    };
    let mut trail = Vec::new();
    let mut cursor = match &base {
        Some(root) => {
            trail.push(Crumb {
                label: project_name(root),
                path: root.clone(),
                is_dir: true,
            });
            root.clone()
        }
        None => PathBuf::new(),
    };
    let components: Vec<_> = folders.components().collect();
    for (index, component) in components.iter().enumerate() {
        cursor.push(component.as_os_str());
        let label = match component {
            std::path::Component::Normal(name) => name.to_string_lossy().into_owned(),
            std::path::Component::CurDir
            | std::path::Component::ParentDir
            | std::path::Component::Prefix(_) => continue,
            _ => cursor.display().to_string(),
        };
        trail.push(Crumb {
            label,
            path: cursor.clone(),
            is_dir: index + 1 < components.len(),
        });
    }
    trail.retain(|crumb| !crumb.label.is_empty());
    trail
}

pub(super) fn render_tree_toggle(
    open: bool,
    ui: UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let hover = crate::app::constants::sidebar_tab_hover_background();
    div()
        .id("diff-dock-files-toggle")
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .accessible_control(
            gpui::accesskit::Role::Button,
            if open {
                "Hide file tree"
            } else {
                "Show file tree"
            },
        )
        .animated_hover_bg(
            if open {
                hover
            } else {
                gpui::transparent_black()
            },
            hover,
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.handle_toggle_files_sidebar(&crate::ToggleFilesSidebar, window, cx);
            cx.stop_propagation();
        }))
        .child(
            svg()
                .path("icons/dock/folder-tree.svg")
                .size(px(16.))
                .text_color(if open { ui.text } else { ui.muted }),
        )
        .into_any_element()
}

pub(super) fn render_file_breadcrumbs(
    trail: Vec<Crumb>,
    ignore_root: PathBuf,
    menu_dir: Option<PathBuf>,
    scroll: &ScrollHandle,
    ui: UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let count = trail.len();
    let current = trail.last().map(|crumb| crumb.path.clone());
    let mut row = div()
        .id("diff-dock-breadcrumbs")
        .flex()
        .flex_row()
        .items_center()
        .h_full()
        .gap(px(2.))
        .overflow_x_scroll()
        .track_scroll(scroll);
    row.style().restrict_scroll_to_axis = Some(true);
    for (index, crumb) in trail.into_iter().enumerate() {
        if index > 0 {
            let dir = crumb
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let open = menu_dir.as_deref() == Some(dir.as_path());
            let ignore_root = ignore_root.clone();
            let marked = current.clone().unwrap_or_default();
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "diff-dock-crumb-chevron-{index}"
                    )))
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .accessible_control(gpui::accesskit::Role::Button, "Show folder contents")
                    .animated_hover_bg(
                        if open {
                            crate::app::constants::sidebar_tab_hover_background()
                        } else {
                            gpui::transparent_black()
                        },
                        crate::app::constants::sidebar_tab_hover_background(),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        let position = event.position();
                        this.open_crumb_menu(
                            ignore_root.clone(),
                            dir.clone(),
                            marked.clone(),
                            gpui::point(position.x, position.y + px(12.)),
                            cx,
                        );
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .path("icons/dock/arrow-right-01.svg")
                            .size(px(12.))
                            .text_color(ui.muted),
                    ),
            );
        }
        let last = index + 1 == count;
        let label = div()
            .id(SharedString::from(format!("diff-dock-crumb-{index}")))
            .flex_none()
            .px(px(4.))
            .h(px(22.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .whitespace_nowrap()
            .text_color(if last { ui.text } else { ui.muted })
            .child(crumb.label.clone());
        let label = if crumb.is_dir {
            let path = crumb.path.clone();
            label
                .cursor_pointer()
                .accessible_control(gpui::accesskit::Role::Link, crumb.label.clone())
                .animated_hover_bg(
                    gpui::transparent_black(),
                    crate::app::constants::sidebar_tab_hover_background(),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.reveal_folder_in_tree(path.clone(), window, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        } else {
            label.into_any_element()
        };
        row = row.child(label);
    }
    div()
        .flex_1()
        .min_w_0()
        .h(px(40.))
        .flex()
        .items_center()
        .px(px(12.))
        .overflow_hidden()
        .text_size(px(12.))
        .text_color(ui.muted)
        .child(row)
        .into_any_element()
}

impl PaneFlowApp {
    pub(crate) fn dock_tree_root(&self) -> PathBuf {
        self.files_sidebar_root
            .clone()
            .or_else(|| self.active_workspace().map(|ws| PathBuf::from(&ws.cwd)))
            .unwrap_or_default()
    }

    pub(super) fn render_dock_crumbs(
        &mut self,
        file: Option<&Path>,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let root = self.dock_tree_root();
        let trail = crumb_trail(&root, file);
        if std::mem::take(&mut self.diff_dock.crumb_scroll_pending) && !trail.is_empty() {
            self.diff_dock
                .crumb_scroll
                .scroll_to_item(2 * (trail.len() - 1));
        }
        let menu_dir = self
            .diff_dock
            .crumb_menu
            .as_ref()
            .and_then(|menu| menu.stack.first().cloned());
        let scroll = self.diff_dock.crumb_scroll.clone();
        render_file_breadcrumbs(trail, root, menu_dir, &scroll, ui, cx)
    }

    pub(crate) fn reveal_folder_in_tree(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.files_sidebar_open {
            self.toggle_files_sidebar(cx);
        }
        if !self.files_sidebar_open {
            return;
        }
        self.files_sidebar
            .update(cx, |panel, cx| panel.reveal_dir(&path, cx));
        self.focus_files_sidebar(window, cx);
    }

    pub(crate) fn open_crumb_menu(
        &mut self,
        ignore_root: PathBuf,
        dir: PathBuf,
        current: PathBuf,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transient_surfaces();
        self.diff_dock.tab_menu = None;
        self.diff_dock.crumb_menu = Some(CrumbMenu {
            ignore_root,
            stack: vec![dir],
            current,
            position,
            listing: CrumbListing::Loading,
            generation: 0,
        });
        self.load_crumb_listing(cx);
    }

    fn load_crumb_listing(&mut self, cx: &mut Context<Self>) {
        self.diff_dock.crumb_generation = self.diff_dock.crumb_generation.wrapping_add(1);
        let generation = self.diff_dock.crumb_generation;
        let Some(menu) = self.diff_dock.crumb_menu.as_mut() else {
            return;
        };
        menu.generation = generation;
        menu.listing = CrumbListing::Loading;
        let Some(dir) = menu.stack.last().cloned() else {
            return;
        };
        let root = if dir.starts_with(&menu.ignore_root) {
            menu.ignore_root.clone()
        } else {
            dir.clone()
        };
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let listing = smol::unblock(move || read_dir_listing(&root, &dir)).await;
                let _ = this.update(cx, |app, cx| {
                    app.apply_crumb_listing(generation, listing, cx);
                });
            },
        )
        .detach();
    }

    pub(crate) fn apply_crumb_listing(
        &mut self,
        generation: u64,
        listing: Option<Vec<FileNode>>,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.diff_dock.crumb_menu.as_mut() else {
            return;
        };
        if menu.generation != generation {
            return;
        }
        menu.listing = match listing {
            Some(nodes) => CrumbListing::Ready(nodes),
            None => CrumbListing::Missing,
        };
        cx.notify();
    }

    fn enter_crumb_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if let Some(menu) = self.diff_dock.crumb_menu.as_mut() {
            menu.stack.push(dir);
        }
        self.load_crumb_listing(cx);
    }

    fn leave_crumb_dir(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.diff_dock.crumb_menu.as_mut()
            && menu.stack.len() > 1
        {
            menu.stack.pop();
        }
        self.load_crumb_listing(cx);
    }

    pub(crate) fn render_crumb_menu(
        &self,
        menu: &CrumbMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let actions = crumb_menu_items(menu, ui);
        let handlers = self.menu_handlers(
            cx,
            |this, action: &CrumbAction, window, cx| match action {
                CrumbAction::Back => this.leave_crumb_dir(cx),
                CrumbAction::Enter(dir) => this.enter_crumb_dir(dir.clone(), cx),
                CrumbAction::Open(path) => {
                    this.diff_dock.crumb_menu = None;
                    this.open_diff_file_tab(path.clone(), window, cx);
                }
            },
            |this| this.diff_dock.crumb_menu = None,
        );
        crate::app::sidebar::context_menu::render_context_menu(
            SharedString::from("diff-dock-crumb-menu"),
            actions,
            menu.position,
            handlers,
            window,
            cx,
        )
    }
}

fn crumb_menu_items(menu: &CrumbMenu, ui: UiColors) -> ActionMenu<CrumbAction> {
    let mut actions = ActionMenu::new();
    if menu.stack.len() > 1
        && let Some(previous) = menu.stack.get(menu.stack.len() - 2)
    {
        actions = actions
            .item(
                MenuItem::new(
                    format!("Back to {}", project_name(previous)),
                    CrumbAction::Back,
                )
                .icon("icons/palette/arrow-left-02.svg")
                .keep_open(),
            )
            .separator();
    }
    match &menu.listing {
        CrumbListing::Loading => actions.item(MenuItem::disabled("Loading folder…")),
        CrumbListing::Missing => {
            actions.item(MenuItem::disabled("This folder is no longer available."))
        }
        CrumbListing::Ready(nodes) if nodes.is_empty() => {
            actions.item(MenuItem::disabled("This folder is empty."))
        }
        CrumbListing::Ready(nodes) => {
            for node in nodes.iter().take(CRUMB_MENU_MAX_ENTRIES) {
                let name = node_name(node);
                let marked = menu.current.starts_with(&node.path);
                let item = if node.is_dir {
                    MenuItem::new(name.clone(), CrumbAction::Enter(node.path.clone()))
                        .icon(crate::app::menu_icons::FOLDER_OPEN)
                        .keep_open()
                } else {
                    let icon = crate::file_icons::language_icon_path(&name);
                    MenuItem::new(name.clone(), CrumbAction::Open(node.path.clone())).glyph(Some(
                        MenuGlyph::Element(Rc::new(move || {
                            super::render::file_icon_element(icon, px(14.), ui.muted)
                        })),
                    ))
                };
                actions = actions.item(item.toggle(marked));
            }
            if nodes.len() > CRUMB_MENU_MAX_ENTRIES {
                actions = actions.item(MenuItem::disabled(format!(
                    "{} more",
                    crate::app::plural(nodes.len() - CRUMB_MENU_MAX_ENTRIES, "item", "items")
                )));
            }
            actions
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(trail: &[Crumb]) -> Vec<&str> {
        trail.iter().map(|crumb| crumb.label.as_str()).collect()
    }

    #[test]
    fn a_file_under_the_root_lists_the_project_then_its_folders() {
        let root = Path::new("/home/me/repo");
        let trail = crumb_trail(root, Some(Path::new("/home/me/repo/src/app/main.rs")));
        assert_eq!(labels(&trail), ["repo", "src", "app", "main.rs"]);
        assert_eq!(trail[0].path, root);
        assert_eq!(trail[2].path, Path::new("/home/me/repo/src/app"));
        assert!(trail[..3].iter().all(|crumb| crumb.is_dir));
        assert!(!trail[3].is_dir);
    }

    #[test]
    fn a_file_outside_the_root_lists_its_absolute_folders() {
        let trail = crumb_trail(Path::new("/repo"), Some(Path::new("/etc/ssh/config")));
        assert_eq!(
            trail.last().map(|crumb| crumb.label.as_str()),
            Some("config")
        );
        assert!(
            trail
                .iter()
                .any(|crumb| crumb.label == "ssh" && crumb.is_dir)
        );
        assert_eq!(
            trail
                .iter()
                .find(|crumb| crumb.label == "ssh")
                .map(|crumb| crumb.path.clone()),
            Some(PathBuf::from("/etc/ssh"))
        );
    }

    #[test]
    fn the_files_tab_shows_only_the_project() {
        let trail = crumb_trail(Path::new("/repo"), None);
        assert_eq!(labels(&trail), ["repo"]);
        assert!(trail[0].is_dir);
        assert!(crumb_trail(Path::new(""), None).is_empty());
    }

    fn menu(listing: CrumbListing, stack: Vec<PathBuf>) -> CrumbMenu {
        CrumbMenu {
            ignore_root: PathBuf::from("/repo"),
            stack,
            current: PathBuf::from("/repo/src/main.rs"),
            position: gpui::point(px(0.), px(0.)),
            listing,
            generation: 1,
        }
    }

    fn node(path: &str, is_dir: bool) -> FileNode {
        FileNode {
            path: PathBuf::from(path),
            is_dir,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        }
    }

    fn entry_labels(menu: &ActionMenu<CrumbAction>) -> Vec<String> {
        menu.entries()
            .iter()
            .filter_map(|entry| match entry {
                crate::ui_primitives::action_menu::MenuEntry::Item(item) => {
                    Some(item.label_for_test().to_string())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_folder_menu_states_its_loading_empty_and_missing_states() {
        let ui = crate::theme::ui_colors_with(&crate::theme::paneflow_dark());
        let root = vec![PathBuf::from("/repo/src")];
        assert_eq!(
            entry_labels(&crumb_menu_items(
                &menu(CrumbListing::Loading, root.clone()),
                ui
            )),
            ["Loading folder…"]
        );
        assert_eq!(
            entry_labels(&crumb_menu_items(
                &menu(CrumbListing::Ready(Vec::new()), root.clone()),
                ui
            )),
            ["This folder is empty."]
        );
        assert_eq!(
            entry_labels(&crumb_menu_items(&menu(CrumbListing::Missing, root), ui)),
            ["This folder is no longer available."]
        );
    }

    #[test]
    fn a_nested_folder_menu_offers_the_way_back_and_lists_its_entries() {
        let ui = crate::theme::ui_colors_with(&crate::theme::paneflow_dark());
        let listing = CrumbListing::Ready(vec![
            node("/repo/src/app", true),
            node("/repo/src/main.rs", false),
        ]);
        let nested = menu(
            listing,
            vec![PathBuf::from("/repo/src"), PathBuf::from("/repo/src/app")],
        );
        let items = crumb_menu_items(&nested, ui);
        assert_eq!(entry_labels(&items), ["Back to src", "app", "main.rs"]);
        assert_eq!(items.action_at(0), Some(CrumbAction::Back));
        assert_eq!(
            items.action_at(2),
            Some(CrumbAction::Enter(PathBuf::from("/repo/src/app")))
        );
        assert_eq!(
            items.action_at(3),
            Some(CrumbAction::Open(PathBuf::from("/repo/src/main.rs")))
        );
        assert!(
            items.checked_at_for_test(3),
            "the open file is marked in its folder"
        );
        assert!(!items.checked_at_for_test(2));
    }
}

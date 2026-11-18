use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, CursorStyle, DispatchPhase, Entity, InteractiveElement, IntoElement,
    MouseButton, MouseMoveEvent, MouseUpEvent, Orientation, ParentElement,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, canvas, div,
    prelude::FluentBuilder, px,
};

use super::{FILES_TREE_MAX_WIDTH, FILES_TREE_MIN_WIDTH, FilesEvent, FilesSidebar, FilesTreeStep};
use crate::PaneFlowApp;
use crate::settings::components::with_alpha;

const FILES_TREE_HANDLE_HIT: f32 = 7.;
const FILES_TREE_HANDLE_GROUP: &str = "files-tree-resize-group";

impl PaneFlowApp {
    pub(crate) fn handle_files_event(
        &mut self,
        _: Entity<FilesSidebar>,
        event: &FilesEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            FilesEvent::Close(window) => {
                self.close_files_sidebar(cx);
                let host = cx.weak_entity();
                let window = *window;
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = host.update(cx, |app, cx| {
                            app.focus_diff_tab(app.diff_dock.diff_active_tab, window, cx);
                        });
                    });
                });
            }
            FilesEvent::Toast(message) => self.show_toast(message.clone(), cx),
            FilesEvent::ContextMenu(menu) => {
                self.dismiss_transient_surfaces();
                self.files_menu_open = Some(menu.clone());
                cx.notify();
            }
            FilesEvent::Expanded { root, paths } => {
                if let Some(ws) = self.active_workspace_mut()
                    && Path::new(&ws.cwd) == root
                    && ws.files_expanded != *paths
                {
                    ws.files_expanded = paths.clone();
                    self.save_session(cx);
                }
            }
            FilesEvent::OpenFile { path, root, window } => {
                let host = cx.weak_entity();
                let path = path.clone();
                let root = root.clone();
                let window = *window;
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = host.update(cx, |app, cx| {
                            app.open_file_in_diff_dock(path, root, window, cx)
                        });
                    });
                });
            }
        }
    }

    pub(crate) fn open_file_in_diff_dock(
        &mut self,
        path: PathBuf,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section.is_some() {
            self.close_settings(cx);
        }
        if !self.diff_dock.open {
            self.open_diff_dock_panel(root.to_string_lossy().into_owned(), cx);
        }
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        self.open_diff_file_tab(path, window, cx);
    }

    pub(crate) fn render_files_sidebar_fill(&self) -> AnyElement {
        self.files_sidebar
            .clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element()
    }

    pub(crate) fn render_files_tree_column(
        &self,
        width: f32,
        progress: f32,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .relative()
            .flex_none()
            .w(px(width * progress))
            .h_full()
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .opacity(progress)
                    .child(self.render_files_sidebar((width - 1.).max(0.))),
            )
            .child(self.render_files_tree_handle(width, ui, window, cx))
            .into_any_element()
    }

    fn render_files_tree_handle(
        &self,
        width: f32,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.files_tree_resize_focus.is_focused(window);
        let dragging = self.files_tree_resize.is_some();
        let accent = with_alpha(ui.text, 0.35);
        let line = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px((FILES_TREE_HANDLE_HIT - 1.) / 2.))
            .w(px(1.))
            .bg(if focused || dragging {
                accent
            } else {
                ui.border
            })
            .group_hover(FILES_TREE_HANDLE_GROUP, move |style| style.bg(accent));
        let focus = self.files_tree_resize_focus.clone();
        let app = cx.weak_entity();
        div()
            .id("files-tree-resize")
            .group(FILES_TREE_HANDLE_GROUP)
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-((FILES_TREE_HANDLE_HIT - 1.) / 2.)))
            .w(px(FILES_TREE_HANDLE_HIT))
            .cursor(CursorStyle::ResizeLeftRight)
            .track_focus(&self.files_tree_resize_focus)
            .role(gpui::accesskit::Role::Splitter)
            .aria_label("Resize the file tree")
            .aria_orientation(Orientation::Vertical)
            .aria_numeric_value(f64::from(width))
            .aria_min_numeric_value(f64::from(FILES_TREE_MIN_WIDTH))
            .aria_max_numeric_value(f64::from(FILES_TREE_MAX_WIDTH))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    focus.focus(window, cx);
                    this.begin_files_tree_resize(f32::from(event.position.x), width, cx);
                }),
            )
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if let Some(step) = FilesTreeStep::from_key(event.keystroke.key.as_str()) {
                    cx.stop_propagation();
                    this.step_files_tree_width(step, width, cx);
                }
            }))
            .child(line)
            .when(dragging, |handle| {
                handle.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let moved = app.clone();
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, _window, cx| {
                                    if phase != DispatchPhase::Capture {
                                        return;
                                    }
                                    let _ = moved.update(cx, |this, cx| {
                                        if event.pressed_button == Some(MouseButton::Left) {
                                            this.drag_files_tree_resize(
                                                f32::from(event.position.x),
                                                cx,
                                            );
                                        } else {
                                            this.end_files_tree_resize(cx);
                                        }
                                    });
                                },
                            );
                            let released = app.clone();
                            window.on_mouse_event(
                                move |event: &MouseUpEvent, phase, _window, cx| {
                                    if phase == DispatchPhase::Capture
                                        && event.button == MouseButton::Left
                                    {
                                        let _ = released.update(cx, |this, cx| {
                                            this.end_files_tree_resize(cx);
                                        });
                                    }
                                },
                            );
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .into_any_element()
    }

    pub(crate) fn render_files_sidebar(&self, width: f32) -> AnyElement {
        self.files_sidebar
            .clone()
            .cached(
                StyleRefinement::default()
                    .w(px(width))
                    .h_full()
                    .flex_shrink_0(),
            )
            .into_any_element()
    }
}

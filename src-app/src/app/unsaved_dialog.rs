use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Pixels, SharedString, Window, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::app::close_policy::{CloseTarget, STALE_CLOSE_MESSAGE, close_target_tabs};
use crate::app::diff_dock::code::view::CodeView;
use crate::settings::components::{
    ModalKey, confirmation_list, confirmation_warning, modal_backdrop, modal_card, modal_footer,
    modal_header, modal_key, secondary_button, solid_button, switch_blue,
};

const DIALOG_WIDTH: Pixels = px(460.);
const CARD_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;
const MAX_LISTED_FILES: usize = 6;

#[derive(Clone)]
pub(crate) enum UnsavedContinuation {
    Close(CloseTarget),
    Quit,
    UpdateRestart,
}

impl UnsavedContinuation {
    fn question(&self) -> &'static str {
        match self {
            Self::Close(CloseTarget::Workspace(_)) => "Save changes before closing this workspace?",
            Self::Close(_) => "Save changes before closing this tab?",
            Self::Quit => "Save changes before quitting?",
            Self::UpdateRestart => "Save changes before restarting to update?",
        }
    }
}

pub(crate) struct UnsavedDialog {
    files: Vec<Entity<CodeView>>,
    then: UnsavedContinuation,
    tabs: Vec<u64>,
    saving: bool,
    focused: bool,
    return_focus: crate::FocusReturn,
}

pub(crate) fn unsaved_summary(count: usize) -> String {
    format!(
        "You have unsaved changes in {}.",
        super::plural(count, "file", "files")
    )
}

pub(crate) fn unsaved_close_error(names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| {
        format!(
            "Workspace has unsaved changes in {}: {}",
            super::plural(names.len(), "file", "files"),
            names.join(", ")
        )
    })
}

pub(crate) fn file_rows(
    views: &[Entity<CodeView>],
    cx: &gpui::App,
) -> Vec<(SharedString, SharedString)> {
    views
        .iter()
        .map(|view| {
            let path = view.read(cx).path();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let folder = path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default();
            (SharedString::from(name), SharedString::from(folder))
        })
        .collect()
}

impl PaneFlowApp {
    pub(crate) fn unsaved_views_for_close(
        &self,
        target: &CloseTarget,
        cx: &gpui::App,
    ) -> Vec<Entity<CodeView>> {
        close_target_tabs(&self.workspaces, target)
            .into_iter()
            .flat_map(|id| self.unsaved_views_of_tab(id, cx))
            .collect()
    }

    pub(crate) fn ask_about_unsaved(
        &mut self,
        files: Vec<Entity<CodeView>>,
        then: UnsavedContinuation,
        cx: &mut Context<Self>,
    ) -> bool {
        if files.is_empty() {
            return false;
        }
        if self.unsaved_dialog.is_none() {
            self.dismiss_transient_surfaces();
            let tabs = match &then {
                UnsavedContinuation::Close(target) => close_target_tabs(&self.workspaces, target),
                _ => Vec::new(),
            };
            self.unsaved_dialog = Some(UnsavedDialog {
                files,
                then,
                tabs,
                saving: false,
                focused: false,
                return_focus: crate::FocusReturn::default(),
            });
            cx.notify();
        }
        true
    }

    pub(crate) fn close_unsaved_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.unsaved_dialog.take() {
            self.return_focus(&dialog.return_focus, window, cx);
            cx.notify();
        }
    }

    fn continue_after_unsaved(
        &mut self,
        then: UnsavedContinuation,
        tabs: Vec<u64>,
        discard: Vec<Entity<CodeView>>,
        origin: &crate::FocusReturn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.return_focus(origin, window, cx);
        match then {
            UnsavedContinuation::Close(target) => {
                if close_target_tabs(&self.workspaces, &target) != tabs {
                    self.show_toast(STALE_CLOSE_MESSAGE, cx);
                    return;
                }
                self.request_close_checked(target, discard, Some(window), cx)
            }
            UnsavedContinuation::Quit => self.request_quit_checked(cx),
            UnsavedContinuation::UpdateRestart => self.request_update_restart_checked(cx),
        }
    }

    fn discard_unsaved_and_continue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.unsaved_dialog.take() else {
            return;
        };
        if dialog.saving {
            self.unsaved_dialog = Some(dialog);
            return;
        }
        self.continue_after_unsaved(
            dialog.then,
            dialog.tabs,
            dialog.files,
            &dialog.return_focus,
            window,
            cx,
        );
        cx.notify();
    }

    fn save_unsaved_and_continue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.unsaved_dialog.as_mut() else {
            return;
        };
        if dialog.saving {
            return;
        }
        dialog.saving = true;
        let files = dialog.files.clone();
        let saves: Vec<_> = files
            .iter()
            .map(|view| view.update(cx, |view, cx| view.save_for_close(cx)))
            .collect();
        let window_handle = window.window_handle();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut failures = Vec::new();
                for save in saves {
                    if let Err(message) = save.await {
                        failures.push(message);
                    }
                }
                let _ = window_handle.update(cx, |_, window, cx| {
                    let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let Some(dialog) = app.unsaved_dialog.take() else {
                            return;
                        };
                        if let Some(first) = failures.first() {
                            app.return_focus(&dialog.return_focus, window, cx);
                            app.show_toast(format!("Nothing was closed: {first}"), cx);
                            cx.notify();
                            return;
                        }
                        app.continue_after_unsaved(
                            dialog.then,
                            dialog.tabs,
                            Vec::new(),
                            &dialog.return_focus,
                            window,
                            cx,
                        );
                        cx.notify();
                    });
                });
            },
        )
        .detach();
        cx.notify();
    }

    fn handle_unsaved_dialog_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.unsaved_dialog.is_none() {
            return;
        }
        match modal_key(event) {
            Some(ModalKey::Dismiss) => self.close_unsaved_dialog(window, cx),
            Some(ModalKey::Confirm) => self.save_unsaved_and_continue(window, cx),
            None => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_unsaved_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(dialog) = self.unsaved_dialog.as_mut() else {
            return div().into_any_element();
        };
        if !dialog.focused {
            dialog.focused = true;
            dialog.return_focus.capture_once(window, cx);
            self.unsaved_dialog_focus.focus(window, cx);
        }
        let Some(dialog) = self.unsaved_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();

        let header = modal_header(
            ui,
            dialog.then.question(),
            unsaved_summary(dialog.files.len()),
        );
        let list = confirmation_list(ui, file_rows(&dialog.files, cx), MAX_LISTED_FILES);
        let explanation = confirmation_warning(
            ui,
            "Don't Save discards these edits. A file that cannot be saved stays open with \
             its changes, and nothing is closed.",
        );
        let footer = modal_footer()
            .child(secondary_button(
                "unsaved-dialog-cancel",
                "Cancel",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_unsaved_dialog(window, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(secondary_button(
                "unsaved-dialog-discard",
                "Don't Save",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.discard_unsaved_and_continue(window, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(
                solid_button(
                    "unsaved-dialog-save",
                    if dialog.saving { "Saving…" } else { "Save" },
                    switch_blue(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.save_unsaved_and_continue(window, cx);
                    cx.stop_propagation();
                })),
            );

        let card = modal_card(
            "unsaved-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div()
                .child(header)
                .child(list)
                .child(explanation)
                .child(footer),
        )
        .track_focus(&self.unsaved_dialog_focus)
        .on_key_down(cx.listener(Self::handle_unsaved_dialog_key_down));

        modal_backdrop(
            "unsaved-dialog-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.close_unsaved_dialog(window, cx);
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_counts_the_unsaved_files() {
        assert_eq!(unsaved_summary(1), "You have unsaved changes in 1 file.");
        assert_eq!(unsaved_summary(3), "You have unsaved changes in 3 files.");
    }

    #[test]
    fn an_ipc_close_names_every_unsaved_file() {
        assert_eq!(unsaved_close_error(&[]), None);
        assert_eq!(
            unsaved_close_error(&["main.rs".to_string(), "lib.rs".to_string()]).as_deref(),
            Some("Workspace has unsaved changes in 2 files: main.rs, lib.rs")
        );
    }
}

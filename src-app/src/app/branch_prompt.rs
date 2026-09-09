use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::widgets::text_input::TextInput;

pub(crate) struct BranchPromptState {
    pub(crate) ws_idx: usize,
    pub(crate) path: std::path::PathBuf,
    pub(crate) input: Entity<TextInput>,
    pub(crate) running: bool,
    pub(crate) error: Option<String>,
}

impl PaneFlowApp {
    pub(crate) fn open_branch_prompt(
        &mut self,
        ws_idx: usize,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transient_surfaces();
        let input = cx.new(|cx| TextInput::new("", "feat/topic", cx));
        let focus = input.read(cx).focus_handle.clone();
        self.branch_prompt = Some(BranchPromptState {
            ws_idx,
            path,
            input,
            running: false,
            error: None,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn branch_prompt_cancel(&mut self, cx: &mut Context<Self>) {
        if self.branch_prompt.as_ref().is_some_and(|p| p.running) {
            return;
        }
        self.branch_prompt = None;
        cx.notify();
    }

    fn branch_prompt_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.branch_prompt.as_ref() else {
            return;
        };
        if prompt.running {
            return;
        }
        let branch = prompt.input.read(cx).value().trim().to_string();
        if branch.is_empty() {
            self.branch_prompt_set_error("Branch name is empty", cx);
            return;
        }
        let (ws_idx, path) = (prompt.ws_idx, prompt.path.clone());
        if let Some(prompt) = self.branch_prompt.as_mut() {
            prompt.running = true;
            prompt.error = None;
        }
        cx.notify();
        self.create_branch_here(ws_idx, path, branch, cx);
    }

    fn branch_prompt_set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        if let Some(prompt) = self.branch_prompt.as_mut() {
            prompt.running = false;
            prompt.error = Some(message.into());
            cx.notify();
        }
    }

    pub(crate) fn branch_prompt_finished(
        &mut self,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.branch_prompt = None;
                cx.notify();
            }
            Err(message) => self.branch_prompt_set_error(message, cx),
        }
    }

    fn handle_branch_prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.branch_prompt_cancel(cx),
            "enter" => self.branch_prompt_confirm(cx),
            _ => {}
        }
    }

    pub(crate) fn render_branch_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.branch_prompt.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let running = prompt.running;
        let checkout = prompt
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(ui.muted)
                    .child(SharedString::from(format!(
                        "Names the detached checkout {checkout}. Uncommitted changes stay."
                    ))),
            )
            .child(
                div()
                    .border_1()
                    .border_color(ui.border)
                    .rounded(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .child(prompt.input.clone()),
            );
        if let Some(err) = &prompt.error {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(ui.vc_deleted)
                    .child(err.clone()),
            );
        }

        let confirm_label: SharedString = if running {
            "Creating…".into()
        } else {
            "Create branch".into()
        };
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(16.))
            .py(px(10.))
            .border_t_1()
            .border_color(ui.border)
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child("Enter creates · Esc cancels"),
            )
            .child(
                div()
                    .id("branch-prompt-confirm")
                    .px(px(12.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .bg(if running {
                        ui.subtle
                    } else {
                        ui.accent.opacity(0.15)
                    })
                    .text_color(if running { ui.muted } else { ui.accent })
                    .when(!running, |d| d.cursor_pointer())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.branch_prompt_confirm(cx);
                        cx.stop_propagation();
                    }))
                    .child(confirm_label),
            );

        let card = div()
            .id("branch-prompt")
            .occlude()
            .track_focus(&self.branch_prompt_focus)
            .on_key_down(cx.listener(Self::handle_branch_prompt_key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.branch_prompt_cancel(cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .w(px(420.))
            .flex()
            .flex_col()
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .rounded(px(10.))
            .overflow_hidden()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(6.))
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child("Create branch here"),
            )
            .child(body)
            .child(footer);

        deferred(
            div()
                .id("branch-prompt-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(72.))
                .bg(gpui::hsla(0., 0., 0., 0.4))
                .child(card),
        )
        .with_priority(8)
        .into_any_element()
    }
}

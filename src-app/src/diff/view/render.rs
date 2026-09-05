use super::*;
use crate::ui_primitives::panel_empty_state;
use crate::widgets::callout::{Callout, CalloutIcon, CalloutSeverity};

impl Focusable for DiffView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        self.reload_if_theme_changed(cx);
        let root = div()
            .id(self.element_id.clone())
            .key_context("DiffView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &crate::CopyDiffHunk, window, cx| {
                this.copy_hovered_hunk(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::DiffNextHunk, window, cx| {
                this.goto_hunk(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::DiffPrevHunk, window, cx| {
                this.goto_hunk(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::DiffToggleView, _window, cx| {
                this.toggle_view_mode(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::DiffDismiss, window, cx| {
                this.dismiss_overlays(window, cx);
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.h_scroll_drag.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_horizontal_scrollbar(event.position.x, cx);
                    } else {
                        this.end_horizontal_scrollbar_drag(cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.end_horizontal_scrollbar_drag(cx);
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.base)
            .text_color(ui.text);

        let mode = self.effective_mode(window);
        self.last_effective_mode = mode;
        self.ensure_mode_loaded(mode, cx);

        let mut root = root.child(self.render_body(mode, ui, cx));
        if let Some(menu) = &self.body_menu {
            root = root.child(self.render_body_menu(menu, ui, cx));
        }
        if let Some(flash) = &self.flash {
            root = root.child(self.render_flash(flash.clone(), ui));
        }
        root
    }
}

impl DiffView {
    pub(super) fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = self.mode.other();
        cx.notify();
    }

    fn dismiss_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.body_menu = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn render_body(
        &self,
        mode: ViewMode,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let col = &self.column;
        match &col.state {
            ColumnState::Loading => panel_empty_state(
                ui,
                Some("icons/loader-circle.svg"),
                None,
                "Computing diff…",
                true,
            )
            .into_any_element(),
            ColumnState::Failed(e) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(16.))
                .child(
                    Callout::new(CalloutSeverity::Error, "Diff failed")
                        .icon(CalloutIcon::TriangleAlert)
                        .description(e.clone())
                        .render(),
                )
                .into_any_element(),
            ColumnState::Loaded { file_count, .. } if *file_count == 0 => panel_empty_state(
                ui,
                Some("icons/check.svg"),
                Some("Clean".into()),
                format!("No changes vs {}", self.base_ref),
                false,
            )
            .into_any_element(),
            ColumnState::Loaded { .. } if !col.has_rows_for_mode(mode) => panel_empty_state(
                ui,
                Some("icons/loader-circle.svg"),
                None,
                format!("Preparing {} diff…", mode.label()),
                true,
            )
            .into_any_element(),
            ColumnState::Loaded { .. } => {
                let body = match mode {
                    ViewMode::Split => DiffBody::Split {
                        rows: col.disp_split.clone(),
                        offsets: col.disp_split_offsets.clone(),
                        max_line_no: col.disp_split_max_no,
                        spans: col.disp_split_spans.clone(),
                        h_offsets: col.h_offsets.clone(),
                    },
                    ViewMode::Unified => DiffBody::Unified {
                        rows: col.disp_unified.clone(),
                        offsets: col.disp_unified_offsets.clone(),
                        max_line_no: col.disp_unified_max_no,
                        spans: col.disp_unified_spans.clone(),
                        h_offsets: col.h_offsets.clone(),
                    },
                };
                let mut element = div()
                    .id(SharedString::from(format!("{}-body", self.element_id)))
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&col.el_scroll)
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| {
                        this.apply_horizontal_wheel(ev, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            let mode = this.effective_mode(window);
                            if this.handle_horizontal_scrollbar_mouse_down(ev.position, mode, cx) {
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                        this.handle_body_click(ev, window, cx);
                    }))
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, _cx| {
                        this.last_body_pos = Some(ev.position);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            let mode = this.effective_mode(window);
                            this.open_body_menu(ev.position, mode, cx);
                        }),
                    )
                    .child(
                        DiffElement::new(body, palette(ui)).with_highlight(self.options.highlight),
                    );
                element.style().restrict_scroll_to_axis = Some(true);
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(element)
                    .child(self.vertical_scrollbar.render(&col.el_scroll, cx))
                    .into_any_element()
            }
        }
    }
}

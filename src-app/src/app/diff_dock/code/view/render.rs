use super::*;

impl CodeView {
    fn render_load_error(
        &self,
        message: String,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel =
            super::super::render::diff_panel_centered("icons/triangle-alert.svg", message, ui);
        if !self.state.is_retriable() {
            return panel;
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .pb(px(20.))
            .child(panel)
            .child(
                div()
                    .id("code-reload")
                    .flex_none()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ui.border)
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(ui.subtle))
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        let path = this.path.clone();
                        this.open(path, cx);
                    }))
                    .child("Reload"),
            )
            .into_any_element()
    }

    pub(super) fn banners(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        let row = || {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .text_xs()
                .border_b_1()
                .border_color(ui.border)
        };
        if let Some(reason) = self
            .state
            .document()
            .and_then(CodeDocument::read_only_reason)
        {
            let flashing = self.read_only_flash.is_some();
            out.push(
                row()
                    .bg(if flashing {
                        ui.vc_conflict.opacity(0.22)
                    } else {
                        ui.overlay
                    })
                    .text_color(if flashing { ui.text } else { ui.muted })
                    .child(read_only_text(reason))
                    .into_any_element(),
            );
        }
        match self.disk {
            DiskState::Conflict => out.push(
                row()
                    .bg(ui.vc_conflict.opacity(0.16))
                    .text_color(ui.text)
                    .child(div().flex_1().child(
                        "This file changed on disk while you were editing it. Nothing has been \
                         overwritten.",
                    ))
                    .child(conflict_button(
                        "code-conflict-keep",
                        "Keep mine",
                        ui,
                        cx.listener(|this, _: &MouseDownEvent, _w, cx| this.resolve_keep_mine(cx)),
                    ))
                    .child(conflict_button(
                        "code-conflict-reload",
                        "Reload from disk",
                        ui,
                        cx.listener(|this, _: &MouseDownEvent, _w, cx| this.resolve_reload(cx)),
                    ))
                    .into_any_element(),
            ),
            DiskState::Deleted => out.push(
                row()
                    .bg(ui.vc_conflict.opacity(0.16))
                    .text_color(ui.text)
                    .child("This file was deleted on disk. Saving recreates it.")
                    .into_any_element(),
            ),
            DiskState::InSync => {}
        }
        if let Some(message) = &self.save_error {
            out.push(
                row()
                    .bg(ui.vc_deleted.opacity(0.16))
                    .text_color(ui.text)
                    .child(format!("{message} Your edits are still here."))
                    .into_any_element(),
            );
        }
        if self
            .state
            .highlighter()
            .is_some_and(CodeHighlighter::is_too_complex)
        {
            out.push(
                row()
                    .bg(ui.overlay)
                    .text_color(ui.muted)
                    .child(TOO_COMPLEX_BANNER)
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_focus_observers(window, cx);
        self.sync_theme();
        self.fill_visible_highlights(window);
        let ui = crate::theme::ui_colors();

        let Some(doc) = self.state.document() else {
            return match self.state.error_message() {
                Some(message) => self.render_load_error(message, ui, cx),
                None => super::super::render::diff_panel_centered(
                    "icons/loader-circle.svg",
                    "Loading file…",
                    ui,
                ),
            };
        };

        self.scroll.set_line_count(doc.line_count());
        let banners = self.banners(ui, cx);
        let popup = self.render_marker_popup(ui, window, cx);
        let theme = crate::theme::active_theme();
        let focused = self.focus.is_focused(window);
        let element = CodeElement::new(
            cx.entity(),
            palette(ui),
            CodeColors {
                scrollbar_thumb: theme.scrollbar_thumb,
                cursor: theme.cursor,
                selection: theme.selection,
                selection_fg: theme.selection_foreground,
                marker_added: ui.vc_added,
                marker_modified: ui.vc_modified,
                marker_deleted: ui.vc_deleted,
            },
            self.scroll.clone(),
            self.h_offset,
            CodeCaret {
                cursor: self.selection.cursor(),
                selection: self.selection.range(),
                focused,
                visible: self.blink_visible,
                marked: self.marked.clone().unwrap_or(0..0),
            },
            self.geometry.clone(),
            self.gutter_memo.clone(),
            self.hits.clone(),
        );

        let host = div()
            .id(self.element_id.clone())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .line_height(px(CODE_ROW_HEIGHT))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered && this.navigation.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if this.on_scrollbar_down(ev, cx)
                        || this.on_marker_down(ev, window, cx)
                        || this.on_text_down(ev, window, cx)
                    {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                this.apply_wheel(ev, cx);
            }))
            .child(element);

        div()
            .id("code-view-body")
            .key_context(CODE_KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::select_page_up))
            .on_action(cx.listener(Self::select_page_down))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste_action))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::save_action))
            .on_action(cx.listener(Self::escape))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                this.on_scrollbar_move(ev, cx);
                this.on_text_move(ev, cx);
                this.on_marker_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _w, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _w, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                }),
            )
            .children(banners)
            .child(host)
            .children(popup)
            .into_any_element()
    }
}

fn read_only_text(reason: ReadOnlyReason) -> String {
    format!(
        "{} Nothing you type is discarded - it simply is not applied.",
        reason.banner()
    )
}

pub(super) fn conflict_button(
    id: &'static str,
    label: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(ui.border)
        .bg(ui.surface)
        .text_color(ui.text)
        .cursor_pointer()
        .hover(|style| style.bg(ui.overlay))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::super::tests::*;
    use super::*;
    use crate::app::diff_dock::code::highlight::MAX_QUERY_ROWS;

    fn scroll_to(view: &Entity<CodeView>, cx: &mut VisualTestContext, rows: f64) {
        view.update(cx, |view, cx| {
            view.scroll.set_rows(rows);
            cx.notify();
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_warm_frame_shapes_nothing_it_already_shaped(cx: &mut TestAppContext) {
        let text: String = (0..400)
            .map(|row| format!("row {row} of plain text\n"))
            .collect();
        let (view, cx) = scrolled(cx, "/nonexistent/warm.txt", &text);

        frame(&view, cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_lines()),
            0,
            "a warm frame must not build a single line string"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_numbers()),
            0,
            "a warm frame must not build a single number string"
        );

        scroll_to(&view, cx, 200.0);
        frame(&view, cx);
        scroll_to(&view, cx, 0.0);
        assert!(
            view.read_with(cx, |view, _| view.materialized_lines()) > 0,
            "rows the layout cache dropped must be shaped again"
        );
    }

    #[gpui::test]
    fn an_edit_only_reshapes_the_row_it_touched(cx: &mut TestAppContext) {
        let rows = 100;
        let text: String = (0..rows)
            .map(|row| format!("row {row} of plain text\n"))
            .collect();
        let (view, cx) = scrolled(cx, "/nonexistent/edit.txt", &text);

        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.materialized_lines()), 0);

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 0 };
            view.replace_text_in_range(None, "z", window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_lines()),
            1,
            "only the edited row may miss the layout cache"
        );
    }

    #[gpui::test]
    fn identical_rows_share_one_shaped_line(cx: &mut TestAppContext) {
        let text: String = (0..400)
            .map(|row| {
                if row % 2 == 0 {
                    "same line\n"
                } else {
                    "other line\n"
                }
            })
            .collect();
        let (view, cx) = scrolled(cx, "/nonexistent/twins.txt", &text);

        let (even, odd, twin) = view.read_with(cx, |view, _| {
            (view.row_width(0), view.row_width(1), view.row_width(2))
        });
        assert_eq!(even, twin, "identical rows must carry identical layouts");
        assert_ne!(even, odd, "the probe needs two measurably different texts");

        frame(&view, cx);
        scroll_to(&view, cx, 200.0);

        let visible = view.read_with(cx, |view, _| view.visible_row_range().len());
        assert!(visible > 4, "the probe needs more rows than distinct texts");
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_lines()),
            0,
            "{visible} rows of already shaped texts must all hit at their new indices"
        );
    }

    #[gpui::test]
    fn a_starved_fill_schedules_the_next_frame_itself(cx: &mut TestAppContext) {
        let text = rows_of_code(500);
        let (view, cx) = view_with_budget(cx, &text, Duration::ZERO);
        cx.simulate_resize(size(px(800.), px(6_000.)));
        cx.run_until_parked();
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();

        let visible = view.read_with(cx, |view, _| view.visible_row_range());
        assert!(
            visible.len() > MAX_QUERY_ROWS,
            "the probe needs a viewport taller than one query slice, got {visible:?}"
        );

        let mut stale = view.read_with(cx, |view, _| view.stale_visible_rows());
        assert!(stale > 0, "a zero budget must leave visible rows stale");

        let mut frames = 0usize;
        while stale > 0 {
            assert_eq!(
                cx.update(|window, cx| window.simulate_next_frame(cx)),
                1,
                "a starved fill must schedule its own follow-up frame"
            );
            cx.run_until_parked();
            let left = view.read_with(cx, |view, _| view.stale_visible_rows());
            assert!(
                left < stale,
                "a follow-up frame must colour more rows, {stale} -> {left}"
            );
            stale = left;
            frames += 1;
            assert!(frames < 8, "the progressive fill must converge");
        }

        assert_eq!(
            cx.update(|window, cx| window.simulate_next_frame(cx)),
            0,
            "a fresh viewport must not schedule another frame"
        );
    }

    #[gpui::test]
    fn a_fresh_viewport_schedules_no_frame(cx: &mut TestAppContext) {
        let text = rows_of_code(40);
        let (view, cx) = view(cx, &text);
        cx.run_until_parked();

        assert_eq!(view.read_with(cx, |view, _| view.stale_visible_rows()), 0);
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    }

    #[gpui::test]
    fn a_loading_document_never_asks_for_a_frame(cx: &mut TestAppContext) {
        let (view, cx) = view_with_budget(cx, "", Duration::ZERO);
        cx.run_until_parked();

        let scheduled = cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            view.update(cx, |view, _cx| view.fill_visible_highlights(window));
            window.simulate_next_frame(cx)
        });
        assert_eq!(
            scheduled, 0,
            "while the document loads only the spinner may drive frames"
        );
    }

    #[gpui::test]
    fn a_file_past_the_highlight_cap_schedules_no_frame(cx: &mut TestAppContext) {
        let mut text = String::with_capacity(crate::diff::MAX_HIGHLIGHT_BYTES + 64);
        while text.len() <= crate::diff::MAX_HIGHLIGHT_BYTES {
            text.push_str("pub fn f() -> i32 { 1 }\n");
        }
        let (view, cx) = view_named(cx, "/nonexistent/huge.rs", &text, Duration::ZERO);
        cx.run_until_parked();

        assert_eq!(view.read_with(cx, |view, _| view.stale_visible_rows()), 0);
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    }
}

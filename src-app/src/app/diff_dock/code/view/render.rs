use super::*;

pub(crate) const RETRY_LABEL: &str = "Retry";

impl CodeView {
    fn render_load_error(
        &self,
        message: String,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel =
            super::super::render::diff_panel_centered("icons/dock/alert-02.svg", message, ui);
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
                    .id("code-retry")
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
                    .child(RETRY_LABEL),
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
        self.start_deferred_load(cx);
        self.ensure_focus_observers(window, cx);
        self.sync_theme();
        self.fill_visible_highlights(window);
        let ui = crate::theme::ui_colors();

        if let Some(preview) = self.render_preview(ui, cx) {
            return preview;
        }
        if let Some(rendered) = self.render_rendered(cx) {
            return rendered;
        }

        let Some(doc) = self.state.document() else {
            return match self.state.error_message() {
                Some(message) => self.render_load_error(message, ui, cx),
                None => super::super::render::diff_panel_centered(
                    "icons/dock/loader-circle.svg",
                    "Loading file…",
                    ui,
                ),
            };
        };

        self.scroll.set_line_count(doc.line_count());
        self.sync_comment_block();
        self.apply_pending_cursor();
        self.apply_pending_scroll(window);
        self.apply_pending_reveal(window, cx);
        let banners = self.banners(ui, cx);
        if self.find_bar().is_some() {
            self.refresh_find(cx);
        }
        let popup = self.render_marker_popup(ui, window, cx);
        let comment_block = self.render_comment_block(ui, cx);
        let comment_button = self.render_comment_button(ui, cx);
        let editor_menu = self.render_editor_menu(window, cx);
        let key_context = if self.comment_focused(window, cx) {
            comment::COMMENT_KEY_CONTEXT
        } else {
            CODE_KEY_CONTEXT
        };
        let find_bar = self.render_find_bar(ui, cx);
        let theme = crate::theme::active_theme();
        let focused = self.focus.is_focused(window);
        let element = CodeElement::new(
            cx.entity(),
            palette(ui).on_material(
                crate::window_chrome::window_backdrop(&crate::config_snapshot::current(cx), window)
                    .is_blended(),
            ),
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
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .line_height(px(CODE_ROW_HEIGHT))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.leave_gutter(cx);
                }
                if !*hovered && this.navigation.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.on_context_down(ev, window, cx);
                }),
            )
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
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _window, cx| {
                this.apply_wheel(ev, cx);
            }))
            .child(element)
            .children(comment_block)
            .children(comment_button);

        let body = div()
            .id("code-view-body")
            .key_context(key_context)
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
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::comment_at_caret))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| {
                this.on_scrollbar_move(ev, cx);
                this.on_text_move(ev, cx);
                this.on_marker_move(ev, cx);
                this.on_comment_move(ev, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                    this.on_comment_up(window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                    this.on_scrollbar_up(ev, cx);
                    this.on_text_up(ev, cx);
                    this.on_comment_up(window, cx);
                }),
            )
            .children(banners)
            .child(host)
            .children(popup);
        div()
            .id("code-view-frame")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(body)
            .children(find_bar)
            .children(editor_menu)
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
    fn a_wider_gutter_reshapes_each_visible_number_once(cx: &mut TestAppContext) {
        let text = (0..999)
            .map(|row| format!("row {row} of plain text"))
            .collect::<Vec<_>>()
            .join("\n");
        let (view, cx) = scrolled(cx, "/nonexistent/gutter.txt", &text);
        scroll_to(&view, cx, 960.0);
        frame(&view, cx);
        assert_eq!(view.read_with(cx, |view, _| view.materialized_numbers()), 0);
        let shaped_before = view.read_with(cx, |view, _| view.numbers_shaped_total());

        view.update_in(cx, |view, window, cx| {
            let end = view.document().map(CodeDocument::len_bytes).unwrap_or(0);
            view.selection = CodeSelection {
                anchor: end,
                head: end,
            };
            view.replace_text_in_range(None, "\nrow 999 of plain text", window, cx);
        });
        cx.run_until_parked();

        let (line_count, shaped_rows, rematerialized) = view.read_with(cx, |view, _| {
            (
                view.document().map(CodeDocument::line_count),
                view.hits.borrow().lines.len(),
                view.numbers_shaped_total() - shaped_before,
            )
        });
        assert_eq!(line_count, Some(1000));
        assert!(shaped_rows > 0, "the probe needs visible rows");
        assert_eq!(
            rematerialized, shaped_rows,
            "every visible number is shaped once for the wider gutter, across every frame the edit drew"
        );

        frame(&view, cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.materialized_numbers()),
            0,
            "a warm frame after the widening shapes no number"
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

    #[gpui::test]
    fn a_tab_indented_row_is_as_wide_as_its_four_column_expansion(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(
            cx,
            "/nonexistent/tabs.txt",
            "\tx\n    x\na\tb\na   b\nabcd\te\nabcd    e\n",
        );
        let widths = view.read_with(cx, |view, _| {
            (0..6).map(|row| view.row_width(row)).collect::<Vec<_>>()
        });
        assert!(widths.iter().all(Option::is_some), "{widths:?}");
        assert_eq!(widths[0], widths[1], "a leading tab spans four columns");
        assert_eq!(
            widths[2], widths[3],
            "a tab after one column reaches column four"
        );
        assert_eq!(
            widths[4], widths[5],
            "a tab on a stop advances a whole stop"
        );
    }

    #[gpui::test]
    fn a_painted_tab_keeps_one_buffer_character_and_maps_clicks_and_carets(
        cx: &mut TestAppContext,
    ) {
        use gpui::{point, px};
        let (view, cx) = scrolled(cx, "/nonexistent/tab-caret.txt", "a\tbcd\n");
        view.read_with(cx, |view, _| {
            let doc = view.document().expect("document");
            assert_eq!(doc.line_string(0).as_deref(), Some("a\tbcd"));
            assert_eq!(doc.slice_string(0..3), "a\tb");
            assert_eq!(doc.to_disk_string(), "a\tbcd\n");

            let hits = view.hits.borrow();
            let line = hits.lines[0].as_ref().expect("a shaped row");
            assert_eq!(line.shaped().text.as_ref(), "a   bcd");
            assert_eq!(line.x_for_index(1), line.shaped().x_for_index(1));
            assert_eq!(line.x_for_index(2), line.shaped().x_for_index(4));
            assert_eq!(line.x_for_index(3), line.shaped().x_for_index(5));

            let y = px(hits.row_top(0) + CODE_ROW_HEIGHT / 2.);
            let at_column = |column: usize| {
                let x = px(hits.text_x) + line.shaped().x_for_index(column);
                hits.offset_at(doc, &view.scroll.map(), point(x, y))
            };
            assert_eq!(at_column(1), 1, "the left edge of the tab is before it");
            assert_eq!(at_column(2), 1, "the left half of the tab lands before it");
            assert_eq!(at_column(3), 2, "the right half of the tab lands after it");
            assert_eq!(at_column(4), 2, "the right edge of the tab is after it");
        });
    }

    #[gpui::test]
    fn the_caret_after_a_tab_is_revealed_at_its_expanded_column(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/tab-reveal.txt", "\t\tx\n");
        view.read_with(cx, |view, _| {
            let doc = view.document().expect("document");
            assert_eq!(
                crate::app::diff_dock::code::cursor::display_column(doc, 2),
                8
            );
            assert_eq!(crate::app::diff_dock::code::cursor::goal_column(doc, 2), 2);
        });
    }
}

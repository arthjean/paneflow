//! Review-terminal lifecycle (launch / pick / close) + per-column review menu
//! for the Review view (US-004 code-motion). See [`super`] for `DiffView`.

use super::*;
use crate::ui_primitives::AnimatedHoverExt;
use paneflow_config::schema::TerminalSurfaceProfile;

impl DiffView {
    /// A branch column has something to review when it's loaded with > 0 files.
    pub(super) fn column_has_changes(col: &Column) -> bool {
        matches!(&col.state, ColumnState::Loaded { file_count, .. } if *file_count > 0)
    }

    /// Open/close a column's Review CLI multi-select. On open, sync the pick
    /// toggles to the CLI list (default first-on). Clicking the same column's
    /// Review button again (or a different one) toggles / re-targets the popover.
    pub(super) fn toggle_review_menu(&mut self, col_idx: usize, cx: &mut Context<Self>) {
        if self.review_menu_open == Some(col_idx) {
            self.review_menu_open = None;
        } else {
            self.review_menu_open = Some(col_idx);
            let n = super::super::review_terminal::ReviewCli::all().len();
            if self.review_picks.len() != n {
                self.review_picks = (0..n).map(|i| i == 0).collect();
            }
        }
        cx.notify();
    }

    /// Toggle one CLI's inclusion in the next review.
    pub(super) fn toggle_review_pick(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(p) = self.review_picks.get_mut(i) {
            *p = !*p;
            cx.notify();
        }
    }

    /// Launch the selected CLIs to review column `col_idx`'s branch: one real
    /// terminal per CLI, embedded UNDER the column's diff (in the Diff interface,
    /// not the CLI mode), cwd-pinned to the worktree, with a compact review prompt
    /// pre-filled (the human submits). Human-in-the-loop - no headless session.
    pub(super) fn launch_review(
        &mut self,
        col_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.review_menu_open = None;
        let clis = super::super::review_terminal::ReviewCli::all();
        let selected: Vec<usize> = (0..clis.len())
            .filter(|i| self.review_picks.get(*i).copied().unwrap_or(*i == 0))
            .collect();
        if selected.is_empty() {
            self.set_flash("Select at least one CLI".into(), cx);
            return;
        }
        let blocked_by_running_review = {
            let Some(col) = self.columns.get_mut(col_idx) else {
                return;
            };
            if col.has_running_review_terminal(cx) {
                col.drop_exited_review_terminals(cx);
                true
            } else {
                col.drop_review_terminals();
                false
            }
        };
        if blocked_by_running_review {
            self.set_flash(
                "Close Review terminals before running Review again".into(),
                cx,
            );
            return;
        }
        let Some(col) = self.columns.get(col_idx) else {
            return;
        };
        let cwd = col.path.clone();
        let branch = col.branch.clone();
        let ws_id = col.workspace_id.unwrap_or(0);
        let base = col
            .base_override
            .clone()
            .unwrap_or_else(|| self.base_ref.clone());

        // One terminal per selected CLI; the 2nd+ get the adversarial framing so a
        // multi-CLI panel is a real second opinion, not an echo.
        let mut created: Vec<ReviewTerminal> = Vec::new();
        let mut first_prompt: Option<String> = None;
        let mut focus_target: Option<Entity<crate::terminal::TerminalView>> = None;
        let config = paneflow_config::loader::load_config();
        // US-011: configurable prefill delay (default 2000 ms); clipboard write
        // (below) remains the synchronous safety net.
        let delay = config.resolved_review_prefill_delay_ms();
        for (rank, &i) in selected.iter().enumerate() {
            let cli = clis[i];
            let prompt =
                super::super::review_terminal::build_cli_review_prompt(&branch, &base, rank > 0);
            let term = cx.new(|cx| {
                crate::terminal::TerminalView::with_cwd_and_profile(
                    ws_id,
                    Some(cwd.clone()),
                    None,
                    TerminalSurfaceProfile::Review,
                    cx,
                )
            });
            // Launch the CLI in the embedded terminal's shell.
            let command = cli.launch_command(&config);
            term.read(cx).send_command(&command);
            // Pre-fill the prompt once the CLI has booted (tmux send-keys style):
            // a delayed write with NO Enter - the human reviews + submits. The
            // clipboard fallback (below) covers a missed timing window.
            let prefill = prompt.clone();
            let term_weak = term.downgrade();
            cx.spawn(async move |_, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(Duration::from_millis(delay)).await;
                cx.update(|cx| {
                    if let Some(t) = term_weak.upgrade() {
                        t.read(cx).send_text(&prefill);
                    }
                });
            })
            .detach();
            let label = if rank > 0 {
                format!("{} · 2nd opinion", cli.label())
            } else {
                cli.label().to_string()
            };
            if focus_target.is_none() {
                focus_target = Some(term.clone());
            }
            if first_prompt.is_none() {
                first_prompt = Some(prompt.clone());
            }
            created.push(ReviewTerminal {
                label: label.into(),
                terminal: term,
                prompt_ready: true,
                prompt: Some(prompt),
            });
        }

        if let Some(col) = self.columns.get_mut(col_idx) {
            col.review_terminals = created; // replace any prior run (drops old PTYs)
            col.active_review_terminal = 0;
        }
        if let Some(t) = focus_target {
            t.read(cx).focus_handle(cx).focus(window, cx);
        }
        if let Some(p) = first_prompt {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(p));
        }
        cx.notify();
    }

    /// Close one embedded terminal (drops it → PTY shutdown).
    pub(super) fn close_review_terminal(
        &mut self,
        col_idx: usize,
        term_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(col) = self.columns.get_mut(col_idx) else {
            return;
        };
        if term_idx < col.review_terminals.len() {
            let was_active = col.active_review_terminal == term_idx;
            col.review_terminals.remove(term_idx);
            if col.review_terminals.is_empty() {
                col.active_review_terminal = 0;
            } else if was_active {
                col.active_review_terminal = term_idx.min(col.review_terminals.len() - 1);
            } else if col.active_review_terminal > term_idx {
                col.active_review_terminal -= 1;
            } else if col.active_review_terminal >= col.review_terminals.len() {
                col.active_review_terminal = col.review_terminals.len() - 1;
            }
            if let Some(term) = col.review_terminals.get(col.active_review_terminal) {
                term.terminal.read(cx).focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn select_review_terminal(
        &mut self,
        col_idx: usize,
        term_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(col) = self.columns.get_mut(col_idx) else {
            return;
        };
        if let Some(term) = col.review_terminals.get(term_idx) {
            col.active_review_terminal = term_idx;
            term.terminal.read(cx).focus_handle(cx).focus(window, cx);
            if let Some(prompt) = &term.prompt {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(prompt.clone()));
            }
            cx.notify();
        }
    }

    /// Terminal button on a column header: open a plain shell terminal in the
    /// branch's worktree, embedded under the diff. Just a terminal - no CLI
    /// launch, no prefill (distinct from Review).
    pub(super) fn open_terminal_for_column(
        &mut self,
        col_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((existing_idx, existing_terminal)) = self.columns.get(col_idx).and_then(|col| {
            col.review_terminals
                .iter()
                .enumerate()
                .find(|(_, rt)| rt.prompt.is_none())
                .map(|(idx, rt)| (idx, rt.terminal.clone()))
        }) {
            if let Some(col) = self.columns.get_mut(col_idx) {
                col.active_review_terminal = existing_idx;
            }
            existing_terminal
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
            cx.notify();
            return;
        }

        let Some(col) = self.columns.get(col_idx) else {
            return;
        };
        let cwd = col.path.clone();
        let ws_id = col.workspace_id.unwrap_or(0);
        let term = cx.new(|cx| {
            crate::terminal::TerminalView::with_cwd_and_profile(
                ws_id,
                Some(cwd),
                None,
                TerminalSurfaceProfile::Review,
                cx,
            )
        });
        term.read(cx).focus_handle(cx).focus(window, cx);
        if let Some(col) = self.columns.get_mut(col_idx) {
            col.active_review_terminal = col.review_terminals.len();
            col.review_terminals.push(ReviewTerminal {
                label: "Terminal".into(),
                terminal: term,
                prompt_ready: false,
                prompt: None,
            });
        }
        cx.notify();
    }

    /// Render the embedded review terminals under a column's diff body as a
    /// tabbed dock matching the Agents bottom terminal panel.
    pub(super) fn render_review_terminals(
        &self,
        col_idx: usize,
        col: &Column,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if col.review_terminals.is_empty() {
            return None;
        }
        // US-049: the review prompt is pre-filled after a fixed delay, but on a
        // slow CLI cold-start (notably Windows ConPTY) the auto-fill can miss
        // its window. The prompt is always copied to the clipboard as a fallback
        // - surface that explicitly so the user can paste it instead of staring
        // at an empty input.
        let paste_key = if cfg!(target_os = "macos") {
            "⌘V"
        } else {
            "Ctrl+V"
        };
        let active_idx = col
            .active_review_terminal
            .min(col.review_terminals.len().saturating_sub(1));
        let show_prompt_hint = col
            .review_terminals
            .get(active_idx)
            .is_some_and(|rt| rt.prompt_ready);
        let mut tabs = div()
            .id(SharedString::from(format!(
                "diff-review-tabs-scroll-{col_idx}"
            )))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .overflow_x_scroll();
        for (ti, rt) in col.review_terminals.iter().enumerate() {
            tabs = tabs.child(render_review_terminal_tab(
                col_idx,
                ti,
                rt.label.clone(),
                active_idx == ti,
                ui,
                cx,
            ));
        }
        let tab_strip = div()
            .id(SharedString::from(format!(
                "diff-review-tabstrip-{col_idx}"
            )))
            .flex_none()
            .h(px(40.))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .pl(px(8.))
            .pr(px(8.))
            .bg(ui.base)
            .child(tabs)
            // US-011: surface the clipboard safety net immediately. Kept in the
            // dock toolbar instead of per-terminal chrome so the terminal body
            // matches the Agents bottom panel surface.
            .when(show_prompt_hint, |d| {
                d.child(render_review_prompt_pill(paste_key, ui))
            });
        let active_terminal = col
            .review_terminals
            .get(active_idx)
            .map(|rt| rt.terminal.clone());
        let terminal_surface = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(ui.base)
            .children(active_terminal);
        let resize_handle = div()
            .id(SharedString::from(format!("diff-review-resize-{col_idx}")))
            .absolute()
            .top(px(-3.))
            .left_0()
            .right_0()
            .h(px(7.))
            .cursor(CursorStyle::ResizeUpDown)
            .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.06))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    let start_h = this
                        .columns
                        .get(col_idx)
                        .map(|c| c.review_height)
                        .unwrap_or(REVIEW_DEFAULT_HEIGHT);
                    this.review_resizing = Some((col_idx, f32::from(ev.position.y), start_h));
                    cx.stop_propagation();
                }),
            );
        let region = div()
            .relative()
            .flex_none()
            .h(px(col.review_height))
            .flex()
            .flex_col()
            .bg(ui.base)
            .border_t_1()
            .border_color(ui.border)
            .child(resize_handle)
            .child(tab_strip)
            .child(terminal_surface);
        Some(region.into_any_element())
    }

    /// The Review chip's CLI multi-select popover. Lists the CLIs as toggles and
    /// a Review button that opens one terminal pane per checked CLI under the
    /// branch's worktree.
    pub(super) fn render_review_menu(
        &self,
        col_idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clis = super::super::review_terminal::ReviewCli::all();
        let mut menu = menu_surface(div().id("diff-review-menu"), ui)
            .occlude()
            .absolute()
            // Anchored just below this branch's header.
            .top(px(COL_HEADER_HEIGHT))
            .right(px(6.))
            .w(px(256.))
            .flex()
            .flex_col()
            .p(px(6.))
            .gap(px(2.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.review_menu_open = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .text_size(crate::ui_primitives::LABEL_XS)
                    .text_color(ui.muted)
                    .child("Launch a CLI to review this branch"),
            );
        for (i, cli) in clis.iter().enumerate() {
            let checked = self.review_picks.get(i).copied().unwrap_or(i == 0);
            let label = cli.label();
            menu = menu.child(
                select_item(
                    SharedString::from(format!("diff-review-pick-{i}")),
                    false,
                    ui,
                )
                .cursor(CursorStyle::Arrow)
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.toggle_review_pick(i, cx);
                }))
                .child(
                    div()
                        .flex_none()
                        .size(px(14.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(ui.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(checked, |d| {
                            d.bg(ui.accent.opacity(0.18)).child(
                                gpui::svg()
                                    .size(px(10.))
                                    .path("icons/check.svg")
                                    .text_color(ui.accent),
                            )
                        }),
                )
                .child(div().flex_1().text_color(ui.text).child(label)),
            );
        }
        menu = menu.child(
            div()
                .id("diff-review-run")
                .mt(px(2.))
                .flex()
                .items_center()
                .justify_center()
                .py(px(5.))
                .rounded(px(5.))
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.accent)
                .animated_hover_bg(ui.accent.opacity(0.15), ui.accent.opacity(0.25))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.launch_review(col_idx, window, cx);
                }))
                .child("Review"),
        );
        deferred(menu).priority(8).into_any_element()
    }
}

fn render_review_terminal_tab(
    col_idx: usize,
    term_idx: usize,
    label: SharedString,
    active: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<DiffView>,
) -> AnyElement {
    let (bg, fg) = review_tab_colors(active, ui);
    let hover_bg = with_alpha(ui.text, if active { 0.09 } else { 0.05 });
    div()
        .id(SharedString::from(format!(
            "diff-review-term-tab-{col_idx}-{term_idx}"
        )))
        .flex_none()
        .h(px(28.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.))
        .pl(px(11.))
        .pr(px(5.))
        .rounded(px(8.))
        .animated_hover_bg(bg, hover_bg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _e: &ClickEvent, window, cx| {
            this.select_review_terminal(col_idx, term_idx, window, cx);
        }))
        .child(
            gpui::svg()
                .size(px(13.))
                .flex_none()
                .path("icons/terminal.svg")
                .text_color(fg),
        )
        .child(
            div()
                .max_w(px(150.))
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.5))
                .text_color(fg)
                .child(label),
        )
        .child(render_review_tab_close_button(col_idx, term_idx, ui, cx))
        .into_any_element()
}

fn render_review_tab_close_button(
    col_idx: usize,
    term_idx: usize,
    ui: crate::theme::UiColors,
    cx: &mut Context<DiffView>,
) -> AnyElement {
    div()
        .id(SharedString::from(format!(
            "diff-review-term-close-{col_idx}-{term_idx}"
        )))
        .flex_none()
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.14))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _e: &ClickEvent, window, cx| {
            this.close_review_terminal(col_idx, term_idx, window, cx);
        }))
        .child(
            gpui::svg()
                .size(px(11.))
                .flex_none()
                .path("icons/close.svg")
                .text_color(ui.muted),
        )
        .into_any_element()
}

fn render_review_prompt_pill(paste_key: &'static str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_none()
        .h(px(24.))
        .max_w(px(180.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .px(px(8.))
        .rounded(px(7.))
        .bg(with_alpha(ui.text, 0.07))
        .text_size(crate::ui_primitives::LABEL_XS)
        .text_color(with_alpha(ui.text, 0.78))
        .child(
            gpui::svg()
                .size(px(11.))
                .flex_none()
                .path("icons/sparkles.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(format!("Prompt ready · {paste_key} to paste")),
        )
        .into_any_element()
}

fn review_tab_colors(active: bool, ui: crate::theme::UiColors) -> (gpui::Hsla, gpui::Hsla) {
    if active {
        (with_alpha(ui.text, 0.09), ui.text)
    } else {
        (with_alpha(ui.text, 0.0), ui.muted)
    }
}

//! Click / hover / context-menu interaction for the Review view
//! (US-004 code-motion). See [`super`] for the `DiffView` definition.

use super::*;
use crate::ui_primitives::AnimatedHoverExt;

impl DiffView {
    /// Select the column whose changed-file list feeds the sidebar and whose
    /// body `jump_to_file` scrolls. Bound to a column-header click.
    pub(super) fn select_column(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.selected_column != idx {
            self.selected_column = idx;
            cx.notify();
        }
    }
    /// Jump the selected column to the next/previous hunk relative to its
    /// current scroll position (cycles at the ends). Stateless: the target is
    /// derived from where the viewport is, so it stays correct after manual
    /// scrolling. The synced columns follow via the per-render broadcast.
    pub(super) fn goto_hunk(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.effective_mode(window);
        let Some(ci) = self.selected_or_first_visible() else {
            return;
        };
        let Some((handle, tops, cur_y)) = self.columns.get(ci).map(|col| {
            let cur_y = f32::from(-col.el_scroll.offset().y).max(0.0);
            (col.el_scroll.clone(), col.hunk_tops(mode).clone(), cur_y)
        }) else {
            return;
        };
        if tops.is_empty() {
            return;
        }
        // A jumped-to hunk is parked HUNK_JUMP_MARGIN px below the viewport top,
        // so the hunk "at" the current position is the one near
        // `cur_y + HUNK_JUMP_MARGIN` - not `cur_y`. Pivot on that: otherwise
        // `forward` keeps matching the already-parked hunk (its top is still
        // > cur_y), and the down arrow looks dead while up works.
        let pivot = cur_y + HUNK_JUMP_MARGIN;
        let target = if forward {
            tops.iter()
                .copied()
                .find(|&t| t > pivot + 4.0)
                .unwrap_or(tops[0])
        } else {
            tops.iter()
                .rev()
                .copied()
                .find(|&t| t < pivot - 4.0)
                .unwrap_or_else(|| *tops.last().unwrap_or(&0.0))
        };
        let x = handle.offset().x;
        handle.set_offset(point(x, px((HUNK_JUMP_MARGIN - target).min(0.0))));
        self.selected_column = ci;
        self.scroll_driver = ci;
        cx.notify();
    }

    /// Body click: focus the column, and if it landed on a file-header row,
    /// toggle that file's collapse. Maps the click Y to a displayed row via the
    /// scroll handle's painted bounds + offset (uniform [`ROW_HEIGHT`]).
    pub(super) fn handle_body_click(
        &mut self,
        col_idx: usize,
        ev: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_column(col_idx, cx);
        let mode = self.effective_mode(window);
        // EP-003 US-009: focus the DiffView body so the keyboard review loop
        // ([`/`]/u/s/Esc) is live without first tabbing into the surface.
        window.focus(&self.focus_handle, cx);
        if self.handle_horizontal_scrollbar_click(col_idx, ev.position(), mode, cx) {
            return;
        }
        let row = {
            let Some(col) = self.columns.get(col_idx) else {
                return;
            };
            let bounds = col.el_scroll.bounds();
            let y = ev.position().y;
            if y < bounds.top() || y > bounds.bottom() {
                return;
            }
            let target = f32::from(y - bounds.top() - col.el_scroll.offset().y).max(0.0);
            // US-050: variable row heights (taller file-header cards) make this a
            // band lookup - shared with `row_at_point` / `jump_to_file`.
            let offsets = match mode {
                ViewMode::Unified => &col.disp_unified_offsets,
                ViewMode::Split => &col.disp_split_offsets,
            };
            match hit_test::row_at_offset(offsets, target) {
                Some(r) => r,
                None => return, // click past the last row
            }
        };
        let fold_key = {
            let Some(col) = self.columns.get(col_idx) else {
                return;
            };
            match mode {
                ViewMode::Unified => col
                    .disp_unified
                    .get(row)
                    .filter(|r| r.kind == RowKind::Fold)
                    .and_then(|r| r.fold_key.as_ref())
                    .map(|key| key.to_string()),
                ViewMode::Split => match col.disp_split.get(row) {
                    Some(SplitRow::Fold(fold)) => Some(fold.key.to_string()),
                    _ => None,
                },
            }
        };
        if let Some(key) = fold_key {
            if let Some(col) = self.columns.get_mut(col_idx) {
                if !col.expanded_folds.remove(&key) {
                    col.expanded_folds.insert(key);
                }
                col.recompute_display();
                cx.notify();
            }
            return;
        }
        let path = {
            let Some(col) = self.columns.get(col_idx) else {
                return;
            };
            let anchors = match mode {
                ViewMode::Unified => &col.disp_anchors_unified,
                ViewMode::Split => &col.disp_anchors_split,
            };
            anchors
                .iter()
                .find(|(_, i)| *i == row)
                .map(|(p, _)| p.clone())
        };
        let Some(path) = path else {
            return; // not a file header - nothing to collapse
        };
        if let Some(col) = self.columns.get_mut(col_idx) {
            if !col.collapsed.remove(&path) {
                discard_expanded_folds_for_path(&mut col.expanded_folds, &path);
                col.collapsed.insert(path);
            }
            col.recompute_display();
            cx.notify();
        }
    }

    /// US-002: map a window-space point over column `col_idx`'s body to its row
    /// index, walking the same variable row heights as [`Self::handle_body_click`].
    pub(super) fn row_at_point(
        &self,
        col_idx: usize,
        point: Point<Pixels>,
        mode: ViewMode,
    ) -> Option<usize> {
        let col = self.columns.get(col_idx)?;
        let bounds = col.el_scroll.bounds();
        if point.y < bounds.top() || point.y > bounds.bottom() {
            return None;
        }
        let target = f32::from(point.y - bounds.top() - col.el_scroll.offset().y).max(0.0);
        let offsets = match mode {
            ViewMode::Unified => &col.disp_unified_offsets,
            ViewMode::Split => &col.disp_split_offsets,
        };
        hit_test::row_at_offset(offsets, target)
    }

    /// US-002: resolve a body point to the file (+ optional enclosing hunk) under
    /// it. Returns `None` for a click in a gap, on a collapsed/blank area, or when
    /// the column is not loaded. Hunk resolution is unified-mode only (the split
    /// view resolves to file scope); a click on a context/header line yields a
    /// file scope with no hunk.
    pub(super) fn resolve_body_scope(
        &self,
        col_idx: usize,
        point: Point<Pixels>,
        mode: ViewMode,
    ) -> Option<DiffBodyScope> {
        let row = self.row_at_point(col_idx, point, mode)?;
        let col = self.columns.get(col_idx)?;
        let ColumnState::Loaded { files_full, .. } = &col.state else {
            return None;
        };
        let anchors = match mode {
            ViewMode::Unified => &col.disp_anchors_unified,
            ViewMode::Split => &col.disp_anchors_split,
        };
        // The file whose header row is the closest one at or above `row`.
        let path = anchors
            .iter()
            .filter(|(_, hdr)| *hdr <= row)
            .max_by_key(|(_, hdr)| *hdr)
            .map(|(p, _)| p.clone())?;
        let file_idx = files_full.iter().position(|f| f.path == path)?;
        let hunk_idx = match mode {
            ViewMode::Unified => {
                let r = col.disp_unified.get(row)?;
                let file = files_full.get(file_idx)?;
                match r.kind {
                    RowKind::Added => r.new_no.and_then(|n| n.checked_sub(1)).and_then(|idx| {
                        file.hunks
                            .iter()
                            .position(|h| h.new_row_range.contains(&idx))
                    }),
                    RowKind::Removed => r.old_no.and_then(|n| n.checked_sub(1)).and_then(|idx| {
                        file.hunks
                            .iter()
                            .position(|h| h.base_row_range.contains(&idx))
                    }),
                    _ => None,
                }
            }
            ViewMode::Split => None,
        };
        Some(DiffBodyScope { file_idx, hunk_idx })
    }

    /// US-003: serialize the scope (a single hunk when `want_hunk`, else the whole
    /// file) to the clipboard and flash a confirmation. Copying a hunk on a
    /// non-hunk scope is a no-op with a "No hunk here" flash.
    pub(super) fn copy_scope(
        &mut self,
        col_idx: usize,
        scope: DiffBodyScope,
        want_hunk: bool,
        cx: &mut Context<Self>,
    ) {
        let result = {
            let Some(col) = self.columns.get(col_idx) else {
                return;
            };
            let ColumnState::Loaded { files_full, .. } = &col.state else {
                return;
            };
            let Some(file) = files_full.get(scope.file_idx) else {
                return;
            };
            if want_hunk {
                scope.hunk_idx.and_then(|h| file.hunks.get(h)).map(|hunk| {
                    (
                        super::super::extract::hunk_to_unified(file, hunk),
                        format!(
                            "Hunk copied ({})",
                            super::super::extract::hunk_tag(file, hunk)
                        ),
                    )
                })
            } else {
                Some((
                    super::super::extract::file_to_unified(file),
                    format!("Copied {} diff", file.path),
                ))
            }
        };
        match result {
            Some((diff, msg)) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(diff));
                self.set_flash(msg.into(), cx);
            }
            None => self.set_flash("No hunk here".into(), cx),
        }
    }

    /// US-003 action handler (`Ctrl+Shift+C` in the `DiffView` context): copy the
    /// hunk under the last-known cursor position.
    pub(super) fn copy_hovered_hunk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.effective_mode(window);
        let Some((col_idx, point)) = self.last_body_pos else {
            self.set_flash("No hunk here".into(), cx);
            return;
        };
        match self.resolve_body_scope(col_idx, point, mode) {
            Some(scope) => self.copy_scope(col_idx, scope, true, cx),
            None => self.set_flash("No hunk here".into(), cx),
        }
    }

    /// US-003: open the right-click body menu, pre-resolving the scope under the
    /// pointer. A right-click that resolves to nothing closes any open menu.
    pub(super) fn open_body_menu(
        &mut self,
        col_idx: usize,
        point: Point<Pixels>,
        mode: ViewMode,
        cx: &mut Context<Self>,
    ) {
        self.select_column(col_idx, cx);
        self.body_menu = self
            .resolve_body_scope(col_idx, point, mode)
            .map(|scope| DiffBodyMenu {
                position: point,
                col_idx,
                scope,
                mode,
            });
        cx.notify();
    }

    /// US-003: show a transient confirmation pill, auto-cleared after a beat.
    pub(super) fn set_flash(&mut self, msg: SharedString, cx: &mut Context<Self>) {
        self.flash = Some(msg);
        cx.notify();
        cx.spawn(async move |this, cx| {
            smol::Timer::after(Duration::from_millis(1600)).await;
            let _ = this.update(cx, |this, cx| {
                this.flash = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// US-003: the deferred right-click menu, window-anchored at the click point.
    pub(super) fn render_body_menu(
        &self,
        menu: &DiffBodyMenu,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_hunk = menu.scope.hunk_idx.is_some();
        let copy_hunk_label = if !has_hunk && menu.mode == ViewMode::Split {
            "Copy hunk (Unified only)"
        } else {
            "Copy hunk"
        };
        let col_idx = menu.col_idx;
        let scope = menu.scope;
        let copy_hunk_item = div()
            .id("diff-menu-copy-hunk")
            .h(px(28.))
            .px(px(8.))
            .rounded(px(7.))
            .flex()
            .flex_row()
            .items_center()
            .text_size(crate::ui_primitives::BODY)
            .text_color(if has_hunk { ui.text } else { ui.muted })
            .child(copy_hunk_label);
        let copy_hunk_item = if has_hunk {
            copy_hunk_item
                .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.05))
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.body_menu = None;
                    this.copy_scope(col_idx, scope, true, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        } else {
            copy_hunk_item.into_any_element()
        };
        let panel = menu_surface(div().id("diff-body-context-menu"), ui)
            .occlude()
            .w(px(230.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.body_menu = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // Conditionally disabled, so kept as a bespoke row (matching the
            // `select_item` geometry) rather than `select_item` itself, which
            // always advertises a hover/cursor affordance.
            .child(copy_hunk_item)
            .child(
                select_item("diff-menu-copy-file", false, ui)
                    .cursor(CursorStyle::Arrow)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.body_menu = None;
                        this.copy_scope(col_idx, scope, false, cx);
                        cx.stop_propagation();
                    }))
                    .child(div().text_color(ui.text).child("Copy file diff")),
            );
        deferred(
            anchored()
                .position(menu.position)
                .snap_to_window()
                .child(panel),
        )
        .priority(3)
        .into_any_element()
    }

    /// US-003: the transient "copied" pill, centered near the bottom of the view.
    pub(super) fn render_flash(&self, msg: SharedString, ui: crate::theme::UiColors) -> AnyElement {
        deferred(
            div()
                .absolute()
                .bottom(px(16.))
                .left_0()
                .right_0()
                .flex()
                .flex_row()
                .justify_center()
                .child(
                    div()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .bg(ui.overlay)
                        .border_1()
                        .border_color(ui.border)
                        .shadow_lg()
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .text_color(ui.text)
                        .child(msg),
                ),
        )
        .priority(4)
        .into_any_element()
    }
}

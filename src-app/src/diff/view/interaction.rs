use super::*;
use crate::ui_primitives::AnimatedHoverExt;

impl DiffView {
    pub(super) fn select_column(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.selected_column != idx {
            self.selected_column = idx;
            cx.notify();
        }
    }
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

    pub(super) fn handle_body_click(
        &mut self,
        col_idx: usize,
        ev: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_column(col_idx, cx);
        let mode = self.effective_mode(window);
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
            let offsets = match mode {
                ViewMode::Unified => &col.disp_unified_offsets,
                ViewMode::Split => &col.disp_split_offsets,
            };
            match hit_test::row_at_offset(offsets, target) {
                Some(r) => r,
                None => return,
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
            return;
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

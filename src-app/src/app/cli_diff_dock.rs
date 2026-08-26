//! Right-docked git diff for the CLI cockpit.
//!
//! The trailing `layout-sidebar-right` button of a pane header toggles the side
//! dock ([`crate::app::agents_diff`]) on that pane's *workspace folder*. The
//! dock hosts three surfaces - the working-tree diff against `HEAD`, a shell,
//! and open files - and asks which one the first time it is opened
//! (`agents_diff::surface_picker`). This module owns the CLI plumbing only - the
//! toggle and the dock host; the panel itself is rendered once by
//! [`crate::app::agents_diff`].

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, div,
    px,
};

use crate::PaneFlowApp;

impl PaneFlowApp {
    /// Pane-header button handler: close the dock when it already shows this
    /// folder, otherwise (re)open it there.
    pub(crate) fn toggle_cli_diff_dock(&mut self, cwd: String, cx: &mut Context<Self>) {
        let cwd = cwd.trim().to_string();
        let showing = self.agents_view.agents_diff_open
            && self
                .agents_view
                .agents_diff
                .as_ref()
                .is_some_and(|data| data.cwd == cwd);
        if showing {
            self.close_agents_diff_panel(cx);
        } else {
            // The button opens the dock, not the diff: until this workspace has
            // said once what it wants in it, the dock comes up on its surface
            // picker. Afterwards it restores whatever tab was last active there.
            self.agents_view.diff_dock_picker = !self.agents_view.diff_dock_picked;
            self.open_agents_diff_panel(cwd, cx);
        }
    }

    /// Whether the dock is actually on screen.
    ///
    /// `agents_diff_open` alone is not enough: the flag survives a mode switch
    /// and a trip through Settings, both of which unmount the dock in
    /// [`Self::wrap_cli_diff_dock`]. Anything that acts *on* the dock without
    /// putting it back on screen has to ask this instead, or it mutates a strip
    /// nobody can see.
    pub(crate) fn diff_dock_visible(&self) -> bool {
        self.agents_view.agents_diff_open
            && self.settings_section.is_none()
            && matches!(self.mode, paneflow_config::schema::AppMode::Cli)
    }

    /// Dock the diff panel to the right of the CLI pane grid when it is open.
    /// The resize / horizontal-scrollbar drags are captured on this wrapper (a
    /// full-height surface) so a drag keeps tracking once the cursor outruns its
    /// handle and crosses into the panes beside it.
    pub(crate) fn wrap_cli_diff_dock(
        &mut self,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.diff_dock_visible() {
            return body;
        }
        let ui = crate::theme::ui_colors();
        div()
            .size_full()
            .flex()
            .flex_row()
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _w, cx| {
                if this.agents_view.agents_diff_h_scroll_drag.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_agents_diff_h_scrollbar(event.position.x, cx);
                    } else {
                        this.end_agents_diff_h_scrollbar_drag(cx);
                    }
                } else if this.agents_view.agents_diff_resize.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_agents_diff_resize(f32::from(event.position.x), cx);
                    } else {
                        this.end_agents_diff_resize(cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _w, cx| {
                    this.end_agents_diff_h_scrollbar_drag(cx);
                    this.end_agents_diff_resize(cx);
                }),
            )
            .child(div().flex_1().min_w_0().h_full().child(body))
            // The pane grid already pads its own right edge, so the dock only
            // has to reproduce the other three gutters to sit on the same
            // margins as the cards it docks beside.
            .child(
                div()
                    .flex_none()
                    .h_full()
                    .flex()
                    .flex_col()
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::layout::PANE_GUTTER_PX))
                    .pr(px(crate::layout::PANE_GUTTER_PX))
                    .child(self.render_agents_diff_panel(ui, cx)),
            )
            .into_any_element()
    }
}

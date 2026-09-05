use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, Window, deferred, px,
};

use super::ReviewRailMenu;
use crate::PaneFlowApp;
use crate::app::sidebar::context_menu::clamped_context_menu_position;
use crate::settings::components::select_menu;

impl PaneFlowApp {
    pub(crate) fn render_review_rail_menu(
        &self,
        menu: ReviewRailMenu,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_workspace = menu.subject.worktree.workspace_id.is_some();
        let rows = 1 + usize::from(has_workspace);
        let menu_height = px(8. + rows as f32 * 29.);
        let menu_pos = clamped_context_menu_position(menu.position, px(232.), menu_height, window);

        let mut context_menu = select_menu("review-rail-menu", ui)
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(px(232.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.review.rail_menu = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        if let Some(workspace_id) = menu.subject.worktree.workspace_id {
            context_menu = context_menu.child(self.render_select_menu_item(
                "review-rail-show-in-agents".into(),
                "Show in Agents",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.review.rail_menu = None;
                    this.enter_cli_mode(window, cx);
                    if let Some(idx) = this.workspaces.iter().position(|ws| ws.id == workspace_id) {
                        this.select_workspace(idx, window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ));
        }

        let path = menu.subject.worktree.path.to_string_lossy().into_owned();
        context_menu = context_menu.child(self.render_select_menu_item(
            "review-rail-copy-path".into(),
            "Copy Path",
            None,
            ui,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                this.review.rail_menu = None;
                this.show_toast("Copied path", cx);
                cx.stop_propagation();
            }),
        ));

        deferred(context_menu).priority(3).into_any_element()
    }
}

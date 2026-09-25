use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SidebarDropSlot {
    tab: Option<(usize, usize)>,
    workspace: Option<usize>,
}

pub(super) const SIDEBAR_DROP_GROUP: &str = "sidebar-drop-zone";
const SIDEBAR_DROP_PLACEHOLDER_MARGIN: f32 = 6.0;
const SIDEBAR_DROP_PLACEHOLDER_RADIUS: f32 = 8.0;
const SIDEBAR_DROP_PLACEHOLDER_FILL_ALPHA: f32 = 0.10;
const SIDEBAR_DROP_PLACEHOLDER_BORDER_ALPHA: f32 = 0.22;
const SIDEBAR_DROP_LINE_PX: f32 = 2.0;
const SIDEBAR_DROP_BAND_REACH: f32 = SIDEBAR_ROW_LINE_HEIGHT / 2.0 + SIDEBAR_ROW_PADDING_Y;

fn reorder_target(from: usize, slot: usize) -> usize {
    if from < slot { slot - 1 } else { slot }
}

pub(super) fn sidebar_drop_slots(
    rows: &[SidebarRow],
    workspace_count: usize,
) -> Vec<SidebarDropSlot> {
    (0..=rows.len())
        .map(|k| SidebarDropSlot {
            tab: match k.checked_sub(1).map(|above| rows[above]) {
                Some(SidebarRow::Folder(ws)) => Some((ws, 0)),
                Some(SidebarRow::Tab(ws, tab)) => Some((ws, tab + 1)),
                None => None,
            },
            workspace: match rows.get(k) {
                Some(SidebarRow::Folder(ws)) => Some(*ws),
                None => Some(workspace_count),
                Some(SidebarRow::Tab(..)) => None,
            },
        })
        .collect()
}

impl PaneFlowApp {
    pub(super) fn render_drop_divider(
        &self,
        key: usize,
        slot: SidebarDropSlot,
        spacing: f32,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let color = ui.text.opacity(0.5);
        let group = SharedString::from(format!("drop-slot-{key}"));
        let mut band = div()
            .id(SharedString::from(format!("drop-band-{key}")))
            .group(group.clone())
            .absolute()
            .top(px(-SIDEBAR_DROP_BAND_REACH))
            .w_full()
            .px(px(SIDEBAR_ROW_MARGIN_X))
            .h(px(spacing + SIDEBAR_DROP_BAND_REACH * 2.0))
            .flex()
            .flex_col()
            .justify_center();
        let mut line = div()
            .group(SharedString::from(format!("drop-line-{key}")))
            .h(px(SIDEBAR_DROP_LINE_PX))
            .w_full()
            .rounded_full();

        if let Some((ws_idx, tab_idx)) = slot.tab {
            let ws_id = self.workspaces.get(ws_idx).map(|ws| ws.id);
            band = band
                .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                    if ws_id == Some(drag.workspace_id) {
                        let Some(from) = this
                            .workspaces
                            .get(ws_idx)
                            .and_then(|ws| ws.tabs().iter().position(|tab| tab.id == drag.tab_id))
                        else {
                            return;
                        };
                        this.reorder_workspace_tab(drag, ws_idx, reorder_target(from, tab_idx), cx);
                    } else {
                        this.move_tab_to_workspace(drag, ws_idx, tab_idx, window, cx);
                    }
                }))
                .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                    this.move_pane_to_new_tab(drag.pane_id, ws_idx, tab_idx, window, cx);
                }));
            line = line
                .group_drag_over::<TabDrag>(group.clone(), move |style| style.bg(color))
                .group_drag_over::<PaneDrag>(group.clone(), move |style| style.bg(color));
        }

        if let Some(ws_slot) = slot.workspace {
            band = band.on_drop(cx.listener(move |this, drag: &WorkspaceDrag, _window, cx| {
                let Some(from) = this.workspaces.iter().position(|ws| ws.id == drag.id) else {
                    return;
                };
                this.reorder_workspace(drag.id, reorder_target(from, ws_slot), cx);
            }));
            line =
                line.group_drag_over::<WorkspaceDrag>(group.clone(), move |style| style.bg(color));
        }

        div()
            .h(px(spacing))
            .flex_none()
            .relative()
            .child(band.child(line))
    }

    pub(super) fn render_sidebar_drop_placeholder(cx: &mut Context<Self>) -> impl IntoElement {
        let tint = crate::theme::ui_colors().text;
        div()
            .absolute()
            .top(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .left(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .right(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .bottom(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .rounded(px(SIDEBAR_DROP_PLACEHOLDER_RADIUS))
            .bg(tint.opacity(SIDEBAR_DROP_PLACEHOLDER_FILL_ALPHA))
            .border_2()
            .border_color(tint.opacity(SIDEBAR_DROP_PLACEHOLDER_BORDER_ALPHA))
            .invisible()
            .group_drag_over::<gpui::ExternalPaths>(SIDEBAR_DROP_GROUP, |style| style.visible())
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                    this.open_workspace_folders(paths.paths(), cx);
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{
        AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
        size,
    };

    #[test]
    fn reorder_target_accounts_for_the_removed_source() {
        assert_eq!(reorder_target(0, 3), 2);
        assert_eq!(reorder_target(4, 1), 1);
        assert_eq!(reorder_target(2, 2), 2);
        assert_eq!(reorder_target(2, 3), 2);
    }

    #[test]
    fn drop_slots_sit_between_the_rendered_rows() {
        let rows = [
            SidebarRow::Folder(0),
            SidebarRow::Tab(0, 0),
            SidebarRow::Tab(0, 1),
            SidebarRow::Folder(1),
        ];
        let slots = sidebar_drop_slots(&rows, 2);

        assert_eq!(slots.len(), rows.len() + 1);
        assert_eq!(
            slots[0],
            SidebarDropSlot {
                tab: None,
                workspace: Some(0)
            }
        );
        assert_eq!(
            slots[1],
            SidebarDropSlot {
                tab: Some((0, 0)),
                workspace: None
            }
        );
        assert_eq!(
            slots[2],
            SidebarDropSlot {
                tab: Some((0, 1)),
                workspace: None
            }
        );
        assert_eq!(
            slots[3],
            SidebarDropSlot {
                tab: Some((0, 2)),
                workspace: Some(1)
            }
        );
        assert_eq!(
            slots[4],
            SidebarDropSlot {
                tab: Some((1, 0)),
                workspace: Some(2)
            }
        );
    }

    #[gpui::test]
    fn a_drop_divider_spans_the_rail_and_reaches_over_its_neighbors(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(div().h(px(30.)))
                    .child(
                        div()
                            .h(px(SIDEBAR_ROW_SPACING))
                            .flex_none()
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-SIDEBAR_DROP_BAND_REACH))
                                    .w_full()
                                    .px(px(SIDEBAR_ROW_MARGIN_X))
                                    .h(px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0))
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .debug_selector(|| "band".into())
                                    .child(
                                        div()
                                            .h(px(SIDEBAR_DROP_LINE_PX))
                                            .w_full()
                                            .bg(gpui::rgb(0xff0000))
                                            .debug_selector(|| "line".into()),
                                    ),
                            ),
                    )
            },
        );
        let band = cx.debug_bounds("band").expect("drop band not painted");
        let line = cx.debug_bounds("line").expect("drop line not painted");

        assert_eq!(band.size.width, px(SIDEBAR_WIDTH));
        assert_eq!(
            line.size.width,
            px(SIDEBAR_WIDTH - 2. * SIDEBAR_ROW_MARGIN_X)
        );
        assert_eq!(band.origin.y, px(30.) - px(SIDEBAR_DROP_BAND_REACH));
        assert_eq!(
            band.size.height,
            px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0)
        );
        assert_eq!(line.origin.y, px(30.) + px(SIDEBAR_ROW_SPACING / 2.0 - 1.0));
    }
}

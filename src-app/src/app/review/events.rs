use gpui::{Context, Entity};

use crate::PaneFlowApp;
use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::{Pane, PaneEvent};

impl PaneFlowApp {
    pub(crate) fn handle_review_pane_event(
        &mut self,
        pane: Entity<Pane>,
        event: &PaneEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Remove => self.review_close_pane(pane, cx),
            PaneEvent::Split(direction) => {
                if let Err(message) = self.review_split_pane(pane, *direction, cx) {
                    self.show_toast(message, cx);
                }
            }
            PaneEvent::ToggleAgentSessions | PaneEvent::ToggleDiffDock => {}
            PaneEvent::OpenPaneMenu { position } => {
                self.dismiss_transient_surfaces();
                self.pane_menu_open = Some(crate::PaneContextMenu {
                    pane: pane.clone(),
                    position: *position,
                });
                cx.notify();
            }
            PaneEvent::DropSessionSplit { .. } => {}
            PaneEvent::DropSubjectSplit { edge, subject } => {
                self.review_drop_subject(pane, *edge, subject.clone(), cx);
            }
            PaneEvent::DropPaneMove {
                source_pane_id,
                edge,
            } => {
                if pane.entity_id().as_u64() == *source_pane_id {
                    return;
                }
                let Some(root) = self.review.layout.as_mut() else {
                    return;
                };
                let Some(source) = root
                    .collect_leaves()
                    .into_iter()
                    .find(|leaf| leaf.entity_id().as_u64() == *source_pane_id)
                else {
                    return;
                };
                let moved = match edge {
                    None => root.swap_panes(&source, &pane),
                    Some(edge) => {
                        let Some(tree) = self.review.layout.take() else {
                            return;
                        };
                        let (pruned, removed) = tree.remove_pane(&source);
                        let mut tree = pruned.unwrap_or_else(|| LayoutTree::Leaf(source.clone()));
                        let mut moved = false;
                        if removed && tree.contains_leaf(&pane) {
                            moved = crate::app::event_handlers::split_pane_at_edge(
                                &mut tree,
                                &pane,
                                *edge,
                                source.clone(),
                            );
                            if !moved {
                                moved = tree.first_leaf().is_some_and(|anchor| {
                                    tree.split_at_pane(
                                        &anchor,
                                        SplitDirection::Vertical,
                                        source.clone(),
                                    )
                                });
                            }
                        }
                        self.review.layout = Some(tree);
                        moved
                    }
                };
                if !moved {
                    return;
                }
                self.review.active_pane = Some(source.clone());
                self.pending_pane_focus = Some(source);
                self.save_session(cx);
                cx.notify();
            }
        }
    }
}

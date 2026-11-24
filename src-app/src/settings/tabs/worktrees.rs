use gpui::{
    AnyElement, ClickEvent, Context, Div, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, SharedString, StatefulInteractiveElement, Styled, div, prelude::*, px,
};
use paneflow_config::schema::{WORKTREES_KEEP_LIMIT_MAX, WorktreesConfig};

use crate::PaneFlowApp;
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::{self, Block, SearchCard};
use crate::workspace::worktree::{self, ManagedWorktree, Snapshot};

const WORKTREES_INTRO: &str =
    "Where Paneflow creates worktrees, when it removes them, and the changes it saves first.";
const MANAGED_HINT: &str =
    "Removing one asks first while a tab or a terminal uses it. Its branch is never deleted.";
const SNAPSHOTS_HINT: &str = "Uncommitted changes saved when a worktree was removed.";
const BRANCH_ICON: &str = "icons/settings/git-branch.svg";
const SNAPSHOT_ICON: &str = "icons/settings/layers-01.svg";
const ADD_ICON: &str = "icons/settings/add-01.svg";
const REMOVE_ICON: &str = "icons/settings/remove-01.svg";

impl PaneFlowApp {
    pub(crate) fn render_worktrees_heading(&self, title: Div) -> AnyElement {
        let ink = Ink::of(crate::theme::ui_colors());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(ink, WORKTREES_INTRO))
            .into_any_element()
    }

    pub(crate) fn render_worktrees_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ink = Ink::of(crate::theme::ui_colors());
        let mut first = true;
        div()
            .flex()
            .flex_col()
            .child(self.render_worktree_storage(kit::section_gap(&mut first), ink, cx))
            .child(self.render_managed_worktrees(kit::section_gap(&mut first), ink, cx))
            .child(self.render_worktree_snapshots(kit::section_gap(&mut first), ink, cx))
            .into_any_element()
    }

    fn render_worktree_storage(
        &self,
        top_gap: f32,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = &self.cached_config.worktrees;
        let auto_remove = config.auto_remove_enabled();
        let mut card = SearchCard::new()
            .row(&search::WORKTREE_ROOT, self.worktree_root_row(ink, cx))
            .row(
                &search::AUTO_REMOVE_WORKTREES,
                kit::row(
                    ink,
                    None,
                    setting_text(&search::AUTO_REMOVE_WORKTREES, ink),
                    Some(
                        kit::switch(
                            "worktrees-auto-remove",
                            search::AUTO_REMOVE_WORKTREES.title,
                            auto_remove,
                            ink,
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            let mut next = this.cached_config.worktrees.clone();
                            next.auto_remove = Some(!auto_remove);
                            this.persist_worktrees(next, cx);
                        }))
                        .into_any_element(),
                    ),
                ),
            );
        if auto_remove {
            card = card.row(
                &search::WORKTREE_KEEP_LIMIT,
                self.worktree_keep_limit_row(config.keep_limit(), ink, cx),
            );
        }
        let (matched, rows) = card.into_rows();
        Block::new("Storage and cleanup")
            .top_gap(top_gap)
            .found(matched)
            .child(kit::section(
                ink,
                "Storage and cleanup",
                None,
                kit::search_group(ink, rows),
            ))
            .finish()
    }

    fn worktree_root_row(&self, ink: Ink, cx: &mut Context<Self>) -> Div {
        let custom = self.cached_config.worktrees.dir_path();
        let shown = custom
            .clone()
            .or_else(worktree::default_worktrees_root)
            .map(|dir| display_path(&dir))
            .unwrap_or_else(|| "Beside each repository".to_string());
        let mut controls = div().flex().flex_row().items_center().gap(px(8.)).child(
            kit::button(
                "worktrees-dir-change",
                "Change…",
                Tone::Secondary,
                Size::Small,
                ink,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.choose_worktrees_dir(cx);
            })),
        );
        if custom.is_some() {
            controls = controls.child(
                kit::button(
                    "worktrees-dir-default",
                    "Use default",
                    Tone::Ghost,
                    Size::Small,
                    ink,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    let mut next = this.cached_config.worktrees.clone();
                    next.dir = None;
                    this.persist_worktrees(next, cx);
                })),
            );
        }
        kit::row(
            ink,
            None,
            setting_text(&search::WORKTREE_ROOT, ink).line(kit::mono_line(ink, shown)),
            Some(controls.into_any_element()),
        )
    }

    fn worktree_keep_limit_row(&self, keep_limit: u32, ink: Ink, cx: &mut Context<Self>) -> Div {
        let can_decrease = keep_limit > 0;
        let can_increase = keep_limit < WORKTREES_KEEP_LIMIT_MAX;
        let decrease = kit::step_button(
            "worktrees-keep-limit-dec",
            "Keep one fewer worktree",
            REMOVE_ICON,
            can_decrease,
            ink,
        )
        .when(can_decrease, |button| {
            button.on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                let mut next = this.cached_config.worktrees.clone();
                next.keep_limit = Some(keep_limit.saturating_sub(1));
                this.persist_worktrees(next, cx);
            }))
        });
        let increase = kit::step_button(
            "worktrees-keep-limit-inc",
            "Keep one more worktree",
            ADD_ICON,
            can_increase,
            ink,
        )
        .when(can_increase, |button| {
            button.on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                let mut next = this.cached_config.worktrees.clone();
                next.keep_limit = Some((keep_limit + 1).min(WORKTREES_KEEP_LIMIT_MAX));
                this.persist_worktrees(next, cx);
            }))
        });
        kit::row(
            ink,
            None,
            setting_text(&search::WORKTREE_KEEP_LIMIT, ink),
            Some(kit::stepper(ink, keep_limit.to_string(), decrease, increase).into_any_element()),
        )
    }

    fn render_managed_worktrees(
        &self,
        top_gap: f32,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let managed = self.managed_worktrees_snapshot();
        let body = if managed.is_empty() {
            kit::group(ink).child(empty_row(
                ink,
                BRANCH_ICON,
                "No worktrees yet",
                "Worktrees Paneflow creates for a branch appear here.",
            ))
        } else {
            kit::separated(
                ink,
                managed
                    .iter()
                    .enumerate()
                    .map(|(idx, (ws_id, wt))| self.managed_worktree_row(idx, *ws_id, wt, ink, cx)),
            )
        };
        Block::new("Managed worktrees")
            .top_gap(top_gap)
            .child(kit::section(
                ink,
                "Managed worktrees",
                (!managed.is_empty()).then_some(MANAGED_HINT),
                body,
            ))
            .finish()
    }

    fn managed_worktree_row(
        &self,
        idx: usize,
        ws_id: u64,
        wt: &ManagedWorktree,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = wt.path.clone();
        let remove = kit::button(
            SharedString::from(format!(
                "worktrees-remove-{}",
                branch_slug_id(&wt.branch, idx)
            )),
            "Remove",
            Tone::Danger,
            Size::Small,
            ink,
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.request_worktree_removal(ws_id, path.clone(), cx);
        }));
        kit::row(
            ink,
            Some(kit::disc(ink, BRANCH_ICON)),
            Text::new(wt.branch.clone()).line(kit::mono_line(ink, display_path(&wt.path))),
            Some(remove.into_any_element()),
        )
        .into_any_element()
    }

    fn render_worktree_snapshots(
        &self,
        top_gap: f32,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snapshots = self.worktree_snapshots();
        let body = if snapshots.is_empty() {
            kit::group(ink).child(empty_row(
                ink,
                SNAPSHOT_ICON,
                "No snapshots",
                "Uncommitted changes of a removed worktree are kept here until you restore or \
                 delete them.",
            ))
        } else {
            kit::separated(
                ink,
                snapshots
                    .iter()
                    .enumerate()
                    .map(|(idx, (ws_id, snapshot))| {
                        self.snapshot_row(idx, *ws_id, snapshot, ink, cx)
                    }),
            )
        };
        Block::new("Snapshots")
            .top_gap(top_gap)
            .child(kit::section(
                ink,
                "Snapshots",
                (!snapshots.is_empty()).then_some(SNAPSHOTS_HINT),
                body,
            ))
            .finish()
    }

    fn snapshot_row(
        &self,
        idx: usize,
        ws_id: u64,
        snapshot: &Snapshot,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let armed = self.worktree_snapshot_delete_armed.as_deref() == Some(&snapshot.reference);
        let restore_snapshot = snapshot.clone();
        let delete_snapshot = snapshot.clone();
        let reference = snapshot.reference.clone();
        let disarm_reference = snapshot.reference.clone();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                kit::button(
                    SharedString::from(format!("worktrees-snapshot-restore-{idx}")),
                    "Restore",
                    Tone::Secondary,
                    Size::Small,
                    ink,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.restore_worktree_snapshot(ws_id, restore_snapshot.clone(), cx);
                })),
            )
            .child(
                kit::button(
                    SharedString::from(format!("worktrees-snapshot-delete-{idx}")),
                    if armed { "Confirm delete" } else { "Delete" },
                    Tone::Danger,
                    Size::Small,
                    ink,
                )
                .on_mouse_down_out(cx.listener(move |this, _, _w, cx| {
                    if this.worktree_snapshot_delete_armed.as_deref()
                        == Some(disarm_reference.as_str())
                    {
                        this.worktree_snapshot_delete_armed = None;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    if this.worktree_snapshot_delete_armed.as_deref() == Some(reference.as_str()) {
                        this.worktree_snapshot_delete_armed = None;
                        this.delete_worktree_snapshot(ws_id, delete_snapshot.clone(), cx);
                    } else {
                        this.worktree_snapshot_delete_armed = Some(reference.clone());
                        cx.notify();
                    }
                })),
            );
        kit::row(
            ink,
            Some(kit::disc(ink, SNAPSHOT_ICON)),
            Text::new(snapshot.label())
                .line(kit::line(ink, snapshot_age(snapshot.taken_at)))
                .line(kit::mono_line(ink, display_path(&snapshot.path))),
            Some(controls.into_any_element()),
        )
        .into_any_element()
    }

    pub(crate) fn persist_worktrees(&mut self, next: WorktreesConfig, cx: &mut Context<Self>) {
        let value = if next == WorktreesConfig::default() {
            serde_json::Value::Null
        } else {
            serde_json::to_value(&next).unwrap_or(serde_json::Value::Null)
        };
        self.persist_setting(false, "worktrees", value, cx);
    }

    fn choose_worktrees_dir(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if let Ok(Ok(Some(paths))) = receiver.await {
                    let Some(path) = paths.into_iter().next() else {
                        return;
                    };
                    let path = path.to_string_lossy().into_owned();
                    cx.update(|cx| {
                        this.update(cx, |app, cx| {
                            let mut next = app.cached_config.worktrees.clone();
                            next.dir = Some(path);
                            app.persist_worktrees(next, cx);
                        })
                        .ok();
                    });
                }
            },
        )
        .detach();
    }
}

fn setting_text(copy: &search::SettingCopy, ink: Ink) -> Text {
    Text::new(copy.title).line(kit::line(ink, copy.description))
}

fn empty_row(
    ink: Ink,
    icon: &'static str,
    title: &'static str,
    description: &'static str,
) -> AnyElement {
    kit::row(
        ink,
        Some(kit::disc(ink, icon)),
        Text::new(title).line(kit::line(ink, description)),
        None,
    )
    .into_any_element()
}

fn snapshot_age(taken_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let elapsed = now.saturating_sub(taken_at);
    match elapsed {
        s if s < 60 => "Saved just now".to_string(),
        s if s < 3600 => format!("Saved {} min ago", s / 60),
        s if s < 86_400 => format!("Saved {} h ago", s / 3600),
        s => format!("Saved {} d ago", s / 86_400),
    }
}

fn branch_slug_id(branch: &str, idx: usize) -> String {
    format!("{}-{idx}", worktree::branch_slug(branch))
}

pub(crate) fn display_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display());
    }
    path.display().to_string()
}

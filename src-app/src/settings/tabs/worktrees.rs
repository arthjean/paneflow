use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, SharedString, Styled, div, prelude::*, px,
};
use paneflow_config::schema::{WORKTREES_KEEP_LIMIT_MAX, WorktreesConfig};

use crate::PaneFlowApp;
use crate::settings::components::{
    hairline, secondary_button, section_header, setting_card, toggle_pill, toggle_row_with,
    with_alpha,
};
use crate::workspace::worktree;

impl PaneFlowApp {
    pub(crate) fn render_worktrees_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let config = self.cached_config.worktrees.clone();
        let custom_dir = config.dir_path();
        let shown_dir = custom_dir
            .clone()
            .or_else(worktree::default_worktrees_root)
            .map(|dir| display_path(&dir))
            .unwrap_or_else(|| "beside each repository".to_string());
        let auto_remove = config.auto_remove_enabled();
        let keep_limit = config.keep_limit();

        let mut dir_control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .max_w(px(360.))
            .child(
                div()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child(shown_dir),
            )
            .child(secondary_button(
                "worktrees-dir-change",
                "Change…",
                ui,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.choose_worktrees_dir(cx);
                }),
            ));
        if custom_dir.is_some() {
            dir_control = dir_control.child(secondary_button(
                "worktrees-dir-default",
                "Default",
                ui,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    let mut next = this.cached_config.worktrees.clone();
                    next.dir = None;
                    this.persist_worktrees(next, cx);
                }),
            ));
        }
        let dir_row = toggle_row_with(
            "Worktree root",
            "Directory where Paneflow creates managed worktrees, one subdirectory per \
             repository. Worktrees already created elsewhere stay where they are.",
            None,
            ui,
            dir_control,
        );

        let auto_remove_row = toggle_row_with(
            "Remove old worktrees automatically",
            "A managed worktree is removed when its workspace closes, and the oldest ones \
             are trimmed past the keep limit. Uncommitted changes are saved as a snapshot \
             first, and the branch is never deleted.",
            None,
            ui,
            div()
                .id("worktrees-auto-remove")
                .flex_shrink_0()
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let mut next = this.cached_config.worktrees.clone();
                    next.auto_remove = Some(!auto_remove);
                    this.persist_worktrees(next, cx);
                }))
                .child(toggle_pill(auto_remove, ui)),
        );

        let limit_control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .flex_shrink_0()
            .child(secondary_button(
                "worktrees-keep-limit-dec",
                "−",
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let mut next = this.cached_config.worktrees.clone();
                    next.keep_limit = Some(keep_limit.saturating_sub(1));
                    this.persist_worktrees(next, cx);
                }),
            ))
            .child(
                div()
                    .w(px(36.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(ui.text)
                    .child(SharedString::from(format!("{keep_limit}"))),
            )
            .child(secondary_button(
                "worktrees-keep-limit-inc",
                "+",
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let mut next = this.cached_config.worktrees.clone();
                    next.keep_limit = Some((keep_limit + 1).min(WORKTREES_KEEP_LIMIT_MAX));
                    this.persist_worktrees(next, cx);
                }),
            ));
        let limit_row = toggle_row_with(
            "Keep limit",
            "Number of managed worktrees to keep before the oldest unopened ones are \
             removed.",
            None,
            ui,
            limit_control,
        );

        let mut settings_card = setting_card(ui)
            .child(dir_row)
            .child(hairline(ui))
            .child(auto_remove_row);
        if auto_remove {
            settings_card = settings_card.child(hairline(ui)).child(limit_row);
        }

        let managed = self.managed_worktrees_snapshot();
        let mut list = setting_card(ui);
        if managed.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("Worktrees created by Paneflow will appear here"),
            );
        }
        for (idx, (ws_id, wt)) in managed.iter().enumerate() {
            if idx > 0 {
                list = list.child(hairline(ui));
            }
            let path = wt.path.clone();
            let remove_id = SharedString::from(format!(
                "worktrees-remove-{}",
                branch_slug_id(&wt.branch, idx)
            ));
            let ws_id = *ws_id;
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(16.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(ui.text)
                                    .whitespace_nowrap()
                                    .overflow_x_hidden()
                                    .text_ellipsis()
                                    .child(wt.branch.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui.muted)
                                    .whitespace_nowrap()
                                    .overflow_x_hidden()
                                    .text_ellipsis()
                                    .child(display_path(&wt.path)),
                            ),
                    )
                    .child(
                        div()
                            .id(remove_id)
                            .flex_shrink_0()
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(7.))
                            .bg(ui.subtle)
                            .hover(|style| style.bg(with_alpha(ui.text, 0.12)))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.remove_managed_worktree(ws_id, path.clone(), cx);
                            }))
                            .child("Remove"),
                    ),
            );
        }

        let count_label: &'static str = if managed.is_empty() {
            "No worktrees yet"
        } else {
            "Managed worktrees"
        };

        let snapshots = self.worktree_snapshots();
        let mut snapshot_list = setting_card(ui);
        if snapshots.is_empty() {
            snapshot_list = snapshot_list.child(
                div()
                    .px(px(12.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child(
                        "Uncommitted changes of a removed worktree are kept here until you \
                         restore or delete them",
                    ),
            );
        }
        for (idx, (ws_id, snapshot)) in snapshots.iter().enumerate() {
            if idx > 0 {
                snapshot_list = snapshot_list.child(hairline(ui));
            }
            let ws_id = *ws_id;
            let restore_id = SharedString::from(format!("worktrees-snapshot-restore-{idx}"));
            let delete_id = SharedString::from(format!("worktrees-snapshot-delete-{idx}"));
            let restore_snapshot = snapshot.clone();
            let delete_snapshot = snapshot.clone();
            snapshot_list = snapshot_list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(16.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(ui.text)
                                    .whitespace_nowrap()
                                    .overflow_x_hidden()
                                    .text_ellipsis()
                                    .child(snapshot.label()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui.muted)
                                    .whitespace_nowrap()
                                    .overflow_x_hidden()
                                    .text_ellipsis()
                                    .child(format!(
                                        "{} · {}",
                                        snapshot_age(snapshot.taken_at),
                                        display_path(&snapshot.path)
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .flex_shrink_0()
                            .child(secondary_button(
                                restore_id,
                                "Restore",
                                ui,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.restore_worktree_snapshot(
                                        ws_id,
                                        restore_snapshot.clone(),
                                        cx,
                                    );
                                }),
                            ))
                            .child(secondary_button(
                                delete_id,
                                "Delete",
                                ui,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.delete_worktree_snapshot(
                                        ws_id,
                                        delete_snapshot.clone(),
                                        cx,
                                    );
                                }),
                            )),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(section_header(ui, "Storage and cleanup"))
                    .child(settings_card),
            )
            .child(
                div()
                    .mt(px(24.))
                    .flex()
                    .flex_col()
                    .child(section_header(ui, count_label))
                    .child(list),
            )
            .child(
                div()
                    .mt(px(24.))
                    .flex()
                    .flex_col()
                    .child(section_header(ui, "Snapshots"))
                    .child(snapshot_list),
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

fn snapshot_age(taken_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let elapsed = now.saturating_sub(taken_at);
    match elapsed {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86_400),
    }
}

fn branch_slug_id(branch: &str, idx: usize) -> String {
    format!("{}-{idx}", worktree::branch_slug(branch))
}

fn display_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display());
    }
    path.display().to_string()
}

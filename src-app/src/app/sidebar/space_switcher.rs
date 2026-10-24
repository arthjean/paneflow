use super::*;

use std::collections::HashMap;

use gpui::{ImageSource, MouseUpEvent, ObjectFit, StyledImage, deferred, img};
use paneflow_config::schema::{DEFAULT_SPACE_NAME, MAX_SPACES, SpaceColor, SpaceId};

use crate::app::attention_queue::{AttentionLevel, SpaceAttention};
use crate::app::spaces::{SPACE_LIMIT_REASON, space_label, space_order};
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::settings::components::{menu_panel, menu_row};

use super::context_menu::context_menu_divider;
use super::customize_menu::menu_label;
use super::lane::lane_visual;

const SPACE_CONTROL_RADIUS: f32 = 7.0;
const SPACE_MARK_SIZE: f32 = 16.0;
const SPACE_MENU_WIDTH: f32 = 240.0;
const SPACE_MENU_ICON_SLOT: f32 = SPACE_MARK_SIZE;
const SPACE_MENU_GLYPH_SIZE: f32 = 13.0;
const SPACE_MENU_CHECK_SLOT: f32 = 14.0;

type SpaceAttentionMap = HashMap<Option<SpaceId>, SpaceAttention>;

fn agents(count: usize) -> String {
    match count {
        1 => "1 agent".to_string(),
        n => format!("{n} agents"),
    }
}

pub(super) fn attention_label(attention: SpaceAttention) -> String {
    [
        (attention.input, "waiting for input"),
        (attention.errors, "failed"),
        (attention.finished, "finished"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{} {what}", agents(count)))
    .collect::<Vec<_>>()
    .join(", ")
}

pub(super) fn space_row_label(name: &str, current: bool, attention: SpaceAttention) -> String {
    if current {
        return format!("Space {name}, current");
    }
    match attention.total() {
        0 => format!("Space {name}"),
        _ => format!("Space {name}, {}", attention_label(attention)),
    }
}

pub(super) fn attention_button_label(
    target: &str,
    attention: SpaceAttention,
    elsewhere: usize,
) -> String {
    let label = format!("Go to {target}: {}", attention_label(attention));
    match elsewhere {
        0 => label,
        n => format!("{label}, {n} more in other spaces"),
    }
}

pub(super) fn most_urgent_space(
    hidden: &[(Option<SpaceId>, SpaceAttention)],
) -> Option<(Option<SpaceId>, AttentionLevel)> {
    [
        AttentionLevel::Input,
        AttentionLevel::Error,
        AttentionLevel::Finished,
    ]
    .into_iter()
    .find_map(|level| {
        hidden
            .iter()
            .find(|(_, attention)| attention.most_urgent() == Some(level))
            .map(|(space, _)| (space.clone(), level))
    })
}

fn combined_attention(hidden: &[(Option<SpaceId>, SpaceAttention)]) -> SpaceAttention {
    hidden
        .iter()
        .fold(SpaceAttention::default(), |sum, (_, attention)| {
            SpaceAttention {
                input: sum.input + attention.input,
                errors: sum.errors + attention.errors,
                finished: sum.finished + attention.finished,
            }
        })
}

pub(crate) fn space_mark(
    color: Option<SpaceColor>,
    avatar: Option<&str>,
    size: f32,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let placeholder = color_circle(
        color.map(crate::space_avatar::space_color_fill),
        ui.muted,
        size,
    );
    if let Some(path) = avatar.and_then(crate::space_avatar::avatar_path) {
        return circular_image(path, size, placeholder);
    }
    match color {
        Some(color) => circular_image(
            crate::space_avatar::space_color_image(color),
            size,
            placeholder,
        ),
        None => placeholder(),
    }
}

pub(crate) fn default_space_mark(size: f32, ui: crate::theme::UiColors) -> AnyElement {
    circular_image(
        crate::space_avatar::default_space_image(),
        size,
        color_circle(None, ui.muted, size),
    )
}

fn color_circle(
    fill: Option<gpui::Hsla>,
    outline: gpui::Hsla,
    size: f32,
) -> impl Fn() -> AnyElement + Copy + 'static {
    move || {
        let circle = div().flex_none().size(px(size)).rounded_full();
        match fill {
            Some(fill) => circle.bg(fill),
            None => circle.border_1().border_color(outline),
        }
        .into_any_element()
    }
}

fn circular_image(
    source: impl Into<ImageSource>,
    size: f32,
    placeholder: impl Fn() -> AnyElement + Copy + 'static,
) -> AnyElement {
    img(source)
        .flex_none()
        .size(px(size))
        .rounded_full()
        .object_fit(ObjectFit::Cover)
        .with_loading(placeholder)
        .with_fallback(placeholder)
        .into_any_element()
}

fn render_attention_mark(
    attention: SpaceAttention,
    ui: crate::theme::UiColors,
) -> Option<AnyElement> {
    let state = match attention.most_urgent()? {
        AttentionLevel::Input => SidebarAgentState::NeedsInput,
        AttentionLevel::Error => SidebarAgentState::Errored,
        AttentionLevel::Finished => SidebarAgentState::Finished,
    };
    let total = attention.total();
    let (color, glyph) = lane_visual(
        Lane::Agent(SidebarAgentSummary {
            state,
            count: total,
            tint: None,
        }),
        ui,
    );
    Some(
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .child(glyph)
            .when(total > 1, |mark| {
                mark.child(
                    div()
                        .text_size(crate::ui_primitives::LABEL_XS)
                        .font_weight(FontWeight::MEDIUM)
                        .font_features(super::tabular_numerals())
                        .text_color(color)
                        .child(total.to_string()),
                )
            })
            .into_any_element(),
    )
}

impl PaneFlowApp {
    pub(crate) fn close_space_menu(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_space_menu_open {
            self.sidebar_space_menu_open = false;
            cx.notify();
        }
    }

    fn space_name_and_mark(
        &self,
        space: Option<&SpaceId>,
        ui: crate::theme::UiColors,
    ) -> (String, AnyElement) {
        let declared = space.and_then(|id| self.cached_config.space(id));
        (
            declared
                .map_or(DEFAULT_SPACE_NAME, |space| space.name.as_str())
                .to_string(),
            match declared {
                Some(space) => {
                    space_mark(space.color, space.avatar.as_deref(), SPACE_MARK_SIZE, ui)
                }
                None => default_space_mark(SPACE_MARK_SIZE, ui),
            },
        )
    }

    pub(super) fn render_space_header(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.cached_config.spaces.is_empty() {
            return None;
        }
        let attention = self.space_attention(cx);
        let hidden: Vec<(Option<SpaceId>, SpaceAttention)> = space_order(&self.cached_config)
            .into_iter()
            .filter(|space| *space != self.active_space)
            .filter_map(|space| {
                let pending = attention.get(&space).copied().unwrap_or_default();
                (pending.total() > 0).then_some((space, pending))
            })
            .collect();
        Some(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.))
                .child(self.render_space_trigger(&attention, ui, cx))
                .children(self.render_space_attention_button(&hidden, ui, cx))
                .into_any_element(),
        )
    }

    fn render_space_trigger(
        &self,
        attention: &SpaceAttentionMap,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.sidebar_space_menu_open;
        let (name, mark) = self.space_name_and_mark(self.active_space.as_ref(), ui);
        let active_bg = crate::app::constants::sidebar_tab_active_background();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        squircle_skin(
            div()
                .id("sidebar-space-trigger")
                .min_w_0()
                .h(px(SIDEBAR_ACTION_BUTTON_SIZE))
                .pl(px(SIDEBAR_ROW_PADDING_X))
                .pr(px(6.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(SIDEBAR_TITLE_ROW_GAP)),
            "sidebar-space-trigger-group",
            px(SPACE_CONTROL_RADIUS),
            open.then_some(active_bg),
            (!open).then_some(hover_bg),
        )
        .role(Role::Button)
        .aria_label(format!("Space {name}"))
        .aria_expanded(open)
        .delayed_tooltip(crate::ui_primitives::text_tooltip("Switch space"))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.sidebar_space_menu_open = !open;
            cx.notify();
        }))
        .child(
            div()
                .w(px(SIDEBAR_FOLDER_SLOT_WIDTH))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(mark),
        )
        .child(
            div()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(crate::ui_primitives::BODY_EMPHASIS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(name),
        )
        .child(
            svg()
                .size(px(12.))
                .flex_none()
                .path("icons/chevron-down.svg")
                .text_color(ui.muted),
        )
        .when(open, |trigger| {
            trigger.child(self.render_space_menu(attention, ui, cx))
        })
        .into_any_element()
    }

    fn render_space_attention_button(
        &self,
        hidden: &[(Option<SpaceId>, SpaceAttention)],
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (target, level) = most_urgent_space(hidden)?;
        let target_attention = hidden
            .iter()
            .find(|(space, _)| *space == target)
            .map_or_else(SpaceAttention::default, |(_, attention)| *attention);
        let all = combined_attention(hidden);
        let label = SharedString::from(attention_button_label(
            space_label(&self.cached_config, target.as_ref()),
            target_attention,
            all.total() - target_attention.total(),
        ));
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let mark = render_attention_mark(all, ui)?;
        Some(
            squircle_skin(
                div()
                    .id("sidebar-space-attention")
                    .flex_none()
                    .h(px(SIDEBAR_ACTION_BUTTON_SIZE))
                    .px(px(5.))
                    .flex()
                    .items_center(),
                "sidebar-space-attention-group",
                px(SPACE_CONTROL_RADIUS),
                None,
                Some(hover_bg),
            )
            .role(Role::Button)
            .aria_label(label.clone())
            .delayed_tooltip(crate::ui_primitives::text_tooltip(label))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.go_to_space_attention(target.clone(), level, window, cx);
            }))
            .child(mark)
            .into_any_element(),
        )
    }

    fn go_to_space_attention(
        &mut self,
        space: Option<SpaceId>,
        level: AttentionLevel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if level == AttentionLevel::Finished {
            let unread = self.workspaces.iter().position(|ws| {
                ws.space == space
                    && !ws.muted
                    && ws.agent_completion_notification.unread_count() > 0
            });
            if let Some(idx) = unread {
                self.activate_workspace_at(idx, WorkspaceFocusTarget::FirstPane, window, cx);
                return;
            }
        } else {
            let errored = level == AttentionLevel::Error;
            let surface = self.attention_queue_rows(cx).into_iter().find_map(|row| {
                let in_space = row
                    .workspace_idx
                    .and_then(|idx| self.workspaces.get(idx))
                    .is_some_and(|ws| ws.space == space);
                (in_space && row.errored == errored)
                    .then_some(row.surface_id)
                    .flatten()
            });
            if let Some(surface_id) = surface {
                self.attention_queue_activate(surface_id, window, cx);
                return;
            }
        }
        self.activate_space(space, Some(window), cx);
    }

    fn render_space_menu(
        &self,
        attention: &SpaceAttentionMap,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut menu = menu_panel(div().id("sidebar-space-menu"), ui)
            .font_family("Geist")
            .w(px(SPACE_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                    this.close_space_menu(cx);
                }),
            );
        for (position, space) in space_order(&self.cached_config).into_iter().enumerate() {
            menu = menu.child(self.render_space_menu_row(position, space, attention, ui, cx));
        }
        menu = menu
            .child(context_menu_divider(ui))
            .child(self.render_new_space_row(ui, cx))
            .child(render_manage_spaces_row(ui, cx));

        deferred(crate::ui_primitives::menu_reveal(
            "sidebar-space-menu-reveal",
            div()
                .absolute()
                .top(px(32.))
                .left(px(0.))
                .occlude()
                .child(menu),
        ))
        .with_priority(3)
        .into_any_element()
    }

    fn render_space_menu_row(
        &self,
        position: usize,
        space: Option<SpaceId>,
        attention: &SpaceAttentionMap,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = space == self.active_space;
        let (name, mark) = self.space_name_and_mark(space.as_ref(), ui);
        let pending = if current {
            SpaceAttention::default()
        } else {
            attention.get(&space).copied().unwrap_or_default()
        };
        menu_row(
            SharedString::from(format!("sidebar-space-menu-{position}")),
            false,
            ui,
        )
        .role(Role::Button)
        .aria_label(space_row_label(&name, current, pending))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.sidebar_space_menu_open = false;
            this.activate_space(space.clone(), Some(window), cx);
            cx.notify();
        }))
        .child(
            div()
                .w(px(SPACE_MENU_ICON_SLOT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(mark),
        )
        .child(menu_label(name, ui))
        .children(render_attention_mark(pending, ui))
        .child(
            div()
                .w(px(SPACE_MENU_CHECK_SLOT))
                .flex_none()
                .when(current, |slot| {
                    slot.child(
                        svg()
                            .size(px(13.))
                            .path("icons/check.svg")
                            .text_color(ui.text),
                    )
                }),
        )
        .into_any_element()
    }

    fn render_new_space_row(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.cached_config.spaces.len() >= MAX_SPACES {
            return Self::render_disabled_select_menu_item(
                "sidebar-space-menu-new".into(),
                SPACE_LIMIT_REASON,
                ui,
            )
            .into_any_element();
        }
        menu_row("sidebar-space-menu-new", false, ui)
            .role(Role::Button)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.sidebar_space_menu_open = false;
                this.new_space_in_settings(window, cx);
            }))
            .child(menu_icon("icons/plus.svg", ui))
            .child(menu_label("New space…", ui))
            .into_any_element()
    }
}

fn menu_icon(path: &'static str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .w(px(SPACE_MENU_ICON_SLOT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .size(px(SPACE_MENU_GLYPH_SIZE))
                .flex_none()
                .path(path)
                .text_color(ui.muted),
        )
        .into_any_element()
}

fn render_manage_spaces_row(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    menu_row("sidebar-space-menu-manage", false, ui)
        .role(Role::Button)
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.sidebar_space_menu_open = false;
            this.open_settings_at(crate::SettingsSection::Spaces, window, cx);
        }))
        .child(menu_icon("icons/settings.svg", ui))
        .child(menu_label("Manage spaces", ui))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attention(input: usize, errors: usize, finished: usize) -> SpaceAttention {
        SpaceAttention {
            input,
            errors,
            finished,
        }
    }

    #[test]
    fn an_attention_label_names_each_kind_in_urgency_order() {
        assert_eq!(
            attention_label(attention(1, 0, 0)),
            "1 agent waiting for input"
        );
        assert_eq!(
            attention_label(attention(2, 1, 3)),
            "2 agents waiting for input, 1 agent failed, 3 agents finished"
        );
    }

    #[test]
    fn a_space_row_names_its_space_and_the_current_one() {
        assert_eq!(
            space_row_label("Work", false, attention(0, 0, 0)),
            "Space Work"
        );
        assert_eq!(
            space_row_label("Work", true, attention(2, 0, 0)),
            "Space Work, current"
        );
        assert_eq!(
            space_row_label("Work", false, attention(0, 1, 0)),
            "Space Work, 1 agent failed"
        );
    }

    #[test]
    fn the_attention_button_targets_input_before_an_earlier_error() {
        let failed = Some(SpaceId::new());
        let waiting = Some(SpaceId::new());
        let hidden = vec![
            (None, attention(0, 0, 2)),
            (failed, attention(0, 1, 0)),
            (waiting.clone(), attention(1, 0, 0)),
        ];
        assert_eq!(
            most_urgent_space(&hidden),
            Some((waiting, AttentionLevel::Input))
        );
        assert_eq!(most_urgent_space(&[]), None);
    }

    #[test]
    fn the_attention_button_counts_what_waits_in_other_spaces() {
        assert_eq!(
            attention_button_label("Work", attention(1, 0, 0), 0),
            "Go to Work: 1 agent waiting for input"
        );
        assert_eq!(
            attention_button_label("Work", attention(1, 0, 0), 3),
            "Go to Work: 1 agent waiting for input, 3 more in other spaces"
        );
    }
}

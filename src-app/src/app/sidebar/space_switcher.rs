use super::*;

use paneflow_config::schema::{DEFAULT_SPACE_NAME, MAX_SPACES, SpaceColor, SpaceId};

use crate::app::attention_queue::{AttentionLevel, SpaceAttention};
use crate::app::spaces::SPACE_LIMIT_REASON;

pub(super) const SPACE_BUTTON_SIZE: f32 = 22.0;
pub(super) const SPACE_BUTTON_GAP: f32 = 4.0;
const SPACE_BUTTON_RADIUS: f32 = 7.0;
const SPACE_BADGE_DOT: f32 = 6.0;
const SPACE_BADGE_PILL: f32 = 11.0;

pub(crate) fn space_tint(color: Option<SpaceColor>, ui: crate::theme::UiColors) -> gpui::Hsla {
    let theme = crate::theme::active_theme();
    match color {
        Some(SpaceColor::Red) => theme.red,
        Some(SpaceColor::Green) => theme.green,
        Some(SpaceColor::Yellow) => theme.yellow,
        Some(SpaceColor::Blue) => theme.blue,
        Some(SpaceColor::Magenta) => theme.magenta,
        Some(SpaceColor::Cyan) => theme.cyan,
        None => ui.muted,
    }
}

pub(crate) fn space_initial(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default()
}

pub(super) fn space_button_label(name: &str, current: bool, attention: usize) -> String {
    match (current, attention) {
        (true, _) => format!("Space {name}, current"),
        (false, 0) => format!("Space {name}"),
        (false, 1) => format!("Space {name}, 1 agent needs attention"),
        (false, n) => format!("Space {name}, {n} agents need attention"),
    }
}

pub(super) fn hidden_space_attention(
    current: bool,
    attention: Option<SpaceAttention>,
) -> SpaceAttention {
    attention.filter(|_| !current).unwrap_or_default()
}

fn attention_color(level: AttentionLevel, ui: crate::theme::UiColors) -> gpui::Hsla {
    match level {
        AttentionLevel::Input => crate::app::constants::sidebar_needs_input_color(),
        AttentionLevel::Error => ui.agent_error,
        AttentionLevel::Finished => crate::app::constants::sidebar_finished_color(),
    }
}

fn render_space_badge(attention: SpaceAttention, ui: crate::theme::UiColors) -> Option<AnyElement> {
    let color = attention_color(attention.most_urgent()?, ui);
    let total = attention.total();
    let badge = div().absolute().rounded_full().bg(color);
    if total == 1 {
        return Some(
            badge
                .top(px(1.))
                .right(px(1.))
                .size(px(SPACE_BADGE_DOT))
                .into_any_element(),
        );
    }
    Some(
        badge
            .top(px(-3.))
            .right(px(-3.))
            .h(px(SPACE_BADGE_PILL))
            .min_w(px(SPACE_BADGE_PILL))
            .px(px(2.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(8.))
            .line_height(px(SPACE_BADGE_PILL))
            .font_weight(FontWeight::BOLD)
            .text_color(ui.base)
            .child(total.to_string())
            .into_any_element(),
    )
}

impl PaneFlowApp {
    pub(super) fn render_space_switcher(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.cached_config.spaces.is_empty() {
            return None;
        }
        let entries = std::iter::once((None, DEFAULT_SPACE_NAME, None)).chain(
            self.cached_config
                .spaces
                .iter()
                .map(|space| (Some(space.id.clone()), space.name.as_str(), space.color)),
        );
        let attention = self.space_attention(cx);
        let mut row = div()
            .id("sidebar-space-switcher")
            .role(Role::Group)
            .aria_label("Spaces")
            .flex_none()
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .pt(px(6.))
            .flex()
            .flex_row()
            .flex_nowrap()
            .items_center()
            .gap(px(SPACE_BUTTON_GAP))
            .overflow_hidden();
        for (position, (space, name, color)) in entries.enumerate() {
            let current = space == self.active_space;
            let badge = hidden_space_attention(current, attention.get(&space).copied());
            row = row.child(
                self.render_space_button(position, space, name, color, current, badge, ui, cx),
            );
        }
        row = row.child(render_new_space_button(
            self.cached_config.spaces.len() >= MAX_SPACES,
            ui,
            cx,
        ));
        Some(row.into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_space_button(
        &self,
        position: usize,
        space: Option<SpaceId>,
        name: &str,
        color: Option<SpaceColor>,
        current: bool,
        attention: SpaceAttention,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active_bg = crate::app::constants::sidebar_tab_active_background();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let group = SharedString::from(format!("sidebar-space-{position}-group"));
        squircle_skin(
            div()
                .id(SharedString::from(format!("sidebar-space-{position}")))
                .flex_none()
                .size(px(SPACE_BUTTON_SIZE))
                .flex()
                .items_center()
                .justify_center(),
            group,
            px(SPACE_BUTTON_RADIUS),
            current.then_some(active_bg),
            (!current).then_some(hover_bg),
        )
        .role(Role::Button)
        .aria_label(space_button_label(name, current, attention.total()))
        .cursor_pointer()
        .delayed_tooltip(crate::ui_primitives::text_tooltip(name.to_string()))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.activate_space(space.clone(), Some(window), cx);
        }))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(space_tint(color, ui))
                .child(space_initial(name)),
        )
        .children(render_space_badge(attention, ui))
        .into_any_element()
    }
}

fn render_new_space_button(
    at_limit: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let hover_bg = crate::app::constants::sidebar_tab_hover_background();
    let button = squircle_skin(
        div()
            .id("sidebar-space-new")
            .flex_none()
            .size(px(SPACE_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center(),
        "sidebar-space-new-group",
        px(SPACE_BUTTON_RADIUS),
        None,
        (!at_limit).then_some(hover_bg),
    )
    .role(Role::Button)
    .child(
        svg()
            .size(px(13.))
            .flex_none()
            .path("icons/plus.svg")
            .text_color(ui.muted)
            .when(at_limit, |icon| icon.opacity(0.5)),
    );
    if at_limit {
        return button
            .aria_label(format!("New space, unavailable: {SPACE_LIMIT_REASON}"))
            .delayed_tooltip(crate::ui_primitives::text_tooltip(SPACE_LIMIT_REASON))
            .into_any_element();
    }
    button
        .aria_label("New space")
        .cursor_pointer()
        .delayed_tooltip(crate::ui_primitives::text_tooltip("New space"))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.new_space_in_settings(window, cx);
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_space_buttons_fit_the_sidebar_on_one_line() {
        let slots = (MAX_SPACES + 2) as f32;
        let width =
            slots * SPACE_BUTTON_SIZE + (slots - 1.) * SPACE_BUTTON_GAP + SIDEBAR_ROW_MARGIN_X * 2.;
        assert!(
            width <= SIDEBAR_WIDTH,
            "{slots} buttons need {width}px, the sidebar has {SIDEBAR_WIDTH}px"
        );
    }

    #[test]
    fn a_space_button_names_its_space_and_the_current_one() {
        assert_eq!(space_button_label("Work", false, 0), "Space Work");
        assert_eq!(space_button_label("Work", true, 0), "Space Work, current");
    }

    #[test]
    fn a_hidden_space_button_announces_its_waiting_agents() {
        assert_eq!(
            space_button_label("Work", false, 1),
            "Space Work, 1 agent needs attention"
        );
        assert_eq!(
            space_button_label("Work", false, 3),
            "Space Work, 3 agents need attention"
        );
    }

    #[test]
    fn the_active_space_shows_no_badge() {
        let waiting = SpaceAttention {
            input: 2,
            errors: 1,
            finished: 0,
        };
        assert_eq!(
            hidden_space_attention(true, Some(waiting)),
            SpaceAttention::default()
        );
        assert_eq!(hidden_space_attention(false, Some(waiting)), waiting);
        assert_eq!(
            hidden_space_attention(false, None),
            SpaceAttention::default()
        );
    }

    #[test]
    fn a_space_shows_the_uppercase_first_letter_of_its_name() {
        assert_eq!(space_initial("work"), "W");
        assert_eq!(space_initial("  élan"), "É");
        assert_eq!(space_initial(""), "");
    }
}

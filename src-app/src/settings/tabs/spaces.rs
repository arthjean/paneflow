use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
    svg,
};
use paneflow_config::schema::{
    DEFAULT_SPACE_NAME, MAX_SPACES, SpaceColor, SpaceConfig, SpaceId, validate_space_name,
};

use crate::PaneFlowApp;
use crate::app::spaces::SPACE_LIMIT_REASON;
use crate::settings::components::{
    CARD_PADDING_X, CARD_PADDING_Y, SETTINGS_CONTROL_CORNER_RADIUS, destructive_button,
    destructive_color, hairline_inset, secondary_button, section_title, setting_card, with_alpha,
};
use crate::settings::search::Block;
use crate::ui_primitives::{
    AccessibleControlExt, BODY, LABEL_SM, ROW_RADIUS, TooltipDelayExt, text_tooltip,
};

const NAME_WIDTH: f32 = 220.;
const INPUT_HEIGHT: f32 = 28.;
const SWATCH_SIZE: f32 = 16.;
const LETTER_SIZE: f32 = 22.;

pub(crate) struct SpaceNameEditor {
    pub(crate) id: SpaceId,
    pub(crate) error: Option<&'static str>,
}

pub(crate) const SPACE_COLORS: [SpaceColor; 6] = [
    SpaceColor::Red,
    SpaceColor::Green,
    SpaceColor::Yellow,
    SpaceColor::Blue,
    SpaceColor::Magenta,
    SpaceColor::Cyan,
];

pub(crate) fn space_color_name(color: SpaceColor) -> &'static str {
    match color {
        SpaceColor::Red => "Red",
        SpaceColor::Green => "Green",
        SpaceColor::Yellow => "Yellow",
        SpaceColor::Blue => "Blue",
        SpaceColor::Magenta => "Magenta",
        SpaceColor::Cyan => "Cyan",
    }
}

pub(crate) fn space_delete_message(workspaces: usize) -> String {
    match workspaces {
        1 => "Its 1 workspace moves to Default".to_string(),
        n => format!("Its {n} workspaces move to Default"),
    }
}

pub(crate) fn renamed_spaces(
    spaces: &[SpaceConfig],
    id: &SpaceId,
    name: &str,
) -> Result<Option<Vec<SpaceConfig>>, &'static str> {
    let others = spaces
        .iter()
        .filter(|space| &space.id != id)
        .map(|space| space.name.as_str());
    let name = validate_space_name(name, others).map_err(|error| error.message())?;
    let mut next = spaces.to_vec();
    let Some(space) = next.iter_mut().find(|space| &space.id == id) else {
        return Ok(None);
    };
    if space.name == name {
        return Ok(None);
    }
    space.name = name;
    Ok(Some(next))
}

impl PaneFlowApp {
    pub(crate) fn edit_space_name(
        &mut self,
        id: SpaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .cached_config
            .space(&id)
            .map(|space| space.name.clone())
        else {
            return;
        };
        self.space_name_input.update(cx, |input, cx| {
            input.set_value(SharedString::from(name), cx)
        });
        self.space_name_editor = Some(SpaceNameEditor { id, error: None });
        self.space_delete_armed = None;
        let focus = self.space_name_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn cancel_space_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_name_editor = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn commit_space_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.space_name_editor.as_ref() else {
            return;
        };
        let id = editor.id.clone();
        let name = self.space_name_input.read(cx).value();
        match renamed_spaces(&self.cached_config.spaces, &id, &name) {
            Err(message) => {
                if let Some(editor) = self.space_name_editor.as_mut() {
                    editor.error = Some(message);
                }
                cx.notify();
            }
            Ok(next) => {
                self.space_name_editor = None;
                if let Some(spaces) = next {
                    self.persist_spaces(spaces, cx);
                }
                self.settings_focus.focus(window, cx);
                cx.notify();
            }
        }
    }

    fn set_space_color(&mut self, id: &SpaceId, color: Option<SpaceColor>, cx: &mut Context<Self>) {
        let mut spaces = self.cached_config.spaces.clone();
        let Some(space) = spaces.iter_mut().find(|space| &space.id == id) else {
            return;
        };
        if space.color == color {
            return;
        }
        space.color = color;
        self.persist_spaces(spaces, cx);
    }

    fn delete_space(&mut self, id: &SpaceId, cx: &mut Context<Self>) {
        self.space_delete_armed = None;
        if self
            .space_name_editor
            .as_ref()
            .is_some_and(|editor| &editor.id == id)
        {
            self.space_name_editor = None;
        }
        let spaces: Vec<SpaceConfig> = self
            .cached_config
            .spaces
            .iter()
            .filter(|space| &space.id != id)
            .cloned()
            .collect();
        self.persist_spaces(spaces, cx);
    }

    fn space_workspace_count(&self, id: Option<&SpaceId>) -> usize {
        self.workspaces
            .iter()
            .filter(|workspace| crate::app::spaces::in_space(workspace, id))
            .count()
    }

    pub(crate) fn render_spaces_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let spaces = &self.cached_config.spaces;
        let mut card = setting_card(ui).py(px(CARD_PADDING_Y));
        card = card.child(self.render_default_space_row(ui));
        for space in spaces {
            card = card.child(hairline_inset(ui));
            card = card.child(self.render_space_row(space, ui, cx));
        }
        card = card.child(hairline_inset(ui));
        card = card.child(render_new_space_row(spaces.len() >= MAX_SPACES, ui, cx));

        div().flex().flex_col().child(
            Block::new("Spaces")
                .child(section_title(
                    ui,
                    "Spaces",
                    Some(
                        "Group workspaces into contexts, such as work and personal projects. \
                         A space organizes your workspaces; it does not isolate them.",
                    ),
                ))
                .child(card)
                .finish(),
        )
    }

    fn render_default_space_row(&self, ui: crate::theme::UiColors) -> AnyElement {
        let count = self.space_workspace_count(None);
        space_row_shell()
            .child(space_letter(DEFAULT_SPACE_NAME, None, ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(BODY)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .child(DEFAULT_SPACE_NAME),
                    )
                    .child(
                        div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(workspace_count_label(count, "Workspaces without a space")),
                    ),
            )
            .into_any_element()
    }

    fn render_space_row(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.space_workspace_count(Some(&space.id));
        let editing = self
            .space_name_editor
            .as_ref()
            .filter(|editor| editor.id == space.id);
        let armed = self.space_delete_armed.as_ref() == Some(&space.id);
        let key = space.id.to_string();

        let name_field = match editing {
            Some(_) => div()
                .id(SharedString::from(format!("space-name-{key}")))
                .w(px(NAME_WIDTH))
                .h(px(INPUT_HEIGHT))
                .flex()
                .items_center()
                .px(px(10.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.subtle)
                .border_1()
                .border_color(with_alpha(ui.text, 0.25))
                .text_size(BODY)
                .text_color(ui.text)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "enter" => {
                            this.commit_space_name(window, cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            this.cancel_space_name(window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                    if this.space_name_editor.is_some() {
                        this.commit_space_name(window, cx);
                    }
                }))
                .child(self.space_name_input.clone())
                .into_any_element(),
            None => {
                let id = space.id.clone();
                div()
                    .id(SharedString::from(format!("space-name-{key}")))
                    .w(px(NAME_WIDTH))
                    .h(px(INPUT_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(ui.subtle)
                    .cursor(CursorStyle::IBeam)
                    .text_size(BODY)
                    .text_color(ui.text)
                    .accessible_control(
                        gpui::accesskit::Role::TextInput,
                        format!("Name of space {}", space.name),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.edit_space_name(id.clone(), window, cx);
                    }))
                    .child(div().truncate().child(space.name.clone()))
                    .into_any_element()
            }
        };

        let id = space.id.clone();
        let mut row = space_row_shell()
            .child(space_letter(&space.name, space.color, ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(name_field)
                    .child(match editing.and_then(|editor| editor.error) {
                        Some(error) => div()
                            .text_size(LABEL_SM)
                            .text_color(destructive_color())
                            .child(error),
                        None => div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(workspace_count_label(count, "No workspaces")),
                    }),
            )
            .child(self.render_space_colors(space, ui, cx))
            .child(
                div()
                    .id(SharedString::from(format!("space-delete-{key}")))
                    .flex_none()
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(with_alpha(ui.text, 0.06)))
                    .role(gpui::accesskit::Role::Button)
                    .aria_label(format!("Delete space {}", space.name))
                    .delayed_tooltip(text_tooltip("Delete space"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.space_delete_armed = Some(id.clone());
                        this.space_name_editor = None;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/trash.svg")
                            .text_color(if armed { destructive_color() } else { ui.muted }),
                    ),
            );

        if armed {
            let id = space.id.clone();
            row = row.flex_wrap();
            let confirm = div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .pt(px(8.))
                .child(
                    div()
                        .text_size(LABEL_SM)
                        .text_color(ui.text)
                        .child(space_delete_message(count)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(6.))
                        .child(secondary_button(
                            "space-delete-cancel",
                            "Cancel",
                            ui,
                            cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.space_delete_armed = None;
                                cx.notify();
                            }),
                        ))
                        .child(
                            destructive_button("space-delete-confirm", "Delete").on_click(
                                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                    this.delete_space(&id, cx);
                                }),
                            ),
                        ),
                );
            row = row.child(confirm);
        }
        row.into_any_element()
    }

    fn render_space_colors(
        &self,
        space: &SpaceConfig,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = space.id.to_string();
        let mut swatches = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.));
        let choices = std::iter::once(None).chain(SPACE_COLORS.into_iter().map(Some));
        for choice in choices {
            let selected = space.color == choice;
            let id = space.id.clone();
            let label = choice.map_or("No color", space_color_name);
            let tint = crate::app::sidebar::space_tint(choice, ui);
            let swatch_id = format!(
                "space-color-{key}-{}",
                choice.map_or("none", |color| space_color_name(color))
            );
            swatches = swatches.child(
                div()
                    .id(SharedString::from(swatch_id))
                    .size(px(SWATCH_SIZE + 6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor(CursorStyle::PointingHand)
                    .when(selected, |swatch| {
                        swatch.border_1().border_color(with_alpha(ui.text, 0.6))
                    })
                    .role(gpui::accesskit::Role::RadioButton)
                    .aria_label(format!(
                        "{label}{}",
                        if selected { ", selected" } else { "" }
                    ))
                    .delayed_tooltip(text_tooltip(label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_space_color(&id, choice, cx);
                    }))
                    .child(match choice {
                        Some(_) => div()
                            .size(px(SWATCH_SIZE))
                            .rounded_full()
                            .bg(tint)
                            .into_any_element(),
                        None => div()
                            .size(px(SWATCH_SIZE))
                            .rounded_full()
                            .border_1()
                            .border_color(ui.muted)
                            .into_any_element(),
                    }),
            );
        }
        swatches.into_any_element()
    }
}

fn space_row_shell() -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .px(px(CARD_PADDING_X))
        .py(px(10.))
}

fn space_letter(name: &str, color: Option<SpaceColor>, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_none()
        .size(px(LETTER_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(ROW_RADIUS)
        .bg(with_alpha(ui.text, 0.06))
        .text_size(LABEL_SM)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(crate::app::sidebar::space_tint(color, ui))
        .child(crate::app::sidebar::space_initial(name))
        .into_any_element()
}

fn workspace_count_label(count: usize, empty: &'static str) -> String {
    match count {
        0 => empty.to_string(),
        1 => "1 workspace".to_string(),
        n => format!("{n} workspaces"),
    }
}

fn render_new_space_row(
    at_limit: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let row = div()
        .id("space-new")
        .h(px(36.))
        .px(px(CARD_PADDING_X))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .text_size(BODY)
        .text_color(ui.muted)
        .child("+ New space");
    if at_limit {
        return row
            .opacity(0.5)
            .accessible_control(
                gpui::accesskit::Role::Button,
                format!("New space, unavailable: {SPACE_LIMIT_REASON}"),
            )
            .child(div().text_size(LABEL_SM).child(SPACE_LIMIT_REASON))
            .into_any_element();
    }
    row.cursor(CursorStyle::PointingHand)
        .accessible_control(gpui::accesskit::Role::Button, "New space")
        .hover(move |style| style.text_color(ui.text).bg(with_alpha(ui.text, 0.05)))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            if let Some(id) = this.add_space(cx) {
                this.edit_space_name(id, window, cx);
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(n: u8, name: &str) -> SpaceConfig {
        SpaceConfig {
            id: SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap(),
            name: name.to_string(),
            color: None,
        }
    }

    #[test]
    fn renaming_a_space_rejects_invalid_names_and_writes_nothing() {
        let spaces = vec![space(1, "Work"), space(2, "Perso")];
        let id = spaces[0].id.clone();
        assert_eq!(renamed_spaces(&spaces, &id, "  "), Err("Name required"));
        assert_eq!(
            renamed_spaces(&spaces, &id, "perso"),
            Err("Name already used")
        );
        assert_eq!(
            renamed_spaces(&spaces, &id, "default"),
            Err("Default is reserved")
        );
        assert_eq!(
            renamed_spaces(&spaces, &id, &"x".repeat(33)),
            Err("32 characters maximum")
        );
    }

    #[test]
    fn renaming_a_space_keeps_its_own_name_and_trims_the_new_one() {
        let spaces = vec![space(1, "Work"), space(2, "Perso")];
        let id = spaces[0].id.clone();
        assert_eq!(renamed_spaces(&spaces, &id, "Work"), Ok(None));
        let renamed = renamed_spaces(&spaces, &id, "  Client A ")
            .unwrap()
            .unwrap();
        assert_eq!(renamed[0].name, "Client A");
        assert_eq!(renamed[1].name, "Perso");
    }

    #[test]
    fn deleting_a_space_names_how_many_workspaces_move() {
        assert_eq!(space_delete_message(3), "Its 3 workspaces move to Default");
        assert_eq!(space_delete_message(1), "Its 1 workspace moves to Default");
    }
}

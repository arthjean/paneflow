use std::ops::Range;

use gpui::{
    AnyElement, Div, ElementId, FontWeight, HighlightStyle, Hsla, InteractiveElement,
    ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled, StyledText,
    accesskit::{Role, Toggled},
    div, px,
};

use crate::settings::components::{MENU_RADIUS, select_menu_surface, surface_edge, with_alpha};
use crate::theme::UiColors;
use crate::ui_primitives::interface_style::{CLASSIC_CONTROL, classic, pick};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill_beneath_border};

pub(crate) const PANEL_RADIUS: Pixels = px(13.);
pub(crate) const PANEL_PADDING: f32 = 6.;
pub(crate) const FIELD_INSET: f32 = 2.;
pub(crate) const FIELD_GAP: f32 = 6.;
pub(crate) const ROW_HEIGHT: f32 = 29.;
pub(crate) const ROW_SPACING: f32 = 2.;
pub(crate) const ROW_RADIUS: Pixels = px(8.);
pub(crate) const ROW_TEXT_INSET: f32 = 11.;
pub(crate) const ROW_GAP: f32 = 8.;
pub(crate) const ICON_SIZE: f32 = 14.;
pub(crate) const ICON_GAP: f32 = 8.;
pub(crate) const LABEL_SIZE: f32 = 14.;

const HIGHLIGHT_WASH: f32 = 0.08;

const CLASSIC_ROW_HEIGHT: f32 = 36.;
const CLASSIC_ROW_SPACING: f32 = 0.;
const CLASSIC_HIGHLIGHT_WASH: f32 = 0.14;

pub(crate) struct RowColors {
    pub(crate) fill: Hsla,
    pub(crate) label: Hsla,
    pub(crate) muted: Hsla,
}

pub(crate) fn row_colors(selected: bool, ui: UiColors) -> RowColors {
    if selected && !classic() {
        return RowColors {
            fill: crate::theme::selection_color(),
            label: crate::theme::on_selection_color(),
            muted: crate::theme::on_selection_muted(),
        };
    }
    RowColors {
        fill: with_alpha(ui.text, pick(HIGHLIGHT_WASH, CLASSIC_HIGHLIGHT_WASH)),
        label: ui.text,
        muted: ui.muted,
    }
}

pub(crate) fn row_radius() -> Pixels {
    pick(ROW_RADIUS, CLASSIC_CONTROL)
}

pub(crate) fn row_height() -> f32 {
    pick(ROW_HEIGHT, CLASSIC_ROW_HEIGHT)
}

pub(crate) fn row_spacing() -> f32 {
    pick(ROW_SPACING, CLASSIC_ROW_SPACING)
}

pub(crate) fn panel_radius() -> Pixels {
    pick(PANEL_RADIUS, MENU_RADIUS)
}

pub(crate) fn shell(
    panel: Stateful<Div>,
    width: Pixels,
    ui: UiColors,
    field: AnyElement,
    list: AnyElement,
) -> Stateful<Div> {
    panel
        .relative()
        .child(squircle_fill_beneath_border(
            PANEL_RADIUS,
            select_menu_surface(ui),
            px(1.),
            surface_edge(ui),
        ))
        .child(squircle_border(PANEL_RADIUS, px(1.), surface_edge(ui)))
        .flex()
        .flex_col()
        .p(px(PANEL_PADDING))
        .w(width)
        .child(field)
        .child(list)
}

pub(crate) fn highlighted_label(label: &str, highlights: &[Range<usize>]) -> StyledText {
    StyledText::new(label.to_string()).with_highlights(highlights.iter().map(|range| {
        (
            range.clone(),
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        )
    }))
}

pub(crate) struct PickOption {
    pub(crate) label: SharedString,
    pub(crate) description: Option<SharedString>,
    pub(crate) shortcut: Option<SharedString>,
    pub(crate) checked: Option<bool>,
    pub(crate) selected: bool,
    pub(crate) index: usize,
    pub(crate) count: usize,
}

impl PickOption {
    pub(crate) fn new(label: impl Into<SharedString>, index: usize, count: usize) -> Self {
        Self {
            label: label.into(),
            description: None,
            shortcut: None,
            checked: None,
            selected: false,
            index,
            count,
        }
    }

    pub(crate) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub(crate) fn description(mut self, description: Option<impl Into<SharedString>>) -> Self {
        self.description = description.map(Into::into);
        self
    }

    pub(crate) fn shortcut(mut self, shortcut: Option<impl Into<SharedString>>) -> Self {
        self.shortcut = shortcut.map(Into::into);
        self
    }

    pub(crate) fn checked(mut self, checked: Option<bool>) -> Self {
        self.checked = checked;
        self
    }
}

pub(crate) fn list_box(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::ListBox)
        .aria_label(label)
        .w_full()
        .flex()
        .flex_col()
}

pub(crate) fn expose_option<E: StatefulInteractiveElement>(row: E, option: PickOption) -> E {
    let mut row = row
        .role(Role::ListBoxOption)
        .aria_label(option.label)
        .aria_selected(option.selected)
        .aria_position_in_set(option.index + 1)
        .aria_size_of_set(option.count);
    if let Some(description) = option.description {
        row = row.aria_description(description);
    }
    if let Some(shortcut) = option.shortcut {
        row = row.aria_keyshortcuts(shortcut);
    }
    if let Some(checked) = option.checked {
        row = row.aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        });
    }
    row
}

pub(crate) fn empty_notice(
    id: impl Into<ElementId>,
    message: impl Into<SharedString>,
    ui: UiColors,
) -> Stateful<Div> {
    let message: SharedString = message.into();
    div()
        .id(id)
        .role(Role::Label)
        .aria_label(message.clone())
        .h(px(ROW_HEIGHT))
        .px(px(ROW_TEXT_INSET))
        .flex()
        .items_center()
        .text_size(px(LABEL_SIZE))
        .text_color(ui.muted)
        .child(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_primitives::interface_style::{Shape, scoped, with_rounded_shape};

    #[test]
    fn classic_rows_swap_the_selection_fill_for_a_neutral_wash() {
        let ui = crate::theme::ui_colors();
        assert_eq!(row_colors(true, ui).fill, crate::theme::selection_color());
        assert_eq!(row_radius(), ROW_RADIUS);
        assert_eq!((row_height(), row_spacing()), (ROW_HEIGHT, ROW_SPACING));
        assert_eq!(panel_radius(), PANEL_RADIUS);
        scoped(Shape::Classic, || {
            let colors = row_colors(true, ui);
            assert_eq!(colors.fill, with_alpha(ui.text, CLASSIC_HIGHLIGHT_WASH));
            assert_eq!(colors.label, ui.text);
            assert_eq!(row_radius(), CLASSIC_CONTROL);
            assert_eq!((row_height(), row_spacing()), (36., 0.));
            assert_eq!(panel_radius(), MENU_RADIUS);
        });
    }

    #[test]
    fn the_command_palette_scope_keeps_the_rounded_rows_in_the_classic_style() {
        let ui = crate::theme::ui_colors();
        scoped(Shape::Classic, || {
            with_rounded_shape(|| {
                assert_eq!(row_colors(true, ui).fill, crate::theme::selection_color());
            });
        });
    }

    fn exposed<E: gpui::Element>(element: &E) -> gpui::accesskit::Node {
        let role = element
            .a11y_role()
            .expect("the element exposes an accessibility role");
        let mut node = gpui::accesskit::Node::new(role);
        element.write_a11y_info(&mut node);
        node
    }

    #[test]
    fn a_pick_option_reports_its_name_state_and_place_in_the_visible_list() {
        let plain = exposed(&expose_option(
            div().id("plain"),
            PickOption::new("Split right", 0, 3),
        ));
        assert_eq!(plain.role(), Role::ListBoxOption);
        assert_eq!(plain.label(), Some("Split right"));
        assert_eq!(plain.is_selected(), Some(false));
        assert_eq!(plain.position_in_set(), Some(1));
        assert_eq!(plain.size_of_set(), Some(3));
        assert_eq!(plain.description(), None);
        assert_eq!(plain.keyboard_shortcut(), None);
        assert_eq!(plain.toggled(), None);
        let full = exposed(&expose_option(
            div().id("full"),
            PickOption::new("Word wrap", 2, 3)
                .selected(true)
                .description(Some("Editor"))
                .shortcut(Some("⌥Z"))
                .checked(Some(false)),
        ));
        assert_eq!(full.is_selected(), Some(true));
        assert_eq!(full.position_in_set(), Some(3));
        assert_eq!(full.description(), Some("Editor"));
        assert_eq!(full.keyboard_shortcut(), Some("⌥Z"));
        assert_eq!(full.toggled(), Some(Toggled::False));
    }

    #[test]
    fn every_quick_pick_list_exposes_its_rows_as_options() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let mut pickers = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs")
                    || path.ends_with("ui_primitives/quick_pick.rs")
                {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let production = source.split("#[cfg(test)]").next().unwrap_or_default();
                if production.contains("row_colors(") {
                    pickers.push((path, production.to_string()));
                }
            }
        }
        assert!(
            pickers.len() >= 5,
            "{} quick pick lists found",
            pickers.len()
        );
        let missing: Vec<String> = pickers
            .iter()
            .filter(|(_, source)| {
                !source.contains("expose_option(")
                    || !(source.contains("list_box(") || source.contains("Role::ListBox"))
            })
            .map(|(path, _)| path.strip_prefix(&root).unwrap().display().to_string())
            .collect();
        assert!(
            missing.is_empty(),
            "a quick pick row must be a list box option inside a list box, through expose_option and list_box: {missing:?}"
        );
    }
}

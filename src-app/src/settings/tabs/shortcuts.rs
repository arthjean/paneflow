//! "Shortcuts" settings tab - grouped, searchable list of every rebindable
//! action with click-to-record key capture.
//!
//! The page used to be one flat card of ~80 rows in registry order, which made
//! finding a binding a scrolling exercise and answering "what already owns this
//! chord?" impossible. Three things fix that:
//!
//! - **Sections.** Rows are filed under [`ShortcutGroup`], declared on the
//!   action in `keybindings::registry` rather than implied by table order.
//!   Each section collapses, and a header control folds or unfolds all of them.
//! - **Text filter.** One field matching the action description *and* the
//!   rendered keystroke, so "workspace" and "ctrl+shift" both narrow the list.
//! - **Key capture.** A toggle that turns the next pressed chord into the
//!   filter (the VS Code / KDE recipe). Text search cannot answer "who owns
//!   this key?" unless you already know how the chord is spelled; capture can.
//!
//! Filtering auto-expands: a collapsed section that contains a match opens for
//! the duration of the query, so a hit is never hidden behind a closed header.
//! Rebind capture itself is still driven by
//! `PaneFlowApp::handle_shortcut_recording` (in `app::settings`), and every row
//! carries its index into the *unfiltered* `effective_shortcuts`, because that
//! is what the rebind keys off.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, Styled, div,
    prelude::*, px, svg,
};

use crate::keybindings::ShortcutGroup;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, hairline, secondary_button, setting_card,
};
use crate::ui_primitives::{LABEL_SM, ROW_RADIUS, squircle_skin};
use crate::{PaneFlowApp, config_writer, keybindings};

/// A row that survived the filter, paired with its index into the unfiltered
/// `effective_shortcuts` - the index the rebind must use.
struct VisibleRow<'a> {
    idx: usize,
    entry: &'a keybindings::ShortcutEntry,
}

impl PaneFlowApp {
    /// Rows matching the active filter, bucketed by section in display order.
    ///
    /// Matching is case-insensitive and substring-based over both the action
    /// description and the displayed keystroke. A captured chord is compared
    /// against the whole keystroke instead, since a chord is an exact thing:
    /// substring-matching "Ctrl+C" would also drag in "Ctrl+Shift+C".
    fn filtered_shortcut_groups(
        &self,
        cx: &Context<Self>,
    ) -> Vec<(ShortcutGroup, Vec<VisibleRow<'_>>)> {
        let query = self
            .shortcut_search_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();

        let matches = |entry: &keybindings::ShortcutEntry| -> bool {
            if query.is_empty() {
                return true;
            }
            entry.description.to_lowercase().contains(&query)
                || entry.key.to_lowercase().contains(&query)
                // `key` renders Apple glyphs on macOS, so the ASCII spellings
                // are what makes "cmd+shift" / "ctrl+shift" find anything there.
                || entry.search_key.contains(&query)
        };

        ShortcutGroup::ALL
            .iter()
            .filter_map(|group| {
                let rows: Vec<VisibleRow<'_>> = self
                    .effective_shortcuts
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.group == *group && matches(entry))
                    .map(|(idx, entry)| VisibleRow { idx, entry })
                    .collect();
                (!rows.is_empty()).then_some((*group, rows))
            })
            .collect()
    }

    pub(crate) fn render_shortcuts_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let filtering = !self
            .shortcut_search_input
            .read(cx)
            .value()
            .trim()
            .is_empty();

        let groups = self.filtered_shortcut_groups(cx);
        let total_visible: usize = groups.iter().map(|(_, rows)| rows.len()).sum();

        let toolbar = self.render_shortcut_toolbar(ui, filtering, cx);

        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(toolbar)
            .child(self.render_shortcut_group_controls(ui, &groups, cx));

        if groups.is_empty() {
            column = column.child(
                setting_card(ui).p(px(4.)).child(
                    div()
                        .px(px(8.))
                        .py(px(14.))
                        .text_size(px(12.))
                        .text_color(ui.muted)
                        .child("No shortcut matches this filter"),
                ),
            );
        } else {
            for (group, rows) in &groups {
                column =
                    column.child(self.render_shortcut_section(ui, *group, rows, filtering, cx));
            }
        }

        let hint = if self.shortcut_capture_active {
            "Press a chord to find what owns it. Escape to leave capture mode."
        } else {
            "Click a row to record a new shortcut. Escape to cancel."
        };

        column.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .pt(px(2.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(ui.muted)
                        .child(hint.to_string()),
                )
                .child(div().text_size(px(11.)).text_color(ui.muted).child(format!(
                    "{total_visible} of {}",
                    self.effective_shortcuts.len()
                ))),
        )
    }

    /// Search field + key-capture toggle + "Reset to defaults".
    fn render_shortcut_toolbar(
        &self,
        ui: crate::theme::UiColors,
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capture_active = self.shortcut_capture_active;

        // One field in both modes. In capture mode the interceptor writes the
        // pressed chord straight into it, so the user always reads back exactly
        // what was captured instead of trusting an invisible filter.
        let field = crate::ui_primitives::filter_pill(
            "shortcut-search",
            "shortcut-search-clear",
            ui,
            self.shortcut_search_input.clone(),
            filtering,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.clear_shortcut_filters(cx);
                cx.notify();
            }),
        )
        .flex_1()
        .min_w_0();

        let capture_toggle = squircle_skin(
            div()
                .id("shortcut-capture-toggle")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.)),
            "shortcut-capture-skin",
            ROW_RADIUS,
            // Armed state is a resting fill, not just a hover: the mode
            // swallows keystrokes, so it must be visible without pointing at it.
            capture_active.then_some(ui.accent),
            Some(ui.subtle),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            let next = !this.shortcut_capture_active;
            this.set_shortcut_capture(next, cx);
            if next {
                // The chord has to land on the settings surface, not on
                // whatever held focus before.
                this.settings_focus.focus(window, cx);
            }
            cx.notify();
        }))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/keyboard.svg")
                .text_color(if capture_active { ui.text } else { ui.muted }),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(if capture_active { ui.text } else { ui.muted })
                .child(if capture_active {
                    "Capturing"
                } else {
                    "Find by key"
                }),
        );

        let reset_btn = secondary_button(
            "reset-shortcuts",
            "Reset to defaults",
            ui,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                config_writer::reset_shortcuts();
                let config = paneflow_config::loader::load_config();
                keybindings::apply_keybindings(cx, &config.shortcuts);
                this.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
                this.recording_shortcut_idx = None;
                cx.notify();
            }),
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(field)
            .child(capture_toggle)
            .child(reset_btn)
            .into_any_element()
    }

    /// The "Expand all / Collapse all" control above the sections.
    fn render_shortcut_group_controls(
        &self,
        ui: crate::theme::UiColors,
        groups: &[(ShortcutGroup, Vec<VisibleRow<'_>>)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Offer whichever action actually changes something: if every visible
        // section is already folded, the only useful verb is "expand".
        let all_collapsed = !groups.is_empty()
            && groups
                .iter()
                .all(|(group, _)| self.collapsed_shortcut_groups.contains(group));
        let (label, collapse) = if all_collapsed {
            ("Expand all", false)
        } else {
            ("Collapse all", true)
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child("Bindings"),
            )
            .child(secondary_button(
                "shortcut-toggle-all",
                label,
                ui,
                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.collapsed_shortcut_groups.clear();
                    if collapse {
                        this.collapsed_shortcut_groups
                            .extend(ShortcutGroup::ALL.iter().copied());
                    }
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    /// One collapsible section: a clickable header, then its rows.
    fn render_shortcut_section(
        &self,
        ui: crate::theme::UiColors,
        group: ShortcutGroup,
        rows: &[VisibleRow<'_>],
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A filter overrides the fold: hiding a match behind a closed header
        // would make the search look broken. The user's fold state is kept, not
        // cleared, so it comes back when the query does.
        let collapsed = !filtering && self.collapsed_shortcut_groups.contains(&group);
        let count = rows.len();

        let header = squircle_skin(
            div()
                .id(("shortcut-group", group as usize))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(6.)),
            format!("shortcut-group-skin-{}", group as usize),
            ROW_RADIUS,
            None,
            Some(ui.subtle),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            if !this.collapsed_shortcut_groups.remove(&group) {
                this.collapsed_shortcut_groups.insert(group);
            }
            cx.notify();
        }))
        .child(
            svg()
                .size(px(11.))
                .flex_none()
                .path(if collapsed {
                    "icons/chevron-right.svg"
                } else {
                    "icons/chevron-down.svg"
                })
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.text)
                .truncate()
                .child(group.label()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(ui.muted)
                .child(count.to_string()),
        );

        let mut section = div().flex().flex_col().gap(px(6.)).child(header);

        if !collapsed {
            let mut card = setting_card(ui).p(px(4.));
            for (position, row) in rows.iter().enumerate() {
                card = card.child(self.render_shortcut_row(ui, row, cx));
                if position + 1 != count {
                    card = card.child(hairline(ui));
                }
            }
            section = section.child(card);
        }

        section.into_any_element()
    }

    fn render_shortcut_row(
        &self,
        ui: crate::theme::UiColors,
        row: &VisibleRow<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = row.idx;
        let is_recording = self.recording_shortcut_idx == Some(idx);
        let unassigned = row.entry.key == "Unassigned";

        let key_badge = if is_recording {
            div()
                .px(px(10.))
                .py(px(3.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.accent)
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child("Press a key…")
        } else {
            div()
                .px(px(10.))
                .py(px(3.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.subtle)
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                // An unassigned row is an absence, not a binding, so it reads
                // muted instead of sitting at the same weight as a real chord.
                .text_color(if unassigned { ui.muted } else { ui.text })
                .child(row.entry.key.clone())
        };

        squircle_skin(
            div()
                .id(("shortcut", idx))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(8.))
                .py(px(10.)),
            format!("shortcut-squircle-{idx}"),
            ROW_RADIUS,
            None,
            Some(ui.subtle),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            // Recording a rebind and capturing a search chord both want the
            // keyboard; arming one disarms the other.
            this.set_shortcut_capture(false, cx);
            this.recording_shortcut_idx = Some(idx);
            this.settings_focus.focus(window, cx);
            cx.notify();
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .text_color(ui.text)
                .truncate()
                .child(row.entry.description.clone()),
        )
        .child(key_badge)
        .into_any_element()
    }
}

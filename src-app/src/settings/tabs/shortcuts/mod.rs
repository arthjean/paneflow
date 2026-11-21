mod filter;
mod motion;

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, MouseButton, ParentElement, Pixels, Stateful,
    StatefulInteractiveElement, Styled, Window, div, list, prelude::*, px, svg,
};

use crate::keybindings::{ShortcutEntry, ShortcutGroup};
use crate::settings::chrome::{SETTINGS_COLUMN_PADDING, settings_column};
use crate::settings::components::with_alpha;
use crate::settings::kit::{self, Ink, Size, Tone};
use crate::settings::search;
use crate::ui_primitives::{
    AccessibleControlExt, FilterFieldShape, FilterFieldStyle, TooltipDelayExt, filter_field,
    highlight_ranges, text_tooltip,
};
use crate::{PaneFlowApp, config_writer, keybindings};

pub(crate) use filter::{ShortcutFilter, ShortcutSection};
pub(crate) use motion::ShortcutMotion;

const SECTION_GAP: Pixels = px(kit::SECTION_GAP);
const LIST_BOTTOM: Pixels = px(32.);
const HEADING_GAP: Pixels = px(16.);
const SEARCH_GAP: Pixels = px(20.);
const NOTE_GAP: Pixels = px(4.);
const NOTE_TEXT: Pixels = px(kit::SIZE_MD);
const NOTE_LINE: Pixels = px(20.);
const SECTION_TITLE_LINE: Pixels = px(kit::LINE_XS);
const SECTION_HINT_LINE: Pixels = px(kit::LINE_SM);
const SECTION_HINT_GAP: Pixels = px(4.);
const SECTION_TITLE_GAP: Pixels = px(8.);
const ROW_HEIGHT: Pixels = px(44.);
const ROW_GAP: Pixels = px(16.);
const RULE: Pixels = px(1.);
const CONTROL_HEIGHT: Pixels = px(28.);
const CONTROL_RADIUS: Pixels = px(6.);
const CONTROL_GAP: Pixels = px(8.);
const CONTROL_TEXT: Pixels = px(kit::SIZE_SM);
const RECORDER_MIN_WIDTH: Pixels = px(64.);
const RECORDER_PADDING_X: Pixels = px(10.);
const LISTENING_TINT: f32 = 0.14;
const HELD_TINT: f32 = 0.14;
const HELD_EDGE: f32 = 0.5;
const UNDO_ICON: &str = "icons/settings/undo.svg";
const LABEL_MIN_WIDTH: Pixels = px(96.);
const REVEAL_MARGIN: Pixels = px(8.);
const EMPTY_STATE_TOP: Pixels = px(48.);
const EMPTY_ICON: &str = "icons/settings/keyboard.svg";
const KEY_TOGGLE_SIZE: Pixels = px(24.);
const KEY_TOGGLE_GLYPH: Pixels = px(14.);
const KEY_TOGGLE_ARMED_ALPHA: f32 = 0.15;
const KEY_TOGGLE_HOVER_ALPHA: f32 = 0.08;
const KEY_TOGGLE_ICON: &str = "icons/settings/keyboard-keys.svg";

const RESTING_NOTE: &str = "The keys for what you do most. Click one to change it.";
const LISTENING_NOTE: &str = if cfg!(target_os = "macos") {
    "Press the new shortcut. Escape cancels, Delete removes it."
} else {
    "Press the new shortcut. Escape cancels, Backspace removes it."
};
const FINDING_NOTE: &str = "Press the keys to see what they do. Escape cancels.";
const SEARCH_LABEL: &str = "Search shortcuts";
pub(crate) const SEARCH_PLACEHOLDER: &str = "Search by name or keys";
const FINDING_PLACEHOLDER: &str = "Press the keys to look up\u{2026}";
const LISTENING_FACE: &str = "Press a shortcut\u{2026}";
const UNASSIGNED: &str = "Unassigned";
const UNASSIGNED_FACE: &str = "None";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ShortcutCursor {
    pub(crate) action: &'static str,
    pub(crate) keyboard: bool,
}

pub(crate) struct ShortcutConflict {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) owner: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PageNote<'a> {
    Resting,
    Listening,
    FindingKeys,
    Filtered { shown: usize, total: usize },
    Taken { keys: &'a str, owner: &'a str },
}

impl PageNote<'_> {
    pub(crate) fn text(self) -> String {
        match self {
            PageNote::Resting => RESTING_NOTE.to_string(),
            PageNote::Listening => LISTENING_NOTE.to_string(),
            PageNote::FindingKeys => FINDING_NOTE.to_string(),
            PageNote::Filtered { shown, total } => format!(
                "Showing {shown} of {total} shortcuts. Press Enter to change the selected one."
            ),
            PageNote::Taken { keys, owner } => {
                format!("{keys} is already assigned to {owner}. Press it again to reassign.")
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TypedKey {
    Character,
    Find,
    Other,
}

fn classify_typed_key(keystroke: &gpui::Keystroke, query_empty: bool, cx: &App) -> TypedKey {
    let modifiers = &keystroke.modifiers;
    if keystroke.key == "f"
        && modifiers.secondary(cx)
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.function
    {
        return TypedKey::Find;
    }
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return TypedKey::Other;
    }
    match keystroke.key_char.as_deref() {
        Some(" ") if query_empty => TypedKey::Other,
        Some(typed) if !typed.is_empty() && !typed.chars().any(char::is_control) => {
            TypedKey::Character
        }
        _ => TypedKey::Other,
    }
}

fn search_field_style() -> FilterFieldStyle {
    FilterFieldStyle::palette()
}

fn key_toggle_frame(style: FilterFieldStyle) -> (Pixels, Pixels) {
    let inset = (style.height - KEY_TOGGLE_SIZE) / 2.;
    let field_radius = match style.shape {
        FilterFieldShape::Well(radius) => radius,
        FilterFieldShape::Capsule => style.height / 2.,
    };
    (inset, (field_radius - inset).max(px(0.)))
}

pub(crate) fn new_shortcut_list_state() -> ListState {
    ListState::new(0, ListAlignment::Top, px(0.)).measure_all()
}

pub(crate) fn any_shortcut_matches(entries: &[ShortcutEntry], query: &str) -> bool {
    let filter = ShortcutFilter::parse(query);
    entries
        .iter()
        .enumerate()
        .any(|(index, entry)| filter.hit(index, entry).is_some())
}

fn section_hint(group: ShortcutGroup) -> Option<String> {
    group.context_hint().map(|hint| {
        let mut chars = hint.chars();
        let first = chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_default();
        format!("{first}{}.", chars.as_str())
    })
}

fn section_header_height(group: ShortcutGroup) -> Pixels {
    let hint = if group.context_hint().is_some() {
        SECTION_HINT_GAP + SECTION_HINT_LINE
    } else {
        px(0.)
    };
    SECTION_TITLE_LINE + hint + SECTION_TITLE_GAP
}

fn section_top_gap(first: bool) -> Pixels {
    if first { px(0.) } else { SECTION_GAP }
}

fn row_top_in_section(group: ShortcutGroup, first: bool, position: usize) -> Pixels {
    section_top_gap(first)
        + section_header_height(group)
        + px(kit::CARD_BORDER)
        + (ROW_HEIGHT + RULE) * position as f32
}

fn chords(entry: &ShortcutEntry) -> Vec<String> {
    if entry.key == UNASSIGNED {
        return Vec::new();
    }
    std::iter::once(entry.key.clone())
        .chain(
            entry
                .extra_raw_keys
                .iter()
                .map(|raw| keybindings::format_keystroke(raw)),
        )
        .collect()
}

impl PaneFlowApp {
    fn shortcut_search_text(&self, cx: &App) -> String {
        self.shortcut_search_input.read(cx).value()
    }

    pub(crate) fn rebuild_shortcut_rows(&mut self, cx: &mut Context<Self>) {
        self.refresh_shortcut_rows(false, cx);
    }

    fn refresh_shortcut_rows(&mut self, animate: bool, cx: &mut Context<Self>) {
        let previous_len = self.shortcut_list.item_count();
        let previous_top = self.shortcut_list.logical_scroll_top();

        self.shortcut_filter = ShortcutFilter::parse(&self.shortcut_search_text(cx));
        self.shortcut_rows = self.shortcut_filter.sections(&self.effective_shortcuts);
        let entries = &self.effective_shortcuts;
        let len = {
            let mut motion = self.shortcut_motion.borrow_mut();
            motion.retarget(
                &self.shortcut_rows,
                |index| entries.get(index).map_or("", |entry| entry.action_name),
                std::time::Instant::now(),
                animate && !crate::ui_primitives::reduce_motion(),
            );
            motion.sections().len()
        };
        self.shortcut_list.reset(len);
        self.shortcut_scroll.content_replaced();

        if len > 0 && len == previous_len {
            self.shortcut_list.scroll_to(previous_top);
        }
    }

    fn settle_shortcut_motion(&self, window: &Window) {
        let (was_animating, animating, len) = {
            let mut motion = self.shortcut_motion.borrow_mut();
            let was_animating = motion.is_animating();
            let animating = motion.sample(std::time::Instant::now());
            (was_animating, animating, motion.sections().len())
        };
        if len != self.shortcut_list.item_count() {
            let top = self.shortcut_list.logical_scroll_top();
            self.shortcut_list.reset(len);
            self.shortcut_scroll.content_replaced();
            if top.item_ix < len {
                self.shortcut_list.scroll_to(top);
            }
        } else if was_animating {
            self.shortcut_list.remeasure();
        }
        if animating {
            window.request_animation_frame();
        }
    }

    pub(crate) fn shortcut_search_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh_shortcut_rows(true, cx);
        self.shortcut_list.scroll_to(ListOffset::default());
        self.shortcut_cursor = if self.shortcut_filter.is_empty() {
            None
        } else {
            self.visible_shortcut_order()
                .first()
                .map(|(_, _, action)| ShortcutCursor {
                    action,
                    keyboard: true,
                })
        };
        cx.notify();
    }

    pub(crate) fn seed_shortcut_search(&mut self, cx: &mut Context<Self>) {
        let raw = self.settings_search_input.read(cx).value();
        let seeded = if crate::settings::chrome::shortcuts_named_by(&search::normalize(&raw)) {
            String::new()
        } else {
            raw.trim().to_string()
        };
        if self.shortcut_search_text(cx) != seeded {
            self.shortcut_search_input
                .update(cx, |input, cx| input.set_value(seeded, cx));
        }
    }

    pub(crate) fn reset_shortcut_search(&mut self, cx: &mut Context<Self>) {
        self.set_shortcut_key_search(false, cx);
        if !self.shortcut_search_text(cx).is_empty() {
            self.shortcut_search_input
                .update(cx, |input, cx| input.clear(cx));
        }
    }

    fn set_shortcut_key_search(&mut self, armed: bool, cx: &mut Context<Self>) {
        if self.shortcut_key_search == armed {
            return;
        }
        self.shortcut_key_search = armed;
        let placeholder = if armed {
            FINDING_PLACEHOLDER
        } else {
            SEARCH_PLACEHOLDER
        };
        self.shortcut_search_input.update(cx, |input, cx| {
            if armed {
                input.clear(cx);
            }
            input.set_placeholder(placeholder, cx);
        });
        if armed {
            self.cancel_shortcut_recording();
        }
        cx.notify();
    }

    fn focus_shortcut_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.shortcut_search_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    }

    pub(crate) fn shortcut_search_focused(&self, window: &Window, cx: &App) -> bool {
        self.shortcut_search_input
            .read(cx)
            .focus_handle
            .is_focused(window)
    }

    pub(crate) fn handle_shortcut_page_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let field_focused = self.shortcut_search_focused(window, cx);
        let sidebar_focused = self
            .settings_search_input
            .read(cx)
            .focus_handle
            .is_focused(window);
        if !(field_focused || sidebar_focused || self.settings_focus.is_focused(window)) {
            return false;
        }
        if self.shortcut_key_search {
            if keystroke.key != "escape" {
                let raw = keystroke.unparse(keybindings::host_os());
                let shown = keybindings::format_keystroke(&raw);
                self.shortcut_search_input
                    .update(cx, |input, cx| input.set_value(shown, cx));
            }
            self.set_shortcut_key_search(false, cx);
            return true;
        }

        let query_empty = self.shortcut_search_text(cx).is_empty();
        if keystroke.key == "escape" && !keystroke.modifiers.modified() && !sidebar_focused {
            if !query_empty {
                self.shortcut_search_input
                    .update(cx, |input, cx| input.clear(cx));
                return true;
            }
            if field_focused {
                self.settings_focus.focus(window, cx);
                cx.notify();
                return true;
            }
            return false;
        }

        match classify_typed_key(keystroke, query_empty, cx) {
            TypedKey::Find => {
                self.focus_shortcut_search(window, cx);
                self.shortcut_search_input
                    .update(cx, |input, cx| input.select_all_text(cx));
                return true;
            }
            TypedKey::Character if !field_focused && !sidebar_focused => {
                if let Some(typed) = keystroke.key_char.as_deref() {
                    let next = format!("{}{typed}", self.shortcut_search_text(cx));
                    self.shortcut_search_input
                        .update(cx, |input, cx| input.set_value(next, cx));
                    self.focus_shortcut_search(window, cx);
                }
                return true;
            }
            TypedKey::Character | TypedKey::Other => {}
        }

        self.navigate_shortcut_list(keystroke, window, cx)
    }

    fn any_shortcut_changed(&self) -> bool {
        self.effective_shortcuts
            .iter()
            .any(|entry| entry.customized)
    }

    fn shown_shortcut_count(&self) -> usize {
        self.shortcut_rows
            .iter()
            .map(|section| section.hits.len())
            .sum()
    }

    fn visible_shortcut_order(&self) -> Vec<(usize, usize, &'static str)> {
        self.shortcut_motion
            .borrow()
            .sections()
            .iter()
            .enumerate()
            .filter(|(_, section)| section.is_target())
            .flat_map(|(group_ix, section)| {
                section
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.is_target())
                    .map(move |(position, row)| (group_ix, position, row.key))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn navigate_shortcut_list(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if keystroke.modifiers.modified() {
            return false;
        }
        let order = self.visible_shortcut_order();
        let current = self.shortcut_cursor.and_then(|cursor| {
            order
                .iter()
                .position(|(_, _, action)| *action == cursor.action)
        });
        let last = order.len().saturating_sub(1);
        let next = match keystroke.key.as_str() {
            "down" if !order.is_empty() => current.map_or(0, |at| (at + 1).min(last)),
            "up" if !order.is_empty() => current.map_or(last, |at| at.saturating_sub(1)),
            "enter" => {
                let Some(&(_, _, action)) = current
                    .or_else(|| (!self.shortcut_filter.is_empty()).then_some(0))
                    .and_then(|at| order.get(at))
                else {
                    return false;
                };
                self.shortcut_cursor = Some(ShortcutCursor {
                    action,
                    keyboard: true,
                });
                self.start_shortcut_recording(action);
                self.settings_focus.focus(window, cx);
                cx.notify();
                return true;
            }
            _ => return false,
        };
        let (group_ix, position, action) = order[next];
        self.shortcut_cursor = Some(ShortcutCursor {
            action,
            keyboard: true,
        });
        self.reveal_shortcut_row(group_ix, position);
        cx.notify();
        true
    }

    fn reveal_shortcut_row(&self, group_ix: usize, position: usize) {
        self.shortcut_scroll.ease(&self.shortcut_list, || {
            self.jump_to_shortcut_row(group_ix, position);
        });
    }

    fn jump_to_shortcut_row(&self, group_ix: usize, position: usize) {
        let Some(group) = self
            .shortcut_motion
            .borrow()
            .sections()
            .get(group_ix)
            .map(|section| section.group)
        else {
            return;
        };
        let first = group_ix == 0;
        let row_top = row_top_in_section(group, first, position);
        let reveal_top = if position == 0 {
            section_top_gap(first)
        } else {
            row_top - REVEAL_MARGIN
        };
        let Some(item) = self.shortcut_list.bounds_for_item(group_ix) else {
            self.shortcut_list.scroll_to(ListOffset {
                item_ix: group_ix,
                offset_in_item: reveal_top.max(px(0.)),
            });
            return;
        };
        let viewport = self.shortcut_list.viewport_bounds();
        let top = item.top() + reveal_top;
        let bottom = item.top() + row_top + ROW_HEIGHT + REVEAL_MARGIN;
        if top < viewport.top() {
            self.shortcut_list.scroll_by(top - viewport.top());
        } else if bottom > viewport.bottom() {
            self.shortcut_list.scroll_by(bottom - viewport.bottom());
        }
    }

    fn shortcut_page_note(&self) -> PageNote<'_> {
        match (&self.recording_shortcut, &self.shortcut_conflict) {
            (Some(_), Some(conflict)) => PageNote::Taken {
                keys: &conflict.label,
                owner: &conflict.owner,
            },
            (Some(_), None) => PageNote::Listening,
            (None, _) if self.shortcut_key_search => PageNote::FindingKeys,
            (None, _) if !self.shortcut_filter.is_empty() && !self.shortcut_rows.is_empty() => {
                PageNote::Filtered {
                    shown: self.shown_shortcut_count(),
                    total: self.effective_shortcuts.len(),
                }
            }
            (None, _) => PageNote::Resting,
        }
    }

    pub(crate) fn render_shortcuts_heading(
        &self,
        title: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        let note = self.shortcut_page_note();
        let note_color = match note {
            PageNote::Taken { .. } => ui.vc_conflict,
            _ => ink.secondary,
        };
        let restore = self.any_shortcut_changed().then(|| {
            kit::icon_button(
                "restore-shortcut-defaults",
                "Restore defaults",
                UNDO_ICON,
                ink,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.persist_shortcut_change(config_writer::reset_shortcuts_checked, cx);
            }))
        });

        div()
            .pb(HEADING_GAP)
            .px(px(kit::TEXT_INSET))
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(NOTE_GAP)
                    .child(title.min_w_0())
                    .child(
                        div()
                            .id("shortcut-page-note")
                            .text_size(NOTE_TEXT)
                            .line_height(NOTE_LINE)
                            .text_color(note_color)
                            .truncate()
                            .role(gpui::accesskit::Role::Status)
                            .child(note.text()),
                    ),
            )
            .children(restore.map(|button| div().flex_none().pt(px(6.)).child(button)))
            .into_any_element()
    }

    fn render_shortcut_search(
        &self,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.shortcut_search_focused(window, cx);
        let has_query = !self.shortcut_search_text(cx).is_empty();
        let armed = self.shortcut_key_search;
        let accent = ui.accent;
        let toggle_rest = if armed {
            with_alpha(accent, KEY_TOGGLE_ARMED_ALPHA)
        } else {
            with_alpha(ui.text, 0.)
        };
        let toggle_hover = if armed {
            toggle_rest
        } else {
            with_alpha(ui.text, KEY_TOGGLE_HOVER_ALPHA)
        };
        let style = search_field_style();
        let (toggle_inset, toggle_radius) = key_toggle_frame(style);
        let key_toggle = div()
            .id("shortcut-find-by-keys")
            .size(KEY_TOGGLE_SIZE)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(toggle_radius)
            .bg(toggle_rest)
            .hover(move |style| style.bg(toggle_hover))
            .cursor_pointer()
            .accessible_control(
                gpui::accesskit::Role::Button,
                if armed {
                    "Stop finding by keys"
                } else {
                    "Find by keys"
                },
            )
            .aria_toggled(if armed {
                gpui::accesskit::Toggled::True
            } else {
                gpui::accesskit::Toggled::False
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.set_shortcut_key_search(!armed, cx);
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
            .child(
                svg()
                    .size(KEY_TOGGLE_GLYPH)
                    .path(KEY_TOGGLE_ICON)
                    .text_color(if armed { accent } else { ui.muted }),
            );

        filter_field(
            "shortcut-search",
            "shortcut-search-clear",
            SEARCH_LABEL,
            ui,
            style,
            focused || armed,
            has_query,
            true,
            None,
            self.shortcut_search_input.clone(),
            cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.shortcut_search_input
                    .update(cx, |input, cx| input.clear(cx));
                this.focus_shortcut_search(window, cx);
            }),
        )
        .pr(toggle_inset)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.set_shortcut_key_search(false, cx);
                if this.recording_shortcut.is_some() {
                    this.cancel_shortcut_recording();
                }
                this.focus_shortcut_search(window, cx);
                cx.notify();
            }),
        )
        .child(key_toggle)
        .into_any_element()
    }

    pub(crate) fn render_shortcuts_page(
        &self,
        heading: AnyElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        self.settle_shortcut_motion(window);
        self.shortcut_scroll.settle(&self.shortcut_list, window);
        let body = if self.shortcut_motion.borrow().sections().is_empty() {
            self.render_shortcut_empty_state(ink, cx)
        } else {
            self.render_shortcut_list(ui, ink, cx)
        };
        let search = self.render_shortcut_search(ui, window, cx);

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                list_column(
                    div()
                        .w_full()
                        .pt(SETTINGS_COLUMN_PADDING)
                        .pb(SEARCH_GAP)
                        .child(heading)
                        .child(search),
                )
                .flex_none(),
            )
            .child(body)
            .into_any_element()
    }

    fn render_shortcut_empty_state(&self, ink: Ink, cx: &mut Context<Self>) -> AnyElement {
        let query = self.shortcut_search_text(cx).trim().to_string();
        let (message, detail) = match self.shortcut_filter.sole_chord().and_then(|c| c.raw()) {
            Some(raw) => (
                format!("Nothing uses {}", keybindings::format_keystroke(&raw)),
                Some("It is free to assign."),
            ),
            None => (format!("No shortcut matches \u{201c}{query}\u{201d}"), None),
        };
        list_column(
            div()
                .w_full()
                .pt(EMPTY_STATE_TOP)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(kit::disc(ink, EMPTY_ICON))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(kit::SIZE_MD))
                                .line_height(px(kit::LINE_MD))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ink.primary)
                                .child(message),
                        )
                        .children(detail.map(|detail| kit::line(ink, detail))),
                ),
        )
        .flex_none()
        .into_any_element()
    }

    fn render_shortcut_list(
        &self,
        ui: crate::theme::UiColors,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = list(
            self.shortcut_list.clone(),
            cx.processor(move |this, index: usize, _window, cx| {
                let (section, above, last) = {
                    let motion = this.shortcut_motion.borrow();
                    let sections = motion.sections();
                    let Some(section) = sections.get(index).cloned() else {
                        return gpui::Empty.into_any_element();
                    };
                    let above = sections[..index]
                        .iter()
                        .map(|earlier| earlier.amount)
                        .fold(0., f32::max);
                    (section, above, index + 1 == sections.len())
                };
                let words: Vec<&str> = this.shortcut_filter.words().collect();
                let title = filter::word_ranges(section.group.label(), &words);
                let rows = section
                    .rows
                    .iter()
                    .map(|row| (row.amount, this.render_shortcut_row(ui, ink, &row.hit, cx)))
                    .collect();
                list_column(shortcut_section(
                    ink,
                    section.group,
                    &title,
                    rows,
                    SectionMotion {
                        gap: SECTION_GAP * section.amount.min(above),
                        amount: section.amount,
                    },
                    last,
                ))
                .into_any_element()
            }),
        )
        .size_full();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.shortcut_list_region(rows))
            .into_any_element()
    }

    fn shortcut_list_region(&self, rows: gpui::List) -> Div {
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(rows)
            .child(self.shortcut_scroll.wheel_layer(self.shortcut_list.clone()))
    }

    fn render_shortcut_row(
        &self,
        ui: crate::theme::UiColors,
        ink: Ink,
        hit: &filter::ShortcutHit,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = hit.index;
        let Some(entry) = self.effective_shortcuts.get(idx) else {
            return gpui::Empty.into_any_element();
        };
        let action_name = entry.action_name;
        let recording = self.recording_shortcut == Some(action_name);
        let face = match (recording, self.shortcut_conflict.as_ref()) {
            (true, Some(conflict)) => RecorderFace::Held(conflict.label.clone()),
            (true, None) => RecorderFace::Listening,
            (false, _) => RecorderFace::Keys {
                chords: chords(entry),
                matched: hit.chords.clone(),
            },
        };
        let keyboard_focused = !recording
            && self
                .shortcut_cursor
                .is_some_and(|cursor| cursor.keyboard && cursor.action == action_name);
        let shown = match chords(entry).as_slice() {
            [] => UNASSIGNED_FACE.to_string(),
            chords => chords.join(" or "),
        };

        let reset = entry.customized.then(|| {
            kit::button(
                ("shortcut-reset", idx),
                "Reset",
                Tone::Ghost,
                Size::Small,
                ink,
            )
            .aria_label(format!("Reset {} to its default", entry.description))
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.persist_shortcut_change(
                    move || config_writer::reset_shortcut(action_name),
                    cx,
                );
            }))
        });

        let recorder_label = if recording {
            "Press a shortcut".to_string()
        } else {
            format!("Change {}, now {shown}", entry.description)
        };
        let recorder = recorder(("shortcut-recorder", idx), &face, ui, ink)
            .accessible_control(gpui::accesskit::Role::Button, recorder_label)
            .when(keyboard_focused, |button| {
                button.child(crate::app::usage::focus_ring(CONTROL_RADIUS, ui))
            })
            .when(recording, |button| {
                button.on_mouse_down_out(cx.listener(move |this, _, _window, cx| {
                    if this.recording_shortcut == Some(action_name) {
                        this.cancel_shortcut_recording();
                        cx.notify();
                    }
                }))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.shortcut_cursor = Some(ShortcutCursor {
                    action: action_name,
                    keyboard: false,
                });
                if this.recording_shortcut == Some(action_name) {
                    this.cancel_shortcut_recording();
                } else {
                    this.set_shortcut_key_search(false, cx);
                    this.start_shortcut_recording(action_name);
                }
                this.settings_focus.focus(window, cx);
                cx.notify();
            }));

        shortcut_row(
            idx,
            entry.description.clone(),
            &hit.label,
            ink,
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(CONTROL_GAP)
                .children(reset)
                .child(recorder),
        )
        .into_any_element()
    }
}

enum RecorderFace {
    Keys {
        chords: Vec<String>,
        matched: Vec<usize>,
    },
    Listening,
    Held(String),
}

fn recorder(
    id: impl Into<gpui::ElementId>,
    face: &RecorderFace,
    ui: crate::theme::UiColors,
    ink: Ink,
) -> Stateful<Div> {
    let (resting, hovered, edge) = match face {
        RecorderFace::Keys { .. } => (ink.well, Some(ink.well_hover), None),
        RecorderFace::Listening => (
            with_alpha(ink.accent, LISTENING_TINT),
            None,
            Some(ink.accent),
        ),
        RecorderFace::Held(_) => (
            with_alpha(ui.vc_conflict, HELD_TINT),
            None,
            Some(with_alpha(ui.vc_conflict, HELD_EDGE)),
        ),
    };
    let button = div()
        .id(id.into())
        .relative()
        .flex_none()
        .h(CONTROL_HEIGHT)
        .min_w(RECORDER_MIN_WIDTH)
        .px(RECORDER_PADDING_X)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .rounded(CONTROL_RADIUS)
        .border_1()
        .border_color(edge.unwrap_or(gpui::transparent_black()))
        .bg(resting)
        .cursor_pointer()
        .when_some(hovered, |button, hovered| {
            button
                .hover(move |style| style.bg(hovered))
                .active(move |style| style.bg(ink.well_pressed))
        })
        .text_size(CONTROL_TEXT)
        .font_weight(FontWeight::MEDIUM);
    match face {
        RecorderFace::Keys { chords, .. } if chords.is_empty() => button
            .text_color(ink.tertiary)
            .child(div().relative().child(UNASSIGNED_FACE)),
        RecorderFace::Keys { chords, matched } => {
            let mut button = button.text_color(ink.primary);
            for (position, chord) in chords.iter().enumerate() {
                if position > 0 {
                    button = button.child(
                        div()
                            .relative()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(ink.tertiary)
                            .child("or"),
                    );
                }
                let ranges = if matched.contains(&position) {
                    std::iter::once(0..chord.len()).collect()
                } else {
                    Vec::new()
                };
                button = button.child(
                    div()
                        .relative()
                        .child(highlight_ranges(chord.clone(), ranges)),
                );
            }
            button
        }
        RecorderFace::Listening => button
            .text_color(ink.primary)
            .child(div().relative().child(LISTENING_FACE)),
        RecorderFace::Held(keys) => button
            .text_color(ui.vc_conflict)
            .child(div().relative().child(keys.clone())),
    }
}

fn shortcut_row(
    idx: usize,
    label: String,
    highlights: &[std::ops::Range<usize>],
    ink: Ink,
    control: impl IntoElement,
) -> Div {
    div()
        .h(ROW_HEIGHT)
        .px(px(kit::ROW_INSET))
        .flex()
        .flex_row()
        .items_center()
        .gap(ROW_GAP)
        .child(
            div()
                .id(("shortcut-label", idx))
                .flex_1()
                .min_w(LABEL_MIN_WIDTH)
                .text_size(px(kit::SIZE_MD))
                .line_height(px(kit::LINE_MD))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ink.primary)
                .truncate()
                .delayed_tooltip(text_tooltip(label.clone()))
                .child(highlight_ranges(label, highlights.to_vec())),
        )
        .child(control)
}

#[derive(Clone, Copy)]
struct SectionMotion {
    gap: Pixels,
    amount: f32,
}

#[cfg(test)]
impl SectionMotion {
    fn at_rest(first: bool) -> Self {
        Self {
            gap: section_top_gap(first),
            amount: 1.,
        }
    }
}

fn folded(element: impl IntoElement, height: Pixels, amount: f32, fade: bool) -> Div {
    div()
        .flex_none()
        .w_full()
        .overflow_hidden()
        .when(amount < 1., |wrapper| {
            wrapper
                .h(height * amount)
                .when(fade, |wrapper| wrapper.opacity(amount))
        })
        .child(element)
}

fn shortcut_section(
    ink: Ink,
    group: ShortcutGroup,
    title: &[std::ops::Range<usize>],
    rows: Vec<(f32, AnyElement)>,
    motion: SectionMotion,
    last: bool,
) -> Div {
    let header = div()
        .flex_none()
        .h(section_header_height(group))
        .px(px(kit::TEXT_INSET))
        .pb(SECTION_TITLE_GAP)
        .flex()
        .flex_col()
        .gap(SECTION_HINT_GAP)
        .min_w_0()
        .child(
            div()
                .h(SECTION_TITLE_LINE)
                .text_size(px(kit::SIZE_XS))
                .line_height(SECTION_TITLE_LINE)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ink.tertiary)
                .truncate()
                .child(highlight_ranges(group.label().to_string(), title.to_vec())),
        )
        .children(section_hint(group).map(|hint| {
            div()
                .h(SECTION_HINT_LINE)
                .text_size(px(kit::SIZE_SM))
                .line_height(SECTION_HINT_LINE)
                .text_color(ink.tertiary)
                .truncate()
                .child(hint)
        }));

    let amount = motion.amount;
    div()
        .pt(motion.gap)
        .when(last, |d| d.pb(LIST_BOTTOM))
        .when(amount < 1., |section| section.opacity(amount))
        .child(folded(header, section_header_height(group), amount, false))
        .child(shortcut_card(ink, rows, amount))
}

fn shortcut_card(ink: Ink, rows: Vec<(f32, AnyElement)>, amount: f32) -> Div {
    let fade_rows = amount >= 1.;
    let mut card = kit::group(ink);
    let mut above: Option<f32> = None;
    for (row_amount, row) in rows {
        if let Some(above) = above {
            let rule = above.min(row_amount);
            card = card.child(folded(kit::rule(ink), RULE, rule, fade_rows));
        }
        card = card.child(folded(row, ROW_HEIGHT, row_amount, fade_rows));
        above = Some(row_amount);
    }
    card
}

fn list_column(inner: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_start()
        .child(settings_column().child(inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(description: &str, key: &str, group: ShortcutGroup) -> ShortcutEntry {
        ShortcutEntry {
            key: key.to_string(),
            raw_key: None,
            extra_raw_keys: Vec::new(),
            customized: false,
            description: description.to_string(),
            action_name: "probe",
            group,
            search_key: key.to_lowercase(),
        }
    }

    #[test]
    fn the_page_note_follows_the_page_state() {
        assert_eq!(PageNote::Resting.text(), RESTING_NOTE);
        assert_eq!(PageNote::Listening.text(), LISTENING_NOTE);
        assert_eq!(PageNote::FindingKeys.text(), FINDING_NOTE);
        assert_eq!(
            PageNote::Filtered {
                shown: 3,
                total: 85
            }
            .text(),
            "Showing 3 of 85 shortcuts. Press Enter to change the selected one."
        );
        assert_eq!(
            PageNote::Taken {
                keys: "Ctrl+B",
                owner: "Toggle sidebar",
            }
            .text(),
            "Ctrl+B is already assigned to Toggle sidebar. Press it again to reassign."
        );
    }

    #[test]
    fn the_find_by_keys_toggle_is_concentric_with_the_search_field() {
        let style = search_field_style();
        let (inset, radius) = key_toggle_frame(style);
        assert_eq!(inset * 2. + KEY_TOGGLE_SIZE, style.height);
        let FilterFieldShape::Well(field_radius) = style.shape else {
            panic!("the search field is a well");
        };
        assert_eq!(radius + inset, field_radius);
        assert!(crate::assets::Assets::get(KEY_TOGGLE_ICON).is_some());
    }

    #[test]
    fn a_context_hint_reads_as_a_sentence() {
        assert_eq!(
            section_hint(ShortcutGroup::Terminal).as_deref(),
            Some("While a pane has focus.")
        );
        assert_eq!(section_hint(ShortcutGroup::Panes), None);
    }

    #[test]
    fn an_unassigned_shortcut_has_no_chord() {
        let none = entry("Open file", UNASSIGNED, ShortcutGroup::Application);
        assert!(chords(&none).is_empty());
    }

    #[gpui::test]
    fn typing_a_character_starts_a_search_but_shortcuts_do_not(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let typed = |source: &'static str, key_char: Option<&str>, empty: bool| {
                let mut keystroke = keybindings::parse_keystroke(source).expect("a keystroke");
                keystroke.key_char = key_char.map(str::to_string);
                classify_typed_key(&keystroke, empty, cx)
            };
            assert_eq!(typed("s", Some("s"), true), TypedKey::Character);
            assert_eq!(typed("shift-s", Some("S"), true), TypedKey::Character);
            assert_eq!(typed("space", Some(" "), false), TypedKey::Character);
            assert_eq!(typed("space", Some(" "), true), TypedKey::Other);
            assert_eq!(typed("ctrl-s", None, true), TypedKey::Other);
            assert_eq!(typed("alt-s", Some("ß"), true), TypedKey::Other);
            assert_eq!(typed("enter", Some("\n"), true), TypedKey::Other);
            assert_eq!(typed("down", None, true), TypedKey::Other);
            assert_eq!(typed("secondary-f", None, true), TypedKey::Find);
        });
    }

    struct Probe {
        state: ListState,
    }

    impl gpui::Render for Probe {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut Context<Self>,
        ) -> impl IntoElement {
            let ui = crate::theme::ui_colors();
            let ink = Ink::of(ui);
            list(self.state.clone(), move |index, _window, _cx| {
                let group = PROBE_GROUPS[index];
                let rows = (0..3)
                    .map(|row| {
                        let element = shortcut_row(
                            index * 10 + row,
                            format!("action {row}"),
                            &[],
                            ink,
                            recorder(
                                ("probe-recorder", index * 10 + row),
                                &RecorderFace::Keys {
                                    chords: vec!["Ctrl+X".to_string()],
                                    matched: vec![0],
                                },
                                ui,
                                ink,
                            ),
                        )
                        .debug_selector(move || format!("probe-row-{index}-{row}"))
                        .into_any_element();
                        (1., element)
                    })
                    .collect();
                list_column(
                    shortcut_section(
                        ink,
                        group,
                        &[],
                        rows,
                        SectionMotion::at_rest(index == 0),
                        false,
                    )
                    .debug_selector(move || format!("probe-section-{index}")),
                )
                .debug_selector(move || format!("probe-item-{index}"))
                .into_any_element()
            })
            .size_full()
        }
    }

    const PROBE_GROUPS: [ShortcutGroup; 3] = [
        ShortcutGroup::Panes,
        ShortcutGroup::Terminal,
        ShortcutGroup::Tabs,
    ];
    const PROBE_WIDTH: f32 = 640.0;

    fn mount_probe(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::Entity<Probe>, &mut gpui::VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_, _| {
            let state = new_shortcut_list_state();
            state.reset(PROBE_GROUPS.len());
            Probe { state }
        });
        cx.simulate_resize(gpui::size(px(PROBE_WIDTH), px(900.0)));
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui::test]
    fn sections_span_the_reading_column(cx: &mut gpui::TestAppContext) {
        let (view, cx) = mount_probe(cx);
        let item = cx
            .debug_bounds("probe-item-1")
            .expect("item 1 must be painted");
        let section = cx
            .debug_bounds("probe-section-1")
            .expect("section 1 must be painted");
        let viewport = view.read_with(cx, |probe, _| probe.state.viewport_bounds().size.width);
        assert_eq!(
            item.size.width, viewport,
            "a list item that does not span the list is shrink-wrapping its content"
        );
        let expected = px(PROBE_WIDTH) - SETTINGS_COLUMN_PADDING * 2.;
        assert_eq!(
            section.size.width, expected,
            "the section must sit in the reading column, as wide as on every other Settings page"
        );
        assert_eq!(section.origin.x, SETTINGS_COLUMN_PADDING);
    }

    #[gpui::test]
    fn the_reveal_offsets_match_the_painted_rows(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = mount_probe(cx);
        for (index, group) in PROBE_GROUPS.iter().enumerate() {
            let item = cx
                .debug_bounds(Box::leak(format!("probe-item-{index}").into_boxed_str()))
                .expect("every probe item must be painted");
            for position in 0..3 {
                let row = cx
                    .debug_bounds(Box::leak(
                        format!("probe-row-{index}-{position}").into_boxed_str(),
                    ))
                    .expect("every probe row must be painted");
                assert_eq!(
                    row.origin.y - item.origin.y,
                    row_top_in_section(*group, index == 0, position),
                    "row {position} of section {index} is not where the keyboard reveal scrolls to"
                );
                assert_eq!(row.size.height, ROW_HEIGHT);
            }
        }
    }
}

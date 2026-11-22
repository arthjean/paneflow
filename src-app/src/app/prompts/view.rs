use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, ScrollStrategy, SharedString, StatefulInteractiveElement,
    Styled, UniformListScrollHandle, Window, deferred, div, px, svg, uniform_list,
};
use paneflow_api::methods::prompt::{OtherAnswer, PickItem, PromptAnswer, PromptKind, PromptSpec};

use super::{PendingPrompt, send_reply};
use crate::PaneFlowApp;
use crate::app::command_palette::matcher;
use crate::settings::components::{MENU_MAX_HEIGHT, select_item_shaped, with_alpha};
use crate::ui_primitives::interface_style::with_rounded_shape;
use crate::ui_primitives::quick_pick::{
    self, FIELD_GAP, FIELD_INSET, ICON_GAP, ICON_SIZE, LABEL_SIZE, ROW_GAP, ROW_HEIGHT, ROW_RADIUS,
    ROW_SPACING, ROW_TEXT_INSET, highlighted_label,
};
use crate::ui_primitives::{FilterFieldStyle, filter_field};

const PROMPT_WIDTH: f32 = 560.;
const PROMPT_TOP_MARGIN: f32 = 96.;
const PROMPT_CHROME: f32 = 112.;
const PROMPT_BOTTOM_GAP: f32 = 16.;
const ORIGIN_SIZE: f32 = 13.;
const QUESTION_MAX_HEIGHT: f32 = 120.;
const DETAIL_MAX_WIDTH: f32 = 200.;
const HEADER_GAP: f32 = 2.;
const HEADER_BOTTOM: f32 = 8.;
const PICK_PLACEHOLDER: &str = "Filter…";
const DEFAULT_CONFIRM: &str = "Confirm";
const NO_ITEMS: &str = "No items";
const NO_MATCHING_ITEM: &str = "No matching item";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowKind {
    Item(usize),
    Other(String),
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptRow {
    pub(crate) kind: RowKind,
    pub(crate) label: String,
    pub(crate) highlights: Vec<Range<usize>>,
    pub(crate) subtitle: Option<String>,
    pub(crate) detail: Option<String>,
    pub(crate) icon: Option<String>,
}

impl PromptRow {
    fn plain(kind: RowKind, label: String) -> Self {
        Self {
            kind,
            label,
            highlights: Vec::new(),
            subtitle: None,
            detail: None,
            icon: None,
        }
    }
}

pub(crate) fn icon_path(name: &str) -> Option<&'static str> {
    Some(match name {
        "document" => "icons/file-text.svg",
        "folder" => "icons/folder.svg",
        "branch" => "icons/git-branch.svg",
        "terminal" => "icons/terminal.svg",
        "code" => "icons/code.svg",
        "server" => "icons/server.svg",
        "database" => "icons/database.svg",
        "package" => "icons/package.svg",
        "globe" => "icons/world.svg",
        "link" => "icons/palette/link-square-02.svg",
        "person" => "icons/prompt/person.svg",
        "tag" => "icons/prompt/tag.svg",
        "clock" => "icons/clock.svg",
        "star" => "icons/prompt/star.svg",
        "gear" => "icons/settings.svg",
        "plus" => "icons/plus.svg",
        _ => return None,
    })
}

fn quoted_use(text: &str) -> String {
    format!("Use \u{201c}{text}\u{201d}")
}

pub(crate) fn input_rows(spec: &PromptSpec, typed: &str) -> Vec<PromptRow> {
    let label = spec.confirm.clone().unwrap_or_else(|| quoted_use(typed));
    vec![PromptRow::plain(RowKind::Confirm, label)]
}

fn one_line(text: &str) -> String {
    super::plain_text(text).replace('\n', " ")
}

fn item_row(index: usize, item: &PickItem, highlights: Vec<Range<usize>>) -> PromptRow {
    PromptRow {
        kind: RowKind::Item(index),
        label: one_line(item.title()),
        highlights,
        subtitle: item.subtitle().map(one_line),
        detail: item.detail().map(one_line),
        icon: item.icon().map(str::to_string),
    }
}

pub(crate) fn pick_rows(spec: &PromptSpec, query: &str) -> Vec<PromptRow> {
    let query = query.trim();
    let items = spec.items();
    let mut ranked: Vec<(matcher::Rank, PromptRow)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let keywords = [Some(item.id()), item.detail()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let title = one_line(item.title());
        let subtitle = item.subtitle().map(one_line);
        let Some(matched) = matcher::match_entry(&title, subtitle.as_deref(), &keywords, query)
        else {
            continue;
        };
        ranked.push((matched.rank, item_row(index, item, matched.highlights)));
    }
    if !query.is_empty()
        && let Some(best) = ranked
            .iter()
            .enumerate()
            .min_by_key(|(position, (rank, _))| (*rank, *position))
            .map(|(position, _)| position)
    {
        let promoted = ranked.remove(best);
        ranked.insert(0, promoted);
    }
    let mut rows: Vec<PromptRow> = ranked.into_iter().map(|(_, row)| row).collect();
    if spec.allow_other()
        && !query.is_empty()
        && !items
            .iter()
            .any(|item| item.id() == query || item.title() == query)
    {
        rows.push(PromptRow::plain(
            RowKind::Other(query.to_string()),
            quoted_use(query),
        ));
    }
    if spec.multiple() {
        rows.push(PromptRow::plain(
            RowKind::Confirm,
            spec.confirm
                .clone()
                .unwrap_or_else(|| DEFAULT_CONFIRM.to_string()),
        ));
    }
    rows
}

pub(crate) fn answer_for(
    spec: &PromptSpec,
    row: &RowKind,
    typed: &str,
    ticked: &[bool],
) -> Option<PromptAnswer> {
    match (spec.kind, row) {
        (PromptKind::Input, RowKind::Confirm) => Some(PromptAnswer::Text(typed.to_string())),
        (PromptKind::Pick, RowKind::Item(index)) if !spec.multiple() => spec
            .items()
            .get(*index)
            .map(|item| PromptAnswer::Text(item.id().to_string())),
        (PromptKind::Pick, RowKind::Other(text)) => {
            Some(PromptAnswer::Other(OtherAnswer { text: text.clone() }))
        }
        (PromptKind::Pick, RowKind::Confirm) if spec.multiple() => Some(PromptAnswer::Ids(
            spec.items()
                .iter()
                .zip(ticked)
                .filter(|(_, ticked)| **ticked)
                .map(|(item, _)| item.id().to_string())
                .collect(),
        )),
        _ => None,
    }
}

pub(crate) struct ShownPrompt {
    pub(crate) prompt: PendingPrompt,
    pub(crate) selected: usize,
    pub(crate) ticked: Vec<bool>,
    pub(crate) rows: Rc<Vec<PromptRow>>,
    pub(crate) query: String,
    pub(crate) generation: u64,
    pub(crate) filtering: bool,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) return_focus: crate::FocusReturn,
}

impl ShownPrompt {
    fn ticked_count(&self) -> usize {
        self.ticked.iter().filter(|ticked| **ticked).count()
    }

    fn has_items_shown(&self) -> bool {
        self.rows
            .iter()
            .any(|row| matches!(row.kind, RowKind::Item(_) | RowKind::Other(_)))
    }
}

pub(crate) fn list_height(rows: usize, viewport_height: f32) -> Pixels {
    let row = ROW_HEIGHT + ROW_SPACING;
    let room = (viewport_height - PROMPT_TOP_MARGIN - PROMPT_CHROME - PROMPT_BOTTOM_GAP).max(row);
    px((rows as f32 * row)
        .min(f32::from(MENU_MAX_HEIGHT))
        .min(room))
}

impl PaneFlowApp {
    pub(crate) fn prompt_may_show(&self) -> bool {
        !self.command_palette_open && self.file_finder.is_none()
    }

    pub(crate) fn show_next_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompts.shown.is_some() || !self.prompt_may_show() {
            return;
        }
        let Some(prompt) =
            super::next_prompt(&self.prompts.pending, self.prompts.activated, None).cloned()
        else {
            if let Some(origin) = self.prompts.return_focus.take() {
                self.return_focus(&origin, window, cx);
            }
            return;
        };
        if self.prompts.activated == Some(prompt.id) {
            self.prompts.activated = None;
        }
        let return_focus = self
            .prompts
            .return_focus
            .take()
            .unwrap_or_else(|| crate::FocusReturn::capture(window, cx));
        let spec = prompt.spec.clone();
        let value = spec.value.clone().unwrap_or_default();
        let placeholder = match spec.kind {
            PromptKind::Input => spec.placeholder.clone().unwrap_or_default(),
            PromptKind::Pick => spec
                .placeholder
                .clone()
                .unwrap_or_else(|| PICK_PLACEHOLDER.to_string()),
        };
        for item in spec.items() {
            if let Some(icon) = item.icon()
                && icon_path(icon).is_none()
            {
                self.prompts.warn_unknown_icon(icon);
            }
        }
        let rows = match spec.kind {
            PromptKind::Input => input_rows(&spec, &value),
            PromptKind::Pick => pick_rows(&spec, ""),
        };
        self.prompts.shown = Some(ShownPrompt {
            ticked: spec.items().iter().map(PickItem::selected).collect(),
            prompt,
            selected: 0,
            rows: Rc::new(rows),
            query: String::new(),
            generation: 0,
            filtering: false,
            scroll: UniformListScrollHandle::new(),
            return_focus,
        });
        self.prompt_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            input.set_value(value, cx);
        });
        let focus = self.prompt_input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn prompt_query_changed(&mut self, cx: &mut Context<Self>) {
        let typed = self.prompt_input.read(cx).value();
        let Some(shown) = self.prompts.shown.as_mut() else {
            return;
        };
        if shown.query == typed {
            return;
        }
        shown.query = typed.clone();
        shown.selected = 0;
        shown.scroll.scroll_to_item(0, ScrollStrategy::Top);
        let spec = shown.prompt.spec.clone();
        if spec.kind == PromptKind::Input {
            shown.rows = Rc::new(input_rows(&spec, &typed));
            cx.notify();
            return;
        }
        shown.generation = shown.generation.wrapping_add(1);
        shown.filtering = true;
        let generation = shown.generation;
        let id = shown.prompt.id;
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let rows = smol::unblock(move || filter_off_thread(&spec, &typed)).await;
                let _ = this.update(cx, |app, cx| {
                    let Some(shown) = app.prompts.shown.as_mut() else {
                        return;
                    };
                    if shown.prompt.id != id || shown.generation != generation {
                        return;
                    }
                    shown.filtering = false;
                    shown.rows = Rc::new(rows);
                    cx.notify();
                });
            },
        )
        .detach();
    }

    fn finish_prompt(&mut self, answer: Option<PromptAnswer>, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompts.close_shown() else {
            return;
        };
        self.prompts
            .pending
            .retain(|pending| pending.id != prompt.id);
        send_reply(prompt.id, answer);
        cx.notify();
    }

    pub(crate) fn cancel_prompt(&mut self, cx: &mut Context<Self>) {
        self.finish_prompt(None, cx);
    }

    fn select_prompt_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(shown) = self.prompts.shown.as_mut() {
            shown.selected = index;
            shown.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    pub(crate) fn activate_prompt_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let typed = self.prompt_input.read(cx).value();
        let Some(shown) = self.prompts.shown.as_mut() else {
            return;
        };
        let Some(row) = shown.rows.get(index).map(|row| row.kind.clone()) else {
            return;
        };
        shown.selected = index;
        let spec = shown.prompt.spec.clone();
        if let RowKind::Item(item) = row
            && spec.multiple()
        {
            if let Some(ticked) = shown.ticked.get_mut(item) {
                *ticked = !*ticked;
            }
            cx.notify();
            return;
        }
        let answer = answer_for(&spec, &row, &typed, &shown.ticked);
        if answer.is_some() {
            self.finish_prompt(answer, cx);
        }
    }

    pub(crate) fn confirm_prompt(&mut self, cx: &mut Context<Self>) {
        let typed = self.prompt_input.read(cx).value();
        let Some(shown) = self.prompts.shown.as_ref() else {
            return;
        };
        let answer = answer_for(&shown.prompt.spec, &RowKind::Confirm, &typed, &shown.ticked);
        if answer.is_some() {
            self.finish_prompt(answer, cx);
        }
    }

    fn handle_prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((len, selected)) = self
            .prompts
            .shown
            .as_ref()
            .map(|shown| (shown.rows.len(), shown.selected))
        else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.cancel_prompt(cx),
            "enter" if event.keystroke.modifiers.secondary(cx) => self.confirm_prompt(cx),
            "enter" if selected < len => self.activate_prompt_row(selected, cx),
            "enter" => {}
            "up" if selected > 0 => self.select_prompt_row(selected - 1, cx),
            "down" if selected + 1 < len => self.select_prompt_row(selected + 1, cx),
            "up" | "down" => {}
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_prompt_row(
        &self,
        index: usize,
        row: &PromptRow,
        shown: &ShownPrompt,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = index == shown.selected;
        let colors = quick_pick::row_colors(selected, ui);
        let mut label = div()
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(ICON_GAP));
        if let Some(icon) = row.icon.as_deref().and_then(icon_path) {
            label = label.child(
                svg()
                    .flex_none()
                    .size(px(ICON_SIZE))
                    .path(icon)
                    .text_color(with_alpha(colors.label, 0.8)),
            );
        }
        label = label.child(
            div()
                .flex_none()
                .max_w_full()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(colors.label)
                .child(highlighted_label(&row.label, &row.highlights)),
        );
        if let Some(subtitle) = &row.subtitle {
            label = label.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(colors.muted)
                    .child(SharedString::from(subtitle.clone())),
            );
        }
        let trailing_value = match &row.kind {
            RowKind::Confirm if shown.prompt.spec.multiple() => {
                Some(format!("{} selected", shown.ticked_count()))
            }
            _ => row.detail.clone(),
        };
        let ticked = matches!(row.kind, RowKind::Item(item) if shown.ticked.get(item).copied().unwrap_or(false));
        select_item_shaped(
            SharedString::from(format!("prompt-row-{index}")),
            selected,
            ui,
            colors.fill,
            ROW_RADIUS,
        )
        .h(px(ROW_HEIGHT))
        .px(px(ROW_TEXT_INSET))
        .gap(px(ROW_GAP))
        .text_size(px(LABEL_SIZE))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.activate_prompt_row(index, cx);
            cx.stop_propagation();
        }))
        .child(label)
        .when_some(trailing_value, |row, value| {
            row.child(
                div()
                    .relative()
                    .flex_none()
                    .max_w(px(DETAIL_MAX_WIDTH))
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(colors.muted)
                    .child(SharedString::from(value)),
            )
        })
        .when(ticked, |row| {
            row.child(
                svg()
                    .relative()
                    .flex_none()
                    .size(px(13.))
                    .path("icons/check.svg")
                    .text_color(colors.label),
            )
        })
        .into_any_element()
    }

    fn render_prompt_header(
        &self,
        shown: &ShownPrompt,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.prompt_input.read(cx).focus_handle.clone();
        let focused = focus.is_focused(window);
        let has_query = !self.prompt_input.read(cx).value().is_empty();
        let title = shown.prompt.spec.title.clone();
        let field = filter_field(
            "prompt-field",
            "prompt-field-clear",
            SharedString::from(title.replace('\n', " ")),
            ui,
            FilterFieldStyle::palette(),
            focused,
            has_query,
            true,
            None,
            self.prompt_input.clone(),
            cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.prompt_input.update(cx, |input, cx| input.clear(cx));
                let focus = this.prompt_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }),
        )
        .flex_none()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .mx(px(FIELD_INSET))
            .mb(px(FIELD_GAP))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(HEADER_GAP))
                    .px(px(ROW_TEXT_INSET))
                    .pb(px(HEADER_BOTTOM))
                    .child(
                        div()
                            .id("prompt-question")
                            .max_h(px(QUESTION_MAX_HEIGHT))
                            .overflow_y_scroll()
                            .text_size(px(LABEL_SIZE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_size(px(ORIGIN_SIZE))
                            .text_color(ui.muted)
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(
                                self.prompt_origin_line(&shown.prompt, cx),
                            )),
                    ),
            )
            .child(field)
            .into_any_element()
    }

    pub(crate) fn render_prompt(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        with_rounded_shape(|| self.build_prompt(window, cx))
    }

    fn build_prompt(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let Some(shown) = self.prompts.shown.as_ref() else {
            return div().into_any_element();
        };
        let header = self.render_prompt_header(shown, ui, window, cx);
        let rows = shown.rows.clone();
        let empty =
            (shown.prompt.spec.kind == PromptKind::Pick && !shown.has_items_shown()).then(|| {
                if shown.prompt.spec.items().is_empty() {
                    NO_ITEMS
                } else {
                    NO_MATCHING_ITEM
                }
            });
        let list = div()
            .flex()
            .flex_col()
            .children(empty.map(|message| {
                div()
                    .h(px(ROW_HEIGHT))
                    .px(px(ROW_TEXT_INSET))
                    .flex()
                    .items_center()
                    .text_size(px(LABEL_SIZE))
                    .text_color(ui.muted)
                    .child(message)
            }))
            .when(!rows.is_empty(), |list| {
                let items = rows.clone();
                list.child(
                    uniform_list(
                        "prompt-list",
                        rows.len(),
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            let Some(shown) = this.prompts.shown.as_ref() else {
                                return Vec::new();
                            };
                            range
                                .filter_map(|index| {
                                    let row = items.get(index)?;
                                    Some(
                                        div().w_full().flex().flex_col().pb(px(ROW_SPACING)).child(
                                            with_rounded_shape(|| {
                                                this.render_prompt_row(index, row, shown, ui, cx)
                                            }),
                                        ),
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .w_full()
                    .h(list_height(
                        rows.len(),
                        f32::from(window.viewport_size().height),
                    ))
                    .track_scroll(&shown.scroll),
                )
            })
            .into_any_element();
        let panel = div()
            .id("prompt")
            .occlude()
            .on_key_down(cx.listener(Self::handle_prompt_key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.cancel_prompt(cx)))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let panel = quick_pick::shell(panel, px(PROMPT_WIDTH), ui, header, list);
        deferred(
            div()
                .id("prompt-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(PROMPT_TOP_MARGIN))
                .child(crate::ui_primitives::menu_reveal("prompt-reveal", panel)),
        )
        .with_priority(7)
        .into_any_element()
    }
}

fn filter_off_thread(spec: &Arc<PromptSpec>, typed: &str) -> Vec<PromptRow> {
    pick_rows(spec, typed)
}

#[cfg(test)]
impl PaneFlowApp {
    pub(crate) fn shown_prompt_rows(&self) -> Vec<String> {
        self.prompts
            .shown
            .as_ref()
            .map(|shown| shown.rows.iter().map(|row| row.label.clone()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn shown_prompt_title(&self, cx: &gpui::App) -> Option<(String, String)> {
        self.prompts.shown.as_ref().map(|shown| {
            (
                shown.prompt.spec.title.clone(),
                self.prompt_origin_line(&shown.prompt, cx),
            )
        })
    }

    pub(crate) fn shown_prompt_filtering(&self) -> bool {
        self.prompts
            .shown
            .as_ref()
            .is_some_and(|shown| shown.filtering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(items: serde_json::Value, extra: serde_json::Value) -> PromptSpec {
        let mut spec = serde_json::json!({"kind": "pick", "title": "Project", "items": items});
        if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
            spec.extend(extra.clone());
        }
        serde_json::from_value(spec).unwrap()
    }

    #[test]
    fn items_keep_their_order_show_their_fields_and_filter_with_the_palette_matcher() {
        let spec = pick(
            serde_json::json!(["api", {"id": "web", "title": "Website", "subtitle": "Next.js", "icon": "globe", "detail": "3 days"}]),
            serde_json::json!({}),
        );
        let rows = pick_rows(&spec, "");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "api");
        assert_eq!(rows[1].label, "Website");
        assert_eq!(rows[1].subtitle.as_deref(), Some("Next.js"));
        assert_eq!(rows[1].detail.as_deref(), Some("3 days"));
        assert_eq!(rows[1].icon.as_deref(), Some("globe"));
        let filtered = pick_rows(&spec, "web");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].kind, RowKind::Item(1));
        assert_eq!(filtered[0].highlights, vec![0..3]);
        assert!(pick_rows(&spec, "zzz").is_empty());
    }

    #[test]
    fn a_single_pick_answers_the_id_and_typed_text_only_with_allow_other() {
        let spec = pick(
            serde_json::json!(["api", "web"]),
            serde_json::json!({"allow_other": true}),
        );
        assert_eq!(
            answer_for(&spec, &RowKind::Item(1), "", &[false, false]),
            Some(PromptAnswer::Text("web".into()))
        );
        let rows = pick_rows(&spec, "cli");
        assert_eq!(
            rows.last().map(|row| row.label.as_str()),
            Some("Use \u{201c}cli\u{201d}")
        );
        assert_eq!(
            answer_for(&spec, &rows[0].kind, "cli", &[false, false]),
            Some(PromptAnswer::Other(OtherAnswer { text: "cli".into() }))
        );
        assert!(
            !pick_rows(&spec, "api")
                .iter()
                .any(|row| matches!(row.kind, RowKind::Other(_))),
            "typed text that names an item is that item"
        );
        let closed = pick(serde_json::json!(["api"]), serde_json::json!({}));
        assert!(pick_rows(&closed, "cli").is_empty());
    }

    #[test]
    fn a_multiple_pick_sends_the_ticked_ids_in_item_order_and_an_empty_list_when_none() {
        let spec = pick(
            serde_json::json!(["a", {"id": "b", "selected": true}, "c"]),
            serde_json::json!({"multiple": true, "confirm": "Open"}),
        );
        let rows = pick_rows(&spec, "");
        assert_eq!(
            rows.last()
                .map(|row| (row.kind.clone(), row.label.as_str())),
            Some((RowKind::Confirm, "Open"))
        );
        assert_eq!(
            answer_for(&spec, &RowKind::Confirm, "", &[true, false, true]),
            Some(PromptAnswer::Ids(vec!["a".into(), "c".into()]))
        );
        assert_eq!(
            answer_for(&spec, &RowKind::Confirm, "", &[false, false, false]),
            Some(PromptAnswer::Ids(Vec::new()))
        );
        assert_eq!(answer_for(&spec, &RowKind::Item(0), "", &[false; 3]), None);
        assert!(spec.items()[1].selected());
    }

    #[test]
    fn an_input_sends_the_typed_text_unchanged_and_labels_its_row_with_it() {
        let spec: PromptSpec =
            serde_json::from_value(serde_json::json!({"kind": "input", "title": "Name"})).unwrap();
        assert_eq!(
            input_rows(&spec, " a b ")[0].label,
            "Use \u{201c} a b \u{201d}"
        );
        assert_eq!(
            answer_for(&spec, &RowKind::Confirm, " a b ", &[]),
            Some(PromptAnswer::Text(" a b ".into()))
        );
        assert_eq!(
            answer_for(&spec, &RowKind::Confirm, "", &[]),
            Some(PromptAnswer::Text(String::new()))
        );
        let labelled: PromptSpec = serde_json::from_value(
            serde_json::json!({"kind": "input", "title": "Name", "confirm": "Create branch"}),
        )
        .unwrap();
        assert_eq!(input_rows(&labelled, "x")[0].label, "Create branch");
    }

    #[test]
    fn every_documented_icon_has_an_asset_and_others_have_none() {
        for name in paneflow_api::methods::prompt::ICONS {
            let path = icon_path(name).unwrap_or_else(|| panic!("{name} has no icon"));
            assert!(
                crate::assets::Assets::get(path).is_some(),
                "{path} is embedded"
            );
        }
        assert_eq!(icon_path("rocket-ship"), None);
    }

    #[test]
    fn the_list_never_runs_past_the_bottom_of_the_smallest_window() {
        let row = ROW_HEIGHT + ROW_SPACING;
        assert_eq!(list_height(3, 1000.), px(3. * row));
        assert_eq!(list_height(2_000, 1000.), MENU_MAX_HEIGHT);
        let short = f32::from(list_height(2_000, 500.));
        assert!(short + PROMPT_TOP_MARGIN + PROMPT_CHROME <= 500.);
        assert_eq!(list_height(2_000, 100.), px(row), "one row stays reachable");
    }

    #[test]
    fn two_thousand_items_filter_to_their_matches() {
        let items: Vec<String> = (0..2_000).map(|index| format!("item-{index}")).collect();
        let spec = pick(serde_json::json!(items), serde_json::json!({}));
        assert_eq!(pick_rows(&spec, "").len(), 2_000);
        let rows = pick_rows(&spec, "item-1999");
        assert_eq!(rows[0].label, "item-1999");
    }
}

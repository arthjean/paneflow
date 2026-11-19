use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Entity, KeyDownEvent, Subscription, prelude::FluentBuilder};
use regex::{Regex, RegexBuilder};

use super::*;
use crate::ui_primitives::{AccessibleControlExt, ROW_RADIUS, squircle_skin};
use crate::widgets::text_input::TextInput;

pub(crate) const FIND_MATCH_CAP: usize = 100_000;
const FIND_CHUNK_BYTES: usize = 1 << 20;
const FIND_LIVE_CHECK: usize = 1_024;
const FIND_BAR_WIDTH: f32 = 440.0;
const FIND_BAR_INSET: f32 = 6.0;
const FIND_ROW_H: f32 = 36.0;
const FIND_CONTROL: f32 = 28.0;

pub(crate) const MATCH_CASE_LABEL: &str = "Match Case";
pub(crate) const REGEX_LABEL: &str = "Regular Expression";
const TOGGLE_REPLACE_LABEL: &str = "Toggle Replace";
const PREVIOUS_LABEL: &str = "Previous Match";
const NEXT_LABEL: &str = "Next Match";
const CLOSE_LABEL: &str = "Close";
const REPLACE_LABEL: &str = "Replace";
const REPLACE_ALL_LABEL: &str = "Replace All";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FindRequest {
    query: String,
    match_case: bool,
    regex: bool,
    revision: u64,
}

impl FindRequest {
    fn same_query(&self, other: &Self) -> bool {
        self.query == other.query
            && self.match_case == other.match_case
            && self.regex == other.regex
    }
}

#[derive(Debug)]
pub(crate) enum FindPlan {
    Empty,
    GoToLine(usize),
    Search(Regex),
}

pub(crate) fn find_plan(query: &str, match_case: bool, regex: bool) -> Result<FindPlan, String> {
    if query.is_empty() {
        return Ok(FindPlan::Empty);
    }
    if let Some(digits) = query.strip_prefix(':')
        && !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Ok(FindPlan::GoToLine(digits.parse().unwrap_or(usize::MAX)));
    }
    let pattern = if regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!match_case)
        .multi_line(true)
        .build()
        .map(FindPlan::Search)
        .map_err(|err| parser_message(&err))
}

fn parser_message(err: &regex::Error) -> String {
    let text = err.to_string();
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches("error:").trim().to_string())
        .unwrap_or(text)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FindOutcome {
    pub(crate) matches: Vec<Range<usize>>,
    pub(crate) truncated: bool,
}

fn chunk_end(text: &str, from: usize, literal: bool) -> usize {
    if !literal {
        return text.len();
    }
    let probe = from.saturating_add(FIND_CHUNK_BYTES);
    if probe >= text.len() {
        return text.len();
    }
    text[probe..]
        .find('\n')
        .map_or(text.len(), |at| probe + at + 1)
}

pub(crate) fn find_all(
    re: &Regex,
    text: &str,
    literal: bool,
    live: impl Fn() -> bool,
) -> Option<FindOutcome> {
    let mut outcome = FindOutcome::default();
    let mut start = 0usize;
    while start < text.len() {
        if !live() {
            return None;
        }
        let end = chunk_end(text, start, literal);
        let haystack = &text[..end];
        let mut at = start;
        while at <= end {
            let Some(found) = re.find_at(haystack, at) else {
                break;
            };
            if found.start() == found.end() {
                at = found.end()
                    + haystack[found.end()..]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8);
                continue;
            }
            outcome.matches.push(found.range());
            if outcome.matches.len() >= FIND_MATCH_CAP {
                outcome.truncated = true;
                return Some(outcome);
            }
            if outcome.matches.len().is_multiple_of(FIND_LIVE_CHECK) && !live() {
                return None;
            }
            at = found.end();
        }
        start = end;
    }
    Some(outcome)
}

pub(crate) fn replacement_ops(
    re: &Regex,
    text: &str,
    matches: &[Range<usize>],
    replacement: &str,
    expand: bool,
) -> Vec<(Range<usize>, String)> {
    matches
        .iter()
        .rev()
        .filter_map(|found| {
            if !expand {
                return Some((found.clone(), replacement.to_string()));
            }
            let captures = re.captures_at(text, found.start)?;
            let whole = captures.get(0)?;
            if whole.range() != *found {
                return None;
            }
            let mut out = String::new();
            captures.expand(replacement, &mut out);
            Some((found.clone(), normalize_newlines(&out).into_owned()))
        })
        .collect()
}

pub(crate) fn match_color() -> Hsla {
    gpui::hsla(0.14, 0.95, 0.55, 0.28)
}

pub(crate) fn current_match_color() -> Hsla {
    gpui::hsla(0.10, 1.0, 0.55, 0.55)
}

pub(crate) struct FindBar {
    pub(crate) query: Entity<TextInput>,
    pub(crate) replacement: Entity<TextInput>,
    pub(crate) match_case: bool,
    pub(crate) regex: bool,
    pub(crate) replace_open: bool,
    pub(crate) matches: Arc<Vec<Range<usize>>>,
    pub(crate) truncated: bool,
    pub(crate) current: Option<usize>,
    pub(crate) error: Option<String>,
    pub(crate) go_to: Option<usize>,
    searched: Option<FindRequest>,
    pending: Option<FindRequest>,
    resume_at: Option<usize>,
    generation: Arc<AtomicU64>,
    _query_changes: Subscription,
}

impl FindBar {
    pub(crate) fn status(&self) -> String {
        if self.error.is_some() {
            return "Invalid".to_string();
        }
        if let Some(line) = self.go_to {
            return format!("Go to line {line}");
        }
        if self
            .searched
            .as_ref()
            .is_none_or(|req| req.query.is_empty())
        {
            return String::new();
        }
        if self.matches.is_empty() {
            return "No results".to_string();
        }
        let plus = if self.truncated { "+" } else { "" };
        match self.current {
            Some(index) => format!("{} of {}{plus}", index + 1, self.matches.len()),
            None => format!("{}{plus} results", self.matches.len()),
        }
    }
}

impl CodeView {
    pub(crate) fn find_bar(&self) -> Option<&FindBar> {
        self.find.as_ref()
    }

    pub(super) fn open_find(&mut self, _: &CeFind, window: &mut Window, cx: &mut Context<Self>) {
        let prefill = self.state.document().and_then(|doc| {
            let range = self.selection.range();
            let text = doc.slice_string(range);
            (!text.is_empty() && !text.contains('\n')).then_some(text)
        });
        if self.find.is_none() {
            let query = cx
                .new(|cx| TextInput::new("", "Search", cx).with_accessible_name("Search in file"));
            let replacement =
                cx.new(|cx| TextInput::new("", "Replace", cx).with_accessible_name("Replace with"));
            let changes = cx.observe(&query, |view: &mut Self, _, cx| view.refresh_find(cx));
            self.find = Some(FindBar {
                query,
                replacement,
                match_case: false,
                regex: false,
                replace_open: false,
                matches: Arc::new(Vec::new()),
                truncated: false,
                current: None,
                error: None,
                go_to: None,
                searched: None,
                pending: None,
                resume_at: None,
                generation: Arc::new(AtomicU64::new(0)),
                _query_changes: changes,
            });
        }
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let query = find.query.clone();
        if let Some(text) = prefill {
            query.update(cx, |input, cx| input.set_value(text, cx));
        }
        let focus = query.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        self.refresh_find(cx);
        cx.notify();
    }

    pub(crate) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = self.find.take() {
            find.generation.fetch_add(1, Ordering::SeqCst);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn find_request(&self, cx: &App) -> Option<FindRequest> {
        let find = self.find.as_ref()?;
        let doc = self.state.document()?;
        Some(FindRequest {
            query: find.query.read(cx).value(),
            match_case: find.match_case,
            regex: find.regex,
            revision: doc.revision(),
        })
    }

    pub(crate) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.find_request(cx) else {
            return;
        };
        let Some(find) = self.find.as_mut() else {
            return;
        };
        if find.searched.as_ref() == Some(&request) || find.pending.as_ref() == Some(&request) {
            return;
        }
        let generation = find.generation.fetch_add(1, Ordering::SeqCst) + 1;
        match find_plan(&request.query, request.match_case, request.regex) {
            Err(message) => {
                find.error = Some(message);
                find.go_to = None;
                find.matches = Arc::new(Vec::new());
                find.current = None;
                find.truncated = false;
                find.pending = None;
                find.searched = Some(request);
                cx.notify();
            }
            Ok(FindPlan::Empty) => {
                find.error = None;
                find.go_to = None;
                find.matches = Arc::new(Vec::new());
                find.current = None;
                find.truncated = false;
                find.pending = None;
                find.searched = Some(request);
                cx.notify();
            }
            Ok(FindPlan::GoToLine(line)) => {
                find.error = None;
                find.go_to = Some(line);
                find.matches = Arc::new(Vec::new());
                find.current = None;
                find.truncated = false;
                find.pending = None;
                find.searched = Some(request);
                cx.notify();
            }
            Ok(FindPlan::Search(re)) => {
                find.error = None;
                find.go_to = None;
                find.pending = Some(request.clone());
                let live = find.generation.clone();
                let literal = !request.regex;
                let Some(doc) = self.state.document() else {
                    return;
                };
                let text = doc.text().clone();
                spawn_blocking_then(
                    cx,
                    move || {
                        let haystack = text.to_string();
                        find_all(&re, &haystack, literal, || {
                            live.load(Ordering::SeqCst) == generation
                        })
                    },
                    move |view: &mut Self, outcome, cx| {
                        if let Some(outcome) = outcome {
                            view.accept_find(request, outcome, cx);
                        }
                    },
                );
            }
        }
    }

    fn accept_find(&mut self, request: FindRequest, outcome: FindOutcome, cx: &mut Context<Self>) {
        let cursor = self.selection.range().start;
        let Some(find) = self.find.as_mut() else {
            return;
        };
        if find.pending.as_ref() != Some(&request) {
            return;
        }
        let new_query = find
            .searched
            .as_ref()
            .is_none_or(|previous| !previous.same_query(&request));
        let resume = find.resume_at.take();
        let anchor = resume.unwrap_or(cursor);
        let current = (!outcome.matches.is_empty()).then(|| {
            outcome
                .matches
                .partition_point(|found| found.start < anchor)
                .min(outcome.matches.len() - 1)
        });
        let current = if resume.is_none() && !new_query {
            find.current
                .filter(|index| *index < outcome.matches.len())
                .or(current)
        } else {
            current
        };
        find.matches = Arc::new(outcome.matches);
        find.truncated = outcome.truncated;
        find.current = current;
        find.pending = None;
        find.searched = Some(request);
        if new_query || resume.is_some() {
            self.select_current_match(cx);
        }
        cx.notify();
    }

    fn select_current_match(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self
            .find
            .as_ref()
            .and_then(|find| find.matches.get(find.current?).cloned())
        else {
            return;
        };
        self.dismiss_reveal();
        self.end_typing_group();
        self.selection = CodeSelection {
            anchor: found.start,
            head: found.end,
        };
        if let Some(doc) = self.state.document() {
            self.goal_column = cursor::goal_column(doc, found.end);
        }
        self.after_motion(cx);
    }

    pub(crate) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        if let Some(line) = find.go_to {
            self.go_to_line(line, cx);
            return;
        }
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        find.current = Some(match (find.current, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
        });
        self.select_current_match(cx);
        cx.notify();
    }

    pub(crate) fn go_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.reveal(FileReveal::line(line), cx);
    }

    pub(crate) fn toggle_find_option(&mut self, regex: bool, cx: &mut Context<Self>) {
        if let Some(find) = self.find.as_mut() {
            if regex {
                find.regex = !find.regex;
            } else {
                find.match_case = !find.match_case;
            }
        }
        self.refresh_find(cx);
        cx.notify();
    }

    pub(crate) fn toggle_replace_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            return;
        }
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.replace_open = !find.replace_open;
        let focus = if find.replace_open {
            find.replacement.read(cx).focus_handle.clone()
        } else {
            find.query.read(cx).focus_handle.clone()
        };
        window.focus(&focus, cx);
        cx.notify();
    }

    fn fresh_find_regex(&self, cx: &App) -> Option<(Regex, bool, String)> {
        let find = self.find.as_ref()?;
        let request = self.find_request(cx)?;
        if find.searched.as_ref() != Some(&request) {
            return None;
        }
        let FindPlan::Search(re) =
            find_plan(&request.query, request.match_case, request.regex).ok()?
        else {
            return None;
        };
        Some((re, request.regex, find.replacement.read(cx).value()))
    }

    pub(crate) fn replace_current(&mut self, cx: &mut Context<Self>) {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            return;
        }
        let Some((re, expand, replacement)) = self.fresh_find_regex(cx) else {
            return;
        };
        let Some(found) = self
            .find
            .as_ref()
            .and_then(|find| find.matches.get(find.current?).cloned())
        else {
            return;
        };
        let Some(doc) = self.state.document() else {
            return;
        };
        let window_start = doc.line_to_byte(doc.byte_to_line(found.start));
        let last_line = doc.byte_to_line(found.end);
        let window_end = doc
            .line_byte_range(last_line)
            .map_or(doc.len_bytes(), |range| range.end);
        let haystack = doc.slice_string(window_start..window_end);
        let local = found.start - window_start..found.end - window_start;
        let ops: Vec<(Range<usize>, String)> = replacement_ops(
            &re,
            &haystack,
            std::slice::from_ref(&local),
            &replacement,
            expand,
        )
        .into_iter()
        .map(|(range, text)| (range.start + window_start..range.end + window_start, text))
        .collect();
        let Some((range, text)) = ops.first().cloned() else {
            return;
        };
        let resume = range.start + text.len();
        self.end_typing_group();
        if self.splice_all(&ops, CodeSelection::at(resume), EditGroup::Atomic, cx)
            && let Some(find) = self.find.as_mut()
        {
            find.resume_at = Some(resume);
        }
        self.refresh_find(cx);
    }

    pub(crate) fn replace_all(&mut self, cx: &mut Context<Self>) {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            return;
        }
        let Some((re, expand, replacement)) = self.fresh_find_regex(cx) else {
            return;
        };
        let Some(find) = self.find.as_ref() else {
            return;
        };
        if find.matches.is_empty() {
            return;
        }
        let matches = find.matches.clone();
        let Some(doc) = self.state.document() else {
            return;
        };
        let text = doc.text().clone();
        let revision = doc.revision();
        spawn_blocking_then(
            cx,
            move || {
                let haystack = text.to_string();
                replacement_ops(&re, &haystack, &matches, &replacement, expand)
            },
            move |view: &mut Self, ops, cx| {
                if view.state.document().map(CodeDocument::revision) != Some(revision)
                    || ops.is_empty()
                {
                    return;
                }
                let caret = view.selection;
                view.end_typing_group();
                view.splice_all(&ops, caret, EditGroup::Atomic, cx);
                view.refresh_find(cx);
            },
        );
    }

    pub(crate) fn find_match_bands(&self, range: Range<usize>) -> Vec<MatchBand> {
        let Some(find) = self.find.as_ref() else {
            return Vec::new();
        };
        let revision = self.state.document().map(CodeDocument::revision);
        if find.pending.is_some()
            || find.searched.as_ref().map(|request| request.revision) != revision
        {
            return Vec::new();
        }
        let first = find
            .matches
            .partition_point(|found| found.end <= range.start);
        find.matches[first..]
            .iter()
            .enumerate()
            .take_while(|(_, found)| found.start < range.end)
            .map(|(offset, found)| MatchBand {
                range: found.start.max(range.start)..found.end.min(range.end),
                color: if find.current == Some(first + offset) {
                    current_match_color()
                } else {
                    match_color()
                },
            })
            .filter(|band| band.range.start < band.range.end)
            .collect()
    }

    fn find_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                let in_replace = self
                    .find
                    .as_ref()
                    .is_some_and(|find| find.replacement.read(cx).focus_handle.is_focused(window));
                if in_replace {
                    self.replace_current(cx);
                } else {
                    self.find_step(!event.keystroke.modifiers.shift, cx);
                }
                cx.stop_propagation();
            }
            "escape" => {
                self.close_find(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    pub(super) fn render_find_bar(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let editable = self.state.document().is_some_and(|doc| !doc.is_read_only());
        let hover = crate::app::constants::sidebar_tab_hover_background();
        let has_matches = !find.matches.is_empty() || find.go_to.is_some();
        let icon_button =
            move |id: &'static str, label: &'static str, icon: &'static str, color: Hsla| {
                squircle_skin(
                    div()
                        .id(id)
                        .accessible_control(gpui::accesskit::Role::Button, label)
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(FIND_CONTROL)),
                    id,
                    px(8.),
                    None,
                    Some(hover),
                )
                .child(
                    gpui::svg()
                        .size(px(14.))
                        .flex_none()
                        .path(icon)
                        .text_color(color),
                )
            };
        let toggle = move |id: &'static str, label: &'static str, glyph: &'static str, on: bool| {
            div()
                .id(id)
                .accessible_control(gpui::accesskit::Role::Button, label)
                .aria_toggled(if on {
                    gpui::accesskit::Toggled::True
                } else {
                    gpui::accesskit::Toggled::False
                })
                .flex_none()
                .h(px(22.))
                .px(px(5.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { ui.text } else { ui.muted.opacity(0.6) })
                .when(on, |toggle| toggle.bg(hover))
                .hover(|style| style.bg(hover))
                .child(glyph)
        };
        let nav = if has_matches {
            ui.muted
        } else {
            ui.muted.opacity(0.35)
        };
        let replace_toggle = editable.then(|| {
            icon_button(
                "code-find-toggle-replace",
                TOGGLE_REPLACE_LABEL,
                if find.replace_open {
                    "icons/chevron_down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                ui.muted,
            )
            .on_pointer_press(
                cx.listener(|this, _, window, cx| this.toggle_replace_row(window, cx)),
            )
        });
        let field = |input: &Entity<TextInput>| {
            div()
                .flex_1()
                .min_w(px(48.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(ui.text)
                .child(input.clone())
        };
        let status = find.status();
        let first_row = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(FIND_ROW_H))
            .pl(px(4.))
            .pr(px(4.))
            .gap(px(6.))
            .children(replace_toggle)
            .when(!editable, |row| row.pl(px(12.)))
            .child(field(&find.query))
            .child(
                toggle(
                    "code-find-match-case",
                    MATCH_CASE_LABEL,
                    "Aa",
                    find.match_case,
                )
                .on_pointer_press(
                    cx.listener(|this, _, _window, cx| this.toggle_find_option(false, cx)),
                ),
            )
            .child(
                toggle("code-find-regex", REGEX_LABEL, ".*", find.regex).on_pointer_press(
                    cx.listener(|this, _, _window, cx| this.toggle_find_option(true, cx)),
                ),
            )
            .when(!status.is_empty(), |row| {
                row.child(
                    div()
                        .id("code-find-status")
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(if find.error.is_some() {
                            ui.agent_error
                        } else {
                            ui.muted
                        })
                        .child(status),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(px(1.))
                    .h(px(16.))
                    .bg(ui.text.opacity(0.12)),
            )
            .child(
                icon_button(
                    "code-find-previous",
                    PREVIOUS_LABEL,
                    "icons/chevron_up.svg",
                    nav,
                )
                .on_pointer_press(cx.listener(|this, _, _window, cx| this.find_step(false, cx))),
            )
            .child(
                icon_button("code-find-next", NEXT_LABEL, "icons/chevron_down.svg", nav)
                    .on_pointer_press(cx.listener(|this, _, _window, cx| this.find_step(true, cx))),
            )
            .child(
                icon_button("code-find-close", CLOSE_LABEL, "icons/close.svg", ui.muted)
                    .on_pointer_press(
                        cx.listener(|this, _, window, cx| this.close_find(window, cx)),
                    ),
            );
        let text_button = move |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .accessible_control(gpui::accesskit::Role::Button, label)
                .flex_none()
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(ui.border)
                .text_size(px(12.))
                .text_color(if enabled {
                    ui.text
                } else {
                    ui.muted.opacity(0.5)
                })
                .when(enabled, |button| button.hover(|style| style.bg(hover)))
                .child(label)
        };
        let can_replace = !find.matches.is_empty();
        let replace_row = (editable && find.replace_open).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(FIND_ROW_H))
                .pl(px(4. + FIND_CONTROL + 6.))
                .pr(px(8.))
                .gap(px(6.))
                .child(field(&find.replacement))
                .child(
                    text_button("code-find-replace", REPLACE_LABEL, can_replace).on_pointer_press(
                        cx.listener(|this, _, _window, cx| this.replace_current(cx)),
                    ),
                )
                .child(
                    text_button("code-find-replace-all", REPLACE_ALL_LABEL, can_replace)
                        .on_pointer_press(cx.listener(|this, _, _window, cx| this.replace_all(cx))),
                )
        });
        let error_row = find.error.clone().map(|message| {
            div()
                .id("code-find-error")
                .px(px(12.))
                .pb(px(8.))
                .text_size(px(12.))
                .text_color(ui.agent_error)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(message)
        });
        let border = if find.error.is_some() {
            ui.agent_error
        } else {
            ui.border
        };
        let width = (f32::from(self.scroll.bounds().size.width) - 2.0 * FIND_BAR_INSET)
            .clamp(0.0, FIND_BAR_WIDTH);
        let panel = squircle_skin(
            div()
                .id("code-find-bar")
                .occlude()
                .flex()
                .flex_col()
                .w(px(width))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(Self::find_key_down)),
            "code-find-bar",
            ROW_RADIUS,
            None,
            None,
        )
        .child(
            crate::ui_primitives::squircle::squircle_fill_beneath_border(
                ROW_RADIUS,
                ui.subtle,
                px(1.),
                border,
            ),
        )
        .child(first_row)
        .children(replace_row)
        .children(error_row);
        Some(
            div()
                .absolute()
                .top(px(FIND_BAR_INSET))
                .right(px(FIND_BAR_INSET + super::super::navigation::SCROLLBAR_SIZE))
                .child(panel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn search(query: &str, match_case: bool, regex: bool, text: &str) -> Vec<Range<usize>> {
        let FindPlan::Search(re) = find_plan(query, match_case, regex).expect("a valid query")
        else {
            panic!("{query} is a search");
        };
        find_all(&re, text, !regex, || true).expect("live").matches
    }

    #[test]
    fn a_plain_query_matches_every_occurrence_and_ignores_case_by_default() {
        let text = "Alpha alpha ALPHA beta";
        assert_eq!(
            search("alpha", false, false, text),
            vec![0..5, 6..11, 12..17]
        );
        assert_eq!(search("alpha", true, false, text), vec![6..11]);
        assert_eq!(
            search("a.p", false, false, "a.p alp"),
            vec![0..3],
            "a plain query is literal"
        );
    }

    #[test]
    fn a_regex_query_uses_line_anchors_and_skips_empty_matches() {
        let text = "fn one()\nlet x = 1;\nfn two()\n";
        assert_eq!(search("^fn \\w+", true, true, text), vec![0..6, 20..26]);
        assert!(search("x*", true, true, "abc").is_empty());
    }

    #[test]
    fn an_invalid_regex_reports_the_parser_message() {
        let err = find_plan("(unclosed", false, true).expect_err("invalid");
        assert_eq!(err, "unclosed group");
        assert!(
            find_plan("(unclosed", false, false).is_ok(),
            "a plain query never fails"
        );
    }

    #[test]
    fn a_colon_number_is_a_go_to_line() {
        assert!(matches!(
            find_plan(":120", false, false),
            Ok(FindPlan::GoToLine(120))
        ));
        assert!(matches!(
            find_plan(":", false, false),
            Ok(FindPlan::Search(_))
        ));
        assert!(matches!(
            find_plan(":12a", false, false),
            Ok(FindPlan::Search(_))
        ));
        assert!(matches!(
            find_plan(":99999999999999999999999", false, false),
            Ok(FindPlan::GoToLine(usize::MAX))
        ));
        assert!(matches!(find_plan("", false, false), Ok(FindPlan::Empty)));
    }

    #[test]
    fn a_literal_search_finds_matches_on_both_sides_of_a_chunk_boundary() {
        let mut text = String::new();
        let mut row = 0usize;
        while text.len() < 3 * FIND_CHUNK_BYTES {
            text.push_str("filler line without it\n");
            row += 1;
            if row.is_multiple_of(251) {
                text.push_str("needle\n");
            }
        }
        let expected = text.matches("needle").count();
        assert!(expected > 10);
        assert_eq!(search("needle", true, false, &text).len(), expected);
    }

    #[test]
    fn a_superseded_search_stops_and_returns_nothing() {
        let text = "x\n".repeat(3 * FIND_CHUNK_BYTES / 2);
        let FindPlan::Search(re) = find_plan("x", true, false).expect("valid") else {
            panic!("search");
        };
        let live = AtomicBool::new(true);
        let outcome = find_all(&re, &text, true, || {
            live.swap(false, std::sync::atomic::Ordering::SeqCst)
        });
        assert!(outcome.is_none(), "a newer request cancels the scan");
    }

    #[test]
    fn the_match_list_is_capped() {
        let text = "a".repeat(FIND_MATCH_CAP + 50);
        let FindPlan::Search(re) = find_plan("a", true, false).expect("valid") else {
            panic!("search");
        };
        let outcome = find_all(&re, &text, true, || true).expect("live");
        assert_eq!(outcome.matches.len(), FIND_MATCH_CAP);
        assert!(outcome.truncated);
    }

    use gpui::{TestAppContext, VisualTestContext};

    use super::super::tests::{frame, scrolled, view_named};

    fn setup(cx: &mut TestAppContext) {
        cx.update(|cx| {
            super::super::keymap::register_keybindings(cx);
            crate::widgets::text_input::register_keybindings(cx);
        });
    }

    fn focus_editor(view: &Entity<CodeView>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| view.read(cx).focus.clone().focus(window, cx));
        cx.run_until_parked();
    }

    fn type_query(view: &Entity<CodeView>, cx: &mut VisualTestContext, query: &str) {
        let input = view.read_with(cx, |view, _| view.find_bar().expect("open").query.clone());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value(query.to_string(), cx);
                input.focus_handle.focus(window, cx);
            })
        });
        cx.run_until_parked();
        frame(view, cx);
    }

    fn status(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |view, _| view.find_bar().expect("open").status())
    }

    fn seventeen_alphas() -> String {
        (0..17)
            .map(|row| format!("let alpha_{row} = {row};\n"))
            .collect()
    }

    #[gpui::test]
    fn secondary_f_opens_the_bar_prefilled_with_a_one_line_selection(cx: &mut TestAppContext) {
        setup(cx);
        let (view, cx) = scrolled(cx, "/nonexistent/find.rs", &seventeen_alphas());
        focus_editor(&view, cx);
        view.update(cx, |view, cx| {
            view.selection = CodeSelection { anchor: 4, head: 9 };
            cx.notify();
        });
        cx.simulate_keystrokes("secondary-f");
        cx.run_until_parked();
        frame(&view, cx);
        let (value, focused) = view.read_with(cx, |view, cx| {
            let find = view.find_bar().expect("secondary-f opened the bar");
            (
                find.query.read(cx).value(),
                find.query.read(cx).focus_handle.clone(),
            )
        });
        assert_eq!(value, "alpha");
        assert!(
            cx.update(|window, _| focused.is_focused(window)),
            "the field takes focus"
        );
        assert_eq!(status(&view, cx), "1 of 17");

        view.update_in(cx, |view, window, cx| {
            view.close_find(window, cx);
            view.selection = CodeSelection {
                anchor: 4,
                head: 30,
            };
            view.open_find(&CeFind, window, cx);
        });
        cx.run_until_parked();
        let value = view.read_with(cx, |view, cx| {
            view.find_bar().expect("open").query.read(cx).value()
        });
        assert_eq!(value, "", "a selection across lines does not prefill");
    }

    #[gpui::test]
    fn typing_highlights_every_match_and_counts_them(cx: &mut TestAppContext) {
        setup(cx);
        let (view, cx) = scrolled(cx, "/nonexistent/count.rs", &seventeen_alphas());
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(view.document().expect("doc").line_to_byte(2));
            view.open_find(&CeFind, window, cx);
        });
        type_query(&view, cx, "alpha");
        assert_eq!(
            status(&view, cx),
            "3 of 17",
            "the first match after the caret is current"
        );
        view.read_with(cx, |view, _| {
            let doc = view.document().expect("doc");
            let line = doc.line_byte_range(5).expect("line");
            let bands = view.match_bands(line.clone());
            assert_eq!(bands.len(), 1);
            assert_eq!(bands[0].range, line.start + 4..line.start + 9);
            assert_eq!(bands[0].color, match_color());
            let current = doc.line_byte_range(2).expect("line");
            assert_eq!(view.match_bands(current)[0].color, current_match_color());
        });
    }

    #[gpui::test]
    fn enter_steps_forward_shift_enter_back_and_escape_returns_to_the_editor(
        cx: &mut TestAppContext,
    ) {
        setup(cx);
        let (view, cx) = scrolled(cx, "/nonexistent/step.rs", &seventeen_alphas());
        view.update_in(cx, |view, window, cx| view.open_find(&CeFind, window, cx));
        type_query(&view, cx, "alpha");
        assert_eq!(status(&view, cx), "1 of 17");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(status(&view, cx), "2 of 17");
        let selected = view.read_with(cx, |view, _| view.selection());
        let line = view.read_with(cx, |view, _| view.document().expect("doc").line_to_byte(1));
        assert_eq!(selected, line + 4..line + 9, "the match is selected");
        cx.simulate_keystrokes("shift-enter shift-enter");
        cx.run_until_parked();
        assert_eq!(status(&view, cx), "17 of 17", "previous wraps around");
        let text_before = view.read_with(cx, |view, _| super::super::tests::text_of(view));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.find_bar().is_none()));
        let focus = view.read_with(cx, |view, _| view.focus.clone());
        assert!(cx.update(|window, _| focus.is_focused(window)));
        assert_eq!(
            view.read_with(cx, |view, _| super::super::tests::text_of(view)),
            text_before,
            "Enter in the bar never types into the file"
        );
    }

    #[gpui::test]
    fn match_case_and_regex_toggles_change_the_matches(cx: &mut TestAppContext) {
        setup(cx);
        let (view, cx) = scrolled(cx, "/nonexistent/toggles.rs", "Alpha alpha\nbeta 12\n");
        view.update_in(cx, |view, window, cx| view.open_find(&CeFind, window, cx));
        type_query(&view, cx, "alpha");
        assert_eq!(status(&view, cx), "1 of 2");
        view.update(cx, |view, cx| view.toggle_find_option(false, cx));
        cx.run_until_parked();
        assert_eq!(status(&view, cx), "1 of 1");
        type_query(&view, cx, "\\d+");
        assert_eq!(status(&view, cx), "No results", "a plain query is literal");
        view.update(cx, |view, cx| view.toggle_find_option(true, cx));
        cx.run_until_parked();
        assert_eq!(status(&view, cx), "1 of 1");
    }

    #[gpui::test]
    fn an_invalid_regex_shows_the_parser_message_and_highlights_nothing(cx: &mut TestAppContext) {
        setup(cx);
        let (view, cx) = scrolled(cx, "/nonexistent/bad.rs", "(a) (b)\n");
        view.update_in(cx, |view, window, cx| {
            view.open_find(&CeFind, window, cx);
            view.toggle_find_option(true, cx);
        });
        type_query(&view, cx, "(a");
        view.read_with(cx, |view, _| {
            let find = view.find_bar().expect("open");
            assert_eq!(find.error.as_deref(), Some("unclosed group"));
            assert!(find.matches.is_empty());
            assert!(view.match_bands(0..7).is_empty());
        });
    }

    #[gpui::test]
    fn replace_and_replace_all_edit_the_file_and_replace_all_undoes_at_once(
        cx: &mut TestAppContext,
    ) {
        setup(cx);
        let (view, cx) = scrolled(cx, "/nonexistent/replace.rs", "foo foo\nfoo\n");
        view.update_in(cx, |view, window, cx| {
            view.open_find(&CeFind, window, cx);
            view.toggle_replace_row(window, cx);
        });
        type_query(&view, cx, "foo");
        let replacement = view.read_with(cx, |view, _| {
            view.find_bar().expect("open").replacement.clone()
        });
        cx.update(|_, cx| replacement.update(cx, |input, cx| input.set_value("bar", cx)));
        view.update(cx, |view, cx| view.replace_current(cx));
        cx.run_until_parked();
        frame(&view, cx);
        assert_eq!(
            view.read_with(cx, |view, _| super::super::tests::text_of(view)),
            "bar foo\nfoo\n"
        );
        assert_eq!(
            status(&view, cx),
            "1 of 2",
            "the next match becomes current"
        );

        view.update(cx, |view, cx| view.replace_all(cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| super::super::tests::text_of(view)),
            "bar bar\nbar\n"
        );
        view.update_in(cx, |view, window, cx| view.undo(&CeUndo, window, cx));
        assert_eq!(
            view.read_with(cx, |view, _| super::super::tests::text_of(view)),
            "bar foo\nfoo\n",
            "one undo takes back the whole Replace All"
        );
    }

    #[gpui::test]
    fn a_read_only_file_has_no_replace_row(cx: &mut TestAppContext) {
        setup(cx);
        let giant = format!(
            "{}\nfoo\n",
            "x".repeat(super::super::super::load::MAX_LINE_CHARS + 1)
        );
        let (view, cx) = scrolled(cx, "/nonexistent/ro.json", &giant);
        view.update_in(cx, |view, window, cx| {
            view.open_find(&CeFind, window, cx);
            view.toggle_replace_row(window, cx);
        });
        type_query(&view, cx, "foo");
        view.read_with(cx, |view, _| {
            assert!(view.document().expect("doc").is_read_only());
            assert!(!view.find_bar().expect("open").replace_open);
        });
        view.update(cx, |view, cx| view.replace_all(cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| super::super::tests::text_of(view)),
            giant
        );
    }

    #[gpui::test]
    fn a_colon_line_number_goes_to_that_line_or_the_last_one(cx: &mut TestAppContext) {
        setup(cx);
        let text: String = (0..300).map(|row| format!("row {row}\n")).collect();
        let (view, cx) = scrolled(cx, "/nonexistent/goto.rs", &text);
        view.update_in(cx, |view, window, cx| view.open_find(&CeFind, window, cx));
        type_query(&view, cx, ":120");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.cursor_row()), 119);
        let visible = view.read_with(cx, |view, _| view.visible_row_range());
        assert!(visible.contains(&119), "the line is revealed");
        type_query(&view, cx, ":99999");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let last = view.read_with(cx, |view, _| view.document().expect("doc").line_count() - 1);
        assert_eq!(view.read_with(cx, |view, _| view.cursor_row()), last);
    }

    #[gpui::test]
    fn stepping_to_a_match_clears_the_highlight_of_a_revealed_line(cx: &mut TestAppContext) {
        setup(cx);
        let text: String = (0..300).map(|row| format!("row {row}\n")).collect();
        let (view, cx) = scrolled(cx, "/nonexistent/goto-then-find.rs", &text);
        view.update_in(cx, |view, window, cx| view.open_find(&CeFind, window, cx));
        type_query(&view, cx, ":120");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        frame(&view, cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.revealed_lines()),
            Some(119..120)
        );
        type_query(&view, cx, "row 250");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.cursor_row()), 250);
        assert_eq!(
            view.read_with(cx, |view, _| view.revealed_lines()),
            None,
            "the wash leaves with the caret"
        );
    }

    #[gpui::test]
    fn a_newer_query_supersedes_a_search_still_in_flight(cx: &mut TestAppContext) {
        setup(cx);
        let (view, cx) = view_named(
            cx,
            "/nonexistent/flight.rs",
            "alpha beta\n",
            crate::app::diff_dock::code::highlight::HIGHLIGHT_FRAME_BUDGET,
        );
        view.update_in(cx, |view, window, cx| view.open_find(&CeFind, window, cx));
        let input = view.read_with(cx, |view, _| view.find_bar().expect("open").query.clone());
        cx.update(|_, cx| {
            input.update(cx, |input, cx| input.set_value("alpha", cx));
            view.update(cx, |view, cx| view.refresh_find(cx));
            input.update(cx, |input, cx| input.set_value("beta", cx));
            view.update(cx, |view, cx| view.refresh_find(cx));
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let find = view.find_bar().expect("open");
            assert_eq!(
                find.matches.as_slice(),
                std::slice::from_ref(&(6..10)),
                "only the newest query lands"
            );
        });
    }

    #[test]
    fn replacement_ops_descend_and_expand_captures_in_regex_mode() {
        let text = "let a = 1;\nlet b = 2;\n";
        let FindPlan::Search(re) = find_plan("let (\\w)", true, true).expect("valid") else {
            panic!("search");
        };
        let matches = find_all(&re, text, false, || true).expect("live").matches;
        let ops = replacement_ops(&re, text, &matches, "const $1", true);
        assert_eq!(
            ops,
            vec![
                (11..16, "const b".to_string()),
                (0..5, "const a".to_string())
            ]
        );
        let literal = replacement_ops(&re, text, &matches, "$1", false);
        assert_eq!(literal[0].1, "$1", "a plain replacement is taken literally");
    }
}

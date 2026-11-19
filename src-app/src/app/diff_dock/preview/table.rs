use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

use super::Delimiter;

pub(crate) const TABLE_MAX_COLUMNS: usize = 1024;
pub(crate) const WIDTH_SAMPLE_ROWS: usize = 100;
pub(crate) const COLUMN_MIN_WIDTH: f32 = 60.0;
pub(crate) const COLUMN_MAX_WIDTH: f32 = 400.0;
pub(crate) const SHOW_SOURCE: &str = "Show source";
pub(crate) const SHOW_TABLE: &str = "Show table";
const CELL_PAD_X: f32 = 8.0;
const ROW_HEIGHT: f32 = 24.0;
const CELL_DISPLAY_CHARS: usize = 512;
const PARSING_LABEL: &str = "Reading table…";

pub(crate) fn detect_delimiter(text: &str, hint: Delimiter) -> char {
    match hint {
        Delimiter::Tab => '\t',
        Delimiter::Comma => {
            if semicolons_win(text) {
                ';'
            } else {
                ','
            }
        }
    }
}

fn semicolons_win(text: &str) -> bool {
    let mut commas = [0usize; 2];
    let mut semicolons = [0usize; 2];
    let mut record = 0usize;
    let mut second = false;
    let mut quoted = false;
    let mut field_start = true;
    let mut chars = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .chars()
        .peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            continue;
        }
        match ch {
            '"' if field_start => quoted = true,
            '\n' | '\r' => {
                if record == 1 {
                    break;
                }
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record = 1;
                field_start = true;
                continue;
            }
            ',' => commas[record] += 1,
            ';' => semicolons[record] += 1,
            _ => {}
        }
        second |= record == 1;
        field_start = ch == ',' || ch == ';';
    }
    if semicolons[0] <= commas[0] {
        return false;
    }
    if !second {
        return true;
    }
    let consistent_commas = commas[0] > 0 && commas[1] == commas[0];
    semicolons[1] == semicolons[0] && !consistent_commas
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ParsedTable {
    pub(crate) rows: Vec<Vec<SharedString>>,
    pub(crate) columns: usize,
    pub(crate) hidden_columns: usize,
}

#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<SharedString>>,
    row: Vec<SharedString>,
    cell: String,
    row_columns: usize,
    widest_row: usize,
}

impl TableBuilder {
    fn end_cell(&mut self) {
        let cell = std::mem::take(&mut self.cell);
        if self.row.len() < TABLE_MAX_COLUMNS {
            self.row.push(SharedString::from(cell));
        }
        self.row_columns += 1;
    }

    fn end_row(&mut self) {
        self.end_cell();
        self.widest_row = self.widest_row.max(self.row_columns);
        self.row_columns = 0;
        self.rows.push(std::mem::take(&mut self.row));
    }

    fn finish(self) -> ParsedTable {
        ParsedTable {
            rows: self.rows,
            columns: self.widest_row.min(TABLE_MAX_COLUMNS),
            hidden_columns: self.widest_row.saturating_sub(TABLE_MAX_COLUMNS),
        }
    }
}

pub(crate) fn parse_table(text: &str, delimiter: char) -> ParsedTable {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut builder = TableBuilder::default();
    let mut quoted = false;
    let mut cell_started = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    builder.cell.push('"');
                } else {
                    quoted = false;
                }
            } else {
                builder.cell.push(ch);
            }
            continue;
        }
        if ch == '"' && !cell_started {
            quoted = true;
            cell_started = true;
        } else if ch == delimiter {
            builder.end_cell();
            cell_started = false;
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            builder.end_row();
            cell_started = false;
        } else {
            cell_started = true;
            builder.cell.push(ch);
        }
    }
    if cell_started || !builder.row.is_empty() {
        builder.end_row();
    }
    builder.finish()
}

pub(crate) fn column_widths(table: &ParsedTable, char_width: f32) -> Vec<f32> {
    (0..table.columns)
        .map(|column| {
            let chars = table
                .rows
                .iter()
                .take(WIDTH_SAMPLE_ROWS)
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0);
            (chars as f32 * char_width + 2.0 * CELL_PAD_X).clamp(COLUMN_MIN_WIDTH, COLUMN_MAX_WIDTH)
        })
        .collect()
}

pub(crate) fn hidden_columns_banner(hidden: usize) -> String {
    format!(
        "Showing the first {} columns; {} hidden.",
        super::group_digits(TABLE_MAX_COLUMNS),
        crate::app::plural(hidden, "more column is", "more columns are")
    )
}

fn display_cell(cell: &SharedString) -> SharedString {
    let one_line = cell.contains(['\n', '\r']);
    if cell.chars().count() <= CELL_DISPLAY_CHARS && !one_line {
        return cell.clone();
    }
    cell.chars()
        .take(CELL_DISPLAY_CHARS)
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect::<String>()
        .into()
}

pub(crate) struct TableView {
    focus: FocusHandle,
    table: Option<Arc<ParsedTable>>,
    widths: Rc<Vec<f32>>,
    revision: Option<u64>,
    generation: u64,
    list: UniformListScrollHandle,
    horizontal: ScrollHandle,
    char_width: f32,
}

impl TableView {
    pub(crate) fn new(focus: FocusHandle) -> Self {
        Self {
            focus,
            table: None,
            widths: Rc::new(Vec::new()),
            revision: None,
            generation: 0,
            list: UniformListScrollHandle::new(),
            horizontal: ScrollHandle::new(),
            char_width: 7.0,
        }
    }

    pub(crate) fn needs(&self, revision: u64) -> bool {
        self.revision != Some(revision)
    }

    pub(crate) fn load(
        &mut self,
        text: String,
        delimiter: Delimiter,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        self.revision = Some(revision);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let work = cx.background_spawn(async move {
            let separator = detect_delimiter(&text, delimiter);
            parse_table(&text, separator)
        });
        cx.spawn(async move |this, cx| {
            let table = work.await;
            let _ = this.update(cx, |view: &mut Self, cx| {
                if view.generation != generation {
                    return;
                }
                view.widths = Rc::new(column_widths(&table, view.char_width));
                view.table = Some(Arc::new(table));
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn table(&self) -> Option<Arc<ParsedTable>> {
        self.table.clone()
    }

    fn row_element(
        cells: &[SharedString],
        number: Option<usize>,
        widths: &[f32],
        number_w: f32,
        header: bool,
        ui: crate::theme::UiColors,
    ) -> AnyElement {
        let mut row = div()
            .flex()
            .flex_row()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(ui.border)
            .child(
                div()
                    .flex_none()
                    .w(px(number_w))
                    .h_full()
                    .px(px(CELL_PAD_X))
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_color(ui.muted)
                    .children(number.map(|number| SharedString::from(number.to_string()))),
            );
        for (column, width) in widths.iter().enumerate() {
            let text = cells.get(column).map(display_cell).unwrap_or_default();
            row = row.child(
                div()
                    .flex_none()
                    .w(px(*width))
                    .h_full()
                    .px(px(CELL_PAD_X))
                    .flex()
                    .items_center()
                    .border_l_1()
                    .border_color(ui.border)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(if header {
                        ui.text
                    } else {
                        ui.text.opacity(0.92)
                    })
                    .when_header(header)
                    .child(text),
            );
        }
        row.into_any_element()
    }
}

trait HeaderCell {
    fn when_header(self, header: bool) -> Self;
}

impl HeaderCell for gpui::Div {
    fn when_header(self, header: bool) -> Self {
        if header {
            self.font_weight(gpui::FontWeight::SEMIBOLD)
        } else {
            self
        }
    }
}

impl Render for TableView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let font_size = crate::ui_primitives::BODY;
        let font = window.text_style().font();
        let font_id = window.text_system().resolve_font(&font);
        if let Ok(advance) = window.text_system().advance(font_id, font_size, '0') {
            self.char_width = f32::from(advance.width);
        }
        let Some(table) = self.table.clone() else {
            return div()
                .size_full()
                .track_focus(&self.focus)
                .child(crate::ui_primitives::panel_empty_state(
                    ui,
                    Some("icons/dock/loader-circle.svg"),
                    None,
                    PARSING_LABEL,
                    true,
                ))
                .into_any_element();
        };
        let widths = self.widths.clone();
        let digits = crate::app::diff_dock::code::element::digit_count(table.rows.len());
        let number_w = digits as f32 * self.char_width + 2.0 * CELL_PAD_X;
        let total_w = number_w + widths.iter().sum::<f32>();
        let header_cells = table.rows.first().cloned().unwrap_or_default();
        let header = Self::row_element(&header_cells, None, &widths, number_w, true, ui);
        let body_rows = table.rows.len().saturating_sub(1);
        let rows_table = table.clone();
        let rows_widths = widths.clone();
        let list = uniform_list(
            "csv-table-rows",
            body_rows,
            move |range: Range<usize>, _window, _cx| {
                range
                    .map(|index| {
                        let cells = rows_table
                            .rows
                            .get(index + 1)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        Self::row_element(cells, Some(index + 2), &rows_widths, number_w, false, ui)
                    })
                    .collect::<Vec<_>>()
            },
        )
        .flex_1()
        .min_h_0()
        .w(px(total_w))
        .track_scroll(&self.list);
        let banner = (table.hidden_columns > 0).then(|| {
            div()
                .flex_none()
                .px_3()
                .py_1p5()
                .text_xs()
                .border_b_1()
                .border_color(ui.border)
                .bg(ui.overlay)
                .text_color(ui.muted)
                .child(hidden_columns_banner(table.hidden_columns))
        });
        let mut scroller = div()
            .id("csv-table-horizontal")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_x_scroll()
            .track_scroll(&self.horizontal);
        scroller.style().restrict_scroll_to_axis = Some(true);
        div()
            .id("csv-table")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_size(font_size)
            .children(banner)
            .child(
                scroller.child(
                    div()
                        .w(px(total_w))
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(div().flex_none().bg(ui.overlay).child(header))
                        .child(list),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(table: &ParsedTable) -> Vec<Vec<String>> {
        table
            .rows
            .iter()
            .map(|row| row.iter().map(ToString::to_string).collect())
            .collect()
    }

    #[test]
    fn quotes_escaped_quotes_and_newlines_stay_inside_their_cell() {
        let table = parse_table(
            "\u{feff}name,note\n\"Doe, J\",\"said \"\"hi\"\"\nthen left\"\r\n",
            ',',
        );
        assert_eq!(
            cells(&table),
            vec![
                vec!["name".to_string(), "note".to_string()],
                vec!["Doe, J".to_string(), "said \"hi\"\nthen left".to_string()],
            ]
        );
        assert_eq!(table.columns, 2);
    }

    #[test]
    fn a_ragged_row_keeps_every_line_and_leaves_missing_cells_empty() {
        let table = parse_table("a,b,c\n1\n2,3,4,5\n", ',');
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.columns, 4);
        assert_eq!(table.rows[1].len(), 1);
        let widths = column_widths(&table, 7.0);
        assert_eq!(widths.len(), 4);
        assert!(
            widths
                .iter()
                .all(|w| (COLUMN_MIN_WIDTH..=COLUMN_MAX_WIDTH).contains(w))
        );
    }

    #[test]
    fn a_semicolon_wins_only_when_consistent_across_the_first_two_lines() {
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n", Delimiter::Comma), ';');
        assert_eq!(detect_delimiter("a;b;c\n1,2;3\n", Delimiter::Comma), ',');
        assert_eq!(detect_delimiter("a,b;c\n1,2;3\n", Delimiter::Comma), ',');
        assert_eq!(detect_delimiter("a;b;c", Delimiter::Comma), ';');
        assert_eq!(detect_delimiter("a;b\tc", Delimiter::Tab), '\t');
        assert_eq!(
            cells(&parse_table("x\ty\n1\t2", '\t')),
            vec![
                vec!["x".to_string(), "y".to_string()],
                vec!["1".to_string(), "2".to_string()]
            ]
        );
    }

    #[test]
    fn columns_past_the_limit_are_hidden_and_counted() {
        let wide: String = (0..TABLE_MAX_COLUMNS + 3)
            .map(|n| format!("c{n},"))
            .collect();
        let table = parse_table(wide.trim_end_matches(','), ',');
        assert_eq!(table.columns, TABLE_MAX_COLUMNS);
        assert_eq!(table.hidden_columns, 3);
        assert_eq!(table.rows[0].len(), TABLE_MAX_COLUMNS);
        assert!(hidden_columns_banner(3).contains("3 more columns are hidden"));
    }

    #[test]
    fn column_widths_come_from_the_first_hundred_rows_and_are_clamped() {
        let mut text = String::from("id,description\n");
        for _ in 0..WIDTH_SAMPLE_ROWS {
            text.push_str("1,short\n");
        }
        text.push_str(&format!("2,{}\n", "x".repeat(300)));
        let table = parse_table(&text, ',');
        let widths = column_widths(&table, 7.0);
        assert_eq!(widths[0], COLUMN_MIN_WIDTH);
        assert!(
            widths[1] < COLUMN_MAX_WIDTH,
            "the long row 102 is not sampled"
        );
        let long = parse_table(&format!("a\n{}\n", "y".repeat(500)), ',');
        assert_eq!(column_widths(&long, 7.0)[0], COLUMN_MAX_WIDTH);
    }
}

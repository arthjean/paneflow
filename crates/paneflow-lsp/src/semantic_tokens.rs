use std::borrow::Cow;

use ropey::Rope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::PositionEncoding;

const FIELDS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub token_type: u32,
    pub modifiers: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensEdit {
    pub start: u32,
    pub delete_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokensResult {
    Full {
        result_id: Option<String>,
        data: Vec<u32>,
    },
    Delta {
        result_id: Option<String>,
        edits: Vec<SemanticTokensEdit>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FullShape {
    #[serde(default)]
    result_id: Option<String>,
    data: Vec<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeltaShape {
    #[serde(default)]
    result_id: Option<String>,
    edits: Vec<SemanticTokensEdit>,
}

pub fn parse_result(value: &Value) -> Option<TokensResult> {
    if value.get("edits").is_some() {
        let delta: DeltaShape = serde_json::from_value(value.clone()).ok()?;
        return Some(TokensResult::Delta {
            result_id: delta.result_id,
            edits: delta.edits,
        });
    }
    let full: FullShape = serde_json::from_value(value.clone()).ok()?;
    Some(TokensResult::Full {
        result_id: full.result_id,
        data: full.data,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeltaOutOfRange {
    pub start: u32,
    pub len: usize,
}

pub fn apply_delta(
    previous: &[u32],
    edits: &[SemanticTokensEdit],
) -> Result<Vec<u32>, DeltaOutOfRange> {
    let mut order: Vec<&SemanticTokensEdit> = edits.iter().collect();
    order.sort_by_key(|edit| edit.start);
    let mut data = previous.to_vec();
    for edit in order.into_iter().rev() {
        let start = edit.start as usize;
        let end = start.saturating_add(edit.delete_count as usize);
        if start > data.len() || end > data.len() {
            return Err(DeltaOutOfRange {
                start: edit.start,
                len: previous.len(),
            });
        }
        data.splice(start..end, edit.data.iter().flatten().copied());
    }
    Ok(data)
}

struct LineText<'a> {
    text: Cow<'a, str>,
    content_len: usize,
    units: usize,
    bytes: usize,
}

impl<'a> LineText<'a> {
    fn of(rope: &'a Rope, line: usize) -> Self {
        let slice = rope.line(line);
        let text = match slice.as_str() {
            Some(text) => Cow::Borrowed(text),
            None => Cow::Owned(slice.to_string()),
        };
        let content_len = text.trim_end_matches(['\n', '\r']).len();
        Self {
            text,
            content_len,
            units: 0,
            bytes: 0,
        }
    }

    fn byte_at(&mut self, unit: usize, encoding: PositionEncoding) -> Option<usize> {
        match encoding {
            PositionEncoding::Utf8 => {
                (unit <= self.content_len && self.text.is_char_boundary(unit)).then_some(unit)
            }
            PositionEncoding::Utf16 => {
                if unit < self.units {
                    self.units = 0;
                    self.bytes = 0;
                }
                let content = &self.text[..self.content_len];
                while self.units < unit {
                    let ch = content[self.bytes..].chars().next()?;
                    self.units += ch.len_utf16();
                    self.bytes += ch.len_utf8();
                }
                (self.units == unit).then_some(self.bytes)
            }
        }
    }
}

pub fn decode(
    data: &[u32],
    text: &Rope,
    encoding: PositionEncoding,
    type_count: usize,
) -> Vec<SemanticToken> {
    let line_count = text.len_lines();
    let mut tokens = Vec::with_capacity(data.len() / FIELDS);
    let mut line = 0usize;
    let mut column = 0usize;
    let mut current: Option<(usize, LineText<'_>)> = None;
    let mut last_end: Option<(usize, u32)> = None;
    for &[delta_line, delta_start, length, token_type, modifiers] in data.as_chunks::<FIELDS>().0 {
        if delta_line > 0 {
            line = line.saturating_add(delta_line as usize);
            column = delta_start as usize;
        } else {
            column = column.saturating_add(delta_start as usize);
        }
        if token_type as usize >= type_count || length == 0 || line >= line_count {
            continue;
        }
        if current.as_ref().is_none_or(|(at, _)| *at != line) {
            current = Some((line, LineText::of(text, line)));
        }
        let Some((_, line_text)) = current.as_mut() else {
            continue;
        };
        let Some(start) = line_text.byte_at(column, encoding) else {
            continue;
        };
        let Some(end) = line_text.byte_at(column.saturating_add(length as usize), encoding) else {
            continue;
        };
        let (Ok(start), Ok(end), Ok(at_line)) = (
            u32::try_from(start),
            u32::try_from(end),
            u32::try_from(line),
        ) else {
            continue;
        };
        if last_end.is_some_and(|(previous_line, previous_end)| {
            previous_line == line && start < previous_end
        }) {
            continue;
        }
        last_end = Some((line, end));
        tokens.push(SemanticToken {
            line: at_line,
            start,
            end,
            token_type,
            modifiers,
        });
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token(line: u32, start: u32, end: u32, token_type: u32) -> SemanticToken {
        SemanticToken {
            line,
            start,
            end,
            token_type,
            modifiers: 0,
        }
    }

    #[test]
    fn relative_tokens_become_absolute_lines_and_byte_ranges_with_their_type_and_modifiers() {
        let text = Rope::from_str("fn main() {\n    let x = 1;\n}\n");
        let data = [0, 3, 4, 0, 1, 1, 8, 1, 1, 2, 0, 4, 1, 0, 0];
        let tokens = decode(&data, &text, PositionEncoding::Utf8, 2);
        assert_eq!(
            tokens,
            vec![
                SemanticToken {
                    line: 0,
                    start: 3,
                    end: 7,
                    token_type: 0,
                    modifiers: 1
                },
                SemanticToken {
                    line: 1,
                    start: 8,
                    end: 9,
                    token_type: 1,
                    modifiers: 2
                },
                token(1, 12, 13, 0),
            ]
        );
    }

    #[test]
    fn in_utf16_a_token_after_an_astral_character_starts_after_its_four_bytes() {
        let text = Rope::from_str("let 🦀 = crab;\n");
        let crab_end_units = 4 + 2;
        let data = [0, crab_end_units + 3, 4, 0, 0];
        let tokens = decode(&data, &text, PositionEncoding::Utf16, 1);
        let start = "let 🦀 = ".len() as u32;
        assert_eq!(tokens, vec![token(0, start, start + 4, 0)]);
        assert_eq!(start, 4 + 4 + 3);
        let utf8 = decode(&[0, start, 4, 0, 0], &text, PositionEncoding::Utf8, 1);
        assert_eq!(utf8, tokens);
    }

    #[test]
    fn a_column_inside_a_surrogate_pair_or_a_utf8_sequence_drops_only_that_token() {
        let text = Rope::from_str("🦀x\n");
        assert_eq!(
            decode(
                &[0, 1, 1, 0, 0, 0, 1, 1, 0, 0],
                &text,
                PositionEncoding::Utf16,
                1
            ),
            vec![token(0, 4, 5, 0)]
        );
        assert_eq!(
            decode(
                &[0, 1, 1, 0, 0, 0, 3, 1, 0, 0],
                &text,
                PositionEncoding::Utf8,
                1
            ),
            vec![token(0, 4, 5, 0)]
        );
    }

    #[test]
    fn a_token_past_the_end_of_its_line_is_dropped_and_the_others_kept() {
        let text = Rope::from_str("ab\r\ncd\n");
        let data = [0, 0, 2, 0, 0, 0, 1, 3, 0, 0, 1, 0, 2, 0, 0, 5, 0, 1, 0, 0];
        assert_eq!(
            decode(&data, &text, PositionEncoding::Utf8, 1),
            vec![token(0, 0, 2, 0), token(1, 0, 2, 0)]
        );
    }

    #[test]
    fn a_type_index_outside_the_legend_is_dropped_and_the_others_kept() {
        let text = Rope::from_str("a b c\n");
        let data = [0, 0, 1, 0, 0, 0, 2, 1, 7, 0, 0, 2, 1, 1, 0];
        assert_eq!(
            decode(&data, &text, PositionEncoding::Utf8, 2),
            vec![token(0, 0, 1, 0), token(0, 4, 5, 1)]
        );
    }

    #[test]
    fn overlapping_or_empty_tokens_and_a_trailing_partial_token_are_dropped() {
        let text = Rope::from_str("abcdef\n");
        let data = [
            0, 0, 4, 0, 0, 0, 2, 2, 0, 0, 0, 2, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1,
        ];
        assert_eq!(
            decode(&data, &text, PositionEncoding::Utf8, 1),
            vec![token(0, 0, 4, 0), token(0, 5, 6, 0)]
        );
    }

    #[test]
    fn delta_edits_apply_to_the_previous_array_from_the_last_edit_to_the_first() {
        let previous = [10, 11, 12, 13, 14, 15, 16, 17, 18, 19];
        let edits = vec![
            SemanticTokensEdit {
                start: 1,
                delete_count: 2,
                data: Some(vec![1, 2, 3]),
            },
            SemanticTokensEdit {
                start: 5,
                delete_count: 0,
                data: Some(vec![50]),
            },
            SemanticTokensEdit {
                start: 8,
                delete_count: 2,
                data: None,
            },
        ];
        assert_eq!(
            apply_delta(&previous, &edits),
            Ok(vec![10, 1, 2, 3, 13, 14, 50, 15, 16, 17])
        );
        let mut reversed = edits.clone();
        reversed.reverse();
        assert_eq!(
            apply_delta(&previous, &reversed),
            apply_delta(&previous, &edits)
        );
    }

    #[test]
    fn a_delta_edit_beyond_the_previous_array_is_refused() {
        let previous = [1, 2, 3];
        let beyond = SemanticTokensEdit {
            start: 4,
            delete_count: 0,
            data: Some(vec![9]),
        };
        assert_eq!(
            apply_delta(&previous, &[beyond]),
            Err(DeltaOutOfRange { start: 4, len: 3 })
        );
        let past_end = SemanticTokensEdit {
            start: 2,
            delete_count: 5,
            data: None,
        };
        assert!(apply_delta(&previous, &[past_end]).is_err());
        let append = SemanticTokensEdit {
            start: 3,
            delete_count: 0,
            data: Some(vec![4]),
        };
        assert_eq!(apply_delta(&previous, &[append]), Ok(vec![1, 2, 3, 4]));
    }

    #[test]
    fn a_response_is_read_as_full_tokens_or_as_a_delta() {
        assert_eq!(
            parse_result(&json!({"resultId": "1", "data": [0, 0, 1, 0, 0]})),
            Some(TokensResult::Full {
                result_id: Some("1".to_string()),
                data: vec![0, 0, 1, 0, 0]
            })
        );
        assert_eq!(
            parse_result(
                &json!({"resultId": "2", "edits": [{"start": 0, "deleteCount": 5, "data": []}]})
            ),
            Some(TokensResult::Delta {
                result_id: Some("2".to_string()),
                edits: vec![SemanticTokensEdit {
                    start: 0,
                    delete_count: 5,
                    data: Some(Vec::new())
                }]
            })
        );
        assert_eq!(parse_result(&Value::Null), None);
        assert_eq!(parse_result(&json!({"data": "nope"})), None);
    }
}

use gpui::Hsla;
use paneflow_lsp::protocol::TokenLegend;
use paneflow_lsp::semantic_tokens::SemanticToken;

use crate::diff::DiffSyntax;

use super::document::CodeEdit;
use super::highlight::LineRuns;

pub(crate) const CAPTURES: &[&str] = &[
    "namespace",
    "type",
    "type.builtin",
    "variable.parameter",
    "variable",
    "variable.builtin",
    "variable.special",
    "property",
    "function",
    "function.builtin",
    "keyword",
    "comment",
    "comment.doc",
    "string",
    "string.regex",
    "string.special",
    "number",
    "operator",
    "attribute",
];

const DEFAULT_LIBRARY: &str = "defaultLibrary";
const DOCUMENTATION: &str = "documentation";

fn base_capture(token_type: &str) -> Option<&'static str> {
    Some(match token_type {
        "namespace" => "namespace",
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "enumMember" => {
            "type"
        }
        "parameter" => "variable.parameter",
        "variable" | "event" => "variable",
        "property" => "property",
        "function" | "method" | "macro" | "procMacro" => "function",
        "keyword" | "modifier" => "keyword",
        "comment" => "comment",
        "string" => "string",
        "number" => "number",
        "regexp" => "string.regex",
        "operator" => "operator",
        "decorator" => "attribute",
        "selfKeyword" => "variable.special",
        "builtinType" => "type.builtin",
        "formatSpecifier" => "string.special",
        _ => return None,
    })
}

fn refined(base: &'static str, default_library: bool, documentation: bool) -> &'static str {
    match base {
        "function" if default_library => "function.builtin",
        "type" if default_library => "type.builtin",
        "variable" if default_library => "variable.builtin",
        "comment" if documentation => "comment.doc",
        other => other,
    }
}

#[cfg(test)]
pub(crate) fn capture_for<'a>(
    token_type: &str,
    modifiers: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    let base = base_capture(token_type)?;
    let (mut default_library, mut documentation) = (false, false);
    for modifier in modifiers {
        default_library |= modifier == DEFAULT_LIBRARY;
        documentation |= modifier == DOCUMENTATION;
    }
    Some(refined(base, default_library, documentation))
}

fn capture_index(name: &str) -> Option<u8> {
    CAPTURES
        .iter()
        .position(|capture| *capture == name)
        .and_then(|index| u8::try_from(index).ok())
}

pub(crate) struct LegendMap {
    types: Vec<Option<&'static str>>,
    default_library: u32,
    documentation: u32,
}

impl LegendMap {
    pub(crate) fn new(legend: &TokenLegend) -> Self {
        let bits = |wanted: &str| {
            legend
                .token_modifiers
                .iter()
                .take(32)
                .enumerate()
                .filter(|(_, name)| name.as_str() == wanted)
                .fold(0u32, |bits, (index, _)| bits | (1 << index))
        };
        Self {
            types: legend
                .token_types
                .iter()
                .map(|name| base_capture(name))
                .collect(),
            default_library: bits(DEFAULT_LIBRARY),
            documentation: bits(DOCUMENTATION),
        }
    }

    pub(crate) fn capture(&self, token_type: u32, modifiers: u32) -> Option<&'static str> {
        let base = (*self.types.get(usize::try_from(token_type).ok()?)?)?;
        Some(refined(
            base,
            modifiers & self.default_library != 0,
            modifiers & self.documentation != 0,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayerToken {
    line: u32,
    start: u32,
    end: u32,
    capture: u8,
}

pub(crate) struct SemanticLayer {
    tokens: Vec<LayerToken>,
    colors: Vec<Option<Hsla>>,
}

fn colors(syntax: &DiffSyntax) -> Vec<Option<Hsla>> {
    CAPTURES
        .iter()
        .map(|name| syntax.color_for_capture(name))
        .collect()
}

impl SemanticLayer {
    pub(crate) fn new(tokens: &[SemanticToken], legend: &TokenLegend, syntax: &DiffSyntax) -> Self {
        let map = LegendMap::new(legend);
        let tokens = tokens
            .iter()
            .filter_map(|token| {
                let capture = capture_index(map.capture(token.token_type, token.modifiers)?)?;
                Some(LayerToken {
                    line: token.line,
                    start: token.start,
                    end: token.end,
                    capture,
                })
            })
            .collect();
        Self {
            tokens,
            colors: colors(syntax),
        }
    }

    pub(crate) fn set_syntax(&mut self, syntax: &DiffSyntax) {
        self.colors = colors(syntax);
    }

    pub(crate) fn edit(&mut self, edits: &[CodeEdit]) {
        for edit in edits {
            let start_row = edit.start_point.row;
            let old_end_row = edit.old_end_point.row.max(start_row);
            let new_end_row = edit.new_end_point.row.max(start_row);
            let (Ok(start_row), Ok(old_end_row), Ok(new_end_row)) = (
                u32::try_from(start_row),
                u32::try_from(old_end_row),
                u32::try_from(new_end_row),
            ) else {
                self.tokens.clear();
                return;
            };
            let first = self.tokens.partition_point(|token| token.line < start_row);
            let after = self
                .tokens
                .partition_point(|token| token.line <= old_end_row);
            self.tokens.drain(first..after);
            if new_end_row != old_end_row {
                for token in &mut self.tokens[first..] {
                    token.line = token.line - old_end_row + new_end_row;
                }
            }
        }
    }

    pub(crate) fn has_tokens_on(&self, row: usize) -> bool {
        let Ok(row) = u32::try_from(row) else {
            return false;
        };
        let first = self.tokens.partition_point(|token| token.line < row);
        self.tokens
            .get(first)
            .is_some_and(|token| token.line == row)
    }

    pub(crate) fn push_runs(&self, row: usize, out: &mut LineRuns) {
        let Ok(row) = u32::try_from(row) else {
            return;
        };
        let first = self.tokens.partition_point(|token| token.line < row);
        for token in self.tokens[first..]
            .iter()
            .take_while(|token| token.line == row)
        {
            if let Some(color) = self
                .colors
                .get(usize::from(token.capture))
                .copied()
                .flatten()
            {
                out.push((token.start as usize..token.end as usize, color));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::document::DocPoint;
    use super::*;
    use crate::theme::paneflow_dark;

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&paneflow_dark())
    }

    #[test]
    fn every_standard_token_type_gives_the_capture_name_of_td_09() {
        let expected = [
            ("namespace", "namespace"),
            ("type", "type"),
            ("class", "type"),
            ("enum", "type"),
            ("interface", "type"),
            ("struct", "type"),
            ("typeParameter", "type"),
            ("parameter", "variable.parameter"),
            ("variable", "variable"),
            ("property", "property"),
            ("enumMember", "type"),
            ("event", "variable"),
            ("function", "function"),
            ("method", "function"),
            ("macro", "function"),
            ("keyword", "keyword"),
            ("modifier", "keyword"),
            ("comment", "comment"),
            ("string", "string"),
            ("number", "number"),
            ("regexp", "string.regex"),
            ("operator", "operator"),
            ("decorator", "attribute"),
        ];
        let standard: Vec<&str> = expected.iter().map(|(name, _)| *name).collect();
        assert_eq!(standard, paneflow_lsp::protocol::STANDARD_TOKEN_TYPES);
        for (token_type, capture) in expected {
            assert_eq!(capture_for(token_type, []), Some(capture), "{token_type}");
        }
    }

    #[test]
    fn the_rust_analyzer_types_give_their_listed_names_or_stay_transparent() {
        assert_eq!(capture_for("selfKeyword", []), Some("variable.special"));
        assert_eq!(capture_for("builtinType", []), Some("type.builtin"));
        assert_eq!(capture_for("procMacro", []), Some("function"));
        assert_eq!(capture_for("formatSpecifier", []), Some("string.special"));
        for transparent in ["lifetime", "macroBang", "unresolvedReference"] {
            assert_eq!(capture_for(transparent, []), None, "{transparent}");
        }
        for listed in paneflow_lsp::protocol::RUST_ANALYZER_TOKEN_TYPES {
            let _ = capture_for(listed, []);
        }
    }

    #[test]
    fn a_custom_type_outside_the_table_is_transparent() {
        assert_eq!(capture_for("label", []), None);
        assert_eq!(capture_for("angle", ["defaultLibrary"]), None);
        assert_eq!(capture_for("", []), None);
    }

    #[test]
    fn default_library_appends_builtin_to_functions_types_and_variables_only() {
        assert_eq!(
            capture_for("function", ["static", "defaultLibrary"]),
            Some("function.builtin")
        );
        assert_eq!(
            capture_for("method", ["defaultLibrary"]),
            Some("function.builtin")
        );
        assert_eq!(
            capture_for("struct", ["defaultLibrary"]),
            Some("type.builtin")
        );
        assert_eq!(
            capture_for("variable", ["defaultLibrary"]),
            Some("variable.builtin")
        );
        assert_eq!(
            capture_for("property", ["defaultLibrary"]),
            Some("property")
        );
        assert_eq!(capture_for("comment", ["defaultLibrary"]), Some("comment"));
    }

    #[test]
    fn documentation_on_a_comment_gives_comment_doc_and_other_modifiers_are_ignored() {
        assert_eq!(
            capture_for("comment", ["documentation"]),
            Some("comment.doc")
        );
        assert_eq!(capture_for("variable", ["documentation"]), Some("variable"));
        assert_eq!(
            capture_for("variable", ["readonly", "declaration", "mutable"]),
            Some("variable")
        );
    }

    #[test]
    fn every_name_the_mapping_can_give_resolves_to_a_palette_color() {
        let syntax = syntax();
        let mut names: Vec<&str> = paneflow_lsp::protocol::STANDARD_TOKEN_TYPES
            .iter()
            .chain(paneflow_lsp::protocol::RUST_ANALYZER_TOKEN_TYPES)
            .flat_map(|token_type| {
                [
                    capture_for(token_type, []),
                    capture_for(token_type, ["defaultLibrary"]),
                    capture_for(token_type, ["documentation"]),
                ]
            })
            .flatten()
            .collect();
        names.sort_unstable();
        names.dedup();
        let mut table: Vec<&str> = CAPTURES.to_vec();
        table.sort_unstable();
        assert_eq!(
            names, table,
            "CAPTURES lists exactly the names the mapping gives"
        );
        for name in CAPTURES {
            assert!(
                syntax.color_for_capture(name).is_some(),
                "{name} has no color"
            );
        }
    }

    #[test]
    fn a_legend_that_repeats_a_type_name_maps_each_index_independently() {
        let legend = TokenLegend {
            token_types: vec![
                "variable".into(),
                "variable".into(),
                "lifetime".into(),
                "function".into(),
            ],
            token_modifiers: vec![
                "defaultLibrary".into(),
                "documentation".into(),
                "defaultLibrary".into(),
            ],
        };
        let map = LegendMap::new(&legend);
        assert_eq!(map.capture(0, 0), Some("variable"));
        assert_eq!(map.capture(1, 0), Some("variable"));
        assert_eq!(map.capture(1, 0b100), Some("variable.builtin"));
        assert_eq!(map.capture(0, 0b001), Some("variable.builtin"));
        assert_eq!(map.capture(2, 0), None);
        assert_eq!(map.capture(3, u32::MAX), Some("function.builtin"));
        assert_eq!(map.capture(4, 0), None);
        assert_eq!(map.capture(u32::MAX, u32::MAX), None);
        let huge = TokenLegend {
            token_types: vec!["comment".into()],
            token_modifiers: (0..40).map(|_| "documentation".to_string()).collect(),
        };
        assert_eq!(
            LegendMap::new(&huge).capture(0, 1 << 31),
            Some("comment.doc")
        );
    }

    fn token(line: u32, start: u32, end: u32, token_type: u32) -> SemanticToken {
        SemanticToken {
            line,
            start,
            end,
            token_type,
            modifiers: 0,
        }
    }

    fn legend() -> TokenLegend {
        TokenLegend {
            token_types: vec!["property".into(), "variable".into(), "lifetime".into()],
            token_modifiers: Vec::new(),
        }
    }

    fn edit(start_row: usize, old_end_row: usize, new_end_row: usize) -> CodeEdit {
        let point = |row| DocPoint { row, column: 0 };
        CodeEdit {
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: 0,
            start_point: point(start_row),
            old_end_point: point(old_end_row),
            new_end_point: point(new_end_row),
        }
    }

    fn lines(layer: &SemanticLayer) -> Vec<(u32, u32)> {
        layer
            .tokens
            .iter()
            .map(|token| (token.line, token.start))
            .collect()
    }

    #[test]
    fn unmapped_tokens_take_no_storage_and_mapped_ones_paint_their_color() {
        let syntax = syntax();
        let layer = SemanticLayer::new(
            &[token(0, 0, 3, 0), token(0, 4, 6, 2), token(0, 7, 9, 1)],
            &legend(),
            &syntax,
        );
        assert_eq!(layer.tokens.len(), 2);
        let mut runs = LineRuns::new();
        layer.push_runs(0, &mut runs);
        assert_eq!(
            runs,
            vec![
                (0..3, syntax.color_for_capture("property").unwrap()),
                (7..9, syntax.color_for_capture("variable").unwrap()),
            ]
        );
        let mut none = LineRuns::new();
        layer.push_runs(1, &mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn an_edit_drops_the_tokens_of_its_rows_and_shifts_the_rows_after_it() {
        let tokens = [
            token(0, 0, 1, 0),
            token(2, 0, 1, 0),
            token(3, 0, 1, 1),
            token(5, 2, 3, 0),
        ];
        let mut layer = SemanticLayer::new(&tokens, &legend(), &syntax());
        layer.edit(&[edit(2, 2, 4)]);
        assert_eq!(lines(&layer), vec![(0, 0), (5, 0), (7, 2)]);
        layer.edit(&[edit(4, 6, 4)]);
        assert_eq!(lines(&layer), vec![(0, 0), (5, 2)]);
        layer.edit(&[edit(5, 5, 5), edit(0, 0, 0)]);
        assert!(lines(&layer).is_empty());
        assert!(!layer.has_tokens_on(0));
    }
}

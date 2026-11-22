use std::ops::Range;
use std::path::Path;

use gpui::Hsla;
use paneflow_highlights::{Language, LexDocument, LexError, language_for_path};

use super::highlighter::grammar_for_ext;
use super::syntax::DiffSyntax;

type LineRuns = Vec<(Range<usize>, Hsla)>;

pub(crate) fn lexer_only_language(path: &str, ext: &str) -> Option<Language> {
    if grammar_for_ext(ext).is_some() {
        return None;
    }
    language_for_path(Path::new(path))
}

pub(crate) fn lexical_lines(text: &str, language: Language, syntax: &DiffSyntax) -> Vec<LineRuns> {
    lexical_lines_of(text, LexDocument::lex(text, language), syntax)
}

fn lexical_lines_of(
    text: &str,
    lexed: Result<LexDocument, LexError>,
    syntax: &DiffSyntax,
) -> Vec<LineRuns> {
    let rows: Vec<Range<usize>> = text
        .lines()
        .map(|line| {
            let start = line.as_ptr() as usize - text.as_ptr() as usize;
            start..start + line.len()
        })
        .collect();
    let mut out: Vec<LineRuns> = vec![Vec::new(); rows.len()];
    let lexed = match lexed {
        Ok(lexed) => lexed,
        Err(err) => {
            tracing::debug!(
                target: "paneflow_app::diff::lexical",
                "a diff side stays plain: {err}"
            );
            return out;
        }
    };
    let colors = syntax.lexical_colors();
    let walked = lexed.for_each_line(0..lexed.line_count(), |line, _, tokens| {
        let start = lexed.line_start(line).unwrap_or(0);
        let row = rows
            .partition_point(|range| range.start <= start)
            .saturating_sub(1);
        let (Some(range), Some(runs)) = (rows.get(row), out.get_mut(row)) else {
            return;
        };
        let offset = start - range.start;
        for token in tokens {
            let Some(color) = colors.get(usize::from(token.id.get())).copied().flatten() else {
                continue;
            };
            let from = token.range.start + offset;
            let to = (token.range.end + offset).min(range.len());
            if from >= to {
                continue;
            }
            match runs.last_mut() {
                Some((last, previous)) if last.end == from && *previous == color => last.end = to,
                _ => runs.push((from..to, color)),
            }
        }
    });
    if let Err(err) = walked {
        tracing::debug!(
            target: "paneflow_app::diff::lexical",
            "a diff side stays plain: {err}"
        );
        return vec![Vec::new(); rows.len()];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::paneflow_dark;

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&paneflow_dark())
    }

    #[test]
    fn only_a_file_without_a_grammar_takes_the_lexical_path() {
        assert!(lexer_only_language("src/main.kt", "kt").is_some());
        assert!(lexer_only_language("init.lua", "lua").is_some());
        assert!(lexer_only_language("Dockerfile", "").is_some());
        assert!(lexer_only_language("src/main.rs", "rs").is_none());
        assert!(lexer_only_language("notes.unknownext", "unknownext").is_none());
    }

    #[test]
    fn every_line_of_a_crlf_or_lone_cr_side_lines_up_with_the_diff_rows() {
        let language = lexer_only_language("init.lua", "lua").expect("lua");
        let text = "local a = 1\r\nlocal b = \"x\"\rlocal c = 3\n";
        let lines = lexical_lines(text, language, &syntax());
        assert_eq!(lines.len(), text.lines().count());
        let first = text.lines().next().expect("a line");
        assert!(lines[0].iter().all(|(range, _)| range.end <= first.len()));
        assert!(!lines[0].is_empty() && !lines[1].is_empty());
        assert!(
            lines[1]
                .iter()
                .any(|(range, _)| range.start >= "local b = \"x\"\r".len()),
            "the lexer line after the lone CR lands on the same diff row: {:?}",
            lines[1]
        );
    }

    #[test]
    fn a_lexer_error_leaves_every_line_plain() {
        let text = "fun a() {}\nfun b() {}\n";
        let lines = lexical_lines_of(text, Err(LexError::Trap), &syntax());
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(Vec::is_empty));
    }
}

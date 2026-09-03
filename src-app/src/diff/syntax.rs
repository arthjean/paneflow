use gpui::Hsla;

use crate::theme::{SyntaxPalette, TerminalTheme};

fn cap_has(name: &str, p: &str) -> bool {
    name == p || (name.starts_with(p) && name.as_bytes().get(p.len()) == Some(&b'.'))
}

pub struct DiffSyntax {
    palette: SyntaxPalette,
}

impl DiffSyntax {
    pub fn from_theme(t: &TerminalTheme) -> Self {
        Self { palette: t.syntax }
    }

    pub fn color_for_capture(&self, name: &str) -> Option<Hsla> {
        let p = &self.palette;
        let c = if cap_has(name, "comment.doc") || cap_has(name, "comment.documentation") {
            p.comment_doc
        } else if cap_has(name, "comment") {
            p.comment
        } else if cap_has(name, "string.escape") || cap_has(name, "escape") {
            p.string_escape
        } else if cap_has(name, "string.special")
            || cap_has(name, "string.regex")
            || cap_has(name, "string.regexp")
        {
            p.string_special
        } else if cap_has(name, "string") || cap_has(name, "character") {
            p.string
        } else if cap_has(name, "text.literal")
            || cap_has(name, "markup.raw")
            || cap_has(name, "markup.code")
        {
            p.text_literal
        } else if cap_has(name, "text.title")
            || cap_has(name, "markup.heading")
            || cap_has(name, "title")
        {
            p.title
        } else if cap_has(name, "text.uri")
            || cap_has(name, "markup.link.url")
            || cap_has(name, "markup.link.uri")
            || cap_has(name, "link.uri")
            || cap_has(name, "uri")
        {
            p.link_uri
        } else if cap_has(name, "text.reference")
            || cap_has(name, "markup.link.label")
            || cap_has(name, "markup.link")
            || cap_has(name, "link")
        {
            p.link_text
        } else if cap_has(name, "text.strong")
            || cap_has(name, "markup.strong")
            || cap_has(name, "markup.bold")
            || cap_has(name, "emphasis.strong")
        {
            p.emphasis_strong
        } else if cap_has(name, "text.emphasis")
            || cap_has(name, "markup.italic")
            || cap_has(name, "markup.emphasis")
            || cap_has(name, "emphasis")
        {
            p.emphasis
        } else if cap_has(name, "boolean") {
            p.boolean
        } else if cap_has(name, "number") || cap_has(name, "float") {
            p.number
        } else if cap_has(name, "constant.builtin") {
            p.constant_builtin
        } else if cap_has(name, "constant") {
            p.constant
        } else if cap_has(name, "keyword")
            || cap_has(name, "storage")
            || cap_has(name, "conditional")
            || cap_has(name, "repeat")
            || cap_has(name, "include")
            || cap_has(name, "preproc")
            || cap_has(name, "define")
        {
            p.keyword
        } else if cap_has(name, "constructor") {
            p.constructor
        } else if cap_has(name, "enum") {
            p.r#enum
        } else if cap_has(name, "type") {
            p.r#type
        } else if cap_has(name, "function") || cap_has(name, "method") {
            p.function
        } else if cap_has(name, "attribute")
            || cap_has(name, "annotation")
            || cap_has(name, "decorator")
        {
            p.attribute
        } else if cap_has(name, "tag") {
            p.tag
        } else if cap_has(name, "property")
            || cap_has(name, "field")
            || cap_has(name, "variable.member")
        {
            p.property
        } else if cap_has(name, "label") {
            p.label
        } else if cap_has(name, "namespace") || cap_has(name, "module") {
            p.namespace
        } else if cap_has(name, "variable.builtin") {
            p.variable_builtin
        } else if cap_has(name, "variable") {
            p.variable
        } else if cap_has(name, "operator") {
            p.operator
        } else if cap_has(name, "punctuation.special")
            || cap_has(name, "punctuation.list_marker")
            || cap_has(name, "markup.list")
        {
            p.punctuation_special
        } else if cap_has(name, "punctuation") {
            p.punctuation
        } else {
            return None;
        };
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::paneflow_dark;

    #[test]
    fn keyword_and_string_map_to_distinct_palette_hues() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let kw = syn.color_for_capture("keyword.control").unwrap();
        let st = syn.color_for_capture("string").unwrap();
        assert_ne!(kw, st);
        assert_eq!(syn.color_for_capture("keyword.control.return"), Some(kw));
    }

    #[test]
    fn dotted_capture_falls_back_to_longest_prefix() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let func = syn.color_for_capture("function").unwrap();
        assert_eq!(syn.color_for_capture("function.method"), Some(func));
        assert_eq!(syn.color_for_capture("function.method.call"), Some(func));
    }

    #[test]
    fn operators_punctuation_variables_now_colored() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        assert!(syn.color_for_capture("operator").is_some());
        assert!(syn.color_for_capture("punctuation").is_some());
        assert!(syn.color_for_capture("punctuation.bracket").is_some());
        assert!(syn.color_for_capture("variable").is_some());
    }

    #[test]
    fn exact_builtin_arms_win_over_their_prefix() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let var = syn.color_for_capture("variable").unwrap();
        let var_builtin = syn.color_for_capture("variable.builtin").unwrap();
        assert_ne!(var, var_builtin);

        let constant = syn.color_for_capture("constant").unwrap();
        let constant_builtin = syn.color_for_capture("constant.builtin").unwrap();
        assert_ne!(constant, constant_builtin);

        let comment = syn.color_for_capture("comment").unwrap();
        let comment_doc = syn.color_for_capture("comment.doc").unwrap();
        assert_ne!(comment, comment_doc);
    }

    #[test]
    fn variable_member_resolves_to_property_not_variable() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let property = syn.color_for_capture("property").unwrap();
        let variable = syn.color_for_capture("variable").unwrap();
        assert_eq!(syn.color_for_capture("variable.member"), Some(property));
        assert_ne!(property, variable);
    }

    #[test]
    fn legacy_markdown_captures_map_to_palette_slots() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        for name in [
            "text.title",
            "text.literal",
            "text.uri",
            "text.reference",
            "punctuation.special",
        ] {
            assert!(
                syn.color_for_capture(name).is_some(),
                "expected markdown capture `{name}` to map to a palette slot"
            );
        }
    }

    #[test]
    fn unknown_capture_inherits_default_without_panic() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        assert_eq!(syn.color_for_capture("none"), None);
        assert_eq!(syn.color_for_capture("embedded"), None);
        assert_eq!(syn.color_for_capture("hint"), None);
        assert_eq!(syn.color_for_capture("text"), None);
    }
}

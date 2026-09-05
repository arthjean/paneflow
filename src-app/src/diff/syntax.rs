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
        } else if cap_has(name, "link_uri")
            || cap_has(name, "text.uri")
            || cap_has(name, "markup.link.url")
            || cap_has(name, "markup.link.uri")
            || cap_has(name, "link.uri")
            || cap_has(name, "uri")
        {
            p.link_uri
        } else if cap_has(name, "link_text")
            || cap_has(name, "text.reference")
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
        } else if cap_has(name, "constant")
            || cap_has(name, "selector.class")
            || cap_has(name, "selector.id")
        {
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
            || cap_has(name, "selector.pseudo")
        {
            p.attribute
        } else if cap_has(name, "tag") {
            p.tag
        } else if cap_has(name, "property")
            || cap_has(name, "field")
            || cap_has(name, "variable.member")
            || cap_has(name, "variable.parameter")
        {
            p.property
        } else if cap_has(name, "label") {
            p.label
        } else if cap_has(name, "namespace") || cap_has(name, "module") {
            p.namespace
        } else if cap_has(name, "variable.builtin") || cap_has(name, "variable.special") {
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

    #[test]
    fn every_vendored_capture_has_an_explicit_palette_role() {
        let mut palette = paneflow_dark().syntax;
        palette.comment = gpui::hsla(0.0 / 32.0, 0.5, 0.5, 1.0);
        palette.comment_doc = gpui::hsla(1.0 / 32.0, 0.5, 0.5, 1.0);
        palette.keyword = gpui::hsla(2.0 / 32.0, 0.5, 0.5, 1.0);
        palette.function = gpui::hsla(3.0 / 32.0, 0.5, 0.5, 1.0);
        palette.r#type = gpui::hsla(4.0 / 32.0, 0.5, 0.5, 1.0);
        palette.r#enum = gpui::hsla(5.0 / 32.0, 0.5, 0.5, 1.0);
        palette.constructor = gpui::hsla(6.0 / 32.0, 0.5, 0.5, 1.0);
        palette.string = gpui::hsla(7.0 / 32.0, 0.5, 0.5, 1.0);
        palette.string_escape = gpui::hsla(8.0 / 32.0, 0.5, 0.5, 1.0);
        palette.string_special = gpui::hsla(9.0 / 32.0, 0.5, 0.5, 1.0);
        palette.number = gpui::hsla(10.0 / 32.0, 0.5, 0.5, 1.0);
        palette.boolean = gpui::hsla(11.0 / 32.0, 0.5, 0.5, 1.0);
        palette.constant = gpui::hsla(12.0 / 32.0, 0.5, 0.5, 1.0);
        palette.constant_builtin = gpui::hsla(13.0 / 32.0, 0.5, 0.5, 1.0);
        palette.property = gpui::hsla(14.0 / 32.0, 0.5, 0.5, 1.0);
        palette.variable = gpui::hsla(15.0 / 32.0, 0.5, 0.5, 1.0);
        palette.variable_builtin = gpui::hsla(16.0 / 32.0, 0.5, 0.5, 1.0);
        palette.operator = gpui::hsla(17.0 / 32.0, 0.5, 0.5, 1.0);
        palette.punctuation = gpui::hsla(18.0 / 32.0, 0.5, 0.5, 1.0);
        palette.punctuation_special = gpui::hsla(19.0 / 32.0, 0.5, 0.5, 1.0);
        palette.attribute = gpui::hsla(20.0 / 32.0, 0.5, 0.5, 1.0);
        palette.tag = gpui::hsla(21.0 / 32.0, 0.5, 0.5, 1.0);
        palette.label = gpui::hsla(22.0 / 32.0, 0.5, 0.5, 1.0);
        palette.namespace = gpui::hsla(23.0 / 32.0, 0.5, 0.5, 1.0);
        palette.title = gpui::hsla(24.0 / 32.0, 0.5, 0.5, 1.0);
        palette.text_literal = gpui::hsla(25.0 / 32.0, 0.5, 0.5, 1.0);
        palette.link_uri = gpui::hsla(26.0 / 32.0, 0.5, 0.5, 1.0);
        palette.link_text = gpui::hsla(27.0 / 32.0, 0.5, 0.5, 1.0);
        palette.emphasis = gpui::hsla(28.0 / 32.0, 0.5, 0.5, 1.0);
        palette.emphasis_strong = gpui::hsla(29.0 / 32.0, 0.5, 0.5, 1.0);
        let expected = [
            ("comment", Some(palette.comment)),
            ("comment.doc", Some(palette.comment_doc)),
            ("attribute", Some(palette.attribute)),
            ("attribute.builtin", Some(palette.attribute)),
            ("attribute.jsx", Some(palette.attribute)),
            ("attribute.special", Some(palette.attribute)),
            ("selector.pseudo", Some(palette.attribute)),
            ("boolean", Some(palette.boolean)),
            ("constant", Some(palette.constant)),
            ("selector.class", Some(palette.constant)),
            ("selector.id", Some(palette.constant)),
            ("constant.builtin", Some(palette.constant_builtin)),
            ("constructor", Some(palette.constructor)),
            ("emphasis.markup", Some(palette.emphasis)),
            ("emphasis.strong.markup", Some(palette.emphasis_strong)),
            ("function", Some(palette.function)),
            ("function.builtin", Some(palette.function)),
            ("function.call", Some(palette.function)),
            ("function.decorator", Some(palette.function)),
            ("function.decorator.call", Some(palette.function)),
            ("function.definition", Some(palette.function)),
            ("function.kwargs", Some(palette.function)),
            ("function.method", Some(palette.function)),
            ("function.method.call", Some(palette.function)),
            ("function.method.constructor", Some(palette.function)),
            ("function.special", Some(palette.function)),
            ("function.special.definition", Some(palette.function)),
            ("keyword", Some(palette.keyword)),
            ("keyword.control", Some(palette.keyword)),
            ("keyword.declaration", Some(palette.keyword)),
            ("keyword.definition", Some(palette.keyword)),
            ("keyword.directive", Some(palette.keyword)),
            ("keyword.import", Some(palette.keyword)),
            ("keyword.operator", Some(palette.keyword)),
            ("keyword.operator.regex", Some(palette.keyword)),
            ("keyword.preproc", Some(palette.keyword)),
            ("preproc", Some(palette.keyword)),
            ("label", Some(palette.label)),
            ("link_text.markup", Some(palette.link_text)),
            ("link_uri.markup", Some(palette.link_uri)),
            ("namespace", Some(palette.namespace)),
            ("module", Some(palette.namespace)),
            ("number", Some(palette.number)),
            ("operator", Some(palette.operator)),
            ("operator.spaceship", Some(palette.operator)),
            ("property", Some(palette.property)),
            ("property.json_key", Some(palette.property)),
            ("property.name", Some(palette.property)),
            ("variable.parameter", Some(palette.property)),
            ("punctuation.bracket", Some(palette.punctuation)),
            ("punctuation.bracket.jsx", Some(palette.punctuation)),
            ("punctuation.delimiter", Some(palette.punctuation)),
            ("punctuation.delimiter.jsx", Some(palette.punctuation)),
            ("punctuation.embedded.markup", Some(palette.punctuation)),
            ("punctuation.markup", Some(palette.punctuation)),
            (
                "punctuation.list_marker.markup",
                Some(palette.punctuation_special),
            ),
            ("punctuation.special", Some(palette.punctuation_special)),
            ("string", Some(palette.string)),
            ("string.doc", Some(palette.string)),
            ("string.escape", Some(palette.string_escape)),
            ("string.regex", Some(palette.string_special)),
            ("string.special", Some(palette.string_special)),
            ("tag", Some(palette.tag)),
            ("tag.component.jsx", Some(palette.tag)),
            ("tag.jsx", Some(palette.tag)),
            ("text.literal.markup", Some(palette.text_literal)),
            ("title.markup", Some(palette.title)),
            ("type", Some(palette.r#type)),
            ("type.builtin", Some(palette.r#type)),
            ("type.class", Some(palette.r#type)),
            ("type.class.builtin", Some(palette.r#type)),
            ("type.class.call", Some(palette.r#type)),
            ("type.class.definition", Some(palette.r#type)),
            ("type.class.inheritance", Some(palette.r#type)),
            ("type.definition", Some(palette.r#type)),
            ("type.interface", Some(palette.r#type)),
            ("type.name", Some(palette.r#type)),
            ("type.unit", Some(palette.r#type)),
            ("variable", Some(palette.variable)),
            ("variable.special", Some(palette.variable_builtin)),
            ("variable.builtin", Some(palette.variable_builtin)),
            ("strikethrough.markup", None),
            ("embedded", None),
            ("text", None),
            ("text.jsx", None),
            ("none", None),
            ("nested", None),
            ("lifetime", None),
            ("_isinstance", None),
            ("_issubclass", None),
            ("_parent", None),
            ("concept", None),
        ];
        let syntax = DiffSyntax { palette };
        for (name, color) in expected {
            assert_eq!(syntax.color_for_capture(name), color, "{name}");
        }
        let mut grammars: Vec<_> = [
            "rs", "json", "jsonc", "sh", "py", "ts", "tsx", "js", "md", "go", "yaml", "css", "c",
            "cpp",
        ]
        .into_iter()
        .map(|ext| crate::diff::grammar_for_ext(ext).unwrap())
        .collect();
        grammars.push(crate::diff::markdown_inline_grammar().unwrap());
        for grammar in grammars {
            for name in grammar.query.capture_names() {
                assert!(
                    expected.iter().any(|(known, _)| known == name),
                    "unlisted capture {name}"
                );
            }
        }
    }

    fn assert_neutral_roles(theme: TerminalTheme) {
        let ui = crate::theme::ui_colors_with(&theme);
        let palette = theme.syntax;
        assert_eq!(palette.variable, ui.text);
        assert_eq!(palette.namespace, ui.text);
        assert!(palette.punctuation == ui.text || palette.punctuation == ui.muted);
        assert_eq!(palette.constructor, palette.function);
        assert_ne!(palette.variable_builtin, palette.variable);
        assert_ne!(palette.operator, ui.text);
        let syntax = DiffSyntax::from_theme(&theme);
        assert_eq!(
            syntax.color_for_capture("variable.special"),
            Some(palette.variable_builtin)
        );
        assert_eq!(
            syntax.color_for_capture("variable.parameter"),
            Some(palette.property)
        );
    }

    #[test]
    fn catppuccin_mocha_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::paneflow_dark());
    }
    #[test]
    fn catppuccin_latte_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::paneflow_light());
    }
    #[test]
    fn vercel_dark_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::vercel_dark());
    }
    #[test]
    fn vercel_light_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::vercel_light());
    }
    #[test]
    fn claude_dark_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::claude_dark());
    }
    #[test]
    fn claude_light_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::claude_light());
    }
    #[test]
    fn cursor_dark_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::cursor_dark());
    }
    #[test]
    fn cursor_light_uses_neutral_roles() {
        assert_neutral_roles(crate::theme::cursor_light());
    }
}

//! Capture-name → color mapping for diff syntax highlighting.
//!
//! The tree-sitter parse + query lives in [`super::highlighter`]; this module
//! is just the theme binding: it maps a tree-sitter highlight capture name
//! (`keyword.control`, `string`, `type.builtin`, …) to a color drawn from the
//! theme's dedicated [`SyntaxPalette`] (prd-diff-syntax-palette-2026-Q3.md,
//! EP-001) - a ≈30-slot semantic palette in Paneflow's Catppuccin brand, NOT
//! the 8-hue ANSI terminal set. Resolution is longest-prefix-wins (Zed's
//! `SyntaxTheme` model): a dotted capture like `function.method.call` falls
//! back to the `function` slot, with exact-name arms (`variable.builtin`,
//! `constant.builtin`, `comment.doc`) checked before their prefixes.
//!
//! Only genuinely-unmapped captures (`@none`, `@embedded`, `@hint`, plain
//! `@text`) return `None` and inherit the row foreground; operators,
//! punctuation, and plain variables are now first-class colored slots.

use gpui::Hsla;

use crate::theme::{SyntaxPalette, TerminalTheme};

/// `name == p` or `name` starts with `p.` (a dotted sub-capture). Lets a prefix
/// like `keyword` cover `keyword.control.return` without matching `keywordx`.
fn cap_has(name: &str, p: &str) -> bool {
    name == p || (name.starts_with(p) && name.as_bytes().get(p.len()) == Some(&b'.'))
}

/// A diff syntax color map bound to one theme snapshot. Cheap to build (copies
/// the `Copy` palette); rebuilt per diff load so theme hot-reload is picked up.
pub struct DiffSyntax {
    palette: SyntaxPalette,
}

impl DiffSyntax {
    pub fn from_theme(t: &TerminalTheme) -> Self {
        Self { palette: t.syntax }
    }

    /// Map a tree-sitter highlight capture name to a [`SyntaxPalette`] color,
    /// or `None` if it is unmapped and inherits the default foreground. Arms
    /// are ordered most-specific-first so dotted captures resolve to their
    /// longest matching prefix (FR-02).
    pub fn color_for_capture(&self, name: &str) -> Option<Hsla> {
        let p = &self.palette;
        let c = if cap_has(name, "comment.doc") || cap_has(name, "comment.documentation") {
            p.comment_doc
        } else if cap_has(name, "comment") {
            p.comment
        }
        // Strings - escape / special checked before the generic `string` prefix
        // (which would otherwise swallow `string.escape` / `string.special`).
        else if cap_has(name, "string.escape") || cap_has(name, "escape") {
            p.string_escape
        } else if cap_has(name, "string.special")
            || cap_has(name, "string.regex")
            || cap_has(name, "string.regexp")
        {
            p.string_special
        } else if cap_has(name, "string") || cap_has(name, "character") {
            p.string
        }
        // Markdown / markup - legacy `text.*` names (tree-sitter-md) and modern
        // `markup.*` names both map here (US-004).
        else if cap_has(name, "text.literal")
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
        }
        // Numbers / booleans.
        else if cap_has(name, "boolean") {
            p.boolean
        } else if cap_has(name, "number") || cap_has(name, "float") {
            p.number
        }
        // Constants - builtin before the generic prefix.
        else if cap_has(name, "constant.builtin") {
            p.constant_builtin
        } else if cap_has(name, "constant") {
            p.constant
        }
        // Keywords (+ storage / control-flow / preprocessor families).
        else if cap_has(name, "keyword")
            || cap_has(name, "storage")
            || cap_has(name, "conditional")
            || cap_has(name, "repeat")
            || cap_has(name, "include")
            || cap_has(name, "preproc")
            || cap_has(name, "define")
        {
            p.keyword
        }
        // Types / enums / constructors.
        else if cap_has(name, "constructor") {
            p.constructor
        } else if cap_has(name, "enum") {
            p.r#enum
        } else if cap_has(name, "type") {
            p.r#type
        }
        // Functions / methods.
        else if cap_has(name, "function") || cap_has(name, "method") {
            p.function
        }
        // Attributes / annotations / decorators.
        else if cap_has(name, "attribute")
            || cap_has(name, "annotation")
            || cap_has(name, "decorator")
        {
            p.attribute
        }
        // HTML / JSX tags.
        else if cap_has(name, "tag") {
            p.tag
        }
        // Object/struct fields & properties (`variable.member` is the modern
        // capture for a struct field - check it before the `variable` prefix).
        else if cap_has(name, "property")
            || cap_has(name, "field")
            || cap_has(name, "variable.member")
        {
            p.property
        }
        // Labels (loop labels, goto targets, YAML/JSON keys via `label`).
        else if cap_has(name, "label") {
            p.label
        }
        // Namespaces / modules.
        else if cap_has(name, "namespace") || cap_has(name, "module") {
            p.namespace
        }
        // Variables - builtin (`self` / `this` / `super`) before the prefix.
        else if cap_has(name, "variable.builtin") {
            p.variable_builtin
        } else if cap_has(name, "variable") {
            p.variable
        }
        // Operators.
        else if cap_has(name, "operator") {
            p.operator
        }
        // Punctuation - special / list markers before the generic prefix.
        else if cap_has(name, "punctuation.special")
            || cap_has(name, "punctuation.list_marker")
            || cap_has(name, "markup.list")
        {
            p.punctuation_special
        } else if cap_has(name, "punctuation") {
            p.punctuation
        } else {
            // `@none`, `@embedded`, `@hint`, plain `@text`, … → inherit fg.
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
        // Sub-captures fall back to their prefix category (longest-prefix-wins).
        assert_eq!(syn.color_for_capture("keyword.control.return"), Some(kw));
    }

    #[test]
    fn dotted_capture_falls_back_to_longest_prefix() {
        // US-002 AC #2: `function.method.call` → the `function` slot.
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let func = syn.color_for_capture("function").unwrap();
        assert_eq!(syn.color_for_capture("function.method"), Some(func));
        assert_eq!(syn.color_for_capture("function.method.call"), Some(func));
    }

    #[test]
    fn operators_punctuation_variables_now_colored() {
        // US-002 AC #3: previously these returned `None`; now each is a slot.
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        assert!(syn.color_for_capture("operator").is_some());
        assert!(syn.color_for_capture("punctuation").is_some());
        assert!(syn.color_for_capture("punctuation.bracket").is_some());
        assert!(syn.color_for_capture("variable").is_some());
    }

    #[test]
    fn exact_builtin_arms_win_over_their_prefix() {
        // US-002 AC #2: `variable.builtin` / `constant.builtin` / `comment.doc`
        // resolve to their dedicated slot, distinct from the prefix slot.
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
        // The modern struct-field capture must not be swallowed by the
        // `variable` prefix arm.
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let property = syn.color_for_capture("property").unwrap();
        let variable = syn.color_for_capture("variable").unwrap();
        assert_eq!(syn.color_for_capture("variable.member"), Some(property));
        assert_ne!(property, variable);
    }

    #[test]
    fn legacy_markdown_captures_map_to_palette_slots() {
        // US-004 AC #1: tree-sitter-md's legacy capture names each resolve.
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
        // US-002 AC #5 (unhappy path): a capture matched by no arm returns
        // `None` (inherits row fg) rather than panicking or mis-coloring.
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        assert_eq!(syn.color_for_capture("none"), None);
        assert_eq!(syn.color_for_capture("embedded"), None);
        assert_eq!(syn.color_for_capture("hint"), None);
        assert_eq!(syn.color_for_capture("text"), None);
    }
}

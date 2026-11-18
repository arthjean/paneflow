use gpui::{
    AnyElement, AvailableSpace, IntoElement, ParentElement, Pixels, Styled, StyledText,
    VisualTestContext, div, point, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

const NOOP_ADVANCE_EM: f32 = 0.6;
const FIT_SLACK: Pixels = px(0.1);

pub(crate) fn noop_width(text: &str, font_size: Pixels) -> Pixels {
    let units: usize = text.chars().map(char::len_utf16).sum();
    font_size * (NOOP_ADVANCE_EM * units as f32) + FIT_SLACK
}

pub(crate) fn wrapped_lines(
    cx: &mut VisualTestContext,
    text: StyledText,
    width: Pixels,
    container: impl FnOnce(StyledText) -> AnyElement,
) -> Vec<String> {
    let layout = text.layout().clone();
    let element = div().w(width).child(container(text)).into_any_element();
    cx.draw(
        point(px(0.), px(0.)),
        size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
        move |_, _| element,
    );
    layout
        .wrapped_text()
        .split('\n')
        .map(str::to_owned)
        .collect()
}

pub(crate) fn assert_breaks_on_grapheme_boundaries(text: &str, lines: &[String]) {
    assert_eq!(lines.concat(), text, "wrapping must only insert breaks");
    let boundaries: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut offset = 0usize;
    for line in lines {
        assert!(
            boundaries.contains(&offset),
            "a break at byte {offset} splits a grapheme cluster: {lines:?}"
        );
        offset += line.len();
    }
}

pub(crate) fn assert_no_line_opens_with(lines: &[String], marks: &[char]) {
    for line in lines.iter().skip(1) {
        assert!(
            !line.starts_with(marks),
            "a break fell before one of {marks:?}: {lines:?}"
        );
    }
}

struct Case {
    text: &'static str,
    fits: &'static str,
    first_line: &'static str,
}

const WRAP_CASES: &[Case] = &[
    Case {
        text: "on feat/gpui-upgrade now",
        fits: "on feat/gp",
        first_line: "on feat/",
    },
    Case {
        text: "on feat/gpui-upgrade now",
        fits: "on feat/gpui-up",
        first_line: "on feat/gpui-",
    },
    Case {
        text: "in C:\\dev\\paneflow\\src-app now",
        fits: "in C:\\dev\\paneflow\\src-a",
        first_line: "in C:\\dev\\paneflow\\src-",
    },
    Case {
        text: "in C:\\dev\\paneflow\\src-app now",
        fits: "in C:\\dev\\pan",
        first_line: "in C:",
    },
    Case {
        text: "see https://example.com/a/b?x=1 now",
        fits: "see https://example.com/a/b",
        first_line: "see https://example.com/a/",
    },
    Case {
        text: "a well-known path",
        fits: "a well-kn",
        first_line: "a well-",
    },
];

pub(crate) fn assert_wrap_cases(
    cx: &mut VisualTestContext,
    font_size: Pixels,
    pad_x: Pixels,
    make_text: impl Fn(&'static str) -> StyledText,
    container: impl Fn(StyledText) -> AnyElement,
) {
    for case in WRAP_CASES {
        let width = noop_width(case.fits, font_size) + pad_x;
        let lines = wrapped_lines(cx, make_text(case.text), width, &container);
        assert_eq!(
            lines.first().map(String::as_str),
            Some(case.first_line),
            "{:?} at {width:?}: {lines:?}",
            case.text
        );
        assert_eq!(lines.concat(), case.text);
        assert_no_line_opens_with(&lines, &['/', '?', '-']);
    }
}

#[cfg(test)]
mod tests {
    use gpui::{SharedString, TestAppContext};

    use super::*;
    use crate::ui_primitives::{LABEL_SM, single_line_label, tooltip_shell};

    const TOOLTIP_TEXT: Pixels = px(14.);
    const TOOLTIP_PAD_X: Pixels = px(16.);

    #[gpui::test]
    fn a_tooltip_breaks_paths_branches_and_urls_after_their_separators(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        assert_wrap_cases(cx, TOOLTIP_TEXT, TOOLTIP_PAD_X, StyledText::new, |text| {
            tooltip_shell().child(text).into_any_element()
        });
    }

    #[gpui::test]
    fn a_settings_description_breaks_paths_branches_and_urls_after_their_separators(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let ui = crate::theme::ui_colors();
        assert_wrap_cases(
            cx,
            LABEL_SM,
            px(0.),
            |text| crate::ui_primitives::highlight_matches(text.to_string(), ""),
            |text| crate::settings::components::setting_description(ui, text).into_any_element(),
        );
    }

    #[gpui::test]
    fn a_row_label_truncates_on_one_line_instead_of_wrapping(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let title = "a branch named feat/gpui-upgrade with a long description";
        let lines = wrapped_lines(
            cx,
            StyledText::new(SharedString::from(title)),
            px(120.),
            |text| {
                div()
                    .flex()
                    .text_size(px(12.))
                    .child(single_line_label().child(text))
                    .into_any_element()
            },
        );
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].ends_with('…'), "{lines:?}");
        assert!(lines[0].len() < title.len(), "{lines:?}");
    }

    #[gpui::test]
    fn a_token_without_break_opportunity_breaks_between_grapheme_clusters(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let unit = "e\u{301}👍🏽🇯🇵👩\u{200d}💻ab";
        let mut token = String::new();
        while token.chars().count() < 200 {
            token.push_str(unit);
        }
        for columns in [7usize, 11, 16, 23] {
            let width = TOOLTIP_TEXT * (NOOP_ADVANCE_EM * columns as f32) + TOOLTIP_PAD_X;
            let lines = wrapped_lines(cx, StyledText::new(token.clone()), width, |text| {
                tooltip_shell().child(text).into_any_element()
            });
            assert!(lines.len() > 1, "the probe must force a break: {lines:?}");
            assert_breaks_on_grapheme_boundaries(&token, &lines);
        }
    }

    #[gpui::test]
    fn closing_cjk_punctuation_stays_with_the_character_before_it(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        for (text, fits, first, second) in [
            ("aaa 你好！", "aaa 你好", "aaa 你", "好！"),
            ("aaa こんにちは。", "aaa こんにちは", "aaa こんにち", "は。"),
            ("aaa 「引用」です", "aaa 「引用", "aaa 「引", "用」です"),
        ] {
            let width = noop_width(fits, TOOLTIP_TEXT) + TOOLTIP_PAD_X;
            let lines = wrapped_lines(cx, StyledText::new(text), width, |text| {
                tooltip_shell().child(text).into_any_element()
            });
            assert_eq!(lines.first().map(String::as_str), Some(first), "{lines:?}");
            assert_eq!(lines.get(1).map(String::as_str), Some(second), "{lines:?}");
            assert_no_line_opens_with(&lines, &['！', '。', '」']);
        }
    }
}

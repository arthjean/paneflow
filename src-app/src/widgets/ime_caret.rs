use std::cell::Cell;

use gpui::{App, Bounds, Entity, EntityInputHandler, FocusHandle, Pixels, Window};

#[derive(Default)]
pub(crate) struct ImeCaretReport(Cell<Option<Bounds<Pixels>>>);

impl ImeCaretReport {
    pub(crate) fn follow(&self, caret: Option<Bounds<Pixels>>) -> bool {
        let moved = caret.is_some() && caret != self.0.get();
        self.0.set(caret);
        moved
    }

    #[cfg(test)]
    pub(crate) fn reported(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }
}

pub(crate) fn follow_ime_caret<V: EntityInputHandler>(
    view: &Entity<V>,
    report: impl Fn(&V) -> &ImeCaretReport,
    focus: &FocusHandle,
    element_bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let focused = focus.is_focused(window);
    let moved = view.update(cx, |view, cx| {
        let caret = if focused {
            view.selected_text_range(false, window, cx)
                .and_then(|selection| {
                    view.bounds_for_range(selection.range, element_bounds, window, cx)
                })
        } else {
            None
        };
        report(view).follow(caret)
    });
    if moved {
        window.invalidate_character_coordinates();
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    fn caret(x: f32) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(point(px(x), px(20.)), size(px(8.), px(18.))))
    }

    #[test]
    fn a_caret_is_reported_once_per_position_and_again_after_focus_returns() {
        let report = ImeCaretReport::default();
        assert!(report.follow(caret(40.)), "the first caret is reported");
        assert!(
            !report.follow(caret(40.)),
            "an unchanged caret asks for no further frame"
        );
        assert!(report.follow(caret(96.)), "a moved caret is reported");
        assert!(!report.follow(None), "losing focus reports nothing");
        assert_eq!(report.reported(), None);
        assert!(
            report.follow(caret(96.)),
            "the same caret is reported again once focus returns"
        );
    }
}

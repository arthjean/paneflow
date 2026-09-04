use std::ops::Range;

use gpui::{
    AnyElement, App, Styled, UniformList, UniformListScrollHandle, Window, px, uniform_list,
};

use crate::app::sidebar::SIDEBAR_ROW_MARGIN_X;

pub(super) fn files_list(
    row_count: usize,
    scroll: &UniformListScrollHandle,
    render_rows: impl Fn(Range<usize>, &mut Window, &mut App) -> Vec<AnyElement> + 'static,
) -> UniformList {
    uniform_list("files-sidebar-body", row_count, render_rows)
        .flex_1()
        .min_h_0()
        .w_full()
        .px(px(SIDEBAR_ROW_MARGIN_X))
        .py(px(4.))
        .track_scroll(scroll)
}

#[cfg(test)]
mod tests;

use super::*;

pub(crate) const SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_X: f32 = 7.0;
pub(crate) const SIDEBAR_ROW_PADDING_Y: f32 = 6.0;
pub(super) const SIDEBAR_ROW_GAP: f32 = 3.0;
pub(crate) const SIDEBAR_ROW_LINE_HEIGHT: f32 = 20.0;
pub(crate) const SIDEBAR_TITLE_ROW_GAP: f32 = 8.0;
pub(super) const SIDEBAR_BRANCH_GAP: f32 = 6.0;
const TITLE_LINE_SNAP_SLACK: f32 = 1.0;
pub(super) const SIDEBAR_TRAILING_MAX_SHARE: f32 = 0.5;
const SIDEBAR_ROW_MIN_HEIGHT: f32 = 32.0;
pub(crate) const SIDEBAR_FOLDER_SLOT_WIDTH: f32 = 20.0;
pub(super) use crate::app::constants::rail_row_radius;
pub(super) const SIDEBAR_ACTION_BUTTON_SIZE: f32 = 22.0;
pub(super) const SIDEBAR_ACTION_BUTTON_GAP: f32 = 1.0;
pub(super) const SIDEBAR_LANE_GLYPH_SIZE: f32 = 11.0;
const GEIST_ASCENT_EM: f32 = 1.005;
const GEIST_DESCENT_EM: f32 = 0.295;
pub(crate) const GEIST_X_HEIGHT_EM: f32 = 0.532;
pub(crate) const GEIST_CAP_HEIGHT_EM: f32 = 0.710;
pub(super) const SIDEBAR_ROW_SPACING: f32 = 2.0;
pub(super) const SIDEBAR_GROUP_SPACING: f32 = SIDEBAR_ROW_SPACING + SIDEBAR_TITLE_ROW_GAP;
pub(super) const SIDEBAR_HEADER_ICON_WIDTH: f32 = 15.0;
pub(super) const SIDEBAR_WORKSPACE_FOLDER_ICON_WIDTH: f32 = 15.0;
pub(super) const SIDEBAR_TITLE_TOOLTIP_MIN_CHARS: usize = 13;

pub(super) fn sidebar_workspace_row_content_width(sidebar_width: f32) -> f32 {
    sidebar_width - SIDEBAR_ROW_MARGIN_X * 2.0 - SIDEBAR_ROW_PADDING_X * 2.0
}

pub(super) fn sidebar_title_row(
    content_width: f32,
    main: gpui::Div,
    trailing: gpui::Div,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w(px(content_width))
        .max_w(px(content_width))
        .min_w_0()
        .overflow_x_hidden()
        .child(
            main.flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(SIDEBAR_TITLE_ROW_GAP)),
        )
        .child(trailing)
}

pub(super) fn sidebar_row_trailing(
    revealed: impl IntoElement,
    lane_slot: Option<AnyElement>,
    reveal: f32,
) -> gpui::Div {
    let lead = if lane_slot.is_some() { 1. } else { reveal };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .pl(px(SIDEBAR_TITLE_ROW_GAP * lead))
        .child(revealed)
        .children(lane_slot)
}

pub(super) fn sidebar_title_with_trailing(
    title: impl IntoElement,
    trailing: Option<impl IntoElement>,
    gap: f32,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_initial()
        .min_w_0()
        .overflow_x_hidden()
        .child(TitleLine {
            title: title_line_slot(title),
            trailing: trailing.map(title_line_slot),
            gap: px(gap),
        })
}

fn title_line_slot(content: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_w_0()
        .max_w_full()
        .overflow_x_hidden()
        .child(content)
        .into_any_element()
}

pub(super) fn title_line_widths(
    available: gpui::Pixels,
    title: gpui::Pixels,
    trailing: Option<gpui::Pixels>,
    gap: gpui::Pixels,
) -> (gpui::Pixels, Option<gpui::Pixels>) {
    let slack = px(TITLE_LINE_SNAP_SLACK);
    let Some(trailing) = trailing else {
        let width = if available + slack >= title {
            title
        } else {
            available
        };
        return (width, None);
    };
    if available + slack >= title + gap + trailing {
        return (title, Some(trailing));
    }
    let left_by_title = available - gap - title;
    let trailing_width = trailing
        .min((available * SIDEBAR_TRAILING_MAX_SHARE).max(left_by_title))
        .max(px(0.));
    let title_width = title.min(available - gap - trailing_width).max(px(0.));
    (title_width, Some(trailing_width))
}

struct TitleLine {
    title: AnyElement,
    trailing: Option<AnyElement>,
    gap: gpui::Pixels,
}

struct TitleLineNaturalSizes {
    title: gpui::Size<gpui::Pixels>,
    trailing: Option<gpui::Size<gpui::Pixels>>,
}

fn title_line_space(width: gpui::AvailableSpace) -> gpui::Size<gpui::AvailableSpace> {
    gpui::size(width, gpui::AvailableSpace::MinContent)
}

impl gpui::Element for TitleLine {
    type RequestLayoutState = TitleLineNaturalSizes;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, TitleLineNaturalSizes) {
        let unbounded = title_line_space(gpui::AvailableSpace::MaxContent);
        let title = self.title.layout_as_root(unbounded, window, cx);
        let trailing = self
            .trailing
            .as_mut()
            .map(|trailing| trailing.layout_as_root(unbounded, window, cx));
        let natural_width = title.width + trailing.map_or(px(0.), |size| self.gap + size.width);
        let natural_height = trailing.map_or(title.height, |size| size.height.max(title.height));
        let style = gpui::Style {
            flex_shrink: 1.,
            min_size: gpui::size(px(0.).into(), gpui::Length::Auto),
            ..Default::default()
        };
        let layout_id = window.request_measured_layout(style, move |known, _, _, _| {
            gpui::size(
                known.width.unwrap_or(natural_width),
                known.height.unwrap_or(natural_height),
            )
        });
        (layout_id, TitleLineNaturalSizes { title, trailing })
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        natural: &mut TitleLineNaturalSizes,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        let (title_width, trailing_width) = title_line_widths(
            bounds.size.width,
            natural.title.width,
            natural.trailing.map(|size| size.width),
            self.gap,
        );
        let mut place = |slot: &mut AnyElement, natural: gpui::Size<gpui::Pixels>, width, left| {
            let size = if width < natural.width {
                slot.layout_as_root(
                    title_line_space(gpui::AvailableSpace::Definite(width)),
                    window,
                    cx,
                )
            } else {
                natural
            };
            let top = bounds.origin.y + (bounds.size.height - size.height) / 2.;
            slot.prepaint_at(gpui::point(left, top), window, cx);
        };
        place(&mut self.title, natural.title, title_width, bounds.origin.x);
        if let (Some(trailing), Some(natural), Some(width)) =
            (self.trailing.as_mut(), natural.trailing, trailing_width)
        {
            place(
                trailing,
                natural,
                width,
                bounds.origin.x + title_width + self.gap,
            );
        }
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut TitleLineNaturalSizes,
        _: &mut (),
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        self.title.paint(window, cx);
        if let Some(trailing) = self.trailing.as_mut() {
            trailing.paint(window, cx);
        }
    }
}

pub(super) fn sidebar_row_shell() -> gpui::Div {
    div()
        .px(px(SIDEBAR_ROW_PADDING_X))
        .py(px(SIDEBAR_ROW_PADDING_Y))
        .min_h(px(SIDEBAR_ROW_MIN_HEIGHT))
        .justify_center()
        .flex_none()
        .relative()
        .overflow_x_hidden()
        .flex()
        .flex_col()
        .gap(px(SIDEBAR_ROW_GAP))
}

pub(super) fn render_sidebar_indent_guide(ui: crate::theme::UiColors) -> gpui::Div {
    let color = ui.text.opacity(0.08);
    let left = px(SIDEBAR_ROW_PADDING_X + (SIDEBAR_FOLDER_SLOT_WIDTH / 2.).floor());
    div()
        .absolute()
        .left(left)
        .top(px(-SIDEBAR_ROW_SPACING))
        .bottom_0()
        .w(px(1.))
        .bg(color)
}

pub(super) fn sidebar_cursor_ring(ui: crate::theme::UiColors) -> impl IntoElement {
    crate::ui_primitives::squircle::squircle_border(
        rail_row_radius(),
        px(1.),
        ui.text.opacity(0.35),
    )
}

pub(super) fn sidebar_action_button(
    id: SharedString,
    icon: &'static str,
    icon_size: f32,
    label: SharedString,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    let active_bg = crate::app::constants::sidebar_tab_active_background();
    let group = SharedString::from(format!("{id}-hover"));
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .text_color(ui.muted),
        group.clone(),
        px(6.),
        None,
        Some(active_bg),
    )
    .role(Role::Button)
    .aria_label(label.clone())
    .delayed_tooltip(crate::ui_primitives::text_tooltip(label))
    .child(
        svg()
            .size(px(icon_size))
            .flex_none()
            .path(icon)
            .text_color(ui.muted)
            .group_hover(group, move |style| style.text_color(ui.text)),
    )
}

pub(crate) fn sidebar_text_ink_axis(font_size: f32, ink_height_em: f32) -> f32 {
    SIDEBAR_ROW_LINE_HEIGHT / 2.
        + font_size * (GEIST_ASCENT_EM - GEIST_DESCENT_EM - ink_height_em) / 2.
}

pub(crate) fn sidebar_glyph_top(glyph_size: f32) -> f32 {
    ((SIDEBAR_ROW_LINE_HEIGHT - glyph_size) / 2.).ceil()
}

pub(super) fn sidebar_lane_glyph_top() -> f32 {
    sidebar_glyph_top(SIDEBAR_LANE_GLYPH_SIZE)
}

pub(super) fn sidebar_lane_glyph_center() -> f32 {
    sidebar_lane_glyph_top() + SIDEBAR_LANE_GLYPH_SIZE / 2.
}

pub(super) fn sidebar_hover_revealed_glyph_button(
    id: SharedString,
    icon: &'static str,
    label: SharedString,
    reveal: f32,
    trailing_space: f32,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("{id}-hover"));
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .w(px((SIDEBAR_LANE_GLYPH_SIZE + trailing_space) * reveal))
        .h(px(SIDEBAR_ROW_LINE_HEIGHT))
        .overflow_hidden()
        .opacity(reveal)
        .flex()
        .items_start()
        .justify_start()
        .role(Role::Button)
        .aria_label(label.clone())
        .delayed_tooltip(crate::ui_primitives::text_tooltip(label))
        .child(
            svg()
                .size(px(SIDEBAR_LANE_GLYPH_SIZE))
                .mt(px(sidebar_lane_glyph_top()))
                .flex_none()
                .path(icon)
                .text_color(ui.muted)
                .group_hover(group, move |style| style.text_color(ui.text)),
        )
}

pub(super) fn tabular_numerals() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

pub(super) fn render_diffstat_counts(
    stats: &crate::workspace::GitDiffStats,
    ui: crate::theme::UiColors,
) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .text_size(crate::ui_primitives::BODY)
        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
        .font_features(tabular_numerals())
        .child(
            div()
                .text_color(ui.vc_added)
                .child(format!("+{}", stats.insertions)),
        )
        .child(
            div()
                .text_color(ui.vc_deleted)
                .child(format!("\u{2212}{}", stats.deletions)),
        )
}

impl PaneFlowApp {
    pub(super) fn inline_rename_field(&self, ui: crate::theme::UiColors) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .text_color(ui.text)
            .text_size(px(14.))
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .bg(ui.overlay)
            .px_1()
            .rounded_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .debug_selector(|| "tab-rename-field".to_string())
            .child(self.rename_input.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{
        AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
        size,
    };

    #[gpui::test]
    fn row_glyphs_share_the_row_vertical_center(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                sidebar_row_shell()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(SIDEBAR_FOLDER_SLOT_WIDTH))
                                    .debug_selector(|| "folder".into()),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                    .debug_selector(|| "title".into())
                                    .child("paneflow"),
                            )
                            .child(
                                div().flex_none().h(px(SIDEBAR_ROW_LINE_HEIGHT)).child(
                                    div()
                                        .size(px(SIDEBAR_LANE_GLYPH_SIZE))
                                        .mt(px(sidebar_lane_glyph_top()))
                                        .debug_selector(|| "action".into()),
                                ),
                            ),
                    )
                    .debug_selector(|| "row".into())
            },
        );

        let row = cx.debug_bounds("row").expect("row not painted");
        for selector in ["folder", "title"] {
            let glyph = cx.debug_bounds(selector).expect("glyph not painted");
            assert_eq!(
                glyph.center().y,
                row.center().y,
                "{selector} is off the row's vertical center"
            );
        }
        let action = cx.debug_bounds("action").expect("action not painted");
        assert_eq!(
            action.center().y,
            row.center().y + px(0.5),
            "an odd-sized action glyph takes the whole pixel just below the row's center"
        );
    }

    #[gpui::test]
    fn a_single_line_row_is_thirty_two_pixels_tall(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                sidebar_row_shell()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(div().flex_none().size(px(SIDEBAR_FOLDER_SLOT_WIDTH)))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                    .child("paneflow"),
                            ),
                    )
                    .debug_selector(|| "row".into())
            },
        );

        let bounds = cx.debug_bounds("row").expect("row not painted");
        assert_eq!(
            bounds.size.height,
            px((SIDEBAR_ROW_LINE_HEIGHT + 2. * SIDEBAR_ROW_PADDING_Y).max(32.)),
            "a title line must not let the font's own line height set the row height"
        );
        assert_eq!(bounds.size.height, px(32.));
        assert!(
            crate::app::constants::RAIL_ROW_RADIUS <= bounds.size.height / 2.,
            "row corner {:?} exceeds half of a {:?} row",
            crate::app::constants::RAIL_ROW_RADIUS,
            bounds.size.height
        );
    }

    #[gpui::test]
    fn sidebar_workspace_rows_keep_height_when_list_overflows(cx: &mut TestAppContext) {
        const ROWS: [&str; 8] = [
            "sidebar-row-0",
            "sidebar-row-1",
            "sidebar-row-2",
            "sidebar-row-3",
            "sidebar-row-4",
            "sidebar-row-5",
            "sidebar-row-6",
            "sidebar-row-7",
        ];

        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(240.)),
                AvailableSpace::Definite(px(200.)),
            ),
            |_, _| {
                let mut list = div()
                    .w(px(240.))
                    .h(px(200.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(4.));

                for selector in ROWS {
                    list = list.child(
                        sidebar_row_shell()
                            .child(div().h(px(20.)).flex_none())
                            .child(div().h(px(14.)).flex_none())
                            .debug_selector(move || selector.into()),
                    );
                }
                list
            },
        );

        for selector in ROWS {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} not painted"));
            assert_eq!(bounds.size.height, px(49.), "{selector}");
        }
    }

    #[test]
    fn a_lane_glyph_sits_on_the_whole_pixel_below_the_line_center() {
        assert_eq!(sidebar_lane_glyph_top(), 5.);
        assert!((sidebar_lane_glyph_center() - SIDEBAR_ROW_LINE_HEIGHT / 2.).abs() <= 0.5);
        let word_axis =
            sidebar_text_ink_axis(crate::ui_primitives::LABEL_XS.as_f32(), GEIST_X_HEIGHT_EM);
        assert_eq!((sidebar_lane_glyph_center() - word_axis).round(), 0.);
    }

    struct RevealHarness {
        reveal: f32,
    }

    impl gpui::Render for RevealHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .flex()
                .flex_row()
                .child(sidebar_hover_revealed_glyph_button(
                    "reveal-close".into(),
                    "icons/close.svg",
                    "Close tab".into(),
                    self.reveal,
                    SIDEBAR_TITLE_ROW_GAP,
                    crate::theme::ui_colors(),
                ))
                .child(
                    div()
                        .flex_none()
                        .size(px(SIDEBAR_LANE_GLYPH_SIZE))
                        .debug_selector(|| "lane".into()),
                )
        }
    }

    #[gpui::test]
    fn a_revealed_glyph_pushes_the_lane_by_its_reveal_progress(cx: &mut TestAppContext) {
        for (reveal, lane_left) in [
            (0., 0.),
            (0.5, (SIDEBAR_LANE_GLYPH_SIZE + SIDEBAR_TITLE_ROW_GAP) / 2.),
            (1., SIDEBAR_LANE_GLYPH_SIZE + SIDEBAR_TITLE_ROW_GAP),
        ] {
            let (_view, cx) = cx.add_window_view(move |_, _| RevealHarness { reveal });
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            assert_eq!(
                cx.debug_bounds("lane").expect("lane not painted").left(),
                px(lane_left),
                "reveal {reveal}"
            );
        }
    }

    struct TitleRowHarness {
        reveal: f32,
    }

    impl gpui::Render for TitleRowHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let title = div()
                .debug_selector(|| "row-title".into())
                .flex_initial()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .child("Sidebar title written to sit right at the truncation edge of its row");
            let close = div()
                .flex_none()
                .w(px(SIDEBAR_LANE_GLYPH_SIZE * self.reveal))
                .h(px(SIDEBAR_ROW_LINE_HEIGHT));
            div().child(sidebar_title_row(
                300.,
                div().child(title),
                sidebar_row_trailing(close, None, self.reveal),
            ))
        }
    }

    fn title_right_at(cx: &mut TestAppContext, reveal: f32) -> gpui::Pixels {
        let (_view, cx) = cx.add_window_view(move |_, _| TitleRowHarness { reveal });
        cx.simulate_resize(size(px(500.), px(100.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("row-title").expect("title painted").right()
    }

    #[gpui::test]
    fn a_resting_row_gives_its_title_the_whole_line(cx: &mut TestAppContext) {
        let right = title_right_at(cx, 0.);
        assert!(
            right >= px(300.) - px(0.5),
            "a hidden close button keeps no gap at rest, the title ends at {right:?} of 300"
        );
    }

    #[gpui::test]
    fn a_hovered_row_yields_exactly_the_close_button_and_its_gap(cx: &mut TestAppContext) {
        let right = title_right_at(cx, 1.);
        let expected = px(300. - SIDEBAR_TITLE_ROW_GAP - SIDEBAR_LANE_GLYPH_SIZE);
        assert!(
            (right - expected).abs() <= px(0.5),
            "the title ends at {right:?}, expected {expected:?}"
        );
    }

    #[cfg(target_os = "linux")]
    const TRUNCATED_TITLE: &str = "A workspace title long enough to be cut";

    #[cfg(target_os = "linux")]
    #[derive(Default)]
    struct PaintedTitleRow {
        title: Option<gpui::TextLayout>,
        branch: Option<gpui::TextLayout>,
    }

    #[cfg(target_os = "linux")]
    struct ResizedTitleRow {
        width: f32,
        painted: std::rc::Rc<std::cell::RefCell<PaintedTitleRow>>,
    }

    #[cfg(target_os = "linux")]
    impl gpui::Render for ResizedTitleRow {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let title_text = gpui::StyledText::new(TRUNCATED_TITLE);
            let branch_text = gpui::StyledText::new("main");
            *self.painted.borrow_mut() = PaintedTitleRow {
                title: Some(title_text.layout().clone()),
                branch: Some(branch_text.layout().clone()),
            };
            let title = div()
                .flex_initial()
                .max_w_full()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(title_text);
            let branch = div().flex_none().whitespace_nowrap().child(branch_text);
            div()
                .font_family("Geist")
                .text_size(px(14.))
                .child(sidebar_title_row(
                    self.width,
                    div().child(sidebar_title_with_trailing(
                        title,
                        Some(branch),
                        SIDEBAR_BRANCH_GAP,
                    )),
                    div(),
                ))
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_title_truncated_by_a_narrow_row_regains_its_text_once_the_row_widens() {
        let mut cx = gpui::HeadlessAppContext::with_asset_source(
            std::sync::Arc::new(gpui_wgpu::CosmicTextSystem::new("Geist")),
            std::sync::Arc::new(crate::assets::Assets),
        );
        cx.update(|cx| crate::assets::Assets.load_fonts(cx))
            .expect("the embedded fonts load");
        let painted = std::rc::Rc::new(std::cell::RefCell::new(PaintedTitleRow::default()));
        let shared = painted.clone();
        let window = gpui::HeadlessAppContext::open_window(
            &mut cx,
            size(px(800.), px(100.)),
            move |_, cx| {
                cx.new(|_| ResizedTitleRow {
                    width: 60.,
                    painted: shared,
                })
            },
        )
        .expect("a headless window");
        let mut frames = Vec::new();
        for width in [60., 600., 600., 60., 60., 600., 60., 600., 120., 600.] {
            cx.update_window(window.into(), |root, window, cx| {
                let root = root.downcast::<ResizedTitleRow>().expect("the row root");
                root.update(cx, |row, cx| {
                    row.width = width;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            })
            .expect("an open window");
            let painted = painted.borrow();
            let title = painted.title.as_ref().expect("the title was laid out");
            let branch = painted.branch.as_ref().expect("the branch was laid out");
            frames.push((
                width,
                title.wrapped_text(),
                title.bounds().size.width,
                branch.bounds().size.width,
            ));
        }
        let (_, _, natural_title, natural_branch) = frames
            .iter()
            .find(|(_, text, _, _)| text == TRUNCATED_TITLE)
            .cloned()
            .expect("some frame paints the whole title");
        let natural = natural_title.as_f32() + SIDEBAR_BRANCH_GAP + natural_branch.as_f32();
        assert!(
            natural > 120. && natural < 600.,
            "the row widths must straddle the title's natural width {natural}: {frames:?}"
        );
        for (width, text, _, _) in &frames {
            if *width < natural {
                assert!(
                    text != TRUNCATED_TITLE && text.ends_with('\u{2026}'),
                    "a {width} px row narrower than {natural} px painted {text:?}: {frames:?}"
                );
            } else {
                assert_eq!(
                    text, TRUNCATED_TITLE,
                    "a {width} px row wider than {natural} px painted {text:?}: {frames:?}"
                );
            }
        }
    }
}

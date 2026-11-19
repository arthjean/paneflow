use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    A11ySubtreeBuilder, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    LayoutId, Pixels, ScrollHandle, Styled, Window, accesskit, canvas, prelude::*, px,
};

pub(crate) const SCROLL_EDGE_FADE_HEIGHT: f32 = 48.;
const HIDDEN_CONTENT_EPSILON: f32 = 1.;
const OPAQUE_ALPHA: f32 = 0.999;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EdgeFadeSurface {
    Opaque(Hsla),
    Translucent,
}

impl EdgeFadeSurface {
    pub(crate) fn behind(layers_top_down: impl IntoIterator<Item = Hsla>) -> Self {
        layers_top_down
            .into_iter()
            .find(|layer| layer.a > 0.)
            .filter(|layer| layer.a >= OPAQUE_ALPHA)
            .map_or(Self::Translucent, Self::Opaque)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EdgeFadeEdges {
    pub(crate) top: Option<Pixels>,
    pub(crate) bottom: Option<Pixels>,
}

impl EdgeFadeEdges {
    fn of(scroll: &ScrollHandle, bounds: Bounds<Pixels>) -> Self {
        Self {
            top: content_hidden_above(scroll).then_some(bounds.top()),
            bottom: content_hidden_below(scroll).then_some(bounds.bottom()),
        }
    }

    pub(crate) fn opacity_for(self, row: Bounds<Pixels>) -> f32 {
        let center = row.center().y;
        let from_top = self.top.map_or(1., |edge| eased_ramp(center - edge));
        let from_bottom = self.bottom.map_or(1., |edge| eased_ramp(edge - center));
        from_top * from_bottom
    }
}

pub(crate) type EdgeFadeLine = Rc<Cell<EdgeFadeEdges>>;

pub(crate) fn content_hidden_above(scroll: &ScrollHandle) -> bool {
    -scroll.offset().y > px(HIDDEN_CONTENT_EPSILON)
}

pub(crate) fn content_hidden_below(scroll: &ScrollHandle) -> bool {
    scroll.max_offset().y + scroll.offset().y > px(HIDDEN_CONTENT_EPSILON)
}

pub(crate) fn scroll_edge_fades(
    scroll: ScrollHandle,
    surface: EdgeFadeSurface,
    line: EdgeFadeLine,
) -> impl IntoElement {
    canvas(
        move |bounds, _, _| {
            let edges = EdgeFadeEdges::of(&scroll, bounds);
            line.set(match surface {
                EdgeFadeSurface::Translucent => edges,
                EdgeFadeSurface::Opaque(_) => EdgeFadeEdges::default(),
            });
            edges
        },
        move |bounds, edges, window, _| {
            let EdgeFadeSurface::Opaque(color) = surface else {
                return;
            };
            let height = px(SCROLL_EDGE_FADE_HEIGHT).min(bounds.size.height);
            let band = |top: Pixels| Bounds {
                origin: gpui::point(bounds.left(), top),
                size: gpui::size(bounds.size.width, height),
            };
            if let Some(edge) = edges.top {
                window.paint_quad(gpui::fill(
                    band(edge),
                    gpui::linear_gradient(
                        180.,
                        gpui::linear_color_stop(color, 0.),
                        gpui::linear_color_stop(color.opacity(0.), 1.),
                    ),
                ));
            }
            if let Some(edge) = edges.bottom {
                window.paint_quad(gpui::fill(
                    band(edge - height),
                    gpui::linear_gradient(
                        180.,
                        gpui::linear_color_stop(color.opacity(0.), 0.),
                        gpui::linear_color_stop(color, 1.),
                    ),
                ));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn eased_ramp(distance: Pixels) -> f32 {
    let t = (f32::from(distance) / SCROLL_EDGE_FADE_HEIGHT).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

pub(crate) struct EdgeFaded<E> {
    element: E,
    rest_opacity: f32,
    line: EdgeFadeLine,
}

pub(crate) fn edge_faded<E: Element + Styled>(mut element: E, line: &EdgeFadeLine) -> EdgeFaded<E> {
    let rest_opacity = element.style().opacity.unwrap_or(1.);
    EdgeFaded {
        element,
        rest_opacity,
        line: line.clone(),
    }
}

impl<E: Element + Styled> Element for EdgeFaded<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.element.source_location()
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.element.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.element.write_a11y_info(node);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        self.element.a11y_synthetic_children(prepaint, builder);
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let edges = self.line.get();
        if edges != EdgeFadeEdges::default() {
            self.element.style().opacity = Some(self.rest_opacity * edges.opacity_for(bounds));
        }
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_centered_at(y: f32) -> Bounds<Pixels> {
        Bounds {
            origin: gpui::point(px(0.), px(y - 16.)),
            size: gpui::size(px(200.), px(32.)),
        }
    }

    #[test]
    fn an_opaque_top_layer_is_the_color_the_fade_paints() {
        let chrome = gpui::black();
        let backdrop = gpui::white().opacity(0.72);
        assert_eq!(
            EdgeFadeSurface::behind([chrome, backdrop]),
            EdgeFadeSurface::Opaque(chrome)
        );
    }

    #[test]
    fn a_transparent_chrome_defers_to_the_backdrop_below_it() {
        let backdrop = gpui::white();
        assert_eq!(
            EdgeFadeSurface::behind([gpui::transparent_black(), backdrop]),
            EdgeFadeSurface::Opaque(backdrop)
        );
    }

    #[test]
    fn a_veiled_or_empty_stack_is_translucent() {
        let veil = gpui::black().opacity(0.72);
        assert_eq!(
            EdgeFadeSurface::behind([gpui::transparent_black(), veil]),
            EdgeFadeSurface::Translucent
        );
        assert_eq!(
            EdgeFadeSurface::behind([gpui::transparent_black(), gpui::transparent_black()]),
            EdgeFadeSurface::Translucent
        );
    }

    fn bottom_at(y: f32) -> EdgeFadeEdges {
        EdgeFadeEdges {
            top: None,
            bottom: Some(px(y)),
        }
    }

    fn top_at(y: f32) -> EdgeFadeEdges {
        EdgeFadeEdges {
            top: Some(px(y)),
            bottom: None,
        }
    }

    #[test]
    fn rows_clear_of_the_fade_band_keep_full_opacity() {
        let edges = bottom_at(400.);
        assert_eq!(edges.opacity_for(row_centered_at(300.)), 1.);
        assert_eq!(
            edges.opacity_for(row_centered_at(400. - SCROLL_EDGE_FADE_HEIGHT)),
            1.
        );
        assert_eq!(
            EdgeFadeEdges::default().opacity_for(row_centered_at(400.)),
            1.
        );
    }

    #[test]
    fn a_row_centered_on_or_past_either_edge_is_invisible() {
        assert_eq!(bottom_at(400.).opacity_for(row_centered_at(400.)), 0.);
        assert_eq!(bottom_at(400.).opacity_for(row_centered_at(420.)), 0.);
        assert_eq!(top_at(100.).opacity_for(row_centered_at(100.)), 0.);
        assert_eq!(top_at(100.).opacity_for(row_centered_at(80.)), 0.);
    }

    #[test]
    fn opacity_falls_monotonically_toward_the_bottom_edge() {
        let edges = bottom_at(400.);
        let samples: Vec<f32> = (0..=12)
            .map(|step| edges.opacity_for(row_centered_at(352. + step as f32 * 4.)))
            .collect();
        assert!(samples.windows(2).all(|pair| pair[0] >= pair[1]));
        assert!((samples[6] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn the_top_fade_mirrors_the_bottom_fade() {
        let below = bottom_at(400.).opacity_for(row_centered_at(380.));
        let above = top_at(100.).opacity_for(row_centered_at(120.));
        assert!((below - above).abs() < 1e-6);
    }

    #[test]
    fn a_row_in_both_bands_takes_both_fades() {
        let edges = EdgeFadeEdges {
            top: Some(px(100.)),
            bottom: Some(px(160.)),
        };
        let row = row_centered_at(124.);
        let expected = top_at(100.).opacity_for(row) * bottom_at(160.).opacity_for(row);
        assert!((edges.opacity_for(row) - expected).abs() < 1e-6);
        assert!(edges.opacity_for(row) < top_at(100.).opacity_for(row));
    }
}

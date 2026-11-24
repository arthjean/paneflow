use gpui::{
    AnyElement, AnyView, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement as _, Pixels, StyleRefinement, Styled as _, Window, div,
};

pub(crate) struct ViewCache {
    view: AnyView,
    style: StyleRefinement,
    element: Option<AnyElement>,
}

pub(crate) fn cached_view(view: impl Into<AnyView>, style: StyleRefinement) -> ViewCache {
    ViewCache {
        view: view.into(),
        style,
        element: None,
    }
}

pub(crate) fn view_element(
    view: AnyView,
    style: StyleRefinement,
    accessibility_active: bool,
) -> AnyElement {
    if accessibility_active {
        let mut host = div();
        *host.style() = style;
        host.child(view).into_any_element()
    } else {
        view.cached(style).into_any_element()
    }
}

impl Element for ViewCache {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut element = view_element(
            self.view.clone(),
            self.style.clone(),
            window.is_a11y_active(),
        );
        let layout_id = element.request_layout(window, cx);
        self.element = Some(element);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(element) = self.element.as_mut() {
            element.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = self.element.as_mut() {
            element.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, Context, Entity, Render, TestAppContext, px};

    use super::*;
    use crate::redraw_guard::WatchedView;

    struct Counted {
        watched: WatchedView,
    }

    impl Render for Counted {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.watched.rendered();
            div().size_full()
        }
    }

    struct Host {
        child: Entity<Counted>,
        accessibility_active: bool,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().child(view_element(
                self.child.clone().into(),
                StyleRefinement::default().w(px(200.)).h_full(),
                self.accessibility_active,
            ))
        }
    }

    fn child_renders_after_three_parent_frames(
        accessibility_active: bool,
        cx: &mut TestAppContext,
    ) -> usize {
        let watched = WatchedView::new("Counted");
        let (host, cx) = cx.add_window_view({
            let watched = watched.clone();
            move |_, cx| Host {
                child: cx.new(|_| Counted { watched }),
                accessibility_active,
            }
        });
        cx.run_until_parked();
        for _ in 0..3 {
            host.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
        }
        watched.renders.get()
    }

    #[gpui::test]
    fn a_clean_child_view_is_reused_while_accessibility_is_off(cx: &mut TestAppContext) {
        assert_eq!(child_renders_after_three_parent_frames(false, cx), 1);
    }

    #[gpui::test]
    fn a_clean_child_view_renders_on_every_frame_while_accessibility_is_on(
        cx: &mut TestAppContext,
    ) {
        assert_eq!(
            child_renders_after_three_parent_frames(true, cx),
            4,
            "a reused view sends no accessibility nodes, so its subtree must render each frame"
        );
    }

    #[test]
    fn production_code_caches_views_only_through_view_cache() {
        use std::path::{Path, PathBuf};

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.extension().and_then(|e| e.to_str()) != Some("rs")
                    || rel == "ui_primitives/view_cache.rs"
                    || rel.ends_with("/tests.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let production = text.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
                for (i, line) in production.lines().enumerate() {
                    if line.contains(".cached(") {
                        violations.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "a view cached outside ui_primitives::view_cache drops out of the accessibility tree on every frame that reuses it:\n{}",
            violations.join("\n")
        );
    }
}

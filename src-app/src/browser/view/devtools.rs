use super::*;

impl BrowserView {
    pub(super) fn open_devtools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector_target.is_some() {
            return;
        }
        if let Some(inspector) = &self.inspector {
            inspector.update(cx, |view, cx| view.focus_document(window, cx));
            return;
        }
        let Some(document) = self.live.as_ref().and_then(LivePage::document).cloned() else {
            self.notice = Some("Load a page before opening DevTools".to_string());
            cx.notify();
            return;
        };
        let id = crate::app::browser_dock::new_browser_id();
        let descriptor = BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: id.as_str().to_owned(),
            url: self.url.clone(),
            title: "DevTools".to_string(),
            zoom: 100,
            muted: false,
            active: false,
        };
        let owner = self.owner.clone();
        let local = self.local_document.clone();
        let parent_focus = self.focus.clone();
        let inspector = cx.new(|cx| {
            let mut view = BrowserView::new(owner, id, local, &descriptor, cx);
            view.inspector_target = Some(document);
            view.inspector_parent_focus = Some(parent_focus);
            view
        });
        cx.subscribe(&inspector, |view, _inspector, event, cx| {
            if matches!(event, BrowserViewEvent::InspectorUnavailable) {
                view.notice =
                    Some("Embedded DevTools is unavailable in this CEF runtime".to_string());
            }
            if matches!(
                event,
                BrowserViewEvent::CloseReady | BrowserViewEvent::InspectorUnavailable
            ) {
                view.inspector = None;
                cx.notify();
            }
        })
        .detach();
        inspector.update(cx, |view, cx| {
            view.set_visible(true, cx);
            view.select(window, cx);
        });
        self.inspector = Some(inspector);
        cx.notify();
    }

    pub(super) fn render_inspector(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let inspector = self.inspector.clone()?;
        Some(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .flex_grow(1.0 - self.inspector_ratio)
                .min_h_0()
                .border_t_1()
                .border_color(ui.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(CONTROL_GAP))
                        .px(px(TOOLBAR_PADDING))
                        .child("DevTools")
                        .child(text_button(
                            "browser-inspector-larger",
                            "+",
                            ui,
                            cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.inspector_ratio = (this.inspector_ratio - 0.1).max(0.2);
                                cx.notify();
                            }),
                        ))
                        .child(text_button(
                            "browser-inspector-smaller",
                            "-",
                            ui,
                            cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.inspector_ratio = (this.inspector_ratio + 0.1).min(0.8);
                                cx.notify();
                            }),
                        ))
                        .child(text_button(
                            "browser-inspector-close",
                            "Close",
                            ui,
                            cx.listener(|this, _: &ClickEvent, _w, cx| {
                                if let Some(inspector) = &this.inspector {
                                    inspector.update(cx, |view, cx| {
                                        view.close_requested = true;
                                        view.forward(|document| Command::Close { document }, cx);
                                    });
                                }
                            }),
                        )),
                )
                .child(inspector)
                .into_any_element(),
        )
    }
}

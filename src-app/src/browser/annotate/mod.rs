mod prompt;

use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyWindowHandle, Context, Hsla, Window};
use paneflow_browser_client::{BrowserClient, Ticket};
use paneflow_browser_control::ANNOTATION_BINDING;
use paneflow_browser_proto::{Reply, TabId};
use serde_json::{Value, json};

use prompt::{Annotation, MAX_PICKS, PageMessage, parse, prompt};

use super::view::{BrowserView, BrowserViewEvent};
use crate::settings::kit::Ink;

const SCRIPT: &str = include_str!("annotator.js");
const WORLD: &str = "paneflow-annotator";
const INSTALL_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct World {
    tab: TabId,
    context: i64,
}

#[derive(Clone, Copy)]
struct Recipient {
    surface_id: u64,
    window: AnyWindowHandle,
}

#[derive(Default)]
pub(crate) struct Annotator {
    active: bool,
    picks: Vec<Annotation>,
    recipient: Option<Recipient>,
    world: Option<World>,
    nonce: u64,
}

fn css(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    if rgba.a >= 0.999 {
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(rgba.r),
            channel(rgba.g),
            channel(rgba.b)
        )
    } else {
        format!(
            "rgba({},{},{},{:.3})",
            channel(rgba.r),
            channel(rgba.g),
            channel(rgba.b),
            rgba.a
        )
    }
}

fn palette(ink: Ink) -> Value {
    let ui = crate::theme::ui_colors();
    json!({
        "accent": css(ink.accent.opacity(1.)),
        "surface": css(super::render::popup_fill()),
        "border": css(ink.edge),
        "text": css(ink.primary),
        "muted": css(ink.secondary),
        "faint": css(ink.tertiary),
        "dark": ui.surface.l < 0.5,
    })
}

fn wait_value(ticket: Ticket) -> Option<Value> {
    match ticket.wait(INSTALL_DEADLINE) {
        Ok(Reply::DevTools(value)) => Some(value),
        _ => None,
    }
}

fn install(client: &BrowserClient, tab: TabId) -> Option<i64> {
    wait_value(client.devtools(tab, "Runtime.enable", json!({})))?;
    let tree = wait_value(client.devtools(tab, "Page.getFrameTree", json!({})))?;
    let frame = tree
        .pointer("/frameTree/frame/id")
        .and_then(Value::as_str)?
        .to_string();
    wait_value(client.devtools(
        tab,
        "Runtime.addBinding",
        json!({ "name": ANNOTATION_BINDING, "executionContextName": WORLD }),
    ))?;
    let world = wait_value(client.devtools(
        tab,
        "Page.createIsolatedWorld",
        json!({ "frameId": frame, "worldName": WORLD }),
    ))?;
    let context = world.get("executionContextId").and_then(Value::as_i64)?;
    let installed = wait_value(client.devtools(
        tab,
        "Runtime.evaluate",
        json!({ "expression": SCRIPT, "contextId": context }),
    ))?;
    installed
        .get("exceptionDetails")
        .is_none()
        .then_some(context)
}

fn start_call(nonce: u64, palette: &Value) -> String {
    format!("window.__paneflowAnnotator.start({nonce}, {palette})")
}

impl BrowserView {
    pub(crate) fn annotating(&self) -> bool {
        self.annotator.active
    }

    pub(crate) fn annotate_for(
        &mut self,
        surface_id: u64,
        ink: Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        self.stop_annotating(cx);
        let Some(tab) = self.tab else {
            return;
        };
        let Ok(runtime) = crate::browser_control::runtime() else {
            return;
        };
        self.annotator.nonce = self.annotator.nonce.wrapping_add(1);
        self.annotator.active = true;
        self.annotator.recipient = Some(Recipient {
            surface_id,
            window: window.window_handle(),
        });
        window.focus(&self.focus, cx);
        cx.notify();
        let nonce = self.annotator.nonce;
        let call = start_call(nonce, &palette(ink));
        if let Some(world) = self.annotator.world.filter(|world| world.tab == tab) {
            self.call_page(world, call);
            return;
        }
        let client = Arc::clone(&runtime.client);
        cx.spawn(async move |this, cx| {
            let context = smol::unblock(move || install(&client, tab)).await;
            let _ = this.update(cx, |view, cx| {
                if !view.annotator.active || view.annotator.nonce != nonce || view.tab != Some(tab)
                {
                    return;
                }
                let Some(context) = context else {
                    view.annotator.active = false;
                    cx.notify();
                    return;
                };
                let world = World { tab, context };
                view.annotator.world = Some(world);
                view.call_page(world, call);
            });
        })
        .detach();
    }

    pub(crate) fn stop_annotating(&mut self, cx: &mut Context<Self>) {
        if !self.annotator.active {
            return;
        }
        self.annotator.active = false;
        self.annotator.picks.clear();
        self.call_marks("window.__paneflowAnnotator.stop()");
        self.call_marks("window.__paneflowAnnotator.keep([])");
        cx.notify();
    }

    fn call_page(&self, world: World, expression: String) {
        if let Ok(runtime) = crate::browser_control::runtime() {
            drop(runtime.client.devtools(
                world.tab,
                "Runtime.evaluate",
                json!({ "expression": expression, "contextId": world.context }),
            ));
        }
    }

    fn call_marks(&self, expression: &str) {
        if let Some(world) = self.annotator.world {
            self.call_page(world, expression.to_string());
        }
    }

    pub(crate) fn annotation_message(
        &mut self,
        context: i64,
        payload: &str,
        cx: &mut Context<Self>,
    ) {
        if self.annotator.world.map(|world| world.context) != Some(context)
            || !self.annotator.active
        {
            return;
        }
        match parse(payload, self.annotator.nonce) {
            Some(PageMessage::Pick(annotation)) => {
                let marker = annotation.marker;
                self.annotator.picks.push(*annotation);
                let number = self.annotator.picks.len();
                self.call_marks(&format!(
                    "window.__paneflowAnnotator.label({marker}, {number})"
                ));
                if number >= MAX_PICKS {
                    self.call_marks("window.__paneflowAnnotator.stop()");
                    self.finish_annotating(cx);
                }
            }
            Some(PageMessage::Done) => self.finish_annotating(cx),
            Some(PageMessage::Cancelled) => self.stop_annotating(cx),
            None => {}
        }
    }

    fn finish_annotating(&mut self, cx: &mut Context<Self>) {
        self.annotator.active = false;
        let picks = std::mem::take(&mut self.annotator.picks);
        if let (Some(recipient), false) = (self.annotator.recipient, picks.is_empty()) {
            cx.emit(BrowserViewEvent::SendAnnotations {
                surface_id: recipient.surface_id,
                text: prompt(&picks),
                window: recipient.window,
            });
        }
        self.call_marks("window.__paneflowAnnotator.keep([])");
        cx.notify();
    }

    pub(crate) fn annotation_page_left(&mut self, cx: &mut Context<Self>) {
        let was_active = self.annotator.active;
        self.annotator.world = None;
        self.annotator.active = false;
        self.annotator.picks.clear();
        if was_active {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_reach_the_page_as_css() {
        assert_eq!(css(gpui::hsla(0., 0., 1., 1.)), "#ffffff");
        assert_eq!(css(gpui::hsla(0., 0., 0., 0.5)), "rgba(0,0,0,0.500)");
    }

    #[test]
    fn the_picker_script_answers_through_the_binding_and_never_names_another_app() {
        assert!(SCRIPT.contains("window.__paneflowAnnotate"));
        assert!(SCRIPT.contains("'__paneflowAnnotator'"));
        assert!(!SCRIPT.to_ascii_lowercase().contains("zeron"));
        assert!(
            start_call(3, &json!({"accent": "#123456"}))
                .starts_with("window.__paneflowAnnotator.start(3, {")
        );
    }
}

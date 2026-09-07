use super::*;
use gpui::ClipboardItem;
use paneflow_browser_protocol::{EditAction, clipboard_text_is_valid};

impl BrowserView {
    fn clipboard_request(&mut self) -> u64 {
        self.interaction.sequence = self.interaction.sequence.wrapping_add(1).max(1);
        self.interaction.clipboard = None;
        self.interaction.sequence
    }

    pub(super) fn edit_document(&mut self, action: EditAction, cx: &mut Context<Self>) {
        let request = self.clipboard_request();
        if matches!(action, EditAction::Copy | EditAction::Cut) {
            self.interaction.clipboard = Some(request);
        }
        self.input(InputEvent::Edit { action, request }, cx);
    }

    pub(super) fn receive_clipboard(&mut self, request: u64, text: String, cx: &mut Context<Self>) {
        if self.interaction.clipboard != Some(request) {
            return;
        }
        self.interaction.clipboard = None;
        if self.visible && self.interaction.focused == Some(true) && clipboard_text_is_valid(&text)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.input(InputEvent::ClipboardWritten { request }, cx);
        }
    }

    pub(super) fn paste_from_system(&mut self, cx: &mut Context<Self>) {
        let request = self.clipboard_request();
        let generation = self.live_generation;
        let document = self.live.as_ref().and_then(LivePage::document).cloned();
        let clipboard = cx.read_from_clipboard_async();
        cx.spawn(async move |view, cx| {
            let content = clipboard.await;
            let _ = view.update(cx, |view, cx| {
                if view.interaction.sequence != request || view.live_generation != generation
                    || !view.visible || view.interaction.focused != Some(true)
                    || view.live.as_ref().and_then(LivePage::document) != document.as_ref() { return; }
                match content {
                    Ok(Some(item)) => {
                        if let Some(text) = item.text() {
                            if clipboard_text_is_valid(&text) {
                                view.input(InputEvent::Edit { action: EditAction::Paste { text }, request }, cx);
                            } else {
                                view.notice = Some("Clipboard text exceeds the browser limit or contains a null character".into());
                                cx.notify();
                            }
                        }
                    }
                    Err(_) => { view.notice = Some("Could not read the clipboard".into()); cx.notify(); }
                    Ok(None) => {}
                }
            });
        }).detach();
    }
}

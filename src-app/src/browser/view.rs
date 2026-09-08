use std::ops::Range;
use std::time::{Duration, Instant};

mod clipboard;
mod colors;
mod context_menu;
mod devtools;
mod dialogs;
mod interaction;
mod transfers;

use interaction::InteractionState;

use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, Context, CursorStyle, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, deferred, div,
    prelude::FluentBuilder, px, svg,
};
use paneflow_browser_protocol::{
    AgentAccess, BrowserError, BrowserId, Command, Document, Event, HistoryDirection, InputEvent,
    MAX_TITLE_CHARS, OperationId, Owner, ProfileId, SessionState, exported_url, normalize_address,
    zoom_percent_is_valid,
};
use paneflow_config::schema::{BROWSER_DESCRIPTOR_VERSION, BrowserDescriptor};

use super::authority::{BrowserAuthority, refusal_message};
use super::input::{button_modifier, key_events, modifiers, mouse_button, relative_position};
use super::page::{Geometry, LivePage, PageSignal};
use crate::settings::components::{menu_surface, select_item, with_alpha};
use crate::ui_primitives::{AnimatedHoverExt, TooltipDelayExt, text_tooltip};
use crate::widgets::text_input::TextInput;

pub const TOOLBAR_HEIGHT: f32 = 36.0;
pub const CONTROL_SIZE: f32 = 24.0;
pub const CONTROL_GAP: f32 = 4.0;
pub const TOOLBAR_PADDING: f32 = 8.0;
pub const ADDRESS_HEIGHT: f32 = 28.0;
pub const ADDRESS_MIN_WIDTH: f32 = 160.0;
pub const COMPACT_WIDTH: f32 = 520.0;
#[cfg(test)]
const VISIBLE_CONTROLS: f32 = 4.0;
const MENU_WIDTH: f32 = 220.0;
const AGENT_DIAGNOSTIC_MAX_BYTES: usize = paneflow_browser_protocol::MAX_AGENT_DIAGNOSTIC_BYTES;
const AGENT_DIAGNOSTIC_MAX_AGE: Duration =
    Duration::from_secs(paneflow_browser_protocol::MAX_AGENT_DIAGNOSTIC_AGE_SECS);
const ZOOM_STEPS: [u32; 17] = [
    25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500,
];

#[cfg(test)]
pub fn address_width_at(dock_width: f32) -> f32 {
    dock_width - 2.0 * TOOLBAR_PADDING - VISIBLE_CONTROLS * (CONTROL_SIZE + CONTROL_GAP)
}

pub fn bounded_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn next_zoom(current: u32, up: bool) -> u32 {
    if up {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current)
            .unwrap_or(current)
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current)
            .unwrap_or(current)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserOwner {
    pub workspace_id: u64,
    pub tab_id: u64,
    pub profile: ProfileId,
}

impl BrowserOwner {
    pub fn scope(&self) -> Owner {
        BrowserAuthority::scope(self.workspace_id, self.tab_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Navigation {
    Idle,
    Loading,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum BrowserViewEvent {
    DescriptorChanged,
    CloseReady,
    InspectorUnavailable,
    PopupRequested(String),
    QuotaChoicesRequested,
    QuotaSleepRequested(BrowserId),
    SelectionReady(super::agent::BrowserContextSelection),
    AgentAccessRequested(AgentAccess),
    AgentOperationCompleted {
        browser: BrowserId,
        operation: OperationId,
        result: Result<Event, BrowserError>,
    },
}

struct AgentDiagnosticEntry {
    received_at: Instant,
    value: serde_json::Value,
}

pub struct BrowserView {
    owner: BrowserOwner,
    id: BrowserId,
    local_document: Document,
    url: Option<String>,
    title: String,
    zoom: u32,
    muted: bool,
    state: SessionState,
    navigation: Navigation,
    can_go_back: bool,
    can_go_forward: bool,
    notice: Option<String>,
    address: Entity<TextInput>,
    find: Entity<TextInput>,
    find_open: bool,
    find_submitted: Option<String>,
    web_dialog: Option<dialogs::WebDialog>,
    inspector: Option<Entity<BrowserView>>,
    inspector_target: Option<Document>,
    inspector_parent_focus: Option<FocusHandle>,
    inspector_ratio: f32,
    download_progress: std::collections::BTreeMap<u64, (i64, i64)>,
    fullscreen: bool,
    permissions_allowed: bool,
    find_result: Option<(i32, i32)>,
    dialog_input: Entity<TextInput>,
    close_requested: bool,
    sleep_requested: bool,
    close_ready: bool,
    accessibility: super::accessibility::AccessibilityTree,
    accessibility_document: Option<Document>,
    focus: FocusHandle,
    live: Option<LivePage>,
    live_generation: u64,
    visible: bool,
    dock_width: f32,
    viewport: Option<Bounds<Pixels>>,
    page_cursor: CursorStyle,
    last_geometry: Option<Geometry>,
    resize_task: Option<gpui::Task<()>>,
    ime_composing: bool,
    ime: super::ime::ImeState,
    menu_open: bool,
    address_dirty: bool,
    wake_pending: bool,
    native_created: bool,
    pending_navigation: Option<String>,
    quota_choices: Option<Vec<(BrowserId, String)>>,
    interaction: InteractionState,
    console_entries: Vec<AgentDiagnosticEntry>,
    network_entries: Vec<AgentDiagnosticEntry>,
    console_diagnostics_enabled: bool,
    network_diagnostics_enabled: bool,
    selection_mode: bool,
    selection_anchor: Option<gpui::Point<Pixels>>,
    selection_preview: Option<super::agent::BrowserContextSelection>,
}

impl EventEmitter<BrowserViewEvent> for BrowserView {}

impl Focusable for BrowserView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BrowserView {
    pub fn new(
        owner: BrowserOwner,
        id: BrowserId,
        local_document: Document,
        descriptor: &BrowserDescriptor,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = descriptor.url.clone();
        let address = cx.new(|cx| {
            TextInput::new(
                url.clone().unwrap_or_default(),
                "Search Google or enter a URL",
                cx,
            )
        });
        Self {
            owner,
            id,
            local_document,
            url,
            title: bounded_title(&descriptor.title),
            zoom: if zoom_percent_is_valid(descriptor.zoom) {
                descriptor.zoom
            } else {
                100
            },
            muted: descriptor.muted,
            state: SessionState::Dormant,
            navigation: Navigation::Idle,
            can_go_back: false,
            can_go_forward: false,
            notice: None,
            address,
            find: cx.new(|cx| TextInput::new("", "Find in page", cx)),
            find_open: false,
            find_submitted: None,
            web_dialog: None,
            inspector: None,
            inspector_target: None,
            inspector_parent_focus: None,
            inspector_ratio: 0.6,
            download_progress: std::collections::BTreeMap::new(),
            fullscreen: false,
            permissions_allowed: false,
            find_result: None,
            dialog_input: cx.new(|cx| TextInput::new("", "Response", cx)),
            close_requested: false,
            sleep_requested: false,
            close_ready: false,
            accessibility: super::accessibility::AccessibilityTree::default(),
            accessibility_document: None,
            focus: cx.focus_handle(),
            live: None,
            live_generation: 0,
            visible: false,
            dock_width: crate::app::diff_dock::DIFF_DOCK_PANEL_WIDTH,
            viewport: None,
            page_cursor: CursorStyle::Arrow,
            last_geometry: None,
            resize_task: None,
            ime_composing: false,
            ime: super::ime::ImeState::default(),
            menu_open: false,
            address_dirty: false,
            wake_pending: false,
            native_created: false,
            pending_navigation: None,
            quota_choices: None,
            interaction: InteractionState::default(),
            console_entries: Vec::new(),
            network_entries: Vec::new(),
            console_diagnostics_enabled: false,
            network_diagnostics_enabled: false,
            selection_mode: false,
            selection_anchor: None,
            selection_preview: None,
        }
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    #[cfg(test)]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn navigation(&self) -> &Navigation {
        &self.navigation
    }

    pub(crate) fn owner_ids(&self) -> (u64, u64) {
        (self.owner.workspace_id, self.owner.tab_id)
    }

    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    pub(crate) fn browser_id(&self) -> &BrowserId {
        &self.id
    }

    pub(crate) fn document(&self) -> &Document {
        &self.local_document
    }

    pub(crate) fn session_state(&self) -> SessionState {
        self.state
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn agent_url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub(crate) fn agent_title(&self) -> &str {
        &self.title
    }

    pub(crate) fn agent_snapshot(&self) -> Option<serde_json::Value> {
        self.accessibility.agent_snapshot()
    }

    pub(crate) fn agent_selection_at(
        &self,
        x: i32,
        y: i32,
    ) -> Option<super::agent::BrowserContextSelection> {
        self.accessibility.agent_selection_at(x, y)
    }

    pub(crate) fn agent_selection_is_current(
        &self,
        selection: &super::agent::BrowserContextSelection,
    ) -> bool {
        self.accessibility.agent_selection_is_current(selection)
    }

    pub(crate) fn agent_diagnostics(&self, network: bool) -> (Vec<serde_json::Value>, bool) {
        let entries = if network {
            &self.network_entries
        } else {
            &self.console_entries
        };
        let mut output = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        let cutoff = Instant::now().checked_sub(AGENT_DIAGNOSTIC_MAX_AGE);
        for entry in entries.iter().rev() {
            if cutoff.is_some_and(|cutoff| entry.received_at < cutoff) {
                truncated = true;
                break;
            }
            let size = entry.value.to_string().len();
            if output.len() >= paneflow_browser_protocol::MAX_AGENT_DIAGNOSTIC_ENTRIES
                || bytes + size > paneflow_browser_protocol::MAX_AGENT_DIAGNOSTIC_BYTES
            {
                truncated = true;
                break;
            }
            bytes += size;
            output.push(entry.value.clone());
        }
        output.reverse();
        (output, truncated)
    }

    pub(crate) fn enable_agent_diagnostics(&mut self, network: bool) {
        if network {
            self.network_diagnostics_enabled = true;
        } else {
            self.console_diagnostics_enabled = true;
        }
    }

    fn record_agent_diagnostic(&mut self, network: bool, value: serde_json::Value) {
        let enabled = if network {
            self.network_diagnostics_enabled
        } else {
            self.console_diagnostics_enabled
        };
        if !enabled {
            return;
        }
        let entries = if network {
            &mut self.network_entries
        } else {
            &mut self.console_entries
        };
        entries.push(AgentDiagnosticEntry {
            received_at: Instant::now(),
            value,
        });
        let cutoff = Instant::now().checked_sub(AGENT_DIAGNOSTIC_MAX_AGE);
        entries.retain(|entry| cutoff.is_none_or(|cutoff| entry.received_at >= cutoff));
        while entries.len() > paneflow_browser_protocol::MAX_AGENT_DIAGNOSTIC_ENTRIES
            || entries
                .iter()
                .map(|entry| entry.value.to_string().len())
                .sum::<usize>()
                > AGENT_DIAGNOSTIC_MAX_BYTES
        {
            if entries.is_empty() {
                break;
            }
            entries.remove(0);
        }
    }

    pub(crate) fn begin_agent_selection(&mut self, cx: &mut Context<Self>) {
        self.selection_mode = true;
        self.selection_anchor = None;
        self.selection_preview = None;
        self.notice = Some("Select a page element to preview it in the composer".to_string());
        cx.notify();
    }

    pub(crate) fn agent_navigate(
        &mut self,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Result<OperationId, BrowserError> {
        let url = normalize_address(url)?;
        if !self.native_created {
            return Err(BrowserError::Unavailable);
        }
        let Some(live) = &self.live else {
            return Err(BrowserError::Unavailable);
        };
        let operation = live.send_to_document(|document| Command::AgentNavigate {
            document,
            url: url.clone(),
        })?;

        self.url = Some(url.clone());
        self.address
            .update(cx, |address, cx| address.set_value(url, cx));
        self.title.clear();
        self.navigation = Navigation::Loading;
        self.notice = None;
        self.selection_preview = None;
        cx.emit(BrowserViewEvent::DescriptorChanged);
        cx.notify();
        Ok(operation)
    }

    pub(crate) fn agent_history(
        &mut self,
        direction: HistoryDirection,
        cx: &mut Context<Self>,
    ) -> Result<OperationId, BrowserError> {
        if !self.native_created {
            return Err(BrowserError::Unavailable);
        }
        let Some(live) = &self.live else {
            return Err(BrowserError::Unavailable);
        };
        let operation = live.send_to_document(|document| Command::History {
            document,
            direction,
        })?;
        self.navigation = Navigation::Loading;
        cx.notify();
        Ok(operation)
    }

    pub(crate) fn agent_reload(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<OperationId, BrowserError> {
        if !self.native_created {
            return Err(BrowserError::Unavailable);
        }
        let Some(live) = &self.live else {
            return Err(BrowserError::Unavailable);
        };
        let operation = live.send_to_document(|document| Command::Reload {
            document,
            ignore_cache: false,
        })?;
        self.navigation = Navigation::Loading;
        cx.notify();
        Ok(operation)
    }

    pub(crate) fn agent_input(&mut self, input: InputEvent) -> Result<OperationId, BrowserError> {
        if !input.is_valid() {
            return Err(BrowserError::InvalidMessage);
        }
        if !self.native_created {
            return Err(BrowserError::Unavailable);
        }
        let Some(live) = &self.live else {
            return Err(BrowserError::Unavailable);
        };
        live.send_to_document(|document| Command::Input { document, input })
    }

    pub(crate) fn agent_screenshot(&mut self) -> Result<OperationId, BrowserError> {
        if !self.native_created {
            return Err(BrowserError::Unavailable);
        }
        let Some(live) = &self.live else {
            return Err(BrowserError::Unavailable);
        };
        live.send_to_document(|document| Command::Screenshot { document })
    }

    pub fn chip_label(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        match &self.url {
            Some(url) => url
                .split("://")
                .nth(1)
                .unwrap_or(url)
                .trim_end_matches('/')
                .to_string(),
            None => "New tab".to_string(),
        }
    }

    pub fn descriptor(&self, active: bool) -> BrowserDescriptor {
        BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: self.id.as_str().to_string(),
            url: self.url.clone(),
            title: self.title.clone(),
            zoom: self.zoom,
            muted: self.muted,
            active,
        }
    }

    pub fn address_focus_handle(&self, cx: &App) -> FocusHandle {
        self.address.read(cx).focus_handle(cx)
    }

    pub fn set_dock_width(&mut self, width: f32) {
        self.dock_width = width;
    }

    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.address_focus_handle(cx);
        window.focus(&handle, cx);
        self.address.update(cx, |input, cx| {
            let value = input.value();
            input.set_value(value, cx);
        });
        window.dispatch_action(Box::new(crate::widgets::text_input::SelectAll), cx);
        cx.notify();
    }

    pub fn focus_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if let Some(inspector) = &self.inspector {
            inspector.update(cx, |view, cx| view.set_visible(visible, cx));
        }
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.ime_cancel(cx);
            self.ime = super::ime::ImeState::default();
            self.release_input(cx);
            self.input(InputEvent::Focus { focused: false }, cx);
            self.interaction.focused = Some(false);
            self.menu_open = false;
            if let (Some(live), Some(geometry)) = (&mut self.live, self.last_geometry) {
                let _ = live.present_if_needed(geometry, false);
            }
            if self.state == SessionState::Visible {
                self.state = SessionState::Hidden;
            }
        }
        cx.notify();
    }

    pub fn request_wake(&mut self, cx: &mut Context<Self>) {
        self.set_visible(true, cx);
        self.wake_pending = self.url.is_some();
        cx.notify();
    }

    pub fn select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_visible(true, cx);
        if self.url.is_none() {
            self.focus_address(window, cx);
            return;
        }
        if self.live.is_none() && matches!(self.state, SessionState::Dormant) {
            self.wake(window, cx);
        }
    }

    pub fn navigate(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        let url = match super::address::resolve(input) {
            Ok(url) => url,
            Err(error) => {
                self.notice = Some(refusal_message(error));
                cx.notify();
                return;
            }
        };
        super::benchmark::record(self.id.as_str(), "navigate", serde_json::json!({}));
        self.notice = None;
        self.page_cursor = CursorStyle::Arrow;
        self.address_dirty = false;
        self.address
            .update(cx, |address, cx| address.set_value(url.clone(), cx));
        let reusable = self
            .live
            .as_ref()
            .is_some_and(|live| live.document().is_some())
            && matches!(self.state, SessionState::Visible | SessionState::Hidden);
        self.url = Some(url.clone());
        self.title.clear();
        self.navigation = Navigation::Loading;
        cx.emit(BrowserViewEvent::DescriptorChanged);
        if self.defer_startup_navigation(&url) {
            self.focus_document(window, cx);
            cx.notify();
            return;
        }
        if reusable {
            if let Some(live) = &self.live {
                match live.send_to_document(|document| Command::Navigate { document, url }) {
                    Ok(_) => {
                        self.last_geometry = None;
                    }
                    Err(error) => self.notice = Some(refusal_message(error)),
                }
            }
            self.focus_document(window, cx);
            return;
        }
        self.retire_live(cx);
        self.state = SessionState::Dormant;
        self.wake(window, cx);
    }

    fn defer_startup_navigation(&mut self, url: &str) -> bool {
        if !self.native_created && (self.live.is_some() || self.state == SessionState::Starting) {
            self.pending_navigation = Some(url.to_owned());
            true
        } else {
            false
        }
    }

    pub fn submit_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.address.read(cx).value();
        self.navigate(&input, window, cx);
    }

    pub fn go(&mut self, direction: HistoryDirection, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        self.forward(
            |document| Command::History {
                document,
                direction,
            },
            cx,
        );
    }

    pub fn reload(&mut self, ignore_cache: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        if self.live.is_none() {
            if self.url.is_some() {
                self.notice = None;
                self.state = SessionState::Dormant;
                self.wake(window, cx);
            }
            return;
        }
        self.navigation = Navigation::Loading;
        self.forward(
            |document| Command::Reload {
                document,
                ignore_cache,
            },
            cx,
        );
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        self.forward(|document| Command::Stop { document }, cx);
    }

    pub fn zoom_by(&mut self, up: bool, cx: &mut Context<Self>) {
        self.set_zoom(next_zoom(self.zoom, up), cx);
    }

    pub fn set_zoom(&mut self, percent: u32, cx: &mut Context<Self>) {
        if !zoom_percent_is_valid(percent) || percent == self.zoom {
            return;
        }
        self.zoom = percent;
        self.forward(|document| Command::Zoom { document, percent }, cx);
        cx.emit(BrowserViewEvent::DescriptorChanged);
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        let muted = self.muted;
        self.forward(|document| Command::Mute { document, muted }, cx);
        cx.emit(BrowserViewEvent::DescriptorChanged);
    }

    pub fn sleep(&mut self, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        if self.live.is_none() {
            self.release_live_slot(cx);
            self.state = SessionState::Dormant;
            cx.notify();
            return;
        }
        self.sleep_requested = true;
        self.forward(|document| Command::Close { document }, cx);
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.cancel_agent_control(cx);
        if let Some(inspector) = &self.inspector
            && inspector.read(cx).interaction.focused == Some(true)
        {
            inspector.update(cx, |view, cx| {
                view.close_requested = true;
                view.forward(|document| Command::Close { document }, cx);
            });
            return false;
        }
        if self.live.is_none() || self.close_ready {
            return true;
        }
        if !self.close_requested {
            self.close_requested = true;
            self.forward(|document| Command::Close { document }, cx);
        }
        false
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.retire_live(cx);
        self.state = SessionState::Closing;
        if cx.has_global::<BrowserAuthority>() {
            let _ = cx.global_mut::<BrowserAuthority>().dispatch(
                &self.owner.scope(),
                Command::Close {
                    document: self.local_document.clone(),
                },
            );
        }
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.menu_open = false;
        self.find.read(cx).focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    fn submit_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        let text = self.find.read(cx).value();
        let find_next = self.find_submitted.as_ref() == Some(&text);
        self.find_submitted = Some(text.clone());
        self.forward(
            |document| Command::Find {
                document,
                text,
                forward,
                find_next,
            },
            cx,
        );
    }

    fn close_find(&mut self, cx: &mut Context<Self>) {
        self.find_open = false;
        self.find_submitted = None;
        self.forward(|document| Command::StopFinding { document }, cx);
        cx.notify();
    }

    fn render_find(&mut self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("browser-find")
            .text_color(ui.text)
            .text_size(px(12.))
            .flex()
            .items_center()
            .gap(px(CONTROL_GAP))
            .px(px(TOOLBAR_PADDING))
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(ui.border)
            .child(
                div()
                    .id("browser-find-input")
                    .aria_label("Find in page")
                    .flex_1()
                    .min_w_0()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "enter" => {
                                this.submit_find(!event.keystroke.modifiers.shift, cx);
                                cx.stop_propagation();
                            }
                            "escape" => {
                                this.close_find(cx);
                                this.focus_document(window, cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }))
                    .child(self.find.clone()),
            )
            .child(control_button(
                "browser-find-prev",
                "icons/arrow_left.svg",
                "Previous match",
                true,
                false,
                ui,
                cx.listener(|this, _: &ClickEvent, _w, cx| this.submit_find(false, cx)),
            ))
            .child(control_button(
                "browser-find-next",
                "icons/arrow_left.svg",
                "Next match",
                true,
                true,
                ui,
                cx.listener(|this, _: &ClickEvent, _w, cx| this.submit_find(true, cx)),
            ))
            .children(
                self.find_result
                    .map(|(active, count)| format!("{active}/{count}")),
            )
            .child(text_button(
                "browser-find-close",
                "Close",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_find(cx);
                    this.focus_document(window, cx);
                }),
            ))
            .into_any_element()
    }

    pub fn escape(&mut self, cx: &mut Context<Self>) {
        if self.interaction.menu.is_some() || self.interaction.menu_requested {
            self.dismiss_context_menu(cx);
        } else if self.menu_open {
            self.menu_open = false;
            cx.notify();
        } else if self.ime_composing {
            self.ime_cancel(cx);
        } else if self.fullscreen {
            self.fullscreen = false;
            self.input(
                InputEvent::Key {
                    kind: paneflow_browser_protocol::KeyKind::RawDown,
                    key_code: 27,
                    native_key_code: 9,
                    character: 0,
                    unmodified_character: 0,
                    modifiers: 0,
                },
                cx,
            );
            self.input(
                InputEvent::Key {
                    kind: paneflow_browser_protocol::KeyKind::Up,
                    key_code: 27,
                    native_key_code: 9,
                    character: 0,
                    unmodified_character: 0,
                    modifiers: 0,
                },
                cx,
            );
            cx.notify();
        } else if self.find_open {
            self.close_find(cx);
        } else if self.navigation == Navigation::Loading {
            self.stop(cx);
        }
    }

    fn forward(&mut self, build: impl FnOnce(Document) -> Command, cx: &mut Context<Self>) {
        let Some(live) = &self.live else {
            return;
        };
        match live.send_to_document(build) {
            Ok(_) => cx.notify(),
            Err(error) => {
                self.notice = Some(refusal_message(error));
                cx.notify();
            }
        }
    }

    fn retire_live(&mut self, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        self.native_created = false;
        self.pending_navigation = None;
        self.release_input(cx);
        self.input(InputEvent::Focus { focused: false }, cx);
        self.interaction.focused = None;
        self.live_generation += 1;
        if let Some(inspector) = self.inspector.take() {
            inspector.update(cx, |view, cx| view.retire_live(cx));
        }
        self.web_dialog = None;
        self.permissions_allowed = false;
        self.fullscreen = false;
        self.download_progress.clear();
        self.accessibility.reset(None);
        self.accessibility_document = None;
        self.ime_composing = false;
        self.ime = super::ime::ImeState::default();
        self.last_geometry = None;
        if let Some(live) = self.live.take() {
            let _ = live.send_to_document(|document| Command::Close { document });
            live.shutdown();
        }
        self.release_live_slot(cx);
        self.state = SessionState::Dormant;
        cx.notify();
    }

    fn release_live_slot(&mut self, cx: &mut Context<Self>) {
        if self.inspector_target.is_some() || !cx.has_global::<BrowserAuthority>() {
            return;
        }
        let scope = self.owner.scope();
        let document = self.local_document.clone();
        let _ = cx
            .global_mut::<BrowserAuthority>()
            .dispatch(&scope, Command::Sleep { document });
    }

    pub(crate) fn start_refused(&mut self, error: BrowserError, cx: &mut Context<Self>) {
        if error == BrowserError::LimitReached {
            self.state = SessionState::Dormant;
            self.wake_pending = false;
            self.quota_choices = Some(Vec::new());
            self.notice = None;
            cx.emit(BrowserViewEvent::QuotaChoicesRequested);
        } else {
            self.notice = Some(refusal_message(error));
        }
        cx.notify();
    }

    pub(crate) fn set_quota_choices(
        &mut self,
        choices: Vec<(BrowserId, String)>,
        cx: &mut Context<Self>,
    ) {
        if self.quota_choices.is_some() {
            self.quota_choices = Some(
                choices
                    .into_iter()
                    .take(paneflow_browser_protocol::MAX_LIVE_BROWSERS)
                    .collect(),
            );
            cx.notify();
        }
    }

    pub(crate) fn cancel_quota(&mut self, cx: &mut Context<Self>) {
        self.quota_choices = None;
        self.notice = None;
        self.wake_pending = false;
        self.navigation = Navigation::Idle;
        cx.notify();
    }

    fn wake(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.wake_pending = false;
        self.quota_choices = None;
        let Some(url) = self.url.clone() else {
            return;
        };
        let origin = match super::origin_of(&url) {
            Ok(origin) => origin,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let scope = self.owner.scope();
        let local_document = self.local_document.clone();
        let (paths, profile_dir, stage_root) = {
            if !cx.has_global::<BrowserAuthority>() {
                self.notice = Some("The browser is unavailable".to_string());
                cx.notify();
                return;
            }
            let authority = cx.global_mut::<BrowserAuthority>();
            let Some(paths) = authority.host_paths() else {
                self.notice = Some("The browser is unavailable on this platform".to_string());
                cx.notify();
                return;
            };
            let profiles = match authority.profiles() {
                Ok(profiles) => profiles,
                Err(error) => {
                    self.notice = Some(error.message());
                    cx.notify();
                    return;
                }
            };
            let profile_dir = profiles
                .root()
                .join("profiles")
                .join(self.owner.profile.as_str());
            let stage_root = profiles.root().to_path_buf();
            if self.inspector_target.is_none()
                && let Err(error) = authority.dispatch(
                    &scope,
                    Command::Start {
                        document: local_document,
                    },
                )
            {
                self.start_refused(error, cx);
                return;
            }
            (paths, profile_dir, stage_root)
        };
        let page = super::page::PageConfig {
            benchmark_id: self.id.as_str().to_owned(),
            host_binary: paths.0,
            runtime_root: paths.1,
            stage_root,
            profile_dir,
            origin,
            owner: scope,
        };
        match LivePage::start(window, cx, page) {
            Ok(start) => {
                self.live_generation += 1;
                let generation = self.live_generation;
                self.live = Some(start.page);
                self.state = SessionState::Starting;
                self.navigation = Navigation::Loading;
                self.notice = None;
                let host_events = start.host_events;
                let benchmark_id = self.id.as_str().to_owned();
                cx.spawn(async move |this, cx| {
                    let mut retired = false;
                    while let Ok(event) = host_events.recv().await {
                        if let super::supervisor::HostEvent::Native(value) = &event {
                            let kind = value
                                .get("native")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("");
                            if kind == "capture_frame_rate" {
                                super::benchmark::record(
                                    &benchmark_id,
                                    kind,
                                    serde_json::json!({"host_at_ns":value.get("at_ns"), "fps":value.get("fps"), "reason":value.get("reason")}),
                                );
                            }
                            if matches!(
                                kind,
                                "trace_started"
                                    | "trace_stop_requested"
                                    | "trace_completed"
                                    | "trace_failed"
                                    | "close_requested"
                                    | "close_dispatched"
                            ) {
                                super::benchmark::record(
                                    &benchmark_id,
                                    kind,
                                    serde_json::json!({"trace_us":value.get("trace_us")}),
                                );
                            }
                        }
                        let finished = matches!(
                            event,
                            super::supervisor::HostEvent::Stopped
                                | super::supervisor::HostEvent::Lost(_)
                        );
                        if retired {
                            if finished {
                                break;
                            }
                            continue;
                        }
                        let alive = this.update(cx, |view, cx| {
                            if view.live_generation != generation {
                                return false;
                            }
                            view.on_host_event(event, cx);
                            true
                        });
                        if !matches!(alive, Ok(true)) {
                            retired = true;
                        }
                        if finished {
                            break;
                        }
                    }
                })
                .detach();
                let completions = start.gpu_completions;
                cx.spawn(async move |this, cx| {
                    while let Ok(completion) = completions.recv().await {
                        let alive = this.update(cx, |view, cx| {
                            if view.live_generation != generation {
                                return false;
                            }
                            if let Some(live) = &mut view.live {
                                match live.on_gpu_completion(completion) {
                                    Ok(()) => cx.notify(),
                                    Err(error) => view.fail(error, cx),
                                }
                            }
                            true
                        });
                        if !matches!(alive, Ok(true)) {
                            break;
                        }
                    }
                })
                .detach();
                cx.notify();
            }
            Err(error) => {
                self.release_live_slot(cx);
                self.notice = Some(format!("Acceleration unavailable: {error}"));
                self.state = SessionState::Crashed;
                cx.notify();
            }
        }
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        log::warn!("browser page {}: {reason}", self.id.as_str());
        self.retire_live(cx);
        self.state = SessionState::Crashed;
        self.navigation = Navigation::Failed(reason.clone());
        self.notice = Some(reason);
        cx.notify();
    }

    #[cfg(target_os = "linux")]
    fn on_host_event(&mut self, event: super::supervisor::HostEvent, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else {
            return;
        };
        let signals = live.on_host_event(event, cx);
        for signal in signals {
            self.on_signal(signal, cx);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn on_host_event(&mut self, _event: (), _cx: &mut Context<Self>) {}

    fn on_signal(&mut self, signal: PageSignal, cx: &mut Context<Self>) {
        match signal {
            PageSignal::Ready => {
                self.pending_navigation = None;
                let Some(url) = self.url.clone() else {
                    return;
                };
                let scope = self.owner.scope();
                let command = if let Some(document) = &self.inspector_target {
                    Command::CreateDevTools {
                        document: document.clone(),
                        browser: self.id.clone(),
                    }
                } else {
                    Command::Create {
                        owner: scope,
                        browser: self.id.clone(),
                        profile: self.owner.profile.clone(),
                        url,
                        title: self.title.clone(),
                    }
                };
                if let Some(live) = &self.live
                    && let Err(error) = live.send(command)
                {
                    self.fail(format!("host refused the page: {error:?}"), cx);
                }
            }
            PageSignal::Created => {
                self.native_created = true;
                if let Some(url) = self.pending_navigation.take()
                    && let Some(live) = &self.live
                    && let Err(error) =
                        live.send_to_document(|document| Command::Navigate { document, url })
                {
                    self.notice = Some(refusal_message(error));
                }
                cx.notify();
            }
            PageSignal::State(session) => {
                self.local_document = session.document.clone();
                if self.accessibility_document.as_ref() != Some(&session.document) {
                    self.clear_stale_document_notice();
                    if let Some(inspector) = self.inspector.take() {
                        inspector.update(cx, |view, cx| view.retire_live(cx));
                    }
                    self.accessibility.reset(Some(session.document.clone()));
                    self.accessibility_document = Some(session.document.clone());
                    self.web_dialog = None;
                    self.find_submitted = None;
                    self.find_result = None;
                    self.permissions_allowed = false;
                    self.fullscreen = false;
                    self.download_progress.clear();
                    self.console_entries.clear();
                    self.network_entries.clear();
                    self.console_diagnostics_enabled = false;
                    self.network_diagnostics_enabled = false;
                    self.selection_preview = None;
                }
                self.state = session.state;
                if session.state == SessionState::Dormant
                    && let Some(live) = &self.live
                    && let Err(error) =
                        live.send_to_document(|document| Command::Start { document })
                {
                    self.fail(format!("host refused to start the page: {error:?}"), cx);
                    return;
                }
                if !session.presentation.mounted {
                    self.last_geometry = None;
                }
                cx.notify();
            }
            PageSignal::Closed => {
                self.retire_live(cx);
                self.can_go_back = false;
                self.can_go_forward = false;
                self.navigation = Navigation::Idle;
                self.sleep_requested = false;
                if self.close_requested {
                    self.close_ready = true;
                    cx.emit(BrowserViewEvent::CloseReady);
                }
            }
            PageSignal::WebDialog {
                document,
                request,
                kind,
                origin,
                message,
                default_text,
            } => {
                if self.web_dialog.is_some() {
                    self.input(
                        InputEvent::WebResponse {
                            request,
                            accept: false,
                            text: String::new(),
                        },
                        cx,
                    );
                    return;
                }
                self.dismiss_context_menu(cx);
                self.dialog_input
                    .update(cx, |input, cx| input.set_value(default_text, cx));
                self.web_dialog = Some(dialogs::WebDialog {
                    document,
                    request,
                    kind,
                    origin,
                    message,
                });
                cx.notify();
            }
            PageSignal::CloseCancelled => {
                self.close_requested = false;
                self.sleep_requested = false;
                self.notice = Some("Closing cancelled".to_string());
                cx.notify();
            }
            PageSignal::Fullscreen(enabled) => {
                self.fullscreen = enabled;
                cx.notify();
            }
            PageSignal::FindResult { count, active } => {
                self.find_result = Some((active, count));
                cx.notify();
            }
            PageSignal::ExternalOpen(url) => {
                cx.spawn(async move |this, cx| {
                    if let Err(error) =
                        smol::unblock(move || crate::external_open::open_url(&url)).await
                    {
                        let _ = this.update(cx, |view, cx| {
                            view.notice = Some(format!("Could not open externally: {error}"));
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            PageSignal::Transfer(value) => {
                self.handle_transfer(&value, cx);
            }
            PageSignal::WebDialogClosed { request } => {
                if self
                    .web_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.request == request)
                {
                    self.web_dialog = None;
                }
                cx.notify();
            }
            PageSignal::Accessibility {
                document,
                kind,
                value,
            } => {
                if self.accessibility.apply(&document, &kind, &value) {
                    cx.notify();
                }
            }
            PageSignal::Console { document, value } => {
                if self.local_document == document {
                    self.record_agent_diagnostic(false, value);
                    cx.notify();
                }
            }
            PageSignal::Network { document, value } => {
                if self.local_document == document {
                    self.record_agent_diagnostic(true, value);
                    cx.notify();
                }
            }
            PageSignal::OperationCompleted { operation, result } => {
                cx.emit(BrowserViewEvent::AgentOperationCompleted {
                    browser: self.id.clone(),
                    operation,
                    result,
                });
            }
            PageSignal::PopupRequested(url) => {
                cx.emit(BrowserViewEvent::PopupRequested(url));
            }

            PageSignal::Refused(error) => {
                if error == paneflow_browser_protocol::BrowserError::EmbeddedDevToolsUnavailable
                    && self.inspector_target.is_some()
                {
                    self.retire_live(cx);
                    cx.emit(BrowserViewEvent::InspectorUnavailable);
                } else if self.state == SessionState::Starting {
                    self.fail(format!("host refused the page: {error:?}"), cx);
                } else {
                    self.notice = Some(refusal_message(error));
                    cx.notify();
                }
            }
            PageSignal::Loading {
                loading,
                can_go_back,
                can_go_forward,
            } => {
                self.can_go_back = can_go_back;
                self.can_go_forward = can_go_forward;
                if loading {
                    self.clear_stale_document_notice();
                    self.release_input(cx);
                    self.interaction.focused = None;
                    self.navigation = Navigation::Loading;
                } else if self.navigation == Navigation::Loading {
                    self.navigation = Navigation::Idle;
                }
                cx.notify();
            }
            PageSignal::Title(title) => {
                let title = bounded_title(&title);
                if title != self.title {
                    self.title = title;
                    cx.emit(BrowserViewEvent::DescriptorChanged);
                    cx.notify();
                }
            }
            PageSignal::Address(url) => {
                if normalize_address(&url).is_ok() && self.url.as_deref() != Some(url.as_str()) {
                    self.url = Some(url.clone());
                    if !self.address_dirty {
                        self.address
                            .update(cx, |address, cx| address.set_value(url, cx));
                    }
                    cx.emit(BrowserViewEvent::DescriptorChanged);
                    cx.notify();
                }
            }
            PageSignal::Loaded => {
                self.clear_stale_document_notice();
                self.interaction.focused = None;
                if self.navigation == Navigation::Loading {
                    self.navigation = Navigation::Idle;
                }
                cx.notify();
            }
            PageSignal::LoadFailed(reason) => {
                self.navigation = Navigation::Failed(reason);
                cx.notify();
            }
            PageSignal::Clipboard { request, text } => self.receive_clipboard(request, text, cx),
            PageSignal::ContextMenu {
                request,
                x,
                y,
                items,
            } => self.receive_context_menu(request, x, y, items, cx),
            PageSignal::ContextMenuClosed { request } => {
                if self
                    .interaction
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.request == request)
                {
                    self.interaction.menu = None;
                    cx.notify();
                }
            }
            PageSignal::ImeSelection(snapshot) => {
                if self.visible && self.interaction.focused == Some(true) {
                    self.ime.selection_changed(snapshot);
                    cx.notify();
                }
            }
            PageSignal::ImeBounds(snapshot) => {
                if self.ime_composing && self.visible && self.interaction.focused == Some(true) {
                    self.ime.composition_bounds(snapshot);
                    cx.notify();
                }
            }
            PageSignal::Cursor(cursor) => {
                self.page_cursor = cursor;
                cx.notify();
            }
            PageSignal::Repaint => cx.notify(),
            PageSignal::Lost(reason) => {
                self.retire_live(cx);
                self.state = SessionState::Crashed;
                self.navigation = Navigation::Failed(reason.clone());
                self.notice = Some("The browser stopped. Reload to relaunch it.".to_string());
                log::warn!("browser page {} lost its host: {reason}", self.id.as_str());
                cx.notify();
            }
            PageSignal::Fatal(reason) => self.fail(reason, cx),
        }
    }

    fn input(&mut self, input: InputEvent, cx: &mut Context<Self>) {
        self.cancel_agent_control(cx);
        if self.web_dialog.is_some()
            && !matches!(
                input,
                InputEvent::WebResponse { .. }
                    | InputEvent::TransferResponse { .. }
                    | InputEvent::CancelDownload { .. }
            )
        {
            return;
        }
        let Some(live) = &self.live else {
            return;
        };
        if self.state == SessionState::Starting {
            return;
        }
        if super::benchmark::enabled() && matches!(&input, InputEvent::MouseWheel { .. }) {
            super::benchmark::record(self.id.as_str(), "wheel", serde_json::json!({}));
        }
        if matches!(input, InputEvent::Focus { focused: false }) {
            if self.ime_composing {
                let _ = live.send_to_document(|document| Command::Input {
                    document,
                    input: InputEvent::ImeCancel,
                });
            }
            self.ime_composing = false;
            self.ime = super::ime::ImeState::default();
        }
        let _ = live.send_to_document(|document| Command::Input { document, input });
        let _ = cx;
    }

    fn cancel_agent_control(&self, cx: &mut Context<Self>) {
        if cx.has_global::<BrowserAuthority>() {
            cx.global_mut::<BrowserAuthority>()
                .cancel_agent_for_browser(self.owner.workspace_id, &self.id);
        }
    }

    fn viewport_origin(&self) -> gpui::Point<Pixels> {
        self.viewport
            .map(|bounds| bounds.origin)
            .unwrap_or_default()
    }

    fn browser_position(&self, point: gpui::Point<Pixels>) -> (i32, i32) {
        let (x, y) = relative_position(point, self.viewport_origin());
        match (self.viewport, self.last_geometry) {
            (Some(bounds), Some(geometry))
                if bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.) =>
            {
                (
                    (x as f32 * geometry.width as f32 / f32::from(bounds.size.width)).round()
                        as i32,
                    (y as f32 * geometry.height as f32 / f32::from(bounds.size.height)).round()
                        as i32,
                )
            }
            _ => (x, y),
        }
    }

    fn mouse_button_event(
        &mut self,
        button: MouseButton,
        point: gpui::Point<Pixels>,
        down: bool,
        click_count: usize,
        held: &gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(button) = mouse_button(button) else {
            return;
        };
        if self.selection_mode && button == paneflow_browser_protocol::MouseButton::Left {
            if down {
                self.selection_anchor = Some(point);
            } else if self.selection_anchor.take().is_some() {
                let (x, y) = self.browser_position(point);
                if let Some(selection) = self.agent_selection_at(x, y) {
                    let mut selection = selection;
                    selection.url = self.url.as_deref().and_then(exported_url);
                    self.selection_preview = Some(selection.clone());
                    cx.emit(BrowserViewEvent::SelectionReady(selection));
                }
                self.selection_mode = false;
                self.notice = None;
                cx.notify();
            }
            return;
        }
        let (x, y) = self.browser_position(point);
        self.input(
            InputEvent::MouseButton {
                x,
                y,
                button,
                down,
                clicks: click_count.clamp(1, 3) as u8,
                modifiers: modifiers(held) | self.interaction.buttons,
            },
            cx,
        );
    }

    pub fn ime_compose(
        &mut self,
        text: &str,
        cursor: Option<Range<usize>>,
        replacement: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.ime_composing = !text.is_empty();
        let Ok(replacement) = super::ime::replacement_range(replacement) else {
            return;
        };
        if let Some([start, end]) = replacement {
            self.ime.marked = Some(start as usize..end as usize);
        }
        let selected = self.ime.compose(text, cursor);
        let cursor = selected.end as u32;
        self.input(
            InputEvent::ImeComposition {
                text: text.to_string(),
                cursor,
                selection_start: Some(selected.start as u32),
                replacement,
            },
            cx,
        );
    }

    pub fn ime_commit(
        &mut self,
        text: &str,
        replacement: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let Ok(replacement) = super::ime::replacement_range(replacement) else {
            return;
        };
        self.ime_composing = false;
        self.ime.finish();
        self.input(
            InputEvent::ImeCommit {
                text: text.to_string(),
                replacement,
            },
            cx,
        );
    }

    pub fn ime_cancel(&mut self, cx: &mut Context<Self>) {
        if self.ime_composing {
            self.ime_composing = false;
            self.ime = super::ime::ImeState::default();
            self.input(InputEvent::ImeCancel, cx);
        }
    }

    fn present(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.viewport else {
            return;
        };
        let width = f32::from(bounds.size.width).round() as u32;
        let height = f32::from(bounds.size.height).round() as u32;
        if width == 0 || height == 0 {
            return;
        }
        let geometry = Geometry {
            width,
            height,
            scale_percent: ((window.scale_factor() * 100.0).round() as u32).clamp(50, 400),
        };
        let result = self
            .live
            .as_mut()
            .map(|live| live.present_if_needed(geometry, self.visible));
        match result {
            Some(Ok(true)) => {
                self.last_geometry = Some(geometry);
                self.resize_task = None;
            }
            Some(Ok(false)) => {
                if self.resize_task.is_none() && self.visible {
                    self.resize_task = Some(cx.spawn(async move |view, cx| {
                        smol::Timer::after(std::time::Duration::from_millis(16)).await;
                        let _ = view.update(cx, |view, cx| {
                            view.resize_task = None;
                            cx.notify();
                        });
                    }));
                }
            }
            Some(Err(error)) if error != BrowserError::Unavailable => {
                self.notice = Some(refusal_message(error));
                cx.notify();
            }
            _ => {}
        }
    }

    fn render_toolbar(&mut self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.navigation == Navigation::Loading && self.live.is_some();
        let compact = self.dock_width < COMPACT_WIDTH;
        let address_bg = with_alpha(ui.text, 0.06);
        let toolbar = div()
            .h(px(TOOLBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(CONTROL_GAP))
            .px(px(TOOLBAR_PADDING))
            .border_b_1()
            .border_color(ui.border)
            .child(control_button(
                "browser-back",
                "icons/arrow_left.svg",
                "Back",
                self.can_go_back,
                false,
                ui,
                cx.listener(|this, _: &ClickEvent, _w, cx| this.go(HistoryDirection::Back, cx)),
            ))
            .child(control_button(
                "browser-forward",
                "icons/arrow_left.svg",
                "Forward",
                self.can_go_forward,
                true,
                ui,
                cx.listener(|this, _: &ClickEvent, _w, cx| this.go(HistoryDirection::Forward, cx)),
            ))
            .child(if loading {
                control_button(
                    "browser-stop",
                    "icons/player-stop.svg",
                    "Stop",
                    true,
                    false,
                    ui,
                    cx.listener(|this, _: &ClickEvent, _w, cx| this.stop(cx)),
                )
            } else {
                control_button(
                    "browser-reload",
                    "icons/refresh.svg",
                    "Reload",
                    self.url.is_some(),
                    false,
                    ui,
                    cx.listener(|this, _: &ClickEvent, window, cx| this.reload(false, window, cx)),
                )
            })
            .child(
                div()
                    .id("browser-address")
                    .flex_1()
                    .min_w(px(ADDRESS_MIN_WIDTH))
                    .h(px(ADDRESS_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .rounded(px(6.))
                    .bg(address_bg)
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "enter" => {
                                this.submit_address(window, cx);
                                cx.stop_propagation();
                            }
                            "escape" => {
                                let url = this.url.clone().unwrap_or_default();
                                this.address_dirty = false;
                                this.address
                                    .update(cx, |address, cx| address.set_value(url, cx));
                                if this.url.is_some() {
                                    this.focus_document(window, cx);
                                }
                                cx.stop_propagation();
                            }
                            _ => this.address_dirty = true,
                        }
                    }))
                    .child(self.address.clone()),
            );
        let toolbar = toolbar.child(
            div()
                .relative()
                .child(control_button(
                    "browser-menu",
                    "icons/dots.svg",
                    "More",
                    true,
                    false,
                    ui,
                    cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    }),
                ))
                .when(self.menu_open, |slot| {
                    slot.child(self.render_menu(compact, ui, cx))
                }),
        );
        toolbar.into_any_element()
    }

    fn render_menu(
        &self,
        compact: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = |id: &'static str, label: String| {
            select_item(id, false, ui)
                .h(px(28.))
                .px(px(8.))
                .cursor(CursorStyle::PointingHand)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_size(px(13.))
                        .text_color(ui.text)
                        .child(label),
                )
        };
        let menu = menu_surface(div().id("browser-menu-surface"), ui)
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .w(px(MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                    this.menu_open = false;
                    cx.notify();
                }),
            )
            .child(
                row("browser-menu-inspect", "Inspect".to_string()).on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.menu_open = false;
                        this.open_devtools(window, cx);
                    },
                )),
            )
            .child(
                row("browser-menu-find", "Find in page".to_string()).on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.open_find(window, cx)),
                ),
            )
            .child(
                row(
                    "browser-menu-select-context",
                    "Select page context".to_string(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.menu_open = false;
                    this.begin_agent_selection(cx);
                })),
            )
            .child(
                row(
                    "browser-menu-agent-read",
                    "Allow agent to read pages".to_string(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.menu_open = false;
                    cx.emit(BrowserViewEvent::AgentAccessRequested(AgentAccess::Read));
                })),
            )
            .child(
                row(
                    "browser-menu-agent-interact",
                    "Allow agent to interact".to_string(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.menu_open = false;
                    cx.emit(BrowserViewEvent::AgentAccessRequested(
                        AgentAccess::Interact,
                    ));
                })),
            )
            .child(
                row(
                    "browser-menu-agent-disabled",
                    "Disable agent browser access".to_string(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.menu_open = false;
                    cx.emit(BrowserViewEvent::AgentAccessRequested(
                        AgentAccess::Disabled,
                    ));
                })),
            )
            .child(
                row("browser-menu-external", "Open externally".to_string()).on_click(cx.listener(
                    |this, _: &ClickEvent, _w, cx| {
                        this.menu_open = false;
                        if let Some(url) = this.url.clone()
                            && let Err(error) = crate::external_open::open_url(&url)
                        {
                            this.notice = Some(format!("Could not open externally: {error}"));
                        }
                        cx.notify();
                    },
                )),
            )
            .child(
                row(
                    "browser-menu-reload-hard",
                    "Reload ignoring cache".to_string(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.menu_open = false;
                    this.reload(true, window, cx);
                })),
            )
            .child(
                row("browser-menu-zoom-in", format!("Zoom in ({}%)", self.zoom)).on_click(
                    cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.zoom_by(true, cx);
                    }),
                ),
            )
            .child(
                row("browser-menu-zoom-out", "Zoom out".to_string()).on_click(cx.listener(
                    |this, _: &ClickEvent, _w, cx| {
                        this.zoom_by(false, cx);
                    },
                )),
            )
            .child(
                row("browser-menu-zoom-reset", "Reset zoom".to_string()).on_click(cx.listener(
                    |this, _: &ClickEvent, _w, cx| {
                        this.set_zoom(100, cx);
                    },
                )),
            )
            .child(
                row(
                    "browser-menu-mute",
                    if self.muted {
                        "Unmute page"
                    } else {
                        "Mute page"
                    }
                    .to_string(),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.menu_open = false;
                    this.toggle_mute(cx);
                })),
            )
            .when(self.permissions_allowed, |menu| {
                menu.child(
                    row(
                        "browser-permissions-revoke",
                        "Revoke permissions and sleep".to_string(),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                        this.menu_open = false;
                        this.sleep(cx);
                    })),
                )
            })
            .child(
                row("browser-menu-sleep", "Put to sleep".to_string()).on_click(cx.listener(
                    |this, _: &ClickEvent, _w, cx| {
                        this.menu_open = false;
                        this.sleep(cx);
                    },
                )),
            )
            .when(compact, |menu| {
                menu.child(
                    row("browser-menu-focus-address", "Edit address".to_string()).on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.menu_open = false;
                            this.focus_address(window, cx);
                        }),
                    ),
                )
            });
        deferred(
            div()
                .absolute()
                .top(px(CONTROL_SIZE + 4.))
                .right(px(0.))
                .occlude()
                .child(menu),
        )
        .with_priority(3)
        .into_any_element()
    }

    fn render_body(&mut self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        if let Some(choices) = &self.quota_choices {
            return div()
                .id("browser-live-limit")
                .role(gpui::accesskit::Role::Group)
                .aria_label("Browser live page limit")
                .track_focus(&self.focus)
                .flex()
                .flex_col()
                .items_center()
                .size_full()
                .min_h_0()
                .overflow_y_scroll()
                .gap(px(10.))
                .p(px(16.))
                .child("Eight browser pages are already live")
                .child("Choose a page to put to sleep, then return here and Reload.")
                .children(choices.iter().map(|(id, title)| {
                    let target = id.clone();
                    text_button(
                        SharedString::from(format!("browser-quota-{}", id.as_str())),
                        SharedString::from(format!("Put {title} to sleep")),
                        ui,
                        cx.listener(move |view, _: &ClickEvent, _, cx| {
                            cx.emit(BrowserViewEvent::QuotaSleepRequested(target.clone()));
                            view.cancel_quota(cx);
                        }),
                    )
                }))
                .child(text_button(
                    "browser-quota-cancel",
                    "Cancel",
                    ui,
                    cx.listener(|view, _: &ClickEvent, _, cx| view.cancel_quota(cx)),
                ))
                .into_any_element();
        }
        let surface = self.live.as_ref().and_then(LivePage::surface);
        let message: Option<(&'static str, String, bool)> = match (&self.url, self.state) {
            (None, _) => Some((
                "icons/world.svg",
                "Search Google or enter a URL".to_string(),
                false,
            )),
            (Some(_), SessionState::Crashed) => Some((
                "icons/triangle-alert.svg",
                self.notice
                    .clone()
                    .unwrap_or_else(|| "The page stopped".to_string()),
                true,
            )),
            (Some(_), SessionState::Dormant) => Some((
                "icons/moon.svg",
                self.notice
                    .clone()
                    .unwrap_or_else(|| "Sleeping. Reload to load the page.".to_string()),
                true,
            )),
            (Some(_), SessionState::Starting) if surface.is_none() => Some((
                "icons/loader-circle.svg",
                "Starting the browser…".to_string(),
                false,
            )),
            (Some(_), _) => match &self.navigation {
                Navigation::Failed(reason) if surface.is_none() => {
                    Some(("icons/triangle-alert.svg", reason.clone(), true))
                }
                Navigation::Loading if surface.is_none() => {
                    Some(("icons/loader-circle.svg", "Loading…".to_string(), false))
                }
                _ => None,
            },
        };
        let viewport = div()
            .id("browser-viewport")
            .role(gpui::accesskit::Role::Pane)
            .aria_label("Web document")
            .on_drop(cx.listener(Self::drop_external_files))
            .a11y_synthetic_children({
                let tree = self.accessibility.clone();
                move |builder| tree.append(builder)
            })
            .cursor(if self.selection_mode {
                CursorStyle::Crosshair
            } else if self.live.is_some() && self.state == SessionState::Visible {
                self.page_cursor
            } else {
                CursorStyle::Arrow
            })
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .bg(gpui::white())
            .track_focus(&self.focus)
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button.is_some() && view.interaction.buttons == 0 {
                    return;
                }
                view.pointer_move(event, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event, window, cx| view.pointer_down(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event, window, cx| view.pointer_down(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|view, event, window, cx| view.pointer_down(event, window, cx)),
            )
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _, cx| {
                let (x, y) = view.browser_position(event.position);
                let (delta_x, delta_y) = view.interaction.scroll.consume(event.delta);
                if delta_x == 0 && delta_y == 0 {
                    cx.stop_propagation();
                    return;
                }
                view.input(
                    InputEvent::MouseWheel {
                        x,
                        y,
                        delta_x,
                        delta_y,
                        modifiers: modifiers(&event.modifiers),
                    },
                    cx,
                );
                cx.stop_propagation();
            }))
            .on_key_down(
                cx.listener(|view, event: &KeyDownEvent, _, cx| view.document_key_down(event, cx)),
            )
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, _, cx| {
                if view.interaction.menu.is_none() {
                    for input in key_events(&event.keystroke, false) {
                        view.input(input, cx);
                    }
                }
                cx.stop_propagation();
            }))
            .child({
                let view = cx.entity();
                let prepaint_view = view.clone();
                let focus = self.focus.clone();
                canvas(
                    move |bounds, _window, cx| {
                        prepaint_view.update(cx, |view, _| view.viewport = Some(bounds));
                    },
                    move |bounds, (), window, cx| {
                        Self::install_pointer_capture(&view, window);
                        if let Some(surface) = surface.clone() {
                            window.paint_external_surface(bounds, surface);
                        }
                        if focus.is_focused(window) {
                            window.handle_input(
                                &focus,
                                BrowserInputHandler {
                                    view: view.clone(),
                                    bounds,
                                },
                                cx,
                            );
                        }
                    },
                )
                .size_full()
            });
        let selection_overlay = self.render_selection_overlay(ui);
        let viewport = match message {
            Some((icon, text, actionable)) => viewport.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(ui.base)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .child(svg().size(px(20.)).path(icon).text_color(ui.muted))
                    .child(
                        div()
                            .max_w(px(420.))
                            .text_size(px(13.))
                            .text_color(ui.muted)
                            .text_center()
                            .child(text),
                    )
                    .when(actionable, |body| {
                        body.child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(8.))
                                .child(text_button(
                                    "browser-retry",
                                    "Reload",
                                    ui,
                                    cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.reload(false, window, cx);
                                    }),
                                ))
                                .child(text_button(
                                    "browser-open-externally",
                                    "Open externally",
                                    ui,
                                    cx.listener(|this, _: &ClickEvent, _w, cx| {
                                        if let Some(url) = this.url.clone()
                                            && let Err(error) = crate::external_open::open_url(&url)
                                        {
                                            this.notice =
                                                Some(format!("Could not open externally: {error}"));
                                            cx.notify();
                                        }
                                    }),
                                )),
                        )
                    }),
            ),
            None => viewport,
        };
        match selection_overlay {
            Some(overlay) => viewport.child(overlay).into_any_element(),
            None => viewport.into_any_element(),
        }
    }

    fn render_selection_overlay(&self, ui: crate::theme::UiColors) -> Option<AnyElement> {
        let selection = self.selection_preview.as_ref()?;
        let bounds = self.viewport?;
        let geometry = self.last_geometry?;
        let scale_x = f32::from(bounds.size.width) / geometry.width.max(1) as f32;
        let scale_y = f32::from(bounds.size.height) / geometry.height.max(1) as f32;
        let [x, y, width, height] = selection.rect;
        let left = (x.max(0) as f32 * scale_x).min(f32::from(bounds.size.width));
        let top = (y.max(0) as f32 * scale_y).min(f32::from(bounds.size.height));
        let width =
            (width.max(1) as f32 * scale_x).min((f32::from(bounds.size.width) - left).max(1.));
        let height =
            (height.max(1) as f32 * scale_y).min((f32::from(bounds.size.height) - top).max(1.));
        Some(
            div()
                .id("browser-selection-overlay")
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(height))
                .border_1()
                .border_color(ui.accent)
                .bg(ui.accent.opacity(0.08))
                .child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .top(px(4.))
                        .max_w(px(320.))
                        .px(px(6.))
                        .py(px(4.))
                        .bg(ui.overlay)
                        .text_size(px(11.))
                        .text_color(ui.accent)
                        .child(selection.summary.clone()),
                )
                .into_any_element(),
        )
    }
}

impl gpui::Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = colors::chrome_colors(crate::theme::ui_colors());
        if self.wake_pending && self.visible && self.live.is_none() {
            self.wake(window, cx);
        }
        self.present(window, cx);
        self.sync_input_focus(window, cx);
        if let Some(live) = &mut self.live
            && let Err(error) = live.schedule_releases(window, cx)
        {
            self.fail(error, cx);
        }
        let notice = self.notice.clone().filter(|_| {
            self.url.is_some()
                && !matches!(self.state, SessionState::Crashed | SessionState::Dormant)
        });
        div()
            .id(SharedString::from(format!("browser-{}", self.id.as_str())))
            .key_context("Browser")
            .text_color(ui.text)
            .size_full()
            .flex()
            .flex_col()
            .on_action(
                cx.listener(|this, _: &crate::BrowserFocusAddress, window, cx| {
                    this.focus_address(window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::BrowserReload, window, cx| {
                this.reload(false, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::BrowserReloadIgnoreCache, window, cx| {
                    this.reload(true, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::BrowserBack, _w, cx| {
                this.go(HistoryDirection::Back, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::BrowserForward, _w, cx| {
                this.go(HistoryDirection::Forward, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::BrowserFind, window, cx| {
                this.open_find(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::BrowserZoomIn, _w, cx| {
                this.zoom_by(true, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::BrowserZoomOut, _w, cx| {
                this.zoom_by(false, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::BrowserZoomReset, _w, cx| {
                this.set_zoom(100, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::BrowserEscape, _w, cx| {
                this.escape(cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::BrowserFocusNext, window, cx| {
                    cx.stop_propagation();
                    this.cycle_focus(true, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::BrowserFocusPrev, window, cx| {
                    cx.stop_propagation();
                    this.cycle_focus(false, window, cx);
                }),
            )
            .when(self.inspector_target.is_none(), |root| {
                root.child(self.render_toolbar(ui, cx))
            })
            .when(self.find_open, |root| root.child(self.render_find(ui, cx)))
            .children(notice.map(|notice| {
                div()
                    .flex_none()
                    .px(px(TOOLBAR_PADDING))
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(ui.vc_deleted)
                    .border_b_1()
                    .border_color(ui.border)
                    .child(notice)
            }))
            .when(self.fullscreen, |root| {
                root.child(
                    div()
                        .px(px(TOOLBAR_PADDING))
                        .text_size(px(12.))
                        .child(format!(
                            "{} · Fullscreen in dock · Escape to exit",
                            self.url
                                .as_deref()
                                .and_then(|url| super::origin_of(url).ok())
                                .unwrap_or_default()
                        )),
                )
            })
            .when(self.permissions_allowed, |root| {
                root.child(
                    div()
                        .px(px(TOOLBAR_PADDING))
                        .text_size(px(12.))
                        .child("Permissions allowed for this page"),
                )
            })
            .children(self.render_web_dialog(ui, cx))
            .children(
                self.download_progress
                    .iter()
                    .map(|(&request, &(received, total))| {
                        div()
                            .id(SharedString::from(format!("browser-download-{request}")))
                            .flex()
                            .items_center()
                            .gap(px(CONTROL_GAP))
                            .child(format!("Download: {received} / {total} bytes"))
                            .child(text_button(
                                "browser-download-cancel",
                                "Cancel",
                                ui,
                                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                    this.cancel_download(request, cx)
                                }),
                            ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .flex_grow(if self.inspector.is_some() && !self.fullscreen {
                        self.inspector_ratio
                    } else {
                        1.0
                    })
                    .child(self.render_body(ui, cx)),
            )
            .when(!self.fullscreen, |root| {
                root.children(self.render_inspector(ui, cx))
            })
            .children(self.render_context_menu(ui, window, cx))
    }
}

impl BrowserView {
    fn clear_stale_document_notice(&mut self) {
        if self.notice.as_deref()
            == Some(
                refusal_message(paneflow_browser_protocol::BrowserError::StaleGeneration).as_str(),
            )
        {
            self.notice = None;
        }
    }

    pub(crate) fn focus_from_terminal(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if forward {
            self.focus_address(window, cx);
        } else if let Some(inspector) = &self.inspector {
            inspector.update(cx, |view, cx| view.focus_document(window, cx));
        } else if self.url.is_some() {
            self.focus_document(window, cx);
        } else {
            self.focus_address(window, cx);
        }
    }

    fn cycle_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        super::benchmark::record(
            self.id.as_str(),
            "focus_cycle",
            serde_json::json!({"forward": forward, "address": self.address_focus_handle(cx).is_focused(window), "document": self.focus.is_focused(window)}),
        );
        if let Some(parent) = &self.inspector_parent_focus {
            if forward {
                window.dispatch_action(Box::new(crate::BrowserFocusTerminal), cx);
            } else {
                parent.focus(window, cx);
            }
            return;
        }
        if let Some(inspector) = &self.inspector {
            if forward && self.focus.is_focused(window) {
                inspector.update(cx, |view, cx| view.focus_document(window, cx));
                return;
            }
            if inspector.read(cx).focus.is_focused(window) {
                if forward {
                    window.dispatch_action(Box::new(crate::BrowserFocusTerminal), cx);
                } else {
                    self.focus_document(window, cx);
                }
                return;
            }
        }
        let address_focused = self.address_focus_handle(cx).is_focused(window);
        let document_focused = self.focus.is_focused(window);
        match (forward, address_focused, document_focused) {
            (true, true, _) if self.url.is_some() => self.focus_document(window, cx),
            (true, _, true) | (true, true, _) | (false, true, _) => {
                window.dispatch_action(Box::new(crate::BrowserFocusTerminal), cx);
            }
            (true, false, false) => self.focus_address(window, cx),
            (false, _, true) => self.focus_address(window, cx),
            (false, false, false) if self.url.is_some() => self.focus_document(window, cx),
            (false, false, false) => self.focus_address(window, cx),
        }
    }
}

struct BrowserInputHandler {
    view: Entity<BrowserView>,
    bounds: Bounds<Pixels>,
}

impl gpui::InputHandler for BrowserInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui::UTF16Selection> {
        let range = self.view.read(cx).ime.selection.clone()?;
        Some(gpui::UTF16Selection {
            range: range.start.min(range.end)..range.start.max(range.end),
            reversed: range.start > range.end,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view.read(cx).ime.marked.clone()
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        let text = self.view.read(cx).ime.text_for_range(range_utf16.clone())?;
        *adjusted_range = Some(range_utf16);
        Some(text)
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.view
            .update(cx, |view, cx| view.ime_commit(text, replacement_range, cx));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.ime_compose(new_text, new_selected_range, range_utf16, cx)
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            if view.ime_composing {
                view.ime_composing = false;
                view.ime.finish();
                view.input(InputEvent::ImeFinish, cx);
            }
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let view = self.view.read(cx);
        let [x, y, w, h] = view.ime.caret_bounds(range_utf16.start)?;
        let geometry = view.last_geometry?;
        let scale_x = f32::from(self.bounds.size.width) / geometry.width.max(1) as f32;
        let scale_y = f32::from(self.bounds.size.height) / geometry.height.max(1) as f32;
        Some(Bounds::new(
            self.bounds.origin + gpui::point(px(x as f32 * scale_x), px(y as f32 * scale_y)),
            gpui::size(px(w.max(1) as f32 * scale_x), px(h.max(1) as f32 * scale_y)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

fn control_button(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
    mirrored: bool,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let color = if enabled {
        ui.muted
    } else {
        with_alpha(ui.muted, 0.35)
    };
    let mut glyph = svg().size(px(14.)).flex_none().path(icon).text_color(color);
    if mirrored {
        glyph = glyph.with_transformation(gpui::Transformation::scale(gpui::size(-1.0, 1.0)));
    }
    div()
        .id(id)
        .role(gpui::accesskit::Role::Button)
        .aria_label(label)
        .when(enabled, |button| button.tab_index(0))
        .a11y_synthetic_children(move |builder| {
            if !enabled {
                builder.parent_node().set_disabled();
            }
        })
        .focus_visible(|style| style.border_1().border_color(ui.text))
        .flex_none()
        .size(px(CONTROL_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .animated_hover_bg(
            gpui::transparent_black(),
            if enabled {
                with_alpha(ui.text, 0.08)
            } else {
                gpui::transparent_black()
            },
        )
        .delayed_tooltip(text_tooltip(label))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(enabled, |button| button.on_click(on_click))
        .child(glyph)
        .into_any_element()
}

fn text_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    div()
        .id(id)
        .role(gpui::accesskit::Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .focus_visible(|style| style.border_1().border_color(ui.text))
        .flex_none()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(ui.border)
        .cursor(CursorStyle::PointingHand)
        .animated_hover_bg(gpui::transparent_black(), with_alpha(ui.text, 0.08))
        .text_size(px(12.))
        .text_color(ui.text)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn quota_cancellation_preserves_the_dormant_descriptor(cx: &mut gpui::TestAppContext) {
        let view = test_view(cx);
        view.update(cx, |view, cx| {
            let descriptor = view.descriptor(true);
            view.start_refused(BrowserError::LimitReached, cx);
            assert!(view.quota_choices.is_some());
            view.cancel_quota(cx);
            assert!(view.quota_choices.is_none());
            assert_eq!(view.state, SessionState::Dormant);
            assert_eq!(view.descriptor(true), descriptor);
            assert!(!view.wake_pending);
        });
    }

    #[gpui::test]
    fn startup_navigation_coalesces_until_native_browser_exists(cx: &mut gpui::TestAppContext) {
        let view = test_view(cx);
        view.update(cx, |view, cx| {
            view.state = SessionState::Starting;
            let generation = view.live_generation;
            assert!(view.defer_startup_navigation("https://example.com/first"));
            assert!(view.defer_startup_navigation("https://example.com/latest"));
            assert_eq!(
                view.pending_navigation.as_deref(),
                Some("https://example.com/latest")
            );
            assert_eq!(view.live_generation, generation);
            view.on_signal(PageSignal::Created, cx);
            assert!(view.native_created);
            assert!(view.pending_navigation.is_none());
            assert!(!view.defer_startup_navigation("https://example.com/after"));
        });
    }

    #[gpui::test]
    fn successful_navigation_clears_stale_generation_notice_only(cx: &mut gpui::TestAppContext) {
        let view = test_view(cx);
        view.update(cx, |view, cx| {
            view.notice = Some(refusal_message(
                paneflow_browser_protocol::BrowserError::StaleGeneration,
            ));
            view.on_signal(PageSignal::Loaded, cx);
            assert!(view.notice.is_none());
            view.notice = Some("Current failure".to_string());
            view.on_signal(PageSignal::Loaded, cx);
            assert_eq!(view.notice.as_deref(), Some("Current failure"));
        });
    }

    fn test_view(cx: &mut gpui::TestAppContext) -> Entity<BrowserView> {
        let owner = BrowserOwner {
            workspace_id: 1,
            tab_id: 1,
            profile: "p-test".to_string().try_into().unwrap(),
        };
        let id: BrowserId = "b-test".to_string().try_into().unwrap();
        let document = Document {
            owner: owner.scope(),
            browser: id.clone(),
            generation: 1,
        };
        let descriptor = BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: id.as_str().to_string(),
            url: Some("http://127.0.0.1/".to_string()),
            title: "Test".to_string(),
            zoom: 100,
            muted: false,
            active: true,
        };
        cx.new(|cx| BrowserView::new(owner, id, document, &descriptor, cx))
    }

    #[gpui::test]
    fn document_close_clears_permissions_fullscreen_and_all_transfers(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = test_view(cx);
        view.update(cx, |view, cx| {
            view.permissions_allowed = true;
            view.fullscreen = true;
            view.download_progress.insert(1, (10, 100));
            view.download_progress.insert(2, (20, 100));
            view.on_signal(PageSignal::Closed, cx);
            assert!(!view.permissions_allowed);
            assert!(!view.fullscreen);
            assert!(view.download_progress.is_empty());
        });
    }

    #[gpui::test]
    fn concurrent_web_request_does_not_displace_the_visible_request(cx: &mut gpui::TestAppContext) {
        let view = test_view(cx);
        view.update(cx, |view, cx| {
            for request in [1, 2] {
                view.on_signal(
                    PageSignal::WebDialog {
                        document: view.local_document.clone(),
                        request,
                        kind: "permission".to_string(),
                        origin: "http://127.0.0.1".to_string(),
                        message: "Microphone".to_string(),
                        default_text: String::new(),
                    },
                    cx,
                );
            }
            assert_eq!(view.web_dialog.as_ref().unwrap().request, 1);
            view.on_signal(PageSignal::WebDialogClosed { request: 2 }, cx);
            assert_eq!(view.web_dialog.as_ref().unwrap().request, 1);
            view.on_signal(PageSignal::WebDialogClosed { request: 1 }, cx);
            assert!(view.web_dialog.is_none());
        });
    }

    #[test]
    fn browser_chrome_meets_wcag_contrast_on_all_bundled_themes() {
        for (name, theme) in crate::theme::THEMES {
            let ui = colors::chrome_colors(crate::theme::ui_colors_with(&theme()));
            for (label, foreground) in [
                ("text", ui.text),
                ("muted", ui.muted),
                ("error", ui.vc_deleted),
            ] {
                for background in [ui.base, ui.surface, ui.overlay] {
                    let ratio = crate::theme::contrast_ratio(foreground, background);
                    assert!(
                        ratio >= crate::theme::WCAG_AA_TEXT_RATIO,
                        "{name} {label}: {ratio}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_address_field_keeps_at_least_160_px_at_the_narrowest_dock() {
        let narrowest = address_width_at(crate::app::diff_dock::DIFF_DOCK_PANEL_MIN_WIDTH);
        assert!(
            narrowest >= ADDRESS_MIN_WIDTH,
            "{narrowest}px of address field at 360px"
        );
        assert!(address_width_at(COMPACT_WIDTH) > narrowest);
    }

    #[test]
    fn titles_are_bounded_and_stripped_of_control_characters() {
        let long = "x".repeat(MAX_TITLE_CHARS + 50);
        assert_eq!(bounded_title(&long).chars().count(), MAX_TITLE_CHARS);
        assert_eq!(bounded_title("  Vite\u{7} App \n"), "Vite App");
    }

    #[test]
    fn zoom_steps_stay_inside_the_contract_bounds() {
        assert_eq!(next_zoom(100, true), 110);
        assert_eq!(next_zoom(100, false), 90);
        assert_eq!(next_zoom(500, true), 500);
        assert_eq!(next_zoom(25, false), 25);
        assert_eq!(next_zoom(101, true), 110);
        for step in ZOOM_STEPS {
            assert!(zoom_percent_is_valid(step));
        }
    }
}

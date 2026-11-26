use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, AppContext, Bounds, Context, CursorStyle, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, RenderImage, ScrollDelta, ScrollEvent, UTF16Selection, WeakEntity,
    Window, point, px, size,
};
use paneflow_api::methods::browser::{TabOwner, TabState};
use paneflow_browser_control::{PageDialog, SurfaceEvent, TabView};
use paneflow_browser_proto::surface::{
    Cursor, FileMode, HistoryStep, Input, Modifiers, MouseButton as PageButton, Rect, View,
};
use paneflow_browser_proto::{ProfileName, TabId};

use super::{address, kept, keys, omnibox, router, zoom};
use crate::widgets::text_input::TextInput;

const LINE_PIXELS: f32 = 40.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserViewEvent {
    Changed,
    Persist,
    Opened(TabId),
    Show(TabId),
    WantsAgents,
    WantsDevServers,
    OpenSettings(gpui::AnyWindowHandle),
    SendAnnotations {
        surface_id: u64,
        text: String,
        window: gpui::AnyWindowHandle,
    },
}

#[derive(Default)]
pub(crate) struct OmniboxState {
    pub(crate) field: String,
    pub(crate) typed: String,
    pub(crate) suggestions: omnibox::Suggestions,
    pub(crate) selected: usize,
    pub(crate) open: bool,
    pub(crate) servers: Vec<omnibox::DevServer>,
    pub(crate) bounds: Option<Bounds<Pixels>>,
    typed_url: Option<String>,
    recorded: Option<(String, Option<String>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentChoice {
    pub(crate) session: String,
    pub(crate) label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserMenu {
    Tabs,
    Pointer,
}

pub(crate) struct OpenDialog {
    pub(crate) dialog: PageDialog,
    pub(crate) text: Option<Entity<TextInput>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OpenFailure {
    NotInstalled,
    Refused(String),
    Failed(String),
}

pub(crate) struct BrowserView {
    pub(crate) workspace_id: u64,
    pub(crate) tab: Option<TabId>,
    pub(crate) dormant: Option<String>,
    pub(crate) restored_title: Option<String>,
    pub(crate) snapshot: Option<TabView>,
    pub(crate) opening: bool,
    pub(crate) failure: Option<OpenFailure>,
    pub(crate) address: Entity<TextInput>,
    pub(crate) address_error: Option<String>,
    pub(crate) focus: FocusHandle,
    pub(crate) image: Option<Arc<RenderImage>>,
    pub(crate) retired: Vec<Arc<RenderImage>>,
    pub(crate) frame_size: (u32, u32),
    pub(crate) backing: Option<gpui::Rgba>,
    pub(crate) loader: super::loader::Loader,
    pub(crate) page_bounds: Option<Bounds<Pixels>>,
    presented: Option<View>,
    displayed: bool,
    pub(crate) cursor: CursorStyle,
    composing: Option<String>,
    caret: Option<Rect>,
    pressed: Option<MouseButton>,
    page_focused: bool,
    pub(crate) takes_input: bool,
    pub(crate) dialog: Option<OpenDialog>,
    pub(crate) agents: Vec<AgentChoice>,
    pub(crate) menu: Option<BrowserMenu>,
    pub(crate) listed: Vec<TabView>,
    owner_on_open: TabOwner,
    shown_url: String,
    pub(crate) omnibox: OmniboxState,
    zoomed: Option<(TabId, String, u16)>,
    pub(crate) site_info: Option<super::site_info::SiteInfo>,
    pub(crate) site_icon_bounds: Option<Bounds<Pixels>>,
    pub(crate) security: Option<(String, paneflow_browser_proto::Connection)>,
    pub(crate) annotator: super::annotate::Annotator,
    pub(crate) targets: Vec<crate::app::agent_send::AgentTarget>,
    pub(crate) inspects: Option<TabId>,
    pub(crate) docked: Option<super::inspector::Docked>,
}

impl EventEmitter<BrowserViewEvent> for BrowserView {}

impl BrowserView {
    pub(crate) fn new(workspace_id: u64, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            TextInput::new("", "Search or type a local address", cx).with_accessible_name("Address")
        });
        cx.on_release(|view: &mut Self, _| view.close_unless_agent_owned())
            .detach();
        cx.observe(&address, |view: &mut Self, _, cx| view.address_edited(cx))
            .detach();
        omnibox::start(cx);
        zoom::start(cx);
        cx.observe_global::<kept::Store<zoom::SiteZooms>>(|view: &mut Self, cx| {
            view.apply_zoom(cx);
            cx.notify();
        })
        .detach();
        cx.observe_global::<crate::config_snapshot::ConfigSnapshot>(|view: &mut Self, cx| {
            view.apply_zoom(cx);
        })
        .detach();
        Self {
            workspace_id,
            tab: None,
            dormant: None,
            restored_title: None,
            snapshot: None,
            opening: false,
            failure: None,
            address,
            address_error: None,
            focus: cx.focus_handle().tab_stop(true),
            image: None,
            retired: Vec::new(),
            frame_size: (0, 0),
            backing: None,
            loader: super::loader::Loader::default(),
            page_bounds: None,
            presented: None,
            displayed: false,
            cursor: CursorStyle::Arrow,
            composing: None,
            caret: None,
            pressed: None,
            page_focused: false,
            takes_input: false,
            dialog: None,
            agents: Vec::new(),
            menu: None,
            listed: Vec::new(),
            owner_on_open: TabOwner::Human,
            shown_url: String::new(),
            omnibox: OmniboxState::default(),
            zoomed: None,
            site_info: None,
            site_icon_bounds: None,
            security: None,
            annotator: super::annotate::Annotator::default(),
            targets: Vec::new(),
            inspects: None,
            docked: None,
        }
    }

    pub(crate) fn shown(&self) -> bool {
        self.displayed
    }

    pub(crate) fn set_shown(&mut self, displayed: bool) {
        self.displayed = displayed;
    }

    pub(crate) fn site(&self) -> Option<String> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| zoom::site_of(&snapshot.url))
    }

    pub(crate) fn site_zoom(&self, cx: &App) -> Option<(String, u16, u16)> {
        let site = self.site()?;
        let default = crate::config_snapshot::current(cx).browser.default_zoom();
        let percent = zoom::snapshot(cx).of(&site, default);
        Some((site, percent, default))
    }

    fn apply_zoom(&mut self, cx: &mut Context<Self>) {
        let (Some(tab), Some((site, percent, _))) = (self.tab, self.site_zoom(cx)) else {
            return;
        };
        let wanted = (tab, site, percent);
        if self.zoomed.as_ref() == Some(&wanted) {
            return;
        }
        if let Ok(runtime) = crate::browser_control::runtime() {
            drop(runtime.client.zoom(tab, percent));
        }
        self.zoomed = Some(wanted);
    }

    pub(crate) fn zoom_by(&mut self, step: i8, cx: &mut Context<Self>) {
        let Some((site, percent, default)) = self.site_zoom(cx) else {
            return;
        };
        let next = if step == 0 {
            default
        } else {
            paneflow_config::schema::step_zoom(percent, step)
        };
        zoom::set(&site, next, default, cx);
    }

    pub(crate) fn restored(
        workspace_id: u64,
        url: String,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(workspace_id, cx);
        view.show_in_field(url.clone(), false, cx);
        view.dormant = Some(url);
        view.restored_title = title;
        view.owner_on_open = TabOwner::Unclaimed;
        view
    }

    pub(crate) fn attached(workspace_id: u64, tab: TabId, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(workspace_id, cx);
        view.adopt(tab, cx);
        view
    }

    pub(crate) fn profile(&self) -> Option<ProfileName> {
        crate::workspace::durable_workspace_id(self.workspace_id)
            .and_then(|durable| ProfileName::new(durable.to_string()).ok())
    }

    pub(crate) fn address_focus(&self, cx: &App) -> FocusHandle {
        self.address.read(cx).focus_handle.clone()
    }

    pub(crate) fn title(&self) -> String {
        if let Some(title) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.title.clone())
            .filter(|title| !title.trim().is_empty() && !is_blank(title))
        {
            return title;
        }
        if let Some(title) = self.restored_title.clone().filter(|t| !t.trim().is_empty()) {
            return title;
        }
        let url = self.url();
        if url.is_empty() {
            return "New tab".to_string();
        }
        host_of(&url).unwrap_or(url)
    }

    pub(crate) fn url(&self) -> String {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.url.clone())
            .filter(|url| !url.is_empty() && !is_blank(url))
            .or_else(|| self.dormant.clone())
            .unwrap_or_default()
    }

    pub(crate) fn persisted(&self) -> Option<(String, Option<String>)> {
        let url = self.url();
        if url.is_empty() {
            return None;
        }
        let title = Some(self.title()).filter(|title| title != "New tab");
        Some((url, title))
    }

    pub(crate) fn owner(&self) -> Option<&TabOwner> {
        self.snapshot.as_ref().map(|snapshot| &snapshot.owner)
    }

    pub(crate) fn set_agents(&mut self, agents: Vec<AgentChoice>, cx: &mut Context<Self>) {
        if self.agents != agents {
            self.agents = agents;
            if self.menu == Some(BrowserMenu::Pointer) {
                cx.notify();
            }
        }
    }

    pub(crate) fn set_targets(
        &mut self,
        targets: Vec<crate::app::agent_send::AgentTarget>,
        cx: &mut Context<Self>,
    ) {
        if self.targets != targets {
            self.targets = targets;
            cx.notify();
        }
    }

    pub(crate) fn set_displayed(&mut self, displayed: bool, cx: &mut Context<Self>) {
        if let Some(docked) = self.docked.as_ref() {
            docked
                .view
                .clone()
                .update(cx, |inspector, cx| inspector.set_displayed(displayed, cx));
        }
        if self.displayed == displayed {
            return;
        }
        self.displayed = displayed;
        if displayed {
            if let Some(url) = self.dormant.take() {
                self.open(url, cx);
            }
            cx.notify();
            return;
        }
        self.menu = None;
        self.page_bounds = None;
        self.stop_annotating(cx);
        if let Some(tab) = self.tab
            && self.presented.take().is_some()
            && let Ok(runtime) = crate::browser_control::runtime()
        {
            drop(runtime.client.present(tab, None));
            runtime.control.set_presented(tab, false);
        }
        cx.notify();
    }

    pub(crate) fn submit_address(&mut self, cx: &mut Context<Self>) {
        if self.omnibox.open && !self.omnibox.suggestions.rows.is_empty() {
            self.open_row(self.omnibox.selected, cx);
            return;
        }
        let typed = self.address.read(cx).value();
        match address::resolve(&typed) {
            address::Address::Go(url) => {
                self.address_error = None;
                self.omnibox.typed_url = Some(url.clone());
                self.open(url, cx);
            }
            address::Address::Refused(_) if !typed.trim().is_empty() => {
                self.address_error = None;
                let search = crate::config_snapshot::current(cx).browser.web_search();
                self.open(search.url(&typed), cx);
            }
            address::Address::Refused(reason) => self.address_error = Some(reason),
        }
        self.close_suggestions(cx);
    }

    pub(crate) fn reset_address(&mut self, cx: &mut Context<Self>) {
        let url = self.url();
        self.address_error = None;
        self.show_in_field(url, false, cx);
        cx.notify();
    }

    pub(crate) fn show_in_field(&mut self, text: String, select_all: bool, cx: &mut Context<Self>) {
        self.omnibox.field.clone_from(&text);
        self.omnibox.typed.clone_from(&text);
        self.omnibox.open = false;
        self.omnibox.suggestions = omnibox::Suggestions::default();
        self.address.update(cx, |input, cx| {
            input.set_value(text, cx);
            if select_all {
                input.select_all_text(cx);
            }
        });
    }

    fn address_edited(&mut self, cx: &mut Context<Self>) {
        let (value, selection, focused) = {
            let input = self.address.read(cx);
            (
                input.value(),
                input.selection(),
                input.focused_since().is_some(),
            )
        };
        if value == self.omnibox.field {
            return;
        }
        self.omnibox.field.clone_from(&value);
        if !focused {
            self.omnibox.typed = value;
            return;
        }
        let shrank = value.len() < self.omnibox.typed.len()
            || (self.omnibox.suggestions.inline.is_some() && value == self.omnibox.typed);
        let at_end = selection.is_empty() && selection.end == value.len();
        let was_open = self.omnibox.open;
        self.omnibox.typed = value;
        self.address_error = None;
        self.suggest(!shrank && at_end, cx);
        if !was_open && self.omnibox.open {
            cx.emit(BrowserViewEvent::WantsDevServers);
        }
    }

    fn suggest(&mut self, allow_inline: bool, cx: &mut Context<Self>) {
        let history = omnibox::history_snapshot(cx);
        let search = crate::config_snapshot::current(cx).browser.web_search();
        let suggestions = omnibox::suggest(
            &self.omnibox.typed,
            &omnibox::Sources {
                history: &history,
                servers: &self.omnibox.servers,
                search: &search,
                now: omnibox::now(),
                allow_inline,
            },
        );
        if let Some(inline) = &suggestions.inline {
            let text = inline.text.clone();
            self.omnibox.field.clone_from(&text);
            self.address
                .update(cx, |input, cx| input.complete(text, cx));
        }
        self.omnibox.open = !suggestions.rows.is_empty();
        self.omnibox.selected = 0;
        self.omnibox.suggestions = suggestions;
        cx.notify();
    }

    pub(crate) fn set_dev_servers(
        &mut self,
        servers: Vec<omnibox::DevServer>,
        cx: &mut Context<Self>,
    ) {
        if self.omnibox.servers == servers {
            return;
        }
        self.omnibox.servers = servers;
        if self.omnibox.open {
            self.suggest(false, cx);
        }
    }

    pub(crate) fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) -> bool {
        let count = self.omnibox.suggestions.rows.len();
        if !self.omnibox.open || count == 0 {
            return false;
        }
        let next = self
            .omnibox
            .selected
            .saturating_add_signed(step)
            .min(count - 1);
        if next != self.omnibox.selected {
            self.omnibox.selected = next;
            self.fill_field_with_selection(cx);
            cx.notify();
        }
        true
    }

    fn fill_field_with_selection(&mut self, cx: &mut Context<Self>) {
        let inline = self
            .omnibox
            .suggestions
            .inline
            .clone()
            .filter(|_| self.omnibox.selected == 0);
        let Some(row) = self.omnibox.suggestions.rows.get(self.omnibox.selected) else {
            return;
        };
        let fill = match &row.kind {
            omnibox::RowKind::Search { query, .. } => query.clone(),
            _ if inline.is_some() => self.omnibox.typed.clone(),
            _ => row.shown.clone(),
        };
        self.omnibox.field = inline
            .as_ref()
            .map_or_else(|| fill.clone(), |inline| inline.text.clone());
        self.address.update(cx, |input, cx| {
            input.set_value(fill, cx);
            if let Some(inline) = inline {
                input.complete(inline.text, cx);
            }
        });
    }

    pub(crate) fn close_suggestions(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.omnibox.open {
            return false;
        }
        self.omnibox.open = false;
        if self.omnibox.suggestions.inline.take().is_some() {
            let typed = self.omnibox.typed.clone();
            self.omnibox.field.clone_from(&typed);
            self.address
                .update(cx, |input, cx| input.set_value(typed, cx));
        }
        cx.notify();
        true
    }

    pub(crate) fn open_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.omnibox.suggestions.rows.get(index).cloned() else {
            return;
        };
        let typed = matches!(row.kind, omnibox::RowKind::Typed)
            || (index == 0 && self.omnibox.suggestions.inline.is_some());
        self.omnibox.typed_url = typed.then(|| row.url.clone());
        self.omnibox.open = false;
        self.omnibox.suggestions = omnibox::Suggestions::default();
        self.address_error = None;
        self.show_in_field(row.shown.clone(), false, cx);
        self.open(row.url, cx);
        cx.notify();
    }

    pub(crate) fn forget_row(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(row) = self
            .omnibox
            .suggestions
            .rows
            .get(index)
            .filter(|row| row.removable())
            .cloned()
        else {
            return false;
        };
        omnibox::forget(&row.url, cx);
        self.omnibox.suggestions.rows.remove(index);
        if self
            .omnibox
            .suggestions
            .inline
            .as_ref()
            .is_some_and(|inline| inline.url == row.url)
        {
            self.close_suggestions(cx);
            self.suggest(false, cx);
            return true;
        }
        self.omnibox.selected = self
            .omnibox
            .selected
            .min(self.omnibox.suggestions.rows.len().saturating_sub(1));
        self.omnibox.open = !self.omnibox.suggestions.rows.is_empty();
        cx.notify();
        true
    }

    fn record_history(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if !crate::config_snapshot::current(cx).browser.keeps_history() {
            return;
        }
        let human = matches!(snapshot.owner, TabOwner::Human | TabOwner::Unclaimed);
        let settled = snapshot.state == TabState::Loaded
            && snapshot.failure.is_none()
            && snapshot.held.is_none();
        if !human || !settled || !is_web(&snapshot.url) {
            return;
        }
        let url = snapshot.url.clone();
        let title = snapshot
            .title
            .clone()
            .filter(|title| !title.trim().is_empty() && !is_blank(title));
        match &self.omnibox.recorded {
            Some((recorded, shown)) if *recorded == url => {
                if let Some(title) = title.as_deref()
                    && shown.as_deref() != Some(title)
                {
                    omnibox::retitle(&url, title, cx);
                    self.omnibox.recorded = Some((url, Some(title.to_string())));
                }
            }
            _ => {
                let typed =
                    self.omnibox.typed_url.take().is_some_and(|typed| {
                        omnibox::shown_url(&typed) == omnibox::shown_url(&url)
                    });
                omnibox::record_visit(&url, title.as_deref(), typed, cx);
                self.omnibox.recorded = Some((url, title));
            }
        }
    }

    pub(crate) fn open(&mut self, url: String, cx: &mut Context<Self>) {
        self.failure = None;
        if let Some(tab) = self.tab {
            if let Some(refusal) = address::refusal(&url) {
                self.address_error = Some(refusal);
                cx.notify();
                return;
            }
            self.take_over(cx);
            match crate::browser_control::runtime() {
                Ok(runtime) => drop(runtime.client.navigate(tab, url)),
                Err(reason) => self.failure = Some(OpenFailure::Failed(reason)),
            }
            cx.notify();
            return;
        }
        let Some(profile) = self.profile() else {
            self.failure = Some(OpenFailure::Failed(
                "this workspace has no browser profile".to_string(),
            ));
            cx.notify();
            return;
        };
        if self.opening {
            return;
        }
        if let Err(reason) = router::install(cx) {
            self.failure = Some(OpenFailure::Failed(reason));
            cx.notify();
            return;
        }
        self.opening = true;
        self.shown_url = url.clone();
        let owner = std::mem::replace(&mut self.owner_on_open, TabOwner::Human);
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let opened = smol::unblock(move || {
                let runtime = crate::browser_control::runtime()?;
                runtime
                    .control
                    .open_surface(profile, owner, &url)
                    .map_err(|error| match error.reason {
                        "not_installed" => "not_installed".to_string(),
                        "navigation_refused" | "invalid_params" => {
                            format!("refused:{}", error.message)
                        }
                        _ => error.message,
                    })
            })
            .await;
            let _ = this.update(cx, |view, cx| view.opened(opened, cx));
        })
        .detach();
    }

    fn opened(&mut self, opened: Result<TabId, String>, cx: &mut Context<Self>) {
        self.opening = false;
        match opened {
            Ok(tab) => self.adopt(tab, cx),
            Err(reason) if reason == "not_installed" => {
                self.failure = Some(OpenFailure::NotInstalled);
            }
            Err(reason) => match reason.strip_prefix("refused:") {
                Some(refusal) => self.failure = Some(OpenFailure::Refused(refusal.to_string())),
                None => self.failure = Some(OpenFailure::Failed(reason)),
            },
        }
        cx.emit(BrowserViewEvent::Changed);
        cx.notify();
    }

    fn adopt(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if router::install(cx).is_err() {
            return;
        }
        self.tab = Some(tab);
        router::register(tab, cx.weak_entity(), cx);
        self.refresh(cx);
    }

    pub(crate) fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab.take() {
            router::unregister(tab, &cx.weak_entity(), cx);
            if self.presented.take().is_some()
                && let Ok(runtime) = crate::browser_control::runtime()
            {
                drop(runtime.client.present(tab, None));
                runtime.control.set_presented(tab, false);
            }
        }
    }

    pub(crate) fn close_tab(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            return;
        };
        self.release(cx);
        if let Ok(runtime) = crate::browser_control::runtime() {
            drop(runtime.client.close_tab(tab));
        }
    }

    fn close_unless_agent_owned(&mut self) {
        let Some(tab) = self.tab.take() else {
            return;
        };
        if matches!(self.owner(), Some(TabOwner::Agent { .. })) {
            if self.presented.take().is_some()
                && let Ok(runtime) = crate::browser_control::runtime()
            {
                drop(runtime.client.present(tab, None));
                runtime.control.set_presented(tab, false);
            }
            return;
        }
        if let Ok(runtime) = crate::browser_control::runtime() {
            drop(runtime.client.close_tab(tab));
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            return;
        };
        if self.inspects.is_some() {
            return;
        }
        let snapshot = crate::browser_control::control()
            .ok()
            .and_then(|control| control.view(tab));
        if snapshot.is_none() {
            router::unregister(tab, &cx.weak_entity(), cx);
            self.tab = None;
            self.presented = None;
        }
        self.drop_inspector_unless(self.tab);
        let url = snapshot
            .as_ref()
            .map(|s| s.url.clone())
            .filter(|url| !is_blank(url))
            .unwrap_or_default();
        let editing = self.address.read(cx).focused_since().is_some();
        if !editing && url != self.shown_url {
            self.shown_url = url.clone();
            self.show_in_field(url, false, cx);
        }
        if snapshot.is_some() && snapshot == self.snapshot {
            return;
        }
        self.track_loading(snapshot.as_ref());
        let before = self.persisted();
        if zoom_resets(self.snapshot.as_ref(), snapshot.as_ref()) {
            self.zoomed = None;
        }
        let left = snapshot.as_ref().is_some_and(|after| {
            self.snapshot
                .as_ref()
                .is_none_or(|before| before.tab != after.tab || (after.loading && !before.loading))
        });
        self.snapshot = snapshot;
        if left {
            self.annotation_page_left(cx);
        }
        self.drop_site_info_for(&self.url());
        self.apply_zoom(cx);
        self.refresh_security(cx);
        self.record_history(cx);
        if self.persisted() != before {
            cx.emit(BrowserViewEvent::Persist);
        }
        cx.emit(BrowserViewEvent::Changed);
        cx.notify();
    }

    fn track_loading(&mut self, next: Option<&TabView>) {
        let now = std::time::Instant::now();
        match loading_step(self.snapshot.as_ref(), next) {
            LoadingStep::Begin(progress) => self.loader.begin(now, progress),
            LoadingStep::Advance(progress) => self.loader.advance(now, progress),
            LoadingStep::Finish => self.loader.finish(now),
            LoadingStep::Hide => self.loader.reset(),
            LoadingStep::Keep => {}
        }
    }

    pub(crate) fn surface_event(&mut self, event: SurfaceEvent, cx: &mut Context<Self>) {
        match event {
            SurfaceEvent::Changed(_) => self.refresh(cx),
            SurfaceEvent::Closed(_) => {
                self.refresh(cx);
                self.image = None;
                self.backing = None;
            }
            SurfaceEvent::Cursor { cursor, .. } => {
                let style = cursor_style(cursor);
                if style != self.cursor {
                    self.cursor = style;
                    cx.notify();
                }
            }
            SurfaceEvent::ImeCaret { caret, .. } => self.caret = Some(caret),
            SurfaceEvent::Dialog { dialog, .. } => {
                let text = (dialog.kind == paneflow_browser_proto::surface::DialogKind::Prompt)
                    .then(|| {
                        let initial = dialog.default_text.clone();
                        cx.new(|cx| TextInput::new(initial, "", cx).with_accessible_name("Answer"))
                    });
                self.dialog = Some(OpenDialog { dialog, text });
                cx.notify();
            }
            SurfaceEvent::FileChooser { chooser, .. } => self.choose_files(chooser, cx),
            SurfaceEvent::Opened { tab, .. } => cx.emit(BrowserViewEvent::Opened(tab)),
            SurfaceEvent::Annotation {
                context, payload, ..
            } => self.annotation_message(context, &payload, cx),
            SurfaceEvent::Host => self.refresh(cx),
        }
    }

    pub(crate) fn answer_dialog(&mut self, accept: bool, cx: &mut Context<Self>) {
        let (Some(tab), Some(open)) = (self.tab, self.dialog.take()) else {
            return;
        };
        let text = open
            .text
            .as_ref()
            .filter(|_| accept)
            .map(|input| input.read(cx).value());
        if let Ok(runtime) = crate::browser_control::runtime() {
            drop(
                runtime
                    .client
                    .answer_dialog(tab, open.dialog.dialog, accept, text),
            );
        }
        cx.notify();
    }

    fn choose_files(
        &mut self,
        chooser: paneflow_browser_control::FileChooser,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tab else {
            return;
        };
        let request = chooser.request;
        let picked = match chooser.mode {
            FileMode::Save => {
                let directory = dirs::download_dir()
                    .or_else(dirs::home_dir)
                    .unwrap_or_default();
                let chosen = cx.prompt_for_new_path(&directory, None);
                cx.spawn(async move |_, _| {
                    chosen
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .flatten()
                        .map(|path| vec![path])
                })
            }
            mode => {
                let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: mode != FileMode::OpenFolder,
                    directories: mode == FileMode::OpenFolder,
                    multiple: mode == FileMode::OpenMultiple,
                    prompt: None,
                });
                cx.spawn(async move |_, _| chosen.await.ok().and_then(Result::ok).flatten())
            }
        };
        cx.spawn(async move |_, _| {
            let paths = picked.await.map(|paths| {
                paths
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            });
            if let Ok(runtime) = crate::browser_control::runtime() {
                drop(runtime.client.answer_files(tab, request, paths));
            }
        })
        .detach();
    }

    pub(crate) fn frame_ready(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            return;
        };
        let Ok(runtime) = crate::browser_control::runtime() else {
            return;
        };
        let Some(frame) = runtime.client.take_frame(tab) else {
            return;
        };
        self.backing = corner_color(&frame.pixels);
        let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        if let Some(old) = self.image.replace(image) {
            self.retired.push(old);
        }
        self.frame_size = (frame.width, frame.height);
        cx.notify();
    }

    pub(crate) fn place(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        self.page_bounds = Some(bounds);
        let Some(tab) = self.tab else {
            return;
        };
        if !self.displayed {
            return;
        }
        let width = f32::from(bounds.size.width).floor().max(1.) as u32;
        let height = f32::from(bounds.size.height).floor().max(1.) as u32;
        let view = View::new(width, height, scale);
        if self.presented == Some(view) {
            return;
        }
        let Ok(runtime) = crate::browser_control::runtime() else {
            return;
        };
        let first = self.presented.is_none();
        self.presented = Some(view);
        drop(runtime.client.present(tab, Some(view)));
        if first {
            runtime.control.set_presented(tab, true);
        }
    }

    pub(crate) fn sync_focus(&mut self, window: &Window) {
        let focused = self.focus.is_focused(window);
        if focused == self.page_focused {
            return;
        }
        self.page_focused = focused;
        if !focused {
            self.composing = None;
        }
        self.send(Input::Focus { focused });
    }

    fn send(&self, input: Input) {
        if let (Some(tab), Ok(runtime)) = (self.tab, crate::browser_control::runtime()) {
            runtime.client.input(tab, input);
        }
    }

    pub(crate) fn take_over(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            return;
        };
        let human = matches!(self.owner(), Some(TabOwner::Human));
        if human {
            return;
        }
        if let Ok(control) = crate::browser_control::control()
            && control.take_over(tab)
        {
            self.refresh(cx);
        }
    }

    pub(crate) fn hand_to(&mut self, choice: &AgentChoice, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(tab) = self.tab else {
            return;
        };
        if let Ok(control) = crate::browser_control::control() {
            control.hand_to(tab, choice.session.clone(), Some(choice.label.clone()));
        }
        self.refresh(cx);
    }

    pub(crate) fn history(&mut self, step: HistoryStep, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            return;
        };
        self.take_over(cx);
        if let Ok(runtime) = crate::browser_control::runtime() {
            drop(runtime.client.history(tab, step));
        }
    }

    pub(crate) fn reload_or_stop(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            let url = self.url();
            if !url.is_empty() {
                self.open(url, cx);
            }
            return;
        };
        self.take_over(cx);
        let loading = self.snapshot.as_ref().is_some_and(|s| s.loading);
        if let Ok(runtime) = crate::browser_control::runtime() {
            let engine = &runtime.client;
            let ticket = if loading {
                engine.stop(tab)
            } else {
                engine.reload(tab)
            };
            drop(ticket);
        }
    }

    pub(crate) fn toggle_menu(&mut self, menu: BrowserMenu, cx: &mut Context<Self>) {
        if self.menu == Some(menu) {
            self.menu = None;
        } else {
            if menu == BrowserMenu::Pointer {
                cx.emit(BrowserViewEvent::WantsAgents);
            }
            if menu == BrowserMenu::Tabs {
                self.listed = match (crate::browser_control::control(), self.profile()) {
                    (Ok(control), Some(profile)) => control.views(&profile),
                    _ => Vec::new(),
                };
            }
            self.menu = Some(menu);
        }
        cx.notify();
    }

    pub(crate) fn show_listed(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.menu = None;
        cx.emit(BrowserViewEvent::Show(tab));
        cx.notify();
    }

    fn page_point(&self, position: Point<Pixels>) -> (i32, i32) {
        let origin = self.page_bounds.map_or(Point::default(), |b| b.origin);
        (
            f32::from(position.x - origin.x).round() as i32,
            f32::from(position.y - origin.y).round() as i32,
        )
    }

    pub(crate) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        if !self.takes_input {
            return;
        }
        window.focus(&self.focus, cx);
        self.take_over(cx);
        self.pressed = Some(event.button);
        let (x, y) = self.page_point(event.position);
        if let Some(button) = page_button(event.button) {
            self.send(Input::MouseButton {
                x,
                y,
                button,
                pressed: true,
                clicks: u32::try_from(event.click_count).unwrap_or(1),
                modifiers: keys::pointer_modifiers(&event.modifiers, self.pressed),
            });
        }
        cx.stop_propagation();
    }

    pub(crate) fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.pressed.take().is_none() {
            return;
        }
        let (x, y) = self.page_point(event.position);
        if let Some(button) = page_button(event.button) {
            self.send(Input::MouseButton {
                x,
                y,
                button,
                pressed: false,
                clicks: u32::try_from(event.click_count).unwrap_or(1),
                modifiers: keys::modifiers(&event.modifiers),
            });
        }
        cx.stop_propagation();
    }

    pub(crate) fn mouse_move(&mut self, event: &MouseMoveEvent, inside: bool) {
        let (x, y) = self.page_point(event.position);
        let modifiers = keys::pointer_modifiers(&event.modifiers, self.pressed);
        if (inside && self.takes_input) || self.pressed.is_some() {
            self.send(Input::MouseMove { x, y, modifiers });
        }
    }

    pub(crate) fn mouse_left(&mut self) {
        if self.pressed.is_none() {
            self.send(Input::MouseLeave {
                x: -1,
                y: -1,
                modifiers: Modifiers::default(),
            });
        }
    }

    pub(crate) fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if !self.takes_input {
            return;
        }
        let (x, y) = self.page_point(event.position);
        let (delta_x, delta_y) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(delta) => (delta.x * LINE_PIXELS, delta.y * LINE_PIXELS),
        };
        self.send(Input::Wheel {
            x,
            y,
            delta_x: delta_x.round() as i32,
            delta_y: delta_y.round() as i32,
            modifiers: keys::modifiers(&event.modifiers),
        });
        cx.stop_propagation();
    }

    pub(crate) fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.tab.is_none() || !self.takes_input {
            return;
        }
        self.take_over(cx);
        let Some(down) = keys::key_down(&event.keystroke, event.is_held) else {
            return;
        };
        for input in down.events {
            self.send(input);
        }
        if down.consumed {
            cx.stop_propagation();
        }
    }

    pub(crate) fn key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        if !self.takes_input {
            return;
        }
        if let Some(input) = keys::key_up(&event.keystroke) {
            self.send(input);
            cx.stop_propagation();
        }
    }

    pub(crate) fn drop_retired(&mut self, window: &mut Window) {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
    }

    pub(crate) fn page_state(&self) -> PageState {
        if self.inspects.is_some() {
            return PageState::Page;
        }
        page_state(
            self.failure.as_ref(),
            self.snapshot.as_ref(),
            self.opening,
            self.dormant.is_some(),
            self.tab.is_some(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PageState {
    Empty,
    Opening,
    NotInstalled,
    Refused(String),
    Failed(String),
    Crashed(String),
    PresentationFailed(String),
    Page,
}

impl PageState {
    pub(crate) fn takes_input(&self) -> bool {
        *self == Self::Page
    }
}

fn page_state(
    failure: Option<&OpenFailure>,
    snapshot: Option<&TabView>,
    opening: bool,
    dormant: bool,
    attached: bool,
) -> PageState {
    match failure {
        Some(OpenFailure::NotInstalled) => return PageState::NotInstalled,
        Some(OpenFailure::Refused(reason)) if !attached => {
            return PageState::Refused(reason.clone());
        }
        Some(OpenFailure::Failed(reason)) if !attached => return PageState::Failed(reason.clone()),
        _ => {}
    }
    let Some(snapshot) = snapshot else {
        return if opening || dormant {
            PageState::Opening
        } else {
            PageState::Empty
        };
    };
    if let Some(crash) = &snapshot.crash {
        return PageState::Crashed(crash.clone());
    }
    if let Some(reason) = &snapshot.presentation_failure {
        return PageState::PresentationFailed(reason.clone());
    }
    PageState::Page
}

fn zoom_resets(before: Option<&TabView>, after: Option<&TabView>) -> bool {
    let Some(after) = after else {
        return false;
    };
    let arrived = after.state == TabState::Loaded
        && before.is_none_or(|before| before.state != TabState::Loaded || before.url != after.url);
    let started = after.loading && before.is_none_or(|before| !before.loading);
    arrived || started
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LoadingStep {
    Begin(f32),
    Advance(f32),
    Finish,
    Hide,
    Keep,
}

fn loading_step(before: Option<&TabView>, after: Option<&TabView>) -> LoadingStep {
    let Some(after) = after.filter(|view| is_web(&view.url)) else {
        return LoadingStep::Hide;
    };
    let progress = f32::from(after.load_progress) / 1000.;
    let was_loading = before.is_some_and(|view| view.loading && view.tab == after.tab);
    let restarted = before.is_some_and(|view| after.load_progress < view.load_progress);
    match (was_loading, after.loading) {
        (false, true) => LoadingStep::Begin(progress),
        (true, true) if restarted => LoadingStep::Begin(progress),
        (true, true) => LoadingStep::Advance(progress),
        (true, false) => LoadingStep::Finish,
        (false, false) => LoadingStep::Keep,
    }
}

pub(crate) fn is_web(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn is_blank(text: &str) -> bool {
    text.trim() == "about:blank"
}

fn corner_color(bgra: &[u8]) -> Option<gpui::Rgba> {
    let [blue, green, red, alpha] = *bgra.first_chunk::<4>()?;
    if alpha != u8::MAX {
        return None;
    }
    Some(gpui::rgb(
        (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue),
    ))
}

pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| host.to_string())
}

fn page_button(button: MouseButton) -> Option<PageButton> {
    match button {
        MouseButton::Left => Some(PageButton::Left),
        MouseButton::Middle => Some(PageButton::Middle),
        MouseButton::Right => Some(PageButton::Right),
        _ => None,
    }
}

pub(crate) fn cursor_style(cursor: Cursor) -> CursorStyle {
    match cursor {
        Cursor::Pointer => CursorStyle::PointingHand,
        Cursor::Text => CursorStyle::IBeam,
        Cursor::Crosshair => CursorStyle::Crosshair,
        Cursor::Move | Cursor::Grab => CursorStyle::OpenHand,
        Cursor::Grabbing => CursorStyle::ClosedHand,
        Cursor::NotAllowed | Cursor::NoDrop => CursorStyle::OperationNotAllowed,
        Cursor::ResizeColumn => CursorStyle::ResizeColumn,
        Cursor::ResizeRow => CursorStyle::ResizeRow,
        Cursor::ResizeEast => CursorStyle::ResizeRight,
        Cursor::ResizeWest => CursorStyle::ResizeLeft,
        Cursor::ResizeNorth => CursorStyle::ResizeUp,
        Cursor::ResizeSouth => CursorStyle::ResizeDown,
        Cursor::ResizeEastWest => CursorStyle::ResizeLeftRight,
        Cursor::ResizeNorthSouth => CursorStyle::ResizeUpDown,
        Cursor::ResizeNorthEastSouthWest => CursorStyle::ResizeUpRightDownLeft,
        Cursor::ResizeNorthWestSouthEast => CursorStyle::ResizeUpLeftDownRight,
        Cursor::ContextMenu => CursorStyle::ContextualMenu,
        Cursor::Copy => CursorStyle::DragCopy,
        Cursor::Alias => CursorStyle::DragLink,
        Cursor::Default
        | Cursor::Wait
        | Cursor::Progress
        | Cursor::Help
        | Cursor::ZoomIn
        | Cursor::ZoomOut
        | Cursor::None => CursorStyle::Arrow,
    }
}

impl Focusable for BrowserView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

impl EntityInputHandler for BrowserView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.composing.clone().unwrap_or_default();
        let units: Vec<u16> = text.encode_utf16().collect();
        let end = range.end.min(units.len());
        let start = range.start.min(end);
        *adjusted_range = Some(start..end);
        String::from_utf16(&units[start..end]).ok()
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.composing.as_deref().map_or(0, utf16_len);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.composing.as_deref().map(|text| 0..utf16_len(text))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.composing.take().is_some() {
            self.send(Input::ImeFinish);
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.take_over(cx);
        if self.composing.take().is_some() {
            self.send(Input::ImeCommit {
                text: text.to_string(),
            });
            return;
        }
        for input in keys::typed(text, Modifiers::default()) {
            self.send(input);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.take_over(cx);
        if new_text.is_empty() {
            if self.composing.take().is_some() {
                self.send(Input::ImeCancel);
            }
            return;
        }
        let length = utf16_len(new_text);
        let selection = new_selected_range.unwrap_or(length..length);
        self.composing = Some(new_text.to_string());
        self.send(Input::ImeSetComposition {
            text: new_text.to_string(),
            selection_start: u32::try_from(selection.start).unwrap_or(0),
            selection_end: u32::try_from(selection.end).unwrap_or(0),
        });
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let caret = self.caret?;
        Some(Bounds {
            origin: element_bounds.origin + point(px(caret.x as f32), px(caret.y as f32)),
            size: size(
                px(caret.width.max(1) as f32),
                px(caret.height.max(1) as f32),
            ),
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.tab.is_some() && self.takes_input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::methods::browser::TabState;

    fn snapshot() -> TabView {
        TabView {
            tab: TabId(1),
            workspace: ProfileName::new("w-1").unwrap(),
            url: "http://127.0.0.1:3000/".to_string(),
            title: None,
            state: TabState::Loaded,
            owner: TabOwner::Human,
            loading: false,
            load_progress: 0,
            can_go_back: false,
            can_go_forward: false,
            held: None,
            refusal: None,
            failure: None,
            crash: None,
            presentation_failure: None,
            taken_over_from: None,
            blocked: Vec::new(),
        }
    }

    #[test]
    fn the_zoom_is_sent_again_for_each_page_that_starts_or_arrives() {
        let loading = TabView {
            loading: true,
            state: TabState::Loading,
            ..snapshot()
        };
        let loaded = snapshot();
        assert!(zoom_resets(None, Some(&loaded)));
        assert!(zoom_resets(Some(&loaded), Some(&loading)));
        assert!(zoom_resets(Some(&loading), Some(&loaded)));
        assert!(!zoom_resets(Some(&loaded), Some(&loaded)));
        let moved = TabView {
            url: "http://127.0.0.1:3000/next".to_string(),
            ..snapshot()
        };
        assert!(zoom_resets(Some(&loaded), Some(&moved)));
        assert!(!zoom_resets(Some(&loaded), None));
    }

    #[test]
    fn each_failure_and_hold_has_its_own_page_state() {
        assert_eq!(
            page_state(None, None, false, false, false),
            PageState::Empty
        );
        assert_eq!(
            page_state(None, None, true, false, false),
            PageState::Opening
        );
        assert_eq!(
            page_state(Some(&OpenFailure::NotInstalled), None, false, false, false),
            PageState::NotInstalled
        );
        let mut held = snapshot();
        held.held = Some(paneflow_browser_control::HeldSite {
            origin: "https://example.com".to_string(),
            url: "https://example.com/".to_string(),
        });
        assert_eq!(
            page_state(None, Some(&held), false, false, true),
            PageState::Page,
            "a site an agent waits on keeps the page shown; the strip names it"
        );
        let mut crashed = snapshot();
        crashed.crash = Some("the renderer was killed".to_string());
        assert_eq!(
            page_state(None, Some(&crashed), false, false, true),
            PageState::Crashed("the renderer was killed".to_string())
        );
        let mut unshared = snapshot();
        unshared.presentation_failure = Some("no shared memory".to_string());
        assert_eq!(
            page_state(None, Some(&unshared), false, false, true),
            PageState::PresentationFailed("no shared memory".to_string())
        );
        assert_eq!(
            page_state(None, Some(&snapshot()), false, false, true),
            PageState::Page
        );
    }

    #[test]
    fn a_page_cursor_maps_to_the_window_cursor() {
        assert_eq!(cursor_style(Cursor::Pointer), CursorStyle::PointingHand);
        assert_eq!(cursor_style(Cursor::Text), CursorStyle::IBeam);
        assert_eq!(cursor_style(Cursor::Default), CursorStyle::Arrow);
    }

    #[test]
    fn the_page_backing_is_the_frames_top_left_pixel_read_as_bgra() {
        assert_eq!(corner_color(&[]), None);
        assert_eq!(corner_color(&[0x10, 0x20, 0x30, 0x00]), None);
        assert_eq!(
            corner_color(&[0x10, 0x20, 0x30, 0xff, 0, 0, 0, 0]),
            Some(gpui::rgb(0x302010))
        );
    }

    #[test]
    fn loading_begins_advances_restarts_and_finishes_on_web_pages_only() {
        let page = |loading: bool, load_progress: u16| TabView {
            loading,
            load_progress,
            ..snapshot()
        };
        assert_eq!(
            loading_step(Some(&page(false, 0)), Some(&page(true, 100))),
            LoadingStep::Begin(0.1)
        );
        assert_eq!(
            loading_step(Some(&page(true, 100)), Some(&page(true, 600))),
            LoadingStep::Advance(0.6)
        );
        assert_eq!(
            loading_step(Some(&page(true, 600)), Some(&page(true, 0))),
            LoadingStep::Begin(0.)
        );
        assert_eq!(
            loading_step(Some(&page(true, 900)), Some(&page(false, 1000))),
            LoadingStep::Finish
        );
        let mut file = page(true, 100);
        file.url = "about:blank".to_string();
        assert_eq!(loading_step(None, Some(&file)), LoadingStep::Hide);
        assert_eq!(
            loading_step(Some(&page(true, 100)), None),
            LoadingStep::Hide
        );
    }

    #[gpui::test]
    fn a_click_on_the_shown_address_keeps_the_field_focused_and_editable(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            BrowserView::restored(1, "http://127.0.0.1:3000/app".to_string(), None, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(800.), gpui::px(400.)));
        cx.run_until_parked();
        let address = view.read_with(cx, |view, cx| view.address_focus(cx));
        cx.update(|window, cx| window.focus(&address, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(address.is_focused(window), "the field kept the focus");
            assert!(
                view.read(cx).address.read(cx).focused_since().is_some(),
                "the text input was drawn in place of the shown address"
            );
        });
        cx.simulate_input("x");
        cx.run_until_parked();
        let value = view.read_with(cx, |view, cx| view.address.read(cx).value());
        assert!(value.ends_with('x'), "typing reaches the field: {value:?}");
    }

    #[gpui::test]
    fn the_zoom_keys_give_the_site_its_own_zoom_and_reset_brings_the_default_back(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.bind_keys([
                gpui::KeyBinding::new("ctrl-=", crate::FontSizeIncrease, None),
                gpui::KeyBinding::new("ctrl--", crate::FontSizeDecrease, None),
                gpui::KeyBinding::new("ctrl-0", crate::FontSizeReset, None),
            ]);
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = BrowserView::new(1, cx);
            view.snapshot = Some(TabView {
                url: "https://Vercel.com/strive-x".to_string(),
                ..snapshot()
            });
            view
        });
        cx.simulate_resize(gpui::size(gpui::px(800.), gpui::px(400.)));
        let focus = view.read_with(cx, |view, _| view.focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.run_until_parked();
        let zoom =
            |cx: &mut gpui::VisualTestContext| view.read_with(cx, |view, cx| view.site_zoom(cx));
        assert_eq!(zoom(cx), Some(("vercel.com".to_string(), 100, 100)));
        cx.simulate_keystrokes("ctrl-= ctrl-=");
        cx.run_until_parked();
        assert_eq!(zoom(cx), Some(("vercel.com".to_string(), 125, 100)));
        assert!(
            cx.debug_bounds("browser-zoom-125-of-100").is_some(),
            "the field shows a zoom off the default"
        );
        cx.simulate_keystrokes("ctrl--");
        cx.run_until_parked();
        assert_eq!(zoom(cx), Some(("vercel.com".to_string(), 110, 100)));
        cx.simulate_keystrokes("ctrl-0");
        cx.run_until_parked();
        assert_eq!(zoom(cx), Some(("vercel.com".to_string(), 100, 100)));
        assert!(cx.update(|_, cx| zoom::snapshot(cx).list().is_empty()));
        assert!(cx.debug_bounds("browser-zoom-100-of-100").is_none());
    }

    #[gpui::test]
    fn typing_a_known_prefix_completes_it_inline_and_deleting_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::widgets::text_input::register_keybindings(cx);
            for _ in 0..3 {
                omnibox::record_visit("https://vercel.com/", Some("Vercel"), true, cx);
            }
        });
        let (view, cx) = cx.add_window_view(|_, cx| BrowserView::new(1, cx));
        cx.simulate_resize(gpui::size(gpui::px(800.), gpui::px(400.)));
        cx.run_until_parked();
        let address = view.read_with(cx, |view, cx| view.address_focus(cx));
        cx.update(|window, cx| window.focus(&address, cx));
        cx.run_until_parked();
        cx.simulate_input("verc");
        cx.run_until_parked();
        let (value, selection, open) = view.read_with(cx, |view, cx| {
            let input = view.address.read(cx);
            (input.value(), input.selection(), view.omnibox.open)
        });
        assert_eq!(value, "vercel.com");
        assert_eq!(selection, 4..10, "the completed part is selected");
        assert!(open);
        cx.simulate_keystrokes("backspace");
        cx.run_until_parked();
        let value = view.read_with(cx, |view, cx| view.address.read(cx).value());
        assert_eq!(
            value, "verc",
            "deleting the completion does not bring it back"
        );
        cx.simulate_input("e");
        cx.run_until_parked();
        let value = view.read_with(cx, |view, cx| view.address.read(cx).value());
        assert_eq!(value, "vercel.com");
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        let value = view.read_with(cx, |view, cx| view.address.read(cx).value());
        assert_eq!(
            value, "verce",
            "the search row fills the field with the query"
        );
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        let (value, selection) = view.read_with(cx, |view, cx| {
            let input = view.address.read(cx);
            (input.value(), input.selection())
        });
        assert_eq!((value.as_str(), selection), ("vercel.com", 5..10));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let (value, open) = view.read_with(cx, |view, cx| {
            (view.address.read(cx).value(), view.omnibox.open)
        });
        assert_eq!((value.as_str(), open), ("verce", false));
    }

    #[test]
    fn the_host_is_the_short_title_of_an_untitled_page() {
        assert_eq!(
            host_of("http://localhost:3000/a").as_deref(),
            Some("localhost:3000")
        );
        assert_eq!(host_of("about:blank"), None);
    }
}

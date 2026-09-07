use gpui::{AppContext, Context, Entity, Window};
use paneflow_browser_protocol::{
    BrowserError, BrowserId, Command, Document, Event, MAX_BROWSERS_PER_SESSION, ProfileId,
    normalize_address, validate_url, zoom_percent_is_valid,
};
use paneflow_config::schema::{BROWSER_DESCRIPTOR_VERSION, BrowserDescriptor};

use crate::PaneFlowApp;
use crate::app::cli_diff_dock::DiffDockSlot;
use crate::app::diff_dock::DiffDockTab;
use crate::browser::authority::{BrowserAuthority, refusal_message};
use crate::browser::profile::allocate_profile_id;
use crate::browser::view::{BrowserOwner, BrowserView, BrowserViewEvent};
use crate::terminal::TerminalView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BrowserSessionRef {
    pub(crate) workspace_id: u64,
    pub(crate) tab_id: u64,
}

pub(crate) struct RestoredBrowserTab {
    pub(crate) workspace_id: u64,
    pub(crate) tab_id: u64,
    pub(crate) descriptors: Vec<BrowserDescriptor>,
}

pub(crate) fn validate_descriptors(
    descriptors: &[BrowserDescriptor],
) -> (Vec<BrowserDescriptor>, Vec<String>) {
    let mut kept: Vec<BrowserDescriptor> = Vec::new();
    let mut ignored = Vec::new();
    for descriptor in descriptors {
        if descriptor.version > BROWSER_DESCRIPTOR_VERSION {
            ignored.push(format!(
                "{}: descriptor version {} is newer than {}",
                descriptor.id, descriptor.version, BROWSER_DESCRIPTOR_VERSION
            ));
            continue;
        }
        if BrowserId::try_from(descriptor.id.clone()).is_err() {
            ignored.push(format!("{:?}: invalid identity", descriptor.id));
            continue;
        }
        if kept.iter().any(|existing| existing.id == descriptor.id) {
            ignored.push(format!("{}: duplicate identity", descriptor.id));
            continue;
        }
        if let Some(url) = &descriptor.url
            && validate_url(url).is_err()
        {
            ignored.push(format!("{}: inadmissible URL", descriptor.id));
            continue;
        }
        if kept.len() >= MAX_BROWSERS_PER_SESSION {
            ignored.push(format!(
                "{}: above the {MAX_BROWSERS_PER_SESSION} descriptors per session",
                descriptor.id
            ));
            continue;
        }
        let mut descriptor = descriptor.clone();
        descriptor.version = BROWSER_DESCRIPTOR_VERSION;
        descriptor.title = crate::browser::view::bounded_title(&descriptor.title);
        if !zoom_percent_is_valid(descriptor.zoom) {
            descriptor.zoom = 100;
        }
        kept.push(descriptor);
    }
    (kept, ignored)
}

pub(crate) fn new_browser_id() -> BrowserId {
    BrowserId::try_from(format!("b-{}", uuid::Uuid::new_v4().simple()))
        .unwrap_or_else(|_| unreachable!("uuid simple form is alphanumeric"))
}

impl PaneFlowApp {
    pub(crate) fn browser_available(&self, cx: &gpui::App) -> bool {
        BrowserAuthority::available(cx)
    }

    fn workspace_browser_profile(
        &mut self,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> Option<ProfileId> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|ws| ws.id == workspace_id)?;
        if let Some(existing) = ws
            .browser_profile
            .clone()
            .and_then(|id| ProfileId::try_from(id).ok())
        {
            return Some(existing);
        }
        let profile = allocate_profile_id();
        ws.browser_profile = Some(profile.as_str().to_string());
        self.save_session(cx);
        Some(profile)
    }

    fn resolve_browser_session(&self, session: BrowserSessionRef) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|ws| ws.id == session.workspace_id)
            .filter(|idx| {
                self.workspaces[*idx]
                    .tabs()
                    .iter()
                    .any(|tab| tab.id == session.tab_id)
            })
    }

    pub(crate) fn active_browser_session(&self) -> Option<BrowserSessionRef> {
        let ws = self.active_workspace()?;
        Some(BrowserSessionRef {
            workspace_id: ws.id,
            tab_id: ws.active_tab().id,
        })
    }

    pub(crate) fn browser_session_for_terminal(
        &self,
        terminal: &Entity<TerminalView>,
        cx: &gpui::App,
    ) -> Option<BrowserSessionRef> {
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                if pane.read(cx).contains_terminal(terminal)
                    && let Some(tab) = ws.tab_for_pane(&pane)
                {
                    return Some(BrowserSessionRef {
                        workspace_id: ws.id,
                        tab_id: tab.id,
                    });
                }
            }
        }
        let in_dock = self
            .diff_dock
            .diff_tabs
            .iter()
            .any(|tab| matches!(tab, DiffDockTab::Terminal(dock) if dock == terminal));
        if in_dock {
            let tab_id = self.diff_dock.owner?;
            let ws = self
                .workspaces
                .iter()
                .find(|ws| ws.tabs().iter().any(|tab| tab.id == tab_id))?;
            return Some(BrowserSessionRef {
                workspace_id: ws.id,
                tab_id,
            });
        }
        None
    }

    fn create_browser_view(
        &mut self,
        session: BrowserSessionRef,
        descriptor: BrowserDescriptor,
        cx: &mut Context<Self>,
    ) -> Result<Entity<BrowserView>, String> {
        let profile = self
            .workspace_browser_profile(session.workspace_id, cx)
            .ok_or_else(|| "This session no longer exists".to_string())?;
        let id = BrowserId::try_from(descriptor.id.clone())
            .map_err(|error| format!("invalid browser identity: {error}"))?;
        let owner = BrowserOwner {
            workspace_id: session.workspace_id,
            tab_id: session.tab_id,
            profile: profile.clone(),
        };
        let scope = owner.scope();
        if !cx.has_global::<BrowserAuthority>() {
            return Err(refusal_message(BrowserError::Unavailable));
        }
        let event = cx
            .global_mut::<BrowserAuthority>()
            .dispatch(
                &scope,
                Command::Create {
                    owner: scope.clone(),
                    browser: id.clone(),
                    profile,
                    url: descriptor
                        .url
                        .clone()
                        .unwrap_or_else(|| paneflow_browser_protocol::BLANK_URL.to_string()),
                    title: descriptor.title.clone(),
                },
            )
            .map_err(refusal_message)?;
        let local_document: Document = match event {
            Event::State { session } => session.document,
            _ => return Err(refusal_message(BrowserError::InvalidMessage)),
        };
        let view = cx.new(|cx| BrowserView::new(owner, id, local_document, &descriptor, cx));
        cx.subscribe(&view, |this, _view, event: &BrowserViewEvent, cx| {
            if matches!(event, BrowserViewEvent::DescriptorChanged) {
                this.save_session(cx);
            }
        })
        .detach();
        Ok(view)
    }

    pub(crate) fn open_diff_browser_tab(
        &mut self,
        url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_browser_session() else {
            return;
        };
        let descriptor = BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: new_browser_id().as_str().to_string(),
            url,
            title: String::new(),
            zoom: 100,
            muted: false,
            active: true,
        };
        match self.create_browser_view(session, descriptor, cx) {
            Ok(view) => {
                self.diff_dock.diff_tabs.push(DiffDockTab::Browser(view));
                let index = self.diff_dock.diff_tabs.len() - 1;
                self.select_diff_tab(index, cx);
                self.focus_diff_tab(index, window, cx);
                self.save_session(cx);
            }
            Err(message) => self.show_toast(message, cx),
        }
    }

    pub(crate) fn open_url_in_browser(
        &mut self,
        session: BrowserSessionRef,
        url: &str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(ws_idx) = self.resolve_browser_session(session) else {
            return Err("This session no longer exists; the link was not opened".to_string());
        };
        if !self.browser_available(cx) {
            return crate::external_open::open_url(url).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "Could not open URL - install xdg-utils (Linux), or check your default browser"
                        .to_string()
                } else {
                    format!("Could not open URL: {error}")
                }
            });
        }
        let url = normalize_address(url).map_err(refusal_message)?;
        let descriptor = BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: new_browser_id().as_str().to_string(),
            url: Some(url.clone()),
            title: String::new(),
            zoom: 100,
            muted: false,
            active: true,
        };
        if self.active_browser_session() == Some(session) {
            let cwd = self
                .workspaces
                .get(ws_idx)
                .and_then(|ws| {
                    ws.tabs()
                        .iter()
                        .find(|tab| tab.id == session.tab_id)
                        .and_then(|tab| tab.worktree.as_ref())
                        .map(|path| path.to_string_lossy().into_owned())
                        .or_else(|| (!ws.cwd.is_empty()).then(|| ws.cwd.clone()))
                })
                .unwrap_or_default();
            self.diff_dock.picker = false;
            self.diff_dock.picked = true;
            self.open_diff_dock_panel(cwd, cx);
            let existing = self.diff_dock.diff_tabs.iter().position(|tab| {
                matches!(tab, DiffDockTab::Browser(view) if view.read(cx).url() == Some(url.as_str()))
            });
            let index = match existing {
                Some(index) => index,
                None => {
                    let view = self.create_browser_view(session, descriptor, cx)?;
                    self.diff_dock.diff_tabs.push(DiffDockTab::Browser(view));
                    self.diff_dock.diff_tabs.len() - 1
                }
            };
            self.select_diff_tab(index, cx);
            match window {
                Some(window) => self.focus_diff_tab(index, window, cx),
                None => {
                    if let Some(DiffDockTab::Browser(view)) = self.diff_dock.diff_tabs.get(index) {
                        view.update(cx, |view, cx| view.request_wake(cx));
                    }
                }
            }
            self.save_session(cx);
            return Ok(());
        }
        let view = self.create_browser_view(session, descriptor, cx)?;
        match self.diff_dock.parked.get_mut(&session.tab_id) {
            Some(slot) => slot.push_browser(view),
            None => {
                self.diff_dock
                    .parked
                    .insert(session.tab_id, DiffDockSlot::with_browser(view));
            }
        }
        self.save_session(cx);
        Ok(())
    }

    pub(crate) fn open_url_from_terminal(
        &mut self,
        terminal: &Entity<TerminalView>,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        let session = self.browser_session_for_terminal(terminal, cx);
        let outcome = match session {
            Some(session) => self.open_url_in_browser(session, url, None, cx),
            None => Err("No session owns this terminal; the link was not opened".to_string()),
        };
        if let Err(message) = outcome {
            log::warn!("terminal: open URL failed: {message}");
            self.show_toast(message, cx);
        }
    }

    pub(crate) fn open_workspace_service_url(
        &mut self,
        workspace_idx: usize,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        let session = self
            .workspaces
            .get(workspace_idx)
            .map(|ws| BrowserSessionRef {
                workspace_id: ws.id,
                tab_id: ws.active_tab().id,
            });
        let outcome = match session {
            Some(session) => self.open_url_in_browser(session, url, None, cx),
            None => Err("This workspace no longer exists".to_string()),
        };
        if let Err(message) = outcome {
            log::warn!("sidebar: open URL failed: {message}");
            self.show_toast(message, cx);
        }
    }

    pub(crate) fn browser_descriptors_for_tab(
        &self,
        tab_id: u64,
        cx: &gpui::App,
    ) -> Vec<BrowserDescriptor> {
        if self.diff_dock.owner == Some(tab_id) {
            return browser_descriptors(
                &self.diff_dock.diff_tabs,
                self.diff_dock.diff_active_tab,
                cx,
            );
        }
        self.diff_dock
            .parked
            .get(&tab_id)
            .map(|slot| slot.browser_descriptors(cx))
            .unwrap_or_default()
    }

    pub(crate) fn restore_browser_tabs(
        &mut self,
        restored: Vec<RestoredBrowserTab>,
        cx: &mut Context<Self>,
    ) {
        let mut ignored_total = 0;
        for tab in restored {
            let (kept, ignored) = validate_descriptors(&tab.descriptors);
            for reason in &ignored {
                log::warn!("session restore: browser descriptor ignored: {reason}");
            }
            ignored_total += ignored.len();
            let session = BrowserSessionRef {
                workspace_id: tab.workspace_id,
                tab_id: tab.tab_id,
            };
            let mut views = Vec::new();
            let mut active = 0;
            for descriptor in kept {
                let is_active = descriptor.active;
                match self.create_browser_view(session, descriptor, cx) {
                    Ok(view) => {
                        if is_active {
                            active = views.len();
                        }
                        views.push(view);
                    }
                    Err(message) => {
                        log::warn!("session restore: browser tab not restored: {message}");
                        ignored_total += 1;
                    }
                }
            }
            if views.is_empty() {
                continue;
            }
            let mut slot = DiffDockSlot::with_browsers(views);
            slot.set_active_tab(active);
            self.diff_dock.parked.insert(tab.tab_id, slot);
        }
        if ignored_total > 0 {
            self.show_toast(
                format!("{ignored_total} browser tab(s) from the saved session were ignored"),
                cx,
            );
        }
    }

    pub(crate) fn workspace_has_browser_data(&self, workspace_idx: usize, cx: &gpui::App) -> bool {
        let Some(profile) = self
            .workspaces
            .get(workspace_idx)
            .and_then(|ws| ws.browser_profile.clone())
            .and_then(|id| ProfileId::try_from(id).ok())
        else {
            return false;
        };
        cx.try_global::<BrowserAuthority>()
            .and_then(|authority| {
                authority
                    .profiles()
                    .ok()
                    .map(|store| store.has_data(&profile))
            })
            .unwrap_or(false)
    }

    pub(crate) fn clear_workspace_browser_data(
        &mut self,
        workspace_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(workspace_idx) else {
            return;
        };
        let workspace_id = ws.id;
        let tab_ids: Vec<u64> = ws.tabs().iter().map(|tab| tab.id).collect();
        let profile = ws
            .browser_profile
            .clone()
            .and_then(|id| ProfileId::try_from(id).ok());
        for tab_id in tab_ids {
            if self.diff_dock.owner == Some(tab_id) {
                let indexes: Vec<usize> = self
                    .diff_dock
                    .diff_tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, tab)| matches!(tab, DiffDockTab::Browser(_)))
                    .map(|(index, _)| index)
                    .rev()
                    .collect();
                for index in indexes {
                    self.close_diff_tab(index, cx);
                }
            } else if let Some(slot) = self.diff_dock.parked.get_mut(&tab_id) {
                for view in slot.take_browsers() {
                    view.update(cx, |view, cx| view.close(cx));
                }
                if slot.is_idle() {
                    self.diff_dock.parked.remove(&tab_id);
                }
            }
        }
        let Some(profile) = profile else {
            self.show_toast("This workspace has no browser data", cx);
            return;
        };
        let moved = cx
            .try_global::<BrowserAuthority>()
            .map(|authority| {
                authority
                    .profiles()
                    .map_err(|error| error.message())
                    .and_then(|store| {
                        store
                            .erase_profile(&profile)
                            .map_err(|error| error.message())
                    })
            })
            .unwrap_or_else(|| Err("The browser is unavailable".to_string()));
        match moved {
            Ok(target) => {
                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    ws.browser_profile = None;
                }
                self.save_session(cx);
                if let Some(target) = target {
                    cx.background_spawn(async move {
                        if let Err(error) = std::fs::remove_dir_all(&target) {
                            log::warn!(
                                "browser: erased profile {} not removed: {error}",
                                target.display()
                            );
                        }
                    })
                    .detach();
                }
                self.show_toast("Browser data cleared for this workspace", cx);
            }
            Err(message) => self.show_toast(message, cx),
        }
    }

    pub(crate) fn handle_browser_new_tab(
        &mut self,
        _: &crate::BrowserNewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.diff_dock_visible() && self.browser_available(cx) {
            self.open_diff_browser_tab(None, window, cx);
        }
    }

    pub(crate) fn handle_browser_close(
        &mut self,
        _: &crate::BrowserClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.diff_dock.diff_active_tab;
        if self.diff_dock_visible()
            && matches!(
                self.diff_dock.diff_tabs.get(index),
                Some(DiffDockTab::Browser(_))
            )
        {
            self.request_close_diff_tab(index, window, cx);
        }
    }

    pub(crate) fn handle_browser_focus_terminal(
        &mut self,
        _: &crate::BrowserFocusTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.workspaces.get(self.active_idx) {
            ws.focus_first(window, cx);
        }
    }
}

pub(crate) fn browser_descriptors(
    tabs: &[DiffDockTab],
    active: usize,
    cx: &gpui::App,
) -> Vec<BrowserDescriptor> {
    tabs.iter()
        .enumerate()
        .filter_map(|(index, tab)| match tab {
            DiffDockTab::Browser(view) => Some(view.read(cx).descriptor(index == active)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "paneflow-browser-dock-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    fn create_view(
        cx: &mut gpui::VisualTestContext,
        session: BrowserSessionRef,
        descriptor: BrowserDescriptor,
    ) -> Result<Entity<BrowserView>, BrowserError> {
        cx.update(|_, cx| {
            let profile = ProfileId::try_from(format!("p-{}", session.workspace_id)).unwrap();
            let owner = BrowserOwner {
                workspace_id: session.workspace_id,
                tab_id: session.tab_id,
                profile: profile.clone(),
            };
            let scope = owner.scope();
            let id = BrowserId::try_from(descriptor.id.clone()).unwrap();
            let event = cx.global_mut::<BrowserAuthority>().dispatch(
                &scope,
                Command::Create {
                    owner: scope.clone(),
                    browser: id.clone(),
                    profile,
                    url: descriptor
                        .url
                        .clone()
                        .unwrap_or_else(|| paneflow_browser_protocol::BLANK_URL.to_string()),
                    title: descriptor.title.clone(),
                },
            )?;
            let Event::State { session } = event else {
                return Err(BrowserError::InvalidMessage);
            };
            Ok(cx.new(|cx| BrowserView::new(owner, id, session.document, &descriptor, cx)))
        })
    }

    #[gpui::test]
    fn a_browser_only_dock_needs_no_git_snapshot_and_keeps_its_descriptors(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = scratch("dock");
        cx.update(|cx| cx.set_global(BrowserAuthority::for_test(root.clone())));
        let cx = cx.add_empty_window();
        let session = BrowserSessionRef {
            workspace_id: 1,
            tab_id: 7,
        };
        let first = create_view(
            cx,
            session,
            descriptor("b-1", Some("http://localhost:5173/")),
        )
        .unwrap();
        let second = create_view(cx, session, descriptor("b-2", None)).unwrap();
        let tabs = vec![
            DiffDockTab::Browser(first.clone()),
            DiffDockTab::Browser(second.clone()),
        ];
        assert!(!crate::app::diff_dock::needs_git_snapshot(&tabs));
        assert!(crate::app::diff_dock::needs_git_snapshot(&[
            DiffDockTab::Changes
        ]));
        assert!(crate::app::diff_dock::needs_git_snapshot(&[]));

        let mut slot = DiffDockSlot::with_browsers(vec![first, second]);
        slot.set_active_tab(1);
        let descriptors = cx.update(|_, cx| slot.browser_descriptors(cx));
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].id, "b-1");
        assert_eq!(
            descriptors[0].url.as_deref(),
            Some("http://localhost:5173/")
        );
        assert!(!descriptors[0].active);
        assert_eq!(descriptors[1].id, "b-2");
        assert_eq!(descriptors[1].url, None);
        assert!(descriptors[1].active);
        let _ = std::fs::remove_dir_all(root);
    }

    #[gpui::test]
    fn an_invalid_address_is_refused_before_any_navigation(cx: &mut gpui::TestAppContext) {
        let root = scratch("address");
        cx.update(|cx| cx.set_global(BrowserAuthority::for_test(root.clone())));
        let cx = cx.add_empty_window();
        let session = BrowserSessionRef {
            workspace_id: 2,
            tab_id: 9,
        };
        let view = create_view(cx, session, descriptor("b-3", None)).unwrap();
        view.update_in(cx, |view, window, cx| {
            view.navigate("javascript:alert(1)", window, cx)
        });
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.url(), None);
            assert_eq!(view.notice(), Some("This address is not supported"));
        });
        view.update_in(cx, |view, window, cx| {
            view.navigate(" localhost:5173 ", window, cx)
        });
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).url(), Some("http://localhost:5173/"));
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[gpui::test]
    fn closing_a_page_releases_its_identity_in_the_authority(cx: &mut gpui::TestAppContext) {
        let root = scratch("close");
        cx.update(|cx| cx.set_global(BrowserAuthority::for_test(root.clone())));
        let cx = cx.add_empty_window();
        let session = BrowserSessionRef {
            workspace_id: 3,
            tab_id: 11,
        };
        let view = create_view(cx, session, descriptor("b-4", None)).unwrap();
        assert_eq!(
            create_view(cx, session, descriptor("b-4", None)).err(),
            Some(BrowserError::Busy)
        );
        let foreign = BrowserSessionRef {
            workspace_id: 4,
            tab_id: 12,
        };
        assert_eq!(
            create_view(cx, foreign, descriptor("b-4", None)).err(),
            Some(BrowserError::Busy)
        );
        view.update(cx, |view, cx| view.close(cx));
        assert!(create_view(cx, session, descriptor("b-4", None)).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[gpui::test]
    fn putting_a_page_to_sleep_frees_its_live_slot_in_the_authority(cx: &mut gpui::TestAppContext) {
        let root = scratch("sleep");
        cx.update(|cx| cx.set_global(BrowserAuthority::for_test(root.clone())));
        let cx = cx.add_empty_window();
        let session = BrowserSessionRef {
            workspace_id: 5,
            tab_id: 13,
        };
        let scope = BrowserAuthority::scope(session.workspace_id, session.tab_id);
        let mut views = Vec::new();
        for index in 0..paneflow_browser_protocol::MAX_LIVE_BROWSERS {
            let id = format!("live-{index}");
            let view =
                create_view(cx, session, descriptor(&id, Some("http://localhost:5173/"))).unwrap();
            let document = Document {
                owner: scope.clone(),
                browser: BrowserId::try_from(id).unwrap(),
                generation: index as u64 + 1,
            };
            cx.update(|_, cx| {
                cx.global_mut::<BrowserAuthority>()
                    .dispatch(&scope, Command::Start { document })
                    .unwrap();
            });
            views.push(view);
        }
        let foreign = BrowserSessionRef {
            workspace_id: 6,
            tab_id: 14,
        };
        let ninth = create_view(
            cx,
            foreign,
            descriptor("ninth", Some("http://localhost:5174/")),
        )
        .unwrap();
        let ninth_document = Document {
            owner: BrowserAuthority::scope(foreign.workspace_id, foreign.tab_id),
            browser: BrowserId::try_from("ninth".to_string()).unwrap(),
            generation: paneflow_browser_protocol::MAX_LIVE_BROWSERS as u64 + 1,
        };
        let start_ninth = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                cx.global_mut::<BrowserAuthority>().dispatch(
                    &ninth_document.owner,
                    Command::Start {
                        document: ninth_document.clone(),
                    },
                )
            })
        };
        assert_eq!(start_ninth(cx).err(), Some(BrowserError::LimitReached));
        views[0].update(cx, |view, cx| view.sleep(cx));
        assert!(start_ninth(cx).is_ok());
        ninth.update(cx, |view, cx| view.sleep(cx));
        let state = cx.update(|_, cx| {
            cx.global_mut::<BrowserAuthority>().dispatch(
                &ninth_document.owner,
                Command::State {
                    document: ninth_document.clone(),
                },
            )
        });
        assert!(matches!(
            state,
            Ok(Event::State { session }) if session.state == paneflow_browser_protocol::SessionState::Dormant
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    fn descriptor(id: &str, url: Option<&str>) -> BrowserDescriptor {
        BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION,
            id: id.to_string(),
            url: url.map(str::to_string),
            title: "t".to_string(),
            zoom: 100,
            muted: false,
            active: false,
        }
    }

    #[test]
    fn restore_ignores_forbidden_urls_bad_identities_newer_versions_and_the_surplus() {
        let mut descriptors = vec![
            descriptor("ok-1", Some("http://localhost:5173/")),
            descriptor("bad url", Some("http://localhost/")),
            descriptor("js", Some("javascript:alert(1)")),
            descriptor("file", Some("file:///etc/passwd")),
            descriptor("ok-1", Some("http://localhost:5174/")),
            descriptor("empty", None),
        ];
        descriptors.push(BrowserDescriptor {
            version: BROWSER_DESCRIPTOR_VERSION + 1,
            ..descriptor("newer", None)
        });
        descriptors.extend((0..10).map(|i| descriptor(&format!("more-{i}"), None)));
        let (kept, ignored) = validate_descriptors(&descriptors);
        assert_eq!(kept.len(), MAX_BROWSERS_PER_SESSION);
        let ids: Vec<_> = kept.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(&ids[..2], ["ok-1", "empty"]);
        assert!(!ids.contains(&"js") && !ids.contains(&"file") && !ids.contains(&"newer"));
        assert_eq!(ignored.len(), descriptors.len() - MAX_BROWSERS_PER_SESSION);
    }

    #[test]
    fn restore_clamps_zoom_and_bounds_titles_without_dropping_the_tab() {
        let mut bad = descriptor("zoom", None);
        bad.zoom = 9000;
        bad.title = "x".repeat(2000);
        let (kept, ignored) = validate_descriptors(&[bad]);
        assert!(ignored.is_empty());
        assert_eq!(kept[0].zoom, 100);
        assert_eq!(kept[0].title.chars().count(), 512);
    }
}

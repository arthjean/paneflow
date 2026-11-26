use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
    deferred, div, prelude::FluentBuilder, px, svg,
};
use paneflow_browser_client::BrowserClient;
use paneflow_browser_control::BlockedPermission;
use paneflow_browser_proto::navigation::origin_of;
use paneflow_browser_proto::{CertificateName, Connection, Permission, Reply, SiteSecurity, TabId};
use serde_json::{Value, json};

use super::address::{self, is_loopback_url};
use super::render::{ADDRESS_TEXT_SIZE, SUGGESTION_GAP, popup_panel};
use super::view::{BrowserView, BrowserViewEvent, host_of};
use crate::settings::kit::{self, Ink, Size, Tone};

const LOOKUP_DEADLINE: Duration = Duration::from_secs(5);
const PANEL_WIDTH: f32 = 320.;
const PANEL_PADDING: f32 = 6.;
const HEADER_HEIGHT: f32 = 32.;
const ROW_HEIGHT: f32 = 36.;
const ROW_RADIUS: f32 = 8.;
const ROW_PADDING: f32 = 8.;
const ROW_GAP: f32 = 10.;
const ICON_SIZE: f32 = 16.;
const CHEVRON_SIZE: f32 = 14.;
const DETAIL_TEXT_SIZE: f32 = 12.;
const SECTION_GAP: f32 = 6.;
const FACT_LABEL_WIDTH: f32 = 92.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SitePage {
    Main,
    Security,
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Lookup<T> {
    Pending,
    Ready(T),
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cookie {
    pub(crate) name: String,
    pub(crate) domain: String,
    pub(crate) path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SiteData {
    pub(crate) cookies: Vec<Cookie>,
    pub(crate) bytes: u64,
}

pub(crate) struct SiteInfo {
    pub(crate) page: SitePage,
    pub(crate) url: String,
    pub(crate) origin: Option<String>,
    pub(crate) host: String,
    pub(crate) security: Lookup<SiteSecurity>,
    pub(crate) data: Lookup<SiteData>,
    pub(crate) deleting: bool,
    pub(crate) focus: FocusHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Standing {
    Secure,
    MixedContent,
    CertificateError,
    LocalServer,
    NotSecure,
}

impl Standing {
    pub(crate) fn of(url: &str, connection: Option<Connection>) -> Self {
        match connection {
            Some(Connection::Secure) => Self::Secure,
            Some(Connection::MixedContent) => Self::MixedContent,
            Some(Connection::CertificateError) => Self::CertificateError,
            Some(Connection::Plain) | None if is_loopback_url(url) => Self::LocalServer,
            None if url.to_ascii_lowercase().starts_with("https://") => Self::Secure,
            Some(Connection::Plain) | None => Self::NotSecure,
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Secure => "icons/lock.svg",
            Self::CertificateError => "icons/triangle-alert.svg",
            Self::MixedContent | Self::LocalServer | Self::NotSecure => "icons/info-circle.svg",
        }
    }

    pub(crate) fn color(self, ink: Ink) -> gpui::Hsla {
        match self {
            Self::CertificateError => ink.danger,
            Self::Secure | Self::MixedContent | Self::LocalServer | Self::NotSecure => ink.primary,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Secure => "Connection is secure",
            Self::MixedContent => "Parts of this page are not secure",
            Self::CertificateError => "Certificate is not valid",
            Self::LocalServer => "Local server",
            Self::NotSecure => "Connection is not secure",
        }
    }

    pub(crate) fn explanation(self) -> &'static str {
        match self {
            Self::Secure => {
                "Passwords, card numbers and anything else you send to this site are encrypted on the way."
            }
            Self::MixedContent => {
                "The page is encrypted, but it loads some content, such as images, over an unencrypted connection."
            }
            Self::CertificateError => {
                "Paneflow could not trust this site's certificate. Do not enter passwords or payment details here."
            }
            Self::LocalServer => {
                "This page comes from a server running on this computer, so it never crosses the network."
            }
            Self::NotSecure => {
                "Do not enter passwords or payment details on this site: they would travel unencrypted."
            }
        }
    }
}

pub(crate) fn cookies_of(result: &Value) -> Vec<Cookie> {
    result
        .get("cookies")
        .and_then(Value::as_array)
        .map(|cookies| {
            cookies
                .iter()
                .filter_map(|cookie| {
                    Some(Cookie {
                        name: cookie.get("name")?.as_str()?.to_string(),
                        domain: cookie.get("domain")?.as_str()?.to_string(),
                        path: cookie.get("path")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn usage_of(result: &Value) -> u64 {
    result
        .get("usage")
        .and_then(Value::as_f64)
        .filter(|usage| usage.is_finite() && *usage > 0.)
        .map_or(0, |usage| usage.round() as u64)
}

pub(crate) fn data_summary(data: &SiteData) -> String {
    let cookies = match data.cookies.len() {
        0 => None,
        1 => Some("1 cookie".to_string()),
        count => Some(format!("{count} cookies")),
    };
    let stored = (data.bytes > 0).then(|| crate::app::diff_dock::preview::format_size(data.bytes));
    match (cookies, stored) {
        (None, None) => "Nothing stored".to_string(),
        (Some(cookies), None) => cookies,
        (None, Some(stored)) => format!("{stored} stored"),
        (Some(cookies), Some(stored)) => format!("{cookies}, {stored} stored"),
    }
}

pub(crate) fn kept_sentence(host: &str, data: &SiteData) -> String {
    let cookies = match data.cookies.len() {
        0 => None,
        1 => Some("1 cookie".to_string()),
        count => Some(format!("{count} cookies")),
    };
    let stored = (data.bytes > 0).then(|| {
        format!(
            "{} of data",
            crate::app::diff_dock::preview::format_size(data.bytes)
        )
    });
    let kept = match (cookies, stored) {
        (None, None) => "nothing".to_string(),
        (Some(one), None) | (None, Some(one)) => one,
        (Some(cookies), Some(stored)) => format!("{cookies} and {stored}"),
    };
    format!("{host} keeps {kept} on this computer.")
}

pub(crate) fn permission_label(permission: Permission) -> &'static str {
    match permission {
        Permission::Camera => "Camera",
        Permission::Microphone => "Microphone",
        Permission::Geolocation => "Location",
        Permission::Notifications => "Notifications",
        Permission::Other => "Other permission",
    }
}

pub(crate) fn permission_icon(permission: Permission) -> &'static str {
    match permission {
        Permission::Notifications => "icons/bell.svg",
        Permission::Camera
        | Permission::Microphone
        | Permission::Geolocation
        | Permission::Other => "icons/shield-sandbox.svg",
    }
}

pub(crate) fn blocked_here(blocked: &[BlockedPermission], origin: &str) -> Vec<Permission> {
    let mut here = Vec::new();
    for entry in blocked {
        if entry.origin == origin && !here.contains(&entry.permission) {
            here.push(entry.permission);
        }
    }
    here
}

pub(crate) fn name_label(name: &CertificateName) -> String {
    let common = name.common_name.trim();
    match name.organization.as_deref().map(str::trim) {
        Some(organization) if !organization.is_empty() && !common.is_empty() => {
            format!("{common} ({organization})")
        }
        Some(organization) if !organization.is_empty() => organization.to_string(),
        _ if !common.is_empty() => common.to_string(),
        _ => "Unknown".to_string(),
    }
}

pub(crate) fn date_label(unix_seconds: i64) -> String {
    jiff::Timestamp::from_second(unix_seconds)
        .map(|at| {
            jiff::tz::TimeZone::system()
                .to_datetime(at)
                .strftime("%b %-d, %Y")
                .to_string()
        })
        .unwrap_or_else(|_| "Unknown".to_string())
}

fn wait_value(ticket: paneflow_browser_client::Ticket) -> Option<Value> {
    match ticket.wait(LOOKUP_DEADLINE) {
        Ok(Reply::DevTools(value)) => Some(value),
        _ => None,
    }
}

fn read_data(
    client: &BrowserClient,
    tab: TabId,
    url: &str,
    origin: Option<&str>,
) -> Option<SiteData> {
    let cookies = client.devtools(tab, "Network.getCookies", json!({ "urls": [url] }));
    let usage = origin.map(|origin| {
        client.devtools(tab, "Storage.getUsageAndQuota", json!({ "origin": origin }))
    });
    let cookies = cookies_of(&wait_value(cookies)?);
    let bytes = usage
        .and_then(wait_value)
        .map_or(0, |usage| usage_of(&usage));
    Some(SiteData { cookies, bytes })
}

fn delete_data(client: &BrowserClient, tab: TabId, origin: &str, cookies: &[Cookie]) {
    let cleared = client.devtools(
        tab,
        "Storage.clearDataForOrigin",
        json!({ "origin": origin, "storageTypes": "all" }),
    );
    let deletions: Vec<_> = cookies
        .iter()
        .map(|cookie| {
            client.devtools(
                tab,
                "Network.deleteCookies",
                json!({ "name": cookie.name, "domain": cookie.domain, "path": cookie.path }),
            )
        })
        .collect();
    drop(wait_value(cleared));
    for deletion in deletions {
        drop(wait_value(deletion));
    }
}

impl BrowserView {
    pub(crate) fn standing(&self) -> Option<Standing> {
        let url = self.url();
        address::display(&url)?;
        let connection = self
            .security
            .as_ref()
            .filter(|(known, _)| *known == url)
            .map(|(_, connection)| *connection);
        Some(Standing::of(&url, connection))
    }

    pub(crate) fn refresh_security(&mut self, cx: &mut Context<Self>) {
        let (Some(tab), Some(snapshot)) = (self.tab, self.snapshot.as_ref()) else {
            return;
        };
        let url = snapshot.url.clone();
        let arrived =
            !snapshot.loading && snapshot.state == paneflow_api::methods::browser::TabState::Loaded;
        if !arrived
            || self
                .security
                .as_ref()
                .is_some_and(|(known, _)| *known == url)
        {
            return;
        }
        let Ok(runtime) = crate::browser_control::runtime() else {
            return;
        };
        let ticket = runtime.client.security(tab);
        cx.spawn(async move |this, cx| {
            let Ok(Reply::Security(security)) =
                smol::unblock(move || ticket.wait(LOOKUP_DEADLINE)).await
            else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                view.security = Some((security.url.clone(), security.connection));
                if let Some(info) = view.site_info.as_mut()
                    && info.url == security.url
                {
                    info.security = Lookup::Ready(security);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_site_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.site_info.is_some() {
            self.close_site_info(window, cx);
            return;
        }
        let Some(tab) = self.tab else {
            return;
        };
        let url = self.url();
        let origin = origin_of(&url);
        let host = host_of(&url).unwrap_or_else(|| url.clone());
        let focus = cx.focus_handle();
        self.menu = None;
        self.site_info = Some(SiteInfo {
            page: SitePage::Main,
            url: url.clone(),
            origin: origin.clone(),
            host,
            security: Lookup::Pending,
            data: Lookup::Pending,
            deleting: false,
            focus: focus.clone(),
        });
        window.focus(&focus, cx);
        self.load_site_info(tab, url, origin, cx);
        cx.notify();
    }

    fn load_site_info(
        &mut self,
        tab: TabId,
        url: String,
        origin: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let client = match crate::browser_control::runtime() {
            Ok(runtime) => Arc::clone(&runtime.client),
            Err(_) => {
                if let Some(info) = self.site_info.as_mut() {
                    info.security = Lookup::Failed;
                    info.data = Lookup::Failed;
                }
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let asked = url.clone();
            let (security, data) = smol::unblock(move || {
                let security = match client.security(tab).wait(LOOKUP_DEADLINE) {
                    Ok(Reply::Security(security)) => Some(security),
                    _ => None,
                };
                (security, read_data(&client, tab, &url, origin.as_deref()))
            })
            .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(security) = &security {
                    view.security = Some((security.url.clone(), security.connection));
                }
                let Some(info) = view.site_info.as_mut().filter(|info| info.url == asked) else {
                    return;
                };
                info.security = security.map_or(Lookup::Failed, Lookup::Ready);
                info.data = data.map_or(Lookup::Failed, Lookup::Ready);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn close_site_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.site_info.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    pub(crate) fn drop_site_info_for(&mut self, url: &str) {
        let stale = self
            .site_info
            .as_ref()
            .is_some_and(|info| origin_of(&info.url) != origin_of(url));
        if stale {
            self.site_info = None;
        }
    }

    fn show_site_page(&mut self, page: SitePage, cx: &mut Context<Self>) {
        if let Some(info) = self.site_info.as_mut() {
            info.page = page;
            cx.notify();
        }
    }

    fn delete_site_data(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tab else {
            return;
        };
        let Some(info) = self.site_info.as_mut() else {
            return;
        };
        let Some(origin) = info.origin.clone() else {
            return;
        };
        if info.deleting {
            return;
        }
        let Ok(runtime) = crate::browser_control::runtime() else {
            return;
        };
        let client = Arc::clone(&runtime.client);
        let cookies = match &info.data {
            Lookup::Ready(data) => data.cookies.clone(),
            Lookup::Pending | Lookup::Failed => Vec::new(),
        };
        let url = info.url.clone();
        info.deleting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let asked = url.clone();
            let data = smol::unblock(move || {
                delete_data(&client, tab, &origin, &cookies);
                read_data(&client, tab, &url, Some(&origin))
            })
            .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(info) = view.site_info.as_mut().filter(|info| info.url == asked) {
                    info.deleting = false;
                    info.data = data.map_or(Lookup::Failed, Lookup::Ready);
                }
                view.reload_or_stop(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_site_info(&self, ink: Ink, cx: &mut Context<Self>) -> Option<AnyElement> {
        let info = self.site_info.as_ref()?;
        let bounds = self.omnibox.bounds?;
        let standing = self.standing().unwrap_or(Standing::NotSecure);
        let close = kit::icon_button("browser-site-close", "Close", "icons/close.svg", ink)
            .on_click(cx.listener(|this, _, window, cx| this.close_site_info(window, cx)));
        let (title, back): (SharedString, Option<AnyElement>) = match info.page {
            SitePage::Main => (info.host.clone().into(), None),
            SitePage::Security => ("Security".into(), Some(self.back_button(ink, cx))),
            SitePage::Data => (
                "Cookies and site data".into(),
                Some(self.back_button(ink, cx)),
            ),
        };
        let header = div()
            .h(px(HEADER_HEIGHT))
            .pl(px(if back.is_some() { 0. } else { ROW_PADDING }))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .children(back)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(close);
        let body = match info.page {
            SitePage::Main => self.render_site_main(info, standing, ink, cx),
            SitePage::Security => render_security_page(info, standing, ink),
            SitePage::Data => self.render_data_page(info, ink, cx),
        };
        let icon_bounds = self.site_icon_bounds;
        let panel = popup_panel(ink)
            .id("browser-site-info")
            .w(px(PANEL_WIDTH))
            .p(px(PANEL_PADDING))
            .text_size(px(ADDRESS_TEXT_SIZE))
            .track_focus(&info.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_site_info(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down_out(
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if icon_bounds.is_some_and(|icon| icon.contains(&event.position)) {
                        return;
                    }
                    this.close_site_info(window, cx);
                }),
            )
            .flex()
            .flex_col()
            .child(header)
            .child(body);
        Some(
            deferred(
                gpui::anchored()
                    .position(gpui::point(
                        bounds.origin.x,
                        bounds.origin.y + bounds.size.height + px(SUGGESTION_GAP),
                    ))
                    .child(panel),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    fn back_button(&self, ink: Ink, cx: &mut Context<Self>) -> AnyElement {
        kit::icon_button("browser-site-back", "Back", "icons/chevron-left.svg", ink)
            .on_click(cx.listener(|this, _, _, cx| this.show_site_page(SitePage::Main, cx)))
            .into_any_element()
    }

    fn render_site_main(
        &self,
        info: &SiteInfo,
        standing: Standing,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let connection = nav_row(
            "browser-site-security",
            standing.icon(),
            standing.color(ink),
            standing.title(),
            None,
            ink,
        )
        .on_click(cx.listener(|this, _, _, cx| this.show_site_page(SitePage::Security, cx)));
        let data_detail = match &info.data {
            Lookup::Pending => "Reading".to_string(),
            Lookup::Ready(data) => data_summary(data),
            Lookup::Failed => "Paneflow could not read this site's data".to_string(),
        };
        let data = nav_row(
            "browser-site-data",
            "icons/database.svg",
            ink.primary,
            "Cookies and site data",
            Some(data_detail),
            ink,
        )
        .on_click(cx.listener(|this, _, _, cx| this.show_site_page(SitePage::Data, cx)));
        let blocked = match (&self.snapshot, &info.origin) {
            (Some(snapshot), Some(origin)) => blocked_here(&snapshot.blocked, origin),
            _ => Vec::new(),
        };
        let permissions = (!blocked.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(divider(ink))
                .child(
                    div()
                        .px(px(ROW_PADDING))
                        .pt(px(SECTION_GAP))
                        .pb(px(2.))
                        .text_size(px(DETAIL_TEXT_SIZE))
                        .text_color(ink.secondary)
                        .child("Blocked on this site"),
                )
                .children(blocked.into_iter().map(|permission| {
                    fact_row(
                        permission_icon(permission),
                        permission_label(permission),
                        ink,
                    )
                }))
                .child(
                    div()
                        .px(px(ROW_PADDING))
                        .pb(px(SECTION_GAP))
                        .text_size(px(DETAIL_TEXT_SIZE))
                        .text_color(ink.tertiary)
                        .child("Paneflow's browser does not give pages these permissions."),
                )
        });
        let settings = nav_row(
            "browser-site-settings",
            "icons/settings.svg",
            ink.primary,
            "Browser settings",
            None,
            ink,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.close_site_info(window, cx);
            cx.emit(BrowserViewEvent::OpenSettings(window.window_handle()));
        }));
        div()
            .flex()
            .flex_col()
            .child(connection)
            .child(data)
            .children(permissions)
            .child(divider(ink))
            .child(settings)
            .into_any_element()
    }

    fn render_data_page(&self, info: &SiteInfo, ink: Ink, cx: &mut Context<Self>) -> AnyElement {
        let text = match &info.data {
            Lookup::Pending => "Reading what this site stores".to_string(),
            Lookup::Ready(data) => kept_sentence(&info.host, data),
            Lookup::Failed => "Paneflow could not read this site's data.".to_string(),
        };
        let can_delete = info.origin.is_some()
            && !info.deleting
            && matches!(&info.data, Lookup::Ready(data) if !data.cookies.is_empty() || data.bytes > 0);
        let label = if info.deleting {
            "Deleting"
        } else {
            "Delete site data"
        };
        let delete = kit::button_with(
            "browser-site-delete",
            label,
            Tone::Danger,
            Size::Small,
            false,
            can_delete,
            ink,
        )
        .when(can_delete, |button| {
            button.on_click(cx.listener(|this, _, _, cx| this.delete_site_data(cx)))
        });
        div()
            .px(px(ROW_PADDING))
            .pb(px(ROW_PADDING))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_color(ink.secondary).child(text))
            .child(
                div()
                    .text_size(px(DETAIL_TEXT_SIZE))
                    .text_color(ink.tertiary)
                    .child("Deleting signs you out of this site and reloads the page."),
            )
            .child(div().flex().child(delete))
            .into_any_element()
    }
}

fn render_security_page(info: &SiteInfo, standing: Standing, ink: Ink) -> AnyElement {
    let headline = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .child(
            svg()
                .path(standing.icon())
                .size(px(ICON_SIZE))
                .flex_none()
                .text_color(standing.color(ink)),
        )
        .child(
            div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(standing.color(ink))
                .child(standing.title()),
        );
    let mut facts: Vec<(&'static str, String)> = Vec::new();
    if let Lookup::Ready(security) = &info.security {
        if let Some(certificate) = &security.certificate {
            facts.push(("Issued to", name_label(&certificate.subject)));
            facts.push(("Issued by", name_label(&certificate.issuer)));
            facts.push(("Valid from", date_label(certificate.valid_from)));
            facts.push(("Expires", date_label(certificate.valid_until)));
        }
        if let Some(protocol) = &security.protocol {
            facts.push(("Protocol", protocol.clone()));
        }
    }
    let certificate = (!facts.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(px(DETAIL_TEXT_SIZE))
            .child(
                div()
                    .text_color(ink.secondary)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Certificate"),
            )
            .children(facts.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_none()
                            .w(px(FACT_LABEL_WIDTH))
                            .text_color(ink.tertiary)
                            .child(label),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(value))
            }))
    });
    let pending = matches!(info.security, Lookup::Pending).then(|| {
        div()
            .text_size(px(DETAIL_TEXT_SIZE))
            .text_color(ink.tertiary)
            .child("Reading the certificate")
    });
    div()
        .px(px(ROW_PADDING))
        .pb(px(ROW_PADDING))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(headline)
        .child(
            div()
                .text_color(ink.secondary)
                .child(standing.explanation()),
        )
        .children(certificate)
        .children(pending)
        .into_any_element()
}

fn nav_row(
    id: &'static str,
    icon: &'static str,
    icon_color: gpui::Hsla,
    title: &'static str,
    detail: Option<String>,
    ink: Ink,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_h(px(ROW_HEIGHT))
        .px(px(ROW_PADDING))
        .py(px(4.))
        .rounded(px(ROW_RADIUS))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .cursor(gpui::CursorStyle::PointingHand)
        .hover(move |row| row.bg(ink.hover))
        .active(move |row| row.bg(ink.pressed))
        .role(gpui::accesskit::Role::Button)
        .aria_label(SharedString::from(title))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            svg()
                .path(icon)
                .size(px(ICON_SIZE))
                .flex_none()
                .text_color(icon_color),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().truncate().child(title))
                .children(detail.map(|detail| {
                    div()
                        .truncate()
                        .text_size(px(DETAIL_TEXT_SIZE))
                        .text_color(ink.secondary)
                        .child(detail)
                })),
        )
        .child(
            svg()
                .path("icons/chevron-right.svg")
                .size(px(CHEVRON_SIZE))
                .flex_none()
                .text_color(ink.tertiary),
        )
}

fn fact_row(icon: &'static str, label: &'static str, ink: Ink) -> AnyElement {
    div()
        .h(px(ROW_HEIGHT - 8.))
        .px(px(ROW_PADDING))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .child(
            svg()
                .path(icon)
                .size(px(ICON_SIZE))
                .flex_none()
                .text_color(ink.secondary),
        )
        .child(div().flex_1().child(label))
        .child(
            div()
                .text_size(px(DETAIL_TEXT_SIZE))
                .text_color(ink.secondary)
                .child("Blocked"),
        )
        .into_any_element()
}

fn divider(ink: Ink) -> gpui::Div {
    div().my(px(4.)).h(px(1.)).bg(ink.edge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standing_follows_the_engine_and_falls_back_to_the_scheme_until_it_answers() {
        assert_eq!(
            Standing::of("https://vercel.com/", Some(Connection::Secure)),
            Standing::Secure
        );
        assert_eq!(
            Standing::of("https://vercel.com/", Some(Connection::CertificateError)),
            Standing::CertificateError
        );
        assert_eq!(
            Standing::of("https://vercel.com/", Some(Connection::MixedContent)),
            Standing::MixedContent
        );
        assert_eq!(Standing::of("https://vercel.com/", None), Standing::Secure);
        assert_eq!(
            Standing::of("http://localhost:3000/", Some(Connection::Plain)),
            Standing::LocalServer
        );
        assert_eq!(
            Standing::of("http://127.0.0.1:5173/", None),
            Standing::LocalServer
        );
        assert_eq!(
            Standing::of("http://neverssl.com/", Some(Connection::Plain)),
            Standing::NotSecure
        );
    }

    #[test]
    fn cookies_and_storage_read_from_devtools_and_summarize_in_words() {
        let cookies = cookies_of(&json!({"cookies": [
            {"name": "sid", "domain": "vercel.com", "path": "/", "value": "secret"},
            {"name": "theme", "domain": ".vercel.com", "path": "/"},
            {"name": "broken"}
        ]}));
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(
            usage_of(&json!({"usage": 1_258_291.0, "quota": 1e9})),
            1_258_291
        );
        assert_eq!(usage_of(&json!({"usage": -1.0})), 0);
        assert_eq!(usage_of(&json!({})), 0);
        assert_eq!(data_summary(&SiteData::default()), "Nothing stored");
        assert_eq!(
            data_summary(&SiteData {
                cookies: cookies[..1].to_vec(),
                bytes: 0
            }),
            "1 cookie"
        );
        let both = SiteData {
            cookies,
            bytes: 1_258_291,
        };
        assert_eq!(data_summary(&both), "2 cookies, 1.2 MB stored");
        assert_eq!(
            kept_sentence("vercel.com", &both),
            "vercel.com keeps 2 cookies and 1.2 MB of data on this computer."
        );
        assert_eq!(
            kept_sentence("localhost:3000", &SiteData::default()),
            "localhost:3000 keeps nothing on this computer."
        );
    }

    #[test]
    fn only_the_permissions_this_origin_asked_for_are_listed_once() {
        let blocked = vec![
            BlockedPermission {
                origin: "https://meet.example".to_string(),
                permission: Permission::Camera,
            },
            BlockedPermission {
                origin: "https://other.example".to_string(),
                permission: Permission::Geolocation,
            },
            BlockedPermission {
                origin: "https://meet.example".to_string(),
                permission: Permission::Microphone,
            },
        ];
        assert_eq!(
            blocked_here(&blocked, "https://meet.example"),
            vec![Permission::Camera, Permission::Microphone]
        );
        assert!(blocked_here(&blocked, "https://vercel.com").is_empty());
    }

    #[test]
    fn a_certificate_name_reads_as_common_name_and_organization() {
        let name = |common: &str, organization: Option<&str>| CertificateName {
            common_name: common.to_string(),
            organization: organization.map(str::to_string),
        };
        assert_eq!(
            name_label(&name("R11", Some("Let's Encrypt"))),
            "R11 (Let's Encrypt)"
        );
        assert_eq!(name_label(&name("vercel.com", None)), "vercel.com");
        assert_eq!(
            name_label(&name("", Some("Google Trust Services"))),
            "Google Trust Services"
        );
        assert_eq!(name_label(&name(" ", None)), "Unknown");
    }
}

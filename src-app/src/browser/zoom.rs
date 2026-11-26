use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::App;
use serde::{Deserialize, Serialize};

use super::kept::{self, Kept};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SiteZooms {
    #[serde(default)]
    sites: BTreeMap<String, u16>,
}

impl Kept for SiteZooms {
    const FILE_NAME: &'static str = "zoom.json";
}

impl SiteZooms {
    pub(crate) fn of(&self, host: &str, default: u16) -> u16 {
        self.sites.get(host).copied().unwrap_or(default)
    }

    pub(crate) fn list(&self) -> Vec<(String, u16)> {
        self.sites
            .iter()
            .map(|(host, zoom)| (host.clone(), *zoom))
            .collect()
    }

    fn set(&mut self, host: &str, zoom: u16, default: u16) -> bool {
        if zoom == default {
            return self.sites.remove(host).is_some();
        }
        self.sites.insert(host.to_string(), zoom) != Some(zoom)
    }
}

pub(crate) fn site_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_lowercase())
}

pub(crate) fn start(cx: &mut App) {
    kept::start::<SiteZooms>(cx);
}

pub(crate) fn snapshot(cx: &App) -> Arc<SiteZooms> {
    kept::snapshot::<SiteZooms>(cx)
}

pub(crate) fn set(host: &str, zoom: u16, default: u16, cx: &mut App) {
    let host = host.to_string();
    kept::change::<SiteZooms>(cx, move |zooms| zooms.set(&host, zoom, default));
}

pub(crate) fn reset(host: &str, cx: &mut App) {
    let host = host.to_string();
    kept::change::<SiteZooms>(cx, move |zooms| zooms.sites.remove(&host).is_some());
}

pub(crate) fn reset_all(cx: &mut App) {
    kept::change::<SiteZooms>(cx, |zooms| {
        let had = !zooms.sites.is_empty();
        zooms.sites.clear();
        had
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_keeps_only_a_zoom_off_the_default() {
        let mut zooms = SiteZooms::default();
        assert!(zooms.set("vercel.com", 125, 100));
        assert_eq!(zooms.of("vercel.com", 100), 125);
        assert_eq!(zooms.of("github.com", 110), 110);
        assert!(zooms.set("vercel.com", 100, 100));
        assert!(zooms.list().is_empty(), "the default forgets the site");
        assert!(!zooms.set("vercel.com", 100, 100));
    }

    #[test]
    fn a_site_is_the_lowercase_host_and_port_of_a_web_address() {
        assert_eq!(
            site_of("https://Vercel.com/strive-x?tab=1").as_deref(),
            Some("vercel.com")
        );
        assert_eq!(
            site_of("http://user@localhost:5173/").as_deref(),
            Some("localhost:5173")
        );
        assert_eq!(site_of("about:blank"), None);
        assert_eq!(site_of("https://"), None);
    }
}

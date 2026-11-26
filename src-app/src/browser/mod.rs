mod address;
mod annotate;
mod inspector;
pub(crate) mod kept;
mod keys;
mod loader;
pub(crate) mod omnibox;
mod render;
mod router;
mod site_info;
mod view;
pub(crate) mod zoom;

pub(crate) use address::is_loopback_url;
pub(crate) use omnibox::DevServer;
pub(crate) use router::view_for;
pub(crate) use view::{AgentChoice, BrowserView, BrowserViewEvent};

pub(crate) const SURFACE_TYPE: &str = "browser";

pub(crate) fn empty_surface_definition() -> paneflow_config::schema::SurfaceDefinition {
    paneflow_config::schema::SurfaceDefinition {
        surface_type: Some(SURFACE_TYPE.to_string()),
        ..paneflow_config::schema::SurfaceDefinition::default()
    }
}

pub(crate) fn surface_definition(view: &BrowserView) -> paneflow_config::schema::SurfaceDefinition {
    let (url, title) = view.persisted().unzip();
    paneflow_config::schema::SurfaceDefinition {
        surface_type: Some(SURFACE_TYPE.to_string()),
        path: url,
        name: title.flatten(),
        ..paneflow_config::schema::SurfaceDefinition::default()
    }
}

#[derive(Default)]
struct Tracked {
    views: Vec<gpui::WeakEntity<BrowserView>>,
    displayed: std::collections::HashSet<gpui::EntityId>,
}

impl gpui::Global for Tracked {}

pub(crate) fn track(view: &gpui::Entity<BrowserView>, cx: &mut gpui::App) {
    let tracked = cx.default_global::<Tracked>();
    tracked.views.retain(|view| view.upgrade().is_some());
    tracked.views.push(view.downgrade());
}

pub(crate) fn sync_displayed(shown: std::collections::HashSet<gpui::EntityId>, cx: &mut gpui::App) {
    let tracked = cx.default_global::<Tracked>();
    if tracked.displayed == shown {
        return;
    }
    let changed: Vec<(gpui::Entity<BrowserView>, bool)> = tracked
        .views
        .iter()
        .filter_map(gpui::WeakEntity::upgrade)
        .filter_map(|view| {
            let id = view.entity_id();
            let now = shown.contains(&id);
            (now != tracked.displayed.contains(&id)).then_some((view, now))
        })
        .collect();
    tracked.displayed = shown;
    if changed.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for (view, displayed) in changed {
            view.update(cx, |view, cx| view.set_displayed(displayed, cx));
        }
    });
}

pub(crate) fn restorable(url: &str) -> bool {
    paneflow_browser_control::validate_url(url).is_ok()
}

pub(crate) fn opens_in_paneflow(
    config: &paneflow_config::schema::BrowserConfig,
    url: &str,
) -> bool {
    config.opens_local_links() && is_loopback_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::BrowserConfig;

    #[test]
    fn only_a_loopback_url_with_the_setting_on_opens_in_paneflow() {
        let off = BrowserConfig {
            open_local_links: Some(false),
            ..BrowserConfig::default()
        };
        let on = BrowserConfig::default();
        assert!(!opens_in_paneflow(&off, "http://localhost:3000/"));
        assert!(opens_in_paneflow(&on, "http://localhost:3000/"));
        assert!(opens_in_paneflow(&on, "http://127.0.0.1:5173/app"));
        assert!(opens_in_paneflow(&on, "http://[::1]:8080/"));
        assert!(!opens_in_paneflow(&on, "https://example.com/"));
        assert!(!opens_in_paneflow(&on, "file:///etc/hosts"));
    }

    #[test]
    fn local_links_open_in_paneflow_by_default_and_a_retired_display_key_is_ignored() {
        assert!(BrowserConfig::default().opens_local_links());
        let parsed: paneflow_config::schema::PaneFlowConfig =
            serde_json::from_str(r#"{"browser": {"display": false, "open_local_links": false}}"#)
                .unwrap();
        assert!(!parsed.browser.opens_local_links());
    }

    #[test]
    fn only_a_displayed_page_takes_pointer_and_key_input() {
        use super::view::PageState;
        assert!(PageState::Page.takes_input());
        for covered in [
            PageState::Empty,
            PageState::Opening,
            PageState::NotInstalled,
            PageState::Crashed("gone".into()),
            PageState::PresentationFailed("no shared memory".into()),
            PageState::Refused("file:".into()),
            PageState::Failed("boom".into()),
        ] {
            assert!(
                !covered.takes_input(),
                "{covered:?} forwards input to a page nobody sees"
            );
        }
    }

    #[test]
    fn a_restored_url_with_a_refused_scheme_is_not_restorable() {
        assert!(restorable("http://localhost:3000/"));
        assert!(!restorable("file:///etc/passwd"));
        assert!(!restorable("javascript:alert(1)"));
    }
}

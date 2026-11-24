use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Styled, div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::with_alpha;
use crate::ui_primitives::squircle::squircle_background_fill;
use crate::ui_primitives::{ROW_RADIUS, TooltipDelayExt, dismiss_button};

const STAR_PROMPT_LABEL: &str = "Star Paneflow on GitHub";
const REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StarPrompt {
    #[serde(default)]
    dismissed: bool,
}

impl StarPrompt {
    pub(crate) fn is_due(&self) -> bool {
        !self.dismissed
    }

    pub(crate) fn dismiss(&mut self) -> bool {
        if self.dismissed {
            return false;
        }
        self.dismissed = true;
        true
    }
}

fn star_prompt_path() -> Option<PathBuf> {
    paneflow_home::star_prompt_path()
}

fn read_from_disk(path: &Path) -> StarPrompt {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return StarPrompt::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        log::warn!(
            "star prompt: {} is not valid JSON ({error}), starting over",
            path.display()
        );
        StarPrompt::default()
    })
}

fn write_to_disk(path: &Path, prompt: &StarPrompt) {
    match serde_json::to_string_pretty(prompt) {
        Ok(json) => {
            if let Err(error) = paneflow_home::write_atomically(path, json.as_bytes()) {
                log::warn!("star prompt: could not write {}: {error}", path.display());
            }
        }
        Err(error) => log::warn!("star prompt: could not serialize: {error}"),
    }
}

pub(crate) fn load() -> StarPrompt {
    star_prompt_path()
        .map(|path| read_from_disk(&path))
        .unwrap_or_default()
}

fn display_url(url: &str) -> &str {
    url.strip_prefix("https://").unwrap_or(url)
}

impl PaneFlowApp {
    fn dismiss_star_prompt(&mut self, cx: &mut Context<Self>) {
        if !self.star_prompt.dismiss() {
            return;
        }
        self.persist_star_prompt(cx);
        cx.notify();
    }

    fn persist_star_prompt(&self, cx: &mut Context<Self>) {
        let Some(path) = star_prompt_path() else {
            return;
        };
        let prompt = self.star_prompt.clone();
        cx.background_spawn(async move {
            smol::unblock(move || write_to_disk(&path, &prompt)).await;
        })
        .detach();
    }

    pub(crate) fn render_sidebar_star_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.star_prompt.is_due() {
            return None;
        }
        let ui = crate::theme::ui_colors();
        let dismiss = dismiss_button(
            "sidebar-star-dismiss",
            "Dismiss",
            px(7.),
            ui.muted,
            ui.text,
            with_alpha(ui.text, 0.08),
        )
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            this.dismiss_star_prompt(cx);
        }));
        let banner = div()
            .id("sidebar-star-banner")
            .relative()
            .mx(px(6.))
            .mb(px(2.))
            .h(px(30.))
            .pl(px(8.))
            .pr(px(3.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(squircle_background_fill(
                ROW_RADIUS,
                crate::app::constants::star_banner_wash(),
            ))
            .role(gpui::accesskit::Role::Link)
            .aria_label(STAR_PROMPT_LABEL)
            .delayed_tooltip(crate::ui_primitives::text_tooltip(display_url(
                REPOSITORY_URL,
            )))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.open_external_url(REPOSITORY_URL.to_string(), cx);
                this.dismiss_star_prompt(cx);
            }))
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path("icons/star.svg")
                    .text_color(crate::app::constants::star_banner_glyph_color()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .truncate()
                    .child(STAR_PROMPT_LABEL),
            )
            .child(dismiss);
        Some(banner.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_prompt_shows_on_the_first_launch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("star-prompt.json");
        assert!(read_from_disk(&path).is_due());
    }

    #[test]
    fn dismissed_star_prompt_never_returns() {
        let mut prompt = StarPrompt::default();
        assert!(prompt.dismiss());
        assert!(!prompt.dismiss());
        assert!(!prompt.is_due());
    }

    #[test]
    fn dismissal_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("star-prompt.json");
        let mut prompt = StarPrompt::default();
        prompt.dismiss();
        write_to_disk(&path, &prompt);
        assert!(!read_from_disk(&path).is_due());
    }

    #[test]
    fn star_prompt_file_from_the_day_count_build_keeps_its_dismissal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("star-prompt.json");
        std::fs::write(
            &path,
            r#"{"active_days":3,"last_active_day":20781,"dismissed":true}"#,
        )
        .unwrap();
        assert!(!read_from_disk(&path).is_due());
    }

    #[test]
    fn unreadable_star_prompt_file_shows_the_prompt_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("star-prompt.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_from_disk(&path).is_due());
    }

    #[test]
    fn star_prompt_tooltip_names_the_repository_without_its_scheme() {
        assert_eq!(display_url(REPOSITORY_URL), "github.com/arthjean/paneflow");
    }
}

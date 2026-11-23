use gpui::{
    AnyElement, App, ClickEvent, Context, Div, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use paneflow_config::schema::PaneFlowConfig;

use crate::PaneFlowApp;
use crate::app::diff_dock::code::controls::EditorDisplay;
use crate::app::diff_dock::code::language_server;
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::settings::search::{self, Block, SearchCard};

const EDITOR_INTRO: &str = "How the Files dock editor colors code.";

impl PaneFlowApp {
    pub(crate) fn render_editor_heading(&self, title: Div) -> AnyElement {
        let ink = Ink::of(crate::theme::ui_colors());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(kit::TEXT_INSET))
            .pb(px(kit::SECTION_GAP))
            .child(title)
            .child(kit::page_description(ink, EDITOR_INTRO))
            .into_any_element()
    }

    pub(crate) fn render_editor_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.cached_config.editor.semantic_tokens_enabled();
        editor_section(
            Ink::of(crate::theme::ui_colors()),
            enabled,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.persist_semantic_tokens(!enabled, cx);
            }),
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.forget_language_server_permissions(cx);
            }),
        )
    }

    pub(crate) fn persist_semantic_tokens(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let value = semantic_tokens_value(&self.cached_config, enabled);
        self.persist_setting_or_revert(false, "editor", value, cx);
    }

    pub(crate) fn forget_language_server_permissions(&mut self, cx: &mut Context<Self>) {
        let hub = language_server::hub(cx);
        hub.forget_accepted();
        let trust_path = hub.trust_path();
        cx.spawn(async move |this, cx| {
            let outcome = smol::unblock(move || {
                language_server::forget_consents_blocking(trust_path.as_deref())
            })
            .await;
            let _ = this.update(cx, |this, cx| match outcome {
                Ok(()) => this.show_toast("Language server permissions forgotten", cx),
                Err(error) => {
                    log::warn!("language servers: could not forget the permissions: {error}");
                    this.show_toast("Could not forget language server permissions", cx);
                }
            });
        })
        .detach();
    }
}

pub(crate) const SEMANTIC_TOKENS_SWITCH: &str = "editor-semantic-tokens";
pub(crate) const SEMANTIC_TOKENS_ON: &str = "editor-semantic-tokens:on";
pub(crate) const SEMANTIC_TOKENS_OFF: &str = "editor-semantic-tokens:off";
pub(crate) const FORGET_PERMISSIONS_BUTTON: &str = "editor-forget-language-server-permissions";

pub(crate) fn semantic_tokens_value(config: &PaneFlowConfig, enabled: bool) -> serde_json::Value {
    EditorDisplay {
        semantic_tokens: enabled,
        ..EditorDisplay::from_config(&config.editor)
    }
    .to_config_value()
}

pub(crate) fn editor_section(
    ink: Ink,
    enabled: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_forget: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let mut first = true;
    let state = if enabled {
        SEMANTIC_TOKENS_ON
    } else {
        SEMANTIC_TOKENS_OFF
    };
    let card = SearchCard::new()
        .row(
            &search::SEMANTIC_HIGHLIGHTING,
            kit::row(
                ink,
                None,
                setting_text(&search::SEMANTIC_HIGHLIGHTING, ink),
                Some(
                    kit::switch(
                        SEMANTIC_TOKENS_SWITCH,
                        search::SEMANTIC_HIGHLIGHTING.title,
                        enabled,
                        ink,
                    )
                    .debug_selector(move || state.into())
                    .on_click(on_toggle)
                    .into_any_element(),
                ),
            ),
        )
        .row(
            &search::LANGUAGE_SERVER_PERMISSIONS,
            kit::row(
                ink,
                None,
                setting_text(&search::LANGUAGE_SERVER_PERMISSIONS, ink),
                Some(
                    kit::button(
                        FORGET_PERMISSIONS_BUTTON,
                        "Forget language server permissions",
                        Tone::Secondary,
                        Size::Small,
                        ink,
                    )
                    .debug_selector(|| FORGET_PERMISSIONS_BUTTON.into())
                    .on_click(on_forget)
                    .into_any_element(),
                ),
            ),
        );
    let (matched, rows) = card.into_rows();
    div()
        .flex()
        .flex_col()
        .child(
            Block::new("Language servers")
                .top_gap(kit::section_gap(&mut first))
                .found(matched)
                .child(kit::section(
                    ink,
                    "Language servers",
                    None,
                    kit::search_group(ink, rows),
                ))
                .finish(),
        )
        .into_any_element()
}

fn setting_text(copy: &search::SettingCopy, ink: Ink) -> Text {
    Text::new(copy.title).line(kit::line(ink, copy.description))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use gpui::{
        Context, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, size,
    };
    use paneflow_config::schema::PaneFlowConfig;

    use super::*;
    use crate::app::diff_dock::code::language_server::forget_consents_blocking;
    use crate::config_writer::{self, SettingWrite};

    struct EditorTab {
        config: PaneFlowConfig,
        config_path: PathBuf,
        trust_path: PathBuf,
    }

    impl EditorTab {
        fn toggle(&mut self) {
            let enabled = self.config.editor.semantic_tokens_enabled();
            let value = semantic_tokens_value(&self.config, !enabled);
            let stored = config_writer::stored_field(&self.config, "editor");
            self.config = config_writer::with_field(&self.config, false, "editor", value.clone())
                .expect("a valid editor value");
            let outcome = config_writer::save_setting_ordered_at(
                &self.config_path,
                false,
                "editor",
                value,
                config_writer::next_setting_sequence(),
            );
            if outcome == SettingWrite::Failed
                && let Some(restored) = config_writer::config_after_failed_write(
                    &self.config,
                    false,
                    "editor",
                    Some(stored),
                )
            {
                self.config = restored;
            }
        }
    }

    impl Render for EditorTab {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(editor_section(
                Ink::of(crate::theme::ui_colors()),
                self.config.editor.semantic_tokens_enabled(),
                cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.toggle();
                    cx.notify();
                }),
                cx.listener(|this, _: &ClickEvent, _w, _cx| {
                    forget_consents_blocking(Some(&this.trust_path)).expect("forget");
                }),
            ))
        }
    }

    fn open<'a>(cx: &'a mut TestAppContext, dir: &Path, file: &str) -> &'a mut VisualTestContext {
        let config_path = dir.join("paneflow.json");
        std::fs::write(&config_path, file).expect("config file");
        let config = paneflow_config::loader::try_parse_and_validate(file).expect("config");
        let trust_path = dir.join("language-server-trust.json");
        let (_view, cx) = cx.add_window_view(move |_window, _cx| EditorTab {
            config,
            config_path,
            trust_path,
        });
        cx.simulate_resize(size(px(900.), px(600.)));
        redraw(cx);
        cx
    }

    fn redraw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("the control is painted");
        cx.simulate_click(bounds.center(), Modifiers::default());
        redraw(cx);
    }

    fn stored_semantic_tokens(dir: &Path) -> Option<bool> {
        let text = std::fs::read_to_string(dir.join("paneflow.json")).expect("config file");
        let config = paneflow_config::loader::try_parse_and_validate(&text).expect("config");
        config.editor.semantic_tokens
    }

    #[gpui::test]
    fn the_switch_shows_the_setting_and_a_click_writes_it_to_the_config_file(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cx = open(cx, dir.path(), "{\"editor\": {\"minimap\": true}}");
        assert!(cx.debug_bounds(SEMANTIC_TOKENS_OFF).is_some());
        assert!(cx.debug_bounds(SEMANTIC_TOKENS_ON).is_none());

        click(cx, SEMANTIC_TOKENS_OFF);

        assert!(cx.debug_bounds(SEMANTIC_TOKENS_ON).is_some());
        assert_eq!(stored_semantic_tokens(dir.path()), Some(true));
        let text = std::fs::read_to_string(dir.path().join("paneflow.json")).expect("config");
        assert!(text.contains("\"minimap\": true"), "{text}");
    }

    #[gpui::test]
    fn a_click_on_an_unwritable_config_shows_the_value_actually_stored(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = "{\"editor\": {\"semantic_tokens\": false}}";
        let cx = open(cx, dir.path(), original);
        let path = dir.path().join("paneflow.json");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).expect("read only");

        click(cx, SEMANTIC_TOKENS_OFF);

        assert!(cx.debug_bounds(SEMANTIC_TOKENS_OFF).is_some());
        assert!(cx.debug_bounds(SEMANTIC_TOKENS_ON).is_none());
        assert_eq!(std::fs::read_to_string(&path).expect("config"), original);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).expect("writable again");
    }

    #[gpui::test]
    fn forget_language_server_permissions_empties_the_trust_file(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        let trust = dir.path().join("language-server-trust.json");
        paneflow_lsp::trust::TrustStore::load(&trust)
            .allow("rust-analyzer", dir.path())
            .expect("an allowed root");
        let cx = open(cx, dir.path(), "{}");

        click(cx, FORGET_PERMISSIONS_BUTTON);

        assert!(
            !paneflow_lsp::trust::TrustStore::load(&trust).is_allowed("rust-analyzer", dir.path())
        );
        let text = std::fs::read_to_string(&trust).expect("trust file");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).expect("json")["allowed"],
            serde_json::json!([])
        );
    }
}

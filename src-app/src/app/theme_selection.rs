use gpui::Context;

use crate::{PaneFlowApp, ThemeMode, config_writer};

impl PaneFlowApp {
    pub(crate) fn current_theme_name(&self) -> String {
        self.cached_config
            .theme
            .as_deref()
            .and_then(crate::theme::canonical_theme_name)
            .unwrap_or(crate::theme::DEFAULT_THEME)
            .to_string()
    }

    pub(crate) fn current_theme_preset(&self) -> &'static crate::theme::ThemePreset {
        crate::theme::preset_for_theme(&self.current_theme_name())
    }

    pub(crate) fn persist_theme_selection(
        &mut self,
        mode: ThemeMode,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        self.theme_mode = mode;
        self.cached_config.theme_mode = Some(mode.as_config_str().to_string());
        self.cached_config.theme = Some(name.to_string());
        crate::theme::set_active_theme(Some(name));
        crate::theme::publish_theme_generation(cx);
        cx.notify();
        let mode_value = serde_json::Value::String(mode.as_config_str().to_string());
        let name_value = serde_json::Value::String(name.to_string());
        cx.spawn(async move |this, cx| {
            let ok = smol::unblock(move || {
                config_writer::save_config_values_checked([
                    ("theme_mode", mode_value),
                    ("theme", name_value),
                ])
            })
            .await;
            if !ok {
                log::warn!("theme: failed to persist the selection; it is in-memory only");
                let _ = this.update(cx, |this, cx| {
                    this.show_toast("Could not save theme", cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn apply_theme_preset(
        &mut self,
        preset: &crate::theme::ThemePreset,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let mode = self.theme_mode;
        let name = mode.resolved_theme_name(preset, window.appearance());
        self.persist_theme_selection(mode, name, cx);
    }

    pub(crate) fn reset_theme_selection(&mut self, cx: &mut Context<Self>) {
        let ok = config_writer::save_config_values_checked([
            ("theme_mode", serde_json::Value::Null),
            ("theme", serde_json::Value::Null),
        ]);
        if !ok {
            self.show_toast("Could not reset theme", cx);
            return;
        }
        self.theme_mode = ThemeMode::Dark;
        self.cached_config.theme_mode = None;
        self.cached_config.theme = None;
        crate::theme::set_active_theme(None);
        crate::theme::publish_theme_generation(cx);
        cx.notify();
    }
}

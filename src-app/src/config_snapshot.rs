use std::sync::Arc;

use gpui::{App, Global};
use paneflow_config::schema::PaneFlowConfig;

struct ConfigSnapshot(Arc<PaneFlowConfig>);

impl Global for ConfigSnapshot {}

pub(crate) fn publish(config: &PaneFlowConfig, cx: &mut App) {
    cx.set_global(ConfigSnapshot(Arc::new(config.clone())));
}

pub(crate) fn current(cx: &App) -> Arc<PaneFlowConfig> {
    cx.try_global::<ConfigSnapshot>()
        .map(|snapshot| Arc::clone(&snapshot.0))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_published_value_is_what_the_next_pane_reads(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let next = crate::config_writer::with_field(
                &PaneFlowConfig::default(),
                true,
                "scrollback_lines",
                serde_json::json!(4321),
            )
            .expect("valid setting");
            publish(&next, cx);
            assert_eq!(
                current(cx)
                    .terminal
                    .clone()
                    .unwrap_or_default()
                    .scrollback_lines,
                Some(4321),
                "a setting that is not on disk yet is already visible"
            );
        });
    }
}

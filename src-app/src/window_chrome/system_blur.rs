use gpui::WindowBackgroundAppearance;

use super::veil::Backdrop;

pub(crate) fn appearance(backdrop: Backdrop) -> WindowBackgroundAppearance {
    if backdrop.is_blended() {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AppliedAppearance(Option<WindowBackgroundAppearance>);

impl AppliedAppearance {
    fn change_for(&mut self, backdrop: Backdrop) -> Option<WindowBackgroundAppearance> {
        let next = appearance(backdrop);
        if self.0 == Some(next) {
            return None;
        }
        self.0 = Some(next);
        Some(next)
    }

    pub(crate) fn sync(&mut self, window: &gpui::Window, backdrop: Backdrop) {
        if let Some(next) = self.change_for(backdrop) {
            window.set_background_appearance(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_chrome::veil::BlurSupport;
    use paneflow_config::schema::{BackgroundStyle, PaneFlowConfig};

    struct Probe;

    impl gpui::Render for Probe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::Empty
        }
    }

    fn config(json: serde_json::Value) -> PaneFlowConfig {
        serde_json::from_value(json).expect("the config deserializes")
    }

    #[test]
    fn blended_asks_for_the_untinted_system_blur() {
        for blur in [BlurSupport::Full, BlurSupport::None] {
            assert_eq!(
                appearance(Backdrop::new(BackgroundStyle::Blended, true, blur)),
                WindowBackgroundAppearance::Blurred
            );
        }
    }

    #[test]
    fn themed_keeps_the_appearance_a_config_without_material_keys_had() {
        for surface_has_alpha in [true, false] {
            assert_eq!(
                appearance(Backdrop::new(
                    BackgroundStyle::Themed,
                    surface_has_alpha,
                    BlurSupport::Full
                )),
                WindowBackgroundAppearance::Transparent
            );
        }
        assert_eq!(
            appearance(Backdrop::THEMED),
            WindowBackgroundAppearance::Transparent
        );
    }

    #[test]
    fn an_invalid_style_removes_the_blur_view() {
        let mut applied = AppliedAppearance::default();
        let blended = config(serde_json::json!({ "background_style": "blended" }));
        let invalid = config(serde_json::json!({ "background_style": "mica" }));

        assert_eq!(
            applied.change_for(Backdrop::new(
                blended.resolved_background_style(),
                true,
                BlurSupport::Full
            )),
            Some(WindowBackgroundAppearance::Blurred)
        );
        assert_eq!(
            applied.change_for(Backdrop::new(
                invalid.resolved_background_style(),
                true,
                BlurSupport::Full
            )),
            Some(WindowBackgroundAppearance::Transparent)
        );
    }

    #[test]
    fn the_appearance_is_set_only_when_it_changes() {
        let mut applied = AppliedAppearance::default();
        let blended = Backdrop::new(BackgroundStyle::Blended, true, BlurSupport::Full);
        let themed = Backdrop::new(BackgroundStyle::Themed, true, BlurSupport::Full);

        assert_eq!(
            applied.change_for(blended),
            Some(WindowBackgroundAppearance::Blurred)
        );
        assert_eq!(applied.change_for(blended), None);
        assert_eq!(
            applied.change_for(themed),
            Some(WindowBackgroundAppearance::Transparent)
        );
        assert_eq!(applied.change_for(themed), None);
        assert_eq!(
            applied.change_for(blended),
            Some(WindowBackgroundAppearance::Blurred)
        );
    }

    #[gpui::test]
    fn a_fullscreen_window_drops_the_blur_and_gets_it_back_on_leaving(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, cx) = cx.add_window_view(|_, _| Probe);
        let mut applied = AppliedAppearance::default();
        let mut sync = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| {
                let backdrop =
                    crate::window_chrome::backdrop_for(BackgroundStyle::Blended, window, true);
                applied.sync(window, backdrop);
                (backdrop.is_blended(), applied)
            })
        };

        let (blended, windowed) = sync(cx);
        assert!(blended);
        assert_eq!(
            windowed,
            AppliedAppearance(Some(WindowBackgroundAppearance::Blurred))
        );

        cx.update(|window, _| window.toggle_fullscreen());
        cx.run_until_parked();
        let (blended, fullscreen) = sync(cx);
        assert!(!blended, "a fullscreen window keeps the Themed fills");
        assert_eq!(
            fullscreen,
            AppliedAppearance(Some(WindowBackgroundAppearance::Transparent))
        );

        cx.update(|window, _| window.toggle_fullscreen());
        cx.run_until_parked();
        let (blended, restored) = sync(cx);
        assert!(blended);
        assert_eq!(
            restored,
            AppliedAppearance(Some(WindowBackgroundAppearance::Blurred))
        );
    }
}

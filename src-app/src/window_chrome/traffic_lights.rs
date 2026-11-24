use gpui::{Pixels, Point, point, px};

const LEADING_INSET: Pixels = px(12.);
const APPKIT_BUTTON_FRAME_HEIGHT: Pixels = px(16.);

pub(crate) fn origin_centered_in(title_bar_height: Pixels) -> Point<Pixels> {
    let top = ((title_bar_height - APPKIT_BUTTON_FRAME_HEIGHT) * 0.5).max(px(0.));
    point(LEADING_INSET, top)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AppliedTrafficLights(Option<Pixels>);

impl AppliedTrafficLights {
    fn change_for(&mut self, title_bar_height: Pixels) -> Option<Point<Pixels>> {
        if self.0 == Some(title_bar_height) {
            return None;
        }
        self.0 = Some(title_bar_height);
        Some(origin_centered_in(title_bar_height))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn sync(&mut self, window: &gpui::Window, title_bar_height: Pixels) {
        if let Some(origin) = self.change_for(title_bar_height) {
            window.set_traffic_light_position(origin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_share_the_vertical_center_of_the_main_title_bar() {
        let height = crate::app::constants::TITLE_BAR_MIN_HEIGHT;
        let origin = origin_centered_in(height);
        assert_eq!(origin.y + APPKIT_BUTTON_FRAME_HEIGHT * 0.5, height * 0.5);
        assert_eq!(origin, point(px(12.), px(12.)));
    }

    #[test]
    fn a_taller_title_bar_moves_the_buttons_down_to_its_center() {
        assert_eq!(origin_centered_in(px(44.)).y, px(14.));
        assert_eq!(origin_centered_in(px(56.)).y, px(20.));
    }

    #[test]
    fn a_title_bar_shorter_than_the_buttons_pins_them_to_the_top() {
        assert_eq!(origin_centered_in(px(10.)).y, px(0.));
    }

    #[test]
    fn the_position_is_applied_only_when_the_title_bar_height_changes() {
        let mut applied = AppliedTrafficLights::default();
        assert_eq!(applied.change_for(px(40.)), Some(point(px(12.), px(12.))));
        assert_eq!(applied.change_for(px(40.)), None);
        assert_eq!(applied.change_for(px(48.)), Some(point(px(12.), px(16.))));
    }
}

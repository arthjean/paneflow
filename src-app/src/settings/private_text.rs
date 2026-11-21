use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, Div, ElementId, Hsla, IntoElement, ParentElement, SharedString,
    Stateful, Styled, div, prelude::*, px,
};

use crate::settings::components::keyboard_focus_inset;
use crate::ui_primitives::{AccessibleControlExt, ease_out_cubic, reduce_motion};

const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const KEPT: [char; 5] = ['@', '.', '-', '_', '+'];
const BLUR_RADIUS: f32 = 4.;
const BLUR_TAPS: usize = 32;
const TAP_ALPHA: f32 = 0.08;
const GOLDEN_ANGLE: f32 = 2.399_963;
const FOCUS_RADIUS: f32 = 4.;
const TRANSITION: Duration = Duration::from_millis(180);
pub(crate) const SHOW_LABEL: &str = "Show email";
pub(crate) const HIDE_LABEL: &str = "Hide email";

#[derive(Default)]
pub(crate) struct Reveals {
    shown: HashSet<String>,
    last: Option<(String, Instant)>,
}

impl Reveals {
    pub(crate) fn is_shown(&self, key: &str) -> bool {
        self.shown.contains(key)
    }

    pub(crate) fn toggle(&mut self, key: &str) {
        if !self.shown.remove(key) {
            self.shown.insert(key.to_string());
        }
        self.last = Some((key.to_string(), Instant::now()));
    }

    fn moving(&self, key: &str) -> bool {
        !reduce_motion()
            && self
                .last
                .as_ref()
                .is_some_and(|(last, at)| last == key && at.elapsed() < TRANSITION)
    }
}

pub(crate) fn is_private(text: &str) -> bool {
    text.contains('@')
}

pub(crate) fn stand_in(value: &str) -> String {
    let mut state = value.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    value
        .chars()
        .map(|ch| {
            if KEPT.contains(&ch) {
                return ch;
            }
            state = (state ^ (state >> 13)).wrapping_mul(0x85eb_ca6b);
            state = (state ^ (state >> 16)).wrapping_mul(0xc2b2_ae35);
            char::from(ALPHABET[state as usize % ALPHABET.len()])
        })
        .collect()
}

fn blur_taps(radius: f32) -> impl Iterator<Item = (f32, f32)> {
    (0..BLUR_TAPS).map(move |tap| {
        let reach = radius * ((tap as f32 + 0.5) / BLUR_TAPS as f32).sqrt();
        let angle = tap as f32 * GOLDEN_ANGLE;
        (reach * angle.cos(), reach * angle.sin())
    })
}

fn blurred(text: SharedString, color: Hsla, spread: f32) -> Div {
    let line = || div().whitespace_nowrap();
    if spread <= 0. {
        return line().text_color(color).child(text);
    }
    let mut frame = line()
        .relative()
        .text_color(color.opacity(1. - spread))
        .child(text.clone());
    for (x, y) in blur_taps(BLUR_RADIUS * spread) {
        frame = frame.child(
            line()
                .absolute()
                .left(px(x))
                .top(px(y))
                .text_color(color.opacity(TAP_ALPHA * spread))
                .child(text.clone()),
        );
    }
    frame
}

pub(crate) fn private_text(
    key: &str,
    value: &str,
    reveals: &Reveals,
    color: Hsla,
    accent: Hsla,
) -> Stateful<Div> {
    let shown = reveals.is_shown(key);
    let real: SharedString = value.to_string().into();
    let veiled: SharedString = stand_in(value).into();
    let content = if reveals.moving(key) {
        let phase = if shown { "show" } else { "hide" };
        div()
            .with_animation(
                ElementId::from(SharedString::from(format!("private-text-{phase}-{key}"))),
                Animation::new(TRANSITION).with_easing(ease_out_cubic),
                move |_, delta| match (shown, delta < 1.) {
                    (true, _) => blurred(real.clone(), color, 1. - delta),
                    (false, true) => blurred(real.clone(), color, delta),
                    (false, false) => blurred(veiled.clone(), color, 1.),
                },
            )
            .into_any_element()
    } else if shown {
        blurred(real, color, 0.).into_any_element()
    } else {
        blurred(veiled, color, 1.).into_any_element()
    };
    let ring = keyboard_focus_inset(accent);
    div()
        .id(SharedString::from(format!("private-text-{key}")))
        .min_w_0()
        .rounded(px(FOCUS_RADIUS))
        .cursor_pointer()
        .tab_index(0)
        .focus_visible(move |style| style.shadow(ring.clone()))
        .accessible_control(
            gpui::accesskit::Role::Button,
            if shown { HIDE_LABEL } else { SHOW_LABEL },
        )
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stand_in_keeps_the_shape_and_never_the_letters() {
        let email = "arthur.jean+work@example-mail.dev";
        let veiled = stand_in(email);
        assert_eq!(veiled.chars().count(), email.chars().count());
        for (real, shown) in email.chars().zip(veiled.chars()) {
            if KEPT.contains(&real) {
                assert_eq!(shown, real);
            } else {
                assert!(ALPHABET.contains(&(shown as u8)), "{shown}");
            }
        }
        assert_ne!(veiled, email);
        assert_eq!(
            stand_in(email),
            veiled,
            "the same address gives the same stand-in"
        );
        assert_ne!(stand_in("someone@else.dev"), stand_in("someone@elsf.dev"));
    }

    #[test]
    fn the_blur_taps_fill_a_disk_of_the_radius() {
        let taps: Vec<_> = blur_taps(BLUR_RADIUS).collect();
        assert_eq!(taps.len(), BLUR_TAPS);
        let reach = |(x, y): (f32, f32)| (x * x + y * y).sqrt();
        assert!(taps.iter().all(|tap| reach(*tap) <= BLUR_RADIUS + 1e-4));
        assert!(taps.iter().any(|tap| reach(*tap) > BLUR_RADIUS * 0.9));
        assert!(taps.iter().any(|tap| reach(*tap) < BLUR_RADIUS * 0.25));
        for quadrant in [(1., 1.), (-1., 1.), (-1., -1.), (1., -1.)] {
            assert!(
                taps.iter()
                    .any(|(x, y)| x * quadrant.0 > 0. && y * quadrant.1 > 0.),
                "every quadrant holds a tap"
            );
        }
    }

    #[test]
    fn a_toggle_shows_then_hides_one_address() {
        let mut reveals = Reveals::default();
        assert!(!reveals.is_shown("a"));
        reveals.toggle("a");
        assert!(reveals.is_shown("a"));
        assert!(!reveals.is_shown("b"));
        reveals.toggle("a");
        assert!(!reveals.is_shown("a"));
    }
}

use std::time::Duration;

use gpui::{
    AnyElement, Bounds, ContentMask, FontId, GlyphId, Hsla, IntoElement, ParentElement, Pixels,
    Point, SharedString, Styled, TextOverflow, TruncateFrom, canvas, div, point, px, size,
};

use super::{animation_clock, lerp_color};

const SHIMMER_FRAME: Duration = Duration::from_millis(animation_clock::ANIMATION_STEP_MS);
const SHIMMER_SWEEP: Duration = Duration::from_millis(3_400);
const SHIMMER_HALF_WIDTH: f32 = 0.36;
const SHIMMER_STRIP_WIDTH: f32 = 2.0;
const SHIMMER_INK_OVERHANG_EM: f32 = 0.2;
const SHIMMER_COPY_SPACING: f32 = 3.0;
const SHIMMER_TRAVEL: f32 = 6.0;

pub(crate) fn shimmer_glint(text: Hsla) -> Hsla {
    let extreme = if text.l >= 0.5 {
        gpui::white()
    } else {
        gpui::black()
    };
    extreme.opacity(text.a)
}

pub(crate) fn working_title_shimmer(ui: crate::theme::UiColors) -> (Hsla, Hsla) {
    (ui.muted, shimmer_glint(ui.text))
}

#[cfg(test)]
thread_local! {
    static PINNED_PHASE: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    static LINE_SHAPES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static GLYPH_PAINTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn shimmer_phase(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / SHIMMER_SWEEP.as_secs_f32()).fract()
}

fn shimmer_amount(x: f32, phase: f32) -> f32 {
    let primary_center = -2.5 + phase.clamp(0.0, 1.0) * SHIMMER_TRAVEL;
    (-2..=2)
        .map(|copy| primary_center + copy as f32 * SHIMMER_COPY_SPACING)
        .map(|center| (1.0 - (x - center).abs() / SHIMMER_HALF_WIDTH).clamp(0.0, 1.0))
        .fold(0.0, f32::max)
}

pub(crate) fn until_next_shimmer_frame(elapsed: Duration) -> Duration {
    let sweep = SHIMMER_SWEEP.as_secs_f32();
    let entered = (-2.5 + shimmer_phase(elapsed) * SHIMMER_TRAVEL + SHIMMER_HALF_WIDTH)
        .rem_euclid(SHIMMER_COPY_SPACING);
    let visible = 1.0 + 2.0 * SHIMMER_HALF_WIDTH;
    let settle = SHIMMER_TRAVEL * SHIMMER_FRAME.as_secs_f32() / sweep;
    if entered < visible + settle {
        animation_clock::until_next_tick(elapsed, SHIMMER_FRAME)
    } else {
        let until_next_copy =
            Duration::from_secs_f32((SHIMMER_COPY_SPACING - entered) / SHIMMER_TRAVEL * sweep);
        until_next_copy + animation_clock::until_next_tick(elapsed + until_next_copy, SHIMMER_FRAME)
    }
}

struct Strip {
    left: f32,
    right: f32,
    color: Hsla,
}

struct InkGlyph {
    origin: Point<Pixels>,
    left: Pixels,
    right: Pixels,
    font_id: FontId,
    glyph_id: GlyphId,
}

pub(crate) fn shimmer_text(
    label: SharedString,
    base: Hsla,
    peak: Hsla,
    sizer: impl IntoElement,
) -> AnyElement {
    let phase = shimmer_phase(animation_clock::animation_epoch().elapsed());
    #[cfg(test)]
    let phase = PINNED_PHASE.with(|p| p.get()).unwrap_or(phase);
    let overlay = canvas(
        move |bounds, window, cx| {
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let available = bounds.size.width;
            let plain = [style.to_run(label.len())];
            #[cfg(test)]
            LINE_SHAPES.with(|count| count.set(count.get() + 1));
            let unclipped = window
                .text_system()
                .shape_line(label.clone(), font_size, &plain, None);
            let truncation = style
                .text_overflow
                .clone()
                .filter(|_| unclipped.width() > available)
                .map(|overflow| match overflow {
                    TextOverflow::Truncate(affix) => (affix, TruncateFrom::End),
                    TextOverflow::TruncateStart(affix) => (affix, TruncateFrom::Start),
                    TextOverflow::TruncateMiddle(affix) => (affix, TruncateFrom::Middle),
                });
            let line = match truncation {
                Some((affix, from)) => {
                    let (shown, runs) = cx
                        .text_system()
                        .line_wrapper(style.font(), font_size)
                        .truncate_line(label.clone(), available, &affix, &plain, from);
                    #[cfg(test)]
                    LINE_SHAPES.with(|count| count.set(count.get() + 1));
                    window
                        .text_system()
                        .shape_line(shown, font_size, &runs, None)
                }
                None => unclipped,
            };
            let text_width = f32::from(line.width().min(available));
            let strip_count = (text_width / SHIMMER_STRIP_WIDTH).ceil() as usize;
            let mut strips: Vec<Strip> = Vec::new();
            for ix in 0..strip_count {
                let left = ix as f32 * SHIMMER_STRIP_WIDTH;
                let ink_right = ((ix + 1) as f32 * SHIMMER_STRIP_WIDTH).min(text_width);
                let right = if ix + 1 == strip_count {
                    f32::from(available)
                } else {
                    ink_right
                };
                let x = (left + ink_right) * 0.5 / text_width.max(1.0);
                let amount = shimmer_amount(x, phase);
                let color = if amount > 0.001 {
                    lerp_color(base, peak, amount)
                } else {
                    base
                };
                match strips.last_mut() {
                    Some(last) if last.color == color => last.right = right,
                    _ => strips.push(Strip { left, right, color }),
                }
            }
            (line, strips)
        },
        move |bounds, (line, strips), window, _| {
            let line_height = window.line_height();
            let top = bounds.origin.y + (bounds.size.height - line_height) / 2.;
            let baseline = top + (line_height - line.ascent - line.descent) / 2. + line.ascent;
            let font_size = line.font_size;
            let overhang = font_size * SHIMMER_INK_OVERHANG_EM;
            let positioned = line
                .runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(move |glyph| (run.font_id, glyph)))
                .collect::<Vec<_>>();
            let glyphs = positioned
                .iter()
                .enumerate()
                .filter(|(_, (_, glyph))| !glyph.is_emoji)
                .map(|(ix, &(font_id, glyph))| {
                    let next_x = positioned
                        .get(ix + 1)
                        .map_or(line.width, |(_, next)| next.position.x);
                    InkGlyph {
                        origin: point(
                            bounds.origin.x + glyph.position.x,
                            baseline + glyph.position.y,
                        ),
                        left: bounds.origin.x + glyph.position.x - overhang,
                        right: bounds.origin.x + next_x + overhang,
                        font_id,
                        glyph_id: glyph.id,
                    }
                })
                .collect::<Vec<_>>();
            window.paint_layer(bounds, |window| {
                let mut first = 0;
                for strip in strips {
                    let left = bounds.origin.x + px(strip.left);
                    let right = bounds.origin.x + px(strip.right);
                    while glyphs.get(first).is_some_and(|glyph| glyph.right <= left) {
                        first += 1;
                    }
                    let mask = ContentMask {
                        bounds: Bounds {
                            origin: point(left, bounds.origin.y),
                            size: size(right - left, bounds.size.height),
                        },
                    };
                    window.with_content_mask(Some(mask), |window| {
                        for glyph in glyphs[first..]
                            .iter()
                            .take_while(|glyph| glyph.left < right)
                        {
                            #[cfg(test)]
                            GLYPH_PAINTS.with(|count| count.set(count.get() + 1));
                            let _ = window.paint_glyph(
                                glyph.origin,
                                glyph.font_id,
                                glyph.glyph_id,
                                font_size,
                                strip.color,
                            );
                        }
                    });
                }
            });
        },
    )
    .absolute()
    .inset_0();
    div()
        .relative()
        .min_w_0()
        .overflow_hidden()
        .text_color(gpui::transparent_black())
        .child(sizer)
        .child(overlay)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shimmer_peaks_under_its_center_and_fades_beyond_the_shoulder() {
        let phase = (2.5 + 0.5) / 6.0;
        assert!((shimmer_amount(0.5, phase) - 1.0).abs() < 1e-5);
        assert_eq!(shimmer_amount(1.0, phase), 0.0);
        assert_eq!(shimmer_phase(Duration::ZERO), 0.0);
        assert!((shimmer_phase(Duration::from_millis(1_700)) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn the_clock_ticks_while_a_band_crosses_the_text_and_sleeps_between_passes() {
        let at = |center: f32| {
            Duration::from_secs_f32((center + 2.5) / SHIMMER_TRAVEL * SHIMMER_SWEEP.as_secs_f32())
        };
        assert!(until_next_shimmer_frame(at(0.5)) <= SHIMMER_FRAME + Duration::from_millis(1));
        assert!(
            until_next_shimmer_frame(at(1.0 + SHIMMER_HALF_WIDTH + 0.01))
                <= SHIMMER_FRAME + Duration::from_millis(1),
            "one more frame erases the band that just left"
        );
        let idle = until_next_shimmer_frame(at(2.04));
        assert!(
            idle.as_secs_f32() >= 0.341 && idle <= Duration::from_millis(341) + SHIMMER_FRAME,
            "sleeps until the first spinner step after the next copy enters: {idle:?}"
        );
    }

    #[test]
    fn every_shimmer_wake_lands_on_a_spinner_step_so_both_share_one_frame() {
        let step = Duration::from_millis(animation_clock::ANIMATION_STEP_MS);
        assert_eq!(SHIMMER_FRAME, step);
        for offset_us in (0..2 * SHIMMER_SWEEP.as_micros() as u64).step_by(1_337) {
            let elapsed = Duration::from_micros(offset_us);
            let wake_at = elapsed + until_next_shimmer_frame(elapsed);
            let spinner_asleep_since = wake_at - Duration::from_millis(2);
            assert_eq!(
                spinner_asleep_since + animation_clock::until_next_tick(spinner_asleep_since, step),
                wake_at,
                "the shimmer from {elapsed:?} wakes off the spinner grid"
            );
        }
    }

    const ROW_LABEL: &str = "Running focused E2E files for EP-004 and the rest";

    struct ShimmerRow;

    impl gpui::Render for ShimmerRow {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let label = SharedString::from(ROW_LABEL);
            let text = gpui::hsla(0.0, 0.0, 0.93, 1.0);
            div()
                .w(px(240.))
                .text_size(px(14.))
                .line_height(px(22.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(shimmer_text(
                    label.clone(),
                    text,
                    shimmer_glint(text),
                    label,
                ))
        }
    }

    fn shapes_and_paints_of_one_frame_at(phase: f32, cx: &mut gpui::TestAppContext) -> (u64, u64) {
        PINNED_PHASE.with(|pinned| pinned.set(Some(phase)));
        let (view, cx) = cx.add_window_view(|_, _| ShimmerRow);
        cx.simulate_resize(size(px(400.), px(80.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        view.update(cx, |_, cx| cx.notify());
        LINE_SHAPES.with(|count| count.set(0));
        GLYPH_PAINTS.with(|count| count.set(0));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        PINNED_PHASE.with(|pinned| pinned.set(None));
        (
            LINE_SHAPES.with(|count| count.get()),
            GLYPH_PAINTS.with(|count| count.get()),
        )
    }

    #[gpui::test]
    fn a_shimmer_frame_shapes_its_line_once_and_repaints_only_the_glyphs_a_strip_crosses(
        cx: &mut gpui::TestAppContext,
    ) {
        let (shapes, paints) = shapes_and_paints_of_one_frame_at(0.5, cx);
        assert_eq!(
            shapes, 2,
            "one shape for the label and one for its truncation, however many strips the band spans"
        );
        let max_strips = 240 / SHIMMER_STRIP_WIDTH as u64;
        let glyphs = ROW_LABEL.len() as u64;
        assert!(
            paints <= 3 * max_strips + glyphs,
            "a strip repaints only the glyphs it crosses: {paints}"
        );
    }

    #[gpui::test]
    fn between_two_passes_the_resting_text_is_inked_once_so_no_stroke_gains_weight(
        cx: &mut gpui::TestAppContext,
    ) {
        let band_off_the_text = 0.75;
        assert_eq!(shimmer_amount(0.0, band_off_the_text), 0.0);
        assert_eq!(shimmer_amount(1.0, band_off_the_text), 0.0);
        let (_, paints) = shapes_and_paints_of_one_frame_at(band_off_the_text, cx);
        let glyphs_and_ellipsis = ROW_LABEL.len() as u64 + 1;
        assert!(
            (1..=glyphs_and_ellipsis).contains(&paints),
            "one resting span inks each visible glyph exactly once: {paints}"
        );
    }

    #[test]
    fn a_working_title_rests_muted_and_glints_to_the_extreme_of_the_text_color() {
        let ui = crate::theme::ui_colors();
        let (base, peak) = working_title_shimmer(ui);
        assert_eq!(base, ui.muted);
        assert_eq!(peak, shimmer_glint(ui.text));
        assert_ne!(
            base, ui.text,
            "the resting title steps back from the text color"
        );
    }

    #[test]
    fn the_glint_pushes_light_text_to_white_and_dark_text_to_black() {
        let light = gpui::hsla(0.0, 0.0, 0.93, 1.0);
        let dark = gpui::hsla(0.0, 0.0, 0.09, 1.0);
        assert_eq!(shimmer_glint(light), gpui::white());
        assert_eq!(shimmer_glint(dark), gpui::black());
    }
}

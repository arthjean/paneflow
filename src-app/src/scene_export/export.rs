//! Converts a captured GPUI frame into a `paneflow-scene/1` document: boxes,
//! shadows, vector paths, text runs and icon references, in the window's
//! logical pixels, ordered the way GPUI painted them.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use gpui::{
    Bounds, FontId, Hsla, PathCurve, PathPaint, PathTile, PlatformTextSystem, SUBPIXEL_VARIANTS_X,
    SUBPIXEL_VARIANTS_Y, ScaledPixels,
};
use serde_json::{Value, json};

use super::spy::{CapturedScene, RecordedLayout, SpyTextSystem, TileKey, TileSource, tile_key};

pub(crate) const FORMAT: &str = "paneflow-scene/1";

const EPS: f32 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn new(bounds: &Bounds<ScaledPixels>, inset: f32) -> Self {
        Self {
            x: bounds.origin.x.0 - inset,
            y: bounds.origin.y.0 - inset,
            w: bounds.size.width.0,
            h: bounds.size.height.0,
        }
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn is_empty(&self) -> bool {
        self.w <= EPS || self.h <= EPS
    }

    fn contains(&self, other: &Rect) -> bool {
        self.x <= other.x + EPS
            && self.y <= other.y + EPS
            && self.right() >= other.right() - EPS
            && self.bottom() >= other.bottom() - EPS
    }

    fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect {
            x,
            y,
            w: (right - x).max(0.),
            h: (bottom - y).max(0.),
        }
    }

    fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    fn json(&self) -> Value {
        json!([r(self.x), r(self.y), r(self.w), r(self.h)])
    }
}

/// Rounds to 1/1000 px so the document stays stable and small.
fn r(value: f32) -> f64 {
    (f64::from(value) * 1000.).round() / 1000.
}

fn css(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    let (red, green, blue, alpha) = (
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b),
        channel(rgba.a),
    );
    if alpha == 255 {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}")
    }
}

fn background(background: &gpui::Background) -> Value {
    match background.as_solid() {
        Some(color) => Value::String(css(color)),
        None => serde_json::to_value(background).unwrap_or(Value::Null),
    }
}

fn radii(corners: &gpui::Corners<ScaledPixels>) -> Option<Value> {
    let values = [
        corners.top_left.0,
        corners.top_right.0,
        corners.bottom_right.0,
        corners.bottom_left.0,
    ];
    values
        .iter()
        .any(|value| *value > EPS)
        .then(|| json!(values.map(r)))
}

/// Paint rank of each primitive kind inside one draw order, as GPUI batches them.
const RANK_SHADOW: u32 = 0;
const RANK_QUAD: u32 = 1;
const RANK_PATH: u32 = 2;
const RANK_UNDERLINE: u32 = 3;
const RANK_MONO: u32 = 4;
const RANK_POLY: u32 = 6;

struct Element {
    order: u32,
    rank: u32,
    seq: usize,
    value: Value,
}

pub(crate) struct Export {
    pub document: Value,
    /// Icon SVG paths referenced by the document, relative to the app's assets.
    pub icons: Vec<String>,
    /// Polychrome sprites to crop from the lossless frame: (id, rect in window px).
    pub images: Vec<(String, [f32; 4])>,
    pub report: Vec<String>,
}

pub(crate) struct Input<'a> {
    pub scene: &'a CapturedScene,
    pub tiles: &'a HashMap<TileKey, TileSource>,
    pub text: &'a SpyTextSystem,
    pub inset: f32,
    pub width: f32,
    pub height: f32,
}

pub(crate) fn build(input: Input<'_>) -> Result<Export> {
    let window = Rect {
        x: 0.,
        y: 0.,
        w: input.width,
        h: input.height,
    };
    let mut elements: Vec<Element> = Vec::new();
    let mut report = Vec::new();
    // `seq` breaks ties inside one draw order and kind: the primitive's index in
    // GPUI's sorted array, which keeps its insertion order.
    let push = |elements: &mut Vec<Element>, order: u32, rank: u32, seq: usize, value: Value| {
        elements.push(Element {
            order,
            rank,
            seq,
            value,
        });
    };
    let clip_of = |mask: &Rect, extent: &Rect| -> Option<Value> {
        (!mask.contains(extent)).then(|| mask.json())
    };

    // ---- shadows ----
    for (seq, shadow) in input.scene.shadows.iter().enumerate() {
        let mask = Rect::new(&shadow.content_mask.bounds, input.inset).intersect(&window);
        let bounds = Rect::new(&shadow.bounds, input.inset);
        if mask.intersect(&bounds).is_empty() {
            continue;
        }
        let shape = Rect::new(&shadow.shape_bounds, input.inset);
        push(
            &mut elements,
            shadow.order,
            RANK_SHADOW,
            seq,
            json!({
                "kind": "shadow",
                "inset": shadow.inset.as_bool(),
                "rect": bounds.json(),
                "radii": radii(&shadow.corner_radii),
                "shape": shape.json(),
                "shape_radii": radii(&shadow.shape_corner_radii),
                "sigma": r(shadow.blur_radius.0),
                "color": css(shadow.color),
                "clip": clip_of(&mask, &bounds),
            }),
        );
    }

    // ---- quads: undo GPUI's interior + strips split ----
    struct Piece {
        order: u32,
        bounds: Rect,
        mask: Rect,
        quad: gpui::Quad,
    }
    let pieces: Vec<Piece> = input
        .scene
        .quads
        .iter()
        .map(|quad| Piece {
            order: quad.order,
            bounds: Rect::new(&quad.bounds, input.inset),
            mask: Rect::new(&quad.content_mask.bounds, input.inset),
            quad: *quad,
        })
        .collect();
    let plain = |quad: &gpui::Quad| {
        radii(&quad.corner_radii).is_none()
            && quad.border_widths.top.0 <= EPS
            && quad.border_widths.right.0 <= EPS
            && quad.border_widths.bottom.0 <= EPS
            && quad.border_widths.left.0 <= EPS
    };
    let bits = |rect: &Rect| [rect.x, rect.y, rect.w, rect.h].map(f32::to_bits);
    // Shaped strips of one quad share everything but their mask.
    let mut shaped: BTreeMap<String, (u32, usize, Rect, Rect, gpui::Quad)> = BTreeMap::new();
    let mut shaped_bounds: HashMap<[u32; 4], Vec<String>> = HashMap::new();
    for (seq, piece) in pieces.iter().enumerate() {
        if plain(&piece.quad) || piece.mask.contains(&piece.bounds) {
            continue;
        }
        let key = format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
            bits(&piece.bounds),
            piece.quad.background,
            piece.quad.border_color,
            piece.quad.border_widths,
            piece.quad.corner_radii,
            piece.quad.border_style,
        );
        let visible = piece.mask.intersect(&piece.bounds);
        shaped
            .entry(key.clone())
            .and_modify(|entry| {
                (entry.0, entry.1) = (entry.0, entry.1).min((piece.order, seq));
                entry.3 = entry.3.union(&visible);
            })
            .or_insert_with(|| {
                shaped_bounds
                    .entry(bits(&piece.bounds))
                    .or_default()
                    .push(key.clone());
                (piece.order, seq, piece.bounds, visible, piece.quad)
            });
    }
    let quad_value = |bounds: &Rect, quad: &gpui::Quad, clip: Option<Value>| {
        let widths = [
            quad.border_widths.top.0,
            quad.border_widths.right.0,
            quad.border_widths.bottom.0,
            quad.border_widths.left.0,
        ];
        let border = (widths.iter().any(|width| *width > EPS)
            && !quad.border_color.is_transparent())
        .then(|| {
            json!({
                "widths": widths.map(r),
                "color": css(quad.border_color),
                "style": format!("{:?}", quad.border_style).to_lowercase(),
            })
        });
        json!({
            "kind": "box",
            "rect": bounds.json(),
            "bg": (!quad.background.is_transparent()).then(|| background(&quad.background)),
            "border": border,
            "radii": radii(&quad.corner_radii),
            "clip": clip,
        })
    };
    let mut merged_strips = 0usize;
    for (seq, piece) in pieces.iter().enumerate() {
        let visible = piece.mask.intersect(&piece.bounds).intersect(&window);
        if visible.is_empty() {
            continue;
        }
        if piece.mask.contains(&piece.bounds) {
            push(
                &mut elements,
                piece.order,
                RANK_QUAD,
                seq,
                quad_value(&piece.bounds, &piece.quad, None),
            );
            continue;
        }
        if plain(&piece.quad) {
            // The interior of a shaped quad is redrawn by the merged quad, which
            // must then paint as early as that interior did.
            let owner = shaped_bounds.get(&bits(&piece.bounds)).and_then(|keys| {
                keys.iter()
                    .find(|key| {
                        shaped
                            .get(*key)
                            .is_some_and(|entry| entry.4.background == piece.quad.background)
                    })
                    .cloned()
            });
            if let Some(entry) = owner.and_then(|key| shaped.get_mut(&key)) {
                (entry.0, entry.1) = (entry.0, entry.1).min((piece.order, seq));
                entry.3 = entry.3.union(&visible);
                merged_strips += 1;
                continue;
            }
            // A clipped rectangle is just a smaller rectangle.
            push(
                &mut elements,
                piece.order,
                RANK_QUAD,
                seq,
                quad_value(&visible, &piece.quad, None),
            );
            continue;
        }
        merged_strips += 1;
    }
    for (order, seq, bounds, visible, quad) in shaped.values() {
        let visible = visible.intersect(&window);
        push(
            &mut elements,
            *order,
            RANK_QUAD,
            *seq,
            quad_value(bounds, quad, clip_of(&visible, bounds)),
        );
    }
    report.push(format!(
        "quads: {} raw, {} pieces folded into {} shaped quads",
        pieces.len(),
        merged_strips,
        shaped.len()
    ));

    // ---- vector paths ----
    for (seq, path) in input.scene.paths.iter().enumerate() {
        let mut paints: Vec<PathPaint> = Vec::new();
        let mut curves: Vec<PathCurve> = Vec::new();
        let mut tiles: Vec<PathTile> = Vec::new();
        path.append_to(&mut paints, &mut curves, &mut tiles);
        let Some(paint) = paints.first() else {
            continue;
        };
        let mask = Rect::new(&paint.content_mask.bounds, input.inset).intersect(&window);
        let bounds = Rect::new(&paint.bounds, input.inset);
        if mask.intersect(&bounds).is_empty() {
            continue;
        }
        let (data, gaps) = svg_path(&curves, input.inset);
        if gaps > 0 {
            report.push(format!(
                "path at {:?}: {gaps} contour gaps closed",
                bounds.json()
            ));
        }
        push(
            &mut elements,
            path_order(path),
            RANK_PATH,
            seq,
            json!({
                "kind": "path",
                "rect": bounds.json(),
                "d": data,
                "fill": background(&paint.color),
                "rule": if paint.even_odd.as_bool() { "evenodd" } else { "nonzero" },
                "clip": clip_of(&mask, &bounds),
            }),
        );
    }

    // ---- underlines ----
    for (seq, underline) in input.scene.underlines.iter().enumerate() {
        let bounds = Rect::new(&underline.bounds, input.inset);
        let mask = Rect::new(&underline.content_mask.bounds, input.inset).intersect(&window);
        if mask.intersect(&bounds).is_empty() {
            continue;
        }
        push(
            &mut elements,
            underline.order,
            RANK_UNDERLINE,
            seq,
            json!({
                "kind": "underline",
                "rect": bounds.json(),
                "thickness": r(underline.thickness.0),
                "wavy": underline.wavy.as_bool(),
                "color": css(underline.color),
                "clip": clip_of(&mask, &bounds),
            }),
        );
    }

    // ---- monochrome sprites: glyphs become text runs, SVG tiles become icons ----
    let mut hits = Vec::new();
    let mut icons: Vec<String> = Vec::new();
    let mut unmapped = 0usize;
    for (seq, sprite) in input.scene.monochrome.iter().enumerate() {
        let bounds = Rect::new(&sprite.bounds, input.inset);
        let mask = Rect::new(&sprite.content_mask.bounds, input.inset).intersect(&window);
        if mask.intersect(&bounds).is_empty() {
            continue;
        }
        match input.tiles.get(&tile_key(&sprite.tile)) {
            Some(TileSource::Glyph(params)) => {
                let raster = input.text.glyph_raster_bounds(params)?;
                let x = sprite.bounds.origin.x.0 - raster.origin.x.0 as f32
                    + f32::from(params.subpixel_variant.x) / f32::from(SUBPIXEL_VARIANTS_X)
                    - input.inset;
                let y = sprite.bounds.origin.y.0 - raster.origin.y.0 as f32
                    + f32::from(params.subpixel_variant.y) / f32::from(SUBPIXEL_VARIANTS_Y)
                    - input.inset;
                hits.push(Hit {
                    seq,
                    order: sprite.order,
                    font_id: params.font_id.0,
                    glyph_id: params.glyph_id.0,
                    size: params.font_size.as_f32(),
                    x,
                    y,
                    color: sprite.color,
                    mask,
                    ink: bounds,
                });
            }
            Some(TileSource::Svg(params)) => {
                let path = params.path.to_string();
                if !icons.contains(&path) {
                    icons.push(path.clone());
                }
                push(
                    &mut elements,
                    sprite.order,
                    RANK_MONO,
                    seq,
                    json!({
                        "kind": "icon",
                        "rect": bounds.json(),
                        "src": path,
                        "color": css(sprite.color),
                        "clip": clip_of(&mask, &bounds),
                    }),
                );
            }
            _ => unmapped += 1,
        }
    }
    let layouts = input.text.layouts.lock().clone();
    let fonts = input.text.fonts.lock().clone();
    let (runs, stats) = text_runs(&hits, &layouts, input.text);
    report.push(stats);
    for run in runs {
        push(&mut elements, run.order, RANK_MONO, run.seq, run.value);
    }

    // ---- polychrome sprites: cropped from the lossless frame ----
    let mut images = Vec::new();
    for (seq, sprite) in input.scene.polychrome.iter().enumerate() {
        let bounds = Rect::new(&sprite.bounds, input.inset);
        let mask = Rect::new(&sprite.content_mask.bounds, input.inset).intersect(&window);
        let visible = mask.intersect(&bounds);
        if visible.is_empty() {
            continue;
        }
        let source = match input.tiles.get(&tile_key(&sprite.tile)) {
            Some(TileSource::Glyph(params)) => format!(
                "emoji font {} glyph {}",
                params.font_id.0, params.glyph_id.0
            ),
            Some(TileSource::Image(params)) => format!("image {:?}", params.image_id),
            Some(TileSource::Svg(params)) => format!("svg {}", params.path),
            None => "unknown".into(),
        };
        let id = format!("image-{}", images.len());
        report.push(format!("{id}: {source} at {}", visible.json()));
        images.push((id.clone(), [visible.x, visible.y, visible.w, visible.h]));
        push(
            &mut elements,
            sprite.order,
            RANK_POLY,
            seq,
            json!({
                "kind": "image",
                "rect": visible.json(),
                "src": id,
                "opacity": r(sprite.opacity),
                "radii": radii(&sprite.corner_radii),
            }),
        );
    }
    if !input.scene.subpixel.is_empty() || input.scene.surfaces > 0 {
        report.push(format!(
            "not exported: {} subpixel sprites, {} surfaces",
            input.scene.subpixel.len(),
            input.scene.surfaces
        ));
    }
    if unmapped > 0 {
        report.push(format!("{unmapped} monochrome sprites had no atlas source"));
    }

    elements.sort_by_key(|element| (element.order, element.rank, element.seq));
    let mut used_fonts: Vec<usize> = elements
        .iter()
        .filter_map(|element| element.value.get("font").and_then(Value::as_u64))
        .map(|id| id as usize)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    used_fonts.sort_unstable();
    let fonts_json: Vec<Value> = used_fonts
        .iter()
        .map(|id| {
            let font = fonts.get(id);
            let metrics = input.text.font_metrics(FontId(*id));
            json!({
                "id": id,
                "family": font.map(|font| font.family.to_string()),
                "weight": font.map(|font| font.weight.0),
                "style": font.map(|font| format!("{:?}", font.style).to_lowercase()),
                "features": font.map(|font| font.features.tag_value_list().to_vec()),
                "units_per_em": metrics.units_per_em,
                "advance": input
                    .text
                    .glyph_for_char(FontId(*id), '0')
                    .and_then(|glyph| input.text.advance(FontId(*id), glyph).ok())
                    .map(|advance| r(advance.width / metrics.units_per_em as f32)),
                "ascent": metrics.ascent,
                "descent": metrics.descent,
            })
        })
        .collect();
    let document = json!({
        "format": FORMAT,
        "width": input.width,
        "height": input.height,
        "fonts": fonts_json,
        "elements": elements.into_iter().map(|element| element.value).collect::<Vec<_>>(),
    });
    Ok(Export {
        document,
        icons,
        images,
        report,
    })
}

fn path_order(path: &gpui::PaintedPath) -> u32 {
    // `order` is private on `PaintedPath`; its Debug output starts with it.
    let debug = format!("{path:?}");
    debug
        .split("order: ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// Rebuilds closed SVG contours from GPUI's downward-oriented monotone curves.
fn svg_path(curves: &[PathCurve], inset: f32) -> (String, usize) {
    type P = (f32, f32);
    let mut seen = HashSet::new();
    let mut segments: Vec<(P, P, P)> = Vec::new();
    for curve in curves {
        let key = [
            curve.p0.x, curve.p0.y, curve.p1.x, curve.p1.y, curve.b.x, curve.b.y, curve.sign,
        ]
        .map(f32::to_bits);
        if !seen.insert(key) {
            continue;
        }
        let p0 = (curve.p0.x - inset, curve.p0.y - inset);
        let p1 = (curve.p1.x - inset, curve.p1.y - inset);
        let ctrl = (p0.0 + curve.b.x / 2., p0.1 + curve.b.y / 2.);
        if curve.sign < 0. {
            segments.push((p1, ctrl, p0));
        } else {
            segments.push((p0, ctrl, p1));
        }
    }
    let near = |a: P, b: P| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3;
    let fmt = |p: P| format!("{} {}", r(p.0), r(p.1));
    let mut used = vec![false; segments.len()];
    let mut out = String::new();
    let mut gaps = 0usize;
    while let Some(start) = used.iter().position(|used| !used) {
        used[start] = true;
        let first = segments[start].0;
        let mut current = segments[start];
        out.push_str(&format!("M{}", fmt(first)));
        loop {
            let (_, ctrl, end) = current;
            let line = {
                let (from, to) = (current.0, end);
                let mid = ((from.0 + to.0) / 2., (from.1 + to.1) / 2.);
                near(ctrl, mid)
            };
            if line {
                out.push_str(&format!("L{}", fmt(end)));
            } else {
                out.push_str(&format!("Q{} {}", fmt(ctrl), fmt(end)));
            }
            if near(end, first) {
                break;
            }
            match (0..segments.len()).find(|&i| !used[i] && near(segments[i].0, end)) {
                Some(next) => {
                    used[next] = true;
                    current = segments[next];
                }
                None => {
                    // Horizontal edges carry no winding and may be absent: bridge
                    // to the closest unused start, else close the contour.
                    let next = (0..segments.len()).filter(|&i| !used[i]).min_by(|&a, &b| {
                        let da = (segments[a].0.0 - end.0).hypot(segments[a].0.1 - end.1);
                        let db = (segments[b].0.0 - end.0).hypot(segments[b].0.1 - end.1);
                        da.total_cmp(&db)
                    });
                    match next {
                        Some(next)
                            if (segments[next].0.1 - end.1).abs() < 1e-3
                                && (segments[next].0.0 - end.0).abs()
                                    < (first.0 - end.0).abs() + (first.1 - end.1).abs() + 1e-3 =>
                        {
                            used[next] = true;
                            out.push_str(&format!("L{}", fmt(segments[next].0)));
                            current = segments[next];
                        }
                        _ => {
                            if !near(end, first) && (end.1 - first.1).abs() > 1e-3 {
                                gaps += 1;
                            }
                            break;
                        }
                    }
                }
            }
        }
        out.push('Z');
    }
    (out, gaps)
}

struct Hit {
    seq: usize,
    order: u32,
    font_id: usize,
    glyph_id: u32,
    size: f32,
    x: f32,
    y: f32,
    color: Hsla,
    mask: Rect,
    ink: Rect,
}

/// A layout matched against a row of glyph sprites.
struct Match<'a> {
    layout: &'a RecordedLayout,
    /// Indices of the layout's inked glyphs that matched, in order.
    inked: Vec<usize>,
    /// The sprites they matched, in the same order.
    taken: Vec<usize>,
    /// The terminal cell pitch, when the glyphs sit on a cell grid.
    pitch: Option<f32>,
    /// Whether some of the layout's glyphs fall outside the content mask.
    clipped: bool,
}

struct Run {
    order: u32,
    seq: usize,
    value: Value,
}

/// Whether a shaped glyph leaves ink. Whitespace and ligature spacers do not,
/// so GPUI paints no sprite for them.
struct Ink<'a> {
    text: &'a SpyTextSystem,
    cache: std::cell::RefCell<HashMap<(usize, u32, u32), Option<Rect>>>,
}

impl Ink<'_> {
    /// The glyph's raster bounds relative to its origin on the baseline, or
    /// `None` when it leaves no ink.
    fn raster(&self, layout: &RecordedLayout, k: usize) -> Option<Rect> {
        let glyph = &layout.glyphs[k];
        if layout.text[glyph.index..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            return None;
        }
        *self
            .cache
            .borrow_mut()
            .entry((glyph.font_id, glyph.glyph_id, layout.font_size.to_bits()))
            .or_insert_with(|| {
                let params = gpui::RenderGlyphParams {
                    font_id: FontId(glyph.font_id),
                    glyph_id: gpui::GlyphId(glyph.glyph_id),
                    font_size: gpui::px(layout.font_size),
                    subpixel_variant: gpui::point(0, 0),
                    scale_factor: 1.,
                    is_emoji: false,
                    subpixel_rendering: false,
                    dilation: 0,
                };
                self.text
                    .glyph_raster_bounds(&params)
                    .ok()
                    .filter(|bounds| bounds.size.width.0 > 0 && bounds.size.height.0 > 0)
                    .map(|bounds| Rect {
                        x: bounds.origin.x.0 as f32,
                        y: bounds.origin.y.0 as f32,
                        w: bounds.size.width.0 as f32,
                        h: bounds.size.height.0 as f32,
                    })
            })
    }

    fn has(&self, layout: &RecordedLayout, k: usize) -> bool {
        self.raster(layout, k).is_some()
    }

    fn inked(&self, layout: &RecordedLayout) -> Vec<usize> {
        (0..layout.glyphs.len())
            .filter(|&k| self.has(layout, k))
            .collect()
    }
}

/// Pairs glyph sprites with the line layouts the text system shaped, so each
/// run keeps its exact string (ligatures, repeated spaces) and origin.
fn text_runs(hits: &[Hit], layouts: &[RecordedLayout], text: &SpyTextSystem) -> (Vec<Run>, String) {
    // Unique layouts, longest first so short ones never steal a long line's glyphs.
    let mut unique: Vec<&RecordedLayout> = Vec::new();
    let mut seen = HashSet::new();
    for layout in layouts {
        let key = (
            layout.text.clone(),
            layout.font_size.to_bits(),
            layout
                .glyphs
                .iter()
                .map(|glyph| (glyph.font_id, glyph.glyph_id))
                .collect::<Vec<_>>(),
        );
        if seen.insert(key) {
            unique.push(layout);
        }
    }
    unique.sort_by_key(|layout| std::cmp::Reverse(layout.glyphs.len()));
    let ink = Ink {
        text,
        cache: Default::default(),
    };
    let inked_of: Vec<Vec<usize>> = unique.iter().map(|layout| ink.inked(layout)).collect();

    // Rows: glyphs sharing a baseline and a size, left to right. A glyph
    // painted again at the same spot (a sticky header over its row) goes to
    // the next layer of the row, so each pass matches as its own line.
    let mut rows: BTreeMap<(i64, u32, usize), Vec<usize>> = BTreeMap::new();
    let mut repeats: HashMap<(i64, u32, u32, usize, u32), usize> = HashMap::new();
    for (index, hit) in hits.iter().enumerate() {
        let baseline = (hit.y * 4.).round() as i64;
        let layer = repeats
            .entry((
                baseline,
                hit.size.to_bits(),
                hit.x.to_bits(),
                hit.font_id,
                hit.glyph_id,
            ))
            .or_default();
        rows.entry((baseline, hit.size.to_bits(), *layer))
            .or_default()
            .push(index);
        *layer += 1;
    }
    for row in rows.values_mut() {
        row.sort_by(|&a, &b| hits[a].x.total_cmp(&hits[b].x));
    }

    let mut runs = Vec::new();
    let mut found: Vec<Match<'_>> = Vec::new();
    let mut matched = 0usize;
    let mut orphans: Vec<usize> = Vec::new();
    for row in rows.values() {
        let mut at = 0usize;
        while at < row.len() {
            let first = &hits[row[at]];
            let mut best: Option<Match<'_>> = None;
            for (layout, inked) in unique.iter().zip(&inked_of) {
                if layout.font_size.to_bits() != first.size.to_bits() {
                    continue;
                }
                let Some(&lead) = inked.first() else {
                    continue;
                };
                if layout.glyphs[lead].glyph_id != first.glyph_id
                    || layout.glyphs[lead].font_id != first.font_id
                {
                    continue;
                }
                // Count how many inked glyphs line up with the row from here.
                let mut taken = Vec::new();
                for (offset, &k) in inked.iter().enumerate() {
                    let Some(&hit_index) = row.get(at + offset) else {
                        break;
                    };
                    let hit = &hits[hit_index];
                    let glyph = &layout.glyphs[k];
                    if hit.glyph_id != glyph.glyph_id || hit.font_id != glyph.font_id {
                        break;
                    }
                    taken.push(hit_index);
                }
                if taken.is_empty() {
                    continue;
                }
                let complete = taken.len() == inked.len();
                // A truncated match only counts when the rest is clipped away.
                if !complete {
                    let last = &hits[*taken.last().unwrap_or(&row[at])];
                    let clipped = last.ink.right() >= last.mask.right() - 1.;
                    if !clipped {
                        continue;
                    }
                }
                let fit = placement(layout, &inked[..taken.len()], &taken, hits);
                let Some(pitch) = fit else {
                    continue;
                };
                // On a tie a complete line beats a longer one cut short.
                if best.as_ref().is_none_or(|best| {
                    taken.len() > best.taken.len()
                        || (taken.len() == best.taken.len() && complete && best.clipped)
                }) {
                    best = Some(Match {
                        layout,
                        clipped: !complete,
                        inked: inked[..taken.len()].to_vec(),
                        taken,
                        pitch,
                    });
                }
            }
            let best = best.or_else(|| {
                unique
                    .iter()
                    .zip(&inked_of)
                    .filter_map(|(layout, inked)| masked_match(layout, inked, row, at, hits, &ink))
                    .max_by_key(|found| found.taken.len())
            });
            let Some(best) = best else {
                orphans.push(row[at]);
                at += 1;
                continue;
            };
            let count = best.taken.len();
            matched += count;
            found.push(best);
            at += count;
        }
    }
    // A run with a single inked glyph shows no grid; give it the cell pitch
    // other runs of its font and size sit on, if any do.
    let mut grid: HashMap<(usize, u32), f32> = HashMap::new();
    let key = |found: &Match<'_>| {
        (
            found.layout.glyphs[found.inked[0]].font_id,
            found.layout.font_size.to_bits(),
        )
    };
    for found in &found {
        if let Some(pitch) = found.pitch {
            grid.insert(key(found), pitch);
        }
    }
    for found in &found {
        let pitch = found.pitch.or_else(|| {
            (found.inked.len() == 1)
                .then(|| grid.get(&key(found)).copied())
                .flatten()
        });
        runs.extend(split_runs(found, pitch, hits, text));
    }
    let mut stats = format!(
        "text: {} glyph sprites, {matched} matched into runs, {} orphans",
        hits.len(),
        orphans.len()
    );
    // One line per stretch of orphans on a baseline.
    // Keyed by (baseline, font, size): leftmost x, rightmost x, glyph ids.
    type Stretch = (f32, f32, Vec<u32>);
    let mut stretches: BTreeMap<(i64, usize, u32), Stretch> = BTreeMap::new();
    for &orphan in &orphans {
        let hit = &hits[orphan];
        let entry = stretches
            .entry(((hit.y * 4.).round() as i64, hit.font_id, hit.size.to_bits()))
            .or_insert((f32::MAX, f32::MIN, Vec::new()));
        entry.0 = entry.0.min(hit.x);
        entry.1 = entry.1.max(hit.x);
        entry.2.push(hit.glyph_id);
    }
    for ((baseline, font, size), (from, to, glyphs)) in stretches.iter().take(40) {
        stats.push_str(&format!(
            "\n  orphans: font {font} size {} y {} x {from}..{to}: glyphs {glyphs:?}",
            f32::from_bits(*size),
            *baseline as f32 / 4.,
        ));
    }
    (runs, stats)
}

/// Checks the hits' x positions against the layout. Returns `Some(None)` when they
/// follow the shaped advances, `Some(Some(pitch))` when they sit on a fixed cell
/// grid (the terminal re-aligns glyphs to cells), `None` when neither fits.
fn placement(
    layout: &RecordedLayout,
    inked: &[usize],
    taken: &[usize],
    hits: &[Hit],
) -> Option<Option<f32>> {
    let x0 = hits[taken[0]].x;
    let g0 = layout.glyphs[inked[0]].x;
    let shaped = inked
        .iter()
        .zip(taken)
        .all(|(&k, &hit)| (hits[hit].x - x0 - (layout.glyphs[k].x - g0)).abs() <= 0.3);
    if shaped {
        return Some(None);
    }
    let column = |k: usize| layout.text[..layout.glyphs[k].index].chars().count() as f32;
    let c0 = column(inked[0]);
    let span = column(inked[inked.len() - 1]) - c0;
    if span <= 0. {
        return None;
    }
    let pitch = (hits[taken[taken.len() - 1]].x - x0) / span;
    let grid = inked
        .iter()
        .zip(taken)
        .all(|(&k, &hit)| (hits[hit].x - x0 - (column(k) - c0) * pitch).abs() <= 0.3);
    grid.then_some(Some(pitch))
}

/// Matches a line the content mask cuts through: GPUI skips every glyph
/// whose ink falls outside the mask, so a row clipped at the bottom keeps
/// only its descenders and a long line loses its tail. Places `layout` so
/// that one of its glyphs is the sprite at `row[at]`, then requires the
/// sprites that follow to be exactly the glyphs whose ink meets the mask.
fn masked_match<'a>(
    layout: &'a RecordedLayout,
    inked: &[usize],
    row: &[usize],
    at: usize,
    hits: &[Hit],
    ink: &Ink<'_>,
) -> Option<Match<'a>> {
    let first = &hits[row[at]];
    if layout.font_size.to_bits() != first.size.to_bits() {
        return None;
    }
    let mut best: Option<Match<'a>> = None;
    for &anchor in inked {
        let glyph = &layout.glyphs[anchor];
        if glyph.glyph_id != first.glyph_id || glyph.font_id != first.font_id {
            continue;
        }
        let origin = first.x - glyph.x;
        // The ink each glyph would leave inside the mask. GPUI culls with the
        // bounds of the subpixel variant it rasterizes, which can differ from
        // these by a pixel, so a glyph that barely reaches in may be absent.
        let reach: Vec<(usize, Rect)> = inked
            .iter()
            .filter_map(|&k| {
                let raster = ink.raster(layout, k)?;
                let bounds = Rect {
                    x: raster.x + origin + layout.glyphs[k].x,
                    y: raster.y + first.y,
                    ..raster
                };
                let inside = bounds.intersect(&first.mask);
                (!inside.is_empty()).then_some((k, inside))
            })
            .collect();
        if reach.len() == inked.len() || reach.first().map(|(k, _)| *k) != Some(anchor) {
            continue;
        }
        let mut visible = Vec::with_capacity(reach.len());
        let mut taken = Vec::with_capacity(reach.len());
        for &(k, inside) in &reach {
            let glyph = &layout.glyphs[k];
            let hit = row
                .get(at + taken.len())
                .map(|&index| (index, &hits[index]));
            let matches = hit.is_some_and(|(_, hit)| {
                hit.glyph_id == glyph.glyph_id
                    && hit.font_id == glyph.font_id
                    && (hit.x - origin - glyph.x).abs() <= 0.3
            });
            match hit {
                Some((index, _)) if matches => {
                    visible.push(k);
                    taken.push(index);
                }
                _ if inside.w <= 1. || inside.h <= 1. => {}
                _ => {
                    taken.clear();
                    break;
                }
            }
        }
        if !taken.is_empty()
            && best
                .as_ref()
                .is_none_or(|best| taken.len() > best.taken.len())
        {
            best = Some(Match {
                layout,
                inked: visible,
                taken,
                pitch: None,
                clipped: true,
            });
        }
    }
    best
}

fn split_runs(
    found: &Match<'_>,
    pitch: Option<f32>,
    hits: &[Hit],
    text: &SpyTextSystem,
) -> Vec<Run> {
    let (layout, inked, taken) = (found.layout, &found.inked, &found.taken);
    let glyphs = &layout.glyphs;
    let hit_of: HashMap<usize, usize> = inked.iter().copied().zip(taken.iter().copied()).collect();
    let last_glyph = *inked.last().unwrap_or(&0);
    let end_of = |k: usize| {
        if k + 1 < glyphs.len() {
            glyphs[k + 1].index
        } else {
            layout.text.len()
        }
    };
    // Proportional text: refine the line origin from every glyph, each of which
    // GPUI quantized to a quarter pixel.
    let origin = pitch.is_none().then(|| {
        // In glyph order, so the float sum is the same on every run.
        let deltas: Vec<f32> = inked
            .iter()
            .zip(taken.iter())
            .map(|(&k, &h)| hits[h].x - glyphs[k].x)
            .collect();
        deltas.iter().sum::<f32>() / deltas.len() as f32
    });
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    let column = |k: usize| layout.text[..glyphs[k].index].chars().count() as f32;
    let flush = |runs: &mut Vec<Run>, floor: usize, from: usize, to: usize| {
        let lead = hit_of[&from];
        let hit = &hits[lead];
        // A ligature starts with spacer glyphs that leave no ink (JetBrains
        // Mono draws `<=` as a spacer then the ligature); keep them in the run.
        let mut first = from;
        while first > floor
            && !hit_of.contains_key(&(first - 1))
            && !layout.text[glyphs[first - 1].index..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            first -= 1;
        }
        let x = match (pitch, origin) {
            (Some(pitch), _) => hit.x - (column(from) - column(first)) * pitch,
            (None, Some(origin)) => origin + glyphs[first].x,
            (None, None) => hit.x,
        };
        let slice = layout.text[glyphs[first].index..end_of(to)].trim_end();
        let members: Vec<&Hit> = (from..=to)
            .filter_map(|k| hit_of.get(&k).map(|&h| &hits[h]))
            .collect();
        let extent = members
            .iter()
            .map(|member| member.ink)
            .reduce(|a, b| a.union(&b))
            .unwrap_or(hit.ink);
        let natural = natural_pitch(layout, text, glyphs[from].font_id);
        let spacing = pitch
            .zip(natural)
            .map(|(pitch, natural)| r(pitch - natural))
            .filter(|spacing| spacing.abs() > 0.001);
        runs.push(Run {
            order: members
                .iter()
                .map(|member| member.order)
                .min()
                .unwrap_or(hit.order),
            seq: members
                .iter()
                .map(|member| member.seq)
                .min()
                .unwrap_or(hit.seq),
            value: json!({
                "kind": "text",
                "text": slice,
                "x": r(x),
                "y": r(hit.y),
                "size": r(layout.font_size),
                "font": glyphs[from].font_id,
                "color": css(hit.color),
                "spacing": spacing,
                "clip": (found.clipped || !hit.mask.contains(&extent)).then(|| hit.mask.json()),
            }),
        });
    };
    // cosmic-text shapes each word on its own, so proportional text carries
    // no kerning across a break opportunity (after `/`, say) while a browser
    // kerns the whole run. Such text splits into one run per word, each at
    // the origin GPUI gave it. Monospace text has no kerning to lose.
    let words: HashSet<usize> = if pitch.is_none() && !is_monospace(text, glyphs[inked[0]].font_id)
    {
        unicode_linebreak::linebreaks(&layout.text)
            .map(|(index, _)| index)
            .collect()
    } else {
        HashSet::new()
    };
    let mut floor = 0usize;
    for k in 0..=last_glyph {
        let Some(&hit_index) = hit_of.get(&k) else {
            continue;
        };
        match start {
            None => start = Some(k),
            Some(from) => {
                let lead = &hits[hit_of[&from]];
                let hit = &hits[hit_index];
                if hit.color != lead.color
                    || glyphs[k].font_id != glyphs[from].font_id
                    || words.contains(&glyphs[k].index)
                {
                    flush(&mut runs, floor, from, k - 1);
                    floor = k;
                    start = Some(k);
                }
            }
        }
    }
    if let Some(from) = start {
        flush(&mut runs, floor, from, last_glyph);
    }
    runs
}

/// The advance the browser will use between two monospace glyphs of this layout.
/// Whether every glyph of the font has the same advance.
fn is_monospace(text: &SpyTextSystem, font_id: usize) -> bool {
    let advance = |ch| {
        let glyph = text.glyph_for_char(FontId(font_id), ch)?;
        text.advance(FontId(font_id), glyph)
            .ok()
            .map(|size| size.width)
    };
    matches!((advance('i'), advance('M')), (Some(i), Some(m)) if (i - m).abs() < 0.01)
}

fn natural_pitch(layout: &RecordedLayout, text: &SpyTextSystem, font_id: usize) -> Option<f32> {
    let glyphs: Vec<_> = layout
        .glyphs
        .iter()
        .filter(|glyph| glyph.font_id == font_id)
        .collect();
    if glyphs.len() >= 2 {
        let first = glyphs[0];
        let last = glyphs[glyphs.len() - 1];
        let columns = layout.text[first.index..last.index].chars().count() as f32;
        if columns > 0. {
            return Some((last.x - first.x) / columns);
        }
    }
    let glyph = text.glyph_for_char(FontId(font_id), '0')?;
    let advance = text.advance(FontId(font_id), glyph).ok()?;
    let metrics = text.font_metrics(FontId(font_id));
    Some(advance.width / metrics.units_per_em as f32 * layout.font_size)
}

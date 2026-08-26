//! Custom GPUI `Element` for the diff body - virtualized, direct-paint.
//!
//! Replaces the per-row `div`+`StyledText` `uniform_list`, which re-ran Taffy
//! flex layout AND line shaping for every visible row on every frame (~300
//! Taffy nodes/frame across 3 split columns) and scrolled choppily in debug
//! builds. This element is modeled on Paneflow's `TerminalElement` (which is
//! the same approach Zed's `EditorElement` uses): it reports its full content
//! height and is hosted inside an `overflow_y_scroll` div, computes the visible
//! window from `window.content_mask()` (pixel math, no flex), shapes ONLY the
//! visible lines via the frame-cached `text_system().shape_line`, and paints
//! background quads + glyphs directly. Row data (`DisplayRow`/`SplitRow`) is
//! built off-thread and consumed unchanged - zero changes to `rows.rs`.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Corners, Element, ElementId, Font, FontFeatures,
    FontStyle, FontWeight, GlobalElementId, Hsla, ImgResourceLoader, InspectorElementId,
    IntoElement, LayoutId, Length, Path, PathBuilder, Pixels, Point, RenderImage, Resource,
    ShapedLine, SharedString, Style, TextAlign, TextRun, Window, fill, point, px, quad, relative,
    size,
};

use super::align::CellKind;
use super::hscroll::{
    H_SCROLLBAR_TRACK_HEIGHT, file_at_row, file_side_offset, h_scrollbar_segments,
};
use super::rows::{
    DisplayRow, FileSpan, HalfCell, HeaderParts, ROW_HEIGHT, RowKind, RowPalette, SplitRow,
};

const PAD: f32 = 6.0; // compact text inset
const STAT_GAP: f32 = 16.0; // min gap between the path region and the right-aligned diffstat (US-006)
const FILE_HEADER_ICON_X: f32 = 16.0;
const FILE_HEADER_ICON_SLOT_W: f32 = 20.0;
const FILE_HEADER_ICON_SIZE: f32 = 17.0;
const FILE_HEADER_FILE_ICON_SIZE: f32 = 16.0;
const FILE_HEADER_RUST_ICON_SIZE: f32 = 20.0;
const FILE_HEADER_PATH_X: f32 = 42.0;
const FILE_HEADER_STAT_PAD_R: f32 = 14.0;
const FILE_HEADER_SEPARATOR_H: f32 = 2.0;
const HALF_PAD: f32 = 4.0; // split half-cell left padding (after the bar)
const GUTTER_W: f32 = 36.0; // single line-number column FLOOR width (widened per max digits)
const NUM_GAP: f32 = 6.0; // right padding inside the gutter (number → code gap)
const GUTTER_PAD_L: f32 = 8.0; // left breathing room inside the derived gutter
/// Pinned sticky file-header height. Slimmer than the inline 40px header card
/// so the floating context bar stays unobtrusive while scrolling a file.
const STICKY_HEADER_HEIGHT: f32 = 24.0;
// Zed's gutter diff-hunk strip width: floor(0.275 * line_height) = 4px at ROW_HEIGHT 18.
const BAR_W: f32 = 4.0; // colored hunk-indicator bar
const DELETED_BAR_DASH_H: f32 = 1.0;
const DELETED_BAR_DASH_STEP: f32 = 2.0;
const FOLD_RADIUS: f32 = 6.0;
const FOLD_SEGMENT_GAP: f32 = 2.0;
const FOLD_LABEL_PAD_X: f32 = 8.0;
const FOLD_CHEVRON_W: f32 = 3.5;
const FOLD_CHEVRON_H: f32 = 2.5;
const FOLD_CHEVRON_OFFSET: f32 = 4.0;
const PHANTOM_HATCH_SPACING: f32 = 8.0;
const PHANTOM_HATCH_STROKE: f32 = 1.0;
const SPLIT_DIVIDER_W: f32 = 3.0;
const PAD2: f32 = 6.0; // gap between the hunk bar and the line-number gutter

/// The row source for one column - either unified or side-by-side.
///
/// Each variant carries its precomputed cumulative row offsets (`offsets[i]` =
/// top of row `i`, `offsets[len]` = total content height) and widest line
/// number. Both are derived ONCE off the per-frame path - in
/// [`super::view::Column::recompute_display`] - and shared as an `Rc`, so
/// `request_layout` / `prepaint` never re-walk every row (previously two O(N)
/// `Vec<f32>` allocations per column per frame).
pub enum DiffBody {
    Unified {
        rows: Rc<Vec<DisplayRow>>,
        offsets: Rc<Vec<f32>>,
        max_line_no: u32,
        /// Per-file horizontal-scroll spans (widest line per file) + live
        /// per-file horizontal offsets (px), lockstep with `rows`. The element
        /// shifts each file's code left by its own offset; everything else
        /// (gutter, headers, sticky bar) stays pinned.
        spans: Rc<Vec<FileSpan>>,
        h_offsets: Rc<Vec<f32>>,
    },
    Split {
        rows: Rc<Vec<SplitRow>>,
        offsets: Rc<Vec<f32>>,
        max_line_no: u32,
        /// Per-file horizontal-scroll spans plus live split-side horizontal
        /// offsets (px), lockstep with `rows`. The element shifts each file
        /// half's code left by its own offset; everything else (gutter, headers,
        /// sticky bar) stays pinned.
        spans: Rc<Vec<FileSpan>>,
        h_offsets: Rc<Vec<f32>>,
    },
}

impl DiffBody {
    fn len(&self) -> usize {
        match self {
            DiffBody::Unified { rows, .. } => rows.len(),
            DiffBody::Split { rows, .. } => rows.len(),
        }
    }

    /// Widest line number anywhere in the body (across both sides), used to size
    /// the gutter so wide line numbers never clip past its left edge. Precomputed
    /// at build time; `0` for an empty body.
    fn max_line_no(&self) -> u32 {
        match self {
            DiffBody::Unified { max_line_no, .. } | DiffBody::Split { max_line_no, .. } => {
                *max_line_no
            }
        }
    }

    /// Cheap clone of the shared cumulative-offset vector (length `len + 1`).
    /// Returned as an owned `Rc` so the caller can mutate other `self` fields
    /// (e.g. `gutter_w`) without holding a borrow of `self.body`.
    fn offsets_rc(&self) -> Rc<Vec<f32>> {
        match self {
            DiffBody::Unified { offsets, .. } | DiffBody::Split { offsets, .. } => offsets.clone(),
        }
    }

    /// Per-file scroll spans (widest line + header row per file).
    fn spans_rc(&self) -> Rc<Vec<FileSpan>> {
        match self {
            DiffBody::Unified { spans, .. } | DiffBody::Split { spans, .. } => spans.clone(),
        }
    }

    /// Live horizontal offsets (px), indexed by mode/file/side.
    fn h_offsets_rc(&self) -> Rc<Vec<f32>> {
        match self {
            DiffBody::Unified { h_offsets, .. } | DiffBody::Split { h_offsets, .. } => {
                h_offsets.clone()
            }
        }
    }
}

/// One solid rectangle to paint (row / cell background, divider, word-diff bg).
struct Quad {
    bounds: Bounds<Pixels>,
    color: Hsla,
}

struct RoundedQuad {
    bounds: Bounds<Pixels>,
    color: Hsla,
    corners: Corners<Pixels>,
}

/// One shaped text fragment to paint at `origin`, optionally clipped to `clip`
/// (used to keep a split half's long line from bleeding past its column).
struct Glyphs {
    origin: Point<Pixels>,
    line: ShapedLine,
    clip: Option<Bounds<Pixels>>,
}

struct HatchLine {
    start: Point<Pixels>,
    end: Point<Pixels>,
    color: Hsla,
}

struct HatchPath {
    path: Path<Pixels>,
    color: Hsla,
}

struct ImagePaint {
    bounds: Bounds<Pixels>,
    image: Arc<RenderImage>,
}

/// Prepaint output: the fully-resolved draw lists for the visible window only.
pub struct DiffPrepaint {
    quads: Vec<Quad>,
    rounded_quads: Vec<RoundedQuad>,
    images: Vec<ImagePaint>,
    glyphs: Vec<Glyphs>,
    hatches: Vec<HatchPath>,
    scrollbars: Vec<RoundedQuad>,
    /// Pinned sticky-header draw list, painted AFTER the body (and after
    /// `glyphs`) so it floats over the scrolling rows. Empty when the viewport
    /// top sits above the first file's inline header (nothing to pin yet).
    sticky_quads: Vec<Quad>,
    sticky_images: Vec<ImagePaint>,
    sticky_glyphs: Vec<Glyphs>,
}

pub struct DiffElement {
    body: DiffBody,
    palette: RowPalette,
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
    /// Gutter column width, derived each prepaint from the body's widest line
    /// number (floor [`GUTTER_W`]). A field so the layout helpers read one
    /// resolved value instead of threading it through every signature.
    gutter_w: Pixels,
}

impl DiffElement {
    pub fn new(body: DiffBody, palette: RowPalette) -> Self {
        // The mono family is constant (always the embedded default) and a fresh
        // `DiffElement` is built every frame, so resolve it once per thread and
        // clone the cheap `SharedString` handle instead of re-resolving + heap-
        // allocating a family string on every render.
        thread_local! {
            static MONO_FAMILY: SharedString =
                crate::terminal::element::resolve_font_family(None).into();
        }
        let family = MONO_FAMILY.with(|f| f.clone());
        Self {
            body,
            palette,
            font: Font {
                family,
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            },
            font_size: px(12.),
            line_height: px(ROW_HEIGHT),
            gutter_w: px(GUTTER_W),
        }
    }

    /// Split a line into `TextRun`s: the syntax runs carry their own color, the
    /// gaps fall back to `default`. With syntax off, `syntax` is empty → one
    /// run. The run lengths sum to `text.len()`, which `shape_line` requires.
    fn text_runs(
        &self,
        text: &str,
        syntax: &[(Range<usize>, Hsla)],
        default: Hsla,
    ) -> Vec<TextRun> {
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if syntax.is_empty() {
            return vec![run(text.len(), default)];
        }
        let len = text.len();
        let mut runs = Vec::new();
        let mut ix = 0usize;
        for (r, color) in syntax {
            let start = r.start.min(len);
            let end = r.end.min(len);
            if start < ix || start >= end {
                continue; // defensive: skip malformed/overlapping ranges
            }
            if start > ix {
                runs.push(run(start - ix, default));
            }
            runs.push(run(end - start, *color));
            ix = end;
        }
        if ix < len {
            runs.push(run(len - ix, default));
        }
        runs
    }

    /// Shape a styled line (main text column).
    fn shape(
        &self,
        window: &mut Window,
        text: &SharedString,
        syntax: &[(Range<usize>, Hsla)],
        default: Hsla,
    ) -> ShapedLine {
        if syntax.is_empty() {
            let runs = [TextRun {
                len: text.len(),
                font: self.font.clone(),
                color: default,
                background_color: None,
                underline: None,
                strikethrough: None,
            }];
            return window
                .text_system()
                .shape_line(text.clone(), self.font_size, &runs, None);
        }

        let runs = self.text_runs(text, syntax, default);
        window
            .text_system()
            .shape_line(text.clone(), self.font_size, &runs, None)
    }

    /// Shape a single line in a specific font weight (EP-002 US-006: the
    /// semibold basename segment of a file header). One run, one color.
    fn shape_weighted(
        &self,
        window: &mut Window,
        text: SharedString,
        color: Hsla,
        weight: FontWeight,
    ) -> ShapedLine {
        let font = Font {
            weight,
            ..self.font.clone()
        };
        let runs = [TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        window
            .text_system()
            .shape_line(text, self.font_size, &runs, None)
    }

    fn language_icon_size(icon_path: &str) -> f32 {
        match icon_path {
            "icons/languages/file.svg" => FILE_HEADER_FILE_ICON_SIZE,
            "icons/languages/rust-small.svg" => FILE_HEADER_RUST_ICON_SIZE,
            _ => FILE_HEADER_ICON_SIZE,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_file_icon(
        &self,
        window: &mut Window,
        cx: &mut App,
        basename: &str,
        origin: Point<Pixels>,
        row_h: Pixels,
        bounds: Bounds<Pixels>,
        images: &mut Vec<ImagePaint>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let icon_path = crate::file_icons::language_icon_path(basename);
        let source = Resource::Embedded(icon_path.into());
        if let Some(Ok(image)) = window.use_asset::<ImgResourceLoader>(&source, cx) {
            let image_size = image.size(0);
            let image_w = i32::from(image_size.width).max(1) as f32;
            let image_h = i32::from(image_size.height).max(1) as f32;
            let target_size = Self::language_icon_size(icon_path);
            let scale = (target_size / image_w).min(target_size / image_h);
            let paint_w = px(image_w * scale);
            let paint_h = px(image_h * scale);
            let icon_x = origin.x
                + px(FILE_HEADER_ICON_X)
                + ((px(FILE_HEADER_ICON_SLOT_W) - paint_w) / 2.0).max(px(0.));
            images.push(ImagePaint {
                bounds: Bounds::new(
                    point(icon_x, origin.y + (row_h - paint_h) / 2.0),
                    size(paint_w, paint_h),
                ),
                image,
            });
            return;
        }

        let is_rust = icon_path == "icons/languages/rust-small.svg";
        let fallback = if is_rust { "R" } else { "◻" };
        let color = if is_rust {
            self.palette.file_icon_hot
        } else {
            self.palette.muted.opacity(0.72)
        };
        let icon_line = if is_rust {
            self.shape_weighted(window, fallback.into(), color, FontWeight::SEMIBOLD)
        } else {
            self.shape_plain(window, fallback.into(), color)
        };
        let icon_x = origin.x
            + px(FILE_HEADER_ICON_X)
            + ((px(FILE_HEADER_ICON_SLOT_W) - icon_line.width()) / 2.0).max(px(0.));
        glyphs.push(Glyphs {
            origin: point(icon_x, origin.y + (row_h - self.line_height) / 2.0),
            line: icon_line,
            clip: Some(bounds),
        });
    }

    /// Paint a compact changed-file row: file-type icon, muted directory
    /// prefix, emphasized basename, right-aligned diffstat, and trailing
    /// actions. Drives BOTH the inline file header and the pinned sticky bar so
    /// the two never drift.
    #[allow(clippy::too_many_arguments)]
    fn paint_file_header(
        &self,
        window: &mut Window,
        cx: &mut App,
        parts: &HeaderParts,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        _collapsed: bool,
        sticky: bool,
        quads: &mut Vec<Quad>,
        images: &mut Vec<ImagePaint>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let p = &self.palette;
        let lh = self.line_height;
        let bounds = Bounds::new(origin, size(width, row_h));
        // Card / sticky background.
        quads.push(Quad {
            bounds,
            color: if sticky {
                p.sticky_header_bg
            } else {
                p.header_bg
            },
        });
        // A sticky bar floats with a bottom separator; an inline card is
        // divided from the row above it by a top separator. The separator uses
        // the diff body background so it reads as spacing between surfaces.
        let sep_h = px(FILE_HEADER_SEPARATOR_H);
        let sep_y = if sticky {
            origin.y + row_h - sep_h
        } else {
            origin.y
        };
        quads.push(Quad {
            bounds: Bounds::new(point(origin.x, sep_y), size(width, sep_h)),
            color: p.context_bg,
        });

        let basename: &str = parts.basename.as_ref();
        self.push_file_icon(window, cx, basename, origin, row_h, bounds, images, glyphs);

        let ty = origin.y + (row_h - lh) / 2.0;

        // Right-aligned diffstat: "+N" (added, green) then "-N" (deleted, red).
        let stat_text = format!("+{} -{}", parts.added, parts.removed);
        let split = stat_text.find(" -").unwrap_or(stat_text.len());
        let stat_runs = [(0..split, p.add_fg), (split..stat_text.len(), p.del_fg)];
        let stat_ss: SharedString = stat_text.into();
        let stat_line = self.shape(window, &stat_ss, &stat_runs, p.muted);
        let stat_w = stat_line.width();
        let path_x = origin.x + px(FILE_HEADER_PATH_X);
        let stat_x = (bounds.right() - px(FILE_HEADER_STAT_PAD_R) - stat_w).max(path_x);
        glyphs.push(Glyphs {
            origin: point(stat_x, ty),
            line: stat_line,
            clip: Some(bounds),
        });

        // Path region [path_x, region_right). The basename is emphasized and
        // NEVER truncated; the directory prefix is muted and gives way first -
        // trailing-aligned into its shrunken slot so the immediate parent dir
        // survives the clip.
        let region_right = (stat_x - px(STAT_GAP)).max(path_x);
        let avail = (region_right - path_x).max(px(0.));
        let base_line =
            self.shape_weighted(window, parts.basename.clone(), p.text, FontWeight::SEMIBOLD);
        let dir_line = self.shape_plain(window, parts.dir_prefix.clone(), p.muted);
        let bw = base_line.width().min(avail);
        let dir_avail = (avail - bw).max(px(0.));
        let dw = dir_line.width();
        if dw <= dir_avail {
            let base_x = path_x + dw;
            if dw > px(0.) {
                glyphs.push(Glyphs {
                    origin: point(path_x, ty),
                    line: dir_line,
                    clip: Some(bounds),
                });
            }
            glyphs.push(Glyphs {
                origin: point(base_x, ty),
                line: base_line,
                clip: Some(Bounds::new(point(base_x, origin.y), size(bw, row_h))),
            });
        } else {
            let base_x = path_x + dir_avail;
            let dir_origin_x = base_x - dw; // overflows left; masked by the clip
            glyphs.push(Glyphs {
                origin: point(dir_origin_x, ty),
                line: dir_line,
                clip: Some(Bounds::new(point(path_x, origin.y), size(dir_avail, row_h))),
            });
            glyphs.push(Glyphs {
                origin: point(base_x, ty),
                line: base_line,
                clip: Some(Bounds::new(point(base_x, origin.y), size(bw, row_h))),
            });
        }
    }

    /// Shape a single-color string (gutter numbers, signs, headers).
    fn shape_plain(&self, window: &mut Window, text: SharedString, color: Hsla) -> ShapedLine {
        let runs = [TextRun {
            len: text.len(),
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        window
            .text_system()
            .shape_line(text, self.font_size, &runs, None)
    }

    fn push_hunk_bar(
        origin: Point<Pixels>,
        height: Pixels,
        color: Hsla,
        dashed: bool,
        quads: &mut Vec<Quad>,
    ) {
        if !dashed {
            quads.push(Quad {
                bounds: Bounds::new(origin, size(px(BAR_W), height)),
                color,
            });
            return;
        }

        let height = f32::from(height).max(0.0);
        let mut y = 0.0;
        while y < height {
            let dash_h = DELETED_BAR_DASH_H.min(height - y);
            quads.push(Quad {
                bounds: Bounds::new(
                    point(origin.x, origin.y + px(y)),
                    size(px(BAR_W), px(dash_h)),
                ),
                color,
            });
            y += DELETED_BAR_DASH_STEP;
        }
    }

    fn push_phantom_hatches(&self, bounds: Bounds<Pixels>, hatches: &mut Vec<HatchLine>) {
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return;
        }

        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let phase = f32::from(bounds.origin.x + bounds.origin.y).rem_euclid(PHANTOM_HATCH_SPACING);
        let mut k = -phase;

        while k <= width + height {
            let mut points = Vec::with_capacity(4);
            if (0.0..=height).contains(&k) {
                points.push((0.0, k));
            }
            let top_x = k;
            if (0.0..=width).contains(&top_x) {
                points.push((top_x, 0.0));
            }
            let right_y = k - width;
            if (0.0..=height).contains(&right_y) {
                points.push((width, right_y));
            }
            let bottom_x = k - height;
            if (0.0..=width).contains(&bottom_x) {
                points.push((bottom_x, height));
            }

            if points.len() >= 2 {
                let (sx, sy) = points[0];
                let (ex, ey) = points[1];
                hatches.push(HatchLine {
                    start: point(bounds.origin.x + px(sx), bounds.origin.y + px(sy)),
                    end: point(bounds.origin.x + px(ex), bounds.origin.y + px(ey)),
                    color: self.palette.phantom_hatch,
                });
            }
            k += PHANTOM_HATCH_SPACING;
        }
    }

    fn push_fold_icon(&self, bounds: Bounds<Pixels>, hatches: &mut Vec<HatchLine>) {
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return;
        }

        let cx = bounds.origin.x + bounds.size.width / 2.0;
        let cy = bounds.origin.y + bounds.size.height / 2.0;
        let w = px(FOLD_CHEVRON_W);
        let h = px(FOLD_CHEVRON_H);
        let offset = px(FOLD_CHEVRON_OFFSET);
        let color = self.palette.muted.blend(self.palette.text.opacity(0.25));

        let top_y = cy - offset;
        hatches.push(HatchLine {
            start: point(cx - w, top_y + h / 2.0),
            end: point(cx, top_y - h / 2.0),
            color,
        });
        hatches.push(HatchLine {
            start: point(cx, top_y - h / 2.0),
            end: point(cx + w, top_y + h / 2.0),
            color,
        });

        let bottom_y = cy + offset;
        hatches.push(HatchLine {
            start: point(cx - w, bottom_y - h / 2.0),
            end: point(cx, bottom_y + h / 2.0),
            color,
        });
        hatches.push(HatchLine {
            start: point(cx, bottom_y + h / 2.0),
            end: point(cx + w, bottom_y - h / 2.0),
            color,
        });
    }

    fn fold_corners(&self, round_left: bool, round_right: bool) -> Corners<Pixels> {
        let r = px(FOLD_RADIUS);
        Corners {
            top_left: if round_left { r } else { px(0.) },
            top_right: if round_right { r } else { px(0.) },
            bottom_right: if round_right { r } else { px(0.) },
            bottom_left: if round_left { r } else { px(0.) },
        }
    }

    fn build_hatch_paths(lines: Vec<HatchLine>) -> Vec<HatchPath> {
        let mut paths = Vec::new();
        let mut current_color = None;
        let mut builder = PathBuilder::stroke(px(PHANTOM_HATCH_STROKE));
        let mut has_lines = false;

        for line in lines {
            if current_color.is_some_and(|color| color != line.color) {
                if has_lines
                    && let Some(color) = current_color
                    && let Ok(path) = builder.build()
                {
                    paths.push(HatchPath { path, color });
                }
                builder = PathBuilder::stroke(px(PHANTOM_HATCH_STROKE));
            }

            current_color = Some(line.color);
            builder.move_to(line.start);
            builder.line_to(line.end);
            has_lines = true;
        }

        if has_lines
            && let Some(color) = current_color
            && let Ok(path) = builder.build()
        {
            paths.push(HatchPath { path, color });
        }

        paths
    }

    fn scrollbar_corners() -> Corners<Pixels> {
        let r = px(H_SCROLLBAR_TRACK_HEIGHT / 2.0);
        Corners {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        }
    }

    fn push_horizontal_scrollbars(
        &self,
        bounds: Bounds<Pixels>,
        segments: &[super::hscroll::HScrollbarSegment],
        scrollbars: &mut Vec<RoundedQuad>,
    ) {
        let corners = Self::scrollbar_corners();
        for segment in segments {
            let y = bounds.origin.y + px(segment.y);
            let x = bounds.origin.x + px(segment.x);
            scrollbars.push(RoundedQuad {
                bounds: Bounds::new(
                    point(x, y),
                    size(px(segment.width), px(H_SCROLLBAR_TRACK_HEIGHT)),
                ),
                color: self.palette.muted.opacity(0.18),
                corners,
            });
            scrollbars.push(RoundedQuad {
                bounds: Bounds::new(
                    point(x + px(segment.thumb_x), y),
                    size(px(segment.thumb_width), px(H_SCROLLBAR_TRACK_HEIGHT)),
                ),
                color: self.palette.muted.opacity(0.62),
                corners,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_fold_region(
        &self,
        window: &mut Window,
        label: Option<&SharedString>,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        icon_w: Pixels,
        round_left: bool,
        round_right: bool,
        quads: &mut Vec<RoundedQuad>,
        glyphs: &mut Vec<Glyphs>,
        hatches: &mut Vec<HatchLine>,
    ) {
        if width <= px(0.) || row_h <= px(0.) {
            return;
        }

        let p = &self.palette;
        let bg = p.header_bg.blend(p.text.opacity(0.04));
        let icon_bg = p.header_bg.blend(p.text.opacity(0.02));
        let text_color = p.muted.blend(p.text.opacity(0.28));
        let icon_w = icon_w.clamp(px(0.), width);

        if icon_w > px(0.) {
            let icon_bounds = Bounds::new(origin, size(icon_w, row_h));
            quads.push(RoundedQuad {
                bounds: icon_bounds,
                color: icon_bg,
                corners: self.fold_corners(round_left, label.is_none() && round_right),
            });
            self.push_fold_icon(icon_bounds, hatches);
        }

        let gap = if icon_w > px(0.) {
            px(FOLD_SEGMENT_GAP)
        } else {
            px(0.)
        };
        let label_x = origin.x + icon_w + gap;
        let label_w = (width - icon_w - gap).max(px(0.));
        if label_w <= px(0.) {
            return;
        }

        let label_bounds = Bounds::new(point(label_x, origin.y), size(label_w, row_h));
        quads.push(RoundedQuad {
            bounds: label_bounds,
            color: bg,
            corners: self.fold_corners(icon_w <= px(0.) && round_left, round_right),
        });

        if let Some(label) = label {
            glyphs.push(Glyphs {
                origin: point(
                    label_x + px(FOLD_LABEL_PAD_X),
                    origin.y + (row_h - self.line_height) / 2.0,
                ),
                line: self.shape_plain(window, label.clone(), text_color),
                clip: Some(label_bounds),
            });
        }
    }

    /// Emit draw commands for one unified row at top-left `origin`.
    #[allow(clippy::too_many_arguments)]
    fn layout_unified(
        &self,
        window: &mut Window,
        cx: &mut App,
        row: &DisplayRow,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        collapsed: bool,
        h_offset: Pixels,
        quads: &mut Vec<Quad>,
        rounded_quads: &mut Vec<RoundedQuad>,
        images: &mut Vec<ImagePaint>,
        glyphs: &mut Vec<Glyphs>,
        hatches: &mut Vec<HatchLine>,
    ) {
        let p = &self.palette;
        let lh = self.line_height;
        let row_bounds = Bounds::new(origin, size(width, row_h));

        match row.kind {
            RowKind::FileHeader => match &row.header {
                // EP-002 US-006: structured header (icon + path + diffstat).
                // Always present for header rows.
                Some(parts) => self.paint_file_header(
                    window, cx, parts, origin, width, row_h, collapsed, false, quads, images,
                    glyphs,
                ),
                // Defensive fallback for the impossible header-less case.
                None => {
                    quads.push(Quad {
                        bounds: row_bounds,
                        color: p.header_bg,
                    });
                    glyphs.push(Glyphs {
                        origin: point(origin.x + px(PAD), origin.y + (row_h - lh) / 2.0),
                        line: self.shape_plain(window, row.text.clone(), p.text),
                        clip: Some(row_bounds),
                    });
                }
            },
            RowKind::Binary | RowKind::Truncated => {
                glyphs.push(Glyphs {
                    origin: point(origin.x + px(PAD), origin.y),
                    line: self.shape_plain(window, row.text.clone(), p.muted),
                    clip: Some(row_bounds),
                });
            }
            RowKind::Fold => {
                let icon_w = px(BAR_W + PAD2) + self.gutter_w;
                self.layout_fold_region(
                    window,
                    Some(&row.text),
                    origin,
                    width,
                    row_h,
                    icon_w,
                    true,
                    true,
                    rounded_quads,
                    glyphs,
                    hatches,
                );
            }
            RowKind::Context | RowKind::Added | RowKind::Removed => {
                // (row-bg wash, gutter tint, opaque hunk-bar color). EP-002
                // US-007: context now gets a faint document wash instead of the
                // bare window background.
                let (bg, gutter_bg, bar_color) = match row.kind {
                    RowKind::Added => (Some(p.add_bg), p.add_gutter_bg, Some(p.add_bar)),
                    RowKind::Removed => (Some(p.del_bg), p.del_gutter_bg, Some(p.del_bar)),
                    _ => (Some(p.context_bg), p.gutter_bg, None),
                };
                if let Some(bg) = bg {
                    quads.push(Quad {
                        bounds: row_bounds,
                        color: bg,
                    });
                }
                // EP-002 US-007: gutter rail - a slightly stronger tint over the
                // line-number column (incl. the hunk-bar lane) so the gutter
                // reads as a structural rail on every content row.
                quads.push(Quad {
                    bounds: Bounds::new(origin, size(px(BAR_W + PAD2) + self.gutter_w, row_h)),
                    color: gutter_bg,
                });
                // Zed-style colored hunk-indicator bar at the far left.
                if let Some(c) = bar_color {
                    Self::push_hunk_bar(origin, lh, c, row.kind == RowKind::Removed, quads);
                }
                // One line-number gutter (Zed shows a single merged display-row
                // number: new line for adds/context, old line for deletes).
                let line_no = row.new_no.or(row.old_no);
                let num_color = match row.kind {
                    RowKind::Added => p.gutter_add,
                    RowKind::Removed => p.gutter_del,
                    _ => p.muted,
                };
                self.gutter(
                    window,
                    line_no,
                    origin.x + px(BAR_W + PAD2),
                    origin.y,
                    num_color,
                    glyphs,
                );
                // Text - no +/- sign column (the bar + wash convey status). The
                // code shifts left by this file's horizontal offset; the gutter
                // (painted above) stays pinned and the clip holds the shifted
                // text inside the code column.
                let text_x = origin.x + px(BAR_W + PAD2) + self.gutter_w;
                let code_x = text_x - h_offset;
                if !row.text.is_empty() {
                    let line = self.shape(window, &row.text, &row.syntax_runs, p.text);
                    glyphs.push(Glyphs {
                        origin: point(code_x, origin.y),
                        line,
                        clip: Some(Bounds::new(
                            point(text_x, origin.y),
                            size((row_bounds.right() - text_x).max(px(0.)), lh),
                        )),
                    });
                }
            }
        }
    }

    /// Emit draw commands for one split row at top-left `origin`.
    #[allow(clippy::too_many_arguments)]
    fn layout_split(
        &self,
        window: &mut Window,
        cx: &mut App,
        row: &SplitRow,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        collapsed: bool,
        h_offset_left: Pixels,
        h_offset_right: Pixels,
        quads: &mut Vec<Quad>,
        rounded_quads: &mut Vec<RoundedQuad>,
        images: &mut Vec<ImagePaint>,
        glyphs: &mut Vec<Glyphs>,
        hatches: &mut Vec<HatchLine>,
    ) {
        let p = &self.palette;
        let lh = self.line_height;
        let row_bounds = Bounds::new(origin, size(width, row_h));

        match row {
            // EP-002 US-006: same structured header as the unified view.
            SplitRow::Header(parts) => {
                self.paint_file_header(
                    window, cx, parts, origin, width, row_h, collapsed, false, quads, images,
                    glyphs,
                );
            }
            SplitRow::Note(text) => {
                glyphs.push(Glyphs {
                    origin: point(origin.x + px(PAD), origin.y),
                    line: self.shape_plain(window, text.clone(), p.muted),
                    clip: Some(row_bounds),
                });
            }
            SplitRow::Fold(fold) => {
                let divider_w = px(SPLIT_DIVIDER_W);
                let half_w = ((width - divider_w) / 2.0).max(px(0.));
                let right_x = origin.x + half_w + divider_w;
                self.layout_fold_region(
                    window,
                    Some(&fold.text),
                    origin,
                    half_w,
                    row_h,
                    px(BAR_W + HALF_PAD) + self.gutter_w,
                    true,
                    false,
                    rounded_quads,
                    glyphs,
                    hatches,
                );
                quads.push(Quad {
                    bounds: Bounds::new(point(origin.x + half_w, origin.y), size(divider_w, row_h)),
                    color: p.context_bg,
                });
                self.layout_fold_region(
                    window,
                    None,
                    point(right_x, origin.y),
                    half_w,
                    row_h,
                    px(0.),
                    false,
                    true,
                    rounded_quads,
                    glyphs,
                    hatches,
                );
            }
            SplitRow::Pair { left, right } => {
                let divider_w = px(SPLIT_DIVIDER_W);
                let half_w = ((width - divider_w) / 2.0).max(px(0.));
                let left_x = origin.x;
                let right_x = origin.x + half_w + divider_w;
                self.layout_half(
                    window,
                    left,
                    point(left_x, origin.y),
                    half_w,
                    h_offset_left,
                    quads,
                    glyphs,
                    hatches,
                );
                // Divider.
                quads.push(Quad {
                    bounds: Bounds::new(point(origin.x + half_w, origin.y), size(divider_w, lh)),
                    color: p.context_bg,
                });
                self.layout_half(
                    window,
                    right,
                    point(right_x, origin.y),
                    half_w,
                    h_offset_right,
                    quads,
                    glyphs,
                    hatches,
                );
            }
        }
    }

    /// One half-cell of a split row, occupying `[origin.x, origin.x + width)`.
    #[allow(clippy::too_many_arguments)]
    fn layout_half(
        &self,
        window: &mut Window,
        cell: &HalfCell,
        origin: Point<Pixels>,
        width: Pixels,
        h_offset: Pixels,
        quads: &mut Vec<Quad>,
        glyphs: &mut Vec<Glyphs>,
        hatches: &mut Vec<HatchLine>,
    ) {
        let p = &self.palette;
        let lh = self.line_height;
        let half_bounds = Bounds::new(origin, size(width, lh));
        let (bg, gutter_bg, bar_color) = match cell.kind {
            CellKind::Added => (Some(p.add_bg), p.add_gutter_bg, Some(p.add_bar)),
            CellKind::Removed => (Some(p.del_bg), p.del_gutter_bg, Some(p.del_bar)),
            CellKind::Phantom => (Some(p.phantom_bg), p.gutter_bg, None),
            // EP-002 US-007: faint document wash on unchanged code.
            CellKind::Context => (Some(p.context_bg), p.gutter_bg, None),
        };
        if let Some(bg) = bg {
            quads.push(Quad {
                bounds: half_bounds,
                color: bg,
            });
        }
        if matches!(cell.kind, CellKind::Phantom) {
            let hatch_x = origin.x + px(BAR_W + HALF_PAD) + self.gutter_w;
            let hatch_w = (half_bounds.right() - hatch_x).max(px(0.));
            self.push_phantom_hatches(
                Bounds::new(point(hatch_x, origin.y), size(hatch_w, lh)),
                hatches,
            );
            return; // empty gap: no hunk bar, number, or code text
        }
        // EP-002 US-007: gutter rail over this half's line-number column.
        quads.push(Quad {
            bounds: Bounds::new(origin, size(px(BAR_W + HALF_PAD) + self.gutter_w, lh)),
            color: gutter_bg,
        });
        // Zed-style colored hunk-indicator bar at the half's left edge.
        if let Some(c) = bar_color {
            Self::push_hunk_bar(origin, lh, c, cell.kind == CellKind::Removed, quads);
        }
        let num_color = match cell.kind {
            CellKind::Added => p.gutter_add,
            CellKind::Removed => p.gutter_del,
            _ => p.muted,
        };
        self.gutter(
            window,
            cell.no,
            origin.x + px(BAR_W + HALF_PAD),
            origin.y,
            num_color,
            glyphs,
        );
        let text_x = origin.x + px(BAR_W + HALF_PAD) + self.gutter_w;
        let code_x = text_x - h_offset;
        if !cell.text.is_empty() {
            let line = self.shape(window, &cell.text, &cell.syntax_runs, p.text);
            glyphs.push(Glyphs {
                origin: point(code_x, origin.y),
                line,
                clip: Some(Bounds::new(
                    point(text_x, origin.y),
                    size((half_bounds.right() - text_x).max(px(0.)), lh),
                )),
            });
        }
    }

    fn gutter(
        &self,
        window: &mut Window,
        n: Option<u32>,
        x: Pixels,
        y: Pixels,
        color: Hsla,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let Some(n) = n else { return };
        let line = self.shape_plain(window, n.to_string().into(), color);
        // Right-align within the derived gutter column [x, x + gutter_w) (Zed
        // places line numbers flush-right with a small gap before the code),
        // clamped so very wide numbers don't underflow past the gutter's left
        // edge.
        let right_x = (x + self.gutter_w - px(NUM_GAP) - line.width()).max(x);
        glyphs.push(Glyphs {
            origin: point(right_x, y),
            line,
            clip: None,
        });
    }
}

impl Element for DiffElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<DiffPrepaint>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        // Full content height (sum of variable row heights) - the hosting
        // `overflow_y_scroll` div clips/scrolls. Reads the precomputed offsets
        // (no per-frame walk).
        let h = px(self.body.offsets_rc().last().copied().unwrap_or(0.0));
        style.size.height = Length::Definite(h.into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let row_count = self.body.len();

        // Variable-height layout: cumulative top offsets per row (precomputed,
        // shared). Cull against them (binary search) instead of a uniform line
        // height.
        let offsets = self.body.offsets_rc();
        let mask = window.content_mask();
        let vtop = f32::from(mask.bounds.origin.y - bounds.origin.y).max(0.0);
        let vbot = vtop + f32::from(mask.bounds.size.height);
        // First row whose top is at/above the viewport top; last row (exclusive)
        // whose top is still above the viewport bottom.
        let first = offsets
            .partition_point(|&o| o <= vtop)
            .saturating_sub(1)
            .min(row_count);
        let last = offsets.partition_point(|&o| o < vbot).min(row_count);
        // Diagnostic: fires only if the clip rect did NOT bound the visible
        // window (i.e. content_mask == full content height) → culling broken.
        if last.saturating_sub(first) > 200 {
            log::warn!(
                "diff element: visible {first}..{last} = {} of {row_count} rows - culling off?",
                last - first
            );
        }

        // Derive the gutter width from the body's widest line number so 5-digit
        // line numbers in large files never clip past the gutter's left edge.
        // One shaped digit gives the exact monospace advance.
        let digits = {
            // Count decimal digits by integer division - robust across libm
            // implementations, unlike `log10().floor()` whose rounding can be
            // off-by-one at exact powers of ten (log10(1000) may be 2.9999998).
            let mut n = self.body.max_line_no().max(1);
            let mut count = 0usize;
            while n > 0 {
                count += 1;
                n /= 10;
            }
            count.max(2)
        };
        let digit_w = f32::from(
            self.shape_plain(window, "0".into(), self.palette.muted)
                .width(),
        );
        self.gutter_w = px((GUTTER_PAD_L + digits as f32 * digit_w + NUM_GAP).max(GUTTER_W));

        // Per-file horizontal scroll inputs (spans + live offsets), read once so
        // the shaping loop offsets each file's code without re-borrowing `body`.
        let spans = self.body.spans_rc();
        let h_offsets = self.body.h_offsets_rc();
        let width = bounds.size.width;
        let split = matches!(&self.body, DiffBody::Split { .. });
        let visible_rows = last.saturating_sub(first);
        let mut quads = Vec::with_capacity(visible_rows.saturating_mul(if split { 5 } else { 4 }));
        let mut rounded_quads = Vec::with_capacity(visible_rows / 4);
        let mut images = Vec::with_capacity(visible_rows / 12);
        let mut glyphs = Vec::with_capacity(visible_rows.saturating_mul(if split { 4 } else { 2 }));
        let mut hatches = Vec::with_capacity(visible_rows);
        let mut scrollbars = Vec::new();
        let mut sticky_quads = Vec::with_capacity(2);
        let mut sticky_images = Vec::with_capacity(1);
        let mut sticky_glyphs = Vec::with_capacity(3);

        // Clone the Rc so the borrow of `self.body` doesn't conflict with the
        // `&mut self` shaping calls below.
        match &self.body {
            DiffBody::Unified { rows, .. } => {
                let rows = rows.clone();
                for i in first..last {
                    let origin = point(bounds.origin.x, bounds.origin.y + px(offsets[i]));
                    let row_h = px(offsets[i + 1] - offsets[i]);
                    // A file is folded when the row after its header is another
                    // header (or EOF) - collapsed files emit a header-only row.
                    let collapsed = rows
                        .get(i + 1)
                        .is_none_or(|r| r.kind == RowKind::FileHeader);
                    let h_offset = px(file_at_row(&spans, i)
                        .map(|f| {
                            file_side_offset(&spans, &h_offsets, f, false, false, f32::from(width))
                        })
                        .unwrap_or(0.0));
                    self.layout_unified(
                        window,
                        cx,
                        &rows[i],
                        origin,
                        width,
                        row_h,
                        collapsed,
                        h_offset,
                        &mut quads,
                        &mut rounded_quads,
                        &mut images,
                        &mut glyphs,
                        &mut hatches,
                    );
                }
                // Pinned sticky header for the file under the viewport top.
                let cur = spans
                    .partition_point(|span| span.header_row <= first)
                    .checked_sub(1)
                    .and_then(|file_idx| {
                        spans.get(file_idx).map(|span| (file_idx, span.header_row))
                    });
                if let Some((file_idx, hidx)) = cur
                    && offsets[hidx] < vtop
                {
                    // Next file header within the slide-up band → push the sticky
                    // up so the incoming file's inline header displaces it.
                    let nh = spans.get(file_idx + 1).map(|span| span.header_row);
                    let mut sticky_y = mask.bounds.origin.y;
                    if let Some(nh) = nh {
                        let nh_abs = bounds.origin.y + px(offsets[nh]);
                        if nh_abs < mask.bounds.origin.y + px(STICKY_HEADER_HEIGHT) {
                            sticky_y = nh_abs - px(STICKY_HEADER_HEIGHT);
                        }
                    }
                    if sticky_y + px(STICKY_HEADER_HEIGHT) > mask.bounds.origin.y
                        && let Some(parts) = rows[hidx].header.as_ref()
                    {
                        self.paint_file_header(
                            window,
                            cx,
                            parts,
                            point(bounds.origin.x, sticky_y),
                            width,
                            px(STICKY_HEADER_HEIGHT),
                            false,
                            true,
                            &mut sticky_quads,
                            &mut sticky_images,
                            &mut sticky_glyphs,
                        );
                    }
                }
            }
            DiffBody::Split { rows, .. } => {
                let rows = rows.clone();
                for i in first..last {
                    let origin = point(bounds.origin.x, bounds.origin.y + px(offsets[i]));
                    let row_h = px(offsets[i + 1] - offsets[i]);
                    let collapsed = rows
                        .get(i + 1)
                        .is_none_or(|r| matches!(r, SplitRow::Header(_)));
                    let (h_offset_left, h_offset_right) = file_at_row(&spans, i)
                        .map(|f| {
                            (
                                px(file_side_offset(
                                    &spans,
                                    &h_offsets,
                                    f,
                                    false,
                                    true,
                                    f32::from(width),
                                )),
                                px(file_side_offset(
                                    &spans,
                                    &h_offsets,
                                    f,
                                    true,
                                    true,
                                    f32::from(width),
                                )),
                            )
                        })
                        .unwrap_or((px(0.0), px(0.0)));
                    self.layout_split(
                        window,
                        cx,
                        &rows[i],
                        origin,
                        width,
                        row_h,
                        collapsed,
                        h_offset_left,
                        h_offset_right,
                        &mut quads,
                        &mut rounded_quads,
                        &mut images,
                        &mut glyphs,
                        &mut hatches,
                    );
                }
                // Pinned sticky header for the file under the viewport top.
                let cur = spans
                    .partition_point(|span| span.header_row <= first)
                    .checked_sub(1)
                    .and_then(|file_idx| {
                        spans.get(file_idx).map(|span| (file_idx, span.header_row))
                    });
                if let Some((file_idx, hidx)) = cur
                    && offsets[hidx] < vtop
                {
                    let nh = spans.get(file_idx + 1).map(|span| span.header_row);
                    let mut sticky_y = mask.bounds.origin.y;
                    if let Some(nh) = nh {
                        let nh_abs = bounds.origin.y + px(offsets[nh]);
                        if nh_abs < mask.bounds.origin.y + px(STICKY_HEADER_HEIGHT) {
                            sticky_y = nh_abs - px(STICKY_HEADER_HEIGHT);
                        }
                    }
                    if sticky_y + px(STICKY_HEADER_HEIGHT) > mask.bounds.origin.y
                        && let SplitRow::Header(parts) = &rows[hidx]
                    {
                        self.paint_file_header(
                            window,
                            cx,
                            parts,
                            point(bounds.origin.x, sticky_y),
                            width,
                            px(STICKY_HEADER_HEIGHT),
                            false,
                            true,
                            &mut sticky_quads,
                            &mut sticky_images,
                            &mut sticky_glyphs,
                        );
                    }
                }
            }
        }

        let segments = h_scrollbar_segments(
            &spans,
            &offsets,
            &h_offsets,
            split,
            f32::from(width),
            vtop,
            vbot,
        );
        self.push_horizontal_scrollbars(bounds, &segments, &mut scrollbars);
        let hatches = Self::build_hatch_paths(hatches);

        log::trace!(
            target: "paneflow::diff::render",
            "diff prepaint rows={}-{} visible={} total={} glyphs={} quads={} rounded={} images={} hatches={} scrollbars={} spans={}",
            first,
            last,
            last.saturating_sub(first),
            row_count,
            glyphs.len() + sticky_glyphs.len(),
            quads.len() + sticky_quads.len(),
            rounded_quads.len(),
            images.len() + sticky_images.len(),
            hatches.len(),
            scrollbars.len(),
            spans.len()
        );

        Some(DiffPrepaint {
            quads,
            rounded_quads,
            images,
            glyphs,
            hatches,
            scrollbars,
            sticky_quads,
            sticky_images,
            sticky_glyphs,
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = prepaint.take() else {
            return;
        };
        let lh = self.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in &layout.quads {
                window.paint_quad(fill(q.bounds, q.color));
            }
            for q in &layout.rounded_quads {
                window.paint_quad(quad(
                    q.bounds,
                    q.corners,
                    q.color,
                    px(0.),
                    q.color,
                    BorderStyle::Solid,
                ));
            }
            for image in &layout.images {
                let _ = window.paint_image(
                    image.bounds,
                    Corners::default(),
                    image.image.clone(),
                    0,
                    false,
                );
            }
            for hatch in layout.hatches {
                window.paint_path(hatch.path, hatch.color);
            }
            for g in layout.glyphs {
                if let Some(clip) = g.clip {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let _ = g
                            .line
                            .paint(g.origin, lh, TextAlign::Left, None, window, cx);
                    });
                } else {
                    let _ = g
                        .line
                        .paint(g.origin, lh, TextAlign::Left, None, window, cx);
                }
            }
            // Sticky header floats above the scrolled body: paint its quads then
            // glyphs LAST so they overlay the rows that scroll underneath.
            for q in &layout.sticky_quads {
                window.paint_quad(fill(q.bounds, q.color));
            }
            for image in &layout.sticky_images {
                let _ = window.paint_image(
                    image.bounds,
                    Corners::default(),
                    image.image.clone(),
                    0,
                    false,
                );
            }
            for g in layout.sticky_glyphs {
                if let Some(clip) = g.clip {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let _ = g
                            .line
                            .paint(g.origin, lh, TextAlign::Left, None, window, cx);
                    });
                } else {
                    let _ = g
                        .line
                        .paint(g.origin, lh, TextAlign::Left, None, window, cx);
                }
            }
            if !layout.scrollbars.is_empty() {
                window.paint_layer(bounds, |window| {
                    for q in &layout.scrollbars {
                        window.paint_quad(quad(
                            q.bounds,
                            q.corners,
                            q.color,
                            px(0.),
                            q.color,
                            BorderStyle::Solid,
                        ));
                    }
                });
            }
        });
    }
}

impl IntoElement for DiffElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

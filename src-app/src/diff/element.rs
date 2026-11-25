use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Corners, Element, ElementId, Font, FontFeatures,
    FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, LayoutId, Length, Path,
    PathBuilder, Pixels, Point, ShapedLine, SharedString, Style, TextRun, TransformationMatrix,
    Window, accesskit, fill, point, px, quad, relative, size,
};

use super::align::CellKind;
use super::engine::{ChangeTone, HighlightPolicy};
use super::git::FileChange;
use super::hscroll::{
    H_SCROLLBAR_TRACK_HEIGHT, SPLIT_DIVIDER_W, file_at_row, file_side_offset, h_scrollbar_segments,
};
use super::rows::{
    DisplayRow, FILE_HEADER_HEIGHT, FileSpan, HalfCell, HeaderParts, ROW_HEIGHT, RowKind,
    RowPalette, SplitRow,
};
use crate::tab_stops::{PaintedLine, TabExpandedLine};

const UI_FONT_FAMILY: &str = "Geist";
const CODE_FONT_SIZE: f32 = 13.0;
const HEADER_FONT_SIZE: f32 = 12.0;
const SEPARATOR_FONT_SIZE: f32 = 11.0;
const NOTE_PAD_X: f32 = 8.0;
const FILE_HEADER_CHEVRON_X: f32 = 8.0;
const FILE_HEADER_ICON_X: f32 = 34.0;
const FILE_HEADER_TITLE_X: f32 = 58.0;
const FILE_HEADER_ICON_SIZE: f32 = 16.0;
const FILE_HEADER_GAP: f32 = 8.0;
const FILE_HEADER_PAD_R: f32 = 12.0;
const FILE_HEADER_STAT_COLUMN_CH: f32 = 4.0;
const FILE_HEADER_HOVER_EDGE_W: f32 = 3.0;
const FILE_HEADER_PREVIOUS_PATH_ALPHA: f32 = 0.7;
const FILE_HEADER_CHEVRON_ALPHA: f32 = 0.8;
const FILE_HEADER_EXPANDED_ICON: &str = "icons/chevron-down.svg";
const FILE_HEADER_COLLAPSED_ICON: &str = "icons/chevron-right.svg";
const FILE_ADDED_ICON: &str = "icons/diff/file-added.svg";
const FILE_MODIFIED_ICON: &str = "icons/diff/file-modified.svg";
const FILE_DELETED_ICON: &str = "icons/diff/file-deleted.svg";
const FILE_MOVED_ICON: &str = "icons/diff/file-moved.svg";
const RENAME_ARROW_ICON: &str = "icons/diff/arrow-right-short.svg";
const EXPAND_UP_ICON: &str = "icons/diff/expand-up.svg";
const EXPAND_DOWN_ICON: &str = "icons/diff/expand-down.svg";
const EXPAND_ALL_ICON: &str = "icons/diff/expand-all.svg";
const GUTTER_PAD_L_CH: f32 = 2.0;
const GUTTER_PAD_R_CH: f32 = 1.0;
const GUTTER_MIN_DIGITS: usize = 3;
const GUTTER_BORDER_W: f32 = 2.0;
const LINE_PAD_CH: f32 = 1.0;
const BAR_W: f32 = 4.0;
const DELETED_BAR_DASH_H: f32 = 1.0;
const DELETED_BAR_DASH_STEP: f32 = 2.0;
const SEPARATOR_INSET_X: f32 = 8.0;
const SEPARATOR_BUTTON_W: f32 = 32.0;
const SEPARATOR_BUTTON_GAP: f32 = 2.0;
const SEPARATOR_RADIUS: f32 = 6.0;
const SEPARATOR_ICON_SIZE: f32 = 16.0;
const SEPARATOR_LABEL_PAD_X: f32 = 6.0;
const PHANTOM_HATCH_SPACING: f32 = 8.0;
const PHANTOM_HATCH_STROKE: f32 = 1.4;
pub(crate) const REVERT_CHIP_W: f32 = 56.0;
pub(crate) const REVERT_CHIP_H: f32 = 16.0;
pub(crate) const REVERT_CHIP_PAD_R: f32 = 10.0;
const REVERT_CHIP_LABEL: &str = "Revert";

pub(crate) fn revert_chip_bounds(
    row_origin: Point<Pixels>,
    row_width: Pixels,
    row_h: Pixels,
) -> Bounds<Pixels> {
    let x = row_origin.x + row_width - px(REVERT_CHIP_PAD_R + REVERT_CHIP_W);
    let y = row_origin.y + (row_h - px(REVERT_CHIP_H)) / 2.0;
    Bounds::new(point(x, y), size(px(REVERT_CHIP_W), px(REVERT_CHIP_H)))
}

pub(super) fn for_each_word_rect(
    runs: &[Range<usize>],
    x_for_index: impl Fn(usize) -> f32,
    code_x: f32,
    clip_left: f32,
    clip_right: f32,
    mut emit: impl FnMut(f32, f32),
) {
    for run in runs {
        let x0 = (code_x + x_for_index(run.start)).max(clip_left);
        let x1 = (code_x + x_for_index(run.end)).min(clip_right);
        if x1 > x0 {
            emit(x0, x1);
        }
    }
}

pub enum DiffBody {
    Unified {
        rows: Rc<Vec<DisplayRow>>,
        offsets: Rc<Vec<f32>>,
        max_line_no: u32,
        spans: Rc<Vec<FileSpan>>,
        h_offsets: Rc<Vec<f32>>,
    },
    Split {
        rows: Rc<Vec<SplitRow>>,
        offsets: Rc<Vec<f32>>,
        max_line_no: u32,
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

    fn max_line_no(&self) -> u32 {
        match self {
            DiffBody::Unified { max_line_no, .. } | DiffBody::Split { max_line_no, .. } => {
                *max_line_no
            }
        }
    }

    fn offsets_rc(&self) -> Rc<Vec<f32>> {
        match self {
            DiffBody::Unified { offsets, .. } | DiffBody::Split { offsets, .. } => offsets.clone(),
        }
    }

    fn spans_rc(&self) -> Rc<Vec<FileSpan>> {
        match self {
            DiffBody::Unified { spans, .. } | DiffBody::Split { spans, .. } => spans.clone(),
        }
    }

    fn h_offsets_rc(&self) -> Rc<Vec<f32>> {
        match self {
            DiffBody::Unified { h_offsets, .. } | DiffBody::Split { h_offsets, .. } => {
                h_offsets.clone()
            }
        }
    }
}

struct Quad {
    bounds: Bounds<Pixels>,
    color: Hsla,
}

struct RoundedQuad {
    bounds: Bounds<Pixels>,
    color: Hsla,
    corners: Corners<Pixels>,
}

struct Glyphs {
    origin: Point<Pixels>,
    line: PaintedLine,
    clip: Option<Bounds<Pixels>>,
}

struct HatchLine {
    start: Point<Pixels>,
    end: Point<Pixels>,
    color: Hsla,
}

struct HatchPath {
    path: Path,
    color: Hsla,
}

struct MaskIcon {
    bounds: Bounds<Pixels>,
    path: &'static str,
    color: Hsla,
}

fn paint_icons(icons: &[MaskIcon], window: &mut Window, cx: &App) {
    for icon in icons {
        let _ = window.paint_svg(
            icon.bounds,
            icon.path.into(),
            None,
            TransformationMatrix::unit(),
            icon.color,
            cx,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FoldEdge {
    Leading,
    Between,
    Trailing,
}

impl FoldEdge {
    fn between(after_header: bool, before_header: bool) -> Self {
        match (after_header, before_header) {
            (true, false) => FoldEdge::Leading,
            (false, true) => FoldEdge::Trailing,
            _ => FoldEdge::Between,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            FoldEdge::Leading => EXPAND_UP_ICON,
            FoldEdge::Between => EXPAND_ALL_ICON,
            FoldEdge::Trailing => EXPAND_DOWN_ICON,
        }
    }
}

#[derive(Clone, Copy)]
struct RowContext {
    collapsed: bool,
    hovered: bool,
    fold_edge: FoldEdge,
}

struct LineCell<'a> {
    added: Option<bool>,
    tone: ChangeTone,
    number: Option<u32>,
    text: &'a SharedString,
    syntax_runs: &'a [(Range<usize>, Hsla)],
    change_runs: &'a [Range<usize>],
}

fn change_icon(change: FileChange, palette: &RowPalette) -> (&'static str, Hsla) {
    match change {
        FileChange::Added => (FILE_ADDED_ICON, palette.icon_added),
        FileChange::Deleted => (FILE_DELETED_ICON, palette.icon_deleted),
        FileChange::Renamed => (FILE_MOVED_ICON, palette.icon_modified),
        FileChange::Modified | FileChange::TypeChanged => {
            (FILE_MODIFIED_ICON, palette.icon_modified)
        }
    }
}

fn sticky_header(
    spans: &[FileSpan],
    offsets: &[f32],
    first: usize,
    vtop: f32,
    content_top: Pixels,
    viewport_top: Pixels,
) -> Option<(usize, Pixels)> {
    let header_top = |row: usize| offsets[row + 1] - FILE_HEADER_HEIGHT;
    let file = spans
        .partition_point(|span| span.header_row <= first)
        .checked_sub(1)?;
    let header_row = spans.get(file)?.header_row;
    if header_top(header_row) >= vtop {
        return None;
    }
    let height = px(FILE_HEADER_HEIGHT);
    let mut y = viewport_top;
    if let Some(next) = spans.get(file + 1) {
        let next_top = content_top + px(header_top(next.header_row));
        if next_top < viewport_top + height {
            y = next_top - height;
        }
    }
    (y + height > viewport_top).then_some((header_row, y))
}

pub struct DiffPrepaint {
    quads: Vec<Quad>,
    rounded_quads: Vec<RoundedQuad>,
    images: Vec<MaskIcon>,
    glyphs: Vec<Glyphs>,
    hatches: Vec<HatchPath>,
    chips: Vec<RoundedQuad>,
    chip_glyphs: Vec<Glyphs>,
    scrollbars: Vec<RoundedQuad>,
    sticky_quads: Vec<Quad>,
    sticky_images: Vec<MaskIcon>,
    sticky_glyphs: Vec<Glyphs>,
    headers: Vec<FileHeaderTarget>,
    scale: f32,
    header_nodes: Vec<(accesskit::NodeId, SharedString)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileHeaderTarget {
    pub path: SharedString,
    pub change: FileChange,
    pub collapsed: bool,
    pub bounds: Bounds<Pixels>,
}

pub type FileToggle = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

pub const DIFF_FILES_LABEL: &str = "Changed files";

pub fn file_header_node(header: &FileHeaderTarget, scale: f32) -> accesskit::Node {
    let mut node = accesskit::Node::new(accesskit::Role::Button);
    node.set_label(format!("{}, {}", header.path, header.change.spoken()));
    node.set_expanded(!header.collapsed);
    node.add_action(accesskit::Action::Click);
    let bounds = header.bounds;
    node.set_bounds(accesskit::Rect {
        x0: f64::from(f32::from(bounds.origin.x) * scale),
        y0: f64::from(f32::from(bounds.origin.y) * scale),
        x1: f64::from(f32::from(bounds.origin.x + bounds.size.width) * scale),
        y1: f64::from(f32::from(bounds.origin.y + bounds.size.height) * scale),
    });
    node
}

pub fn visible_file_headers(
    body: &DiffBody,
    first: usize,
    last: usize,
    sticky: Option<(usize, Pixels)>,
    origin: Point<Pixels>,
    width: Pixels,
) -> Vec<FileHeaderTarget> {
    let offsets = body.offsets_rc();
    let header_at = |row: usize| -> Option<(SharedString, FileChange, bool)> {
        match body {
            DiffBody::Unified { rows, .. } => {
                let parts = rows.get(row)?.header.as_ref()?;
                let collapsed = rows
                    .get(row + 1)
                    .is_none_or(|next| next.kind == RowKind::FileHeader);
                Some((parts.path.clone(), parts.change, collapsed))
            }
            DiffBody::Split { rows, .. } => {
                let SplitRow::Header(parts) = rows.get(row)? else {
                    return None;
                };
                let collapsed = rows
                    .get(row + 1)
                    .is_none_or(|next| matches!(next, SplitRow::Header(_)));
                Some((parts.path.clone(), parts.change, collapsed))
            }
        }
    };
    let target = |row: usize, top: Pixels| {
        header_at(row).map(|(path, change, collapsed)| FileHeaderTarget {
            path,
            change,
            collapsed,
            bounds: Bounds::new(point(origin.x, top), size(width, px(FILE_HEADER_HEIGHT))),
        })
    };
    let sticky = sticky
        .filter(|(row, _)| !(first..last).contains(row))
        .and_then(|(row, top)| target(row, top));
    sticky
        .into_iter()
        .chain((first..last).filter_map(|row| {
            let top = origin.y + px(*offsets.get(row)?);
            target(row, top)
        }))
        .collect()
}

pub struct DiffElement {
    body: DiffBody,
    palette: RowPalette,
    font: Font,
    ui_font: Font,
    font_size: Pixels,
    line_height: Pixels,
    ch: Pixels,
    gutter_w: Pixels,
    revert_chip: Option<usize>,
    hovered_header: Option<usize>,
    highlight: HighlightPolicy,
    file_toggle: Option<FileToggle>,
}

impl DiffElement {
    pub fn new(body: DiffBody, palette: RowPalette) -> Self {
        thread_local! {
            static MONO_FAMILY: SharedString =
                crate::terminal::element::resolve_font_family(None).into();
        }
        let family = MONO_FAMILY.with(|f| f.clone());
        let ch = CODE_FONT_SIZE * 0.6;
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
            ui_font: Font {
                family: UI_FONT_FAMILY.into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            },
            font_size: px(CODE_FONT_SIZE),
            line_height: px(ROW_HEIGHT),
            ch: px(ch),
            gutter_w: px(ch * (GUTTER_PAD_L_CH + GUTTER_MIN_DIGITS as f32 + GUTTER_PAD_R_CH)),
            revert_chip: None,
            hovered_header: None,
            highlight: HighlightPolicy::Words,
            file_toggle: None,
        }
    }

    pub fn with_file_toggle(mut self, toggle: FileToggle) -> Self {
        self.file_toggle = Some(toggle);
        self
    }

    pub fn with_highlight(mut self, highlight: HighlightPolicy) -> Self {
        self.highlight = highlight;
        self
    }

    pub fn with_revert_chip(mut self, row: Option<usize>) -> Self {
        self.revert_chip = row;
        self
    }

    pub fn with_hovered_header(mut self, row: Option<usize>) -> Self {
        self.hovered_header = row;
        self
    }

    fn push_revert_chip(
        &self,
        window: &mut Window,
        row_origin: Point<Pixels>,
        row_width: Pixels,
        row_h: Pixels,
        chips: &mut Vec<RoundedQuad>,
        chip_glyphs: &mut Vec<Glyphs>,
    ) {
        let bounds = revert_chip_bounds(row_origin, row_width, row_h);
        chips.push(RoundedQuad {
            bounds,
            color: self.palette.chip_bg,
            corners: Corners::all(bounds.size.height / 2.0),
        });
        let label = shape_plain(
            window,
            &self.font,
            self.font_size,
            REVERT_CHIP_LABEL.into(),
            self.palette.chip_fg,
        );
        let x = bounds.origin.x + ((bounds.size.width - label.width()) / 2.0).max(px(0.));
        let y = bounds.origin.y + (bounds.size.height - self.line_height) / 2.0;
        chip_glyphs.push(Glyphs {
            origin: point(x, y),
            line: label.into(),
            clip: None,
        });
    }

    fn shape(
        &self,
        window: &mut Window,
        text: &SharedString,
        syntax: &[(Range<usize>, Hsla)],
        default: Hsla,
    ) -> TabExpandedLine {
        if syntax.is_empty() {
            let runs = [TextRun {
                len: text.len(),
                font: self.font.clone(),
                color: default,
                background_color: None,
                underline: None,
                strikethrough: None,
            }];
            return TabExpandedLine::shape(text, self.font_size, &runs, window.text_system());
        }

        let runs = text_runs(text, syntax, &self.font, default);
        TabExpandedLine::shape(text, self.font_size, &runs, window.text_system())
    }

    fn shape_ui(
        &self,
        window: &mut Window,
        text: SharedString,
        font_size: f32,
        color: Hsla,
    ) -> ShapedLine {
        shape_plain(window, &self.ui_font, px(font_size), text, color)
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_file_header(
        &self,
        window: &mut Window,
        parts: &HeaderParts,
        origin: Point<Pixels>,
        width: Pixels,
        collapsed: bool,
        hovered: bool,
        quads: &mut Vec<Quad>,
        icons: &mut Vec<MaskIcon>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let p = &self.palette;
        let row_h = px(FILE_HEADER_HEIGHT);
        let bounds = Bounds::new(origin, size(width, row_h));
        quads.push(Quad {
            bounds,
            color: p.header_bg,
        });
        if hovered {
            quads.push(Quad {
                bounds: Bounds::new(origin, size(px(FILE_HEADER_HOVER_EDGE_W), row_h)),
                color: p.header_hover_edge,
            });
        }

        let icon = px(FILE_HEADER_ICON_SIZE);
        let gap = px(FILE_HEADER_GAP);
        let icon_y = origin.y + (row_h - icon) / 2.0;
        let icon_at = |x: Pixels| Bounds::new(point(x, icon_y), size(icon, icon));
        icons.push(MaskIcon {
            bounds: icon_at(origin.x + px(FILE_HEADER_CHEVRON_X)),
            path: if collapsed {
                FILE_HEADER_COLLAPSED_ICON
            } else {
                FILE_HEADER_EXPANDED_ICON
            },
            color: p.muted.opacity(FILE_HEADER_CHEVRON_ALPHA),
        });
        let (change_path, change_color) = change_icon(parts.change, p);
        icons.push(MaskIcon {
            bounds: icon_at(origin.x + px(FILE_HEADER_ICON_X)),
            path: change_path,
            color: change_color,
        });

        let ty = origin.y + (row_h - self.line_height) / 2.0;
        let stat_size = px(HEADER_FONT_SIZE);
        let column_w = shape_plain(window, &self.font, stat_size, "0".into(), p.muted).width()
            * FILE_HEADER_STAT_COLUMN_CH;
        let added_line = shape_plain(
            window,
            &self.font,
            stat_size,
            format!("+{}", parts.added).into(),
            p.add_fg,
        );
        let removed_line = shape_plain(
            window,
            &self.font,
            stat_size,
            format!("-{}", parts.removed).into(),
            p.del_fg,
        );
        let removed_right = bounds.right() - px(FILE_HEADER_PAD_R);
        let added_right = removed_right - column_w.max(removed_line.width()) - gap;
        let added_x = added_right - added_line.width();
        let stats_left = added_x.min(added_right - column_w);
        glyphs.push(Glyphs {
            origin: point(added_x, ty),
            line: added_line.into(),
            clip: Some(bounds),
        });
        glyphs.push(Glyphs {
            origin: point(removed_right - removed_line.width(), ty),
            line: removed_line.into(),
            clip: Some(bounds),
        });

        let title_x = origin.x + px(FILE_HEADER_TITLE_X);
        let title_right = (stats_left - gap).max(title_x);
        let title = self.shape_ui(window, parts.path.clone(), HEADER_FONT_SIZE, p.text);
        let mut name_x = title_x;
        if let Some(old_path) = &parts.old_path {
            let previous = self.shape_ui(
                window,
                old_path.clone(),
                HEADER_FONT_SIZE,
                p.text.opacity(FILE_HEADER_PREVIOUS_PATH_ALPHA),
            );
            let arrow_x = title_x + previous.width() + gap;
            let renamed_x = arrow_x + icon + gap;
            if renamed_x + title.width() <= title_right {
                glyphs.push(Glyphs {
                    origin: point(title_x, ty),
                    line: previous.into(),
                    clip: Some(bounds),
                });
                icons.push(MaskIcon {
                    bounds: icon_at(arrow_x),
                    path: RENAME_ARROW_ICON,
                    color: p.text,
                });
                name_x = renamed_x;
            }
        }

        let name_w = (title_right - name_x).max(px(0.));
        if title.width() <= name_w {
            glyphs.push(Glyphs {
                origin: point(name_x, ty),
                line: title.into(),
                clip: Some(bounds),
            });
            return;
        }
        let ellipsis = self.shape_ui(window, "\u{2026}".into(), HEADER_FONT_SIZE, p.text);
        let ellipsis_w = ellipsis.width().min(name_w);
        let clip_x = name_x + ellipsis_w;
        glyphs.push(Glyphs {
            origin: point(name_x, ty),
            line: ellipsis.into(),
            clip: Some(Bounds::new(
                point(name_x, origin.y),
                size(ellipsis_w, row_h),
            )),
        });
        glyphs.push(Glyphs {
            origin: point(title_right - title.width(), ty),
            line: title.into(),
            clip: Some(Bounds::new(
                point(clip_x, origin.y),
                size((title_right - clip_x).max(px(0.)), row_h),
            )),
        });
    }

    fn change_colors(&self, added: bool, tone: ChangeTone) -> (Hsla, Hsla, Option<Hsla>) {
        let p = &self.palette;
        let tone = match self.highlight {
            HighlightPolicy::Words => tone,
            HighlightPolicy::Lines => ChangeTone::Full,
            HighlightPolicy::None => ChangeTone::Plain,
        };
        let show_words = self.highlight == HighlightPolicy::Words;
        match (added, tone) {
            (true, ChangeTone::Full) => (p.add_bg, p.add_bar, show_words.then_some(p.word_add)),
            (false, ChangeTone::Full) => (p.del_bg, p.del_bar, show_words.then_some(p.word_del)),
            (true, ChangeTone::Muted) => (p.add_bg_muted, p.add_bar_muted, None),
            (false, ChangeTone::Muted) => (p.del_bg_muted, p.del_bar_muted, None),
            (true, ChangeTone::Plain) => (p.context_bg, p.add_bar, None),
            (false, ChangeTone::Plain) => (p.context_bg, p.del_bar, None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_word_rects(
        &self,
        line: &TabExpandedLine,
        runs: &[Range<usize>],
        code_x: Pixels,
        clip_left: Pixels,
        clip_right: Pixels,
        y: Pixels,
        color: Hsla,
        quads: &mut Vec<Quad>,
    ) {
        for_each_word_rect(
            runs,
            |index| f32::from(line.x_for_index(index)),
            f32::from(code_x),
            f32::from(clip_left),
            f32::from(clip_right),
            |x0, x1| {
                quads.push(Quad {
                    bounds: Bounds::new(point(px(x0), y), size(px(x1 - x0), self.line_height)),
                    color,
                });
            },
        );
    }

    fn push_hunk_bar(
        origin: Point<Pixels>,
        height: Pixels,
        color: Hsla,
        dash_gap: Option<Hsla>,
        quads: &mut Vec<Quad>,
    ) {
        let bar = Bounds::new(origin, size(px(BAR_W), height));
        let Some(dash_gap) = dash_gap else {
            quads.push(Quad { bounds: bar, color });
            return;
        };

        quads.push(Quad {
            bounds: bar,
            color: dash_gap,
        });
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
    fn layout_separator(
        &self,
        window: &mut Window,
        label: Option<&SharedString>,
        icon: Option<&'static str>,
        left: Pixels,
        right: Pixels,
        top: Pixels,
        row_h: Pixels,
        round_right: bool,
        rounded_quads: &mut Vec<RoundedQuad>,
        icons: &mut Vec<MaskIcon>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        if right <= left || row_h <= px(0.) {
            return;
        }

        let p = &self.palette;
        let radius = px(SEPARATOR_RADIUS);
        let square = px(0.);
        let mut content_left = left;
        if let Some(icon) = icon {
            let button_w = px(SEPARATOR_BUTTON_W).min(right - left);
            rounded_quads.push(RoundedQuad {
                bounds: Bounds::new(point(left, top), size(button_w, row_h)),
                color: p.separator_bg,
                corners: Corners {
                    top_left: radius,
                    top_right: square,
                    bottom_right: square,
                    bottom_left: radius,
                },
            });
            let icon_size = px(SEPARATOR_ICON_SIZE);
            icons.push(MaskIcon {
                bounds: Bounds::new(
                    point(
                        left + (button_w - icon_size) / 2.0,
                        top + (row_h - icon_size) / 2.0,
                    ),
                    size(icon_size, icon_size),
                ),
                path: icon,
                color: p.separator_fg,
            });
            content_left = left + button_w + px(SEPARATOR_BUTTON_GAP);
        }
        if right <= content_left {
            return;
        }

        let content = Bounds::new(point(content_left, top), size(right - content_left, row_h));
        let right_radius = if round_right { radius } else { square };
        rounded_quads.push(RoundedQuad {
            bounds: content,
            color: p.separator_bg,
            corners: Corners {
                top_left: square,
                top_right: right_radius,
                bottom_right: right_radius,
                bottom_left: square,
            },
        });
        if let Some(label) = label {
            glyphs.push(Glyphs {
                origin: point(
                    content_left + px(SEPARATOR_LABEL_PAD_X),
                    top + (row_h - self.line_height) / 2.0,
                ),
                line: self
                    .shape_ui(window, label.clone(), SEPARATOR_FONT_SIZE, p.separator_fg)
                    .into(),
                clip: Some(content),
            });
        }
    }

    fn layout_note(
        &self,
        window: &mut Window,
        text: &SharedString,
        row_bounds: Bounds<Pixels>,
        quads: &mut Vec<Quad>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let p = &self.palette;
        quads.push(Quad {
            bounds: row_bounds,
            color: p.context_bg,
        });
        glyphs.push(Glyphs {
            origin: point(
                row_bounds.origin.x + px(NOTE_PAD_X),
                row_bounds.origin.y + (row_bounds.size.height - self.line_height) / 2.0,
            ),
            line: shape_plain(window, &self.font, self.font_size, text.clone(), p.muted).into(),
            clip: Some(row_bounds),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_header_row(
        &self,
        window: &mut Window,
        parts: &HeaderParts,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        ctx: RowContext,
        quads: &mut Vec<Quad>,
        icons: &mut Vec<MaskIcon>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let gap = (row_h - px(FILE_HEADER_HEIGHT)).max(px(0.));
        if gap > px(0.) {
            quads.push(Quad {
                bounds: Bounds::new(origin, size(width, gap)),
                color: self.palette.context_bg,
            });
        }
        self.paint_file_header(
            window,
            parts,
            point(origin.x, origin.y + gap),
            width,
            ctx.collapsed,
            ctx.hovered,
            quads,
            icons,
            glyphs,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_line(
        &self,
        window: &mut Window,
        cell: LineCell<'_>,
        origin: Point<Pixels>,
        width: Pixels,
        h_offset: Pixels,
        quads: &mut Vec<Quad>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let p = &self.palette;
        let lh = self.line_height;
        let bounds = Bounds::new(origin, size(width, lh));
        let content_x = origin.x + self.gutter_w + px(GUTTER_BORDER_W);
        let content_w = (bounds.right() - content_x).max(px(0.));
        quads.push(Quad {
            bounds,
            color: p.context_bg,
        });

        let mut number_color = p.number;
        let mut word_color = None;
        if let Some(added) = cell.added {
            let (bg, bar, word) = self.change_colors(added, cell.tone);
            if bg != p.context_bg {
                let gutter_bg = if bg == p.add_bg {
                    p.add_gutter_bg
                } else if bg == p.del_bg {
                    p.del_gutter_bg
                } else {
                    bg
                };
                quads.push(Quad {
                    bounds: Bounds::new(origin, size(self.gutter_w, lh)),
                    color: gutter_bg,
                });
                quads.push(Quad {
                    bounds: Bounds::new(point(content_x, origin.y), size(content_w, lh)),
                    color: bg,
                });
            }
            let dash_gap = (!added).then_some(if bar == p.del_bar {
                p.del_bar_gap
            } else {
                gpui::transparent_black()
            });
            Self::push_hunk_bar(origin, lh, bar, dash_gap, quads);
            number_color = if added { p.gutter_add } else { p.gutter_del };
            word_color = word;
        }

        self.gutter(window, cell.number, origin, number_color, glyphs);
        if cell.text.is_empty() {
            return;
        }
        let code_x = content_x + self.ch * LINE_PAD_CH - h_offset;
        let line = self.shape(window, cell.text, cell.syntax_runs, p.text);
        if let Some(word) = word_color {
            self.push_word_rects(
                &line,
                cell.change_runs,
                code_x,
                content_x,
                bounds.right(),
                origin.y,
                word,
                quads,
            );
        }
        glyphs.push(Glyphs {
            origin: point(code_x, origin.y),
            line: line.into_painted(),
            clip: Some(Bounds::new(point(content_x, origin.y), size(content_w, lh))),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_unified(
        &self,
        window: &mut Window,
        row: &DisplayRow,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        ctx: RowContext,
        h_offset: Pixels,
        quads: &mut Vec<Quad>,
        rounded_quads: &mut Vec<RoundedQuad>,
        icons: &mut Vec<MaskIcon>,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let row_bounds = Bounds::new(origin, size(width, row_h));

        match row.kind {
            RowKind::FileHeader => match &row.header {
                Some(parts) => self.layout_header_row(
                    window, parts, origin, width, row_h, ctx, quads, icons, glyphs,
                ),
                None => self.layout_note(window, &row.text, row_bounds, quads, glyphs),
            },
            RowKind::Binary | RowKind::Truncated => {
                self.layout_note(window, &row.text, row_bounds, quads, glyphs);
            }
            RowKind::Fold => {
                quads.push(Quad {
                    bounds: row_bounds,
                    color: self.palette.context_bg,
                });
                self.layout_separator(
                    window,
                    Some(&row.text),
                    Some(ctx.fold_edge.icon()),
                    origin.x + px(SEPARATOR_INSET_X),
                    row_bounds.right() - px(SEPARATOR_INSET_X),
                    origin.y,
                    row_h,
                    true,
                    rounded_quads,
                    icons,
                    glyphs,
                );
            }
            RowKind::Context | RowKind::Added | RowKind::Removed => {
                let added = match row.kind {
                    RowKind::Added => Some(true),
                    RowKind::Removed => Some(false),
                    _ => None,
                };
                self.layout_line(
                    window,
                    LineCell {
                        added,
                        tone: row.tone,
                        number: row.new_no.or(row.old_no),
                        text: &row.text,
                        syntax_runs: &row.syntax_runs,
                        change_runs: &row.change_runs,
                    },
                    origin,
                    width,
                    h_offset,
                    quads,
                    glyphs,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_split(
        &self,
        window: &mut Window,
        row: &SplitRow,
        origin: Point<Pixels>,
        width: Pixels,
        row_h: Pixels,
        ctx: RowContext,
        h_offset_left: Pixels,
        h_offset_right: Pixels,
        quads: &mut Vec<Quad>,
        rounded_quads: &mut Vec<RoundedQuad>,
        icons: &mut Vec<MaskIcon>,
        glyphs: &mut Vec<Glyphs>,
        hatches: &mut Vec<HatchLine>,
    ) {
        let p = &self.palette;
        let lh = self.line_height;
        let row_bounds = Bounds::new(origin, size(width, row_h));
        let divider_w = px(SPLIT_DIVIDER_W);
        let half_w = ((width - divider_w) / 2.0).max(px(0.));
        let right_x = origin.x + half_w + divider_w;

        match row {
            SplitRow::Header(parts) => {
                self.layout_header_row(
                    window, parts, origin, width, row_h, ctx, quads, icons, glyphs,
                );
            }
            SplitRow::Note(text) => self.layout_note(window, text, row_bounds, quads, glyphs),
            SplitRow::Fold(fold) => {
                quads.push(Quad {
                    bounds: row_bounds,
                    color: p.context_bg,
                });
                self.layout_separator(
                    window,
                    Some(&fold.text),
                    Some(ctx.fold_edge.icon()),
                    origin.x + px(SEPARATOR_INSET_X),
                    origin.x + half_w,
                    origin.y,
                    row_h,
                    false,
                    rounded_quads,
                    icons,
                    glyphs,
                );
                self.layout_separator(
                    window,
                    None,
                    None,
                    right_x,
                    row_bounds.right() - px(SEPARATOR_INSET_X),
                    origin.y,
                    row_h,
                    true,
                    rounded_quads,
                    icons,
                    glyphs,
                );
            }
            SplitRow::Pair { left, right } => {
                self.layout_half(
                    window,
                    left,
                    origin,
                    half_w,
                    h_offset_left,
                    quads,
                    glyphs,
                    hatches,
                );
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
        let added = match cell.kind {
            CellKind::Added => Some(true),
            CellKind::Removed => Some(false),
            CellKind::Context => None,
            CellKind::Phantom => {
                let lh = self.line_height;
                quads.push(Quad {
                    bounds: Bounds::new(origin, size(width, lh)),
                    color: self.palette.phantom_bg,
                });
                let hatch_x = origin.x + self.gutter_w + px(GUTTER_BORDER_W);
                let hatch_w = (origin.x + width - hatch_x).max(px(0.));
                self.push_phantom_hatches(
                    Bounds::new(point(hatch_x, origin.y), size(hatch_w, lh)),
                    hatches,
                );
                return;
            }
        };
        self.layout_line(
            window,
            LineCell {
                added,
                tone: cell.tone,
                number: cell.no,
                text: &cell.text,
                syntax_runs: &cell.syntax_runs,
                change_runs: &cell.change_runs,
            },
            origin,
            width,
            h_offset,
            quads,
            glyphs,
        );
    }

    fn gutter(
        &self,
        window: &mut Window,
        n: Option<u32>,
        origin: Point<Pixels>,
        color: Hsla,
        glyphs: &mut Vec<Glyphs>,
    ) {
        let Some(n) = n else { return };
        let line = shape_plain(
            window,
            &self.font,
            self.font_size,
            n.to_string().into(),
            color,
        );
        let right = origin.x + self.gutter_w - self.ch * GUTTER_PAD_R_CH;
        glyphs.push(Glyphs {
            origin: point((right - line.width()).max(origin.x), origin.y),
            line: line.into(),
            clip: None,
        });
    }
}

impl Element for DiffElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<DiffPrepaint>;

    fn id(&self) -> Option<ElementId> {
        self.file_toggle
            .as_ref()
            .map(|_| ElementId::Name("diff-files".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.file_toggle.as_ref().map(|_| accesskit::Role::Group)
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        node.set_label(DIFF_FILES_LABEL);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut gpui::A11ySubtreeBuilder,
    ) {
        let Some(layout) = prepaint.as_mut() else {
            return;
        };
        for header in &layout.headers {
            let id = builder.synthetic_node_id(header.path.as_ref());
            if builder.push_child(id, file_header_node(header, layout.scale)) {
                layout.header_nodes.push((id, header.path.clone()));
            }
        }
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
        let h = px(super::rows::content_height(&self.body.offsets_rc()));
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
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let row_count = self.body.len();

        let offsets = self.body.offsets_rc();
        let mask = window.content_mask();
        let vtop = f32::from(mask.bounds.origin.y - bounds.origin.y).max(0.0);
        let vbot = vtop + f32::from(mask.bounds.size.height);
        let first = offsets
            .partition_point(|&o| o <= vtop)
            .saturating_sub(1)
            .min(row_count);
        let last = offsets.partition_point(|&o| o < vbot).min(row_count);
        if last.saturating_sub(first) > 200 {
            log::warn!(
                "diff element: visible {first}..{last} = {} of {row_count} rows - culling off?",
                last - first
            );
        }

        let digits = {
            let mut n = self.body.max_line_no().max(1);
            let mut count = 0usize;
            while n > 0 {
                count += 1;
                n /= 10;
            }
            count.max(GUTTER_MIN_DIGITS)
        };
        let digit_w = f32::from(
            shape_plain(
                window,
                &self.font,
                self.font_size,
                "0".into(),
                self.palette.muted,
            )
            .width(),
        );
        self.ch = px(digit_w);
        self.gutter_w = px(digit_w * (GUTTER_PAD_L_CH + digits as f32 + GUTTER_PAD_R_CH));

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
        let mut chips = Vec::new();
        let mut chip_glyphs = Vec::new();
        let mut scrollbars = Vec::new();
        let mut sticky_quads = Vec::with_capacity(2);
        let mut sticky_images = Vec::with_capacity(1);
        let mut sticky_glyphs = Vec::with_capacity(3);

        let sticky = sticky_header(
            &spans,
            &offsets,
            first,
            vtop,
            bounds.origin.y,
            mask.bounds.origin.y,
        );

        match &self.body {
            DiffBody::Unified { rows, .. } => {
                let rows = rows.clone();
                let is_header =
                    |i: usize| rows.get(i).is_none_or(|r| r.kind == RowKind::FileHeader);
                for i in first..last {
                    let origin = point(bounds.origin.x, bounds.origin.y + px(offsets[i]));
                    let row_h = px(offsets[i + 1] - offsets[i]);
                    let ctx = RowContext {
                        collapsed: is_header(i + 1),
                        hovered: self.hovered_header == Some(i),
                        fold_edge: FoldEdge::between(i > 0 && is_header(i - 1), is_header(i + 1)),
                    };
                    let h_offset = px(file_at_row(&spans, i)
                        .map(|f| {
                            file_side_offset(&spans, &h_offsets, f, false, false, f32::from(width))
                        })
                        .unwrap_or(0.0));
                    self.layout_unified(
                        window,
                        &rows[i],
                        origin,
                        width,
                        row_h,
                        ctx,
                        h_offset,
                        &mut quads,
                        &mut rounded_quads,
                        &mut images,
                        &mut glyphs,
                    );
                }
                if let Some((header_row, sticky_y)) = sticky
                    && let Some(parts) = rows[header_row].header.as_ref()
                {
                    self.paint_file_header(
                        window,
                        parts,
                        point(bounds.origin.x, sticky_y),
                        width,
                        is_header(header_row + 1),
                        false,
                        &mut sticky_quads,
                        &mut sticky_images,
                        &mut sticky_glyphs,
                    );
                }
            }
            DiffBody::Split { rows, .. } => {
                let rows = rows.clone();
                let is_header =
                    |i: usize| rows.get(i).is_none_or(|r| matches!(r, SplitRow::Header(_)));
                for i in first..last {
                    let origin = point(bounds.origin.x, bounds.origin.y + px(offsets[i]));
                    let row_h = px(offsets[i + 1] - offsets[i]);
                    let ctx = RowContext {
                        collapsed: is_header(i + 1),
                        hovered: self.hovered_header == Some(i),
                        fold_edge: FoldEdge::between(i > 0 && is_header(i - 1), is_header(i + 1)),
                    };
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
                        &rows[i],
                        origin,
                        width,
                        row_h,
                        ctx,
                        h_offset_left,
                        h_offset_right,
                        &mut quads,
                        &mut rounded_quads,
                        &mut images,
                        &mut glyphs,
                        &mut hatches,
                    );
                }
                if let Some((header_row, sticky_y)) = sticky
                    && let SplitRow::Header(parts) = &rows[header_row]
                {
                    self.paint_file_header(
                        window,
                        parts,
                        point(bounds.origin.x, sticky_y),
                        width,
                        is_header(header_row + 1),
                        false,
                        &mut sticky_quads,
                        &mut sticky_images,
                        &mut sticky_glyphs,
                    );
                }
            }
        }

        if let Some(row) = self.revert_chip
            && row >= first
            && row < last
            && let (Some(top), Some(bottom)) = (offsets.get(row), offsets.get(row + 1))
        {
            self.push_revert_chip(
                window,
                point(bounds.origin.x, bounds.origin.y + px(*top)),
                width,
                px(bottom - top),
                &mut chips,
                &mut chip_glyphs,
            );
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

        let headers = if self.file_toggle.is_some() {
            visible_file_headers(
                &self.body,
                first,
                last,
                sticky,
                point(bounds.origin.x, bounds.origin.y),
                width,
            )
        } else {
            Vec::new()
        };

        Some(DiffPrepaint {
            quads,
            rounded_quads,
            images,
            glyphs,
            hatches,
            chips,
            chip_glyphs,
            scrollbars,
            sticky_quads,
            sticky_images,
            sticky_glyphs,
            headers,
            scale: window.scale_factor(),
            header_nodes: Vec::new(),
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
        if let Some(toggle) = self.file_toggle.as_ref() {
            for (id, path) in &layout.header_nodes {
                let toggle = toggle.clone();
                let path = path.clone();
                window.on_a11y_action(*id, accesskit::Action::Click, move |_, window, cx| {
                    toggle(&path, window, cx)
                });
            }
        }
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
            paint_icons(&layout.images, window, cx);
            for hatch in layout.hatches {
                window.paint_path(hatch.path, hatch.color);
            }
            for g in layout.glyphs {
                if let Some(clip) = g.clip {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let _ = g.line.paint(g.origin, lh, window, cx);
                    });
                } else {
                    let _ = g.line.paint(g.origin, lh, window, cx);
                }
            }
            for q in &layout.chips {
                window.paint_quad(quad(
                    q.bounds,
                    q.corners,
                    q.color,
                    px(0.),
                    q.color,
                    BorderStyle::Solid,
                ));
            }
            for g in layout.chip_glyphs {
                let _ = g.line.paint(g.origin, lh, window, cx);
            }
            for q in &layout.sticky_quads {
                window.paint_quad(fill(q.bounds, q.color));
            }
            paint_icons(&layout.sticky_images, window, cx);
            for g in layout.sticky_glyphs {
                if let Some(clip) = g.clip {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let _ = g.line.paint(g.origin, lh, window, cx);
                    });
                } else {
                    let _ = g.line.paint(g.origin, lh, window, cx);
                }
            }
            if !layout.scrollbars.is_empty() {
                window.paint_layer(|window| {
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

pub(crate) fn fill_text_runs(
    font: &Font,
    out: &mut Vec<TextRun>,
    len: usize,
    syntax: &[(Range<usize>, Hsla)],
    default: Hsla,
) {
    out.clear();
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    if syntax.is_empty() {
        out.push(run(len, default));
        return;
    }
    let mut ix = 0usize;
    for (r, color) in syntax {
        let start = r.start.min(len);
        let end = r.end.min(len);
        if start < ix || start >= end {
            continue;
        }
        if start > ix {
            out.push(run(start - ix, default));
        }
        out.push(run(end - start, *color));
        ix = end;
    }
    if ix < len {
        out.push(run(len - ix, default));
    }
}

pub(crate) fn text_runs(
    text: &str,
    syntax: &[(Range<usize>, Hsla)],
    font: &Font,
    default: Hsla,
) -> Vec<TextRun> {
    let mut runs = Vec::new();
    fill_text_runs(font, &mut runs, text.len(), syntax, default);
    runs
}

pub(crate) fn shape_plain(
    window: &mut Window,
    font: &Font,
    font_size: Pixels,
    text: SharedString,
    color: Hsla,
) -> ShapedLine {
    let runs = [TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    window
        .text_system()
        .shape_line(text, font_size, &runs, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(
        runs: &[Range<usize>],
        code_x: f32,
        clip_left: f32,
        clip_right: f32,
    ) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        for_each_word_rect(
            runs,
            |index| index as f32 * 7.0,
            code_x,
            clip_left,
            clip_right,
            |x0, x1| out.push((x0, x1)),
        );
        out
    }

    #[test]
    fn highlight_changes_apply_at_paint_time_without_rebuilding_rows() {
        let palette = super::super::rows::palette(crate::theme::ui_colors());
        let body = DiffBody::Unified {
            rows: Rc::new(Vec::new()),
            offsets: Rc::new(vec![0.0]),
            max_line_no: 0,
            spans: Rc::new(Vec::new()),
            h_offsets: Rc::new(Vec::new()),
        };
        let mut element = DiffElement::new(body, palette);
        for added in [true, false] {
            let (bg, bar, word) = if added {
                (palette.add_bg, palette.add_bar, palette.word_add)
            } else {
                (palette.del_bg, palette.del_bar, palette.word_del)
            };
            element = element.with_highlight(HighlightPolicy::Lines);
            assert_eq!(
                element.change_colors(added, ChangeTone::Full),
                (bg, bar, None)
            );
            assert_eq!(
                element.change_colors(added, ChangeTone::Muted),
                (bg, bar, None)
            );
            element = element.with_highlight(HighlightPolicy::None);
            assert_eq!(
                element.change_colors(added, ChangeTone::Full),
                (palette.context_bg, bar, None),
            );
            element = element.with_highlight(HighlightPolicy::Words);
            assert_eq!(
                element.change_colors(added, ChangeTone::Full),
                (bg, bar, Some(word))
            );
            assert!(element.change_colors(added, ChangeTone::Muted).2.is_none());
        }
    }

    #[test]
    fn word_rects_follow_shaped_positions() {
        assert_eq!(
            rects(&[2..5, 8..10], 100.0, 100.0, 1000.0),
            vec![(114.0, 135.0), (156.0, 170.0)]
        );
    }

    #[test]
    fn word_rects_are_clipped_to_the_cell_when_scrolled() {
        assert_eq!(
            rects(std::slice::from_ref(&(2..5)), 90.0, 100.0, 1000.0),
            vec![(104.0, 125.0)]
        );
        assert_eq!(
            rects(std::slice::from_ref(&(2..5)), 100.0, 100.0, 120.0),
            vec![(114.0, 120.0)]
        );
        assert!(rects(std::slice::from_ref(&(2..5)), 0.0, 100.0, 1000.0).is_empty());
        assert!(rects(std::slice::from_ref(&(20..30)), 100.0, 100.0, 200.0).is_empty());
    }

    #[test]
    fn the_revert_chip_sits_at_the_right_edge_of_the_row() {
        let bounds = revert_chip_bounds(point(px(100.), px(200.)), px(400.), px(ROW_HEIGHT));
        assert_eq!(
            bounds.origin.x,
            px(100. + 400. - REVERT_CHIP_PAD_R - REVERT_CHIP_W)
        );
        assert_eq!(
            bounds.origin.y,
            px(200. + (ROW_HEIGHT - REVERT_CHIP_H) / 2.0)
        );
        assert_eq!(bounds.size.width, px(REVERT_CHIP_W));
        assert_eq!(bounds.size.height, px(REVERT_CHIP_H));
        assert!(bounds.contains(&point(px(440.), px(210.))));
        assert!(!bounds.contains(&point(px(300.), px(210.))));
    }

    #[test]
    fn word_rects_span_the_displayed_width_of_a_tab() {
        let tab_width = |index: usize| {
            if index == 0 {
                0.0
            } else {
                32.0 + (index as f32 - 1.0) * 7.0
            }
        };
        let mut out = Vec::new();
        for_each_word_rect(
            std::slice::from_ref(&(0..1)),
            tab_width,
            100.0,
            100.0,
            1000.0,
            |x0, x1| out.push((x0, x1)),
        );
        assert_eq!(out, vec![(100.0, 132.0)]);
    }

    #[gpui::test]
    fn a_word_diff_range_over_a_tab_covers_its_expanded_width(cx: &mut gpui::TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let text = SharedString::from("x\ty");
            let runs = [TextRun {
                len: text.len(),
                font: gpui::font("Lilex"),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }];
            let line = TabExpandedLine::shape(&text, px(13.), &runs, window.text_system());
            assert_eq!(line.shaped().text.as_ref(), "x   y");
            let mut out = Vec::new();
            for_each_word_rect(
                std::slice::from_ref(&(1..2)),
                |index| f32::from(line.x_for_index(index)),
                0.0,
                0.0,
                1000.0,
                |x0, x1| out.push((x0, x1)),
            );
            let start = f32::from(line.shaped().x_for_index(1));
            let end = f32::from(line.shaped().x_for_index(4));
            assert_eq!(out, vec![(start, end)]);
            assert!(
                end - start > 2.0 * start,
                "the tab spans three columns after x"
            );
        });
    }

    fn display_row(kind: RowKind, header: Option<(&str, FileChange)>) -> DisplayRow {
        DisplayRow {
            kind,
            text: "".into(),
            old_no: None,
            new_no: None,
            syntax_runs: Vec::new(),
            change_runs: Vec::new(),
            tone: ChangeTone::Full,
            header: header.map(|(path, change)| HeaderParts {
                path: path.to_string().into(),
                old_path: None,
                change,
                added: 1,
                removed: 0,
            }),
            fold_key: None,
            fold_base_start: None,
            fold_new_start: None,
            fold_count: 0,
            folded_rows: Vec::new(),
        }
    }

    #[test]
    fn each_visible_file_header_is_a_named_button_that_reports_whether_its_file_is_open() {
        let rows = vec![
            display_row(
                RowKind::FileHeader,
                Some(("README.md", FileChange::Modified)),
            ),
            display_row(RowKind::Added, None),
            display_row(RowKind::FileHeader, Some(("src/lib.rs", FileChange::Added))),
        ];
        let offsets = vec![
            0.0,
            FILE_HEADER_HEIGHT,
            FILE_HEADER_HEIGHT + ROW_HEIGHT,
            2.0 * FILE_HEADER_HEIGHT + ROW_HEIGHT,
        ];
        let body = DiffBody::Unified {
            rows: Rc::new(rows),
            offsets: Rc::new(offsets),
            max_line_no: 1,
            spans: Rc::new(Vec::new()),
            h_offsets: Rc::new(Vec::new()),
        };
        let origin = point(px(10.), px(100.));
        let headers = visible_file_headers(&body, 0, 3, None, origin, px(400.));
        let seen: Vec<_> = headers
            .iter()
            .map(|header| {
                (
                    header.path.to_string(),
                    header.collapsed,
                    header.bounds.origin.y,
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("README.md".to_string(), false, px(100.)),
                (
                    "src/lib.rs".to_string(),
                    true,
                    px(100. + FILE_HEADER_HEIGHT + ROW_HEIGHT)
                ),
            ]
        );
        let readme = file_header_node(&headers[0], 2.0);
        assert_eq!(readme.role(), accesskit::Role::Button);
        assert_eq!(readme.label(), Some("README.md, modified"));
        assert_eq!(readme.is_expanded(), Some(true));
        assert!(readme.supports_action(accesskit::Action::Click));
        assert_eq!(
            readme.bounds().map(|rect| (rect.x0, rect.y0)),
            Some((20.0, 200.0))
        );
        let lib = file_header_node(&headers[1], 1.0);
        assert_eq!(lib.label(), Some("src/lib.rs, added"));
        assert_eq!(lib.is_expanded(), Some(false));
        let scrolled = visible_file_headers(&body, 1, 2, Some((0, px(100.))), origin, px(400.));
        assert_eq!(
            scrolled
                .iter()
                .map(|header| header.path.to_string())
                .collect::<Vec<_>>(),
            ["README.md"],
            "the sticky header of a file scrolled past stays pressable"
        );
        let palette = super::super::rows::palette(crate::theme::ui_colors());
        let plain = DiffElement::new(body, palette);
        assert_eq!(plain.a11y_role(), None);
        let toggled = plain.with_file_toggle(Rc::new(|_, _, _| {}));
        assert_eq!(toggled.a11y_role(), Some(accesskit::Role::Group));
        assert!(toggled.id().is_some());
    }
}

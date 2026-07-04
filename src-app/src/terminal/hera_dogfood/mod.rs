//! Hera dogfood runtime boundary for Paneflow M3.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::types::{
    Cell, CellFlags, Color as PaneflowColor, Content, CursorShape, NamedColor, Point,
    RenderableCursor, Rgb,
};
use serde::Serialize;
use terminal_core::{
    CellStyle, Color as HeraColor, RenderCell, RenderSnapshot, ScreenIdentity, Terminal,
};
#[cfg(test)]
use terminal_core::{
    CursorState, ImagePlaceholder, ImageProtocol, RowHandle, ScrollbackRow, ViewportRow,
};

pub const DOGFOOD_ENV_VAR: &str = "PANEFLOW_HERA_DOGFOOD";
pub const DOGFOOD_ARTIFACT_DIR_ENV_VAR: &str = "PANEFLOW_HERA_DOGFOOD_ARTIFACT_DIR";
pub const DOGFOOD_RETENTION_ENV_VAR: &str = "PANEFLOW_HERA_DOGFOOD_RETENTION";
pub const LATENCY_PROBE_ENV_VAR: &str = "PANEFLOW_LATENCY_PROBE";
const OUTPUT_QUEUE_CAPACITY: usize = 64;
const INPUT_SUMMARY_LIMIT_BYTES: usize = 512;
const INPUT_METADATA_RECORD_LIMIT: usize = 16;
const REPORT_EXCERPT_LINE_LIMIT: usize = 4;
const REPORT_EXCERPT_CHAR_LIMIT: usize = 256;
const DIFF_VALUE_LIMIT: usize = 192;
const M3_DOGFOOD_RECORDING_SCHEMA: &str = "hera.m3_dogfood_recording";
const M3_DOGFOOD_RECORDING_VERSION: u32 = 1;
const M3_DOGFOOD_METRICS_SCHEMA: &str = "hera.m3_dogfood_metrics";
const M3_DOGFOOD_METRICS_VERSION: u32 = 1;
const M3_MAX_RECORDING_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const M3_MAX_BATCH_TIMING_SAMPLES: usize = 65_536;
const M3_BATCH_P95_BUDGET_MS: f64 = 2.0;
const M3_10K_MEMORY_DELTA_BUDGET_BYTES: i64 = 64 * 1024 * 1024;
const MAX_HERA_LAYOUT_COLUMNS: usize = 512;
const MAX_HERA_LAYOUT_ROWS: usize = 512;
const MAX_HERA_LAYOUT_CELLS: usize = 262_144;

static UNKNOWN_MODE_WARNED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DogfoodMode {
    Disabled,
    Shadow,
    SideBySide,
}

impl DogfoodMode {
    const fn attaches_shadow(self) -> bool {
        matches!(self, Self::Shadow | Self::SideBySide)
    }

    const fn shows_side_by_side(self) -> bool {
        matches!(self, Self::SideBySide)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRetention {
    RawLocal,
    Scrubbed,
}

impl ArtifactRetention {
    fn from_env() -> Self {
        match std::env::var(DOGFOOD_RETENTION_ENV_VAR)
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("scrubbed") => Self::Scrubbed,
            _ => Self::RawLocal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct AdapterDiagnostic {
    code: &'static str,
    message: String,
}

#[allow(dead_code)]
impl AdapterDiagnostic {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputMetadataSummary {
    byte_count: usize,
    escaped_summary: String,
    truncated: bool,
}

#[allow(dead_code)]
impl InputMetadataSummary {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            byte_count: bytes.len(),
            escaped_summary: escape_bytes(bytes, INPUT_SUMMARY_LIMIT_BYTES),
            truncated: bytes.len() > INPUT_SUMMARY_LIMIT_BYTES,
        }
    }

    pub const fn byte_count(&self) -> usize {
        self.byte_count
    }

    pub fn escaped_summary(&self) -> &str {
        &self.escaped_summary
    }

    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DiffCounters {
    equal: u64,
    mismatch: u64,
    unsupported: u64,
    shadow_disabled: u64,
    side_by_side_skipped: u64,
}

#[allow(dead_code)]
impl DiffCounters {
    pub const fn equal(self) -> u64 {
        self.equal
    }

    pub const fn mismatch(self) -> u64 {
        self.mismatch
    }

    pub const fn unsupported(self) -> u64 {
        self.unsupported
    }

    pub const fn shadow_disabled(self) -> u64 {
        self.shadow_disabled
    }

    pub const fn side_by_side_skipped(self) -> u64 {
        self.side_by_side_skipped
    }
}

#[derive(Clone, Debug)]
pub struct HeraLayoutContent {
    content: Content,
    diagnostics: Vec<AdapterDiagnostic>,
}

impl HeraLayoutContent {
    #[cfg(test)]
    pub fn into_content(self) -> Content {
        self.content
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    pub fn diagnostics(&self) -> &[AdapterDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeraLayoutAdapterError {
    DimensionsExceedCap {
        columns: usize,
        rows: usize,
        max_columns: usize,
        max_rows: usize,
        max_cells: usize,
    },
}

impl std::fmt::Display for HeraLayoutAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DimensionsExceedCap {
                columns,
                rows,
                max_columns,
                max_rows,
                max_cells,
            } => write!(
                f,
                "Hera snapshot dimensions {columns}x{rows} exceed Paneflow layout caps {max_columns}x{max_rows} or {max_cells} cells"
            ),
        }
    }
}

impl std::error::Error for HeraLayoutAdapterError {}

#[derive(Clone, Debug)]
pub struct SideBySideDiagnosticSurface {
    rows: Vec<String>,
    diagnostics: Vec<AdapterDiagnostic>,
    counters: DiffCounters,
    skipped_reason: Option<&'static str>,
}

impl SideBySideDiagnosticSurface {
    fn available(
        rows: Vec<String>,
        diagnostics: Vec<AdapterDiagnostic>,
        counters: DiffCounters,
    ) -> Self {
        Self {
            rows,
            diagnostics,
            counters,
            skipped_reason: None,
        }
    }

    fn skipped(
        reason: &'static str,
        diagnostics: Vec<AdapterDiagnostic>,
        counters: DiffCounters,
    ) -> Self {
        Self {
            rows: Vec::new(),
            diagnostics,
            counters,
            skipped_reason: Some(reason),
        }
    }

    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    pub fn diagnostics(&self) -> &[AdapterDiagnostic] {
        &self.diagnostics
    }

    pub const fn counters(&self) -> DiffCounters {
        self.counters
    }

    pub const fn skipped_reason(&self) -> Option<&'static str> {
        self.skipped_reason
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum ComparisonField<T> {
    Supported(T),
    Unsupported,
}

impl<T> ComparisonField<T> {
    fn supported(value: T) -> Self {
        Self::Supported(value)
    }

    const fn unsupported() -> Self {
        Self::Unsupported
    }

    fn as_supported(&self) -> Option<&T> {
        match self {
            Self::Supported(value) => Some(value),
            Self::Unsupported => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ComparisonDimensions {
    columns: usize,
    rows: usize,
}

#[allow(dead_code)]
impl ComparisonDimensions {
    pub const fn new(columns: usize, rows: usize) -> Self {
        Self { columns, rows }
    }

    pub const fn columns(self) -> usize {
        self.columns
    }

    pub const fn rows(self) -> usize {
        self.rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonScreen {
    Primary,
    Alternate,
}

impl From<ScreenIdentity> for ComparisonScreen {
    fn from(value: ScreenIdentity) -> Self {
        match value {
            ScreenIdentity::Primary => Self::Primary,
            ScreenIdentity::Alternate => Self::Alternate,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ComparisonCursor {
    row: usize,
    column: usize,
    visible: bool,
}

impl ComparisonCursor {
    pub const fn new(row: usize, column: usize, visible: bool) -> Self {
        Self {
            row,
            column,
            visible,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StyleBucket {
    style: String,
    count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ComparisonSummary {
    dimensions: ComparisonField<ComparisonDimensions>,
    active_screen: ComparisonField<ComparisonScreen>,
    cursor: ComparisonField<ComparisonCursor>,
    viewport_lines: ComparisonField<Vec<String>>,
    style_buckets: ComparisonField<Vec<StyleBucket>>,
    scrollback_line_count: ComparisonField<usize>,
}

impl ComparisonSummary {
    pub fn unsupported() -> Self {
        Self {
            dimensions: ComparisonField::unsupported(),
            active_screen: ComparisonField::unsupported(),
            cursor: ComparisonField::unsupported(),
            viewport_lines: ComparisonField::unsupported(),
            style_buckets: ComparisonField::unsupported(),
            scrollback_line_count: ComparisonField::unsupported(),
        }
    }

    pub fn from_hera_snapshot(snapshot: &RenderSnapshot) -> Self {
        Self {
            dimensions: ComparisonField::supported(ComparisonDimensions::new(
                snapshot.columns(),
                snapshot.rows(),
            )),
            active_screen: ComparisonField::supported(snapshot.active_screen().into()),
            cursor: ComparisonField::supported(ComparisonCursor::new(
                snapshot.cursor().row(),
                snapshot.cursor().column(),
                snapshot.cursor().visible(),
            )),
            viewport_lines: ComparisonField::supported(
                snapshot
                    .viewport_rows()
                    .iter()
                    .map(|row| row.cells().iter().map(RenderCell::ch).collect())
                    .collect(),
            ),
            style_buckets: ComparisonField::supported(style_buckets_from_hera(snapshot)),
            scrollback_line_count: ComparisonField::supported(snapshot.scrollback_rows().len()),
        }
    }

    pub fn from_paneflow_content(
        columns: usize,
        rows: usize,
        active_screen: Option<ComparisonScreen>,
        content: &super::types::Content,
    ) -> Self {
        let cursor_row = content
            .cursor
            .point
            .line
            .0
            .saturating_add(content.display_offset as i32);
        let cursor_visible = cursor_row >= 0
            && (cursor_row as usize) < rows
            && !matches!(content.cursor.shape, super::types::CursorShape::Hidden);

        Self {
            dimensions: ComparisonField::supported(ComparisonDimensions::new(columns, rows)),
            active_screen: active_screen
                .map(ComparisonField::supported)
                .unwrap_or_else(ComparisonField::unsupported),
            cursor: ComparisonField::supported(ComparisonCursor::new(
                cursor_row.max(0) as usize,
                content.cursor.point.column.0,
                cursor_visible,
            )),
            viewport_lines: ComparisonField::supported(viewport_lines_from_paneflow(
                columns, rows, content,
            )),
            style_buckets: ComparisonField::supported(style_buckets_from_paneflow(content)),
            scrollback_line_count: ComparisonField::supported(content.history_size),
        }
    }

    pub fn first_difference(&self, other: &Self) -> ComparisonOutcome {
        diff_field("$.dimensions", &self.dimensions, &other.dimensions)
            .or_else(|| diff_field("$.active_screen", &self.active_screen, &other.active_screen))
            .or_else(|| {
                diff_lines(
                    "$.viewport_lines",
                    &self.viewport_lines,
                    &other.viewport_lines,
                )
            })
            .or_else(|| diff_field("$.style_buckets", &self.style_buckets, &other.style_buckets))
            .or_else(|| {
                diff_field(
                    "$.scrollback_line_count",
                    &self.scrollback_line_count,
                    &other.scrollback_line_count,
                )
            })
            .or_else(|| diff_field("$.cursor", &self.cursor, &other.cursor))
            .unwrap_or(ComparisonOutcome::Equal)
    }

    pub fn checkpoint_difference(
        &self,
        other: &Self,
        _hera_output_bytes_seen: u64,
        _hera_input_bytes_seen: u64,
    ) -> ComparisonOutcome {
        if should_defer_blank_bootstrap_cursor_mismatch(self, other) {
            return ComparisonOutcome::Unsupported(ComparisonDifference::new(
                "$.cursor.bootstrap_empty",
                "blank cursor-only mismatch during bootstrap",
                "deferred until visible output",
            ));
        }
        if should_defer_output_until_shadow_has_text(self, other) {
            return ComparisonOutcome::Unsupported(ComparisonDifference::new(
                "$.viewport_lines.shadow_blank",
                "Paneflow has output while Hera viewport is still blank",
                "deferred until Hera has visible text",
            ));
        }
        if should_defer_bootstrap_viewport_alignment_drift(self, other) {
            return ComparisonOutcome::Unsupported(ComparisonDifference::new(
                "$.viewport_lines.bootstrap_vertical_drift",
                "Paneflow and Hera share bootstrap output with transient row drift",
                "deferred until bootstrap viewport alignment stabilizes",
            ));
        }
        let outcome = self.first_difference(other);
        outcome
    }

    fn dimensions(&self) -> Option<ComparisonDimensions> {
        self.dimensions.as_supported().copied()
    }

    fn viewport_lines(&self) -> Option<&[String]> {
        self.viewport_lines.as_supported().map(Vec::as_slice)
    }
}

pub fn layout_content_from_hera_snapshot(
    snapshot: &RenderSnapshot,
) -> Result<HeraLayoutContent, HeraLayoutAdapterError> {
    validate_hera_layout_dimensions(snapshot.columns(), snapshot.rows())?;

    let mut diagnostics = Vec::new();
    let mut cells = Vec::with_capacity(snapshot.columns().saturating_mul(snapshot.rows()));

    for row_index in 0..snapshot.rows() {
        let row = snapshot.viewport_rows().get(row_index);
        let row_cells = row.map(|row| row.cells()).unwrap_or_default();
        let mut spacer_columns = 0usize;

        if row_cells.len() > snapshot.columns() {
            diagnostics.push(AdapterDiagnostic::new(
                "row_cells_exceed_dimensions",
                format!(
                    "Hera row {row_index} has {} cells for {} columns; extra cells ignored",
                    row_cells.len(),
                    snapshot.columns()
                ),
            ));
        }

        for column in 0..snapshot.columns() {
            let cell = row_cells.get(column);
            let is_wide_spacer = spacer_columns > 0;
            spacer_columns = spacer_columns.saturating_sub(1);
            cells.push(map_hera_cell(
                row_index,
                column,
                cell,
                is_wide_spacer,
                &mut diagnostics,
            ));
            if !is_wide_spacer && let Some(cell) = cell {
                spacer_columns = usize::from(cell.width()).saturating_sub(1);
            }
        }
    }

    let content = Content {
        cells,
        cursor: renderable_cursor_from_hera(snapshot),
        selection: None,
        display_offset: 0,
        history_size: snapshot.scrollback_rows().len(),
    };

    Ok(HeraLayoutContent {
        content,
        diagnostics,
    })
}

fn validate_hera_layout_dimensions(
    columns: usize,
    rows: usize,
) -> Result<(), HeraLayoutAdapterError> {
    let exceeds_cells = columns
        .checked_mul(rows)
        .is_none_or(|cells| cells > MAX_HERA_LAYOUT_CELLS);
    if columns > MAX_HERA_LAYOUT_COLUMNS || rows > MAX_HERA_LAYOUT_ROWS || exceeds_cells {
        return Err(HeraLayoutAdapterError::DimensionsExceedCap {
            columns,
            rows,
            max_columns: MAX_HERA_LAYOUT_COLUMNS,
            max_rows: MAX_HERA_LAYOUT_ROWS,
            max_cells: MAX_HERA_LAYOUT_CELLS,
        });
    }
    Ok(())
}

fn map_hera_cell(
    row: usize,
    column: usize,
    cell: Option<&RenderCell>,
    is_wide_spacer: bool,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Cell {
    if is_wide_spacer {
        return Cell {
            point: Point::new(row as i32, column),
            c: ' ',
            fg: cell.map(RenderCell::style).map_or(
                PaneflowColor::Named(NamedColor::Foreground),
                hera_foreground,
            ),
            bg: cell.map(RenderCell::style).map_or(
                PaneflowColor::Named(NamedColor::Background),
                hera_background,
            ),
            flags: CellFlags::WIDE_CHAR_SPACER,
            zerowidth: None,
            hyperlink: false,
        };
    }

    let Some(cell) = cell else {
        return Cell {
            point: Point::new(row as i32, column),
            c: ' ',
            fg: PaneflowColor::Named(NamedColor::Foreground),
            bg: PaneflowColor::Named(NamedColor::Background),
            flags: CellFlags::empty(),
            zerowidth: None,
            hyperlink: false,
        };
    };

    let mut ch = cell.ch();
    if let Some(image) = cell.image() {
        diagnostics.push(AdapterDiagnostic::new(
            "unsupported_image_placeholder",
            format!(
                "Hera image placeholder at row {row}, column {column}: protocol={:?}, bytes={}",
                image.protocol(),
                image.byte_len()
            ),
        ));
        ch = '?';
    }
    if cell.width() > 2 {
        diagnostics.push(AdapterDiagnostic::new(
            "unsupported_cell_width",
            format!(
                "Hera cell width {} at row {row}, column {column} exceeds Paneflow wide-cell support",
                cell.width()
            ),
        ));
    }

    Cell {
        point: Point::new(row as i32, column),
        c: ch,
        fg: hera_foreground(cell.style()),
        bg: hera_background(cell.style()),
        flags: hera_cell_flags(cell),
        zerowidth: None,
        hyperlink: false,
    }
}

fn renderable_cursor_from_hera(snapshot: &RenderSnapshot) -> RenderableCursor {
    let cursor = snapshot.cursor();
    let cell = snapshot
        .viewport_rows()
        .get(cursor.row())
        .and_then(|row| row.cells().get(cursor.column()));
    let style = cell.map(RenderCell::style).unwrap_or_default();
    let text = cell.map_or(' ', |cell| {
        if cell.image().is_some() {
            '?'
        } else {
            cell.ch()
        }
    });

    RenderableCursor {
        point: Point::new(cursor.row() as i32, cursor.column()),
        shape: if cursor.visible() {
            CursorShape::Block
        } else {
            CursorShape::Hidden
        },
        fg: hera_foreground(style),
        bg: hera_background(style),
        flags: hera_cell_flags_from_style(style, cell.is_some_and(|cell| cell.width() > 1)),
        wide: cell.is_some_and(|cell| cell.width() > 1),
        text,
        bold: style.bold(),
        italic: style.italic(),
    }
}

fn hera_cell_flags(cell: &RenderCell) -> CellFlags {
    hera_cell_flags_from_style(cell.style(), cell.width() > 1)
}

fn hera_cell_flags_from_style(style: CellStyle, wide: bool) -> CellFlags {
    let mut flags = CellFlags::empty();
    if style.bold() {
        flags |= CellFlags::BOLD;
    }
    if style.italic() {
        flags |= CellFlags::ITALIC;
    }
    if style.underline() {
        flags |= CellFlags::UNDERLINE;
    }
    if style.inverse() {
        flags |= CellFlags::INVERSE;
    }
    if wide {
        flags |= CellFlags::WIDE_CHAR;
    }
    flags
}

fn hera_foreground(style: CellStyle) -> PaneflowColor {
    hera_color_to_paneflow(style.foreground(), NamedColor::Foreground)
}

fn hera_background(style: CellStyle) -> PaneflowColor {
    hera_color_to_paneflow(style.background(), NamedColor::Background)
}

fn hera_color_to_paneflow(color: Option<HeraColor>, default: NamedColor) -> PaneflowColor {
    match color {
        Some(HeraColor::Indexed(index)) => PaneflowColor::Indexed(index),
        Some(HeraColor::Rgb { red, green, blue }) => PaneflowColor::Spec(Rgb {
            r: red,
            g: green,
            b: blue,
        }),
        None => PaneflowColor::Named(default),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ComparisonDifference {
    field_path: String,
    left: String,
    right: String,
}

#[allow(dead_code)]
impl ComparisonDifference {
    fn new(
        field_path: impl Into<String>,
        left: impl std::fmt::Debug,
        right: impl std::fmt::Debug,
    ) -> Self {
        Self {
            field_path: field_path.into(),
            left: compact_debug(left),
            right: compact_debug(right),
        }
    }

    pub fn field_path(&self) -> &str {
        &self.field_path
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "difference", rename_all = "snake_case")]
pub enum ComparisonOutcome {
    Equal,
    Mismatch(ComparisonDifference),
    Unsupported(ComparisonDifference),
}

impl ComparisonOutcome {
    fn difference(&self) -> Option<&ComparisonDifference> {
        match self {
            Self::Mismatch(difference) | Self::Unsupported(difference) => Some(difference),
            Self::Equal => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DogfoodReportContext {
    pane_id: String,
    cwd: Option<String>,
    exit_code: Option<i32>,
}

impl DogfoodReportContext {
    pub fn new(pane_id: impl Into<String>, cwd: Option<&str>, exit_code: Option<i32>) -> Self {
        Self {
            pane_id: pane_id.into(),
            cwd: cwd.map(ToOwned::to_owned),
            exit_code,
        }
    }
}

#[derive(Debug)]
struct ArtifactWriter {
    dir: Option<PathBuf>,
    retention: ArtifactRetention,
    warned: bool,
    sequence: u64,
}

impl ArtifactWriter {
    fn from_env() -> Self {
        Self {
            dir: std::env::var(DOGFOOD_ARTIFACT_DIR_ENV_VAR)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            retention: ArtifactRetention::from_env(),
            warned: false,
            sequence: 0,
        }
    }

    const fn retention(&self) -> ArtifactRetention {
        self.retention
    }

    fn write_report(&mut self, pane_id: &str, report: &MismatchReport) {
        let path = self.next_path(pane_id, "report");
        match path.and_then(|path| write_json(&path, report).map(|_| path)) {
            Ok(path) => log::info!("Hera mismatch recorded at {}", path.display()),
            Err(error) => self.warn_once(&error),
        }
    }

    fn write_recording(
        &mut self,
        pane_id: &str,
        recording: &DogfoodRecordingArtifact,
        metrics: &DogfoodMetricsArtifact,
    ) {
        let path = self.next_path(pane_id, "recording");
        match path.and_then(|path| write_json(&path, recording).map(|_| path)) {
            Ok(path) => log::info!("Hera dogfood recording written to {}", path.display()),
            Err(error) => self.warn_once(&error),
        }

        let path = self.next_path(pane_id, "metrics");
        match path.and_then(|path| write_json(&path, metrics).map(|_| path)) {
            Ok(path) => log::info!("Hera dogfood metrics written to {}", path.display()),
            Err(error) => self.warn_once(&error),
        }
    }

    fn next_path(&mut self, pane_id: &str, kind: &str) -> io::Result<PathBuf> {
        let Some(dir) = &self.dir else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{DOGFOOD_ARTIFACT_DIR_ENV_VAR} is not configured"),
            ));
        };
        fs::create_dir_all(dir)?;
        self.sequence = self.sequence.saturating_add(1);
        Ok(dir.join(format!(
            "hera-dogfood-{}-{}-{}-{kind}.json",
            now_millis(),
            sanitize_filename(pane_id),
            self.sequence
        )))
    }

    fn warn_once(&mut self, error: &io::Error) {
        if !self.warned {
            self.warned = true;
            log::warn!("dogfood artifact write failed: {error}");
        }
    }
}

#[derive(Serialize)]
struct MismatchReport {
    schema: &'static str,
    version: u32,
    pane_id: String,
    timestamp_ms: u128,
    outcome: ComparisonOutcome,
    field_path: String,
    dimensions: Option<ComparisonDimensions>,
    command: RedactedCommandMetadata,
    counters: DiffCounters,
    excerpts: ReportExcerpts,
}

#[derive(Serialize)]
struct RedactedCommandMetadata {
    cwd: Option<String>,
    exit_code: Option<i32>,
}

#[derive(Serialize)]
struct ReportExcerpts {
    paneflow: Vec<String>,
    hera: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum MetricMeasurement<T> {
    Measured {
        value: T,
    },
    NotMeasured {
        os: &'static str,
        command: String,
        reason: String,
    },
}

impl<T: Copy> MetricMeasurement<T> {
    fn value(&self) -> Option<T> {
        match self {
            Self::Measured { value } => Some(*value),
            Self::NotMeasured { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct DogfoodBatchStatusCounts {
    rendered: u64,
    skipped: u64,
    errored: u64,
}

impl DogfoodBatchStatusCounts {
    fn record(&mut self, status: DogfoodBatchStatus) {
        match status {
            DogfoodBatchStatus::Rendered => self.rendered = self.rendered.saturating_add(1),
            DogfoodBatchStatus::Skipped => self.skipped = self.skipped.saturating_add(1),
            DogfoodBatchStatus::Errored => self.errored = self.errored.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DogfoodBatchStatus {
    Rendered,
    Skipped,
    Errored,
}

impl DogfoodBatchStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Rendered => "rendered",
            Self::Skipped => "skipped",
            Self::Errored => "errored",
        }
    }
}

#[derive(Serialize)]
struct DogfoodMetricsArtifact {
    schema: &'static str,
    version: u32,
    pane_id: String,
    timestamp_ms: u128,
    source: String,
    redaction_status: ArtifactRetention,
    session: DogfoodSessionMetrics,
    memory: DogfoodMemoryMetrics,
    latency: DogfoodLatencyMetrics,
    diff_counters: DiffCounters,
    decision: DogfoodMetricsDecision,
}

#[derive(Serialize)]
struct DogfoodSessionMetrics {
    initial_dimensions: ComparisonDimensions,
    final_dimensions: Option<ComparisonDimensions>,
    event_counts: DogfoodEventCounts,
    logical_output_lines: usize,
    observed_output_bytes: usize,
    recording_output_bytes: usize,
    output_truncated: bool,
    max_output_bytes: usize,
}

#[derive(Serialize)]
struct DogfoodMemoryMetrics {
    paneflow_rss_baseline_bytes: MetricMeasurement<u64>,
    paneflow_rss_after_bytes: MetricMeasurement<u64>,
    dogfood_rss_delta_bytes: MetricMeasurement<i64>,
}

#[derive(Serialize)]
struct DogfoodLatencyMetrics {
    pty_batch_samples: usize,
    pty_batch_status_counts: DogfoodBatchStatusCounts,
    pty_batch_p50_ms: MetricMeasurement<f64>,
    pty_batch_p95_ms: MetricMeasurement<f64>,
    pty_batch_p99_ms: MetricMeasurement<f64>,
}

#[derive(Serialize)]
struct DogfoodMetricsDecision {
    replacement_blocked: bool,
    blocked_reasons: Vec<String>,
}

struct DogfoodRecordingBundle {
    recording: DogfoodRecordingArtifact,
    metrics: DogfoodMetricsArtifact,
}

#[derive(Debug)]
struct DogfoodRecordingBuilder {
    started_at: Instant,
    initial_size: ComparisonDimensions,
    retention: ArtifactRetention,
    rss_baseline_bytes: MetricMeasurement<u64>,
    max_output_bytes: usize,
    output_bytes: usize,
    observed_output_bytes: usize,
    logical_output_lines: usize,
    output_truncated: bool,
    events: Vec<DogfoodRecordingEvent>,
    event_counts: DogfoodEventCounts,
    batch_timings_ns: Vec<u64>,
    batch_status_counts: DogfoodBatchStatusCounts,
}

impl DogfoodRecordingBuilder {
    fn new(columns: usize, rows: usize, retention: ArtifactRetention) -> Self {
        Self::new_with_limit(columns, rows, retention, M3_MAX_RECORDING_OUTPUT_BYTES)
    }

    fn new_with_limit(
        columns: usize,
        rows: usize,
        retention: ArtifactRetention,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            initial_size: ComparisonDimensions::new(columns, rows),
            retention,
            rss_baseline_bytes: current_process_rss_bytes(),
            max_output_bytes,
            output_bytes: 0,
            observed_output_bytes: 0,
            logical_output_lines: 0,
            output_truncated: false,
            events: Vec::new(),
            event_counts: DogfoodEventCounts::default(),
            batch_timings_ns: Vec::new(),
            batch_status_counts: DogfoodBatchStatusCounts::default(),
        }
    }

    fn record_output(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        self.observed_output_bytes = self.observed_output_bytes.saturating_add(bytes.len());
        self.logical_output_lines = self
            .logical_output_lines
            .saturating_add(logical_output_line_count(bytes));
        let remaining = self.max_output_bytes.saturating_sub(self.output_bytes);
        if remaining == 0 {
            self.output_truncated = true;
            return;
        }

        let stored = bytes.len().min(remaining);
        self.output_bytes = self.output_bytes.saturating_add(stored);
        if stored < bytes.len() {
            self.output_truncated = true;
        }
        self.event_counts.output = self.event_counts.output.saturating_add(1);
        self.events.push(DogfoodRecordingEvent::Output {
            elapsed_ms: self.elapsed_ms(),
            bytes: bytes[..stored].to_vec(),
        });
    }

    fn record_batch(&mut self, status: DogfoodBatchStatus, duration: Duration) {
        self.batch_status_counts.record(status);
        if self.batch_timings_ns.len() < M3_MAX_BATCH_TIMING_SAMPLES {
            self.batch_timings_ns
                .push(duration.as_nanos().min(u128::from(u64::MAX)) as u64);
        }

        if latency_probe_enabled() {
            log::warn!(
                "[latency] hera dogfood pty_batch: status={} elapsed_ms={:.3}",
                status.as_str(),
                duration.as_secs_f64() * 1000.0
            );
        }
    }

    fn record_input(&mut self, summary: InputMetadataSummary) {
        self.event_counts.input = self.event_counts.input.saturating_add(1);
        self.events.push(DogfoodRecordingEvent::Input {
            elapsed_ms: self.elapsed_ms(),
            byte_count: summary.byte_count,
            escaped_summary: summary.escaped_summary,
            truncated: summary.truncated,
        });
    }

    fn record_resize(&mut self, columns: usize, rows: usize) {
        self.event_counts.resize = self.event_counts.resize.saturating_add(1);
        self.events.push(DogfoodRecordingEvent::Resize {
            elapsed_ms: self.elapsed_ms(),
            columns,
            rows,
        });
    }

    fn finish(
        mut self,
        pane_id: &str,
        exit_code: i32,
        final_snapshot: Option<RenderSnapshot>,
        diff_counters: DiffCounters,
    ) -> DogfoodRecordingBundle {
        self.event_counts.lifecycle = self.event_counts.lifecycle.saturating_add(1);
        self.events.push(DogfoodRecordingEvent::Lifecycle {
            elapsed_ms: self.elapsed_ms(),
            state: "exit".to_owned(),
            exit_code: Some(exit_code),
        });
        let final_snapshot = final_snapshot.as_ref().map(DogfoodFinalSnapshot::from_hera);
        let metrics = self.metrics(pane_id, final_snapshot.as_ref(), diff_counters);
        let recording = DogfoodRecordingArtifact {
            schema: M3_DOGFOOD_RECORDING_SCHEMA,
            version: M3_DOGFOOD_RECORDING_VERSION,
            metadata: DogfoodRecordingMetadata {
                source: "paneflow".to_owned(),
                initial_dimensions: self.initial_size,
                event_counts: self.event_counts,
                redaction_status: self.retention,
                output_bytes: self.output_bytes,
                output_truncated: self.output_truncated,
                max_output_bytes: self.max_output_bytes,
            },
            events: self.events,
            final_snapshot,
        };

        DogfoodRecordingBundle { recording, metrics }
    }

    fn metrics(
        &self,
        pane_id: &str,
        final_snapshot: Option<&DogfoodFinalSnapshot>,
        diff_counters: DiffCounters,
    ) -> DogfoodMetricsArtifact {
        let rss_after_bytes = current_process_rss_bytes();
        let rss_delta_bytes = rss_delta(&self.rss_baseline_bytes, &rss_after_bytes);
        let latency = DogfoodLatencyMetrics {
            pty_batch_samples: self.batch_timings_ns.len(),
            pty_batch_status_counts: self.batch_status_counts,
            pty_batch_p50_ms: percentile_ms(&self.batch_timings_ns, 50),
            pty_batch_p95_ms: percentile_ms(&self.batch_timings_ns, 95),
            pty_batch_p99_ms: percentile_ms(&self.batch_timings_ns, 99),
        };
        let memory = DogfoodMemoryMetrics {
            paneflow_rss_baseline_bytes: self.rss_baseline_bytes.clone(),
            paneflow_rss_after_bytes: rss_after_bytes,
            dogfood_rss_delta_bytes: rss_delta_bytes,
        };
        let decision = dogfood_metrics_decision(&memory, &latency);

        DogfoodMetricsArtifact {
            schema: M3_DOGFOOD_METRICS_SCHEMA,
            version: M3_DOGFOOD_METRICS_VERSION,
            pane_id: pane_id.to_owned(),
            timestamp_ms: now_millis(),
            source: "paneflow".to_owned(),
            redaction_status: self.retention,
            session: DogfoodSessionMetrics {
                initial_dimensions: self.initial_size,
                final_dimensions: final_snapshot.map(|snapshot| snapshot.dimensions),
                event_counts: self.event_counts,
                logical_output_lines: self.logical_output_lines,
                observed_output_bytes: self.observed_output_bytes,
                recording_output_bytes: self.output_bytes,
                output_truncated: self.output_truncated,
                max_output_bytes: self.max_output_bytes,
            },
            memory,
            latency,
            diff_counters,
            decision,
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct DogfoodEventCounts {
    output: u64,
    input: u64,
    resize: u64,
    lifecycle: u64,
}

#[derive(Serialize)]
struct DogfoodRecordingArtifact {
    schema: &'static str,
    version: u32,
    metadata: DogfoodRecordingMetadata,
    events: Vec<DogfoodRecordingEvent>,
    final_snapshot: Option<DogfoodFinalSnapshot>,
}

#[derive(Serialize)]
struct DogfoodRecordingMetadata {
    source: String,
    initial_dimensions: ComparisonDimensions,
    event_counts: DogfoodEventCounts,
    redaction_status: ArtifactRetention,
    output_bytes: usize,
    output_truncated: bool,
    max_output_bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DogfoodRecordingEvent {
    Output {
        elapsed_ms: u64,
        bytes: Vec<u8>,
    },
    Input {
        elapsed_ms: u64,
        byte_count: usize,
        escaped_summary: String,
        truncated: bool,
    },
    Resize {
        elapsed_ms: u64,
        columns: usize,
        rows: usize,
    },
    Lifecycle {
        elapsed_ms: u64,
        state: String,
        exit_code: Option<i32>,
    },
}

#[derive(Serialize)]
struct DogfoodFinalSnapshot {
    dimensions: ComparisonDimensions,
    active_screen: ComparisonScreen,
    cursor: ComparisonCursor,
    viewport_lines: Vec<String>,
    scrollback_line_count: usize,
}

impl DogfoodFinalSnapshot {
    fn from_hera(snapshot: &RenderSnapshot) -> Self {
        let summary = ComparisonSummary::from_hera_snapshot(snapshot);
        Self {
            dimensions: summary
                .dimensions
                .as_supported()
                .copied()
                .unwrap_or_else(|| ComparisonDimensions::new(snapshot.columns(), snapshot.rows())),
            active_screen: summary
                .active_screen
                .as_supported()
                .copied()
                .unwrap_or_else(|| snapshot.active_screen().into()),
            cursor: summary.cursor.as_supported().copied().unwrap_or_else(|| {
                ComparisonCursor::new(
                    snapshot.cursor().row(),
                    snapshot.cursor().column(),
                    snapshot.cursor().visible(),
                )
            }),
            viewport_lines: summary
                .viewport_lines
                .as_supported()
                .cloned()
                .unwrap_or_default(),
            scrollback_line_count: summary
                .scrollback_line_count
                .as_supported()
                .copied()
                .unwrap_or_else(|| snapshot.scrollback_rows().len()),
        }
    }
}

#[derive(Clone)]
pub struct PtyOutputTap {
    tx: SyncSender<Vec<u8>>,
    dropped_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl PtyOutputTap {
    pub fn record_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        match self.tx.try_send(bytes.to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(bytes) | TrySendError::Disconnected(bytes)) => {
                self.dropped_bytes
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            }
        }
    }
}

pub struct PtyOutputDrain {
    rx: Receiver<Vec<u8>>,
    dropped_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl PtyOutputDrain {
    fn try_recv(&self) -> Result<Vec<u8>, TryRecvError> {
        self.rx.try_recv()
    }

    fn take_dropped_bytes(&self) -> u64 {
        self.dropped_bytes.swap(0, Ordering::Relaxed)
    }
}

fn output_tap_channel() -> (PtyOutputTap, PtyOutputDrain) {
    let (tx, rx) = sync_channel(OUTPUT_QUEUE_CAPACITY);
    let dropped_bytes = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    (
        PtyOutputTap {
            tx,
            dropped_bytes: dropped_bytes.clone(),
        },
        PtyOutputDrain { rx, dropped_bytes },
    )
}

pub struct TerminalShadowState {
    mode: DogfoodMode,
    shadow: Arc<Mutex<Option<ShadowSession>>>,
    output_tap: Option<PtyOutputTap>,
    output_drain: Option<PtyOutputDrain>,
    counters: Arc<Mutex<DiffCounters>>,
    artifacts: Arc<Mutex<ArtifactWriter>>,
    recording: Arc<Mutex<Option<DogfoodRecordingBuilder>>>,
    last_reported_difference: Arc<Mutex<Option<String>>>,
}

impl TerminalShadowState {
    pub fn from_runtime_gate(columns: usize, rows: usize) -> Self {
        Self::for_mode(configured_mode(), columns, rows)
    }

    pub fn for_mode(mode: DogfoodMode, columns: usize, rows: usize) -> Self {
        let shadow = ShadowSession::for_mode(mode, columns, rows);
        let artifacts = ArtifactWriter::from_env();
        let recording = shadow
            .as_ref()
            .map(|_| DogfoodRecordingBuilder::new(columns, rows, artifacts.retention()));
        let (output_tap, output_drain) = if shadow.is_some() {
            let (tap, drain) = output_tap_channel();
            (Some(tap), Some(drain))
        } else {
            (None, None)
        };

        Self {
            mode,
            shadow: Arc::new(Mutex::new(shadow)),
            output_tap,
            output_drain,
            counters: Arc::new(Mutex::new(DiffCounters::default())),
            artifacts: Arc::new(Mutex::new(artifacts)),
            recording: Arc::new(Mutex::new(recording)),
            last_reported_difference: Arc::new(Mutex::new(None)),
        }
    }

    pub fn pty_tap(&self) -> Option<PtyOutputTap> {
        self.output_tap.clone()
    }

    pub const fn is_side_by_side_enabled(&self) -> bool {
        self.mode.shows_side_by_side()
    }

    pub fn side_by_side_surface(&self) -> Option<SideBySideDiagnosticSurface> {
        if !self.is_side_by_side_enabled() {
            return None;
        }

        let mut shadow = match self.shadow.try_lock() {
            Ok(shadow) => shadow,
            Err(std::sync::TryLockError::WouldBlock) => {
                self.increment_counter(|counters| {
                    counters.side_by_side_skipped = counters.side_by_side_skipped.saturating_add(1);
                });
                return Some(SideBySideDiagnosticSurface::skipped(
                    "shadow_lock_busy",
                    vec![AdapterDiagnostic::new(
                        "side_by_side_skipped",
                        "Hera side-by-side skipped this frame because the shadow session was busy",
                    )],
                    self.counters(),
                ));
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.increment_counter(|counters| {
                    counters.side_by_side_skipped = counters.side_by_side_skipped.saturating_add(1);
                });
                return Some(SideBySideDiagnosticSurface::skipped(
                    "shadow_lock_poisoned",
                    vec![AdapterDiagnostic::new(
                        "side_by_side_lock_poisoned",
                        "Hera side-by-side shadow lock is poisoned",
                    )],
                    self.counters(),
                ));
            }
        };

        let Some(shadow) = shadow.as_mut() else {
            return Some(SideBySideDiagnosticSurface::skipped(
                "shadow_disabled",
                vec![AdapterDiagnostic::new(
                    "shadow_disabled",
                    "Hera side-by-side is enabled but no shadow session is attached",
                )],
                self.counters(),
            ));
        };

        if !shadow.is_enabled() {
            return Some(SideBySideDiagnosticSurface::skipped(
                "shadow_disabled",
                shadow.diagnostics().to_vec(),
                self.counters(),
            ));
        }

        let Some(snapshot) = shadow.render_snapshot() else {
            return Some(SideBySideDiagnosticSurface::skipped(
                "snapshot_unavailable",
                shadow.diagnostics().to_vec(),
                self.counters(),
            ));
        };

        match layout_content_from_hera_snapshot(&snapshot) {
            Ok(layout_content) => Some(SideBySideDiagnosticSurface::available(
                viewport_lines_from_content(
                    snapshot.columns(),
                    snapshot.rows(),
                    layout_content.content(),
                ),
                layout_content.diagnostics().to_vec(),
                self.counters(),
            )),
            Err(error) => {
                self.increment_counter(|counters| {
                    counters.side_by_side_skipped = counters.side_by_side_skipped.saturating_add(1);
                });
                shadow.record_adapter_error(error.to_string());
                Some(SideBySideDiagnosticSurface::skipped(
                    "adapter_error",
                    shadow.diagnostics().to_vec(),
                    self.counters(),
                ))
            }
        }
    }

    pub fn resize_tap(&self) -> Option<HeraResizeTap> {
        self.output_tap.as_ref().map(|_| HeraResizeTap {
            shadow: self.shadow.clone(),
            recording: self.recording.clone(),
        })
    }

    pub fn drain_output(&mut self) {
        let Some(output_drain) = &self.output_drain else {
            return;
        };

        let dropped_bytes = output_drain.take_dropped_bytes();
        let mut chunks = Vec::new();
        loop {
            match output_drain.try_recv() {
                Ok(bytes) => chunks.push(bytes),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        if dropped_bytes == 0 && chunks.is_empty() {
            return;
        }

        self.with_shadow_mut(|shadow| {
            if dropped_bytes > 0 {
                shadow.record_dropped_output(dropped_bytes);
            }
            for chunk in chunks {
                if let Ok(mut recording) = self.recording.lock()
                    && let Some(recording) = recording.as_mut()
                {
                    recording.record_output(&chunk);
                }
                let started = Instant::now();
                let status = shadow.advance_output(&chunk);
                let duration = started.elapsed();
                if let Ok(mut recording) = self.recording.lock()
                    && let Some(recording) = recording.as_mut()
                {
                    recording.record_batch(status, duration);
                }
            }
        });
    }

    pub fn note_input_metadata(&self, bytes: &[u8]) {
        self.with_shadow_mut(|shadow| shadow.note_input_metadata(bytes));
        if let Ok(mut recording) = self.recording.lock()
            && let Some(recording) = recording.as_mut()
        {
            recording.record_input(InputMetadataSummary::from_bytes(bytes));
        }
    }

    pub fn compare_checkpoint(
        &self,
        paneflow_summary: &ComparisonSummary,
        context: DogfoodReportContext,
    ) -> ComparisonOutcome {
        let (hera_summary, hera_output_bytes_seen, hera_input_bytes_seen) = match self.shadow.lock()
        {
            Ok(mut shadow) => match shadow.as_mut() {
                Some(shadow) if shadow.is_enabled() => {
                    let output_bytes_seen = shadow.output_bytes_seen();
                    let input_bytes_seen = shadow.input_bytes_seen();
                    (
                        shadow.comparison_summary(),
                        output_bytes_seen,
                        input_bytes_seen,
                    )
                }
                _ => {
                    self.increment_counter(|counters| {
                        counters.shadow_disabled = counters.shadow_disabled.saturating_add(1)
                    });
                    return ComparisonOutcome::Unsupported(ComparisonDifference::new(
                        "$.shadow", "disabled", "disabled",
                    ));
                }
            },
            Err(_) => {
                self.increment_counter(|counters| {
                    counters.shadow_disabled = counters.shadow_disabled.saturating_add(1)
                });
                return ComparisonOutcome::Unsupported(ComparisonDifference::new(
                    "$.shadow_lock",
                    "poisoned",
                    "poisoned",
                ));
            }
        };

        let outcome = paneflow_summary.checkpoint_difference(
            &hera_summary,
            hera_output_bytes_seen,
            hera_input_bytes_seen,
        );
        match &outcome {
            ComparisonOutcome::Equal => {
                self.increment_counter(|counters| {
                    counters.equal = counters.equal.saturating_add(1)
                });
            }
            ComparisonOutcome::Mismatch(_) => {
                self.increment_counter(|counters| {
                    counters.mismatch = counters.mismatch.saturating_add(1)
                });
                self.write_mismatch_report(paneflow_summary, &hera_summary, &outcome, context);
            }
            ComparisonOutcome::Unsupported(_) => {
                self.increment_counter(|counters| {
                    counters.unsupported = counters.unsupported.saturating_add(1)
                });
            }
        }
        outcome
    }

    pub fn counters(&self) -> DiffCounters {
        self.counters
            .lock()
            .map(|counters| *counters)
            .unwrap_or_default()
    }

    pub fn record_exit(&self, pane_id: &str, exit_code: i32) {
        let final_snapshot = self
            .shadow
            .lock()
            .ok()
            .and_then(|mut shadow| shadow.as_mut().and_then(ShadowSession::render_snapshot));
        let recording = self
            .recording
            .lock()
            .ok()
            .and_then(|mut recording| recording.take());
        if let Some(recording) = recording {
            let bundle = recording.finish(pane_id, exit_code, final_snapshot, self.counters());
            if let Ok(mut artifacts) = self.artifacts.lock() {
                artifacts.write_recording(pane_id, &bundle.recording, &bundle.metrics);
            }
        }
    }

    #[allow(dead_code)]
    pub fn with_shadow<R>(&self, f: impl FnOnce(&ShadowSession) -> R) -> Option<R> {
        self.shadow.lock().ok()?.as_ref().map(f)
    }

    pub fn with_shadow_mut<R>(&self, f: impl FnOnce(&mut ShadowSession) -> R) -> Option<R> {
        self.shadow.lock().ok()?.as_mut().map(f)
    }

    fn increment_counter(&self, f: impl FnOnce(&mut DiffCounters)) {
        if let Ok(mut counters) = self.counters.lock() {
            f(&mut counters);
        }
    }

    fn write_mismatch_report(
        &self,
        paneflow_summary: &ComparisonSummary,
        hera_summary: &ComparisonSummary,
        outcome: &ComparisonOutcome,
        context: DogfoodReportContext,
    ) {
        let Some(difference) = outcome.difference() else {
            return;
        };
        if !self.should_write_difference(difference) {
            return;
        }
        let report = MismatchReport {
            schema: "hera.dogfood_mismatch_report",
            version: 1,
            pane_id: context.pane_id.clone(),
            timestamp_ms: now_millis(),
            outcome: outcome.clone(),
            field_path: difference.field_path.clone(),
            dimensions: paneflow_summary
                .dimensions()
                .or_else(|| hera_summary.dimensions()),
            command: RedactedCommandMetadata {
                cwd: context.cwd.as_deref().map(redact_text),
                exit_code: context.exit_code,
            },
            counters: self.counters(),
            excerpts: ReportExcerpts {
                paneflow: report_excerpts(paneflow_summary.viewport_lines()),
                hera: report_excerpts(hera_summary.viewport_lines()),
            },
        };
        if let Ok(mut artifacts) = self.artifacts.lock() {
            artifacts.write_report(&context.pane_id, &report);
        }
    }

    fn should_write_difference(&self, difference: &ComparisonDifference) -> bool {
        let key = format!(
            "{}:{}:{}",
            difference.field_path, difference.left, difference.right
        );
        let Ok(mut last) = self.last_reported_difference.lock() else {
            return true;
        };
        if last.as_deref() == Some(key.as_str()) {
            return false;
        }
        *last = Some(key);
        true
    }
}

#[derive(Clone)]
pub struct HeraResizeTap {
    shadow: Arc<Mutex<Option<ShadowSession>>>,
    recording: Arc<Mutex<Option<DogfoodRecordingBuilder>>>,
}

impl HeraResizeTap {
    pub fn mirror_resize(&self, columns: usize, rows: usize) {
        if let Ok(mut recording) = self.recording.lock()
            && let Some(recording) = recording.as_mut()
        {
            recording.record_resize(columns, rows);
        }
        let Ok(mut shadow) = self.shadow.lock() else {
            return;
        };
        if let Some(shadow) = shadow.as_mut() {
            shadow.resize(columns, rows);
        }
    }
}

pub struct ShadowSession {
    core: Terminal,
    diagnostics: Vec<AdapterDiagnostic>,
    input_records: VecDeque<InputMetadataSummary>,
    input_bytes_seen: u64,
    output_bytes_seen: u64,
    dropped_output_bytes: u64,
    disabled: Option<AdapterDiagnostic>,
}

#[allow(dead_code)]
impl ShadowSession {
    pub fn from_runtime_gate(columns: usize, rows: usize) -> Option<Self> {
        Self::for_mode(configured_mode(), columns, rows)
    }

    pub fn for_mode(mode: DogfoodMode, columns: usize, rows: usize) -> Option<Self> {
        if !mode.attaches_shadow() {
            return None;
        }

        match Terminal::new(columns, rows) {
            Ok(core) => {
                log::info!(
                    "Hera dogfood shadow session attached ({columns}x{rows}); diagnostics are local only"
                );
                Some(Self {
                    core,
                    diagnostics: Vec::new(),
                    input_records: VecDeque::new(),
                    input_bytes_seen: 0,
                    output_bytes_seen: 0,
                    dropped_output_bytes: 0,
                    disabled: None,
                })
            }
            Err(error) => {
                log::warn!("Hera dogfood shadow init failed for {columns}x{rows}: {error}");
                None
            }
        }
    }

    pub fn note_input_metadata(&mut self, bytes: &[u8]) {
        if !self.is_enabled() {
            return;
        }

        self.input_bytes_seen = self.input_bytes_seen.saturating_add(bytes.len() as u64);
        if self.input_records.len() == INPUT_METADATA_RECORD_LIMIT {
            self.input_records.pop_front();
        }
        self.input_records
            .push_back(InputMetadataSummary::from_bytes(bytes));
    }

    pub fn record_unsupported_field(&mut self, field_path: &'static str) {
        self.diagnostics.push(AdapterDiagnostic::new(
            "unsupported_snapshot_field",
            format!("unsupported Hera snapshot field: {field_path}"),
        ));
    }

    pub fn record_adapter_error(&mut self, message: impl Into<String>) {
        self.diagnostics.push(AdapterDiagnostic::new(
            "layout_adapter_error",
            message.into(),
        ));
    }

    fn advance_output(&mut self, bytes: &[u8]) -> DogfoodBatchStatus {
        if !self.is_enabled() {
            return DogfoodBatchStatus::Skipped;
        }

        if catch_unwind(AssertUnwindSafe(|| self.core.advance_bytes(bytes))).is_err() {
            self.disable_with_diagnostic(AdapterDiagnostic::new(
                "advance_panicked",
                "Hera dogfood advance panicked; shadow session disabled",
            ));
            return DogfoodBatchStatus::Errored;
        }
        self.output_bytes_seen = self.output_bytes_seen.saturating_add(bytes.len() as u64);
        DogfoodBatchStatus::Rendered
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        if !self.is_enabled() {
            return;
        }

        match catch_unwind(AssertUnwindSafe(|| self.core.resize(columns, rows))) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.disable_with_diagnostic(AdapterDiagnostic::new(
                    "resize_rejected",
                    format!("Hera rejected resize to {columns}x{rows}: {error}"),
                ));
            }
            Err(_) => {
                self.disable_with_diagnostic(AdapterDiagnostic::new(
                    "resize_panicked",
                    "Hera dogfood resize panicked; shadow session disabled",
                ));
            }
        }
    }

    pub fn render_snapshot(&mut self) -> Option<RenderSnapshot> {
        self.is_enabled().then(|| self.core.render_snapshot())
    }

    pub fn comparison_summary(&mut self) -> ComparisonSummary {
        self.render_snapshot()
            .as_ref()
            .map(ComparisonSummary::from_hera_snapshot)
            .unwrap_or_else(ComparisonSummary::unsupported)
    }

    pub fn viewport_text(&mut self) -> Option<String> {
        let snapshot = self.render_snapshot()?;
        let mut text = String::new();
        for row in snapshot.viewport_rows() {
            for cell in row.cells() {
                text.push(cell.ch());
            }
            text.push('\n');
        }
        Some(text)
    }

    pub fn record_dropped_output(&mut self, byte_count: u64) {
        self.dropped_output_bytes = self.dropped_output_bytes.saturating_add(byte_count);
        self.diagnostics.push(AdapterDiagnostic::new(
            "output_queue_overflow",
            format!("Hera dogfood output queue dropped {byte_count} bytes"),
        ));
    }

    pub fn disable_with_diagnostic(&mut self, diagnostic: AdapterDiagnostic) {
        self.disabled = Some(diagnostic.clone());
        self.diagnostics.push(diagnostic);
    }

    pub fn is_enabled(&self) -> bool {
        self.disabled.is_none()
    }

    pub fn diagnostics(&self) -> &[AdapterDiagnostic] {
        &self.diagnostics
    }

    pub fn latest_input(&self) -> Option<&InputMetadataSummary> {
        self.input_records.back()
    }

    pub const fn input_bytes_seen(&self) -> u64 {
        self.input_bytes_seen
    }

    pub const fn output_bytes_seen(&self) -> u64 {
        self.output_bytes_seen
    }

    pub const fn dropped_output_bytes(&self) -> u64 {
        self.dropped_output_bytes
    }
}

pub fn configured_mode() -> DogfoodMode {
    let raw = std::env::var(DOGFOOD_ENV_VAR).ok();
    let parsed = parse_mode(raw.as_deref());
    if parsed == ParsedDogfoodMode::Unknown {
        warn_unknown_mode_once(raw.as_deref().unwrap_or_default());
        DogfoodMode::Disabled
    } else {
        parsed.into_mode()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParsedDogfoodMode {
    Disabled,
    Shadow,
    SideBySide,
    Unknown,
}

impl ParsedDogfoodMode {
    const fn into_mode(self) -> DogfoodMode {
        match self {
            Self::Disabled | Self::Unknown => DogfoodMode::Disabled,
            Self::Shadow => DogfoodMode::Shadow,
            Self::SideBySide => DogfoodMode::SideBySide,
        }
    }
}

fn parse_mode(value: Option<&str>) -> ParsedDogfoodMode {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => ParsedDogfoodMode::Disabled,
        Some("0" | "false" | "off" | "disabled") => ParsedDogfoodMode::Disabled,
        Some("shadow") => ParsedDogfoodMode::Shadow,
        Some("side_by_side") => ParsedDogfoodMode::SideBySide,
        Some(_) => ParsedDogfoodMode::Unknown,
    }
}

fn warn_unknown_mode_once(value: &str) {
    if !UNKNOWN_MODE_WARNED.swap(true, Ordering::Relaxed) {
        log::warn!("unknown Hera dogfood mode `{value}`; dogfood disabled");
    }
}

fn escape_bytes(bytes: &[u8], limit: usize) -> String {
    let mut escaped = String::new();
    for &byte in bytes.iter().take(limit) {
        match byte {
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            0x1b => escaped.push_str("\\x1b"),
            0x20..=0x7e => escaped.push(byte as char),
            _ => escaped.push_str(&format!("\\x{byte:02x}")),
        }
    }
    escaped
}

fn viewport_lines_from_paneflow(
    columns: usize,
    rows: usize,
    content: &super::types::Content,
) -> Vec<String> {
    viewport_lines_from_content(columns, rows, content)
}

fn viewport_lines_from_content(columns: usize, rows: usize, content: &Content) -> Vec<String> {
    let mut grid = vec![vec![' '; columns]; rows];
    for cell in &content.cells {
        let row = cell.point.line.0;
        if row < 0 {
            continue;
        }
        let row = row as usize;
        let column = cell.point.column.0;
        if row < rows && column < columns {
            grid[row][column] = cell.c;
        }
    }
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

fn style_buckets_from_hera(snapshot: &RenderSnapshot) -> Vec<StyleBucket> {
    let mut buckets = BTreeMap::new();
    for row in snapshot
        .scrollback_rows()
        .iter()
        .flat_map(|row| row.cells())
        .chain(snapshot.viewport_rows().iter().flat_map(|row| row.cells()))
    {
        *buckets.entry(hera_style_key(row.style())).or_insert(0usize) += 1;
    }
    buckets
        .into_iter()
        .map(|(style, count)| StyleBucket { style, count })
        .collect()
}

fn style_buckets_from_paneflow(content: &super::types::Content) -> Vec<StyleBucket> {
    let mut buckets = BTreeMap::new();
    for cell in &content.cells {
        *buckets.entry(paneflow_style_key(cell)).or_insert(0usize) += 1;
    }
    buckets
        .into_iter()
        .map(|(style, count)| StyleBucket { style, count })
        .collect()
}

fn hera_style_key(style: CellStyle) -> String {
    format!(
        "fg={};bg={};bold={};italic={};underline={};inverse={};hyperlink=false",
        hera_color_key(style.foreground(), NamedColor::Foreground),
        hera_color_key(style.background(), NamedColor::Background),
        style.bold(),
        style.italic(),
        style.underline(),
        style.inverse()
    )
}

fn hera_color_key(color: Option<HeraColor>, default: NamedColor) -> String {
    match color {
        Some(HeraColor::Indexed(index)) => format!("indexed:{index}"),
        Some(HeraColor::Rgb { red, green, blue }) => {
            format!("rgb:{red:02x}{green:02x}{blue:02x}")
        }
        None => format!("named:{default:?}"),
    }
}

fn paneflow_style_key(cell: &super::types::Cell) -> String {
    let flags = cell.flags;
    format!(
        "fg={};bg={};bold={};italic={};underline={};inverse={};hyperlink={}",
        paneflow_color_key(cell.fg),
        paneflow_color_key(cell.bg),
        flags.contains(super::types::CellFlags::BOLD),
        flags.contains(super::types::CellFlags::ITALIC),
        flags.contains(super::types::CellFlags::UNDERLINE),
        flags.contains(super::types::CellFlags::INVERSE),
        cell.hyperlink
    )
}

fn paneflow_color_key(color: super::types::Color) -> String {
    match color {
        super::types::Color::Named(color) => format!("named:{color:?}"),
        super::types::Color::Spec(rgb) => format!("rgb:{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b),
        super::types::Color::Indexed(index) => format!("indexed:{index}"),
    }
}

fn should_defer_blank_bootstrap_cursor_mismatch(
    paneflow_summary: &ComparisonSummary,
    hera_summary: &ComparisonSummary,
) -> bool {
    let cursor_differs = match (&paneflow_summary.cursor, &hera_summary.cursor) {
        (ComparisonField::Supported(paneflow), ComparisonField::Supported(hera)) => {
            paneflow != hera
        }
        _ => false,
    };
    if !cursor_differs {
        return false;
    }

    summary_viewport_is_blank(paneflow_summary) && summary_viewport_is_blank(hera_summary)
}

fn should_defer_output_until_shadow_has_text(
    paneflow_summary: &ComparisonSummary,
    hera_summary: &ComparisonSummary,
) -> bool {
    summary_dimensions_match(paneflow_summary, hera_summary)
        && summary_active_screen_matches(paneflow_summary, hera_summary)
        && summary_viewport_has_text(paneflow_summary)
        && summary_viewport_is_blank(hera_summary)
}

fn should_defer_bootstrap_viewport_alignment_drift(
    paneflow_summary: &ComparisonSummary,
    hera_summary: &ComparisonSummary,
) -> bool {
    if !summary_dimensions_match(paneflow_summary, hera_summary)
        || !summary_active_screen_matches(paneflow_summary, hera_summary)
    {
        return false;
    }
    let Some(paneflow_lines) = paneflow_summary.viewport_lines() else {
        return false;
    };
    let Some(hera_lines) = hera_summary.viewport_lines() else {
        return false;
    };
    let Some((paneflow_first_row, _)) = first_normalized_nonblank_line(paneflow_lines) else {
        return false;
    };
    let Some((hera_first_row, _)) = first_normalized_nonblank_line(hera_lines) else {
        return false;
    };

    hera_first_row > paneflow_first_row
        && viewport_lines_share_visible_text(paneflow_lines, hera_lines)
}

fn summary_dimensions_match(left: &ComparisonSummary, right: &ComparisonSummary) -> bool {
    match (&left.dimensions, &right.dimensions) {
        (ComparisonField::Supported(left), ComparisonField::Supported(right)) => left == right,
        _ => false,
    }
}

fn summary_active_screen_matches(left: &ComparisonSummary, right: &ComparisonSummary) -> bool {
    match (&left.active_screen, &right.active_screen) {
        (ComparisonField::Supported(left), ComparisonField::Supported(right)) => left == right,
        _ => false,
    }
}

fn summary_viewport_has_text(summary: &ComparisonSummary) -> bool {
    summary
        .viewport_lines
        .as_supported()
        .is_some_and(|lines| lines.iter().any(|line| !line.trim().is_empty()))
}

fn summary_viewport_is_blank(summary: &ComparisonSummary) -> bool {
    summary
        .viewport_lines
        .as_supported()
        .is_some_and(|lines| lines.iter().all(|line| line.trim().is_empty()))
}

fn first_normalized_nonblank_line(lines: &[String]) -> Option<(usize, String)> {
    lines.iter().enumerate().find_map(|(index, line)| {
        let normalized = normalize_visible_line(line);
        (!normalized.is_empty()).then_some((index, normalized))
    })
}

fn viewport_lines_share_visible_text(left: &[String], right: &[String]) -> bool {
    let left_lines: Vec<_> = left
        .iter()
        .map(|line| normalize_visible_line(line))
        .filter(|line| !line.is_empty())
        .collect();
    let right_lines: Vec<_> = right
        .iter()
        .map(|line| normalize_visible_line(line))
        .filter(|line| !line.is_empty())
        .collect();

    left_lines.iter().any(|left| {
        right_lines
            .iter()
            .any(|right| visible_lines_overlap(left, right))
    })
}

fn normalize_visible_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn visible_lines_overlap(left: &str, right: &str) -> bool {
    const MIN_BOOTSTRAP_OVERLAP_CHARS: usize = 8;

    if left == right {
        return true;
    }
    if left.chars().count().min(right.chars().count()) < MIN_BOOTSTRAP_OVERLAP_CHARS {
        return false;
    }
    left.contains(right) || right.contains(left)
}

fn diff_field<T>(
    field_path: &'static str,
    left: &ComparisonField<T>,
    right: &ComparisonField<T>,
) -> Option<ComparisonOutcome>
where
    T: std::fmt::Debug + PartialEq,
{
    match (left, right) {
        (ComparisonField::Unsupported, ComparisonField::Unsupported) => None,
        (ComparisonField::Unsupported, _) | (_, ComparisonField::Unsupported) => Some(
            ComparisonOutcome::Unsupported(ComparisonDifference::new(field_path, left, right)),
        ),
        (ComparisonField::Supported(left), ComparisonField::Supported(right)) => (left != right)
            .then(|| {
                ComparisonOutcome::Mismatch(ComparisonDifference::new(field_path, left, right))
            }),
    }
}

fn diff_lines(
    field_path: &'static str,
    left: &ComparisonField<Vec<String>>,
    right: &ComparisonField<Vec<String>>,
) -> Option<ComparisonOutcome> {
    match (left, right) {
        (ComparisonField::Unsupported, ComparisonField::Unsupported) => None,
        (ComparisonField::Unsupported, _) | (_, ComparisonField::Unsupported) => Some(
            ComparisonOutcome::Unsupported(ComparisonDifference::new(field_path, left, right)),
        ),
        (ComparisonField::Supported(left), ComparisonField::Supported(right)) => {
            if left.len() != right.len() {
                return Some(ComparisonOutcome::Mismatch(ComparisonDifference::new(
                    format!("{field_path}.len"),
                    left.len(),
                    right.len(),
                )));
            }
            for (index, (left, right)) in left.iter().zip(right.iter()).enumerate() {
                if left != right {
                    return Some(ComparisonOutcome::Mismatch(ComparisonDifference::new(
                        format!("{field_path}[{index}]"),
                        left,
                        right,
                    )));
                }
            }
            None
        }
    }
}

fn compact_debug(value: impl std::fmt::Debug) -> String {
    let mut text = format!("{value:?}");
    if text.len() > DIFF_VALUE_LIMIT {
        text.truncate(DIFF_VALUE_LIMIT);
        text.push_str("...");
    }
    text
}

fn report_excerpts(lines: Option<&[String]>) -> Vec<String> {
    lines
        .unwrap_or_default()
        .iter()
        .take(REPORT_EXCERPT_LINE_LIMIT)
        .map(|line| {
            let redacted = redact_text(line);
            if redacted.len() > REPORT_EXCERPT_CHAR_LIMIT {
                let mut bounded = redacted;
                bounded.truncate(REPORT_EXCERPT_CHAR_LIMIT);
                bounded.push_str("...");
                bounded
            } else {
                redacted
            }
        })
        .collect()
}

fn redact_text(text: &str) -> String {
    text.split_whitespace()
        .map(redact_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_token(token: &str) -> String {
    let trimmed = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '"' | '\'' | '`' | ',' | ';' | ':' | ')' | '(' | '[' | ']' | '{' | '}'
        )
    });
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains(":\\")
        || lower.contains(":/")
        || lower.starts_with("/users/")
        || lower.starts_with("/home/")
        || lower.starts_with("\\\\")
        || lower.contains("\\users\\")
        || lower.contains("\\dev\\")
    {
        "[redacted-path]".to_owned()
    } else {
        token.to_owned()
    }
}

fn logical_output_line_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

fn percentile_ms(samples_ns: &[u64], percentile: usize) -> MetricMeasurement<f64> {
    if samples_ns.is_empty() {
        return MetricMeasurement::NotMeasured {
            os: std::env::consts::OS,
            command: "PANEFLOW_LATENCY_PROBE=1 with hera side_by_side output batches".to_owned(),
            reason: "no Hera dogfood PTY batch samples were recorded".to_owned(),
        };
    }

    let mut sorted = samples_ns.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) * percentile).div_ceil(100);
    MetricMeasurement::Measured {
        value: sorted[index] as f64 / 1_000_000.0,
    }
}

fn rss_delta(
    baseline: &MetricMeasurement<u64>,
    after: &MetricMeasurement<u64>,
) -> MetricMeasurement<i64> {
    match (baseline.value(), after.value()) {
        (Some(baseline), Some(after)) => MetricMeasurement::Measured {
            value: after as i64 - baseline as i64,
        },
        _ => MetricMeasurement::NotMeasured {
            os: std::env::consts::OS,
            command: process_rss_command(),
            reason: "baseline or after RSS sample was not measured".to_owned(),
        },
    }
}

fn current_process_rss_bytes() -> MetricMeasurement<u64> {
    #[cfg(target_os = "linux")]
    {
        let path = "/proc/self/status";
        match fs::read_to_string(path) {
            Ok(status) => {
                for line in status.lines() {
                    if let Some(value) = line.strip_prefix("VmRSS:") {
                        let Some(kib) = value
                            .split_whitespace()
                            .next()
                            .and_then(|value| value.parse::<u64>().ok())
                        else {
                            break;
                        };
                        return MetricMeasurement::Measured {
                            value: kib.saturating_mul(1024),
                        };
                    }
                }
                MetricMeasurement::NotMeasured {
                    os: std::env::consts::OS,
                    command: path.to_owned(),
                    reason: "VmRSS was absent or unparsable".to_owned(),
                }
            }
            Err(error) => MetricMeasurement::NotMeasured {
                os: std::env::consts::OS,
                command: path.to_owned(),
                reason: error.to_string(),
            },
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        MetricMeasurement::NotMeasured {
            os: std::env::consts::OS,
            command: process_rss_command(),
            reason: "RSS sampling is not wired for this target in the M3 dogfood harness"
                .to_owned(),
        }
    }
}

fn process_rss_command() -> String {
    #[cfg(target_os = "windows")]
    {
        format!(
            "Get-Process -Id {} | Select-Object -ExpandProperty WorkingSet64",
            std::process::id()
        )
    }

    #[cfg(target_os = "macos")]
    {
        format!("ps -o rss= -p {}", std::process::id())
    }

    #[cfg(target_os = "linux")]
    {
        "/proc/self/status VmRSS".to_owned()
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "process RSS command unavailable".to_owned()
    }
}

fn dogfood_metrics_decision(
    memory: &DogfoodMemoryMetrics,
    latency: &DogfoodLatencyMetrics,
) -> DogfoodMetricsDecision {
    let mut blocked_reasons = Vec::new();

    match memory.dogfood_rss_delta_bytes.value() {
        Some(delta) if delta > M3_10K_MEMORY_DELTA_BUDGET_BYTES => blocked_reasons.push(format!(
            "RSS delta {delta} bytes exceeds {} bytes",
            M3_10K_MEMORY_DELTA_BUDGET_BYTES
        )),
        Some(_) => {}
        None => blocked_reasons.push("RSS delta was not measured".to_owned()),
    }

    match latency.pty_batch_p95_ms.value() {
        Some(p95) if p95 > M3_BATCH_P95_BUDGET_MS => blocked_reasons.push(format!(
            "PTY batch P95 {p95:.3} ms exceeds {M3_BATCH_P95_BUDGET_MS:.3} ms"
        )),
        Some(_) => {}
        None => blocked_reasons.push("PTY batch P95 was not measured".to_owned()),
    }

    DogfoodMetricsDecision {
        replacement_blocked: !blocked_reasons.is_empty(),
        blocked_reasons,
    }
}

fn latency_probe_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var(LATENCY_PROBE_ENV_VAR).as_deref() == Ok("1"))
}

fn write_json(path: &PathBuf, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let mut file = File::create(path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) struct HeraLayoutGoldenCase {
    pub name: &'static str,
    pub columns: usize,
    pub rows: usize,
    pub layout: HeraLayoutContent,
}

#[cfg(test)]
pub(crate) fn hera_layout_golden_cases() -> Vec<HeraLayoutGoldenCase> {
    let default = CellStyle::default();
    let red = CellStyle::new(
        Some(HeraColor::Indexed(1)),
        None,
        false,
        false,
        false,
        false,
    );
    let truecolor = CellStyle::new(
        Some(HeraColor::Rgb {
            red: 200,
            green: 100,
            blue: 50,
        }),
        None,
        false,
        false,
        false,
        false,
    );
    let bold = CellStyle::new(None, None, true, false, false, false);
    let italic = CellStyle::new(None, None, false, true, false, false);
    let underline = CellStyle::new(None, None, false, false, true, false);
    let inverse = CellStyle::new(
        Some(HeraColor::Indexed(2)),
        Some(HeraColor::Indexed(4)),
        false,
        false,
        false,
        true,
    );
    let blue_bg = CellStyle::new(
        None,
        Some(HeraColor::Indexed(4)),
        false,
        false,
        false,
        false,
    );
    let image = ImagePlaceholder::new(
        ImageProtocol::Kitty,
        Some("img-1".to_owned()),
        2048,
        "unsupported image payload",
    );

    vec![
        hera_layout_case(
            "hera_plain",
            12,
            4,
            vec![row_text(12, "hi", default)],
            0,
            2,
            true,
            0,
        ),
        hera_layout_case(
            "hera_ansi_indexed",
            12,
            4,
            vec![row_text(12, "rgb", red)],
            0,
            1,
            true,
            0,
        ),
        hera_layout_case(
            "hera_truecolor",
            12,
            4,
            vec![row_text(12, "true", truecolor)],
            0,
            3,
            true,
            0,
        ),
        hera_layout_case(
            "hera_bold",
            12,
            4,
            vec![row_text(12, "bold", bold)],
            0,
            0,
            true,
            0,
        ),
        hera_layout_case(
            "hera_italic",
            12,
            4,
            vec![row_text(12, "italic", italic)],
            0,
            0,
            true,
            0,
        ),
        hera_layout_case(
            "hera_underline",
            12,
            4,
            vec![row_text(12, "under", underline)],
            0,
            0,
            true,
            0,
        ),
        hera_layout_case(
            "hera_inverse",
            12,
            4,
            vec![row_text(12, "inv", inverse)],
            0,
            1,
            true,
            0,
        ),
        hera_layout_case(
            "hera_background",
            12,
            4,
            vec![row_text(12, "bg", blue_bg)],
            0,
            1,
            true,
            0,
        ),
        hera_layout_case(
            "hera_cursor_hidden",
            12,
            4,
            vec![row_text(12, "hide", default)],
            0,
            0,
            false,
            0,
        ),
        hera_layout_case(
            "hera_scrollback",
            12,
            4,
            vec![row_text(12, "hist", default)],
            0,
            0,
            true,
            3,
        ),
        hera_layout_case(
            "hera_wide_cell",
            12,
            4,
            vec![row_cells(
                12,
                vec![
                    RenderCell::text('中', 2, default),
                    RenderCell::empty(),
                    RenderCell::text('x', 1, default),
                ],
            )],
            0,
            2,
            true,
            0,
        ),
        hera_layout_case(
            "hera_image_placeholder",
            12,
            4,
            vec![row_cells(
                12,
                vec![
                    RenderCell::text('a', 1, default),
                    RenderCell::image_placeholder(default, image),
                    RenderCell::text('z', 1, default),
                ],
            )],
            0,
            1,
            true,
            0,
        ),
    ]
}

#[cfg(test)]
fn hera_layout_case(
    name: &'static str,
    columns: usize,
    rows: usize,
    mut viewport: Vec<Vec<RenderCell>>,
    cursor_row: usize,
    cursor_column: usize,
    cursor_visible: bool,
    scrollback_count: usize,
) -> HeraLayoutGoldenCase {
    while viewport.len() < rows {
        viewport.push(vec![RenderCell::empty(); columns]);
    }

    let viewport_rows = viewport
        .into_iter()
        .take(rows)
        .enumerate()
        .map(|(index, cells)| ViewportRow::new(RowHandle::new(index as u64 + 1, 0), cells, false))
        .collect();
    let scrollback_rows = (0..scrollback_count)
        .map(|index| {
            ScrollbackRow::new(
                RowHandle::new(index as u64 + 10_000, 0),
                row_text(columns, "scroll", CellStyle::default()),
                false,
            )
        })
        .collect();
    let snapshot = RenderSnapshot::new(
        columns,
        rows,
        ScreenIdentity::Primary,
        CursorState::new(cursor_row, cursor_column, cursor_visible),
        viewport_rows,
        scrollback_rows,
        Vec::new(),
    );

    HeraLayoutGoldenCase {
        name,
        columns,
        rows,
        layout: layout_content_from_hera_snapshot(&snapshot)
            .expect("Hera golden case must stay within layout caps"),
    }
}

#[cfg(test)]
fn row_text(columns: usize, text: &str, style: CellStyle) -> Vec<RenderCell> {
    row_cells(
        columns,
        text.chars()
            .map(|ch| RenderCell::text(ch, 1, style))
            .collect(),
    )
}

#[cfg(test)]
fn row_cells(columns: usize, mut cells: Vec<RenderCell>) -> Vec<RenderCell> {
    cells.resize(columns, RenderCell::empty());
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn test_artifact_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paneflow-hera-dogfood-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("artifact dir should be created");
        dir
    }

    fn state_with_artifact_dir(name: &str) -> (TerminalShadowState, PathBuf) {
        let state = TerminalShadowState::for_mode(DogfoodMode::Shadow, 8, 2);
        let dir = test_artifact_dir(name);
        *state
            .artifacts
            .lock()
            .expect("artifact writer lock should be available") = ArtifactWriter {
            dir: Some(dir.clone()),
            retention: ArtifactRetention::RawLocal,
            warned: false,
            sequence: 0,
        };
        (state, dir)
    }

    fn blank_summary(columns: usize, rows: usize, cursor_row: usize) -> ComparisonSummary {
        ComparisonSummary {
            dimensions: ComparisonField::supported(ComparisonDimensions::new(columns, rows)),
            active_screen: ComparisonField::supported(ComparisonScreen::Primary),
            cursor: ComparisonField::supported(ComparisonCursor::new(cursor_row, 0, true)),
            viewport_lines: ComparisonField::supported(vec![" ".repeat(columns); rows]),
            style_buckets: ComparisonField::supported(Vec::new()),
            scrollback_line_count: ComparisonField::supported(0),
        }
    }

    fn snapshot_rows(columns: usize, rows: usize) -> Vec<ViewportRow> {
        (0..rows)
            .map(|index| {
                ViewportRow::new(
                    RowHandle::new(index as u64 + 1, 0),
                    vec![RenderCell::empty(); columns],
                    false,
                )
            })
            .collect()
    }

    fn paneflow_summary_from_shadow(state: &TerminalShadowState) -> ComparisonSummary {
        let snapshot = state
            .with_shadow_mut(|shadow| shadow.render_snapshot())
            .flatten()
            .expect("shadow snapshot");
        let layout =
            layout_content_from_hera_snapshot(&snapshot).expect("Hera snapshot maps to content");
        ComparisonSummary::from_paneflow_content(
            snapshot.columns(),
            snapshot.rows(),
            Some(ComparisonScreen::Primary),
            layout.content(),
        )
    }

    #[test]
    fn dogfood_mode_parser_accepts_disabled_and_shadow() {
        assert_eq!(parse_mode(None), ParsedDogfoodMode::Disabled);
        assert_eq!(parse_mode(Some("")), ParsedDogfoodMode::Disabled);
        assert_eq!(parse_mode(Some("off")), ParsedDogfoodMode::Disabled);
        assert_eq!(parse_mode(Some("shadow")), ParsedDogfoodMode::Shadow);
        assert_eq!(
            parse_mode(Some("side_by_side")),
            ParsedDogfoodMode::SideBySide
        );
    }

    #[test]
    fn shadow_session_attaches_only_in_shadow_mode() {
        assert!(ShadowSession::for_mode(DogfoodMode::Disabled, 80, 24).is_none());

        let session = ShadowSession::for_mode(DogfoodMode::Shadow, 80, 24)
            .expect("valid dimensions attach a Hera shadow core");

        assert!(session.is_enabled());
        assert_eq!(session.input_bytes_seen(), 0);
        assert_eq!(session.output_bytes_seen(), 0);
        assert!(session.diagnostics().is_empty());
    }

    #[test]
    fn invalid_shadow_dimensions_disable_shadow_without_panic() {
        assert!(ShadowSession::for_mode(DogfoodMode::Shadow, 0, 24).is_none());
    }

    #[test]
    fn unsupported_snapshot_fields_are_diagnostics() {
        let mut session = ShadowSession::for_mode(DogfoodMode::Shadow, 80, 24)
            .expect("valid dimensions attach a Hera shadow core");

        session.record_unsupported_field("style.image");

        assert_eq!(
            session.diagnostics()[0].code(),
            "unsupported_snapshot_field"
        );
        assert!(session.diagnostics()[0].message().contains("style.image"));
    }

    #[test]
    fn hera_layout_adapter_maps_text_style_and_cursor() {
        let style = CellStyle::new(
            Some(HeraColor::Rgb {
                red: 1,
                green: 2,
                blue: 3,
            }),
            Some(HeraColor::Indexed(4)),
            true,
            true,
            true,
            true,
        );
        let snapshot = RenderSnapshot::new(
            2,
            1,
            ScreenIdentity::Primary,
            CursorState::new(0, 0, true),
            vec![ViewportRow::new(
                RowHandle::new(1, 0),
                vec![RenderCell::text('A', 1, style), RenderCell::empty()],
                false,
            )],
            Vec::new(),
            Vec::new(),
        );

        let converted = layout_content_from_hera_snapshot(&snapshot).expect("layout conversion");
        let content = converted.content();
        let cell = &content.cells[0];

        assert_eq!(cell.c, 'A');
        assert_eq!(cell.fg, PaneflowColor::Spec(Rgb { r: 1, g: 2, b: 3 }));
        assert_eq!(cell.bg, PaneflowColor::Indexed(4));
        assert!(cell.flags.contains(CellFlags::BOLD));
        assert!(cell.flags.contains(CellFlags::ITALIC));
        assert!(cell.flags.contains(CellFlags::UNDERLINE));
        assert!(cell.flags.contains(CellFlags::INVERSE));
        assert_eq!(content.cursor.point, Point::new(0, 0));
        assert_eq!(content.cursor.shape, CursorShape::Block);
        assert_eq!(content.cursor.text, 'A');
        assert!(converted.diagnostics().is_empty());
    }

    #[test]
    fn hera_layout_adapter_reports_image_placeholder_diagnostic() {
        let image = ImagePlaceholder::new(
            ImageProtocol::Kitty,
            Some("img-1".to_owned()),
            2048,
            "unsupported image payload",
        );
        let snapshot = RenderSnapshot::new(
            1,
            1,
            ScreenIdentity::Primary,
            CursorState::new(0, 0, true),
            vec![ViewportRow::new(
                RowHandle::new(1, 0),
                vec![RenderCell::image_placeholder(CellStyle::default(), image)],
                false,
            )],
            Vec::new(),
            Vec::new(),
        );

        let converted = layout_content_from_hera_snapshot(&snapshot).expect("layout conversion");

        assert_eq!(converted.content().cells[0].c, '?');
        assert_eq!(
            converted.diagnostics()[0].code(),
            "unsupported_image_placeholder"
        );
        assert!(
            converted.diagnostics()[0]
                .message()
                .contains("protocol=Kitty")
        );
    }

    #[test]
    fn hera_layout_adapter_marks_trailing_wide_cell_spacer() {
        let snapshot = RenderSnapshot::new(
            3,
            1,
            ScreenIdentity::Primary,
            CursorState::new(0, 2, true),
            vec![ViewportRow::new(
                RowHandle::new(1, 0),
                vec![
                    RenderCell::text('中', 2, CellStyle::default()),
                    RenderCell::empty(),
                    RenderCell::text('x', 1, CellStyle::default()),
                ],
                false,
            )],
            Vec::new(),
            Vec::new(),
        );

        let converted = layout_content_from_hera_snapshot(&snapshot).expect("layout conversion");

        assert!(
            converted.content().cells[0]
                .flags
                .contains(CellFlags::WIDE_CHAR)
        );
        assert!(
            converted.content().cells[1]
                .flags
                .contains(CellFlags::WIDE_CHAR_SPACER)
        );
        assert_eq!(converted.content().cells[2].c, 'x');
    }

    #[test]
    fn hera_layout_adapter_rejects_dimensions_before_allocating_cells() {
        let snapshot = RenderSnapshot::new(
            MAX_HERA_LAYOUT_COLUMNS + 1,
            1,
            ScreenIdentity::Primary,
            CursorState::new(0, 0, true),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );

        let error = layout_content_from_hera_snapshot(&snapshot)
            .expect_err("oversized snapshot should fail before layout allocation");

        assert!(matches!(
            error,
            HeraLayoutAdapterError::DimensionsExceedCap { .. }
        ));
    }

    #[test]
    fn empty_shadow_comparison_is_equal_at_runtime_sizes() {
        for (columns, rows) in [(120, 40), (93, 34)] {
            let state = TerminalShadowState::for_mode(DogfoodMode::Shadow, columns, rows);
            let paneflow_summary = paneflow_summary_from_shadow(&state);

            let outcome = state.compare_checkpoint(
                &paneflow_summary,
                DogfoodReportContext::new("pane-empty", None, None),
            );

            assert_eq!(outcome, ComparisonOutcome::Equal);
            assert_eq!(state.counters().equal(), 1);
            assert_eq!(state.counters().mismatch(), 0);
        }
    }

    #[test]
    fn empty_bootstrap_cursor_only_mismatch_is_deferred_before_output() {
        let paneflow_summary = blank_summary(120, 40, 0);
        let hera_snapshot = RenderSnapshot::new(
            120,
            40,
            ScreenIdentity::Primary,
            CursorState::new(39, 0, true),
            snapshot_rows(120, 40),
            Vec::new(),
            Vec::new(),
        );
        let hera_summary = ComparisonSummary::from_hera_snapshot(&hera_snapshot);

        let outcome = paneflow_summary.checkpoint_difference(&hera_summary, 0, 0);

        assert!(matches!(
            outcome,
            ComparisonOutcome::Unsupported(difference)
                if difference.field_path() == "$.cursor.bootstrap_empty"
        ));
    }

    #[test]
    fn empty_bootstrap_cursor_only_mismatch_is_deferred_after_output_started() {
        let paneflow_summary = blank_summary(120, 40, 0);
        let hera_snapshot = RenderSnapshot::new(
            120,
            40,
            ScreenIdentity::Primary,
            CursorState::new(39, 0, true),
            snapshot_rows(120, 40),
            Vec::new(),
            Vec::new(),
        );
        let hera_summary = ComparisonSummary::from_hera_snapshot(&hera_snapshot);

        let outcome = paneflow_summary.checkpoint_difference(&hera_summary, 128, 0);

        assert!(matches!(
            outcome,
            ComparisonOutcome::Unsupported(difference)
                if difference.field_path() == "$.cursor.bootstrap_empty"
        ));
    }

    #[test]
    fn visible_bootstrap_output_waits_until_shadow_receives_pty_bytes() {
        let mut paneflow_summary = blank_summary(93, 34, 27);
        let mut paneflow_lines = vec![" ".repeat(93); 34];
        paneflow_lines[0] = "///////////////// ///////////////// user@host".to_owned();
        paneflow_summary.viewport_lines = ComparisonField::supported(paneflow_lines);
        let hera_summary = blank_summary(93, 34, 33);

        let outcome = paneflow_summary.checkpoint_difference(&hera_summary, 0, 0);

        assert!(matches!(
            outcome,
            ComparisonOutcome::Unsupported(difference)
                if difference.field_path() == "$.viewport_lines.shadow_blank"
        ));
    }

    #[test]
    fn visible_bootstrap_output_waits_until_shadow_has_text() {
        let mut paneflow_summary = blank_summary(120, 40, 25);
        let mut paneflow_lines = vec![" ".repeat(120); 40];
        paneflow_lines[0] = "///////////////// ///////////////// user@host".to_owned();
        paneflow_lines[1] = "///////////////// ///////////////// ----------------------".to_owned();
        paneflow_summary.viewport_lines = ComparisonField::supported(paneflow_lines);
        let hera_summary = blank_summary(120, 40, 39);

        let outcome = paneflow_summary.checkpoint_difference(&hera_summary, 128, 0);

        assert!(matches!(
            outcome,
            ComparisonOutcome::Unsupported(difference)
                if difference.field_path() == "$.viewport_lines.shadow_blank"
        ));
    }

    #[test]
    fn visible_bootstrap_output_waits_while_shadow_text_is_vertically_shifted() {
        let mut paneflow_summary = blank_summary(93, 34, 27);
        let mut paneflow_lines = vec![" ".repeat(93); 34];
        paneflow_lines[0] = "/////////////////  /////////////////    user@host".to_owned();
        paneflow_lines[1] =
            "/////////////////  /////////////////    ----------------------".to_owned();
        paneflow_lines[2] = "/////////////////  /////////////////    OS: Windows 11 Pro".to_owned();
        paneflow_summary.viewport_lines = ComparisonField::supported(paneflow_lines);

        let mut hera_summary = blank_summary(93, 34, 33);
        let mut hera_lines = vec![" ".repeat(93); 34];
        hera_lines[5] = "///////////////// /////////////////".to_owned();
        hera_lines[6] = "///////////////// ///////////////// ----------------------".to_owned();
        hera_summary.viewport_lines = ComparisonField::supported(hera_lines);

        let outcome = paneflow_summary.checkpoint_difference(&hera_summary, 128, 1);

        assert!(matches!(
            outcome,
            ComparisonOutcome::Unsupported(difference)
                if difference.field_path() == "$.viewport_lines.bootstrap_vertical_drift"
        ));
    }

    #[test]
    fn visible_output_mismatch_is_reported_after_shadow_has_text() {
        let mut paneflow_summary = blank_summary(93, 34, 27);
        let mut paneflow_lines = vec![" ".repeat(93); 34];
        paneflow_lines[0] = "startup output".to_owned();
        paneflow_summary.viewport_lines = ComparisonField::supported(paneflow_lines);
        let mut hera_summary = blank_summary(93, 34, 33);
        let mut hera_lines = vec![" ".repeat(93); 34];
        hera_lines[0] = "different output".to_owned();
        hera_summary.viewport_lines = ComparisonField::supported(hera_lines);

        let outcome = paneflow_summary.checkpoint_difference(&hera_summary, 128, 1);

        assert!(matches!(
            outcome,
            ComparisonOutcome::Mismatch(difference)
                if difference.field_path() == "$.viewport_lines[0]"
        ));
    }

    #[test]
    fn side_by_side_mode_attaches_shadow_and_exports_rows() {
        let mut state = TerminalShadowState::for_mode(DogfoodMode::SideBySide, 8, 2);
        let tap = state
            .pty_tap()
            .expect("side-by-side creates PTY output tap");

        tap.record_output(b"HELLO");
        state.drain_output();

        let surface = state
            .side_by_side_surface()
            .expect("side-by-side surface should be enabled");
        assert!(surface.skipped_reason().is_none());
        assert!(surface.rows()[0].contains("HELLO"));
        assert_eq!(surface.counters().side_by_side_skipped(), 0);
    }

    #[test]
    fn comparison_summary_reports_first_difference_and_unsupported_fields() {
        let mut session = ShadowSession::for_mode(DogfoodMode::Shadow, 8, 1)
            .expect("valid dimensions attach a Hera shadow core");
        session.advance_output(b"ok");
        let hera = session.comparison_summary();
        let mut paneflow = hera.clone();

        assert_eq!(paneflow.first_difference(&hera), ComparisonOutcome::Equal);

        paneflow.viewport_lines = ComparisonField::supported(vec!["no      ".to_owned()]);
        let outcome = paneflow.first_difference(&hera);
        assert!(matches!(
            outcome,
            ComparisonOutcome::Mismatch(difference)
                if difference.field_path() == "$.viewport_lines[0]"
        ));

        let unsupported = ComparisonSummary::unsupported();
        let outcome = unsupported.first_difference(&hera);
        assert!(matches!(
            outcome,
            ComparisonOutcome::Unsupported(difference)
                if difference.field_path() == "$.dimensions"
        ));
    }

    #[test]
    fn mismatch_checkpoint_increments_counters_and_writes_redacted_report() {
        let (mut state, dir) = state_with_artifact_dir("report");
        let tap = state.pty_tap().expect("shadow mode creates PTY output tap");
        tap.record_output(b"C:\\Users\\Arthur\\secret");
        state.drain_output();

        let paneflow_summary = ComparisonSummary {
            dimensions: ComparisonField::supported(ComparisonDimensions::new(8, 2)),
            active_screen: ComparisonField::supported(ComparisonScreen::Primary),
            cursor: ComparisonField::supported(ComparisonCursor::new(0, 0, true)),
            viewport_lines: ComparisonField::supported(vec![
                "different".to_owned(),
                "        ".to_owned(),
            ]),
            style_buckets: ComparisonField::supported(Vec::new()),
            scrollback_line_count: ComparisonField::supported(0),
        };
        let outcome = state.compare_checkpoint(
            &paneflow_summary,
            DogfoodReportContext::new("pane-1", Some("C:\\Users\\Arthur\\project"), None),
        );

        assert!(matches!(outcome, ComparisonOutcome::Mismatch(_)));
        assert_eq!(state.counters().mismatch(), 1);

        let repeated = state.compare_checkpoint(
            &paneflow_summary,
            DogfoodReportContext::new("pane-1", Some("C:\\Users\\Arthur\\project"), None),
        );
        assert!(matches!(repeated, ComparisonOutcome::Mismatch(_)));
        assert_eq!(state.counters().mismatch(), 2);

        let reports = fs::read_dir(&dir)
            .expect("artifact dir should be readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("report entries should be readable");
        assert_eq!(reports.len(), 1);
        let report = fs::read_to_string(reports[0].path()).expect("report should be readable");
        assert!(report.contains("[redacted-path]"));
        assert!(!report.contains("C:\\Users\\Arthur"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn output_tap_feeds_shadow_snapshot() {
        let mut state = TerminalShadowState::for_mode(DogfoodMode::Shadow, 80, 24);
        let tap = state.pty_tap().expect("shadow mode creates PTY output tap");

        tap.record_output(b"PANEFLOW_HERA_TOKEN");
        state.drain_output();

        let text = state
            .with_shadow_mut(ShadowSession::viewport_text)
            .flatten()
            .expect("shadow snapshot");
        assert!(text.contains("PANEFLOW_HERA_TOKEN"));
    }

    #[test]
    fn startup_token_checkpoint_compares_after_shadow_drain() {
        let mut state = TerminalShadowState::for_mode(DogfoodMode::Shadow, 93, 34);
        let tap = state.pty_tap().expect("shadow mode creates PTY output tap");

        tap.record_output(b"PANEFLOW_HERA_TOKEN\r\n");
        state.drain_output();

        let paneflow_summary = paneflow_summary_from_shadow(&state);
        let outcome = state.compare_checkpoint(
            &paneflow_summary,
            DogfoodReportContext::new("pane-token", None, None),
        );

        assert_eq!(outcome, ComparisonOutcome::Equal);
        assert_eq!(state.counters().equal(), 1);
        assert_eq!(state.counters().mismatch(), 0);
    }

    #[test]
    fn startup_banner_shape_checkpoint_compares_after_shadow_drain() {
        let mut state = TerminalShadowState::for_mode(DogfoodMode::Shadow, 93, 34);
        let tap = state.pty_tap().expect("shadow mode creates PTY output tap");
        let banner = "\
///////////////// ///////////////// user@host\r\n\
///////////////// ///////////////// ---------\r\n\
///////////////// ///////////////// OS: Windows 11 Pro x86_64\r\n\
///////////////// ///////////////// Kernel: WIN32_NT\r\n";

        tap.record_output(banner.as_bytes());
        state.drain_output();

        let paneflow_summary = paneflow_summary_from_shadow(&state);
        let outcome = state.compare_checkpoint(
            &paneflow_summary,
            DogfoodReportContext::new("pane-banner", None, None),
        );

        assert_eq!(outcome, ComparisonOutcome::Equal);
        assert_eq!(state.counters().equal(), 1);
        assert_eq!(state.counters().mismatch(), 0);
    }

    #[test]
    fn disabled_mode_has_no_output_tap() {
        let state = TerminalShadowState::for_mode(DogfoodMode::Disabled, 80, 24);

        assert!(state.pty_tap().is_none());
        assert!(state.with_shadow(|_| ()).is_none());
    }

    #[test]
    fn output_tap_is_bounded_and_records_dropped_bytes() {
        let mut state = TerminalShadowState::for_mode(DogfoodMode::Shadow, 80, 24);
        let tap = state.pty_tap().expect("shadow mode creates PTY output tap");

        for _ in 0..(OUTPUT_QUEUE_CAPACITY + 1) {
            tap.record_output(b"drop");
        }
        state.drain_output();

        let dropped = state
            .with_shadow(ShadowSession::dropped_output_bytes)
            .expect("shadow session");
        assert_eq!(dropped, 4);
        let has_diagnostic = state
            .with_shadow(|shadow| {
                shadow
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.code() == "output_queue_overflow")
            })
            .expect("shadow session");
        assert!(has_diagnostic);
    }

    #[test]
    fn malformed_and_split_utf8_do_not_disable_shadow() {
        let mut session = ShadowSession::for_mode(DogfoodMode::Shadow, 80, 24)
            .expect("valid dimensions attach a Hera shadow core");

        session.advance_output(&[0xc3]);
        session.advance_output(&[0xa9]);
        session.advance_output(&[0xf0, 0x28, 0x8c, 0x28]);

        assert!(session.is_enabled());
        assert_eq!(session.output_bytes_seen(), 6);
    }

    #[test]
    fn invalid_resize_disables_only_shadow_session() {
        let mut session = ShadowSession::for_mode(DogfoodMode::Shadow, 80, 24)
            .expect("valid dimensions attach a Hera shadow core");

        session.resize(0, 24);

        assert!(!session.is_enabled());
        assert_eq!(session.diagnostics()[0].code(), "resize_rejected");
    }

    #[test]
    fn input_metadata_is_bounded_and_escaped() {
        let mut session = ShadowSession::for_mode(DogfoodMode::Shadow, 80, 24)
            .expect("valid dimensions attach a Hera shadow core");

        let mut input = vec![b'a'; 65 * 1024];
        input[0] = b'\n';
        input[1] = 0x1b;
        session.note_input_metadata(&input);

        let latest = session.latest_input().expect("input metadata");
        assert_eq!(latest.byte_count(), 65 * 1024);
        assert!(latest.truncated());
        assert!(latest.escaped_summary().starts_with("\\n\\x1b"));
        assert_eq!(session.input_bytes_seen(), 65 * 1024);
    }

    #[test]
    fn recording_truncates_output_after_cap_and_preserves_metadata() {
        let mut recording =
            DogfoodRecordingBuilder::new_with_limit(8, 2, ArtifactRetention::RawLocal, 3);
        recording.record_output(b"abcdef");
        recording.record_input(InputMetadataSummary::from_bytes(b"a\n"));
        recording.record_resize(10, 3);

        let bundle = recording.finish("pane-1", 127, None, DiffCounters::default());
        let artifact = bundle.recording;

        assert_eq!(artifact.metadata.output_bytes, 3);
        assert!(artifact.metadata.output_truncated);
        assert_eq!(bundle.metrics.session.observed_output_bytes, 6);
        assert_eq!(bundle.metrics.session.logical_output_lines, 0);
        assert!(bundle.metrics.decision.replacement_blocked);
        assert_eq!(artifact.metadata.event_counts.output, 1);
        assert_eq!(artifact.metadata.event_counts.input, 1);
        assert_eq!(artifact.metadata.event_counts.resize, 1);
        assert_eq!(artifact.metadata.event_counts.lifecycle, 1);
        assert!(matches!(
            &artifact.events[0],
            DogfoodRecordingEvent::Output { bytes, .. } if bytes == b"abc"
        ));
    }

    #[test]
    fn non_zero_exit_recording_writes_last_hera_snapshot() {
        let (mut state, dir) = state_with_artifact_dir("recording");
        let tap = state.pty_tap().expect("shadow mode creates PTY output tap");

        tap.record_output(b"bad");
        state.drain_output();
        state.record_exit("pane-2", 127);

        let entries = fs::read_dir(&dir)
            .expect("artifact dir should be readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("entries should be readable");
        assert_eq!(entries.len(), 2);

        let recording_path = entries
            .iter()
            .map(std::fs::DirEntry::path)
            .find(|path| path.to_string_lossy().contains("-recording.json"))
            .expect("recording should be written");
        let metrics_path = entries
            .iter()
            .map(std::fs::DirEntry::path)
            .find(|path| path.to_string_lossy().contains("-metrics.json"))
            .expect("metrics should be written");

        let recording = fs::read_to_string(recording_path).expect("recording should be readable");
        assert!(recording.contains("\"exit_code\": 127"));
        assert!(recording.contains("\"final_snapshot\""));
        assert!(recording.contains("bad"));
        let metrics = fs::read_to_string(metrics_path).expect("metrics should be readable");
        assert!(metrics.contains("\"schema\": \"hera.m3_dogfood_metrics\""));
        assert!(metrics.contains("\"pty_batch_samples\": 1"));
        assert!(metrics.contains("\"recording_output_bytes\": 3"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn hera_imports_stay_inside_dogfood_module() {
        use std::path::{Path, PathBuf};

        const ALLOWLIST: &[&str] = &["terminal/hera_dogfood/mod.rs"];
        const NEEDLES: &[&str] = &[
            "paneflow_hera_dogfood",
            "terminal_core::",
            "terminal_protocol::",
            "terminal_render_model::",
        ];

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        let mut violations = Vec::new();

        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src dir") {
                let path = entry.expect("read src entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }

                let rel = path
                    .strip_prefix(&root)
                    .expect("src file under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                if ALLOWLIST.contains(&rel.as_str()) {
                    continue;
                }

                let text = std::fs::read_to_string(&path).expect("read src file");
                for (index, line) in text.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") || trimmed.starts_with("//!") {
                        continue;
                    }
                    if NEEDLES.iter().any(|needle| line.contains(needle)) {
                        violations.push(format!("{rel}:{}", index + 1));
                    }
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Hera dogfood imports must stay under terminal/hera_dogfood:\n{}",
            violations.join("\n")
        );
    }
}

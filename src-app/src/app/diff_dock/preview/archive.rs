use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, CursorStyle, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px, svg, uniform_list,
};

use super::large_text::PreviewEvent;
use super::{ArchiveFormat, NoPreview, OpenedFile, format_size};
use crate::app::diff_dock::code::load::io_error;
use crate::ui_primitives::AccessibleControlExt as _;

pub(crate) const ARCHIVE_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub(crate) const ARCHIVE_MAX_ENTRIES: usize = 2048;
pub(crate) const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const ARCHIVE_MAX_EXPANSION: u64 = 200;
pub(crate) const BACK_LABEL: &str = "Back";
const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const EOCD_LEN: usize = 22;
const CENTRAL_LEN: usize = 46;
const MAX_COMMENT: usize = 65_535;
const TAR_NAME_MAX_BYTES: u64 = 64 * 1024;
const ROW_HEIGHT: f32 = 26.0;
const SIZE_COLUMN: f32 = 90.0;
const MODIFIED_COLUMN: f32 = 140.0;
const NAME_COLUMN_MIN: f32 = 120.0;
const LOADING_LABEL: &str = "Reading archive…";

pub(crate) fn admit(format: ArchiveFormat, size: u64) -> OpenedFile {
    if size > ARCHIVE_MAX_BYTES {
        return OpenedFile::NoPreview(NoPreview::new(
            Some(size),
            format!(
                "This archive is larger than {}, the preview limit.",
                format_size(ARCHIVE_MAX_BYTES)
            ),
        ));
    }
    OpenedFile::Archive { format, size }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveEntry {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) modified: Option<String>,
    pub(crate) is_dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ArchiveListing {
    pub(crate) entries: Vec<ArchiveEntry>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveError {
    Corrupt,
    Encrypted,
    TimedOut,
    Expanded,
    Cancelled,
    Io(String),
}

impl ArchiveError {
    pub(crate) fn no_preview(&self, size: u64) -> NoPreview {
        let reason = match self {
            Self::Corrupt => {
                "This archive is damaged or not in a format Paneflow can read.".to_string()
            }
            Self::Encrypted => "This archive is encrypted.".to_string(),
            Self::TimedOut => "Reading this archive took longer than 30 seconds.".to_string(),
            Self::Expanded => {
                "This archive expands far past its size, so it was not read to the end.".to_string()
            }
            Self::Cancelled => "Reading this archive was stopped.".to_string(),
            Self::Io(message) => message.clone(),
        };
        NoPreview::new(Some(size), reason)
    }
}

pub(crate) struct ArchiveGuard<'a> {
    deadline: Instant,
    cancel: &'a AtomicBool,
}

impl<'a> ArchiveGuard<'a> {
    pub(crate) fn new(timeout: Duration, cancel: &'a AtomicBool) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            cancel,
        }
    }

    fn check(&self) -> Result<(), ArchiveError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(ArchiveError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(ArchiveError::TimedOut);
        }
        Ok(())
    }
}

pub(crate) fn list_archive(
    path: &Path,
    format: ArchiveFormat,
    guard: &ArchiveGuard<'_>,
) -> Result<ArchiveListing, ArchiveError> {
    let mut file = paneflow_home::open_for_reading(path)
        .map_err(|err| ArchiveError::Io(io_error(&err).message()))?;
    let len = file
        .metadata()
        .map_err(|err| ArchiveError::Io(io_error(&err).message()))?
        .len();
    if len > ARCHIVE_MAX_BYTES {
        return Err(ArchiveError::Corrupt);
    }
    match format {
        ArchiveFormat::Zip => list_zip(&mut file, len, guard),
        ArchiveFormat::Tar => list_tar(file, len, guard),
        ArchiveFormat::TarGz => list_tar(flate2::read::GzDecoder::new(file), len, guard),
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, ArchiveError> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(ArchiveError::Corrupt)
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ArchiveError> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(ArchiveError::Corrupt)
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, ArchiveError> {
    bytes
        .get(at..at + 8)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ArchiveError::Corrupt)
}

fn read_at<R: Read + Seek>(file: &mut R, range: Range<u64>) -> Result<Vec<u8>, ArchiveError> {
    file.seek(SeekFrom::Start(range.start))
        .map_err(|_| ArchiveError::Corrupt)?;
    let mut bytes =
        vec![0u8; usize::try_from(range.end - range.start).map_err(|_| ArchiveError::Corrupt)?];
    file.read_exact(&mut bytes)
        .map_err(|_| ArchiveError::Corrupt)?;
    Ok(bytes)
}

pub(crate) fn dos_timestamp(date: u16, time: u16) -> Option<String> {
    let year = 1980 + u32::from(date >> 9);
    let month = u32::from((date >> 5) & 0x0f);
    let day = u32::from(date & 0x1f);
    let hour = u32::from(time >> 11);
    let minute = u32::from((time >> 5) & 0x3f);
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || minute > 59 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"
    ))
}

fn unix_timestamp(seconds: u64) -> Option<String> {
    let at = jiff::Timestamp::from_second(i64::try_from(seconds).ok()?).ok()?;
    let local = jiff::tz::TimeZone::system().to_datetime(at);
    Some(local.strftime("%Y-%m-%d %H:%M").to_string())
}

fn list_zip<R: Read + Seek>(
    file: &mut R,
    len: u64,
    guard: &ArchiveGuard<'_>,
) -> Result<ArchiveListing, ArchiveError> {
    if len < EOCD_LEN as u64 {
        return Err(ArchiveError::Corrupt);
    }
    let tail_len = len.min((EOCD_LEN + MAX_COMMENT) as u64);
    let tail = read_at(file, len - tail_len..len)?;
    let eocd = (0..=tail.len() - EOCD_LEN)
        .rev()
        .find(|at| u32_at(&tail, *at).ok() == Some(EOCD_SIGNATURE))
        .ok_or(ArchiveError::Corrupt)?;
    let mut count = u64::from(u16_at(&tail, eocd + 10)?);
    let mut cd_size = u64::from(u32_at(&tail, eocd + 12)?);
    let mut cd_offset = u64::from(u32_at(&tail, eocd + 16)?);
    if count == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff {
        let locator = eocd.checked_sub(20).ok_or(ArchiveError::Corrupt)?;
        if u32_at(&tail, locator)? != ZIP64_LOCATOR_SIGNATURE {
            return Err(ArchiveError::Corrupt);
        }
        let record_at = u64_at(&tail, locator + 8)?;
        if record_at.checked_add(56).is_none_or(|end| end > len) {
            return Err(ArchiveError::Corrupt);
        }
        let record = read_at(file, record_at..record_at + 56)?;
        if u32_at(&record, 0)? != ZIP64_EOCD_SIGNATURE {
            return Err(ArchiveError::Corrupt);
        }
        count = u64_at(&record, 32)?;
        cd_size = u64_at(&record, 40)?;
        cd_offset = u64_at(&record, 48)?;
    }
    if cd_offset.checked_add(cd_size).is_none_or(|end| end > len) {
        return Err(ArchiveError::Corrupt);
    }
    let directory = read_at(file, cd_offset..cd_offset + cd_size)?;
    let mut listing = ArchiveListing::default();
    let mut at = 0usize;
    for seen in 0..count {
        if seen % 64 == 0 {
            guard.check()?;
        }
        if listing.entries.len() == ARCHIVE_MAX_ENTRIES {
            listing.truncated = true;
            break;
        }
        if u32_at(&directory, at)? != CENTRAL_SIGNATURE {
            return Err(ArchiveError::Corrupt);
        }
        let flags = u16_at(&directory, at + 8)?;
        if flags & 1 != 0 {
            return Err(ArchiveError::Encrypted);
        }
        let time = u16_at(&directory, at + 12)?;
        let date = u16_at(&directory, at + 14)?;
        let mut size = u64::from(u32_at(&directory, at + 24)?);
        let name_len = usize::from(u16_at(&directory, at + 28)?);
        let extra_len = usize::from(u16_at(&directory, at + 30)?);
        let comment_len = usize::from(u16_at(&directory, at + 32)?);
        let name_at = at + CENTRAL_LEN;
        let name = directory
            .get(name_at..name_at + name_len)
            .ok_or(ArchiveError::Corrupt)?;
        let extra = directory
            .get(name_at + name_len..name_at + name_len + extra_len)
            .ok_or(ArchiveError::Corrupt)?;
        if size == 0xffff_ffff {
            size = zip64_size(extra).unwrap_or(size);
        }
        let name = String::from_utf8_lossy(name).into_owned();
        listing.entries.push(ArchiveEntry {
            is_dir: name.ends_with('/'),
            name,
            size,
            modified: dos_timestamp(date, time),
        });
        at = name_at + name_len + extra_len + comment_len;
    }
    Ok(listing)
}

fn zip64_size(extra: &[u8]) -> Option<u64> {
    let mut at = 0usize;
    while at + 4 <= extra.len() {
        let id = u16_at(extra, at).ok()?;
        let len = usize::from(u16_at(extra, at + 2).ok()?);
        if id == 1 {
            return u64_at(extra, at + 4).ok();
        }
        at += 4 + len;
    }
    None
}

struct GuardedReader<'a, R> {
    inner: R,
    read: u64,
    budget: u64,
    guard: &'a ArchiveGuard<'a>,
    stopped: Option<ArchiveError>,
}

impl<R: Read> Read for GuardedReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Err(stop) = self.guard.check() {
            self.stopped = Some(stop);
            return Err(std::io::Error::other("archive read stopped"));
        }
        let read = self.inner.read(buf)?;
        self.read += read as u64;
        if self.read > self.budget {
            self.stopped = Some(ArchiveError::Expanded);
            return Err(std::io::Error::other("archive expanded past its budget"));
        }
        Ok(read)
    }
}

fn list_tar<R: Read>(
    reader: R,
    len: u64,
    guard: &ArchiveGuard<'_>,
) -> Result<ArchiveListing, ArchiveError> {
    let mut guarded = GuardedReader {
        inner: reader,
        read: 0,
        budget: len.max(1).saturating_mul(ARCHIVE_MAX_EXPANSION),
        guard,
        stopped: None,
    };
    let outcome = tar_entries(&mut guarded);
    match (outcome, guarded.stopped.take()) {
        (_, Some(stop)) => Err(stop),
        (outcome, None) => outcome,
    }
}

fn read_tar_name<R: Read>(entry: &mut R) -> Result<Vec<u8>, ArchiveError> {
    let mut bytes = Vec::new();
    entry
        .take(TAR_NAME_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ArchiveError::Corrupt)?;
    if bytes.len() as u64 > TAR_NAME_MAX_BYTES {
        return Err(ArchiveError::Corrupt);
    }
    Ok(bytes)
}

fn pax_path(records: &[u8]) -> Option<Vec<u8>> {
    tar::PaxExtensions::new(records)
        .filter_map(Result::ok)
        .find(|record| record.key_bytes() == b"path")
        .map(|record| record.value_bytes().to_vec())
}

fn tar_entries<R: Read>(reader: &mut R) -> Result<ArchiveListing, ArchiveError> {
    let mut archive = tar::Archive::new(reader);
    let mut listing = ArchiveListing::default();
    let mut long_name: Option<Vec<u8>> = None;
    let entries = archive
        .entries()
        .map_err(|_| ArchiveError::Corrupt)?
        .raw(true);
    for entry in entries {
        let mut entry = entry.map_err(|_| ArchiveError::Corrupt)?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname() {
            let mut name = read_tar_name(&mut entry)?;
            while name.last() == Some(&0) {
                name.pop();
            }
            long_name = Some(name);
            continue;
        }
        if kind.is_pax_local_extensions() {
            if let Some(path) = pax_path(&read_tar_name(&mut entry)?) {
                long_name = Some(path);
            }
            continue;
        }
        if kind.is_gnu_longlink() || kind.is_pax_global_extensions() {
            continue;
        }
        if listing.entries.len() == ARCHIVE_MAX_ENTRIES {
            listing.truncated = true;
            break;
        }
        let header = entry.header();
        let name = long_name
            .take()
            .unwrap_or_else(|| entry.path_bytes().into_owned());
        let name = String::from_utf8_lossy(&name).into_owned();
        let is_dir = kind.is_dir() || name.ends_with('/');
        listing.entries.push(ArchiveEntry {
            name,
            size: if is_dir {
                0
            } else {
                header.size().unwrap_or(0)
            },
            modified: header.mtime().ok().and_then(unix_timestamp),
            is_dir,
        });
    }
    Ok(listing)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveRow {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) size: Option<u64>,
    pub(crate) modified: Option<String>,
}

pub(crate) fn is_unsafe_name(name: &str) -> bool {
    name.starts_with('/')
        || name.starts_with('\\')
        || name.as_bytes().get(1) == Some(&b':')
        || name.split(['/', '\\']).any(|part| part == "..")
}

fn segments(name: &str) -> Vec<&str> {
    name.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

pub(crate) fn rows_at(entries: &[ArchiveEntry], dir: &[String]) -> Vec<ArchiveRow> {
    let mut rows: Vec<ArchiveRow> = Vec::new();
    for entry in entries {
        if is_unsafe_name(&entry.name) {
            if dir.is_empty() {
                rows.push(ArchiveRow {
                    name: entry.name.clone(),
                    is_dir: false,
                    size: (!entry.is_dir).then_some(entry.size),
                    modified: entry.modified.clone(),
                });
            }
            continue;
        }
        let parts = segments(&entry.name);
        if parts.len() <= dir.len() || parts.iter().zip(dir).any(|(part, want)| part != want) {
            continue;
        }
        let name = parts[dir.len()];
        let leaf = parts.len() == dir.len() + 1;
        let is_dir = !leaf || entry.is_dir;
        if let Some(known) = rows
            .iter_mut()
            .find(|row| row.name == name && row.is_dir == is_dir)
        {
            if leaf && entry.is_dir {
                known.modified = entry.modified.clone();
            }
            continue;
        }
        rows.push(ArchiveRow {
            name: name.to_string(),
            is_dir,
            size: (!is_dir).then_some(entry.size),
            modified: if leaf { entry.modified.clone() } else { None },
        });
    }
    rows.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

pub(crate) fn truncated_banner() -> String {
    format!(
        "Showing the first {} entries.",
        super::group_digits(ARCHIVE_MAX_ENTRIES)
    )
}

enum ArchiveState {
    Loading,
    Ready(Arc<ArchiveListing>),
    Failed,
}

pub(crate) struct ArchiveView {
    path: PathBuf,
    format: ArchiveFormat,
    size: u64,
    focus: FocusHandle,
    state: ArchiveState,
    dir: Vec<String>,
    cancel: Arc<AtomicBool>,
    generation: u64,
}

impl EventEmitter<PreviewEvent> for ArchiveView {}

impl Drop for ArchiveView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ArchiveView {
    pub(crate) fn new(
        path: PathBuf,
        format: ArchiveFormat,
        size: u64,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            path,
            format,
            size,
            focus,
            state: ArchiveState::Loading,
            dir: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
        };
        view.start_listing(cx);
        view
    }

    pub(crate) fn reload(&mut self, size: u64, cx: &mut Context<Self>) {
        self.size = size;
        self.start_listing(cx);
    }

    fn start_listing(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let path = self.path.clone();
        let format = self.format;
        let cancel = self.cancel.clone();
        let work = cx.background_spawn(async move {
            let guard = ArchiveGuard::new(ARCHIVE_TIMEOUT, &cancel);
            list_archive(&path, format, &guard)
        });
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |view: &mut Self, cx| {
                if view.generation != generation {
                    return;
                }
                match outcome {
                    Ok(listing) => {
                        view.state = ArchiveState::Ready(Arc::new(listing));
                        let rows = view.rows();
                        if !view.dir.is_empty() && rows.is_empty() {
                            view.dir.clear();
                        }
                    }
                    Err(ArchiveError::Cancelled) => {}
                    Err(error) => {
                        view.state = ArchiveState::Failed;
                        cx.emit(PreviewEvent::Refused(error.no_preview(view.size)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn rows(&self) -> Vec<ArchiveRow> {
        match &self.state {
            ArchiveState::Ready(listing) => rows_at(&listing.entries, &self.dir),
            _ => Vec::new(),
        }
    }

    pub(crate) fn enter(&mut self, name: String, cx: &mut Context<Self>) {
        self.dir.push(name);
        cx.notify();
    }

    pub(crate) fn back(&mut self, cx: &mut Context<Self>) {
        self.dir.pop();
        cx.notify();
    }

    pub(crate) fn go_to(&mut self, depth: usize, cx: &mut Context<Self>) {
        self.dir.truncate(depth);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn dir(&self) -> &[String] {
        &self.dir
    }

    #[cfg(test)]
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self.state, ArchiveState::Loading)
    }

    fn render_crumbs(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let root = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut row = div()
            .flex_none()
            .h(px(30.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .border_b_1()
            .border_color(ui.border)
            .overflow_hidden()
            .text_size(crate::ui_primitives::BODY);
        let labels = std::iter::once(root).chain(self.dir.iter().cloned());
        let last = self.dir.len();
        for (depth, label) in labels.enumerate() {
            if depth > 0 {
                row = row.child(div().text_color(ui.muted).child("/"));
            }
            let current = depth == last;
            row = row.child(
                div()
                    .id(SharedString::from(format!("archive-crumb-{depth}")))
                    .flex_none()
                    .px(px(4.))
                    .rounded(px(4.))
                    .whitespace_nowrap()
                    .text_color(if current { ui.text } else { ui.muted })
                    .when(!current, |crumb| {
                        crumb
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(ui.subtle))
                            .on_activate(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.go_to(depth, cx);
                            }))
                    })
                    .child(SharedString::from(label)),
            );
        }
        row.into_any_element()
    }
}

fn cell(text: impl Into<SharedString>, width: Option<f32>, color: gpui::Hsla) -> AnyElement {
    let trailing = width == Some(SIZE_COLUMN);
    let base = div()
        .h_full()
        .flex()
        .items_center()
        .px(px(8.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(color);
    match width {
        Some(width) => base
            .flex_shrink_1()
            .min_w_0()
            .w(px(width))
            .when(trailing, |cell| cell.justify_end()),
        None => base.flex_1().min_w(px(NAME_COLUMN_MIN)),
    }
    .child(text.into())
    .into_any_element()
}

impl Render for ArchiveView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let ArchiveState::Ready(listing) = &self.state else {
            let label = if matches!(self.state, ArchiveState::Loading) {
                LOADING_LABEL
            } else {
                ""
            };
            return div()
                .size_full()
                .track_focus(&self.focus)
                .child(crate::ui_primitives::panel_empty_state(
                    ui,
                    Some("icons/dock/loader-circle.svg"),
                    None,
                    label,
                    true,
                ))
                .into_any_element();
        };
        let truncated = listing.truncated;
        let rows = Arc::new(self.rows());
        let has_back = !self.dir.is_empty();
        let count = rows.len() + usize::from(has_back);
        let crumbs = self.render_crumbs(ui, cx);
        let entity = cx.entity();
        let list_rows = rows.clone();
        let list = uniform_list("archive-rows", count, move |range: Range<usize>, _, _| {
            range
                .map(|index| {
                    let hover = ui.subtle;
                    if has_back && index == 0 {
                        let entity = entity.clone();
                        return div()
                            .id("archive-back")
                            .h(px(ROW_HEIGHT))
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(hover))
                            .on_activate(move |_, _, cx| {
                                entity.update(cx, |view, cx| view.back(cx));
                            })
                            .child(
                                svg()
                                    .size(px(14.))
                                    .path("icons/palette/arrow-left-02.svg")
                                    .text_color(ui.muted),
                            )
                            .child(div().text_color(ui.text).child(BACK_LABEL))
                            .into_any_element();
                    }
                    let row = &list_rows[index - usize::from(has_back)];
                    let name = row.name.clone();
                    let is_dir = row.is_dir;
                    let entity = entity.clone();
                    div()
                        .id(SharedString::from(format!("archive-row-{index}")))
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .when(is_dir, |el| {
                            el.cursor(CursorStyle::PointingHand)
                                .hover(move |style| style.bg(hover))
                                .on_activate(move |_, _, cx| {
                                    let name = name.clone();
                                    entity.update(cx, |view, cx| view.enter(name, cx));
                                })
                        })
                        .child(
                            div().flex_none().pl(px(8.)).child(
                                svg()
                                    .size(px(14.))
                                    .path(if is_dir {
                                        "icons/dock/arrow-right-01.svg"
                                    } else {
                                        "icons/palette/file-01.svg"
                                    })
                                    .text_color(ui.muted),
                            ),
                        )
                        .child(cell(row.name.clone(), None, ui.text))
                        .child(cell(
                            row.size.map(format_size).unwrap_or_default(),
                            Some(SIZE_COLUMN),
                            ui.muted,
                        ))
                        .child(cell(
                            row.modified.clone().unwrap_or_default(),
                            Some(MODIFIED_COLUMN),
                            ui.muted,
                        ))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .min_h_0()
        .w_full();
        let header = div()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(ui.border)
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(div().flex_none().w(px(22.)))
            .child(cell("Name", None, ui.muted))
            .child(cell("Size", Some(SIZE_COLUMN), ui.muted))
            .child(cell("Modified", Some(MODIFIED_COLUMN), ui.muted));
        div()
            .id("archive-view")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_size(crate::ui_primitives::BODY)
            .children(truncated.then(|| {
                div()
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .text_xs()
                    .border_b_1()
                    .border_color(ui.border)
                    .bg(ui.overlay)
                    .text_color(ui.muted)
                    .child(truncated_banner())
            }))
            .child(crumbs)
            .child(header)
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_fixture(entries: &[(&str, &[u8], u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, flags) in entries {
            let offset = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0]);
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&0x6000u16.to_le_bytes());
            out.extend_from_slice(&0x5951u16.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0]);
            central.extend_from_slice(&flags.to_le_bytes());
            central.extend_from_slice(&[0, 0]);
            central.extend_from_slice(&0x6000u16.to_le_bytes());
            central.extend_from_slice(&0x5951u16.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn list(bytes: &[u8], format: ArchiveFormat) -> Result<ArchiveListing, ArchiveError> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fixture");
        std::fs::write(&path, bytes).expect("write");
        let cancel = AtomicBool::new(false);
        list_archive(&path, format, &ArchiveGuard::new(ARCHIVE_TIMEOUT, &cancel))
    }

    #[test]
    fn a_zip_lists_names_sizes_and_dates_and_extracts_nothing() {
        let bytes = zip_fixture(&[
            ("src/", b"", 0),
            ("src/main.rs", b"fn main() {}\n", 0),
            ("README.md", b"# hi\n", 0),
        ]);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pkg.zip");
        std::fs::write(&path, &bytes).expect("write");
        let cancel = AtomicBool::new(false);
        let listing = list_archive(
            &path,
            ArchiveFormat::Zip,
            &ArchiveGuard::new(ARCHIVE_TIMEOUT, &cancel),
        )
        .expect("list");
        assert_eq!(listing.entries.len(), 3);
        assert_eq!(listing.entries[1].name, "src/main.rs");
        assert_eq!(listing.entries[1].size, 13);
        assert_eq!(
            listing.entries[1].modified.as_deref(),
            Some("2024-10-17 12:00")
        );
        assert!(listing.entries[0].is_dir);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("pkg.zip")]);
    }

    #[test]
    fn the_listing_shows_folders_first_and_walks_in_and_out() {
        let entries = list(
            &zip_fixture(&[
                ("b.txt", b"b", 0),
                ("src/lib/mod.rs", b"x", 0),
                ("src/main.rs", b"y", 0),
                ("A.txt", b"a", 0),
            ]),
            ArchiveFormat::Zip,
        )
        .expect("list")
        .entries;
        let root = rows_at(&entries, &[]);
        let names: Vec<_> = root.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["src", "A.txt", "b.txt"]);
        assert!(root[0].is_dir && root[0].size.is_none());
        let src = rows_at(&entries, &["src".to_string()]);
        let names: Vec<_> = src.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["lib", "main.rs"]);
    }

    #[test]
    fn an_encrypted_or_damaged_zip_is_refused_with_its_reason() {
        let encrypted = zip_fixture(&[("secret.txt", b"xx", 1)]);
        assert_eq!(
            list(&encrypted, ArchiveFormat::Zip).unwrap_err(),
            ArchiveError::Encrypted
        );
        assert_eq!(
            list(b"PK\x03\x04 not a real archive", ArchiveFormat::Zip).unwrap_err(),
            ArchiveError::Corrupt
        );
        assert!(
            ArchiveError::Encrypted
                .no_preview(10)
                .reason
                .expect("reason")
                .contains("encrypted")
        );
        assert!(matches!(
            admit(ArchiveFormat::Zip, ARCHIVE_MAX_BYTES + 1),
            OpenedFile::NoPreview(_)
        ));
    }

    #[test]
    fn the_listing_stops_at_two_thousand_and_forty_eight_entries() {
        let names: Vec<String> = (0..ARCHIVE_MAX_ENTRIES + 5)
            .map(|n| format!("f{n}"))
            .collect();
        let entries: Vec<(&str, &[u8], u16)> = names
            .iter()
            .map(|name| (name.as_str(), &b""[..], 0))
            .collect();
        let listing = list(&zip_fixture(&entries), ArchiveFormat::Zip).expect("list");
        assert_eq!(listing.entries.len(), ARCHIVE_MAX_ENTRIES);
        assert!(listing.truncated);
        assert!(truncated_banner().contains("2,048"));
    }

    #[test]
    fn hostile_names_are_shown_as_is_and_never_become_folders() {
        let entries = list(
            &zip_fixture(&[
                ("../../etc/passwd", b"x", 0),
                ("/abs/path", b"y", 0),
                ("ok/file", b"z", 0),
            ]),
            ArchiveFormat::Zip,
        )
        .expect("list")
        .entries;
        let root = rows_at(&entries, &[]);
        let names: Vec<_> = root.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["ok", "../../etc/passwd", "/abs/path"]);
        assert!(
            root.iter()
                .filter(|row| row.name != "ok")
                .all(|row| !row.is_dir)
        );
    }

    fn tar_fixture(gzip: bool) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in [("pkg/a.txt", &b"alpha"[..]), ("pkg/b/c.txt", b"gamma!")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mtime(1_700_000_000);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, data)
                .expect("append");
        }
        let bytes = builder.into_inner().expect("tar");
        if !gzip {
            return bytes;
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &bytes).expect("gzip");
        encoder.finish().expect("gzip")
    }

    #[test]
    fn tar_and_tar_gz_list_their_headers() {
        for (gzip, format) in [(false, ArchiveFormat::Tar), (true, ArchiveFormat::TarGz)] {
            let listing = list(&tar_fixture(gzip), format).expect("list");
            let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, vec!["pkg/a.txt", "pkg/b/c.txt"], "{format:?}");
            assert_eq!(listing.entries[1].size, 6);
            assert!(listing.entries[0].modified.is_some());
        }
        assert_eq!(
            list(b"\x1f\x8b\x08garbage", ArchiveFormat::TarGz).unwrap_err(),
            ArchiveError::Corrupt
        );
    }

    #[test]
    fn a_long_tar_name_is_listed_and_an_oversized_name_record_is_refused() {
        let long = format!("{}/leaf.txt", "deep".repeat(60));
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, &long, &b"hi"[..])
            .expect("append");
        let listing = list(&builder.into_inner().expect("tar"), ArchiveFormat::Tar).expect("list");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, long);
        assert_eq!(listing.entries[0].size, 2);

        let record = vec![b'a'; TAR_NAME_MAX_BYTES as usize + 512];
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::GNULongName);
        header.set_path("././@LongLink").expect("path");
        header.set_size(record.len() as u64);
        header.set_cksum();
        builder.append(&header, &record[..]).expect("append");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &builder.into_inner().expect("tar")).expect("gzip");
        assert_eq!(
            list(&encoder.finish().expect("gzip"), ArchiveFormat::TarGz).unwrap_err(),
            ArchiveError::Corrupt
        );
    }

    #[test]
    fn a_listing_past_its_deadline_or_cancelled_stops() {
        let bytes = tar_fixture(true);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("slow.tgz");
        std::fs::write(&path, &bytes).expect("write");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            list_archive(
                &path,
                ArchiveFormat::TarGz,
                &ArchiveGuard::new(Duration::ZERO, &cancel)
            )
            .unwrap_err(),
            ArchiveError::TimedOut
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            list_archive(
                &path,
                ArchiveFormat::TarGz,
                &ArchiveGuard::new(ARCHIVE_TIMEOUT, &cancel)
            )
            .unwrap_err(),
            ArchiveError::Cancelled
        );
    }

    #[test]
    fn dos_dates_read_as_calendar_dates() {
        assert_eq!(
            dos_timestamp(0x5951, 0x6000).as_deref(),
            Some("2024-10-17 12:00")
        );
        assert_eq!(dos_timestamp(0, 0), None);
    }
}

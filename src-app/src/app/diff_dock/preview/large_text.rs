use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{
    AnyElement, AppContext, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, ScrollDelta, ScrollEvent, SharedString, Styled, Window, canvas,
    div, prelude::FluentBuilder, px,
};

use super::NoPreview;
use super::{format_size, group_digits};
use crate::app::diff_dock::code::element::{CODE_FONT_SIZE, CODE_ROW_HEIGHT, code_font};
use crate::app::diff_dock::code::load::{CodeLoadError, io_error};
use crate::app::diff_dock::code::view::{
    CODE_KEY_CONTEXT, CeCopy, CeDocEnd, CeDocStart, CeDown, CePageDown, CePageUp, CeSelectAll,
    CeSelectDown, CeSelectUp, CeUp,
};

pub(crate) const LARGE_TEXT_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const LARGE_TEXT_MAX_LINES: usize = 5_000_000;
pub(crate) const LARGE_TEXT_MAX_LINE_BYTES: u64 = 1024 * 1024;
pub(crate) const FIRST_SCREEN_BYTES: usize = 16 * 1024;
pub(crate) const PAGE_LINES: usize = 32;
pub(crate) const PAGES_KEPT: usize = 12;
pub(crate) const COPY_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const LARGE_FILE_BANNER: &str = "Large file: read-only, no syntax colors";
pub(crate) const INDEXING_LABEL: &str = "Indexing…";
pub(crate) const COPY_REFUSED: &str = "Selection is larger than 16 MB, so it was not copied.";
const INDEX_CHUNK_BYTES: usize = 1024 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(120);
const TAB_WIDTH: usize = 4;
const GUTTER_PAD: f32 = 8.0;
const TEXT_PAD: f32 = 6.0;
const SCROLLBAR_W: f32 = 6.0;
const SCROLLBAR_MIN_THUMB: f32 = 24.0;
const WHEEL_LINES_PER_TICK: f64 = 3.0;

pub(crate) fn refusal_for_size(size: u64) -> Option<NoPreview> {
    (size > LARGE_TEXT_MAX_BYTES).then(|| IndexError::TooLarge.no_preview(Some(size)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IndexError {
    TooLarge,
    TooManyLines,
    LineTooLong,
    Binary,
    Cancelled,
    Io(CodeLoadError),
}

impl IndexError {
    pub(crate) fn no_preview(&self, size: Option<u64>) -> NoPreview {
        let reason = match self {
            Self::TooLarge => format!(
                "This file is larger than {}, the limit for showing text.",
                format_size(LARGE_TEXT_MAX_BYTES)
            ),
            Self::TooManyLines => "This file has more than 5,000,000 lines.".to_string(),
            Self::LineTooLong => "A line in this file is longer than 1 MB.".to_string(),
            Self::Binary => {
                "This file contains a null byte, so it can't be shown as text.".to_string()
            }
            Self::Cancelled => "Reading this file was stopped.".to_string(),
            Self::Io(err) => err.message(),
        };
        NoPreview::new(size, reason)
    }
}

#[derive(Debug)]
pub(crate) struct LineIndex {
    starts: Vec<u32>,
    indexed: u64,
    size: u64,
    complete: bool,
}

impl Default for LineIndex {
    fn default() -> Self {
        Self {
            starts: vec![0],
            indexed: 0,
            size: 0,
            complete: false,
        }
    }
}

impl LineIndex {
    pub(crate) fn line_count(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn line_bytes(&self, lines: Range<usize>) -> Option<Range<u64>> {
        if lines.start >= lines.end || lines.end > self.line_count() {
            return None;
        }
        Some(u64::from(self.starts[lines.start])..u64::from(self.starts[lines.end]))
    }

    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        self.starts.capacity() * std::mem::size_of::<u32>()
    }

    fn finish(&mut self) {
        let last = self.starts.last().copied().map_or(0, u64::from);
        if self.indexed > last {
            self.starts.push(self.indexed as u32);
        }
        self.size = self.indexed;
        self.starts.shrink_to_fit();
        self.complete = true;
    }
}

#[derive(Default)]
pub(crate) struct LineScanner {
    pos: u64,
    line_start: u64,
}

impl LineScanner {
    pub(crate) fn scan(&mut self, chunk: &[u8], index: &mut LineIndex) -> Result<(), IndexError> {
        if self.pos + chunk.len() as u64 > LARGE_TEXT_MAX_BYTES {
            return Err(IndexError::TooLarge);
        }
        for (offset, &byte) in chunk.iter().enumerate() {
            match byte {
                0 => return Err(IndexError::Binary),
                b'\n' => {
                    let next = self.pos + offset as u64 + 1;
                    if next - 1 - self.line_start > LARGE_TEXT_MAX_LINE_BYTES {
                        return Err(IndexError::LineTooLong);
                    }
                    if index.starts.len() > LARGE_TEXT_MAX_LINES {
                        return Err(IndexError::TooManyLines);
                    }
                    index.starts.push(next as u32);
                    self.line_start = next;
                }
                _ => {}
            }
        }
        self.pos += chunk.len() as u64;
        if self.pos - self.line_start > LARGE_TEXT_MAX_LINE_BYTES {
            return Err(IndexError::LineTooLong);
        }
        index.indexed = self.pos;
        Ok(())
    }

    pub(crate) fn finish(&self, index: &mut LineIndex) -> Result<(), IndexError> {
        let trailing = self.pos > self.line_start;
        if trailing && index.starts.len() > LARGE_TEXT_MAX_LINES {
            return Err(IndexError::TooManyLines);
        }
        index.finish();
        Ok(())
    }
}

fn lock(index: &Mutex<LineIndex>) -> MutexGuard<'_, LineIndex> {
    index.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn index_file(
    path: &Path,
    index: &Mutex<LineIndex>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(),
) -> Result<(), IndexError> {
    let mut file =
        paneflow_home::open_for_reading(path).map_err(|err| IndexError::Io(io_error(&err)))?;
    let meta = file
        .metadata()
        .map_err(|err| IndexError::Io(io_error(&err)))?;
    if meta.len() > LARGE_TEXT_MAX_BYTES {
        return Err(IndexError::TooLarge);
    }
    let mut scanner = LineScanner::default();
    let mut buffer = vec![0u8; INDEX_CHUNK_BYTES];
    let mut first = true;
    let mut last_progress = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(IndexError::Cancelled);
        }
        let want = if first {
            FIRST_SCREEN_BYTES
        } else {
            INDEX_CHUNK_BYTES
        };
        let read = file
            .read(&mut buffer[..want])
            .map_err(|err| IndexError::Io(io_error(&err)))?;
        if read == 0 {
            break;
        }
        scanner.scan(&buffer[..read], &mut lock(index))?;
        if first || last_progress.elapsed() >= PROGRESS_EVERY {
            progress();
            last_progress = Instant::now();
        }
        first = false;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(IndexError::Cancelled);
    }
    scanner.finish(&mut lock(index))?;
    progress();
    Ok(())
}

pub(crate) fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + TAB_WIDTH);
    let mut column = 0usize;
    for ch in line.chars() {
        if ch == '\t' {
            let pad = TAB_WIDTH - column % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', pad));
            column += pad;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    out
}

pub(crate) fn read_bytes(path: &Path, range: Range<u64>) -> Result<Vec<u8>, CodeLoadError> {
    let mut file = paneflow_home::open_for_reading(path).map_err(|err| io_error(&err))?;
    file.seek(SeekFrom::Start(range.start))
        .map_err(|err| io_error(&err))?;
    let len = range.end.saturating_sub(range.start);
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(len)
        .read_to_end(&mut bytes)
        .map_err(|err| io_error(&err))?;
    Ok(bytes)
}

pub(crate) fn decode_lines(bytes: &[u8], first_line: usize, count: usize) -> Vec<SharedString> {
    let bytes = if first_line == 0 {
        bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
    } else {
        bytes
    };
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes
        .split(|byte| *byte == b'\n')
        .take(count)
        .map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            SharedString::from(expand_tabs(&String::from_utf8_lossy(line)))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct Page {
    pub(crate) index: usize,
    pub(crate) lines: Vec<SharedString>,
}

#[derive(Default)]
pub(crate) struct PageCache {
    pages: VecDeque<Page>,
    in_flight: HashSet<usize>,
}

impl PageCache {
    pub(crate) fn line(&mut self, line: usize) -> Option<SharedString> {
        let index = line / PAGE_LINES;
        let position = self.pages.iter().position(|page| page.index == index)?;
        if position > 0
            && let Some(page) = self.pages.remove(position)
        {
            self.pages.push_front(page);
        }
        self.pages
            .front()
            .and_then(|page| page.lines.get(line % PAGE_LINES).cloned())
    }

    pub(crate) fn page_len(&self, index: usize) -> Option<usize> {
        self.pages
            .iter()
            .find(|page| page.index == index)
            .map(|page| page.lines.len())
    }

    pub(crate) fn insert(&mut self, page: Page) {
        self.in_flight.remove(&page.index);
        self.pages.retain(|known| known.index != page.index);
        self.pages.push_front(page);
        self.pages.truncate(PAGES_KEPT);
    }

    pub(crate) fn begin(&mut self, index: usize) -> bool {
        self.in_flight.insert(index)
    }

    pub(crate) fn clear(&mut self) {
        self.pages.clear();
        self.in_flight.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pages.len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PreviewEvent {
    Refused(NoPreview),
    Toast(SharedString),
}

#[derive(Clone, Copy, Debug)]
struct ScrollDrag {
    start_y: f32,
    start_top: f64,
}

pub(crate) struct LargeTextView {
    path: PathBuf,
    focus: FocusHandle,
    size: u64,
    index: Arc<Mutex<LineIndex>>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    pages: PageCache,
    top: f64,
    h_col: usize,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    selection: Option<(usize, usize)>,
    selecting: bool,
    scroll_drag: Option<ScrollDrag>,
    char_w: f32,
}

impl EventEmitter<PreviewEvent> for LargeTextView {}

impl Drop for LargeTextView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl LargeTextView {
    pub(crate) fn new(
        path: PathBuf,
        size: u64,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            path,
            focus,
            size,
            index: Arc::new(Mutex::new(LineIndex::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            pages: PageCache::default(),
            top: 0.0,
            h_col: 0,
            viewport: Rc::new(Cell::new(Bounds::default())),
            selection: None,
            selecting: false,
            scroll_drag: None,
            char_w: 0.0,
        };
        view.start_index(cx);
        view
    }

    pub(crate) fn reindex(&mut self, cx: &mut Context<Self>) {
        self.start_index(cx);
        cx.notify();
    }

    pub(crate) fn line_count(&self) -> usize {
        lock(&self.index).line_count()
    }

    #[cfg(test)]
    pub(crate) fn is_indexed(&self) -> bool {
        lock(&self.index).is_complete()
    }

    #[cfg(test)]
    pub(crate) fn top_line(&self) -> f64 {
        self.top
    }

    #[cfg(test)]
    pub(crate) fn cached_pages(&self) -> usize {
        self.pages.len()
    }

    #[cfg(test)]
    pub(crate) fn retained_index_bytes(&self) -> usize {
        lock(&self.index).retained_bytes()
    }

    #[cfg(test)]
    pub(crate) fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(crate) fn status_label(&self) -> String {
        let index = lock(&self.index);
        if index.is_complete() {
            let count = index.line_count();
            format!(
                "{} {} · {}",
                group_digits(count),
                if count == 1 { "line" } else { "lines" },
                format_size(index.size)
            )
        } else {
            INDEXING_LABEL.to_string()
        }
    }

    pub(crate) fn scroll_to_line(&mut self, line: f64, cx: &mut Context<Self>) {
        self.top = line.max(0.0);
        self.clamp_top_to_known();
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn select_lines(&mut self, anchor: usize, head: usize) {
        self.selection = Some((anchor, head));
    }

    pub(crate) fn selected_lines(&self) -> Option<Range<usize>> {
        let (anchor, head) = self.selection?;
        Some(anchor.min(head)..anchor.max(head) + 1)
    }

    fn start_index(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.index = Arc::new(Mutex::new(LineIndex::default()));
        self.generation = self.generation.wrapping_add(1);
        self.pages.clear();
        let generation = self.generation;
        let path = self.path.clone();
        let index = self.index.clone();
        let cancel = self.cancel.clone();
        let (progress_tx, mut progress_rx) = mpsc::unbounded::<()>();
        let work = cx.background_spawn(async move {
            index_file(&path, &index, &cancel, || {
                let _ = progress_tx.unbounded_send(());
            })
        });
        cx.spawn(async move |this, cx| {
            while progress_rx.next().await.is_some() {
                let alive = this.update(cx, |view: &mut Self, cx| {
                    if view.generation == generation {
                        view.on_index_progress();
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            let outcome = work.await;
            let _ = this.update(cx, |view: &mut Self, cx| {
                if view.generation != generation {
                    return;
                }
                match outcome {
                    Ok(()) => view.on_index_progress(),
                    Err(IndexError::Cancelled) => {}
                    Err(error) => {
                        let size = Some(view.size);
                        cx.emit(PreviewEvent::Refused(error.no_preview(size)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_index_progress(&mut self) {
        let index = lock(&self.index);
        let count = index.line_count();
        let complete = index.is_complete();
        drop(index);
        let stale: Vec<usize> = (0..count.div_ceil(PAGE_LINES))
            .filter(|page| {
                self.pages.page_len(*page).is_some_and(|len| {
                    len < PAGE_LINES && (complete || count > page * PAGE_LINES + len)
                })
            })
            .collect();
        for page in stale {
            self.pages.pages.retain(|known| known.index != page);
        }
        if complete {
            self.clamp_top();
            if let Some((anchor, head)) = self.selection {
                let last = count.saturating_sub(1);
                self.selection = (count > 0).then_some((anchor.min(last), head.min(last)));
            }
        }
    }

    fn visible_rows(&self) -> f64 {
        let height = f32::from(self.viewport.get().size.height);
        f64::from((height / CODE_ROW_HEIGHT).max(1.0))
    }

    fn max_top(&self) -> f64 {
        (self.line_count() as f64 - self.visible_rows()).max(0.0)
    }

    fn clamp_top(&mut self) {
        if self.is_complete() {
            self.clamp_top_to_known();
        } else if self.top < 0.0 || !self.top.is_finite() {
            self.top = 0.0;
        }
    }

    fn clamp_top_to_known(&mut self) {
        let max = self.max_top();
        if self.top > max || !self.top.is_finite() {
            self.top = max;
        }
        if self.top < 0.0 {
            self.top = 0.0;
        }
    }

    fn is_complete(&self) -> bool {
        lock(&self.index).is_complete()
    }

    fn request_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let (range, count) = {
            let index = lock(&self.index);
            let first = page * PAGE_LINES;
            let end = (first + PAGE_LINES).min(index.line_count());
            if first >= end {
                return;
            }
            let Some(range) = index.line_bytes(first..end) else {
                return;
            };
            (range, end - first)
        };
        if !self.pages.begin(page) {
            return;
        }
        let path = self.path.clone();
        let generation = self.generation;
        let read = cx.background_spawn(async move {
            read_bytes(&path, range).map(|bytes| decode_lines(&bytes, page * PAGE_LINES, count))
        });
        cx.spawn(async move |this, cx| {
            let lines = read.await;
            let _ = this.update(cx, |view: &mut Self, cx| {
                if view.generation != generation {
                    return;
                }
                let lines = lines.unwrap_or_else(|_| vec![SharedString::default(); count]);
                view.pages.insert(Page { index: page, lines });
                cx.notify();
            });
        })
        .detach();
    }

    fn line_text(&mut self, line: usize, cx: &mut Context<Self>) -> Option<SharedString> {
        let text = self.pages.line(line);
        if text.is_none() {
            self.pages
                .pages
                .retain(|page| page.index != line / PAGE_LINES);
            self.request_page(line / PAGE_LINES, cx);
        }
        text
    }

    fn scroll_by(&mut self, lines: f64, cx: &mut Context<Self>) {
        self.top += lines;
        self.clamp_top_to_known();
        cx.notify();
    }

    fn on_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let (dx, dy) = match event.delta {
            ScrollDelta::Pixels(delta) => (
                f64::from(f32::from(delta.x)) / f64::from(self.char_w.max(1.0)),
                f64::from(f32::from(delta.y)) / f64::from(CODE_ROW_HEIGHT),
            ),
            ScrollDelta::Lines(delta) => (
                f64::from(delta.x) * WHEEL_LINES_PER_TICK,
                f64::from(delta.y) * WHEEL_LINES_PER_TICK,
            ),
        };
        if dy != 0.0 {
            self.scroll_by(-dy, cx);
        }
        if dx != 0.0 {
            let next = (self.h_col as f64 - dx).max(0.0);
            self.h_col = next.round() as usize;
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn line_at(&self, y: Pixels) -> Option<usize> {
        let bounds = self.viewport.get();
        let local = f64::from(f32::from(y - bounds.origin.y)) / f64::from(CODE_ROW_HEIGHT);
        let count = self.line_count();
        if count == 0 {
            return None;
        }
        let line = (self.top + local).floor().max(0.0) as usize;
        Some(line.min(count - 1))
    }

    fn scrollbar_geometry(&self) -> Option<(f32, f32, f32)> {
        let height = f32::from(self.viewport.get().size.height);
        let count = self.line_count() as f64;
        let visible = self.visible_rows();
        if height <= 0.0 || count <= visible {
            return None;
        }
        let thumb = ((visible / count) as f32 * height)
            .max(SCROLLBAR_MIN_THUMB)
            .min(height);
        let progress = (self.top / self.max_top().max(1.0)).clamp(0.0, 1.0) as f32;
        Some((height, thumb, progress * (height - thumb)))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let bounds = self.viewport.get();
        let right = bounds.origin.x + bounds.size.width;
        if event.position.x >= right - px(SCROLLBAR_W + 4.0)
            && let Some((height, thumb, thumb_top)) = self.scrollbar_geometry()
        {
            let y = f32::from(event.position.y - bounds.origin.y);
            if y < thumb_top || y > thumb_top + thumb {
                let progress = ((y - thumb / 2.0) / (height - thumb).max(1.0)).clamp(0.0, 1.0);
                self.top = f64::from(progress) * self.max_top();
            }
            self.scroll_drag = Some(ScrollDrag {
                start_y: f32::from(event.position.y),
                start_top: self.top,
            });
            cx.notify();
            return;
        }
        let Some(line) = self.line_at(event.position.y) else {
            return;
        };
        let anchor = match self.selection {
            Some((anchor, _)) if event.modifiers.shift => anchor,
            _ => line,
        };
        self.selection = Some((anchor, line));
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(drag) = self.scroll_drag {
            if let Some((height, thumb, _)) = self.scrollbar_geometry() {
                let delta = f32::from(event.position.y) - drag.start_y;
                let per_px = self.max_top() / f64::from((height - thumb).max(1.0));
                self.top = drag.start_top + f64::from(delta) * per_px;
                self.clamp_top_to_known();
                cx.notify();
            }
            return;
        }
        if !self.selecting {
            return;
        }
        let bounds = self.viewport.get();
        if event.position.y < bounds.origin.y {
            self.scroll_by(-1.0, cx);
        } else if event.position.y > bounds.origin.y + bounds.size.height {
            self.scroll_by(1.0, cx);
        }
        if let (Some(line), Some((anchor, _))) = (self.line_at(event.position.y), self.selection) {
            self.selection = Some((anchor, line));
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting || self.scroll_drag.is_some() {
            self.selecting = false;
            self.scroll_drag = None;
            cx.notify();
        }
    }

    fn move_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let count = self.line_count();
        if count == 0 {
            return;
        }
        let head = self.selection.map_or(self.top as usize, |(_, head)| head);
        let next = head.saturating_add_signed(delta).min(count - 1);
        let anchor = match self.selection {
            Some((anchor, _)) if extend => anchor,
            _ => next,
        };
        self.selection = Some((anchor, next));
        let visible = self.visible_rows();
        let next_f = next as f64;
        if next_f < self.top {
            self.top = next_f;
        } else if next_f + 1.0 > self.top + visible {
            self.top = next_f + 1.0 - visible;
        }
        self.clamp_top_to_known();
        cx.notify();
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(lines) = self.selected_lines() else {
            return;
        };
        let range = {
            let index = lock(&self.index);
            let end = lines.end.min(index.line_count());
            index.line_bytes(lines.start.min(end)..end)
        };
        let Some(range) = range else {
            return;
        };
        if range.end - range.start > COPY_MAX_BYTES {
            cx.emit(PreviewEvent::Toast(COPY_REFUSED.into()));
            return;
        }
        let path = self.path.clone();
        let read = cx.background_spawn(async move { read_bytes(&path, range) });
        cx.spawn(async move |this, cx| {
            let bytes = read.await;
            let _ = this.update(cx, |_: &mut Self, cx| match bytes {
                Ok(bytes) => cx.write_to_clipboard(ClipboardItem::new_string(
                    String::from_utf8_lossy(&bytes).into_owned(),
                )),
                Err(err) => cx.emit(PreviewEvent::Toast(err.message().into())),
            });
        })
        .detach();
    }

    fn render_rows(
        &mut self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let count = self.line_count();
        let rows = self.visible_rows().ceil() as usize + 1;
        let first = self.top.floor() as usize;
        let shift = -((self.top - first as f64) as f32) * CODE_ROW_HEIGHT;
        let digits = crate::app::diff_dock::code::element::digit_count(count);
        let gutter_w = GUTTER_PAD * 2.0 + digits as f32 * self.char_w.max(1.0);
        let selected = self.selected_lines();
        let selection_bg = crate::theme::active_theme().selection;
        let mut out = Vec::with_capacity(rows);
        for (slot, line) in (first..count.min(first + rows)).enumerate() {
            let text = self.line_text(line, cx).unwrap_or_default();
            let shown: SharedString = if self.h_col == 0 {
                text
            } else {
                text.chars().skip(self.h_col).collect::<String>().into()
            };
            let is_selected = selected.as_ref().is_some_and(|range| range.contains(&line));
            out.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(shift + slot as f32 * CODE_ROW_HEIGHT))
                    .h(px(CODE_ROW_HEIGHT))
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(is_selected, |row| row.bg(selection_bg))
                    .child(
                        div()
                            .flex_none()
                            .w(px(gutter_w))
                            .pr(px(GUTTER_PAD))
                            .flex()
                            .justify_end()
                            .text_color(ui.muted)
                            .child(SharedString::from((line + 1).to_string())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(TEXT_PAD))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(ui.text)
                            .child(shown),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn render_scrollbar(&self, ui: crate::theme::UiColors) -> Option<AnyElement> {
        let (_, thumb, thumb_top) = self.scrollbar_geometry()?;
        Some(
            div()
                .absolute()
                .top(px(thumb_top))
                .right(px(2.))
                .w(px(SCROLLBAR_W))
                .h(px(thumb))
                .rounded(px(3.))
                .bg(ui.muted.opacity(0.5))
                .into_any_element(),
        )
    }
}

impl Render for LargeTextView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let font = code_font();
        if self.char_w <= 0.0 {
            let font_id = window.text_system().resolve_font(&font);
            self.char_w = window
                .text_system()
                .advance(font_id, px(CODE_FONT_SIZE), '0')
                .map(|size| f32::from(size.width))
                .unwrap_or(7.0);
        }
        self.clamp_top();
        let rows = self.render_rows(ui, cx);
        let scrollbar = self.render_scrollbar(ui);
        let status = self.status_label();
        let viewport = self.viewport.clone();
        let banner = div()
            .flex_none()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .text_xs()
            .border_b_1()
            .border_color(ui.border)
            .bg(ui.overlay)
            .text_color(ui.muted)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(LARGE_FILE_BANNER),
            )
            .child(
                div()
                    .id("large-text-status")
                    .flex_none()
                    .whitespace_nowrap()
                    .child(SharedString::from(status)),
            );
        let body = div()
            .id("large-text-body")
            .key_context(CODE_KEY_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .font(font)
            .text_size(px(CODE_FONT_SIZE))
            .line_height(px(CODE_ROW_HEIGHT))
            .on_action(cx.listener(|this, _: &CeUp, _, cx| this.move_selection(-1, false, cx)))
            .on_action(cx.listener(|this, _: &CeDown, _, cx| this.move_selection(1, false, cx)))
            .on_action(cx.listener(|this, _: &CeSelectUp, _, cx| this.move_selection(-1, true, cx)))
            .on_action(
                cx.listener(|this, _: &CeSelectDown, _, cx| this.move_selection(1, true, cx)),
            )
            .on_action(cx.listener(|this, _: &CePageUp, _, cx| {
                let rows = this.visible_rows();
                this.scroll_by(-rows, cx);
            }))
            .on_action(cx.listener(|this, _: &CePageDown, _, cx| {
                let rows = this.visible_rows();
                this.scroll_by(rows, cx);
            }))
            .on_action(cx.listener(|this, _: &CeDocStart, _, cx| this.scroll_to_line(0.0, cx)))
            .on_action(cx.listener(|this, _: &CeDocEnd, _, cx| {
                let end = this.line_count() as f64;
                this.scroll_to_line(end, cx);
            }))
            .on_action(cx.listener(|this, _: &CeSelectAll, _, cx| {
                let count = this.line_count();
                if count > 0 {
                    this.selection = Some((0, count - 1));
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &CeCopy, _, cx| this.copy_selection(cx)))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                this.on_wheel(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.on_mouse_down(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.on_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| this.on_mouse_up(event, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| this.on_mouse_up(event, cx)),
            )
            .child(
                canvas(move |bounds, _, _| viewport.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .children(rows)
            .children(scrollbar);
        if self.viewport.get().size.height <= px(0.) {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(banner)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(bytes: &[u8]) -> Result<LineIndex, IndexError> {
        let mut index = LineIndex::default();
        let mut scanner = LineScanner::default();
        for chunk in bytes.chunks(7) {
            scanner.scan(chunk, &mut index)?;
        }
        scanner.finish(&mut index)?;
        Ok(index)
    }

    #[test]
    fn the_index_records_where_every_line_starts() {
        let index = index_of(b"one\ntwo\r\n\nlast").expect("index");
        assert_eq!(index.line_count(), 4);
        assert_eq!(index.line_bytes(0..1), Some(0..4));
        assert_eq!(index.line_bytes(1..2), Some(4..9));
        assert_eq!(index.line_bytes(2..3), Some(9..10));
        assert_eq!(index.line_bytes(3..4), Some(10..14));
        assert!(index.is_complete());

        let trailing = index_of(b"a\nb\n").expect("index");
        assert_eq!(trailing.line_count(), 2);
        assert_eq!(index_of(b"").expect("index").line_count(), 0);
    }

    #[test]
    fn the_index_retains_at_most_eight_bytes_per_line() {
        let text: Vec<u8> = (0..100_000)
            .flat_map(|n| format!("line {n}\n").into_bytes())
            .collect();
        let index = index_of(&text).expect("index");
        assert_eq!(index.line_count(), 100_000);
        assert!(
            index.retained_bytes() <= 8 * (index.line_count() + 1),
            "{} bytes for {} lines",
            index.retained_bytes(),
            index.line_count()
        );
    }

    #[test]
    fn a_null_byte_a_giant_line_or_too_many_lines_refuses_the_file() {
        assert_eq!(index_of(b"text\0more").unwrap_err(), IndexError::Binary);

        let giant = vec![b'x'; LARGE_TEXT_MAX_LINE_BYTES as usize + 2];
        assert_eq!(index_of(&giant).unwrap_err(), IndexError::LineTooLong);
        let mut giant_terminated = giant.clone();
        giant_terminated.push(b'\n');
        assert_eq!(
            index_of(&giant_terminated).unwrap_err(),
            IndexError::LineTooLong
        );

        let mut index = LineIndex::default();
        let mut scanner = LineScanner::default();
        let lines = vec![b'\n'; LARGE_TEXT_MAX_LINES + 1];
        let refused = scanner.scan(&lines, &mut index);
        assert_eq!(refused.unwrap_err(), IndexError::TooManyLines);

        let mut index = LineIndex::default();
        let mut scanner = LineScanner::default();
        scanner
            .scan(&vec![b'\n'; LARGE_TEXT_MAX_LINES], &mut index)
            .expect("exactly the limit is fine");
        scanner.finish(&mut index).expect("finish");
        assert_eq!(index.line_count(), LARGE_TEXT_MAX_LINES);
    }

    #[test]
    fn every_refusal_names_its_reason_on_the_no_preview_screen() {
        for (error, needle) in [
            (IndexError::TooLarge, "256 MB"),
            (IndexError::TooManyLines, "5,000,000 lines"),
            (IndexError::LineTooLong, "longer than 1 MB"),
            (IndexError::Binary, "null byte"),
        ] {
            let screen = error.no_preview(Some(42));
            assert!(
                screen.reason.as_deref().unwrap_or("").contains(needle),
                "{error:?}"
            );
            assert_eq!(screen.size, Some(42));
        }
        assert!(refusal_for_size(LARGE_TEXT_MAX_BYTES).is_none());
        assert!(refusal_for_size(LARGE_TEXT_MAX_BYTES + 1).is_some());
    }

    #[test]
    fn a_cancelled_index_stops_before_reading_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.log");
        std::fs::write(&path, vec![b'a'; 4 * INDEX_CHUNK_BYTES]).expect("write");
        let index = Mutex::new(LineIndex::default());
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let outcome = index_file(&path, &index, &cancel, || {
            calls += 1;
            cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(outcome.unwrap_err(), IndexError::Cancelled);
        assert_eq!(calls, 1);
        assert_eq!(lock(&index).indexed, FIRST_SCREEN_BYTES as u64);
        assert!(!lock(&index).is_complete());
    }

    #[test]
    fn the_first_progress_report_arrives_after_sixteen_kilobytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.log");
        let text: Vec<u8> = (0..400_000)
            .flat_map(|n| format!("row {n}\n").into_bytes())
            .collect();
        std::fs::write(&path, &text).expect("write");
        let index = Mutex::new(LineIndex::default());
        let cancel = AtomicBool::new(false);
        let mut first: Option<(u64, bool)> = None;
        index_file(&path, &index, &cancel, || {
            if first.is_none() {
                let index = lock(&index);
                first = Some((index.indexed, index.is_complete()));
            }
        })
        .expect("index");
        assert_eq!(first, Some((FIRST_SCREEN_BYTES as u64, false)));
        assert_eq!(lock(&index).line_count(), 400_000);
    }

    #[test]
    fn pages_decode_their_lines_and_the_cache_keeps_twelve() {
        let lines = decode_lines(b"\xEF\xBB\xBFa\tb\r\nc\n", 0, 2);
        assert_eq!(lines, vec![SharedString::from("a   b"), "c".into()]);
        assert_eq!(decode_lines(b"x\n", 5, 1), vec![SharedString::from("x")]);

        let mut cache = PageCache::default();
        for index in 0..20 {
            assert!(cache.begin(index));
            cache.insert(Page {
                index,
                lines: vec![format!("page {index}").into()],
            });
        }
        assert_eq!(cache.len(), PAGES_KEPT);
        assert!(cache.line(0).is_none(), "the oldest pages are dropped");
        assert_eq!(
            cache.line(19 * PAGE_LINES),
            Some(SharedString::from("page 19"))
        );
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
        assert_eq!(expand_tabs("plain"), "plain");
    }
}

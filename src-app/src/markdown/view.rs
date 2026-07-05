//! `MarkdownView` - GPUI entity that renders an owned `MdNode` AST.
//!
//! The view does no parsing of its own - `MarkdownView::open` reads the file
//! from disk, runs `parser::parse_with_limit`, and stores the resulting AST.
//! `Render` walks the AST and emits a nested `div` element tree styled from
//! `MarkdownPalette` (which itself snapshots the active terminal theme).
//!
//! US-021 - live reload: `start_watcher` registers a `notify::RecommendedWatcher`
//! on the file's parent directory and spawns a `cx.spawn` task that debounces
//! events at 200 ms before re-reading the file and calling `cx.notify()`. The
//! watcher is owned by the entity, so closing the pane drops it and frees the
//! OS handle. Scroll position is preserved automatically: the GPUI element id
//! (`element_id`) is stable across re-renders.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::Either;
use gpui::{
    AnyElement, App, ClipboardItem, Context, FocusHandle, Focusable, Font, FontFeatures, FontStyle,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyContext, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Point, Render, ScrollHandle, SharedString,
    StrikethroughStyle, Styled, StyledText, TextRun, UnderlineStyle, Window, div, point,
    prelude::*, px,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use pulldown_cmark::{Alignment, HeadingLevel};

use super::parser::{MAX_INPUT_BYTES, MdNode, ParseError, Span, parse_with_limit};
use super::state;
use super::theme::MarkdownPalette;

/// Debounce window for the live-reload watcher (US-021 AC). Many editors and
/// AI agents stream writes - a single user-perceived save fires multiple
/// `Modify` events within ~50 ms. 200 ms is the sweet spot per AC: long
/// enough to coalesce a streaming write, short enough to feel instant.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);

/// Approximate scroll page-step in CSS pixels. Used by `MarkdownScrollPageUp`
/// / `MarkdownScrollPageDown` (US-022) when we don't know the precise viewport
/// height - close enough to one screen for typical terminal-pane sizes.
const PAGE_SCROLL_PX: f32 = 480.0;

/// Throttle window for scroll-position persistence writes. Scrolling fires
/// many GPUI ticks per second; we coalesce within this window to avoid a
/// disk write per pixel.
const SCROLL_PERSIST_THROTTLE: Duration = Duration::from_millis(750);

/// Polling cadence for the persistence task - checks the scroll handle's
/// current offset, writes if it changed and the throttle has elapsed.
const SCROLL_POLL_CADENCE: Duration = Duration::from_millis(250);

/// Cap on the byte size of clipboard payloads produced by `MarkdownCopy`.
/// Larger documents are truncated with a trailing ellipsis. Most platform
/// clipboards (NSPasteboard, X11 selections, Win32) accept multi-MB payloads
/// fine, but a 10 MB markdown copied to the clipboard is almost certainly
/// not what the user wanted - search-match copies are the common path.
const COPY_MAX_BYTES: usize = 64 * 1024;

const RENDER_PATH_ROOT: u64 = 14_695_981_039_346_656_037;
const MAX_RENDERED_TABLE_COLUMNS: u16 = 64;

static MARKDOWN_VIEW_ID: AtomicU64 = AtomicU64::new(1);

/// A markdown viewer pane. One instance per opened file.
pub struct MarkdownView {
    /// Absolute path to the file on disk. Stored for display in the title bar
    /// and consumed by the live-reload watcher (US-021).
    pub path: PathBuf,
    /// Parsed AST. `None` when the file failed to parse (size cap, IO error)
    /// `error` carries the user-visible message in that case.
    ast: Option<Vec<MdNode>>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    /// Stable GPUI element id, computed once at construction so the render
    /// hot path doesn't re-`format!` the path on every frame.
    element_id: SharedString,
    /// US-021 - owned watcher handle. `Some` when the live-reload pipeline is
    /// active. Dropping the entity drops this field, which unregisters the OS
    /// watch and closes the channel sender; the spawned debounce loop sees
    /// the closed channel on its next `next().await` and terminates.
    _watcher: Option<RecommendedWatcher>,
    /// US-022 - scroll handle attached to the viewer's outer scroll container.
    /// Owned here so action handlers (`MarkdownScrollPageUp/Down`) and the
    /// persistence task can read/write the offset.
    scroll_handle: ScrollHandle,
    /// US-022 - vertical offset to restore on next render. `Some` until the
    /// pending value is applied to the scroll handle (handled by a one-shot
    /// task that fires after the first paint computes `max_offset`). Storing
    /// raw f32 (CSS pixels) keeps the on-disk format simple.
    pending_restore_y: Option<f32>,
    /// US-022 - search overlay state. `search_active` gates the bar visibility
    /// and the `MarkdownSearch` key context that captures Enter/Esc/typing.
    search_active: bool,
    search_query: String,
    /// Plain-text snapshot of the rendered AST, lazily rebuilt when the AST
    /// changes. Searching this string is O(n) per query - fine for files up
    /// to `MAX_INPUT_BYTES`.
    search_corpus: String,
    /// Lowercased search corpus plus a byte-offset map back to `search_corpus`.
    /// Kept only while search is active so normal reading does not pay for it.
    search_corpus_lower: String,
    search_lower_to_source: Vec<usize>,
    /// Byte offsets of each match in `search_corpus`. Empty when no query is
    /// set or no matches exist.
    search_matches: Vec<usize>,
    /// Index into `search_matches` for the currently focused match.
    search_current: usize,
    /// Drag-to-scroll state for the visible scrollbar overlay. `None` when
    /// the user isn't currently dragging the thumb.
    scroll_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
}

impl MarkdownView {
    /// Read `path` from disk and build a view. IO errors are surfaced via the
    /// `error` field; the view is still created so the user sees the message
    /// instead of the click silently failing.
    ///
    /// US-021: on a successful first read, registers a `notify` watcher on
    /// the file's parent directory and spawns the debounce/reload loop.
    pub fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let element_id = make_element_id(&path);
        // US-022 - restore last-known scroll offset for this file (if any).
        // Goes through the shared state mutex so concurrent panes never
        // observe a half-written cache.
        let pending_restore_y = state::lookup_offset_for(&path);
        let view = Self {
            path,
            ast: None,
            error: Some("Loading...".into()),
            focus_handle: cx.focus_handle(),
            element_id,
            _watcher: None,
            scroll_handle: ScrollHandle::new(),
            pending_restore_y,
            search_active: false,
            search_query: String::new(),
            search_corpus: String::new(),
            search_corpus_lower: String::new(),
            search_lower_to_source: Vec::new(),
            search_matches: Vec::new(),
            search_current: 0,
            scroll_drag: None,
        };
        view.start_initial_load(cx);
        view.start_scroll_persistence(cx);
        view
    }

    fn start_initial_load(&self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (ast, error) = smol::unblock(move || load_from_disk(&path)).await;
                cx.update(|cx| {
                    let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                        view.apply_loaded(ast, error);
                        // Start watching after the first snapshot is applied
                        // so a slow initial read cannot overwrite a newer
                        // watcher reload that raced ahead of it.
                        view.start_watcher(cx);
                        view.maybe_apply_pending_restore(cx);
                        cx.notify();
                    });
                });
            },
        )
        .detach();
    }

    fn apply_loaded(&mut self, ast: Option<Vec<MdNode>>, error: Option<SharedString>) {
        self.ast = ast;
        self.error = error;
        // US-022 - only refresh the search corpus when the find bar is open
        // (M-1). Live-reload (US-021) fires every 200 ms during streaming
        // writes; rebuilding a multi-MB corpus on every tick would be wasted
        // work for the common case where the user is just reading.
        if self.search_active {
            self.search_corpus = self.ast.as_deref().map(harvest_text).unwrap_or_default();
            let (lower, map) = lowercase_with_byte_map(&self.search_corpus);
            self.search_corpus_lower = lower;
            self.search_lower_to_source = map;
            self.recompute_matches();
        } else {
            self.search_corpus.clear();
            self.search_corpus_lower.clear();
            self.search_lower_to_source.clear();
            self.search_matches.clear();
            self.search_current = 0;
        }
    }

    /// Rebuild `search_matches` from the current `search_query` and corpus.
    /// Called on query change, on AST reload, and when the bar opens.
    fn recompute_matches(&mut self) {
        self.search_matches.clear();
        if self.search_query.is_empty() {
            self.search_current = 0;
            return;
        }
        let needle = self.search_query.to_lowercase();
        let haystack = &self.search_corpus_lower;
        let mut start = 0;
        while let Some(pos) = haystack[start..].find(&needle) {
            let abs = start + pos;
            if let Some(&source_abs) = self.search_lower_to_source.get(abs) {
                self.search_matches.push(source_abs);
            }
            start = abs + needle.len().max(1);
        }
        if !self.search_matches.is_empty() {
            self.search_current = self.search_current.min(self.search_matches.len() - 1);
        } else {
            self.search_current = 0;
        }
    }

    /// US-022 - proportional scroll-to-match. We don't have per-span pixel
    /// offsets in the AST, so we approximate by mapping the byte offset in
    /// the search corpus to a fraction of `max_offset`. Coarse but useful:
    /// the user lands close enough to the match to spot it visually.
    fn scroll_to_current_match(&self) {
        let Some(byte_offset) = self.search_matches.get(self.search_current).copied() else {
            return;
        };
        let total = self.search_corpus.len();
        if total == 0 {
            return;
        }
        let fraction = byte_offset as f32 / total as f32;
        let max = self.scroll_handle.max_offset();
        // Offset is negative-down per ScrollHandle convention.
        let target = max.y * fraction;
        self.scroll_handle.set_offset(point(px(0.0), -target));
    }

    /// US-022 - schedule the pending scroll restore once the document has
    /// painted at least once. `set_offset` itself does not clamp, but until
    /// the scroll container has been laid out the `ScrollHandle` is not yet
    /// linked to the layout node (`track_scroll` only takes effect during
    /// prepaint). Setting an offset before the first paint writes to a
    /// detached handle and is functionally a no-op. Sleeping one tick lets
    /// the first paint complete; afterwards the handle drives layout.
    /// Non-finite values (NaN/Inf) from a hand-edited cache are dropped.
    fn maybe_apply_pending_restore(&self, cx: &mut Context<Self>) {
        if self.pending_restore_y.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            smol::Timer::after(Duration::from_millis(80)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, _cx| {
                    if let Some(y) = view.pending_restore_y.take()
                        && y.is_finite()
                    {
                        view.scroll_handle.set_offset(point(px(0.0), px(-y)));
                    }
                });
            });
        })
        .detach();
    }

    /// US-022 - long-running task that persists the scroll offset to the JSON
    /// cache as the user scrolls. Polls every `SCROLL_POLL_CADENCE`; flushes
    /// to disk when the offset has changed by more than 1 px AND the throttle
    /// window has elapsed since the last write. The task self-terminates on
    /// entity drop via the standard `WeakEntity` cancellation.
    ///
    /// Writes go through `state::save_offset_for` which serialises through a
    /// process-wide mutex - concurrent persistence tasks from multiple
    /// markdown panes therefore never lose updates from each other.
    fn start_scroll_persistence(&self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let handle = self.scroll_handle.clone();
        cx.spawn(async move |this: gpui::WeakEntity<Self>, _cx| {
            let mut last_persisted: f32 = f32::from(handle.offset().y);
            let mut last_write = Instant::now();
            loop {
                smol::Timer::after(SCROLL_POLL_CADENCE).await;
                if this.upgrade().is_none() {
                    let current: f32 = f32::from(handle.offset().y);
                    if (current - last_persisted).abs() >= 1.0
                        && let Err(e) = state::save_offset_for(&path, -current)
                    {
                        log::warn!("markdown_state.json final save failed: {}", e);
                    }
                    break;
                }
                let current: f32 = f32::from(handle.offset().y);
                if (current - last_persisted).abs() < 1.0 {
                    continue;
                }
                if last_write.elapsed() < SCROLL_PERSIST_THROTTLE {
                    continue;
                }
                // Store as positive vertical offset for clarity in the JSON.
                if let Err(e) = state::save_offset_for(&path, -current) {
                    log::warn!("markdown_state.json save failed: {}", e);
                }
                last_persisted = current;
                last_write = Instant::now();
            }
        })
        .detach();
    }

    // ---------------------------------------------------------------------
    // US-022 action handlers
    // ---------------------------------------------------------------------

    fn handle_scroll_page_up(
        &mut self,
        _: &crate::MarkdownScrollPageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cur = self.scroll_handle.offset();
        // Less-negative y = scrolled up.
        self.scroll_handle
            .set_offset(point(cur.x, (cur.y + px(PAGE_SCROLL_PX)).min(px(0.0))));
        cx.notify();
    }

    fn handle_scroll_page_down(
        &mut self,
        _: &crate::MarkdownScrollPageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cur = self.scroll_handle.offset();
        let max = self.scroll_handle.max_offset();
        // Bottom of content corresponds to `-max.y`. Clamp so we don't
        // over-scroll past it.
        let target_y = (cur.y - px(PAGE_SCROLL_PX)).max(-max.y);
        self.scroll_handle.set_offset(point(cur.x, target_y));
        cx.notify();
    }

    fn handle_find_open(
        &mut self,
        _: &crate::MarkdownFindOpen,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_active = true;
        // Build the corpus on demand the first time the bar opens (M-1).
        if let Some(ast) = self.ast.as_deref() {
            self.search_corpus = harvest_text(ast);
            let (lower, map) = lowercase_with_byte_map(&self.search_corpus);
            self.search_corpus_lower = lower;
            self.search_lower_to_source = map;
        } else {
            self.search_corpus.clear();
            self.search_corpus_lower.clear();
            self.search_lower_to_source.clear();
        }
        self.recompute_matches();
        cx.notify();
    }

    fn handle_find_dismiss(
        &mut self,
        _: &crate::MarkdownFindDismiss,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_active = false;
        self.search_query.clear();
        self.search_corpus.clear();
        self.search_corpus_lower.clear();
        self.search_lower_to_source.clear();
        self.search_matches.clear();
        self.search_current = 0;
        cx.notify();
    }

    fn handle_find_next(
        &mut self,
        _: &crate::MarkdownFindNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current = (self.search_current + 1) % self.search_matches.len();
        self.scroll_to_current_match();
        cx.notify();
    }

    fn handle_find_prev(
        &mut self,
        _: &crate::MarkdownFindPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_matches.is_empty() {
            return;
        }
        let len = self.search_matches.len();
        self.search_current = (self.search_current + len - 1) % len;
        self.scroll_to_current_match();
        cx.notify();
    }

    /// US-022 - copy support. GPUI in the pinned commit does not expose
    /// drag-text-selection across `div(...).child(SharedString)` trees. As a
    /// pragmatic substitute we copy either the active search match (with
    /// surrounding context) or the entire flat text. Mouse drag-selection
    /// over rendered markdown is a documented follow-up gap.
    fn handle_copy(&mut self, _: &crate::MarkdownCopy, _: &mut Window, cx: &mut Context<Self>) {
        let payload = if self.search_active && !self.search_matches.is_empty() {
            self.context_around_match()
        } else if let Some(ast) = self.ast.as_deref() {
            // Build the flat text on demand - the corpus field is only kept
            // up to date when the find bar is active (M-1).
            harvest_text(ast)
        } else {
            return;
        };
        if payload.is_empty() {
            return;
        }
        let bounded = truncate_for_clipboard(&payload);
        cx.write_to_clipboard(ClipboardItem::new_string(bounded));
    }

    /// Extract the line of `search_corpus` containing the current match,
    /// trimmed to a reasonable preview length. Used by `handle_copy`.
    fn context_around_match(&self) -> String {
        let Some(&offset) = self.search_matches.get(self.search_current) else {
            return String::new();
        };
        let bytes = self.search_corpus.as_bytes();
        let mut start = offset;
        while start > 0 && bytes[start - 1] != b'\n' {
            start -= 1;
        }
        let mut end = offset;
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        self.search_corpus[start..end].to_string()
    }

    /// US-022 - handle keystrokes routed via the `MarkdownSearch` key context
    /// when the find bar is open. Printable ASCII chars append to the query;
    /// Backspace removes the last char. Arrow keys / Enter / Esc are handled
    /// by their respective bound actions.
    fn handle_search_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_active {
            return;
        }
        let key = &event.keystroke.key;
        match key.as_str() {
            "backspace" => {
                if self.search_query.pop().is_some() {
                    self.recompute_matches();
                    self.scroll_to_current_match();
                    cx.notify();
                }
            }
            _ => {
                if let Some(ime_key) = event.keystroke.key_char.as_deref()
                    && !ime_key.is_empty()
                    && ime_key.chars().all(|c| !c.is_control())
                {
                    self.search_query.push_str(ime_key);
                    self.recompute_matches();
                    self.scroll_to_current_match();
                    cx.notify();
                }
            }
        }
    }

    /// US-021 - install the file watcher and spawn the debounce loop.
    ///
    /// We watch the *parent directory* non-recursively (matching
    /// `ConfigWatcher` / `ThemeWatcher`) so atomic-save patterns
    /// (`write to tmp + rename over original`) are caught: the file being
    /// removed and recreated would defeat a watch on the file inode itself.
    /// Events for siblings are filtered out by file_name match.
    fn start_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(parent) = self.path.parent().map(|p| p.to_path_buf()) else {
            log::warn!(
                "markdown watcher: path {} has no parent directory; live reload disabled",
                self.path.display()
            );
            return;
        };
        if !parent.exists() {
            log::warn!(
                "markdown watcher: parent dir {} does not exist; live reload disabled",
                parent.display()
            );
            return;
        }
        let target_filename = match self.path.file_name() {
            Some(name) => name.to_os_string(),
            None => {
                log::warn!(
                    "markdown watcher: path {} has no file name; live reload disabled",
                    self.path.display()
                );
                return;
            }
        };

        // `mpsc::unbounded` is the only async-friendly channel that supports
        // sync `unbounded_send` from the notify OS thread without blocking.
        // Critical invariant: events delivered between this line and the
        // first `rx.next().await` in the spawned task below are *queued*,
        // not lost - `unbounded` has no capacity limit. Switching to a
        // bounded channel without revisiting this race window would silently
        // drop the very-first event after `start_watcher` returns.
        let (tx, mut rx) = mpsc::unbounded::<notify::Result<notify::Event>>();
        let mut watcher = match RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                let _ = tx.unbounded_send(res);
            },
            notify::Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("markdown watcher: failed to create watcher: {}", e);
                return;
            }
        };
        if let Err(e) = watcher.watch(&parent, RecursiveMode::NonRecursive) {
            log::warn!(
                "markdown watcher: failed to watch {}: {}",
                parent.display(),
                e
            );
            return;
        }
        // Keep the watcher alive on the entity. Dropping the entity drops the
        // watcher, which unregisters the OS handle and closes the channel.
        self._watcher = Some(watcher);
        let path = self.path.clone();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // Outer loop: each iteration consumes one debounced burst.
                // `rx.next().await == None` ⇒ channel closed (entity /
                // watcher dropped) ⇒ exit cleanly.
                while let Some(first) = rx.next().await {
                    if !event_is_relevant(&first, &target_filename) {
                        continue;
                    }
                    // Coalesce subsequent events that arrive within the debounce
                    // window. We re-read once after the burst settles.
                    let deadline = Instant::now() + RELOAD_DEBOUNCE;
                    loop {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        let timer = smol::Timer::after(remaining);
                        match futures::future::select(rx.next(), timer).await {
                            Either::Left((Some(res), _)) => {
                                let _ = res; // event already accounted for; we re-read once at end
                            }
                            Either::Left((None, _)) => return,
                            Either::Right(_) => break,
                        }
                    }
                    let path = path.clone();
                    let (ast, error) = smol::unblock(move || load_from_disk(&path)).await;

                    // Apply the parsed snapshot on the GPUI main thread.
                    // `is_err()` catches the AsyncApp-dropped case directly. The
                    // entity-dropped case (this.update returning Err) is
                    // handled by the natural channel-closure chain: when
                    // MarkdownView is dropped, `_watcher` drops, the notify
                    // sender drops, `rx.next().await` returns None, and the
                    // outer `while let` exits on the next iteration.
                    if cx
                        .update(|cx| {
                            this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                                view.apply_loaded(ast, error);
                                view.maybe_apply_pending_restore(cx);
                                cx.notify();
                            })
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            },
        )
        .detach();
    }

    /// User-facing display title. Matches the file's basename so the pane's
    /// tab strip shows e.g. `README.md` rather than the absolute path.
    pub fn title(&self) -> SharedString {
        let owned: String = match self.path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => self.path.to_string_lossy().into_owned(),
        };
        SharedString::from(owned)
    }
}

impl Focusable for MarkdownView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = MarkdownPalette::from_active();

        let body = if let Some(msg) = &self.error {
            div()
                .p(px(16.))
                .text_color(palette.body)
                .child(msg.clone())
                .into_any_element()
        } else if let Some(ast) = &self.ast {
            // `w_full()` is load-bearing: list items and paragraphs use
            // `w_full()` on their inner `StyledText` wrappers to opt into
            // soft-wrap. Without a bounded outer width those `w_full()` calls
            // resolve to the intrinsic content width and stop wrapping.
            let mut col = div().flex().flex_col().gap(px(12.)).p(px(16.)).w_full();
            for (idx, node) in ast.iter().enumerate() {
                col = col.child(render_node(RENDER_PATH_ROOT, idx, node, palette));
            }
            col.into_any_element()
        } else {
            div().p(px(16.)).child("(empty)").into_any_element()
        };

        // US-022 - key contexts: `Markdown` always-on; `MarkdownSearch`
        // layered when the find bar is open so Enter/Esc/typing route to
        // the search handlers instead of the document.
        let mut key_ctx = KeyContext::default();
        key_ctx.add("Markdown");
        if self.search_active {
            key_ctx.add("MarkdownSearch");
        }

        let scroll_root = div()
            .id(self.element_id.clone())
            .size_full()
            .bg(palette.background)
            .text_color(palette.body)
            .text_size(px(14.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .child(body);

        // US-022 - visible scrollbar overlay. The markdown viewer fills
        // its pane so we don't have a meaningful first-frame content
        // estimate; pass `None` and let the scrollbar appear after the
        // first paint populates real bounds.
        let bar = crate::widgets::scrollbar::render(
            &self.scroll_handle,
            crate::theme::ui_colors(),
            None,
            "markdown-scrollbar-track",
            "markdown-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(off) = crate::widgets::scrollbar::track_click_offset(
                    &this.scroll_handle,
                    ev.position.y,
                ) {
                    this.scroll_handle.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.scroll_drag = Some(crate::widgets::scrollbar::begin_drag(
                    &this.scroll_handle,
                    ev.position.y,
                ));
                cx.stop_propagation();
            }),
        );

        let mut root = div()
            .key_context(key_ctx)
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .on_action(cx.listener(Self::handle_scroll_page_up))
            .on_action(cx.listener(Self::handle_scroll_page_down))
            .on_action(cx.listener(Self::handle_find_open))
            .on_action(cx.listener(Self::handle_find_next))
            .on_action(cx.listener(Self::handle_find_prev))
            .on_action(cx.listener(Self::handle_find_dismiss))
            .on_action(cx.listener(Self::handle_copy))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(drag) = this.scroll_drag
                    && let Some(off) = crate::widgets::scrollbar::drag_offset(
                        &this.scroll_handle,
                        &drag,
                        ev.position.y,
                    )
                {
                    this.scroll_handle.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.scroll_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            );
        if self.search_active {
            root = root.on_key_down(cx.listener(Self::handle_search_key));
        }
        root = root.child(scroll_root);
        if let Some(bar) = bar {
            root = root.child(bar);
        }

        if self.search_active {
            root = root.child(self.render_search_overlay(palette));
        }
        root
    }
}

impl MarkdownView {
    fn render_search_overlay(&self, palette: MarkdownPalette) -> impl IntoElement {
        let total = self.search_matches.len();
        let position = if total == 0 {
            "0 of 0".to_string()
        } else {
            format!("{} of {}", self.search_current + 1, total)
        };
        let label: SharedString = if self.search_query.is_empty() {
            "Type to search…".into()
        } else {
            SharedString::from(self.search_query.clone())
        };
        let position: SharedString = position.into();
        div()
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(palette.code_bg)
            .border_1()
            .border_color(palette.rule)
            .text_color(palette.body)
            .text_size(px(12.0))
            .child(div().child("Find:"))
            .child(
                div()
                    .min_w(px(120.0))
                    .text_color(palette.heading)
                    .child(label),
            )
            .child(div().text_color(palette.blockquote_text).child(position))
    }
}

/// US-022 - bound the payload size of a clipboard write. Truncates at
/// `COPY_MAX_BYTES` and appends an ellipsis marker when the cap fires so
/// the user knows the content was clipped. Truncation respects UTF-8
/// codepoint boundaries by walking back to the most recent boundary at or
/// before `COPY_MAX_BYTES`.
fn truncate_for_clipboard(text: &str) -> String {
    if text.len() <= COPY_MAX_BYTES {
        return text.to_string();
    }
    let mut end = COPY_MAX_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push_str("\n…[truncated]");
    out
}

/// US-022 - flatten an AST into plain text for substring search. Each block
/// is followed by `\n` so the per-line context heuristic in `handle_copy`
/// can recover the surrounding line. Inline spans concat without separators.
fn harvest_text(nodes: &[MdNode]) -> String {
    let mut buf = String::new();
    walk_text(nodes, &mut buf);
    buf
}

fn lowercase_with_byte_map(input: &str) -> (String, Vec<usize>) {
    let mut lower = String::with_capacity(input.len());
    let mut map = Vec::with_capacity(input.len());
    for (source_idx, ch) in input.char_indices() {
        for lower_ch in ch.to_lowercase() {
            let mut encoded = [0_u8; 4];
            let encoded = lower_ch.encode_utf8(&mut encoded);
            lower.push_str(encoded);
            map.extend(std::iter::repeat_n(source_idx, encoded.len()));
        }
    }
    (lower, map)
}

fn walk_text(nodes: &[MdNode], buf: &mut String) {
    for node in nodes {
        match node {
            MdNode::Heading { spans, .. } | MdNode::Paragraph { spans } => {
                for span in spans {
                    buf.push_str(&span.text);
                }
                buf.push('\n');
            }
            MdNode::CodeBlock { text, .. } => {
                buf.push_str(text);
                if !text.ends_with('\n') {
                    buf.push('\n');
                }
            }
            MdNode::BlockQuote { children } => walk_text(children, buf),
            MdNode::List { items, .. } => {
                for item in items {
                    walk_text(item, buf);
                }
            }
            MdNode::Table { header, rows, .. } => {
                for cell in header {
                    for span in cell {
                        buf.push_str(&span.text);
                    }
                    buf.push('\t');
                }
                buf.push('\n');
                for row in rows {
                    for cell in row {
                        for span in cell {
                            buf.push_str(&span.text);
                        }
                        buf.push('\t');
                    }
                    buf.push('\n');
                }
            }
            MdNode::Rule => buf.push_str("---\n"),
            MdNode::Footnote { label, children } => {
                buf.push_str("[^");
                buf.push_str(label);
                buf.push_str("]: ");
                walk_text(children, buf);
            }
        }
    }
}

/// Compute a stable GPUI element id for one view instance. Multiple tabs may
/// point at the same file, so the path alone is not unique enough for GPUI.
fn make_element_id(path: &std::path::Path) -> SharedString {
    let id = MARKDOWN_VIEW_ID.fetch_add(1, Ordering::Relaxed);
    SharedString::from(format!("markdown-{id}-{}", path.display()))
}

fn render_path_child(parent: u64, idx: usize) -> u64 {
    parent
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(idx as u64 + 1)
}

/// US-021 - true when `result` carries a notify event that should trigger a
/// reload of the file we are watching. Event-level errors (`Err`) are ignored;
/// events whose `paths` do not include `target_filename` are siblings in the
/// watched parent directory and ignored.
fn event_is_relevant(
    result: &notify::Result<notify::Event>,
    target_filename: &std::ffi::OsStr,
) -> bool {
    let Ok(event) = result else {
        return false;
    };
    event
        .paths
        .iter()
        .any(|p| p.file_name() == Some(target_filename))
}

/// Outcome of resolving + reading the watched path exactly once. Distinguishes
/// the three states `load_from_disk` needs to surface distinct messages for,
/// without leaking platform-specific `io::ErrorKind`/errno details to callers.
enum ReadOutcome {
    /// File read successfully within the markdown byte cap.
    Bytes(Vec<u8>),
    /// File exceeded the markdown byte cap.
    TooLarge(usize),
    /// The final path component was (or became) a symlink - refused.
    Symlink,
    /// The file does not exist (deleted between watch fire and read).
    NotFound,
    /// Any other IO failure; carries the error for the user-visible message.
    Other(std::io::Error),
}

/// Resolve `path` and read its bytes with a SINGLE name resolution, closing the
/// TOCTOU window (CWE-367) between the old `symlink_metadata` check and the
/// subsequent symlink-following `fs::read`.
///
/// Unix: open with `O_NOFOLLOW` so the kernel refuses (`ELOOP`) if the final
/// component is a symlink - the check and the read are the same syscall, so an
/// attacker cannot swap a regular file for a symlink in between. We read from
/// the resulting fd, never re-resolving the name.
///
/// Windows: open with `FILE_FLAG_OPEN_REPARSE_POINT`, inspect the handle's
/// attributes, and refuse reparse points before reading from that same handle.
fn read_no_follow(path: &std::path::Path) -> ReadOutcome {
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(f) => f,
            // `O_NOFOLLOW` on a symlinked final component fails with ELOOP.
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return ReadOutcome::Symlink,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::NotFound,
            Err(e) => return ReadOutcome::Other(e),
        };
        let mut bytes = Vec::with_capacity(MAX_INPUT_BYTES.min(64 * 1024));
        let mut limited = file.take((MAX_INPUT_BYTES + 1) as u64);
        match limited.read_to_end(&mut bytes) {
            Ok(_) if bytes.len() > MAX_INPUT_BYTES => ReadOutcome::TooLarge(bytes.len()),
            Ok(_) => ReadOutcome::Bytes(bytes),
            Err(e) => ReadOutcome::Other(e),
        }
    }
    #[cfg(windows)]
    {
        use std::io::Read;
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
        };

        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::NotFound,
            Err(e) => return ReadOutcome::Other(e),
        };
        match file.metadata() {
            Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return ReadOutcome::Symlink;
            }
            Ok(_) => {}
            Err(e) => return ReadOutcome::Other(e),
        }
        let mut bytes = Vec::with_capacity(MAX_INPUT_BYTES.min(64 * 1024));
        let mut limited = file.take((MAX_INPUT_BYTES + 1) as u64);
        match limited.read_to_end(&mut bytes) {
            Ok(_) if bytes.len() > MAX_INPUT_BYTES => ReadOutcome::TooLarge(bytes.len()),
            Ok(_) => ReadOutcome::Bytes(bytes),
            Err(e) => ReadOutcome::Other(e),
        }
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => return ReadOutcome::Symlink,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::NotFound,
            Err(e) => return ReadOutcome::Other(e),
        }
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::NotFound,
            Err(e) => return ReadOutcome::Other(e),
        };
        let mut bytes = Vec::with_capacity(MAX_INPUT_BYTES.min(64 * 1024));
        let mut limited = std::io::Read::take(&mut file, (MAX_INPUT_BYTES + 1) as u64);
        match std::io::Read::read_to_end(&mut limited, &mut bytes) {
            Ok(_) if bytes.len() > MAX_INPUT_BYTES => ReadOutcome::TooLarge(bytes.len()),
            Ok(_) => ReadOutcome::Bytes(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadOutcome::NotFound,
            Err(e) => ReadOutcome::Other(e),
        }
    }
}

/// Load and parse a markdown file from disk, returning `(ast, error)` where
/// exactly one is `Some`. Free function so unit tests can exercise the
/// initial-load and reload paths without a GPUI context.
///
/// Security: refuses to follow a path that became a symlink since the original
/// open. US-019 canonicalises the path at click time, so the path stored on
/// `MarkdownView` is the real on-disk target (not a symlink). If between
/// initial open and reload an attacker creates a symlink and atomically
/// renames it over the original (e.g. `README.md.evil → /etc/passwd` then
/// `mv README.md.evil README.md`), the post-rename file IS a symlink. We refuse
/// to follow it. The resolution is atomic via `read_no_follow` (`O_NOFOLLOW` on
/// unix) so there is no TOCTOU window between the symlink check and the read
/// (CWE-367). This blocks the information-disclosure attack from adversarial
/// agents writing into the user's project directory. Hard-link attacks remain
/// out of scope (require write access to the disclosure target itself).
fn load_from_disk(path: &std::path::Path) -> (Option<Vec<MdNode>>, Option<SharedString>) {
    let bytes = match read_no_follow(path) {
        ReadOutcome::Bytes(bytes) => bytes,
        ReadOutcome::TooLarge(bytes) => {
            return (
                None,
                Some(
                    format!(
                        "Markdown file too large ({} KB) - max {} KB.",
                        bytes / 1024,
                        MAX_INPUT_BYTES / 1024
                    )
                    .into(),
                ),
            );
        }
        ReadOutcome::Symlink => {
            return (
                None,
                Some("File path was replaced by a symlink - refusing to read.".into()),
            );
        }
        // US-021 AC: deletion during the session shows a stable message
        // and keeps the pane open (no crash, no auto-close).
        ReadOutcome::NotFound => return (None, Some("File deleted".into())),
        ReadOutcome::Other(e) => return (None, Some(format!("Could not read file: {}", e).into())),
    };
    match String::from_utf8(bytes) {
        Ok(text) => match parse_with_limit(&text) {
            Ok(nodes) => (Some(nodes), None),
            Err(ParseError::TooLarge { bytes, limit }) => (
                None,
                Some(
                    format!(
                        "Markdown file too large ({} KB) - max {} KB. Open externally to view.",
                        bytes / 1024,
                        limit / 1024
                    )
                    .into(),
                ),
            ),
        },
        Err(_) => (
            None,
            Some("File is not valid UTF-8 - cannot render as markdown.".into()),
        ),
    }
}

// ---------------------------------------------------------------------------
// Render helpers - pure functions, no `&mut Context` needed.
// ---------------------------------------------------------------------------

fn render_node(
    parent_path: u64,
    idx: usize,
    node: &MdNode,
    palette: MarkdownPalette,
) -> AnyElement {
    let path = render_path_child(parent_path, idx);
    match node {
        MdNode::Heading { level, spans } => render_heading(*level, spans, palette),
        MdNode::Paragraph { spans } => render_paragraph(spans, palette).into_any_element(),
        MdNode::CodeBlock { lang: _, text } => render_code_block(path, text, palette),
        MdNode::BlockQuote { children } => render_blockquote(path, children, palette),
        MdNode::List {
            ordered_start,
            items,
        } => render_list(path, *ordered_start, items, palette),
        MdNode::Table {
            alignments,
            header,
            rows,
        } => render_table(alignments, header, rows, palette),
        MdNode::Rule => render_rule(palette),
        MdNode::Footnote { label, children } => render_footnote(path, label, children, palette),
    }
}

/// Build a single `StyledText` element from a sequence of inline spans.
///
/// Why one element instead of N child divs (the previous approach): GPUI's
/// soft-wrap only kicks in *inside* a `StyledText` - a paragraph rendered as
/// many sibling divs lays out as a row that grows past the parent width and
/// never breaks. Coalescing every span of a paragraph into a single element
/// with per-run styling lets the text shaper reflow naturally inside the
/// pane's bounded width.
///
/// `base_color` and `base_weight` apply to plain (non-styled) spans; per-span
/// flags (strong, emphasis, code, link, strikethrough) override on a per-run
/// basis. Returns `None` when the spans contain no text - caller can then
/// skip the element entirely instead of painting an empty StyledText.
///
/// Known limitation: link spans are styled (link color + underline) but are
/// not yet clickable. The previous implementation routed clicks via a per-
/// span `on_mouse_down`, which loses inline reflow. Restoring clickability
/// here requires hit-testing the StyledText's laid-out runs (Zed's approach
/// in `crates/markdown/src/markdown.rs` - `RenderedText` + `LinkAndRange`).
/// Tracked as follow-up work; until then `Cmd/Ctrl-click` on a `.md` path
/// from a terminal pane remains the supported way to open another markdown.
///
/// SECURITY (US-044): when that rewire lands, the resolved `span.link_url`
/// MUST be passed through `crate::markdown::security::validate_link_url(url)?`
/// before reaching `open::that`. `.md` content is potentially hostile, so a
/// raw `file://`/`javascript:`/`data:` URL must never hit `xdg-open`. The
/// allow-list is regression-tested by `security::tests::allowlist_is_http_https_only`.
fn build_styled_text(
    spans: &[Span],
    palette: MarkdownPalette,
    base_color: Hsla,
    base_weight: FontWeight,
) -> Option<StyledText> {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::with_capacity(spans.len());
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let len = span.text.len();
        let is_code = span.style.code;
        let family: SharedString = if is_code {
            "monospace".into()
        } else {
            ".SystemUIFont".into()
        };
        let weight = if span.style.strong {
            FontWeight::BOLD
        } else {
            base_weight
        };
        let style = if span.style.emphasis {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        let mut color = if is_code { palette.code_fg } else { base_color };
        let bg = if is_code { Some(palette.code_bg) } else { None };
        let mut underline: Option<UnderlineStyle> = None;
        if span.link_url.is_some() {
            color = palette.link;
            underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            });
        }
        let strikethrough = if span.style.strikethrough {
            Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            })
        } else {
            None
        };
        runs.push(TextRun {
            len,
            font: Font {
                family,
                features: FontFeatures::default(),
                fallbacks: None,
                weight,
                style,
            },
            color,
            background_color: bg,
            underline,
            strikethrough,
        });
        text.push_str(&span.text);
    }
    if text.is_empty() {
        None
    } else {
        Some(StyledText::new(text).with_runs(runs))
    }
}

fn render_heading(level: HeadingLevel, spans: &[Span], palette: MarkdownPalette) -> AnyElement {
    let (size, weight, top_gap) = match level {
        HeadingLevel::H1 => (px(28.), FontWeight::BOLD, px(8.)),
        HeadingLevel::H2 => (px(22.), FontWeight::BOLD, px(6.)),
        HeadingLevel::H3 => (px(18.), FontWeight::SEMIBOLD, px(4.)),
        HeadingLevel::H4 => (px(16.), FontWeight::SEMIBOLD, px(2.)),
        HeadingLevel::H5 | HeadingLevel::H6 => (px(14.), FontWeight::SEMIBOLD, px(2.)),
    };
    let mut row = div().w_full().text_size(size).pt(top_gap);
    if let Some(styled) = build_styled_text(spans, palette, palette.heading, weight) {
        row = row.child(styled);
    }
    row.into_any_element()
}

fn render_paragraph(spans: &[Span], palette: MarkdownPalette) -> impl IntoElement {
    let mut row = div().w_full();
    if let Some(styled) = build_styled_text(spans, palette, palette.body, FontWeight::NORMAL) {
        row = row.child(styled);
    }
    row
}

fn render_code_block(path: u64, text: &str, palette: MarkdownPalette) -> AnyElement {
    // Code blocks contain pre-formatted content that must NOT soft-wrap
    // (preserves indentation + intent). Long lines previously clipped at the
    // pane edge; following Zed's `markdown.rs` pattern (`overflow_x_scroll`
    // + a stable id), each block becomes its own horizontally scrollable
    // container. The id is derived from the AST path, not the text, so large
    // blocks do not get hashed during render.
    div()
        .id(("md-code-block", path))
        .bg(palette.code_bg)
        .text_color(palette.code_fg)
        .font_family("monospace")
        .text_size(px(13.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(4.))
        .w_full()
        .overflow_x_scroll()
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn render_blockquote(path: u64, children: &[MdNode], palette: MarkdownPalette) -> AnyElement {
    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .border_l_2()
        .border_color(palette.blockquote_border)
        .pl(px(12.))
        .w_full()
        .text_color(palette.blockquote_text);
    for (idx, child) in children.iter().enumerate() {
        col = col.child(render_node(path, idx, child, palette));
    }
    col.into_any_element()
}

fn render_list(
    path: u64,
    ordered_start: Option<u64>,
    items: &[Vec<MdNode>],
    palette: MarkdownPalette,
) -> AnyElement {
    let mut col = div().flex().flex_col().gap(px(4.)).pl(px(20.)).w_full();
    for (idx, item) in items.iter().enumerate() {
        let item_path = render_path_child(path, idx);
        let marker: SharedString = match ordered_start {
            Some(start) => format!("{}.", start.saturating_add(idx as u64)).into(),
            None => "•".into(),
        };
        // `w_full()` bounds the row to its parent. The marker is fixed-width
        // and `flex_shrink_0` so it never collapses; the body claims the rest
        // (`flex_1`) and `min_w(px(0.))` lets it shrink below its intrinsic
        // text width - without this, a flex item's min-width defaults to
        // `auto` (= content size) and long lines push the row past the
        // viewport instead of wrapping inside StyledText.
        let mut item_row = div().flex().flex_row().gap(px(8.)).w_full();
        item_row = item_row.child(
            div()
                .w(px(20.))
                .flex_shrink_0()
                .text_color(palette.body)
                .child(marker),
        );
        let mut item_body = div().flex().flex_col().gap(px(4.)).flex_1().min_w(px(0.));
        for (cidx, child) in item.iter().enumerate() {
            item_body = item_body.child(render_node(item_path, cidx, child, palette));
        }
        item_row = item_row.child(item_body);
        col = col.child(item_row);
    }
    col.into_any_element()
}

/// Column count for a markdown table: the max of header arity and the longest
/// data row, capped to a renderer-safe maximum.
///
/// U-050: the input is untrusted file content (MAX_INPUT_BYTES = 10 MB admits
/// a multi-million-column delimiter row). `grid_cols` with thousands of
/// columns is not useful UI, so we render a bounded prefix.
fn table_col_count(header: &[Vec<Span>], rows: &[Vec<Vec<Span>>]) -> u16 {
    let cols = header
        .len()
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    u16::try_from(cols)
        .unwrap_or(MAX_RENDERED_TABLE_COLUMNS)
        .min(MAX_RENDERED_TABLE_COLUMNS)
}

fn render_table(
    _alignments: &[Alignment],
    header: &[Vec<Span>],
    rows: &[Vec<Vec<Span>>],
    palette: MarkdownPalette,
) -> AnyElement {
    // Column count: max of header arity and the longest data row. Empty
    // tables (zero header + zero rows, or rows with zero cells) bail out
    // before invoking grid_cols(0) which would be a runtime no-op.
    let cols = table_col_count(header, rows);
    if cols == 0 {
        return div().into_any_element();
    }

    // Per Zed's `MarkdownElement` (crates/markdown/src/markdown.rs:1981):
    // CSS grid + equal-fraction columns + `overflow_hidden` is the only
    // layout that prevents wide cells from blowing the table past the pane
    // width. `flex_row` rows would let any single cell push the row wider
    // than its parent - which is exactly what the original implementation
    // did. With grid the row width is always `w_full`, columns are 1fr each,
    // and StyledText cell content reflows inside its column.
    let mut table = div()
        .grid()
        .grid_cols(cols)
        .w_full()
        .overflow_hidden()
        .border_1()
        .border_color(palette.rule)
        .rounded(px(4.));

    if !header.is_empty() {
        for cell in header.iter().take(cols as usize) {
            table = table.child(render_table_cell(cell, palette, true));
        }
    }
    for row in rows {
        for cell in row.iter().take(cols as usize) {
            table = table.child(render_table_cell(cell, palette, false));
        }
    }
    table.into_any_element()
}

fn render_table_cell(
    spans: &[Span],
    palette: MarkdownPalette,
    is_header: bool,
) -> impl IntoElement {
    let weight = if is_header {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let mut cell = div()
        .px(px(8.))
        .py(px(4.))
        .border_b_1()
        .border_r_1()
        .border_color(palette.rule)
        .text_color(palette.body);
    if is_header {
        cell = cell.bg(palette.code_bg);
    }
    if let Some(styled) = build_styled_text(spans, palette, palette.body, weight) {
        cell = cell.child(styled);
    }
    cell
}

fn render_rule(palette: MarkdownPalette) -> AnyElement {
    div()
        .h(px(1.))
        .my(px(4.))
        .bg(palette.rule)
        .into_any_element()
}

fn render_footnote(
    path: u64,
    label: &str,
    children: &[MdNode],
    palette: MarkdownPalette,
) -> AnyElement {
    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .w_full()
        .text_color(palette.blockquote_text)
        .text_size(px(12.));
    col = col.child(
        div()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(SharedString::from(format!("[^{}]", label))),
    );
    for (idx, child) in children.iter().enumerate() {
        col = col.child(render_node(path, idx, child, palette));
    }
    col.into_any_element()
}

// ---------------------------------------------------------------------------
// Tests - exercise the data-only paths (non-rendering) so the file doesn't
// drift from `parser` without notice. Render paths require a GPUI context
// and are verified manually per repo convention.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// U-050: a pathological table must be capped before it reaches grid_cols.
    /// A genuinely empty table still reports 0.
    #[test]
    fn table_col_count_caps_pathological_tables() {
        let huge: Vec<Vec<Span>> = vec![Vec::new(); u16::MAX as usize + 2];
        assert_eq!(
            table_col_count(&[], std::slice::from_ref(&huge)),
            MAX_RENDERED_TABLE_COLUMNS
        );
        // Empty table reports 0 so the `cols == 0` bail still fires.
        assert_eq!(table_col_count(&[], &[]), 0);
        // Header arity counts too, and a normal small table is unchanged.
        let header: Vec<Vec<Span>> = vec![Vec::new(); 3];
        assert_eq!(table_col_count(&header, &[]), 3);
    }

    #[test]
    fn lowercase_map_preserves_source_offsets_for_unicode_search() {
        let corpus = "Cafe İSTANBUL";
        let (lower, map) = lowercase_with_byte_map(corpus);
        let pos = lower.find("i").expect("lowercase dotted I should match i");
        assert_eq!(&corpus[map[pos]..map[pos] + "İ".len()], "İ");
    }

    fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write");
    }

    #[test]
    fn loads_existing_file_into_ast() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("doc.md");
        write(&path, b"# Hello\n");
        let (ast, error) = load_from_disk(&path);
        assert!(error.is_none(), "unexpected error: {:?}", error);
        let ast = ast.expect("ast");
        assert!(matches!(ast.first(), Some(MdNode::Heading { .. })));
    }

    #[test]
    fn reload_picks_up_modified_content() {
        // US-021: a second read after content change must reflect the new AST.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("live.md");
        write(&path, b"# v1\n");
        let (ast_v1, _) = load_from_disk(&path);
        let v1_text = match ast_v1.as_deref().and_then(|nodes| nodes.first()) {
            Some(MdNode::Heading { spans, .. }) => {
                spans.iter().map(|s| s.text.as_str()).collect::<String>()
            }
            _ => panic!("expected heading"),
        };
        assert_eq!(v1_text, "v1");

        write(&path, b"# v2\n");
        let (ast_v2, _) = load_from_disk(&path);
        let v2_text = match ast_v2.as_deref().and_then(|nodes| nodes.first()) {
            Some(MdNode::Heading { spans, .. }) => {
                spans.iter().map(|s| s.text.as_str()).collect::<String>()
            }
            _ => panic!("expected heading"),
        };
        assert_eq!(v2_text, "v2", "reload must reflect new content");
    }

    #[test]
    fn deleted_file_surfaces_file_deleted_message() {
        // US-021 AC: deletion during the session must produce the literal
        // "File deleted" message, not a crash or auto-close.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("doomed.md");
        write(&path, b"# alive\n");
        fs::remove_file(&path).expect("rm");
        let (ast, error) = load_from_disk(&path);
        assert!(ast.is_none());
        let msg: &str = error.as_ref().expect("error message").as_ref();
        assert_eq!(msg, "File deleted");
    }

    #[test]
    fn oversized_file_shows_size_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("huge.md");
        let bytes = vec![b'a'; MAX_INPUT_BYTES + 1];
        write(&path, &bytes);
        let (ast, error) = load_from_disk(&path);
        assert!(ast.is_none());
        let msg: &str = error.as_ref().expect("error message").as_ref();
        assert!(msg.contains("too large"), "expected size warning: {}", msg);
    }

    #[test]
    fn invalid_utf8_shows_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("not_utf8.md");
        // 0xFF is invalid as a leading byte in UTF-8.
        write(&path, &[0xFF, 0xFE, 0xFD]);
        let (ast, error) = load_from_disk(&path);
        assert!(ast.is_none());
        let msg: &str = error.as_ref().expect("error message").as_ref();
        assert!(msg.contains("UTF-8"), "expected utf-8 warning: {}", msg);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_replacement_is_rejected() {
        // SEC defence: if the watched path becomes a symlink between open
        // and reload, refuse to follow it. Simulates the rename-over attack
        // an adversarial agent could mount in the user's project directory.
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().expect("tempdir");
        let real_target = tmp.path().join("secret.txt");
        write(&real_target, b"sensitive\n");
        let view_path = tmp.path().join("README.md");
        symlink(&real_target, &view_path).expect("symlink");

        let (ast, error) = load_from_disk(&view_path);
        assert!(ast.is_none(), "must not parse a symlinked target");
        let msg: &str = error.as_ref().expect("error message").as_ref();
        assert!(
            msg.contains("symlink"),
            "expected symlink rejection message, got: {}",
            msg
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_swapped_in_after_check_is_still_rejected() {
        // CWE-367 regression: the old code stat'd the path, then did a separate
        // symlink-following read. `read_no_follow` collapses both into one
        // O_NOFOLLOW open, so even a symlink that is the *current* final
        // component (the post-swap state) is refused at read time - there is no
        // window where a regular-file stat is paired with a symlink read.
        // Before the fix, the read path followed the symlink and disclosed the
        // target; after it, the open fails with ELOOP and we surface the
        // refusal message.
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().expect("tempdir");
        let secret = tmp.path().join("secret.txt");
        write(&secret, b"# TOP SECRET\n");
        let view_path = tmp.path().join("README.md");
        symlink(&secret, &view_path).expect("symlink");

        let (ast, error) = load_from_disk(&view_path);
        assert!(ast.is_none(), "must not read through a symlink");
        let msg: &str = error.as_ref().expect("error message").as_ref();
        assert!(
            msg.contains("symlink"),
            "expected symlink rejection, got: {}",
            msg
        );
    }

    #[test]
    fn event_is_relevant_filters_siblings() {
        use std::ffi::OsString;
        let target = OsString::from("README.md");
        let make = |path: &str| -> notify::Result<notify::Event> {
            Ok(notify::Event {
                kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
                paths: vec![PathBuf::from(path)],
                attrs: Default::default(),
            })
        };
        assert!(event_is_relevant(&make("/x/README.md"), &target));
        assert!(!event_is_relevant(&make("/x/other.md"), &target));
        // Errors are ignored.
        assert!(!event_is_relevant(
            &Err(notify::Error::generic("boom")),
            &target
        ));
    }

    #[test]
    fn harvest_text_concatenates_paragraph_spans_with_inline_styles() {
        // Verifies the search corpus contains the visible text of a
        // formatted paragraph. pulldown-cmark preserves inter-span spaces in
        // its Text events, so the corpus reads "this is bold text" as the
        // user sees it (CR H-3 regression guard).
        let nodes = parse_with_limit("this is **bold** text\n").expect("parse");
        let corpus = harvest_text(&nodes);
        assert!(
            corpus.contains("this is bold text"),
            "corpus missing space-joined text: {:?}",
            corpus
        );
    }

    #[test]
    fn harvest_text_includes_code_block_content() {
        let nodes = parse_with_limit("```rust\nfn main() {}\n```\n").expect("parse");
        let corpus = harvest_text(&nodes);
        assert!(corpus.contains("fn main() {}"));
    }

    #[test]
    fn harvest_text_walks_nested_lists() {
        let src = "- top1\n  - nested-a\n- top2\n";
        let nodes = parse_with_limit(src).expect("parse");
        let corpus = harvest_text(&nodes);
        for needle in &["top1", "nested-a", "top2"] {
            assert!(
                corpus.contains(needle),
                "missing {} in {:?}",
                needle,
                corpus
            );
        }
    }

    #[test]
    fn truncate_for_clipboard_short_input_unchanged() {
        let small = "small";
        assert_eq!(truncate_for_clipboard(small), "small");
    }

    #[test]
    fn truncate_for_clipboard_caps_large_payload() {
        let huge: String = std::iter::repeat_n('a', COPY_MAX_BYTES + 100).collect();
        let bounded = truncate_for_clipboard(&huge);
        assert!(bounded.len() <= COPY_MAX_BYTES + "\n…[truncated]".len());
        assert!(bounded.ends_with("[truncated]"));
    }

    #[test]
    fn truncate_for_clipboard_respects_utf8_boundaries() {
        // Build a string whose byte at COPY_MAX_BYTES falls inside a 3-byte
        // codepoint. The truncator must walk back to the boundary rather
        // than splitting the codepoint (which would panic the slice).
        let mut s = String::with_capacity(COPY_MAX_BYTES + 8);
        while s.len() < COPY_MAX_BYTES - 2 {
            s.push('a');
        }
        // 3-byte UTF-8 codepoint that straddles the cap.
        s.push('日');
        while s.len() < COPY_MAX_BYTES + 32 {
            s.push('a');
        }
        let _ = truncate_for_clipboard(&s);
    }
}

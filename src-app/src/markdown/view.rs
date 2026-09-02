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

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);

const PAGE_SCROLL_PX: f32 = 480.0;

const SCROLL_PERSIST_THROTTLE: Duration = Duration::from_millis(750);

const SCROLL_POLL_CADENCE: Duration = Duration::from_millis(250);

const COPY_MAX_BYTES: usize = 64 * 1024;

const RENDER_PATH_ROOT: u64 = 14_695_981_039_346_656_037;
const MAX_RENDERED_TABLE_COLUMNS: u16 = 64;

static MARKDOWN_VIEW_ID: AtomicU64 = AtomicU64::new(1);

pub struct MarkdownView {
    pub path: PathBuf,
    ast: Option<Vec<MdNode>>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    element_id: SharedString,
    _watcher: Option<RecommendedWatcher>,
    scroll_handle: ScrollHandle,
    pending_restore_y: Option<f32>,
    search_active: bool,
    search_query: String,
    search_corpus: String,
    search_corpus_lower: String,
    search_lower_to_source: Vec<usize>,
    search_matches: Vec<usize>,
    search_current: usize,
    scroll_drag: Option<crate::widgets::scrollbar::ScrollDragState>,
}

impl MarkdownView {
    pub fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let element_id = make_element_id(&path);
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
        let target = max.y * fraction;
        self.scroll_handle.set_offset(point(px(0.0), -target));
    }

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
                if let Err(e) = state::save_offset_for(&path, -current) {
                    log::warn!("markdown_state.json save failed: {}", e);
                }
                last_persisted = current;
                last_write = Instant::now();
            }
        })
        .detach();
    }

    fn handle_scroll_page_up(
        &mut self,
        _: &crate::MarkdownScrollPageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cur = self.scroll_handle.offset();
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

    fn handle_copy(&mut self, _: &crate::MarkdownCopy, _: &mut Window, cx: &mut Context<Self>) {
        let payload = if self.search_active && !self.search_matches.is_empty() {
            self.context_around_match()
        } else if let Some(ast) = self.ast.as_deref() {
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
        self._watcher = Some(watcher);
        let path = self.path.clone();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                while let Some(first) = rx.next().await {
                    if !event_is_relevant(&first, &target_filename) {
                        continue;
                    }
                    let deadline = Instant::now() + RELOAD_DEBOUNCE;
                    loop {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        let timer = smol::Timer::after(remaining);
                        match futures::future::select(rx.next(), timer).await {
                            Either::Left((Some(res), _)) => {
                                let _ = res;
                            }
                            Either::Left((None, _)) => return,
                            Either::Right(_) => break,
                        }
                    }
                    let path = path.clone();
                    let (ast, error) = smol::unblock(move || load_from_disk(&path)).await;

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
            let mut col = div().flex().flex_col().gap(px(12.)).p(px(16.)).w_full();
            for (idx, node) in ast.iter().enumerate() {
                col = col.child(render_node(RENDER_PATH_ROOT, idx, node, palette));
            }
            col.into_any_element()
        } else {
            div().p(px(16.)).child("(empty)").into_any_element()
        };

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

fn make_element_id(path: &std::path::Path) -> SharedString {
    let id = MARKDOWN_VIEW_ID.fetch_add(1, Ordering::Relaxed);
    SharedString::from(format!("markdown-{id}-{}", path.display()))
}

fn render_path_child(parent: u64, idx: usize) -> u64 {
    parent
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(idx as u64 + 1)
}

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

enum ReadOutcome {
    Bytes(Vec<u8>),
    TooLarge(usize),
    Symlink,
    NotFound,
    Other(std::io::Error),
}

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
    let cols = table_col_count(header, rows);
    if cols == 0 {
        return div().into_any_element();
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn table_col_count_caps_pathological_tables() {
        let huge: Vec<Vec<Span>> = vec![Vec::new(); u16::MAX as usize + 2];
        assert_eq!(
            table_col_count(&[], std::slice::from_ref(&huge)),
            MAX_RENDERED_TABLE_COLUMNS
        );
        assert_eq!(table_col_count(&[], &[]), 0);
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
        write(&path, &[0xFF, 0xFE, 0xFD]);
        let (ast, error) = load_from_disk(&path);
        assert!(ast.is_none());
        let msg: &str = error.as_ref().expect("error message").as_ref();
        assert!(msg.contains("UTF-8"), "expected utf-8 warning: {}", msg);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_replacement_is_rejected() {
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
        assert!(!event_is_relevant(
            &Err(notify::Error::generic("boom")),
            &target
        ));
    }

    #[test]
    fn harvest_text_concatenates_paragraph_spans_with_inline_styles() {
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
        let mut s = String::with_capacity(COPY_MAX_BYTES + 8);
        while s.len() < COPY_MAX_BYTES - 2 {
            s.push('a');
        }
        s.push('日');
        while s.len() < COPY_MAX_BYTES + 32 {
            s.push('a');
        }
        let _ = truncate_for_clipboard(&s);
    }
}

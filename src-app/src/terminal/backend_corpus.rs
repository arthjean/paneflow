use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::Event as AlacEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Side as AlacSide;
use alacritty_terminal::selection::{Selection as AlacSelection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, TermMode};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};

use super::listener::{SpikeTermSize, ZedListener};
use super::types::{Content, Modes, Point, SelectionRange, content_from_term};

pub(crate) const CORPUS_SEED: u64 = 0x5041_4e45_464c_4f57;
const CORPUS_FAMILIES: usize = 27;
const CORPUS_VARIANTS: usize = 5;
const CORPUS_SIZE: usize = CORPUS_FAMILIES * CORPUS_VARIANTS;
#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
const PERFORMANCE_ROUNDS: usize = 5;

struct CorpusCase {
    name: String,
    bytes: Vec<u8>,
    resize_after_feed: Option<(usize, usize)>,
    selection_after_feed: Option<SelectionRange>,
    search_after_feed: Option<&'static str>,
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    comparison: SnapshotComparison,
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
#[derive(Clone, Copy)]
enum SnapshotComparison {
    Exact,
    ReflowViewportAnchor,
    EraseDisplayScrollback,
    InvalidUtf8Replacement,
    TabExpansion,
}

struct Harness {
    term: Arc<FairMutex<Term<ZedListener>>>,
    events: UnboundedReceiver<AlacEvent>,
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedSnapshot {
    content: String,
    logical_text: String,
    modes: String,
    events: Vec<String>,
    search: Option<SearchObservation>,
    resize_damage: Option<ResizeDamageObservation>,
    history_size: usize,
    cell_count: usize,
    absolute_cursor_line: i64,
    cursor_column: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct SearchObservation {
    matches: Vec<(i32, usize, i32, usize)>,
    regex_error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ResizeDamageObservation {
    before_dimensions: (usize, usize),
    after_dimensions: (usize, usize),
    before_cell_count: usize,
    after_cell_count: usize,
    snapshot_changed: bool,
}

struct ResizeBefore {
    dimensions: (usize, usize),
    cell_count: usize,
    content: String,
}

impl ResizeBefore {
    fn capture(content: &Content) -> Self {
        Self {
            dimensions: (content.cols, content.rows),
            cell_count: content.cells.len(),
            content: normalize_content(content.clone()),
        }
    }

    fn complete(self, content: &Content) -> ResizeDamageObservation {
        ResizeDamageObservation {
            before_dimensions: self.dimensions,
            after_dimensions: (content.cols, content.rows),
            before_cell_count: self.cell_count,
            after_cell_count: content.cells.len(),
            snapshot_changed: self.content != normalize_content(content.clone()),
        }
    }
}

impl Harness {
    fn new() -> Self {
        let (events_tx, events) = unbounded();
        let listener = ZedListener::new(events_tx);
        let dimensions = SpikeTermSize {
            columns: 80,
            screen_lines: 24,
        };
        let config = TermConfig {
            scrolling_history: 10_000,
            ..TermConfig::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &dimensions, listener)));
        Self { term, events }
    }

    fn replay(mut self, case: &CorpusCase, chunks: &[usize]) -> NormalizedSnapshot {
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut offset = 0;
        for &size in chunks {
            if offset >= case.bytes.len() {
                break;
            }
            let end = offset.saturating_add(size).min(case.bytes.len());
            processor.advance(&mut *self.term.lock(), &case.bytes[offset..end]);
            offset = end;
        }
        if offset < case.bytes.len() {
            processor.advance(&mut *self.term.lock(), &case.bytes[offset..]);
        }

        let resize_before = if let Some((columns, screen_lines)) = case.resize_after_feed {
            let before = {
                let term = self.term.lock_unfair();
                ResizeBefore::capture(&content_from_term(&term))
            };
            self.term.lock().resize(SpikeTermSize {
                columns,
                screen_lines,
            });
            Some(before)
        } else {
            None
        };
        if let Some(range) = case.selection_after_feed {
            let mut selection =
                AlacSelection::new(SelectionType::Simple, range.start.into(), AlacSide::Left);
            selection.update(range.end.into(), AlacSide::Right);
            self.term.lock().selection = Some(selection);
        }

        let search = case.search_after_feed.map(|query| {
            normalize_alacritty_search(crate::search::search_term(&self.term, query, false))
        });

        let (content, logical_text, modes, history_size, cell_count) = {
            let term = self.term.lock_unfair();
            let content = content_from_term(&term);
            let history_size = content.history_size;
            let cell_count = content.cells.len();
            (
                content,
                normalize_alacritty_grid(&term),
                normalize_modes(*term.mode()),
                history_size,
                cell_count,
            )
        };
        let resize_damage = resize_before.map(|before| before.complete(&content));
        let absolute_cursor_line = history_size as i64 + i64::from(content.cursor.point.line.0);
        let cursor_column = content.cursor.point.column.0;
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            if let Some(normalized) = normalize_alacritty_event(event) {
                events.push(normalized);
            }
        }
        NormalizedSnapshot {
            content: normalize_content(content),
            logical_text,
            modes,
            events,
            search,
            resize_damage,
            history_size,
            cell_count,
            absolute_cursor_line,
            cursor_column,
        }
    }
}

fn normalize_alacritty_grid(term: &Term<ZedListener>) -> String {
    let mut lines = Vec::new();
    let mut row = term.topmost_line().0;
    let bottom = term.bottommost_line().0;
    while row <= bottom {
        let text = term.bounds_to_string(
            alacritty_terminal::index::Point::new(
                alacritty_terminal::index::Line(row),
                alacritty_terminal::index::Column(0),
            ),
            alacritty_terminal::index::Point::new(
                alacritty_terminal::index::Line(row),
                term.last_column(),
            ),
        );
        lines.push(text.trim_end().to_owned());
        row += 1;
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn normalize_alacritty_event(event: AlacEvent) -> Option<String> {
    match event {
        AlacEvent::Wakeup | AlacEvent::MouseCursorDirty | AlacEvent::CursorBlinkingChange => None,
        AlacEvent::PtyWrite(text) => Some(normalize_pty_write(&text)),
        AlacEvent::ClipboardStore(_, text) => Some(format!("ClipboardStore({text:?})")),
        AlacEvent::Bell => Some("Bell".to_owned()),
        AlacEvent::Title(title) => Some(format!("Title({title:?})")),
        other => Some(format!("{other:?}")),
    }
}

fn normalize_pty_write(text: &str) -> String {
    if text.starts_with("\x1b[?") && text.ends_with('c') {
        "PtyWrite(PrimaryDeviceAttributes)".to_owned()
    } else if text.starts_with("\x1b[>") && text.ends_with('c') {
        "PtyWrite(SecondaryDeviceAttributes)".to_owned()
    } else {
        format!("PtyWrite({text:?})")
    }
}

fn normalize_alacritty_search(result: crate::search::SearchResult) -> SearchObservation {
    SearchObservation {
        matches: result
            .matches
            .into_iter()
            .map(|found| {
                (
                    found.start.line.0,
                    found.start.column.0,
                    found.end.line.0,
                    found.end.column.0,
                )
            })
            .collect(),
        regex_error: result.regex_error,
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
struct GhosttyHarness {
    terminal: paneflow_terminal_ghostty::DisplayTerminal,
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
impl GhosttyHarness {
    fn new() -> Self {
        let size = paneflow_terminal_ghostty::WindowSize::new(80, 24, 0, 0)
            .expect("corpus dimensions are valid");
        Self {
            terminal: paneflow_terminal_ghostty::DisplayTerminal::new(size, 10_000)
                .expect("pinned Ghostty terminal initializes"),
        }
    }

    fn replay(mut self, case: &CorpusCase, chunks: &[usize]) -> NormalizedSnapshot {
        let mut offset = 0;
        for &size in chunks {
            if offset >= case.bytes.len() {
                break;
            }
            let end = offset.saturating_add(size).min(case.bytes.len());
            self.terminal
                .feed(&case.bytes[offset..end])
                .expect("Ghostty accepts corpus chunk");
            offset = end;
        }
        if offset < case.bytes.len() {
            self.terminal
                .feed(&case.bytes[offset..])
                .expect("Ghostty accepts corpus tail");
        }
        let resize_before = if let Some((columns, rows)) = case.resize_after_feed {
            // Initialize the native snapshot cache before resize so the second
            // snapshot must consume Ghostty's resize damage, not a cold cache.
            let before = super::ghostty_session::content_from_ghostty(
                self.terminal
                    .snapshot()
                    .expect("Ghostty pre-resize snapshot"),
            );
            let before = ResizeBefore::capture(&before);
            let size = paneflow_terminal_ghostty::WindowSize::new(columns, rows, 0, 0)
                .expect("corpus resize dimensions are valid");
            self.terminal.resize(size).expect("Ghostty corpus resize");
            Some(before)
        } else {
            None
        };
        if let Some(range) = case.selection_after_feed {
            self.terminal
                .set_selection(paneflow_terminal_ghostty::SelectionRange {
                    start: paneflow_terminal_ghostty::Point::new(
                        range.start.line.0,
                        range.start.column.0,
                    ),
                    end: paneflow_terminal_ghostty::Point::new(
                        range.end.line.0,
                        range.end.column.0,
                    ),
                    rectangle: range.is_block,
                })
                .expect("Ghostty corpus selection");
        }

        let search = case.search_after_feed.map(|query| {
            normalize_ghostty_search(
                self.terminal
                    .search(query, false)
                    .expect("Ghostty corpus search"),
            )
        });

        let modes = super::ghostty_session::modes_from_ghostty(
            self.terminal.modes().expect("Ghostty modes"),
        );
        let scrollback = self
            .terminal
            .extract_scrollback()
            .expect("Ghostty grid extraction")
            .unwrap_or_default();
        let content = super::ghostty_session::content_from_ghostty(
            self.terminal.snapshot().expect("Ghostty snapshot"),
        );
        let logical_text = normalize_ghostty_grid(&scrollback, &content);
        let resize_damage = resize_before.map(|before| before.complete(&content));
        let history_size = content.history_size;
        let cell_count = content.cells.len();
        let absolute_cursor_line = history_size as i64 + i64::from(content.cursor.point.line.0);
        let cursor_column = content.cursor.point.column.0;
        let events = self
            .terminal
            .drain_events()
            .into_iter()
            .filter_map(normalize_ghostty_event)
            .collect();
        NormalizedSnapshot {
            content: normalize_content(content),
            logical_text,
            modes: format!("{modes:?}"),
            events,
            search,
            resize_damage,
            history_size,
            cell_count,
            absolute_cursor_line,
            cursor_column,
        }
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
fn normalize_ghostty_grid(scrollback: &str, content: &Content) -> String {
    let mut lines = if scrollback.is_empty() {
        Vec::new()
    } else {
        scrollback.lines().map(str::to_owned).collect()
    };
    for row in 0..content.rows {
        let mut text = String::with_capacity(content.cols);
        for cell in content
            .cells
            .iter()
            .filter(|cell| cell.point.line.0 == row as i32)
        {
            if cell
                .flags
                .contains(super::types::CellFlags::WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
            if let Some(zero_width) = &cell.zerowidth {
                text.extend(zero_width);
            }
        }
        lines.push(text.trim_end().to_owned());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
fn normalize_ghostty_event(event: paneflow_terminal_ghostty::BackendEvent) -> Option<String> {
    match event {
        paneflow_terminal_ghostty::BackendEvent::WritePty(bytes) => {
            Some(normalize_pty_write(&String::from_utf8_lossy(&bytes)))
        }
        paneflow_terminal_ghostty::BackendEvent::ClipboardStore(text) => {
            Some(format!("ClipboardStore({text:?})"))
        }
        paneflow_terminal_ghostty::BackendEvent::Bell => Some("Bell".to_owned()),
        paneflow_terminal_ghostty::BackendEvent::Title(title) => Some(format!("Title({title:?})")),
        paneflow_terminal_ghostty::BackendEvent::WorkingDirectory(cwd) => {
            Some(format!("WorkingDirectory({cwd:?})"))
        }
        paneflow_terminal_ghostty::BackendEvent::CallbackPanicked
        | paneflow_terminal_ghostty::BackendEvent::InputDropped { .. } => None,
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
fn normalize_ghostty_search(result: paneflow_terminal_ghostty::SearchResult) -> SearchObservation {
    SearchObservation {
        matches: result
            .matches
            .into_iter()
            .map(|found| {
                (
                    found.start.line,
                    found.start.column,
                    found.end.line,
                    found.end.column,
                )
            })
            .collect(),
        regex_error: result.regex_error,
    }
}

fn normalize_content(content: Content) -> String {
    let mut cells = String::new();
    for cell in content.cells.iter() {
        use std::fmt::Write as _;
        let _ = write!(
            cells,
            "{}:{}:{:?}:{:?}:{:?}:{:?}:{:?}:{}|",
            cell.point.line.0,
            cell.point.column.0,
            cell.c,
            cell.fg,
            cell.bg,
            cell.flags,
            cell.zerowidth,
            cell.hyperlink
        );
    }
    format!(
        "history={};offset={};cursor={:?};selection={:?};cells={cells}",
        content.history_size, content.display_offset, content.cursor, content.selection
    )
}

fn normalize_modes(mode: TermMode) -> String {
    format!("{:?}", Modes::from(mode))
}

fn fixed_chunks(len: usize, size: usize) -> Vec<usize> {
    vec![size; len.div_ceil(size)]
}

fn seeded_chunks(len: usize, seed: u64) -> Vec<usize> {
    let mut state = seed;
    let mut remaining = len;
    let mut chunks = Vec::new();
    while remaining > 0 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let size = 1 + (state as usize % 257);
        chunks.push(size.min(remaining));
        remaining = remaining.saturating_sub(size);
    }
    chunks
}

fn corpus() -> Vec<CorpusCase> {
    let mut cases = Vec::with_capacity(CORPUS_SIZE);
    for index in 0..CORPUS_SIZE {
        let variant = index / CORPUS_FAMILIES;
        let family = index % CORPUS_FAMILIES;
        let (bytes, resize_after_feed) = match family {
            0 => (format!("plain-ascii-{variant}\r\n").into_bytes(), None),
            1 => (format!("unicode-{variant}: café Καλημέρα हिन्दी 🦀\r\n").into_bytes(), None),
            2 => (format!("grapheme-{variant}: e\u{301} n\u{303} 👨‍👩‍👧‍👦\r\n").into_bytes(), None),
            3 => (format!("wide-{variant}: 中文 日本語 한글\r\n").into_bytes(), None),
            4 => (format!("\x1b[1;3;4;9mstyled-{variant}\x1b[0m\r\n").into_bytes(), None),
            5 => (format!("\x1b[38;2;{};{};{}mtruecolor-{variant}\x1b[0m", 20 + variant, 80 + variant, 140 + variant).into_bytes(), None),
            6 => (format!("origin\x1b[{};{}Hcursor-{variant}\x1b[2A\x1b[3C", 2 + variant, 3 + variant).into_bytes(), None),
            7 => ((format!("wrap-{variant}-") + &"x".repeat(180 + variant)).into_bytes(), None),
            8 => ((format!("reflow-{variant}-") + &"0123456789".repeat(24)).into_bytes(), Some((41 + variant, 18 + variant))),
            9 => (format!("before\x1b[?1049halt-{variant}\x1b[?1049lafter").into_bytes(), None),
            10 => ((0..40).map(|line| format!("scroll-{variant}-{line}\r\n")).collect::<String>().into_bytes(), None),
            11 => (format!("\x1b[?1h\x1b[?1000h\x1b[?1006hmode-{variant}").into_bytes(), None),
            12 => (format!("\x1b]2;synthetic-title-{variant}\x07title-body").into_bytes(), None),
            13 => (
                format!("query-{variant}\x1b[5n\x1b[6n\x1b[c\x1b[>c").into_bytes(),
                None,
            ),
            14 => (format!("malformed-{variant}\x1b[999999999999999999999;?;mend").into_bytes(), None),
            15 => (format!("truncated-{variant}\x1b]8;;https://synthetic.invalid/unterminated").into_bytes(), None),
            16 => (format!("erase-{variant}\x1b[2J\x1b[Hredrawn-{variant}").into_bytes(), None),
            17 => (format!("\x1b]8;id=synthetic-{variant};https://example.invalid/{variant}\x07link\x1b]8;;\x07").into_bytes(), None),
            18 => (format!("\x1b]133;A\x07prompt-{variant}\x1b]133;B\x07command\x1b]133;C\x07output\x1b]133;D;0\x07").into_bytes(), None),
            19 => (format!("\x1b]52;c;c3ludGhldGljLWNsaXBib2FyZC0{variant}=\x07").into_bytes(), None),
            20 => (format!("\x1b[{};{}mansi16-{variant}\x1b[0m", 30 + variant, 40 + ((variant + 2) % 6)).into_bytes(), None),
            21 => (format!("\x1b[38;5;{};48;5;{}mindexed256-{variant}\x1b[0m", 16 + variant * 17, 231 - variant * 11).into_bytes(), None),
            22 => (format!("\x1b[2;7mdim-inverse-{variant}\x1b[0m").into_bytes(), None),
            23 => (format!("\x1b[{} qcursor-shape-{variant}", variant + 1).into_bytes(), None),
            24 => {
                let mut bytes = format!("invalid-utf8-{variant}:").into_bytes();
                bytes.extend_from_slice(&[0xf0, 0x28, 0x8c, 0x28, b'\r', b'\n']);
                (bytes, None)
            }
            25 => (format!("tabs-{variant}:\talpha\t中\tomega\r\n").into_bytes(), None),
            26 => (format!("selection-{variant}-target").into_bytes(), None),
            _ => unreachable!(),
        };
        cases.push(CorpusCase {
            name: format!("family-{family:02}-variant-{variant}"),
            bytes,
            resize_after_feed,
            selection_after_feed: (family == 26).then_some(SelectionRange {
                start: Point::new(0, 0),
                end: Point::new(0, 8),
                is_block: false,
            }),
            search_after_feed: (family == 26).then_some("target"),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            comparison: match family {
                8 => SnapshotComparison::ReflowViewportAnchor,
                16 => SnapshotComparison::EraseDisplayScrollback,
                24 => SnapshotComparison::InvalidUtf8Replacement,
                25 => SnapshotComparison::TabExpansion,
                _ => SnapshotComparison::Exact,
            },
        });
    }
    cases
}

pub(crate) fn deterministic_streams() -> Vec<Vec<u8>> {
    corpus().into_iter().map(|case| case.bytes).collect()
}

#[test]
fn alacritty_corpus_is_chunk_invariant() {
    let corpus = corpus();
    assert_eq!(corpus.len(), CORPUS_SIZE);
    for (index, case) in corpus.iter().enumerate() {
        let baseline = Harness::new().replay(case, &[case.bytes.len().max(1)]);
        for (label, chunks) in [
            ("1", fixed_chunks(case.bytes.len(), 1)),
            ("7", fixed_chunks(case.bytes.len(), 7)),
            ("64", fixed_chunks(case.bytes.len(), 64)),
            ("4096", fixed_chunks(case.bytes.len(), 4096)),
            (
                "seeded",
                seeded_chunks(case.bytes.len(), CORPUS_SEED ^ index as u64),
            ),
        ] {
            assert_eq!(
                Harness::new().replay(case, &chunks),
                baseline,
                "chunk divergence in {} with {label}-byte plan",
                case.name
            );
        }
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
#[test]
fn ghostty_corpus_matches_alacritty() {
    for (index, case) in corpus().iter().enumerate() {
        let chunks = seeded_chunks(case.bytes.len(), CORPUS_SEED ^ index as u64);
        let ghostty = GhosttyHarness::new().replay(case, &chunks);
        let alacritty = Harness::new().replay(case, &chunks);
        assert_eq!(ghostty.search, alacritty.search, "search in {}", case.name);
        assert_eq!(
            ghostty.resize_damage, alacritty.resize_damage,
            "resize damage in {}",
            case.name
        );
        match case.comparison {
            SnapshotComparison::Exact => {
                assert_eq!(ghostty, alacritty, "backend divergence in {}", case.name)
            }
            SnapshotComparison::ReflowViewportAnchor => {
                // Both engines reflow to the same logical grid and cursor. When height
                // shrinks at the same time, Alacritty anchors three rows in scrollback
                // while Ghostty keeps all seven non-empty rows visible. The pinned C API
                // exposes no resize-anchor policy, so this viewport split is the sole
                // documented corpus divergence. Every semantic field remains strict.
                assert_eq!(
                    ghostty.logical_text, alacritty.logical_text,
                    "logical reflow text in {}",
                    case.name
                );
                assert_eq!(ghostty.modes, alacritty.modes, "modes in {}", case.name);
                assert_eq!(ghostty.events, alacritty.events, "events in {}", case.name);
                assert_eq!(
                    ghostty.cell_count, alacritty.cell_count,
                    "viewport dimensions in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.absolute_cursor_line, alacritty.absolute_cursor_line,
                    "absolute cursor line in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.cursor_column, alacritty.cursor_column,
                    "cursor column in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.history_size, 0,
                    "Ghostty reflow anchor changed in {}",
                    case.name
                );
                assert!(
                    alacritty.history_size > 0,
                    "Alacritty reflow anchor changed in {}",
                    case.name
                );
            }
            SnapshotComparison::EraseDisplayScrollback => {
                // CSI 2 J clears the visible display in both engines. Alacritty moves
                // the erased row into scrollback, while Ghostty discards it. The C API
                // exposes no erase-history policy, so pin both native semantics while
                // keeping every visible field strict.
                assert_eq!(
                    ghostty.content.replacen("history=0;", "history=1;", 1),
                    alacritty.content,
                    "visible erase result in {}",
                    case.name
                );
                assert_eq!(ghostty.modes, alacritty.modes, "modes in {}", case.name);
                assert_eq!(ghostty.events, alacritty.events, "events in {}", case.name);
                assert_eq!(
                    ghostty.cell_count, alacritty.cell_count,
                    "viewport dimensions in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.cursor_column, alacritty.cursor_column,
                    "cursor column in {}",
                    case.name
                );
                let variant = case.name.rsplit('-').next().expect("corpus variant suffix");
                assert_eq!(ghostty.logical_text, format!("redrawn-{variant}"));
                assert_eq!(
                    alacritty.logical_text,
                    format!("erase-{variant}\nredrawn-{variant}")
                );
                assert_eq!(
                    ghostty.history_size, 0,
                    "Ghostty erase policy changed in {}",
                    case.name
                );
                assert_eq!(
                    alacritty.history_size, 1,
                    "Alacritty erase policy changed in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.absolute_cursor_line + 1,
                    alacritty.absolute_cursor_line
                );
            }
            SnapshotComparison::InvalidUtf8Replacement => {
                // Ghostty replaces both invalid UTF-8 subsequences in F0 28 8C 28;
                // Alacritty drops the isolated continuation byte. Sanitizing the PTY
                // stream would alter binary protocol bytes, so pin both deterministic
                // parser semantics and keep every surrounding contract strict.
                let variant = case.name.rsplit('-').next().expect("corpus variant suffix");
                assert_eq!(ghostty.logical_text, format!("invalid-utf8-{variant}:�(�("));
                assert_eq!(
                    alacritty.logical_text,
                    format!("invalid-utf8-{variant}:�((")
                );
                assert_eq!(ghostty.modes, alacritty.modes, "modes in {}", case.name);
                assert_eq!(ghostty.events, alacritty.events, "events in {}", case.name);
                assert_eq!(
                    ghostty.history_size, alacritty.history_size,
                    "history in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.cell_count, alacritty.cell_count,
                    "viewport dimensions in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.absolute_cursor_line, alacritty.absolute_cursor_line,
                    "absolute cursor line in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.cursor_column, alacritty.cursor_column,
                    "cursor column in {}",
                    case.name
                );
            }
            SnapshotComparison::TabExpansion => {
                // Both engines advance through the same tab stops. Ghostty
                // materializes the tab origins as spaces, while Alacritty keeps
                // a literal tab in those cells, so pin that representation only.
                assert_eq!(
                    ghostty.content,
                    alacritty.content.replace("'\\t'", "' '"),
                    "tab-expanded cells in {}",
                    case.name
                );
                let variant = case.name.rsplit('-').next().expect("corpus variant suffix");
                assert_eq!(
                    ghostty.logical_text,
                    format!("tabs-{variant}: alpha   中      omega")
                );
                assert_eq!(
                    alacritty.logical_text,
                    format!("tabs-{variant}:\talpha\t中\tomega")
                );
                assert_eq!(ghostty.modes, alacritty.modes, "modes in {}", case.name);
                assert_eq!(ghostty.events, alacritty.events, "events in {}", case.name);
                assert_eq!(
                    ghostty.history_size, alacritty.history_size,
                    "history in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.cell_count, alacritty.cell_count,
                    "viewport dimensions in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.absolute_cursor_line, alacritty.absolute_cursor_line,
                    "absolute cursor line in {}",
                    case.name
                );
                assert_eq!(
                    ghostty.cursor_column, alacritty.cursor_column,
                    "cursor column in {}",
                    case.name
                );
            }
        }
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
#[test]
fn corpus_observes_search_and_resize_damage() {
    let cases = corpus();
    let search_case = cases
        .iter()
        .find(|case| case.name == "family-26-variant-0")
        .expect("search corpus case");
    let search_chunks = seeded_chunks(search_case.bytes.len(), CORPUS_SEED);
    for (backend, snapshot) in [
        (
            "Alacritty",
            Harness::new().replay(search_case, &search_chunks),
        ),
        (
            "Ghostty",
            GhosttyHarness::new().replay(search_case, &search_chunks),
        ),
    ] {
        let search = snapshot.search.expect("search observation");
        assert_eq!(
            search.matches,
            vec![(0, 12, 0, 17)],
            "{backend} search coordinates"
        );
        assert_eq!(search.regex_error, None, "{backend} search error");
    }

    let resize_case = cases
        .iter()
        .find(|case| case.name == "family-08-variant-0")
        .expect("resize corpus case");
    let resize_chunks = seeded_chunks(resize_case.bytes.len(), CORPUS_SEED);
    for (backend, snapshot) in [
        (
            "Alacritty",
            Harness::new().replay(resize_case, &resize_chunks),
        ),
        (
            "Ghostty",
            GhosttyHarness::new().replay(resize_case, &resize_chunks),
        ),
    ] {
        let damage = snapshot.resize_damage.expect("resize damage observation");
        assert_eq!(
            damage.before_dimensions,
            (80, 24),
            "{backend} before resize"
        );
        assert_eq!(damage.after_dimensions, (41, 18), "{backend} after resize");
        assert_eq!(
            damage.before_cell_count,
            80 * 24,
            "{backend} cells before resize"
        );
        assert_eq!(
            damage.after_cell_count,
            41 * 18,
            "{backend} cells after resize"
        );
        assert!(
            damage.snapshot_changed,
            "{backend} resize produced no damage"
        );
    }
}

#[test]
fn malformed_and_oversized_streams_are_deterministic() {
    let mut hostile = vec![b'A'; 1024 * 1024];
    hostile.extend_from_slice(b"\x1b]52;c;");
    hostile.extend(std::iter::repeat_n(b'B', 128 * 1024));
    hostile.extend_from_slice(b"\x1b\\\x1b[999999999999999999999999999999m");
    let case = CorpusCase {
        name: "hostile-bounded-fixture".to_owned(),
        bytes: hostile,
        resize_after_feed: None,
        selection_after_feed: None,
        search_after_feed: None,
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        comparison: SnapshotComparison::Exact,
    };
    let first = Harness::new().replay(&case, &fixed_chunks(case.bytes.len(), 4096));
    let second = Harness::new().replay(&case, &fixed_chunks(case.bytes.len(), 4096));
    assert_eq!(first, second);
    assert!(first.history_size <= 10_000, "scrollback cap was exceeded");
    assert!(
        first.cell_count <= 80 * 24,
        "snapshot escaped the viewport bound"
    );
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    {
        let ghostty_first =
            GhosttyHarness::new().replay(&case, &seeded_chunks(case.bytes.len(), CORPUS_SEED));
        let ghostty_second =
            GhosttyHarness::new().replay(&case, &seeded_chunks(case.bytes.len(), CORPUS_SEED));
        assert_eq!(ghostty_first, ghostty_second);
        assert!(ghostty_first.history_size <= 10_000);
        assert!(ghostty_first.cell_count <= 80 * 24);
    }
}

#[test]
#[ignore = "captures the machine-specific EP-001 performance baseline"]
fn alacritty_eight_pane_baseline() {
    let cases = corpus();
    let total_bytes = cases.iter().map(|case| case.bytes.len()).sum::<usize>() * 8;
    let wall_start = Instant::now();
    let cpu_start = process_cpu_time();
    let rss_start = resident_set_bytes();
    let mut frame_latencies = Vec::with_capacity(cases.len() * 8);
    let mut lock_durations = Vec::with_capacity(cases.len() * 8);

    let mut panes = (0..8).map(|_| Harness::new()).collect::<Vec<_>>();
    for (index, case) in cases.iter().enumerate() {
        for (pane, harness) in panes.iter_mut().enumerate() {
            let feed_start = Instant::now();
            let chunks = seeded_chunks(case.bytes.len(), CORPUS_SEED ^ pane as u64 ^ index as u64);
            let mut processor = Processor::<StdSyncHandler>::new();
            let mut offset: usize = 0;
            for size in chunks {
                let end = offset.saturating_add(size).min(case.bytes.len());
                processor.advance(&mut *harness.term.lock(), &case.bytes[offset..end]);
                offset = end;
            }
            if let Some((columns, screen_lines)) = case.resize_after_feed {
                harness.term.lock().resize(SpikeTermSize {
                    columns,
                    screen_lines,
                });
            }
            let lock_start = Instant::now();
            let snapshot = {
                let term = harness.term.lock_unfair();
                content_from_term(&term)
            };
            lock_durations.push(lock_start.elapsed());
            std::hint::black_box(snapshot);
            frame_latencies.push(feed_start.elapsed());
        }
    }

    let wall = wall_start.elapsed();
    let cpu = process_cpu_time().saturating_sub(cpu_start);
    let rss_end = resident_set_bytes();
    frame_latencies.sort_unstable();
    lock_durations.sort_unstable();
    let throughput = total_bytes as f64 / wall.as_secs_f64() / (1024.0 * 1024.0);
    println!(
        "{{\"seed\":\"0x{CORPUS_SEED:016x}\",\"panes\":8,\"streams_per_pane\":{},\"bytes\":{total_bytes},\"throughput_mib_s\":{throughput:.3},\"input_to_snapshot_p50_us\":{},\"input_to_snapshot_p95_us\":{},\"lock_p95_us\":{},\"wall_ms\":{},\"cpu_ms\":{},\"rss_start_bytes\":{},\"rss_end_bytes\":{},\"cpu_model\":{:?},\"profile\":{:?},\"measurement_scope\":\"persistent-eight-pane-parser-to-neutral-snapshot\"}}",
        cases.len(),
        percentile_us(&frame_latencies, 50),
        percentile_us(&frame_latencies, 95),
        percentile_us(&lock_durations, 95),
        wall.as_millis(),
        cpu.as_millis(),
        rss_start,
        rss_end,
        cpu_model(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
}

#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
#[derive(Debug, Default)]
struct BackendPerformance {
    wall: Duration,
    cpu: Duration,
    rss_growth: u64,
    feed_durations: Vec<Duration>,
    input_to_snapshot: Vec<Duration>,
    snapshot_durations: Vec<Duration>,
}

#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
impl BackendPerformance {
    fn absorb(&mut self, mut sample: Self) {
        self.wall = self.wall.saturating_add(sample.wall);
        self.cpu = self.cpu.saturating_add(sample.cpu);
        self.rss_growth = self.rss_growth.max(sample.rss_growth);
        self.feed_durations.append(&mut sample.feed_durations);
        self.input_to_snapshot.append(&mut sample.input_to_snapshot);
        self.snapshot_durations
            .append(&mut sample.snapshot_durations);
    }
}

#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
fn measure_alacritty_performance(cases: &[CorpusCase]) -> BackendPerformance {
    let mut panes = (0..8).map(|_| Harness::new()).collect::<Vec<_>>();
    let rss_start = resident_set_bytes();
    let cpu_start = process_cpu_time();
    let wall_start = Instant::now();
    let mut input_to_snapshot = Vec::with_capacity(cases.len() * panes.len());
    let mut snapshot_durations = Vec::with_capacity(cases.len() * panes.len());
    let mut feed_durations = Vec::with_capacity(cases.len() * panes.len());
    for (case_index, case) in cases.iter().enumerate() {
        for (pane_index, pane) in panes.iter_mut().enumerate() {
            let started = Instant::now();
            let chunks = seeded_chunks(
                case.bytes.len(),
                CORPUS_SEED ^ case_index as u64 ^ pane_index as u64,
            );
            let mut processor = Processor::<StdSyncHandler>::new();
            let mut offset = 0usize;
            for size in chunks {
                let end = offset.saturating_add(size).min(case.bytes.len());
                processor.advance(&mut *pane.term.lock(), &case.bytes[offset..end]);
                offset = end;
            }
            feed_durations.push(started.elapsed());
            if let Some((columns, screen_lines)) = case.resize_after_feed {
                pane.term.lock().resize(SpikeTermSize {
                    columns,
                    screen_lines,
                });
            }
            let snapshot_started = Instant::now();
            let snapshot = {
                let term = pane.term.lock_unfair();
                content_from_term(&term)
            };
            snapshot_durations.push(snapshot_started.elapsed());
            std::hint::black_box(snapshot);
            input_to_snapshot.push(started.elapsed());
        }
    }
    BackendPerformance {
        wall: wall_start.elapsed(),
        cpu: process_cpu_time().saturating_sub(cpu_start),
        rss_growth: resident_set_bytes().saturating_sub(rss_start),
        feed_durations,
        input_to_snapshot,
        snapshot_durations,
    }
}

#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
fn measure_ghostty_performance(cases: &[CorpusCase]) -> BackendPerformance {
    let mut panes = (0..8).map(|_| GhosttyHarness::new()).collect::<Vec<_>>();
    let rss_start = resident_set_bytes();
    let cpu_start = process_cpu_time();
    let wall_start = Instant::now();
    let mut input_to_snapshot = Vec::with_capacity(cases.len() * panes.len());
    let mut snapshot_durations = Vec::with_capacity(cases.len() * panes.len());
    let mut feed_durations = Vec::with_capacity(cases.len() * panes.len());
    for (case_index, case) in cases.iter().enumerate() {
        for (pane_index, pane) in panes.iter_mut().enumerate() {
            let started = Instant::now();
            let chunks = seeded_chunks(
                case.bytes.len(),
                CORPUS_SEED ^ case_index as u64 ^ pane_index as u64,
            );
            let mut offset = 0usize;
            for size in chunks {
                let end = offset.saturating_add(size).min(case.bytes.len());
                pane.terminal
                    .feed(&case.bytes[offset..end])
                    .expect("Ghostty benchmark feed");
                offset = end;
            }
            feed_durations.push(started.elapsed());
            if let Some((columns, rows)) = case.resize_after_feed {
                pane.terminal
                    .resize(
                        paneflow_terminal_ghostty::WindowSize::new(columns, rows, 0, 0)
                            .expect("Ghostty benchmark resize dimensions"),
                    )
                    .expect("Ghostty benchmark resize");
            }
            let snapshot_started = Instant::now();
            let snapshot = super::ghostty_session::content_from_ghostty(
                pane.terminal
                    .snapshot()
                    .expect("Ghostty benchmark snapshot"),
            );
            snapshot_durations.push(snapshot_started.elapsed());
            std::hint::black_box(snapshot);
            input_to_snapshot.push(started.elapsed());
        }
    }
    BackendPerformance {
        wall: wall_start.elapsed(),
        cpu: process_cpu_time().saturating_sub(cpu_start),
        rss_growth: resident_set_bytes().saturating_sub(rss_start),
        feed_durations,
        input_to_snapshot,
        snapshot_durations,
    }
}

#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
#[test]
#[ignore = "EP-005 promotion gate: release-only relative parser and snapshot benchmark"]
fn ghostty_parser_and_snapshot_performance_gate() {
    assert!(
        !cfg!(debug_assertions),
        "run the performance gate in release"
    );
    let cases = corpus();
    let mut alacritty = BackendPerformance::default();
    let mut ghostty = BackendPerformance::default();
    for round in 0..PERFORMANCE_ROUNDS {
        // Alternate order so CPU-frequency and allocator state do not
        // systematically favor the backend measured second.
        if round % 2 == 0 {
            alacritty.absorb(measure_alacritty_performance(&cases));
            ghostty.absorb(measure_ghostty_performance(&cases));
        } else {
            ghostty.absorb(measure_ghostty_performance(&cases));
            alacritty.absorb(measure_alacritty_performance(&cases));
        }
    }
    alacritty.feed_durations.sort_unstable();
    alacritty.input_to_snapshot.sort_unstable();
    alacritty.snapshot_durations.sort_unstable();
    ghostty.feed_durations.sort_unstable();
    ghostty.input_to_snapshot.sort_unstable();
    ghostty.snapshot_durations.sort_unstable();

    let alacritty_feed = alacritty.feed_durations.iter().sum::<Duration>();
    let ghostty_feed = ghostty.feed_durations.iter().sum::<Duration>();
    let parser_ratio = alacritty_feed.as_secs_f64() / ghostty_feed.as_secs_f64();
    let alacritty_p95 = percentile_duration(&alacritty.input_to_snapshot, 95);
    let ghostty_p95 = percentile_duration(&ghostty.input_to_snapshot, 95);
    let latency_regression =
        ghostty_p95.as_secs_f64() / alacritty_p95.max(Duration::from_nanos(1)).as_secs_f64() - 1.0;
    let alacritty_p95_us = alacritty_p95.as_secs_f64() * 1_000_000.0;
    let ghostty_p95_us = ghostty_p95.as_secs_f64() * 1_000_000.0;
    let alacritty_feed_p95 = percentile_duration(&alacritty.feed_durations, 95);
    let ghostty_feed_p95 = percentile_duration(&ghostty.feed_durations, 95);
    let alacritty_snapshot_p95 = percentile_duration(&alacritty.snapshot_durations, 95);
    let ghostty_snapshot_p95 = percentile_duration(&ghostty.snapshot_durations, 95);
    let alacritty_feed_p95_us = alacritty_feed_p95.as_secs_f64() * 1_000_000.0;
    let ghostty_feed_p95_us = ghostty_feed_p95.as_secs_f64() * 1_000_000.0;
    let alacritty_snapshot_p95_us = alacritty_snapshot_p95.as_secs_f64() * 1_000_000.0;
    let ghostty_snapshot_p95_us = ghostty_snapshot_p95.as_secs_f64() * 1_000_000.0;
    println!(
        "{{\"parser_ratio\":{parser_ratio:.4},\"input_to_snapshot_p95_regression\":{latency_regression:.4},\"alacritty_input_to_snapshot_p95_us\":{alacritty_p95_us:.3},\"ghostty_input_to_snapshot_p95_us\":{ghostty_p95_us:.3},\"alacritty_feed_p95_us\":{alacritty_feed_p95_us:.3},\"ghostty_feed_p95_us\":{ghostty_feed_p95_us:.3},\"alacritty_snapshot_p95_us\":{alacritty_snapshot_p95_us:.3},\"ghostty_snapshot_p95_us\":{ghostty_snapshot_p95_us:.3},\"alacritty_wall_ms\":{},\"ghostty_wall_ms\":{},\"alacritty_cpu_ms\":{},\"ghostty_cpu_ms\":{},\"alacritty_rss_growth\":{},\"ghostty_rss_growth\":{},\"panes\":8,\"streams_per_pane\":{},\"measurement_rounds\":{PERFORMANCE_ROUNDS},\"profile\":\"release\"}}",
        alacritty.wall.as_millis(),
        ghostty.wall.as_millis(),
        alacritty.cpu.as_millis(),
        ghostty.cpu.as_millis(),
        alacritty.rss_growth,
        ghostty.rss_growth,
        cases.len() * PERFORMANCE_ROUNDS,
    );
    assert!(
        parser_ratio >= 0.95,
        "Ghostty parser ratio {parser_ratio:.4} is below 0.95"
    );
    assert!(
        latency_regression <= 0.05,
        "Ghostty input-to-snapshot p95 regression {latency_regression:.4} exceeds 0.05"
    );
    assert!(
        ghostty_snapshot_p95 < Duration::from_millis(2),
        "Ghostty native snapshot p95 {ghostty_snapshot_p95_us:.3} us exceeds 2 ms"
    );
}

fn percentile_duration(values: &[Duration], percentile: usize) -> Duration {
    let index = values.len().saturating_sub(1).saturating_mul(percentile) / 100;
    values.get(index).copied().unwrap_or_default()
}

pub(crate) fn percentile_us(values: &[Duration], percentile: usize) -> u128 {
    percentile_duration(values, percentile).as_micros()
}

#[cfg(target_os = "linux")]
pub(crate) fn resident_set_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let resident_pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
    resident_pages.saturating_mul(page_size)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn resident_set_bytes() -> u64 {
    0
}

#[cfg(target_os = "linux")]
pub(crate) fn process_cpu_time() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields = stat
        .rsplit_once(')')
        .map(|(_, fields)| fields)
        .unwrap_or("");
    let mut values = fields.split_whitespace();
    let user_ticks = values
        .nth(11)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let system_ticks = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    Duration::from_secs_f64((user_ticks + system_ticks) as f64 / ticks_per_second as f64)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn process_cpu_time() -> Duration {
    Duration::ZERO
}

#[cfg(target_os = "linux")]
pub(crate) fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix("model name\t: "))
        .unwrap_or("unknown")
        .to_owned()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn cpu_model() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

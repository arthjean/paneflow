//! Off-thread file loading for the editor, with the guardrails that decide
//! whether a file opens editable, opens read-only, or does not open at all.
//!
//! prd-file-editor-2026-Q3, US-002 (async load + generation guard) and US-003
//! (refusal rules).
//!
//! **Why off-thread.** `std::fs::read` on a cold 8 MB file is tens of
//! milliseconds of blocking syscall; run on the GPUI thread it is a visible
//! stall of the whole window, every pane included. The load therefore follows
//! the shape `markdown/view.rs::start_initial_load` already established here:
//! `cx.spawn` + `smol::unblock` for the blocking part, then a single
//! `WeakEntity::update` back on the main thread. A dead entity (tab closed
//! mid-read) makes that update fail silently, which is the intended outcome.
//!
//! **Why a generation guard.** Clicking three files quickly starts three reads
//! that can finish in any order; without a guard, the slowest read wins and the
//! tab shows a file the user is no longer on. [`CodeLoadSlot`] stamps every
//! request with a monotonically increasing generation and accepts a result only
//! when it still matches the latest one. It is a plain struct, deliberately
//! free of any GPUI type, so the ordering rule is unit-testable without a test
//! app; [`spawn_code_load`] is the thin GPUI wrapper over it.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use gpui::{AsyncApp, Context, WeakEntity};

use super::document::{CodeDocument, ReadOnlyReason};
use super::highlight::CodeHighlighter;
use crate::diff::DiffSyntax;

/// Largest file the editor will open, aligned with the markdown viewer's
/// [`crate::markdown::MAX_INPUT_BYTES`] so the two file surfaces refuse the
/// same files. Past this, the rope itself is fine but the initial full parse
/// and the row cache are not worth the stall.
pub(crate) const MAX_FILE_BYTES: usize = crate::markdown::MAX_INPUT_BYTES;

/// Longest line the editor will let the user edit (US-003). Rendering such a
/// line is bounded work; editing it is not, because every keystroke re-measures
/// and re-highlights the whole row.
pub(crate) const MAX_LINE_CHARS: usize = 10_000;

/// How far into the file the binary sniff looks. A nul byte in the first 8 KB
/// is the same heuristic `git diff` uses to call a blob binary.
pub(crate) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// A file the editor refuses to open, carrying the exact sentence shown in the
/// tab. Every variant renders a written explanation - never a raw `io::Error`
/// (FR-7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum CodeLoadError {
    /// Larger than [`MAX_FILE_BYTES`].
    TooLarge { bytes: usize, limit: usize },
    /// Nul byte inside the first [`BINARY_SNIFF_BYTES`], or bytes that are not
    /// UTF-8 at all.
    Binary,
    /// Valid bytes, but not valid UTF-8 (a Latin-1 or UTF-16 file, say).
    NotUtf8,
    /// Deleted or never existed.
    NotFound,
    /// Present but unreadable by this process.
    PermissionDenied,
    /// A directory, socket, or any other non-regular path.
    NotAFile,
    /// Anything else the OS reported. Carries a short description so the
    /// message stays specific without leaking a debug-formatted error.
    Io { detail: String },
}

impl CodeLoadError {
    /// The sentence rendered in place of the file.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::TooLarge { bytes, limit } => format!(
                "This file is {} MB, past the {} MB editing limit.",
                bytes / (1024 * 1024),
                limit / (1024 * 1024)
            ),
            Self::Binary => "Binary file - this file cannot be shown as text.".to_string(),
            Self::NotUtf8 => "This file is not valid UTF-8, so it cannot be edited.".to_string(),
            Self::NotFound => "File not found - it may have been moved or deleted.".to_string(),
            Self::PermissionDenied => "Permission denied - this file cannot be read.".to_string(),
            Self::NotAFile => "This path is not a file.".to_string(),
            Self::Io { detail } => format!("This file could not be read: {detail}."),
        }
    }

    /// Whether re-reading the path could plausibly succeed (US-018). Size,
    /// binary and encoding refusals are properties of the bytes themselves, so
    /// a retry can only repeat itself; a missing file, a permission change and
    /// a transient I/O fault can all clear on their own, which is what earns
    /// those states a reload button.
    pub(crate) fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::NotFound | Self::PermissionDenied | Self::NotAFile | Self::Io { .. }
        )
    }
}

/// What reading a file produced: either an open document (possibly read-only)
/// or a written refusal.
pub(crate) type CodeLoad = Result<CodeDocument, CodeLoadError>;

/// A document plus the highlighter built from it, both produced by the same
/// off-thread pass.
pub(crate) struct LoadedCode {
    pub(crate) document: CodeDocument,
    pub(crate) highlighter: CodeHighlighter,
}

/// What a completed open produced (US-002): the pair, or the same written
/// refusal a bare read would have given.
pub(crate) type CodeOpen = Result<LoadedCode, CodeLoadError>;

/// Read `path` and build its document. **Blocking** - the caller runs it inside
/// `smol::unblock` (see [`spawn_code_load`]); it is a free function precisely so
/// the guardrails can be tested without an async runtime.
///
/// Order matters: metadata first (cheapest refusal), then bytes, then the
/// binary sniff, then UTF-8, then the giant-line downgrade. A refusal never
/// panics and never surfaces a raw OS error.
pub(crate) fn load_blocking(path: &Path) -> CodeLoad {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) => return Err(io_error(&err)),
    };
    if !meta.is_file() {
        return Err(CodeLoadError::NotAFile);
    }
    let len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
    if len > MAX_FILE_BYTES {
        return Err(CodeLoadError::TooLarge {
            bytes: len,
            limit: MAX_FILE_BYTES,
        });
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => return Err(io_error(&err)),
    };
    // Re-check after the read: the file may have grown between `metadata` and
    // `read`, and `metadata` reports 0 for several virtual filesystems.
    if bytes.len() > MAX_FILE_BYTES {
        return Err(CodeLoadError::TooLarge {
            bytes: bytes.len(),
            limit: MAX_FILE_BYTES,
        });
    }
    if looks_binary(&bytes) {
        return Err(CodeLoadError::Binary);
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Err(CodeLoadError::NotUtf8),
    };

    Ok(build_document(
        path.to_path_buf(),
        &text,
        is_read_only(&meta),
    ))
}

/// Assemble the document and apply the two read-only rules: a file the process
/// cannot write, and a line past [`MAX_LINE_CHARS`]. Split out of
/// [`load_blocking`] so the rules can be tested without touching the disk.
pub(crate) fn build_document(path: PathBuf, text: &str, read_only_on_disk: bool) -> CodeDocument {
    let mut doc = CodeDocument::new(path, text);
    let longest = doc.longest_line_chars();
    if longest > MAX_LINE_CHARS {
        // The giant line wins over the permission bit: it is the more specific
        // explanation, and both end in the same disabled-editing state.
        doc.set_read_only(Some(ReadOnlyReason::GiantLine {
            chars: longest,
            limit: MAX_LINE_CHARS,
        }));
    } else if read_only_on_disk {
        doc.set_read_only(Some(ReadOnlyReason::Permissions));
    }
    doc
}

/// [`load_blocking`] plus the one full tree-sitter parse the file gets.
/// **Blocking** - the caller runs it inside `smol::unblock` (see
/// [`spawn_code_load`]).
///
/// The initial parse rides the same thread as the read on purpose (US-002): on
/// a source file near [`crate::diff::MAX_HIGHLIGHT_BYTES`] it costs the same
/// order of magnitude as the read itself, so leaving it on the render thread
/// would hand back the stall the off-thread load was there to remove.
pub(crate) fn open_blocking(path: &Path, syntax: DiffSyntax) -> CodeOpen {
    let document = load_blocking(path)?;
    let highlighter = CodeHighlighter::new(&document, syntax);
    Ok(LoadedCode {
        document,
        highlighter,
    })
}

/// A nul byte inside the first [`BINARY_SNIFF_BYTES`] - the `git diff`
/// heuristic. Cheap, and it catches the cases that matter (executables, images,
/// UTF-16 text) without reading the whole file twice.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// Whether the file's metadata says this process cannot write it. On Unix that
/// is the mode bits; on Windows it is the read-only attribute that
/// `Permissions::readonly` already reports. Advisory either way - a save can
/// still fail on an ACL or a read-only mount, which US-015 handles at write
/// time.
fn is_read_only(meta: &std::fs::Metadata) -> bool {
    meta.permissions().readonly()
}

fn io_error(err: &std::io::Error) -> CodeLoadError {
    match err.kind() {
        ErrorKind::NotFound => CodeLoadError::NotFound,
        ErrorKind::PermissionDenied => CodeLoadError::PermissionDenied,
        _ => CodeLoadError::Io {
            detail: err.kind().to_string(),
        },
    }
}

/// Ordering guard for concurrent loads. Holds nothing but a counter: every
/// request takes a generation from [`Self::begin`], and only a result carrying
/// the current generation is accepted by [`Self::accept`].
#[derive(Default)]
pub(crate) struct CodeLoadSlot {
    generation: u64,
}

impl CodeLoadSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Claim the next generation. Every earlier in-flight load is stale from
    /// this point on, whatever order the reads finish in.
    pub(crate) fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    #[allow(dead_code)] // EP-001 accessor: the generation guard is checked through `accept`; no caller reads the counter yet.
    pub(crate) fn current(&self) -> u64 {
        self.generation
    }

    /// `true` when `generation` is still the live one, meaning the caller
    /// should apply the result and repaint. `false` means a newer request
    /// superseded it: drop the result silently, with no state change and no
    /// `cx.notify()`.
    pub(crate) fn accept(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

/// Read `path` and parse it off the GPUI thread, then hand the outcome back to
/// `view` on the main thread, but only if `generation` is still current when
/// it lands. `syntax` is the theme snapshot the highlighter is built against,
/// taken by the caller before the task starts.
///
/// Generic over the hosting view so `code/view.rs` (EP-002) and any later
/// consumer share one loader instead of each re-deriving the spawn + guard
/// dance. `apply` runs on the main thread with `&mut V` and is responsible for
/// its own `cx.notify()`; it is simply never called for a stale result, which
/// is the AC's "ignored without repainting".
pub(crate) fn spawn_code_load<V, F>(
    path: PathBuf,
    generation: u64,
    syntax: DiffSyntax,
    cx: &mut Context<V>,
    apply: F,
) where
    V: 'static,
    F: FnOnce(&mut V, u64, CodeOpen, &mut Context<V>) + 'static,
{
    cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
        let outcome = smol::unblock(move || open_blocking(&path, syntax)).await;
        // A closed tab drops the entity: `update` returns `Err` and the result
        // is discarded. That failure is the expected path, not an error.
        cx.update(|cx| {
            let _ = this.update(cx, |view: &mut V, cx: &mut Context<V>| {
                apply(view, generation, outcome, cx);
            });
        });
    })
    .detach();
}

/// What a file tab shows. The `Loading` variant exists so the tab renders a
/// spinner rather than an empty pane while [`spawn_code_load`] is in flight
/// (US-002); it is replaced by exactly one of the other two when the guarded
/// result lands.
pub(crate) enum CodeLoadState {
    Loading,
    Ready(Box<LoadedCode>),
    Failed(CodeLoadError),
}

impl CodeLoadState {
    /// Fold a guarded open outcome into the state a tab renders.
    pub(crate) fn from_outcome(outcome: CodeOpen) -> Self {
        match outcome {
            Ok(loaded) => Self::Ready(Box::new(loaded)),
            Err(err) => Self::Failed(err),
        }
    }

    pub(crate) fn document(&self) -> Option<&CodeDocument> {
        match self {
            Self::Ready(loaded) => Some(&loaded.document),
            _ => None,
        }
    }

    pub(crate) fn document_mut(&mut self) -> Option<&mut CodeDocument> {
        match self {
            Self::Ready(loaded) => Some(&mut loaded.document),
            _ => None,
        }
    }

    pub(crate) fn highlighter(&self) -> Option<&CodeHighlighter> {
        match self {
            Self::Ready(loaded) => Some(&loaded.highlighter),
            _ => None,
        }
    }

    /// The document and its highlighter together, which is what applying an
    /// edit needs: the rope mutation and the tree edit have to see the same
    /// text.
    pub(crate) fn editable(&mut self) -> Option<(&mut CodeDocument, &mut CodeHighlighter)> {
        match self {
            Self::Ready(loaded) => Some((&mut loaded.document, &mut loaded.highlighter)),
            _ => None,
        }
    }

    #[allow(dead_code)] // EP-001 accessor: the render path matches on `CodeLoadState` directly.
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    /// The sentence to render instead of the file, if it could not be opened.
    pub(crate) fn error_message(&self) -> Option<String> {
        match self {
            Self::Failed(err) => Some(err.message()),
            _ => None,
        }
    }

    /// Whether the current failure, if any, is worth offering a reload for
    /// (US-018).
    pub(crate) fn is_retriable(&self) -> bool {
        matches!(self, Self::Failed(err) if err.is_retriable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-018: every failure the editor can reach renders a written sentence,
    /// never a debug-formatted `std::io::Error`, and each one is classified as
    /// retriable or not so the panel knows whether to offer a reload.
    #[test]
    fn every_load_error_reads_as_prose_and_declares_its_retriability() {
        let cases = [
            (
                CodeLoadError::TooLarge {
                    bytes: 8 * 1024 * 1024,
                    limit: MAX_FILE_BYTES,
                },
                false,
            ),
            (CodeLoadError::Binary, false),
            (CodeLoadError::NotUtf8, false),
            (CodeLoadError::NotFound, true),
            (CodeLoadError::PermissionDenied, true),
            (CodeLoadError::NotAFile, true),
            (io_error(&std::io::Error::other("raw failure")), true),
        ];

        for (error, retriable) in cases {
            let message = error.message();
            assert!(
                message.ends_with('.') && message.chars().next().is_some_and(char::is_uppercase),
                "`{message}` is not a written sentence"
            );
            for leak in ["Os {", "Custom {", "kind:", "raw failure", "Error"] {
                assert!(
                    !message.contains(leak),
                    "`{message}` leaks the technical error (`{leak}`)"
                );
            }
            assert_eq!(
                error.is_retriable(),
                retriable,
                "wrong retriability for `{message}`"
            );
            assert_eq!(
                CodeLoadState::Failed(error).is_retriable(),
                retriable,
                "the state must mirror its error"
            );
        }

        // A load still in flight shows the loader, not an error panel.
        assert!(CodeLoadState::Loading.error_message().is_none());
        assert!(!CodeLoadState::Loading.is_retriable());
    }

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    #[test]
    fn a_plain_file_opens_editable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "main.rs", b"fn main() {}\n");
        let doc = load_blocking(&path).expect("load");
        assert_eq!(doc.ext(), "rs");
        assert_eq!(doc.line_count(), 2);
        assert!(!doc.is_read_only());
    }

    #[test]
    fn a_file_past_ten_megabytes_is_refused_with_its_size_and_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "huge.txt", &vec![b'a'; MAX_FILE_BYTES + 1]);
        let err = load_blocking(&path).expect_err("refused");
        assert_eq!(
            err,
            CodeLoadError::TooLarge {
                bytes: MAX_FILE_BYTES + 1,
                limit: MAX_FILE_BYTES,
            }
        );
        let message = err.message();
        assert!(message.contains("10 MB"), "{message}");
        // Aligned with the markdown viewer rather than a second private limit.
        assert_eq!(MAX_FILE_BYTES, crate::markdown::MAX_INPUT_BYTES);
    }

    #[test]
    fn a_line_past_ten_thousand_characters_opens_read_only_with_a_banner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut bytes = vec![b'x'; MAX_LINE_CHARS + 1];
        bytes.push(b'\n');
        let path = write(&dir, "bundle.js", &bytes);

        let mut doc = load_blocking(&path).expect("load");
        // The file still opens and is still readable - only editing is off.
        assert_eq!(doc.line_count(), 2);
        let reason = doc.read_only_reason().expect("read-only");
        assert_eq!(
            reason,
            ReadOnlyReason::GiantLine {
                chars: MAX_LINE_CHARS + 1,
                limit: MAX_LINE_CHARS,
            }
        );
        let banner = reason.banner();
        assert!(banner.contains("10000-character editing limit"), "{banner}");
        // And the keystroke is refused rather than silently swallowed: the
        // caller gets `None` back and shows the banner above.
        assert!(doc.insert(0, "a").is_none());
    }

    #[test]
    fn a_non_utf8_file_is_refused_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Latin-1 "café" - 0xE9 is not valid UTF-8, and there is no nul byte,
        // so this exercises the `from_utf8` arm rather than the binary sniff.
        let path = write(&dir, "latin1.txt", &[b'c', b'a', b'f', 0xE9, b'\n']);
        let err = load_blocking(&path).expect_err("refused");
        assert_eq!(err, CodeLoadError::NotUtf8);
        assert!(err.message().contains("not valid UTF-8"));
    }

    #[test]
    fn a_nul_byte_in_the_first_eight_kilobytes_is_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut bytes = vec![b'a'; 4096];
        bytes.push(0);
        bytes.extend_from_slice(&[b'b'; 4096]);
        let path = write(&dir, "blob.bin", &bytes);
        let err = load_blocking(&path).expect_err("refused");
        assert_eq!(err, CodeLoadError::Binary);
        assert!(err.message().starts_with("Binary file"));
    }

    #[test]
    fn a_nul_byte_past_the_sniff_window_does_not_make_a_file_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut bytes = vec![b'a'; BINARY_SNIFF_BYTES];
        bytes.push(0);
        let path = write(&dir, "late-nul.txt", &bytes);
        // It is not binary by the sniff, but a nul byte is still valid UTF-8,
        // so the file opens - matching `git diff`'s heuristic exactly.
        assert!(load_blocking(&path).is_ok());
    }

    #[test]
    fn a_file_deleted_between_the_click_and_the_load_says_file_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "gone.rs", b"fn main() {}\n");
        std::fs::remove_file(&path).expect("remove");
        let err = load_blocking(&path).expect_err("refused");
        assert_eq!(err, CodeLoadError::NotFound);
        assert!(err.message().starts_with("File not found"));
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = load_blocking(dir.path()).expect_err("refused");
        assert_eq!(err, CodeLoadError::NotAFile);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_write_permission_opens_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "locked.rs", b"fn main() {}\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).expect("chmod 444");

        let mut doc = load_blocking(&path).expect("load");
        assert_eq!(doc.read_only_reason(), Some(ReadOnlyReason::Permissions));
        assert!(doc.insert(0, "x").is_none());
    }

    #[test]
    fn the_giant_line_reason_wins_over_the_permission_bit() {
        let text = format!("{}\n", "x".repeat(MAX_LINE_CHARS + 1));
        let doc = build_document(PathBuf::from("/tmp/min.js"), &text, true);
        assert!(matches!(
            doc.read_only_reason(),
            Some(ReadOnlyReason::GiantLine { .. })
        ));
    }

    #[test]
    fn every_refusal_renders_a_sentence_rather_than_a_raw_error() {
        for err in [
            CodeLoadError::TooLarge {
                bytes: 20 * 1024 * 1024,
                limit: MAX_FILE_BYTES,
            },
            CodeLoadError::Binary,
            CodeLoadError::NotUtf8,
            CodeLoadError::NotFound,
            CodeLoadError::PermissionDenied,
            CodeLoadError::NotAFile,
            CodeLoadError::Io {
                detail: "input/output error".to_string(),
            },
        ] {
            let message = err.message();
            assert!(message.ends_with('.'), "{message}");
            assert!(message.len() > 15, "{message}");
        }
    }

    // US-002: the ordering rule, tested without an app because `CodeLoadSlot`
    // deliberately holds no GPUI type.

    #[test]
    fn the_stale_result_of_two_concurrent_loads_is_rejected() {
        let mut slot = CodeLoadSlot::new();
        let first = slot.begin();
        let second = slot.begin();
        assert_ne!(first, second);

        // The first read finishes last (a cold file, a slow disk): it must be
        // dropped, whatever order the tasks land in.
        assert!(!slot.accept(first));
        assert!(slot.accept(second));
        // And it stays rejected on a retry - the guard is not one-shot.
        assert!(!slot.accept(first));
        assert_eq!(slot.current(), second);
    }

    #[test]
    fn a_slot_accepts_its_own_generation_until_a_newer_one_starts() {
        let mut slot = CodeLoadSlot::new();
        let generation = slot.begin();
        assert!(slot.accept(generation));
        slot.begin();
        assert!(!slot.accept(generation));
    }

    #[test]
    fn load_state_folds_an_outcome_into_what_the_tab_renders() {
        let mut loading = CodeLoadState::Loading;
        assert!(loading.is_loading());
        assert!(loading.document().is_none());
        assert!(loading.error_message().is_none());

        loading = CodeLoadState::from_outcome(Ok(loaded("/tmp/a.rs", "fn main() {}\n")));
        assert!(!loading.is_loading());
        assert!(loading.document_mut().is_some());
        assert!(loading.highlighter().is_some());
        assert!(loading.editable().is_some());

        let failed = CodeLoadState::from_outcome(Err(CodeLoadError::NotFound));
        assert_eq!(
            failed.error_message().as_deref(),
            Some("File not found - it may have been moved or deleted.")
        );
    }

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&crate::theme::paneflow_dark())
    }

    fn loaded(path: &str, text: &str) -> LoadedCode {
        let document = CodeDocument::new(PathBuf::from(path), text);
        let highlighter = CodeHighlighter::new(&document, syntax());
        LoadedCode {
            document,
            highlighter,
        }
    }

    /// US-002: the read, the rope and the initial parse are one blocking unit,
    /// so `smol::unblock` carries all three off the render thread. Proven by
    /// the highlighter coming back already colored, without any main-thread
    /// parse call in between.
    #[test]
    fn opening_a_file_parses_it_in_the_same_blocking_pass_as_the_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "main.rs", b"fn main() {\n    let x = 1;\n}\n");

        let opened = open_blocking(&path, syntax()).expect("open");

        assert_eq!(opened.document.line_count(), 4);
        assert!(opened.highlighter.is_enabled());
        assert!(
            !opened.highlighter.runs(0).is_empty(),
            "the initial parse ran inside open_blocking, not later on the render thread"
        );
    }

    /// A refusal short-circuits before the parse: no grammar work is done for a
    /// file the editor will not open.
    #[test]
    fn a_refused_file_never_reaches_the_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "blob.rs", b"fn main\0() {}\n");

        match open_blocking(&path, syntax()) {
            Err(err) => assert_eq!(err, CodeLoadError::Binary),
            Ok(_) => panic!("a binary file must not open"),
        }
    }
}

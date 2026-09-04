use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

#[cfg(test)]
use gpui::AppContext;
use gpui::{AsyncApp, Context, WeakEntity};

use super::document::{CodeDocument, ReadOnlyReason};
use super::edit::IndentUnit;
use super::highlight::CodeHighlighter;
use super::save::FileStamp;
use crate::diff::DiffSyntax;

pub(crate) const MAX_FILE_BYTES: usize = crate::markdown::MAX_INPUT_BYTES;

pub(crate) const MAX_LINE_CHARS: usize = 10_000;

pub(crate) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum CodeLoadError {
    TooLarge { bytes: usize, limit: usize },
    Binary,
    NotUtf8,
    NotFound,
    PermissionDenied,
    NotAFile,
    Io { detail: String },
}

impl CodeLoadError {
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

    pub(crate) fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::NotFound | Self::PermissionDenied | Self::NotAFile | Self::Io { .. }
        )
    }
}

#[cfg(test)]
pub(crate) type CodeLoad = Result<CodeDocument, CodeLoadError>;

pub(crate) struct LoadedCode {
    pub(crate) document: CodeDocument,
    pub(crate) highlighter: CodeHighlighter,
    pub(crate) indent: IndentUnit,
    pub(crate) stamp: Option<FileStamp>,
}

pub(crate) type CodeOpen = Result<LoadedCode, CodeLoadError>;

#[cfg(test)]
pub(crate) fn load_blocking(path: &Path) -> CodeLoad {
    load_document_and_stamp(path).map(|(document, _)| document)
}

fn load_document_and_stamp(path: &Path) -> Result<(CodeDocument, FileStamp), CodeLoadError> {
    let path_meta = std::fs::metadata(path).map_err(|err| io_error(&err))?;
    if !path_meta.is_file() {
        return Err(CodeLoadError::NotAFile);
    }
    let mut file = File::open(path).map_err(|err| io_error(&err))?;
    let meta = file.metadata().map_err(|err| io_error(&err))?;
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

    let mut bytes = Vec::with_capacity(len);
    file.read_to_end(&mut bytes).map_err(|err| io_error(&err))?;
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

    let stamp = FileStamp::from_metadata(&meta);
    let document = build_document(path.to_path_buf(), &text, is_read_only(&meta));
    Ok((document, stamp))
}

pub(crate) fn build_document(path: PathBuf, text: &str, read_only_on_disk: bool) -> CodeDocument {
    let mut doc = CodeDocument::new(path, text);
    let longest = doc.longest_line_chars();
    if longest > MAX_LINE_CHARS {
        doc.set_read_only(Some(ReadOnlyReason::GiantLine {
            chars: longest,
            limit: MAX_LINE_CHARS,
        }));
    } else if read_only_on_disk {
        doc.set_read_only(Some(ReadOnlyReason::Permissions));
    }
    doc
}

pub(crate) fn open_blocking(path: &Path, syntax: DiffSyntax) -> CodeOpen {
    let (document, stamp) = load_document_and_stamp(path)?;
    let indent = IndentUnit::detect(&document);
    let highlighter = CodeHighlighter::new(&document, syntax);
    Ok(LoadedCode {
        document,
        highlighter,
        indent,
        stamp: Some(stamp),
    })
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

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

#[derive(Default)]
pub(crate) struct CodeLoadSlot {
    generation: u64,
}

impl CodeLoadSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    #[allow(dead_code)]
    pub(crate) fn current(&self) -> u64 {
        self.generation
    }

    pub(crate) fn accept(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

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
        #[cfg(not(test))]
        let outcome = smol::unblock(move || open_blocking(&path, syntax)).await;
        #[cfg(test)]
        let outcome = cx
            .background_spawn(async move { open_blocking(&path, syntax) })
            .await;
        cx.update(|cx| {
            let _ = this.update(cx, |view: &mut V, cx: &mut Context<V>| {
                apply(view, generation, outcome, cx);
            });
        });
    })
    .detach();
}

pub(crate) enum CodeLoadState {
    Loading,
    Ready(Box<LoadedCode>),
    Failed(CodeLoadError),
}

impl CodeLoadState {
    #[cfg(test)]
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

    pub(crate) fn editable(&mut self) -> Option<(&mut CodeDocument, &mut CodeHighlighter)> {
        match self {
            Self::Ready(loaded) => Some((&mut loaded.document, &mut loaded.highlighter)),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub(crate) fn error_message(&self) -> Option<String> {
        match self {
            Self::Failed(err) => Some(err.message()),
            _ => None,
        }
    }

    pub(crate) fn is_retriable(&self) -> bool {
        matches!(self, Self::Failed(err) if err.is_retriable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(MAX_FILE_BYTES, crate::markdown::MAX_INPUT_BYTES);
    }

    #[test]
    fn a_line_past_ten_thousand_characters_opens_read_only_with_a_banner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut bytes = vec![b'x'; MAX_LINE_CHARS + 1];
        bytes.push(b'\n');
        let path = write(&dir, "bundle.js", &bytes);

        let mut doc = load_blocking(&path).expect("load");
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
        assert!(doc.insert(0, "a").is_none());
    }

    #[test]
    fn a_non_utf8_file_is_refused_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
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

    #[test]
    fn the_stale_result_of_two_concurrent_loads_is_rejected() {
        let mut slot = CodeLoadSlot::new();
        let first = slot.begin();
        let second = slot.begin();
        assert_ne!(first, second);

        assert!(!slot.accept(first));
        assert!(slot.accept(second));
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
        let indent = IndentUnit::detect(&document);
        let highlighter = CodeHighlighter::new(&document, syntax());
        LoadedCode {
            document,
            highlighter,
            indent,
            stamp: None,
        }
    }

    #[test]
    fn opening_a_file_leaves_its_first_parse_to_the_deferred_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            &dir,
            "main.rs",
            b"fn main() {\n\tlet x = 1;\n\tlet y = 2;\n}\n",
        );

        let mut opened = open_blocking(&path, syntax()).expect("open");

        assert_eq!(opened.document.line_count(), 5);
        assert!(opened.highlighter.is_enabled());
        assert_eq!(opened.indent, IndentUnit::Tab);
        assert_eq!(opened.stamp, FileStamp::read(&path));
        assert!(
            !opened.highlighter.has_tree(),
            "open_blocking returns the text before any parse has run"
        );
        assert!(opened.highlighter.runs(0).is_empty());

        assert!(
            opened.highlighter.parse_initial_blocking(&opened.document),
            "the deferred initial parse applies to the highlighter it came from"
        );
        assert!(
            opened.highlighter.has_tree(),
            "and the tree arrives with it"
        );
    }

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

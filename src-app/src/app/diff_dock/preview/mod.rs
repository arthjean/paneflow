pub(crate) mod archive;
pub(crate) mod image;
pub(crate) mod large_text;
pub(crate) mod no_preview;
pub(crate) mod table;

use std::io::Read;
use std::path::Path;

use super::code::load::{
    BINARY_SNIFF_BYTES, CodeLoadError, LoadedCode, MAX_FILE_BYTES, io_error, open_blocking,
};
use crate::diff::DiffSyntax;

pub(crate) const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "svg",
];

pub(crate) const UNSUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["avif", "heic", "heif"];

pub(crate) const MARKDOWN_EXTENSIONS: &[&str] = &["md", "mdx", "markdown"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewKind {
    Image,
    Table(Delimiter),
    Markdown,
    Archive(ArchiveFormat),
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delimiter {
    Comma,
    Tab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
}

fn lowered_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn lowered_extension(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

pub(crate) fn preview_kind(path: &Path) -> PreviewKind {
    let ext = lowered_extension(path);
    let name = lowered_name(path);
    if IMAGE_EXTENSIONS.contains(&ext.as_str())
        || UNSUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str())
    {
        return PreviewKind::Image;
    }
    match ext.as_str() {
        "csv" => return PreviewKind::Table(Delimiter::Comma),
        "tsv" => return PreviewKind::Table(Delimiter::Tab),
        _ => {}
    }
    if MARKDOWN_EXTENSIONS.contains(&ext.as_str()) {
        return PreviewKind::Markdown;
    }
    if name.ends_with(".tar.gz") || ext == "tgz" {
        return PreviewKind::Archive(ArchiveFormat::TarGz);
    }
    match ext.as_str() {
        "zip" | "jar" => PreviewKind::Archive(ArchiveFormat::Zip),
        "tar" => PreviewKind::Archive(ArchiveFormat::Tar),
        _ => PreviewKind::Text,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NoPreview {
    pub(crate) size: Option<u64>,
    pub(crate) reason: Option<String>,
}

impl NoPreview {
    pub(crate) fn new(size: Option<u64>, reason: impl Into<String>) -> Self {
        Self {
            size,
            reason: Some(reason.into()),
        }
    }

    pub(crate) fn binary(size: u64) -> Self {
        Self::new(
            Some(size),
            "This file is binary, so it can't be shown as text.",
        )
    }

    pub(crate) fn not_utf8(size: u64) -> Self {
        Self::new(
            Some(size),
            "This file is not valid UTF-8, so it can't be shown as text.",
        )
    }

    pub(crate) fn special() -> Self {
        Self::new(
            None,
            "This is a special file (a pipe, socket or device), so it is not read.",
        )
    }
}

pub(crate) enum FilePreview {
    NoPreview(NoPreview),
    LargeText(gpui::Entity<large_text::LargeTextView>),
    Image(gpui::Entity<image::ImageView>),
    Archive(gpui::Entity<archive::ArchiveView>),
}

pub(crate) enum OpenedFile {
    Code(Box<LoadedCode>),
    LargeText { size: u64 },
    Image(image::ImageProbe),
    Archive { format: ArchiveFormat, size: u64 },
    NoPreview(NoPreview),
}

pub(crate) type FileOpenOutcome = Result<OpenedFile, CodeLoadError>;

pub(crate) fn open_file_blocking(path: &Path, syntax: DiffSyntax) -> FileOpenOutcome {
    let meta = std::fs::metadata(path).map_err(|err| io_error(&err))?;
    if meta.is_dir() {
        return Err(CodeLoadError::NotAFile);
    }
    if !meta.is_file() {
        return Ok(OpenedFile::NoPreview(NoPreview::special()));
    }
    let size = meta.len();
    match preview_kind(path) {
        PreviewKind::Image => Ok(image::probe(path, size)),
        PreviewKind::Archive(format) => Ok(archive::admit(format, size)),
        PreviewKind::Table(_) | PreviewKind::Markdown | PreviewKind::Text => {
            open_text(path, size, syntax)
        }
    }
}

fn open_text(path: &Path, size: u64, syntax: DiffSyntax) -> FileOpenOutcome {
    if size > MAX_FILE_BYTES as u64 {
        return open_large_text(path, size);
    }
    match open_blocking(path, syntax) {
        Ok(loaded) => Ok(OpenedFile::Code(Box::new(loaded))),
        Err(CodeLoadError::Binary) => Ok(OpenedFile::NoPreview(NoPreview::binary(size))),
        Err(CodeLoadError::NotUtf8) => Ok(OpenedFile::NoPreview(NoPreview::not_utf8(size))),
        Err(CodeLoadError::TooLarge { bytes, .. }) => open_large_text(path, bytes as u64),
        Err(err) => Err(err),
    }
}

fn open_large_text(path: &Path, size: u64) -> FileOpenOutcome {
    if let Some(refusal) = large_text::refusal_for_size(size) {
        return Ok(OpenedFile::NoPreview(refusal));
    }
    let head = read_head(path, BINARY_SNIFF_BYTES)?;
    if head.contains(&0) {
        return Ok(OpenedFile::NoPreview(NoPreview::binary(size)));
    }
    Ok(OpenedFile::LargeText { size })
}

pub(crate) fn read_head(path: &Path, limit: usize) -> Result<Vec<u8>, CodeLoadError> {
    let file = paneflow_home::open_for_reading(path).map_err(|err| io_error(&err))?;
    let mut head = Vec::with_capacity(limit);
    file.take(limit as u64)
        .read_to_end(&mut head)
        .map_err(|err| io_error(&err))?;
    Ok(head)
}

pub(crate) fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return if bytes == 1 {
            "1 byte".to_string()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let rounded = (value * 10.0).round() / 10.0;
    if value >= 100.0 || rounded.fract() == 0.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{rounded:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const RETIRED_BINARY_EXTENSIONS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "avif", "tiff", "tif", "psd", "heic",
        "mp3", "wav", "flac", "ogg", "m4a", "aac", "mp4", "mkv", "mov", "avi", "webm", "wmv",
        "zip", "gz", "bz2", "xz", "zst", "7z", "rar", "tar", "tgz", "jar", "whl", "exe", "dll",
        "so", "dylib", "o", "a", "lib", "obj", "pdb", "class", "wasm", "bin", "msi", "appimage",
        "dmg", "deb", "rpm", "ttf", "otf", "woff", "woff2", "eot", "pdf", "db", "sqlite",
        "sqlite3", "bcmap", "pyc",
    ];

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&crate::theme::paneflow_dark())
    }

    fn kind(name: &str) -> PreviewKind {
        preview_kind(&PathBuf::from(format!("/w/{name}")))
    }

    #[test]
    fn the_viewer_is_chosen_by_extension_in_the_documented_order() {
        for name in [
            "a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp", "a.bmp", "a.ico", "a.tif", "a.tiff",
            "a.svg", "a.avif", "a.heic",
        ] {
            assert_eq!(kind(name), PreviewKind::Image, "{name}");
        }
        assert_eq!(kind("data.csv"), PreviewKind::Table(Delimiter::Comma));
        assert_eq!(kind("data.TSV"), PreviewKind::Table(Delimiter::Tab));
        for name in ["README.md", "page.mdx", "notes.markdown"] {
            assert_eq!(kind(name), PreviewKind::Markdown, "{name}");
        }
        assert_eq!(kind("a.zip"), PreviewKind::Archive(ArchiveFormat::Zip));
        assert_eq!(kind("a.jar"), PreviewKind::Archive(ArchiveFormat::Zip));
        assert_eq!(kind("a.tar"), PreviewKind::Archive(ArchiveFormat::Tar));
        assert_eq!(kind("a.tar.gz"), PreviewKind::Archive(ArchiveFormat::TarGz));
        assert_eq!(kind("a.tgz"), PreviewKind::Archive(ArchiveFormat::TarGz));
        for name in ["main.rs", "Makefile", "a.gz", "a.pdf", "a.exe"] {
            assert_eq!(kind(name), PreviewKind::Text, "{name}");
        }
    }

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    fn no_preview(outcome: FileOpenOutcome) -> NoPreview {
        match outcome {
            Ok(OpenedFile::NoPreview(screen)) => screen,
            Ok(_) => panic!("expected the no-preview screen"),
            Err(err) => panic!("expected the no-preview screen, got {err:?}"),
        }
    }

    #[test]
    fn a_binary_file_opens_on_the_no_preview_screen_with_its_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "blob.bin", b"MZ\0\0\x90\0");
        let screen = no_preview(open_file_blocking(&path, syntax()));
        assert_eq!(screen.size, Some(6));
        assert!(screen.reason.expect("reason").contains("binary"));
    }

    #[test]
    fn a_non_utf8_text_file_opens_on_the_no_preview_screen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "latin1.txt", &[b'c', b'a', b'f', 0xE9, b'\n']);
        let screen = no_preview(open_file_blocking(&path, syntax()));
        assert!(screen.reason.expect("reason").contains("UTF-8"));
    }

    #[test]
    fn every_retired_binary_extension_opens_on_something_other_than_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        for ext in RETIRED_BINARY_EXTENSIONS {
            let path = write(
                &dir,
                &format!("sample.{ext}"),
                b"\x00\x01\x02garbage\xff\xfe",
            );
            match open_file_blocking(&path, syntax()) {
                Ok(_) => {}
                Err(err) => panic!(".{ext} ended on the error {err:?}"),
            }
        }
    }

    #[test]
    fn a_file_deleted_before_the_open_reports_that_it_no_longer_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(&dir, "gone.rs", b"fn main() {}\n");
        std::fs::remove_file(&path).expect("remove");
        match open_file_blocking(&path, syntax()) {
            Err(err) => {
                assert_eq!(err, CodeLoadError::NotFound);
                assert!(err.is_retriable());
                assert!(err.message().starts_with("File no longer exists"));
            }
            Ok(_) => panic!("a deleted file must fail to open"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_opens_on_the_no_preview_screen_without_being_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        assert!(made, "mkfifo {}", path.display());
        let screen = no_preview(open_file_blocking(&path, syntax()));
        assert_eq!(screen.size, None);
        assert!(screen.reason.expect("reason").contains("special file"));
    }

    #[test]
    fn a_small_text_file_opens_in_the_editor_and_a_large_one_in_the_large_viewer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let small = write(&dir, "small.log", b"one\ntwo\n");
        assert!(matches!(
            open_file_blocking(&small, syntax()),
            Ok(OpenedFile::Code(_))
        ));
        let big = write(&dir, "big.log", &vec![b'a'; MAX_FILE_BYTES + 1]);
        assert!(matches!(
            open_file_blocking(&big, syntax()),
            Ok(OpenedFile::LargeText { size }) if size == MAX_FILE_BYTES as u64 + 1
        ));
        let mut binary = vec![b'a'; MAX_FILE_BYTES + 1];
        binary[10] = 0;
        let big_binary = write(&dir, "big.dat", &binary);
        assert!(
            no_preview(open_file_blocking(&big_binary, syntax()))
                .reason
                .expect("reason")
                .contains("binary")
        );
    }

    #[test]
    fn a_large_csv_or_markdown_falls_back_to_the_large_text_viewer() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["big.csv", "big.md"] {
            let path = write(&dir, name, &vec![b'a'; MAX_FILE_BYTES + 1]);
            assert!(
                matches!(
                    open_file_blocking(&path, syntax()),
                    Ok(OpenedFile::LargeText { .. })
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn counts_group_their_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(5_000_000), "5,000,000");
    }

    #[test]
    fn sizes_read_as_short_prose() {
        assert_eq!(format_size(1), "1 byte");
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(2048), "2 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(200 * 1024 * 1024), "200 MB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5 GB");
    }
}

//! URL and file-path detection for the terminal renderer.
//!
//! Two scanners share the same line-scoped, char-to-column mapped pattern:
//! - `detect_urls_on_line_mapped`  - Zed-style URL regex (US-015).
//! - `detect_file_paths_on_line_mapped` - `.md` / `.markdown` paths with
//!   existence check + heuristics (US-019).
//!
//! Both return `HyperlinkZone`; the scheme allowlist (`is_url_scheme_openable`)
//! guards what `TerminalView` will actually open.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use crate::terminal::types::{HyperlinkSource, HyperlinkZone};

/// URL regex pattern matching Zed's terminal_hyperlinks.rs.
/// Excludes C0/C1 control chars, whitespace, angle brackets, quotes, and other
/// non-URL characters. Box-drawing chars (U+2500-U+257F) are not valid URL
/// characters and won't match the allowed character class.
pub(super) const URL_REGEX_PATTERN: &str = r#"(mailto:|gemini://|gopher://|https://|http://|news:|git://|ssh:|ftp://|ipfs:|ipns:|magnet:)[^\x00-\x1f\x7f-\x9f<>"\s{}\^⟨⟩`']+"#;

/// Lazily compiled URL regex (compiled once, reused across all calls).
pub(super) fn url_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(URL_REGEX_PATTERN).expect("URL regex compilation failed"))
}

/// Detect URLs on a single terminal line via regex with char-to-column mapping.
/// `char_to_col` maps each character index in `line_text` to its grid column,
/// accounting for wide-char spacers that were skipped during text extraction.
pub fn detect_urls_on_line_mapped(
    line_text: &str,
    line: crate::terminal::types::Line,
    char_to_col: &[usize],
) -> Vec<HyperlinkZone> {
    let re = url_regex();
    re.find_iter(line_text)
        .filter_map(|m| {
            // Convert byte offsets to char indices for column lookup
            let char_start = line_text[..m.start()].chars().count();
            // US-020: trim trailing punctuation / unbalanced close-parens the
            // regex over-captures in prose (e.g. `(see https://x.com/p).`).
            // `char_end` is recomputed from the TRIMMED length so the hover
            // zone ends on the last clickable char, not on the stripped tail.
            let trimmed = sanitize_url_punctuation(m.as_str());
            let char_end = (char_start + trimmed.chars().count()).saturating_sub(1);
            let col_start = char_to_col.get(char_start)?;
            let col_end = char_to_col.get(char_end)?;
            let uri = trimmed.to_string();
            let is_openable = is_url_scheme_openable(&uri);
            Some(HyperlinkZone {
                uri,
                id: String::new(),
                start: crate::terminal::types::Point::new(line.0, *col_start),
                end: crate::terminal::types::Point::new(line.0, *col_end),
                is_openable,
                source: HyperlinkSource::Regex,
                line: None,
                col: None,
            })
        })
        .collect()
}

/// Strip trailing punctuation a URL almost never intends when it appears in
/// free-form prose (US-020). Returns a sub-slice of the input (zero alloc).
///
/// Algorithm mirrors Zed `alacritty/hyperlinks.rs::sanitize_url_punctuation`,
/// adapted to operate on a `&str` instead of a grid `Match`:
/// - count `(`/`)` and `[`/`]` over the WHOLE match first (so balanced pairs
///   are known before trimming);
/// - walk from the right, stripping `. , : ; ! ? ( [` unconditionally and a
///   trailing `)` / `]` only while its closes still exceed its opens.
///
/// So `https://example.com/path).` → `https://example.com/path`, but
/// `https://en.wikipedia.org/wiki/Example_(disambiguation)` is preserved.
/// Paneflow extends Zed with `! ?` (PRD) and `]` (Markdown link tails).
pub(super) fn sanitize_url_punctuation(url: &str) -> &str {
    let (open_parens, mut close_parens, open_brackets, mut close_brackets) = url.chars().fold(
        (0usize, 0usize, 0usize, 0usize),
        |(op, cp, ob, cb), c| match c {
            '(' => (op + 1, cp, ob, cb),
            ')' => (op, cp + 1, ob, cb),
            '[' => (op, cp, ob + 1, cb),
            ']' => (op, cp, ob, cb + 1),
            _ => (op, cp, ob, cb),
        },
    );

    let mut end = url.len();
    while let Some(last) = url[..end].chars().next_back() {
        let strip = match last {
            '.' | ',' | ':' | ';' | '!' | '?' | '(' | '[' => true,
            ')' if close_parens > open_parens => {
                close_parens -= 1;
                true
            }
            ']' if close_brackets > open_brackets => {
                close_brackets -= 1;
                true
            }
            _ => false,
        };
        if !strip {
            break;
        }
        end -= last.len_utf8();
    }
    &url[..end]
}

/// Check if a URL scheme is in the allowlist for opening.
///
/// Mirrors the regex above: all schemes captured by `URL_REGEX_PATTERN` are
/// considered openable, since `open::that` ultimately defers to the OS handler
/// (`xdg-open` / `open` / `start`) which knows whether a scheme is registered.
/// `file://` is intentionally excluded from the generic URL path. Local files
/// must go through the canonicalized file/code scanners instead of OS scheme
/// dispatch.
pub fn is_url_scheme_openable(uri: &str) -> bool {
    if uri.starts_with("http://")
        || uri.starts_with("https://")
        || uri.starts_with("mailto:")
        || uri.starts_with("gemini://")
        || uri.starts_with("gopher://")
        || uri.starts_with("news:")
        || uri.starts_with("git://")
        || uri.starts_with("ssh:")
        || uri.starts_with("ftp://")
        || uri.starts_with("ipfs:")
        || uri.starts_with("ipns:")
        || uri.starts_with("magnet:")
    {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// File-path scanner (US-019)
// ---------------------------------------------------------------------------

const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown"];

fn markdown_extension_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\.(?:md|markdown)\b")
            .expect("markdown-extension regex compilation failed")
    })
}

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rb", "java", "kt", "swift", "c",
    "cpp", "cc", "cxx", "h", "hpp", "cs", "php", "sh", "bash", "zsh", "fish", "lua", "sql", "toml",
    "yaml", "yml", "json", "jsonc", "html", "htm", "css", "scss", "sass", "vue", "svelte", "dart",
    "scala", "clj", "cljs", "hs", "ml", "ex", "exs", "erl", "nim", "zig", "sol", "xml", "gradle",
    "vim", "conf", "ini", "env",
];

fn code_extension_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let extensions = CODE_EXTENSIONS.join("|");
        regex::Regex::new(&format!(r"(?i)\.(?:{extensions})\b"))
            .expect("code-extension regex compilation failed")
    })
}

fn is_path_start_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '[' | '<' | '\'' | '"' | '`' | '{')
}

fn candidate_start_positions(line_text: &str, ext_start: usize) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, ch) in line_text[..ext_start].char_indices() {
        if is_path_start_boundary(ch) {
            let next = idx + ch.len_utf8();
            if next < ext_start {
                starts.push(next);
            }
        }
    }
    starts
}

fn extension_tail_ok(line_text: &str, ext_end: usize) -> bool {
    !line_text[ext_end..].starts_with('.')
}

/// Minimum stem length (basename without extension) for candidates that have
/// no path separator. `123.md` → rejected; `/foo/bar.md` accepted regardless.
const MIN_BARE_STEM_LEN: usize = 4;

/// Returns true if `path_str` looks like a Windows absolute path (`C:\foo` or `C:/foo`).
fn is_windows_absolute(path_str: &str) -> bool {
    let bytes = path_str.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    if path_str.starts_with("\\\\?\\") || path_str.starts_with("\\\\.\\") {
        return true;
    }
    if path_str.starts_with("\\\\") || path_str.starts_with("//") {
        let normalized = path_str.replace('\\', "/");
        let mut parts = normalized.trim_start_matches('/').split('/');
        return parts.next().is_some_and(|p| !p.is_empty())
            && parts.next().is_some_and(|p| !p.is_empty());
    }
    false
}

/// Returns true if `path_str` looks like a POSIX absolute path (`/foo`).
fn is_posix_absolute(path_str: &str) -> bool {
    path_str.starts_with('/')
}

/// Returns true if any character in `s` is a C0/C1 control or DEL.
fn contains_control_char(s: &str) -> bool {
    s.chars()
        .any(|c| (c as u32) < 0x20 || (0x7f..=0x9f).contains(&(c as u32)))
}

/// Returns the file stem (portion before the last `.`) length in chars.
/// For multi-segment paths, only the basename is considered.
/// Example: `/foo/bar/README.md` → 6 (the stem `README`).
fn stem_len(path_str: &str) -> usize {
    let basename = path_str
        .rsplit_once(['/', '\\'])
        .map(|(_, name)| name)
        .unwrap_or(path_str);
    let stem = basename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(basename);
    stem.chars().count()
}

/// Returns true if `candidate` is prefixed with a URL-style scheme (`http:`,
/// `file:`, `mailto:`, `ssh:`, …) - i.e. two or more ASCII letters followed by
/// `:`. Single-letter prefixes (`C:`, `D:`) are Windows drive letters, not
/// schemes, and are NOT classified as schemes here. Used to bar terminal
/// output like `file:///etc/passwd.md` from being passed to `open::that`,
/// where `xdg-open` would honour the URI scheme rather than treat it as a
/// local file.
fn has_url_scheme_prefix(candidate: &str) -> bool {
    let Some(colon_idx) = candidate.find(':') else {
        return false;
    };
    let prefix = &candidate[..colon_idx];
    prefix.len() >= 2 && prefix.chars().all(|c| c.is_ascii_alphabetic())
}

fn expand_tilde_path(path_str: &str) -> Option<PathBuf> {
    if path_str == "~" {
        return dirs::home_dir();
    }
    let rest = path_str
        .strip_prefix("~/")
        .or_else(|| path_str.strip_prefix("~\\"))?;
    dirs::home_dir().map(|home| home.join(rest))
}

/// Resolve `path_str` against `cwd` and canonicalize the result. Returns the
/// canonical absolute path when:
/// - the candidate is a POSIX or Windows absolute path that exists, or
/// - the candidate is relative and joins-with-`cwd` to an existing path.
///
/// `Path::canonicalize` resolves symlinks, normalises `..`/`.` segments, and
/// returns `Err` when the file does not exist - combining the existence check
/// with normalisation in a single call. The canonicalised string is what gets
/// passed to `open::that`, so the user opens the actual resolved target rather
/// than a misleading traversal path printed by the terminal.
fn resolve_path(path_str: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let candidate = if let Some(expanded) = expand_tilde_path(path_str) {
        expanded
    } else if is_posix_absolute(path_str) || is_windows_absolute(path_str) {
        PathBuf::from(path_str)
    } else {
        let cwd = cwd?;
        cwd.join(path_str)
    };
    candidate.canonicalize().ok()
}

/// Returns true if `path` ends with `.md` or `.markdown` (case-insensitive),
/// after canonicalisation may have changed the byte sequence (e.g. case fold
/// on Windows). Used as a defence-in-depth check after `canonicalize` so a
/// symlink target without the right extension cannot be opened as if it were
/// a markdown file.
fn canonical_has_md_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| MARKDOWN_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn has_path_separator(path_str: &str) -> bool {
    path_str.contains('/') || path_str.contains('\\')
}

fn validated_path_candidate(
    path_str: &str,
    cwd: Option<&Path>,
    extension_ok: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if path_str.is_empty() || contains_control_char(path_str) {
        return None;
    }
    if has_url_scheme_prefix(path_str) && !is_windows_absolute(path_str) {
        return None;
    }
    if !has_path_separator(path_str) && stem_len(path_str) < MIN_BARE_STEM_LEN {
        return None;
    }
    let resolved = resolve_path(path_str, cwd)?;
    if !resolved.is_file() {
        return None;
    }
    if !extension_ok(&resolved) {
        return None;
    }
    Some(resolved)
}

fn char_span_for_bytes(
    line_text: &str,
    byte_start: usize,
    byte_end: usize,
) -> Option<(usize, usize)> {
    if byte_end <= byte_start {
        return None;
    }
    let char_start = line_text[..byte_start].chars().count();
    let char_end = line_text[..byte_end].chars().count().checked_sub(1)?;
    Some((char_start, char_end))
}

struct CandidateZoneSpec {
    byte_start: usize,
    byte_end: usize,
    source: HyperlinkSource,
    line_no: Option<u32>,
    col_no: Option<u32>,
}

fn zone_for_candidate(
    line_text: &str,
    line: crate::terminal::types::Line,
    char_to_col: &[usize],
    resolved: PathBuf,
    spec: CandidateZoneSpec,
) -> Option<HyperlinkZone> {
    let (char_start, char_end) = char_span_for_bytes(line_text, spec.byte_start, spec.byte_end)?;
    let col_start = char_to_col.get(char_start)?;
    let col_end = char_to_col.get(char_end)?;
    Some(HyperlinkZone {
        uri: resolved.to_string_lossy().into_owned(),
        id: String::new(),
        start: crate::terminal::types::Point::new(line.0, *col_start),
        end: crate::terminal::types::Point::new(line.0, *col_end),
        is_openable: true,
        source: spec.source,
        line: spec.line_no,
        col: spec.col_no,
    })
}

/// Detect `.md` / `.markdown` file paths on a single terminal line.
///
/// `char_to_col` maps each character index in `line_text` to its grid column,
/// matching the URL scanner's wide-char-aware mapping. `cwd` is used to resolve
/// relative paths; if `None`, only absolute paths are eligible.
///
/// Returned zones have `source = HyperlinkSource::FilePath`, `is_openable = true`,
/// and `uri` set to the resolved absolute path. Anti-false-positive rules:
/// - Stem (basename without extension) must be ≥ 4 chars when the candidate
///   has no path separator. `123.md` → rejected; `/foo/bar.md` accepted.
/// - Candidate must not contain ANSI control chars.
/// - Candidate must resolve to an existing file on disk.
pub fn detect_file_paths_on_line_mapped(
    line_text: &str,
    line: crate::terminal::types::Line,
    char_to_col: &[usize],
    cwd: Option<&Path>,
) -> Vec<HyperlinkZone> {
    let mut zones = Vec::new();
    for ext_match in markdown_extension_regex().find_iter(line_text) {
        let path_end = ext_match.end();
        if !extension_tail_ok(line_text, path_end) {
            continue;
        }
        for start in candidate_start_positions(line_text, ext_match.start()) {
            let candidate = &line_text[start..path_end];
            let Some(resolved) =
                validated_path_candidate(candidate, cwd, canonical_has_md_extension)
            else {
                continue;
            };
            if let Some(zone) = zone_for_candidate(
                line_text,
                line,
                char_to_col,
                resolved,
                CandidateZoneSpec {
                    byte_start: start,
                    byte_end: path_end,
                    source: HyperlinkSource::FilePath,
                    line_no: None,
                    col_no: None,
                },
            ) {
                zones.push(zone);
                break;
            }
        }
    }
    zones
}

// ---------------------------------------------------------------------------
// Code-file scanner (file:line:col)
// ---------------------------------------------------------------------------

// The code-path scanner detects a known source extension, expands possible
// path starts to the left, then validates the resolved file before emitting a
// link. Location suffixes accept `:line[:col]`, `(line,col)`, and `(line:col)`.
// `.md` / `.markdown` are deliberately absent: the markdown scanner routes
// those to the in-pane markdown viewer.

/// US-013: Python traceback frame `File "path", line N`. The path is quoted and
/// the line number lives in a separate clause, so the generic code-path regex
/// would match the bare filename and lose the line. A dedicated pattern with
/// named captures recovers both.
const PYTHON_TRACEBACK_REGEX_PATTERN: &str = r#"File "(?P<path>[^"]+)", line (?P<line>\d+)"#;

fn python_traceback_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(PYTHON_TRACEBACK_REGEX_PATTERN)
            .expect("python-traceback regex compilation failed")
    })
}

/// Peel `:line[:col]` off the right of `matched`, returning
/// `(path, line, col)`. Walks at most two `:`-separated pure-digit
/// suffixes; stops at the first non-digit segment so Windows drive
/// letters (`C:`) and path segments containing colons stay intact.
fn split_path_and_location(matched: &str) -> (&str, Option<u32>, Option<u32>) {
    // US-013: paren-location form `file(42,7)` / `file:(12,3)` (tsc, C#,
    // MSBuild). Peel a trailing `(N,M)` / `:(N,M)` group before falling back to
    // the colon-suffix walk below.
    if let Some(without_close) = matched.strip_suffix(')')
        && let Some(open) = without_close.rfind('(')
    {
        let inner = &without_close[open + 1..];
        let mut parts = inner.splitn(2, [',', ':']);
        if let (Some(l), Some(c)) = (parts.next(), parts.next())
            && let (Ok(line), Ok(col)) = (l.parse::<u32>(), c.parse::<u32>())
        {
            // Drop the `(` and an optional preceding `:` from the path.
            let mut path_end = open;
            if without_close[..path_end].ends_with(':') {
                path_end -= 1;
            }
            return (&matched[..path_end], Some(line), Some(col));
        }
    }

    let mut end = matched.len();
    let mut nums: Vec<u32> = Vec::with_capacity(2);
    while nums.len() < 2 {
        let Some(colon_pos) = matched[..end].rfind(':') else {
            break;
        };
        let suffix = &matched[colon_pos + 1..end];
        if let Ok(n) = suffix.parse::<u32>() {
            nums.push(n);
            end = colon_pos;
        } else {
            break;
        }
    }
    let path = &matched[..end];
    match nums.as_slice() {
        [] => (path, None, None),
        [line] => (path, Some(*line), None),
        // `nums` collected right-to-left: [col, line]
        [col, line] => (path, Some(*line), Some(*col)),
        _ => (path, None, None),
    }
}

fn parse_u32_at(text: &str, start: usize) -> Option<(u32, usize)> {
    let mut end = start;
    for (idx, ch) in text[start..].char_indices() {
        if !ch.is_ascii_digit() {
            break;
        }
        end = start + idx + ch.len_utf8();
    }
    if end == start {
        return None;
    }
    text[start..end].parse::<u32>().ok().map(|n| (n, end))
}

fn location_suffix_tail_is_clean(text: &str, end: usize) -> bool {
    !text[end..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn code_candidate_display_end(line_text: &str, path_end: usize) -> usize {
    let suffix = &line_text[path_end..];

    let paren_digits_start = if suffix.starts_with(":(") {
        Some(path_end + 2)
    } else if suffix.starts_with('(') {
        Some(path_end + 1)
    } else {
        None
    };
    if let Some(digits_start) = paren_digits_start
        && let Some((_, after_line)) = parse_u32_at(line_text, digits_start)
        && let Some(separator) = line_text[after_line..].chars().next()
        && matches!(separator, ',' | ':')
    {
        let col_start = after_line + separator.len_utf8();
        if let Some((_, after_col)) = parse_u32_at(line_text, col_start)
            && line_text[after_col..].starts_with(')')
        {
            let display_end = after_col + 1;
            if location_suffix_tail_is_clean(line_text, display_end) {
                return display_end;
            }
        }
    }

    if !suffix.starts_with(':') {
        return path_end;
    }
    let Some((_, after_line)) = parse_u32_at(line_text, path_end + 1) else {
        return path_end;
    };
    let mut display_end = after_line;
    if line_text[display_end..].starts_with(':') {
        let Some((_, after_col)) = parse_u32_at(line_text, display_end + 1) else {
            return path_end;
        };
        display_end = after_col;
    }
    if location_suffix_tail_is_clean(line_text, display_end) {
        display_end
    } else {
        path_end
    }
}

/// Returns true if the canonicalised path's extension is a recognised
/// code extension. Defence-in-depth against symlinks (`good.rs ->
/// /usr/bin/sudo`) - without this, a malicious link in terminal output
/// could route a system binary through the editor open path.
fn canonical_has_code_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    CODE_EXTENSIONS.contains(&lower.as_str())
}

/// Detect source-code file paths with optional `:line[:col]` on a single
/// terminal line. Mirrors `detect_file_paths_on_line_mapped`'s anti-false-
/// positive rules (left boundary, control chars, URL-scheme reject,
/// bare-stem minimum length, canonical resolve + extension recheck) and
/// adds the path/location split.
///
/// Returned zones have `source = HyperlinkSource::CodePath`,
/// `is_openable = true`, and `line`/`col` populated when the matched
/// text carried a `:N(:M)?` suffix. `uri` is the canonical absolute
/// path (location stripped); the editor open path adds it back via
/// argv when invoking the editor.
pub fn detect_code_paths_on_line_mapped(
    line_text: &str,
    line: crate::terminal::types::Line,
    char_to_col: &[usize],
    cwd: Option<&Path>,
) -> Vec<HyperlinkZone> {
    // US-013: Python traceback frames (`File "x.py", line N`) first, so they
    // win on the quoted path. The generic scan below also matches the bare
    // filename but with no line number; the hover `find` returns the first
    // (Python) match, which carries the correct line.
    let mut zones: Vec<HyperlinkZone> = python_traceback_regex()
        .captures_iter(line_text)
        .filter_map(|cap| {
            let path_m = cap.name("path")?;
            let path_str = path_m.as_str();
            let line_no = cap.name("line")?.as_str().parse::<u32>().ok()?;
            let resolved = validated_path_candidate(path_str, cwd, canonical_has_code_extension)?;
            zone_for_candidate(
                line_text,
                line,
                char_to_col,
                resolved,
                CandidateZoneSpec {
                    byte_start: path_m.start(),
                    byte_end: path_m.end(),
                    source: HyperlinkSource::CodePath,
                    line_no: Some(line_no),
                    col_no: None,
                },
            )
        })
        .collect();

    for ext_match in code_extension_regex().find_iter(line_text) {
        let path_end = ext_match.end();
        if !extension_tail_ok(line_text, path_end) {
            continue;
        }
        let display_end = code_candidate_display_end(line_text, path_end);
        for start in candidate_start_positions(line_text, ext_match.start()) {
            let matched = &line_text[start..display_end];
            let (path_str, line_no, col_no) = split_path_and_location(matched);
            let Some(resolved) =
                validated_path_candidate(path_str, cwd, canonical_has_code_extension)
            else {
                continue;
            };
            if let Some(zone) = zone_for_candidate(
                line_text,
                line,
                char_to_col,
                resolved,
                CandidateZoneSpec {
                    byte_start: start,
                    byte_end: display_end,
                    source: HyperlinkSource::CodePath,
                    line_no,
                    col_no,
                },
            ) {
                zones.push(zone);
                break;
            }
        }
    }
    zones
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Instant;

    fn line0() -> crate::terminal::types::Line {
        crate::terminal::types::Line(0)
    }

    /// Builds a 1-to-1 char→column map for ASCII-only test text.
    fn ascii_map(text: &str) -> Vec<usize> {
        (0..text.chars().count()).collect()
    }

    // ── US-020: URL trailing-punctuation / unbalanced-paren trimming ────────

    #[test]
    fn sanitize_strips_trailing_dot_and_comma() {
        assert_eq!(
            sanitize_url_punctuation("https://example.com/path."),
            "https://example.com/path"
        );
        assert_eq!(
            sanitize_url_punctuation("https://example.com/path,"),
            "https://example.com/path"
        );
    }

    #[test]
    fn sanitize_strips_unbalanced_paren_then_dot() {
        // `).` - `)` is unbalanced (0 opens, 1 close) then `.`.
        assert_eq!(
            sanitize_url_punctuation("https://example.com/path)."),
            "https://example.com/path"
        );
    }

    #[test]
    fn sanitize_preserves_balanced_parens() {
        let url = "https://en.wikipedia.org/wiki/Example_(disambiguation)";
        assert_eq!(sanitize_url_punctuation(url), url);
    }

    #[test]
    fn sanitize_trims_one_of_two_unbalanced_close_parens() {
        // 1 open, 2 close → strip exactly one `)`.
        assert_eq!(
            sanitize_url_punctuation("https://example.com/a(b))"),
            "https://example.com/a(b)"
        );
    }

    #[test]
    fn sanitize_bracket_balance() {
        assert_eq!(
            sanitize_url_punctuation("https://example.com/a[b]"),
            "https://example.com/a[b]"
        );
        assert_eq!(
            sanitize_url_punctuation("https://example.com/a]"),
            "https://example.com/a"
        );
    }

    #[test]
    fn sanitize_strips_bang_question_semicolon_colon() {
        assert_eq!(
            sanitize_url_punctuation("https://example.com/p!?;:"),
            "https://example.com/p"
        );
    }

    #[test]
    fn sanitize_preserves_query_and_fragment() {
        let url = "https://example.com/path?q=1&r=2#anchor";
        assert_eq!(sanitize_url_punctuation(url), url);
    }

    #[test]
    fn detect_urls_trims_trailing_paren_dot_end_to_end() {
        let line = "see https://example.com/path). for details";
        let map = ascii_map(line);
        let zones = detect_urls_on_line_mapped(line, line0(), &map);
        assert_eq!(zones.len(), 1, "expected exactly one URL zone");
        assert_eq!(zones[0].uri, "https://example.com/path");
    }

    #[test]
    fn detect_urls_preserves_wikipedia_disambiguation_end_to_end() {
        let url = "https://en.wikipedia.org/wiki/Example_(disambiguation)";
        let line = format!("see {url}.");
        let map = ascii_map(&line);
        let zones = detect_urls_on_line_mapped(&line, line0(), &map);
        assert_eq!(zones.len(), 1);
        assert_eq!(
            zones[0].uri, url,
            "balanced parens kept; only the trailing . stripped"
        );
    }

    #[test]
    fn file_urls_are_not_generic_openable_links() {
        let line = "see file:///tmp/README.md and https://example.com";
        let map = ascii_map(line);
        let zones = detect_urls_on_line_mapped(line, line0(), &map);

        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].uri, "https://example.com");
        assert!(!is_url_scheme_openable("file:///tmp/README.md"));
        assert!(!is_url_scheme_openable("file://localhost/tmp/README.md"));
    }

    fn write_md(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create dir");
        }
        fs::write(&p, b"# test").expect("write md");
        p
    }

    /// Resolve `p` to a display string that stays readable in test output.
    /// Windows canonicalization may prepend `\\?\`; stripping it keeps tests
    /// independent from the host's verbatim-path formatting.
    fn canonical_display(p: &Path) -> String {
        let canonical = p.canonicalize().expect("canonicalize");
        let s = canonical.to_string_lossy().into_owned();
        s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s)
    }

    #[cfg(unix)]
    #[test]
    fn linux_absolute_path_existing_matches() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let md = write_md(tmp.path(), "doc.md");
        let canonical = md.canonicalize().expect("canonicalize");
        let line_text = format!("see {}", md.to_string_lossy());
        let map = ascii_map(&line_text);
        let zones = detect_file_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert_eq!(zones.len(), 1);
        assert_eq!(PathBuf::from(&zones[0].uri), canonical);
        assert_eq!(zones[0].source, HyperlinkSource::FilePath);
        assert!(zones[0].is_openable);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_absolute_uses_same_unix_path() {
        // macOS uses POSIX paths; same code path as Linux.
        let tmp = tempfile::tempdir().expect("tempdir");
        let md = write_md(tmp.path(), "Users_foo.md");
        let line_text = format!("open {}", md.to_string_lossy());
        let map = ascii_map(&line_text);
        let zones = detect_file_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert_eq!(zones.len(), 1);
    }

    #[test]
    fn windows_absolute_path_classified_correctly() {
        // Pure regex/classification check - file does not need to exist on
        // the host filesystem since `resolve_path` will reject it.
        assert!(is_windows_absolute("C:\\Users\\arthur\\doc.md"));
        assert!(is_windows_absolute("D:/repo/README.md"));
        assert!(is_windows_absolute(r"\\server\share\README.md"));
        assert!(is_windows_absolute(r"\\?\C:\repo\README.md"));
        assert!(!is_windows_absolute("/etc/foo.md"));
        assert!(!is_windows_absolute("foo.md"));
        assert!(!is_windows_absolute("C:foo"));
    }

    #[test]
    fn relative_with_dot_prefix_resolves_against_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "rel.md");
        let line_text = "open ./rel.md now";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
        let resolved = PathBuf::from(&zones[0].uri);
        assert!(resolved.exists());
    }

    #[test]
    fn relative_bare_resolves_against_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "README.md");
        let line_text = "edit README.md please";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
    }

    #[test]
    fn quoted_markdown_path_with_spaces_resolves_against_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "My Project/README.md");
        let line_text = "open \"My Project/README.md\"";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
        assert!(zones[0].uri.ends_with("README.md"));
    }

    #[test]
    fn unicode_markdown_path_resolves_against_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "café.md");
        let line_text = "open café.md";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
        assert!(zones[0].uri.ends_with("café.md"));
    }

    #[test]
    fn markdown_prefix_of_longer_extension_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "README.md");
        let line_text = "open README.md.old";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert!(zones.is_empty());
    }

    #[test]
    fn missing_file_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let line_text = format!("ghost {}/nope.md", tmp.path().to_string_lossy());
        let map = ascii_map(&line_text);
        let zones = detect_file_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert!(zones.is_empty());
    }

    #[test]
    fn short_numeric_stem_is_rejected() {
        // Bare `123.md` with no separator - stem `123` (3 chars) < 4.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "123.md");
        let line_text = "open 123.md";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert!(zones.is_empty(), "short bare stem must be rejected");
    }

    #[test]
    fn short_stem_with_path_separator_is_accepted() {
        // `./os.md` has a separator, so the length heuristic does not apply.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "os.md");
        let line_text = "open ./os.md";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
    }

    #[test]
    fn case_insensitive_extension() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "guide.MD");
        let line_text = "see ./guide.MD";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
    }

    #[test]
    fn markdown_long_extension() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "guide.markdown");
        let line_text = "see ./guide.markdown";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
    }

    #[test]
    fn control_chars_disqualify_match() {
        // A path-shaped token that contains an ANSI escape must be rejected.
        // We construct it directly because regex won't match the escape; the
        // helper is the safety net for unusual inputs.
        assert!(contains_control_char("\x1b[31m/foo.md"));
        assert!(!contains_control_char("/foo/bar.md"));
    }

    #[test]
    fn osc8_priority_does_not_overlap_filepath_scanner() {
        // The scanner does not consider OSC 8 zones - priority is enforced
        // at the call site (handle_mouse_move tries OSC 8 first, then URLs,
        // then file paths). This test documents that the scanner itself does
        // not emit zones for arbitrary OSC 8 cells: it only returns matches
        // from regex over the line text. Pure plain text behaves identically
        // whether or not the cell carries an OSC 8 hyperlink.
        let tmp = tempfile::tempdir().expect("tempdir");
        let md_path = write_md(tmp.path(), "doc.md");
        // `canonical_display` resolves Windows 8.3 short names + strips the
        // `\\?\` UNC prefix so the file-path regex character class accepts
        // every byte. On Unix it's a thin wrapper around canonicalize.
        let display = canonical_display(&md_path);
        let line_text = format!("file {display}");
        let map = ascii_map(&line_text);
        let zones = detect_file_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert_eq!(zones.len(), 1);
    }

    #[test]
    fn boundary_rejects_mid_token_match() {
        // `xyz/foo.md` - the slash makes the regex match `xyz/foo.md`, but if
        // the candidate is preceded by `prefix-` (no whitespace), boundary
        // check rejects. Confirm that whitespace boundary works.
        let tmp = tempfile::tempdir().expect("tempdir");
        let md_path = write_md(tmp.path(), "foo.md");
        // Same Windows-friendly long-form path the OSC-8 test uses.
        let display = canonical_display(&md_path);
        let line_text = format!("ok {display}");
        let map = ascii_map(&line_text);
        let zones = detect_file_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert_eq!(zones.len(), 1);

        // Embedded mid-word: `prefixfoo.md` (no slash, no boundary delim
        // around but at start of string => start-of-line counts as boundary,
        // so this match is allowed at column 0). However we still want the
        // existence check to reject when the file does not exist. This is a
        // pure regex-string check - no real file involved - so it stays
        // cross-platform without any UNC-strip dance.
        let line_text2 = "blob/junk.md";
        let map2 = ascii_map(line_text2);
        let zones2 = detect_file_paths_on_line_mapped(line_text2, line0(), &map2, Some(tmp.path()));
        assert!(zones2.is_empty());
    }

    #[test]
    fn relative_without_cwd_is_rejected() {
        let line_text = "see ./foo.md";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, None);
        assert!(zones.is_empty());
    }

    #[test]
    fn url_scheme_prefix_is_rejected() {
        // `file:///etc/shadow.md` ends in `.md` and matches the regex char
        // class, but the URL-scheme guard must reject it before resolve_path
        // runs - otherwise `open::that` would honour the file:// scheme.
        assert!(has_url_scheme_prefix("file:///etc/shadow.md"));
        assert!(has_url_scheme_prefix("http://evil.example/x.md"));
        assert!(has_url_scheme_prefix("ssh:host.md"));
        // Windows drive letters are single-letter prefixes - NOT schemes.
        assert!(!has_url_scheme_prefix("C:/repo/README.md"));
        assert!(!has_url_scheme_prefix("D:\\proj\\readme.md"));
        // Bare filenames have no colon → no prefix.
        assert!(!has_url_scheme_prefix("README.md"));
        assert!(!has_url_scheme_prefix("./foo.md"));

        // End-to-end: scanner refuses to emit a zone even if the URL-shaped
        // string would otherwise canonicalise to an existing file.
        let line_text = "open file:///tmp/doc.md please";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, None);
        assert!(zones.is_empty());
    }

    #[test]
    fn canonicalize_resolves_dot_dot_traversal() {
        // A relative candidate with `..` segments must canonicalise: the URI
        // emitted to the click handler should be the canonical (real) path,
        // not the misleading traversal string printed by the terminal.
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("nested");
        fs::create_dir_all(&nested).expect("create nested");
        let md = write_md(tmp.path(), "real.md");
        let canonical = md.canonicalize().expect("canonicalize");

        // From `nested/`, the path `../real.md` must resolve to canonical.
        let line_text = "see ../real.md";
        let map = ascii_map(line_text);
        let zones = detect_file_paths_on_line_mapped(line_text, line0(), &map, Some(&nested));
        assert_eq!(zones.len(), 1);
        assert_eq!(PathBuf::from(&zones[0].uri), canonical);
        // The emitted URI must NOT contain `..` - it has been normalised.
        assert!(!zones[0].uri.contains(".."));
    }

    #[test]
    fn perf_scan_200_lines_under_budget() {
        // AC budget: 200×80 grid scan < 5 ms (release).
        // Debug builds are ~5-10× slower; we assert release < 5 ms strictly
        // on Linux/macOS and apply a 25 ms ceiling in debug as a regression
        // guard. On Windows the hosted runners are 2-3× slower at the same
        // workload (US-004 AC5), so we relax to 15 ms in release without
        // weakening the regression intent - anything significantly above
        // 15 ms still surfaces as a perf regression.
        let tmp = tempfile::tempdir().expect("tempdir");
        let md_path = write_md(tmp.path(), "perf.md");
        let target = canonical_display(&md_path);
        let mut lines: Vec<String> = (0..200)
            .map(|i| {
                if i % 20 == 0 {
                    format!("[info] open {} for review", target)
                } else {
                    "plain log line with no path content here at all -----".to_string()
                }
            })
            .collect();
        for line in &mut lines {
            while line.chars().count() < 80 {
                line.push(' ');
            }
        }
        let started = Instant::now();
        let mut total = 0usize;
        for line in &lines {
            let map = ascii_map(line);
            let zones = detect_file_paths_on_line_mapped(line, line0(), &map, None);
            total += zones.len();
        }
        let elapsed = started.elapsed();
        assert!(total >= 10, "expected at least 10 hits, got {}", total);
        let budget_ms: u128 = if cfg!(debug_assertions) {
            25
        } else if cfg!(target_os = "windows") {
            15
        } else {
            5
        };
        assert!(
            elapsed.as_millis() < budget_ms,
            "200×80 scan took {:?}, exceeds {} ms budget",
            elapsed,
            budget_ms
        );
    }

    // ---------------------------------------------------------------------
    // Code-path scanner - split_path_and_location + scanner end-to-end
    // ---------------------------------------------------------------------

    #[test]
    fn split_location_bare_path_no_location() {
        let (p, l, c) = split_path_and_location("foo.rs");
        assert_eq!(p, "foo.rs");
        assert_eq!(l, None);
        assert_eq!(c, None);
    }

    #[test]
    fn split_location_with_line() {
        let (p, l, c) = split_path_and_location("foo.rs:42");
        assert_eq!(p, "foo.rs");
        assert_eq!(l, Some(42));
        assert_eq!(c, None);
    }

    #[test]
    fn split_location_with_line_and_col() {
        let (p, l, c) = split_path_and_location("src/foo.rs:42:7");
        assert_eq!(p, "src/foo.rs");
        assert_eq!(l, Some(42));
        assert_eq!(c, Some(7));
    }

    #[test]
    fn split_location_preserves_windows_drive_letter() {
        // C:\foo\bar.rs - the `C:` must NOT be peeled off as a location.
        let (p, l, c) = split_path_and_location(r"C:\foo\bar.rs");
        assert_eq!(p, r"C:\foo\bar.rs");
        assert_eq!(l, None);
        assert_eq!(c, None);
    }

    #[test]
    fn split_location_windows_drive_with_line_col() {
        let (p, l, c) = split_path_and_location(r"C:\foo\bar.rs:42:7");
        assert_eq!(p, r"C:\foo\bar.rs");
        assert_eq!(l, Some(42));
        assert_eq!(c, Some(7));
    }

    #[test]
    fn split_location_stops_at_non_digit_segment() {
        // `path.rs:42:notnum:7` - peels off `7` only, leaves the rest as path.
        // The downstream canonicalize check rejects the bogus path.
        let (p, l, c) = split_path_and_location("path.rs:42:notnum:7");
        assert_eq!(p, "path.rs:42:notnum");
        assert_eq!(l, Some(7));
        assert_eq!(c, None);
    }

    #[test]
    fn split_location_paren_form_tsc() {
        // US-013: tsc `app.ts(42,7)` → line 42, col 7.
        let (p, l, c) = split_path_and_location("src/app.ts(42,7)");
        assert_eq!(p, "src/app.ts");
        assert_eq!(l, Some(42));
        assert_eq!(c, Some(7));
    }

    #[test]
    fn split_location_paren_form_with_colon_prefix() {
        // US-013: the `:?` allows `file.ts:(12,3)`.
        let (p, l, c) = split_path_and_location("file.ts:(12,3)");
        assert_eq!(p, "file.ts");
        assert_eq!(l, Some(12));
        assert_eq!(c, Some(3));
    }

    #[test]
    fn split_location_paren_colon_separator() {
        // US-013: C#/MSBuild also emit `(line:col)`.
        let (p, l, c) = split_path_and_location("Program.cs(10:5)");
        assert_eq!(p, "Program.cs");
        assert_eq!(l, Some(10));
        assert_eq!(c, Some(5));
    }

    #[test]
    fn split_location_non_numeric_paren_is_not_a_location() {
        // US-013 adversarial: `foo.rs(copy)` must NOT yield a false line/col;
        // the non-numeric paren stays attached so canonicalize rejects it.
        let (p, l, c) = split_path_and_location("foo.rs(copy)");
        assert_eq!(p, "foo.rs(copy)");
        assert_eq!(l, None);
        assert_eq!(c, None);
    }

    #[test]
    fn code_path_scanner_matches_paren_location() {
        // US-013 end-to-end: tsc-style `app.ts(42,7)`.
        let tmp = tempfile::tempdir().expect("tempdir");
        let ts_path = write_md(tmp.path(), "app.ts");
        let display = canonical_display(&ts_path);
        let line_text = format!("{display}(42,7): error TS2345");
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert!(
            zones
                .iter()
                .any(|z| z.line == Some(42) && z.col == Some(7) && z.uri.ends_with("app.ts")),
            "US-013: tsc paren-location must resolve line+col; got {zones:?} zones",
            zones = zones.len()
        );
    }

    #[test]
    fn code_path_scanner_matches_python_traceback() {
        // US-013: `File "main.py", line 10` → line 10 on the quoted path.
        let tmp = tempfile::tempdir().expect("tempdir");
        let py_path = write_md(tmp.path(), "main.py");
        let display = canonical_display(&py_path);
        let line_text = format!("  File \"{display}\", line 10, in <module>");
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert!(
            zones
                .iter()
                .any(|z| z.line == Some(10) && z.uri.ends_with("main.py")),
            "US-013: Python traceback frame must resolve the line number"
        );
    }

    #[test]
    fn code_path_scanner_still_matches_update_paren_wrap() {
        // US-013 regression: Claude-Code `Update(src/cool.rs)` must still match
        // the inner path (leading `(` is a left boundary; the trailing `)` is
        // not a numeric paren-location, so the path is `cool.rs`, line None).
        let tmp = tempfile::tempdir().expect("tempdir");
        let rs_path = write_md(tmp.path(), "cool.rs");
        let display = canonical_display(&rs_path);
        let line_text = format!("Update({display})");
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert!(
            zones
                .iter()
                .any(|z| z.uri.ends_with("cool.rs") && z.line.is_none()),
            "US-013: Update(path) must still match the inner path with no location"
        );
    }

    #[test]
    fn code_path_scanner_matches_rust_at_line_col() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rs_path = write_md(tmp.path(), "lib.rs"); // re-use the .md writer for any file
        let display = canonical_display(&rs_path);
        let line_text = format!("error at {display}:42:7");
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].source, HyperlinkSource::CodePath);
        assert_eq!(zones[0].line, Some(42));
        assert_eq!(zones[0].col, Some(7));
        assert!(zones[0].uri.ends_with("lib.rs"));
    }

    #[test]
    fn code_path_scanner_quoted_path_with_spaces_and_line_col() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "My Project/src/lib.rs");
        let line_text = "\"My Project/src/lib.rs:12:3\"";
        let map = ascii_map(line_text);
        let zones = detect_code_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].line, Some(12));
        assert_eq!(zones[0].col, Some(3));
        assert!(zones[0].uri.ends_with("lib.rs"));
    }

    #[test]
    fn code_path_scanner_unicode_path_resolves_against_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "src/café.rs");
        let line_text = "error at src/café.rs:9";
        let map = ascii_map(line_text);
        let zones = detect_code_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].line, Some(9));
        assert!(zones[0].uri.ends_with("café.rs"));
    }

    #[test]
    fn code_path_prefix_of_backup_extension_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "lib.rs");
        let line_text = "error at lib.rs.bak";
        let map = ascii_map(line_text);
        let zones = detect_code_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert!(zones.is_empty());
    }

    #[test]
    fn code_path_scanner_matches_python_no_location() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let py_path = write_md(tmp.path(), "main.py");
        let display = canonical_display(&py_path);
        let line_text = format!("traceback: {display}");
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].source, HyperlinkSource::CodePath);
        assert_eq!(zones[0].line, None);
        assert_eq!(zones[0].col, None);
    }

    #[test]
    fn code_path_scanner_skips_markdown() {
        // .md files belong to the FilePath scanner (markdown viewer route).
        // The code-path scanner must NOT emit a zone for them.
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "README.md");
        let line_text = format!("see {}/README.md", tmp.path().to_string_lossy());
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert!(
            zones.is_empty(),
            "markdown must not match code-path scanner"
        );
    }

    #[test]
    fn code_path_scanner_relative_resolves_against_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_md(tmp.path(), "config.toml");
        let line_text = "see ./config.toml:5";
        let map = ascii_map(line_text);
        let zones = detect_code_paths_on_line_mapped(line_text, line0(), &map, Some(tmp.path()));
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].line, Some(5));
    }

    #[test]
    fn code_path_scanner_rejects_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let line_text = format!("error at {}/nope.rs:42:7", tmp.path().to_string_lossy());
        let map = ascii_map(&line_text);
        let zones = detect_code_paths_on_line_mapped(&line_text, line0(), &map, None);
        assert!(zones.is_empty());
    }

    #[test]
    fn code_path_scanner_url_scheme_rejected() {
        let line_text = "open file:///tmp/x.rs:42";
        let map = ascii_map(line_text);
        let zones = detect_code_paths_on_line_mapped(line_text, line0(), &map, None);
        assert!(zones.is_empty());
    }
}

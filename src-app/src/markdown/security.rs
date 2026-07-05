//! Markdown viewer security boundary (US-009 - prd-stabilization-2026-q2.md).
//!
//! `.md` files are user-controlled content from anywhere on disk (a project
//! README, an LLM-generated draft, a PR description copied from a hostile
//! site). Without an explicit boundary, image refs and link URLs in those
//! files inherit the privileges of `paneflow` itself - they could load
//! `/etc/passwd` as an image, or hand a `javascript:` / `file://` URL to
//! `open::that` which `xdg-open` would happily honour as a local-file
//! handler. This module is the boundary.
//!
//! Two validators, one shape:
//!
//! - [`validate_image_ref`] takes a `(doc_root, ref)` pair and returns a
//!   sanitised `PathBuf` that is guaranteed to (a) carry no URL scheme,
//!   (b) lexically not escape `doc_root`, and (c) pass a canonical-prefix
//!   check when the file exists. Missing files are accepted (AC6 - a
//!   404 in the renderer is a UX concern, not a security one).
//!
//! - [`validate_link_url`] takes a URL string and returns a `ValidatedUrl`
//!   wrapping it. Only `http://` and `https://` survive; `file://`,
//!   `data:`, `javascript:`, `vbscript:`, and every custom scheme is
//!   rejected. Stricter than the terminal hyperlink helper
//!   (`crate::terminal::element::is_url_scheme_openable`) because
//!   markdown is potentially-hostile file content whereas terminal
//!   output is the user's own shell talking to them.
//!
//! State of the world: BOTH validators are "armed but nothing to gate" today.
//! - paneflow does not *load* images - the parser emits a `[image: <url>]`
//!   placeholder text span (`parser.rs`), so `validate_image_ref` has no load
//!   path to gate yet.
//! - paneflow does not *open* markdown links either - the StyledText render
//!   path does not yet hit-test per-run clicks, so link spans are styled but
//!   not clickable (`view.rs::build_styled_text`, and the `#[allow(dead_code)]`
//!   on this module in `markdown/mod.rs`). There is no `open::that` call in the
//!   markdown viewer, so `validate_link_url` has no caller in production.
//!
//! Both ship with full tests so the boundary is reviewed and in place: when
//! click hit-testing is restored, the rewire MUST route every URL through
//! `validate_link_url(...)?` before handing it to `open::that` - wiring it in
//! is a one-line change at that point. The regression test
//! `allowlist_is_http_https_only` pins the scheme allow-list so a later edit
//! cannot silently widen it.

use std::path::{Component, Path, PathBuf};

/// Wrapper around an `&str` URL that has been validated as starting
/// with `http://` or `https://`. Constructed only via
/// [`validate_link_url`]; the wrapped string is exactly what the
/// caller passed in (no normalisation, percent-encoding, or punycode
/// transformation), so a UI showing the URL back to the user displays
/// the original characters without surprises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedUrl(String);

impl ValidatedUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a URL was refused for `open::that`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    /// Scheme is not in the http/https allowlist. The wrapped
    /// `String` is the offending scheme so the log can quote the
    /// exact value the user agent saw.
    DisallowedScheme(String),
    /// URL has no recognisable scheme prefix at all. Bare strings
    /// like `"example.com"` land here - pulldown-cmark forwards them
    /// verbatim and there is no safe default scheme we could synthesise.
    MissingScheme,
    /// URL exceeded our 8 KiB sanity cap. A real `https://` URL is
    /// short; an 80 KiB `data:` URL or a maliciously deep query
    /// string is a smell. The cap is documented as a defence in
    /// depth - if every other check passes, we still refuse to
    /// pass an unbounded string into `xdg-open`.
    TooLong,
    /// Scheme is allowed, but the URL shape is not safe to delegate to
    /// the OS URL handler. We require `http://` or `https://` with a
    /// non-empty authority and reject whitespace, control characters,
    /// backslashes, and userinfo.
    Malformed,
}

/// Why an image reference was refused.
///
/// `#[allow(dead_code)]` because paneflow does not yet *load* images -
/// the parser emits a `[image: <url>]` placeholder text span (see
/// `parser.rs::on_start` for `Tag::Image`). The validator + tests
/// ship now so the boundary is in place when actual image rendering
/// lands; wiring `validate_image_ref` into a load path is a one-line
/// change at that point.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRefError {
    /// `data:`, `file:`, `http(s):`, `javascript:`, `vbscript:`, …
    /// All schemes are rejected for image refs - markdown viewers
    /// MUST stay disconnected from the network and the URL handler
    /// for image embeds (AC1).
    DisallowedScheme(String),
    /// Lexical normalisation of `doc_root.join(ref)` produced a path
    /// whose components escape `doc_root`. Catches `../../etc/passwd`
    /// even when no file at that path exists.
    TraversalEscape { reference: String },
    /// `doc_root` itself could not be canonicalised (it does not
    /// exist or is unreadable). Should never happen in production
    /// because the document is by definition open at `doc_root`'s
    /// parent - surfaced as a separate error so a test using a
    /// fresh tempdir can pinpoint the cause.
    CanonRoot(String),
}

/// Hard cap on link-URL length passed to `open::that`. Real URLs are
/// well under this; the cap is defence-in-depth so a 1 MB string
/// can't sneak through to `xdg-open` and surface a confused-deputy
/// failure mode further down the chain.
const MAX_LINK_URL_LEN: usize = 8 * 1024;

/// Schemes paneflow's link handler is willing to delegate to the OS.
const ALLOWED_LINK_SCHEMES: &[&str] = &["http", "https"];

pub fn validate_link_url(url: &str) -> Result<ValidatedUrl, UrlError> {
    if url.len() > MAX_LINK_URL_LEN {
        return Err(UrlError::TooLong);
    }
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(UrlError::Malformed);
    }
    let scheme = match extract_scheme(url) {
        Some(s) => s,
        None => return Err(UrlError::MissingScheme),
    };
    let scheme_lower = scheme.to_ascii_lowercase();
    if !ALLOWED_LINK_SCHEMES.contains(&scheme_lower.as_str()) {
        return Err(UrlError::DisallowedScheme(scheme_lower));
    }
    let rest = url
        .get(scheme.len()..)
        .filter(|rest| rest.starts_with("://"))
        .ok_or(UrlError::Malformed)?;
    let authority = &rest[3..];
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    if authority.is_empty() || authority.contains('@') {
        return Err(UrlError::Malformed);
    }
    Ok(ValidatedUrl(url.to_string()))
}

/// `#[allow(dead_code)]` because paneflow does not yet *load* images -
/// see [`ImageRefError`] for the staged-rollout note. The function's
/// behaviour is fully covered by the unit tests below so the boundary
/// is regression-tested today.
#[allow(dead_code)]
pub fn validate_image_ref(doc_root: &Path, image_ref: &str) -> Result<PathBuf, ImageRefError> {
    if let Some(scheme) = extract_scheme(image_ref) {
        return Err(ImageRefError::DisallowedScheme(scheme.to_ascii_lowercase()));
    }
    if image_ref.is_empty() {
        return Err(ImageRefError::TraversalEscape {
            reference: image_ref.to_string(),
        });
    }
    if image_ref.contains('\0') {
        return Err(ImageRefError::TraversalEscape {
            reference: image_ref.to_string(),
        });
    }

    let doc_root_canon = doc_root
        .canonicalize()
        .map_err(|e| ImageRefError::CanonRoot(format!("{e}")))?;

    let candidate = if Path::new(image_ref).is_absolute() {
        // Absolute on Unix (`/foo`) or Windows (`C:\foo`) - we lexically
        // normalise without joining, then force the canonical-prefix
        // check below to reject anything outside `doc_root`.
        PathBuf::from(image_ref)
    } else {
        doc_root_canon.join(image_ref)
    };

    let normalised = lexical_normalize(&candidate);

    // Try canonicalize for the strongest guarantee; fall back to the
    // lexical form if the file does not exist (AC6).
    let resolved = normalised.clone().canonicalize().unwrap_or(normalised);

    if !resolved.starts_with(&doc_root_canon) {
        return Err(ImageRefError::TraversalEscape {
            reference: image_ref.to_string(),
        });
    }

    Ok(resolved)
}

/// Extract a URL-style scheme prefix (the substring before `:`) per
/// RFC 3986 §3.1: `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
///
/// First character must be an ASCII letter; subsequent characters may
/// also be ASCII digits or the trio `+ - .`. The minimum length is two
/// so single-letter prefixes are treated as Windows drive letters
/// (`C:`), not schemes - same disambiguation pattern as
/// `crate::terminal::element::hyperlink::has_url_scheme_prefix`.
///
/// Catching `+`/`-`/`.` matters for image refs: a stricter alpha-only
/// check would miss composite schemes like `git+ssh:`,
/// `chrome-extension:`, `coap+tcp:` and silently treat them as
/// relative paths joined to the document root.
fn extract_scheme(input: &str) -> Option<&str> {
    let colon_idx = input.find(':')?;
    let prefix = &input[..colon_idx];
    if prefix.len() < 2 {
        return None;
    }
    let mut chars = prefix.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(prefix)
}

/// Walk a path's components and resolve `..` / `.` lexically without
/// touching the filesystem. Mirrors the standard
/// `path-clean` crate algorithm - we re-implement it here in ~10
/// lines to avoid taking a dep for a single call site. `#[allow(dead_code)]`
/// follows from `validate_image_ref`'s staged status (see that
/// function for the rollout plan).
#[allow(dead_code)]
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => { /* drop `.` */ }
            Component::ParentDir => {
                // `pop` is a no-op on the empty buf, so a path like
                // `../../etc/passwd` becomes `etc/passwd` lexically -
                // the canonical-prefix check below catches it.
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests - AC3 image refs (4 cases) + AC4 link URLs (3 cases) + AC5
// "8 cases" coverage. Plus AC6 missing-file passthrough.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fresh_doc_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // ─── AC4: link URLs ─────────────────────────────────────────────

    #[test]
    fn link_url_https_is_accepted() {
        let v = validate_link_url("https://example.com/path?q=1").expect("https accepted");
        assert_eq!(v.as_str(), "https://example.com/path?q=1");
    }

    #[test]
    fn link_url_http_is_accepted() {
        let v = validate_link_url("http://localhost:3000/").expect("http accepted");
        assert_eq!(v.as_str(), "http://localhost:3000/");
    }

    #[test]
    fn link_url_file_is_rejected() {
        // AC4 case 1: `[click](file:///bin/sh)` - must reject so a
        // hostile markdown file cannot launch /bin/sh via xdg-open's
        // file:// handler chain.
        let err = validate_link_url("file:///bin/sh").expect_err("file rejected");
        assert!(matches!(err, UrlError::DisallowedScheme(s) if s == "file"));
    }

    #[test]
    fn link_url_javascript_is_rejected() {
        let err = validate_link_url("javascript:alert(1)").expect_err("js rejected");
        assert!(matches!(err, UrlError::DisallowedScheme(s) if s == "javascript"));
    }

    #[test]
    fn link_url_data_is_rejected() {
        let err =
            validate_link_url("data:text/html,<script>x</script>").expect_err("data rejected");
        assert!(matches!(err, UrlError::DisallowedScheme(s) if s == "data"));
    }

    #[test]
    fn link_url_vbscript_is_rejected() {
        let err = validate_link_url("vbscript:msgbox").expect_err("vbscript rejected");
        assert!(matches!(err, UrlError::DisallowedScheme(s) if s == "vbscript"));
    }

    #[test]
    fn link_url_bare_string_is_rejected() {
        // `[click](example.com)` - pulldown-cmark forwards the bare
        // string verbatim; we have no safe default scheme to inject,
        // so we reject rather than guess.
        let err = validate_link_url("example.com").expect_err("bare host rejected");
        assert!(matches!(err, UrlError::MissingScheme));
    }

    #[test]
    fn link_url_scheme_match_is_case_insensitive() {
        // `HTTPS://...` is legal per RFC 3986 §3.1. Browsers normalise
        // the scheme; we should follow suit so a legitimate uppercase
        // tag isn't accidentally rejected.
        let v = validate_link_url("HTTPS://example.com").expect("https accepted");
        assert_eq!(v.as_str(), "HTTPS://example.com");
    }

    #[test]
    fn link_url_allowed_scheme_must_be_absolute() {
        let err = validate_link_url("https:example.com/path").expect_err("relative rejected");
        assert!(matches!(err, UrlError::Malformed));
        let err = validate_link_url("https:///path").expect_err("empty authority rejected");
        assert!(matches!(err, UrlError::Malformed));
    }

    #[test]
    fn link_url_rejects_confusing_payloads() {
        let err = validate_link_url("https://example.com/\nfile:///bin/sh")
            .expect_err("newline rejected");
        assert!(matches!(err, UrlError::Malformed));
        let err = validate_link_url("https:\\\\example.com").expect_err("backslash rejected");
        assert!(matches!(err, UrlError::Malformed));
        let err =
            validate_link_url("https://user@example.com/path").expect_err("userinfo rejected");
        assert!(matches!(err, UrlError::Malformed));
    }

    #[test]
    fn link_url_too_long_is_rejected() {
        let huge = format!("https://x.com/{}", "a".repeat(MAX_LINK_URL_LEN));
        let err = validate_link_url(&huge).expect_err("oversized rejected");
        assert!(matches!(err, UrlError::TooLong));
    }

    #[test]
    fn allowlist_is_http_https_only() {
        // Regression guard (US-044): when markdown link click hit-testing is
        // restored, the rewire MUST route `open::that` through
        // `validate_link_url(...)?`. This test pins the scheme allow-list so a
        // later edit cannot silently add `file`/`data`/`smb`/… and reopen the
        // confused-deputy path into `xdg-open`. If you intentionally widen the
        // allow-list, update this test in the same diff and justify it.
        assert_eq!(ALLOWED_LINK_SCHEMES, &["http", "https"]);
    }

    // ─── AC3: image refs ────────────────────────────────────────────

    #[test]
    fn image_ref_traversal_is_rejected() {
        // AC3 case 1: `![](../../etc/passwd)` - lexical escape catches
        // it even when /etc/passwd is unreadable from doc_root.
        let tmp = fresh_doc_root();
        let err =
            validate_image_ref(tmp.path(), "../../etc/passwd").expect_err("traversal rejected");
        assert!(matches!(err, ImageRefError::TraversalEscape { .. }));
    }

    #[test]
    fn image_ref_file_scheme_is_rejected() {
        let tmp = fresh_doc_root();
        let err =
            validate_image_ref(tmp.path(), "file:///etc/passwd").expect_err("file scheme rejected");
        assert!(matches!(err, ImageRefError::DisallowedScheme(s) if s == "file"));
    }

    #[test]
    fn image_ref_javascript_scheme_is_rejected() {
        let tmp = fresh_doc_root();
        let err =
            validate_image_ref(tmp.path(), "javascript:alert(1)").expect_err("js scheme rejected");
        assert!(matches!(err, ImageRefError::DisallowedScheme(s) if s == "javascript"));
    }

    #[test]
    fn image_ref_data_scheme_is_rejected() {
        let tmp = fresh_doc_root();
        let err = validate_image_ref(tmp.path(), "data:text/html,<script>x</script>")
            .expect_err("data scheme rejected");
        assert!(matches!(err, ImageRefError::DisallowedScheme(s) if s == "data"));
    }

    #[test]
    fn image_ref_https_scheme_is_rejected() {
        // AC1 says reject `http`/`https` for IMAGES specifically - the
        // markdown viewer stays off-network. `[](https://example.com/cat.gif)`
        // would otherwise pull a remote image and is the canonical
        // out-of-band beacon for tracking who opened a doc.
        let tmp = fresh_doc_root();
        let err = validate_image_ref(tmp.path(), "https://example.com/x.png")
            .expect_err("https scheme rejected");
        assert!(matches!(err, ImageRefError::DisallowedScheme(s) if s == "https"));
    }

    #[test]
    fn image_ref_in_doc_root_is_accepted() {
        let tmp = fresh_doc_root();
        let img = tmp.path().join("cat.gif");
        fs::write(&img, b"GIF87a").expect("seed image");
        let resolved = validate_image_ref(tmp.path(), "cat.gif").expect("ok");
        assert_eq!(resolved, img.canonicalize().unwrap());
    }

    #[test]
    fn image_ref_subdir_in_doc_root_is_accepted() {
        let tmp = fresh_doc_root();
        let sub = tmp.path().join("assets");
        fs::create_dir(&sub).expect("mkdir");
        let img = sub.join("cat.gif");
        fs::write(&img, b"GIF87a").expect("seed image");
        let resolved = validate_image_ref(tmp.path(), "assets/cat.gif").expect("ok");
        assert_eq!(resolved, img.canonicalize().unwrap());
    }

    /// AC6: missing image file is NOT a security issue - a 404 is
    /// rendered downstream. The validator must still verify the
    /// path doesn't escape `doc_root` lexically.
    #[test]
    fn image_ref_missing_file_inside_doc_root_returns_ok() {
        let tmp = fresh_doc_root();
        let resolved = validate_image_ref(tmp.path(), "missing.png").expect("missing ok");
        // Resolved as a lexically-joined path (canonicalize fails for
        // a non-existent file) - but starts with the doc_root
        // canonical, satisfying the prefix check.
        assert!(resolved.starts_with(tmp.path().canonicalize().unwrap()));
    }

    /// Symlink that targets a file outside doc_root must be rejected.
    /// canonicalize() resolves the symlink and the prefix check then
    /// catches the escape.
    #[cfg(unix)]
    #[test]
    fn image_ref_symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;
        let outer = fresh_doc_root();
        let secret = outer.path().join("secret.png");
        fs::write(&secret, b"GIF").expect("seed secret");

        let inner = fresh_doc_root();
        let link = inner.path().join("decoy.png");
        symlink(&secret, &link).expect("symlink");

        let err =
            validate_image_ref(inner.path(), "decoy.png").expect_err("symlink escape rejected");
        assert!(matches!(err, ImageRefError::TraversalEscape { .. }));
    }

    #[test]
    fn image_ref_empty_string_is_rejected() {
        let tmp = fresh_doc_root();
        let err = validate_image_ref(tmp.path(), "").expect_err("empty rejected");
        assert!(matches!(err, ImageRefError::TraversalEscape { .. }));
    }

    #[test]
    fn image_ref_with_null_byte_is_rejected() {
        let tmp = fresh_doc_root();
        let err = validate_image_ref(tmp.path(), "ok.png\0evil").expect_err("null byte rejected");
        assert!(matches!(err, ImageRefError::TraversalEscape { .. }));
    }

    // ─── extract_scheme: scheme detection vs Windows drive letters ──

    #[test]
    fn windows_drive_letter_is_not_a_scheme() {
        // `C:\foo\bar.png` - Windows absolute path, NOT an http(s)
        // scheme. The drive-letter disambiguation matters because
        // image refs in cross-platform docs may carry literal
        // backslashes from Windows authoring tools.
        assert_eq!(extract_scheme("C:\\Users\\foo.png"), None);
        assert_eq!(extract_scheme("D:/foo.png"), None);
    }

    #[test]
    fn multi_char_scheme_is_detected() {
        assert_eq!(extract_scheme("http://x"), Some("http"));
        assert_eq!(extract_scheme("javascript:alert(1)"), Some("javascript"));
        assert_eq!(extract_scheme("ssh:foo"), Some("ssh"));
    }

    /// RFC 3986 §3.1 composite schemes use `+`, `-`, `.` after the
    /// first alpha character. Without this branch they would be
    /// mistaken for relative paths and joined to the document root -
    /// silently bypassing the scheme allowlist for image refs.
    #[test]
    fn rfc3986_composite_schemes_are_detected() {
        assert_eq!(extract_scheme("git+ssh:host/repo"), Some("git+ssh"));
        assert_eq!(
            extract_scheme("chrome-extension://abc/x.png"),
            Some("chrome-extension")
        );
        assert_eq!(extract_scheme("coap+tcp://srv"), Some("coap+tcp"));
        assert_eq!(extract_scheme("svn.foo:repo"), Some("svn.foo"));
        // Numeric continuation char (RFC permits ALPHA *( ALPHA / DIGIT / +-. )).
        assert_eq!(extract_scheme("h2c:host"), Some("h2c"));
        // First char must still be alpha - leading digit is rejected.
        assert_eq!(extract_scheme("1http:host"), None);
        // Leading +/-/. is rejected.
        assert_eq!(extract_scheme("+ssh:host"), None);
        assert_eq!(extract_scheme(".net:foo"), None);
    }

    /// End-to-end: `validate_image_ref` now rejects composite schemes
    /// rather than treating them as relative paths inside `doc_root`.
    /// AC1 of US-009 says image-ref scheme allowlist is empty (no
    /// scheme is acceptable for image embeds); this test pins the
    /// regression so the boundary survives the wire-up.
    #[test]
    fn image_ref_composite_scheme_is_rejected() {
        let tmp = fresh_doc_root();
        let err = validate_image_ref(tmp.path(), "git+ssh:host/repo")
            .expect_err("composite scheme rejected");
        assert!(matches!(err, ImageRefError::DisallowedScheme(s) if s == "git+ssh"));
    }
}

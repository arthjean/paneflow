use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedUrl(String);

impl ValidatedUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    DisallowedScheme(String),
    MissingScheme,
    TooLong,
    Malformed,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRefError {
    DisallowedScheme(String),
    TraversalEscape { reference: String },
    CanonRoot(String),
}

const MAX_LINK_URL_LEN: usize = 8 * 1024;

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
        PathBuf::from(image_ref)
    } else {
        doc_root_canon.join(image_ref)
    };

    let normalised = lexical_normalize(&candidate);

    let resolved = normalised.clone().canonicalize().unwrap_or(normalised);

    if !resolved.starts_with(&doc_root_canon) {
        return Err(ImageRefError::TraversalEscape {
            reference: image_ref.to_string(),
        });
    }

    Ok(resolved)
}

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

#[allow(dead_code)]
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fresh_doc_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

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
        let err = validate_link_url("example.com").expect_err("bare host rejected");
        assert!(matches!(err, UrlError::MissingScheme));
    }

    #[test]
    fn link_url_scheme_match_is_case_insensitive() {
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
        assert_eq!(ALLOWED_LINK_SCHEMES, &["http", "https"]);
    }

    #[test]
    fn image_ref_traversal_is_rejected() {
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

    #[test]
    fn image_ref_missing_file_inside_doc_root_returns_ok() {
        let tmp = fresh_doc_root();
        let resolved = validate_image_ref(tmp.path(), "missing.png").expect("missing ok");
        assert!(resolved.starts_with(tmp.path().canonicalize().unwrap()));
    }

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

    #[test]
    fn windows_drive_letter_is_not_a_scheme() {
        assert_eq!(extract_scheme("C:\\Users\\foo.png"), None);
        assert_eq!(extract_scheme("D:/foo.png"), None);
    }

    #[test]
    fn multi_char_scheme_is_detected() {
        assert_eq!(extract_scheme("http://x"), Some("http"));
        assert_eq!(extract_scheme("javascript:alert(1)"), Some("javascript"));
        assert_eq!(extract_scheme("ssh:foo"), Some("ssh"));
    }

    #[test]
    fn rfc3986_composite_schemes_are_detected() {
        assert_eq!(extract_scheme("git+ssh:host/repo"), Some("git+ssh"));
        assert_eq!(
            extract_scheme("chrome-extension://abc/x.png"),
            Some("chrome-extension")
        );
        assert_eq!(extract_scheme("coap+tcp://srv"), Some("coap+tcp"));
        assert_eq!(extract_scheme("svn.foo:repo"), Some("svn.foo"));
        assert_eq!(extract_scheme("h2c:host"), Some("h2c"));
        assert_eq!(extract_scheme("1http:host"), None);
        assert_eq!(extract_scheme("+ssh:host"), None);
        assert_eq!(extract_scheme(".net:foo"), None);
    }

    #[test]
    fn image_ref_composite_scheme_is_rejected() {
        let tmp = fresh_doc_root();
        let err = validate_image_ref(tmp.path(), "git+ssh:host/repo")
            .expect_err("composite scheme rejected");
        assert!(matches!(err, ImageRefError::DisallowedScheme(s) if s == "git+ssh"));
    }
}

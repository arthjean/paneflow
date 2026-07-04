//! Detect local dev-server metadata (port, URL, framework label) from a
//! single line of PTY output.
//!
//! Called by `TerminalState::scan_output` after each output batch.  The
//! returned `ServiceInfo` is surfaced in the workspace sidebar.
//!
//! Keep the matchers string-based and allocation-light: this runs on every
//! terminal write batch, on the GPUI main thread.

/// Metadata about a detected service (server listening on a port).
/// Enriches the bare port number from the OS port scan (`workspace::ports`;
/// Linux `/proc/net/tcp`, macOS libproc, Windows IP Helper) with human-readable info.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceInfo {
    pub port: u16,
    pub url: Option<String>,
    pub label: Option<String>,
    /// True for frontend dev servers (Next.js, Vite, Nuxt) - clickable in sidebar.
    pub is_frontend: bool,
}

/// Parse a terminal output line for local server URL patterns.
/// Derived from VS Code's UrlFinder - anchors on localhost/127.0.0.1/0.0.0.0.
pub(super) fn parse_service_line(line: &str) -> Option<ServiceInfo> {
    let port = extract_local_port(line)?;
    if port == 0 {
        return None;
    }
    // Security (EP-005 review): the URL feeds `open::that` behind a single
    // click (sidebar chip + tab port badge). The PORT anchor above proves a
    // loopback service exists on the line, but `extract_url` independently
    // grabs the first http(s) token - a hostile pane printing
    // `localhost:5173 http://evil.example` would otherwise arm a clickable
    // badge to an attacker URL. Only keep a loopback URL; anything else
    // degrades to a synthesized localhost URL so legitimate frontends stay
    // clickable.
    let url = extract_url(line)
        .and_then(|u| normalize_loopback_url(&u, port))
        .or_else(|| Some(format!("http://localhost:{port}")));
    let (label, is_frontend) = detect_framework(line);
    Some(ServiceInfo {
        port,
        url,
        label,
        is_frontend,
    })
}

/// Whether a URL's host is a loopback/unspecified local address. Tiny
/// scheme-then-host parse - no URL crate; conservative `false` on anything
/// unrecognized (the caller then substitutes a synthesized localhost URL).
#[cfg(test)]
fn is_loopback_url(url: &str) -> bool {
    normalize_loopback_url(url, 0).is_some()
}

fn normalize_loopback_url(url: &str, port: u16) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    let rest = rest?;
    let scheme = if url.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let suffix = &rest[authority_end..];

    let (host, host_tail) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        (&authority[..=close], &authority[close + 1..])
    } else {
        let host_end = authority.find(':').unwrap_or(authority.len());
        (&authority[..host_end], &authority[host_end..])
    };

    if !is_loopback_host(host) {
        return None;
    }
    if host == "0.0.0.0" {
        let tail = if host_tail.is_empty() {
            format!(":{port}")
        } else {
            host_tail.to_string()
        };
        return Some(format!("{scheme}://localhost{tail}{suffix}"));
    }
    Some(url.to_string())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "0.0.0.0"
        || host == "[::1]"
        || host
            .strip_prefix("127.")
            .is_some_and(|tail| tail.split('.').all(|seg| seg.parse::<u8>().is_ok()))
}

/// Extract a port number from localhost:PORT, 127.0.0.1:PORT, 0.0.0.0:PORT,
/// or [::1]:PORT patterns.
/// Also handles Python's `http.server` format: "HTTP on 127.0.0.1 port 8000".
fn extract_local_port(line: &str) -> Option<u16> {
    for anchor in ["localhost:", "127.0.0.1:", "0.0.0.0:", "[::1]:"] {
        if let Some(idx) = line.find(anchor) {
            let after = &line[idx + anchor.len()..];
            let port_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = port_str.parse::<u16>() {
                return Some(port);
            }
        }
    }
    // Python http.server: "HTTP on 127.0.0.1 port 8000"
    if let Some(idx) = line.find(" port ")
        && (line.contains("127.0.0.1") || line.contains("0.0.0.0") || line.contains("[::1]"))
    {
        let after = &line[idx + 6..];
        let port_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(port) = port_str.parse::<u16>() {
            return Some(port);
        }
    }
    None
}

/// Extract a full URL (http:// or https://) from a terminal line.
fn extract_url(line: &str) -> Option<String> {
    for scheme in ["https://", "http://"] {
        if let Some(start) = line.find(scheme) {
            let url: String = line[start..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ')' && *c != '"' && *c != '\'')
                .collect();
            if url.len() > scheme.len() {
                return Some(url);
            }
        }
    }
    None
}

/// Detect the framework/server name from keywords in the terminal line.
/// Returns `(label, is_frontend)` - frontend frameworks get clickable URLs in the sidebar.
/// Uses word-boundary matching to avoid false positives (e.g. "origin" matching "gin").
pub(super) fn detect_framework(line: &str) -> (Option<String>, bool) {
    // (keyword, display_label, is_frontend)
    const FRAMEWORKS: &[(&str, &str, bool)] = &[
        ("next.js", "Next.js", true),
        ("next dev", "Next.js", true),
        ("turbopack", "Next.js", true),
        ("vite", "Vite", true),
        ("nuxt", "Nuxt", true),
        ("remix", "Remix", true),
        ("astro", "Astro", true),
        ("webpack-dev-server", "Webpack", true),
        ("angular", "Angular", true),
        ("express", "Express", false),
        ("fastify", "Fastify", false),
        ("uvicorn", "uvicorn", false),
        ("flask", "Flask", false),
        ("django", "Django", false),
        ("rocket", "Rocket", false),
        ("actix-web", "Actix", false),
        ("axum", "Axum", false),
        ("gin-gonic", "Gin", false),
        ("fiber", "Fiber", false),
        ("puma", "Puma", false),
        ("tomcat", "Tomcat", false),
        ("laravel", "Laravel", false),
        ("spring boot", "Spring", false),
    ];
    let lower = line.to_lowercase();
    for (key, label, frontend) in FRAMEWORKS {
        if contains_keyword(&lower, key) {
            return (Some(label.to_string()), *frontend);
        }
    }
    (None, false)
}

fn contains_keyword(haystack: &str, needle: &str) -> bool {
    let mut offset = 0;
    while let Some(found) = haystack[offset..].find(needle) {
        let start = offset + found;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        offset = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // EP-005 security review: the clickable URL must never leave loopback -
    // a hostile pane printing a localhost anchor next to an attacker URL
    // must not arm `open::that` toward that host.
    #[test]
    fn hostile_url_next_to_local_anchor_is_replaced_by_loopback() {
        let info =
            parse_service_line("vite dev server ready localhost:5173 see http://evil.example/x")
                .unwrap();
        assert_eq!(info.port, 5173);
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173"));
    }

    #[test]
    fn legitimate_loopback_url_is_kept_verbatim() {
        let info = parse_service_line("  ➜  Local:   http://localhost:5173/app").unwrap();
        assert_eq!(info.port, 5173);
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173/app"));
    }

    #[test]
    fn unspecified_host_url_is_rewritten_to_localhost() {
        let info = parse_service_line("Local: http://0.0.0.0:5173/app").unwrap();
        assert_eq!(info.port, 5173);
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173/app"));
    }

    #[test]
    fn url_userinfo_cannot_smuggle_a_remote_host() {
        let info =
            parse_service_line("vite ready at http://localhost:5173@evil.example/path").unwrap();
        assert_eq!(info.port, 5173);
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173"));
    }

    #[test]
    fn ipv6_loopback_anchor_is_detected() {
        let info = parse_service_line("Local: http://[::1]:5173/app").unwrap();
        assert_eq!(info.port, 5173);
        assert_eq!(info.url.as_deref(), Some("http://[::1]:5173/app"));
    }

    #[test]
    fn line_without_printed_url_synthesizes_loopback() {
        let info = parse_service_line("Serving HTTP on 127.0.0.1 port 8000").unwrap();
        assert_eq!(info.port, 8000);
        assert_eq!(info.url.as_deref(), Some("http://localhost:8000"));
    }

    #[test]
    fn frontend_frameworks_are_labeled_clickable() {
        let info = parse_service_line("VITE v7 ready at http://localhost:5173/").unwrap();
        assert_eq!(info.label.as_deref(), Some("Vite"));
        assert!(info.is_frontend);

        let info = parse_service_line("Next.js dev server http://localhost:3000").unwrap();
        assert_eq!(info.label.as_deref(), Some("Next.js"));
        assert!(info.is_frontend);
    }

    #[test]
    fn backend_frameworks_are_labeled_not_clickable_by_text_alone() {
        let info = parse_service_line("Fastify listening at http://127.0.0.1:3001").unwrap();
        assert_eq!(info.label.as_deref(), Some("Fastify"));
        assert!(!info.is_frontend);
    }

    #[test]
    fn framework_detection_rejects_substring_lookalikes() {
        assert_eq!(
            detect_framework("origin: http://localhost:3000"),
            (None, false)
        );
        assert_eq!(
            detect_framework("invite users at localhost:5173"),
            (None, false)
        );
        assert_eq!(
            detect_framework("fibers listening on localhost:3002"),
            (None, false)
        );
    }

    #[test]
    fn is_loopback_url_host_classes() {
        assert!(is_loopback_url("http://localhost:3000"));
        assert!(is_loopback_url("http://LOCALHOST:3000/x"));
        assert!(is_loopback_url("https://127.0.0.1:8443/"));
        assert!(is_loopback_url("http://127.1.2.3:80"));
        assert!(is_loopback_url("http://0.0.0.0:5173"));
        assert!(is_loopback_url("http://[::1]:5173/app"));
        assert!(!is_loopback_url("http://evil.example/x"));
        assert!(!is_loopback_url("http://localhost.evil.example:3000"));
        assert!(!is_loopback_url("http://localhost:3000@evil.example"));
        assert!(!is_loopback_url("http://127.evil.example/"));
        assert!(!is_loopback_url("file:///etc/passwd"));
        assert!(!is_loopback_url("http://192.168.1.10:3000"));
    }
}

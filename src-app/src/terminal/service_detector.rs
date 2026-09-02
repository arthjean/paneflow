use std::collections::VecDeque;

const SERVICE_TAIL_MAX_LINES: usize = 100;
const SERVICE_TAIL_MAX_LINE_BYTES: usize = 8 * 1024;
const SERVICE_TAIL_MAX_TOTAL_BYTES: usize = 64 * 1024;
const TAB_WIDTH: usize = 8;

#[derive(Default)]
pub(super) struct ServiceOutputTail {
    parser: ServiceOutputParser,
    output: ServiceOutputPerformer,
}

impl ServiceOutputTail {
    pub(super) fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.output, bytes);
    }

    pub(super) fn recent_lines(&self) -> Vec<String> {
        self.output.recent_lines()
    }
}

#[derive(Clone, Copy, Default)]
enum ServiceOutputParseState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

#[derive(Default)]
struct ServiceOutputParser {
    state: ServiceOutputParseState,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_expected: usize,
}

impl ServiceOutputParser {
    fn advance(&mut self, output: &mut ServiceOutputPerformer, bytes: &[u8]) {
        for &byte in bytes {
            self.advance_byte(output, byte);
        }
    }

    fn advance_byte(&mut self, output: &mut ServiceOutputPerformer, byte: u8) {
        if self.utf8_len != 0 {
            if byte & 0b1100_0000 == 0b1000_0000 {
                self.utf8[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_expected {
                    let character = std::str::from_utf8(&self.utf8[..self.utf8_len])
                        .ok()
                        .and_then(|text| text.chars().next())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    self.utf8_len = 0;
                    self.utf8_expected = 0;
                    output.push_char(character);
                }
                return;
            }
            self.utf8_len = 0;
            self.utf8_expected = 0;
            output.push_char(char::REPLACEMENT_CHARACTER);
        }

        match self.state {
            ServiceOutputParseState::Ground => match byte {
                0x1b => self.state = ServiceOutputParseState::Escape,
                0x9b => self.state = ServiceOutputParseState::Csi,
                0x9d => self.state = ServiceOutputParseState::Osc,
                0x90 | 0x98 | 0x9e | 0x9f => self.state = ServiceOutputParseState::String,
                0x00..=0x1f | 0x7f => output.execute(byte),
                0x20..=0x7e => output.push_char(char::from(byte)),
                0xc2..=0xdf => self.begin_utf8(byte, 2),
                0xe0..=0xef => self.begin_utf8(byte, 3),
                0xf0..=0xf4 => self.begin_utf8(byte, 4),
                _ => output.push_char(char::REPLACEMENT_CHARACTER),
            },
            ServiceOutputParseState::Escape => {
                self.state = match byte {
                    b'[' => ServiceOutputParseState::Csi,
                    b']' => ServiceOutputParseState::Osc,
                    b'P' | b'X' | b'^' | b'_' => ServiceOutputParseState::String,
                    0x20..=0x2f => ServiceOutputParseState::EscapeIntermediate,
                    0x1b => ServiceOutputParseState::Escape,
                    _ => ServiceOutputParseState::Ground,
                };
            }
            ServiceOutputParseState::EscapeIntermediate => {
                if byte == 0x1b {
                    self.state = ServiceOutputParseState::Escape;
                } else if matches!(byte, 0x18 | 0x1a | 0x30..=0x7e) {
                    self.state = ServiceOutputParseState::Ground;
                }
            }
            ServiceOutputParseState::Csi => {
                if byte == 0x1b {
                    self.state = ServiceOutputParseState::Escape;
                } else if matches!(byte, 0x18 | 0x1a | 0x40..=0x7e) {
                    self.state = ServiceOutputParseState::Ground;
                }
            }
            ServiceOutputParseState::Osc => {
                self.state = match byte {
                    0x07 | 0x18 | 0x1a => ServiceOutputParseState::Ground,
                    0x1b => ServiceOutputParseState::OscEscape,
                    _ => ServiceOutputParseState::Osc,
                };
            }
            ServiceOutputParseState::OscEscape => {
                self.state = match byte {
                    b'\\' | 0x07 | 0x18 | 0x1a => ServiceOutputParseState::Ground,
                    0x1b => ServiceOutputParseState::OscEscape,
                    _ => ServiceOutputParseState::Osc,
                };
            }
            ServiceOutputParseState::String => {
                self.state = match byte {
                    0x18 | 0x1a => ServiceOutputParseState::Ground,
                    0x1b => ServiceOutputParseState::StringEscape,
                    _ => ServiceOutputParseState::String,
                };
            }
            ServiceOutputParseState::StringEscape => {
                self.state = match byte {
                    b'\\' | 0x18 | 0x1a => ServiceOutputParseState::Ground,
                    0x1b => ServiceOutputParseState::StringEscape,
                    _ => ServiceOutputParseState::String,
                };
            }
        }
    }

    fn begin_utf8(&mut self, byte: u8, expected: usize) {
        self.utf8[0] = byte;
        self.utf8_len = 1;
        self.utf8_expected = expected;
    }
}

#[derive(Default)]
struct ServiceOutputPerformer {
    completed: VecDeque<String>,
    completed_bytes: usize,
    current: String,
    carriage_return_pending: bool,
}

impl ServiceOutputPerformer {
    fn prepare_for_write(&mut self) {
        if self.carriage_return_pending {
            self.current.clear();
            self.carriage_return_pending = false;
        }
    }

    fn push_char(&mut self, character: char) {
        self.prepare_for_write();
        if self.current.len().saturating_add(character.len_utf8()) <= SERVICE_TAIL_MAX_LINE_BYTES {
            self.current.push(character);
        }
        self.enforce_caps();
    }

    fn push_tab(&mut self) {
        self.prepare_for_write();
        let column = self.current.chars().count();
        let spaces = TAB_WIDTH - column % TAB_WIDTH;
        for _ in 0..spaces {
            if self.current.len() == SERVICE_TAIL_MAX_LINE_BYTES {
                break;
            }
            self.current.push(' ');
        }
        self.enforce_caps();
    }

    fn backspace(&mut self) {
        self.prepare_for_write();
        self.current.pop();
    }

    fn finish_line(&mut self) {
        self.carriage_return_pending = false;
        let trimmed = self.current.trim_end();
        if !trimmed.is_empty() {
            let line = trimmed.to_owned();
            self.completed_bytes = self.completed_bytes.saturating_add(line.len());
            self.completed.push_back(line);
        }
        self.current.clear();
        self.enforce_caps();
    }

    fn enforce_caps(&mut self) {
        let current_is_non_empty = !self.current.trim().is_empty();
        while self.completed.len() + usize::from(current_is_non_empty) > SERVICE_TAIL_MAX_LINES
            || self.completed_bytes.saturating_add(self.current.len())
                > SERVICE_TAIL_MAX_TOTAL_BYTES
        {
            let Some(removed) = self.completed.pop_front() else {
                break;
            };
            self.completed_bytes = self.completed_bytes.saturating_sub(removed.len());
        }
    }

    fn recent_lines(&self) -> Vec<String> {
        let mut lines =
            Vec::with_capacity(SERVICE_TAIL_MAX_LINES.min(self.completed.len().saturating_add(1)));
        let current = self.current.trim_end();
        if !current.is_empty() {
            lines.push(current.to_owned());
        }
        lines.extend(
            self.completed
                .iter()
                .rev()
                .take(SERVICE_TAIL_MAX_LINES.saturating_sub(lines.len()))
                .cloned(),
        );
        lines
    }
}

impl ServiceOutputPerformer {
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.finish_line(),
            b'\r' => self.carriage_return_pending = true,
            b'\x08' => self.backspace(),
            b'\t' => self.push_tab(),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServiceInfo {
    pub port: u16,
    pub url: Option<String>,
    pub label: Option<String>,
    pub is_frontend: bool,
}

pub(super) fn parse_service_line(line: &str) -> Option<ServiceInfo> {
    let port = extract_local_port(line)?;
    if port == 0 {
        return None;
    }
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

pub(super) fn detect_framework(line: &str) -> (Option<String>, bool) {
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

    #[test]
    fn service_tail_preserves_fragmented_utf8_and_strips_fragmented_ansi() {
        let mut tail = ServiceOutputTail::default();
        let utf8 = "prêt".as_bytes();
        tail.advance(&utf8[..3]);
        tail.advance(&utf8[3..]);
        tail.advance(b" \x1b[3");
        tail.advance(b"2mhttp://localhost:5173\x1b[0m");

        assert_eq!(
            tail.recent_lines(),
            vec!["prêt http://localhost:5173".to_owned()]
        );
    }

    #[test]
    fn service_tail_models_cr_progress_backspace_tabs_and_current_line() {
        let mut tail = ServiceOutputTail::default();
        tail.advance(b"progress 10%\rprogress 20%\r\nport: 1234\x08\x0856\nA\tB");

        assert_eq!(
            tail.recent_lines(),
            vec![
                "A       B".to_owned(),
                "port: 1256".to_owned(),
                "progress 20%".to_owned(),
            ]
        );
    }

    #[test]
    fn service_tail_enforces_line_count_line_size_and_total_size_caps() {
        let mut tail = ServiceOutputTail::default();
        let oversized = vec![b'x'; SERVICE_TAIL_MAX_LINE_BYTES + 512];
        tail.advance(&oversized);
        tail.advance(b"\n");
        assert_eq!(tail.recent_lines()[0].len(), SERVICE_TAIL_MAX_LINE_BYTES);

        let line = vec![b'y'; 1024];
        for _ in 0..120 {
            tail.advance(&line);
            tail.advance(b"\n");
        }
        let lines = tail.recent_lines();
        assert!(lines.len() <= SERVICE_TAIL_MAX_LINES);
        assert!(lines.iter().map(String::len).sum::<usize>() <= SERVICE_TAIL_MAX_TOTAL_BYTES);
        assert!(
            lines
                .iter()
                .all(|line| line.len() <= SERVICE_TAIL_MAX_LINE_BYTES)
        );
    }

    #[test]
    fn service_tail_discards_unterminated_control_strings_without_buffering_them() {
        let mut tail = ServiceOutputTail::default();
        tail.advance(b"\x1b]");
        let hostile = vec![b'x'; SERVICE_TAIL_MAX_TOTAL_BYTES * 2];
        tail.advance(&hostile);
        assert!(tail.recent_lines().is_empty());

        tail.advance(b"\x07http://localhost:4321");
        assert_eq!(
            tail.recent_lines(),
            vec!["http://localhost:4321".to_owned()]
        );
    }

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

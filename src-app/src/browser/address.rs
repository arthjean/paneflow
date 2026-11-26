use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Address {
    Go(String),
    Refused(String),
}

pub(crate) fn resolve(input: &str) -> Address {
    let typed = input.trim();
    if typed.is_empty() {
        return Address::Refused("Type an address, such as localhost:3000".to_string());
    }
    if has_scheme(typed) {
        return Address::Go(typed.to_string());
    }
    if typed.chars().any(char::is_whitespace) {
        return refused(typed);
    }
    let authority = typed.split(['/', '?', '#']).next().unwrap_or_default();
    let Some((host, port)) = split_port(authority) else {
        return refused(typed);
    };
    let ip = parse_ip(host);
    let addressable = host.eq_ignore_ascii_case("localhost")
        || ip.is_some()
        || port.is_some()
        || (host.contains('.') && !host.starts_with('.') && !host.ends_with('.'));
    if !addressable || host.is_empty() {
        return refused(typed);
    }
    let scheme = if is_loopback(host, ip) {
        "http"
    } else {
        "https"
    };
    Address::Go(format!("{scheme}://{typed}"))
}

pub(crate) fn is_loopback_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    split_port(authority).is_some_and(|(host, _)| is_loopback(host, parse_ip(host)))
}

pub(crate) fn refusal(url: &str) -> Option<String> {
    use paneflow_browser_proto::navigation::{Frame, Verdict, classify};
    match classify(url, Frame::TopLevel) {
        Verdict::Refuse(refusal) => Some(format!(
            "Paneflow did not open this address: {refusal}. The page stays as it was."
        )),
        Verdict::Allow | Verdict::External { .. } => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Display {
    pub(crate) host: String,
    pub(crate) rest: String,
    pub(crate) secure: bool,
}

pub(crate) fn display(url: &str) -> Option<Display> {
    let (scheme, after) = url.split_once("://")?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    let split = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, tail) = after.split_at(split);
    let (host, port) = match authority.rfind(':') {
        Some(colon) if !authority[colon..].contains(']') => authority.split_at(colon),
        _ => (authority, ""),
    };
    if host.is_empty() {
        return None;
    }
    let tail = if tail == "/" { "" } else { tail };
    let shown = paneflow_browser_proto::limits::MAX_URL_DISPLAY_CHARS;
    let tail = tail
        .char_indices()
        .nth(shown)
        .map_or(tail, |(cut, _)| &tail[..cut]);
    Some(Display {
        host: host.to_string(),
        rest: format!("{port}{tail}"),
        secure,
    })
}

fn refused(typed: &str) -> Address {
    Address::Refused(format!(
        "\u{201c}{typed}\u{201d} is not an address. Type a URL such as localhost:3000 or example.com"
    ))
}

fn has_scheme(typed: &str) -> bool {
    let Some((scheme, rest)) = typed.split_once(':') else {
        return false;
    };
    let valid = scheme
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return false;
    }
    if rest.starts_with("//") {
        return true;
    }
    !is_port_then_path(rest)
}

fn is_port_then_path(rest: &str) -> bool {
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && rest[digits..]
            .chars()
            .next()
            .is_none_or(|next| matches!(next, '/' | '?' | '#'))
}

fn split_port(authority: &str) -> Option<(&str, Option<u16>)> {
    if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        return match after.strip_prefix(':') {
            Some(port) => Some((host, Some(port.parse().ok()?))),
            None if after.is_empty() => Some((host, None)),
            None => None,
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host, Some(port.parse().ok()?))),
        Some(_) => authority
            .parse::<Ipv6Addr>()
            .ok()
            .map(|_| (authority, None)),
        None => Some((authority, None)),
    }
}

fn parse_ip(host: &str) -> Option<IpAddr> {
    host.parse::<Ipv4Addr>()
        .map(IpAddr::V4)
        .or_else(|_| host.parse::<Ipv6Addr>().map(IpAddr::V6))
        .ok()
}

fn is_loopback(host: &str, ip: Option<IpAddr>) -> bool {
    let lower = host.to_ascii_lowercase();
    lower == "localhost" || lower.ends_with(".localhost") || ip.is_some_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(input: &str) -> String {
        match resolve(input) {
            Address::Go(url) => url,
            Address::Refused(reason) => panic!("{input} was refused: {reason}"),
        }
    }

    #[test]
    fn an_address_with_a_scheme_is_kept_as_typed() {
        assert_eq!(go("http://example.com/a"), "http://example.com/a");
        assert_eq!(go("https://127.0.0.1:8443"), "https://127.0.0.1:8443");
        assert_eq!(go("about:blank"), "about:blank");
        assert_eq!(go("file:///etc/passwd"), "file:///etc/passwd");
        assert_eq!(go("chrome://settings"), "chrome://settings");
    }

    #[test]
    fn a_loopback_host_gets_http_and_any_other_host_gets_https() {
        assert_eq!(go("localhost"), "http://localhost");
        assert_eq!(
            go("localhost:3000/app?x=1"),
            "http://localhost:3000/app?x=1"
        );
        assert_eq!(go("127.0.0.1:5173"), "http://127.0.0.1:5173");
        assert_eq!(go("[::1]:8080/"), "http://[::1]:8080/");
        assert_eq!(go("app.localhost:3000"), "http://app.localhost:3000");
        assert_eq!(go("example.com"), "https://example.com");
        assert_eq!(go("docs.rs/gpui"), "https://docs.rs/gpui");
        assert_eq!(go("10.0.0.2"), "https://10.0.0.2");
        assert_eq!(go("devbox:8080"), "https://devbox:8080");
    }

    #[test]
    fn anything_else_is_refused_with_a_message() {
        for input in ["", "   ", "hello", "how do I center a div", "foo.", ".com"] {
            assert!(
                matches!(resolve(input), Address::Refused(_)),
                "{input:?} was accepted"
            );
        }
        let Address::Refused(reason) = resolve("hello") else {
            unreachable!()
        };
        assert!(reason.contains("hello") && reason.contains("localhost:3000"));
    }

    #[test]
    fn a_web_address_is_shown_without_its_scheme_with_the_host_apart() {
        assert_eq!(
            display("http://127.0.0.1:18731/"),
            Some(Display {
                host: "127.0.0.1".into(),
                rest: ":18731".into(),
                secure: false
            })
        );
        assert_eq!(
            display("https://paneflow.dev/docs?q=1#top"),
            Some(Display {
                host: "paneflow.dev".into(),
                rest: "/docs?q=1#top".into(),
                secure: true
            })
        );
        assert_eq!(
            display("http://[::1]:8080/a"),
            Some(Display {
                host: "[::1]".into(),
                rest: ":8080/a".into(),
                secure: false
            })
        );
        let long = format!("https://www.google.com/search?q={}", "a".repeat(40_000));
        let shown = display(&long).unwrap();
        assert_eq!(
            shown.rest.chars().count(),
            paneflow_browser_proto::limits::MAX_URL_DISPLAY_CHARS,
            "the field shows at most Chromium's display length of a long address"
        );
        assert!(shown.rest.starts_with("/search?q=aaa"));
        assert_eq!(display("about:blank"), None);
        assert_eq!(display("file:///etc/hosts"), None);
    }

    #[test]
    fn a_refused_scheme_is_named_before_anything_is_sent_to_an_open_tab() {
        let file = refusal("file:///etc/passwd").expect("file: is refused");
        assert!(
            file.contains("file:") && file.contains("stays as it was"),
            "{file}"
        );
        let chrome = refusal("chrome://settings").expect("chrome: is refused");
        assert!(chrome.contains("chrome:"), "{chrome}");
        assert_eq!(refusal("http://localhost:3000/"), None);
        assert_eq!(refusal("https://example.com/"), None);
        assert_eq!(refusal("about:blank"), None);
    }

    #[test]
    fn only_http_urls_on_a_loopback_host_count_as_local_links() {
        assert!(is_loopback_url("http://localhost:3000/"));
        assert!(is_loopback_url("http://127.0.0.1:5173/a"));
        assert!(is_loopback_url("https://[::1]:8443"));
        assert!(!is_loopback_url("https://example.com"));
        assert!(!is_loopback_url("http://localhost.example.com"));
        assert!(!is_loopback_url("file:///tmp"));
        assert!(!is_loopback_url("http://user@example.com"));
    }
}

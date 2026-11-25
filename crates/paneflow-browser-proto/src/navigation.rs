use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::limits::MAX_URL_CHARS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    TopLevel,
    Subframe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    External { origin: String },
    Refuse(NavigationRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NavigationRefusal {
    #[error("the {scheme}: scheme is not allowed in the Paneflow browser")]
    Scheme { scheme: String },
    #[error("a URL that embeds credentials is not allowed")]
    Credentials,
    #[error("the URL has {length} characters, over the limit of {max}")]
    TooLong { length: usize, max: usize },
    #[error("the URL is not valid: {reason}")]
    Invalid { reason: String },
    #[error("the origin {origin} was denied")]
    OriginDenied { origin: String },
    #[error("the origin {origin} has no decision, and a frame inside a page cannot ask for one")]
    OriginUndecided { origin: String },
}

const INERT_ABOUT_URLS: &[&str] = &["about:blank", "about:srcdoc"];

pub fn classify(raw: &str, frame: Frame) -> Verdict {
    let length = raw.chars().count();
    if length > MAX_URL_CHARS {
        return Verdict::Refuse(NavigationRefusal::TooLong {
            length,
            max: MAX_URL_CHARS,
        });
    }
    let url = match Url::parse(raw) {
        Ok(url) => url,
        Err(error) => {
            return Verdict::Refuse(NavigationRefusal::Invalid {
                reason: error.to_string(),
            });
        }
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Verdict::Refuse(NavigationRefusal::Credentials);
    }
    match url.scheme() {
        "http" | "https" => classify_web(&url),
        "about" if INERT_ABOUT_URLS.contains(&url.as_str()) => Verdict::Allow,
        "data" if frame == Frame::Subframe => Verdict::Allow,
        "blob" => match Url::parse(url.path()) {
            Ok(inner) if matches!(inner.scheme(), "http" | "https") => classify_web(&inner),
            _ => refuse_scheme(&url),
        },
        _ => refuse_scheme(&url),
    }
}

fn refuse_scheme(url: &Url) -> Verdict {
    Verdict::Refuse(NavigationRefusal::Scheme {
        scheme: url.scheme().to_string(),
    })
}

fn classify_web(url: &Url) -> Verdict {
    match url.host() {
        Some(host) if is_loopback_host(&host) => Verdict::Allow,
        Some(_) => Verdict::External {
            origin: url.origin().ascii_serialization(),
        },
        None => Verdict::Refuse(NavigationRefusal::Invalid {
            reason: "the URL names no host".to_string(),
        }),
    }
}

pub fn origin_of(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let origin = url.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => is_loopback_ipv6(address),
    }
}

fn is_loopback_ipv6(address: &Ipv6Addr) -> bool {
    address.is_loopback()
        || address
            .to_ipv4_mapped()
            .is_some_and(|mapped: Ipv4Addr| IpAddr::V4(mapped).is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(url: &str) -> Verdict {
        classify(url, Frame::TopLevel)
    }

    #[test]
    fn loopback_http_and_https_proceed_without_a_decision() {
        for url in [
            "http://127.0.0.1:5173/",
            "https://localhost/app",
            "http://app.localhost:3000/",
            "http://[::1]:8080/",
            "http://127.4.5.6/",
            "http://[::ffff:127.0.0.1]/",
            "http://LOCALHOST./",
        ] {
            assert_eq!(top(url), Verdict::Allow, "{url}");
        }
    }

    #[test]
    fn a_public_origin_needs_a_decision() {
        assert_eq!(
            top("https://example.com/path?q=1"),
            Verdict::External {
                origin: "https://example.com".to_string()
            }
        );
        assert_eq!(
            top("http://localhost.example.com/"),
            Verdict::External {
                origin: "http://localhost.example.com".to_string()
            }
        );
        assert_eq!(
            top("http://192.168.1.10:3000/"),
            Verdict::External {
                origin: "http://192.168.1.10:3000".to_string()
            }
        );
    }

    #[test]
    fn engine_and_local_schemes_are_refused_naming_the_scheme() {
        for (url, scheme) in [
            ("file:///etc/passwd", "file"),
            ("chrome://settings", "chrome"),
            ("devtools://devtools/bundled/inspector.html", "devtools"),
            ("view-source:http://127.0.0.1/", "view-source"),
            ("javascript:alert(1)", "javascript"),
            ("data:text/html,<p>hi</p>", "data"),
            ("about:version", "about"),
            ("chrome-extension://abc/page.html", "chrome-extension"),
        ] {
            let verdict = top(url);
            assert_eq!(
                verdict,
                Verdict::Refuse(NavigationRefusal::Scheme {
                    scheme: scheme.to_string()
                }),
                "{url}"
            );
            let Verdict::Refuse(refusal) = verdict else {
                unreachable!()
            };
            assert!(refusal.to_string().contains(&format!("{scheme}:")));
        }
    }

    #[test]
    fn a_data_url_is_refused_at_the_top_level_only() {
        assert_eq!(
            classify("data:text/html,<p>hi</p>", Frame::Subframe),
            Verdict::Allow
        );
    }

    #[test]
    fn a_url_that_embeds_credentials_is_refused() {
        for url in [
            "http://user:secret@127.0.0.1/",
            "http://user@localhost/",
            "https://:pw@example.com/",
        ] {
            assert_eq!(
                top(url),
                Verdict::Refuse(NavigationRefusal::Credentials),
                "{url}"
            );
        }
    }

    #[test]
    fn a_url_over_the_limit_is_refused() {
        let url = format!("http://127.0.0.1/{}", "a".repeat(MAX_URL_CHARS));
        assert!(matches!(
            top(&url),
            Verdict::Refuse(NavigationRefusal::TooLong {
                max: MAX_URL_CHARS,
                ..
            })
        ));
    }

    #[test]
    fn blank_pages_and_loopback_blobs_proceed() {
        assert_eq!(top("about:blank"), Verdict::Allow);
        assert_eq!(
            top("blob:http://127.0.0.1:8000/550e8400-e29b-41d4-a716-446655440000"),
            Verdict::Allow
        );
        assert_eq!(
            top("blob:https://example.com/550e8400-e29b-41d4-a716-446655440000"),
            Verdict::External {
                origin: "https://example.com".to_string()
            }
        );
    }
}

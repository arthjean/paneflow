use serde::{Deserialize, Serialize};

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = &'static str;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                if value.is_empty() || value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_') {
                    return Err("identity must contain 1 to 64 ASCII letters, digits, hyphens or underscores");
                }
                Ok(Self(value))
            }
        }
    )+};
}

identity!(BrowserId, ProfileId, WorkspaceId, SessionId, OperationId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub workspace: WorkspaceId,
    pub session: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub owner: Owner,
    pub browser: BrowserId,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Absent,
    Development,
    HumanQualified,
    AgentQualified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Dormant,
    Starting,
    Visible,
    Hidden,
    Closing,
    Crashed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserPresentation {
    pub mounted: bool,
    pub visible: bool,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
    #[serde(default = "default_scale_percent")]
    pub scale_percent: u32,
}

fn default_scale_percent() -> u32 {
    100
}

impl BrowserPresentation {
    pub fn unmounted() -> Self {
        Self {
            mounted: false,
            visible: false,
            width: 0,
            height: 0,
            generation: 0,
            scale_percent: 100,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.width <= 16384
            && self.height <= 16384
            && (50..=400).contains(&self.scale_percent)
            && (!self.visible || (self.mounted && self.width != 0 && self.height != 0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserSession {
    pub document: Document,
    pub profile: ProfileId,
    pub url: String,
    pub title: String,
    pub state: SessionState,
    pub presentation: BrowserPresentation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserError {
    TooLarge,
    InvalidMessage,
    IncompatibleVersion,
    UnknownIdentity,
    AccessDenied,
    StaleGeneration,
    Unavailable,
    Busy,
    LimitReached,
    InvalidUrl,
    InvalidFrame,
}

pub const MAX_URL_BYTES: usize = 8 * 1024;
pub const MAX_TITLE_CHARS: usize = 512;
pub const MAX_BROWSERS_PER_SESSION: usize = 8;
pub const MAX_BROWSERS_TOTAL: usize = 64;
pub const MAX_LIVE_BROWSERS: usize = 8;
pub const MIN_ZOOM_PERCENT: u32 = 25;
pub const MAX_ZOOM_PERCENT: u32 = 500;
pub const BLANK_URL: &str = "about:blank";

pub fn validate_url(value: &str) -> Result<(), BrowserError> {
    if value.len() > MAX_URL_BYTES {
        return Err(BrowserError::TooLarge);
    }
    if value == BLANK_URL {
        return Ok(());
    }
    let parsed = url::Url::parse(value).map_err(|_| BrowserError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.chars().any(char::is_control)
    {
        return Err(BrowserError::InvalidUrl);
    }
    Ok(())
}

pub fn normalize_address(input: &str) -> Result<String, BrowserError> {
    if input.len() > MAX_URL_BYTES {
        return Err(BrowserError::TooLarge);
    }
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(BrowserError::InvalidUrl);
    }
    if trimmed.eq_ignore_ascii_case(BLANK_URL) {
        return Ok(BLANK_URL.to_string());
    }
    let has_scheme = trimmed.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && rest.starts_with("//")
    });
    let bare_scheme = trimmed.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme.bytes().all(|b| b.is_ascii_alphabetic())
            && !scheme.eq_ignore_ascii_case("localhost")
    });
    let candidate = if has_scheme {
        trimmed.to_string()
    } else if bare_scheme {
        return Err(BrowserError::InvalidUrl);
    } else {
        let authority_end = trimmed.find(['/', '?', '#']).unwrap_or(trimmed.len());
        let authority = &trimmed[..authority_end];
        if authority.contains('@') {
            return Err(BrowserError::InvalidUrl);
        }
        let (host, port) = split_host_port(authority).ok_or(BrowserError::InvalidUrl)?;
        let scheme = if is_loopback_or_ip(host) || port.is_some() {
            "http"
        } else if host.contains('.') {
            "https"
        } else {
            return Err(BrowserError::InvalidUrl);
        };
        format!("{scheme}://{trimmed}")
    };
    let parsed = url::Url::parse(&candidate).map_err(|_| BrowserError::InvalidUrl)?;
    validate_url(parsed.as_str())?;
    Ok(parsed.into())
}

fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return None;
        }
        return match &rest[end + 1..] {
            "" => Some((host, None)),
            port => Some((host, Some(port.strip_prefix(':')?.parse().ok()?))),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().ok()?;
            (!host.is_empty()).then_some((host, Some(port)))
        }
        None => Some((authority, None)),
    }
}

fn is_loopback_or_ip(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_loopback_and_ip_addresses_become_http() {
        assert_eq!(
            normalize_address("localhost:5173").unwrap(),
            "http://localhost:5173/"
        );
        assert_eq!(
            normalize_address(" 127.0.0.1:8080/app?x=1 ").unwrap(),
            "http://127.0.0.1:8080/app?x=1"
        );
        assert_eq!(
            normalize_address("[::1]:3000").unwrap(),
            "http://[::1]:3000/"
        );
        assert_eq!(
            normalize_address("app.localhost").unwrap(),
            "http://app.localhost/"
        );
        assert_eq!(
            normalize_address("example.com:8443/x").unwrap(),
            "http://example.com:8443/x"
        );
    }

    #[test]
    fn bare_domains_become_https_and_explicit_schemes_are_kept() {
        assert_eq!(
            normalize_address("example.com/docs").unwrap(),
            "https://example.com/docs"
        );
        assert_eq!(
            normalize_address("HTTP://Example.com").unwrap(),
            "http://example.com/"
        );
        assert_eq!(normalize_address("ABOUT:BLANK").unwrap(), BLANK_URL);
    }

    #[test]
    fn forbidden_schemes_userinfo_and_junk_are_refused_not_searched() {
        for input in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,hi",
            "mailto:someone@example.com",
            "chrome://settings",
            "http://user:pw@example.com",
            "user@example.com",
            "hello world",
            "notaurl",
            "",
            "http://",
            "[::1",
            "localhost:notaport",
            "http://exa\u{7}mple.com",
        ] {
            assert_eq!(
                normalize_address(input),
                Err(BrowserError::InvalidUrl),
                "{input:?} must be refused"
            );
        }
    }

    #[test]
    fn an_address_above_the_url_bound_is_refused_before_parsing() {
        let long = format!("http://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!(normalize_address(&long), Err(BrowserError::TooLarge));
        let at_bound = format!("http://example.com/{}", "a".repeat(MAX_URL_BYTES - 19));
        assert_eq!(at_bound.len(), MAX_URL_BYTES);
        assert!(normalize_address(&at_bound).is_ok());
    }
}

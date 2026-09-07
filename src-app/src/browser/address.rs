use paneflow_browser_protocol::{BrowserError, normalize_address};

pub(super) fn resolve(input: &str) -> Result<String, BrowserError> {
    match normalize_address(input) {
        Ok(url) => return Ok(url),
        Err(BrowserError::InvalidUrl) => {}
        Err(error) => return Err(error),
    }
    let query = input.trim();
    if query.is_empty() || query.chars().any(char::is_control) {
        return Err(BrowserError::InvalidUrl);
    }
    if query.split_once(':').is_some_and(|(scheme, _)| {
        scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    }) {
        return Err(BrowserError::InvalidUrl);
    }
    let mut target = String::from("https://www.google.com/search?q=");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            target.push(char::from(byte));
        } else {
            target.push('%');
            target.push(char::from(HEX[(byte >> 4) as usize]));
            target.push(char::from(HEX[(byte & 15) as usize]));
        }
    }
    normalize_address(&target)
}

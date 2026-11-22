use std::fmt::Write as _;

pub const SECRET_BYTES: usize = paneflow_api::methods::prompt::ANSWER_TOKEN_BYTES;
pub const SECRET_HEX_LEN: usize = SECRET_BYTES * 2;
pub const RANDOM_FAILED: &str = "the operating system's random generator failed";

pub fn fresh_secret() -> Result<String, String> {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| format!("{RANDOM_FAILED}: {error}"))?;
    let mut hex = String::with_capacity(SECRET_HEX_LEN);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

pub fn same_secret(known: &str, offered: &str) -> bool {
    let (known, offered) = (known.as_bytes(), offered.as_bytes());
    if known.len() != offered.len() {
        return false;
    }
    known
        .iter()
        .zip(offered)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_holds_128_random_bits_as_lowercase_hex_and_never_repeats() {
        let first = fresh_secret().unwrap();
        let second = fresh_secret().unwrap();
        assert_eq!(first.len(), SECRET_HEX_LEN);
        assert_eq!(SECRET_BYTES * 8, 128);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(first, second);
    }

    #[test]
    fn two_secrets_are_the_same_only_when_every_byte_matches() {
        assert!(same_secret("00ff", "00ff"));
        assert!(!same_secret("00ff", "00fe"));
        assert!(!same_secret("00ff", "00ff0"));
        assert!(!same_secret("00ff", ""));
    }
}

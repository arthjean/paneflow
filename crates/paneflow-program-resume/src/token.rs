use std::fmt;

pub const TOKEN_MIN_LEN: usize = 22;
pub const TOKEN_MAX_LEN: usize = 64;

#[derive(Clone)]
pub struct Token(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidToken;

impl Token {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidToken> {
        let value = value.into();
        let fits_length = (TOKEN_MIN_LEN..=TOKEN_MAX_LEN).contains(&value.len());
        let fits_alphabet = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if fits_length && fits_alphabet {
            Ok(Self(value))
        } else {
            Err(InvalidToken)
        }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

impl fmt::Display for InvalidToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "an OSC 7577 token is {TOKEN_MIN_LEN} to {TOKEN_MAX_LEN} characters of [A-Za-z0-9_-]"
        )
    }
}

impl std::error::Error for InvalidToken {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specification_example_token_is_valid() {
        assert!(Token::new("dummy-token-replace-me").is_ok());
    }

    #[test]
    fn tokens_outside_the_length_range_are_refused() {
        assert_eq!(Token::new("a".repeat(21)).err(), Some(InvalidToken));
        assert!(Token::new("a".repeat(64)).is_ok());
        assert_eq!(Token::new("a".repeat(65)).err(), Some(InvalidToken));
    }

    #[test]
    fn tokens_outside_the_alphabet_are_refused() {
        assert_eq!(
            Token::new("dummy-token-replace-me=").err(),
            Some(InvalidToken)
        );
        assert_eq!(
            Token::new("dummy token replace me").err(),
            Some(InvalidToken)
        );
    }

    #[test]
    fn debug_output_never_shows_the_token() {
        let token = Token::new("dummy-token-replace-me").expect("valid token");
        assert!(!format!("{token:?}").contains("dummy"));
    }
}

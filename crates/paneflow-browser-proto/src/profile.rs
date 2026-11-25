use std::fmt;

use serde::{Deserialize, Serialize};

use crate::limits::MAX_PROFILE_NAME_CHARS;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "a browser profile name is 1 to {MAX_PROFILE_NAME_CHARS} ASCII letters, digits, '-' or '_', not {0:?}"
)]
pub struct InvalidProfileName(pub String);

impl ProfileName {
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidProfileName> {
        let name = name.into();
        let valid = !name.is_empty()
            && name.len() <= MAX_PROFILE_NAME_CHARS
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if valid {
            Ok(Self(name))
        } else {
            Err(InvalidProfileName(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProfileName {
    type Error = InvalidProfileName;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

impl From<ProfileName> for String {
    fn from(name: ProfileName) -> Self {
        name.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_uuid_is_a_profile_name() {
        assert!(ProfileName::new("0b6d6713-5706-4af6-8061-06e40f757cb9").is_ok());
    }

    #[test]
    fn a_name_that_could_leave_the_profiles_directory_is_refused() {
        for name in ["", "..", "a/b", "a\\b", ".hidden", "x".repeat(65).as_str()] {
            assert!(ProfileName::new(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn deserializing_an_invalid_name_fails() {
        assert!(serde_json::from_str::<ProfileName>("\"../escape\"").is_err());
    }
}

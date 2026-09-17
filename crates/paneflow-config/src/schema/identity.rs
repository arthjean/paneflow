use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("{kind} must be a hyphenated lowercase UUID, got {value:?}")]
    Malformed { kind: &'static str, value: String },
}

macro_rules! uuid_identity {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub const KIND: &'static str = $kind;

            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().hyphenated().to_string())
            }

            pub fn parse(value: &str) -> Result<Self, IdentityError> {
                let parsed =
                    uuid::Uuid::parse_str(value).map_err(|_| IdentityError::Malformed {
                        kind: Self::KIND,
                        value: value.to_string(),
                    })?;
                let canonical = parsed.hyphenated().to_string();
                if canonical != value {
                    return Err(IdentityError::Malformed {
                        kind: Self::KIND,
                        value: value.to_string(),
                    });
                }
                Ok(Self(canonical))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdentityError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::parse(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

uuid_identity!(SessionId, "session id");
uuid_identity!(WorkspaceId, "workspace id");
uuid_identity!(HostInstanceToken, "host instance token");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionGeneration(u64);

impl SessionGeneration {
    pub const FIRST: Self = Self(1);

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl Default for SessionGeneration {
    fn default() -> Self {
        Self::FIRST
    }
}

impl fmt::Display for SessionGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_session_id_is_a_canonical_hyphenated_uuid() {
        let id = SessionId::new();
        assert_eq!(id.as_str().len(), 36);
        assert_eq!(SessionId::parse(id.as_str()), Ok(id.clone()));
        assert_ne!(id, SessionId::new(), "two sessions never share an id");
    }

    #[test]
    fn identities_reject_anything_but_the_canonical_form() {
        for raw in [
            "",
            "not-a-uuid",
            "550E8400-E29B-41D4-A716-446655440000",
            "550e8400e29b41d4a716446655440000",
            "{550e8400-e29b-41d4-a716-446655440000}",
            "12",
        ] {
            assert!(SessionId::parse(raw).is_err(), "{raw:?} must be rejected");
            assert!(WorkspaceId::parse(raw).is_err(), "{raw:?} must be rejected");
            assert!(
                serde_json::from_str::<SessionId>(&format!("{raw:?}")).is_err(),
                "{raw:?} must be rejected on the wire"
            );
        }
        let canonical = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            HostInstanceToken::parse(canonical).map(|t| t.to_string()),
            Ok(canonical.to_string())
        );
    }

    #[test]
    fn identities_serialize_as_plain_strings() {
        let id = WorkspaceId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"550e8400-e29b-41d4-a716-446655440000\""
        );
        let back: WorkspaceId =
            serde_json::from_str("\"550e8400-e29b-41d4-a716-446655440000\"").unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn a_generation_starts_at_one_and_only_grows() {
        assert_eq!(SessionGeneration::default(), SessionGeneration::FIRST);
        assert_eq!(SessionGeneration::FIRST.get(), 1);
        assert_eq!(SessionGeneration::FIRST.next().get(), 2);
        assert_eq!(
            serde_json::to_string(&SessionGeneration::FIRST).unwrap(),
            "1"
        );
    }
}

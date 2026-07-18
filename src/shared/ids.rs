//! UUIDv7 newtype ids. UUIDv7 embeds a millisecond timestamp, so ids sort
//! roughly by creation time — useful for cursor pagination and for reading
//! insertion order straight out of a `SELECT * ORDER BY id`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

macro_rules! define_uuid_id {
    ($name:ident) => {
        // Constructed by the owning context's domain/application layer,
        // which arrives in a later phase (see implementation-plan.md); not
        // yet used by Phase 0 code outside this module's own tests.
        #[allow(dead_code)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        #[allow(dead_code)]
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

define_uuid_id!(UserId);
define_uuid_id!(MemoryId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_display_and_from_str() {
        let id = UserId::new();
        let parsed: UserId = id.to_string().parse().expect("valid uuid");
        assert_eq!(id, parsed);
    }

    #[test]
    fn round_trips_through_serde_json() {
        let id = MemoryId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: MemoryId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn distinct_ids_are_not_equal() {
        assert_ne!(UserId::new(), UserId::new());
    }

    #[test]
    fn new_ids_are_v7() {
        assert_eq!(UserId::new().as_uuid().get_version_num(), 7);
    }
}

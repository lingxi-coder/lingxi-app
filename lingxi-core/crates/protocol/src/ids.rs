//! Newtype-wrapped identifiers used across the engine.
//!
//! Each ID is a UUID v4 internally but serializes as a plain string so the
//! protocol stays language-neutral across the `UniFFI` bridge.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_newtype {
    ($name:ident, $prefix:literal) => {
        #[doc = concat!("Identifier for a ", stringify!($name), ". UUID v4 internally; ")]
        #[doc = concat!("serialized with the `", $prefix, ":` prefix for log-grep-ability.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a fresh random ID.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Nil ID (all zeros) — for sentinel values, not for production.
            #[must_use]
            pub fn nil() -> Self {
                Self(Uuid::nil())
            }

            /// Construct from a raw UUID. Useful for tests and deserialization fallbacks.
            #[must_use]
            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Return the underlying UUID. Useful when interoperating with
            /// libraries that take `uuid::Uuid` directly.
            #[must_use]
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
                write!(f, "{}:{}", $prefix, self.0)
            }
        }
    };
}

id_newtype!(AgentId, "agent");
id_newtype!(SessionId, "sess");
id_newtype!(MessageId, "msg");
id_newtype!(ToolUseId, "tu");
id_newtype!(RequestId, "req");
id_newtype!(HookId, "hook");
id_newtype!(PluginId, "plg");
id_newtype!(McpConnectionId, "mcp");
id_newtype!(SnapshotId, "snap");
id_newtype!(PrefetchId, "pf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_new_is_unique() {
        let a = AgentId::new();
        let b = AgentId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn agent_id_roundtrip_json() {
        let a = AgentId::new();
        let s = serde_json::to_string(&a).unwrap();
        let b: AgentId = serde_json::from_str(&s).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn agent_id_display_prefixed() {
        let a = AgentId::from_uuid(Uuid::nil());
        assert_eq!(format!("{a}"), "agent:00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn session_id_distinct_type_from_agent_id() {
        // This test exists only to lock in type discipline; the assertion is trivial.
        let _: SessionId = SessionId::new();
        let _: AgentId = AgentId::new();
    }
}

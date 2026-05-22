//! Shared protocol types for `LingXi` Core engine.
//!
//! This crate owns the boundary types (`Effect`, `EffectResult`, `EffectError`,
//! IDs, message DTOs, transport DTOs, capability flags) consumed by both
//! `lingxi-core` and `lingxi-traits`. Both depend on this crate to avoid a
//! cyclic dependency.
//!
//! See spec §3 (D16 Shared protocol boundary).

#![forbid(unsafe_code)]

pub mod capabilities;
pub mod effect_result;
pub mod effects;
pub mod ids;
pub mod messages;
pub mod transport;

// Re-exports for ergonomics.
// NOTE: re-exports for other modules are added by each module's task (4-7) once
// the underlying types exist.
pub use capabilities::{FileSystemCapabilities, PlatformCapabilities};
pub use effect_result::{EffectError, EffectErrorKind, EffectResult};
pub use effects::Effect;
pub use ids::{
    AgentId, HookId, McpConnectionId, MessageId, PluginId, PrefetchId, RequestId, SessionId,
    SnapshotId, ToolUseId,
};
pub use messages::{ContentBlock, ConversationMessage, MessageRole};
pub use transport::{HttpMethod, HttpRequest, HttpResponse, SseEvent};

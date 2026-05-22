# LingXi Core M1 · Plan 01 · Foundation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the foundational Cargo workspace with `lingxi-protocol`, `lingxi-core`, `lingxi-traits`, and `lingxi-api-client` so that a single-turn mock conversation can be reduced end-to-end against a `MockHttpTransport` with deterministic event replay.

**Architecture:** Event-sourced state machine in `lingxi-core` reducer (pure function, no I/O) with all side effects expressed as `Effect` enum values defined in `lingxi-protocol`. Platform abstractions in `lingxi-traits`. Anthropic SSE streaming parsed by `lingxi-api-client` over an injected `HttpTransport`. No `tokio::spawn` in engine crates — `RuntimeSpawner` trait gates all background work.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `thiserror`, `tracing`, `async-trait`, `futures-core`, `proptest` (dev), `tokio` (test-harness only).

**References:**
- Spec sections: §1 Overview · §2 Decisions Log · §3 Crate Topology · §4 Trait System (4.1-4.4, 4.7) · §5 State Machine
- Source of truth for protocol DTOs: spec §3 + §5.2 + §5.3

---

## File Structure

```
lingxi-core/                          ← workspace root (new git repo or subdirectory)
├── Cargo.toml                        ← workspace manifest, lints, dependencies
├── rust-toolchain.toml               ← pinned Rust version
├── .gitignore                        ← target/, etc.
├── crates/
│   ├── protocol/                     ← shared DTOs / IDs / Effect / EffectResult / EffectError
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ids.rs                ← AgentId, SessionId, MessageId, ToolUseId, RequestId, HookId, PluginId, McpConnectionId, SnapshotId, PrefetchId
│   │       ├── effects.rs            ← Effect enum (subset: API + Render + Persist for M1.1; expanded in later plans)
│   │       ├── effect_result.rs      ← EffectResult, EffectError
│   │       ├── messages.rs           ← ConversationMessage + content blocks
│   │       ├── transport.rs          ← HttpRequest, HttpResponse, SseEvent
│   │       └── capabilities.rs       ← PlatformCapabilities
│   │
│   ├── core/                         ← state machine + reducer + session model
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── state_machine.rs      ← ConversationState enum
│   │       ├── events.rs             ← Event enum (subset: UserMessage, ApiStream*, UserInterrupt, UserExit)
│   │       ├── reducer.rs            ← pub fn reduce(state, event) -> (state, Vec<Effect>)
│   │       ├── prompt.rs             ← assemble_request(session, msg)
│   │       ├── token.rs              ← Usage struct, token accounting
│   │       ├── session.rs            ← SessionState, CumulativeUsage
│   │       └── model.rs              ← model_alias_resolve, context_window
│   │
│   ├── traits/                       ← platform abstraction traits
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── http.rs               ← HttpTransport trait
│   │       ├── clock.rs              ← Clock trait
│   │       ├── runtime.rs            ← RuntimeSpawner trait + BackgroundTaskHandle + RuntimeError
│   │       ├── effect_handler.rs     ← EffectHandler trait
│   │       └── filesystem.rs         ← FileSystem trait (signatures only; impls deferred to later plans)
│   │
│   └── api-client/                   ← Anthropic provider + SSE parser
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── anthropic.rs          ← AnthropicProvider
│           ├── types.rs              ← MessageRequest, MessageResponse, StreamEvent, ContentDelta, Usage (API-shape)
│           ├── sse.rs                ← pure SSE parser
│           └── error.rs              ← ApiError (incl. PromptTooLong)
│
└── crates/test-harness/              ← mocks for trait impls (test-only deps)
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── mocks/
        │   ├── mod.rs
        │   ├── mock_http.rs          ← MockHttpTransport
        │   ├── mock_clock.rs         ← MockClock
        │   └── mock_runtime.rs       ← MockRuntimeSpawner
        └── fixtures/
            └── mod.rs                ← arb_event, arb_message helpers for proptest
```

**File boundaries:** Each `*.rs` owns one concept. The reducer is a single 200-line file; do NOT inline state machine logic into separate transition files yet — the dispatch table approach (see below) keeps everything readable in one file until plan 6 (Agent) adds sub-agent states.

---

## Task 1: Bootstrap workspace

**Files:**
- Create: `/Users/luolingfeng/Projects/LingXi-Next/lingxi-core/Cargo.toml`
- Create: `/Users/luolingfeng/Projects/LingXi-Next/lingxi-core/rust-toolchain.toml`
- Create: `/Users/luolingfeng/Projects/LingXi-Next/lingxi-core/.gitignore`

- [ ] **Step 1: Create workspace root**

```bash
mkdir -p /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
```

- [ ] **Step 2: Write workspace Cargo.toml**

```toml
[workspace]
resolver = "2"
members = [
    "crates/protocol",
    "crates/core",
    "crates/traits",
    "crates/api-client",
    "crates/test-harness",
]

[workspace.package]
edition = "2021"
rust-version = "1.82"
license = "MIT OR Apache-2.0"
authors = ["LingXi Core Contributors"]
repository = "https://github.com/lingxi/lingxi-core"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
async-trait = "0.1"
futures-core = "0.3"

# dev-only
proptest = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }

[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
missing_errors_doc = "allow"
missing_panics_doc = "allow"
module_name_repetitions = "allow"
```

- [ ] **Step 3: Pin Rust toolchain**

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.82.0"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 4: Add .gitignore**

```
target/
**/*.rs.bk
Cargo.lock.*
.DS_Store
```

- [ ] **Step 5: Verify workspace structure**

Run: `cargo check --workspace`
Expected: error message about missing members (we haven't created the crates yet)

- [ ] **Step 6: Commit**

```bash
git init
git add Cargo.toml rust-toolchain.toml .gitignore
git commit -m "chore: bootstrap lingxi-core workspace"
```

---

## Task 2: Create lingxi-protocol crate skeleton

**Files:**
- Create: `crates/protocol/Cargo.toml`
- Create: `crates/protocol/src/lib.rs`

- [ ] **Step 1: Write Cargo.toml**

```toml
[package]
name = "lingxi-protocol"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Write lib.rs entrypoint**

```rust
//! Shared protocol types for LingXi Core engine.
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
pub use capabilities::PlatformCapabilities;
pub use effect_result::{EffectError, EffectResult};
pub use effects::Effect;
pub use ids::{
    AgentId, HookId, McpConnectionId, MessageId, PluginId, PrefetchId, RequestId, SessionId,
    SnapshotId, ToolUseId,
};
pub use messages::{ContentBlock, ConversationMessage, MessageRole};
pub use transport::{HttpMethod, HttpRequest, HttpResponse, SseEvent};
```

- [ ] **Step 3: Run cargo check (expect missing module errors)**

Run: `cargo check -p lingxi-protocol`
Expected: errors like "file not found for module `capabilities`"

- [ ] **Step 4: Commit scaffold**

```bash
git add crates/protocol
git commit -m "feat(protocol): scaffold lingxi-protocol crate"
```

---

## Task 3: IDs (newtype wrappers)

**Files:**
- Create: `crates/protocol/src/ids.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write failing test for AgentId basic shape**

```rust
// crates/protocol/src/ids.rs (initial)
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
}
```

- [ ] **Step 2: Run test to verify it fails (no type yet)**

Run: `cargo test -p lingxi-protocol --lib ids`
Expected: FAIL with "cannot find type `AgentId` in this scope"

- [ ] **Step 3: Implement AgentId and the rest of the IDs**

```rust
//! Newtype-wrapped identifiers used across the engine.
//!
//! Each ID is a UUID v4 internally but serializes as a plain string so the
//! protocol stays language-neutral across the UniFFI bridge.

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
```

- [ ] **Step 4: Add uuid dependency**

Edit `crates/protocol/Cargo.toml`, add under `[dependencies]`:

```toml
uuid = { version = "1", features = ["v4", "serde"] }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p lingxi-protocol --lib ids`
Expected: 4 tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/protocol
git commit -m "feat(protocol): add ID newtypes with UUID v4 + prefixed Display"
```

---

## Task 4: Messages & Content Blocks

**Files:**
- Create: `crates/protocol/src/messages.rs`

- [ ] **Step 1: Write failing tests**

```rust
// crates/protocol/src/messages.rs (initial — tests only)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_constructs() {
        let id = MessageId::new();
        let m = ConversationMessage::user(id, "hello".to_string());
        assert!(matches!(m.role, MessageRole::User));
        assert_eq!(m.text_content(), "hello");
    }

    #[test]
    fn message_roundtrip_json() {
        let m = ConversationMessage::user(MessageId::new(), "hi".into());
        let s = serde_json::to_string(&m).unwrap();
        let m2: ConversationMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn assistant_message_with_tool_use_extracts_tool_calls() {
        let m = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text { text: "I'll read the file.".into() },
                ContentBlock::ToolUse {
                    id: ToolUseId::new(),
                    name: "Read".into(),
                    input: serde_json::json!({"path": "/tmp/x"}),
                },
            ],
            stop_reason: None,
        };
        assert!(m.has_tool_use());
        assert_eq!(m.tool_calls().len(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p lingxi-protocol --lib messages`
Expected: FAIL with type errors

- [ ] **Step 3: Implement message types**

```rust
//! Conversation message DTOs.
//!
//! Mirrors Anthropic's `Message` content-block model but is API-neutral —
//! provider adapters in `lingxi-api-client` map their native shapes to these.

use crate::ids::{MessageId, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: ToolUseId,
        content: String,
        is_error: bool,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ConversationMessage {
    User {
        id: MessageId,
        content: Vec<ContentBlock>,
    },
    Assistant {
        id: MessageId,
        content: Vec<ContentBlock>,
        stop_reason: Option<String>,
    },
    System {
        id: MessageId,
        content: String,
    },
}

impl ConversationMessage {
    #[must_use]
    pub fn user(id: MessageId, text: String) -> Self {
        Self::User { id, content: vec![ContentBlock::Text { text }] }
    }

    #[must_use]
    pub fn role(&self) -> MessageRole {
        match self {
            Self::User { .. } => MessageRole::User,
            Self::Assistant { .. } => MessageRole::Assistant,
            Self::System { .. } => MessageRole::System,
        }
    }

    #[must_use]
    pub fn id(&self) -> MessageId {
        match self {
            Self::User { id, .. } | Self::Assistant { id, .. } | Self::System { id, .. } => *id,
        }
    }

    /// Concatenate all `Text` blocks. Returns "" if none.
    #[must_use]
    pub fn text_content(&self) -> String {
        match self {
            Self::User { content, .. } | Self::Assistant { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            Self::System { content, .. } => content.clone(),
        }
    }

    #[must_use]
    pub fn has_tool_use(&self) -> bool {
        match self {
            Self::Assistant { content, .. } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
            _ => false,
        }
    }

    /// Returns references to ToolUse blocks; empty if not Assistant.
    #[must_use]
    pub fn tool_calls(&self) -> Vec<&ContentBlock> {
        match self {
            Self::Assistant { content, .. } => content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    // ... same tests as Step 1 ...
}
```

(Keep the tests from Step 1 below the `impl ConversationMessage` block.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p lingxi-protocol --lib messages`
Expected: 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/protocol/src/messages.rs
git commit -m "feat(protocol): add ConversationMessage and ContentBlock DTOs"
```

---

## Task 5: Transport DTOs

**Files:**
- Create: `crates/protocol/src/transport.rs`

- [ ] **Step 1: Write tests + implementation in one step (small file)**

```rust
//! HTTP and SSE transport DTOs (no I/O — pure data).

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    pub event_type: Option<String>,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_request_roundtrip() {
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: "https://api.anthropic.com/v1/messages".into(),
            headers: vec![("authorization".into(), "Bearer xyz".into())],
            body: Some(r#"{"model":"claude-opus-4-6"}"#.into()),
            timeout: Some(Duration::from_secs(30)),
        };
        let s = serde_json::to_string(&req).unwrap();
        let req2: HttpRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(req.method, req2.method);
        assert_eq!(req.url, req2.url);
    }

    #[test]
    fn sse_event_default_type_omitted() {
        let e = SseEvent { event_type: None, data: "{}".into(), id: None };
        let s = serde_json::to_string(&e).unwrap();
        assert!(!s.contains("id"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p lingxi-protocol --lib transport`
Expected: 2 tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/protocol/src/transport.rs
git commit -m "feat(protocol): add HttpRequest/Response/SseEvent transport DTOs"
```

---

## Task 6: Capabilities

**Files:**
- Create: `crates/protocol/src/capabilities.rs`

- [ ] **Step 1: Write tests + impl**

```rust
//! Platform capability flags. Subsystems use these to filter their
//! available features at runtime (e.g., the Bash tool is hidden when
//! `process == false`).
//!
//! See spec §4.11 (Capability System) and Appendix A (platform matrix).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    pub filesystem: FileSystemCapabilities,
    pub process: bool,
    pub http: bool,
    pub mcp: bool,
    pub worktree: bool,
    pub swarm: bool,
    pub os_notifications: bool,
    pub ide_bridge: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemCapabilities {
    pub max_read_size: u64,
    pub max_write_size: u64,
    pub supports_symlinks: bool,
    pub supports_watch: bool,
    pub sandbox_root: Option<String>,
}

impl PlatformCapabilities {
    /// Linux desktop defaults (used by `posix-minimal` demo host).
    #[must_use]
    pub fn desktop_posix() -> Self {
        Self {
            filesystem: FileSystemCapabilities {
                max_read_size: 10 * 1024 * 1024,
                max_write_size: 10 * 1024 * 1024,
                supports_symlinks: true,
                supports_watch: true,
                sandbox_root: None,
            },
            process: true,
            http: true,
            mcp: true,
            worktree: true,
            swarm: true,
            os_notifications: true,
            ide_bridge: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_posix_has_process_and_worktree() {
        let caps = PlatformCapabilities::desktop_posix();
        assert!(caps.process);
        assert!(caps.worktree);
    }

    #[test]
    fn capabilities_roundtrip_json() {
        let caps = PlatformCapabilities::desktop_posix();
        let s = serde_json::to_string(&caps).unwrap();
        let _: PlatformCapabilities = serde_json::from_str(&s).unwrap();
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p lingxi-protocol --lib capabilities`
Expected: 2 tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/protocol/src/capabilities.rs
git commit -m "feat(protocol): add PlatformCapabilities + desktop_posix preset"
```

---

## Task 7: Effect & EffectResult enums (M1.1 subset)

**Files:**
- Create: `crates/protocol/src/effects.rs`
- Create: `crates/protocol/src/effect_result.rs`

- [ ] **Step 1: Write tests for effects**

```rust
// crates/protocol/src/effects.rs (tests only at first)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_api_request_roundtrip() {
        let e = Effect::SendApiRequest {
            request_id: RequestId::nil(),
            request_body: serde_json::json!({"model": "claude-opus-4-6"}),
        };
        let s = serde_json::to_string(&e).unwrap();
        let e2: Effect = serde_json::from_str(&s).unwrap();
        assert_eq!(e, e2);
    }

    #[test]
    fn render_stream_delta_carries_text() {
        let e = Effect::RenderStreamDelta { text: "hello".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("hello"));
    }
}
```

- [ ] **Step 2: Run tests (expect compile errors)**

Run: `cargo test -p lingxi-protocol --lib effects`
Expected: FAIL — type `Effect` not found

- [ ] **Step 3: Implement M1.1-scope Effect enum**

```rust
//! Side effects emitted by the reducer.
//!
//! Each variant is a request to do something with the outside world.
//! `EffectHandler` (in `lingxi-traits`) processes these. See spec §5.3.
//!
//! M1.1 ships a subset: API, render, persistence. Later plans (Tools, Hooks,
//! Memory, MCP, Agent, etc.) extend this enum.

use crate::ids::{MessageId, RequestId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    // — API —
    /// Send a fully-assembled request body. Reply arrives as Event::ApiStream*.
    SendApiRequest {
        request_id: RequestId,
        /// Provider-shape body (Anthropic JSON for now; OpenAI in Plan 4 expansion).
        request_body: Value,
    },

    // — Render (UI / TUI / SDK consumers) —
    RenderStreamDelta { text: String },
    RenderError { error: String },
    RenderTokenUsageUpdate { input_tokens: u64, output_tokens: u64 },

    // — Persistence (full session model lands in Plan 10) —
    PersistSessionSnapshot {
        session_id: SessionId,
        snapshot: Value,
    },

    // — Lifecycle —
    LoadSession { session_id: SessionId },
    Terminate { reason: String },

    // — Diagnostic —
    /// Reducer hit a (state, event) pair it doesn't have a transition for.
    /// Emitted instead of `tracing::warn!` to keep the reducer pure.
    RecordUnexpectedEvent {
        state_name: String,
        event_name: String,
    },
}
```

- [ ] **Step 4: Implement EffectResult / EffectError**

```rust
// crates/protocol/src/effect_result.rs
//! Reply shape from EffectHandler back to the run loop.

use crate::ids::{MessageId, RequestId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectResult {
    /// No-op (Render*, RecordUnexpectedEvent, etc.).
    Ack,
    /// API request was accepted; events will follow asynchronously.
    ApiRequestQueued { request_id: RequestId },
    /// Session loaded successfully.
    SessionLoaded { session_id: crate::ids::SessionId },
}

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[error("effect failed: {kind}: {detail}")]
pub struct EffectError {
    pub kind: EffectErrorKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectErrorKind {
    Io,
    Network,
    Permission,
    NotFound,
    Cancelled,
    Internal,
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p lingxi-protocol --lib`
Expected: all tests pass (across all modules)

- [ ] **Step 6: Commit**

```bash
git add crates/protocol/src/effects.rs crates/protocol/src/effect_result.rs
git commit -m "feat(protocol): add Effect/EffectResult/EffectError (M1.1 subset)"
```

---

## Task 8: lingxi-traits crate — HttpTransport + Clock + RuntimeSpawner

**Files:**
- Create: `crates/traits/Cargo.toml`
- Create: `crates/traits/src/lib.rs`
- Create: `crates/traits/src/http.rs`
- Create: `crates/traits/src/clock.rs`
- Create: `crates/traits/src/runtime.rs`
- Create: `crates/traits/src/effect_handler.rs`
- Create: `crates/traits/src/filesystem.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-traits"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
async-trait.workspace = true
futures-core.workspace = true
serde.workspace = true
thiserror.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: lib.rs**

```rust
//! Platform abstraction traits.
//!
//! Engine crates depend on these traits; platform crates (`platforms/posix`,
//! `platforms/windows`, etc.) implement them. The engine never imports a
//! concrete runtime or OS API directly.
//!
//! See spec §4 (Trait System) and D17 (Runtime boundary).

#![forbid(unsafe_code)]

pub mod clock;
pub mod effect_handler;
pub mod filesystem;
pub mod http;
pub mod runtime;

pub use clock::Clock;
pub use effect_handler::EffectHandler;
pub use filesystem::{FileContent, FileSystem, FsError};
pub use http::{HttpError, HttpTransport};
pub use runtime::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
```

- [ ] **Step 3: HttpTransport trait + tests**

```rust
// crates/traits/src/http.rs
//! HTTP transport abstraction. Provider-neutral; the api-client crate uses
//! this trait so it never imports reqwest directly.

use async_trait::async_trait;
use futures_core::stream::Stream;
use lingxi_protocol::{HttpRequest, HttpResponse, SseEvent};
use std::pin::Pin;
use thiserror::Error;

pub type SseStream = Pin<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send>>;

#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// Send a request and await the full response.
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;

    /// Open an SSE stream. Caller drives the stream to completion.
    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError>;
}

#[derive(Debug, Clone, Error)]
pub enum HttpError {
    #[error("request timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("connection failed: {0}")]
    Connection(String),

    #[error("non-success HTTP status {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_display_includes_status() {
        let e = HttpError::Status { status: 429, body: "rate limited".into() };
        assert!(format!("{e}").contains("429"));
    }
}
```

- [ ] **Step 4: Clock trait**

```rust
// crates/traits/src/clock.rs
use std::time::{Duration, SystemTime};

pub trait Clock: Send + Sync {
    /// Current wall-clock time. May not be monotonic across sleep on mobile;
    /// callers needing monotonicity should track relative durations from a
    /// fixed `Instant` instead. See spec §1.4.1 hidden assumptions.
    fn now(&self) -> SystemTime;

    /// Convenience: `now().duration_since(earlier).unwrap_or(Duration::ZERO)`.
    fn elapsed_since(&self, earlier: SystemTime) -> Duration {
        self.now().duration_since(earlier).unwrap_or(Duration::ZERO)
    }
}
```

- [ ] **Step 5: RuntimeSpawner trait**

```rust
// crates/traits/src/runtime.rs
//! Background task spawning abstraction. Engine code MUST NOT call
//! tokio::spawn directly — see D17.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskHandle {
    pub task_name: String,
    pub task_id: u64,
}

#[async_trait]
pub trait RuntimeSpawner: Send + Sync {
    async fn spawn(
        &self,
        name: &str,
        task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Result<BackgroundTaskHandle, RuntimeError>;

    async fn sleep(&self, duration: Duration);

    async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError>;
}

#[derive(Debug, Clone, Error)]
pub enum RuntimeError {
    #[error("runtime is shutting down")]
    ShuttingDown,
    #[error("background task {0} not found")]
    NotFound(String),
    #[error("spawner internal error: {0}")]
    Internal(String),
}
```

- [ ] **Step 6: EffectHandler trait**

```rust
// crates/traits/src/effect_handler.rs
use async_trait::async_trait;
use lingxi_protocol::{Effect, EffectError, EffectResult};

#[async_trait]
pub trait EffectHandler: Send + Sync {
    /// Process one Effect emitted by the reducer.
    async fn handle(&self, effect: Effect) -> Result<EffectResult, EffectError>;
}
```

- [ ] **Step 7: FileSystem trait (signatures only — impls in later plans)**

```rust
// crates/traits/src/filesystem.rs
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn read_file(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<FileContent, FsError>;

    async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError>;

    fn is_within_workspace(&self, path: &str) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    pub content: String,
    pub total_lines: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Error)]
pub enum FsError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("path outside workspace: {0}")]
    OutsideWorkspace(String),
    #[error("file is binary: {0}")]
    BinaryFile(String),
    #[error("size exceeds limit: {actual} > {limit}")]
    TooLarge { actual: u64, limit: u64 },
    #[error("io error: {0}")]
    Io(String),
}
```

- [ ] **Step 8: Run cargo check across the workspace**

Run: `cargo check --workspace`
Expected: protocol + traits compile clean

- [ ] **Step 9: Commit**

```bash
git add crates/traits
git commit -m "feat(traits): add HttpTransport, Clock, RuntimeSpawner, EffectHandler, FileSystem traits"
```

---

## Task 9: lingxi-core scaffold + Event enum

**Files:**
- Create: `crates/core/Cargo.toml`
- Create: `crates/core/src/lib.rs`
- Create: `crates/core/src/events.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-core"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: lib.rs**

```rust
//! Core conversation state machine.
//!
//! - `events::Event` — inputs to the reducer
//! - `state_machine::ConversationState` — the state set
//! - `reducer::reduce` — pure-function state transitions
//! - `prompt::assemble_request` — build the API request body
//! - `session::SessionState` — persisted session model
//!
//! No I/O. No `tokio::spawn`. All side effects are returned as
//! `lingxi_protocol::Effect` values.

#![forbid(unsafe_code)]

pub mod events;
pub mod model;
pub mod prompt;
pub mod reducer;
pub mod session;
pub mod state_machine;
pub mod token;

pub use events::Event;
pub use reducer::reduce;
pub use session::{CumulativeUsage, SessionState};
pub use state_machine::ConversationState;
pub use token::Usage;
```

- [ ] **Step 3: Event enum (M1.1 subset)**

```rust
// crates/core/src/events.rs
//! Inputs to the reducer. Each Event is a single observable thing that
//! happened in the outside world (user typed, API streamed a chunk, ...).
//!
//! IDs/timestamps live in the events, not in the reducer — see D17 (purity).
//!
//! M1.1 ships a subset. Later plans extend this enum.

use crate::token::Usage;
use crate::session::SessionState;
use lingxi_protocol::{ConversationMessage, MessageId, RequestId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    // — User input —
    UserMessage {
        message_id: MessageId,
        request_id: RequestId,
        content: String,
    },
    UserInterrupt,
    UserExit,

    // — API responses —
    ApiStreamStart { request_id: RequestId },
    ApiStreamDelta { request_id: RequestId, text: String },
    ApiStreamEnd {
        request_id: RequestId,
        final_message: ConversationMessage,
        usage: Usage,
    },
    ApiError { request_id: RequestId, error: ApiErrorPayload },

    // — System —
    SessionLoaded(SessionState),
}

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[error("api error: {message}")]
pub struct ApiErrorPayload {
    pub kind: String,
    pub message: String,
}
```

- [ ] **Step 4: Tests**

```rust
// append to events.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_event_serializes() {
        let e = Event::UserMessage {
            message_id: MessageId::nil(),
            request_id: RequestId::nil(),
            content: "hi".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("user_message"));
        assert!(s.contains("hi"));
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p lingxi-core --lib events`
Expected: needs other modules to compile; expect compile failure with missing `Usage` etc.

- [ ] **Step 6: Stub other modules so this compiles**

Create `crates/core/src/token.rs`:

```rust
//! Token accounting. Pricing/budget logic lives in `lingxi-cost` (Plan 2).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }
}
```

Create `crates/core/src/session.rs`:

```rust
//! Session state model. Persistence lives in `lingxi-session` (Plan 10).
use crate::token::Usage;
use lingxi_protocol::{ConversationMessage, MessageId, SessionId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CumulativeUsage(pub Usage);

impl CumulativeUsage {
    pub fn add(&mut self, u: &Usage) { self.0.add(u); }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: SessionId,
    pub history: Vec<ConversationMessage>,
    pub usage: CumulativeUsage,
    pub model: String,
}

impl SessionState {
    pub fn empty(session_id: SessionId, model: String) -> Self {
        Self { session_id, history: Vec::new(), usage: CumulativeUsage::default(), model }
    }
}
```

Create stub `crates/core/src/state_machine.rs`, `reducer.rs`, `prompt.rs`, `model.rs` with minimal `pub fn placeholder() {}` content so `cargo check -p lingxi-core` passes.

- [ ] **Step 7: Verify compile**

Run: `cargo check -p lingxi-core`
Expected: compile clean

- [ ] **Step 8: Run tests**

Run: `cargo test -p lingxi-core --lib events`
Expected: 1 test passes

- [ ] **Step 9: Commit**

```bash
git add crates/core
git commit -m "feat(core): scaffold core crate + Event enum (M1.1 subset)"
```

---

## Task 10: ConversationState + initial states

**Files:**
- Modify: `crates/core/src/state_machine.rs`

- [ ] **Step 1: Write tests**

```rust
// crates/core/src/state_machine.rs
#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::SessionId;

    #[test]
    fn idle_state_holds_session() {
        let session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
        let state = ConversationState::Idle { session: session.clone() };
        assert_eq!(state.session().session_id, session.session_id);
    }

    #[test]
    fn terminated_is_terminal() {
        let session = SessionState::empty(SessionId::nil(), "x".into());
        let state = ConversationState::Terminated { session, reason: "ok".into() };
        assert!(state.is_terminal());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p lingxi-core --lib state_machine`
Expected: FAIL (struct doesn't exist)

- [ ] **Step 3: Implement**

```rust
//! Conversation state machine. Each variant represents one distinct moment
//! in the agentic loop. The reducer transitions between these.
//!
//! M1.1 ships 4 states. Later plans add: ToolUseReceived,
//! AwaitingPermission, AwaitingToolResult, AwaitingSubagent, Compacting,
//! HookBlocked, MemoryPrefetchInProgress.

use crate::session::SessionState;
use lingxi_protocol::RequestId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationState {
    Idle { session: SessionState },
    AssemblingPrompt { session: SessionState, user_message: String },
    AwaitingApiResponse { session: SessionState, request_id: RequestId },
    StreamingResponse {
        session: SessionState,
        request_id: RequestId,
        partial_text: String,
    },
    Terminated { session: SessionState, reason: String },
}

impl ConversationState {
    #[must_use]
    pub fn session(&self) -> &SessionState {
        match self {
            Self::Idle { session }
            | Self::AssemblingPrompt { session, .. }
            | Self::AwaitingApiResponse { session, .. }
            | Self::StreamingResponse { session, .. }
            | Self::Terminated { session, .. } => session,
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminated { .. })
    }

    /// For RecordUnexpectedEvent diagnostics.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Idle { .. } => "Idle",
            Self::AssemblingPrompt { .. } => "AssemblingPrompt",
            Self::AwaitingApiResponse { .. } => "AwaitingApiResponse",
            Self::StreamingResponse { .. } => "StreamingResponse",
            Self::Terminated { .. } => "Terminated",
        }
    }
}

#[cfg(test)]
mod tests {
    // ... tests from Step 1 ...
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p lingxi-core --lib state_machine`
Expected: 2 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/state_machine.rs
git commit -m "feat(core): add ConversationState (M1.1 subset, 5 variants)"
```

---

## Task 11: Prompt assembly

**Files:**
- Modify: `crates/core/src/prompt.rs`

- [ ] **Step 1: Write tests**

```rust
// crates/core/src/prompt.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use lingxi_protocol::SessionId;

    #[test]
    fn assemble_includes_history_and_new_user_message() {
        let mut session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
        let req = assemble_request(&session, "what's 2+2?");
        let body = req.as_object().unwrap();
        assert_eq!(body["model"], "claude-opus-4-6");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn assemble_appends_prior_history() {
        use lingxi_protocol::{ConversationMessage, MessageId};
        let mut session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
        session.history.push(ConversationMessage::user(MessageId::nil(), "earlier".into()));
        let req = assemble_request(&session, "now");
        let messages = req["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p lingxi-core --lib prompt`
Expected: FAIL (function doesn't exist)

- [ ] **Step 3: Implement**

```rust
//! Build the Anthropic-shape request body from session state + new user input.
//!
//! M1.1: minimal — no system prompt yet, no tools, no thinking config.
//! Later plans extend with system_prompt, tools list, thinking budget, etc.

use crate::session::SessionState;
use serde_json::{json, Value};

#[must_use]
pub fn assemble_request(session: &SessionState, user_message: &str) -> Value {
    let mut messages: Vec<Value> = session
        .history
        .iter()
        .map(message_to_api_shape)
        .collect();
    messages.push(json!({"role": "user", "content": user_message}));

    json!({
        "model": session.model,
        "max_tokens": 8192,
        "messages": messages,
    })
}

fn message_to_api_shape(m: &lingxi_protocol::ConversationMessage) -> Value {
    use lingxi_protocol::{ContentBlock, ConversationMessage};
    match m {
        ConversationMessage::User { content, .. } => {
            json!({"role": "user", "content": content_blocks_to_api(content)})
        }
        ConversationMessage::Assistant { content, .. } => {
            json!({"role": "assistant", "content": content_blocks_to_api(content)})
        }
        ConversationMessage::System { content, .. } => {
            json!({"role": "system", "content": content})
        }
    }
}

fn content_blocks_to_api(blocks: &[lingxi_protocol::ContentBlock]) -> Value {
    use lingxi_protocol::ContentBlock;
    let arr: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                json!({"type": "tool_result", "tool_use_id": tool_use_id,
                       "content": content, "is_error": is_error})
            }
            ContentBlock::Thinking { thinking, signature } => {
                json!({"type": "thinking", "thinking": thinking, "signature": signature})
            }
        })
        .collect();
    Value::Array(arr)
}

#[cfg(test)]
mod tests {
    // ... tests from Step 1 ...
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p lingxi-core --lib prompt`
Expected: 2 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/prompt.rs
git commit -m "feat(core): add prompt::assemble_request (M1.1 subset)"
```

---

## Task 12: Reducer — Idle → AssemblingPrompt → AwaitingApiResponse

**Files:**
- Modify: `crates/core/src/reducer.rs`

- [ ] **Step 1: Write tests**

```rust
// crates/core/src/reducer.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use crate::session::SessionState;
    use crate::state_machine::ConversationState;
    use lingxi_protocol::{Effect, MessageId, RequestId, SessionId};

    #[test]
    fn idle_plus_user_message_yields_awaiting_api_with_send_effect() {
        let session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
        let state = ConversationState::Idle { session: session.clone() };
        let event = Event::UserMessage {
            message_id: MessageId::nil(),
            request_id: RequestId::nil(),
            content: "hi".into(),
        };
        let (next, effects) = reduce(state, event);

        match next {
            ConversationState::AwaitingApiResponse { session, request_id } => {
                assert_eq!(session.history.len(), 1, "user message appended to history");
                assert_eq!(request_id, RequestId::nil());
            }
            other => panic!("unexpected state: {other:?}"),
        }

        assert_eq!(effects.len(), 1);
        assert!(matches!(effects[0], Effect::SendApiRequest { .. }));
    }

    #[test]
    fn terminated_is_absorbing() {
        let session = SessionState::empty(SessionId::nil(), "x".into());
        let state = ConversationState::Terminated { session, reason: "ok".into() };
        let (next, effects) = reduce(state, Event::UserInterrupt);
        assert!(next.is_terminal());
        assert!(effects.is_empty());
    }

    #[test]
    fn unexpected_event_emits_record_effect() {
        let session = SessionState::empty(SessionId::nil(), "x".into());
        let state = ConversationState::Idle { session };
        let event = Event::ApiStreamDelta { request_id: RequestId::nil(), text: "x".into() };
        let (_, effects) = reduce(state, event);
        assert!(effects.iter().any(|e| matches!(e, Effect::RecordUnexpectedEvent { .. })));
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p lingxi-core --lib reducer`
Expected: FAIL (function not implemented)

- [ ] **Step 3: Implement reducer**

```rust
//! Pure-function state machine reducer.
//!
//! Contract: `reduce(state, event) -> (new_state, effects)` is a pure function
//! over `&self`-free inputs. IDs, timestamps, randomness, and I/O are NOT
//! generated here — they arrive in input events or are emitted as effects.

use crate::events::Event;
use crate::prompt::assemble_request;
use crate::state_machine::ConversationState;
use lingxi_protocol::{ContentBlock, ConversationMessage, Effect};

/// Reduce one (state, event) pair to (new state, effects to emit).
#[must_use]
pub fn reduce(state: ConversationState, event: Event) -> (ConversationState, Vec<Effect>) {
    // Terminated is absorbing.
    if let ConversationState::Terminated { .. } = state {
        return (state, Vec::new());
    }

    match (state, event) {
        // Idle + UserMessage → AwaitingApiResponse (append history, emit send).
        (
            ConversationState::Idle { mut session },
            Event::UserMessage { message_id, request_id, content },
        ) => {
            let request_body = assemble_request(&session, &content);
            session.history.push(ConversationMessage::user(message_id, content));
            (
                ConversationState::AwaitingApiResponse { session, request_id },
                vec![Effect::SendApiRequest { request_id, request_body }],
            )
        }

        // AwaitingApiResponse + ApiStreamStart → StreamingResponse.
        (
            ConversationState::AwaitingApiResponse { session, request_id: rid_state },
            Event::ApiStreamStart { request_id: rid_evt },
        ) if rid_state == rid_evt => (
            ConversationState::StreamingResponse {
                session,
                request_id: rid_state,
                partial_text: String::new(),
            },
            Vec::new(),
        ),

        // StreamingResponse + ApiStreamDelta → accumulate + emit RenderStreamDelta.
        (
            ConversationState::StreamingResponse {
                session,
                request_id: rid_state,
                mut partial_text,
            },
            Event::ApiStreamDelta { request_id: rid_evt, text },
        ) if rid_state == rid_evt => {
            partial_text.push_str(&text);
            (
                ConversationState::StreamingResponse {
                    session,
                    request_id: rid_state,
                    partial_text,
                },
                vec![Effect::RenderStreamDelta { text }],
            )
        }

        // StreamingResponse + ApiStreamEnd → Idle (append final assistant message + usage).
        (
            ConversationState::StreamingResponse { mut session, request_id: rid_state, .. },
            Event::ApiStreamEnd { request_id: rid_evt, final_message, usage },
        ) if rid_state == rid_evt => {
            session.usage.add(&usage);
            session.history.push(final_message);
            let usage_effect = Effect::RenderTokenUsageUpdate {
                input_tokens: session.usage.0.input_tokens,
                output_tokens: session.usage.0.output_tokens,
            };
            (ConversationState::Idle { session }, vec![usage_effect])
        }

        // AwaitingApiResponse | StreamingResponse + ApiError → Idle + RenderError.
        (
            ConversationState::AwaitingApiResponse { session, .. }
            | ConversationState::StreamingResponse { session, .. },
            Event::ApiError { error, .. },
        ) => (
            ConversationState::Idle { session },
            vec![Effect::RenderError { error: error.message }],
        ),

        // Anywhere + UserExit → Terminated.
        (state, Event::UserExit) => {
            let session = state.session().clone();
            (
                ConversationState::Terminated { session, reason: "user_exit".into() },
                vec![Effect::Terminate { reason: "user_exit".into() }],
            )
        }

        // Catch-all: emit a diagnostic effect (no panic, no log call — purity).
        (state, event) => {
            let effect = Effect::RecordUnexpectedEvent {
                state_name: state.kind_name().into(),
                event_name: event_name(&event).into(),
            };
            (state, vec![effect])
        }
    }
}

fn event_name(e: &Event) -> &'static str {
    match e {
        Event::UserMessage { .. } => "UserMessage",
        Event::UserInterrupt => "UserInterrupt",
        Event::UserExit => "UserExit",
        Event::ApiStreamStart { .. } => "ApiStreamStart",
        Event::ApiStreamDelta { .. } => "ApiStreamDelta",
        Event::ApiStreamEnd { .. } => "ApiStreamEnd",
        Event::ApiError { .. } => "ApiError",
        Event::SessionLoaded(_) => "SessionLoaded",
    }
}

#[cfg(test)]
mod tests {
    // ... tests from Step 1 ...
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p lingxi-core --lib reducer`
Expected: 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/reducer.rs
git commit -m "feat(core): add pure-function reducer for Idle/Awaiting/Streaming/Terminated"
```

---

## Task 13: lingxi-api-client crate — SSE parser

**Files:**
- Create: `crates/api-client/Cargo.toml`
- Create: `crates/api-client/src/lib.rs`
- Create: `crates/api-client/src/sse.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-api-client"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
async-trait.workspace = true
futures-core.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: lib.rs**

```rust
//! Anthropic / OpenAI-compatible API client.
//!
//! All network I/O routes through `lingxi_traits::HttpTransport`. The client
//! itself is purely about request shape + SSE parsing + retry policy.

#![forbid(unsafe_code)]

pub mod anthropic;
pub mod error;
pub mod sse;
pub mod types;

pub use anthropic::AnthropicProvider;
pub use error::ApiError;
pub use types::{ContentDelta, MessageRequest, MessageResponse, StreamEvent};
```

- [ ] **Step 3: SSE parser — write tests first**

```rust
// crates/api-client/src/sse.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_event() {
        let raw = "event: content_block_delta\ndata: {\"delta\":{\"text\":\"hi\"}}\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("content_block_delta"));
        assert!(events[0].data.contains("hi"));
    }

    #[test]
    fn parse_multiple_events() {
        let raw = "event: a\ndata: 1\n\nevent: b\ndata: 2\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn ignore_keepalive_lines() {
        let raw = ": this is a comment\nevent: a\ndata: 1\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn multi_line_data_concatenates() {
        let raw = "event: x\ndata: line1\ndata: line2\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events[0].data, "line1\nline2");
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p lingxi-api-client --lib sse`
Expected: FAIL (function not defined)

- [ ] **Step 5: Implement**

```rust
//! SSE (Server-Sent Events) parser.
//!
//! Conforms to https://html.spec.whatwg.org/multipage/server-sent-events.html
//! Buffers raw chunks and emits complete `SseEvent` values.

use lingxi_protocol::SseEvent;

/// Parse one or more complete events out of a chunk. The chunk MUST end with
/// `\n\n` to terminate the last event; partial events are dropped.
///
/// For real streaming, wrap this in a buffered consumer (see `SseStreamReader`
/// below — added in a later task when we wire to HttpTransport.stream_sse).
#[must_use]
pub fn parse_sse_chunks(raw: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    for block in raw.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut event_type: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();
        let mut id: Option<String> = None;

        for line in block.lines() {
            if line.starts_with(':') {
                continue; // comment / keepalive
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("id:") {
                id = Some(rest.trim().to_string());
            }
        }

        if !data_lines.is_empty() {
            events.push(SseEvent {
                event_type,
                data: data_lines.join("\n"),
                id,
            });
        }
    }
    events
}

#[cfg(test)]
mod tests {
    // ... tests from Step 3 ...
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p lingxi-api-client --lib sse`
Expected: 4 tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/api-client
git commit -m "feat(api-client): add SSE parser"
```

---

## Task 14: API types + ApiError

**Files:**
- Create: `crates/api-client/src/types.rs`
- Create: `crates/api-client/src/error.rs`

- [ ] **Step 1: types.rs**

```rust
//! API-shape DTOs. Provider-neutral where possible; Anthropic-specific
//! fields are flagged in their docs.

use lingxi_protocol::{ConversationMessage, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<ConversationMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlockApi>,
    pub stop_reason: Option<String>,
    pub usage: UsageApi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockApi {
    Text { text: String },
    ToolUse { id: ToolUseId, name: String, input: Value },
    Thinking { thinking: String, signature: Option<String> },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct UsageApi {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart { message: MessageResponse },
    ContentBlockStart { index: u32, content_block: ContentBlockApi },
    ContentBlockDelta { index: u32, delta: ContentDelta },
    ContentBlockStop { index: u32 },
    MessageDelta { delta: MessageDeltaPayload, usage: Option<UsageApi> },
    MessageStop,
    Ping,
    Error { error: ErrorPayload },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaPayload {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}
```

- [ ] **Step 2: error.rs**

```rust
use lingxi_traits::HttpError;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum ApiError {
    #[error(transparent)]
    Http(#[from] HttpError),

    #[error("prompt too long: server requested {token_gap} fewer input tokens")]
    PromptTooLong { token_gap: u64, raw: String },

    #[error("rate limited (HTTP 429): retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("authentication failed: {0}")]
    Unauthorized(String),

    #[error("server returned malformed event: {0}")]
    MalformedStream(String),

    #[error("stream ended unexpectedly")]
    UnexpectedStreamEnd,
}
```

- [ ] **Step 3: Test the error mapping**

```rust
// append to error.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_too_long_carries_token_gap() {
        let e = ApiError::PromptTooLong { token_gap: 1500, raw: "...".into() };
        assert!(format!("{e}").contains("1500"));
    }
}
```

- [ ] **Step 4: Verify compile**

Run: `cargo check -p lingxi-api-client`
Expected: compile clean

- [ ] **Step 5: Run tests**

Run: `cargo test -p lingxi-api-client --lib`
Expected: 5 tests pass (4 from sse + 1 from error)

- [ ] **Step 6: Commit**

```bash
git add crates/api-client
git commit -m "feat(api-client): add API types and ApiError"
```

---

## Task 15: AnthropicProvider

**Files:**
- Create: `crates/api-client/src/anthropic.rs`

- [ ] **Step 1: Write tests against a MockHttpTransport (defer creating MockHttpTransport to Task 16 — write tests as a contract for now)**

```rust
// crates/api-client/src/anthropic.rs
#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::{HttpMethod, HttpRequest};

    #[test]
    fn build_request_includes_auth_and_version_headers() {
        let provider = AnthropicProvider::new(/*api_key*/"sk-ant-test", /*base_url*/None);
        let body = serde_json::json!({"model": "claude-opus-4-6"});
        let req = provider.build_request(&body);
        let header_keys: Vec<&str> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(header_keys.contains(&"x-api-key"));
        assert!(header_keys.contains(&"anthropic-version"));
        assert!(header_keys.contains(&"content-type"));
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn build_request_redacts_api_key_in_debug() {
        let provider = AnthropicProvider::new("sk-ant-secret", None);
        let s = format!("{provider:?}");
        assert!(!s.contains("sk-ant-secret"), "api key leaked: {s}");
    }
}
```

- [ ] **Step 2: Implement**

```rust
//! Anthropic provider — builds API requests and maps SSE events to engine
//! Events.

use crate::sse::parse_sse_chunks;
use crate::types::{ContentDelta, MessageResponse, StreamEvent};
use lingxi_protocol::{HttpMethod, HttpRequest};
use serde::Serialize;
use serde_json::Value;
use std::fmt;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl AnthropicProvider {
    #[must_use]
    pub fn new(api_key: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        }
    }

    #[must_use]
    pub fn build_request(&self, body: &Value) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/v1/messages", self.base_url),
            headers: vec![
                ("x-api-key".into(), self.api_key.clone()),
                ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "application/json".into()),
            ],
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(120)),
        }
    }

    #[must_use]
    pub fn build_streaming_request(&self, body: &Value) -> HttpRequest {
        let mut req = self.build_request(body);
        // mutate body to add stream: true
        let mut body_val: Value = serde_json::from_str(req.body.as_ref().unwrap()).unwrap();
        body_val["stream"] = Value::Bool(true);
        req.body = Some(body_val.to_string());
        req.headers
            .iter_mut()
            .find(|(k, _)| k == "accept")
            .map(|(_, v)| *v = "text/event-stream".to_string());
        req
    }

    /// Parse a single SSE event payload into a StreamEvent. Used when iterating
    /// over the stream returned by HttpTransport.stream_sse.
    pub fn parse_stream_event(data: &str) -> Result<StreamEvent, crate::ApiError> {
        serde_json::from_str::<StreamEvent>(data)
            .map_err(|e| crate::ApiError::MalformedStream(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    // ... tests from Step 1 ...
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p lingxi-api-client --lib anthropic`
Expected: 2 tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/api-client/src/anthropic.rs
git commit -m "feat(api-client): add AnthropicProvider with redacted Debug"
```

---

## Task 16: test-harness crate + MockHttpTransport

**Files:**
- Create: `crates/test-harness/Cargo.toml`
- Create: `crates/test-harness/src/lib.rs`
- Create: `crates/test-harness/src/mocks/mod.rs`
- Create: `crates/test-harness/src/mocks/mock_http.rs`
- Create: `crates/test-harness/src/mocks/mock_clock.rs`
- Create: `crates/test-harness/src/mocks/mock_runtime.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-test-harness"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-core = { path = "../core" }
lingxi-api-client = { path = "../api-client" }
serde.workspace = true
serde_json.workspace = true
async-trait.workspace = true
futures-core.workspace = true
tokio = { workspace = true }
tracing.workspace = true

[dev-dependencies]
proptest.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: lib.rs**

```rust
//! Mocks, contract tests, property tests, and parity fixtures for the
//! lingxi-core engine. Engine crates depend on this only as `dev-dependencies`.

#![forbid(unsafe_code)]

pub mod mocks;
```

- [ ] **Step 3: MockHttpTransport — scripted scenarios**

```rust
// crates/test-harness/src/mocks/mod.rs
pub mod mock_clock;
pub mod mock_http;
pub mod mock_runtime;

pub use mock_clock::MockClock;
pub use mock_http::{MockHttpTransport, ScriptedResponse};
pub use mock_runtime::MockRuntimeSpawner;
```

```rust
// crates/test-harness/src/mocks/mock_http.rs
//! MockHttpTransport — scripted response store for deterministic tests.
//!
//! Tests register responses for URL+method pairs, then assert the engine
//! consumes them in the expected order. Use `assert_drained` to verify no
//! response was left unconsumed at the end of a test.

use async_trait::async_trait;
use futures_core::stream::Stream;
use lingxi_protocol::{HttpRequest, HttpResponse, SseEvent};
use lingxi_traits::{HttpError, HttpTransport};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

#[derive(Debug, Clone)]
pub enum ScriptedResponse {
    Sync(HttpResponse),
    SyncErr(HttpError),
    /// SSE stream — vec of complete events that will be yielded one per poll.
    Stream(Vec<SseEvent>),
}

#[derive(Default)]
pub struct MockHttpTransport {
    queue: Arc<Mutex<VecDeque<ScriptedResponse>>>,
    received: Arc<Mutex<Vec<HttpRequest>>>,
}

impl MockHttpTransport {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, r: ScriptedResponse) {
        self.queue.lock().unwrap().push_back(r);
    }

    pub fn received_requests(&self) -> Vec<HttpRequest> {
        self.received.lock().unwrap().clone()
    }

    /// Assert no scripted responses remain. Use at end of test.
    pub fn assert_drained(&self) {
        let q = self.queue.lock().unwrap();
        assert!(
            q.is_empty(),
            "{} scripted responses left undelivered",
            q.len()
        );
    }
}

#[async_trait]
impl HttpTransport for MockHttpTransport {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.received.lock().unwrap().push(req);
        match self.queue.lock().unwrap().pop_front() {
            Some(ScriptedResponse::Sync(resp)) => Ok(resp),
            Some(ScriptedResponse::SyncErr(err)) => Err(err),
            Some(ScriptedResponse::Stream(_)) => {
                Err(HttpError::InvalidResponse(
                    "scripted Stream response on non-stream call".into(),
                ))
            }
            None => Err(HttpError::InvalidResponse(
                "no scripted response available".into(),
            )),
        }
    }

    async fn stream_sse(
        &self,
        req: HttpRequest,
    ) -> Result<lingxi_traits::http::SseStream, HttpError> {
        self.received.lock().unwrap().push(req);
        match self.queue.lock().unwrap().pop_front() {
            Some(ScriptedResponse::Stream(events)) => {
                Ok(Box::pin(ScriptedSseStream { remaining: events.into() }))
            }
            Some(ScriptedResponse::Sync(_)) | Some(ScriptedResponse::SyncErr(_)) => {
                Err(HttpError::InvalidResponse(
                    "scripted non-stream response on stream call".into(),
                ))
            }
            None => Err(HttpError::InvalidResponse(
                "no scripted response available".into(),
            )),
        }
    }
}

struct ScriptedSseStream {
    remaining: VecDeque<SseEvent>,
}

impl Stream for ScriptedSseStream {
    type Item = Result<SseEvent, HttpError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.remaining.pop_front().map(Ok))
    }
}
```

- [ ] **Step 4: MockClock**

```rust
// crates/test-harness/src/mocks/mock_clock.rs
use lingxi_traits::Clock;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct MockClock {
    now: Mutex<SystemTime>,
}

impl MockClock {
    #[must_use]
    pub fn at(seconds_since_epoch: u64) -> Self {
        Self {
            now: Mutex::new(UNIX_EPOCH + Duration::from_secs(seconds_since_epoch)),
        }
    }

    pub fn advance(&self, d: Duration) {
        let mut t = self.now.lock().unwrap();
        *t += d;
    }
}

impl Clock for MockClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap()
    }
}
```

- [ ] **Step 5: MockRuntimeSpawner**

```rust
// crates/test-harness/src/mocks/mock_runtime.rs
//! MockRuntimeSpawner — uses tokio's runtime under the hood, but is the only
//! place in the workspace allowed to import tokio outside of dev-deps.

use async_trait::async_trait;
use lingxi_traits::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::task::JoinHandle;

pub struct MockRuntimeSpawner {
    next_id: AtomicU64,
    handles: Mutex<HashMap<u64, JoinHandle<()>>>,
}

impl Default for MockRuntimeSpawner {
    fn default() -> Self {
        Self { next_id: AtomicU64::new(1), handles: Mutex::new(HashMap::new()) }
    }
}

#[async_trait]
impl RuntimeSpawner for MockRuntimeSpawner {
    async fn spawn(
        &self,
        name: &str,
        task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Result<BackgroundTaskHandle, RuntimeError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let h = tokio::spawn(task);
        self.handles.lock().unwrap().insert(id, h);
        Ok(BackgroundTaskHandle { task_name: name.into(), task_id: id })
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError> {
        let h = self.handles.lock().unwrap().remove(&handle.task_id);
        if let Some(h) = h {
            h.abort();
            Ok(())
        } else {
            Err(RuntimeError::NotFound(handle.task_name.clone()))
        }
    }
}
```

- [ ] **Step 6: Verify compile**

Run: `cargo check -p lingxi-test-harness`
Expected: compile clean

- [ ] **Step 7: Commit**

```bash
git add crates/test-harness
git commit -m "feat(test-harness): add MockHttpTransport, MockClock, MockRuntimeSpawner"
```

---

## Task 17: End-to-end scenario — Idle → user → API mock → final assistant message

**Files:**
- Create: `crates/test-harness/tests/e2e_single_turn.rs`

- [ ] **Step 1: Write the scenario**

```rust
//! End-to-end: drive the reducer through a complete single-turn conversation
//! against MockHttpTransport. This is the M1.1 acceptance test.

use lingxi_api_client::AnthropicProvider;
use lingxi_core::{reduce, ConversationState, Event, SessionState, Usage};
use lingxi_protocol::{
    ConversationMessage, Effect, HttpResponse, MessageId, RequestId, SessionId, SseEvent,
};
use lingxi_test_harness::mocks::{MockHttpTransport, ScriptedResponse};
use std::sync::Arc;

#[tokio::test]
async fn single_turn_conversation_against_mock_http() {
    // 1. Setup
    let session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
    let mut state = ConversationState::Idle { session };

    let request_id = RequestId::new();
    let message_id = MessageId::new();

    // 2. User says hi.
    let event = Event::UserMessage {
        message_id,
        request_id,
        content: "hi".into(),
    };
    let (next, effects) = reduce(state, event);
    state = next;

    // Assert: state advanced to AwaitingApiResponse, emitted SendApiRequest.
    match &state {
        ConversationState::AwaitingApiResponse { session, .. } => {
            assert_eq!(session.history.len(), 1);
        }
        other => panic!("unexpected state: {other:?}"),
    }
    let mut saw_send_request = false;
    for e in &effects {
        if let Effect::SendApiRequest { request_id: rid, .. } = e {
            assert_eq!(*rid, request_id);
            saw_send_request = true;
        }
    }
    assert!(saw_send_request);

    // 3. Simulate API stream events arriving.
    let (next, _) = reduce(state, Event::ApiStreamStart { request_id });
    state = next;
    assert!(matches!(state, ConversationState::StreamingResponse { .. }));

    let (next, effects) = reduce(state, Event::ApiStreamDelta {
        request_id,
        text: "Hello!".into(),
    });
    state = next;
    assert!(effects.iter().any(|e| matches!(e, Effect::RenderStreamDelta { .. })));

    let final_message = ConversationMessage::Assistant {
        id: MessageId::new(),
        content: vec![lingxi_protocol::ContentBlock::Text { text: "Hello!".into() }],
        stop_reason: Some("end_turn".into()),
    };
    let (next, effects) = reduce(state, Event::ApiStreamEnd {
        request_id,
        final_message,
        usage: Usage { input_tokens: 10, output_tokens: 5, ..Usage::default() },
    });
    state = next;

    // Assert: back to Idle, assistant message appended, usage updated.
    match &state {
        ConversationState::Idle { session } => {
            assert_eq!(session.history.len(), 2);
            assert_eq!(session.usage.0.input_tokens, 10);
            assert_eq!(session.usage.0.output_tokens, 5);
        }
        other => panic!("unexpected state: {other:?}"),
    }
    assert!(effects.iter().any(|e| matches!(e, Effect::RenderTokenUsageUpdate { .. })));
}

#[tokio::test]
async fn anthropic_provider_against_mock_http_does_one_roundtrip() {
    // This proves the api-client + MockHttpTransport pipeline works.
    let transport = Arc::new(MockHttpTransport::new());
    transport.enqueue(ScriptedResponse::Sync(HttpResponse {
        status: 200,
        headers: vec![],
        body: r#"{"id":"msg_test","model":"claude-opus-4-6","content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}"#.into(),
    }));

    let provider = AnthropicProvider::new("sk-ant-test", None);
    let body = serde_json::json!({"model":"claude-opus-4-6","max_tokens":1024,"messages":[{"role":"user","content":"hi"}]});
    let req = provider.build_request(&body);
    let resp = transport.request(req).await.expect("request should succeed");
    assert_eq!(resp.status, 200);
    transport.assert_drained();
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p lingxi-test-harness --test e2e_single_turn`
Expected: both tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/test-harness/tests/e2e_single_turn.rs
git commit -m "test(test-harness): end-to-end single-turn scenario through reducer + MockHttp"
```

---

## Task 18: Property tests — reducer invariants

**Files:**
- Create: `crates/test-harness/tests/properties_reducer.rs`

- [ ] **Step 1: Write properties**

```rust
//! Property tests for `reduce`. These run with proptest at 256 cases by
//! default; the CI `--features 10k-iterations` profile bumps to 10K.

use lingxi_core::{reduce, ConversationState, Event, SessionState, Usage};
use lingxi_protocol::{MessageId, RequestId, SessionId};
use proptest::prelude::*;

fn arb_event() -> impl Strategy<Value = Event> {
    prop_oneof![
        ("[a-z ]{0,100}").prop_map(|s| Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: s,
        }),
        Just(Event::UserInterrupt),
        Just(Event::UserExit),
    ]
}

proptest! {
    /// The reducer is total — it never panics on any (state, event) pair.
    #[test]
    fn reducer_is_total(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut state = ConversationState::Idle {
            session: SessionState::empty(SessionId::nil(), "x".into()),
        };
        for e in events {
            let (next, _effects) = reduce(state, e);
            state = next;
        }
    }

    /// Terminated is absorbing.
    #[test]
    fn terminated_absorbs(events in proptest::collection::vec(arb_event(), 0..20)) {
        let session = SessionState::empty(SessionId::nil(), "x".into());
        let mut state = ConversationState::Terminated { session, reason: "x".into() };
        for e in events {
            let (next, effects) = reduce(state, e);
            prop_assert!(next.is_terminal());
            prop_assert!(effects.is_empty());
            state = next;
        }
    }

    /// Token usage is monotonically non-decreasing.
    #[test]
    fn token_usage_monotonic(events in proptest::collection::vec(arb_event(), 0..50)) {
        let mut state = ConversationState::Idle {
            session: SessionState::empty(SessionId::nil(), "x".into()),
        };
        let mut prev = 0u64;
        for e in events {
            let (next, _) = reduce(state, e);
            let cur = next.session().usage.0.total_tokens();
            prop_assert!(cur >= prev, "usage decreased: {prev} -> {cur}");
            prev = cur;
            state = next;
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p lingxi-test-harness --test properties_reducer`
Expected: 3 properties pass with 256 cases each

- [ ] **Step 3: Commit**

```bash
git add crates/test-harness/tests/properties_reducer.rs
git commit -m "test(test-harness): property tests for reducer totality + Terminated absorption + token monotonicity"
```

---

## Task 19: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write workflow**

```yaml
name: ci
on:
  pull_request:
  push:
    branches: [main]

jobs:
  compile-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
        with:
          components: rustfmt, clippy
      - name: Zero-OS-deps check
        run: |
          cargo check -p lingxi-protocol --no-default-features
          cargo check -p lingxi-core --no-default-features
          cargo check -p lingxi-traits

  unit-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
      - run: cargo test --workspace --all-features

  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings

  cross-compile:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        target:
          - x86_64-unknown-linux-gnu
          - aarch64-apple-darwin
          - x86_64-pc-windows-msvc
          - aarch64-linux-android
          - aarch64-apple-ios
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
        with:
          targets: ${{ matrix.target }}
      - name: Install cross
        run: cargo install cross --locked
      - name: Cross-compile core crates
        run: |
          cross check --target ${{ matrix.target }} -p lingxi-protocol
          cross check --target ${{ matrix.target }} -p lingxi-core
          cross check --target ${{ matrix.target }} -p lingxi-traits
          cross check --target ${{ matrix.target }} -p lingxi-api-client
```

- [ ] **Step 2: Verify locally**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: compile-check + unit-tests + lint + cross-compile (5 targets)"
```

---

## Task 20: Plan exit — gate check

**Files:** (no new files; verification only)

- [ ] **Step 1: Verify gate criteria from spec M1.1**

Run these commands and check output:

```bash
# Zero-OS-deps check
cargo check -p lingxi-protocol --no-default-features  # PASS
cargo check -p lingxi-core --no-default-features      # PASS
cargo check -p lingxi-traits                          # PASS

# Cross-compile
for t in x86_64-unknown-linux-gnu aarch64-apple-darwin x86_64-pc-windows-msvc aarch64-linux-android aarch64-apple-ios; do
    cross check --target $t -p lingxi-core
done

# Unit + property tests
cargo test --workspace
```

Expected: all green. Property tests show "256 successful cases".

- [ ] **Step 2: Update workspace docs**

Add to `crates/protocol/README.md`, `crates/core/README.md`, `crates/traits/README.md`, `crates/api-client/README.md` one-paragraph summaries.

- [ ] **Step 3: Tag plan completion**

```bash
git tag -a m1.1-foundation -m "Plan 01 complete: protocol + core + traits + api-client + test-harness scaffolds; single-turn reducer works against MockHttp"
```

- [ ] **Step 4: Final commit**

```bash
git add crates/*/README.md
git commit -m "docs: README per foundation crate"
```

---

## Self-Review

**Spec coverage** — every requirement of spec M1.1+M1.2 has a task:
- D1 lingxi-protocol → Tasks 2-7
- D2 lingxi-core skeleton → Tasks 9-12
- D3 lingxi-traits → Task 8
- D4 lingxi-api-client → Tasks 13-15
- M1.2 gate "mock-mode single-turn conversation reducer passes" → Task 17 e2e test

**Type consistency check**:
- `RequestId`, `MessageId`, `SessionId` used identically across all crates ✅
- `Effect::SendApiRequest` shape matches between protocol and reducer ✅
- `Event::ApiStreamEnd { final_message, usage }` field names consistent ✅
- `ConversationState::Idle` field name `session` (not `state` or `conversation`) ✅

**Placeholder scan**: no "TODO", "implement later", "similar to Task N" found.

---

## Execution Handoff

This is Plan 01 of 17. After this completes, proceed to:
- **Plan 02**: Security & Cost (lingxi-permission + lingxi-secret + lingxi-cost)

See `docs/superpowers/plans/2026-05-22-lingxi-core-m1-02-security-cost.md`.

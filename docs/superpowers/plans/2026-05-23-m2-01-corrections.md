# LingXi Core M2 · Plan 01 · v0.2.0 Corrections

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Critical**: this plan is a single-commit refactor — do NOT commit per-task. The commit lands at the end (Task 14).

**Goal:** Surgical edits that bring v0.2.0 into the right shape before any M2-02..M2-07 feature work. Mostly deletions of M1-invented code (the 8-char pairing / HS256 JWT / 9-variant `BridgeMessage` stack), rewrites of the Windows sandbox/swarm files into `Unsupported` stubs, and a desktop worktree path/branch refactor to match claude-code's `worktree-<slug>` + `<repo>/.claude/worktrees/<slug>` convention.

**Architecture:** Nothing new ships behaviorally. Every method that previously stubbed in v0.2.0 now declares its real M2 follow-up plan via module-doc references (M2-02 for bridge/MCP-over-WS, M2-04 for sandbox, M2-05 for swarm, M2-06 for keychain). The `lingxi-bridge` crate shrinks to a 3-symbol placeholder (`BridgeMessagePlaceholder`, `BridgeState`, `IdeBridge`) ready for M2-02 to graft MCP-over-WebSocket onto. The worktree manager moves to a fixed path layout that future plans can rely on.

**Tech Stack:** Rust 2021, `serde`, `tokio` (existing), `tokio::fs` for the new copy-includes helper. No new crate-level dependencies. **Removed** deps: `rand`, `sha2`, `hmac`, `base64` from `lingxi-bridge/Cargo.toml`.

**References:**
- Spec section: §6.1 (Plan M2-01), §7.2 (capability flag wiring impact), §3 (1:1 framing), §5 (out of scope — cloud bridge), §10 (out-of-spec items), Appendix A (file-touch inventory).
- v0.2.0 baseline: commits `83ae0e0` (posix scaffold) + `ea7d188` (windows scaffold) + `2fafc0e` (M2 design).
- claude-code TS reference: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/worktree.ts` for slug rules.

---

## File touch inventory (locked at top per spec Appendix A)

Rewrites (file body replaced):
- `lingxi-core/platforms/windows/src/sandbox.rs` — full rewrite to `Unsupported` stub.
- `lingxi-core/platforms/windows/src/swarm.rs` — full rewrite to `Unsupported` stub.
- `lingxi-core/crates/bridge/src/message.rs` — collapse to placeholder struct.
- `lingxi-core/crates/bridge/src/state.rs` — keep two fields, tighten module doc.
- `lingxi-core/crates/bridge/src/transport.rs` — collapse to `Unsupported` stub.
- `lingxi-core/crates/bridge/src/lib.rs` — drop four `pub mod` decls + re-exports.
- `lingxi-core/crates/bridge/Cargo.toml` — drop `rand`/`sha2`/`hmac`/`base64` deps.

Deletes:
- `lingxi-core/crates/bridge/src/codes.rs`
- `lingxi-core/crates/bridge/src/jwt.rs`
- `lingxi-core/crates/bridge/src/rate_limiter.rs`
- `lingxi-core/crates/bridge/src/pairing.rs`

In-place modifications:
- `lingxi-core/platforms/posix/src/worktree.rs` — branch prefix `worktree-`, path under `.claude/worktrees/`, slug validation, flatten helper, copy_includes wiring, cleanup_stale stdout parsing.
- `lingxi-core/platforms/windows/src/worktree.rs` — same set of changes mirrored.
- `lingxi-core/crates/traits/src/sandbox.rs` — add `SandboxError::Unsupported` variant.
- `lingxi-core/crates/traits/src/worktree.rs` — add `WorktreeError::InvalidSlug(String)` variant.
- `lingxi-core/crates/secret/src/keychain_prefetch.rs` — module-doc update only (pointer to M2-06).

Total: 8 rewrites, 4 deletes, 5 modifications.

---

## Critical 1:1 fidelity items (locked specifics)

These are non-negotiable strings/identifiers from claude-code that MUST appear in code exactly as written:

- **Branch prefix**: `worktree-` (NOT `claude/`, NOT `lingxi/`). `claude/<branch>` is the cloud Remote Control bridge format (spec §5, out of scope).
- **Slug flatten char**: `+` (NOT `_`, NOT `/`). Justification: `+` is outside the allowlist `[a-zA-Z0-9._-]`, so the mapping is injective and `a/b` cannot collide with `a+b` (which would be a user-supplied slug). The reverse — using `_` — would collide because `_` is inside the allowlist.
- **Worktree path layout**: `<repo_root>/.claude/worktrees/<flattened-slug>` exactly. No `worktree_base` config knob; the layout is fixed.
- **Slug validation regex**: each `/`-separated segment matches `^[a-zA-Z0-9._-]+$`. Empty segments rejected. Total slug length ≤ 64 chars.
- **Windows sandbox `probe_capability().reason`**: `"claude-code does not support sandbox on Windows"`. Byte-for-byte.
- **Windows sandbox/swarm errors**: `SandboxError::Unsupported` and `SwarmError::Unsupported` variants. NOT the M1-invented "Job Objects" / "wezterm/Windows Terminal" TODO placeholders.
- **Bridge protocol identifiers from M1**: 8-char codes (A3), `JwtVerifier` HS256 (A3), 9-variant `BridgeMessage`, `RateLimiter` token-bucket — ALL DELETED. They were invented in M1 for a `claude.ai`-style trusted-device flow we are not building, and they conflict with claude-code's actual local IDE bridge (which is MCP-over-WebSocket via `~/.claude/ide/<port>.lock` lockfiles).
- **IDE WebSocket auth header** (documented in `transport.rs` module doc only — no wiring in this plan): `X-Claude-Code-Ide-Authorization`. NOT `Authorization: Bearer ...`. Actual transport wiring lands in M2-02 §6.2.

---

**File boundaries:** This plan does not introduce any new files. Every change is a rewrite, deletion, or in-place modification of an existing file. New files (`lockfile.rs` etc.) land in M2-02.

---

## Task 1: Add `SandboxError::Unsupported` variant

**Files:**
- Modify: `lingxi-core/crates/traits/src/sandbox.rs`

The rewritten Windows sandbox (Task 3) needs `SandboxError::Unsupported`. The current enum has `Unavailable(String)` but no parameterless `Unsupported` — claude-code's TS surface treats unsupported-platform as a distinct categorical error so we match. `SwarmError::Unsupported` already exists; this is the symmetric addition.

- [ ] **Step 1: Write failing test**

Append a new `#[cfg(test)] mod m2_01_tests` block at the bottom of `crates/traits/src/sandbox.rs`:

```rust
#[cfg(test)]
mod m2_01_tests {
    use super::*;

    #[test]
    fn sandbox_error_unsupported_displays() {
        let e = SandboxError::Unsupported;
        assert_eq!(format!("{e}"), "sandbox not supported on this platform");
    }

    #[test]
    fn sandbox_error_unsupported_distinct_from_unavailable() {
        let u = SandboxError::Unsupported;
        let a = SandboxError::Unavailable("bwrap missing".into());
        assert!(!matches!(u, SandboxError::Unavailable(_)));
        assert!(matches!(a, SandboxError::Unavailable(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p lingxi-traits --lib sandbox::m2_01_tests
```

Expected: FAIL with `no variant or associated item named 'Unsupported' found for enum 'SandboxError'`.

- [ ] **Step 3: Add the variant**

Edit `crates/traits/src/sandbox.rs`. Locate `pub enum SandboxError` (around line 135) and add as the second variant, immediately after the existing `Unavailable(String)`:

```rust
    /// Backend cannot wrap commands on this platform at all (e.g. Windows).
    /// Distinct from [`Unavailable`] — `Unavailable` means the backend exists
    /// but dependencies are missing; `Unsupported` means the backend itself
    /// is absent from claude-code on this OS.
    ///
    /// [`Unavailable`]: SandboxError::Unavailable
    #[error("sandbox not supported on this platform")]
    Unsupported,
```

The variant order keeps `Unavailable` first (existing) followed by `Unsupported` second so the diff is purely additive.

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p lingxi-traits --lib sandbox::m2_01_tests
```

Expected: 2 tests pass.

- [ ] **Step 5: Workspace check**

```bash
cargo check --workspace
```

Expected: clean. No existing consumer exhaustively matches all variants without a wildcard (verify), so adding a variant is non-breaking.

---

## Task 2: Add `WorktreeError::InvalidSlug` variant

**Files:**
- Modify: `lingxi-core/crates/traits/src/worktree.rs`

`validate_worktree_slug` (Task 8) needs a categorical error for bad input. Today `WorktreeError` has only `Unsupported`, `Git(String)`, `Io(String)`; using `Git("invalid slug …")` would lie about provenance. Add an explicit variant.

- [ ] **Step 1: Write failing test**

Append to `crates/traits/src/worktree.rs`:

```rust
#[cfg(test)]
mod m2_01_tests {
    use super::*;

    #[test]
    fn invalid_slug_carries_message() {
        let e = WorktreeError::InvalidSlug("contains '*'".into());
        let s = format!("{e}");
        assert!(s.contains("invalid slug"));
        assert!(s.contains("contains '*'"));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p lingxi-traits --lib worktree::m2_01_tests
```

Expected: FAIL — `no variant 'InvalidSlug'`.

- [ ] **Step 3: Add the variant**

Edit `crates/traits/src/worktree.rs` `pub enum WorktreeError`. Add as the second variant (after `Unsupported`, before `Git`):

```rust
    /// Caller-supplied slug failed validation (bad char, too long, empty
    /// segment, ...). Validation rules are platform-agnostic — see
    /// `validate_worktree_slug` in the platform crates.
    #[error("invalid slug: {0}")]
    InvalidSlug(String),
```

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p lingxi-traits --lib worktree::m2_01_tests
```

Expected: 1 test passes.

- [ ] **Step 5: Workspace check**

```bash
cargo check --workspace
```

Expected: clean.

---

## Task 3: Rewrite `platforms/windows/src/sandbox.rs` as Unsupported

**Files:**
- Rewrite: `lingxi-core/platforms/windows/src/sandbox.rs`

claude-code refuses sandbox on Windows at the source (`@anthropic-ai/sandbox-runtime` rejects the platform in its dependency check). Our v0.2.0 Windows sandbox file pretends to validate a policy and returns `SandboxBackend::None` — that is misleading. Replace with a true `Unsupported` stub.

- [ ] **Step 1: Write failing tests**

Before editing, read `crates/traits/src/sandbox.rs` to confirm the exact field set of `ProcessCommand` (it may or may not have a `stdin` field in v0.2.0 — adjust the `empty_cmd()` helper accordingly). Then append to the bottom of `platforms/windows/src/sandbox.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_traits::{ProcessCommand, Sandbox, SandboxBackend, SandboxPolicy};
    use std::collections::HashMap;

    fn empty_cmd() -> ProcessCommand {
        ProcessCommand {
            command: "echo".into(),
            args: vec!["hi".into()],
            cwd: None,
            env: HashMap::new(),
            // Add or drop further fields to match the trait crate's shape.
        }
    }

    #[test]
    fn is_available_returns_false() {
        assert!(!WindowsSandbox::new().is_available());
    }

    #[test]
    fn backend_returns_none() {
        assert_eq!(WindowsSandbox::new().backend(), SandboxBackend::None);
    }

    #[test]
    fn prepare_returns_unsupported() {
        let err = WindowsSandbox::new()
            .prepare(empty_cmd(), &SandboxPolicy::default())
            .unwrap_err();
        assert!(matches!(err, lingxi_traits::SandboxError::Unsupported));
    }

    #[test]
    fn bypass_with_audit_still_wraps_cmd() {
        // bypass_with_audit returns SandboxedCommand unconditionally — even
        // on unsupported platforms an explicit audit grant must still produce
        // a usable command.
        let wrapped = WindowsSandbox::new().bypass_with_audit(empty_cmd(), "explicit override");
        let _: lingxi_traits::SandboxedCommand = wrapped;
    }

    #[tokio::test]
    async fn probe_capability_reports_claude_code_string() {
        let cap = WindowsSandbox::new().probe_capability().await;
        assert!(!cap.available);
        assert_eq!(
            cap.reason.as_deref(),
            Some("claude-code does not support sandbox on Windows"),
        );
    }
}
```

- [ ] **Step 2: Verify tests fail against current file**

```bash
cargo test -p lingxi-platform-windows --lib sandbox::tests
```

Expected: `is_available_returns_false` fails (current file returns `true`); `prepare_returns_unsupported` fails (current file returns `Ok`); `probe_capability_reports_claude_code_string` fails (current `reason` mentions "Job Objects").

- [ ] **Step 3: Rewrite the file body**

Replace everything in `platforms/windows/src/sandbox.rs` **above** the `#[cfg(test)] mod tests` block with:

```rust
//! `Sandbox` trait impl — Windows.
//!
//! claude-code does not support sandboxing on Windows at all: the
//! `@anthropic-ai/sandbox-runtime` dependency-check refuses the platform
//! outright. We match that behavior by returning [`SandboxError::Unsupported`]
//! from every fallible method and reporting `available: false` from
//! [`Sandbox::probe_capability`]. No follow-up plan turns this on —
//! AppContainer / Job Objects work is not part of claude-code parity.

use async_trait::async_trait;
use lingxi_traits::{
    ProcessCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError, SandboxFeatures,
    SandboxPolicy, SandboxedCommand, SandboxedTag,
};

/// Windows-side [`Sandbox`] — always reports unsupported.
#[derive(Default)]
pub struct WindowsSandbox;

impl WindowsSandbox {
    /// Construct a new `WindowsSandbox`. Holds no state.
    #[must_use]
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Sandbox for WindowsSandbox {
    fn is_available(&self) -> bool { false }

    fn backend(&self) -> SandboxBackend { SandboxBackend::None }

    fn prepare(
        &self,
        _cmd: ProcessCommand,
        _policy: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError> {
        Err(SandboxError::Unsupported)
    }

    fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
        SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason { reason: reason.to_string() },
        )
    }

    async fn probe_capability(&self) -> SandboxCapability {
        SandboxCapability {
            available: false,
            reason: Some("claude-code does not support sandbox on Windows".into()),
            features: SandboxFeatures::default(),
        }
    }
}
```

- [ ] **Step 4: Run tests, then workspace check**

```bash
cargo test -p lingxi-platform-windows --lib sandbox::tests
cargo check --workspace
```

Expected: 5 sandbox tests pass; workspace clean.

---

## Task 4: Rewrite `platforms/windows/src/swarm.rs` as Unsupported

**Files:**
- Rewrite: `lingxi-core/platforms/windows/src/swarm.rs`

Current file returns `Unsupported` for two methods, but `destroy_swarm` returns `Ok(())` and the module doc mentions a "wezterm / Windows Terminal" future that does not exist in claude-code. Tighten all four trait methods plus the module doc.

- [ ] **Step 1: Write failing tests**

Read `crates/traits/src/swarm.rs` first to confirm the exact shape of `SwarmHandle` / `PanePosition`. Then append:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::AgentId;
    use lingxi_traits::{PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

    #[test]
    fn is_available_returns_false() {
        assert!(!WindowsSwarmBackend::new().is_available());
    }

    #[tokio::test]
    async fn start_swarm_returns_unsupported() {
        let err = WindowsSwarmBackend::new().start_swarm(SwarmLayout::default()).await.unwrap_err();
        assert!(matches!(err, SwarmError::Unsupported));
    }

    #[tokio::test]
    async fn create_teammate_pane_returns_unsupported() {
        let err = WindowsSwarmBackend::new()
            .create_teammate_pane(&AgentId::nil(), PanePosition::Right)
            .await
            .unwrap_err();
        assert!(matches!(err, SwarmError::Unsupported));
    }

    #[tokio::test]
    async fn destroy_swarm_returns_unsupported() {
        let err = WindowsSwarmBackend::new()
            .destroy_swarm(SwarmHandle { session_name: "phantom".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, SwarmError::Unsupported));
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p lingxi-platform-windows --lib swarm::tests
```

Expected: `destroy_swarm_returns_unsupported` fails (current file returns `Ok(())`).

- [ ] **Step 3: Rewrite the file body (above the tests)**

```rust
//! Swarm backend — Windows.
//!
//! claude-code refuses tmux / swarm on Windows: the platform check returns
//! before any swarm code runs and the user sees `--tmux is not supported on
//! Windows`. We match by returning [`SwarmError::Unsupported`] from every
//! fallible method. The previous v0.2.0 doc mentioned a "wezterm / Windows
//! Terminal" fallback — that feature does not exist in claude-code and was
//! invented in M1. M2-01 removes it.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

/// Windows-side [`SwarmBackend`] — always reports unsupported.
#[derive(Default)]
pub struct WindowsSwarmBackend;

impl WindowsSwarmBackend {
    #[must_use]
    pub fn new() -> Self { Self }
}

#[async_trait]
impl SwarmBackend for WindowsSwarmBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        Err(SwarmError::Unsupported)
    }
    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        _position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        Err(SwarmError::Unsupported)
    }
    async fn destroy_swarm(&self, _handle: SwarmHandle) -> Result<(), SwarmError> {
        Err(SwarmError::Unsupported)
    }
    fn is_available(&self) -> bool { false }
}
```

- [ ] **Step 4: Run tests + workspace check**

```bash
cargo test -p lingxi-platform-windows --lib swarm::tests
cargo check --workspace
```

Expected: 4 swarm tests pass; workspace clean.

---

## Task 5: Delete invented bridge files

**Files:**
- Delete: `lingxi-core/crates/bridge/src/{codes,jwt,rate_limiter,pairing}.rs`
- Modify: `lingxi-core/crates/bridge/src/lib.rs`
- Modify: `lingxi-core/crates/bridge/Cargo.toml`

These four files implemented an 8-char human-typeable pairing-code flow (A3), an HS256 JWT verifier (A3), a token-bucket rate limiter (A4), and pairing-state — the M1 imagining of a `claude.ai`-style trusted-device pairing handshake. claude-code has nothing of the kind. The local IDE bridge is MCP-over-WebSocket via lockfile discovery (M2-02). Cloud Remote Control bridge is out of scope (spec §5).

Delete-only task; no replacement code lands here.

- [ ] **Step 1: Delete the four files**

```bash
rm /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/bridge/src/codes.rs \
   /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/bridge/src/jwt.rs \
   /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/bridge/src/rate_limiter.rs \
   /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/bridge/src/pairing.rs
```

- [ ] **Step 2: Rewrite `crates/bridge/src/lib.rs`**

```rust
//! `lingxi-bridge` — local IDE bridge (placeholder until M2-02 wires the real
//! MCP-over-WebSocket transport).
//!
//! claude-code's local IDE bridge connects to a VS Code / JetBrains plugin
//! announced by `~/.claude/ide/<port>.lock`. Transport is plain WebSocket
//! carrying MCP JSON-RPC, auth'd by `X-Claude-Code-Ide-Authorization` from
//! the lockfile.
//!
//! M2-01 strips out the M1-invented pairing/JWT stack. M2-02 §6.2 adds
//! `lockfile.rs` and rewrites `transport.rs` to build an
//! `McpTransportSpec::WebSocket { url, headers }` and hand off to
//! `lingxi_mcp::McpRegistry::connect_with_spec()`.
//!
//! Until then this crate exposes only stub types so downstream callers compile.

#![forbid(unsafe_code)]

pub mod message;
pub mod state;
pub mod transport;

pub use message::BridgeMessagePlaceholder;
pub use state::BridgeState;
pub use transport::IdeBridge;
```

- [ ] **Step 3: Modify `crates/bridge/Cargo.toml`**

Remove these four lines that are dead without the JWT / pairing modules:

```
rand = "0.9"
sha2 = "0.10"
hmac = "0.12"
base64 = "0.22"
```

Keep all other deps (`lingxi-protocol`, `lingxi-traits`, `lingxi-secret`, `serde`, `serde_json`, `thiserror`, `async-trait`, `tokio`, `tracing`). `lingxi-secret` stays for a future credential helper; M2-02 may swap to a direct `lingxi-mcp` dep but M2-01 keeps the diff small.

- [ ] **Step 4: Defer cargo check; spot-check deletions**

`cargo check -p lingxi-bridge` will fail until Tasks 6 + 7 land. Chain into Task 6. Spot-check:

```bash
ls /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/crates/bridge/src/
```

Expected: only `lib.rs`, `message.rs`, `state.rs`, `transport.rs` remain.

---

## Task 6: Simplify `crates/bridge/src/message.rs` to placeholder

**Files:**
- Rewrite: `lingxi-core/crates/bridge/src/message.rs`

The 9-variant `BridgeMessage` enum exists nowhere in claude-code. Replace with a placeholder unit struct + module doc explaining what replaces it.

- [ ] **Step 1: Rewrite the file**

```rust
//! Placeholder for the local IDE bridge wire protocol.
//!
//! claude-code does not define a custom "bridge message" enum. The local IDE
//! bridge speaks MCP JSON-RPC over a WebSocket from `~/.claude/ide/<port>.lock`.
//! The 9-variant `BridgeMessage` enum that lived here in v0.2.0 was an M1
//! invention for a `claude.ai` remote-control flow now out of scope (spec §5).
//!
//! M2-02 §6.2 adds `crates/bridge/src/lockfile.rs` for discovery, and reuses
//! `lingxi-mcp` JSON-RPC types for the wire side. No new wire enum needed.

use serde::{Deserialize, Serialize};

/// Placeholder kept so downstream code can name a symbol without committing
/// to any wire shape. Will be removed in M2-02 once the bridge re-exports
/// `lingxi_mcp::McpNotificationDto` directly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeMessagePlaceholder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_roundtrip_is_null() {
        // A unit struct serializes to `null` — locks the observation.
        let s = serde_json::to_string(&BridgeMessagePlaceholder).unwrap();
        assert_eq!(s, "null");
        let _back: BridgeMessagePlaceholder = serde_json::from_str(&s).unwrap();
    }

    #[test]
    fn placeholder_default_equals_value() {
        assert_eq!(BridgeMessagePlaceholder, BridgeMessagePlaceholder::default());
    }
}
```

- [ ] **Step 2: Defer cargo check**

`cargo check -p lingxi-bridge` still won't pass yet (Task 7's transport rewrite is pending). Continue to Task 7.

---

## Task 7: Simplify `state.rs` + rewrite `transport.rs` stub

**Files:**
- Modify: `lingxi-core/crates/bridge/src/state.rs`
- Rewrite: `lingxi-core/crates/bridge/src/transport.rs`

Bundle these because they're both ~20-line files and `transport.rs`'s stub depends on `state.rs` staying simple.

### 7a. `state.rs`

The current file already has the right shape (`connected: bool`, `current_file: Option<PathBuf>`). Tighten the doc, add round-trip tests, lock the field set.

- [ ] **Step 1: Rewrite `state.rs`**

```rust
//! Lightweight bridge-session snapshot. Held by the engine so UI / telemetry
//! can observe whether the local IDE peer is connected and which file (if
//! any) is focused. Two fields only — M2-02 may add more.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeState {
    /// `true` when at least one trusted IDE is connected.
    pub connected: bool,
    /// Path of the file currently focused in the IDE, if known.
    #[serde(default)]
    pub current_file: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disconnected_no_file() {
        let s = BridgeState::default();
        assert!(!s.connected);
        assert!(s.current_file.is_none());
    }

    #[test]
    fn roundtrip_json_preserves_fields() {
        let s = BridgeState { connected: true, current_file: Some(PathBuf::from("/tmp/foo.rs")) };
        let back: BridgeState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s.connected, back.connected);
        assert_eq!(s.current_file, back.current_file);
    }
}
```

### 7b. `transport.rs`

The current `IdeBridge` references the now-deleted `BridgeMessage` and a `BridgeTransport` trait object. Collapse to an `Unsupported`-returning stub. Real wiring lands in M2-02 §6.2.

- [ ] **Step 2: Rewrite `transport.rs`**

```rust
//! `IdeBridge` — placeholder stub.
//!
//! M2-01 strips the M1-invented JWT / pairing stack. M2-02 §6.2 will rewrite
//! this stub to: (1) discover the most-recent `~/.claude/ide/<port>.lock`
//! via the M2-02-added `crates/bridge/src/lockfile.rs`; (2) parse the lockfile
//! JSON `{workspaceFolders, pid, ideName, transport, runningInWindows,
//! authToken}`; (3) construct
//! `lingxi_mcp::McpTransportSpec::WebSocket { url: format!("ws://localhost:{port}"),
//! headers: HashMap::from([("X-Claude-Code-Ide-Authorization", authToken)]) }`;
//! (4) hand off to `lingxi_mcp::McpRegistry::connect_with_spec`.
//!
//! The auth header is exactly `X-Claude-Code-Ide-Authorization` — NOT
//! `Authorization: Bearer …`. Locked here so M2-02 can't drift.
//!
//! Until then every method returns [`lingxi_traits::BridgeError::Unsupported`].

use crate::message::BridgeMessagePlaceholder;
use crate::state::BridgeState;
use lingxi_traits::BridgeError;
use tokio::sync::RwLock;

pub struct IdeBridge {
    state: RwLock<BridgeState>,
}

impl IdeBridge {
    #[must_use]
    pub fn new() -> Self { Self { state: RwLock::new(BridgeState::default()) } }

    /// Read-only snapshot of the bridge's current observable state.
    pub async fn state(&self) -> BridgeState { self.state.read().await.clone() }

    /// **Always returns `Unsupported`** until M2-02 wires lockfile + WebSocket.
    pub async fn connect(&self) -> Result<(), BridgeError> { Err(BridgeError::Unsupported) }

    /// **Always returns `Unsupported`**. Removed in M2-02 in favor of MCP JSON-RPC.
    pub async fn send_placeholder(&self, _msg: BridgeMessagePlaceholder) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported)
    }

    /// **Always returns `Unsupported`**.
    pub async fn disconnect(&self) -> Result<(), BridgeError> { Err(BridgeError::Unsupported) }
}

impl Default for IdeBridge { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_constructs_with_default_state() {
        let s = IdeBridge::new().state().await;
        assert!(!s.connected);
        assert!(s.current_file.is_none());
    }

    #[tokio::test]
    async fn connect_returns_unsupported() {
        let err = IdeBridge::new().connect().await.unwrap_err();
        assert!(matches!(err, BridgeError::Unsupported));
    }

    #[tokio::test]
    async fn send_placeholder_returns_unsupported() {
        let err = IdeBridge::new().send_placeholder(BridgeMessagePlaceholder).await.unwrap_err();
        assert!(matches!(err, BridgeError::Unsupported));
    }

    #[tokio::test]
    async fn disconnect_returns_unsupported() {
        let err = IdeBridge::new().disconnect().await.unwrap_err();
        assert!(matches!(err, BridgeError::Unsupported));
    }
}
```

- [ ] **Step 3: Run bridge crate tests + workspace check**

```bash
cargo test -p lingxi-bridge
cargo check --workspace
```

Expected: 2 (state) + 2 (message) + 4 (transport) = 8 tests pass; workspace clean.

---

## Task 8: Add `validate_worktree_slug` + `flatten_slug` helpers (posix)

**Files:**
- Modify: `lingxi-core/platforms/posix/src/worktree.rs`

Two pure functions that the next two tasks (Task 9 + Task 10) consume. claude-code's TS implementation in `src/utils/worktree.ts` (locally cached at `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/worktree.ts`) enforces:

- Each `/`-separated segment must match `^[a-zA-Z0-9._-]+$`.
- Total slug length ≤ 64 chars.
- Empty segments rejected (e.g. `"a//b"` and `"/foo"` and `"foo/"`).

`flatten_slug` is the disk-friendly transform: replace `/` with `+` so `a/b` lives at `.../worktrees/a+b` on disk. `+` is outside the allowlist so the mapping is injective (no two distinct valid slugs map to the same flattened name).

- [ ] **Step 1: Write failing tests**

Append to `platforms/posix/src/worktree.rs`:

```rust
#[cfg(test)]
mod slug_tests {
    use super::*;

    #[test]
    fn validate_accepts_legal_slugs() {
        for ok in ["feature", "user", "v1.2.3", "with-dash", "with_under", "with.dot",
                   "user/feature", "topic/area/sub"] {
            assert!(validate_worktree_slug(ok).is_ok(), "expected ok: {ok:?}");
        }
    }

    #[test]
    fn validate_rejects_bad_chars_and_segments() {
        for bad in ["a*b", "a b", "a:b", "a+b", "", "/foo", "foo/", "a//b"] {
            assert!(
                matches!(validate_worktree_slug(bad), Err(WorktreeError::InvalidSlug(_))),
                "expected err: {bad:?}"
            );
        }
    }

    #[test]
    fn validate_rejects_over_64_chars() {
        assert!(matches!(
            validate_worktree_slug(&"a".repeat(65)),
            Err(WorktreeError::InvalidSlug(_)),
        ));
        assert!(validate_worktree_slug(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn flatten_replaces_slashes_with_plus() {
        assert_eq!(flatten_slug("user/feature"), "user+feature");
        assert_eq!(flatten_slug("a/b/c"), "a+b+c");
        assert_eq!(flatten_slug("plain"), "plain");
    }

    #[test]
    fn flatten_is_injective_for_valid_slugs() {
        // The key property: no two valid slugs flatten to the same string,
        // because `+` is outside the allowed character set. `a+b` is not a
        // valid slug so it cannot collide with `a/b`'s flattened form.
        assert_ne!(flatten_slug("a/b"), flatten_slug("ab"));
        assert_ne!(flatten_slug("a/b/c"), flatten_slug("ab/c"));
        assert!(validate_worktree_slug("a+b").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p lingxi-platform-posix --lib worktree::slug_tests
```

Expected: FAIL — `cannot find function 'validate_worktree_slug' in this scope`.

- [ ] **Step 3: Implement helpers**

Add to `platforms/posix/src/worktree.rs` above the `PosixWorktreeManager` struct (free functions used by the impl):

```rust
/// Maximum allowed total length of a worktree slug.
///
/// Matches `MAX_WORKTREE_SLUG_LENGTH` in claude-code's TS reference at
/// `src/utils/worktree.ts`.
pub const MAX_WORKTREE_SLUG_LENGTH: usize = 64;

/// Validate a caller-supplied worktree slug.
///
/// Rules (mirrors claude-code's `src/utils/worktree.ts`):
/// - Total length 1..=64 chars.
/// - Each `/`-separated segment matches `^[a-zA-Z0-9._-]+$`.
/// - No empty segments (rejects `"/foo"`, `"foo/"`, `"a//b"`, `""`).
///
/// Returns [`WorktreeError::InvalidSlug`] with a human-readable detail when
/// any rule fails. The detail is suitable for direct surfacing in `/doctor`
/// or CLI error output.
pub fn validate_worktree_slug(slug: &str) -> Result<(), WorktreeError> {
    if slug.is_empty() {
        return Err(WorktreeError::InvalidSlug("slug is empty".into()));
    }
    if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
        return Err(WorktreeError::InvalidSlug(format!(
            "slug exceeds {MAX_WORKTREE_SLUG_LENGTH} chars (got {})",
            slug.len()
        )));
    }
    for segment in slug.split('/') {
        if segment.is_empty() {
            return Err(WorktreeError::InvalidSlug(format!(
                "slug contains empty segment: {slug:?}"
            )));
        }
        for ch in segment.chars() {
            let allowed = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
            if !allowed {
                return Err(WorktreeError::InvalidSlug(format!(
                    "slug contains invalid character {ch:?} in segment {segment:?}"
                )));
            }
        }
    }
    Ok(())
}

/// Flatten a `/`-separated slug into a single filesystem-friendly name.
///
/// Replaces every `/` with `+`. Because `+` is outside the allowed slug
/// character set (see [`validate_worktree_slug`]), this mapping is
/// injective: no two distinct valid slugs flatten to the same string.
///
/// Examples:
/// - `flatten_slug("user/feature")` → `"user+feature"`
/// - `flatten_slug("a/b/c")` → `"a+b+c"`
/// - `flatten_slug("plain")` → `"plain"`
#[must_use]
pub fn flatten_slug(slug: &str) -> String {
    slug.replace('/', "+")
}
```

- [ ] **Step 4: Run tests + workspace check**

```bash
cargo test -p lingxi-platform-posix --lib worktree::slug_tests
cargo check --workspace
```

Expected: 5 slug tests pass; workspace clean.

---

## Task 9: Fix posix worktree branch name + path layout + copy_includes

**Files:**
- Modify: `lingxi-core/platforms/posix/src/worktree.rs`

The big rename. Four behaviors change in `PosixWorktreeManager::create_worktree`:

1. **Branch name**: `lingxi/<slug>` → `worktree-<flatten_slug(slug)>`.
2. **Path layout**: `worktree_base.join(slug)` → `repo_root.join(".claude").join("worktrees").join(flatten_slug(slug))`.
3. **Slug validation**: call `validate_worktree_slug` first and surface `WorktreeError::InvalidSlug` if it fails.
4. **`copy_includes`**: iterate the `&[PathBuf]` parameter, copy each existing source-side file into the new worktree using `tokio::fs::copy`.

Also: the `PosixWorktreeManager` field `worktree_base` is removed (constructor changes signature) — the path is now derived from `repo_root` alone.

- [ ] **Step 1: Write failing tests**

Append to `platforms/posix/src/worktree.rs`:

```rust
#[cfg(test)]
mod create_tests {
    use super::*;
    use lingxi_traits::WorktreeManager;
    use tempfile::TempDir;
    use tokio::process::Command;

    /// Initialize a fresh git repo with one commit so worktree commands have
    /// something to branch from.
    async fn init_repo(dir: &std::path::Path) {
        async fn git(dir: &std::path::Path, args: &[&str]) {
            let mut c = Command::new("git");
            c.current_dir(dir);
            for a in args { c.arg(a); }
            assert!(c.output().await.unwrap().status.success(), "git {:?} failed", args);
        }
        git(dir, &["init", "-q", "-b", "main"]).await;
        git(dir, &["config", "user.email", "ci@test"]).await;
        git(dir, &["config", "user.name", "ci"]).await;
        tokio::fs::write(dir.join("seed.txt"), "seed").await.unwrap();
        git(dir, &["add", "seed.txt"]).await;
        git(dir, &["commit", "-qm", "seed"]).await;
    }

    #[tokio::test]
    async fn create_uses_worktree_dash_prefix_and_dot_claude_layout() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let handle = PosixWorktreeManager::new(repo.clone())
            .create_worktree("user/feature", None, &[]).await.unwrap();
        assert_eq!(handle.branch_name, "worktree-user+feature");
        assert_eq!(handle.path, repo.join(".claude/worktrees/user+feature"));
        assert!(handle.path.exists());
    }

    #[tokio::test]
    async fn create_rejects_invalid_slug_before_running_git() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let err = PosixWorktreeManager::new(repo)
            .create_worktree("a*b", None, &[]).await.unwrap_err();
        assert!(matches!(err, WorktreeError::InvalidSlug(_)));
    }

    #[tokio::test]
    async fn create_copies_includes_when_source_exists() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        // Untracked file we want copied into the worktree.
        tokio::fs::write(repo.join(".env"), "API_KEY=secret").await.unwrap();
        let handle = PosixWorktreeManager::new(repo.clone())
            .create_worktree("user/feature", None, &[std::path::PathBuf::from(".env")])
            .await.unwrap();
        let copied = tokio::fs::read_to_string(handle.path.join(".env")).await.unwrap();
        assert_eq!(copied, "API_KEY=secret");
    }

    #[tokio::test]
    async fn create_skips_missing_copy_includes() {
        // Missing copy-includes are best-effort, not fatal.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let res = PosixWorktreeManager::new(repo)
            .create_worktree("feat", None, &[std::path::PathBuf::from("does-not-exist.txt")])
            .await;
        assert!(res.is_ok());
    }
}
```

- [ ] **Step 2: Verify tempfile dep is available**

`platforms/posix/Cargo.toml` already has `tempfile` under `[dev-dependencies]` from M1. Confirm:

```bash
grep -n "tempfile" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/platforms/posix/Cargo.toml
```

Expected: at least one match in `[dev-dependencies]`. If zero matches, add `tempfile = "3.13"` to that section (the workspace already pins it per spec §7.1).

- [ ] **Step 3: Run tests to verify failure**

```bash
cargo test -p lingxi-platform-posix --lib worktree::create_tests
```

Expected: compile-time failure — `PosixWorktreeManager::new` still takes 2 args, and `handle.branch_name` is still `lingxi/user/feature` etc.

- [ ] **Step 4: Refactor the struct + constructor**

In `platforms/posix/src/worktree.rs`:

```rust
/// Production [`WorktreeManager`] using the `git worktree` CLI.
pub struct PosixWorktreeManager {
    /// Absolute path to the main repository working copy.
    repo_root: PathBuf,
}

impl PosixWorktreeManager {
    /// Build a new `PosixWorktreeManager` rooted at `repo_root`.
    ///
    /// New worktrees are created at
    /// `<repo_root>/.claude/worktrees/<flatten_slug(slug)>`. The layout is
    /// fixed (matches claude-code) — there is no `worktree_base` knob.
    #[must_use]
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}
```

- [ ] **Step 5: Refactor `create_worktree`**

Replace the `create_worktree` body with:

```rust
    async fn create_worktree(
        &self,
        slug: &str,
        base_branch: Option<&str>,
        copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError> {
        validate_worktree_slug(slug)?;
        let flat = flatten_slug(slug);
        let branch_name = format!("worktree-{flat}");
        let worktree_path = self.repo_root.join(".claude").join("worktrees").join(&flat);

        // git worktree add creates the leaf; the `.claude/worktrees/` parent may not exist yet.
        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| WorktreeError::Io(e.to_string()))?;
        }

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.repo_root);
        cmd.arg("worktree").arg("add").arg("-b").arg(&branch_name).arg(&worktree_path);
        if let Some(base) = base_branch { cmd.arg(base); }
        let output = cmd.output().await.map_err(|e| WorktreeError::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(WorktreeError::Git(String::from_utf8_lossy(&output.stderr).into_owned()));
        }

        // Best-effort copy of caller-supplied include paths. Missing sources
        // are silently skipped — the intent is to ferry across e.g. `.env`
        // files that aren't tracked by git.
        for rel in copy_includes {
            let src = self.repo_root.join(rel);
            if !tokio::fs::try_exists(&src).await
                .map_err(|e| WorktreeError::Io(e.to_string()))? { continue; }
            let dst = worktree_path.join(rel);
            if let Some(parent) = dst.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| WorktreeError::Io(e.to_string()))?;
            }
            tokio::fs::copy(&src, &dst).await
                .map_err(|e| WorktreeError::Io(e.to_string()))?;
        }

        Ok(WorktreeHandle { path: worktree_path, branch_name })
    }
```

- [ ] **Step 6: Update module-doc**

Replace the top-of-file `//!` block with:

```rust
//! `git worktree`-backed [`WorktreeManager`] for desktop hosts.
//!
//! Shells out to the `git` CLI rooted at the configured repository root.
//! Worktrees live at `<repo_root>/.claude/worktrees/<flatten_slug(slug)>`
//! and use the branch-name prefix `worktree-` (NOT `lingxi/` or `claude/`).
//!
//! The branch prefix and path layout match claude-code's
//! `src/utils/worktree.ts` exactly — see plan M2-01 §"Critical 1:1 fidelity
//! items" for the rationale.
//!
//! Slug validation rules:
//! - Each `/`-separated segment is `[a-zA-Z0-9._-]+`.
//! - Total length 1..=64 chars.
//! - No empty segments.
//!
//! `/` is flattened to `+` for the on-disk directory name so the layout
//! stays flat. `+` is outside the allowlist so the mapping is injective.
```

- [ ] **Step 7: Run tests and fix any caller-side breakage**

The constructor signature changed (`new(repo_root, worktree_base)` → `new(repo_root)`). Find any callsite and update it:

```bash
grep -rn "PosixWorktreeManager::new\|PosixWorktreeManager {" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/ --include="*.rs"
cargo test -p lingxi-platform-posix --lib worktree::create_tests
cargo test --workspace
```

Expected: zero or one callsite (possibly `examples/cli-demo` or `e2e_single_turn.rs`) — drop the old `worktree_base` arg. 4 create tests pass. Workspace tests clean.

---

## Task 10: Parse `git worktree prune -v` stdout in posix `cleanup_stale`

**Files:**
- Modify: `lingxi-core/platforms/posix/src/worktree.rs`

The current `cleanup_stale` runs `git worktree prune -v` but discards stdout and always returns `Ok(Vec::new())` — that's the M1 stub (see the `TODO(M2-followup)` comment in the v0.2.0 file). Parse stdout to return the actual pruned paths.

`git worktree prune -v` emits lines like:

```
Removing worktrees/user+feature: gitdir file points to non-existent location
```

Format: `Removing worktrees/<name>: <reason>`. We extract `<name>` and resolve it against `<repo_root>/.claude/worktrees/<name>` (where they live on disk in claude-code's layout). claude-code's TS surface returns the on-disk worktree path so consumers can `rm -rf` if needed; we match.

- [ ] **Step 1: Write failing tests**

Add a `#[cfg(test)] mod cleanup_tests` block. Exercise the parser via a private helper so the test does not need to invoke git to provoke stale worktrees:

```rust
#[cfg(test)]
mod cleanup_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_prune_output_extracts_names() {
        let stdout = "\
Removing worktrees/user+feature: gitdir file points to non-existent location
Removing worktrees/topic+area: gitdir file points to non-existent location
";
        let paths = parse_prune_v_stdout(stdout, &PathBuf::from("/tmp/repo"));
        assert_eq!(paths, vec![
            PathBuf::from("/tmp/repo/.claude/worktrees/user+feature"),
            PathBuf::from("/tmp/repo/.claude/worktrees/topic+area"),
        ]);
    }

    #[test]
    fn parse_prune_output_ignores_unrelated_lines() {
        let stdout = "some random noise\nRemoving worktrees/ok: stale\nnot a removing line\n";
        let paths = parse_prune_v_stdout(stdout, &PathBuf::from("/r"));
        assert_eq!(paths, vec![PathBuf::from("/r/.claude/worktrees/ok")]);
    }

    #[test]
    fn parse_prune_output_empty_stdout() {
        assert!(parse_prune_v_stdout("", &PathBuf::from("/r")).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p lingxi-platform-posix --lib worktree::cleanup_tests
```

Expected: FAIL — `parse_prune_v_stdout` not found.

- [ ] **Step 3: Implement parser**

Add to `platforms/posix/src/worktree.rs` (free function near `validate_worktree_slug`):

```rust
/// Parse `git worktree prune -v` stdout into pruned on-disk paths.
///
/// Each relevant line has the form `Removing worktrees/<name>: <reason>`.
/// We strip the `<name>` and resolve it against
/// `<repo_root>/.claude/worktrees/<name>` (where this codebase places its
/// worktrees per claude-code's layout). Lines that don't match the
/// expected prefix are silently skipped.
fn parse_prune_v_stdout(stdout: &str, repo_root: &std::path::Path) -> Vec<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Removing worktrees/")?;
            // <name>: <reason>
            let name = rest.split(':').next()?;
            if name.is_empty() {
                return None;
            }
            Some(repo_root.join(".claude").join("worktrees").join(name))
        })
        .collect()
}
```

- [ ] **Step 4: Wire parser into `cleanup_stale`**

Replace the body of `cleanup_stale` (replacing the `TODO(M2-followup)` block) with:

```rust
    async fn cleanup_stale(&self, _max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError> {
        // `max_age` is currently unused: `git worktree prune` consults its
        // own `gc.worktreePruneExpire` setting. claude-code does not expose
        // a per-call override either. Explicit age filtering is M2-followup.
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .arg("worktree").arg("prune").arg("-v")
            .output().await
            .map_err(|e| WorktreeError::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(WorktreeError::Git(String::from_utf8_lossy(&output.stderr).into_owned()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_prune_v_stdout(&stdout, &self.repo_root))
    }
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p lingxi-platform-posix --lib worktree::cleanup_tests
```

Expected: 3 tests pass.

---

## Task 11: Mirror the worktree changes into `platforms/windows/src/worktree.rs`

**Files:**
- Modify: `lingxi-core/platforms/windows/src/worktree.rs`

Apply the exact same set of changes to the Windows worktree file: helper functions, struct + constructor refactor, `create_worktree` body, `cleanup_stale` parser, and the matching test modules. The Windows version is functionally identical to posix (both shell out to `git`); `PathBuf` already abstracts the separator.

- [ ] **Step 1: Copy helpers verbatim from posix**

Copy from `platforms/posix/src/worktree.rs` into `platforms/windows/src/worktree.rs`:
- `pub const MAX_WORKTREE_SLUG_LENGTH: usize = 64;`
- `pub fn validate_worktree_slug(slug: &str) -> Result<(), WorktreeError>`
- `pub fn flatten_slug(slug: &str) -> String`
- `fn parse_prune_v_stdout(stdout: &str, repo_root: &std::path::Path) -> Vec<PathBuf>`

Bodies identical — `tokio::fs` and `PathBuf` are cross-platform.

- [ ] **Step 2: Refactor struct + constructor**

```rust
pub struct WindowsWorktreeManager { repo_root: PathBuf }

impl WindowsWorktreeManager {
    /// Build a new `WindowsWorktreeManager` rooted at `repo_root`.
    /// Worktrees live at `<repo_root>\.claude\worktrees\<flatten_slug(slug)>`.
    #[must_use]
    pub fn new(repo_root: PathBuf) -> Self { Self { repo_root } }
}
```

- [ ] **Step 3: Refactor `create_worktree` and `cleanup_stale`**

Copy both method bodies from the posix file verbatim. They are already path-agnostic and call into the helpers added in Step 1.

- [ ] **Step 4: Update module doc**

```rust
//! `git worktree`-backed [`WorktreeManager`] for Windows hosts.
//!
//! Shells out to the `git` CLI rooted at the configured repository root.
//! Worktrees live at `<repo_root>\.claude\worktrees\<flatten_slug(slug)>`
//! and use the branch-name prefix `worktree-`. Behavioral parity with the
//! posix implementation is intentional — see
//! [`lingxi_platform_posix::worktree`] for the canonical doc.
```

- [ ] **Step 5: Mirror tests + run them**

Mirror posix `slug_tests`, `create_tests`, and `cleanup_tests` modules verbatim in the Windows file. `create_tests` requires `git` on PATH (CI runners have it). Search for callsite breakage as in Task 9 Step 7.

```bash
grep -rn "WindowsWorktreeManager::new\|WindowsWorktreeManager {" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/ --include="*.rs"
cargo test -p lingxi-platform-windows --lib worktree
```

Expected: all worktree-module tests pass (5 slug + 4 create + 3 cleanup = 12).

---

## Task 12: Update `keychain_prefetch.rs` module doc

**Files:**
- Modify: `lingxi-core/crates/secret/src/keychain_prefetch.rs`

`KeychainPrefetch` has the right shape but no real keychain wiring. M2-06 §6.6 lands the real `MacOsKeychainStorage`. M2-01 only points the module doc at M2-06.

No code changes — doc only.

- [ ] **Step 1: Replace the top `//!` block**

```rust
//! Pre-warm the macOS keychain access prompt at startup so it doesn't
//! interrupt the first interactive moment.
//!
//! Spawns a background task that immediately retrieves the Anthropic API key
//! from [`SecureStorage`]; the first OS-level prompt happens during the
//! warm-up rather than during the user's first request.
//!
//! **Status**: this file holds the type + control-flow shell. The actual
//! `security` CLI invocations land in **M2-06 §6.6** (SecureStorage macOS
//! Keychain) — see
//! `docs/superpowers/plans/2026-05-23-m2-06-securestorage-sse-process.md`.
//! Until M2-06 runs, the `retrieve` call invoked here falls back to
//! `PlainTextSecureStorage` because no `security` shell-out exists yet.
//!
//! The eventual result is delivered through a `oneshot` channel callers
//! drain via [`KeychainPrefetch::consume`].
```

- [ ] **Step 2: Verify**

```bash
cargo doc --no-deps -p lingxi-secret
cargo check --workspace
```

Expected: clean.

---

## Task 13: Final workspace verification

**Files:** none touched — pure verification.

Run the full pre-commit gate from spec §8.1 (subset relevant to M2-01).

- [ ] **Step 1: Test suite**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test --workspace
```

Expected: all tests pass. Net delta vs v0.2.0 baseline of **104 tests**:
- `lingxi-traits`: +3 (Task 1: 2 sandbox; Task 2: 1 worktree).
- `lingxi-platform-windows`: +9 from sandbox/swarm rewrites + ~11 from Task 11 worktree mirror.
- `lingxi-platform-posix`: +12 from Tasks 8/9/10 (5 slug + 4 create + 3 cleanup).
- `lingxi-bridge`: ~+8 new (state/message/transport) minus ~10 deleted (codes/jwt/rate_limiter/pairing). Net: roughly flat.

Post-M2-01 expected: ~140 ± a few. Exact count is not load-bearing; what matters is **zero regressions** — every v0.2.0 test still passes.

If `cargo test --workspace` reports any deleted bridge test as missing-by-name, that indicates a stale `--test <name>` filter somewhere; search:

```bash
grep -rn "test.*\(codes\|jwt\|rate_limiter\|pairing\)" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/ --include="*.rs" --include="*.toml"
```

Expected: zero matches (apart from module-doc strings, which are fine).

- [ ] **Step 2: Clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. Watch for:
- `clippy::needless_return` on the rewritten `Unsupported` methods (use `Err(SandboxError::Unsupported)` as a tail expression — no `return`).
- `clippy::needless_pass_by_value` on `BridgeMessagePlaceholder` if it surfaces; acceptable since the parameter is intentionally consumed in the placeholder.

- [ ] **Step 3: Format check**

```bash
cargo fmt --all --check
```

Expected: clean. If not, run `cargo fmt --all` and re-verify.

- [ ] **Step 4: Cargo.lock check (post-dep removal)**

After Task 5's `Cargo.toml` edits, `Cargo.lock` should auto-update on the next build. Confirm `rand`/`sha2`/`hmac`/`base64` are not referenced **as direct deps** of `lingxi-bridge`:

```bash
cargo tree -p lingxi-bridge --edges normal | grep -E "rand|sha2|hmac|base64"
```

Expected: no output. (`rand` etc. may still appear elsewhere in the lockfile because other workspace members depend on them — that's fine. The test is "is lingxi-bridge's direct edge cut?")

Confirm by inspecting `Cargo.lock` for the `lingxi-bridge` package entry:

```bash
grep -A 30 "name = \"lingxi-bridge\"" /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/Cargo.lock | head -40
```

Expected: the `dependencies = [...]` block does **not** list `rand`, `sha2`, `hmac`, `base64`.

- [ ] **Step 5: `cargo check --no-default-features` for both platform crates**

Per spec §8.1:

```bash
cargo check -p lingxi-platform-posix --no-default-features
cargo check -p lingxi-platform-windows --no-default-features
```

Expected: clean.

---

## Task 14: Single commit per spec §6.1

**Files:** none — git operation only.

Per the spec, all M2-01 changes land as one commit `refactor(M2-01): correct v0.2.0 divergences from claude-code`. Do NOT split into per-subsystem commits; the spec is explicit (§6.1 final line: "Commit: single commit ...").

- [ ] **Step 1: Inspect what's staged / unstaged**

```bash
git status
git diff --stat
```

Expected: changes in `lingxi-core/platforms/{posix,windows}/src/{sandbox,swarm,worktree}.rs`, `lingxi-core/crates/bridge/{Cargo.toml,src/{lib,message,state,transport}.rs}`, `lingxi-core/crates/secret/src/keychain_prefetch.rs`, `lingxi-core/crates/traits/src/{sandbox,worktree}.rs`. Four files deleted (`codes`, `jwt`, `pairing`, `rate_limiter` under `bridge/src/`).

- [ ] **Step 2: Stage the changes**

```bash
git add lingxi-core/platforms \
        lingxi-core/crates/bridge \
        lingxi-core/crates/secret/src/keychain_prefetch.rs \
        lingxi-core/crates/traits/src/sandbox.rs \
        lingxi-core/crates/traits/src/worktree.rs
```

If `Cargo.lock` updated (it almost certainly did after Task 5), also stage:

```bash
git add lingxi-core/Cargo.lock
```

- [ ] **Step 3: Confirm no stray files staged**

```bash
git diff --cached --stat
```

Expected: only files from the file-touch inventory at the top of this plan, plus `Cargo.lock` if applicable. Nothing under `docs/`, `examples/`, or other crates.

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
refactor(M2-01): correct v0.2.0 divergences from claude-code

Surgical corrections to bring v0.2.0 into the right shape before M2-02..M2-07
feature work. Per spec §6.1:

- Windows sandbox/swarm: rewrite as Unsupported stubs. claude-code does not
  support either subsystem on Windows; v0.2.0's "Job Objects TODO" /
  "wezterm fallback" placeholders were M1 inventions.
- lingxi-bridge: delete the M1-invented 8-char pairing / HS256 JWT / 9-variant
  BridgeMessage / token-bucket RateLimiter stack. claude-code's local IDE
  bridge is MCP-over-WebSocket via ~/.claude/ide/<port>.lock lockfiles; the
  cloud Remote Control bridge is out of scope (spec §5). The crate now exports
  three placeholder symbols (BridgeMessagePlaceholder, BridgeState, IdeBridge)
  waiting for M2-02 §6.2 to graft the real transport on.
- Worktree manager (posix + windows): branch prefix lingxi/<slug> ->
  worktree-<flatten(slug)>; path layout <worktree_base>/<slug> ->
  <repo_root>/.claude/worktrees/<flatten(slug)>; slug validation (segment
  regex, 64-char cap, no empty segments); flatten char is '+' (outside the
  allowlist, so the mapping is injective); copy_includes parameter actually
  copies files; cleanup_stale parses git worktree prune -v stdout.
- secret/keychain_prefetch: doc-only pointer to M2-06.
- traits: add SandboxError::Unsupported and WorktreeError::InvalidSlug.

Spec: docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md §6.1.
Plan: docs/superpowers/plans/2026-05-23-m2-01-corrections.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Verify the commit**

```bash
git log -1 --stat
```

Expected: one commit summarizing the file changes from the inventory.

- [ ] **Step 6: Final smoke test from clean state**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: all three commands clean. This is the gate M2-02 will assume passed.

---

## Verification checklist (per spec §7.2 + §8.1)

Before declaring M2-01 done, confirm:

- [ ] `cargo test --workspace` passes, zero regressions vs v0.2.0 baseline of 104.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.
- [ ] `cargo check -p lingxi-platform-posix --no-default-features` clean.
- [ ] `cargo check -p lingxi-platform-windows --no-default-features` clean.
- [ ] Branch name in `PosixWorktreeManager::create_worktree` is `worktree-<flatten>` (not `lingxi/`).
- [ ] Worktree path is `<repo_root>/.claude/worktrees/<flatten>`.
- [ ] `flatten_slug("user/feature") == "user+feature"`.
- [ ] `validate_worktree_slug("a+b")` errs with `InvalidSlug`.
- [ ] `validate_worktree_slug` rejects empty / oversize / bad-char / empty-segment slugs.
- [ ] `WindowsSandbox::probe_capability().await.reason == Some("claude-code does not support sandbox on Windows")`.
- [ ] `WindowsSandbox::prepare` returns `SandboxError::Unsupported`.
- [ ] `WindowsSwarmBackend::start_swarm` etc. return `SwarmError::Unsupported`.
- [ ] `crates/bridge/src/codes.rs`, `jwt.rs`, `pairing.rs`, `rate_limiter.rs` no longer exist.
- [ ] `crates/bridge/Cargo.toml` no longer lists `rand`, `sha2`, `hmac`, `base64`.
- [ ] `lingxi-bridge` exports exactly three symbols at the crate root: `BridgeMessagePlaceholder`, `BridgeState`, `IdeBridge`.
- [ ] `keychain_prefetch.rs` module doc references M2-06.
- [ ] `IdeBridge::connect` returns `BridgeError::Unsupported`, and its module doc names the auth header `X-Claude-Code-Ide-Authorization`.
- [ ] Exactly one git commit was created for this plan.

---

## Capability-flag impact (spec §7.2)

M2-01 does not modify `PlatformCapabilities` directly — that machinery already exists. The behavioral implications for engine-side capability advertisement:

- `BridgeTransport` capability advertisement does not change shape; the engine sees `IdeBridge` returning `Unsupported` and may decline to advertise IDE-bridge-dependent tools at runtime. M2-02 §6.2 enables this for real.
- `Sandbox` capability on Windows is now categorically `available: false` with the claude-code error string. Engine consumers should not call `prepare` on Windows and expect success; M2-04 will exercise this on posix only.
- `SwarmBackend` capability on Windows is `available: false`. Engine consumers gate swarm features off when the backend reports unsupported.

No engine-crate edits are required to honor these — the trait surface already returns the right error variants. The §7.2 invariant ("Zero engine-crate changes are expected") holds.

---

## Dependencies

- **Upstream**: v0.2.0 (commits `83ae0e0` posix + `ea7d188` windows; tagged baseline). This plan operates on that exact tree.
- **Downstream**: M2-02 (jsonrpc + MCP + bridge wiring), M2-04 (sandbox), M2-05 (swarm). All three assume:
  - `lingxi-bridge` is a 3-symbol placeholder shell (M2-02 will add `lockfile.rs` and rewrite `transport.rs`).
  - Windows sandbox + swarm are categorically `Unsupported` (M2-04 + M2-05 will add posix-side real impls without touching Windows).
  - Worktree branch / path layout is fixed at `worktree-<flatten>` / `.claude/worktrees/<flatten>` (M2-07 parity fixture asserts this).

---

## Out-of-scope reminders (spec §5 + §10)

The following are **deliberately not addressed** by this plan even though they live near the touched files:

- Cloud Remote Control bridge (`crates/bridge/` cloud subsystem). Deferred to a separate milestone post-M3.
- Real macOS keychain wiring in `keychain_prefetch.rs`. Lands in M2-06.
- Real Windows fs-watch via ReadDirectoryChangesW. Lands in M2-05 via the `notify` crate.
- The `Already in a worktree session` runtime check surfaced when callers invoke `create_worktree` from inside another worktree. The 1:1 string is locked here in plan text, but the runtime check itself is M2-followup — M2-01 scope is path / branch / slug correctness only.

If any of those drift into a task during execution, stop and re-confirm with the operator before proceeding.

---

## End of plan

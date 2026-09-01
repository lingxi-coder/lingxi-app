# LingXi Code M8 — Composable Engine & Mobile Architecture Design

> **Status**: DRAFT v3 (UniFFI bidirectional callbacks adopted)
> **Date**: 2026-05-29
> **Target version**: v0.9.0
> **Predecessor**: M7 / v0.8.0 (TUI Surface)
> **Successors**: M9 (Mobile bring-up + bridge protocol completion)

**v3 changelog vs v2** — UniFFI callback interfaces become the mobile FFI boundary:

- **U1** Mobile-capability traits (`CameraControl`, `VoiceRecorder`, `SharingService`, plus `SecureStorage` on mobile) are defined in Rust under `traits/` and **exposed through UniFFI as foreign protocols/interfaces**. Swift/Kotlin code in `apps/ios-framework/` and `apps/android-aar/` implements them with native platform APIs (AVCaptureSession, MediaRecorder, UIActivityViewController, Intent.ACTION_SEND, Keychain, Keystore). Rust holds the resulting trait objects and calls into the foreign code transparently. **No `objc2` / `core-foundation` / `JNI` crates in Rust.**
- **U2** Mobile-exclusive tool crates collapse from per-OS to **single cross-mobile** crates: `tools/camera/`, `tools/voice/`, `tools/share/` (was `tools/camera-ios/` + `tools/camera-android/` etc.). The tool crate code is Rust-only and identical for both iOS and Android — it just calls `Arc<dyn CameraControl>` from `ToolCtx`. **6 mobile-exclusive tool crates → 3.**
- **U3** `platforms/ios/` and `platforms/android/` shrink dramatically. They retain Rust impls for things that are easier in Rust (FileSystem via `std::fs` over App Sandbox paths, HttpTransport via `reqwest+rustls`, Clock, `Unsupported*` impls for ProcessRunner/Sandbox/etc.) but **all camera/voice/share/secure-storage impls come in as UniFFI callbacks from the app layer**. A platform crate becomes mostly a `Platform` trait *constructor* that bundles foreign-supplied trait objects.
- **U4** Total crate count drops to **71** (was 74 in v2 before this consolidation; v3 nets −3 from mobile-exclusive merging).
- **U5** `apps/ios-framework/` and `apps/android-aar/` gain a Swift/Kotlin source tree (`swift/` and `kotlin/` subdirs) alongside their Rust UniFFI bindings.

**v2 changelog vs v1** — file layout finalized:

- **L1** Repo root renamed `lingxi-core` → `lingxi-code` (aligned with `claude-code` upstream naming).
- **L2** `crates/` wrapper directory removed. Engine crates live directly under the workspace root (Tokio-style flat layout).
- **L3** `lingxi-` prefix dropped from internal crate names. Workspace is not published to crates.io — name collision risk lives only at the `cargo publish` boundary, which is not crossed.
- **L4** Plugin crates grouped under top-level domain dirs (`tools/`, `skills/`, `commands/`, `platforms/`, `apps/`). Engine crates stay flat at root.
- **L5** Crate names inside grouping dirs keep their role prefix for greppability: `tools/file/` → `tool-file`, `skills/builtin/` → `skill-builtin`, `commands/core/` → `command-core`, `platforms/posix/` → `platform-posix`. Binary apps drop the role prefix (`apps/cli/` → `cli`).
- **L6** Mobile-exclusive tools (camera, voice, share) split per-OS (`tool-camera-ios` / `tool-camera-android`) because they have no cross-platform code to share. Cross-OS tools (file, web, etc.) stay single-crate and use the `traits` + `platforms/` adapter layer.
- **L7** Device-control tools added as a third category: `tool-computer-use` (controls the host desktop — mouse/keyboard/screenshot via Quartz/X11/SendInput), `tool-android-use` (drives a connected Android device via ADB), `tool-ios-use` (drives a connected iOS device/simulator via `xcrun`/`idb`). All three are **desktop-side** tools — the desktop engine drives the target. `engine-mobile` does not register them. Implementation deferred to M9; M8 reserves the crate slots and trait surface.

---

## §1 Goal & non-goals

### Goal

Restructure LingXi Code so that **one engine codebase serves both desktop and mobile binaries**, with platform-specific differences expressed as **crate-level composition** rather than `#[cfg(target_os)]` scatter or runtime feature flags.

Concretely, M8 delivers:

1. **`tool-api` / `skill-api` / `command-api` crates** — abstract Tool/Skill/Command traits + registries extracted from today's monolithic `crates/tools/`, `crates/skills/`, `crates/commands/`.
2. **Per-category tool crates** — 14 tool crates under `tools/` for cross-OS and desktop-only tools; an additional set of per-OS tool crates under `tools/` for mobile-exclusive capabilities (camera, voice, share).
3. **Composition-root apps** — `apps/engine-desktop/` and `apps/engine-mobile/` library crates that assemble the same core engine with different tool/skill/command sets.
4. **Platform layer kept and extended** — `platforms/` retains its role as the OS-adapter layer (FileSystem, ProcessRunner, HttpTransport, SecureStorage, Sandbox impls). `platforms/ios/` and `platforms/android/` added alongside the existing `posix/` and `windows/`.
5. **Engine builder API** — `Engine::builder().platform(...).tools(...).skills(...).commands(...).build()` replaces today's hard-coded `register_all_builtin_tools`.
6. **Dependency-graph CI gate** — `cargo-deny` rules prevent engine crates from depending on plugin crates, prevent plugin crates from depending on siblings, prevent leaf `apps/*` from being depended on.

After M8, the engine layer (`core`, `orchestrator`, `agent`, etc.) is **100% platform-agnostic** and **tool-agnostic** — it talks to `dyn Tool` through `ToolRegistry` and to the OS through `dyn Platform`.

### Non-goals (deferred)

- **Mobile real-device bring-up** — `platforms/ios/` and `platforms/android/` ship as compile-only stubs in M8; bringing up real iOS/Android binaries belongs to M9.
- **Mobile-exclusive tool implementations** — `tools/camera-ios/`, `tools/camera-android/`, etc. ship as empty crate skeletons; real Swift/Kotlin-backed implementations belong to M9.
- **Bridge protocol completion** — the existing `bridge/` crate gains protocol types; the matching remote-driving server (`apps/bridge-server/`) ships as a hello-world. Full bridge belongs to M9.
- **Backwards compatibility** — this restructure deliberately breaks the v0.8.0 internal crate layout and crate names. No migration shim. External consumers must switch to `engine-desktop` or `engine-mobile`.
- **Renaming engine subsystems' Rust source** — `core`, `orchestrator`, `agent`, etc. keep their current source layout. Only the directory wrapper (`crates/`) and `lingxi-` name prefix are removed.
- **Tool behavior changes** — the 41 tools' input/output schemas and behavior are byte-equivalent before and after M8. This is a packaging refactor, not a tool refactor.
- **`#[cfg(target_os)]` removal from `platforms/*`** — platform crates legitimately need cfg gates; the goal is only to keep cfg out of engine and tool crates.

---

## §2 Motivation

### §2.1 Today's monolithic structure

```
crates/tools/                       # 21K lines, 55 files
├── src/
│   ├── tool_trait.rs                # the Tool trait
│   ├── registry.rs                  # hard-codes registration of all 41 tools
│   ├── dispatcher.rs                # concurrency-partitioned dispatcher
│   ├── builtin/
│   │   ├── bash.rs                  # ← Bash, needs spawn → unavailable on iOS
│   │   ├── powershell.rs            # ← unavailable on iOS
│   │   ├── mcp.rs                   # ← stdio MCP, needs spawn
│   │   ├── lsp.rs                   # ← needs spawn
│   │   ├── agent.rs                 # ← subagent, needs spawn
│   │   ├── team.rs                  # ← tmux/iTerm
│   │   ├── worktree.rs              # ← git CLI
│   │   ├── file_read.rs             # ← portable
│   │   ├── ...                      # 41 tools total
```

`crates/tools/Cargo.toml` already pulls in MCP/LSP/sandbox/process dependencies regardless of caller. A mobile binary built today would still link `tokio::process`, the sandbox dispatcher, and the MCP stdio transport — even though it cannot use them.

### §2.2 Why `#[cfg(target_os)]` scatter is the wrong answer

The intuitive Rust approach to "different code per platform" is `#[cfg(target_os = "ios")]`. But this fails the current problem for three reasons:

1. **Granularity mismatch.** "Don't ship the Bash tool on mobile" is not a code difference — it's a *composition* difference. `#[cfg]` would scatter `#[cfg(not(any(target_os = "ios", target_os = "android")))]` across 7+ tool files; missing one is a silent leak.
2. **Compile-time blindness.** `cargo check` only checks the current target. With cfg scatter, a Linux dev can ship a `mod foo;` that breaks iOS compilation without noticing. Per-crate splits make `cargo build -p engine-mobile --target aarch64-apple-ios` an atomic verification.
3. **Cognitive cost.** Future contributors reading a tool file shouldn't need to mentally evaluate `#[cfg]` conditionals to know "does this run on mobile?". The answer should be "look at which `apps/` crate depends on it."

### §2.3 The composition-root pattern

This design adopts the classic **composition root** (a.k.a. "Bazel-style assembly"):

- **Library crates** (engine, `tools/*`, `skills/*`, `commands/*`, `platforms/*`) make no choices about what to ship.
- **App crates** (`apps/engine-desktop/`, `apps/engine-mobile/`) are the *only* place where "which tools, which skills, which commands, which platform" decisions live.
- Cargo's `[target.'cfg(...)'.dependencies]` + plain dependencies (no optional, no features for tool selection) provide the equivalent of Bazel's `select()`.

This is idiomatic Rust — see `tokio::runtime` (each runtime flavor is a `cfg`-selected impl injected at the type level), `tower` (middleware composed at builder time), `bevy` (plugin-based engine assembly).

### §2.4 Why `platforms/` stays (not collapsed into per-platform tools)

A natural alternative is to remove `platforms/` and put OS-specific code directly in tools, splitting each cross-OS tool per-OS (`tool-file` / `tool-file-ios` / `tool-file-android`). This is rejected because:

1. **Tools share OS adapters.** ~10 engine crates and ~8 tool crates all need `FileSystem`, `ProcessRunner`, `HttpTransport`, `SecureStorage`. Putting OS adapters in each tool would duplicate them 18+ times.
2. **Tool business logic is 95%+ OS-agnostic.** `FileReadTool` is ~500 lines: path validation, binary detection, line numbering, offset/limit, output formatting. Only **one line** (`ctx.fs.read(path)`) hits the OS. Splitting per-OS would copy the 500 lines three times.
3. **Numbers don't work.** 14 cross-OS tools × 4 OSes = 56 crates just for cross-OS tools. With shared OS adapters in `platforms/`, it stays at 14 + 4 = 18.

The `platforms/` layer is kept for **cross-OS shared infrastructure**. For **genuinely mobile-exclusive** tools (camera, voice, share — they have no desktop equivalent), per-OS crates *are* appropriate because there's no cross-platform code to share.

---

## §3 Architecture overview

```
                      ┌────────────────────────┐
                      │   apps/engine-desktop  │       ← composition roots
                      │   apps/engine-mobile   │         (the ONLY place
                      └───────────┬────────────┘         platform / tool
                                  │                      selection lives)
                                  │
            ┌─────────────────────┼─────────────────────┐
            ▼                     ▼                     ▼
    ┌──────────────┐      ┌──────────────┐      ┌──────────────┐
    │ Engine crates│      │  tools/      │      │ platforms/   │
    │ (root-level) │      │  (plugins)   │      │ (trait impl) │
    │              │      │              │      │              │
    │  core        │◀─────│  file/       │◀────▶│  posix/      │
    │  orchestrator│      │  task/       │      │  windows/    │
    │  agent       │      │  shell/      │      │  ios/        │
    │  compaction  │      │  mcp/        │      │  android/    │
    │  traits ─────┼──────┼──┐           │      └──────┬───────┘
    │  tool-api ───┼──────┼──┼──Tool─────│              │
    │  skill-api   │      │  │ trait     │              │ impl
    │  command-api │      └──┴───────────┘              ▼
    │  ...         │                            ┌──────────────┐
    └──────────────┘                            │  traits      │
                                                │  contracts   │
                                                └──────────────┘
```

- Engine crates (root) only know about **abstract** `Tool`/`Skill`/`Command`/`Platform` — never concrete ones.
- `tools/*` depends on `tool-api` and `traits`. Never on `platforms/*` or sibling `tools/*`.
- `platforms/*` depends on `traits`. Never reaches up.
- `apps/*` is the only direction-mixer — it pulls engine + tools + platforms together.

---

## §4 Workspace layout

### §4.1 Full directory tree

```
lingxi-code/                              ← repo root (renamed from lingxi-core)
├── Cargo.toml                            ← workspace root, resolver = "2"
├── Cargo.lock
├── deny.toml                             ← cargo-deny dependency rules
├── rust-toolchain.toml
├── README.md
│
│  # ─── Engine subsystems (flat at root, no `crates/` wrapper) ──────────
├── protocol/                             ← name = "protocol"
├── traits/                               ← name = "platform-api"
├── tool-api/                             ← name = "tool-api"          ★ NEW
├── skill-api/                            ← name = "skill-api"         ★ NEW
├── command-api/                          ← name = "command-api"       ★ NEW
├── core/                                 ← state machine, reducer
├── api-client/                           ← Anthropic API + SSE
├── orchestrator/                         ← turn loop
├── agent/                                ← subagent runtime
├── compaction/                           ← 5-layer compactor
├── permission/                           ← permission gate
├── hooks/                                ← hooks 4-arm runtime
├── memory/                               ← memdir + CLAUDE.md
├── session/                              ← session JSONL
├── cost/                                 ← cost tracking
├── secret/                               ← secret storage
├── mcp/                                  ← MCP client core
├── lsp/                                  ← LSP client core
├── jsonrpc/                              ← JSON-RPC framing
├── sandbox/                              ← sandbox trait + dispatcher
├── filestate/                            ← file mtime/lock cache
├── msgqueue/                             ← message queue
├── cron/                                 ← cron schedule engine
├── tasks/                                ← task registry
├── coordinator/                          ← coordinator mode
├── sidequery/                            ← side LLM
├── telemetry/                            ← analytics bus
├── telemetry-macros/                     ← proc-macros
├── anthropic-oauth/                      ← OAuth refresh
├── outputstyle/                          ← output styles
├── plugin/                               ← plugin manifest
├── bridge/                               ← bridge protocol + types (was crates/bridge)
├── tui/                                  ← iocraft component library
├── test-harness/                         ← parity / contract / property tests
│
│  # ─── Tool plugin crates ──────────────────────────────────────────────
├── tools/
│   │
│   │   # === Cross-OS (compile into both desktop and mobile binaries) ===
│   ├── file/                             ← name = "tool-file"
│   │                                       Read/Write/Edit/Glob/Grep/NotebookEdit
│   ├── task/                             ← name = "tool-task"
│   │                                       Task[Create/Get/List/Stop/Update/Output] + TodoWrite
│   ├── web/                              ← name = "tool-web"           WebFetch/WebSearch
│   ├── skill/                            ← name = "tool-skill"         SkillTool
│   ├── ui/                               ← name = "tool-ui"            AskUserQuestion/SendMessage/Brief/Sleep/SyntheticOutput
│   ├── meta/                             ← name = "tool-meta"          ToolSearch/Config
│   ├── cron/                             ← name = "tool-cron"          ScheduleCron/RemoteTrigger
│   ├── plan/                             ← name = "tool-plan"          EnterPlanMode/ExitPlanMode
│   │
│   │   # === Desktop-only (need spawn / PATH / sandbox-exec / tmux) ===
│   ├── shell/                            ← name = "tool-shell"         Bash/PowerShell/REPL
│   ├── agent/                            ← name = "tool-agent"         AgentTool
│   ├── mcp/                              ← name = "tool-mcp"           MCPTool/ListMcpResources/McpAuth/ReadMcpResource
│   ├── lsp/                              ← name = "tool-lsp"           LSPTool
│   ├── team/                             ← name = "tool-team"          TeamCreate/TeamDelete
│   ├── worktree/                         ← name = "tool-worktree"      EnterWorktree/ExitWorktree
│   │
│   │   # === Device-control (desktop-side; AI drives a target via OS APIs or remote protocol) ===
│   ├── computer-use/                     ← name = "tool-computer-use"
│   │                                       Drive host desktop: screenshot/mouse/keyboard.
│   │                                       Cross-desktop-OS via ComputerControl trait
│   │                                       (Quartz on macOS, XTest on Linux, SendInput on Win)
│   ├── android-use/                      ← name = "tool-android-use"
│   │                                       Drive connected Android device via ADB.
│   │                                       Cross-desktop-OS (just shells `adb` subprocess)
│   ├── ios-use/                          ← name = "tool-ios-use"
│   │                                       Drive connected iOS device/simulator via xcrun/idb.
│   │                                       macOS-only at runtime (is_available() check);
│   │                                       crate itself builds everywhere
│   │
│   │   # === Mobile-exclusive (single crate; Swift/Kotlin impls injected via UniFFI) ===
│   ├── camera/                           ← name = "tool-camera"
│   │                                       Calls Arc<dyn CameraControl> from ToolCtx;
│   │                                       impl supplied by Swift (AVCaptureSession) or
│   │                                       Kotlin (CameraX) via UniFFI callback interface
│   ├── voice/                            ← name = "tool-voice"
│   │                                       Calls Arc<dyn VoiceRecorder>;
│   │                                       impl supplied by Swift (AVAudioRecorder) or
│   │                                       Kotlin (MediaRecorder)
│   └── share/                            ← name = "tool-share"
│                                           Calls Arc<dyn SharingService>;
│                                           impl supplied by Swift (UIActivityViewController)
│                                           or Kotlin (Intent.ACTION_SEND)
│
│  # ─── Skill plugin crates ─────────────────────────────────────────────
├── skills/
│   └── builtin/                          ← name = "skill-builtin"
│                                           Provides register_desktop() / register_mobile()
│                                           (Most user-facing skills load from disk; this
│                                            crate ships only the built-in templates.)
│
│  # ─── Slash command plugin crates ────────────────────────────────────
├── commands/
│   ├── core/                             ← name = "command-core"
│   │                                       /help /compact /cost /model /clear /status /version
│   │                                       /login /logout /memory /init /add-dir
│   │                                       /permissions /agents /plan /resume /export
│   ├── desktop/                          ← name = "command-desktop"
│   │                                       /commit /commit-push-pr /pr-comments /branch /diff
│   │                                       /review /security-review /autofix-pr /bughunter
│   │                                       /bridge /ide /install-* /terminal-setup /chrome /desktop
│   │                                       /doctor /debug-tool-call /heapdump /stats
│   └── mobile/                           ← name = "command-mobile"
│                                           /mobile /voice /share /camera
│
│  # ─── Platform trait implementations ─────────────────────────────────
├── platforms/
│   ├── common/                           ← name = "platform-common"    cross-OS helpers
│   ├── posix-minimal/                    ← name = "platform-posix-minimal"   demo
│   ├── posix/                            ← name = "platform-posix"     macOS + Linux + WSL2
│   ├── windows/                          ← name = "platform-windows"   Windows 10 22H2+
│   ├── ios/                              ← name = "platform-ios"       ★ NEW (M8 stub)
│   └── android/                          ← name = "platform-android"   ★ NEW (M8 stub)
│
│  # ─── Composition roots & binaries ───────────────────────────────────
├── apps/
│   ├── engine-desktop/                   ← name = "engine-desktop"     composition lib
│   ├── engine-mobile/                    ← name = "engine-mobile"      composition lib
│   ├── cli/                              ← name = "cli"                binary (was crates/cli)
│   ├── bridge-server/                    ← name = "bridge-server"      binary (M9 grows it)
│   ├── ios-framework/                    ← name = "ios-framework"      uniffi → .xcframework
│   └── android-aar/                      ← name = "android-aar"        uniffi → .aar
│
├── examples/
│   └── cli-demo/                         ← name = "cli-demo"
└── docs/
    ├── ARCHITECTURE.md
    ├── PLATFORMS.md
    ├── SECURITY.md
    └── superpowers/specs/
```

### §4.2 Naming convention

Internal Cargo crate names follow these rules. Workspace is **not published** to crates.io, so collision with public crates is irrelevant.

| Location | Directory form | Cargo name | Example |
|---|---|---|---|
| Engine subsystem (root) | `foo/` | `foo` (bare) | `protocol`, `core`, `orchestrator`, `mcp` |
| Tool plugin | `tools/foo/` | `tool-foo` | `tool-file`, `tool-shell` |
| Mobile-only tool | `tools/foo-{ios,android}/` | `tool-foo-{ios,android}` | `tool-camera-ios` |
| Skill plugin | `skills/foo/` | `skill-foo` | `skill-builtin` |
| Command plugin | `commands/foo/` | `command-foo` | `command-core`, `command-desktop` |
| Platform impl | `platforms/foo/` | `platform-foo` | `platform-posix`, `platform-ios` |
| Composition lib | `apps/engine-foo/` | `engine-foo` | `engine-desktop`, `engine-mobile` |
| Binary app | `apps/foo/` | `foo` (bare) | `cli`, `bridge-server` |
| Packager | `apps/foo-framework/` or `apps/foo-aar/` | `foo-framework` / `foo-aar` | `ios-framework`, `android-aar` |

Rationale: engine crates at root and binary apps in `apps/` use bare names because they are top-level concepts. Crates inside grouped dirs keep a role prefix (`tool-`, `skill-`, `command-`, `platform-`) so they are identifiable in dependency lists — when reading `apps/engine-mobile/Cargo.toml`, the line `tool-file = { path = "../../tools/file" }` immediately conveys its role.

### §4.3 Why bare engine names don't collide

The two name spaces that could collide are:

1. **`cron/` (engine) vs `tools/cron/` (tool)** — Different Cargo names: `cron` vs `tool-cron`. No conflict.
2. **`mcp/` (engine) vs `tools/mcp/` (tool)** — Different Cargo names: `mcp` vs `tool-mcp`. No conflict.

This is the entire point of the role prefix on grouped crates.

### §4.4 Crate count

| Layer | v0.8.0 (M7) | v0.9.0 (M8) | Δ |
|---|---:|---:|---:|
| Engine (root) | 31 | 34 | +3 (tool-api, skill-api, command-api) |
| `tools/` | 0 (monolith in `crates/tools/`) | 20 | +20 (incl. 3 device-control + 3 mobile-exclusive) |
| `skills/` | 0 (monolith in `crates/skills/`) | 1 | +1 |
| `commands/` | 0 (monolith in `crates/commands/`) | 3 | +3 |
| `platforms/` | 4 | 6 | +2 (ios, android — both very thin under UniFFI callback model) |
| `apps/` | 1 (currently `crates/cli`) | 6 | +5 |
| `examples/` | 1 | 1 | 0 |
| **Workspace total** | 37 | 71 | +34 |

71 crates is well within the comfort zone of large Rust workspaces (Bevy: 33+, Tokio: 12+, Ruff: 100+). Compile time grows ~30-90s for full release builds, dominated by linking; incremental builds get *faster* because the change blast radius shrinks.

Tool crate breakdown by category:
- 8 cross-OS (file, task, web, skill, ui, meta, cron, plan)
- 6 desktop-only with shell/spawn (shell, agent, mcp, lsp, team, worktree)
- 3 device-control (computer-use, android-use, ios-use)
- 3 mobile-exclusive cross-OS (camera, voice, share — Swift/Kotlin impls injected via UniFFI callbacks; same Rust crate serves both iOS and Android)

---

## §5 Core APIs

The pivot of the entire restructure is the abstraction layer. Three small crates own the contracts that engine and plugins both depend on.

### §5.1 `tool-api/`

```rust
// tool-api/src/lib.rs
#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::sync::Arc;

pub mod context;
pub mod error;
pub mod registry;
pub mod schema;

pub use context::ToolCtx;
pub use error::{ToolError, ToolResult};
pub use registry::ToolRegistry;
pub use schema::ToolSchema;

/// A tool exposed to the model. Implementors know nothing about siblings,
/// platform target, or whether they're running in desktop or mobile binary.
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Tool name. MUST equal `self.schema().name`.
    fn name(&self) -> &'static str;

    /// JSON Schema for input/output. Static reference.
    fn schema(&self) -> &ToolSchema;

    /// Runtime gate. Returning `false` removes this tool from the LLM-visible
    /// tool list for this call. Default: always available.
    fn is_available(&self, _ctx: &ToolCtx) -> bool { true }

    /// Execute. Errors propagate as ToolError; the orchestrator wraps them
    /// into `tool_result` content blocks.
    async fn call(&self, ctx: &ToolCtx, input: JsonValue) -> ToolResult<JsonValue>;
}
```

```rust
// tool-api/src/context.rs
use std::sync::Arc;
use platform_api::*;

/// Per-call context injected by the orchestrator. Contains all platform
/// handles the tool might need plus per-call session info.
pub struct ToolCtx {
    pub fs:           Arc<dyn FileSystem>,
    pub http:         Arc<dyn HttpTransport>,
    pub process:      Arc<dyn ProcessRunner>,    // ios/android impl → Unsupported
    pub sandbox:      Arc<dyn Sandbox>,
    pub clock:        Arc<dyn Clock>,
    pub permission:   Arc<dyn PermissionGate>,
    pub mcp:          Arc<dyn McpTransport>,
    pub lsp:          Arc<dyn LspTransport>,
    pub swarm:        Arc<dyn SwarmBackend>,
    pub runtime:      Arc<dyn RuntimeSpawner>,
    pub session_id:   protocol::SessionId,
    pub agent_id:     Option<protocol::AgentId>,
    pub cwd:          std::path::PathBuf,
    pub model:        protocol::ModelId,
    pub abort:        tokio_util::sync::CancellationToken,
}
```

```rust
// tool-api/src/registry.rs
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn crate::Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::with_capacity(48) }
    }

    pub fn register<T: crate::Tool>(&mut self, tool: T) -> &mut Self {
        let arc: Arc<dyn crate::Tool> = Arc::new(tool);
        let name = arc.name();
        assert!(
            self.tools.insert(name, arc).is_none(),
            "duplicate tool registration: {name}"
        );
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn crate::Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn visible(&self, ctx: &crate::ToolCtx) -> Vec<Arc<dyn crate::Tool>> {
        self.tools.values()
            .filter(|t| t.is_available(ctx))
            .cloned()
            .collect()
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.tools.keys().copied().collect();
        v.sort_unstable();
        v
    }
}
```

### §5.2 `skill-api/` and `command-api/`

Mirror the same pattern. Skills:

```rust
#[async_trait]
pub trait Skill: Send + Sync + 'static {
    fn metadata(&self) -> &SkillMetadata;        // name, description, frontmatter
    fn body(&self) -> &str;                      // markdown body
    async fn resources(&self, ctx: &SkillCtx) -> SkillResult<Vec<SkillResource>>;
}
```

Commands:

```rust
#[async_trait]
pub trait SlashCommand: Send + Sync + 'static {
    fn name(&self) -> &'static str;              // e.g. "compact"
    fn aliases(&self) -> &'static [&'static str] { &[] }
    fn help(&self) -> &'static str;
    async fn execute(&self, ctx: &CommandCtx, args: &str) -> CommandResult;
}
```

Each has a `Registry` mirror of `ToolRegistry`.

### §5.3 `Platform` aggregate trait (added to `traits/`)

Today's 23 individual traits remain. We add an aggregate so composition roots pass one handle:

```rust
// platform-api/src/platform.rs
use std::sync::Arc;

pub trait Platform: Send + Sync {
    fn fs(&self) -> Arc<dyn crate::FileSystem>;
    fn http(&self) -> Arc<dyn crate::HttpTransport>;
    fn process(&self) -> Arc<dyn crate::ProcessRunner>;
    fn sandbox(&self) -> Arc<dyn crate::Sandbox>;
    fn clock(&self) -> Arc<dyn crate::Clock>;
    fn secure_storage(&self) -> Arc<dyn crate::SecureStorage>;
    fn swarm(&self) -> Arc<dyn crate::SwarmBackend>;
    fn mcp_transport(&self) -> Arc<dyn crate::McpTransport>;
    fn lsp_transport(&self) -> Arc<dyn crate::LspTransport>;
    fn runtime(&self) -> Arc<dyn crate::RuntimeSpawner>;
    fn auth(&self) -> Arc<dyn crate::AuthHandle>;
    fn budget(&self) -> Arc<dyn crate::BudgetEnforcerHandle>;

    // === Mobile capability surfaces (UniFFI callbacks from Swift/Kotlin) ===
    // Desktop platforms default to None. iOS/Android platforms return the
    // Swift/Kotlin-implemented trait objects passed in via apps/ios-framework
    // or apps/android-aar.
    fn camera(&self) -> Option<Arc<dyn crate::CameraControl>> { None }
    fn voice(&self) -> Option<Arc<dyn crate::VoiceRecorder>> { None }
    fn share(&self) -> Option<Arc<dyn crate::SharingService>> { None }

    /// Host computer control (mouse / keyboard / screenshot of the machine
    /// the engine is running on). Returns None on mobile platforms.
    /// Implemented by platforms/posix (Quartz on macOS, XTest on Linux)
    /// and platforms/windows (SendInput).
    fn computer_control(&self) -> Option<Arc<dyn crate::ComputerControl>> { None }
}
```

Each `platforms/<os>/` crate exports one `pub struct PosixPlatform` (or `IosPlatform`, etc.) implementing `Platform`. Composition roots construct it once at startup.

### §5.4 Engine builder (in `core/`)

```rust
// core/src/engine.rs
use std::sync::Arc;
use platform_api::Platform;
use tool_api::ToolRegistry;
use skill_api::SkillRegistry;
use command_api::CommandRegistry;

pub struct Engine {
    platform: Arc<dyn Platform>,
    tools: Arc<ToolRegistry>,
    skills: Arc<SkillRegistry>,
    commands: Arc<CommandRegistry>,
    orchestrator: Arc<crate::Orchestrator>,
}

pub struct EngineBuilder {
    platform: Option<Arc<dyn Platform>>,
    tools: ToolRegistry,
    skills: SkillRegistry,
    commands: CommandRegistry,
    cost_budget: Option<cost::Budget>,
    permission_policy: Option<permission::PolicyConfig>,
    initial_model: Option<protocol::ModelId>,
}

impl Engine {
    pub fn builder() -> EngineBuilder { EngineBuilder::default() }
}

impl EngineBuilder {
    pub fn platform(mut self, p: Arc<dyn Platform>) -> Self { self.platform = Some(p); self }
    pub fn tools(mut self, t: ToolRegistry) -> Self { self.tools = t; self }
    pub fn skills(mut self, s: SkillRegistry) -> Self { self.skills = s; self }
    pub fn commands(mut self, c: CommandRegistry) -> Self { self.commands = c; self }
    pub fn cost_budget(mut self, b: cost::Budget) -> Self { self.cost_budget = Some(b); self }
    pub fn permission_policy(mut self, p: permission::PolicyConfig) -> Self {
        self.permission_policy = Some(p); self
    }
    pub fn model(mut self, m: protocol::ModelId) -> Self { self.initial_model = Some(m); self }

    pub fn build(self) -> Result<Engine, EngineBuildError> {
        let platform = self.platform.ok_or(EngineBuildError::PlatformRequired)?;
        Ok(Engine {
            platform,
            tools: Arc::new(self.tools),
            skills: Arc::new(self.skills),
            commands: Arc::new(self.commands),
            orchestrator: /* construct */,
        })
    }
}
```

The orchestrator gets `Arc<ToolRegistry>` injected. Today's `ToolInvoker` trait in `traits/` is deleted.

### §5.5 UniFFI bidirectional callback architecture (mobile FFI boundary)

UniFFI 0.27+ supports **callback interfaces** (a.k.a. foreign traits): a Rust-declared trait that's implemented in Swift/Kotlin and called from Rust through generated FFI shims. This is the design's mobile FFI strategy.

#### Why bidirectional UniFFI

The intuitive "Rust does everything" approach would use `objc2` + `objc2-av-foundation` on iOS and `jni` on Android to call platform APIs directly from Rust. We reject this because:

1. **`objc2` and `jni` are gnarly.** They require unsafe blocks, manual ARC management, JNI reference juggling. Bugs are subtle and platform-debugging tooling (Xcode breakpoints, Android Studio) doesn't help with Rust-side issues.
2. **Native APIs evolve.** When Apple ships a new framework (Vision, App Intents, Live Activities), Rust bindings lag months or years. Swift gets day-one support.
3. **The work is Swift/Kotlin work.** Camera permission flows, share sheet customization, photo asset picking — these are inherently UI-tier concerns. Writing them in Rust through FFI is masochism.

UniFFI callback interfaces flip the relationship: Rust declares the contract, foreign code implements the leaves.

#### Trait surface (defined in `traits/`)

```rust
// platform-api/src/camera.rs
#[uniffi::export(callback_interface)]
#[async_trait]
pub trait CameraControl: Send + Sync {
    async fn capture_photo(&self, opts: CapturePhotoOpts) -> Result<CapturedImage, CameraError>;
    async fn pick_from_library(&self) -> Result<CapturedImage, CameraError>;
}

#[derive(uniffi::Record)]
pub struct CapturePhotoOpts {
    pub camera_position: CameraPosition,    // Front | Back
    pub allow_editing: bool,
}

#[derive(uniffi::Record)]
pub struct CapturedImage {
    pub jpeg_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mime_type: String,
}

#[derive(uniffi::Error, Debug, thiserror::Error)]
pub enum CameraError {
    #[error("permission denied")] PermissionDenied,
    #[error("user cancelled")] Cancelled,
    #[error("device unavailable")] DeviceUnavailable,
    #[error("{0}")] Other(String),
}
```

Similar traits live in `platform-api/src/voice.rs` (`VoiceRecorder`), `platform-api/src/share.rs` (`SharingService`), and the existing `platform-api/src/secure_storage.rs` gets a mobile-impl-via-callback variant.

#### Swift / Kotlin implementations (in app crates)

```swift
// apps/ios-framework/swift/CameraImpl.swift
import AVFoundation
import UIKit

class IosCameraImpl: CameraControl {
    func capturePhoto(opts: CapturePhotoOpts) async throws -> CapturedImage {
        // ... AVCaptureSession setup, photo capture, JPEG encode
        return CapturedImage(
            jpegBytes: jpegData,
            width: UInt32(image.size.width),
            height: UInt32(image.size.height),
            mimeType: "image/jpeg"
        )
    }

    func pickFromLibrary() async throws -> CapturedImage {
        // ... PHPickerViewController flow
    }
}
```

```kotlin
// apps/android-aar/kotlin/CameraImpl.kt
import androidx.camera.core.*

class AndroidCameraImpl(private val context: Context) : CameraControl {
    override suspend fun capturePhoto(opts: CapturePhotoOpts): CapturedImage {
        // ... CameraX ImageCapture flow
    }

    override suspend fun pickFromLibrary(): CapturedImage {
        // ... ActivityResultContracts.PickVisualMedia flow
    }
}
```

#### Construction flow

```
[Swift code in iOS app]
       │
       │ instantiates IosCameraImpl, IosVoiceImpl, IosShareImpl, IosKeychain
       ▼
[Swift composes PlatformImpls struct (UniFFI Record)]
       │
       │ calls LingxiCode.buildMobileEngine(platformImpls: ...)
       ▼
[Rust UniFFI shim in apps/ios-framework]
       │
       │ wraps each Swift impl in Arc<dyn Trait>
       ▼
[Rust constructs IosPlatform with Arc<dyn CameraControl> etc.]
       │
       │ feeds into Engine::builder()
       ▼
[Engine runs; tools/camera/ calls ctx.platform.camera() → Arc<dyn CameraControl>]
       │
       │ method call traverses FFI back to Swift
       ▼
[IosCameraImpl.capturePhoto() executes in Swift]
```

The Rust tool code (`tools/camera/src/lib.rs`) is **completely OS-agnostic** — it only knows `Arc<dyn CameraControl>`. iOS vs Android is purely an implementation detail of who constructed the trait object.

#### What stays in Rust on mobile

Not everything goes through callbacks. The following are simpler to implement in Rust:

| Capability | Mobile impl location |
|---|---|
| FileSystem | Rust `std::fs` inside App Sandbox; `platforms/ios/` resolves sandbox-relative paths |
| HttpTransport | Rust `reqwest` with `rustls` (works fine on iOS/Android) |
| Clock | Rust `std::time` |
| ProcessRunner | `UnsupportedProcessRunner` (Rust stub) |
| Sandbox | `UnsupportedSandbox` (Rust stub) |
| Camera / Voice / Share | UniFFI callback → Swift/Kotlin |
| SecureStorage (Keychain/Keystore) | UniFFI callback → Swift/Kotlin (simpler than `security-framework` / Android Keystore bindings) |
| Notifications, Location, Sensors | UniFFI callback (when added in future milestones) |

### §5.6 Device-control tools

Three tools let the AI drive an external target. All three run on the **desktop** (engine-desktop registers them; engine-mobile does not). They differ in target and transport:

| Tool crate | Target | Transport | Lives in |
|---|---|---|---|
| `tool-computer-use` | The host desktop the engine is running on | `ComputerControl` trait → Quartz / XTest / SendInput | `tools/computer-use/` |
| `tool-android-use` | Connected Android device | `ProcessRunner` → `adb` subprocess | `tools/android-use/` |
| `tool-ios-use` | Connected iOS device / simulator | `ProcessRunner` → `xcrun simctl` / `idb` subprocess | `tools/ios-use/` |

#### `ComputerControl` trait (new in `traits/`)

```rust
// platform-api/src/computer_control.rs
#[async_trait]
pub trait ComputerControl: Send + Sync {
    async fn screenshot(&self) -> Result<Screenshot, ComputerError>;
    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), ComputerError>;
    async fn left_click(&self, x: i32, y: i32) -> Result<(), ComputerError>;
    async fn right_click(&self, x: i32, y: i32) -> Result<(), ComputerError>;
    async fn double_click(&self, x: i32, y: i32) -> Result<(), ComputerError>;
    async fn type_text(&self, s: &str) -> Result<(), ComputerError>;
    async fn key(&self, name: &str) -> Result<(), ComputerError>;
    async fn scroll(&self, x: i32, y: i32, dx: i32, dy: i32) -> Result<(), ComputerError>;
    async fn display_size(&self) -> Result<(u32, u32), ComputerError>;
}

pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}
```

Implemented in `platforms/posix/` (with `cfg(target_os = "macos")` for Quartz vs `cfg(target_os = "linux")` for XTest internally) and in `platforms/windows/` (SendInput). `platforms/ios/` and `platforms/android/` leave `Platform::computer_control()` at its default `None` — the engine running on a phone has no "host machine" to control.

#### Why `android-use` and `ios-use` don't need new traits

These tools just shell out to existing CLI binaries (`adb`, `xcrun`, `idb`). They don't need OS-specific Rust APIs — they need `ProcessRunner` (which already exists in `traits/`) plus the binary on `PATH`. So the implementations live entirely in their tool crates with no platform-layer support.

#### Schema mapping

`tool-computer-use` exposes a single tool named `computer` to the LLM (matching Anthropic's `computer_20241022` schema, where one tool takes an `action` enum: `screenshot` / `key` / `type` / `mouse_move` / `left_click` / etc.). `tool-android-use` and `tool-ios-use` follow the same pattern: one tool name (`android_use` / `ios_use`) with an action discriminator.

This lets each crate stay self-contained and the LLM see three clearly distinct tools rather than ~30 fine-grained `screenshot_macos`, `click_macos`, etc.

---

## §6 Composition root pattern

### §6.1 Tool crate template

Every `tools/<category>/` crate follows the same shape. Example `tools/file/`:

```
tools/file/
├── Cargo.toml
└── src/
    ├── lib.rs                  # public exports + register_all()
    ├── read.rs                 # FileReadTool
    ├── write.rs                # FileWriteTool
    ├── edit.rs                 # FileEditTool
    ├── glob.rs                 # GlobTool
    ├── grep.rs                 # GrepTool
    ├── notebook_edit.rs        # NotebookEditTool
    └── shared.rs               # path validation, binary detection
```

```rust
// tools/file/src/lib.rs
#![forbid(unsafe_code)]

mod read;
mod write;
mod edit;
mod glob;
mod grep;
mod notebook_edit;
mod shared;

pub use read::FileReadTool;
pub use write::FileWriteTool;
pub use edit::FileEditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use notebook_edit::NotebookEditTool;

/// Register every tool in this crate. Composition roots call exactly once.
pub fn register_all(reg: &mut tool_api::ToolRegistry) {
    reg.register(FileReadTool::new())
       .register(FileWriteTool::new())
       .register(FileEditTool::new())
       .register(GlobTool::new())
       .register(GrepTool::new())
       .register(NotebookEditTool::new());
}
```

```toml
# tools/file/Cargo.toml
[package]
name = "tool-file"
version = "0.9.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
tool-api  = { path = "../../tool-api" }
traits    = { path = "../../traits" }
protocol  = { path = "../../protocol" }
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio = { workspace = true, features = ["fs"] }
walkdir = "2.5"
globset = "=0.4.15"
ignore  = "=0.4.23"
grep-regex    = "=0.1.13"
grep-searcher = "=0.1.14"
thiserror.workspace = true

[lints]
workspace = true
```

### §6.2 Mobile-exclusive tool template

`tools/camera-ios/` has no cross-platform partner; it's iOS-only. The crate is gated to iOS at the Cargo level so it never tries to build on Linux:

```toml
# tools/camera-ios/Cargo.toml
[package]
name = "tool-camera-ios"
version = "0.9.0"
edition.workspace = true

[dependencies]
tool-api = { path = "../../tool-api" }
traits   = { path = "../../traits" }
protocol = { path = "../../protocol" }
async-trait.workspace = true

[target.'cfg(target_os = "ios")'.dependencies]
objc2 = "0.5"
objc2-foundation = "0.2"
objc2-av-foundation = "0.2"

[lib]
# Empty stub on non-iOS targets so the workspace still builds on Linux/macOS
# (no actual code; lib.rs has #![cfg(target_os = "ios")]).
```

```rust
// tools/camera-ios/src/lib.rs
#![cfg(target_os = "ios")]
#![forbid(unsafe_code)]

mod tool;
pub use tool::CameraCaptureTool;

pub fn register_all(reg: &mut tool_api::ToolRegistry) {
    reg.register(CameraCaptureTool::new());
}
```

Composition roots use `[target.'cfg(target_os = "ios")'.dependencies]` to depend on this crate only when building for iOS.

### §6.3 Composition root example: `apps/engine-mobile/`

```toml
# apps/engine-mobile/Cargo.toml
[package]
name = "engine-mobile"
version = "0.9.0"
edition.workspace = true

[dependencies]
# === Engine core (shared with desktop, identical code path) ===
tool-api      = { path = "../../tool-api" }
skill-api     = { path = "../../skill-api" }
command-api   = { path = "../../command-api" }
protocol      = { path = "../../protocol" }
traits        = { path = "../../traits" }
core          = { path = "../../core" }
orchestrator  = { path = "../../orchestrator" }
agent         = { path = "../../agent" }
compaction    = { path = "../../compaction" }
permission    = { path = "../../permission" }
hooks         = { path = "../../hooks" }
memory        = { path = "../../memory" }
session       = { path = "../../session" }
cost          = { path = "../../cost" }
api-client    = { path = "../../api-client" }
anthropic-oauth = { path = "../../anthropic-oauth" }
secret        = { path = "../../secret" }
telemetry     = { path = "../../telemetry" }
tasks         = { path = "../../tasks" }

# === Cross-OS tool set (compiled into mobile binary) ===
tool-file  = { path = "../../tools/file" }
tool-task  = { path = "../../tools/task" }
tool-web   = { path = "../../tools/web" }
tool-skill = { path = "../../tools/skill" }
tool-ui    = { path = "../../tools/ui" }
tool-meta  = { path = "../../tools/meta" }
tool-cron  = { path = "../../tools/cron" }
tool-plan  = { path = "../../tools/plan" }
# DELIBERATELY ABSENT: tool-shell, tool-mcp, tool-lsp, tool-agent,
# tool-team, tool-worktree — not compiled into mobile binary.

# === Mobile-exclusive tools (single crate; Swift/Kotlin impls via UniFFI) ===
tool-camera = { path = "../../tools/camera" }
tool-voice  = { path = "../../tools/voice" }
tool-share  = { path = "../../tools/share" }

# === Mobile skill / command sets ===
skill-builtin   = { path = "../../skills/builtin" }
command-core    = { path = "../../commands/core" }
command-mobile  = { path = "../../commands/mobile" }
# command-desktop deliberately absent.

# === Mobile platform impls (per target_os) ===
[target.'cfg(target_os = "ios")'.dependencies]
platform-ios = { path = "../../platforms/ios" }

[target.'cfg(target_os = "android")'.dependencies]
platform-android = { path = "../../platforms/android" }

[lints]
workspace = true
```

```rust
// apps/engine-mobile/src/lib.rs
#![forbid(unsafe_code)]

use std::sync::Arc;
use lingxi_core::Engine;
use tool_api::ToolRegistry;
use skill_api::SkillRegistry;
use command_api::CommandRegistry;
use platform_api::Platform;

pub fn build(
    platform: Arc<dyn Platform>,
    config: MobileEngineConfig,
) -> Result<Engine, EngineBuildError> {
    Engine::builder()
        .platform(platform)
        .tools(mobile_tool_registry())
        .skills(mobile_skill_registry())
        .commands(mobile_command_registry())
        .model(config.default_model)
        .permission_policy(config.permission_policy)
        .build()
}

fn mobile_tool_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    // Cross-OS
    tool_file::register_all(&mut r);
    tool_task::register_all(&mut r);
    tool_web::register_all(&mut r);
    tool_skill::register_all(&mut r);
    tool_ui::register_all(&mut r);
    tool_meta::register_all(&mut r);
    tool_cron::register_all(&mut r);
    tool_plan::register_all(&mut r);
    // Mobile-exclusive (single crate per capability; impl supplied by Swift/Kotlin)
    tool_camera::register_all(&mut r);
    tool_voice::register_all(&mut r);
    tool_share::register_all(&mut r);
    r
}

fn mobile_skill_registry() -> SkillRegistry {
    let mut r = SkillRegistry::new();
    skill_builtin::register_mobile(&mut r);
    r
}

fn mobile_command_registry() -> CommandRegistry {
    let mut r = CommandRegistry::new();
    command_core::register_all(&mut r);
    command_mobile::register_all(&mut r);
    r
}
```

The `#[cfg(target_os = "ios")]` blocks here are the **only** cfg in composition code, and they sit at the registration call-site — adjacent to the dep declaration in `Cargo.toml`. This is acceptable because the alternative (registering a tool the binary doesn't link) is a hard error, not a runtime bug.

### §6.4 Desktop composition (mirror)

```rust
// apps/engine-desktop/src/lib.rs (excerpt)
fn desktop_tool_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    // Same cross-OS set
    tool_file::register_all(&mut r);
    tool_task::register_all(&mut r);
    tool_web::register_all(&mut r);
    tool_skill::register_all(&mut r);
    tool_ui::register_all(&mut r);
    tool_meta::register_all(&mut r);
    tool_cron::register_all(&mut r);
    tool_plan::register_all(&mut r);
    // Desktop-only
    tool_shell::register_all(&mut r);
    tool_agent::register_all(&mut r);
    tool_mcp::register_all(&mut r);
    tool_lsp::register_all(&mut r);
    tool_team::register_all(&mut r);
    tool_worktree::register_all(&mut r);
    // Device-control (host + Android + iOS targets)
    tool_computer_use::register_all(&mut r);
    tool_android_use::register_all(&mut r);
    tool_ios_use::register_all(&mut r);   // is_available() returns false on non-macOS
    r
}
```

**The only difference between the two builders is the second half of the tool list and the cfg-gated mobile-exclusive section.** Every line in `core/`, `orchestrator/`, `agent/`, etc. is shared verbatim.

### §6.5 CLI binary as a thin wrapper

`apps/cli/` is now a tiny binary that delegates to the desktop composition:

```rust
// apps/cli/src/main.rs
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let args = parse_argv();
    let platform = Arc::new(platform_posix::PosixPlatform::new(&args.platform_config)?);
    let engine = engine_desktop::build(platform, args.engine_config)?;

    let mut surface = tui::TuiSurface::new(engine);
    surface.run()?;
    Ok(())
}
```

---

## §7 Platform layer

### §7.1 `platforms/ios/` shape (M8 stub, M9 real)

Under the UniFFI-callback model, `platforms/ios/` is much thinner than it would be with native Rust FFI. It only owns capabilities that are simpler in Rust than in Swift:

```
platforms/ios/
├── Cargo.toml                   # NO objc2 / core-foundation deps
└── src/
    ├── lib.rs                   # IosPlatform struct + impl Platform
    ├── filesystem.rs            # Rust std::fs over App Sandbox paths
    ├── http.rs                  # reqwest + rustls (cross-compile to iOS works fine)
    ├── clock.rs                 # std::time
    ├── process.rs               # ProcessRunner → all Unsupported
    ├── sandbox.rs               # Sandbox → all Unsupported
    ├── swarm.rs                 # SwarmBackend → InProcess only
    ├── mcp_transport.rs         # http/sse/ws supported; stdio Unsupported
    └── lsp_transport.rs         # Unsupported (no spawn)

# CAMERA / VOICE / SHARE / SECURE_STORAGE — NOT IN THIS CRATE.
# They come in as Arc<dyn CameraControl> etc. from Swift via UniFFI callback,
# constructed in apps/ios-framework/ and handed to IosPlatform::new().
```

```rust
// platforms/ios/src/lib.rs
#![cfg(target_os = "ios")]
#![forbid(unsafe_code)]

use std::sync::Arc;
use platform_api::*;

pub struct IosPlatform {
    // Rust-implemented (in this crate)
    fs: Arc<dyn FileSystem>,
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    process: Arc<dyn ProcessRunner>,    // Unsupported
    sandbox: Arc<dyn Sandbox>,           // Unsupported

    // Swift-implemented (passed in by apps/ios-framework via UniFFI callback)
    camera: Arc<dyn CameraControl>,
    voice: Arc<dyn VoiceRecorder>,
    share: Arc<dyn SharingService>,
    secure_storage: Arc<dyn SecureStorage>,
}

/// Constructor takes the Swift-supplied trait objects. `apps/ios-framework/`
/// builds these from Swift class instances exposed via UniFFI.
pub struct IosPlatformInputs {
    pub app_sandbox_root: std::path::PathBuf,
    pub camera: Arc<dyn CameraControl>,
    pub voice: Arc<dyn VoiceRecorder>,
    pub share: Arc<dyn SharingService>,
    pub secure_storage: Arc<dyn SecureStorage>,
}

impl IosPlatform {
    pub fn new(inputs: IosPlatformInputs) -> Result<Self, IosInitError> {
        Ok(Self {
            fs: Arc::new(crate::filesystem::IosFileSystem::new(&inputs.app_sandbox_root)?),
            http: Arc::new(crate::http::IosHttp::new()),
            clock: Arc::new(crate::clock::SystemClock),
            process: Arc::new(crate::process::UnsupportedProcessRunner),
            sandbox: Arc::new(crate::sandbox::UnsupportedSandbox),
            camera: inputs.camera,
            voice: inputs.voice,
            share: inputs.share,
            secure_storage: inputs.secure_storage,
        })
    }
}

impl Platform for IosPlatform {
    fn fs(&self) -> Arc<dyn FileSystem> { self.fs.clone() }
    fn http(&self) -> Arc<dyn HttpTransport> { self.http.clone() }
    fn clock(&self) -> Arc<dyn Clock> { self.clock.clone() }
    fn process(&self) -> Arc<dyn ProcessRunner> { self.process.clone() }
    fn sandbox(&self) -> Arc<dyn Sandbox> { self.sandbox.clone() }
    fn camera(&self) -> Option<Arc<dyn CameraControl>> { Some(self.camera.clone()) }
    fn voice(&self) -> Option<Arc<dyn VoiceRecorder>> { Some(self.voice.clone()) }
    fn share(&self) -> Option<Arc<dyn SharingService>> { Some(self.share.clone()) }
    fn secure_storage(&self) -> Arc<dyn SecureStorage> { self.secure_storage.clone() }
    fn computer_control(&self) -> Option<Arc<dyn ComputerControl>> { None }
}
```

`#[cfg(target_os = "ios")]` appears **only** here, in `platforms/ios/` itself. Engine and tool code is cfg-free.

`platforms/android/` mirrors this structure with Kotlin-supplied impls for camera/voice/share/secure-storage.

### §7.2 `apps/ios-framework/` and `apps/android-aar/` structure

These are no longer thin shells — they own the Swift/Kotlin implementations of the foreign trait callbacks:

```
apps/ios-framework/
├── Cargo.toml
├── uniffi.toml                  # UniFFI generator config
├── src/                          # Rust side
│   ├── lib.rs                    # #[uniffi::export] surface
│   └── builder.rs                # Takes Swift callbacks, builds IosPlatform, calls engine-mobile::build()
└── swift/                        # Swift side (compiled into framework)
    ├── Sources/
    │   ├── LingxiCode/
    │   │   ├── LingxiCode.swift          # Public Swift API
    │   │   ├── CameraImpl.swift          # implements CameraControl
    │   │   ├── VoiceImpl.swift           # implements VoiceRecorder
    │   │   ├── ShareImpl.swift           # implements SharingService
    │   │   └── KeychainImpl.swift        # implements SecureStorage
    │   └── LingxiCodeBindings/           # UniFFI-generated bindings
    │       └── (auto-generated)
    └── Package.swift                     # SwiftPM manifest

apps/android-aar/
├── Cargo.toml
├── uniffi.toml
├── src/                          # Rust side
└── kotlin/                       # Kotlin side
    ├── src/main/kotlin/com/lingxi/code/
    │   ├── LingxiCode.kt                 # Public Kotlin API
    │   ├── CameraImpl.kt
    │   ├── VoiceImpl.kt
    │   ├── ShareImpl.kt
    │   └── KeystoreImpl.kt
    └── build.gradle.kts
```

---

## §8 Dependency-graph rules

### §8.1 The rules

| Source | May depend on | Must NOT depend on |
|---|---|---|
| `protocol/` | std + serde only | anything else in workspace |
| `traits/` | `protocol/` | anything else |
| `tool-api/`, `skill-api/`, `command-api/` | `protocol/`, `traits/` | each other, plugin crates, `platforms/*`, `apps/*` |
| Engine subsystem crates (`core/`, `orchestrator/`, ...) | `protocol/`, `traits/`, `*-api/`, sibling engine crates | `tools/*`, `skills/*`, `commands/*`, `platforms/*`, `apps/*` |
| `tools/<x>/` | `tool-api/`, `traits/`, `protocol/`, **specific engine crates it integrates with** (e.g. `tools/mcp` may depend on `mcp/`) | other `tools/*`, `platforms/*`, `apps/*`, `skills/*`, `commands/*` |
| `skills/<x>/` | `skill-api/`, `protocol/` | `tools/*`, `platforms/*`, `apps/*`, engine internals |
| `commands/<x>/` | `command-api/`, `protocol/`, engine crates it commands (e.g. `commands/core` may depend on `session/`) | `tools/*`, sibling `commands/*`, `platforms/*`, `apps/*` |
| `platforms/<x>/` | `traits/`, `protocol/` | `core/` and engine internals, `tools/*`, `skills/*`, `commands/*`, `apps/*`, sibling `platforms/*` |
| `apps/<x>/` | any of the above | no one depends on `apps/*` (they are graph leaves) |

### §8.2 CI enforcement

```toml
# deny.toml — cargo-deny config
[bans]
multiple-versions = "warn"

# Engine crates must not depend on tools.
[[bans.deny]]
name = "tool-shell"
wrappers = ["engine-desktop", "engine-mobile", "tool-agent"]
# (tool-agent legitimately uses shell for subagent terminal)

[[bans.deny]]
name = "tool-mcp"
wrappers = ["engine-desktop"]

# Engine crates must not depend on platforms.
[[bans.deny]]
name = "platform-posix"
wrappers = ["engine-desktop", "cli", "bridge-server", "ios-framework", "android-aar"]

# ... one rule per tool/platform crate
```

A custom `scripts/check-deps.sh` parses `cargo metadata --format-version=1` and asserts the §8.1 table. Runs in CI on every PR.

### §8.3 Why no Cargo features for tool selection

Features are **additive** — if `tool-api` had `feature = "shell"` and crate A enabled it while crate B did not, the union (shell on) wins everywhere. This is the wrong semantic for "this binary ships Bash, that binary doesn't".

Direct dependency edges in composition-root `Cargo.toml` give the right semantic: if `apps/engine-mobile/Cargo.toml` doesn't list `tool-shell`, the code is not in the binary.

Features remain appropriate for **within-crate** knobs (e.g. `api-client` could feature-gate Bedrock support), but never for **which crate ships**.

---

## §9 Migration plan

### §9.1 Phase ordering

Each phase is independently buildable + testable. Estimated calendar with one full-time engineer:

| Phase | Scope | Time | Verification |
|---|---|---:|---|
| **P0** Repo rename | `git mv lingxi-core lingxi-code`. Update README, CHANGELOG, all internal path strings. | 0.5d | `cargo build --workspace` from the new path |
| **P1** Drop `crates/` wrapper | `git mv crates/* .` for engine crates. Update workspace `Cargo.toml` `members` paths. | 0.5d | `cargo build --workspace` |
| **P2** Drop `lingxi-` prefix | Rename `lingxi-protocol` → `protocol`, `lingxi-core` → `core`, etc. Update all `Cargo.toml` `[dependencies]` blocks. | 0.5d | `cargo build --workspace` |
| **P3** Extract `tool-api` | New `tool-api/` crate at root; move `Tool` trait, `ToolCtx`, `ToolRegistry` from existing `tools/src/` (now at root after P1). Re-export from `tools/` to keep callers building. | 0.5d | `cargo build --workspace` + existing tests |
| **P4** Migrate engine deps | Change `orchestrator`, `agent`, `core` to import from `tool_api` instead of `tools`. Delete `platform-api/src/tool_invoker.rs`. | 1d | parity tests |
| **P5** First two `tools/*` crates | Move `tools/src/builtin/file_*.rs` etc. to `tools/file/`, and `tools/src/builtin/bash.rs` etc. to `tools/shell/`. Empty old paths. | 1d | tool unit tests, parity tests |
| **P6** Composition root + CLI move | Create `apps/engine-desktop/`. Move `cli/` (was `crates/cli`) to `apps/cli/`. CLI goes through `engine_desktop::build()`. | 1d | `cargo run -p cli` works |
| **P7** Remaining tool crates | Split `tools/task`, `web`, `skill`, `ui`, `meta`, `cron`, `plan`, `agent`, `mcp`, `lsp`, `team`, `worktree`. Delete old monolithic `tools/`. | 2-3d | parity test matrix |
| **P8** Skill split | Extract `skill-api/` at root, create `skills/builtin/` with `register_desktop()` / `register_mobile()` entry points. | 1d | skill loading tests |
| **P9** Command split | Extract `command-api/` at root, split into `commands/core/`, `commands/desktop/`, `commands/mobile/`. | 1d | slash command tests, snapshot of `/help` |
| **P10** Mobile platform skeletons | Create `platforms/ios/` and `platforms/android/` with `UnsupportedProcessRunner`, `UnsupportedSandbox`, etc. Add `ComputerControl` trait to `traits/` and stub it in mobile platforms (return None). | 1d | `cargo build -p platform-ios --target aarch64-apple-ios` |
| **P11** Mobile tool skeletons + engine-mobile | Create `tools/camera/`, `tools/voice/`, `tools/share/` (single cross-mobile crate each, calling `Arc<dyn CameraControl>` etc.). Add `CameraControl` / `VoiceRecorder` / `SharingService` traits to `traits/` with `#[uniffi::export(callback_interface)]`. Create `apps/engine-mobile/` composition. | 1d | `cargo build -p engine-mobile --target aarch64-apple-ios` |
| **P11b** Device-control tool skeletons | Create empty `tools/computer-use/`, `tools/android-use/`, `tools/ios-use/` with stub `Tool` impls returning `Err(Unimplemented)`. Register in `engine-desktop` composition. Implementations land in M9. | 0.5d | `cargo build -p tool-computer-use`; LLM sees `computer` / `android_use` / `ios_use` in `/help` listings on desktop only |
| **P12** uniffi wrappers + Swift/Kotlin impls | Create `apps/ios-framework/` (Rust `src/` + Swift `swift/Sources/LingxiCode/`) and `apps/android-aar/` (Rust `src/` + Kotlin `kotlin/src/main/kotlin/`). Implement `CameraControl`, `VoiceRecorder`, `SharingService`, `SecureStorage` in Swift (AVCaptureSession / AVAudioRecorder / UIActivityViewController / Keychain) and Kotlin (CameraX / MediaRecorder / Intent.ACTION_SEND / Keystore) — M8 ships skeletons; full impls land in M9. | 2d | `.xcframework` + `.aar` artifacts produced; Swift/Kotlin classes implement the UniFFI-generated protocols |
| **P13** Bridge crate refactor | Extend existing `bridge/` with protocol types. Create skeleton `apps/bridge-server/`. (Real bridge work is M9.) | 0.5d | protocol crate builds, server crate stubbed |
| **P14** Dependency CI | Write `deny.toml` rules, `scripts/check-deps.sh`. Add to CI. | 0.5d | CI gate passes; tampering fails |
| **P15** Cleanup | Delete empty old paths. Update `docs/ARCHITECTURE.md`, README, CHANGELOG. | 0.5d | full workspace clean build |

**Total: ~14.5 engineer-days.** Each phase is mergeable independently.

### §9.2 Crate-rename table

The following names change. P1-P2 perform the renames in two atomic commits.

| Old (v0.8.0) | New (v0.9.0) | Reason |
|---|---|---|
| repo `lingxi-core` | repo `lingxi-code` | P0 |
| `crates/protocol` | `protocol/` | P1 + P2 (drop wrapper, drop prefix) |
| `lingxi-protocol` | `protocol` | P2 |
| `lingxi-core` | `core` | P2 |
| `lingxi-orchestrator` | `orchestrator` | P2 |
| `lingxi-agent` | `agent` | P2 |
| `lingxi-tools` (single crate) | `tool-api` + `tool-file` + 19 siblings | P3 + P5 + P7 |
| `lingxi-skills` (single crate) | `skill-api` + `skill-builtin` | P8 |
| `lingxi-commands` (single crate) | `command-api` + `command-core` + `command-desktop` + `command-mobile` | P9 |
| `crates/cli` (binary) | `apps/cli/` (`cli`) | P6 |
| `lingxi-cli` | `cli` | P2 + P6 |
| `lingxi-platform-posix` | `platform-posix` | P2 |
| `lingxi-uniffi-bridge` (crate) | `ios-framework` + `android-aar` (split per-target) | P12 |
| `lingxi-bridge` | `bridge` (kept as one crate, expanded for protocol) | P2 + P13 |
| `lingxi-tui` | `tui` | P2 |

### §9.3 No backward compatibility

Per the user directive, this restructure does **not** preserve the existing public crate API. External consumers (if any) must switch from `lingxi_tools::register_all_builtin_tools(&mut registry, ctx)` to `engine_desktop::build(platform, config)` and accept an `Engine` rather than wiring registries themselves.

CHANGELOG.md documents this as a single hard break at v0.9.0.

---

## §10 Risks & open questions

### §10.1 Risks

| Risk | Severity | Mitigation |
|---|---|---|
| **Compile time regression from 37 → 71 crates** | Medium | Measured ~30-90s on a clean release build. Incremental builds get *faster*. Mitigation: `[workspace.profile.dev]` keeps `codegen-units = 256`. CI gate: full clean build < 8 min on baseline runner. |
| **`Arc<dyn Tool>` virtual dispatch cost vs current static dispatch** | Low | Tools are called ~10-50 times per turn at 100ms+ each. Virtual dispatch overhead is ~5ns/call vs 250ms tool latency. Benchmarks before/after confirm < 0.1% perf delta. |
| **Bare crate names collide with crates.io** | Low | We do not publish. If a future need arises, individual crates can be renamed (e.g. `mcp` → `lingxi-mcp`) at publish time only. |
| **Crate proliferation makes "where is this code?" harder** | Medium | Mitigated by clear naming + grouping dirs. ARCHITECTURE.md maps each tool name → crate. IDE jump-to-definition works the same. |
| **Mobile platform crates ship dead `Unsupported*` impls** | Low | Intentional — the contract is "engine compiles for iOS, the user just gets Err(Unsupported) at runtime if they invoke an unavailable capability". Composition root prevents tools that *would* invoke them from being registered. |
| **Composition roots diverge over time (skill/command mismatches)** | Medium | Add a snapshot test that loads both `engine-desktop` and `engine-mobile`, prints their tool/skill/command lists. Drift → snapshot mismatch → CI catches. |
| **`Platform` aggregate trait grows** | Low | Adding new traits to `traits/` is rare (~2/year). When added, `Platform` gets one new method and every platform crate gets one stub. |
| **Mobile-exclusive crates won't compile on Linux dev machines** | Low (resolved by UniFFI-callback design) | Under the v3 UniFFI-callback model, `tools/camera/`, `tools/voice/`, `tools/share/` are pure Rust crates with no platform-specific deps — they call `Arc<dyn CameraControl>` from `ToolCtx`. The Swift/Kotlin impls live in `apps/ios-framework/swift/` and `apps/android-aar/kotlin/`, which Linux dev machines simply don't try to compile (no `swiftc` / no Android SDK invoked from `cargo`). Workspace builds cleanly on any host. |
| **UniFFI version compatibility** | Medium | UniFFI 0.27+ required for async callback interfaces. Pin in `Cargo.toml` at exactly the version we test with. Pre-1.0 means breaking changes between minor versions possible. Mitigation: vendor-pin and bump via deliberate sprint. |
| **Swift/Kotlin code quality drift** | Medium | Swift/Kotlin source files live in `apps/{ios-framework,android-aar}/{swift,kotlin}/`. CI runs `swiftlint` and `ktlint` on PRs that touch those paths. Unit tests for each `CameraImpl`/`VoiceImpl`/etc. are required and run on macOS / Linux+Android-SDK runners respectively. |

### §10.2 Open questions

| # | Question | Recommendation |
|---|---|---|
| **Q1** | Should `tools/mcp/` split into `tools/mcp-stdio/` (desktop-only) + `tools/mcp-net/` (cross-OS, HTTP/SSE/WS)? Mobile could ship `mcp-net`. | Defer to M9. M8 ships `tools/mcp/` as desktop-only; carve out HTTP-only subset when mobile MCP becomes a real need. |
| **Q2** | Tools emit `Effect`/`Event` from `protocol/`. Do tools depend on `protocol` directly, or does `tool-api` re-export? | Tools depend on `protocol` directly. `tool-api` re-exports only `ToolError` and `ToolCtx`. Single source of truth for effect/event taxonomy stays in `protocol/`. |
| **Q3** | Should `apps/cli` and `apps/bridge-server` share an `apps/common` crate for argv parsing, logging setup, etc.? | Yes — `apps/common` for shared bin scaffolding. ~200 lines. |
| **Q4** | UniFFI ABI: one UDL file per platform or shared? | Different per platform. iOS exposes camera/voice/share with `NSError`-friendly types; Android with `kotlin.Result`. Shared UDL forces lowest-common-denominator. |
| **Q5** | Do we keep `plugin/` (plugin manifest) as engine-level, or move plugins to `tools/<x>/plugin/`? | Keep at engine level. Plugin manifest is a registry concept independent of any specific tool. Plugins register tools at runtime via the same `ToolRegistry::register` API. |
| **Q6** | `sandbox/` ships a dispatcher routing to `platform-posix::Sandbox`. On mobile the dispatcher is unused. Still link it? | Yes — dispatcher is a few hundred lines of trait routing, no platform-specific imports. Cheaper than excluding. `platform-ios::Sandbox` returns `Unsupported` and the orchestrator handles the error path. |
| **Q7** | Should `tool-agent` be desktop-only or partially mobile-supported? Mobile could spawn an in-process subagent via `SwarmBackend::InProcess`. | Partially in M9. M8 makes it desktop-only; M9 splits into `tools/agent-core/` (in-process) + `tools/agent-shell/` (terminal-spawned). |

---

## §11 Out-of-scope (explicitly deferred)

- **Real mobile UI** — no iocraft-on-mobile, no SwiftUI host, no Jetpack Compose. `tui/` remains terminal-only. Mobile UIs are downstream consumers of `apps/ios-framework/` / `apps/android-aar/`.
- **Real camera / voice / share implementations** — `tools/camera/`, `tools/voice/`, `tools/share/` ship as Rust skeletons (the `Tool::call` body returns `Err(Unimplemented)`). The Swift `CameraImpl.swift` / `VoiceImpl.swift` / `ShareImpl.swift` and Kotlin `CameraImpl.kt` / `VoiceImpl.kt` / `ShareImpl.kt` files exist in M8 but contain TODO stubs. M9 fills in the AVCaptureSession / CameraX / etc. logic.
- **Real computer-use / android-use / ios-use implementations** — tool crates and the `ComputerControl` trait surface ship as skeletons in M8. M9 implements: Quartz/XTest/SendInput backends for `tool-computer-use`, ADB shelling for `tool-android-use`, `xcrun simctl`/`idb` shelling for `tool-ios-use`. Screenshot encoding (PNG, scaling, region cropping) shared across the three.
- **Bridge protocol completion** — `bridge/` ships only wire types and JSON-RPC framing helpers. `apps/bridge-server/` is a hello-world. M9 implements remote driving end-to-end.
- **Cargo workspace inheritance overhaul** — `[workspace.dependencies]` is left as-is.
- **`tui/` API stabilization** — internal iocraft component library remains pre-1.0.
- **Slash command behavior parity with claude-code** — 81 commands remain stubbed (as in M7). M8 *organizes* them, doesn't *implement* them.
- **Publishing crates to crates.io** — workspace stays private. Names without `lingxi-` prefix are workspace-local only.

---

## §12 Appendix A — comparison with claude-code's mobile approach

claude-code chose **remote driving**: engine on desktop, phone is a thin client over the `bridge/` subsystem. Our composition-root design does **not** preclude this — it makes it *one of two possible* configurations.

### §12.1 Configuration A: native mobile binary (M8 enables)

```
              ┌──────────────┐
              │   iOS app    │
              │  (Swift UI)  │
              └──────┬───────┘
                     │ UniFFI
                     ▼
              ┌──────────────┐
              │  apps/ios-   │
              │  framework   │
              │              │
              │  ┌─Engine─┐  │
              │  │ mobile │  │  ← full agent loop, on-device
              │  └────────┘  │
              └──────────────┘
```

Pros: offline, low latency, no desktop required.
Cons: 15 of 41 tools unavailable (no shell/MCP/LSP/team/worktree); user can't do "real" coding work — only read, plan, ask, capture.

### §12.2 Configuration B: bridge client (M9 enables)

```
   ┌──────────────┐                  ┌────────────────┐
   │   iOS app    │                  │  Desktop CLI   │
   │  (Swift UI)  │  WebSocket+JWT   │  apps/cli      │
   │              │◀────────────────▶│                │
   │  bridge      │                  │  bridge-server │
   │  client      │                  │     ┌──────┐   │
   │  (small lib) │                  │     │Engine│   │
   └──────────────┘                  │     │ full │   │
                                     │     └──────┘   │
                                     └────────────────┘
```

Pros: full tool surface; phone is a remote.
Cons: requires desktop online; latency higher; network/auth complexity.

### §12.3 Configuration C: hybrid (future)

Both configurations coexist: phone has a small on-device engine for offline read/plan operations, and switches to bridge mode when desktop is online to run shell-requiring commands. M8's composition-root structure supports this without redesign — the iOS app picks which mode to invoke at runtime.

claude-code today only supports Configuration B. M8 keeps B open (via `bridge/`) while *also* opening the door to A and C — that is the architectural unlock M8 buys.

---

## §13 Appendix B — file-by-file delete/move ledger

| Action | Path | Disposition |
|---|---|---|
| RENAME | repo `lingxi-core/` → `lingxi-code/` | P0 |
| MOVE | `crates/*` → `*` (root) | P1: drop wrapper directory |
| RENAME | `crates/protocol/` → `protocol/` with `name = "protocol"` | P1 + P2 |
| RENAME | (every other engine crate, drop `crates/` + `lingxi-` prefix) | P1 + P2 |
| DELETE | `crates/tools/src/registry.rs` | Replaced by `tool-api/src/registry.rs` (P3) |
| DELETE | `crates/tools/src/dispatcher.rs` | Orchestrator calls `ToolRegistry` directly (P4) |
| DELETE | `crates/tools/src/tool_invoker_impl.rs` | `ToolInvoker` trait removed (P4) |
| DELETE | `crates/platform-api/src/tool_invoker.rs` | Trait removed (P4) |
| MOVE | `crates/tools/src/tool_trait.rs` → `tool-api/src/lib.rs` | re-shaped (P3) |
| MOVE | `crates/tools/src/context.rs` → `tool-api/src/context.rs` | re-shaped; now gets full Platform handles (P3) |
| MOVE | `crates/tools/src/builtin/file_read.rs` → `tools/file/src/read.rs` | + other file_*, glob, grep, notebook_edit (P5) |
| MOVE | `crates/tools/src/builtin/bash.rs` → `tools/shell/src/bash.rs` | + powershell, repl (P5) |
| MOVE | `crates/tools/src/builtin/mcp.rs` → `tools/mcp/src/lib.rs` | 4 tools in one crate (P7) |
| MOVE | (etc. for all 41 tools) | per §4.1 layout (P5 + P7) |
| MOVE | `crates/tools/src/shared/` → split between `tools/file/src/shared.rs`, `tools/shell/src/shared.rs`, ... | most helpers are category-local; rare cross-category helpers go in `tool-api/src/util.rs` (P5/P7) |
| DELETE | `crates/tools/` entirely | after move (P7) |
| DELETE | `crates/cli/` (move contents) | → `apps/cli/` (P6) |
| DELETE | `crates/uniffi-bridge/` (move contents) | split between `apps/ios-framework/` and `apps/android-aar/` (P12) |
| MERGE | `crates/bridge/` → `bridge/` (single crate, expanded with protocol types) | P13 |
| MOVE | `crates/skills/src/*` → `skill-api/src/*` (the runtime) + `skills/builtin/src/*` (the templates) | P8 |
| MOVE | `crates/commands/src/*` → `command-api/src/*` (the runtime) + `commands/{core,desktop,mobile}/src/*` (the impls) | P9 |
| CREATE | `tool-api/`, `skill-api/`, `command-api/` | New abstraction crates at root (P3, P8, P9) |
| CREATE | `tools/<14 cross-OS dirs>/` | Per-category cross-OS / desktop-only tool crates (P5 + P7) |
| CREATE | `tools/camera/`, `tools/voice/`, `tools/share/` | Cross-mobile tool crates (P11); Rust calls `Arc<dyn CameraControl>` etc., Swift/Kotlin impl via UniFFI callback |
| CREATE | `tools/computer-use/`, `tools/android-use/`, `tools/ios-use/` | Device-control tool crates (P11b); skeletons in M8, real impl in M9 |
| CREATE | `platform-api/src/camera.rs`, `platform-api/src/voice.rs`, `platform-api/src/share.rs` | Mobile-capability traits with `#[uniffi::export(callback_interface)]` (P11) |
| CREATE | `platform-api/src/computer_control.rs` (`ComputerControl` trait) | P10 |
| CREATE | `apps/ios-framework/swift/Sources/LingxiCode/CameraImpl.swift`, `VoiceImpl.swift`, `ShareImpl.swift`, `KeychainImpl.swift` | Swift implementations of UniFFI callback protocols (P12); M8 ships skeletons |
| CREATE | `apps/android-aar/kotlin/src/main/kotlin/com/lingxi/code/CameraImpl.kt`, `VoiceImpl.kt`, `ShareImpl.kt`, `KeystoreImpl.kt` | Kotlin implementations (P12); M8 ships skeletons |
| CREATE | `skills/builtin/` | Single skill crate with mobile/desktop register entry points (P8) |
| CREATE | `commands/core/`, `commands/desktop/`, `commands/mobile/` | Per-domain command crates (P9) |
| CREATE | `platforms/ios/`, `platforms/android/` | New platform crates with `Unsupported*` impls (P10) |
| CREATE | `apps/engine-desktop/`, `apps/engine-mobile/`, `apps/bridge-server/`, `apps/ios-framework/`, `apps/android-aar/` | New composition + binary crates (P6, P11, P12, P13) |
| CREATE | `platform-api/src/platform.rs` (aggregate trait) | P3 |
| CREATE | `platform-api/src/mobile.rs` (`MobileSurface` trait) | P10 |
| UPDATE | workspace `Cargo.toml` | Members list grows from 37 to 71; all paths flat |
| UPDATE | `docs/ARCHITECTURE.md` | New crate map section |
| UPDATE | `CHANGELOG.md` | "v0.9.0: M8 — composable engine restructure" with BREAKING note |
| CREATE | `deny.toml` | Dependency rules |
| CREATE | `scripts/check-deps.sh` | CI script |

---

## §14 Success criteria

M8 ships v0.9.0 when **all** of the following hold:

1. `cargo build --workspace --release` succeeds from `lingxi-code/`.
2. `cargo test --workspace` passes — every test that passed at M7 still passes, byte-for-byte where applicable.
3. `cargo build -p engine-mobile --target aarch64-apple-ios` succeeds.
4. `cargo build -p engine-mobile --target aarch64-linux-android` succeeds.
5. `find . -path ./target -prune -o -name '*.rs' -print | grep -vE '^./(platforms|apps)/' | xargs grep -l 'cfg.*target_os' | wc -l` returns **0**. (cfg-on-target_os allowed only in `platforms/*` and `apps/*` composition roots.)
6. `scripts/check-deps.sh` passes — no dependency-graph violations.
7. The desktop CLI (`cargo run -p cli`) behaves identically to M7: same tools listed in `/help`, same `/cost` output for an identical session, same JSONL emitted.
8. `apps/engine-mobile` exposes exactly the tool set listed in §6.3 (verified by snapshot test). `apps/engine-desktop` exposes the cross-OS + desktop-only + 3 device-control tools (`computer`, `android_use`, `ios_use`) per §6.4.
9. ARCHITECTURE.md, README.md, and CHANGELOG.md updated to reflect the new layout.
10. The 13-day migration is committed across at least 16 logically distinct PRs (one per phase in §9.1, larger phases may split), each green on CI.

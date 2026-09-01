# LingXi Core M1 · Plan 17 · Tests + Polish + Release

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Final polish. Round out the test harness with contract tests for all 13 traits, property tests at 10K iterations, parity fixtures derived from claude-code reference scenarios, documentation (rustdoc + ARCHITECTURE.md + SECURITY.md + CHANGELOG + README), and tag M1 v0.1.0.

**Depends on:** Plans 01-16. Everything must already be passing under default proptest cases.

---

## File Structure

```
crates/test-harness/
├── src/
│   ├── contracts/{filesystem, process, http, mcp, worktree, swarm, secure_storage, sandbox, lsp, bridge, runtime, clock, hook_broadcaster}.rs
│   ├── properties/{state_machine, compaction, mcp, tools, hooks, permission, secret, cost, plugin, agent, session}.rs
│   └── parity/{tools, hooks, subagents, tasks, compaction}.rs
├── tests/
│   ├── contract_filesystem.rs
│   ├── ... (one per trait — drives MockX impls through the contract suite)
│   ├── parity_tools.rs
│   ├── parity_hooks.rs
│   └── ...
└── benches/contract_coverage.rs

docs/
├── ARCHITECTURE.md
├── SECURITY.md
└── README.md (workspace-level)

CHANGELOG.md
.github/workflows/ci.yml          ← MODIFY: add 10K-iteration profile + contract coverage gate
```

---

## Task 1: Contract test suites — one per trait

For each trait, create a `contracts/<trait>.rs` module exposing a public `<trait>_contract_tests<T: TheTrait>(impl: &T)` function. Tests in `tests/contract_*.rs` instantiate each Mock implementation and call the suite.

Example: `contracts/filesystem.rs`

```rust
use lingxi_platform_api::{FileContent, FileSystem};
use std::sync::Arc;

pub async fn filesystem_contract_tests<F: FileSystem>(fs: &F) {
    test_write_then_read(fs).await;
    test_read_nonexistent_returns_error(fs).await;
    test_workspace_boundary_enforced(fs).await;
    test_append_then_read_concatenates(fs).await;
    test_truncate_shrinks_file(fs).await;
    test_file_mtime_advances_after_write(fs).await;
    test_flock_excludes_concurrent(fs).await;
}

async fn test_write_then_read<F: FileSystem>(fs: &F) {
    let path = format!("/tmp/contract-{}.txt", uuid::Uuid::new_v4());
    fs.write_file(&path, "hi").await.unwrap();
    let content = fs.read_file(&path, None, None).await.unwrap();
    assert_eq!(content.content, "hi");
    fs.delete_file(&path).await.ok();
}

async fn test_read_nonexistent_returns_error<F: FileSystem>(fs: &F) {
    let r = fs.read_file("/tmp/__never_exists_zzz", None, None).await;
    assert!(r.is_err());
}

async fn test_workspace_boundary_enforced<F: FileSystem>(fs: &F) {
    // Implementation MUST refuse paths above /tmp for the mock workspace.
    assert!(!fs.is_within_workspace("/etc/passwd"));
}

// ... 4 more cases inline. Each is < 10 lines.

async fn test_append_then_read_concatenates<F: FileSystem>(_fs: &F) { /* ... */ }
async fn test_truncate_shrinks_file<F: FileSystem>(_fs: &F) { /* ... */ }
async fn test_file_mtime_advances_after_write<F: FileSystem>(_fs: &F) { /* ... */ }
async fn test_flock_excludes_concurrent<F: FileSystem>(_fs: &F) { /* ... */ }
```

Then `tests/contract_filesystem.rs`:

```rust
use lingxi_platform_posix_minimal::PosixFileSystem;
use lingxi_test_harness::contracts::filesystem::filesystem_contract_tests;

#[tokio::test]
async fn posix_filesystem_passes_contract() {
    let fs = PosixFileSystem::new(std::env::temp_dir());
    filesystem_contract_tests(&fs).await;
}
```

Repeat for: `process`, `http`, `mcp`, `worktree`, `swarm`, `secure_storage`, `sandbox`, `lsp`, `bridge`, `runtime`, `clock`, `hook_broadcaster`.

Commit:
```bash
cargo test -p lingxi-test-harness --test 'contract_*'
git add crates/test-harness
git commit -m "test(contracts): suites for all 13 traits"
```

---

## Task 2: Property tests at 10K iterations

Create a `10k-iterations` feature flag that bumps proptest config.

```rust
// crates/test-harness/src/properties/state_machine.rs
use proptest::prelude::*;
use proptest::test_runner::Config;

#[cfg(not(feature = "10k-iterations"))]
const CASES: u32 = 256;
#[cfg(feature = "10k-iterations")]
const CASES: u32 = 10_000;

proptest! {
    #![proptest_config(Config { cases: CASES, ..Default::default() })]

    #[test]
    fn reducer_is_total(events in prop::collection::vec(arb_event(), 0..100)) {
        let mut state = lingxi_core::ConversationState::Idle {
            session: lingxi_core::SessionState::empty(lingxi_protocol::SessionId::nil(), "m".into()),
        };
        for e in events {
            let (next, _) = lingxi_core::reduce(state, e);
            state = next;
        }
    }

    #[test]
    fn token_usage_monotonic(events in prop::collection::vec(arb_event(), 0..100)) {
        // Same setup; assert non-decreasing total_tokens().
    }

    #[test]
    fn terminated_is_absorbing(reason in any::<String>(), events in prop::collection::vec(arb_event(), 0..50)) {
        // Once Terminated, all subsequent events leave state == Terminated and effects.is_empty().
    }
}

fn arb_event() -> impl Strategy<Value = lingxi_core::Event> {
    use lingxi_core::Event;
    use lingxi_protocol::{MessageId, RequestId};
    prop_oneof![
        any::<String>().prop_map(|s| Event::UserMessage {
            message_id: MessageId::new(),
            request_id: RequestId::new(),
            content: s,
        }),
        Just(Event::UserInterrupt),
        Just(Event::UserExit),
    ]
}
```

Add similar `properties/*.rs` modules for: compaction (token monotonic, circuit breaker terminates), mcp (state transition validity), tools (partition order preserved), hooks (priority order, ssrf blocks), permission (deny over allow, denial fallback), secret (`Secret<T>::Debug` never leaks), cost (cost monotonic, cache savings non-negative), plugin (install/uninstall roundtrip, blocklist enforced), agent (pool capacity), session (jsonl roundtrip).

Add to `Cargo.toml`:

```toml
[features]
default = []
10k-iterations = []
```

Commit:
```bash
cargo test -p lingxi-test-harness --features 10k-iterations
git add crates/test-harness
git commit -m "test(properties): 10K-iteration suite gated by feature flag"
```

---

## Task 3: Parity fixtures from claude-code

Materialize the 12 mock parity scenarios from claw-code's reference:
- `streaming_text`
- `read_file_roundtrip`
- `grep_chunk_assembly`
- `write_file_allowed`
- `write_file_denied`
- `multi_tool_turn_roundtrip`
- `bash_stdout_roundtrip`
- `bash_permission_prompt_approved`
- `bash_permission_prompt_denied`
- `plugin_tool_roundtrip`
- `auto_compact_triggered`
- `token_cost_reporting`

Store as JSON fixtures in `crates/test-harness/src/parity/fixtures/` and write one driver test per scenario in `tests/parity_*.rs` that feeds the recorded events into the reducer and asserts the effects match the recording.

Example driver:

```rust
// crates/test-harness/tests/parity_streaming_text.rs
use lingxi_core::{reduce, ConversationState, SessionState};
use lingxi_protocol::SessionId;
use lingxi_test_harness::parity::{load_fixture, ParityFixture};

#[tokio::test]
async fn streaming_text_matches_reference() {
    let fixture: ParityFixture = load_fixture("streaming_text").expect("fixture should load");
    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::new(), "claude-opus-4-6".into()),
    };
    let mut all_effects = Vec::new();
    for ev in fixture.events {
        let (next, effects) = reduce(state, ev);
        state = next;
        all_effects.extend(effects);
    }
    // Assert: same number of RenderStreamDelta effects as reference.
    let deltas = all_effects.iter().filter(|e| matches!(e, lingxi_protocol::Effect::RenderStreamDelta { .. })).count();
    assert_eq!(deltas, fixture.expected_render_delta_count);
}
```

Commit:
```bash
cargo test -p lingxi-test-harness --test 'parity_*'
git add crates/test-harness/src/parity crates/test-harness/tests/parity_*.rs
git commit -m "test(parity): 12 scenarios from claude-code reference"
```

---

## Task 4: Contract coverage metric + CI gate

```rust
// crates/test-harness/src/bin/contract-coverage-checker.rs
//! Walks the trait surface and reports unexercised method ratio.
//! Failure mode: ratio > 0.05 → exit code 1.

fn main() {
    // Trait method counting is reflective. M1 ships a small registry that's
    // updated when a new trait method is added — caught in code review.
    let total_methods = TRAIT_METHODS.len();
    let covered = COVERED.len();
    let unexercised = total_methods - covered;
    let ratio = unexercised as f64 / total_methods as f64;
    println!("contract coverage: {covered}/{total_methods} (unexercised ratio {ratio:.3})");
    if ratio > 0.05 {
        eprintln!("ERROR: unexercised trait method ratio {ratio:.3} > 0.05 threshold");
        std::process::exit(1);
    }
}

const TRAIT_METHODS: &[&str] = &[
    "FileSystem::read_file", "FileSystem::write_file", "FileSystem::append_file",
    "FileSystem::truncate", "FileSystem::file_mtime", "FileSystem::file_size",
    "FileSystem::delete_file", "FileSystem::symlink", "FileSystem::flock_exclusive",
    "FileSystem::fsync", "FileSystem::glob", "FileSystem::grep", "FileSystem::watch",
    // ... full list across all 13 traits
];

const COVERED: &[&str] = &[
    // entries appended automatically when a contract test calls the method.
    // For M1.23, hand-maintained.
    "FileSystem::read_file", "FileSystem::write_file", "FileSystem::append_file",
    "FileSystem::truncate", "FileSystem::file_mtime", "FileSystem::flock_exclusive",
    // ...
];
```

Update `.github/workflows/ci.yml`:

```yaml
  contract-coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.82.0
      - run: cargo run --bin contract-coverage-checker
```

Commit:
```bash
git add crates/test-harness/src/bin .github/workflows/ci.yml
git commit -m "test: contract coverage checker + CI gate (ratio ≤ 0.05)"
```

---

## Task 5: ARCHITECTURE.md

```bash
mkdir -p docs
cat > docs/ARCHITECTURE.md <<'EOF'
# Architecture

LingXi Core is an event-sourced conversation engine split across 30 crates.
This document is a navigation aid; full design lives in
`docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`.

## Crate map

- `protocol` — shared DTOs, IDs, Effect/Event envelopes
- `core` — state machine, reducer, prompt assembly, session model
- `traits` — 13 platform abstraction traits
- `api-client` — Anthropic/OpenAI-compatible API + SSE
- `permission/secret/cost` — security & cost foundations (Plan 02)
- `tools/hooks` — execution + extension (Plan 03)
- `memory/mcp` — retrieval + tool surface (Plan 04)
- `compaction` — 5-layer compactor (Plan 05)
- `agent` — subagent runtime (Plan 06)
- `tasks/coordinator` — background work + multi-agent (Plan 07)
- `sidequery` — side LLM + forked agent infra (Plan 08)
- `skills/commands/outputstyles` — user-facing surface (Plan 09)
- `session/filestate/msgqueue` — persistence + caching (Plan 10)
- `cron` — scheduled tasks (Plan 11)
- `sandbox/lsp` — execution support (Plan 12)
- `telemetry/anthropic-oauth` — infra + main auth (Plan 13)
- `bridge` — IDE integration (Plan 14)
- `plugin` — manifest + 8-registry materialization (Plan 15)
- `uniffi-bridge` — FFI façade (Plan 16)
- `test-harness` — contracts + properties + parity (Plan 17)
- `platforms/posix-minimal` — M1 desktop demo host
- `examples/cli-demo` — M1 end-to-end demo

## Key flows

### Single-turn conversation
User input → `reduce(Idle, UserMessage)` → `Effect::SendApiRequest` →
HttpTransport → SSE stream → `Event::ApiStream*` → `Effect::RenderStreamDelta`
→ `Event::ApiStreamEnd` → `Idle`.

### Tool dispatch
Assistant tool_use block → `ToolUseReceived` → permission check (rules + classifier)
→ PreToolUse hook → tool.call() → PostToolUse hook → `Effect::ExecuteTool` result
→ tool_result message → next API turn.

### Compaction
Token estimate > threshold → orchestrator → micro/cached-micro/collapse →
autocompact via ForkedAgentRunner → PostCompactBuilder → boundary message in transcript.

### Subagent
AgentTool → SubagentContext built (Tools/MCP/Hooks/Memory/Permission inherited)
→ StateMachinePool::allocate → sibling slot runs its own reducer → SubagentEvent
to parent → tool_result.

## Cross-cutting concerns

- **No tokio::spawn outside runtime trait** — all background work via `RuntimeSpawner`.
- **No tokio::fs/std::fs in engine crates** — all I/O via `FileSystem` trait.
- **Secrets never logged** — `Secret<T>` debug is always `<redacted>`.
- **Sandbox is type-enforced** — `ProcessRunner::run` only accepts `SandboxedCommand`.
EOF

git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md navigation aid"
```

---

## Task 6: SECURITY.md

```bash
cat > docs/SECURITY.md <<'EOF'
# Security model

## Secrets
- `Secret<T>` (wrapper around `secrecy::SecretBox<T>`) zeroizes on drop.
- `Debug`/`Display` of `Secret<T>` and `SecureStorageData` always emit `<redacted>`.
- The only access path is `expose_secret()`, which is grep-able for audit.
- `SecureStorage` trait abstracts over Keychain/libsecret/Cred Vault/Keystore/PlainText.

## Sandbox
- `ProcessRunner::run` accepts only `SandboxedCommand`.
- `SandboxedCommand` is only constructible via `Sandbox::prepare` (policy applied)
  or `Sandbox::bypass_with_audit` (reason recorded).
- Sandbox canonicalizes paths and rejects symlink escape (A2).
- `should_use_sandbox` decision logic refuses dangerous bash commands when
  sandbox is unavailable.

## Permission
- 8 rule sources with explicit priority.
- `DenialTrackingState` falls back to prompt after threshold per tool.
- `PermissionResult::Ask` carries an optional `pending_classifier_check` that
  can race the user prompt.
- `bypass_killswitch_active` overrides `BypassPermissions` mode.

## OAuth
- Loopback HTTP listener bound only to 127.0.0.1.
- PKCE S256 code verifier/challenge.
- State token validated on callback (CSRF defense).
- Token storage via `SecureStorage`.

## Plugins
- Default trust for git/local plugins is `Untrusted` (A7).
- Plugin agent frontmatter cannot set `permission_mode`/`hooks`/`mcpServers`.
- Sensitive `user_config` fields resolved through `CredentialManager`, never
  stored in plugin manifest plain text.

## Hooks
- SSRF guard blocks RFC1918/loopback IPs by default.
- Hook `Command` executor on mobile is refused at registration time (M3 gap).

## IDE bridge
- 8-char alphanumeric pairing codes excluding visually confusable characters.
- Pairing rate-limited per project (token-bucket).
- JWT tokens are project-scoped: a token issued for project A cannot
  authenticate against project B.

## Telemetry
- PII markers (`Verified`, `PiiTagged`) are real newtypes; the type system
  forces explicit assertion at call sites.
- `_PROTO_*` keys are stripped before any general-access sink.
EOF

git add docs/SECURITY.md
git commit -m "docs: SECURITY.md"
```

---

## Task 7: README.md (workspace)

```bash
cat > README.md <<'EOF'
# LingXi Core

Platform-agnostic Rust engine for an AI coding assistant. M1 desktop-runnable
(Linux/macOS/Windows); Android/iOS land in M3.

## Quickstart

```bash
cargo build --workspace --release
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin lingxi-demo -- --model claude-opus-4-6
```

## Architecture

See `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md` for the
full design (~7000 lines) and `docs/ARCHITECTURE.md` for a navigation aid.

## License

MIT OR Apache-2.0.
EOF

git add README.md
git commit -m "docs: workspace README"
```

---

## Task 8: CHANGELOG + Tag M1 v0.1.0

```bash
cat > CHANGELOG.md <<'EOF'
# Changelog

## [0.1.0] — M1 Foundation Release

### Crates shipped
- protocol, core, traits, api-client (Plan 01)
- permission, secret, cost (Plan 02)
- tools, hooks (Plan 03)
- memory, mcp (Plan 04)
- compaction (Plan 05)
- agent (Plan 06)
- tasks, coordinator (Plan 07)
- sidequery (Plan 08)
- skills, commands, outputstyles (Plan 09)
- session, filestate, msgqueue (Plan 10)
- cron (Plan 11)
- sandbox, lsp (Plan 12)
- telemetry, anthropic-oauth (Plan 13)
- bridge (Plan 14)
- plugin (Plan 15)
- uniffi-bridge, platforms/posix-minimal, examples/cli-demo (Plan 16)
- test-harness (Plan 17)

### Platform support
- Linux/macOS/Windows: runnable via cli-demo + posix-minimal
- Android/iOS: cross-compile gate only; production platform crates land in M3
EOF

git add CHANGELOG.md
git commit -m "docs: CHANGELOG for M1 v0.1.0"

cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features 10k-iterations

git tag -a v0.1.0 -m "M1 v0.1.0 — desktop-runnable, mobile cross-compile only"
```

---

## Self-Review

- §32.1 Contract tests for all 13 traits → Task 1 ✓
- §32.2 Property tests at 10K iterations → Task 2 ✓
- §32.3 Parity fixtures from claude-code → Task 3 ✓
- §32.5 Contract coverage metric + CI gate (≤ 0.05) → Task 4 ✓
- ARCHITECTURE.md + SECURITY.md + README + CHANGELOG → Tasks 5-8 ✓
- Tag v0.1.0 → Task 8 ✓

## Execution Handoff

M1 complete. Next milestones (M2 desktop production, M3 mobile) are out of scope
for this 17-plan series; see `docs/superpowers/specs/...-design.md` §35 Post-M1
Roadmap for the M2/M3 outline.

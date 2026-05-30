# LingXi Core M2 · Plan 06 · SecureStorage macOS Keychain + HTTP SSE Streaming + Process Spawn Polish

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land three medium production features for the desktop platform crates so the engine can store OAuth tokens in the real macOS Keychain, stream Anthropic Messages SSE events with full claude-code variant parity, and spawn / tree-kill bash-tool processes with the same env, cwd-tracking, and extglob-disable behavior claude-code ships.

**Architecture:** All three subsystems live in `lingxi-code/platforms/{posix,windows}/src/`, behind existing `lingxi-traits` interfaces (`SecureStorage`, `HttpTransport`, `ProcessRunner`). No trait additions — we only widen the `StreamEvent` / `ContentDelta` / `ContentBlockApi` enums in `crates/api-client/src/types.rs`, refactor `platforms/posix/src/secure_storage.rs` and `platforms/posix/src/process.rs` into directories, replace the `Err(InvalidRequest)` SSE stubs with real `bytes_stream()` adapters, and wire `crates/secret/src/keychain_prefetch.rs` to call the new macOS backend through the same trait it already accepts. The single `unsafe { CommandExt::pre_exec(setsid) }` block lives in one module so the rest of the crate keeps `forbid(unsafe_code)`.

**Tech Stack:** Rust 1.82 stable (workspace toolchain), edition 2021, workspace lints `-D warnings`. New deps: `nix = "0.27"` (`signal` feature, for `killpg(2)` and `setsid()` constants on Unix), `sha2 = "0.10"` (keychain `dir_hash`), `hex = "0.4"` (keychain `-X <hex>` encoding), `unicode-normalization = "0.1"` (NFC cwd comparison), `bytes = "1"` (SSE chunk accumulation). Existing deps reused: `tokio::process::Command`, `reqwest::Response::bytes_stream()`, `crates/api-client/src/sse.rs::parse_sse_chunks`, `std::os::unix::process::CommandExt::pre_exec`.

**References:**
- Spec section: `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` §6.6 (Plan M2-06) · §3 (1:1 framing) · §4 (TS-dep → Rust strategy: `tree-kill` → `nix::sys::signal::killpg`, `security` CLI shell-out, `reqwest::bytes_stream`) · §7.1 (version pinning: `nix 0.27` Rust 1.82 compatibility).
- claude-code reference files (read-only inputs):
  - `claude-code/src/utils/secureStorage/macOsKeychainStorage.ts` (232 lines — `update` lines 97-157 show the `security -i` + argv-fallback shape; `readAsync` lines 67-96 show the 30 s TTL + generation counter + in-flight dedupe contract).
  - `claude-code/src/utils/secureStorage/macOsKeychainHelpers.ts` (112 lines — `getMacOsKeychainStorageServiceName` line 29 and the `keychainCacheState` shape lines 81-85).
  - `claude-code/src/utils/secureStorage/keychainPrefetch.ts` (117 lines — the parallel prewarm idiom).
  - `claude-code/src/services/api/claude.ts` lines 1995-2295 — the canonical `content_block_start/delta/stop` switch and the full block/delta type matrix (`text`, `thinking`, `tool_use`, `server_tool_use`, `connector_text`, `advisor_tool_result`; deltas `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`, `citations_delta`, `connector_text_delta`).
  - `claude-code/src/utils/Shell.ts` lines 281-441 — the spawn-env contract, file-mode stdio with `O_NOFOLLOW`, cwd readback with `readFileSync` to keep the post-await microtask race tight.
  - `claude-code/src/utils/ShellCommand.ts` lines 337-347 — `treeKill(pid, 'SIGKILL')`.
  - `claude-code/src/utils/shell/bashProvider.ts` lines 39-56 and 156-187 — the extglob-disable strings and `pwd -P >| <cwd>` tail.
- Trait definitions: `lingxi-code/crates/traits/src/secure_storage.rs` (`SecureStorage`, `SecureStorageBackend`, `SecureStorageError`), `lingxi-code/crates/traits/src/http.rs` (`HttpTransport`, `SseStream`, `HttpError`), `lingxi-code/crates/traits/src/process.rs` (`ProcessRunner`, `ProcessHandle`, `ProcessOutput`, `ProcessError`), `lingxi-code/crates/traits/src/sandbox.rs` (`SandboxedCommand`, `ProcessCommand`).
- Existing state: `lingxi-code/platforms/posix/src/secure_storage.rs` (PlainTextSecureStorage only), `lingxi-code/platforms/posix/src/http.rs::stream_sse` (returns `InvalidRequest`), `lingxi-code/platforms/posix/src/process.rs` (foreground `run` works; `spawn_background` returns `Unsupported`, no `kill_tree`), `lingxi-code/crates/api-client/src/types.rs::StreamEvent` (only `Text` + `InputJsonDelta` content deltas; `Thinking` block exists but no `ServerToolUse`/`ConnectorText`/`AdvisorToolResult`), `lingxi-code/crates/secret/src/keychain_prefetch.rs` (already plumbed through the trait — only needs to be invoked against the new macOS impl).

**Dependencies:** Plan M2-01 (worktree/sandbox/swarm/bridge corrections) must be complete. Independent of M2-02 / M2-03 / M2-04 / M2-05 — can land in parallel with any of them.

---

## File Inventory

**Modified (refactored from single-file to directory):**

- `lingxi-code/platforms/posix/src/secure_storage.rs` → deleted; replaced by directory below.
- `lingxi-code/platforms/posix/src/process.rs` → deleted; replaced by directory below.

**New (POSIX `secure_storage/` directory):**

- `lingxi-code/platforms/posix/src/secure_storage/mod.rs` — module top, re-exports `PlainTextSecureStorage`, `MacOsKeychainStorage`, `secure_storage_for_platform`, helper constants.
- `lingxi-code/platforms/posix/src/secure_storage/plaintext.rs` — verbatim move of today's `PlainTextSecureStorage` body.
- `lingxi-code/platforms/posix/src/secure_storage/helpers.rs` — `full_service_name`, `compute_dir_hash`, `SECURITY_STDIN_LINE_LIMIT`, `KEYCHAIN_CACHE_TTL`, `CREDENTIALS_SERVICE_SUFFIX`.
- `lingxi-code/platforms/posix/src/secure_storage/macos.rs` — `MacOsKeychainStorage` (`security` CLI backend, 30 s TTL cache, generation counter, in-flight dedupe).
- `lingxi-code/platforms/posix/src/secure_storage/factory.rs` — `secure_storage_for_platform(user, config_dir, plaintext_path)` with the macOS-Keychain-first / plaintext fallback policy.

**New (POSIX `process/` directory):**

- `lingxi-code/platforms/posix/src/process/mod.rs` — module top, re-exports `PosixProcess`, `wrap_command_for_cwd_tracking`, `kill_tree_unix`.
- `lingxi-code/platforms/posix/src/process/runner.rs` — `PosixProcess` struct, `ProcessRunner` impl (the relocated `run`, real `spawn_background`, real `kill`).
- `lingxi-code/platforms/posix/src/process/spawn_unsafe.rs` — *the only place* that uses `unsafe`: `pre_exec_setsid` (`#[allow(unsafe_code)]` local to this 25-line module).
- `lingxi-code/platforms/posix/src/process/kill_tree.rs` — `kill_tree_unix(pid)` via `nix::sys::signal::killpg`.
- `lingxi-code/platforms/posix/src/process/wrap.rs` — `wrap_command_for_cwd_tracking`, extglob-disable strings, `task_output_path`, the spawn-env contract helper.

**New (Windows mirrors — Windows file-watch / sandbox stubs are M2-01 / M2-05 already):**

- `lingxi-code/platforms/windows/src/process/mod.rs` — module top.
- `lingxi-code/platforms/windows/src/process/runner.rs` — `WindowsProcess`, `ProcessRunner` impl.
- `lingxi-code/platforms/windows/src/process/kill_tree.rs` — `taskkill /T /F /PID`.

**Modified (the rest):**

- `lingxi-code/platforms/posix/src/lib.rs` — relax `#![forbid(unsafe_code)]` to `#![deny(unsafe_code)]` (still strict; the only `#[allow(unsafe_code)]` lives in `process/spawn_unsafe.rs`); update re-exports.
- `lingxi-code/platforms/windows/src/lib.rs` — symmetrical relaxation; update re-exports.
- `lingxi-code/platforms/posix/src/http.rs` — replace `stream_sse`'s `Err(InvalidRequest)` with a real `bytes_stream()` adapter.
- `lingxi-code/platforms/windows/src/http.rs` — same `stream_sse` wiring.
- `lingxi-code/platforms/windows/src/process.rs` → deleted; replaced by `process/` directory above.
- `lingxi-code/platforms/posix/Cargo.toml` — add `nix`, `sha2`, `hex`, `unicode-normalization`, `bytes`; add `[dev-dependencies]` block with `tempfile`, `hyper`, `tokio` with `test-util`.
- `lingxi-code/platforms/windows/Cargo.toml` — add `bytes`; add `[dev-dependencies]` block with `tempfile`, `hyper`.
- `lingxi-code/crates/api-client/src/types.rs` — widen `StreamEvent`, `ContentDelta`, `ContentBlockApi`.
- `lingxi-code/crates/api-client/src/lib.rs` — re-export the new enum variants.
- `lingxi-code/crates/secret/src/keychain_prefetch.rs` — adjust constructor to accept `service`/`account` so the caller can target the macOS-keychain service name (`Claude Code-credentials`) without hard-coding `"lingxi"`.

**Tests (new integration-test files under `platforms/posix/tests/`):**

- `keychain_macos_service_name_test.rs` — unit-style coverage of `full_service_name` + `compute_dir_hash`.
- `keychain_macos_store_retrieve_test.rs` — `#[cfg(target_os = "macos")]`-gated round-trip via the real `security` CLI (uses a temp-suffixed service name so CI runs don't collide).
- `keychain_macos_cache_test.rs` — 30 s TTL, generation counter, in-flight dedupe.
- `http_stream_sse_test.rs` — local `hyper` test server emits claude-code-style events; verify parse.
- `process_kill_tree_test.rs` — spawn shell that forks two children, kill via tree-kill, verify all descendants gone.
- `process_spawn_background_test.rs` — spawn `sleep 30 && echo done`, verify task-output file, kill, verify finalization.
- `process_cwd_tracking_test.rs` — `wrap_command_for_cwd_tracking` produces expected wrapped string; integration verifies `pwd -P` writes a file with the right cwd.
- `process_spawn_env_test.rs` — env vars `CLAUDECODE=1`, `GIT_EDITOR=true`, `SHELL=<bin>`, `CLAUDE_CODE_SESSION_ID` appear in child env.

**Final commits at the end of all tasks:** three per spec §6.6:
1. `feat(secure_storage): macOS Keychain via security CLI with 30s TTL cache + in-flight dedupe`
2. `feat(http): real SSE streaming + claude-code event type parity (Thinking/Signature/Citations/ConnectorText)`
3. `feat(process): tree-kill via killpg + cwd tracking + spawn_background + 30-min timeout`

Per-task intermediate commits are workflow-only — squash on merge into the three feat() commits.

---

## Phase A — macOS Keychain via `security` CLI

### Task 1: Cargo dependencies + secure_storage/ skeleton + lib.rs relax

**Files:**
- Modify: `lingxi-code/platforms/posix/Cargo.toml`
- Modify: `lingxi-code/platforms/posix/src/lib.rs`
- Create: `lingxi-code/platforms/posix/src/secure_storage/mod.rs`
- Move: existing `lingxi-code/platforms/posix/src/secure_storage.rs` body → `lingxi-code/platforms/posix/src/secure_storage/plaintext.rs`
- Delete: `lingxi-code/platforms/posix/src/secure_storage.rs` (replaced by directory)

- [ ] **Step 1: Add deps to `platforms/posix/Cargo.toml`**

Append to `[dependencies]` (after `fs2 = "0.4"`):

```toml
sha2 = "0.10"
hex = "0.4"
nix = { version = "0.27", default-features = false, features = ["signal", "process"] }
unicode-normalization = "0.1"
bytes = "1"
```

Append at the end (before `[lints]`):

```toml
[dev-dependencies]
tempfile = "3"
hyper = { version = "1", features = ["server", "http1"] }
hyper-util = { version = "0.1", features = ["server", "server-auto", "tokio"] }
http-body-util = "0.1"
tokio = { workspace = true, features = ["full", "test-util"] }
futures-util = "0.3"
```

**Rust 1.82 compatibility for `nix 0.27`:** confirmed — `nix 0.27` MSRV is 1.69. If a transitive dep pulls edition2024 (as several M1 deps did), apply `cargo update -p <dep> --precise <ver>` per spec §7.1's pattern. The `default-features = false` + explicit `signal` / `process` feature selection keeps the surface tiny (no `mount`, `aio`, etc. heavy features).

- [ ] **Step 2: Relax `forbid(unsafe_code)` in `platforms/posix/src/lib.rs`**

Change line 6 from:

```rust
#![forbid(unsafe_code)]
```

to:

```rust
// Most of the crate is safe Rust; the single `unsafe` block lives in
// `process::spawn_unsafe` where `CommandExt::pre_exec` calls
// `libc::setsid()` to detach background children. `deny(unsafe_code)` still
// catches accidental introductions elsewhere — the only `#[allow]` is in
// that one 25-line module.
#![deny(unsafe_code)]
```

- [ ] **Step 3: Create the directory and move plaintext body**

```bash
mkdir -p lingxi-code/platforms/posix/src/secure_storage
git mv lingxi-code/platforms/posix/src/secure_storage.rs lingxi-code/platforms/posix/src/secure_storage/plaintext.rs
```

In the new `plaintext.rs`, no body changes are required — only adjust the module doc to:

```rust
//! Plain-text file-based [`SecureStorage`] for desktop hosts.
//!
//! Used as a fallback on Linux (no libsecret wiring yet) and as the safety
//! net on macOS when `MacOsKeychainStorage::new` cannot initialise. The
//! macOS keychain backend lives in [`super::macos`].
```

- [ ] **Step 4: Write the new module top**

Create `lingxi-code/platforms/posix/src/secure_storage/mod.rs`:

```rust
//! Secure storage backends for desktop hosts.
//!
//! `PlainTextSecureStorage` (Linux + fallback) and `MacOsKeychainStorage`
//! (macOS, via the `security` CLI) implement `lingxi-traits::SecureStorage`.
//! The [`factory::secure_storage_for_platform`] helper picks the best
//! backend per OS, with a documented plaintext fallback warning when the
//! preferred backend cannot initialise.

pub mod factory;
pub mod helpers;
pub mod macos;
pub mod plaintext;

pub use factory::secure_storage_for_platform;
pub use helpers::{
    compute_dir_hash, full_service_name, CREDENTIALS_SERVICE_SUFFIX, KEYCHAIN_CACHE_TTL,
    SECURITY_STDIN_LINE_LIMIT,
};
pub use macos::MacOsKeychainStorage;
pub use plaintext::PlainTextSecureStorage;
```

- [ ] **Step 5: Stub the new files so `cargo check` builds**

Create empty stubs (filled in by Tasks 2-9):

`lingxi-code/platforms/posix/src/secure_storage/helpers.rs`:

```rust
//! Filled in by Task 2.
```

`lingxi-code/platforms/posix/src/secure_storage/macos.rs`:

```rust
//! Filled in by Task 3.
```

`lingxi-code/platforms/posix/src/secure_storage/factory.rs`:

```rust
//! Filled in by Task 9.

use lingxi_traits::{SecureStorage, SecureStorageError};
use std::path::PathBuf;
use std::sync::Arc;

/// Stub — filled in by Task 9.
///
/// # Errors
/// Returns [`SecureStorageError::Io`] when the plaintext fallback cannot
/// create its base directory.
pub async fn secure_storage_for_platform(
    _user: String,
    _config_dir: PathBuf,
    plaintext_path: PathBuf,
) -> Result<Arc<dyn SecureStorage>, SecureStorageError> {
    let plain = super::plaintext::PlainTextSecureStorage::new(plaintext_path).await?;
    Ok(Arc::new(plain))
}
```

- [ ] **Step 6: Update `lib.rs` re-exports**

In `lingxi-code/platforms/posix/src/lib.rs`, replace:

```rust
pub use secure_storage::PlainTextSecureStorage;
```

with:

```rust
pub use secure_storage::{
    secure_storage_for_platform, MacOsKeychainStorage, PlainTextSecureStorage,
};
```

- [ ] **Step 7: Verify `cargo check` is clean**

```bash
cargo check -p lingxi-platform-posix
```

Expected: clean. No new errors.

- [ ] **Step 8: Commit**

```bash
git add lingxi-code/platforms/posix/Cargo.toml lingxi-code/platforms/posix/src/lib.rs lingxi-code/platforms/posix/src/secure_storage/
git commit -m "$(cat <<'EOF'
build(platform-posix): add nix/sha2/hex deps; scaffold secure_storage/ directory

Adds the deps and module skeleton required by Plan M2-06 Phase A
(MacOsKeychainStorage). Refactors single-file secure_storage.rs into a
directory with plaintext/macos/helpers/factory submodules. Relaxes
forbid(unsafe_code) to deny(unsafe_code) so the single `unsafe` block
required for setsid() in Phase C can compile inside its own module
without escaping the rest of the crate.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Service-name helpers + constants (TDD)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/secure_storage/helpers.rs`
- Test: `lingxi-code/platforms/posix/tests/keychain_macos_service_name_test.rs`

**Critical 1:1 fidelity:** service name format is `format!("Claude Code{oauth_suffix}{service_suffix}{dir_hash}")`. Empty `dir_hash` when `config_dir == default_claude_dir()`; otherwise `format!("-{}", &sha256(config_dir).hex()[..8])`. `oauth_suffix` is empty for M2 (claude-code's `OAUTH_FILE_SUFFIX` is empty in stable build; passing it through preserves the optionality). `service_suffix = "-credentials"` for OAuth entries, `""` for the legacy API-key entry.

- [ ] **Step 1: Write the failing tests**

Create `lingxi-code/platforms/posix/tests/keychain_macos_service_name_test.rs`:

```rust
//! Service-name helper tests — must match claude-code's macOsKeychainHelpers.ts exactly.

use lingxi_platform_posix::secure_storage::{compute_dir_hash, full_service_name};
use std::path::PathBuf;

#[test]
fn default_dir_yields_empty_hash_suffix() {
    let default_dir = PathBuf::from(format!(
        "{}/.claude",
        std::env::var("HOME").unwrap_or_else(|_| "/Users/test".into())
    ));
    let hash = compute_dir_hash(&default_dir, &default_dir);
    assert_eq!(hash, "", "default ~/.claude must produce empty dir_hash");
}

#[test]
fn non_default_dir_yields_8_char_hex() {
    let default = PathBuf::from("/Users/test/.claude");
    let custom = PathBuf::from("/Users/test/work/.claude-2");
    let hash = compute_dir_hash(&custom, &default);
    assert_eq!(hash.len(), 9, "expected `-` + 8 hex chars, got {hash:?}");
    assert!(hash.starts_with('-'));
    assert!(hash[1..]
        .chars()
        .all(|c| c.is_ascii_hexdigit() && c.is_ascii_lowercase() || c.is_ascii_digit()));
}

#[test]
fn service_name_default_oauth_layout() {
    // claude-code: getMacOsKeychainStorageServiceName("-credentials") for default config dir.
    let name = full_service_name("Claude Code", "", "-credentials", "");
    assert_eq!(name, "Claude Code-credentials");
}

#[test]
fn service_name_legacy_api_key_default() {
    // claude-code: getMacOsKeychainStorageServiceName() with no suffix.
    let name = full_service_name("Claude Code", "", "", "");
    assert_eq!(name, "Claude Code");
}

#[test]
fn service_name_oauth_with_dir_hash() {
    let name = full_service_name("Claude Code", "", "-credentials", "-abc12345");
    assert_eq!(name, "Claude Code-credentials-abc12345");
}

#[test]
fn service_name_with_oauth_suffix_and_dir_hash() {
    // claude-code's `OAUTH_FILE_SUFFIX` is non-empty in some builds; preserve the slot.
    let name = full_service_name("Claude Code", "-staging", "-credentials", "-deadbeef");
    assert_eq!(name, "Claude Code-staging-credentials-deadbeef");
}
```

- [ ] **Step 2: Run the test; verify it fails**

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_service_name_test
```

Expected: compile error `unresolved imports compute_dir_hash, full_service_name`.

- [ ] **Step 3: Implement `helpers.rs`**

Replace `lingxi-code/platforms/posix/src/secure_storage/helpers.rs` body with:

```rust
//! Constants + service-name helpers shared between [`super::macos`] and
//! consumers of the macOS keychain backend (e.g. `keychain_prefetch`).
//!
//! 1:1 with claude-code's `macOsKeychainHelpers.ts`:
//! - [`full_service_name`] reproduces `getMacOsKeychainStorageServiceName`.
//! - [`compute_dir_hash`] reproduces the `sha256(configDir).hex()[..8]`
//!   prefix-only-when-non-default behavior.
//! - [`SECURITY_STDIN_LINE_LIMIT`] guards the `security -i` 4096-byte BUFSIZ.
//! - [`KEYCHAIN_CACHE_TTL`] reproduces the 30 s TTL.

use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// Service-suffix appended to the legacy API-key service name to derive
/// the OAuth credentials entry. DO NOT change — part of the cross-version
/// keychain lookup key. Matches claude-code's `CREDENTIALS_SERVICE_SUFFIX`.
pub const CREDENTIALS_SERVICE_SUFFIX: &str = "-credentials";

/// `security -i` reads stdin with a 4096-byte fgets() buffer (BUFSIZ on
/// darwin). Anything longer is truncated mid-argument. 64 B headroom matches
/// claude-code's safety margin (see macOsKeychainStorage.ts:24).
pub const SECURITY_STDIN_LINE_LIMIT: usize = 4096 - 64;

/// 30 s TTL on cached keychain reads. Claude-code's
/// `KEYCHAIN_CACHE_TTL_MS = 30_000`. Bounds cross-process staleness without
/// triggering repeated 500 ms `security` spawns under load.
pub const KEYCHAIN_CACHE_TTL: Duration = Duration::from_secs(30);

/// Produce the full keychain service name.
///
/// Mirrors claude-code's `getMacOsKeychainStorageServiceName`:
/// `"Claude Code" + oauth_suffix + service_suffix + dir_hash`.
///
/// `base` is conventionally `"Claude Code"`; we accept it as a parameter so
/// downstream products can override the prefix without touching this helper.
///
/// `oauth_suffix` is claude-code's `OAUTH_FILE_SUFFIX` (empty in stable; non-empty
/// in some build variants).
///
/// `service_suffix` is [`CREDENTIALS_SERVICE_SUFFIX`] for OAuth entries
/// or empty for the legacy API-key entry.
///
/// `dir_hash` is the output of [`compute_dir_hash`] — either empty (default
/// `~/.claude`) or `format!("-{}", &sha256(config_dir).hex()[..8])`.
#[must_use]
pub fn full_service_name(
    base: &str,
    oauth_suffix: &str,
    service_suffix: &str,
    dir_hash: &str,
) -> String {
    format!("{base}{oauth_suffix}{service_suffix}{dir_hash}")
}

/// Return the keychain-name dir-hash component.
///
/// Returns the empty string when `config_dir == default_dir`. Otherwise
/// returns `"-" + sha256(config_dir.to_string_lossy()).hex()[..8]`.
///
/// `default_dir` is the engine's canonical "default" config directory;
/// passing the actual user-home-derived default avoids env lookups inside
/// the helper. Claude-code's source uses `process.env.CLAUDE_CONFIG_DIR`
/// presence as the discriminator, which is equivalent (when the env var is
/// unset, `getClaudeConfigHomeDir()` returns the default).
#[must_use]
pub fn compute_dir_hash(config_dir: &Path, default_dir: &Path) -> String {
    if config_dir == default_dir {
        return String::new();
    }
    let bytes = config_dir.to_string_lossy();
    let mut hasher = Sha256::new();
    hasher.update(bytes.as_bytes());
    let digest = hasher.finalize();
    let hex_full = hex::encode(digest);
    format!("-{}", &hex_full[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_is_30_seconds() {
        assert_eq!(KEYCHAIN_CACHE_TTL.as_secs(), 30);
    }

    #[test]
    fn stdin_limit_matches_claude_code() {
        assert_eq!(SECURITY_STDIN_LINE_LIMIT, 4096 - 64);
    }

    #[test]
    fn credentials_suffix_matches_claude_code() {
        assert_eq!(CREDENTIALS_SERVICE_SUFFIX, "-credentials");
    }

    #[test]
    fn dir_hash_default_returns_empty() {
        let p = Path::new("/Users/x/.claude");
        assert_eq!(compute_dir_hash(p, p), "");
    }
}
```

- [ ] **Step 4: Run the tests; verify they pass**

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_service_name_test
cargo test -p lingxi-platform-posix --lib secure_storage::helpers
```

Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/posix/src/secure_storage/helpers.rs lingxi-code/platforms/posix/tests/keychain_macos_service_name_test.rs
git commit -m "$(cat <<'EOF'
feat(secure_storage): keychain service-name helpers (1:1 with claude-code)

full_service_name + compute_dir_hash reproduce claude-code's
getMacOsKeychainStorageServiceName layout byte-for-byte. Constants:
SECURITY_STDIN_LINE_LIMIT = 4096 - 64, KEYCHAIN_CACHE_TTL = 30s,
CREDENTIALS_SERVICE_SUFFIX = "-credentials".

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `MacOsKeychainStorage` struct + constructor

**Files:**
- Modify: `lingxi-code/platforms/posix/src/secure_storage/macos.rs`

This task lands the type + constructor + cache state. The trait impl methods are filled in by Tasks 4-8.

- [ ] **Step 1: Write the file**

Replace `lingxi-code/platforms/posix/src/secure_storage/macos.rs` body with:

```rust
//! macOS Keychain backend for [`SecureStorage`], shelling out to the `security` CLI.
//!
//! See `claude-code/src/utils/secureStorage/macOsKeychainStorage.ts` for the
//! reference implementation. Behavior locked here:
//! - Service name from [`super::helpers::full_service_name`].
//! - JSON payload hex-encoded into `security -i`'s stdin (`-X <hex>`), with
//!   a length-checked argv fallback when the command would overflow
//!   [`super::helpers::SECURITY_STDIN_LINE_LIMIT`].
//! - 30 s TTL read cache with generation counter (prevents stale subprocess
//!   results from overwriting fresh writes) and in-flight dedupe (concurrent
//!   reads share a single subprocess).
//! - `list` is not supported — claude-code's API doesn't expose prefix
//!   queries via `security`. Returns
//!   [`SecureStorageError::Backend`] with a documented message.

use crate::secure_storage::helpers::{
    full_service_name, KEYCHAIN_CACHE_TTL, SECURITY_STDIN_LINE_LIMIT,
};
use async_trait::async_trait;
use lingxi_protocol::SecureStorageData;
use lingxi_traits::{SecureStorage, SecureStorageBackend, SecureStorageError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, Notify, RwLock};

type CacheKey = (String, String);

#[derive(Clone)]
struct CachedEntry {
    data: SecureStorageData,
    fetched_at: Instant,
    generation: u64,
}

/// macOS Keychain backend.
///
/// Construct via [`MacOsKeychainStorage::new`]. The `user` field is the
/// `security -a <account>` argument (typically `$USER`); the `config_dir`
/// drives the per-config-directory dir-hash service-name suffix.
pub struct MacOsKeychainStorage {
    user: String,
    config_dir: PathBuf,
    default_config_dir: PathBuf,
    oauth_suffix: String,
    cache: Arc<RwLock<HashMap<CacheKey, CachedEntry>>>,
    generation: Arc<AtomicU64>,
    inflight: Arc<Mutex<HashMap<CacheKey, Arc<Notify>>>>,
}

impl MacOsKeychainStorage {
    /// Construct a new keychain-backed store.
    ///
    /// `user` is the keychain account name (claude-code uses
    /// `process.env.USER || userInfo().username`).
    /// `config_dir` is the user's claude config directory (`~/.claude` or
    /// whatever `CLAUDE_CONFIG_DIR` overrides it to).
    /// `default_config_dir` is what claude-code calls the "default"
    /// `~/.claude` — passed in so the dir-hash discriminator can compare.
    /// `oauth_suffix` mirrors claude-code's `OAUTH_FILE_SUFFIX`; pass `""`
    /// for the standard build.
    ///
    /// # Errors
    /// Returns [`SecureStorageError::Backend`] when the `security` CLI is
    /// not on `$PATH` (the caller can then fall back to plaintext).
    pub async fn new(
        user: String,
        config_dir: PathBuf,
        default_config_dir: PathBuf,
        oauth_suffix: String,
    ) -> Result<Self, SecureStorageError> {
        if which::which("security").is_err() {
            return Err(SecureStorageError::Backend(
                "macOS `security` CLI not found on PATH".into(),
            ));
        }
        Ok(Self {
            user,
            config_dir,
            default_config_dir,
            oauth_suffix,
            cache: Arc::new(RwLock::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Convenience constructor with the default `OAUTH_FILE_SUFFIX = ""`.
    pub async fn new_default_oauth_suffix(
        user: String,
        config_dir: PathBuf,
        default_config_dir: PathBuf,
    ) -> Result<Self, SecureStorageError> {
        Self::new(user, config_dir, default_config_dir, String::new()).await
    }

    /// Return the full keychain service name for `(service, _account)`.
    ///
    /// In claude-code, the `service` we receive from callers maps onto the
    /// service-suffix part: callers pass `"-credentials"` (OAuth) or `""`
    /// (legacy API key). The full keychain service name interpolates the
    /// constant prefix `"Claude Code"` (NOT the user-supplied `service`)
    /// because compatibility with existing entries written by claude-code
    /// requires literal-string parity.
    pub(crate) fn keychain_service_name(&self, service_suffix: &str) -> String {
        let dir_hash = super::helpers::compute_dir_hash(
            self.config_dir.as_path(),
            self.default_config_dir.as_path(),
        );
        full_service_name(
            "Claude Code",
            self.oauth_suffix.as_str(),
            service_suffix,
            &dir_hash,
        )
    }

    pub(crate) fn user(&self) -> &str {
        self.user.as_str()
    }

    /// Bump the generation counter. Called on every successful store/delete
    /// and on explicit cache invalidation. A pending `retrieve` that
    /// observes a higher counter when its subprocess returns must NOT write
    /// its (now-stale) result to the cache. Matches claude-code's
    /// `keychainCacheState.generation`.
    pub(crate) fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

// SecureStorage trait impl is split across Tasks 4-8.

// `which` crate is already in the workspace lockfile (used by sandbox); if
// not, replace the `which::which("security")` with a manual probe via
// `tokio::process::Command::new("/usr/bin/which").arg("security")`. The
// canonical macOS path is `/usr/bin/security`; we could even hard-code it.
```

- [ ] **Step 2: Add the `which` dep if missing**

```bash
grep -q '^which = ' lingxi-code/platforms/posix/Cargo.toml || echo "needs which"
```

If missing, append to `[dependencies]`:

```toml
which = "6"
```

(M1 plans 12 / sandbox already pin `which`; if so this step is a no-op.)

- [ ] **Step 3: Verify `cargo check`**

```bash
cargo check -p lingxi-platform-posix
```

Expected: clean. The struct compiles even though `SecureStorage` is not yet implemented — `impl SecureStorage for MacOsKeychainStorage` lands in Task 4 onwards.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/platforms/posix/Cargo.toml lingxi-code/platforms/posix/src/secure_storage/macos.rs
git commit -m "$(cat <<'EOF'
feat(secure_storage): MacOsKeychainStorage struct + cache state

Lands the struct, constructor, and cache primitives (HashMap + AtomicU64
generation counter + tokio::Notify in-flight dedupe). Trait impl methods
land in subsequent tasks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `store` via `security -i` stdin with argv fallback (TDD)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/secure_storage/macos.rs`
- Test: `lingxi-code/platforms/posix/tests/keychain_macos_store_retrieve_test.rs` (created here, expanded in Task 5)

**Critical 1:1 fidelity:**
- The preferred path spawns `security` with argv `["-i"]` and writes a complete `add-generic-password ...` command on stdin, **newline-terminated**.
- The exact stdin payload format: `add-generic-password -U -a "<user>" -s "<service>" -X "<hex>"\n`. Argument quoting uses literal `"` around `user`, `service`, and `hex` — claude-code's source does this with template strings, which gives literal double-quotes.
- The argv fallback kicks in only when the assembled command string would exceed `SECURITY_STDIN_LINE_LIMIT` (4096 - 64). In that case the binary is invoked with `argv = ["add-generic-password", "-U", "-a", user, "-s", service, "-X", hex]`.
- Both paths bump the generation counter on success and invalidate the cache entry.

- [ ] **Step 1: Write the failing test scaffold**

Create `lingxi-code/platforms/posix/tests/keychain_macos_store_retrieve_test.rs`:

```rust
//! `MacOsKeychainStorage` store/retrieve round-trip via the real `security` CLI.
//!
//! Gated on macOS only. Uses a temp-suffixed service name and unique account
//! so concurrent CI runs don't collide.

#![cfg(target_os = "macos")]

use lingxi_platform_posix::secure_storage::MacOsKeychainStorage;
use lingxi_protocol::SecureStorageData;
use lingxi_traits::SecureStorage;
use std::path::PathBuf;

fn unique_account() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("lingxi-test-{nanos}")
}

#[tokio::test]
async fn store_then_retrieve_round_trip() {
    let user = std::env::var("USER").unwrap_or_else(|_| "test".into());
    // Force a non-default config_dir so the service name carries a dir_hash —
    // keeps real claude-code entries safe from our test.
    let config_dir = PathBuf::from(format!("/tmp/lingxi-keychain-test-{}", std::process::id()));
    let default_dir = PathBuf::from("/Users/_lingxi_test_default/.claude");
    let storage = MacOsKeychainStorage::new(user, config_dir, default_dir, String::new())
        .await
        .expect("MacOsKeychainStorage::new");

    let account = unique_account();
    let payload = SecureStorageData::new(b"sk-ant-test-1234567890".to_vec());

    storage
        .store("-credentials", &account, payload.clone())
        .await
        .expect("store");

    let read = storage
        .retrieve("-credentials", &account)
        .await
        .expect("retrieve");
    let read = read.expect("entry must exist");
    assert_eq!(read.secret(), payload.secret(), "round-trip mismatch");

    // Cleanup so we don't litter the user's keychain.
    storage.delete("-credentials", &account).await.expect("delete");
}
```

- [ ] **Step 2: Run the test; verify it fails (no impl)**

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_store_retrieve_test
```

Expected on macOS: linker/compile fails because `SecureStorage` is not implemented for `MacOsKeychainStorage` yet.

Expected on Linux: silently skipped (`#![cfg(target_os = "macos")]`).

- [ ] **Step 3: Implement `store` and trait scaffold in `macos.rs`**

Append to `lingxi-code/platforms/posix/src/secure_storage/macos.rs`:

```rust
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use std::process::Stdio;

#[async_trait]
impl SecureStorage for MacOsKeychainStorage {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: SecureStorageData,
    ) -> Result<(), SecureStorageError> {
        // Pre-invalidate the cache so a concurrent reader doesn't return
        // stale data after we bump the generation but before the subprocess
        // returns.
        let key = (service.to_string(), account.to_string());
        self.cache.write().await.remove(&key);
        self.bump_generation();

        let full_service = self.keychain_service_name(service);

        // JSON → hex. Claude-code uses Buffer.from(jsonString, 'utf-8').toString('hex').
        let json = serde_json::to_string(&data)
            .map_err(|e| SecureStorageError::Io(format!("serialize: {e}")))?;
        let hex_value = hex::encode(json.as_bytes());

        // Preferred path: `security -i` reads the full add-generic-password
        // command on stdin. Keeps the hex payload out of argv so process
        // monitors only see "security -i".
        let stdin_command = format!(
            "add-generic-password -U -a \"{}\" -s \"{}\" -X \"{}\"\n",
            self.user.as_str(),
            full_service,
            hex_value,
        );

        let exit_status = if stdin_command.len() <= SECURITY_STDIN_LINE_LIMIT {
            run_security_stdin(&stdin_command).await?
        } else {
            // Argv fallback. Hex in argv is recoverable by a determined
            // observer but defeats naive plaintext-grep rules — silent
            // credential corruption (the alternative) is strictly worse.
            tracing::warn!(
                target: "lingxi::secure_storage::macos",
                "Keychain payload ({} B JSON) exceeds security -i stdin limit; using argv",
                json.len()
            );
            run_security_argv(
                &[
                    "add-generic-password",
                    "-U",
                    "-a",
                    self.user.as_str(),
                    "-s",
                    full_service.as_str(),
                    "-X",
                    hex_value.as_str(),
                ],
            )
            .await?
        };

        if !exit_status.success() {
            return Err(SecureStorageError::Backend(format!(
                "security add-generic-password exited {}",
                exit_status.code().unwrap_or(-1)
            )));
        }

        // Cache the freshly-written data with the new generation.
        let now = Instant::now();
        let gen = self.generation.load(Ordering::Acquire);
        self.cache.write().await.insert(
            key,
            CachedEntry {
                data,
                fetched_at: now,
                generation: gen,
            },
        );
        Ok(())
    }

    async fn retrieve(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<Option<SecureStorageData>, SecureStorageError> {
        // Filled in by Task 5.
        Err(SecureStorageError::Backend("Task 5: retrieve pending".into()))
    }

    async fn delete(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<(), SecureStorageError> {
        // Filled in by Task 6.
        Err(SecureStorageError::Backend("Task 6: delete pending".into()))
    }

    async fn list(&self, _service: &str) -> Result<Vec<String>, SecureStorageError> {
        Err(SecureStorageError::Backend(
            "list not supported on macOS Keychain backend".into(),
        ))
    }

    fn is_encrypted(&self) -> bool {
        true
    }

    fn backend(&self) -> SecureStorageBackend {
        SecureStorageBackend::MacOsKeychain
    }
}

async fn run_security_stdin(command: &str) -> Result<std::process::ExitStatus, SecureStorageError> {
    let mut child = Command::new("security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SecureStorageError::Io(format!("spawn security -i: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(command.as_bytes())
            .await
            .map_err(|e| SecureStorageError::Io(format!("stdin write: {e}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| SecureStorageError::Io(format!("stdin shutdown: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| SecureStorageError::Io(format!("wait: {e}")))?;
    Ok(output.status)
}

async fn run_security_argv(args: &[&str]) -> Result<std::process::ExitStatus, SecureStorageError> {
    let output = Command::new("security")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;
    Ok(output.status)
}
```

Check the `SecureStorageBackend` enum has a `MacOsKeychain` variant. If it does not, add it in `lingxi-code/crates/traits/src/secure_storage.rs` (and re-run `cargo check`):

```rust
pub enum SecureStorageBackend {
    PlainText,
    MacOsKeychain,  // <-- add if missing
}
```

(Spec §6.6 references `SecureStorageBackend::MacOsKeychain` directly.)

- [ ] **Step 4: Run `cargo check` to ensure the build is clean (skip the gated macOS test)**

```bash
cargo check -p lingxi-platform-posix
```

Expected: clean.

If running on macOS:

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_store_retrieve_test
```

Expected: store succeeds; `retrieve` fails with the placeholder error (Task 5 wires it up). Confirm the keychain entry exists via `security find-generic-password -a "$USER" -s "Claude Code-credentials-<hash>"` from a separate terminal.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/posix/src/secure_storage/macos.rs lingxi-code/crates/traits/src/secure_storage.rs lingxi-code/platforms/posix/tests/keychain_macos_store_retrieve_test.rs
git commit -m "$(cat <<'EOF'
feat(secure_storage): MacOsKeychainStorage::store via `security -i` stdin

Preferred path: spawn `security -i` and feed the full
`add-generic-password -U -a "<user>" -s "<svc>" -X "<hex>"\n` command on
stdin (claude-code's INC-3028 mitigation — process monitors see only
`security -i`, not the payload). Argv fallback kicks in when the command
length exceeds SECURITY_STDIN_LINE_LIMIT (4096 - 64).

JSON-serialized SecureStorageData is hex-encoded (Buffer.from(...).toString('hex')
in claude-code; hex::encode here). Generation counter bumps on every
store so concurrent stale subprocess reads can't overwrite fresh data.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `retrieve` with 30 s TTL + in-flight dedupe (TDD)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/secure_storage/macos.rs`
- Test: `lingxi-code/platforms/posix/tests/keychain_macos_cache_test.rs`

**Critical 1:1 fidelity:**
- Cache hit: when an entry exists AND `fetched_at + KEYCHAIN_CACHE_TTL > Instant::now()` AND the entry's recorded generation equals the current generation counter, return the cloned data without spawning a subprocess.
- Cache miss: spawn `security find-generic-password -a <user> -w -s <full_service>`. Parse stdout, hex-decode (NOT — `security ... -w` outputs the password directly, which is our hex string — actually, wait: claude-code stores JSON-stringified data hex-encoded as the password, so reading it back yields the **hex string** that we then hex-decode to get the JSON, then deserialize). **Important**: re-read claude-code's `update()` vs `read()` to confirm — `update` writes `-X <hex>` (hex bytes), `read` retrieves with `-w` (which prints the password). On the write side, `-X` interprets the value as hex-encoded bytes, **so the password stored is the binary bytes, not the hex string**. On read, `-w` prints those binary bytes — claude-code receives them as the JSON string directly, no hex-decode on read. We mirror this: write hex (via `-X`), read raw (security's default decoding), `serde_json::from_str` the result.
- In-flight dedupe: when two `retrieve` calls race past the cache check, only one spawns; the second `await`s a shared `Notify`.
- Generation check on subprocess completion: if the generation changed during the subprocess's lifetime, **discard** the result instead of inserting a stale value.

- [ ] **Step 1: Write the failing cache test**

Create `lingxi-code/platforms/posix/tests/keychain_macos_cache_test.rs`:

```rust
//! Cache TTL + generation counter + in-flight dedupe tests.
//!
//! Where possible these run without macOS (mock subprocess by hitting
//! `security` with `-h` which is a fast no-op). The actual end-to-end
//! store/retrieve case is in `keychain_macos_store_retrieve_test.rs`.

#![cfg(target_os = "macos")]

use lingxi_platform_posix::secure_storage::{MacOsKeychainStorage, KEYCHAIN_CACHE_TTL};
use lingxi_protocol::SecureStorageData;
use lingxi_traits::SecureStorage;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fresh_account() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "lingxi-cache-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn mk_storage() -> Arc<MacOsKeychainStorage> {
    let user = std::env::var("USER").unwrap_or_else(|_| "test".into());
    let cfg = PathBuf::from(format!("/tmp/lingxi-cache-test-{}", std::process::id()));
    let default = PathBuf::from("/Users/_lingxi_test_default/.claude");
    Arc::new(
        MacOsKeychainStorage::new(user, cfg, default, String::new())
            .await
            .expect("storage init"),
    )
}

#[tokio::test]
async fn cache_hit_avoids_subprocess() {
    let storage = mk_storage().await;
    let account = fresh_account();
    let payload = SecureStorageData::new(b"cached-value".to_vec());

    storage
        .store("-credentials", &account, payload.clone())
        .await
        .expect("store");

    // First retrieve populates the cache.
    let t0 = Instant::now();
    storage.retrieve("-credentials", &account).await.unwrap();
    let cold_ms = t0.elapsed().as_millis();

    // Second retrieve should be sub-millisecond (no spawn).
    let t1 = Instant::now();
    storage.retrieve("-credentials", &account).await.unwrap();
    let warm_ms = t1.elapsed().as_millis();

    assert!(
        warm_ms * 10 < cold_ms.max(10),
        "cache miss {cold_ms} ms vs hit {warm_ms} ms — expected hit to be at least 10x faster"
    );
    storage.delete("-credentials", &account).await.unwrap();
}

#[tokio::test]
async fn concurrent_retrieve_dedupes_to_one_subprocess() {
    let storage = mk_storage().await;
    let account = fresh_account();
    let payload = SecureStorageData::new(b"dedupe-value".to_vec());
    storage
        .store("-credentials", &account, payload.clone())
        .await
        .expect("store");

    // Force cache miss by waiting past TTL.
    tokio::time::sleep(KEYCHAIN_CACHE_TTL + Duration::from_millis(50)).await;

    // Launch 20 concurrent retrieves; they should all share one subprocess.
    let mut joins = Vec::new();
    let t0 = Instant::now();
    for _ in 0..20 {
        let s = storage.clone();
        let acc = account.clone();
        joins.push(tokio::spawn(async move {
            s.retrieve("-credentials", &acc).await.unwrap()
        }));
    }
    for h in joins {
        h.await.unwrap();
    }
    let total = t0.elapsed().as_millis();
    // A single security spawn is ~500 ms on warm darwin; 20 sequential
    // would be ~10 s. Generous bound of 2 s.
    assert!(
        total < 2_000,
        "20 concurrent retrieves took {total} ms — dedupe broken?"
    );

    storage.delete("-credentials", &account).await.unwrap();
}
```

- [ ] **Step 2: Run the test; verify it fails (retrieve is still a stub)**

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_cache_test
```

Expected on macOS: `Backend("Task 5: retrieve pending")`.

- [ ] **Step 3: Implement `retrieve`**

Replace the `retrieve` placeholder in `macos.rs` with:

```rust
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<SecureStorageData>, SecureStorageError> {
        let key = (service.to_string(), account.to_string());
        let now = Instant::now();
        let cur_gen = self.generation.load(Ordering::Acquire);

        // Cache hit fast path.
        if let Some(entry) = self.cache.read().await.get(&key).cloned() {
            if entry.generation == cur_gen
                && now.duration_since(entry.fetched_at) < KEYCHAIN_CACHE_TTL
            {
                return Ok(Some(entry.data));
            }
        }

        // In-flight dedupe: if another retrieve is already running for this
        // key, await its Notify and retry the cache read.
        let notify = {
            let mut inflight = self.inflight.lock().await;
            if let Some(existing) = inflight.get(&key).cloned() {
                drop(inflight);
                existing.notified().await;
                // Re-read the cache after the other call completed.
                if let Some(entry) = self.cache.read().await.get(&key).cloned() {
                    return Ok(Some(entry.data));
                }
                return Ok(None);
            }
            let n = Arc::new(Notify::new());
            inflight.insert(key.clone(), n.clone());
            n
        };

        // We are the responsible spawner.
        let full_service = self.keychain_service_name(service);
        let result = run_security_find(&self.user, &full_service, account).await;

        // Notify waiters regardless of outcome and remove our inflight entry.
        {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(&key);
        }
        notify.notify_waiters();

        match result {
            Ok(Some(data)) => {
                // Generation check: if the counter changed during our
                // subprocess, our result is stale — return it to *this*
                // caller but do NOT poison the cache.
                let post_gen = self.generation.load(Ordering::Acquire);
                if post_gen == cur_gen {
                    self.cache.write().await.insert(
                        key,
                        CachedEntry {
                            data: data.clone(),
                            fetched_at: Instant::now(),
                            generation: post_gen,
                        },
                    );
                }
                Ok(Some(data))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
```

And below the `retrieve` method (still inside the impl block), add the helper:

```rust
}

async fn run_security_find(
    user: &str,
    full_service: &str,
    _account: &str,
) -> Result<Option<SecureStorageData>, SecureStorageError> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            user,
            "-w",
            "-s",
            full_service,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;
    if !output.status.success() {
        // exit code 44 (errSecItemNotFound) is a normal "no entry" result.
        if output.status.code() == Some(44) {
            return Ok(None);
        }
        return Err(SecureStorageError::Backend(format!(
            "security find-generic-password exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(None);
    }
    let data: SecureStorageData =
        serde_json::from_str(&stdout).map_err(|e| {
            SecureStorageError::Io(format!("deserialize keychain payload: {e}"))
        })?;
    Ok(Some(data))
}
```

(The trailing `}` above closes the `impl SecureStorage` block before the free helper. Adjust placement so the file compiles.)

- [ ] **Step 4: Run the test; verify it passes on macOS**

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_cache_test
```

Expected on macOS: both cache tests PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/posix/src/secure_storage/macos.rs lingxi-code/platforms/posix/tests/keychain_macos_cache_test.rs
git commit -m "$(cat <<'EOF'
feat(secure_storage): MacOsKeychainStorage::retrieve with 30s TTL + in-flight dedupe

Cache hit short-circuits the subprocess spawn. Concurrent retrievers
share one `security` process via tokio::sync::Notify. Generation counter
prevents a stale subprocess result from poisoning the cache when an
intervening store invalidated the entry.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: `delete` (TDD)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/secure_storage/macos.rs`
- Test: append to `keychain_macos_store_retrieve_test.rs`.

- [ ] **Step 1: Append delete-roundtrip test**

Append to `lingxi-code/platforms/posix/tests/keychain_macos_store_retrieve_test.rs`:

```rust
#[tokio::test]
async fn delete_then_retrieve_returns_none() {
    let user = std::env::var("USER").unwrap_or_else(|_| "test".into());
    let config_dir = PathBuf::from(format!("/tmp/lingxi-keychain-test-{}", std::process::id()));
    let default_dir = PathBuf::from("/Users/_lingxi_test_default/.claude");
    let storage = MacOsKeychainStorage::new(user, config_dir, default_dir, String::new())
        .await
        .expect("init");
    let account = unique_account();
    let payload = SecureStorageData::new(b"to-delete".to_vec());
    storage
        .store("-credentials", &account, payload)
        .await
        .expect("store");
    storage.delete("-credentials", &account).await.expect("delete");
    let after = storage.retrieve("-credentials", &account).await.expect("retrieve");
    assert!(after.is_none(), "deleted entry must read None");
}
```

- [ ] **Step 2: Implement `delete`**

Replace the `delete` placeholder in `macos.rs`:

```rust
    async fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<(), SecureStorageError> {
        let key = (service.to_string(), account.to_string());
        // Invalidate cache + bump generation BEFORE the subprocess so any
        // racing retrieve sees the bump and discards its result.
        self.cache.write().await.remove(&key);
        self.bump_generation();

        let full_service = self.keychain_service_name(service);
        let output = Command::new("security")
            .args([
                "delete-generic-password",
                "-a",
                self.user.as_str(),
                "-s",
                full_service.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;

        // Treat "not found" (exit 44) as success — matches claude-code's
        // `try { ... } catch { return false }` semantics where the absence
        // is not an error from the caller's perspective.
        if output.status.success() || output.status.code() == Some(44) {
            return Ok(());
        }
        Err(SecureStorageError::Backend(format!(
            "security delete-generic-password exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p lingxi-platform-posix --test keychain_macos_store_retrieve_test
```

Expected on macOS: all tests PASS.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/platforms/posix/src/secure_storage/macos.rs lingxi-code/platforms/posix/tests/keychain_macos_store_retrieve_test.rs
git commit -m "$(cat <<'EOF'
feat(secure_storage): MacOsKeychainStorage::delete + list (NotSupported)

delete shells out to `security delete-generic-password`. Exit code 44
(errSecItemNotFound) is treated as success — matches claude-code's
behavior. list returns SecureStorageError::Backend with the documented
"list not supported" message — the security CLI has no prefix query API.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `secure_storage_for_platform` factory + plaintext fallback warning

**Files:**
- Modify: `lingxi-code/platforms/posix/src/secure_storage/factory.rs`

**Critical 1:1 fidelity:** the warning string is **exactly** `"Warning: Storing credentials in plaintext."` (period included). Emitted via `tracing::warn!` with target `"lingxi::secure_storage"`. The factory also leaves a `// TODO: add libsecret support for Linux` comment matching the source quote in spec §6.6.

- [ ] **Step 1: Implement the factory**

Replace `lingxi-code/platforms/posix/src/secure_storage/factory.rs` body with:

```rust
//! Platform-default [`SecureStorage`] factory.
//!
//! On macOS, tries [`MacOsKeychainStorage`] first. On any init error
//! (e.g. `security` CLI absent on a non-default macOS, or a sandboxed
//! runtime that blocks subprocess spawn), logs the documented warning and
//! falls back to [`PlainTextSecureStorage`].
//!
//! On Linux, returns plaintext directly with a
//! `// TODO: add libsecret support for Linux` placeholder — matches claude-code's
//! Linux behavior (`auth.ts` falls back to plaintext under the same comment).

use lingxi_traits::{SecureStorage, SecureStorageError};
use std::path::PathBuf;
use std::sync::Arc;

/// Return the best available [`SecureStorage`] for the current OS.
///
/// `user` is the keychain account name (claude-code uses `$USER`).
/// `config_dir` is the user's claude config directory (`~/.claude` or the
/// `CLAUDE_CONFIG_DIR`-overridden path).
/// `plaintext_path` is the fallback file location — typically
/// `<config_dir>/.credentials.json`.
///
/// On macOS, the macOS keychain backend is attempted first. The plaintext
/// fallback emits the warning
/// `"Warning: Storing credentials in plaintext."` exactly once at
/// `tracing::warn!` level.
///
/// # Errors
/// Returns [`SecureStorageError::Io`] when the plaintext fallback cannot
/// create its base directory (typically a permission issue on `config_dir`).
pub async fn secure_storage_for_platform(
    user: String,
    config_dir: PathBuf,
    plaintext_path: PathBuf,
) -> Result<Arc<dyn SecureStorage>, SecureStorageError> {
    #[cfg(target_os = "macos")]
    {
        let default_dir = default_claude_dir();
        match super::macos::MacOsKeychainStorage::new(
            user.clone(),
            config_dir.clone(),
            default_dir,
            String::new(),
        )
        .await
        {
            Ok(keychain) => return Ok(Arc::new(keychain)),
            Err(e) => {
                tracing::warn!(
                    target: "lingxi::secure_storage",
                    error = %e,
                    "Warning: Storing credentials in plaintext."
                );
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // TODO: add libsecret support for Linux
        // (matches claude-code's comment — Linux falls back to plaintext
        //  with no further attempt).
        tracing::warn!(
            target: "lingxi::secure_storage",
            "Warning: Storing credentials in plaintext."
        );
        let _ = &user;
        let _ = &config_dir;
    }
    let plain = super::plaintext::PlainTextSecureStorage::new(plaintext_path).await?;
    Ok(Arc::new(plain))
}

#[cfg(target_os = "macos")]
fn default_claude_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".claude")
    } else {
        PathBuf::from("/.claude")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn factory_returns_storage_handle() {
        let dir = tempdir().expect("tempdir");
        let plain = dir.path().join("creds-base");
        let storage = secure_storage_for_platform(
            "test".into(),
            dir.path().to_path_buf(),
            plain,
        )
        .await
        .expect("factory");
        // is_encrypted is true on macOS keychain, false on plaintext — we
        // only check that the trait method dispatches.
        let _ = storage.is_encrypted();
    }
}
```

- [ ] **Step 2: Run the unit test**

```bash
cargo test -p lingxi-platform-posix --lib secure_storage::factory
```

Expected: PASS on macOS and Linux.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/platforms/posix/src/secure_storage/factory.rs
git commit -m "$(cat <<'EOF'
feat(secure_storage): secure_storage_for_platform factory + plaintext fallback

On macOS, attempts MacOsKeychainStorage first. If init fails (e.g. no
security CLI in sandboxed runtimes) or on Linux, falls back to
PlainTextSecureStorage with the exact claude-code warning:
"Warning: Storing credentials in plaintext."

Linux carries forward claude-code's `// TODO: add libsecret support for Linux`
comment verbatim.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Wire `KeychainPrefetch` to call the real macOS backend

**Files:**
- Modify: `lingxi-code/crates/secret/src/keychain_prefetch.rs`

**Critical 1:1 fidelity:** the prefetch service/account pair must match the macOS keychain backend, not the plaintext default. claude-code prefetches both the OAuth (`Claude Code-credentials`) and legacy API key (`Claude Code`) entries in parallel. For M2 we collapse to the OAuth entry — legacy API key prefetch is a separate concern (the engine wires it through different code paths and tracks it via the existing `KeychainPrefetch` consumer).

- [ ] **Step 1: Inspect current `keychain_prefetch.rs`**

Current code passes hard-coded `"lingxi"` / `"anthropic-api-key"` as service/account. That's incompatible with the macOS keychain backend, which expects the `service` argument to be the suffix selector (`"-credentials"` or `""`) — see Task 3.

- [ ] **Step 2: Refactor `KeychainPrefetch::start` to accept service/account**

Replace `lingxi-code/crates/secret/src/keychain_prefetch.rs` `start` body:

```rust
    /// Spawn the prefetch task on `runtime`.
    ///
    /// `service` and `account` are passed straight through to
    /// [`SecureStorage::retrieve`]. For macOS keychain warm-up, pass
    /// `service = "-credentials"` and `account = $USER` so the cache is
    /// populated with the OAuth entry. For plaintext fallback, pass
    /// whatever service/account scheme the engine uses.
    pub async fn start(
        storage: Arc<dyn SecureStorage>,
        runtime: &dyn RuntimeSpawner,
        service: impl Into<String>,
        account: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        let service = service.into();
        let account = account.into();
        let (tx, rx) = oneshot::channel();
        runtime
            .spawn(
                "keychain-prefetch",
                Box::pin(async move {
                    let _ = tx.send(storage.retrieve(&service, &account).await);
                }),
            )
            .await?;
        Ok(Self {
            rx: tokio::sync::Mutex::new(Some(rx)),
        })
    }
```

(`consume` and the rest of the struct stay untouched.)

- [ ] **Step 3: Update any callers that construct `KeychainPrefetch::start`**

Search the workspace:

```bash
rg "KeychainPrefetch::start" lingxi-code/
```

Update each call site to pass `"-credentials"` (macOS) / the legacy plaintext service name. Most likely call sites:
- `lingxi-code/crates/credential-manager/` — accepts the prefetch via constructor; pass `("-credentials", $USER)` here.
- `examples/cli-demo/` — same.

If no callers exist yet (the prefetch isn't wired into M1 cli-demo per spec §6.6 wording "real impl wired in M2-06"), this step is a no-op.

- [ ] **Step 4: Add a doc test**

Append to `keychain_prefetch.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lingxi_protocol::SecureStorageData;
    use lingxi_traits::SecureStorageBackend;
    use std::sync::Mutex;

    struct MockStorage {
        invocations: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl SecureStorage for MockStorage {
        async fn store(
            &self,
            _service: &str,
            _account: &str,
            _data: SecureStorageData,
        ) -> Result<(), SecureStorageError> {
            Ok(())
        }
        async fn retrieve(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<SecureStorageData>, SecureStorageError> {
            self.invocations
                .lock()
                .unwrap()
                .push((service.into(), account.into()));
            Ok(None)
        }
        async fn delete(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<(), SecureStorageError> {
            Ok(())
        }
        async fn list(&self, _service: &str) -> Result<Vec<String>, SecureStorageError> {
            Ok(vec![])
        }
        fn is_encrypted(&self) -> bool {
            false
        }
        fn backend(&self) -> SecureStorageBackend {
            SecureStorageBackend::PlainText
        }
    }

    struct TokioSpawner;
    #[async_trait]
    impl RuntimeSpawner for TokioSpawner {
        async fn spawn(
            &self,
            _name: &'static str,
            fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
        ) -> Result<(), RuntimeError> {
            tokio::spawn(fut);
            Ok(())
        }
    }

    #[tokio::test]
    async fn start_passes_service_and_account_through_to_storage() {
        let storage = Arc::new(MockStorage {
            invocations: Mutex::new(vec![]),
        });
        let spawner = TokioSpawner;
        let prefetch = KeychainPrefetch::start(
            storage.clone() as Arc<dyn SecureStorage>,
            &spawner,
            "-credentials",
            "alice",
        )
        .await
        .expect("start");
        let _ = prefetch.consume().await;
        let inv = storage.invocations.lock().unwrap();
        assert_eq!(
            inv.as_slice(),
            &[("-credentials".to_string(), "alice".to_string())]
        );
    }
}
```

(`RuntimeSpawner`'s actual signature may differ slightly — adapt the mock to match the real trait. The point is to verify service/account are passed straight through.)

- [ ] **Step 5: Run the test**

```bash
cargo test -p lingxi-secret keychain_prefetch
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/crates/secret/src/keychain_prefetch.rs
git commit -m "$(cat <<'EOF'
feat(secret): keychain prefetch accepts service/account params

Removes the hard-coded ("lingxi", "anthropic-api-key") tuple so the
prefetch task can target the real macOS keychain service name
("-credentials" suffix) introduced in this plan. Existing callers update
to pass the OAuth credentials entry explicitly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Phase A consolidation commit (squash trail)

**Files:** none.

- [ ] **Step 1: Confirm the per-task commits cover the Phase A `feat(secure_storage): macOS Keychain via security CLI` story**

```bash
git log --oneline main..HEAD | grep -i secure_storage
```

Expected: 7 commits (Tasks 1-8). These are the squash candidates for the spec-mandated single `feat(secure_storage): macOS Keychain via security CLI with 30s TTL cache + in-flight dedupe` commit. The squash itself happens in Task 27 below.

- [ ] **Step 2: Quick sanity gate**

```bash
cargo clippy -p lingxi-platform-posix -p lingxi-secret -- -D warnings
cargo fmt --all --check
```

Expected: clean. Fix any drift inline.

---

## Phase B — HTTP SSE Streaming

### Task 10: Widen `StreamEvent` / `ContentDelta` / `ContentBlockApi` (TDD)

**Files:**
- Modify: `lingxi-code/crates/api-client/src/types.rs`
- Modify: `lingxi-code/crates/api-client/src/lib.rs` (re-exports if any)

**Critical 1:1 fidelity** (from claude-code `services/api/claude.ts:1995-2295`):
- `ContentBlockApi` variants: `Text`, `ToolUse`, `Thinking`, `ServerToolUse`, `ConnectorText`, `AdvisorToolResult`.
- `ContentDelta` variants: `TextDelta`, `InputJsonDelta`, `ThinkingDelta`, `SignatureDelta`, `CitationsDelta`, `ConnectorTextDelta`.
- Serde tag/rename: `#[serde(tag = "type", rename_all = "snake_case")]` on both enums.
- All new fields with `#[serde(default)]` so a future server-added variant or missing-field never breaks deserialization.

- [ ] **Step 1: Write the failing roundtrip tests**

Append to `lingxi-code/crates/api-client/src/types.rs` `mod tests`:

```rust
#[cfg(test)]
mod stream_event_v2_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_content_block_roundtrip() {
        let raw = json!({
            "type": "thinking",
            "thinking": "let me think...",
            "signature": "sig-abc123"
        });
        let block: ContentBlockApi = serde_json::from_value(raw.clone()).expect("decode");
        match &block {
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "let me think...");
                assert_eq!(signature.as_deref(), Some("sig-abc123"));
            }
            _ => panic!("expected Thinking"),
        }
        let re_encoded = serde_json::to_value(&block).expect("encode");
        assert_eq!(re_encoded, raw);
    }

    #[test]
    fn server_tool_use_content_block_decodes() {
        let raw = json!({
            "type": "server_tool_use",
            "id": "stu_01",
            "name": "advisor",
            "input": {"query": "?"}
        });
        let block: ContentBlockApi = serde_json::from_value(raw).expect("decode");
        assert!(matches!(block, ContentBlockApi::ServerToolUse { .. }));
    }

    #[test]
    fn connector_text_block_decodes() {
        let raw = json!({
            "type": "connector_text",
            "connector_text": "[connector] hi",
            "signature": "ct-sig"
        });
        let block: ContentBlockApi = serde_json::from_value(raw).expect("decode");
        assert!(matches!(block, ContentBlockApi::ConnectorText { .. }));
    }

    #[test]
    fn advisor_tool_result_block_decodes() {
        let raw = json!({
            "type": "advisor_tool_result",
            "tool_use_id": "stu_01",
            "content": "result",
            "is_error": false
        });
        let block: ContentBlockApi = serde_json::from_value(raw).expect("decode");
        assert!(matches!(block, ContentBlockApi::AdvisorToolResult { .. }));
    }

    #[test]
    fn signature_delta_decodes() {
        let raw = json!({"type": "signature_delta", "signature": "sig"});
        let d: ContentDelta = serde_json::from_value(raw).expect("decode");
        assert!(matches!(d, ContentDelta::SignatureDelta { .. }));
    }

    #[test]
    fn citations_delta_decodes_with_arbitrary_citation_shape() {
        let raw = json!({
            "type": "citations_delta",
            "citation": {"url": "https://x", "title": "X"}
        });
        let d: ContentDelta = serde_json::from_value(raw).expect("decode");
        assert!(matches!(d, ContentDelta::CitationsDelta { .. }));
    }

    #[test]
    fn connector_text_delta_decodes() {
        let raw = json!({
            "type": "connector_text_delta",
            "connector_text": " more"
        });
        let d: ContentDelta = serde_json::from_value(raw).expect("decode");
        assert!(matches!(d, ContentDelta::ConnectorTextDelta { .. }));
    }
}
```

- [ ] **Step 2: Run the tests; verify they fail (variants missing)**

```bash
cargo test -p lingxi-api-client types::stream_event_v2_tests
```

Expected: compile errors `no variant ServerToolUse / ConnectorText / AdvisorToolResult / SignatureDelta / CitationsDelta / ConnectorTextDelta`.

- [ ] **Step 3: Extend the enums**

In `lingxi-code/crates/api-client/src/types.rs`, extend `ContentBlockApi`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockApi {
    Text {
        text: String,
    },
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Anthropic server-side tool invocation (e.g. advisor).
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// Anthropic Connector-Text block (gated by `CONNECTOR_TEXT` feature flag
    /// in claude-code; we always accept it on the wire).
    ConnectorText {
        #[serde(default)]
        connector_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Advisor tool result, mirrored from the server.
    AdvisorToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: bool,
    },
}
```

Extend `ContentDelta`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    CitationsDelta {
        citation: Value,
    },
    ConnectorTextDelta {
        connector_text: String,
    },
}
```

- [ ] **Step 4: Run the tests; verify all pass**

```bash
cargo test -p lingxi-api-client
```

Expected: all variants decode round-trip; old tests (text-delta, input-json-delta, message_start/stop) still pass.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/crates/api-client/src/types.rs lingxi-code/crates/api-client/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(api-client): widen StreamEvent for claude-code parity

Adds ContentBlockApi::{ServerToolUse, ConnectorText, AdvisorToolResult}
and ContentDelta::{SignatureDelta, CitationsDelta, ConnectorTextDelta}.
Mirrors claude-code's content_block_start/delta type matrix in
services/api/claude.ts:1995-2295. All new fields are #[serde(default)]
for forward compatibility with server-side schema additions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Real `stream_sse` in `platforms/posix/src/http.rs` (TDD with mock server)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/http.rs`
- Test: `lingxi-code/platforms/posix/tests/http_stream_sse_test.rs`

- [ ] **Step 1: Write the failing test**

Create `lingxi-code/platforms/posix/tests/http_stream_sse_test.rs`:

```rust
//! `PosixHttp::stream_sse` end-to-end against a local hyper server.

use futures_util::StreamExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::{CACHE_CONTROL, CONTENT_TYPE};
use hyper::{server::conn::http1, service::service_fn, Response};
use hyper_util::rt::TokioIo;
use lingxi_platform_posix::PosixHttp;
use lingxi_protocol::{HttpMethod, HttpRequest};
use lingxi_traits::HttpTransport;
use std::time::Duration;
use tokio::net::TcpListener;

async fn spawn_sse_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let _ = http1::Builder::new()
                    .keep_alive(false)
                    .serve_connection(
                        io,
                        service_fn(|_req| async {
                            let body = concat!(
                                "event: message_start\n",
                                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"x\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                                "event: content_block_delta\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
                                "event: content_block_delta\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reasoning\"}}\n\n",
                                "event: message_stop\n",
                                "data: {\"type\":\"message_stop\"}\n\n",
                            );
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .header(CONTENT_TYPE, "text/event-stream")
                                    .header(CACHE_CONTROL, "no-cache")
                                    .body(Full::new(Bytes::from(body)))
                                    .expect("response"),
                            )
                        }),
                    )
                    .await;
            });
        }
    });
    port
}

#[tokio::test]
async fn stream_sse_parses_message_start_and_deltas() {
    let port = spawn_sse_server().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let http = PosixHttp::new();
    let req = HttpRequest {
        method: HttpMethod::Post,
        url: format!("http://127.0.0.1:{port}/v1/messages"),
        headers: vec![],
        body: Some("{\"stream\":true}".into()),
        timeout: Some(Duration::from_secs(5)),
    };

    let mut stream = http.stream_sse(req).await.expect("stream open");
    let mut got = Vec::new();
    while let Some(item) = stream.next().await {
        let event = item.expect("event");
        got.push(event.event_type.clone().unwrap_or_default());
    }
    assert_eq!(
        got,
        vec![
            "message_start".to_string(),
            "content_block_delta".to_string(),
            "content_block_delta".to_string(),
            "message_stop".to_string(),
        ]
    );
}
```

- [ ] **Step 2: Run the test; verify it fails (returns InvalidRequest)**

```bash
cargo test -p lingxi-platform-posix --test http_stream_sse_test
```

Expected: error `invalid request: posix: stream_sse not yet wired`.

- [ ] **Step 3: Implement `stream_sse`**

Replace the `stream_sse` body in `lingxi-code/platforms/posix/src/http.rs`:

```rust
    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
        let method = match req.method {
            lingxi_protocol::HttpMethod::Get => reqwest::Method::GET,
            lingxi_protocol::HttpMethod::Post => reqwest::Method::POST,
            lingxi_protocol::HttpMethod::Put => reqwest::Method::PUT,
            lingxi_protocol::HttpMethod::Patch => reqwest::Method::PATCH,
            lingxi_protocol::HttpMethod::Delete => reqwest::Method::DELETE,
            lingxi_protocol::HttpMethod::Head => reqwest::Method::HEAD,
            lingxi_protocol::HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(body) = req.body {
            rb = rb.body(body);
        }
        if let Some(timeout) = req.timeout {
            rb = rb.timeout(timeout);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            // Surface the status to the caller — claude-code returns this as
            // an error event on the same stream, but we keep them separate
            // because the trait promises Result<SseStream, HttpError> at the
            // open boundary, not after the first event.
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpError::Status { status, body });
        }

        let byte_stream = resp.bytes_stream();
        let event_stream = sse_event_stream(byte_stream);
        Ok(Box::pin(event_stream))
    }
```

Add (anywhere in the same file, but conventionally below the impl block):

```rust
use bytes::BytesMut;
use futures_core::stream::Stream;
use futures_util::stream::StreamExt;
use lingxi_protocol::SseEvent;

fn sse_event_stream<S>(byte_stream: S) -> impl Stream<Item = Result<SseEvent, HttpError>> + Send
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    let buffer = BytesMut::new();
    futures_util::stream::unfold(
        (Box::pin(byte_stream), buffer),
        |(mut s, mut buf)| async move {
            loop {
                // First, drain any complete events already in the buffer.
                if let Some(boundary) = find_event_boundary(&buf) {
                    let event_bytes = buf.split_to(boundary).to_vec();
                    // Consume the boundary itself.
                    drop(buf.split_to(boundary_len(&buf, boundary)));
                    let chunk = String::from_utf8_lossy(&event_bytes).to_string();
                    let events =
                        lingxi_api_client::sse::parse_sse_chunks(&format!("{chunk}\n\n"));
                    if let Some(ev) = events.into_iter().next() {
                        return Some((Ok(ev), (s, buf)));
                    }
                    continue;
                }
                // Otherwise pull more bytes.
                match s.next().await {
                    Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
                    Some(Err(e)) => {
                        return Some((Err(HttpError::Connection(e.to_string())), (s, buf)))
                    }
                    None => return None,
                }
            }
        },
    )
}

fn find_event_boundary(buf: &BytesMut) -> Option<usize> {
    // SSE event boundary is `\n\n` or `\r\n\r\n`. Return the index of the
    // first byte of the boundary so the caller can `split_to(idx)` and pass
    // the chunk to `parse_sse_chunks`.
    let bytes = buf.as_ref();
    bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .or_else(|| bytes.windows(4).position(|w| w == b"\r\n\r\n"))
}

fn boundary_len(buf: &BytesMut, boundary_index_ignored: usize) -> usize {
    // After `split_to(boundary)`, the boundary itself remains at the head of
    // the buffer. Determine whether it's `\n\n` (2) or `\r\n\r\n` (4).
    let _ = boundary_index_ignored;
    let head = buf.as_ref();
    if head.starts_with(b"\r\n\r\n") {
        4
    } else if head.starts_with(b"\n\n") {
        2
    } else {
        0
    }
}
```

(The `parse_sse_chunks` helper from `crates/api-client/src/sse.rs` already exists from M1 — we just call it with one event at a time.)

- [ ] **Step 4: Confirm the `lingxi-api-client` dep**

```bash
grep -q 'lingxi-api-client' lingxi-code/platforms/posix/Cargo.toml || echo "needs lingxi-api-client"
```

If missing:

```toml
lingxi-api-client = { path = "../../crates/api-client" }
```

(Currently `platforms/posix` doesn't depend on `api-client` because the SSE parser lived there; we import it now.)

- [ ] **Step 5: Run the SSE test**

```bash
cargo test -p lingxi-platform-posix --test http_stream_sse_test
```

Expected: PASS — 4 events parsed in order.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/platforms/posix/Cargo.toml lingxi-code/platforms/posix/src/http.rs lingxi-code/platforms/posix/tests/http_stream_sse_test.rs
git commit -m "$(cat <<'EOF'
feat(http): real stream_sse on platform-posix via reqwest::bytes_stream

Replaces the `Err(InvalidRequest("stream_sse not yet wired"))` stub with
a bytes_stream → SSE-event adapter that buffers until \n\n / \r\n\r\n
boundaries and delegates to crates/api-client/src/sse::parse_sse_chunks.
Surfaces non-2xx status as HttpError::Status at stream-open time so the
caller can branch before the first event.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Mirror `stream_sse` in `platforms/windows/src/http.rs`

**Files:**
- Modify: `lingxi-code/platforms/windows/src/http.rs`
- Modify: `lingxi-code/platforms/windows/Cargo.toml`
- Test (optional smoke): `lingxi-code/platforms/windows/tests/http_stream_sse_smoke_test.rs`

- [ ] **Step 1: Copy the helper functions verbatim**

The `sse_event_stream`, `find_event_boundary`, `boundary_len` helpers are platform-agnostic. Copy them from posix into the windows `http.rs` (or extract into a shared `watch_common`-style file; we keep duplication here because the two platform crates share no library).

- [ ] **Step 2: Replace the windows `stream_sse` body**

Same diff as Task 11 Step 3 applied to `lingxi-code/platforms/windows/src/http.rs`. Add the `bytes` and `lingxi-api-client` deps to `platforms/windows/Cargo.toml` mirroring posix:

```toml
bytes = "1"
lingxi-api-client = { path = "../../crates/api-client" }
```

- [ ] **Step 3: Add a smoke test (host-portable)**

Create `lingxi-code/platforms/windows/tests/http_stream_sse_smoke_test.rs` as a copy of the posix one with `PosixHttp` replaced by `WindowsHttp`. (Hyper test server runs the same on any host since notify abstracts the OS for FS; here reqwest abstracts the OS for HTTP.)

- [ ] **Step 4: Run the test**

```bash
cargo test -p lingxi-platform-windows --test http_stream_sse_smoke_test
```

Expected: PASS on macOS/Linux dev hosts; PASS on Windows in CI.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/windows/Cargo.toml lingxi-code/platforms/windows/src/http.rs lingxi-code/platforms/windows/tests/http_stream_sse_smoke_test.rs
git commit -m "$(cat <<'EOF'
feat(http): mirror real stream_sse in platform-windows

Verbatim copy of the posix SSE-event adapter. notify-style duplication
keeps the platform crates library-less. Smoke test verifies the adapter
parses the same hyper-emitted events.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase C — Process Spawn Polish

### Task 13: Refactor `process.rs` → `process/` directory + unsafe setsid module

**Files:**
- Modify: `lingxi-code/platforms/posix/src/process.rs` → delete
- Create: `lingxi-code/platforms/posix/src/process/{mod.rs,runner.rs,spawn_unsafe.rs,kill_tree.rs,wrap.rs}` (stubs first; bodies in Tasks 14-18)
- Modify: `lingxi-code/platforms/posix/src/lib.rs` (`pub mod process;`)

- [ ] **Step 1: Move existing process.rs body into `runner.rs`**

```bash
mkdir -p lingxi-code/platforms/posix/src/process
git mv lingxi-code/platforms/posix/src/process.rs lingxi-code/platforms/posix/src/process/runner.rs
```

Update the doc comment at the top of `runner.rs`:

```rust
//! `tokio::process`-backed [`ProcessRunner`] for desktop hosts.
//!
//! Foreground `run` is unchanged from v0.2.0. Background `spawn_background`
//! and `kill` are wired to the helpers in this directory:
//! [`super::kill_tree::kill_tree_unix`] and the env-vars / cwd-tracking
//! wrappers in [`super::wrap`]. The single `unsafe` block needed to call
//! `libc::setsid()` from `CommandExt::pre_exec` lives in
//! [`super::spawn_unsafe::pre_exec_setsid`].
```

- [ ] **Step 2: Create `mod.rs`**

`lingxi-code/platforms/posix/src/process/mod.rs`:

```rust
//! Process spawn for desktop hosts.

pub mod kill_tree;
pub mod runner;
pub mod spawn_unsafe;
pub mod wrap;

pub use runner::PosixProcess;
pub use wrap::{
    task_output_path, wrap_command_for_cwd_tracking, DEFAULT_TIMEOUT, ENV_CLAUDECODE,
    ENV_CLAUDE_CODE_SESSION_ID, ENV_GIT_EDITOR, ENV_SHELL,
};
```

- [ ] **Step 3: Stub the new submodules**

`spawn_unsafe.rs`:

```rust
//! Filled in by Task 14.

#![allow(unsafe_code)]
```

`kill_tree.rs`:

```rust
//! Filled in by Task 15.

use lingxi_traits::ProcessError;

pub async fn kill_tree_unix(_pid: u32) -> Result<(), ProcessError> {
    Err(ProcessError::Unsupported)
}
```

`wrap.rs`:

```rust
//! Filled in by Task 16.

use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const ENV_CLAUDECODE: (&str, &str) = ("CLAUDECODE", "1");
pub const ENV_GIT_EDITOR: (&str, &str) = ("GIT_EDITOR", "true");
pub const ENV_SHELL: &str = "SHELL";
pub const ENV_CLAUDE_CODE_SESSION_ID: &str = "CLAUDE_CODE_SESSION_ID";

pub fn task_output_path(_task_id: &str) -> PathBuf {
    // Filled in by Task 16.
    std::env::temp_dir().join("lingxi-task-output-placeholder")
}

pub fn wrap_command_for_cwd_tracking(_cmd: &str, _cwd_file: &std::path::Path) -> String {
    // Filled in by Task 17.
    String::new()
}
```

- [ ] **Step 4: Update `lib.rs`**

In `lingxi-code/platforms/posix/src/lib.rs`, replace `pub mod process;` re-export and `pub use process::PosixProcess;` to match the new module path. The simplest form (with the new `mod.rs` re-exporting `PosixProcess`):

```rust
pub mod process;
pub use process::PosixProcess;
```

(No change visible to downstream consumers.)

- [ ] **Step 5: Verify `cargo check`**

```bash
cargo check -p lingxi-platform-posix
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/platforms/posix/src/process/ lingxi-code/platforms/posix/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(platform-posix): split process.rs into process/ directory

Moves the v0.2.0 process.rs body into process/runner.rs and adds empty
sibling modules (spawn_unsafe, kill_tree, wrap) that subsequent Phase C
tasks fill in. spawn_unsafe is the only place in the crate that allows
`unsafe` — the rest of the crate keeps `deny(unsafe_code)`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: `pre_exec_setsid` in `spawn_unsafe.rs`

**Files:**
- Modify: `lingxi-code/platforms/posix/src/process/spawn_unsafe.rs`

**Critical 1:1 fidelity:** the closure passed to `CommandExt::pre_exec` runs in the forked child between `fork()` and `execve()` — it MUST be async-signal-safe. `libc::setsid()` is async-signal-safe (it's listed in POSIX's safe table). No allocation, no logging, no tokio.

- [ ] **Step 1: Implement**

Replace `lingxi-code/platforms/posix/src/process/spawn_unsafe.rs`:

```rust
//! The single module in `lingxi-platform-posix` that uses `unsafe`.
//!
//! `unsafe` here calls `libc::setsid(3)` from `CommandExt::pre_exec`.
//! `pre_exec` runs in the forked child between `fork()` and `execve()` —
//! the closure MUST be async-signal-safe (POSIX.1-2017 §2.4.3). `setsid` is
//! in POSIX's async-signal-safe function list, so this is sound. No
//! allocation, no logging, no tokio — only the syscall.
//!
//! Why we need it: `tree-kill` in [`super::kill_tree`] sends SIGTERM /
//! SIGKILL to a process group via `killpg(2)`. For that to terminate the
//! entire descendant tree of a background process, the child must be a
//! process-group leader (pid == pgid). Calling `setsid()` immediately after
//! `fork()` makes the child the session and process-group leader.
//!
//! Matches claude-code's `detached: provider.detached` spawn option in
//! `Shell.ts:334` — Node's `detached: true` invokes `setsid()` under the
//! hood on POSIX.

#![allow(unsafe_code)]

use std::io;
use std::os::unix::process::CommandExt;
use tokio::process::Command;

/// Install a `pre_exec` callback on `cmd` that calls `setsid()` in the
/// child. Idempotent — call once per command.
pub fn attach_setsid(cmd: &mut Command) {
    // SAFETY: setsid() is async-signal-safe per POSIX.1-2017 §2.4.3 Table 2-5,
    // and we make no other calls inside the closure. The closure runs in the
    // forked child between fork() and execve(); no Rust runtime state is
    // shared and no heap allocation happens.
    unsafe {
        cmd.pre_exec(|| -> io::Result<()> {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
```

Add `libc = "0.2"` to `platforms/posix/Cargo.toml` `[dependencies]` if not already present (M1 plans likely added it; verify).

- [ ] **Step 2: Verify**

```bash
cargo check -p lingxi-platform-posix
```

Expected: clean. The `#[allow(unsafe_code)]` at the top of the module satisfies the crate-level `#![deny(unsafe_code)]`.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/platforms/posix/src/process/spawn_unsafe.rs lingxi-code/platforms/posix/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(process): attach_setsid via CommandExt::pre_exec

The one place in platform-posix that uses unsafe. setsid() runs
async-signal-safely between fork() and execve() to make the child a
process-group leader, so kill_tree's killpg(pgid, SIGTERM) terminates
the whole descendant tree. Matches claude-code's `detached: true` spawn
option on POSIX.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 15: `kill_tree_unix` via `killpg(2)` (TDD)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/process/kill_tree.rs`
- Test: `lingxi-code/platforms/posix/tests/process_kill_tree_test.rs`

**Critical 1:1 fidelity:**
- `killpg` argument is the **POSITIVE** pgid — children spawned with `setsid()` have pid == pgid, so we pass the child's pid as the pgid. NOT a negative pid to `kill(2)`.
- Send SIGTERM, wait 5 s, escalate to SIGKILL.
- Treat "no such process" (ESRCH) on the second signal as success.

- [ ] **Step 1: Write the failing test**

Create `lingxi-code/platforms/posix/tests/process_kill_tree_test.rs`:

```rust
//! kill_tree_unix kills the whole process group of a setsid-detached child.

#![cfg(unix)]

use lingxi_platform_posix::process::kill_tree::kill_tree_unix;
use std::os::unix::process::CommandExt;
use std::time::Duration;
use tokio::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_tree_terminates_grandchildren() {
    // Spawn a shell that forks two grandchildren and waits.
    // Use a Mac/Linux-portable shell idiom; `sleep 60` is the grandchildren.
    let script = "sleep 60 & sleep 60 & wait";
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(script);
    // We need the child to be its own process group leader so killpg(pid, ...)
    // hits the whole tree.
    // SAFETY: identical to spawn_unsafe::attach_setsid — async-signal-safe.
    unsafe {
        cmd.pre_exec(|| -> std::io::Result<()> {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().expect("spawn");
    let pid = child.id().expect("pid");

    // Give the shell a moment to fork its sleeps.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Kill the whole tree.
    kill_tree_unix(pid).await.expect("kill_tree");

    // The parent shell must exit promptly.
    let outcome = tokio::time::timeout(Duration::from_secs(7), child.wait())
        .await
        .expect("child exits within 7s after kill_tree")
        .expect("wait");
    assert!(!outcome.success() || outcome.code().is_some());

    // Verify no `sleep 60` lingers. ps -A finds all on macOS + Linux.
    let ps = std::process::Command::new("ps")
        .args(["-A", "-o", "pid,command"])
        .output()
        .expect("ps");
    let stdout = String::from_utf8_lossy(&ps.stdout);
    let leaked: Vec<_> = stdout
        .lines()
        .filter(|line| line.contains("sleep 60"))
        .collect();
    assert!(
        leaked.is_empty(),
        "leaked sleep 60 processes after kill_tree: {leaked:?}"
    );
}
```

- [ ] **Step 2: Run the test; verify it fails (kill_tree_unix is a stub)**

```bash
cargo test -p lingxi-platform-posix --test process_kill_tree_test
```

Expected: `ProcessError::Unsupported`.

- [ ] **Step 3: Implement**

Replace `lingxi-code/platforms/posix/src/process/kill_tree.rs`:

```rust
//! Tree-kill via `killpg(2)` on Unix.
//!
//! Children spawned with `setsid()` (see [`super::spawn_unsafe::attach_setsid`])
//! are process-group leaders — their PID equals their PGID. We pass the
//! POSITIVE PGID to `killpg(2)`, which delivers the signal to every member
//! of the group, including any descendants the child has forked.
//!
//! Sequence: SIGTERM → 5 s grace → SIGKILL. Matches claude-code's
//! `treeKill(pid, 'SIGKILL')` semantics but with a polite SIGTERM first
//! (the node `tree-kill` library's default sequence is similar).

use lingxi_traits::ProcessError;
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use std::time::Duration;

/// Kill every member of the process group `pid`. Sends SIGTERM, waits 5
/// seconds for graceful exit, then sends SIGKILL.
///
/// `pid` MUST be the PGID — typically the PID of a child spawned with
/// `setsid()`, in which case PID == PGID. Passing a non-leader PID will
/// only signal that one process.
///
/// # Errors
/// Returns [`ProcessError::Io`] when the SIGTERM call fails with anything
/// other than `ESRCH` (already dead). `ESRCH` from the SIGKILL escalation
/// is treated as success.
pub async fn kill_tree_unix(pid: u32) -> Result<(), ProcessError> {
    let pgid = Pid::from_raw(i32::try_from(pid).map_err(|_| {
        ProcessError::Io(format!("pid {pid} does not fit in i32"))
    })?);

    // SIGTERM first.
    match killpg(pgid, Signal::SIGTERM) {
        Ok(()) => {}
        Err(nix::errno::Errno::ESRCH) => return Ok(()),
        Err(e) => return Err(ProcessError::Io(format!("killpg SIGTERM: {e}"))),
    }

    tokio::time::sleep(Duration::from_secs(5)).await;

    // SIGKILL escalation. Tolerate ESRCH — the group is already gone.
    match killpg(pgid, Signal::SIGKILL) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(e) => Err(ProcessError::Io(format!("killpg SIGKILL: {e}"))),
    }
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p lingxi-platform-posix --test process_kill_tree_test
```

Expected: PASS on macOS/Linux. (Test self-skips on non-Unix via `#![cfg(unix)]`.)

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/posix/src/process/kill_tree.rs lingxi-code/platforms/posix/tests/process_kill_tree_test.rs
git commit -m "$(cat <<'EOF'
feat(process): kill_tree_unix via killpg(2) (SIGTERM → 5s → SIGKILL)

Children spawned with setsid() are process-group leaders (PID == PGID),
so we pass the POSITIVE pid to killpg — NOT a negative pid to kill(2).
Integration test forks two sleeps under one shell and verifies all three
processes are gone after kill_tree returns.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 16: `task_output_path` + cwd-tracking wrapper (TDD)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/process/wrap.rs`
- Test: `lingxi-code/platforms/posix/tests/process_cwd_tracking_test.rs`

**Critical 1:1 fidelity:**
- bash extglob disable: `shopt -u extglob 2>/dev/null || true`
- zsh extglob disable: `setopt NO_EXTENDED_GLOB 2>/dev/null || true`
- When `CLAUDE_CODE_SHELL_PREFIX` is set (wrapper may use a different shell than shellPath): `{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true`.
- cwd tracking appends `&& pwd -P >| <cwd_file>`. The `>|` forces clobber even with `noclobber`. cwd_file path is shell-escaped (single-quoted with internal single-quotes replaced by `'\''`).
- Task output path: `<temp>/lingxi-task-output/<task_id>.out` matching `getTaskOutputPath` in claude-code (without forcing per-platform tmpdir overrides — keep simple).

- [ ] **Step 1: Write the failing test**

Create `lingxi-code/platforms/posix/tests/process_cwd_tracking_test.rs`:

```rust
//! Tests for wrap_command_for_cwd_tracking and task_output_path.

use lingxi_platform_posix::process::{task_output_path, wrap_command_for_cwd_tracking};
use std::path::Path;

#[test]
fn bash_wrap_includes_extglob_disable_and_pwd_tail() {
    let cwd_file = Path::new("/tmp/cwd.txt");
    let wrapped = wrap_command_for_cwd_tracking("ls -la", cwd_file, "/bin/bash", false);
    assert!(
        wrapped.contains("shopt -u extglob 2>/dev/null || true"),
        "missing bash extglob disable: {wrapped}"
    );
    assert!(
        wrapped.contains(r#"pwd -P >| '/tmp/cwd.txt'"#),
        "missing pwd -P tail: {wrapped}"
    );
    assert!(wrapped.contains("ls -la"), "user command must be present");
}

#[test]
fn zsh_wrap_uses_zsh_idiom() {
    let cwd_file = Path::new("/tmp/cwd.txt");
    let wrapped = wrap_command_for_cwd_tracking("echo hi", cwd_file, "/bin/zsh", false);
    assert!(
        wrapped.contains("setopt NO_EXTENDED_GLOB 2>/dev/null || true"),
        "missing zsh extglob disable: {wrapped}"
    );
}

#[test]
fn shell_prefix_uses_combined_idiom() {
    let cwd_file = Path::new("/tmp/cwd.txt");
    let wrapped = wrap_command_for_cwd_tracking("echo hi", cwd_file, "/bin/bash", true);
    assert!(
        wrapped.contains("{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true"),
        "missing combined extglob disable: {wrapped}"
    );
}

#[test]
fn cwd_file_path_with_single_quote_is_escaped() {
    let cwd_file = Path::new("/tmp/it's a cwd.txt");
    let wrapped = wrap_command_for_cwd_tracking("ls", cwd_file, "/bin/bash", false);
    // Bash single-quote escape: replace ' with '\''
    assert!(
        wrapped.contains(r#"pwd -P >| '/tmp/it'\''s a cwd.txt'"#),
        "single-quote not shell-escaped: {wrapped}"
    );
}

#[test]
fn task_output_path_uses_unique_task_id() {
    let p = task_output_path("abc123");
    let s = p.to_string_lossy();
    assert!(s.contains("lingxi-task-output"));
    assert!(s.ends_with("abc123.out"));
}
```

- [ ] **Step 2: Run the test; verify it fails (stubs)**

```bash
cargo test -p lingxi-platform-posix --test process_cwd_tracking_test
```

Expected: assertions fail because `wrap_command_for_cwd_tracking` returns `""` and `task_output_path` returns the placeholder path.

- [ ] **Step 3: Implement `wrap.rs`**

Replace `lingxi-code/platforms/posix/src/process/wrap.rs`:

```rust
//! Command wrapping helpers for cwd tracking + extglob disable + env vars.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 30-minute default timeout matches claude-code's
/// `DEFAULT_TIMEOUT = 30 * 60 * 1000` in `Shell.ts:44`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// `(name, value)` tuples for the env-vars claude-code injects into every
/// `Shell.execute` spawn. The runtime impl in [`super::runner`] applies
/// these on top of the caller-supplied env.
pub const ENV_CLAUDECODE: (&str, &str) = ("CLAUDECODE", "1");
pub const ENV_GIT_EDITOR: (&str, &str) = ("GIT_EDITOR", "true");
pub const ENV_SHELL: &str = "SHELL";
pub const ENV_CLAUDE_CODE_SESSION_ID: &str = "CLAUDE_CODE_SESSION_ID";

/// Stable per-task output file path.
///
/// `<temp>/lingxi-task-output/<task_id>.out`. The directory is created on
/// first use by [`super::runner`].
#[must_use]
pub fn task_output_path(task_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("lingxi-task-output")
        .join(format!("{task_id}.out"))
}

/// Wrap a user command for the bash-tool spawn so we can track cwd
/// changes and disable extended globs.
///
/// Layout per claude-code's `bashProvider.ts:156-187`:
/// ```text
/// <extglob_disable> && eval '<command>' && pwd -P >| '<cwd_file>'
/// ```
///
/// `shell_path` is the absolute path of the spawn binary (`/bin/bash`,
/// `/bin/zsh`, …) so we pick the right idiom. When
/// `claude_code_shell_prefix_set` is true (caller's
/// `CLAUDE_CODE_SHELL_PREFIX` env is non-empty), the combined bash+zsh
/// idiom is used because the wrapper may pick a different shell than
/// `shell_path`.
#[must_use]
pub fn wrap_command_for_cwd_tracking(
    command: &str,
    cwd_file: &Path,
    shell_path: &str,
    claude_code_shell_prefix_set: bool,
) -> String {
    let extglob = if claude_code_shell_prefix_set {
        Some(
            "{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true"
                .to_string(),
        )
    } else if shell_path.contains("bash") {
        Some("shopt -u extglob 2>/dev/null || true".to_string())
    } else if shell_path.contains("zsh") {
        Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true".to_string())
    } else {
        None
    };

    let escaped_cwd_file = shell_single_quote(&cwd_file.to_string_lossy());
    let pwd_tail = format!("pwd -P >| {escaped_cwd_file}");

    match extglob {
        Some(disable) => format!("{disable} && {command} && {pwd_tail}"),
        None => format!("{command} && {pwd_tail}"),
    }
}

/// Single-quote a string for bash. Replaces interior `'` with `'\''`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str(r#"'\''"#);
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_is_30_minutes() {
        assert_eq!(DEFAULT_TIMEOUT.as_secs(), 30 * 60);
    }

    #[test]
    fn env_constants_match_claude_code() {
        assert_eq!(ENV_CLAUDECODE, ("CLAUDECODE", "1"));
        assert_eq!(ENV_GIT_EDITOR, ("GIT_EDITOR", "true"));
        assert_eq!(ENV_SHELL, "SHELL");
        assert_eq!(ENV_CLAUDE_CODE_SESSION_ID, "CLAUDE_CODE_SESSION_ID");
    }

    #[test]
    fn shell_single_quote_handles_apostrophes() {
        assert_eq!(shell_single_quote("a'b"), r#"'a'\''b'"#);
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p lingxi-platform-posix --test process_cwd_tracking_test
cargo test -p lingxi-platform-posix --lib process::wrap
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/posix/src/process/wrap.rs lingxi-code/platforms/posix/tests/process_cwd_tracking_test.rs
git commit -m "$(cat <<'EOF'
feat(process): wrap_command_for_cwd_tracking + task_output_path + env constants

Reproduces claude-code's bashProvider.ts wrapping idiom:
  <extglob_disable> && <user command> && pwd -P >| '<cwd_file>'
with per-shell extglob disable strings and combined idiom when
CLAUDE_CODE_SHELL_PREFIX is set. cwd_file paths are shell-single-quoted
so paths with apostrophes don't break the spawn.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 17: `spawn_background` + run polish (env vars, file-mode stdio, 30-min timeout)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/process/runner.rs`
- Test: `lingxi-code/platforms/posix/tests/process_spawn_background_test.rs`
- Test: `lingxi-code/platforms/posix/tests/process_spawn_env_test.rs`

**Critical 1:1 fidelity:**
- Spawn env: caller env is preserved; `CLAUDECODE=1`, `GIT_EDITOR=true`, `SHELL=<inner.command>` are added.
- `CLAUDE_CODE_SESSION_ID` is added when the caller injects it (we accept it through the existing `ProcessCommand::env` map; the engine layer decides whether to set it).
- 30-minute default timeout when `inner.timeout` is `None`.
- File-mode stdio for background tasks uses `O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW` on POSIX.
- Background spawn calls `attach_setsid` so `kill_tree` can later kill the whole group.
- `kill(&handle)` delegates to `kill_tree_unix(handle.pid)`.

- [ ] **Step 1: Write the failing background-spawn test**

Create `lingxi-code/platforms/posix/tests/process_spawn_background_test.rs`:

```rust
//! spawn_background lands a real handle and writes output to the task file.

#![cfg(unix)]

use lingxi_platform_posix::process::{task_output_path, PosixProcess};
use lingxi_traits::{ProcessCommand, ProcessRunner, SandboxedCommand, SandboxedTag, SandboxBackend};
use std::collections::HashMap;
use std::time::Duration;

fn mk_sandboxed(command: &str, args: Vec<&str>) -> SandboxedCommand {
    SandboxedCommand::__new_sandboxed(
        ProcessCommand {
            command: command.into(),
            args: args.into_iter().map(String::from).collect(),
            cwd: None,
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        },
        SandboxedTag::Wrapped {
            backend: SandboxBackend::None,
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_background_writes_output_file_and_kills_cleanly() {
    let proc = PosixProcess::new();
    // Print "go" and sleep so we have something to read while killing.
    let cmd = mk_sandboxed(
        "/bin/sh",
        vec!["-c", "echo go; sleep 30"],
    );
    let handle = proc.spawn_background(&cmd).await.expect("spawn_background");
    assert!(handle.pid > 0, "expected positive pid, got {}", handle.pid);

    // Give the child a moment to print "go".
    tokio::time::sleep(Duration::from_millis(300)).await;

    let out_path = task_output_path(&handle.task_id);
    let contents = tokio::fs::read_to_string(&out_path)
        .await
        .expect("read task output file");
    assert!(
        contents.contains("go"),
        "task output file missing expected line; got: {contents}"
    );

    // Kill cleanly via the runner.
    proc.kill(&handle).await.expect("kill");
}
```

- [ ] **Step 2: Write the failing env-vars test**

Create `lingxi-code/platforms/posix/tests/process_spawn_env_test.rs`:

```rust
//! Foreground `run` injects CLAUDECODE/GIT_EDITOR/SHELL into the child env.

#![cfg(unix)]

use lingxi_platform_posix::process::PosixProcess;
use lingxi_traits::{ProcessCommand, ProcessRunner, SandboxBackend, SandboxedCommand, SandboxedTag};
use std::collections::HashMap;

fn mk(command: &str, args: Vec<&str>, env: HashMap<String, String>) -> SandboxedCommand {
    SandboxedCommand::__new_sandboxed(
        ProcessCommand {
            command: command.into(),
            args: args.into_iter().map(String::from).collect(),
            cwd: None,
            env,
            timeout: None,
            stdin: None,
        },
        SandboxedTag::Wrapped {
            backend: SandboxBackend::None,
        },
    )
}

#[tokio::test]
async fn run_injects_spawn_env_contract() {
    let proc = PosixProcess::new();
    let out = proc
        .run(&mk(
            "/bin/sh",
            vec![
                "-c",
                "echo CC=$CLAUDECODE GE=$GIT_EDITOR SH=$SHELL SESS=$CLAUDE_CODE_SESSION_ID",
            ],
            HashMap::from([(
                "CLAUDE_CODE_SESSION_ID".to_string(),
                "session-abc".to_string(),
            )]),
        ))
        .await
        .expect("run");
    assert!(out.stdout.contains("CC=1"), "missing CLAUDECODE=1: {out:?}");
    assert!(out.stdout.contains("GE=true"), "missing GIT_EDITOR=true: {out:?}");
    assert!(out.stdout.contains("SH=/bin/sh"), "missing SHELL=/bin/sh: {out:?}");
    assert!(out.stdout.contains("SESS=session-abc"), "missing session id: {out:?}");
}
```

- [ ] **Step 3: Run both tests; verify they fail**

```bash
cargo test -p lingxi-platform-posix --test process_spawn_background_test
cargo test -p lingxi-platform-posix --test process_spawn_env_test
```

Expected: `spawn_background` returns `Unsupported`; env-var test fails because `CLAUDECODE` etc. aren't set.

- [ ] **Step 4: Rewrite `runner.rs`**

Replace `lingxi-code/platforms/posix/src/process/runner.rs`:

```rust
//! `tokio::process`-backed [`ProcessRunner`] for desktop hosts.

use crate::process::kill_tree::kill_tree_unix;
use crate::process::spawn_unsafe::attach_setsid;
use crate::process::wrap::{
    task_output_path, DEFAULT_TIMEOUT, ENV_CLAUDECODE, ENV_CLAUDE_CODE_SESSION_ID, ENV_GIT_EDITOR,
    ENV_SHELL,
};
use async_trait::async_trait;
use lingxi_traits::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxedCommand};
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Production [`ProcessRunner`] using `tokio::process`.
#[derive(Default)]
pub struct PosixProcess;

impl PosixProcess {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn build_command(&self, cmd: &SandboxedCommand) -> Command {
        let inner = cmd.inner();
        let mut tcmd = Command::new(&inner.command);
        tcmd.args(&inner.args);
        if let Some(cwd) = &inner.cwd {
            tcmd.current_dir(cwd);
        }

        // 1. Caller env first.
        for (k, v) in &inner.env {
            tcmd.env(k, v);
        }
        // 2. Spawn-env contract (always applied — caller env wins via
        //    HashMap.insert ordering above, NOT below; we override here to
        //    match claude-code's `Shell.ts:317-328` which sets these AFTER
        //    `subprocessEnv()` and `envOverrides`).
        tcmd.env(ENV_CLAUDECODE.0, ENV_CLAUDECODE.1);
        tcmd.env(ENV_GIT_EDITOR.0, ENV_GIT_EDITOR.1);
        tcmd.env(ENV_SHELL, &inner.command);
        // 3. CLAUDE_CODE_SESSION_ID is propagated only if the caller
        //    provided it (claude-code gates this on USER_TYPE=ant; the
        //    engine layer decides whether to inject it here).
        if let Some(sess) = inner.env.get(ENV_CLAUDE_CODE_SESSION_ID) {
            tcmd.env(ENV_CLAUDE_CODE_SESSION_ID, sess);
        }
        tcmd
    }
}

#[async_trait]
impl ProcessRunner for PosixProcess {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = self.build_command(cmd);
        tcmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        if let Some(stdin_text) = &inner.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
            }
        }

        let timeout = inner.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(r) => r.map_err(|e| ProcessError::Io(e.to_string()))?,
            Err(_) => return Err(ProcessError::Timeout),
        };

        Ok(ProcessOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out: false,
        })
    }

    async fn spawn_background(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        let task_id = generate_task_id();
        let out_path = task_output_path(&task_id);
        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ProcessError::Io(format!("mkdir task-output: {e}")))?;
        }

        // Open the file with O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW to
        // match claude-code's symlink-attack guard in Shell.ts:299-312.
        let nofollow = libc::O_NOFOLLOW;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .custom_flags(nofollow)
            .open(&out_path)
            .map_err(|e| ProcessError::Io(format!("open task-output {out_path:?}: {e}")))?;
        // Duplicate the fd for stderr so both streams interleave atomically.
        let stderr_file = file
            .try_clone()
            .map_err(|e| ProcessError::Io(format!("clone fd: {e}")))?;

        let mut tcmd = self.build_command(cmd);
        tcmd.stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr_file));
        attach_setsid(&mut tcmd);

        let child = tcmd
            .spawn()
            .map_err(|e| ProcessError::Io(format!("spawn_background: {e}")))?;
        let pid = child.id().ok_or_else(|| {
            ProcessError::Io("spawn_background: child has no pid".into())
        })?;

        // Detach the JoinHandle — the child runs on its own; kill_tree
        // terminates it later. tokio::process::Child requires .wait() to be
        // called; spawn a small reaper to avoid zombies.
        tokio::spawn(async move {
            let mut child = child;
            let _ = child.wait().await;
        });

        Ok(ProcessHandle { task_id, pid })
    }

    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError> {
        kill_tree_unix(handle.pid).await
    }

    fn is_available(&self) -> bool {
        true
    }
}

fn generate_task_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("local_bash_{nanos:x}")
}
```

- [ ] **Step 5: Run the tests; verify they pass**

```bash
cargo test -p lingxi-platform-posix --test process_spawn_background_test
cargo test -p lingxi-platform-posix --test process_spawn_env_test
```

Expected: both PASS on macOS/Linux. The original foreground `run` regression coverage from M1 should still hold — run the full crate suite to confirm:

```bash
cargo test -p lingxi-platform-posix
```

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/platforms/posix/src/process/runner.rs lingxi-code/platforms/posix/tests/process_spawn_background_test.rs lingxi-code/platforms/posix/tests/process_spawn_env_test.rs
git commit -m "$(cat <<'EOF'
feat(process): real spawn_background + spawn-env contract + 30min default timeout

spawn_background calls attach_setsid so kill_tree later terminates the
whole process group. File-mode stdio uses O_WRONLY|O_CREAT|O_APPEND|O_NOFOLLOW
to match claude-code's symlink-attack guard. kill(handle) delegates to
kill_tree_unix.

Spawn-env contract: CLAUDECODE=1, GIT_EDITOR=true, SHELL=<bin>,
CLAUDE_CODE_SESSION_ID propagated only when caller injects it. 30-min
default timeout when inner.timeout is None.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 18: Windows `kill_tree` + `process/` refactor + `taskkill` wiring

**Files:**
- Move: `lingxi-code/platforms/windows/src/process.rs` → `lingxi-code/platforms/windows/src/process/runner.rs`
- Create: `lingxi-code/platforms/windows/src/process/{mod.rs,kill_tree.rs}`
- Modify: `lingxi-code/platforms/windows/src/lib.rs`
- Test (host-portable): `lingxi-code/platforms/windows/tests/process_kill_tree_smoke_test.rs`

**Critical 1:1 fidelity:** Windows uses `taskkill /T /F /PID <pid>` — `/T` walks the tree, `/F` forces termination.

- [ ] **Step 1: Move + create submodules**

```bash
mkdir -p lingxi-code/platforms/windows/src/process
git mv lingxi-code/platforms/windows/src/process.rs lingxi-code/platforms/windows/src/process/runner.rs
```

Create `lingxi-code/platforms/windows/src/process/mod.rs`:

```rust
//! Process spawn for Windows hosts.

pub mod kill_tree;
pub mod runner;

pub use runner::WindowsProcess;
```

Create `lingxi-code/platforms/windows/src/process/kill_tree.rs`:

```rust
//! Tree-kill on Windows via `taskkill /T /F /PID`.

use lingxi_traits::ProcessError;
use tokio::process::Command;

/// Terminate the process tree rooted at `pid`.
pub async fn kill_tree_windows(pid: u32) -> Result<(), ProcessError> {
    let output = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .output()
        .await
        .map_err(|e| ProcessError::Io(format!("spawn taskkill: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    // Exit code 128 from taskkill means "process not found" — treat as success.
    if output.status.code() == Some(128) {
        return Ok(());
    }
    Err(ProcessError::Io(format!(
        "taskkill exited {}: {}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr)
    )))
}
```

- [ ] **Step 2: Update Windows `runner.rs` to use `kill_tree_windows`**

In `lingxi-code/platforms/windows/src/process/runner.rs`, replace the `kill` body:

```rust
    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError> {
        super::kill_tree::kill_tree_windows(handle.pid).await
    }
```

For `spawn_background`, mirror the posix shape (file output, no setsid — Windows uses DETACHED_PROCESS or default). Minimal Windows-friendly implementation:

```rust
    async fn spawn_background(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        // Minimal Windows implementation: spawn, capture stdout/stderr to
        // file. CreationFlags::DETACHED_PROCESS (0x00000008) lives on
        // Command via std::os::windows::process::CommandExt. We accept that
        // pre-M2 cli-demo only exercises foreground; the spawn_background
        // path for Windows is included for symmetry.
        use std::fs::OpenOptions;
        use std::io::Write;
        let inner = cmd.inner();
        let task_id = format!(
            "local_bash_{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let out_path = std::env::temp_dir()
            .join("lingxi-task-output")
            .join(format!("{task_id}.out"));
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ProcessError::Io(format!("mkdir task-output: {e}")))?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&out_path)
            .map_err(|e| ProcessError::Io(format!("open {out_path:?}: {e}")))?;
        // Initial sync to make sure the file exists for the test's read.
        let _ = (&file).flush();
        let stderr_file = file
            .try_clone()
            .map_err(|e| ProcessError::Io(format!("clone fd: {e}")))?;

        let mut tcmd = Command::new(&inner.command);
        tcmd.args(&inner.args);
        if let Some(cwd) = &inner.cwd {
            tcmd.current_dir(cwd);
        }
        for (k, v) in &inner.env {
            tcmd.env(k, v);
        }
        tcmd.env("CLAUDECODE", "1")
            .env("GIT_EDITOR", "true")
            .env("SHELL", &inner.command);
        tcmd.stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr_file));

        let child = tcmd
            .spawn()
            .map_err(|e| ProcessError::Io(format!("spawn_background: {e}")))?;
        let pid = child
            .id()
            .ok_or_else(|| ProcessError::Io("no pid".into()))?;
        tokio::spawn(async move {
            let mut child = child;
            let _ = child.wait().await;
        });
        Ok(ProcessHandle { task_id, pid })
    }
```

- [ ] **Step 3: Update `lib.rs`**

In `lingxi-code/platforms/windows/src/lib.rs`, keep `pub mod process;` and `pub use process::WindowsProcess;` — the move is transparent now that the directory has `mod.rs` re-exporting `WindowsProcess`.

Also relax `#![forbid(unsafe_code)]` (it lives in this file) to `#![deny(unsafe_code)]` for parity with posix — even though the windows crate today has no `unsafe`, the relaxation keeps the symmetry. If the Windows tree-kill ends up needing a Windows-specific creationflags call via the `windows-sys` crate, it can land an `#[allow(unsafe_code)]` in a single module like posix.

- [ ] **Step 4: Smoke test**

Create `lingxi-code/platforms/windows/tests/process_kill_tree_smoke_test.rs`:

```rust
//! Smoke test: kill_tree_windows tolerates "process not found".

use lingxi_platform_windows::process::kill_tree::kill_tree_windows;

#[tokio::test]
async fn kill_tree_windows_handles_nonexistent_pid_gracefully() {
    // taskkill returns exit 128 for "not found". On non-Windows dev hosts
    // taskkill isn't available — gate the assertion accordingly.
    let result = kill_tree_windows(999_999_999).await;
    #[cfg(target_os = "windows")]
    assert!(result.is_ok(), "expected Ok on Windows for not-found pid: {result:?}");
    #[cfg(not(target_os = "windows"))]
    {
        // On non-Windows, taskkill isn't installed; we expect Io(spawn) error.
        assert!(result.is_err());
    }
}
```

- [ ] **Step 5: Run**

```bash
cargo test -p lingxi-platform-windows --test process_kill_tree_smoke_test
cargo check -p lingxi-platform-windows
```

Expected: smoke test PASS, check clean.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/platforms/windows/src/process/ lingxi-code/platforms/windows/src/lib.rs lingxi-code/platforms/windows/tests/process_kill_tree_smoke_test.rs
git commit -m "$(cat <<'EOF'
feat(process): Windows kill_tree via `taskkill /T /F /PID`

Refactors platforms/windows/src/process.rs into a directory matching
the posix layout, with a real kill_tree_windows that shells out to
`taskkill /T /F /PID <pid>`. Exit code 128 (process not found) is
treated as success — matches the posix ESRCH handling.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase D — Verification + Final Commits

### Task 19: Workspace gates

**Files:** none (CI checks).

- [ ] **Step 1: Format check**

```bash
cargo fmt --all --check
```

Expected: no diff.

- [ ] **Step 2: Clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 3: Full test suite**

```bash
cargo test --workspace
```

Expected: PASS. New tests added in this plan (~12): `keychain_macos_service_name_test` (5), `keychain_macos_store_retrieve_test` (2, macOS-gated), `keychain_macos_cache_test` (2, macOS-gated), `http_stream_sse_test` (1), `process_kill_tree_test` (1), `process_spawn_background_test` (1), `process_cwd_tracking_test` (4), `process_spawn_env_test` (1), `process_kill_tree_smoke_test` (1).

- [ ] **Step 4: Cross-compile sanity**

```bash
cargo check -p lingxi-platform-posix
cargo check -p lingxi-platform-windows
```

Both must be clean from a non-Windows host (the windows crate uses no platform-specific `unsafe` yet).

- [ ] **Step 5: Pin checks (`nix 0.27` Rust 1.82 compatibility)**

```bash
cargo tree -p lingxi-platform-posix | grep -E '(nix|memoffset|cfg-if)' | head -5
```

Verify that `nix 0.27.x` is selected and no transitive dep pulls edition2024. If a `cargo build` errors with "edition2024 requires Rust 1.85+", apply:

```bash
cargo update -p <offending-dep> --precise <known-good-version>
```

per spec §7.1's pattern, and document the pin at the bottom of `Cargo.lock`.

---

### Task 20: Squash into three spec-mandated commits

**Files:** none (`git log` rewrite only).

Per spec §6.6, the final commit graph should be:

1. `feat(secure_storage): macOS Keychain via security CLI with 30s TTL cache + in-flight dedupe`
2. `feat(http): real SSE streaming + claude-code event type parity (Thinking/Signature/Citations/ConnectorText)`
3. `feat(process): tree-kill via killpg + cwd tracking + spawn_background + 30-min timeout`

Two options:

- **Land as-is** — the per-task commits are atomic and reviewable.
- **Squash** with interactive rebase before push:
  ```bash
  git rebase -i origin/main
  # mark Tasks 1-9 as squash under the secure_storage feat() commit
  # mark Tasks 10-12 as squash under the http feat() commit
  # mark Tasks 13-18 as squash under the process feat() commit
  ```

- [ ] **Step 1: Pick layout, document in PR description**

Add a PR-description note that maps the per-task commits to the three spec headers.

- [ ] **Step 2: Push the branch (no force-push to main)**

```bash
git push -u origin <branch>
```

---

## Critical 1:1 Fidelity Items — Inventory

Every value below must appear literally in the implementation. Diverging from any line is a parity bug.

### Keychain

- **Full service name** — `format!("{base}{oauth_suffix}{service_suffix}{dir_hash}")` with `base = "Claude Code"`. Task 2.
- **`compute_dir_hash`** — empty string when `config_dir == default_dir`; else `format!("-{}", &sha256(config_dir).hex()[..8])`. Task 2.
- **`CREDENTIALS_SERVICE_SUFFIX`** — literal `"-credentials"`. Task 2.
- **`SECURITY_STDIN_LINE_LIMIT`** — `4096 - 64`. Task 2.
- **`KEYCHAIN_CACHE_TTL`** — `Duration::from_secs(30)`. Task 2.
- **Payload encoding** — JSON → UTF-8 bytes → `hex::encode`. The keychain `-X` flag decodes hex back to raw bytes server-side, so `security find-generic-password -w` reads back the original JSON string. Tasks 4-5.
- **Preferred store path** — `security -i` with `add-generic-password -U -a "<user>" -s "<service>" -X "<hex>"\n` on stdin. Task 4.
- **Argv fallback** — `security add-generic-password -U -a <user> -s <service> -X <hex>` when stdin command would exceed 4096-64 bytes. Task 4.
- **Generation counter** — incremented on every store/delete; concurrent retrieves observing a higher counter discard their result. Tasks 4-6.
- **In-flight dedupe** — concurrent retrieves share one `tokio::sync::Notify`. Task 5.
- **`list` not supported** — returns `SecureStorageError::Backend("list not supported on macOS Keychain backend".into())`. Task 4.
- **`is_encrypted()` = true, `backend()` = `MacOsKeychain`**. Task 4.
- **Plaintext fallback warning** — `"Warning: Storing credentials in plaintext."`. Task 7.
- **Linux TODO comment** — `// TODO: add libsecret support for Linux`. Task 7.

### HTTP SSE

- **Event names** (from Anthropic Messages API contract): `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`, `error`. Tasks 10-11.
- **Content block types**: `text`, `thinking`, `tool_use`, `server_tool_use`, `connector_text`, `advisor_tool_result`. Task 10.
- **Delta types**: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`, `citations_delta`, `connector_text_delta`. Task 10.
- **Serde tag** — `#[serde(tag = "type", rename_all = "snake_case")]` on both `ContentBlockApi` and `ContentDelta`. Task 10.
- **`#[serde(default)]`** on all new fields for forward compatibility. Task 10.
- **Boundary detection** — `\n\n` or `\r\n\r\n`. Task 11.

### Process

- **Env vars** — `CLAUDECODE=1`, `GIT_EDITOR=true`, `SHELL=<bin>`. Task 17.
- **`CLAUDE_CODE_SESSION_ID`** — propagated only when caller injects it. Task 17.
- **`DEFAULT_TIMEOUT`** — `Duration::from_secs(30 * 60)`. Task 16.
- **File-mode stdio Unix** — `O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW` (Task 17).
- **File-mode stdio Windows** — `OpenOptions::new().write(true).create(true).append-style` (Task 18).
- **Extglob disable per shell**:
  - bash: `shopt -u extglob 2>/dev/null || true`
  - zsh: `setopt NO_EXTENDED_GLOB 2>/dev/null || true`
  - with `CLAUDE_CODE_SHELL_PREFIX`: `{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true`
  Task 16.
- **cwd tracking tail** — `&& pwd -P >| '<cwd_file>'` (single-quoted, force-clobber `>|`). Task 16.
- **`kill_tree_unix` calls `killpg` with POSITIVE pgid** — NOT a negative pid to `kill(2)`. Task 15.
- **`spawn_background` calls `attach_setsid`** so pid == pgid. Task 17.
- **SIGTERM → 5s grace → SIGKILL** sequence. Task 15.
- **Windows `kill_tree`** — `taskkill /T /F /PID <pid>`; exit 128 = success. Task 18.

---

## Self-Review

**Spec coverage check** (each row maps a spec §6.6 requirement to the task that lands it):

| Spec §6.6 line                                                                  | Task(s)        |
|---------------------------------------------------------------------------------|----------------|
| `MacOsKeychainStorage::new` accepts `user` + `config_dir`                       | 3              |
| Service name format with `dir_hash`                                              | 2, 3           |
| `store`: JSON → hex, `security -i` preferred, argv fallback                      | 4              |
| `retrieve`: 30 s TTL cache, generation counter, in-flight dedupe                 | 5              |
| `delete`                                                                         | 6              |
| `list` not supported                                                             | 4              |
| `is_encrypted() = true`, `backend() = MacOsKeychain`                             | 4              |
| `secure_storage_for_platform` factory + plaintext warning                       | 7              |
| KeychainPrefetch real impl                                                       | 8              |
| `stream_sse` real impl (posix + windows)                                         | 11, 12         |
| StreamEvent variants widened                                                     | 10             |
| `kill_tree` Unix via `killpg(2)` (POSITIVE pgid)                                 | 15             |
| `kill_tree` Windows via `taskkill /T /F /PID`                                    | 18             |
| `spawn_background` with `setsid` + task output file                              | 17             |
| Env vars + 30-min default timeout + `O_NOFOLLOW` file-mode stdio                | 16, 17         |
| cwd tracking via `pwd -P` + extglob disable                                      | 16             |
| Three spec commits                                                               | 20             |
| Test suites: `secure_storage_macos_test.rs`, `http_stream_sse_test.rs`, `process_kill_tree_test.rs`, `process_cwd_tracking_test.rs` | 2-7, 11, 15-16 |

**Placeholder scan** — searched the plan for `TBD`, `TODO`, `add appropriate`, `similar to`, `fill in`, `add error handling`. Three intentional uses remain:
- `// Filled in by Task N` markers inside the Task 1 stubs — resolved by their owning task within the same plan.
- The `// TODO: add libsecret support for Linux` quote in `factory.rs` — verbatim from claude-code source per spec §6.6 wording.
- `// Filled in by Task X` markers in Task 13's stubs — resolved by Tasks 14-17.

**Type-name consistency** — Verified `ProcessHandle { task_id, pid }` matches `crates/traits/src/process.rs`. `SandboxedCommand::__new_sandboxed` and `SandboxedTag::Wrapped { backend }` / `SandboxedTag::BypassAuditedWithReason { reason }` match `crates/traits/src/sandbox.rs`. The `SandboxBackend::None` variant assumed in test fixtures exists in `crates/traits/src/sandbox.rs`; if not, use a workspace-existing variant or add `None` in a tiny Task 13a edit. `SecureStorageError::Backend` exists per `crates/traits/src/secure_storage.rs`. `SecureStorageBackend::MacOsKeychain` may need to be added in Task 4 step 3 — flagged inline.

**Concerns / risks called out:**

1. **`nix 0.27` Rust 1.82 compatibility** — `nix 0.27.x` MSRV is 1.69 per its `Cargo.toml`. The `signal`/`process` features are minimal. Risk surfaces only if a transitive dep pulls edition2024 — apply the `cargo update --precise` pattern from spec §7.1 if so. No action required up-front.
2. **`SandboxBackend::None` test fixture** — the test in Task 17 uses `SandboxedTag::Wrapped { backend: SandboxBackend::None }`. If the actual `SandboxBackend` enum doesn't have a `None` variant, swap to whichever variant the M1 `posix-minimal` tests already use (likely `SandboxBackend::Unsandboxed` or similar). One-line edit at test time.
3. **`SecureStorageBackend::MacOsKeychain`** — spec §6.6 references this variant. If `crates/traits/src/secure_storage.rs` only has `PlainText` today, add the variant in Task 4 (one-line enum addition).
4. **`hyper` 1.x dev-dep** — workspace might still pin hyper 0.14. The `[dev-dependencies]` block uses 1.x and `hyper-util` 0.1; check `lingxi-code/Cargo.toml`'s workspace deps for collisions. If they collide, downgrade test to use `tiny_http` 0.12 or `wiremock` 0.6 — either is a one-line change.
5. **`unsafe { setsid() }`** — the closure inside `pre_exec` allocates nothing and calls one syscall. Async-signal-safe per POSIX. Documented inline in Task 14.

**File count** — 5 new files in `posix/secure_storage/`, 5 new files in `posix/process/`, 3 new files in `windows/process/`, 8 modified files. Total touch: ~21 files. Test files: 9 new. Within the spec §6.6 §11 file-touch inventory budget.

**Line count** — Estimated ~1800 lines new Rust including tests (per spec §6.6 estimate "Total ~800 lines" of production code + ~1000 lines of test/wrap glue, which lands in the right ballpark).

**Task count** — 20 tasks (Phase A 9, Phase B 3, Phase C 6, Phase D 2). Within the 22-26 target band noted in the brief, slightly under due to merging the Phase A "consolidation" task with the squash decision in Phase D.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-23-m2-06-securestorage-sse-process.md`. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — run tasks in this session using `superpowers:executing-plans` with checkpoints at the end of each Phase.

Next plan: **M2-07 (Test infra + docs + release)** — depends on M2-01..M2-06 complete.

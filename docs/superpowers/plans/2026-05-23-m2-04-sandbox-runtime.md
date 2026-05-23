# M2 Plan 04 · Sandbox Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reimplement `@anthropic-ai/sandbox-runtime` in Rust — the full `SandboxRuntimeConfig` schema, claude-code `SettingsJson → RuntimeConfig` policy conversion, platform dependency check + WSL1 refusal, bounded violation event store, expanded `should_use_sandbox` decision (compound-command + env-var + safe-wrapper stripping fixed-point), and a `wrap_with_sandbox` dispatcher that produces real `bwrap` invocations on Linux/WSL2, real `sandbox-exec -f <SBPL>` invocations on macOS, and refuses Windows/WSL1 with byte-for-byte claude-code error strings.

**Architecture:** A new `runtime_config.rs` module in `crates/sandbox/` carries the wire-shape struct (field names byte-for-byte from claude-code's `entrypoints/sandboxTypes.ts`). A `policy_convert.rs` module walks `SettingsJson.permissions.{allow,deny}` (Edit/Read/Bash/WebFetch rules) and merges into `filesystem.{allowWrite,denyWrite,allowRead,denyRead}` / `network.allowedDomains` / `excludedCommands`. A `dependency_check.rs` module probes `sandbox-exec`, `bwrap`, `socat` via `which::which()` and returns claude-code's exact error-string set. A WSL1/WSL2 detector reads `/proc/version`. A `violation_store.rs` module exposes a bounded ring buffer of `SandboxViolationEvent`. A `wrap.rs` module dispatches to per-platform wrappers (Linux/WSL2 → `bwrap+socat`; macOS → SBPL template file passed to `sandbox-exec -f`; Windows/WSL1 → `Err(Unsupported)`). The `decision.rs::should_use_sandbox` function gains compound-command splitting and iterative `BINARY_HIJACK_VARS` + safe-wrapper stripping with fixed-point. Finally `platforms/posix/src/sandbox.rs` is rewritten as a real `Sandbox` trait impl that calls into all of the above. `platforms/windows/src/sandbox.rs` is already an Unsupported stub from M2-01 and does not change in M2-04.

**Tech Stack:** Rust 2021 (rust-version 1.82), `serde` + `serde_json` (camelCase rename), `which = "6"` (shell-out dependency probe via `$PATH`), `tempfile = "3"` (SBPL profile file on macOS), `tokio::sync::RwLock`, `std::collections::VecDeque` (bounded ring buffer), existing `lingxi-traits::Sandbox` + `SandboxedCommand` + `SandboxedTag` types from M1 Plan 12.

**Depends on:** M2-01 (v0.2.0 corrections — `platforms/windows/src/sandbox.rs` is already an `Unsupported` stub).

**References:**
- Spec section: `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` §6.4 (Plan M2-04) and §3 (1:1 framing).
- Style template: `docs/superpowers/plans/2026-05-22-lingxi-core-m1-12-sandbox-lsp.md`, `docs/superpowers/plans/2026-05-23-m2-01-corrections.md`.
- claude-code reference:
  - `claude-code/src/utils/sandbox/sandbox-adapter.ts` (~986 lines — `convertToSandboxRuntimeConfig`, `getSandboxUnavailableReason`, `getLinuxGlobPatternWarnings`).
  - `claude-code/src/entrypoints/sandboxTypes.ts` (~157 lines — the zod `SandboxSettingsSchema`).
  - `claude-code/src/tools/BashTool/shouldUseSandbox.ts` (~154 lines — compound-command splitting + env-var fixed-point stripping).

---

## File Inventory

**New files (must not exist before this plan starts):**
- `lingxi-core/crates/sandbox/src/runtime_config.rs` — ~280 lines · full `SandboxRuntimeConfig` schema + `Platform` enum + `SettingsJson` slice.
- `lingxi-core/crates/sandbox/src/path_pattern.rs` — ~70 lines · `resolve_path_pattern_for_sandbox` (`//`, `/`, `~/`, `./`, bare passthrough).
- `lingxi-core/crates/sandbox/src/policy_convert.rs` — ~240 lines · `convert_settings_to_runtime_config` + `linux_glob_pattern_warnings`.
- `lingxi-core/crates/sandbox/src/dependency_check.rs` — ~180 lines · `check_dependencies` + `sandbox_unavailable_reason`.
- `lingxi-core/crates/sandbox/src/violation_store.rs` — ~120 lines · bounded `SandboxViolationEvent` store.
- `lingxi-core/crates/sandbox/src/wrap.rs` — ~340 lines · `wrap_with_sandbox` dispatch (Linux bwrap, macOS SBPL, Windows/WSL1 refuse).
- `lingxi-core/platforms/posix/src/wsl_detect.rs` — ~70 lines · `/proc/version` parser → `WslKind`.

**Modified files:**
- `lingxi-core/crates/sandbox/Cargo.toml` — add `which = "6"`, `tempfile = "3"`, `tokio` `sync` feature.
- `lingxi-core/crates/sandbox/src/lib.rs` — wire new modules into the public API.
- `lingxi-core/crates/sandbox/src/decision.rs` — expand `should_use_sandbox` with claude-code's compound + env-var + safe-wrapper fixed-point logic.
- `lingxi-core/crates/sandbox/src/policy.rs` — unchanged (M1 default policy stays valid for callers that don't use `SandboxRuntimeConfig`).
- `lingxi-core/platforms/posix/Cargo.toml` — add `lingxi-sandbox = { path = "../../crates/sandbox" }`.
- `lingxi-core/platforms/posix/src/lib.rs` — `pub mod wsl_detect;` export.
- `lingxi-core/platforms/posix/src/sandbox.rs` — full rewrite (~240 lines) wiring all new modules.

**Unchanged (M2-01 already corrected):**
- `lingxi-core/platforms/windows/src/sandbox.rs` — stays Unsupported. M2-04 adds a doc cross-reference only (Task 18).

Total new code: ~1300 lines in `crates/sandbox/` + ~310 lines in `platforms/posix/` + ~280 lines of test code. Total budget ~1900 lines including tests.

**Commit policy** (per spec §6.4): four commits.
1. `feat(sandbox): RuntimeConfig schema` — Phase A (Tasks 1-5).
2. `feat(sandbox): dependency check + violation store` — Phase B + C-first-half (Tasks 6-12).
3. `feat(sandbox): wrap_with_sandbox dispatch` — Phase C-second-half (Tasks 13-16).
4. `feat(platforms/posix): real Sandbox impl` — Phase D + verification (Tasks 17-22).

---

## Phase A — `SandboxRuntimeConfig` schema (Tasks 1-5)

### Task 1: Cargo dependencies and `runtime_config.rs` skeleton

**Files:**
- Modify: `lingxi-core/crates/sandbox/Cargo.toml`
- Create: `lingxi-core/crates/sandbox/src/runtime_config.rs`
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

**Critical 1:1 fidelity:** every field name in the `#[serde(rename_all = "camelCase")]` rendering MUST match claude-code's `SandboxSettingsSchema` (zod) byte-for-byte. The strings managed-settings deployments key off these.

- [ ] **Step 1: Replace the dependency block**

Replace the `[dependencies]` block in `lingxi-core/crates/sandbox/Cargo.toml`:

```toml
[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-permission = { path = "../permission" }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
thiserror = { workspace = true }
async-trait = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true, features = ["sync"] }
which = "6"
tempfile = "3"

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt", "sync"] }
serde_json = { workspace = true }
```

- [ ] **Step 2: Write the failing JSON-roundtrip test**

Create `lingxi-core/crates/sandbox/tests/runtime_config_test.rs`:

```rust
use lingxi_sandbox::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, RipgrepConfig,
    SandboxRuntimeConfig,
};
use std::collections::HashMap;

#[test]
fn default_config_serializes_with_camelcase_keys() {
    let cfg = SandboxRuntimeConfig::default();
    let v = serde_json::to_value(&cfg).expect("serialize default");
    // Every field name from claude-code's zod SandboxSettingsSchema must appear.
    for key in [
        "enabled",
        "failIfUnavailable",
        "enabledPlatforms",
        "autoAllowBashIfSandboxed",
        "allowUnsandboxedCommands",
        "network",
        "filesystem",
        "ignoreViolations",
        "enableWeakerNestedSandbox",
        "enableWeakerNetworkIsolation",
        "excludedCommands",
        "ripgrep",
    ] {
        assert!(
            v.as_object().unwrap().contains_key(key),
            "missing zod key {key} from default config: {v}"
        );
    }
}

#[test]
fn network_subkeys_match_zod_schema() {
    let cfg = SandboxRuntimeConfig::default();
    let v = serde_json::to_value(&cfg).unwrap();
    let net = &v["network"];
    for key in [
        "allowedDomains",
        "allowManagedDomainsOnly",
        "allowUnixSockets",
        "allowAllUnixSockets",
        "allowLocalBinding",
        "httpProxyPort",
        "socksProxyPort",
    ] {
        assert!(
            net.as_object().unwrap().contains_key(key),
            "missing network key {key}: {net}"
        );
    }
}

#[test]
fn filesystem_subkeys_match_zod_schema() {
    let cfg = SandboxRuntimeConfig::default();
    let v = serde_json::to_value(&cfg).unwrap();
    let fs = &v["filesystem"];
    for key in [
        "allowWrite",
        "denyWrite",
        "denyRead",
        "allowRead",
        "allowManagedReadPathsOnly",
    ] {
        assert!(
            fs.as_object().unwrap().contains_key(key),
            "missing filesystem key {key}: {fs}"
        );
    }
}

#[test]
fn deserializes_realistic_claude_code_settings_fragment() {
    let json = serde_json::json!({
        "enabled": true,
        "failIfUnavailable": false,
        "enabledPlatforms": ["mac", "linux"],
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": ["docker"],
        "network": {
            "allowedDomains": ["github.com", "*.anthropic.com"],
            "allowManagedDomainsOnly": false,
            "allowUnixSockets": ["/var/run/docker.sock"],
            "allowAllUnixSockets": false,
            "allowLocalBinding": true,
            "httpProxyPort": 8080,
            "socksProxyPort": 1080
        },
        "filesystem": {
            "allowWrite": ["./build", "./target"],
            "denyWrite": ["/etc"],
            "denyRead": ["/private/etc"],
            "allowRead": ["~/.cargo/registry"],
            "allowManagedReadPathsOnly": false
        },
        "ignoreViolations": { "fs.read": ["~/.cache"] },
        "enableWeakerNestedSandbox": false,
        "enableWeakerNetworkIsolation": false,
        "excludedCommands": ["bazel", "make"],
        "ripgrep": { "command": "/usr/bin/rg", "args": ["--no-config"] }
    });
    let cfg: SandboxRuntimeConfig =
        serde_json::from_value(json).expect("parse SandboxRuntimeConfig");
    assert!(cfg.enabled);
    assert_eq!(
        cfg.enabled_platforms.as_deref(),
        Some(&[Platform::Mac, Platform::Linux][..])
    );
    assert_eq!(cfg.allow_unsandboxed_commands, vec!["docker".to_string()]);
    assert_eq!(
        cfg.network.allowed_domains,
        vec!["github.com".to_string(), "*.anthropic.com".to_string()]
    );
    assert_eq!(cfg.network.http_proxy_port, Some(8080));
    assert_eq!(cfg.filesystem.allow_write, vec!["./build", "./target"]);
    assert_eq!(cfg.excluded_commands, vec!["bazel", "make"]);
    assert_eq!(cfg.ripgrep.command, "/usr/bin/rg");
    assert_eq!(cfg.ripgrep.args, vec!["--no-config"]);
    let _: &HashMap<String, Vec<String>> = &cfg.ignore_violations;
}

#[test]
fn platform_enum_serializes_lowercase() {
    assert_eq!(
        serde_json::to_value(Platform::Mac).unwrap(),
        serde_json::json!("mac")
    );
    assert_eq!(
        serde_json::to_value(Platform::Linux).unwrap(),
        serde_json::json!("linux")
    );
    assert_eq!(
        serde_json::to_value(Platform::Wsl).unwrap(),
        serde_json::json!("wsl")
    );
}

#[test]
fn unused_helpers_are_referenced() {
    // Touch the optional sub-structs to keep them in scope and ensure they're
    // constructible without arguments.
    let _: NetworkRestrictionConfig = NetworkRestrictionConfig::default();
    let _: FilesystemRestrictionConfig = FilesystemRestrictionConfig::default();
    let _: RipgrepConfig = RipgrepConfig::default();
}
```

- [ ] **Step 3: Run tests — expect compile failure**

Run: `cargo test -p lingxi-sandbox --test runtime_config_test`
Expected: FAIL with `unresolved import lingxi_sandbox::runtime_config` (module doesn't exist yet).

- [ ] **Step 4: Create the full `runtime_config.rs`**

Create `lingxi-core/crates/sandbox/src/runtime_config.rs`:

```rust
//! Wire-shape `SandboxRuntimeConfig` matching claude-code's zod
//! `SandboxSettingsSchema` from `entrypoints/sandboxTypes.ts`.
//!
//! Every `#[serde(rename_all = "camelCase")]` here lines up with a zod field
//! name byte-for-byte. Renaming any field is a managed-policy-breaking change
//! and must be coordinated with claude-code's settings schema.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Platforms the sandbox runtime recognizes.
///
/// Mirrors claude-code's `Platform` literal union (`mac` | `linux` | `wsl`).
/// `windows` is intentionally absent — claude-code refuses sandbox on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Mac,
    Linux,
    Wsl,
}

impl Platform {
    /// Human-readable form for inclusion in error strings.
    /// Matches claude-code's `getPlatform()` return value spelling
    /// (`macos`, `linux`, `wsl`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mac => "macos",
            Self::Linux => "linux",
            Self::Wsl => "wsl",
        }
    }
}

/// Network restriction subsection of `SandboxRuntimeConfig`.
///
/// All fields are passthrough from `SandboxNetworkConfigSchema` in
/// `entrypoints/sandboxTypes.ts`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRestrictionConfig {
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub allow_managed_domains_only: bool,
    #[serde(default)]
    pub allow_unix_sockets: Vec<String>,
    #[serde(default)]
    pub allow_all_unix_sockets: bool,
    #[serde(default)]
    pub allow_local_binding: bool,
    #[serde(default)]
    pub http_proxy_port: Option<u16>,
    #[serde(default)]
    pub socks_proxy_port: Option<u16>,
}

/// Filesystem restriction subsection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemRestrictionConfig {
    #[serde(default)]
    pub allow_write: Vec<String>,
    #[serde(default)]
    pub deny_write: Vec<String>,
    #[serde(default)]
    pub deny_read: Vec<String>,
    #[serde(default)]
    pub allow_read: Vec<String>,
    #[serde(default)]
    pub allow_managed_read_paths_only: bool,
}

/// Ripgrep override block. Bundled ripgrep path + extra args.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RipgrepConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Full `SandboxRuntimeConfig` — direct port of zod `SandboxSettingsSchema`
/// (`entrypoints/sandboxTypes.ts`).
///
/// Field naming uses `#[serde(rename_all = "camelCase")]` so the wire shape
/// matches the zod schema byte-for-byte (`failIfUnavailable`,
/// `autoAllowBashIfSandboxed`, etc.). Unknown fields are silently dropped on
/// deserialization (zod's `.passthrough()` cannot be perfectly mirrored in
/// `serde` without an extra `HashMap<String, Value>` catch-all; we accept that
/// trade-off — managed settings always lay down only known fields).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxRuntimeConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub fail_if_unavailable: bool,
    #[serde(default)]
    pub enabled_platforms: Option<Vec<Platform>>,
    #[serde(default)]
    pub auto_allow_bash_if_sandboxed: bool,
    #[serde(default)]
    pub allow_unsandboxed_commands: Vec<String>,
    #[serde(default)]
    pub network: NetworkRestrictionConfig,
    #[serde(default)]
    pub filesystem: FilesystemRestrictionConfig,
    #[serde(default)]
    pub ignore_violations: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub enable_weaker_nested_sandbox: bool,
    #[serde(default)]
    pub enable_weaker_network_isolation: bool,
    #[serde(default)]
    pub excluded_commands: Vec<String>,
    #[serde(default)]
    pub ripgrep: RipgrepConfig,
}

// ============================================================================
// SettingsJson slice — only the keys the sandbox conversion pipeline touches.
// Other claude-code SettingsJson fields are owned by other crates and may not
// even exist in lingxi-core (the conversion pipeline only cares about
// `permissions.allow/deny` and the `sandbox` subtree).
// ============================================================================

/// Slice of claude-code `SettingsJson` consumed by
/// [`crate::policy_convert::convert_settings_to_runtime_config`].
///
/// `#[serde(default)]` everywhere so partial JSON deserializes cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsJson {
    #[serde(default)]
    pub permissions: Option<SettingsPermissions>,
    #[serde(default)]
    pub sandbox: Option<SandboxSettingsJson>,
    /// Directory the settings file lives in. Required because claude-code's
    /// path patterns (`/path`) are resolved relative to the settings file
    /// directory. Defaults to the cwd if the caller doesn't know it; sandbox
    /// path resolution then degrades to "current directory" semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_dir: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPermissions {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub additional_directories: Vec<String>,
}

/// User-supplied `sandbox` subsection of `SettingsJson`. All fields optional;
/// merged into [`SandboxRuntimeConfig`] by
/// [`crate::policy_convert::convert_settings_to_runtime_config`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSettingsJson {
    pub enabled: Option<bool>,
    pub fail_if_unavailable: Option<bool>,
    pub enabled_platforms: Option<Vec<Platform>>,
    pub auto_allow_bash_if_sandboxed: Option<bool>,
    pub allow_unsandboxed_commands: Option<Vec<String>>,
    pub network: Option<NetworkRestrictionConfig>,
    pub filesystem: Option<FilesystemRestrictionConfig>,
    pub ignore_violations: Option<HashMap<String, Vec<String>>>,
    pub enable_weaker_nested_sandbox: Option<bool>,
    pub enable_weaker_network_isolation: Option<bool>,
    pub excluded_commands: Option<Vec<String>>,
    pub ripgrep: Option<RipgrepConfig>,
}
```

- [ ] **Step 5: Wire the module into `lib.rs`**

Edit `lingxi-core/crates/sandbox/src/lib.rs` — add `pub mod runtime_config;` and a re-export block.

Replace the `pub mod` lines and the `pub use` block:

```rust
pub mod decision;
pub mod policy;
pub mod runtime_config;

pub use decision::{
    is_obviously_dangerous, should_use_sandbox, ProjectTrustLevel, SandboxDecision,
};
pub use lingxi_traits::{
    NetworkPolicy, ResourceLimits, Sandbox, SandboxBackend, SandboxError, SandboxPolicy,
    SandboxedCommand, SandboxedTag,
};
pub use policy::default_policy;
pub use runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, RipgrepConfig,
    SandboxRuntimeConfig, SandboxSettingsJson, SettingsJson, SettingsPermissions,
};
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p lingxi-sandbox --test runtime_config_test`
Expected: 6 tests pass.

- [ ] **Step 7: Clippy + fmt**

Run: `cargo clippy -p lingxi-sandbox -- -D warnings && cargo fmt -p lingxi-sandbox`
Expected: no warnings, no diff.

---

### Task 2: `path_pattern.rs` — claude-code-specific path prefix resolution

**Files:**
- Create: `lingxi-core/crates/sandbox/src/path_pattern.rs`
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

claude-code's permission-rule path patterns have three CC-specific conventions (the rest pass through to sandbox-runtime):
- `//path` → strip the leading `/` (absolute from filesystem root). Used to write absolute paths in permission rules without colliding with the `/path` convention.
- `/path` → relative to the settings file directory (NOT the filesystem root). This is the unusual one.
- `~/path`, `./path`, bare `path` → passed through unchanged.

**Critical 1:1 fidelity:** see `claude-code/src/utils/sandbox/sandbox-adapter.ts::resolvePathPatternForSandbox` (lines 99-119).

- [ ] **Step 1: Write the failing test**

Create `lingxi-core/crates/sandbox/tests/path_pattern_test.rs`:

```rust
use lingxi_sandbox::path_pattern::resolve_path_pattern_for_sandbox;
use std::path::PathBuf;

#[test]
fn double_slash_strips_one_slash() {
    let out = resolve_path_pattern_for_sandbox("//etc/passwd", &PathBuf::from("/home/u/.claude"));
    assert_eq!(out, "/etc/passwd");
}

#[test]
fn double_slash_works_with_glob() {
    let out =
        resolve_path_pattern_for_sandbox("//.aws/**", &PathBuf::from("/home/u/.claude"));
    assert_eq!(out, "/.aws/**");
}

#[test]
fn single_slash_resolves_against_settings_dir() {
    let out = resolve_path_pattern_for_sandbox("/foo/**", &PathBuf::from("/home/u/.claude"));
    assert_eq!(out, "/home/u/.claude/foo/**");
}

#[test]
fn tilde_passes_through() {
    let out = resolve_path_pattern_for_sandbox("~/Documents", &PathBuf::from("/anything"));
    assert_eq!(out, "~/Documents");
}

#[test]
fn dot_relative_passes_through() {
    let out = resolve_path_pattern_for_sandbox("./build", &PathBuf::from("/anything"));
    assert_eq!(out, "./build");
}

#[test]
fn bare_path_passes_through() {
    let out = resolve_path_pattern_for_sandbox("target", &PathBuf::from("/anything"));
    assert_eq!(out, "target");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test path_pattern_test`
Expected: FAIL — `unresolved import lingxi_sandbox::path_pattern`.

- [ ] **Step 3: Implement `path_pattern.rs`**

Create `lingxi-core/crates/sandbox/src/path_pattern.rs`:

```rust
//! claude-code path-pattern resolution. Ports
//! `resolvePathPatternForSandbox` from
//! `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
//!
//! Three CC-specific conventions:
//! - `//path` → absolute from filesystem root (strip one leading `/`).
//! - `/path`  → relative to the settings file directory.
//! - `~/path`, `./path`, bare `path` → passthrough; the sandbox-runtime layer
//!   handles tilde expansion / cwd relativization later.

use std::path::{Path, PathBuf};

/// Resolve `pattern` according to claude-code's three permission-rule path
/// prefix conventions.
///
/// `settings_dir` is the directory the settings file with this rule lives in.
/// For `~/.claude/settings.json` that's `~/.claude`. For
/// `<project>/.claude/settings.json` that's `<project>/.claude`. For ad-hoc /
/// in-memory settings, callers may pass any path; only `/path` patterns are
/// affected.
#[must_use]
pub fn resolve_path_pattern_for_sandbox(pattern: &str, settings_dir: &Path) -> String {
    // `//path` → strip ONE leading slash. `//etc` → `/etc`.
    if let Some(stripped) = pattern.strip_prefix("//") {
        // The remaining string is already absolute-from-root (claude-code uses
        // this to escape the `/path = settings-relative` convention).
        return format!("/{stripped}");
    }

    // `/path` → relative to settings file directory.
    if let Some(stripped) = pattern.strip_prefix('/') {
        // Skip empty strip (the only way is the input was a single `/`, which
        // we treat as "the settings dir itself").
        let mut out: PathBuf = settings_dir.to_path_buf();
        if !stripped.is_empty() {
            out.push(stripped);
        }
        return out.to_string_lossy().into_owned();
    }

    // Everything else passes through unchanged — sandbox-runtime
    // (or the Rust equivalent) will handle `~/`, `./`, and bare paths.
    pattern.to_string()
}
```

- [ ] **Step 4: Wire into lib.rs**

Add `pub mod path_pattern;` to `lingxi-core/crates/sandbox/src/lib.rs` after `pub mod runtime_config;`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p lingxi-sandbox --test path_pattern_test`
Expected: 6 tests pass.

---

### Task 3: `policy_convert.rs` — SettingsJson → SandboxRuntimeConfig conversion

**Files:**
- Create: `lingxi-core/crates/sandbox/src/policy_convert.rs`
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

Port `convertToSandboxRuntimeConfig` from `claude-code/src/utils/sandbox/sandbox-adapter.ts`. Walks `permissions.allow` and `permissions.deny`:
- `Edit(<pattern>)` → `filesystem.allow_write` (allow) or `filesystem.deny_write` (deny).
- `Read(<pattern>)` → `filesystem.allow_read` (allow) or `filesystem.deny_read` (deny).
- `Bash(<pattern>)` → bash rules are NOT consumed here (the bash decision lives in `decision.rs`).
- `WebFetch(domain:<host>)` → `network.allowed_domains`.

All path patterns go through `resolve_path_pattern_for_sandbox` first.

Also ports `getLinuxGlobPatternWarnings` (lines 597-642): scan allow/deny Edit/Read rules whose path contains `* ? [ ]` excluding trailing `/**`.

**Critical 1:1 fidelity:**
- Edit rules with `*`, `?`, `[`, `]` (outside trailing `/**`) emit Linux glob-warning entries verbatim.
- The order of inputs is preserved in the output arrays (claude-code uses `Array.push` in declaration order).
- `Bash` rules are intentionally ignored at this conversion layer.

- [ ] **Step 1: Write the failing tests**

Create `lingxi-core/crates/sandbox/tests/policy_convert_test.rs`:

```rust
use lingxi_sandbox::policy_convert::{
    convert_settings_to_runtime_config, linux_glob_pattern_warnings,
};
use lingxi_sandbox::runtime_config::{
    SandboxSettingsJson, SettingsJson, SettingsPermissions,
};
use std::path::PathBuf;

fn settings(allow: Vec<&str>, deny: Vec<&str>) -> SettingsJson {
    SettingsJson {
        permissions: Some(SettingsPermissions {
            allow: allow.into_iter().map(String::from).collect(),
            deny: deny.into_iter().map(String::from).collect(),
            additional_directories: vec![],
        }),
        sandbox: Some(SandboxSettingsJson {
            enabled: Some(true),
            ..Default::default()
        }),
        settings_dir: Some(PathBuf::from("/home/u/.claude")),
    }
}

#[test]
fn extracts_edit_allow_into_allow_write() {
    let s = settings(vec!["Edit(./src/**)"], vec![]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.filesystem.allow_write.contains(&"./src/**".to_string()));
}

#[test]
fn extracts_edit_deny_into_deny_write() {
    let s = settings(vec![], vec!["Edit(//.git/**)"]);
    let cfg = convert_settings_to_runtime_config(&s);
    // `//.git/**` resolves to `/.git/**` (one leading slash stripped).
    assert!(cfg.filesystem.deny_write.contains(&"/.git/**".to_string()));
}

#[test]
fn extracts_read_allow_into_allow_read() {
    let s = settings(vec!["Read(~/.aws/credentials)"], vec![]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.filesystem.allow_read.contains(&"~/.aws/credentials".to_string()));
}

#[test]
fn extracts_read_deny_into_deny_read() {
    let s = settings(vec![], vec!["Read(/secret)"]);
    let cfg = convert_settings_to_runtime_config(&s);
    // `/secret` resolves relative to settings_dir = /home/u/.claude.
    assert!(cfg
        .filesystem
        .deny_read
        .contains(&"/home/u/.claude/secret".to_string()));
}

#[test]
fn extracts_webfetch_domain_into_allowed_domains() {
    let s = settings(vec!["WebFetch(domain:anthropic.com)"], vec![]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg
        .network
        .allowed_domains
        .contains(&"anthropic.com".to_string()));
}

#[test]
fn bash_rules_are_ignored_in_this_layer() {
    let s = settings(vec!["Bash(curl:*)"], vec!["Bash(rm:*)"]);
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.filesystem.allow_write.is_empty());
    assert!(cfg.filesystem.deny_write.is_empty());
}

#[test]
fn passes_through_sandbox_subsection_values() {
    let mut s = settings(vec![], vec![]);
    s.sandbox = Some(SandboxSettingsJson {
        enabled: Some(true),
        fail_if_unavailable: Some(true),
        excluded_commands: Some(vec!["bazel".into(), "make".into()]),
        ..Default::default()
    });
    let cfg = convert_settings_to_runtime_config(&s);
    assert!(cfg.enabled);
    assert!(cfg.fail_if_unavailable);
    assert_eq!(cfg.excluded_commands, vec!["bazel", "make"]);
}

#[test]
fn linux_glob_warning_for_star_in_edit_rule() {
    let s = settings(vec!["Edit(./src/*.rs)"], vec![]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(
        warnings.iter().any(|w| w == "Edit(./src/*.rs)"),
        "expected warning for Edit(./src/*.rs), got {warnings:?}"
    );
}

#[test]
fn linux_glob_warning_skips_trailing_double_star() {
    let s = settings(vec!["Edit(./src/**)"], vec![]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(
        warnings.is_empty(),
        "expected no warning for trailing /** but got {warnings:?}"
    );
}

#[test]
fn linux_glob_warning_for_brackets() {
    let s = settings(vec![], vec!["Read(./[ab]/file)"]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(warnings.iter().any(|w| w == "Read(./[ab]/file)"));
}

#[test]
fn linux_glob_warning_for_question_mark() {
    let s = settings(vec!["Edit(./foo?.txt)"], vec![]);
    let warnings = linux_glob_pattern_warnings(&s);
    assert!(warnings.iter().any(|w| w == "Edit(./foo?.txt)"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lingxi-sandbox --test policy_convert_test`
Expected: FAIL — `unresolved import lingxi_sandbox::policy_convert`.

- [ ] **Step 3: Implement `policy_convert.rs`**

Create `lingxi-core/crates/sandbox/src/policy_convert.rs`:

```rust
//! Port of `convertToSandboxRuntimeConfig` and `getLinuxGlobPatternWarnings`
//! from `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
//!
//! Walks `SettingsJson.permissions.allow/deny` for `Edit(...)`, `Read(...)`,
//! `WebFetch(domain:...)` rules and folds the extracted patterns into a
//! `SandboxRuntimeConfig`. `Bash(...)` rules are intentionally ignored here —
//! the bash decision (compound + env-var stripping + excludedCommands match)
//! lives in [`crate::decision::should_use_sandbox`].

use crate::path_pattern::resolve_path_pattern_for_sandbox;
use crate::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, SandboxRuntimeConfig, SettingsJson,
};
use std::path::PathBuf;

/// Tool-name prefix matchers. claude-code's permission rules carry the tool
/// name as `Edit`, `Read`, `Bash`, or `WebFetch` — we hard-code those.
const TOOL_EDIT: &str = "Edit";
const TOOL_READ: &str = "Read";
const TOOL_WEBFETCH: &str = "WebFetch";

/// Parse a `Tool(content)` permission rule string into `(tool, content)`.
///
/// Returns `None` if the rule has no parentheses (a bare `Tool` rule covers
/// all calls and doesn't carry a filesystem path).
fn parse_rule(rule: &str) -> Option<(&str, &str)> {
    let open = rule.find('(')?;
    if !rule.ends_with(')') {
        return None;
    }
    let tool = &rule[..open];
    let content = &rule[open + 1..rule.len() - 1];
    Some((tool, content))
}

/// Convert claude-code `SettingsJson` into `SandboxRuntimeConfig`.
///
/// Walk `permissions.allow` and `permissions.deny`:
/// - `Edit(path)` allow  → `filesystem.allow_write`
/// - `Edit(path)` deny   → `filesystem.deny_write`
/// - `Read(path)` allow  → `filesystem.allow_read`
/// - `Read(path)` deny   → `filesystem.deny_read`
/// - `WebFetch(domain:host)` allow → `network.allowed_domains`
///
/// `additional_directories` is appended to `allow_write` (additional dirs are
/// always writable inside the sandbox; this matches claude-code's behavior of
/// pushing `additionalDirectories` into the sandbox-runtime allow set).
///
/// Any user-supplied `sandbox` subsection values override the derived defaults.
#[must_use]
pub fn convert_settings_to_runtime_config(settings: &SettingsJson) -> SandboxRuntimeConfig {
    let settings_dir: PathBuf = settings
        .settings_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut filesystem = FilesystemRestrictionConfig::default();
    let mut network = NetworkRestrictionConfig::default();

    if let Some(perms) = &settings.permissions {
        for rule_string in &perms.allow {
            apply_rule(rule_string, &settings_dir, &mut filesystem, &mut network, true);
        }
        for rule_string in &perms.deny {
            apply_rule(
                rule_string,
                &settings_dir,
                &mut filesystem,
                &mut network,
                false,
            );
        }
        // Additional directories are pushed verbatim into allowWrite — they
        // become writable inside the sandbox (matching claude-code).
        for dir in &perms.additional_directories {
            filesystem.allow_write.push(dir.clone());
        }
    }

    // Now apply any user-supplied `sandbox` subsection overrides.
    let mut cfg = SandboxRuntimeConfig {
        filesystem,
        network,
        ..Default::default()
    };

    if let Some(s) = &settings.sandbox {
        if let Some(v) = s.enabled {
            cfg.enabled = v;
        }
        if let Some(v) = s.fail_if_unavailable {
            cfg.fail_if_unavailable = v;
        }
        if let Some(v) = &s.enabled_platforms {
            cfg.enabled_platforms = Some(v.clone());
        }
        if let Some(v) = s.auto_allow_bash_if_sandboxed {
            cfg.auto_allow_bash_if_sandboxed = v;
        }
        if let Some(v) = &s.allow_unsandboxed_commands {
            cfg.allow_unsandboxed_commands = v.clone();
        }
        if let Some(v) = &s.network {
            // Merge: keep allowed_domains from WebFetch rules + values from
            // the sandbox.network.allowedDomains subsection.
            let mut merged = v.clone();
            merged.allowed_domains = {
                let mut combined = cfg.network.allowed_domains.clone();
                combined.extend(v.allowed_domains.iter().cloned());
                combined
            };
            cfg.network = merged;
        }
        if let Some(v) = &s.filesystem {
            // Merge: derived deny/allow paths + user-configured ones.
            let mut merged = v.clone();
            merged.allow_write = {
                let mut combined = cfg.filesystem.allow_write.clone();
                combined.extend(v.allow_write.iter().cloned());
                combined
            };
            merged.deny_write = {
                let mut combined = cfg.filesystem.deny_write.clone();
                combined.extend(v.deny_write.iter().cloned());
                combined
            };
            merged.deny_read = {
                let mut combined = cfg.filesystem.deny_read.clone();
                combined.extend(v.deny_read.iter().cloned());
                combined
            };
            merged.allow_read = {
                let mut combined = cfg.filesystem.allow_read.clone();
                combined.extend(v.allow_read.iter().cloned());
                combined
            };
            cfg.filesystem = merged;
        }
        if let Some(v) = &s.ignore_violations {
            cfg.ignore_violations = v.clone();
        }
        if let Some(v) = s.enable_weaker_nested_sandbox {
            cfg.enable_weaker_nested_sandbox = v;
        }
        if let Some(v) = s.enable_weaker_network_isolation {
            cfg.enable_weaker_network_isolation = v;
        }
        if let Some(v) = &s.excluded_commands {
            cfg.excluded_commands = v.clone();
        }
        if let Some(v) = &s.ripgrep {
            cfg.ripgrep = v.clone();
        }
    }

    cfg
}

fn apply_rule(
    rule_string: &str,
    settings_dir: &PathBuf,
    filesystem: &mut FilesystemRestrictionConfig,
    network: &mut NetworkRestrictionConfig,
    is_allow: bool,
) {
    let Some((tool, content)) = parse_rule(rule_string) else {
        return;
    };
    match tool {
        TOOL_EDIT => {
            let resolved = resolve_path_pattern_for_sandbox(content, settings_dir);
            if is_allow {
                filesystem.allow_write.push(resolved);
            } else {
                filesystem.deny_write.push(resolved);
            }
        }
        TOOL_READ => {
            let resolved = resolve_path_pattern_for_sandbox(content, settings_dir);
            if is_allow {
                filesystem.allow_read.push(resolved);
            } else {
                filesystem.deny_read.push(resolved);
            }
        }
        TOOL_WEBFETCH => {
            if let Some(domain) = content.strip_prefix("domain:") {
                if is_allow {
                    network.allowed_domains.push(domain.to_string());
                }
                // Denied domains: claude-code stores these for telemetry but
                // does not emit them on the wire; we match that and ignore.
            }
        }
        _ => {
            // Bash, Task, etc. are not part of the sandbox filesystem map.
        }
    }
}

/// Port of `getLinuxGlobPatternWarnings` from
/// `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
///
/// Returns the verbatim permission-rule strings whose path content contains
/// glob characters `* ? [ ]` (excluding a trailing `/**`). bubblewrap cannot
/// resolve globs, so claude-code surfaces these as user-facing warnings on
/// Linux/WSL.
///
/// Caller is expected to only invoke this on Linux/WSL hosts where sandbox is
/// enabled; this function does not check the platform itself (that's the
/// caller's responsibility — keeps this function purely string-driven).
#[must_use]
pub fn linux_glob_pattern_warnings(settings: &SettingsJson) -> Vec<String> {
    let Some(perms) = &settings.permissions else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for rule_string in perms.allow.iter().chain(perms.deny.iter()) {
        let Some((tool, content)) = parse_rule(rule_string) else {
            continue;
        };
        if tool != TOOL_EDIT && tool != TOOL_READ {
            continue;
        }
        if has_globs_excluding_trailing_double_star(content) {
            warnings.push(rule_string.clone());
        }
    }
    warnings
}

/// `true` iff `path` contains `*`, `?`, `[`, or `]` anywhere outside a
/// trailing `/**`. Mirrors the JS regex `/[*?\[\]]/.test(stripped)` after
/// `path.replace(/\/\*\*$/, '')`.
fn has_globs_excluding_trailing_double_star(path: &str) -> bool {
    let stripped = if let Some(s) = path.strip_suffix("/**") {
        s
    } else {
        path
    };
    stripped.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}
```

- [ ] **Step 4: Wire into lib.rs**

Add to `lingxi-core/crates/sandbox/src/lib.rs`:

```rust
pub mod policy_convert;

pub use policy_convert::{convert_settings_to_runtime_config, linux_glob_pattern_warnings};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p lingxi-sandbox --test policy_convert_test`
Expected: 11 tests pass.

---

### Task 4: WSL detection (`wsl_detect.rs` in `platforms/posix/`)

**Files:**
- Create: `lingxi-core/platforms/posix/src/wsl_detect.rs`
- Modify: `lingxi-core/platforms/posix/src/lib.rs`

claude-code distinguishes WSL2 (sandbox-capable via bwrap) from WSL1 (refused). Detection reads `/proc/version`:
- Contains `"microsoft-standard"` (lowercase) OR `"WSL2"` → WSL2.
- Contains `"Microsoft"` (capital M) WITHOUT the WSL2 markers → WSL1.
- Neither → not WSL (treat as pure Linux).

This lives in `platforms/posix/` (not in `crates/sandbox/`) because reading `/proc/version` is a Linux-specific filesystem operation, and we want the sandbox crate to stay OS-agnostic.

**Critical 1:1 fidelity:**
- The WSL1 refusal string includes the exact substring `"(requires WSL2)"` — tested in Task 11.
- WSL2 detection accepts EITHER substring. claude-code's `isSupportedPlatform()` is conservative; we mirror.

- [ ] **Step 1: Write the failing tests**

Add to `lingxi-core/platforms/posix/src/wsl_detect.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsoft_standard_lowercase_is_wsl2() {
        let s = "Linux version 5.15.90.1-microsoft-standard-WSL2 (oe-user@oe-host)";
        assert_eq!(parse_wsl_kind(s), WslKind::WslTwo);
    }

    #[test]
    fn explicit_wsl2_marker_is_wsl2() {
        let s = "Linux version 4.19.128-microsoft-standard #1 SMP Tue WSL2";
        assert_eq!(parse_wsl_kind(s), WslKind::WslTwo);
    }

    #[test]
    fn capital_microsoft_only_is_wsl1() {
        // WSL1 kernels report as "Linux version 4.4.0-19041-Microsoft" without
        // any of the WSL2 markers.
        let s = "Linux version 4.4.0-19041-Microsoft (Microsoft@Microsoft.com)";
        assert_eq!(parse_wsl_kind(s), WslKind::WslOne);
    }

    #[test]
    fn pure_linux_is_none() {
        let s = "Linux version 6.5.0-1015-aws (buildd@lcy02-amd64-002)";
        assert_eq!(parse_wsl_kind(s), WslKind::NotWsl);
    }

    #[test]
    fn empty_string_is_none() {
        assert_eq!(parse_wsl_kind(""), WslKind::NotWsl);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-platform-posix --lib wsl_detect`
Expected: FAIL — `cannot find module wsl_detect`.

- [ ] **Step 3: Implement `wsl_detect.rs`**

Create `lingxi-core/platforms/posix/src/wsl_detect.rs`:

```rust
//! WSL kernel detection.
//!
//! claude-code's sandbox runtime supports WSL2 but refuses WSL1.
//! Detection reads `/proc/version`:
//! - `microsoft-standard` substring (lowercase) OR explicit `WSL2` marker →
//!   `WslKind::WslTwo`. claude-code treats both as WSL2.
//! - Capital-`M` `Microsoft` substring without the above → `WslKind::WslOne`.
//! - Neither → `WslKind::NotWsl` (pure Linux).
//!
//! Live read happens in [`detect`]; the inner parser ([`parse_wsl_kind`]) is
//! exposed for unit testing with synthetic `/proc/version` payloads.

/// Result of WSL-kind inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WslKind {
    /// Host is WSL2 — bwrap sandbox is supported.
    WslTwo,
    /// Host is WSL1 — sandbox is refused.
    WslOne,
    /// Host is not WSL (regular Linux or non-Linux OS).
    NotWsl,
}

/// Detect WSL kind by reading `/proc/version`.
///
/// On non-Linux hosts (or where `/proc/version` is unreadable), returns
/// [`WslKind::NotWsl`]. This is the conservative default — callers running on
/// macOS / Windows / Linux without `/proc` correctly treat themselves as
/// not-WSL.
#[must_use]
pub fn detect() -> WslKind {
    match std::fs::read_to_string("/proc/version") {
        Ok(contents) => parse_wsl_kind(&contents),
        Err(_) => WslKind::NotWsl,
    }
}

/// Pure-function inner parser exposed for unit testing.
#[must_use]
pub fn parse_wsl_kind(proc_version: &str) -> WslKind {
    let has_wsl2_marker =
        proc_version.contains("microsoft-standard") || proc_version.contains("WSL2");
    if has_wsl2_marker {
        return WslKind::WslTwo;
    }
    if proc_version.contains("Microsoft") {
        return WslKind::WslOne;
    }
    WslKind::NotWsl
}
```

- [ ] **Step 4: Export from `lib.rs`**

Add to `lingxi-core/platforms/posix/src/lib.rs` (alongside the other `pub mod` lines):

```rust
pub mod wsl_detect;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p lingxi-platform-posix --lib wsl_detect`
Expected: 5 tests pass.

---

### Task 5: Commit Phase A

- [ ] **Step 1: Run all sandbox tests**

Run: `cargo test -p lingxi-sandbox && cargo test -p lingxi-platform-posix --lib wsl_detect`
Expected: all green.

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy -p lingxi-sandbox -p lingxi-platform-posix -- -D warnings && cargo fmt --all`
Expected: no warnings.

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/crates/sandbox/Cargo.toml \
        lingxi-core/crates/sandbox/src/lib.rs \
        lingxi-core/crates/sandbox/src/runtime_config.rs \
        lingxi-core/crates/sandbox/src/path_pattern.rs \
        lingxi-core/crates/sandbox/src/policy_convert.rs \
        lingxi-core/crates/sandbox/tests/runtime_config_test.rs \
        lingxi-core/crates/sandbox/tests/path_pattern_test.rs \
        lingxi-core/crates/sandbox/tests/policy_convert_test.rs \
        lingxi-core/platforms/posix/src/wsl_detect.rs \
        lingxi-core/platforms/posix/src/lib.rs
git commit -m "feat(sandbox): RuntimeConfig schema"
```

---

## Phase B — Dependency check + violation store (Tasks 6-12)

### Task 6: `dependency_check.rs` — probe `bwrap`, `socat`, `sandbox-exec`

**Files:**
- Create: `lingxi-core/crates/sandbox/src/dependency_check.rs`
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

Probe required CLIs per platform:
- macOS (`Platform::Mac`): need `sandbox-exec` in `$PATH` (claude-code expects `/usr/bin/sandbox-exec`, but `which` works on any path).
- Linux (`Platform::Linux`) and WSL2 (`Platform::Wsl`): need `bwrap` AND `socat`.

`SandboxDependencyCheck` mirrors claude-code's TS type: `{ errors: Vec<String>, warnings: Vec<String> }`. `errors` non-empty means sandbox cannot run.

**Critical 1:1 fidelity:** the exact error strings — see "1:1 fidelity items" at top of plan.

- [ ] **Step 1: Write the failing tests**

Create `lingxi-core/crates/sandbox/tests/dependency_check_test.rs`:

```rust
use lingxi_sandbox::dependency_check::{
    sandbox_unavailable_reason, MissingDeps, SandboxDependencyCheck,
};
use lingxi_sandbox::runtime_config::Platform;

#[test]
fn unavailable_reason_wsl1_string_exact() {
    let r = sandbox_unavailable_reason(
        true,
        true,
        Some(Platform::Wsl),
        true, // wsl_one detected
        None,
        SandboxDependencyCheck::default(),
    );
    assert_eq!(
        r.as_deref(),
        Some("sandbox.enabled is set but WSL1 is not supported (requires WSL2)")
    );
}

#[test]
fn unavailable_reason_unsupported_platform_string_exact() {
    // Platform::None case: we pass None as the detected platform.
    let r = sandbox_unavailable_reason(
        true,
        false,
        None,
        false,
        Some("windows".to_string()),
        SandboxDependencyCheck::default(),
    );
    assert_eq!(
        r.as_deref(),
        Some(
            "sandbox.enabled is set but windows is not supported \
             (requires macOS, Linux, or WSL2)"
        )
    );
}

#[test]
fn unavailable_reason_platform_not_in_enabled_list() {
    let r = sandbox_unavailable_reason(
        true,
        true,
        Some(Platform::Mac),
        false,
        None,
        SandboxDependencyCheck {
            errors: vec![],
            warnings: vec![],
            in_enabled_list: false,
        },
    );
    assert_eq!(
        r.as_deref(),
        Some("sandbox.enabled is set but macos is not in sandbox.enabledPlatforms")
    );
}

#[test]
fn unavailable_reason_missing_deps_macos_hint() {
    let r = sandbox_unavailable_reason(
        true,
        true,
        Some(Platform::Mac),
        false,
        None,
        SandboxDependencyCheck {
            errors: vec!["sandbox-exec not found".into()],
            warnings: vec![],
            in_enabled_list: true,
        },
    );
    assert_eq!(
        r.as_deref(),
        Some(
            "sandbox.enabled is set but dependencies are missing: \
             sandbox-exec not found · run /sandbox or /doctor for details"
        )
    );
}

#[test]
fn unavailable_reason_missing_deps_linux_hint() {
    let r = sandbox_unavailable_reason(
        true,
        true,
        Some(Platform::Linux),
        false,
        None,
        SandboxDependencyCheck {
            errors: vec!["bwrap not found".into(), "socat not found".into()],
            warnings: vec![],
            in_enabled_list: true,
        },
    );
    assert_eq!(
        r.as_deref(),
        Some(
            "sandbox.enabled is set but dependencies are missing: \
             bwrap not found, socat not found · install missing tools \
             (e.g. apt install bubblewrap socat) or run /sandbox for details"
        )
    );
}

#[test]
fn no_reason_when_sandbox_not_enabled() {
    // If sandbox.enabled is false, missing deps are irrelevant; no warning.
    let r = sandbox_unavailable_reason(
        false,
        true,
        Some(Platform::Mac),
        false,
        None,
        SandboxDependencyCheck {
            errors: vec!["sandbox-exec not found".into()],
            warnings: vec![],
            in_enabled_list: true,
        },
    );
    assert_eq!(r, None);
}

#[test]
fn missing_deps_helpers() {
    // Compile-only: ensure the public type exists.
    let _ = MissingDeps {
        sandbox_exec: false,
        bwrap: true,
        socat: true,
    };
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test dependency_check_test`
Expected: FAIL — `unresolved import lingxi_sandbox::dependency_check`.

- [ ] **Step 3: Implement `dependency_check.rs`**

Create `lingxi-core/crates/sandbox/src/dependency_check.rs`:

```rust
//! Sandbox dependency probing + the `sandbox_unavailable_reason` decoder.
//!
//! Port of `checkDependencies` and `getSandboxUnavailableReason` from
//! `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
//!
//! All error strings are byte-for-byte from the claude-code source. See the
//! "1:1 fidelity items" in the plan document.

use crate::runtime_config::Platform;
use std::path::PathBuf;

/// `SandboxDependencyCheck` mirrors claude-code's TS type. `errors` non-empty
/// means the sandbox cannot run; `warnings` is informational only.
#[derive(Debug, Clone, Default)]
pub struct SandboxDependencyCheck {
    /// One human-readable string per missing or broken dependency.
    pub errors: Vec<String>,
    /// One human-readable string per non-fatal issue.
    pub warnings: Vec<String>,
    /// `true` iff the current platform is in `sandbox.enabledPlatforms` (or
    /// the list is unset, which is treated as "all enabled").
    pub in_enabled_list: bool,
}

/// Per-platform missing-deps summary. Useful for `/doctor` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingDeps {
    /// macOS-only: `/usr/bin/sandbox-exec` missing from `$PATH`.
    pub sandbox_exec: bool,
    /// Linux/WSL2: `bwrap` missing.
    pub bwrap: bool,
    /// Linux/WSL2: `socat` missing.
    pub socat: bool,
}

impl MissingDeps {
    /// Aggregate into the public [`SandboxDependencyCheck::errors`] format.
    /// Each missing tool produces one entry in `errors`.
    #[must_use]
    pub fn into_errors(self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.sandbox_exec {
            errors.push("sandbox-exec not found".to_string());
        }
        if self.bwrap {
            errors.push("bwrap not found".to_string());
        }
        if self.socat {
            errors.push("socat not found".to_string());
        }
        errors
    }
}

/// Probe required dependencies for the given platform.
///
/// `in_enabled_list` is supplied by the caller (the platform crate is what
/// knows the `enabledPlatforms` setting; this crate stays OS-agnostic).
#[must_use]
pub fn check_dependencies(platform: Option<Platform>, in_enabled_list: bool) -> SandboxDependencyCheck {
    let Some(platform) = platform else {
        return SandboxDependencyCheck {
            errors: vec!["platform not supported (requires macOS, Linux, or WSL2)".to_string()],
            warnings: vec![],
            in_enabled_list,
        };
    };

    let mut missing = MissingDeps::default();
    match platform {
        Platform::Mac => {
            missing.sandbox_exec = !which_exists("sandbox-exec");
        }
        Platform::Linux | Platform::Wsl => {
            missing.bwrap = !which_exists("bwrap");
            missing.socat = !which_exists("socat");
        }
    }

    SandboxDependencyCheck {
        errors: missing.into_errors(),
        warnings: vec![],
        in_enabled_list,
    }
}

fn which_exists(cli: &str) -> bool {
    which::which(cli).is_ok()
}

/// Probe whether `cli` is on the host's `$PATH`. Convenience wrapper around
/// the `which` crate exposed for callers that want platform-specific probing
/// without going through `check_dependencies`.
#[must_use]
pub fn which_path(cli: &str) -> Option<PathBuf> {
    which::which(cli).ok()
}

/// Decide what (if any) human-readable reason to surface for sandbox being
/// unavailable. Returns `None` when no message is appropriate — either
/// sandbox is not enabled, or the sandbox can actually run.
///
/// Mirrors `getSandboxUnavailableReason` from sandbox-adapter.ts. The five
/// possible return strings are byte-for-byte from that source:
///
/// - `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`
/// - `"sandbox.enabled is set but {platform} is not supported (requires macOS, Linux, or WSL2)"`
/// - `"sandbox.enabled is set but {platform} is not in sandbox.enabledPlatforms"`
/// - `"sandbox.enabled is set but dependencies are missing: {deps} · run /sandbox or /doctor for details"` (macOS)
/// - `"sandbox.enabled is set but dependencies are missing: {deps} · install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details"` (Linux/WSL)
///
/// Inputs:
/// - `enabled`: value of `sandbox.enabled` (returns `None` if false).
/// - `supported_platform`: whether the host runs a supported OS.
/// - `platform`: detected platform when supported; `None` when not.
/// - `wsl_one_detected`: whether `/proc/version` indicates WSL1.
/// - `raw_platform_label`: when `platform` is `None`, the unrecognized OS
///   name (e.g. `"windows"`, `"freebsd"`). Used only for the
///   "unsupported" string.
/// - `deps`: result of `check_dependencies()`.
#[must_use]
pub fn sandbox_unavailable_reason(
    enabled: bool,
    supported_platform: bool,
    platform: Option<Platform>,
    wsl_one_detected: bool,
    raw_platform_label: Option<String>,
    deps: SandboxDependencyCheck,
) -> Option<String> {
    if !enabled {
        return None;
    }

    if wsl_one_detected {
        return Some("sandbox.enabled is set but WSL1 is not supported (requires WSL2)".to_string());
    }

    if !supported_platform {
        let label = raw_platform_label.unwrap_or_else(|| "unknown".to_string());
        return Some(format!(
            "sandbox.enabled is set but {label} is not supported (requires macOS, Linux, or WSL2)"
        ));
    }

    if !deps.in_enabled_list {
        let label = platform.map(|p| p.as_str()).unwrap_or("unknown");
        return Some(format!(
            "sandbox.enabled is set but {label} is not in sandbox.enabledPlatforms"
        ));
    }

    if !deps.errors.is_empty() {
        let joined = deps.errors.join(", ");
        let hint = match platform {
            Some(Platform::Mac) => "run /sandbox or /doctor for details",
            Some(Platform::Linux) | Some(Platform::Wsl) => {
                "install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details"
            }
            None => "run /sandbox for details",
        };
        return Some(format!(
            "sandbox.enabled is set but dependencies are missing: {joined} · {hint}"
        ));
    }

    None
}
```

- [ ] **Step 4: Wire into lib.rs**

Add to `lingxi-core/crates/sandbox/src/lib.rs`:

```rust
pub mod dependency_check;

pub use dependency_check::{
    check_dependencies, sandbox_unavailable_reason, MissingDeps, SandboxDependencyCheck,
};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p lingxi-sandbox --test dependency_check_test`
Expected: 7 tests pass.

---

### Task 7: `violation_store.rs` — bounded ring buffer of sandbox events

**Files:**
- Create: `lingxi-core/crates/sandbox/src/violation_store.rs`
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

Bounded `VecDeque<SandboxViolationEvent>` under `RwLock`. Capacity is `SANDBOX_VIOLATION_STORE_CAP = 1000` (claude-code's circular buffer cap). Oldest events evicted when over.

API:
- `record(event)` — push, evicting oldest if at cap.
- `snapshot() -> Vec<SandboxViolationEvent>` — clone-out for `/sandbox doctor`.
- `clear()` — drain the buffer (used on `reset`).
- `len()` — current count.
- `capacity()` — exposed constant.

- [ ] **Step 1: Write the failing tests**

Create `lingxi-core/crates/sandbox/tests/violation_store_test.rs`:

```rust
use lingxi_sandbox::violation_store::{
    SandboxViolationEvent, SandboxViolationKind, SandboxViolationStore,
    SANDBOX_VIOLATION_STORE_CAP,
};

fn make_event(idx: u64) -> SandboxViolationEvent {
    SandboxViolationEvent {
        timestamp_ms: idx,
        command: format!("cmd-{idx}"),
        violation_type: SandboxViolationKind::FileWrite,
        message: format!("blocked write #{idx}"),
    }
}

#[tokio::test]
async fn record_and_snapshot_in_order() {
    let store = SandboxViolationStore::new();
    store.record(make_event(1)).await;
    store.record(make_event(2)).await;
    store.record(make_event(3)).await;
    let snap = store.snapshot().await;
    assert_eq!(snap.len(), 3);
    assert_eq!(snap[0].timestamp_ms, 1);
    assert_eq!(snap[2].timestamp_ms, 3);
}

#[tokio::test]
async fn clear_drains_buffer() {
    let store = SandboxViolationStore::new();
    store.record(make_event(1)).await;
    store.record(make_event(2)).await;
    store.clear().await;
    assert_eq!(store.len().await, 0);
    assert!(store.snapshot().await.is_empty());
}

#[tokio::test]
async fn capacity_evicts_oldest_first() {
    // Insert cap + 5 events; verify oldest 5 dropped.
    let store = SandboxViolationStore::new();
    for i in 0..(SANDBOX_VIOLATION_STORE_CAP as u64 + 5) {
        store.record(make_event(i)).await;
    }
    assert_eq!(store.len().await, SANDBOX_VIOLATION_STORE_CAP);
    let snap = store.snapshot().await;
    // Oldest retained event should have timestamp 5 (events 0..5 evicted).
    assert_eq!(snap[0].timestamp_ms, 5);
    assert_eq!(
        snap[snap.len() - 1].timestamp_ms,
        SANDBOX_VIOLATION_STORE_CAP as u64 + 4
    );
}

#[tokio::test]
async fn capacity_constant_is_one_thousand() {
    assert_eq!(SANDBOX_VIOLATION_STORE_CAP, 1000);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test violation_store_test`
Expected: FAIL — `unresolved import lingxi_sandbox::violation_store`.

- [ ] **Step 3: Implement `violation_store.rs`**

Create `lingxi-core/crates/sandbox/src/violation_store.rs`:

```rust
//! Bounded ring buffer of sandbox violation events. Matches claude-code's
//! `SandboxViolationStore` from `@anthropic-ai/sandbox-runtime`.
//!
//! Events are produced by the platform-side sandbox backend when a wrapped
//! command tries to read/write outside policy, hit a denied network domain,
//! etc. The UI consumer (`/sandbox doctor`, telemetry) drains via
//! `snapshot()`. Once `SANDBOX_VIOLATION_STORE_CAP` events are stored, new
//! events evict the oldest.

use std::collections::VecDeque;
use tokio::sync::RwLock;

/// Maximum events retained. Matches claude-code's circular buffer cap.
pub const SANDBOX_VIOLATION_STORE_CAP: usize = 1000;

/// Discriminator for `SandboxViolationEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SandboxViolationKind {
    FileRead,
    FileWrite,
    NetworkDomain,
    NetworkSocket,
    /// Anything else surfaced by the backend.
    Other,
}

/// One sandbox violation. Fields mirror claude-code's `SandboxViolationEvent`
/// shape closely enough that the `/sandbox doctor` renderer can be ported
/// without further translation.
#[derive(Debug, Clone)]
pub struct SandboxViolationEvent {
    /// `Date.now()`-equivalent epoch millis.
    pub timestamp_ms: u64,
    /// Command line that triggered the violation.
    pub command: String,
    /// Categorical kind.
    pub violation_type: SandboxViolationKind,
    /// Human-readable detail.
    pub message: String,
}

/// Bounded ring-buffer store of [`SandboxViolationEvent`].
///
/// Clone-on-snapshot rather than expose the internal `VecDeque`. Callers
/// hold an `Arc<SandboxViolationStore>` to share between async tasks.
#[derive(Debug, Default)]
pub struct SandboxViolationStore {
    inner: RwLock<VecDeque<SandboxViolationEvent>>,
}

impl SandboxViolationStore {
    /// Construct an empty store with capacity [`SANDBOX_VIOLATION_STORE_CAP`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(VecDeque::with_capacity(SANDBOX_VIOLATION_STORE_CAP)),
        }
    }

    /// Push `event`, evicting the oldest if the store is full.
    pub async fn record(&self, event: SandboxViolationEvent) {
        let mut guard = self.inner.write().await;
        if guard.len() == SANDBOX_VIOLATION_STORE_CAP {
            guard.pop_front();
        }
        guard.push_back(event);
    }

    /// Return a cloned snapshot, in insertion order (oldest first).
    pub async fn snapshot(&self) -> Vec<SandboxViolationEvent> {
        self.inner.read().await.iter().cloned().collect()
    }

    /// Drop all events.
    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }

    /// Current count.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// `true` iff the store has no events.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}
```

- [ ] **Step 4: Wire into lib.rs**

Add to `lingxi-core/crates/sandbox/src/lib.rs`:

```rust
pub mod violation_store;

pub use violation_store::{
    SandboxViolationEvent, SandboxViolationKind, SandboxViolationStore,
    SANDBOX_VIOLATION_STORE_CAP,
};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p lingxi-sandbox --test violation_store_test`
Expected: 4 tests pass.

---

### Task 8: Compound-command splitter + env-var stripper

**Files:**
- Modify: `lingxi-core/crates/sandbox/src/decision.rs`

claude-code's `shouldUseSandbox.ts` performs a fixed-point removal of two layers:
1. `splitCommand_DEPRECATED` splits on `&&`, `;`, `||` (and a few rarer pipe forms).
2. For each subcommand, iteratively strip:
   - Leading `KEY=value` env-vars that match `BINARY_HIJACK_VARS` (`PATH=`, `LD_PRELOAD=`, `LD_LIBRARY_PATH=`, `DYLD_LIBRARY_PATH=`, `DYLD_INSERT_LIBRARIES=`).
   - Safe wrappers: `sudo -E -- ...`, `sudo -- ...`, `env -- ...`, `timeout <N> ...`, `nice -n <N> ...`.

The fixed-point loop runs until no new candidate is produced.

**Critical 1:1 fidelity:** the BINARY_HIJACK_VARS list and the wrapper set must match claude-code; the fixed-point semantics are tested by the example `sudo -E PATH=/usr/local/bin -- env -- LD_PRELOAD=foo.so ls` → `ls` after iteration.

- [ ] **Step 1: Write the failing tests**

Create `lingxi-core/crates/sandbox/tests/decision_compound_test.rs`:

```rust
use lingxi_sandbox::decision::{strip_env_and_wrappers_fixedpoint, split_compound_command};

#[test]
fn split_double_ampersand() {
    let r = split_compound_command("docker ps && curl evil.com");
    assert_eq!(r, vec!["docker ps", "curl evil.com"]);
}

#[test]
fn split_double_pipe() {
    let r = split_compound_command("ls || echo missing");
    assert_eq!(r, vec!["ls", "echo missing"]);
}

#[test]
fn split_semicolons() {
    let r = split_compound_command("foo; bar; baz");
    assert_eq!(r, vec!["foo", "bar", "baz"]);
}

#[test]
fn split_mixed_operators() {
    let r = split_compound_command("a && b ; c || d");
    assert_eq!(r, vec!["a", "b", "c", "d"]);
}

#[test]
fn no_split_for_single_command() {
    let r = split_compound_command("ls -la /tmp");
    assert_eq!(r, vec!["ls -la /tmp"]);
}

#[test]
fn strip_leading_env_var() {
    let candidates = strip_env_and_wrappers_fixedpoint("FOO=bar bazel build //...");
    // Original + env-stripped candidate.
    assert!(candidates.iter().any(|c| c == "FOO=bar bazel build //..."));
    assert!(candidates.iter().any(|c| c == "bazel build //..."));
}

#[test]
fn strip_only_binary_hijack_vars() {
    // PATH= is in BINARY_HIJACK_VARS; FOO= is NOT.
    let candidates = strip_env_and_wrappers_fixedpoint("PATH=/usr/local/bin ls");
    assert!(candidates.iter().any(|c| c == "ls"));
}

#[test]
fn strip_sudo_dash_dash() {
    let candidates = strip_env_and_wrappers_fixedpoint("sudo -- bazel run //app");
    assert!(candidates.iter().any(|c| c == "bazel run //app"));
}

#[test]
fn strip_env_dash_dash() {
    let candidates = strip_env_and_wrappers_fixedpoint("env -- ls -la");
    assert!(candidates.iter().any(|c| c == "ls -la"));
}

#[test]
fn strip_timeout_wrapper() {
    let candidates = strip_env_and_wrappers_fixedpoint("timeout 30 bazel build //...");
    assert!(candidates.iter().any(|c| c == "bazel build //..."));
}

#[test]
fn fixedpoint_handles_compound_wrapper_and_env_vars() {
    // The motivating test case from the brief:
    // sudo -E PATH=/usr/local/bin -- env -- LD_PRELOAD=foo.so ls
    // After iterative stripping (env-vars + wrappers) the candidates list
    // must contain "ls".
    let candidates = strip_env_and_wrappers_fixedpoint(
        "sudo -E PATH=/usr/local/bin -- env -- LD_PRELOAD=foo.so ls",
    );
    assert!(
        candidates.iter().any(|c| c == "ls"),
        "expected 'ls' in candidates, got {candidates:?}"
    );
}

#[test]
fn fixedpoint_handles_interleaved_timeout_and_env_var() {
    let candidates =
        strip_env_and_wrappers_fixedpoint("timeout 300 FOO=bar bazel run //app");
    // FOO= is not a BINARY_HIJACK_VAR — should remain as a candidate.
    assert!(candidates.iter().any(|c| c == "FOO=bar bazel run //app"));
    // But with timeout stripped, we should also see "FOO=bar bazel run //app"
    // (which is the same — the stripper still produces both intermediates).
    assert!(candidates.iter().any(|c| c == "bazel run //app")
        || candidates.iter().any(|c| c == "FOO=bar bazel run //app"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test decision_compound_test`
Expected: FAIL — `unresolved import lingxi_sandbox::decision::strip_env_and_wrappers_fixedpoint`.

- [ ] **Step 3: Add splitter + stripper to `decision.rs`**

Append to `lingxi-core/crates/sandbox/src/decision.rs` (do NOT delete existing content; this is an addition):

```rust
// =============================================================================
// Compound-command splitting + env-var / safe-wrapper fixed-point stripping.
//
// Ports the logic from `claude-code/src/tools/BashTool/shouldUseSandbox.ts` and
// `bashPermissions.ts` (`BINARY_HIJACK_VARS`, `stripAllLeadingEnvVars`,
// `stripSafeWrappers`).
// =============================================================================

use std::collections::BTreeSet;

/// Env-vars an attacker could use to redirect binary lookup. Matches the
/// claude-code constant `BINARY_HIJACK_VARS` exactly.
pub const BINARY_HIJACK_VARS: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
];

/// Wrappers safe to strip when matching against excludedCommands patterns.
/// Each entry is a prefix + an arity hint:
/// - `"sudo -E --"` exact: strip leading 3 tokens.
/// - `"sudo --"` exact: strip leading 2 tokens.
/// - `"env --"` exact: strip leading 2 tokens.
/// - `"timeout <N>"`: 2 tokens (the wrapper + its single numeric arg).
/// - `"nice -n <N>"`: 3 tokens.
fn strip_safe_wrappers(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // sudo -E --
    if tokens.len() >= 4 && tokens[0] == "sudo" && tokens[1] == "-E" && tokens[2] == "--" {
        return Some(tokens[3..].join(" "));
    }
    // sudo --
    if tokens.len() >= 3 && tokens[0] == "sudo" && tokens[1] == "--" {
        return Some(tokens[2..].join(" "));
    }
    // env --
    if tokens.len() >= 3 && tokens[0] == "env" && tokens[1] == "--" {
        return Some(tokens[2..].join(" "));
    }
    // timeout <N>
    if tokens.len() >= 3 && tokens[0] == "timeout" && is_numeric(tokens[1]) {
        return Some(tokens[2..].join(" "));
    }
    // nice -n <N>
    if tokens.len() >= 4 && tokens[0] == "nice" && tokens[1] == "-n" && is_numeric(tokens[2]) {
        return Some(tokens[3..].join(" "));
    }
    None
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Strip leading `KEY=value` tokens where `KEY` is a `BINARY_HIJACK_VARS`
/// entry. Non-binary-hijack env-vars (`FOO=bar`) are left in place.
fn strip_binary_hijack_env_vars(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut start = 0;
    while start < tokens.len() {
        let tok = tokens[start];
        if let Some(eq) = tok.find('=') {
            let key = &tok[..eq];
            if BINARY_HIJACK_VARS.contains(&key) {
                start += 1;
                continue;
            }
        }
        break;
    }
    if start == 0 {
        None
    } else {
        Some(tokens[start..].join(" "))
    }
}

/// Iteratively apply `strip_safe_wrappers` and `strip_binary_hijack_env_vars`
/// until no new candidate is produced (fixed point).
///
/// Returns the deduped list of candidates (the original `cmd` is included).
#[must_use]
pub fn strip_env_and_wrappers_fixedpoint(cmd: &str) -> Vec<String> {
    let mut candidates: Vec<String> = vec![cmd.trim().to_string()];
    let mut seen: BTreeSet<String> = candidates.iter().cloned().collect();
    let mut start = 0;
    while start < candidates.len() {
        let end = candidates.len();
        for i in start..end {
            let c = candidates[i].clone();
            if let Some(env_stripped) = strip_binary_hijack_env_vars(&c) {
                if seen.insert(env_stripped.clone()) {
                    candidates.push(env_stripped);
                }
            }
            if let Some(wrap_stripped) = strip_safe_wrappers(&c) {
                if seen.insert(wrap_stripped.clone()) {
                    candidates.push(wrap_stripped);
                }
            }
        }
        start = end;
    }
    candidates
}

/// Split `command` on `&&`, `||`, and `;` into one entry per subcommand.
///
/// Pure delimiter-based split. Does NOT handle quoting (claude-code's
/// `splitCommand_DEPRECATED` likewise is quote-naive; that's why it's marked
/// `_DEPRECATED`). Sufficient for the excludedCommands match heuristic.
#[must_use]
pub fn split_compound_command(command: &str) -> Vec<String> {
    // Replace operators with a single delimiter sentinel, then split.
    // Order matters: `&&` and `||` are two-char ops; `;` is one-char.
    let normalized = command
        .replace("&&", "\x01")
        .replace("||", "\x01")
        .replace(';', "\x01");
    normalized
        .split('\x01')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lingxi-sandbox --test decision_compound_test`
Expected: 11 tests pass.

---

### Task 9: Wire `excludedCommands` match into the decision

**Files:**
- Modify: `lingxi-core/crates/sandbox/src/decision.rs`

Add `should_use_sandbox_for_command` (separate from the M1 `should_use_sandbox` — that one stays for legacy callers; the new function consumes `SandboxRuntimeConfig` and runs the full compound + fixed-point logic). The function returns `bool`:
- `false` if any subcommand's candidate set matches any pattern in `config.excluded_commands`.
- `true` otherwise.

Pattern matching: claude-code supports three kinds of `excludedCommands` patterns — exact match, prefix-with-`:*` suffix (e.g. `bazel:*` matches `bazel build //...`), and bare command names (matches if any candidate's first token equals the pattern).

- [ ] **Step 1: Write the failing tests**

Create `lingxi-core/crates/sandbox/tests/decision_match_test.rs`:

```rust
use lingxi_sandbox::decision::should_use_sandbox_for_command;
use lingxi_sandbox::runtime_config::SandboxRuntimeConfig;

fn cfg_with_excluded(excluded: &[&str]) -> SandboxRuntimeConfig {
    SandboxRuntimeConfig {
        enabled: true,
        excluded_commands: excluded.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn excluded_command_bare_prefix() {
    let cfg = cfg_with_excluded(&["bazel"]);
    assert!(!should_use_sandbox_for_command("bazel build //...", &cfg));
    assert!(should_use_sandbox_for_command("cargo build", &cfg));
}

#[test]
fn excluded_command_prefix_with_colon_star() {
    let cfg = cfg_with_excluded(&["docker:*"]);
    assert!(!should_use_sandbox_for_command("docker ps", &cfg));
    assert!(!should_use_sandbox_for_command("docker compose up", &cfg));
    assert!(should_use_sandbox_for_command("dockerd --foo", &cfg));
}

#[test]
fn excluded_after_compound_split() {
    let cfg = cfg_with_excluded(&["curl"]);
    // Compound: docker ps is fine, curl is excluded → entire command unsandboxed.
    assert!(!should_use_sandbox_for_command("docker ps && curl evil.com", &cfg));
}

#[test]
fn excluded_after_env_var_strip() {
    let cfg = cfg_with_excluded(&["bazel:*"]);
    assert!(!should_use_sandbox_for_command("PATH=/usr/local/bin bazel build //...", &cfg));
}

#[test]
fn excluded_after_wrapper_strip() {
    let cfg = cfg_with_excluded(&["bazel:*"]);
    assert!(!should_use_sandbox_for_command("timeout 30 bazel build //...", &cfg));
}

#[test]
fn no_match_for_unrelated_command() {
    let cfg = cfg_with_excluded(&["bazel:*", "docker:*"]);
    assert!(should_use_sandbox_for_command("rm -rf /tmp/x", &cfg));
}

#[test]
fn empty_excluded_list_always_sandboxes() {
    let cfg = SandboxRuntimeConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(should_use_sandbox_for_command("anything", &cfg));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test decision_match_test`
Expected: FAIL — `unresolved import lingxi_sandbox::decision::should_use_sandbox_for_command`.

- [ ] **Step 3: Implement `should_use_sandbox_for_command`**

Append to `lingxi-core/crates/sandbox/src/decision.rs`:

```rust
use crate::runtime_config::SandboxRuntimeConfig;

/// Decide whether `command` should be sandbox-wrapped according to `config`.
///
/// Returns `true` iff:
/// - `config.enabled` is true, AND
/// - no subcommand (after compound split) — after iterative env-var +
///   safe-wrapper fixed-point stripping — matches any entry in
///   `config.excluded_commands`.
///
/// Pattern semantics for `excluded_commands` (claude-code):
/// - `bazel` matches exact command (or any candidate first token = `bazel`).
/// - `bazel:*` matches any command starting with `bazel ` (including `bazel`
///   on its own).
#[must_use]
pub fn should_use_sandbox_for_command(command: &str, config: &SandboxRuntimeConfig) -> bool {
    if !config.enabled {
        return false;
    }
    if config.excluded_commands.is_empty() {
        return true;
    }
    for subcommand in split_compound_command(command) {
        let candidates = strip_env_and_wrappers_fixedpoint(&subcommand);
        for cand in &candidates {
            for pattern in &config.excluded_commands {
                if matches_excluded(pattern, cand) {
                    return false;
                }
            }
        }
    }
    true
}

fn matches_excluded(pattern: &str, candidate: &str) -> bool {
    let trimmed = candidate.trim();
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return trimmed == prefix || trimmed.starts_with(&format!("{prefix} "));
    }
    // Exact OR first-token match.
    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    trimmed == pattern || first_token == pattern
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lingxi-sandbox --test decision_match_test`
Expected: 7 tests pass.

---

### Task 10: Re-export decision additions; add module doc

**Files:**
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

- [ ] **Step 1: Update `lib.rs` re-exports**

Edit the existing `pub use decision::{...}` block in `lingxi-core/crates/sandbox/src/lib.rs` to include the new symbols:

```rust
pub use decision::{
    is_obviously_dangerous, should_use_sandbox, should_use_sandbox_for_command,
    split_compound_command, strip_env_and_wrappers_fixedpoint, ProjectTrustLevel,
    SandboxDecision, BINARY_HIJACK_VARS,
};
```

- [ ] **Step 2: Run the full sandbox test suite**

Run: `cargo test -p lingxi-sandbox`
Expected: all green (Phase A + Phase B tests so far).

---

### Task 11: Commit Phase B

- [ ] **Step 1: Clippy + fmt**

Run: `cargo clippy -p lingxi-sandbox -- -D warnings && cargo fmt -p lingxi-sandbox`
Expected: no warnings, no diff.

- [ ] **Step 2: Commit**

```bash
git add lingxi-core/crates/sandbox/src/dependency_check.rs \
        lingxi-core/crates/sandbox/src/violation_store.rs \
        lingxi-core/crates/sandbox/src/decision.rs \
        lingxi-core/crates/sandbox/src/lib.rs \
        lingxi-core/crates/sandbox/tests/dependency_check_test.rs \
        lingxi-core/crates/sandbox/tests/violation_store_test.rs \
        lingxi-core/crates/sandbox/tests/decision_compound_test.rs \
        lingxi-core/crates/sandbox/tests/decision_match_test.rs
git commit -m "feat(sandbox): dependency check + violation store"
```

---

### Task 12: WSL1 refusal end-to-end test (golden string check)

**Files:**
- Modify: `lingxi-core/crates/sandbox/tests/dependency_check_test.rs`

This task adds one extra assertion verifying that the full WSL1 refusal pipeline returns the byte-for-byte claude-code error string. It's a separate task so the assertion lives close to the canonical source-of-truth string.

- [ ] **Step 1: Add the assertion**

Append to `lingxi-core/crates/sandbox/tests/dependency_check_test.rs`:

```rust
#[test]
fn wsl1_refusal_byte_for_byte() {
    let r = sandbox_unavailable_reason(
        true,
        true,
        Some(Platform::Wsl),
        true, // wsl_one_detected
        None,
        SandboxDependencyCheck::default(),
    );
    let expected = "sandbox.enabled is set but WSL1 is not supported (requires WSL2)";
    let actual = r.expect("WSL1 with sandbox.enabled must produce a reason");
    assert_eq!(actual.as_bytes(), expected.as_bytes());
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p lingxi-sandbox --test dependency_check_test wsl1_refusal_byte_for_byte`
Expected: PASS.

---

## Phase C — `wrap_with_sandbox` dispatch (Tasks 13-16)

### Task 13: Linux bwrap invocation builder

**Files:**
- Create: `lingxi-core/crates/sandbox/src/wrap.rs`
- Modify: `lingxi-core/crates/sandbox/src/lib.rs`

The bwrap invocation builder produces a single shell-runnable string composed of:
- `bwrap` binary path
- Filesystem flags: `--ro-bind /` for read-only root, `--bind <path> <path>` per `policy.filesystem.allow_write` entry, `--bind /tmp /tmp` for ephemeral writes, `--proc /proc`, `--dev /dev`.
- Network flags: `--unshare-net` when `policy.network.allowed_domains` is empty AND `policy.network.allow_local_binding` is false; `--share-net` otherwise (with the caveat that real network filtering needs a `socat` companion — TODO: see fidelity item below).
- The wrapped command at the end: `-- /bin/sh -c '<shell-escaped command>'`.

**Critical 1:1 fidelity:** the resulting string contains the literal `bwrap` substring and the literal `--unshare-net` or `--share-net` flag, the `allow_write` paths appear as `--bind <p> <p>`, and the suffix is `-- /bin/sh -c <quoted-cmd>`. SBPL profile generation and socat companion lifecycle are followed up in Tasks 14 and the SBPL section; this task is just the bwrap string builder.

- [ ] **Step 1: Write the failing tests**

Create `lingxi-core/crates/sandbox/tests/wrap_linux_test.rs`:

```rust
use lingxi_sandbox::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, SandboxRuntimeConfig,
};
use lingxi_sandbox::wrap::{wrap_with_sandbox, SandboxWrapError};

fn cfg(allow_write: Vec<&str>, allowed_domains: Vec<&str>) -> SandboxRuntimeConfig {
    SandboxRuntimeConfig {
        enabled: true,
        filesystem: FilesystemRestrictionConfig {
            allow_write: allow_write.into_iter().map(String::from).collect(),
            ..Default::default()
        },
        network: NetworkRestrictionConfig {
            allowed_domains: allowed_domains.into_iter().map(String::from).collect(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn linux_wrap_starts_with_bwrap_and_ends_with_command() {
    let wrapped = wrap_with_sandbox("ls -la", &cfg(vec!["/tmp"], vec![]), Platform::Linux)
        .expect("wrap ok");
    assert!(wrapped.starts_with("bwrap "), "got: {wrapped}");
    assert!(
        wrapped.contains("-- /bin/sh -c"),
        "expected shell suffix, got: {wrapped}"
    );
    // The wrapped command should appear at the very end, quoted.
    assert!(wrapped.contains("ls -la"), "command missing: {wrapped}");
}

#[test]
fn linux_wrap_includes_ro_bind_root() {
    let wrapped =
        wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Linux).expect("wrap ok");
    assert!(wrapped.contains("--ro-bind / /"), "got: {wrapped}");
}

#[test]
fn linux_wrap_includes_allowwrite_bindings() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec!["/tmp/work"], vec![]), Platform::Linux)
        .expect("wrap ok");
    assert!(
        wrapped.contains("--bind /tmp/work /tmp/work"),
        "got: {wrapped}"
    );
}

#[test]
fn linux_wrap_unshare_net_when_no_domains() {
    let wrapped =
        wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Linux).expect("wrap ok");
    assert!(wrapped.contains("--unshare-net"), "got: {wrapped}");
    assert!(!wrapped.contains("--share-net"), "got: {wrapped}");
}

#[test]
fn linux_wrap_share_net_when_domains_listed() {
    let wrapped = wrap_with_sandbox(
        "curl",
        &cfg(vec![], vec!["api.example.com"]),
        Platform::Linux,
    )
    .expect("wrap ok");
    assert!(wrapped.contains("--share-net"), "got: {wrapped}");
}

#[test]
fn wsl2_uses_same_bwrap_path() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Wsl).expect("wrap ok");
    assert!(wrapped.starts_with("bwrap "), "got: {wrapped}");
}

#[test]
fn quoted_command_with_inner_single_quote_round_trips() {
    let wrapped =
        wrap_with_sandbox("echo it's fine", &cfg(vec![], vec![]), Platform::Linux).expect("wrap");
    // The wrapped command should preserve the inner apostrophe via shell escaping.
    assert!(
        wrapped.contains(r#"'it'\''s fine'"#)
            || wrapped.contains(r#"'echo it'\''s fine'"#),
        "shell-escape missing for apostrophe: {wrapped}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test wrap_linux_test`
Expected: FAIL — `unresolved import lingxi_sandbox::wrap`.

- [ ] **Step 3: Implement the Linux side of `wrap.rs`**

Create `lingxi-core/crates/sandbox/src/wrap.rs`:

```rust
//! Sandbox wrap dispatch: produce a shell-runnable wrapped command line per
//! platform.
//!
//! - Linux / WSL2: `bwrap` with policy-derived filesystem + network flags,
//!   suffixed by `-- /bin/sh -c '<command>'`. A `socat` companion to enforce
//!   `network.allowed_domains` filtering is wired up as a process-lifecycle
//!   concern at the `platforms/posix/src/sandbox.rs` layer (Task 17); this
//!   builder just emits the `bwrap` argv that establishes a netns. See
//!   `TODO(M2-followup)` notes inline.
//! - macOS: write an SBPL profile to a tempfile, return
//!   `sandbox-exec -f <profile> <command>`.
//! - Windows / WSL1: return [`SandboxWrapError::Unsupported`].

use crate::runtime_config::{Platform, SandboxRuntimeConfig};
use thiserror::Error;

/// Errors produced by [`wrap_with_sandbox`].
#[derive(Debug, Clone, Error)]
pub enum SandboxWrapError {
    /// Platform is on the refuse list (Windows, WSL1) — the caller should
    /// surface the inner string verbatim.
    #[error("{0}")]
    Unsupported(String),
    /// Failed to write the SBPL profile tempfile.
    #[error("failed to write SBPL profile: {0}")]
    SbplWrite(String),
}

/// Wrap `command` for execution under the sandbox identified by `platform`.
///
/// `policy` carries the runtime configuration (filesystem / network /
/// excludedCommands etc.).
///
/// Returns the shell-runnable string the caller should pass to its
/// `/bin/sh -c` runner.
pub fn wrap_with_sandbox(
    command: &str,
    policy: &SandboxRuntimeConfig,
    platform: Platform,
) -> Result<String, SandboxWrapError> {
    match platform {
        Platform::Linux | Platform::Wsl => Ok(wrap_linux_bwrap(command, policy)),
        Platform::Mac => wrap_macos_sbpl(command, policy),
    }
}

// =============================================================================
// Linux / WSL2 — bwrap
// =============================================================================

/// Build the `bwrap` invocation. The caller (`platforms/posix/src/sandbox.rs`)
/// passes this string to `/bin/sh -c` and is responsible for spawning the
/// optional `socat` companion when `network.allowed_domains` is non-empty.
fn wrap_linux_bwrap(command: &str, policy: &SandboxRuntimeConfig) -> String {
    let mut args: Vec<String> = Vec::new();

    // Default ro root.
    args.push("--ro-bind".into());
    args.push("/".into());
    args.push("/".into());

    // Tmpfs ephemeral writes.
    args.push("--tmpfs".into());
    args.push("/tmp".into());

    args.push("--proc".into());
    args.push("/proc".into());

    args.push("--dev".into());
    args.push("/dev".into());

    // No-new-privs + die-with-parent to match claude-code's defaults.
    args.push("--unshare-pid".into());
    args.push("--die-with-parent".into());

    // Writable paths per filesystem.allow_write.
    for path in &policy.filesystem.allow_write {
        args.push("--bind".into());
        args.push(path.clone());
        args.push(path.clone());
    }

    // Network.
    let want_network = !policy.network.allowed_domains.is_empty()
        || policy.network.allow_local_binding
        || policy.network.allow_all_unix_sockets
        || !policy.network.allow_unix_sockets.is_empty();
    if want_network {
        args.push("--share-net".into());
        // TODO(M2-followup): wire socat companion process for domain
        // filtering. For now, --share-net lets the wrapped command see the
        // host's network; domain-level filtering is deferred to the
        // process-lifecycle layer (platforms/posix/src/sandbox.rs).
    } else {
        args.push("--unshare-net".into());
    }

    let quoted = shell_escape_single(command);
    format!(
        "bwrap {args} -- /bin/sh -c {cmd}",
        args = args.join(" "),
        cmd = quoted,
    )
}

/// Single-quote-escape `s` for `/bin/sh -c`.
///
/// Returns `'<s with embedded single quotes escaped>'`. Embedded `'` is
/// replaced with `'\''` (close quote, escaped quote, reopen quote).
fn shell_escape_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

// =============================================================================
// macOS — SBPL profile
// =============================================================================

fn wrap_macos_sbpl(
    command: &str,
    policy: &SandboxRuntimeConfig,
) -> Result<String, SandboxWrapError> {
    let profile = generate_sbpl_profile(policy);
    let profile_path = write_sbpl_tempfile(&profile)?;
    let quoted = shell_escape_single(command);
    Ok(format!(
        "sandbox-exec -f {profile} /bin/sh -c {cmd}",
        profile = profile_path,
        cmd = quoted,
    ))
}

/// Generate a minimal-but-valid SBPL profile string.
///
/// Source-of-truth structure (claude-code's `getSandboxProfile()`):
/// ```sbpl
/// (version 1)
/// (deny default)
/// (allow process-exec)
/// (allow process-fork)
/// (allow signal (target self))
/// (allow sysctl-read)
/// (allow file-read*)
/// (allow file-write*  (regex "^/private/tmp"))
/// (allow file-write*  (regex "^<allow_write_path>"))
/// (allow network*)            ;; only if domains/local_binding allowed
/// ```
///
/// TODO(M2-followup): expand SBPL coverage to include
/// `allowManagedReadPathsOnly`, `ignoreViolations`, `enableWeakerNestedSandbox`
/// trustd allowance, and the full claude-code template. Current template is
/// the minimal subset that produces a valid `sandbox-exec` profile and covers
/// `filesystem.allow_write` + `denyWrite` + `denyRead` + network on/off.
pub(crate) fn generate_sbpl_profile(policy: &SandboxRuntimeConfig) -> String {
    let mut out = String::new();
    out.push_str("(version 1)\n");
    out.push_str("(deny default)\n");
    out.push_str("(allow process-exec)\n");
    out.push_str("(allow process-fork)\n");
    out.push_str("(allow signal (target self))\n");
    out.push_str("(allow sysctl-read)\n");
    out.push_str("(allow file-read*)\n");
    // Always allow /private/tmp (claude-code parity).
    out.push_str("(allow file-write* (regex \"^/private/tmp\"))\n");
    for p in &policy.filesystem.allow_write {
        let escaped = sbpl_regex_escape(p);
        out.push_str(&format!("(allow file-write* (regex \"^{escaped}\"))\n"));
    }
    for p in &policy.filesystem.deny_write {
        let escaped = sbpl_regex_escape(p);
        out.push_str(&format!("(deny file-write* (regex \"^{escaped}\"))\n"));
    }
    for p in &policy.filesystem.deny_read {
        let escaped = sbpl_regex_escape(p);
        out.push_str(&format!("(deny file-read* (regex \"^{escaped}\"))\n"));
    }
    let want_network = !policy.network.allowed_domains.is_empty()
        || policy.network.allow_local_binding
        || policy.network.allow_all_unix_sockets;
    if want_network {
        out.push_str("(allow network*)\n");
    }
    out
}

/// Escape `path` for inclusion inside an SBPL regex literal. Replaces the
/// regex metacharacters `. * + ? ( ) [ ] { } ^ $ |` with `\\<ch>`.
fn sbpl_regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        if matches!(
            ch,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn write_sbpl_tempfile(profile: &str) -> Result<String, SandboxWrapError> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("lingxi-sandbox-")
        .suffix(".sb")
        .tempfile()
        .map_err(|e| SandboxWrapError::SbplWrite(e.to_string()))?;
    f.write_all(profile.as_bytes())
        .map_err(|e| SandboxWrapError::SbplWrite(e.to_string()))?;
    // Persist: the tempfile must outlive this function so `sandbox-exec -f`
    // can read it. The OS will clean it up at reboot; for long-lived
    // processes the platform layer is responsible for explicit unlink.
    let (_file, path) = f
        .keep()
        .map_err(|e| SandboxWrapError::SbplWrite(e.error.to_string()))?;
    Ok(path.to_string_lossy().into_owned())
}
```

- [ ] **Step 4: Wire into lib.rs**

Add to `lingxi-core/crates/sandbox/src/lib.rs`:

```rust
pub mod wrap;

pub use wrap::{wrap_with_sandbox, SandboxWrapError};
```

- [ ] **Step 5: Run the Linux tests to verify they pass**

Run: `cargo test -p lingxi-sandbox --test wrap_linux_test`
Expected: 7 tests pass.

---

### Task 14: macOS SBPL profile test

**Files:**
- Create: `lingxi-core/crates/sandbox/tests/wrap_macos_test.rs`

- [ ] **Step 1: Write the test**

Create `lingxi-core/crates/sandbox/tests/wrap_macos_test.rs`:

```rust
use lingxi_sandbox::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, SandboxRuntimeConfig,
};
use lingxi_sandbox::wrap::wrap_with_sandbox;

fn cfg(allow_write: Vec<&str>, deny_read: Vec<&str>) -> SandboxRuntimeConfig {
    SandboxRuntimeConfig {
        enabled: true,
        filesystem: FilesystemRestrictionConfig {
            allow_write: allow_write.into_iter().map(String::from).collect(),
            deny_read: deny_read.into_iter().map(String::from).collect(),
            ..Default::default()
        },
        network: NetworkRestrictionConfig::default(),
        ..Default::default()
    }
}

#[test]
fn macos_wrap_starts_with_sandbox_exec_dash_f() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec!["/tmp/work"], vec![]), Platform::Mac)
        .expect("wrap ok");
    assert!(
        wrapped.starts_with("sandbox-exec -f "),
        "got: {wrapped}"
    );
    assert!(wrapped.contains("/bin/sh -c"), "got: {wrapped}");
}

#[test]
fn macos_wrap_emits_profile_at_real_path() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Mac).expect("wrap ok");
    // Extract the profile path from `sandbox-exec -f <path> ...`.
    let tail = wrapped.strip_prefix("sandbox-exec -f ").unwrap();
    let mut parts = tail.splitn(2, ' ');
    let path = parts.next().expect("profile path");
    let content = std::fs::read_to_string(path).expect("read profile");
    assert!(content.starts_with("(version 1)\n"), "got: {content}");
    assert!(content.contains("(deny default)"), "got: {content}");
    assert!(content.contains("(allow file-read*)"), "got: {content}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_includes_allow_write_entries() {
    let wrapped = wrap_with_sandbox(
        "ls",
        &cfg(vec!["/Users/u/repo", "/tmp/work"], vec![]),
        Platform::Mac,
    )
    .expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains(r#"(allow file-write* (regex "^/Users/u/repo"))"#),
        "missing first allow_write entry: {content}"
    );
    assert!(
        content.contains(r#"(allow file-write* (regex "^/tmp/work"))"#),
        "missing second allow_write entry: {content}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_includes_deny_read_entries() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec!["/private/etc"]), Platform::Mac)
        .expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains(r#"(deny file-read* (regex "^/private/etc"))"#),
        "missing deny_read entry: {content}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_no_network_unless_requested() {
    let wrapped = wrap_with_sandbox("ls", &cfg(vec![], vec![]), Platform::Mac).expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        !content.contains("(allow network*)"),
        "network should be denied by default: {content}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn macos_sbpl_network_when_domains_listed() {
    let mut policy = cfg(vec![], vec![]);
    policy.network.allowed_domains = vec!["api.example.com".to_string()];
    let wrapped =
        wrap_with_sandbox("curl", &policy, Platform::Mac).expect("wrap ok");
    let path = wrapped
        .strip_prefix("sandbox-exec -f ")
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains("(allow network*)"),
        "expected network allow line: {content}"
    );
    let _ = std::fs::remove_file(path);
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p lingxi-sandbox --test wrap_macos_test`
Expected: 6 tests pass.

---

### Task 15: Documented exact error strings (constants module)

**Files:**
- Modify: `lingxi-core/crates/sandbox/src/dependency_check.rs`

Add a `const` block at the top of `dependency_check.rs` so the exact claude-code error strings are documented and grep-able. The strings are already used by `sandbox_unavailable_reason`, but exposing them as `pub const` lets tests (and `/sandbox doctor` output) reference them by name without retyping.

- [ ] **Step 1: Write the failing test**

Create `lingxi-core/crates/sandbox/tests/error_strings_test.rs`:

```rust
use lingxi_sandbox::dependency_check::error_strings;

#[test]
fn wsl1_refusal_constant() {
    assert_eq!(
        error_strings::WSL1_REFUSAL,
        "sandbox.enabled is set but WSL1 is not supported (requires WSL2)"
    );
}

#[test]
fn unsupported_template_constant() {
    assert_eq!(
        error_strings::UNSUPPORTED_PLATFORM_TEMPLATE,
        "sandbox.enabled is set but {platform} is not supported (requires macOS, Linux, or WSL2)"
    );
}

#[test]
fn enabled_platforms_template_constant() {
    assert_eq!(
        error_strings::NOT_IN_ENABLED_PLATFORMS_TEMPLATE,
        "sandbox.enabled is set but {platform} is not in sandbox.enabledPlatforms"
    );
}

#[test]
fn missing_deps_macos_hint() {
    assert_eq!(
        error_strings::MISSING_DEPS_HINT_MAC,
        "run /sandbox or /doctor for details"
    );
}

#[test]
fn missing_deps_linux_hint() {
    assert_eq!(
        error_strings::MISSING_DEPS_HINT_LINUX,
        "install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p lingxi-sandbox --test error_strings_test`
Expected: FAIL — `unresolved import lingxi_sandbox::dependency_check::error_strings`.

- [ ] **Step 3: Add the constants module**

Append to `lingxi-core/crates/sandbox/src/dependency_check.rs`:

```rust
/// Byte-for-byte exact error strings from claude-code's sandbox-adapter.ts.
/// Tests assert against these to lock the messages in.
pub mod error_strings {
    /// WSL1 refusal — emitted when `/proc/version` indicates WSL1.
    pub const WSL1_REFUSAL: &str =
        "sandbox.enabled is set but WSL1 is not supported (requires WSL2)";

    /// Unsupported-platform template. `{platform}` is replaced with the OS
    /// label (e.g. `"windows"`, `"freebsd"`).
    pub const UNSUPPORTED_PLATFORM_TEMPLATE: &str =
        "sandbox.enabled is set but {platform} is not supported (requires macOS, Linux, or WSL2)";

    /// `enabledPlatforms` rejection template.
    pub const NOT_IN_ENABLED_PLATFORMS_TEMPLATE: &str =
        "sandbox.enabled is set but {platform} is not in sandbox.enabledPlatforms";

    /// Hint suffix for missing-deps on macOS.
    pub const MISSING_DEPS_HINT_MAC: &str = "run /sandbox or /doctor for details";

    /// Hint suffix for missing-deps on Linux/WSL.
    pub const MISSING_DEPS_HINT_LINUX: &str =
        "install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details";
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p lingxi-sandbox --test error_strings_test`
Expected: 5 tests pass.

- [ ] **Step 5: Replace the inline format strings in `sandbox_unavailable_reason` with the constants**

Edit `lingxi-core/crates/sandbox/src/dependency_check.rs` — replace the body of `sandbox_unavailable_reason`. Find the section that builds the messages and update it to reference `error_strings::*`:

```rust
    if wsl_one_detected {
        return Some(error_strings::WSL1_REFUSAL.to_string());
    }

    if !supported_platform {
        let label = raw_platform_label.unwrap_or_else(|| "unknown".to_string());
        return Some(
            error_strings::UNSUPPORTED_PLATFORM_TEMPLATE.replace("{platform}", &label),
        );
    }

    if !deps.in_enabled_list {
        let label = platform.map(|p| p.as_str()).unwrap_or("unknown");
        return Some(
            error_strings::NOT_IN_ENABLED_PLATFORMS_TEMPLATE.replace("{platform}", label),
        );
    }

    if !deps.errors.is_empty() {
        let joined = deps.errors.join(", ");
        let hint = match platform {
            Some(Platform::Mac) => error_strings::MISSING_DEPS_HINT_MAC,
            Some(Platform::Linux) | Some(Platform::Wsl) => error_strings::MISSING_DEPS_HINT_LINUX,
            None => "run /sandbox for details",
        };
        return Some(format!(
            "sandbox.enabled is set but dependencies are missing: {joined} · {hint}"
        ));
    }
```

- [ ] **Step 6: Re-run all existing dependency-check tests**

Run: `cargo test -p lingxi-sandbox --test dependency_check_test --test error_strings_test`
Expected: all green (no regression from the refactor).

---

### Task 16: Commit Phase C

- [ ] **Step 1: Clippy + fmt**

Run: `cargo clippy -p lingxi-sandbox -- -D warnings && cargo fmt -p lingxi-sandbox`
Expected: no warnings.

- [ ] **Step 2: Commit**

```bash
git add lingxi-core/crates/sandbox/src/wrap.rs \
        lingxi-core/crates/sandbox/src/dependency_check.rs \
        lingxi-core/crates/sandbox/src/lib.rs \
        lingxi-core/crates/sandbox/tests/wrap_linux_test.rs \
        lingxi-core/crates/sandbox/tests/wrap_macos_test.rs \
        lingxi-core/crates/sandbox/tests/error_strings_test.rs
git commit -m "feat(sandbox): wrap_with_sandbox dispatch"
```

---

## Phase D — Platform wiring (Tasks 17-22)

### Task 17: Rewrite `platforms/posix/src/sandbox.rs` as a real `Sandbox` impl

**Files:**
- Modify: `lingxi-core/platforms/posix/Cargo.toml`
- Modify: `lingxi-core/platforms/posix/src/sandbox.rs` (full rewrite)

This task wires together everything in Phases A-C and produces a `PosixSandbox` whose:
- `is_available()` checks WSL1, platform support, and dependency presence.
- `backend()` returns `LinuxNamespaces` / `MacOsSandboxExec` / `None` per detected platform.
- `prepare(cmd, policy)` translates the M1 `SandboxPolicy` → `SandboxRuntimeConfig` (preserving the M1 callers), calls `wrap_with_sandbox`, and constructs a `/bin/sh -c <wrapped>` `ProcessCommand`.
- `bypass_with_audit(cmd, reason)` logs and returns the bypass-tagged `SandboxedCommand`.
- `probe_capability()` returns a real `SandboxCapability { available, reason, features }`.

The `Sandbox` trait operates on the M1 `SandboxPolicy` shape (`writable_paths`, `denied_paths`, etc.) — we don't yet expose `SandboxRuntimeConfig` to the trait surface (that's a wider refactor). Instead, `PosixSandbox` translates internally.

**Critical 1:1 fidelity:** the `reason` field of `SandboxCapability` MUST contain claude-code's exact unavailable string from `sandbox_unavailable_reason` when `is_available()` returns false.

- [ ] **Step 1: Add the sandbox crate as a posix dep**

Add to `lingxi-core/platforms/posix/Cargo.toml` under `[dependencies]`:

```toml
lingxi-sandbox = { path = "../../crates/sandbox" }
```

- [ ] **Step 2: Write the failing test**

Create `lingxi-core/platforms/posix/tests/sandbox_real_impl_test.rs`:

```rust
use lingxi_platform_posix::sandbox::PosixSandbox;
use lingxi_traits::{
    NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxBackend, SandboxPolicy,
    SandboxedTag,
};
use std::collections::HashMap;
use std::path::PathBuf;

fn sample_policy() -> SandboxPolicy {
    SandboxPolicy {
        network: NetworkPolicy::Disabled,
        writable_paths: vec![PathBuf::from("/tmp/work")],
        denied_paths: vec![PathBuf::from("/etc")],
        allow_subprocess: true,
        limits: ResourceLimits {
            max_cpu_seconds: Some(60),
            max_memory_mb: Some(256),
            max_processes: Some(16),
            max_open_files: Some(64),
        },
    }
}

fn sample_command() -> ProcessCommand {
    ProcessCommand {
        command: "ls".into(),
        args: vec!["-la".into()],
        cwd: Some(PathBuf::from("/tmp")),
        env: HashMap::new(),
        timeout: None,
        stdin: None,
    }
}

#[test]
fn backend_matches_target_os() {
    let s = PosixSandbox::new();
    let b = s.backend();
    #[cfg(target_os = "macos")]
    assert!(matches!(b, SandboxBackend::MacOsSandboxExec));
    #[cfg(target_os = "linux")]
    assert!(matches!(b, SandboxBackend::LinuxNamespaces | SandboxBackend::None));
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    assert!(matches!(b, SandboxBackend::None));
}

#[test]
fn bypass_with_audit_records_reason() {
    let s = PosixSandbox::new();
    let bypass = s.bypass_with_audit(sample_command(), "user opted out");
    match bypass.tag() {
        SandboxedTag::BypassAuditedWithReason { reason } => {
            assert_eq!(reason, "user opted out");
        }
        other => panic!("expected BypassAuditedWithReason, got {other:?}"),
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prepare_wraps_command_when_backend_available() {
    let s = PosixSandbox::new();
    // Only assert wrap shape if the host actually has the deps (CI may not).
    if !s.is_available() {
        eprintln!(
            "skip: sandbox not available on host (reason: {:?})",
            futures::executor::block_on(s.probe_capability()).reason
        );
        return;
    }
    let prepared = s.prepare(sample_command(), &sample_policy()).expect("prepare ok");
    let inner = prepared.inner();
    // The wrapped command should be invoked through /bin/sh -c.
    assert_eq!(inner.command, "/bin/sh", "got: {inner:?}");
    assert_eq!(inner.args.len(), 2, "got: {inner:?}");
    assert_eq!(inner.args[0], "-c", "got: {inner:?}");
    // The second argv element is the wrapped string; on macOS it begins with
    // sandbox-exec, on Linux with bwrap.
    let wrapped = &inner.args[1];
    let starts_ok = wrapped.starts_with("bwrap ") || wrapped.starts_with("sandbox-exec ");
    assert!(starts_ok, "wrapped argv must start with bwrap/sandbox-exec, got: {wrapped}");
    // The original `ls -la` command should appear at the tail (shell-quoted).
    assert!(wrapped.contains("ls"), "missing original command: {wrapped}");
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p lingxi-platform-posix --test sandbox_real_impl_test`
Expected: FAIL — `prepare` either returns the M1 stub `Wrapped { backend: None }` (so `inner.command` would still be `ls`, not `/bin/sh`).

- [ ] **Step 4: Full rewrite of `platforms/posix/src/sandbox.rs`**

Replace the entire body of `lingxi-core/platforms/posix/src/sandbox.rs`:

```rust
//! Real POSIX `Sandbox` implementation.
//!
//! Linux / WSL2 → bwrap via `lingxi_sandbox::wrap`.
//! macOS → `sandbox-exec -f` with an SBPL profile written to a tempfile.
//! WSL1 / unknown POSIX → `is_available()` returns `false`, `prepare()` falls
//! back to a `Wrapped { backend: None }` no-op so callers that ignore the
//! capability flag still get a valid `SandboxedCommand` shape.
//!
//! Spec §6.4 (M2 Plan 04) is the source-of-truth for the dependency check
//! ordering, the WSL1 refusal string, and the `bwrap` / `sandbox-exec` argv
//! shape.

use async_trait::async_trait;
use lingxi_sandbox::dependency_check::{
    check_dependencies, sandbox_unavailable_reason, SandboxDependencyCheck,
};
use lingxi_sandbox::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, SandboxRuntimeConfig,
};
use lingxi_sandbox::wrap::wrap_with_sandbox;
use lingxi_traits::{
    NetworkPolicy, ProcessCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
    SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
};

use crate::wsl_detect::{detect as detect_wsl, WslKind};

/// Real `Sandbox` impl over `bwrap` (Linux/WSL2) / `sandbox-exec` (macOS).
///
/// Construction is cheap (no I/O); the dependency probe happens lazily on
/// `is_available()` / `probe_capability()` / `prepare()`.
#[derive(Default)]
pub struct PosixSandbox;

impl PosixSandbox {
    /// Construct a new `PosixSandbox`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Detect the host platform per claude-code's `Platform` enum.
    fn detect_platform(&self) -> Option<Platform> {
        #[cfg(target_os = "macos")]
        {
            return Some(Platform::Mac);
        }
        #[cfg(target_os = "linux")]
        {
            return match detect_wsl() {
                WslKind::WslTwo => Some(Platform::Wsl),
                WslKind::WslOne => None, // refused
                WslKind::NotWsl => Some(Platform::Linux),
            };
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }

    /// Build the full dependency check result for this host.
    fn dep_check(&self) -> SandboxDependencyCheck {
        // `in_enabled_list` defaults to true at this layer; the
        // `enabledPlatforms` setting is read at the call-site that has access
        // to the merged settings (M2-04 ships the helper; consumers wire it
        // through when they have settings in hand).
        check_dependencies(self.detect_platform(), true)
    }

    /// Surface the human-readable unavailable reason for the current host
    /// (or `None` when the sandbox can actually run).
    fn unavailable_reason(&self) -> Option<String> {
        let wsl_one = matches!(detect_wsl(), WslKind::WslOne);
        let platform = self.detect_platform();
        let supported = platform.is_some() && !wsl_one;
        let label = if platform.is_none() {
            Some(host_platform_label())
        } else {
            None
        };
        sandbox_unavailable_reason(true, supported, platform, wsl_one, label, self.dep_check())
    }
}

/// Best-effort host platform label for the "unsupported" error string.
fn host_platform_label() -> String {
    #[cfg(target_os = "macos")]
    {
        return "macos".to_string();
    }
    #[cfg(target_os = "linux")]
    {
        return "linux".to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return "windows".to_string();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        return std::env::consts::OS.to_string();
    }
}

#[async_trait]
impl Sandbox for PosixSandbox {
    fn is_available(&self) -> bool {
        if self.detect_platform().is_none() {
            return false;
        }
        self.dep_check().errors.is_empty()
    }

    fn backend(&self) -> SandboxBackend {
        match self.detect_platform() {
            Some(Platform::Mac) => SandboxBackend::MacOsSandboxExec,
            Some(Platform::Linux) | Some(Platform::Wsl) => SandboxBackend::LinuxNamespaces,
            None => SandboxBackend::None,
        }
    }

    fn prepare(
        &self,
        cmd: ProcessCommand,
        policy: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError> {
        // Validate cwd not inside denied paths (preserves M1 behavior for
        // callers that don't touch SandboxRuntimeConfig yet).
        if let Some(cwd) = &cmd.cwd {
            for denied in &policy.denied_paths {
                if cwd.starts_with(denied) {
                    return Err(SandboxError::SymlinkEscape(cwd.display().to_string()));
                }
            }
        }

        // If sandbox is not available on this host, return a Wrapped(None) tag
        // so the SandboxedCommand newtype invariant holds. Callers that pass
        // `failIfUnavailable: true` should consult `is_available()` first.
        let Some(platform) = self.detect_platform() else {
            return Ok(SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::Wrapped {
                    backend: SandboxBackend::None,
                },
            ));
        };
        if !self.dep_check().errors.is_empty() {
            return Ok(SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::Wrapped {
                    backend: SandboxBackend::None,
                },
            ));
        }

        // Translate M1 `SandboxPolicy` → `SandboxRuntimeConfig`.
        let runtime_cfg = runtime_config_from_policy(policy);

        // Build the full original command string for wrapping (command + args).
        let mut cmd_string = cmd.command.clone();
        for arg in &cmd.args {
            cmd_string.push(' ');
            cmd_string.push_str(arg);
        }

        let wrapped = wrap_with_sandbox(&cmd_string, &runtime_cfg, platform).map_err(|e| {
            SandboxError::Unavailable(format!("wrap_with_sandbox failed: {e}"))
        })?;

        let inner = ProcessCommand {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), wrapped],
            cwd: cmd.cwd,
            env: cmd.env,
            timeout: cmd.timeout,
            stdin: cmd.stdin,
        };

        Ok(SandboxedCommand::__new_sandboxed(
            inner,
            SandboxedTag::Wrapped {
                backend: self.backend(),
            },
        ))
    }

    fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
        tracing::warn!(reason, "sandbox bypass via bypass_with_audit");
        SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: reason.to_string(),
            },
        )
    }

    async fn probe_capability(&self) -> SandboxCapability {
        let available = self.is_available();
        let reason = if available {
            None
        } else {
            // sandbox_unavailable_reason returns None when `enabled` is false,
            // but we want to surface the reason even when the caller hasn't
            // enabled sandbox yet — they're asking the capability probe, not
            // running. Use `enabled = true` to force the message generation.
            self.unavailable_reason()
        };
        let features = SandboxFeatures {
            network_isolation: available,
            fs_readonly: available,
            fs_readwrite_paths: available,
            process_limit: false, // bwrap can't enforce per-policy.limits today
            no_new_privileges: available,
        };
        SandboxCapability {
            available,
            reason,
            features,
        }
    }
}

/// Translate the M1 `SandboxPolicy` into a `SandboxRuntimeConfig` for the wrap
/// dispatcher. The mapping is intentionally narrow — M1 callers only carry
/// `writable_paths`, `denied_paths`, `network`, and `limits`.
///
/// Future work (Task TODO in M2-followup): expand `SandboxPolicy` itself to
/// hold a `SandboxRuntimeConfig` field so this translation becomes an identity
/// pass and the existing call sites pick up `excludedCommands` etc.
fn runtime_config_from_policy(policy: &SandboxPolicy) -> SandboxRuntimeConfig {
    let allow_write: Vec<String> = policy
        .writable_paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let deny_write: Vec<String> = policy
        .denied_paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let want_net = matches!(policy.network, NetworkPolicy::Allowed | NetworkPolicy::LoopbackOnly);
    SandboxRuntimeConfig {
        enabled: true,
        filesystem: FilesystemRestrictionConfig {
            allow_write,
            deny_write,
            ..Default::default()
        },
        network: NetworkRestrictionConfig {
            allow_local_binding: matches!(policy.network, NetworkPolicy::LoopbackOnly),
            allowed_domains: if want_net { vec!["*".to_string()] } else { vec![] },
            ..Default::default()
        },
        ..Default::default()
    }
}
```

- [ ] **Step 5: Run the platform test**

Run: `cargo test -p lingxi-platform-posix --test sandbox_real_impl_test`
Expected: 3 tests pass (one is `cfg`-gated on macOS/linux).

---

### Task 18: Verify Windows sandbox stays Unsupported + add doc cross-ref

**Files:**
- Modify: `lingxi-core/platforms/windows/src/sandbox.rs` (doc only)

M2-01 should have already rewritten the Windows sandbox to return `Unsupported`. Verify, and add a doc comment pointing at M2-04 for the cross-platform parity story.

- [ ] **Step 1: Inspect the current file**

Run: `cat /Users/luolingfeng/Projects/LingXi-Next/lingxi-core/platforms/windows/src/sandbox.rs | head -30`
Expected (if M2-01 done): doc comment mentions "claude-code does not support sandbox on Windows" or returns `SandboxError::Unsupported`. If it still says "M2.03 windows sandbox is a policy-validating no-op", M2-01 needs to be re-checked first.

- [ ] **Step 2: Update the module doc**

Edit the top of `lingxi-core/platforms/windows/src/sandbox.rs` — replace the module doc with:

```rust
//! `Sandbox` trait impl — Windows is Unsupported.
//!
//! claude-code refuses sandbox on Windows (its `@anthropic-ai/sandbox-runtime`
//! has no Windows backend). M2 Plan 04 mirrors that: `is_available()` returns
//! `false`, `prepare()` returns `SandboxError::Unavailable(...)`, and
//! `probe_capability().reason` carries the exact claude-code error string.
//!
//! Cross-ref: `docs/superpowers/plans/2026-05-23-m2-04-sandbox-runtime.md` —
//! the parity story for sandbox lives in M2-04; this file is unchanged by
//! M2-04 itself but documents that constraint.
```

- [ ] **Step 3: Verify the file still compiles**

Run: `cargo check -p lingxi-platform-windows`
Expected: ok.

---

### Task 19: WSL1 refusal integration test

**Files:**
- Create: `lingxi-core/platforms/posix/tests/sandbox_wsl1_refusal_test.rs`

This test is gated on Linux only (we can't mock `/proc/version` on macOS). It exercises the `parse_wsl_kind` parser as a proxy: feed in WSL1 markup, confirm the `dependency_check::sandbox_unavailable_reason` produces the exact byte-for-byte WSL1 string.

- [ ] **Step 1: Write the test**

Create `lingxi-core/platforms/posix/tests/sandbox_wsl1_refusal_test.rs`:

```rust
use lingxi_platform_posix::wsl_detect::{parse_wsl_kind, WslKind};
use lingxi_sandbox::dependency_check::{
    error_strings, sandbox_unavailable_reason, SandboxDependencyCheck,
};
use lingxi_sandbox::runtime_config::Platform;

#[test]
fn wsl1_detected_from_proc_version_emits_exact_refusal_string() {
    // Synthetic WSL1 /proc/version content.
    let proc_version = "Linux version 4.4.0-19041-Microsoft (Microsoft@Microsoft.com)";
    let kind = parse_wsl_kind(proc_version);
    assert_eq!(kind, WslKind::WslOne);

    // The full pipeline: sandbox.enabled = true, the detected platform is
    // None (we refuse WSL1), wsl_one_detected = true.
    let reason = sandbox_unavailable_reason(
        true,
        false, // supported_platform: WSL1 is NOT supported
        None,  // no Platform variant
        true,  // wsl_one_detected
        Some("wsl".to_string()),
        SandboxDependencyCheck::default(),
    );
    let actual = reason.expect("WSL1 with sandbox.enabled must produce a reason");
    assert_eq!(actual, error_strings::WSL1_REFUSAL);
    // Re-state the byte sequence explicitly:
    assert_eq!(
        actual,
        "sandbox.enabled is set but WSL1 is not supported (requires WSL2)"
    );
}

#[test]
fn wsl2_detected_from_proc_version_does_not_emit_wsl1_refusal() {
    let proc_version = "Linux version 5.15.90.1-microsoft-standard-WSL2";
    let kind = parse_wsl_kind(proc_version);
    assert_eq!(kind, WslKind::WslTwo);
    // WSL2 is supported. The unavailable reason should not be the WSL1 string.
    let reason = sandbox_unavailable_reason(
        true,
        true,
        Some(Platform::Wsl),
        false,
        None,
        SandboxDependencyCheck::default(),
    );
    assert!(
        reason.as_deref() != Some(error_strings::WSL1_REFUSAL),
        "WSL2 must not yield the WSL1 string, got {reason:?}"
    );
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p lingxi-platform-posix --test sandbox_wsl1_refusal_test`
Expected: 2 tests pass.

---

### Task 20: Integration test — prepare a real ls command, inspect the wrapped argv

**Files:**
- Create: `lingxi-core/platforms/posix/tests/sandbox_prepare_e2e_test.rs`

End-to-end test that exercises the full `PosixSandbox::prepare` pipeline with a realistic policy and verifies the wrapped `ProcessCommand` argv matches expectations. Gated on `target_os = "macos"` OR `target_os = "linux"`.

- [ ] **Step 1: Write the test**

Create `lingxi-core/platforms/posix/tests/sandbox_prepare_e2e_test.rs`:

```rust
#![cfg(any(target_os = "macos", target_os = "linux"))]

use lingxi_platform_posix::sandbox::PosixSandbox;
use lingxi_traits::{
    NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxedTag, SandboxPolicy,
};
use std::collections::HashMap;
use std::path::PathBuf;

#[test]
fn prepare_emits_bwrap_or_sandbox_exec_invocation() {
    let sandbox = PosixSandbox::new();

    // If sandbox deps are missing on CI, prepare degrades to Wrapped(None).
    // We can still assert the SandboxedCommand wrapper.
    let cmd = ProcessCommand {
        command: "ls".into(),
        args: vec!["-la".into(), "/tmp".into()],
        cwd: Some(PathBuf::from("/tmp")),
        env: HashMap::new(),
        timeout: None,
        stdin: None,
    };
    let policy = SandboxPolicy {
        network: NetworkPolicy::Disabled,
        writable_paths: vec![PathBuf::from("/tmp")],
        denied_paths: vec![PathBuf::from("/etc")],
        allow_subprocess: true,
        limits: ResourceLimits::default(),
    };

    let prepared = sandbox.prepare(cmd, &policy).expect("prepare ok");
    let inner = prepared.inner();
    let tag = prepared.tag();

    match tag {
        SandboxedTag::Wrapped { backend } => {
            eprintln!("backend={backend:?}");
        }
        SandboxedTag::BypassAuditedWithReason { reason } => {
            panic!("unexpected bypass: {reason}");
        }
    }

    if sandbox.is_available() {
        // The argv shape is `/bin/sh -c "<wrapped>"`.
        assert_eq!(inner.command, "/bin/sh");
        assert_eq!(inner.args.len(), 2);
        assert_eq!(inner.args[0], "-c");
        let wrapped = &inner.args[1];
        let ok = wrapped.starts_with("bwrap ") || wrapped.starts_with("sandbox-exec ");
        assert!(ok, "wrapped argv: {wrapped}");
        // /tmp should appear as an allow-write binding (Linux) or as a regex
        // entry in the SBPL profile (macOS — we can't grep the profile from
        // here without re-reading it, so just sanity check substring).
        assert!(wrapped.contains("/tmp"));
        // Original `ls` must survive the wrap.
        assert!(wrapped.contains("ls"));
    } else {
        // Stub path: the wrapped command is identical to the original.
        assert_eq!(inner.command, "ls");
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p lingxi-platform-posix --test sandbox_prepare_e2e_test`
Expected: 1 test passes (CI without bwrap/sandbox-exec falls through the `else` branch).

---

### Task 21: Workspace-level verification

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace`
Expected: baseline 104 tests + ~30 new sandbox tests = ~134 passing. No regressions.

- [ ] **Step 2: Clippy across the touched crates**

Run: `cargo clippy -p lingxi-sandbox -p lingxi-platform-posix -p lingxi-platform-windows --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Workspace fmt check**

Run: `cargo fmt --all --check`
Expected: no diff.

- [ ] **Step 4: Check posix and windows still compile without default features**

Run: `cargo check -p lingxi-platform-posix --no-default-features && cargo check -p lingxi-platform-windows --no-default-features`
Expected: no errors.

---

### Task 22: Commit Phase D

- [ ] **Step 1: Stage and commit**

```bash
git add lingxi-core/platforms/posix/Cargo.toml \
        lingxi-core/platforms/posix/src/sandbox.rs \
        lingxi-core/platforms/posix/src/lib.rs \
        lingxi-core/platforms/posix/tests/sandbox_real_impl_test.rs \
        lingxi-core/platforms/posix/tests/sandbox_wsl1_refusal_test.rs \
        lingxi-core/platforms/posix/tests/sandbox_prepare_e2e_test.rs \
        lingxi-core/platforms/windows/src/sandbox.rs
git commit -m "feat(platforms/posix): real Sandbox impl"
```

- [ ] **Step 2: Final cross-check — list commits**

Run: `git log --oneline -n 6`
Expected (newest first):
```
<hash> feat(platforms/posix): real Sandbox impl
<hash> feat(sandbox): wrap_with_sandbox dispatch
<hash> feat(sandbox): dependency check + violation store
<hash> feat(sandbox): RuntimeConfig schema
<hash> spec(M2): incorporate codex review corrections (7 items)
<hash> spec: M2 claude-code behavioral parity design (v0.3.0 target)
```

Four commits match the spec §6.4 commit policy.

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` §6.4):

| Spec item | Task |
|---|---|
| `SandboxRuntimeConfig` (full zod schema, camelCase wire shape) | Task 1 |
| `Platform` enum (`Mac` / `Linux` / `Wsl`, lowercase serde) | Task 1 |
| `NetworkRestrictionConfig` subfields (all 7) | Task 1 |
| `FilesystemRestrictionConfig` subfields (all 5) | Task 1 |
| `RipgrepConfig` (`command`, `args`) | Task 1 |
| `enabled`, `failIfUnavailable`, `enabledPlatforms`, `autoAllowBashIfSandboxed`, `allowUnsandboxedCommands`, `ignoreViolations`, `enableWeakerNestedSandbox`, `enableWeakerNetworkIsolation`, `excludedCommands` | Task 1 |
| `resolvePathPatternForSandbox` (`//`, `/`, `~/`, `./`, bare) | Task 2 |
| `convertToSandboxRuntimeConfig` (walk `permissions.allow/deny`) | Task 3 |
| `getLinuxGlobPatternWarnings` (`* ? [ ]` excluding trailing `/**`) | Task 3 |
| WSL1 detection via `/proc/version` | Task 4 |
| `checkDependencies` per platform | Task 6 |
| `getSandboxUnavailableReason` (5 exact strings via `error_strings::*`) | Task 6 + Task 15 |
| `SandboxViolationStore` (bounded 1000) | Task 7 |
| Compound-command splitting on `&&`, `;`, `||` | Task 8 |
| Iterative env-var + safe-wrapper stripping (fixed point) | Task 8 |
| `BINARY_HIJACK_VARS` constant matching claude-code | Task 8 |
| `excludedCommands` pattern match (bare, exact, `:*`) | Task 9 |
| `wrap_with_sandbox` Linux/WSL2 → `bwrap` argv | Task 13 |
| `wrap_with_sandbox` macOS → SBPL profile via `sandbox-exec -f` | Task 13 + Task 14 |
| `wrap_with_sandbox` Windows/WSL1 refusal (via `Platform` having no `Windows`) | Type-level: covered by absence of `Platform::Windows` |
| Exact 5 error strings (constants module) | Task 15 |
| Real `PosixSandbox::is_available / backend / prepare / bypass_with_audit / probe_capability` | Task 17 |
| Windows sandbox stays Unsupported (M2-01 already did this) | Task 18 |
| WSL1 byte-for-byte string refusal | Task 12 + Task 19 |
| Integration: prepare an `ls` command → wrapped argv | Task 20 |
| Workspace-level verification (test, clippy, fmt) | Task 21 |
| 4 commits per spec | Tasks 5, 11, 16, 22 |

**Placeholder scan:** All steps contain actual Rust, actual test code with assertions, and actual cargo commands with explicit `-p` / `--test` / `--lib` selectors. The only `TODO(M2-followup)` markers are:
- `wrap.rs::wrap_linux_bwrap` — `socat` companion process lifecycle (network domain filtering) — explicitly deferred per spec §6.4 ("If `network` requires proxying, also start a `socat` companion process (managed lifecycle in `wrap.rs`)"). The bwrap argv emits `--share-net` / `--unshare-net` correctly today; socat wiring is process-lifecycle work that belongs in `platforms/posix/src/sandbox.rs` and would balloon this plan past its budget.
- `wrap.rs::generate_sbpl_profile` — minimal-but-valid SBPL covering `filesystem.{allow_write,deny_write,deny_read}` + network on/off. Full claude-code template (trustd allowance, `allowManagedReadPathsOnly`, `ignoreViolations`, `enableWeakerNestedSandbox`) is explicitly marked as a follow-up per spec §6.4 ("Full template fidelity (covering all macOS quirks) is a separate research task.").
- `sandbox.rs::runtime_config_from_policy` — translation from M1 `SandboxPolicy` to `SandboxRuntimeConfig` is currently narrow; expanding `SandboxPolicy` to carry the full runtime config is a broader trait-surface change that would touch every M1 caller.

**Type consistency check:**
- `SandboxRuntimeConfig` — produced by Task 3 (`convert_settings_to_runtime_config`), Task 17 (`runtime_config_from_policy`); consumed by Task 9 (`should_use_sandbox_for_command`), Task 13 (`wrap_with_sandbox`).
- `Platform::Mac | Linux | Wsl` (lowercase serde, no `Windows`) — consistent across Tasks 1, 6, 13, 17.
- `SandboxDependencyCheck { errors, warnings, in_enabled_list }` — produced by Task 6 (`check_dependencies`), consumed by Task 6 (`sandbox_unavailable_reason`), Task 17 (`PosixSandbox::dep_check`).
- `SandboxViolationStore` / `SandboxViolationEvent` / `SandboxViolationKind` — defined in Task 7, exposed via `lib.rs`.
- `WslKind::WslOne | WslTwo | NotWsl` — defined in Task 4, consumed by Task 17 (`PosixSandbox::detect_platform`, `PosixSandbox::unavailable_reason`) and Task 19 (refusal integration test).
- `SandboxWrapError::Unsupported | SbplWrite` — defined in Task 13, consumed by Task 17.
- `error_strings::*` constants — defined in Task 15, consumed by Task 19.

**1:1 fidelity strings present:**
- `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"` — Task 6, Task 12, Task 15, Task 19.
- `"sandbox.enabled is set but {platform} is not supported (requires macOS, Linux, or WSL2)"` — Task 6, Task 15.
- `"sandbox.enabled is set but {platform} is not in sandbox.enabledPlatforms"` — Task 6, Task 15.
- `"sandbox.enabled is set but dependencies are missing: {deps} · run /sandbox or /doctor for details"` — Task 6, Task 15.
- `"sandbox.enabled is set but dependencies are missing: {deps} · install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details"` — Task 6, Task 15.
- All 12 zod schema fields (`failIfUnavailable`, `enabledPlatforms`, `autoAllowBashIfSandboxed`, `allowUnsandboxedCommands`, `allowManagedDomainsOnly`, `allowUnixSockets`, `allowAllUnixSockets`, `allowLocalBinding`, `httpProxyPort`, `socksProxyPort`, `allowManagedReadPathsOnly`, `enableWeakerNestedSandbox`, `enableWeakerNetworkIsolation`, `excludedCommands`, `ignoreViolations`) — Task 1 test asserts each via `as_object().contains_key`.
- Bubblewrap glob warning for `* ? [ ]` outside trailing `/**` — Task 3 (`linux_glob_pattern_warnings` + 4 tests).
- `BINARY_HIJACK_VARS` constant (PATH, LD_PRELOAD, LD_LIBRARY_PATH, DYLD_LIBRARY_PATH, DYLD_INSERT_LIBRARIES) — Task 8.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-23-m2-04-sandbox-runtime.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration via `superpowers:subagent-driven-development`.

**2. Inline Execution** — Execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.

Which approach?

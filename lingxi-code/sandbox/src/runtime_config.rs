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
    /// macOS host (Darwin) — sandbox via `sandbox-exec`.
    Mac,
    /// Native Linux host — sandbox via `bwrap`.
    Linux,
    /// WSL2 host — same `bwrap` path as native Linux, distinguished for
    /// claude-code parity (WSL1 is refused before sandbox dispatch).
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
#[allow(clippy::struct_excessive_bools)] // wire-shape mirror; flags are independent and 1:1 with zod schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRestrictionConfig {
    /// Domains the sandboxed process may reach outbound (egress allowlist).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// When `true`, only `allowed_domains` from managed (enterprise) settings
    /// are honored; user-level rules are ignored. Mirrors zod
    /// `allowManagedDomainsOnly`.
    #[serde(default)]
    pub allow_managed_domains_only: bool,
    /// Unix-socket paths the sandboxed process is permitted to connect to
    /// (e.g. `/var/run/docker.sock`).
    #[serde(default)]
    pub allow_unix_sockets: Vec<String>,
    /// When `true`, all unix sockets pass through (overrides
    /// `allow_unix_sockets`).
    #[serde(default)]
    pub allow_all_unix_sockets: bool,
    /// When `true`, the sandbox may bind to localhost (servers can listen
    /// on `127.0.0.1`).
    #[serde(default)]
    pub allow_local_binding: bool,
    /// Optional HTTP proxy port the sandbox-runtime listens on for HTTP
    /// CONNECT-style egress.
    #[serde(default)]
    pub http_proxy_port: Option<u16>,
    /// Optional SOCKS proxy port the sandbox-runtime listens on for
    /// SOCKS5 egress.
    #[serde(default)]
    pub socks_proxy_port: Option<u16>,
}

/// Filesystem restriction subsection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemRestrictionConfig {
    /// Paths the sandboxed process may write to (mounted read-write).
    #[serde(default)]
    pub allow_write: Vec<String>,
    /// Paths explicitly denied for writes even if otherwise within the
    /// writable tree (overrides `allow_write`).
    #[serde(default)]
    pub deny_write: Vec<String>,
    /// Paths explicitly denied for reads (overrides `allow_read`).
    #[serde(default)]
    pub deny_read: Vec<String>,
    /// Paths the sandboxed process may read (mounted read-only).
    #[serde(default)]
    pub allow_read: Vec<String>,
    /// When `true`, only `allow_read` paths from managed (enterprise)
    /// settings are honored. Mirrors zod `allowManagedReadPathsOnly`.
    #[serde(default)]
    pub allow_managed_read_paths_only: bool,
}

/// Ripgrep override block. Bundled ripgrep path + extra args.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RipgrepConfig {
    /// Absolute path to the ripgrep executable. Empty means
    /// "use the bundled / `$PATH` default".
    #[serde(default)]
    pub command: String,
    /// Extra args appended after the user query (e.g. `--no-config`).
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
#[allow(clippy::struct_excessive_bools)] // wire-shape mirror; the bool count comes from claude-code's zod schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxRuntimeConfig {
    /// Master toggle. When `false`, the sandbox runtime is bypassed.
    #[serde(default)]
    pub enabled: bool,
    /// When `true`, sandbox unavailability is fatal (errors propagate to the
    /// caller). When `false`, the runtime falls back to no-sandbox execution.
    #[serde(default)]
    pub fail_if_unavailable: bool,
    /// Optional restriction of the platforms on which the sandbox runs. `None`
    /// means "run on every supported platform".
    #[serde(default)]
    pub enabled_platforms: Option<Vec<Platform>>,
    /// When `true`, sandboxed bash invocations auto-approve permission
    /// prompts (the sandbox is the safety boundary, not the prompt).
    #[serde(default)]
    pub auto_allow_bash_if_sandboxed: bool,
    /// Commands the sandbox refuses to wrap (i.e., these always run outside
    /// the sandbox even when `enabled = true`).
    #[serde(default)]
    pub allow_unsandboxed_commands: Vec<String>,
    /// Network egress configuration.
    #[serde(default)]
    pub network: NetworkRestrictionConfig,
    /// Filesystem allow/deny configuration.
    #[serde(default)]
    pub filesystem: FilesystemRestrictionConfig,
    /// Map of violation-category → patterns whose sandbox violations are
    /// silently ignored (mostly noisy filesystem reads from build tools).
    #[serde(default)]
    pub ignore_violations: HashMap<String, Vec<String>>,
    /// When `true`, allow a sandbox-inside-sandbox configuration with weaker
    /// guarantees on the inner layer (claude-code feature flag).
    #[serde(default)]
    pub enable_weaker_nested_sandbox: bool,
    /// When `true`, relax network isolation (used for certain proxy stacks).
    #[serde(default)]
    pub enable_weaker_network_isolation: bool,
    /// Commands that bypass the sandbox decision (e.g. `bazel`, `make`).
    #[serde(default)]
    pub excluded_commands: Vec<String>,
    /// Ripgrep override.
    #[serde(default)]
    pub ripgrep: RipgrepConfig,
    /// Host paths to mount read-only IN PLACE (`--ro-bind <p> <p>`), overriding
    /// any writable parent. Populated by the posix `prepare` layer from the
    /// EXISTING subset of denied + bare-repo paths (FS access lives there, not
    /// in the pure wrapper). claude-code denyWrite semantics
    /// (sandbox-adapter.ts:264). `#[serde(skip)]` keeps the TS-mirror wire shape
    /// byte-identical — this field is a host-side build artifact, not a managed
    /// setting.
    #[serde(skip)]
    pub ro_bind_in_place: Vec<String>,
    /// Host paths to delete AFTER the command (non-existent-at-config-time
    /// bare-repo files planted during the run). See finding 4 / Task 5.
    /// `#[serde(skip)]` for the same reason as [`Self::ro_bind_in_place`].
    #[serde(skip)]
    pub scrub_paths: Vec<String>,
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
    /// `permissions.allow/deny` slice — drives filesystem and webfetch
    /// extraction.
    #[serde(default)]
    pub permissions: Option<SettingsPermissions>,
    /// User-supplied `sandbox` subsection (overrides extracted defaults).
    #[serde(default)]
    pub sandbox: Option<SandboxSettingsJson>,
    /// Directory the settings file lives in. Required because claude-code's
    /// path patterns (`/path`) are resolved relative to the settings file
    /// directory. Defaults to the cwd if the caller doesn't know it; sandbox
    /// path resolution then degrades to "current directory" semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_dir: Option<std::path::PathBuf>,
}

/// `permissions` subsection of `SettingsJson`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPermissions {
    /// Allow-list of `Tool(arg)` rules.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Deny-list of `Tool(arg)` rules.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Extra directories the caller wants mounted writable inside the
    /// sandbox (claude-code: `additionalDirectories`).
    #[serde(default)]
    pub additional_directories: Vec<String>,
}

/// User-supplied `sandbox` subsection of `SettingsJson`. All fields optional;
/// merged into [`SandboxRuntimeConfig`] by
/// [`crate::policy_convert::convert_settings_to_runtime_config`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSettingsJson {
    /// Override `SandboxRuntimeConfig::enabled`.
    pub enabled: Option<bool>,
    /// Override `SandboxRuntimeConfig::fail_if_unavailable`.
    pub fail_if_unavailable: Option<bool>,
    /// Override `SandboxRuntimeConfig::enabled_platforms`.
    pub enabled_platforms: Option<Vec<Platform>>,
    /// Override `SandboxRuntimeConfig::auto_allow_bash_if_sandboxed`.
    pub auto_allow_bash_if_sandboxed: Option<bool>,
    /// Override `SandboxRuntimeConfig::allow_unsandboxed_commands`.
    pub allow_unsandboxed_commands: Option<Vec<String>>,
    /// Override `SandboxRuntimeConfig::network` (merged with extracted
    /// `allowed_domains`).
    pub network: Option<NetworkRestrictionConfig>,
    /// Override `SandboxRuntimeConfig::filesystem` (merged with extracted
    /// allow/deny paths).
    pub filesystem: Option<FilesystemRestrictionConfig>,
    /// Override `SandboxRuntimeConfig::ignore_violations`.
    pub ignore_violations: Option<HashMap<String, Vec<String>>>,
    /// Override `SandboxRuntimeConfig::enable_weaker_nested_sandbox`.
    pub enable_weaker_nested_sandbox: Option<bool>,
    /// Override `SandboxRuntimeConfig::enable_weaker_network_isolation`.
    pub enable_weaker_network_isolation: Option<bool>,
    /// Override `SandboxRuntimeConfig::excluded_commands`.
    pub excluded_commands: Option<Vec<String>>,
    /// Override `SandboxRuntimeConfig::ripgrep`.
    pub ripgrep: Option<RipgrepConfig>,
}

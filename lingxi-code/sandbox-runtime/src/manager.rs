//! The `SandboxManager` orchestrator — a faithful port of `sandbox-manager.js`
//! (the lifecycle/integration capstone). It ties together config validation
//! ([`crate::config`]), the TLS-termination CA ([`crate::mitm_ca`]), the host
//! forward proxies ([`crate::http_proxy`] and [`crate::socks_proxy`]), the
//! Linux `socat` bridge plus the bwrap wrap ([`crate::linux`]), the
//! ask-callback filter ([`crate::matcher`]), and the violation store
//! ([`crate::violation_store`]).
//!
//! # Owned struct, not a singleton (documented faithful refactor)
//!
//! The TS module keeps **module-level mutable state** (`let config`,
//! `httpProxyServer`, `managerContext`, `mitmCA`, a process-wide
//! `sandboxViolationStore`, plus `process.once('exit'/'SIGINT'/'SIGTERM')`
//! cleanup handlers). The Rust port collapses all of that into an **owned
//! [`SandboxManager`] struct** with NO global mutable state: each instance owns
//! its config, violation store, ask-callback, and the live [`RunningState`].
//! This is the idiomatic-Rust equivalent and removes the singleton's
//! re-entrancy hazards (the TS `initializationPromise` guard). The TS
//! process-signal cleanup handler is intentionally NOT ported — teardown is
//! [`SandboxManager::reset`] (called explicitly) plus `Drop` on the owned
//! [`crate::linux::LinuxBridge`]/[`crate::mitm_ca::MitmCa`]; a host that wants
//! signal-driven cleanup installs its own handler that calls `reset`.
//!
//! # Platform
//!
//! Only the Linux `wrap_with_sandbox` branch is wired (this is the Linux
//! capstone). macOS/Windows are documented P9 seams — see
//! [`SandboxManager::wrap_with_sandbox`]. Proxy/CA initialization and the
//! filter run on every platform; the `socat` bridge is Linux-only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::config::SandboxRuntimeConfig;
use crate::env::Platform;
use crate::fs_args::{ReadConfig, WriteConfig};
use crate::http_proxy::{serve, ProxyOptions};
use crate::linux::{
    check_linux_dependencies, cleanup_bwrap_mount_points, initialize_linux_network_bridge,
    wrap_command_with_sandbox_linux, LinuxBridge, LinuxDependencyCheck, LinuxDependencyOpts,
    WrapParams,
};
use crate::matcher::AskFn;
use crate::mitm_ca::{create_mitm_ca, dispose_mitm_ca, MitmCa, MitmCaOptions};
use crate::parent_proxy::resolve_parent_proxy;
use crate::path_utils::{
    contains_glob_chars, expand_glob_pattern, get_default_write_paths, remove_trailing_glob_suffix,
};
use crate::socks_proxy::{serve_socks, SocksOptions};
use crate::violation_store::SandboxViolationStore;

/// The host OS the manager is running on. Mirrors the TS `getPlatform()` (the
/// Rust [`Platform`] enum only carries the two variants the proxy-env generator
/// branches on, so the manager keeps its own three-way detection for the
/// wrap-branch dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostOs {
    Linux,
    Macos,
    Windows,
    Other,
}

/// Detect the current host OS via `cfg!` (compile-time; matches the TS
/// `process.platform` switch at runtime on the host this binary runs on).
fn host_os() -> HostOs {
    if cfg!(target_os = "linux") {
        HostOs::Linux
    } else if cfg!(target_os = "macos") {
        HostOs::Macos
    } else if cfg!(target_os = "windows") {
        HostOs::Windows
    } else {
        HostOs::Other
    }
}

/// Resolve the sandbox tmpdir (`CLAUDE_CODE_TMPDIR || CLAUDE_TMPDIR ||
/// /tmp/claude`), matching the TS env resolution baked into
/// `generateProxyEnvVars`.
fn resolve_tmpdir() -> String {
    std::env::var("CLAUDE_CODE_TMPDIR")
        .or_else(|_| std::env::var("CLAUDE_TMPDIR"))
        .unwrap_or_else(|_| "/tmp/claude".to_string())
}

/// The live network infrastructure produced by [`SandboxManager::initialize`]
/// (the TS `managerContext` + the proxy server handles + the loaded CA, gathered
/// into one owned value so [`SandboxManager::reset`] can tear it all down).
#[derive(Debug)]
struct RunningState {
    /// Host HTTP proxy port (local listener port, or the external port from
    /// `network.http_proxy_port`).
    http_port: u16,
    /// Host SOCKS proxy port (local listener port, or the external port from
    /// `network.socks_proxy_port`).
    socks_port: u16,
    /// The HTTP proxy `serve` accept-loop task. `None` when an external HTTP
    /// proxy is used (we never started a server). Aborted by `reset`.
    http_task: Option<JoinHandle<()>>,
    /// The SOCKS proxy `serve_socks` accept-loop task. `None` for an external
    /// SOCKS proxy. Aborted by `reset`.
    socks_task: Option<JoinHandle<()>>,
    /// The Linux `socat` bridge (Unix sockets ↔ host proxy ports). `None` off
    /// Linux. Its `Drop` SIGTERMs the bridge children.
    bridge: Option<LinuxBridge>,
    /// The TLS-termination CA, when `network.tls_terminate` is set.
    mitm_ca: Option<Arc<MitmCa>>,
    /// HTTP bridge Unix socket path (Linux only), threaded into the wrap.
    http_socket_path: Option<String>,
    /// SOCKS bridge Unix socket path (Linux only), threaded into the wrap.
    socks_socket_path: Option<String>,
}

/// Errors from [`SandboxManager`] operations.
#[derive(Debug)]
pub enum ManagerError {
    /// Config validation failed (`config.validate()`); carries the message.
    InvalidConfig(String),
    /// `network.tlsTerminate` and `network.mitmProxy` are both set (the TS
    /// hard error: they are mutually exclusive).
    TlsTerminateAndMitmProxy,
    /// CA creation failed (`create_mitm_ca`).
    MitmCa(String),
    /// A dependency required to run the sandbox is missing (the joined error
    /// strings from the platform dependency check).
    DependenciesMissing(String),
    /// An I/O failure (proxy bind, bridge spawn, or the bwrap wrap).
    Io(String),
    /// `wrap_with_sandbox` was called on an unsupported platform branch
    /// (macOS/Windows — the P9 seams) or before [`SandboxManager::initialize`].
    Unsupported(String),
}

impl std::fmt::Display for ManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(m) => write!(f, "invalid sandbox config: {m}"),
            Self::TlsTerminateAndMitmProxy => f.write_str(
                "network.tlsTerminate and network.mitmProxy are mutually exclusive",
            ),
            Self::MitmCa(m) => write!(f, "mitm CA: {m}"),
            Self::DependenciesMissing(m) => write!(f, "Sandbox dependencies not available: {m}"),
            Self::Io(m) => write!(f, "sandbox io error: {m}"),
            Self::Unsupported(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ManagerError {}

/// Orchestrates network + filesystem sandbox restrictions for a session. Runs
/// OUTSIDE the sandbox, on the host. Owns all state (no module-level globals —
/// see the module docs).
pub struct SandboxManager {
    /// The active config (`Some` after [`Self::initialize`]). `None` ⇒ sandbox
    /// disabled (the TS `config === undefined`).
    config: Option<SandboxRuntimeConfig>,
    /// The session violation store (the TS process-wide singleton, here owned).
    violation_store: SandboxViolationStore,
    /// The live infrastructure; `Some` between `initialize` and `reset`.
    running: Option<RunningState>,
    /// The interactive ask-callback threaded into the live proxy filters.
    ask_callback: Option<AskFn>,
}

impl Default for SandboxManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxManager {
    /// Create an idle manager (no config, no running infrastructure).
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: None,
            violation_store: SandboxViolationStore::new(),
            running: None,
            ask_callback: None,
        }
    }

    /// `SandboxManager.getSandboxViolationStore()`.
    #[must_use]
    pub fn violation_store(&self) -> &SandboxViolationStore {
        &self.violation_store
    }

    /// `SandboxManager.getConfig()` — the active config, or `None` if not
    /// initialized.
    #[must_use]
    pub fn get_config(&self) -> Option<&SandboxRuntimeConfig> {
        self.config.as_ref()
    }

    /// `SandboxManager.isSandboxingEnabled()` — `true` once a config is set.
    #[must_use]
    pub fn is_sandboxing_enabled(&self) -> bool {
        self.config.is_some()
    }

    /// Loaded TLS-termination CA, when `network.tls_terminate` is set and the
    /// manager is initialized (the TS `getMitmCA()`).
    #[must_use]
    pub fn mitm_ca(&self) -> Option<&Arc<MitmCa>> {
        self.running.as_ref().and_then(|r| r.mitm_ca.as_ref())
    }

    /// Host HTTP proxy port, when initialized (the TS `getProxyPort()`).
    #[must_use]
    pub fn http_proxy_port(&self) -> Option<u16> {
        self.running.as_ref().map(|r| r.http_port)
    }

    /// Host SOCKS proxy port, when initialized (the TS `getSocksProxyPort()`).
    #[must_use]
    pub fn socks_proxy_port(&self) -> Option<u16> {
        self.running.as_ref().map(|r| r.socks_port)
    }

    /// `SandboxManager.checkDependencies()` — the platform dependency check.
    /// Linux delegates to [`check_linux_dependencies`]; macOS/Windows return the
    /// P9-seam unsupported error. Reads `bwrapPath`/`socatPath`/`seccomp` from
    /// the active config if present.
    #[must_use]
    pub fn check_dependencies(&self) -> LinuxDependencyCheck {
        match host_os() {
            HostOs::Linux => {
                let cfg = self.config.as_ref();
                let seccomp = cfg.and_then(|c| c.seccomp.as_ref());
                check_linux_dependencies(&LinuxDependencyOpts {
                    bwrap_path: cfg.and_then(|c| c.bwrap_path.clone()),
                    socat_path: cfg.and_then(|c| c.socat_path.clone()),
                    seccomp_apply_path: seccomp.and_then(|s| s.apply_path.clone()),
                    seccomp_apply_argv0: seccomp.and_then(|s| s.argv0.clone()),
                })
            }
            _ => LinuxDependencyCheck {
                errors: vec!["Unsupported platform".to_string()],
                warnings: vec![],
            },
        }
    }

    /// `SandboxManager.initialize(config, sandboxAskCallback, enableLogMonitor)`
    /// (`sandbox-manager.js:237-330`).
    ///
    /// Validates the config, loads the TLS-termination CA (if
    /// `network.tls_terminate`), checks dependencies, starts the HTTP + SOCKS
    /// forward proxies bound on localhost (or uses the external
    /// `http_proxy_port`/`socks_proxy_port` when set), and on Linux brings up
    /// the `socat` bridge. The proxies' allowlist filter consults
    /// [`crate::matcher::filter_network_request_with_ask`] with this config +
    /// `ask_callback` (threaded via [`ProxyOptions::ask`]/[`SocksOptions::ask`]).
    ///
    /// `enable_log_monitor` is accepted for signature parity; the macOS log
    /// monitor it gates is a P9 seam (no-op on Linux, the only wired platform).
    ///
    /// # Errors
    /// [`ManagerError`] on invalid config, the tls-terminate/mitm-proxy mutual
    /// exclusion, CA load failure, missing dependencies, or proxy/bridge I/O.
    pub async fn initialize(
        &mut self,
        config: SandboxRuntimeConfig,
        ask_callback: Option<AskFn>,
        enable_log_monitor: bool,
    ) -> Result<(), ManagerError> {
        let _ = enable_log_monitor; // macOS log monitor is a P9 seam.
        config
            .validate()
            .map_err(|e| ManagerError::InvalidConfig(e.0))?;

        // tlsTerminate and mitmProxy are mutually exclusive (TS hard error).
        if config.network.tls_terminate.is_some() && config.network.mitm_proxy.is_some() {
            return Err(ManagerError::TlsTerminateAndMitmProxy);
        }

        // Load the TLS-termination CA (explicit opt-in → a bad config is fatal).
        let mitm_ca: Option<Arc<MitmCa>> = match &config.network.tls_terminate {
            Some(tls) => {
                let opts = MitmCaOptions {
                    ca_cert_path: tls.ca_cert_path.clone().map(PathBuf::from),
                    ca_key_path: tls.ca_key_path.clone().map(PathBuf::from),
                };
                Some(Arc::new(
                    create_mitm_ca(opts).map_err(|e| ManagerError::MitmCa(e.to_string()))?,
                ))
            }
            None => None,
        };

        self.config = Some(config);
        self.ask_callback = ask_callback;

        // Dependency check (after config is stored so it is consulted).
        let deps = self.check_dependencies();
        if !deps.errors.is_empty() {
            // Roll back the partially-applied state so a retry starts clean.
            self.config = None;
            self.ask_callback = None;
            return Err(ManagerError::DependenciesMissing(deps.errors.join(", ")));
        }

        // From here, any failure must tear down what we started.
        match self.start_infrastructure(mitm_ca).await {
            Ok(()) => Ok(()),
            Err(e) => {
                self.reset();
                Err(e)
            }
        }
    }

    /// Bring up the proxies + (on Linux) the bridge and store [`RunningState`].
    /// Split out so `initialize` can `reset()` on any error.
    async fn start_infrastructure(
        &mut self,
        mitm_ca: Option<Arc<MitmCa>>,
    ) -> Result<(), ManagerError> {
        // Safe: `initialize` set `self.config` before calling us.
        let config = self
            .config
            .as_ref()
            .expect("config set before start_infrastructure");

        let net = &config.network;
        let parent_proxy = resolve_parent_proxy(net.parent_proxy.as_ref(), &env_map()).map(Arc::new);
        let net_config = Arc::new(net.clone());

        // ── HTTP proxy ──
        let (http_port, http_task) = if let Some(p) = net.http_proxy_port {
            (p, None) // external proxy; we don't start a server.
        } else {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| ManagerError::Io(format!("HTTP proxy bind: {e}")))?;
            let port = listener
                .local_addr()
                .map_err(|e| ManagerError::Io(format!("HTTP proxy addr: {e}")))?
                .port();
            let options = Arc::new(ProxyOptions {
                config: Arc::clone(&net_config),
                parent_proxy: parent_proxy.clone(),
                // The TS `config.network.filterRequest` is a runtime closure the
                // host supplies; the Rust `NetworkConfig` is a serde struct and
                // cannot carry a closure, so the per-request body hook is wired
                // separately by the library consumer (the manager defaults it to
                // None — a documented faithful gap, not a behavioral change to
                // the allowlist filter).
                filter_request: None,
                mitm_ca: mitm_ca.clone(),
                tls_terminate_upstream_ca: None,
                ask: self.ask_callback.clone(),
            });
            let task = tokio::spawn(async move { serve(listener, options).await });
            (port, Some(task))
        };

        // ── SOCKS proxy ──
        let (socks_port, socks_task) = if let Some(p) = net.socks_proxy_port {
            (p, None)
        } else {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| ManagerError::Io(format!("SOCKS proxy bind: {e}")))?;
            let port = listener
                .local_addr()
                .map_err(|e| ManagerError::Io(format!("SOCKS proxy addr: {e}")))?
                .port();
            let options = Arc::new(SocksOptions {
                config: Arc::clone(&net_config),
                parent_proxy: parent_proxy.clone(),
                ask: self.ask_callback.clone(),
            });
            let task = tokio::spawn(async move {
                let _ = serve_socks(listener, options).await;
            });
            (port, Some(task))
        };

        // ── Linux socat bridge ──
        let (bridge, http_socket_path, socks_socket_path) = if host_os() == HostOs::Linux {
            let bridge = initialize_linux_network_bridge(
                http_port,
                socks_port,
                config.socat_path.as_deref(),
            )
            .map_err(|e| ManagerError::Io(format!("Linux network bridge: {e}")))?;
            let http_sock = bridge.http_socket_path.to_string_lossy().into_owned();
            let socks_sock = bridge.socks_socket_path.to_string_lossy().into_owned();
            (Some(bridge), Some(http_sock), Some(socks_sock))
        } else {
            (None, None, None)
        };

        self.running = Some(RunningState {
            http_port,
            socks_port,
            http_task,
            socks_task,
            bridge,
            mitm_ca,
            http_socket_path,
            socks_socket_path,
        });
        Ok(())
    }

    /// `SandboxManager.wrapWithSandbox(command, binShell, customConfig)`
    /// (`sandbox-manager.js:533-665`) — the LINUX branch only.
    ///
    /// Maps the user-facing [`crate::config::FilesystemConfig`] (custom config
    /// overrides the active config) into the derived
    /// [`ReadConfig`]/[`WriteConfig`], threads the running proxy
    /// sockets/ports/CA into [`wrap_command_with_sandbox_linux`], and returns
    /// `(wrapped_command, mount_points)`.
    ///
    /// FS mapping (faithful to the TS):
    /// - **write** `allow_only = get_default_write_paths() ++
    ///   strip_write_globs(allowWrite)`, `deny_within_allow =
    ///   strip_write_globs(denyWrite)` — `strip_write_globs` = map
    ///   [`remove_trailing_glob_suffix`], then drop entries that still
    ///   [`contains_glob_chars`] on Linux (bwrap has no globs).
    /// - **read** `deny_only = expand(denyRead)`, `allow_within_deny =
    ///   expand(allowRead) ++ ca_cert_path (if a CA is loaded)` — `expand` =
    ///   [`remove_trailing_glob_suffix`]; if it still
    ///   [`contains_glob_chars`] on Linux → [`expand_glob_pattern`], else the
    ///   stripped path. The CA cert is force-added to `allow_within_deny` so the
    ///   sandboxed child can read it even under a user `denyRead`.
    /// - `needs_network_restriction` = the active/custom config defines
    ///   `network` (even empty `allowedDomains` ⇒ block-all).
    ///
    /// # Errors
    /// [`ManagerError::Unsupported`] on macOS/Windows (documented P9 seams) or
    /// when not initialized; [`ManagerError::Io`] from the bwrap wrap.
    pub fn wrap_with_sandbox(
        &self,
        command: &str,
        bin_shell: Option<&str>,
        custom_config: Option<&SandboxRuntimeConfig>,
        cwd: &str,
    ) -> Result<(String, Vec<PathBuf>), ManagerError> {
        match host_os() {
            HostOs::Linux => self.wrap_linux(command, bin_shell, custom_config, cwd),
            HostOs::Macos => Err(ManagerError::Unsupported(
                "wrap_with_sandbox: macOS is a P9 seam (wrapCommandWithSandboxMacOS not ported)"
                    .to_string(),
            )),
            HostOs::Windows => Err(ManagerError::Unsupported(
                "wrap_with_sandbox: Windows is a P9 seam (use the argv wrapper; \
                 wrapCommandWithSandboxWindows not ported)"
                    .to_string(),
            )),
            HostOs::Other => Err(ManagerError::Unsupported(
                "wrap_with_sandbox: unsupported platform".to_string(),
            )),
        }
    }

    /// The Linux `wrap_with_sandbox` branch (`sandbox-manager.js:632-662`).
    fn wrap_linux(
        &self,
        command: &str,
        bin_shell: Option<&str>,
        custom_config: Option<&SandboxRuntimeConfig>,
        cwd: &str,
    ) -> Result<(String, Vec<PathBuf>), ManagerError> {
        let active = self.config.as_ref();
        let ca_cert_path = self
            .mitm_ca()
            .map(|ca| ca.cert_path.to_string_lossy().into_owned());
        let (read_config, write_config) =
            build_fs_configs(active, custom_config, ca_cert_path.as_deref());

        // Network restriction is needed whenever network config defines
        // allowedDomains (even empty = block-all). Mirrors the TS
        // `hasNetworkConfig`: custom or active config carries an allowedDomains
        // field. `NetworkConfig.allowed_domains` is a non-Option `Vec`, so a
        // present `network` block always counts. Defined when a config exists.
        let needs_network_restriction = custom_config.is_some() || active.is_some();

        // Only thread proxy sockets/ports when the proxy is running (= we have
        // network config AND a RunningState). Empty allowlist still routes
        // through the proxy so it can block-all.
        let running = self.running.as_ref();
        let proxy_live = needs_network_restriction && running.is_some();
        let http_socket_path = if proxy_live {
            running.and_then(|r| r.http_socket_path.as_deref())
        } else {
            None
        };
        let socks_socket_path = if proxy_live {
            running.and_then(|r| r.socks_socket_path.as_deref())
        } else {
            None
        };
        let http_proxy_port = if proxy_live {
            running.map(|r| r.http_port)
        } else {
            None
        };
        let socks_proxy_port = if proxy_live {
            running.map(|r| r.socks_port)
        } else {
            None
        };

        let seccomp = active.and_then(|c| c.seccomp.as_ref());
        let ripgrep_cmd = active
            .and_then(|c| c.ripgrep.as_ref().map(|r| r.command.clone()))
            .unwrap_or_else(|| "rg".to_string());
        let mandatory_deny_search_depth =
            active.and_then(|c| c.mandatory_deny_search_depth).unwrap_or(3) as usize;
        let allow_git_config = active
            .and_then(|c| c.filesystem.allow_git_config)
            .unwrap_or(false);
        let enable_weaker_nested_sandbox = active
            .and_then(|c| c.enable_weaker_nested_sandbox)
            .unwrap_or(false);
        let allow_all_unix_sockets = active
            .and_then(|c| c.network.allow_all_unix_sockets)
            .unwrap_or(false);
        let bwrap_path = active.and_then(|c| c.bwrap_path.as_deref());
        let socat_path = active.and_then(|c| c.socat_path.as_deref());
        let tmpdir = resolve_tmpdir();

        let params = WrapParams {
            command,
            needs_network_restriction,
            http_socket_path,
            socks_socket_path,
            http_proxy_port,
            socks_proxy_port,
            ca_cert_path: ca_cert_path.as_deref(),
            read_config: Some(&read_config),
            write_config: Some(&write_config),
            enable_weaker_nested_sandbox,
            allow_all_unix_sockets,
            seccomp_apply_path: seccomp.and_then(|s| s.apply_path.as_deref()),
            seccomp_apply_argv0: seccomp.and_then(|s| s.argv0.as_deref()),
            bin_shell,
            ripgrep_cmd: &ripgrep_cmd,
            mandatory_deny_search_depth,
            allow_git_config,
            bwrap_path,
            socat_path,
            cwd,
            platform: Platform::Linux,
            tmpdir: &tmpdir,
        };
        wrap_command_with_sandbox_linux(&params).map_err(|e| ManagerError::Io(e.to_string()))
    }

    /// `SandboxManager.updateConfig(newConfig)` (`sandbox-manager.js:736-748`).
    ///
    /// Replaces the active config.
    ///
    /// # Per-request vs structural (a documented divergence from the TS)
    ///
    /// In the TS, the running proxies read `config.network.allowedDomains` /
    /// `deniedDomains` **per request** off the shared module `config`, so an
    /// allowlist change here is a **live swap** that affects already-running
    /// proxies with no rebind. The Rust proxies, by contrast, capture an
    /// `Arc<NetworkConfig>` **snapshot** at `serve()` time
    /// ([`ProxyOptions::config`]/[`SocksOptions::config`] are immutable `Arc`s),
    /// so this `update_config` does **NOT** retro-apply the new allowlist to
    /// proxies already running. It DOES take effect for the next
    /// `wrap_with_sandbox` (FS/network mapping) and for the next `initialize`.
    /// To make a network/allowlist change live, `reset()` then `initialize()`
    /// with the new config. (Filesystem changes are baked at wrap time on every
    /// platform, so they are never live — same as the TS.) A future enhancement
    /// could back the proxy config with an `ArcSwap` to restore TS live-swap.
    pub fn update_config(&mut self, new_config: SandboxRuntimeConfig) {
        self.config = Some(new_config);
    }

    /// `SandboxManager.reset()` (`sandbox-manager.js:872-937`): tear everything
    /// down. Aborts the proxy accept-loop tasks, drops the bridge (its `Drop`
    /// SIGTERMs the `socat` children + the manager removes the socket files),
    /// disposes the CA (`dispose_mitm_ca` — removes the ephemeral temp dir),
    /// cleans up leftover bwrap mount points, and clears the running state.
    /// Idempotent.
    pub fn reset(&mut self) {
        // The TS reset() force-cleans an accumulated module-level mount-point
        // set. The Rust `cleanup_bwrap_mount_points` is stateless — it takes the
        // exact mount points to clean — and `wrap_with_sandbox` returns those to
        // its caller (who cleans up after each command, the TS
        // `cleanupAfterCommand`). The manager therefore holds no mount-point set
        // to clean here; the empty-slice call documents the seam and is a no-op.
        cleanup_bwrap_mount_points(&[]);

        if let Some(mut running) = self.running.take() {
            if let Some(task) = running.http_task.take() {
                task.abort();
            }
            if let Some(task) = running.socks_task.take() {
                task.abort();
            }
            // Drop the bridge (SIGTERMs socat children), then remove the socket
            // files (the TS `fs.rmSync(..., { force: true })`).
            drop(running.bridge.take());
            for sock in [
                running.http_socket_path.take(),
                running.socks_socket_path.take(),
            ]
            .into_iter()
            .flatten()
            {
                let _ = std::fs::remove_file(&sock);
            }
            if let Some(ca) = running.mitm_ca.take() {
                dispose_mitm_ca(&ca);
            }
        }

        self.running = None;
        // The TS reset also clears `config`/`parentProxy`. We keep `config` so
        // `get_config` survives a reset (matching the common host pattern of
        // reset-then-reinitialize with the same config); callers that want a
        // full wipe drop the manager. `ask_callback` is cleared so a dropped
        // session can't leak a stale callback.
        self.ask_callback = None;
    }
}

/// Snapshot `std::env` into the `HashMap` shape [`resolve_parent_proxy`] takes
/// (it reads `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` + lowercase variants).
fn env_map() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// Map the user-facing filesystem config (`custom` overrides `active`, else
/// empty) into the derived `(ReadConfig, WriteConfig)` for the Linux wrap, plus
/// force-add `ca_cert_path` (if any) to the read allow-set. The glob handling is
/// the faithful TS port (`sandbox-manager.js:542-587`); see
/// [`SandboxManager::wrap_with_sandbox`] for the rules.
fn build_fs_configs(
    active: Option<&SandboxRuntimeConfig>,
    custom: Option<&SandboxRuntimeConfig>,
    ca_cert_path: Option<&str>,
) -> (ReadConfig, WriteConfig) {
    // Resolve each FS list: custom config wins, else active config, else [].
    let pick = |f: &dyn Fn(&SandboxRuntimeConfig) -> Vec<String>| -> Vec<String> {
        custom.map(f).or_else(|| active.map(f)).unwrap_or_default()
    };
    let allow_write = pick(&|c| c.filesystem.allow_write.clone());
    let deny_write = pick(&|c| c.filesystem.deny_write.clone());
    let deny_read = pick(&|c| c.filesystem.deny_read.clone());
    let allow_read = pick(&|c| c.filesystem.allow_read.clone().unwrap_or_default());

    // strip_write_globs: map remove_trailing_glob_suffix; drop entries that still
    // contain glob chars on Linux (bwrap can't take globs).
    let strip_write_globs = |paths: Vec<String>| -> Vec<String> {
        paths
            .into_iter()
            .map(|p| remove_trailing_glob_suffix(&p))
            .filter(|p| !contains_glob_chars(p))
            .collect()
    };
    let mut allow_only = get_default_write_paths();
    allow_only.extend(strip_write_globs(allow_write));
    let write_config = WriteConfig {
        allow_only,
        deny_within_allow: strip_write_globs(deny_write),
    };

    // expand: remove_trailing_glob_suffix; if still glob on Linux →
    // expand_glob_pattern, else the stripped path.
    let expand = |paths: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        for p in paths {
            let stripped = remove_trailing_glob_suffix(p);
            if contains_glob_chars(&stripped) {
                out.extend(expand_glob_pattern(p));
            } else {
                out.push(stripped);
            }
        }
        out
    };
    let deny_only = expand(&deny_read);
    let mut allow_within_deny = expand(&allow_read);
    // The CA cert must be readable by the child even under a denyRead.
    if let Some(ca) = ca_cert_path {
        allow_within_deny.push(ca.to_string());
    }
    (
        ReadConfig {
            deny_only,
            allow_within_deny,
        },
        write_config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FilesystemConfig, NetworkConfig};

    // Used by the Linux-gated lifecycle test; unused on other hosts.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn github_config() -> SandboxRuntimeConfig {
        SandboxRuntimeConfig {
            network: NetworkConfig {
                allowed_domains: vec!["github.com".into()],
                ..Default::default()
            },
            filesystem: FilesystemConfig::default(),
            ..Default::default()
        }
    }

    /// `wrap_with_sandbox` on a non-Linux host returns the documented P9 seam
    /// error (the macOS/Windows backends are not ported). This dispatch happens
    /// before any Linux work, so it is testable on any host. On Linux this
    /// branch isn't taken, so the assertion is gated off there.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn wrap_on_non_linux_is_p9_seam() {
        let mgr = SandboxManager::new();
        let err = mgr
            .wrap_with_sandbox("echo hi", Some("bash"), None, "/tmp")
            .unwrap_err();
        match err {
            ManagerError::Unsupported(msg) => assert!(msg.contains("P9")),
            other => panic!("expected Unsupported P9 seam, got {other:?}"),
        }
    }

    /// The FS-config mapping is host-independent in its glob/default-write/CA
    /// shape, but `wrap_with_sandbox` only runs the mapping on Linux (else it
    /// returns the P9 seam). This Linux-gated test drives the full lifecycle:
    /// initialize → proxies bound → wrap string shape → denied host blocked →
    /// reset. It runtime-skips when bwrap/socat are unavailable so it is a no-op
    /// on a Linux box without the sandbox deps (and the body always compiles).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_lifecycle_init_wrap_deny_reset() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let mut mgr = SandboxManager::new();

        // Skip if the sandbox dependencies (bwrap/socat) are missing.
        {
            let probe = {
                let mut m = SandboxManager::new();
                m.config = Some(github_config());
                m.check_dependencies()
            };
            if !probe.errors.is_empty() {
                eprintln!("skipping: sandbox deps unavailable: {:?}", probe.errors);
                return;
            }
        }

        // ── initialize: tlsTerminate set so the CA cert path is exercised ──
        let mut cfg = github_config();
        cfg.network.tls_terminate = Some(crate::config::TlsTerminateConfig::default());
        mgr.initialize(cfg, None, false)
            .await
            .expect("initialize should succeed with deps present");

        // Proxies are listening on bound ports.
        let http_port = mgr.http_proxy_port().expect("http proxy port");
        let socks_port = mgr.socks_proxy_port().expect("socks proxy port");
        assert!(http_port > 0 && socks_port > 0);
        assert!(
            TcpStream::connect(("127.0.0.1", http_port)).await.is_ok(),
            "HTTP proxy port should be bound"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", socks_port)).await.is_ok(),
            "SOCKS proxy port should be bound"
        );
        let ca_cert = mgr
            .mitm_ca()
            .map(|c| c.cert_path.to_string_lossy().into_owned())
            .expect("CA loaded for tlsTerminate");

        // ── wrap: assert the bwrap string shape ──
        let (wrapped, _mounts) = mgr
            .wrap_with_sandbox("echo hi", Some("bash"), None, "/tmp")
            .expect("wrap should succeed on Linux with deps");
        assert!(wrapped.contains("--unshare-net"), "wrapped: {wrapped}");
        assert!(wrapped.contains("--bind"), "socket binds: {wrapped}");
        assert!(
            wrapped.contains("--setenv") && wrapped.contains("HTTP_PROXY"),
            "proxy env: {wrapped}"
        );
        assert!(wrapped.contains("socat"), "sandbox socat command: {wrapped}");
        // tlsTerminate → the CA cert path is wired into the read allow set,
        // which surfaces as a bwrap arg referencing the cert path.
        assert!(
            wrapped.contains(&ca_cert),
            "CA cert path should be readable in the sandbox: {ca_cert}"
        );

        // ── denied host through the running HTTP proxy → 403 ──
        {
            let mut c = TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
            c.write_all(b"CONNECT denied.example.com:443 HTTP/1.1\r\nHost: denied.example.com:443\r\n\r\n")
                .await
                .unwrap();
            let mut buf = vec![0u8; 256];
            let n = c.read(&mut buf).await.unwrap();
            let head = String::from_utf8_lossy(&buf[..n]);
            assert!(head.contains("403"), "expected 403, got: {head}");
        }

        // ── denied host through the running SOCKS proxy → REP_NOT_ALLOWED ──
        {
            let mut c = TcpStream::connect(("127.0.0.1", socks_port)).await.unwrap();
            c.write_all(&[0x05, 0x01, 0x00]).await.unwrap(); // greeting
            let mut sel = [0u8; 2];
            c.read_exact(&mut sel).await.unwrap();
            assert_eq!(sel, [0x05, 0x00]);
            let name = b"denied.example.com";
            let dlen = u8::try_from(name.len()).expect("test domain fits in a byte");
            let mut req = vec![0x05, 0x01, 0x00, 0x03, dlen];
            req.extend_from_slice(name);
            req.extend_from_slice(&443u16.to_be_bytes());
            c.write_all(&req).await.unwrap();
            let mut reply = [0u8; 10];
            c.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply[1], crate::socks_proxy::REP_NOT_ALLOWED);
        }

        // ── reset tears down: ports no longer bound ──
        mgr.reset();
        assert!(mgr.http_proxy_port().is_none());
        // Give the aborted accept loop a moment, then confirm the port is free
        // (a fresh bind on the same port succeeds).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            TcpListener::bind(("127.0.0.1", http_port)).await.is_ok(),
            "HTTP proxy port should be released after reset"
        );
    }
}

//! `srt` — run a command inside the sandbox with network + filesystem
//! restrictions. A faithful port of the npm package's `cli.js` (the `srt`
//! command-line entry).
//!
//! Usage mirrors the TS commander CLI:
//! - `srt [command...]` — argv-style: each arg is shell-quoted, then the joined
//!   string is wrapped + executed.
//! - `srt -c '<command>'` — run the command string directly (like `sh -c`), no
//!   escaping applied.
//! - `srt -d/--debug` — set `SRT_DEBUG=true`.
//! - `srt -s/--settings <path>` — config path (default `~/.srt-settings.json`).
//! - `srt --control-fd <fd>` — accepted for parity; see [`run`] for why it is a
//!   best-effort no-op seam in this port.
//! - `srt windows-install` / `srt windows-uninstall` — Windows-only subcommands
//!   (error "Windows-only" off Windows).
//!
//! The wrapped command is a shell string (`env <PROXY...> sandbox-exec -p
//! <profile> <shell> -c <command>` on macOS, the bwrap invocation on Linux);
//! it is executed via `sh -c <wrapped>` (the TS `spawn(cmd, {shell:true})`).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use sandbox_runtime::config::{FilesystemConfig, NetworkConfig};
use sandbox_runtime::{SandboxManager, SandboxRuntimeConfig};

/// Run commands in a sandbox with network and filesystem restrictions.
#[derive(Debug, Parser)]
#[command(
    name = "srt",
    about = "Run commands in a sandbox with network and filesystem restrictions",
    // commander's `.allowUnknownOption()` — pass unrecognized flags through to
    // the wrapped command rather than erroring.
    allow_external_subcommands = true,
    trailing_var_arg = true,
    disable_help_subcommand = true
)]
struct Cli {
    /// Windows-only management subcommands (`windows-install` /
    /// `windows-uninstall`). When absent, the default run command applies.
    #[command(subcommand)]
    command: Option<Sub>,

    /// Enable debug logging (sets `SRT_DEBUG=true`).
    #[arg(short, long)]
    debug: bool,

    /// Path to config file (default: `~/.srt-settings.json`).
    #[arg(short, long, value_name = "path")]
    settings: Option<PathBuf>,

    /// Run command string directly (like `sh -c`), no escaping applied.
    #[arg(short = 'c', value_name = "command")]
    c: Option<String>,

    /// Read config updates from file descriptor (JSON lines protocol).
    /// Accepted for parity; a best-effort no-op seam in this port (see [`run`]).
    #[arg(long = "control-fd", value_name = "fd")]
    control_fd: Option<i32>,

    /// The command to run in the sandbox (argv-style; each arg is shell-quoted).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

/// The cfg-gated management subcommands (`windows-install` /
/// `windows-uninstall`).
#[derive(Debug, Subcommand)]
enum Sub {
    /// Windows: create the discriminator group + install WFP filters.
    WindowsInstall,
    /// Windows: remove WFP filters.
    WindowsUninstall,
}

/// Default config path: `~/.srt-settings.json` (the TS `getDefaultConfigPath`).
fn get_default_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".srt-settings.json")
}

/// The minimal default config when no config file exists (the TS
/// `getDefaultConfig`): empty network allow/deny + empty filesystem lists. An
/// empty `allowedDomains` means "block all network".
fn get_default_config() -> SandboxRuntimeConfig {
    SandboxRuntimeConfig {
        network: NetworkConfig {
            allowed_domains: Vec::new(),
            denied_domains: Vec::new(),
            ..Default::default()
        },
        filesystem: FilesystemConfig {
            deny_read: Vec::new(),
            allow_read: Some(Vec::new()),
            allow_write: Vec::new(),
            deny_write: Vec::new(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Read + parse the JSON config file (the TS `loadConfig`). Returns `None` if
/// the file is absent or cannot be read/parsed (faithful to the TS, which
/// swallows the error and falls back to the default config).
fn load_config(path: &Path) -> Option<SandboxRuntimeConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Debug log to stderr when `SRT_DEBUG` is set (the TS `logForDebugging`, which
/// reads `SRT_DEBUG` — not `DEBUG`).
fn log_for_debugging(msg: &str) {
    if std::env::var_os("SRT_DEBUG").is_some() {
        eprintln!("{msg}");
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Sub::WindowsInstall) => windows_install(),
        Some(Sub::WindowsUninstall) => windows_uninstall(),
        None => run(&cli),
    }
}

/// `windows-install` (cfg-gated; the admin install flow is a tracked gap, so
/// even on Windows this reports the gap rather than performing the install).
#[allow(clippy::unnecessary_wraps)]
fn windows_install() -> ExitCode {
    #[cfg(target_os = "windows")]
    {
        eprintln!(
            "Error: windows-install is not yet wired in this build \
             (the admin install/uninstall flow is a tracked gap)."
        );
        ExitCode::FAILURE
    }
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("Error: windows-install is Windows-only.");
        ExitCode::FAILURE
    }
}

/// `windows-uninstall` (cfg-gated; see [`windows_install`]).
#[allow(clippy::unnecessary_wraps)]
fn windows_uninstall() -> ExitCode {
    #[cfg(target_os = "windows")]
    {
        eprintln!(
            "Error: windows-uninstall is not yet wired in this build \
             (the admin install/uninstall flow is a tracked gap)."
        );
        ExitCode::FAILURE
    }
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("Error: windows-uninstall is Windows-only.");
        ExitCode::FAILURE
    }
}

/// The default run action (`cli.js:118-`): load config, initialize the sandbox,
/// build the command (`-c` direct or shlex-quoted argv), wrap it for the host
/// platform, exec it with inherited stdio, and propagate the child's exit code.
///
/// ## `--control-fd` (divergence)
/// The TS reads JSON-lines off the control fd and calls
/// `SandboxManager.updateConfig` live. The Rust proxies capture an immutable
/// `Arc<NetworkConfig>` snapshot at `serve()` time, so `update_config` does NOT
/// retro-apply to already-running proxies (see `SandboxManager::update_config`).
/// We therefore accept `--control-fd` for parity but treat it as a no-op
/// best-effort seam (only logged under `SRT_DEBUG`).
///
/// ## Signal handling (divergence)
/// The TS forwards SIGINT/SIGTERM to the child and, on exit, maps a
/// SIGINT/SIGTERM-killed child to exit 0 and any other signal to exit 1. We run
/// the wrapped command via `sh -c`; the shell forwards terminal signals to the
/// child for us. We map the child's terminating signal the same way on exit.
fn run(cli: &Cli) -> ExitCode {
    if cli.debug {
        // SAFETY-FREE: set_var is safe in single-threaded startup before we
        // spawn the tokio runtime / child. Mirrors the TS `process.env.SRT_DEBUG`.
        std::env::set_var("SRT_DEBUG", "true");
    }

    if cli.control_fd.is_some() {
        log_for_debugging(
            "--control-fd accepted but is a no-op in this port (proxy config is an \
             immutable Arc snapshot; use reset()+initialize() to change the allowlist).",
        );
    }

    // Load config from file (settings override, else the default path), falling
    // back to the minimal default config.
    let config_path = cli.settings.clone().unwrap_or_else(get_default_config_path);
    let runtime_config = load_config(&config_path).unwrap_or_else(|| {
        log_for_debugging(&format!(
            "No config found at {}, using default config",
            config_path.display()
        ));
        get_default_config()
    });

    // Build the command string before doing async work so a missing command
    // fails fast with the exact TS message.
    let command = if let Some(c) = &cli.c {
        log_for_debugging(&format!("Command string mode (-c): {c}"));
        c.clone()
    } else if cli.args.is_empty() {
        eprintln!("Error: No command specified. Use -c <command> or provide command arguments.");
        return ExitCode::FAILURE;
    } else {
        // argv-style: shell-quote each arg so it survives the later `sh -c`
        // re-parse (the TS `shellquote.quote`).
        let quoted = shlex::try_join(cli.args.iter().map(String::as_str))
            .unwrap_or_else(|_| cli.args.join(" "));
        log_for_debugging(&format!("Original command: {quoted}"));
        quoted
    };

    // A tokio runtime for the async initialize.
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut manager = SandboxManager::new();
    log_for_debugging("Initializing sandbox...");
    if let Err(e) = rt.block_on(manager.initialize(runtime_config, None, false)) {
        eprintln!("Error: {e}");
        return ExitCode::FAILURE;
    }

    let cwd = std::env::current_dir()
        .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().into_owned());

    exec_wrapped(&manager, &command, &cwd)
}

/// Platform-dispatched wrap + exec (the `cli.js` `child` spawn).
///
/// On non-Windows: `wrap_with_sandbox` returns a shell string; run it via
/// `sh -c <wrapped>` with inherited stdio, then clean up any returned mount
/// points (Linux bwrap artifacts; empty on macOS). On Windows the wrapper
/// returns an argv array spawned with no shell — but that path is the tracked
/// P9 seam, so we report it.
#[cfg(not(target_os = "windows"))]
fn exec_wrapped(manager: &SandboxManager, command: &str, cwd: &str) -> ExitCode {
    use sandbox_runtime::linux::cleanup_bwrap_mount_points;

    let (wrapped, mount_points) = match manager.wrap_with_sandbox(command, None, None, cwd) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // The TS `spawn(cmd, {shell:true})` runs the wrapped string through the host
    // shell (`/bin/sh -c` on POSIX). We do the same.
    let status = Command::new("/bin/sh").arg("-c").arg(&wrapped).status();

    // Clean up bwrap mount-point artifacts (the TS `cleanupAfterCommand`;
    // no-op on macOS where the list is empty).
    cleanup_bwrap_mount_points(&mount_points);

    match status {
        Ok(s) => exit_code_from_status(s),
        Err(e) => {
            eprintln!("Failed to execute command: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Map a finished child's status to a process exit code (the TS `child.on('exit',
/// (code, signal) => ...)`): a SIGINT/SIGTERM kill → 0, any other signal → 1,
/// else the child's exit code.
#[cfg(unix)]
fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    if let Some(sig) = status.signal() {
        // libc SIGINT = 2, SIGTERM = 15.
        if sig == 2 || sig == 15 {
            return ExitCode::SUCCESS;
        }
        eprintln!("Process killed by signal: {sig}");
        return ExitCode::FAILURE;
    }
    let code = status.code().unwrap_or(0);
    // ExitCode only carries a u8; clamp like a shell would (code & 0xff).
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}

#[cfg(not(unix))]
fn exit_code_from_status(status: std::process::ExitStatus) -> ExitCode {
    let code = status.code().unwrap_or(0);
    ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1))
}

/// Windows wrap + exec: the argv path is the tracked P9 seam in this port.
#[cfg(target_os = "windows")]
fn exec_wrapped(_manager: &SandboxManager, _command: &str, _cwd: &str) -> ExitCode {
    eprintln!(
        "Error: the Windows argv sandbox path (wrap_with_sandbox_argv) is not yet \
         wired in this build (tracked P9 seam)."
    );
    ExitCode::FAILURE
}

# Platform support matrix (v0.3.0)

LingXi Core's behavioral parity with claude-code targets desktop OS releases.
This document is the authoritative per-OS capability table; it mirrors
`docs/ARCHITECTURE.md#capability-matrix` and adds setup notes.

## Tier 1: full support

### macOS (13 Ventura+ on Apple silicon and Intel)
- **FileSystem watch:** FSEvents via the `notify` crate's `FSEventWatcher`.
- **Sandbox:** `sandbox-exec` with a generated SBPL profile.
- **Swarm:** tmux 3.2+ (preferred) or iTerm via `osascript`. Falls back to
  `InProcess` (no pane visualization) when neither is available.
- **SecureStorage:** macOS Keychain via the `security` CLI. Service-name
  format `Claude Code{oauth_suffix}-credentials{dir_hash}`. 30s TTL cache,
  generation-counter writes, in-flight dedupe. Falls back to plaintext on
  init error with the literal warning `"Warning: Storing credentials in plaintext."`.
- **Process tree-kill:** `nix::sys::signal::killpg` against the child's
  process group (background children are spawned with `setsid()`).
- **HTTP SSE:** `reqwest::Response::bytes_stream()` + `parse_sse_chunks`.

### Linux (glibc, kernel 4.x+; major distros)
- **FileSystem watch:** inotify via `notify`.
- **Sandbox:** `bwrap` (bubblewrap) + `socat` companion for network proxying.
  Install: `apt install bubblewrap socat` (Debian/Ubuntu) or distro equivalent.
- **Swarm:** tmux 3.2+ only (no iTerm path).
- **SecureStorage:** **plaintext** (`PlainTextSecureStorage` at
  `~/.claude/.credentials.json`). Native libsecret backend deferred (matches
  claude-code's plaintext fallback on Linux). The plaintext warning above
  is emitted on first credential write.
- **Process tree-kill:** same as macOS.

### WSL2
- Treated as Linux end-to-end. `bwrap+socat` works the same way.

## Tier 2: limited support

### Windows (10 22H2+, 11)
- **FileSystem watch:** ReadDirectoryChangesW via `notify`.
- **Sandbox:** **Unsupported.** `WindowsSandbox::is_available()` is `false`;
  `prepare()` returns `SandboxError::Unsupported(...)`. claude-code does not
  support a sandbox on Windows; we mirror that to keep policy compatibility.
- **Swarm / tmux:** **Unsupported.** `start_swarm()` returns
  `SwarmError::Unsupported("--tmux is not supported on Windows")`.
- **SecureStorage:** plaintext. Windows Credential Vault backend deferred to M3+.
- **Process tree-kill:** `taskkill /T /F /PID <pid>`.
- **MCP / LSP / HTTP SSE / Worktree:** full support — same code paths as macOS/Linux.

## Refused

### WSL1
- **Sandbox initialize refuses.** Detection: `/proc/version` lacks
  `microsoft-standard` / `WSL2` substring. Error: `"sandbox.enabled is set
  but WSL1 is not supported (requires WSL2)"`.
- Non-sandbox features (LSP, MCP, Worktree, etc.) still work, but the
  `/sandbox` and `/doctor` tools will report sandbox unavailable.

## Out of scope for v0.3.0

### Android / iOS (M3)
- Cross-compile matrix in CI is informational only — failures do not block
  v0.3.0 release. Platform crates (`platforms/android`, `platforms/ios`) and
  capability flag wiring per spec §35 land in M3.

### Browser / web UI (M4+)
- The optional web `pty-server` from claude-code is a separate UI feature;
  not part of v0.3.0.

## Setup notes

### macOS
- Keychain unlock prompts may surface on first credential write — set the
  keychain to "always allow" for the `security` binary if running in CI.
- The `security` CLI is part of macOS; no additional install needed.

### Linux (sandbox)
- Ubuntu / Debian: `sudo apt install bubblewrap socat`.
- Fedora / RHEL: `sudo dnf install bubblewrap socat`.
- Arch: `sudo pacman -S bubblewrap socat`.
- `tmux` is required for the swarm backend: typically pre-installed; install
  via the same package manager if missing.

### Windows
- `git` must be on `PATH` (for worktree operations). Recommended: Git for
  Windows distribution, which includes the `git` CLI and `bash.exe` (we do
  NOT depend on `bash.exe` — production `ProcessRunner` uses native Windows
  process APIs via tokio).
- No bubblewrap / sandbox-exec equivalent; sandbox is intentionally
  unsupported.

## Verifying your install

```bash
cargo run -p lingxi-demo -- --doctor
```

The `/doctor` command prints the resolved platform, the populated
`PlatformCapabilities` struct, and lists any subsystems reporting
`Unsupported`. This is the canonical health check before opening a bug.

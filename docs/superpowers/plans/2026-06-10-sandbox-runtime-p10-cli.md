# sandbox-runtime P10 — `srt` CLI + public API (cli.js + index.js) — FINAL

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `cli.js` (the `srt` command-line entry) + `index.js` (the package's public API surface) — the capstone that makes `sandbox-runtime` a runnable tool and a clean library. After this, the entire `@anthropic-ai/sandbox-runtime@0.0.54` package is ported 1:1.

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/{cli.js, index.js}` — READ both. Drives `manager::SandboxManager` (P8b). `clap` 4.5 is in the lock.

**Branch:** `parity-sandbox-runtime-p10`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. This host is macOS — the `srt -c 'echo hi'` smoke test runs here.

---

### Task 1: public API re-exports (index.js)

**Files:** Modify `lingxi-code/sandbox-runtime/src/lib.rs`.

- [ ] Add crate-root `pub use` re-exports mirroring `index.js`'s public surface (so consumers get a flat API): `SandboxManager` (manager), `SandboxViolationStore` + `Violation` (violation_store), the config types (`SandboxRuntimeConfig`, `NetworkConfig`, `FilesystemConfig`, `IgnoreViolationsConfig`, `RipgrepConfig`, `WindowsConfig` — the Rust structs replace the zod `*Schema` exports), the Windows API (`get_srt_win_path`, `get_windows_group_status`, `get_windows_wfp_status`, `DEFAULT_WINDOWS_GROUP_NAME`, `DEFAULT_WINDOWS_PROXY_PORT_RANGE`), `path_utils::get_default_write_paths`. (Whatever Windows install fns exist; the admin install/uninstall flow is the tracked gap — re-export what's ported.) Add a crate-level doc note listing the public API. No tests needed beyond `cargo build` proving the re-exports resolve. Commit (`feat(sandbox-runtime): public API re-exports (index.js) (P10)`).

### Task 2: the `srt` CLI binary

**Files:** Create `lingxi-code/sandbox-runtime/src/bin/srt.rs` (or a `[[bin]]` target); Modify `Cargo.toml` (+`clap` + `tokio` features for the bin; the lib stays as-is). If a bin in the lib crate pulls clap into the lib build, prefer a `[[bin]]` with `required-features` or a tiny separate `srt` crate — keep `engine-mobile` 0-dep regardless.

- [ ] **`get_default_config_path()`** = `~/.srt-settings.json`; **`get_default_config()`** = `{network:{allowedDomains:[],deniedDomains:[]}, filesystem:{denyRead:[],allowRead:[],allowWrite:[],denyWrite:[]}}` (the minimal default `SandboxRuntimeConfig`); **`load_config(path)`** = read+parse the JSON config file (None if absent/unparseable, faithful to `loadConfig`).
- [ ] **clap CLI** (commander → clap): the default run command — args `[command...]`, `-d/--debug` (sets `SRT_DEBUG`), `-s/--settings <path>`, `-c <command>` (direct string, no escaping), `--control-fd <fd>` (live config updates — JSON-lines; on platforms where the live-update isn't wired due to the Arc-snapshot config, accept the flag + document it's a no-op/best-effort seam), `allow_unknown_option`-equivalent. Plus subcommands `windows-install` / `windows-uninstall` (cfg-gated; call the Windows API or error "Windows-only" on non-Windows).
- [ ] **run action** (cli.js:118-): load config (settings or default path; fall back to `get_default_config`); a tokio runtime → `SandboxManager::initialize(config, None /*ask*/, false)`; build `command` (`-c` direct, else `shlex`-quote the `[command...]` argv; error "No command specified" if neither); platform dispatch: non-Windows → `wrap_with_sandbox(command, bin_shell=None, None, cwd)` → returns the shell string → `std::process::Command::new(shell).arg("-c").arg(wrapped)` with inherited stdio, spawn, wait; Windows → `wrap_with_sandbox_argv` → spawn argv with the env, no shell. After the child exits → `cleanup_bwrap_mount_points(mount_points)` (the points returned by wrap) → propagate the child's exit code (signal SIGINT/SIGTERM → exit 0; other signal → exit 1). SIGINT/SIGTERM forwarding to the child (best-effort). On any error → stderr + exit 1.
- [ ] **Smoke test (macOS, runs here):** a test (or a `scripts/`-driven check) that builds `srt` and runs `srt -c 'echo hi'` with an empty/default config → exits 0 + prints `hi` (the macOS sandbox wraps + execs). And `srt` with no command → exit 1 + the error. Gate the bridge/Linux-only bits; the macOS run works on this host. Commit (`feat(sandbox-runtime): srt CLI — run/windows-install/uninstall + exec (P10)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + `cargo build -p sandbox-runtime --bin srt` + clippy `-D warnings` (lib + bin) + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0 — confirm the srt bin's clap/tokio don't leak into engine-mobile) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. Public API re-exported (index.js surface) — `cargo build` resolves the `pub use`s.
2. `srt` CLI: config load (settings/default/`~/.srt-settings.json`), initialize, `-c`/argv command modes, platform-dispatched wrap + exec with stdio-inherit + exit-code/signal propagation + post-command cleanup; windows-install/uninstall subcommands (cfg-gated). **`srt -c 'echo hi'` runs + exits 0 on this macOS host.**
3. engine-mobile 0-dep (the srt bin's deps don't leak); frozen empty.
4. **The entire `@anthropic-ai/sandbox-runtime` package is now ported 1:1** (P1–P10) — note the two tracked gaps: Windows admin install/uninstall flow + the `update_config` live-config (ArcSwap) refinement.

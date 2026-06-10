# sandbox-runtime P9b — Windows backend (windows-sandbox-utils.js)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `windows-sandbox-utils.js` into `sandbox-runtime/src/windows.rs` — a thin wrapper around the external `srt-win.exe` Rust helper (discriminator group + machine-wide WFP filters). Port `get_srt_win_path` (binary resolution), `group_ref_args`, `wrap_command_with_sandbox_windows` (build the `srt-win exec` argv + env), `check_windows_dependencies`, the group/WFP status queries, and the `DEFAULT_WINDOWS_*` consts. Windows-only; UNIT-TESTABLE for the argv/env/path shapes (the actual `srt-win` invocation can't run on this macOS host). The `srt-win.exe` binary itself is a separate vendored helper (out of scope, like the apply-seccomp binary) — this is the wrapper.

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/windows-sandbox-utils.js` — READ it. Reuses `env::generate_proxy_env_vars` (P4-1), `config::WindowsConfig` (P8a).

**Branch:** `parity-sandbox-runtime-p9b`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. The pure argv/env/path-resolution logic is portable + unit-tested everywhere; gate the actual `srt-win` subprocess calls + JSON parsing behind `#[cfg(target_os="windows")]` where they'd run, but keep the argv-BUILDING pure/portable so it's testable on macOS. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: consts + path resolution + group-ref + wrap

**Files:** Create `lingxi-code/sandbox-runtime/src/windows.rs`; Modify `src/lib.rs`.

- [ ] `DEFAULT_WINDOWS_GROUP_NAME = "sandbox-runtime-net"`, `DEFAULT_WINDOWS_PROXY_PORT_RANGE = (60080, 60089)`.
- [ ] `get_srt_win_path(env, repo_root) -> Result<PathBuf>` — resolution order: `SRT_WIN_PATH` env (if exists) → `<repo>/vendor/srt-win/target/release/srt-win.exe` → `<repo>/dist/vendor/srt-win/target/release/srt-win.exe`; else error with the exact "srt-win.exe not found…" message listing the looked-in paths. (Take `env`/`repo_root` as params for pure testability; a thin wrapper reads the real env + derives the repo root.)
- [ ] `group_ref_args(group: &WindowsGroupRef) -> Vec<String>` — `group_sid` set → `["--group-sid", sid]`; else `["--name", group_name.unwrap_or(DEFAULT_WINDOWS_GROUP_NAME)]`. (`WindowsGroupRef { group_name: Option<String>, group_sid: Option<String> }`.)
- [ ] `wrap_command_with_sandbox_windows(params) -> WindowsInvocation { argv: Vec<String>, env: Vec<(String,String)> }` (windows-sandbox-utils.js:303-340): `argv = [srt_win_exe, "exec", ...group_ref_args, "--"]`; shell dispatch (`bin_shell.to_lowercase()`): `pwsh` → `["pwsh.exe", "-NoProfile", "-Command", command]`; `*powershell*` → `[<SystemRoot>\\System32\\WindowsPowerShell\\v1.0\\powershell.exe, "-NoProfile", "-Command", command]`; else (cmd) → `[<SystemRoot>\\System32\\cmd.exe, "/d", "/s", "/c", command]`. `SystemRoot` from env (default `C:\\Windows`). env = generated proxy vars (`generate_proxy_env_vars(http_port, socks_port, None, Platform::Windows?...)` — NOTE: the TS calls `generateProxyEnvVars(httpProxyPort, socksProxyPort)` with NO platform arg → the GIT_SSH branch needs a platform; for Windows the TS getPlatform()==='windows' so neither the macOS-nc nor linux-socat GIT_SSH branch fires. If the Rust `generate_proxy_env_vars` requires a `Platform`, add a `Platform::Windows` variant (so the GIT_SSH branch is skipped on Windows) — minimal additive change, document) **with `TMPDIR` removed** (the TS `delete generated.TMPDIR`). The returned `env` is the generated vars (the caller merges with process env; document — or merge here faithfully).
- [ ] Tests (portable): `get_srt_win_path` picks SRT_WIN_PATH when set+exists, falls back, errors with the message; `group_ref_args` SID vs name vs default; `wrap_command_with_sandbox_windows` for cmd/powershell/pwsh → the exact argv; TMPDIR removed from env; proxy vars present. Commit (`feat(sandbox-runtime): Windows srt-win wrapper — path/group/wrap argv (P9b)`).

### Task 2: dependency + status queries

- [ ] Port `check_windows_dependencies(group_ref, sublayer_guid) -> {errors, warnings}` + `get_windows_group_status`/`get_windows_wfp_status` (the `srt-win status`/`wfp status` JSON queries). The subprocess + JSON parse is `#[cfg(target_os="windows")]`; the argv-building + the result types are portable. On non-Windows, `check_windows_dependencies` returns an error/"Windows-only" (documented) OR is cfg-gated out. Keep faithful to the TS shape (the `srt-win` arg vectors + the parsed status fields). Tests: the argv built for status/wfp queries; the JSON-result parsing (feed a sample srt-win JSON → parsed struct) — portable. Commit (`feat(sandbox-runtime): Windows dependency + group/WFP status queries (P9b)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. macOS workspace must still build (Windows subprocess code cfg-gated). Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. `wrap_command_with_sandbox_windows` argv byte-faithful for cmd/powershell/pwsh; SystemRoot default; TMPDIR removed; proxy env via generate_proxy_env_vars(Platform::Windows). path resolution + group-ref + the consts faithful.
2. Dep/status queries: argv shape + JSON parse faithful (subprocess Windows-gated). macOS workspace builds.
3. engine-mobile 0-dep; frozen empty.

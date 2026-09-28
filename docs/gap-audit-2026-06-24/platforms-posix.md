# Platform Runtime (posix) Parity Gap Audit — v2.1.186/2.1.187

Audit date: 2026-06-24
Binary: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
TS source: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
LingXi posix: `lingxi-code/platforms/posix/src/` + `crates/tools/shell/src/bash.rs`

**Confirmed gaps: 7 (HIGH: 2, MEDIUM: 3, LOW: 2)**

---

## Gap Summary Table

| # | Area | Item | Oracle (evidence) | LingXi (file:line) | Severity |
|---|------|------|-------------------|-------------------|----------|
| 1 | Shell invocation | Shell snapshot mechanism absent | `bash/ShellSnapshot.ts` + `bashProvider.ts:63-103` | `tools/shell/src/bash.rs:1204-1209` (deferred comment) | HIGH |
| 2 | Env var roster | `CLAUDE_CODE_SHELL` runtime override not honoured | `Shell.ts:75-88`: reads env, validates bash/zsh, uses it | `bash.rs:257-263`: compile-time OS constant, no env read | HIGH |
| 3 | Env var roster | `CLAUDE_CODE_DONT_INHERIT_ENV` not handled | `ShellSnapshot.ts:461-466`: strips parent env on snapshot | 0 hits in lingxi-code grep | MEDIUM |
| 4 | Env var roster | `CLAUDE_ENV_FILE` not sourced per bash command | `sessionEnvironment.ts:72-88` + `hooks.ts:925` | `hooks/src/executor.rs:824` (explicit deferred comment) | MEDIUM |
| 5 | Signal / kill | Kill sends SIGTERM+5s+SIGKILL; binary sends SIGKILL directly | `ShellCommand.ts`: `treeKill(pid, 'SIGKILL')` immediately | `kill_tree.rs:17-18`: SIGTERM → 5s grace → SIGKILL | MEDIUM |
| 6 | Env var roster | `/env` session env vars not injected into bash spawn | `bashProvider.ts:249-251`: `getSessionEnvVars()` merged per spawn | `bash.rs:1117,1211`: `env: HashMap::new()` both paths | LOW |
| 7 | Temp dir | Task output path and task ID prefix diverge | `diskOutput.ts:52`: `/tmp/claude-{uid}/{cwd}/{sessionId}/tasks/{id}.output` | `wrap.rs:80-83`: `/tmp/lingxi-task-output/{id}.out` | LOW |

---

## Confirmed Gaps

| # | Severity | Area | Description |
|---|---|---|---|
| 1 | P0 | Shell invocation | Shell snapshot mechanism entirely absent |
| 2 | P1 | Shell invocation | `CLAUDE_CODE_SHELL` runtime override not honored |
| 3 | P1 | Env var roster | `CLAUDE_CODE_DONT_INHERIT_ENV` not handled in snapshot creation path |
| 4 | P1 | Env var roster | `CLAUDE_ENV_FILE` not sourced by bash provider env overrides |
| 5 | P1 | Process tracking | Kill sequence sends SIGTERM + 5s grace before SIGKILL; TS sends SIGKILL directly |
| 6 | P2 | Env var roster | Session env vars (`/env` command vars via `getSessionEnvVars()`) not injected into bash spawn env |
| 7 | P2 | `~/.claude` dir | Task output file path convention and task ID prefix format differ |

---

## Gap Details

### Gap 1 (P0) — Shell Snapshot Mechanism Absent

**TS behavior** (`src/utils/bash/ShellSnapshot.ts`, `src/utils/shell/bashProvider.ts`):

At session start, claude-code runs `createAndSaveSnapshot(binShell)`:
1. Identifies the user's shell config file (`~/.zshrc` / `~/.bashrc`)
2. Runs `binShell -c -l <snapshotScript>` to source the user's RC file into a subprocess
3. Captures functions, shell options, aliases, PATH, and rg/find/grep integration into a snapshot file at `~/.claude/shell-snapshots/snapshot-{type}-{ts}-{rand}.sh`
4. All subsequent bash command spawns source this snapshot file instead of using `-l` (login shell):
   - With snapshot: `['-c', commandString]` (fast, no login shell overhead)
   - Without snapshot: `['-c', '-l', commandString]` (login shell as fallback)
5. The command build is: `source <snapshot> || true && <sessionEnvScript> && <disableExtglob> && eval <cmd> && pwd -P >| <cwdFile>`

**LingXi behavior** (`crates/tools/shell/src/bash.rs` lines 1206-1209):

> "the snapshot mechanism is deferred here, so `lastSnapshotFilePath` is always undefined ⇒ `skipLoginShell == false` ⇒ `-l` always added"

The bash command is assembled as: `<disableExtglob> && <command> && pwd -P >| <cwdFile>` — no snapshot sourcing, no session env script injection.

**Impact**: User shell aliases, functions, and custom shell options never take effect inside claude-code bash commands. Commands run with the login shell's default profile, not the user's interactive shell state. This is visibly different: user aliases (`ll`, `gs`, etc.) are unavailable; tools like mise/rbenv/nvm loaded by `.zshrc` are absent.

---

### Gap 2 (P1) — `CLAUDE_CODE_SHELL` Runtime Override Ignored

**TS behavior** (`src/utils/Shell.ts:75-88` — `findSuitableShell()`):

```typescript
const shellOverride = process.env.CLAUDE_CODE_SHELL
if (shellOverride) {
  const isSupported = shellOverride.includes('bash') || shellOverride.includes('zsh')
  if (isSupported && isExecutable(shellOverride)) {
    return shellOverride  // Use this shell instead
  }
}
// fallback: check SHELL env, then probe /bin/bash, /bin/zsh, etc.
```

**LingXi behavior** (`crates/tools/shell/src/bash.rs:257-263` — `resolve_shell_path()`):

```rust
pub fn resolve_shell_path() -> &'static str {
    if cfg!(target_os = "macos") { BASH_SHELL_MACOS } // "/bin/zsh"
    else { BASH_SHELL_LINUX } // "/bin/bash"
}
```

LingXi selects shell at compile-time by OS; `CLAUDE_CODE_SHELL` is never read. Any user-set `CLAUDE_CODE_SHELL` env var pointing to a custom bash/zsh path is silently ignored.

**Impact**: Users with non-standard shell installations (`/opt/homebrew/bin/bash`, custom nix shell paths) cannot override the shell. The `CLAUDE_CODE_SHELL` env var documented in claude-code has no effect in LingXi.

---

### Gap 3 (P1) — `CLAUDE_CODE_DONT_INHERIT_ENV` Not Handled

**TS behavior** (`src/utils/bash/ShellSnapshot.ts:461-466`):

When `CLAUDE_CODE_DONT_INHERIT_ENV` is truthy, snapshot creation uses an EMPTY env (not the inherited `process.env`) for the snapshot-capture shell:

```typescript
env: {
  ...((process.env.CLAUDE_CODE_DONT_INHERIT_ENV
    ? {}
    : subprocessEnv()) as typeof process.env),
  SHELL: binShell,
  GIT_EDITOR: 'true',
  CLAUDECODE: '1',
}
```

**LingXi behavior**: `CLAUDE_CODE_DONT_INHERIT_ENV` is not referenced anywhere in `lingxi-code/` (confirmed by grep). LingXi always inherits the parent process env (via `tokio::process::Command`'s default of inheriting parent env).

**Impact**: The "hermetic snapshot" mode used by claude-code-action's non-interactive invocations cannot be replicated. Users relying on isolated env capture get polluted snapshots.

Note: Since the snapshot mechanism itself is absent (Gap 1), this gap is only observable once Gap 1 is fixed.

---

### Gap 4 (P1) — `CLAUDE_ENV_FILE` Not Sourced

**TS behavior** (`src/utils/sessionEnvironment.ts:72-88` — `getSessionEnvironmentScript()`):

```typescript
const envFile = process.env.CLAUDE_ENV_FILE
if (envFile) {
  const envScript = (await readFile(envFile, 'utf8')).trim()
  if (envScript) {
    scripts.push(envScript)  // sourced before each bash command
  }
}
```

This env script is then injected into every bash spawn as part of `commandParts`:
```
source <snapshot> || true && <sessionEnvScript> && <disableExtglob> && eval <cmd>
```

**LingXi behavior**: `CLAUDE_ENV_FILE` is documented as a deferred gap in `lingxi-code/hooks/src/executor.rs:824`:
> "Full `subprocessEnv()` base-env replication and `CLAUDE_ENV_FILE` are out of B2 scope."

Not implemented in bash.rs command construction.

**Impact**: HFI trajectory runner and other external callers that pass `CLAUDE_ENV_FILE` (e.g., for venv/conda activation) won't have their env scripts sourced. Sessions with `CLAUDE_ENV_FILE` set produce different behavior in LingXi vs claude-code.

---

### Gap 5 (P1) — Kill Sequence Adds Extra SIGTERM + 5s Grace

**TS behavior** (`src/utils/ShellCommand.ts:337-343`):

```typescript
#doKill(code?: number): void {
  this.#status = 'killed'
  if (this.#childProcess.pid) {
    treeKill(this.#childProcess.pid, 'SIGKILL')  // SIGKILL directly, no grace
  }
  this.#resolveExitCode(code ?? SIGKILL)
}
```

`treeKill(pid, 'SIGKILL')` sends SIGKILL to the entire process tree immediately. The SIGTERM constant (143) at line 139 is only a numeric exit-code label passed to `#doKill(SIGTERM)`, NOT the actual signal — `#doKill` always calls `treeKill(..., 'SIGKILL')` regardless.

**LingXi behavior** (`lingxi-code/platforms/posix/src/process/kill_tree.rs:17-18`):

```rust
/// Sequence: SIGTERM → 5 s grace → SIGKILL.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);
```

`kill_tree_unix(pid)` calls `kill_tree_with_grace(pid, DEFAULT_GRACE)` which sends SIGTERM, waits 5 seconds, then sends SIGKILL.

**Impact**: On timeout or abort, LingXi takes 5 extra seconds before the child process tree is killed. This is observable as a 5-second hang between timeout and the tool returning its result. On a 2-minute default bash timeout, the actual wall-clock delay is 125 seconds rather than 120.

---

### Gap 6 (P2) — Session Env Vars Not Injected into Bash Spawn Env

**TS behavior** (`src/utils/shell/bashProvider.ts:249-251`):

```typescript
for (const [key, value] of getSessionEnvVars()) {
  env[key] = value
}
```

Session env vars (set by hook-delivered env files and sourced by `getSessionEnvironmentScript()`) are injected as explicit env overrides into each bash command spawn.

**LingXi behavior** (`crates/tools/shell/src/bash.rs:1117, 1211`):

Both foreground and background paths build `ProcessCommand { env: HashMap::new(), ... }`. The bash spawn env comes entirely from the inherited parent process env + the fixed overrides in `PosixProcess::build_command()` (CLAUDECODE, AI_AGENT, GIT_EDITOR, SHELL, CLAUDE_CODE_CHILD_SESSION). No dynamic session-scoped env vars are injected.

**Impact**: If a `SessionStart` hook writes env exports to `CLAUDE_ENV_FILE` (e.g., activating a virtualenv), or if the user uses `/env` to set a key, those vars are NOT propagated to subsequent bash commands in LingXi. Partially mitigated by the fact that the snapshot (Gap 1) also handles this in TS; once Gap 1 is fixed, Gap 6 should be co-fixed.

---

### Gap 7 (P2) — Task Output Path Convention Diverges

**TS behavior** (`src/utils/task/diskOutput.ts:50-54`):

```typescript
_taskOutputDir = join(getProjectTempDir(), getSessionId(), 'tasks')
// → /tmp/claude-{uid}/{sanitized-cwd}/{sessionId}/tasks/
```
File: `{dir}/{taskId}.output` where taskId = `'b' + 8 alphanumeric chars` (prefix `b` for `local_bash`).

**LingXi behavior** (`lingxi-code/platforms/posix/src/process/wrap.rs:79-83`):

```rust
pub fn task_output_path(task_id: &str) -> PathBuf {
    std::env::temp_dir().join("lingxi-task-output").join(format!("{task_id}.out"))
}
// Task ID: "local_bash_{nanos_hex}"
```

File: `/tmp/lingxi-task-output/local_bash_{nanos}.out`

Two sub-differences:
1. **Directory structure**: TS uses per-project/session scoping under `/tmp/claude-{uid}/`; LingXi uses a flat `/tmp/lingxi-task-output/` dir (not per-session, not per-project).
2. **Task ID prefix**: TS uses single-char prefix `b` + 8 random chars; LingXi uses `local_bash_` + hex nanoseconds.

**Impact**: The task ID shown to the model in background-task results differs. TS background result: "Command running in background with ID: b3x7k9m2". LingXi: "local_bash_17a3f2…". This affects model behavior when it tries to reference or list tasks. The file path format is purely internal (model sees the path via the Read tool note), but the different structure means permissions/sandbox allow-list entries would differ.

---

## UNCERTAIN Section

### U1 — CLAUDE_TMPDIR Fallback in Sandbox Tmp Resolution

LingXi's `sandbox-runtime/src/manager.rs:84-89` resolves sandbox tmpdir as:
```
CLAUDE_CODE_TMPDIR || CLAUDE_TMPDIR || /tmp/claude
```

The binary contains `CLAUDE_TMPDIR` as a standalone env var (without `_CODE_`). Whether TS's `getClaudeTempDir()` also falls back to `CLAUDE_TMPDIR` (vs. only `CLAUDE_CODE_TMPDIR`) could not be confirmed from the TS source alone — `filesystem.ts:333` shows only `CLAUDE_CODE_TMPDIR`. The LingXi sandbox path supports both; the TS sandbox path may not.

### U2 — TMUX Socket Isolation for `ant` USER_TYPE

TS `bashProvider.ts:220-228` injects `TMUX=<claudeTmuxEnv>` when `USER_TYPE==='ant'` and the tmux tool has been used or a `tmux` command is running. LingXi's bash spawn sends `env: HashMap::new()` so no TMUX override is injected. This is likely INERT for external users (not ant-internal deployments), but could affect tmux-isolation behavior in internal ant builds. Not investigated further as USER_TYPE=ant is Anthropic-internal.

### U3 — `CLAUDE_CODE_SIMPLE` / bare mode shell behavior

TS `isBareMode()` causes hooks, LSP, and session environment to be skipped. LingXi reads `CLAUDE_CODE_SIMPLE` for bare mode detection but it was not audited whether all bare-mode gates (specifically the skip of session environment sourcing and `CLAUDE_ENV_FILE`) are consistently applied in the bash tool path.

---

## What's Already Correct

- **Spawn env contract**: `CLAUDECODE=1`, `CLAUDE_CODE_CHILD_SESSION=1`, `GIT_EDITOR=true`, `AI_AGENT=claude-code_2-1-183_agent`, `SHELL=<binShell>` (bash provider only) — all implemented correctly.
- **Hook command env**: Hook child (source `"harness"`) correctly omits `AI_AGENT`/`GIT_EDITOR` and strips `WO()` auth denylist + `OTEL_*` sweep — implemented and tested.
- **GHA subprocess scrub**: `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` gate with full `GHA_SUBPROCESS_SCRUB` key list — implemented correctly.
- **setsid / detached process group**: Background processes use `setsid()` via `attach_setsid` — matches TS `detached: true`.
- **cwd tracking**: `pwd -P >| <cwdFile>` and readback after foreground command — matches TS `bashProvider.ts:185-187`.
- **Timeout value**: 30-minute `DEFAULT_TIMEOUT` and per-command override — matches TS.
- **extglob disable**: `shopt -u extglob` / `setopt NO_EXTENDED_GLOB` / `CLAUDE_CODE_SHELL_PREFIX` combined form — all three branches match TS.
- **O_NOFOLLOW**: Background task output file opened with `O_NOFOLLOW` flag — matches TS symlink-attack guard.
- **Git status probe**: `git --no-optional-locks status --short`, `log --oneline -n 5`, `config user.name`, 2000-char truncation, `(clean)` fallback — all match TS `m8r`.
- **CLAUDE_CODE_SHELL_PREFIX**: Combined bash+zsh extglob disable form honored — implemented in bash.rs.
- **~/.claude dir**: `$CLAUDE_CONFIG_DIR ?? $HOME/.claude` convention — implemented in migrations/global_config.rs and env_utils equivalent.

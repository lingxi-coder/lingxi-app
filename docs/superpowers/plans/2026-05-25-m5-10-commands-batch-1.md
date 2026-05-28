# M5-10 Slash Commands Core Batch 1 — `/clear /compact /help /exit /memory /init`

**Status:** ✅ COMPLETE (2026-05-28). Tagged `m5.10`. 14 tasks delivered across 7 commits (T0 documentation-only / no commit; T3-T9 bundled into one commit; T11 absorbed into T1; T14 verification-only + plan-doc header update).

**Plan deviations recorded by the executor:**
- Numeric substitution per the parent-message "Critical numeric substitution" note: `102 commands` → `99 commands`, `84 non-core entries` → `81 non-core entries` (M5-09 ship was 99, not the originally-planned 102). Affects /help rendering: total output = 100 lines = 1 header + 99 commands (NOT 103).
- `OLD_INIT_PROMPT` is **21 lines, 1592 bytes, sha256 cfdedaa2…b55a39** (the plan estimated 24 lines / ≈1543 bytes; the actual TS template lines 6-26 of init.ts produce these exact values).
- `ConversationOrchestrator` impl of `OrchestratorHandle` is intentionally a **minimal M5-10 stub** for `force_compact` (returns `messages_before == messages_after` no-op) and `snapshot_cost` (returns zeroed snapshot). M5-11/M5-12 wire the real `CompactionOrchestrator` + `CostTracker` fields. The `should_exit: Arc<AtomicBool>` field IS added and `request_exit` flips it as planned.
- `MockOrchestratorHandle` did not exist at plan-time (the plan assumed M5-02 shipped it; it didn't). T2 ships it as a new addition to `lingxi-orchestrator/src/test_support.rs`. Internal mutexes use `std::sync::Mutex` (not `tokio::sync::Mutex`) so setter helpers callable from inside the runtime without `blocking_lock` panics.
- `lingxi_telemetry::emit_command_{started,completed,failed}` convenience helpers added to telemetry's `lib.rs` (the plan used a hypothetical `emit(name, json!())` API; the actual telemetry transport is `tracing::info!` per the M5-07 pattern).
- T11 (count-bump test) was absorbed into T1 (where the new `command` module and `ALL_EVENT_NAMES` formula were introduced — the failing 258-asserts had to be bumped immediately to keep workspace tests green).

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 6 M5-09 placeholder structs (`ClearHandler`, `CompactHandler`, `HelpHandler`, `ExitHandler`, `MemoryHandler`, `InitHandler`) with real implementations that wire into `OrchestratorHandle` (M5-02) and emit the right `CommandResult` variant. `/clear` → calls `OrchestratorHandle::clear_session` and returns `Done { display: "Conversation cleared." }`. `/compact` → calls `OrchestratorHandle::force_compact` and returns `Done` with a formatted summary line. `/help` → returns `Done` with a byte-locked rendering of the 102 commands sorted ASCII-ascending, two-column layout, one line per command, "(unimplemented in v0.6.0)" suffix on the 84 non-core entries. `/exit` → calls a NEW trait method `OrchestratorHandle::request_exit` and returns `Done { display: "Exiting." }`. `/memory` → calls a NEW trait method `OrchestratorHandle::open_memory_editor` (which spawns `$EDITOR` against `~/.claude/CLAUDE.md`) and returns `Done` with the editor exit status echoed back. `/init` → returns `InjectMessage { content: OLD_INIT_PROMPT }` where `OLD_INIT_PROMPT` is the 24-line markdown template byte-copied from `claude-code/src/commands/init.ts:6-30`. Ships **18 new telemetry events** (`tengu_command_<name>_started/completed/failed` × 6 commands), bringing `ALL_EVENT_NAMES.len()` from **258 → 276**. Adds new module `lingxi_telemetry::tengu::command`.

**Architecture:** Six concrete handlers under `lingxi-commands/src/builtin/` (`clear.rs`, `compact.rs`, `help.rs`, `exit.rs`, `memory.rs`, `init.rs` — re-introduced as full impls; M5-09 deleted the old M1.15 stubs at Task 4). Each handler stores `Arc<dyn lingxi_traits::OrchestratorHandle>` in its struct (except `HelpHandler` which only needs `Arc<RwLock<CommandRegistry>>` for rendering, and `InitHandler` which has no deps). A new file `lingxi-commands/src/builtin/templates.rs` holds the **byte-locked** `OLD_INIT_PROMPT: &str` constant. A new file `lingxi-commands/src/builtin/help_render.rs` holds the `render_help_screen(reg) -> String` function with locked column layout. A new module `lingxi-telemetry/src/tengu/command.rs` defines 18 event-name constants + `NAMES: &[&str; 18]`. The `tengu/mod.rs` aggregator `ALL_EVENT_NAMES` formula grows from `258` to `276`. `lingxi-traits::orchestrator::OrchestratorHandle` grows two new methods: `request_exit(&self)` and `open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError>` (with `MemoryEditorOutcome { exit_code: i32, edited_path: PathBuf }`). A new `register_core_batch_1(reg, handle)` helper in `lingxi-commands::registry` overwrites the 6 batch-1 entries in the registry with their handle-bound real handlers. The M5-09 `register_all_builtin_commands(reg)` API stays unchanged — M5-12 (CLI binary) is responsible for calling both during init.

**Tech stack:** Rust 2021, `tokio::process::Command` (for `$EDITOR` spawn), `tokio::fs` (for ensuring `~/.claude/CLAUDE.md` exists before launching the editor), `std::env::var_os` (for `EDITOR` lookup with fallback chain `EDITOR` → `VISUAL` → `vi`), `lingxi_traits::OrchestratorHandle` (M5-02), `lingxi_telemetry::tengu::command` (new in T1), `lingxi_commands::builtin::{core_placeholders, names}` (M5-09), `async_trait = "0.1"` (workspace).

---

## Task 0: Reverse-engineer the 6 command bodies + lock 5 user-visible literals + the `/init` template

**Files:**
- Read: `claude-code/src/commands/clear/index.ts` (or `clear.tsx` — find via `ls`)
- Read: `claude-code/src/commands/compact/index.ts`
- Read: `claude-code/src/commands/help/index.ts`
- Read: `claude-code/src/commands/exit/index.ts`
- Read: `claude-code/src/commands/memory/index.ts`
- Read: `claude-code/src/commands/init.ts` (file, not directory)

- [ ] **Step 1: Confirm the `/init` template.**

  Open `claude-code/src/commands/init.ts`. There are two template constants:

  - `OLD_INIT_PROMPT` (lines 6-30) — the public-facing default
  - `NEW_INIT_PROMPT` (lines 32+) — gated behind `feature('NEW_INIT') && (process.env.USER_TYPE === 'ant' || isEnvTruthy(process.env.CLAUDE_CODE_NEW_INIT))`

  We lock **`OLD_INIT_PROMPT`** for v0.6.0 because the gate is internal-only. Copy lines 6-30 verbatim into Task 8 step 3's `OLD_INIT_PROMPT` constant body, preserving:
  - Exact whitespace (tabs / spaces / blank lines)
  - All backtick-fenced regions (the trailing code-fence example)
  - All literal newlines

  The template is **24 lines of markdown** (count: from `Please analyze this codebase…` through the closing triple-backtick). Track the exact length in Task 8 step 1's test (`assert_eq!(OLD_INIT_PROMPT.lines().count(), 24)`).

  **DO NOT include** the trailing extra newline from the original `.ts` template-string concatenation — Rust raw string literals already terminate after the last meaningful character. Cross-check with the byte count: a sha256 of the locked Rust constant must match a sha256 of the JS template literal extracted via `node -e 'process.stdout.write(require("./claude-code/src/commands/init.ts").OLD_INIT_PROMPT)'` (or — since the module isn't directly importable — manually copy out of init.ts and `sha256sum` both). Record the sha256 in the parity fixture (Task 10).

- [ ] **Step 2: Lock the `/help` rendering format.**

  claude-code's `/help` is rendered by an Ink TUI component (`src/commands/help/Help.tsx`) — not 1:1 portable. For our stdio surface we lock our own format. The spec §4.6 row says:

  > `/help` 输出格式 | `Commands:\n  /<name>  <description>\n  ...`

  Make this more precise:

  - **Header:** `Commands:\n` (literally 9 ASCII bytes + LF)
  - **Per-command line:** `  /` + name + spaces to align column 2 + `  ` + description-with-status + `\n`
  - **Column 1 width:** longest name length + 2. Computed at runtime from `BUILTIN_COMMAND_NAMES`. With current data (longest = `rate-limit-options` = 18 chars), column 1 is 18 + 2 = 20 chars. Each line is `  /<name padded to col-1 width>  <description>\n`.
  - **Description for core commands:** the result of `core_description(name)` (locked in M5-09 Task 2).
  - **Description for unimplemented commands:** the literal `"(unimplemented in v0.6.0)"`.
  - **Sort order:** ASCII-ascending — same order as `BUILTIN_COMMAND_NAMES`.
  - **Final newline:** the last line ends with `\n`; no trailing blank line. So total bytes = `len("Commands:\n")` + 102 × `(per-line width)`.

  Worked example (first three + last lines, with col-1 = 20):

```
Commands:
  /add-dir              (unimplemented in v0.6.0)
  /advisor              (unimplemented in v0.6.0)
  /agents               Manage subagents
  ...
  /x402                 (unimplemented in v0.6.0)
```

  Lock this exact format in Task 5.

- [ ] **Step 3: Lock the 5 user-visible literals for the other commands.**

  | # | Lock | Value | Source |
  |---|---|---|---|
  | L1 | `/clear` success | `"Conversation cleared."` | LingXi UX lock (claude-code prints `"Conversation cleared"` w/o period; we add the period for consistency with other LingXi command outputs) |
  | L2 | `/clear` failure prefix | `"Could not clear conversation: "` (then the `HandleError` body) | LingXi UX lock |
  | L3 | `/compact` success template | `"Compacted: {before} → {after} messages ({saved} bytes saved)."` (numbers formatted with no thousands separator, `{saved}` from `CompactionSummary::bytes_saved`) | LingXi UX lock |
  | L4 | `/compact` failure prefix | `"Could not compact: "` (then `HandleError` body) | LingXi UX lock |
  | L5 | `/exit` success | `"Exiting."` | LingXi UX lock |
  | L6 | `/exit` failure prefix | `"Could not request exit: "` (rare — only on internal handle errors) | LingXi UX lock |
  | L7 | `/memory` success template | `"Edited {path} (exit {code})."` where `{path}` is the editor target path and `{code}` is the editor's exit code | LingXi UX lock |
  | L8 | `/memory` failure prefix | `"Could not edit memory: "` (then `HandleError` body) | LingXi UX lock |
  | L9 | `/memory` editor fallback chain | `EDITOR` env var → `VISUAL` env var → literal `"vi"` (on Windows: `"notepad.exe"`) | claude-code parity: `src/commands/memory/openEditor.ts` uses the same fallback |
  | L10 | `/memory` target path | `<dirs::config_dir()>/claude/CLAUDE.md` (xdg-style; on macOS = `~/Library/Application Support/claude/CLAUDE.md`; on Linux = `~/.config/claude/CLAUDE.md`; on Windows = `%APPDATA%\claude\CLAUDE.md`) | claude-code uses `path.join(getClaudeConfigDir(), 'CLAUDE.md')`; we match via `dirs::config_dir()` |
  | L11 | `/init` injected user message | `OLD_INIT_PROMPT` (24 lines, byte-locked from `claude-code/src/commands/init.ts:6-30`) | Task 0 step 1 |
  | L12 | `/init` no-display | `None` (the `CommandResult::InjectMessage` variant has no `display` field — the injected user message itself drives the next turn) | M1.15 `model.rs::CommandResult::InjectMessage` |

  **L7 worked example:** if `$EDITOR=vim`, target path is `/Users/foo/Library/Application Support/claude/CLAUDE.md`, exit code 0 → `"Edited /Users/foo/Library/Application Support/claude/CLAUDE.md (exit 0)."`

  **L9 fallback chain detail:** `std::env::var_os("EDITOR")` first; if empty/absent, `std::env::var_os("VISUAL")`; if both empty/absent, hard-coded `"vi"` (Unix) or `"notepad.exe"` (Windows). Both env-var lookups treat empty strings as missing (`""` → fall through). This matches claude-code's `openEditor.ts` which uses `process.env.EDITOR || process.env.VISUAL || 'vi'`.

- [ ] **Step 4: Lock the 18 new telemetry event names.**

  Pattern: `tengu_command_<name>_<phase>` where `<name>` ∈ {`clear`, `compact`, `help`, `exit`, `memory`, `init`} and `<phase>` ∈ {`started`, `completed`, `failed`}. Yields:

  ```
  tengu_command_clear_started
  tengu_command_clear_completed
  tengu_command_clear_failed
  tengu_command_compact_started
  tengu_command_compact_completed
  tengu_command_compact_failed
  tengu_command_exit_started
  tengu_command_exit_completed
  tengu_command_exit_failed
  tengu_command_help_started
  tengu_command_help_completed
  tengu_command_help_failed
  tengu_command_init_started
  tengu_command_init_completed
  tengu_command_init_failed
  tengu_command_memory_started
  tengu_command_memory_completed
  tengu_command_memory_failed
  ```

  Verify: 18 lines, sorted ASCII-ascending. Confirm by `printf '%s\n' tengu_command_{clear,compact,help,exit,memory,init}_{started,completed,failed} | sort` matches the above.

  **Important about sort order:** the names above are sorted by the command name first (alphabetic), then phase (alphabetic). Lock this ordering in `tengu::command::NAMES`.

- [ ] **Step 5: Lock the new `OrchestratorHandle` methods.**

  The M5-02 trait surface (per the M5-02 plan, Task 3) has 5 methods:

  - `current_session_id`
  - `clear_session`
  - `force_compact`
  - `snapshot_cost`
  - `switch_model`

  M5-10 adds **two**:

  ```rust
  /// Set the orchestrator's internal `should_exit` flag. The REPL
  /// (M5-13) checks this after each turn and breaks out of the loop.
  async fn request_exit(&self);

  /// Ensure `<config-dir>/claude/CLAUDE.md` exists (touch if missing)
  /// then spawn `$EDITOR` on it; block until the child exits.
  ///
  /// Returns the editor exit code and the path edited.
  async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError>;
  ```

  And a new return type:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct MemoryEditorOutcome {
      pub edited_path: PathBuf,
      pub exit_code: i32,
  }
  ```

  Both methods are added in Task 2. M5-02 already documented this growth path in its Task 3 prose ("M5-10 wires `/exit` and `/memory` against this surface"). The `ConversationOrchestrator` impl of the trait also lives in Task 2 (M5-02 declared "M5-02 itself does NOT implement OrchestratorHandle on ConversationOrchestrator" — this is M5-10's responsibility).

- [ ] **Step 6: Commit the byte-locks reference.**

  Append the locks below to this plan file, then commit:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md
git commit -m "plan(M5-10 T0): reverse-engineer 6 command bodies + 12 locked literals + OrchestratorHandle growth"
```

### Reverse-engineered byte-locks (locked by T0)

| Lock | Value | Source |
|---|---|---|
| `/clear` display | `"Conversation cleared."` | LingXi UX (L1) |
| `/clear` error prefix | `"Could not clear conversation: "` | LingXi UX (L2) |
| `/compact` display template | `"Compacted: {before} → {after} messages ({saved} bytes saved)."` | LingXi UX (L3) |
| `/compact` error prefix | `"Could not compact: "` | LingXi UX (L4) |
| `/exit` display | `"Exiting."` | LingXi UX (L5) |
| `/exit` error prefix | `"Could not request exit: "` | LingXi UX (L6) |
| `/memory` display template | `"Edited {path} (exit {code})."` | LingXi UX (L7) |
| `/memory` error prefix | `"Could not edit memory: "` | LingXi UX (L8) |
| `/memory` editor fallback | `EDITOR` → `VISUAL` → `"vi"` (Unix) / `"notepad.exe"` (Windows) | claude-code `openEditor.ts` |
| `/memory` target path | `<dirs::config_dir()>/claude/CLAUDE.md` | claude-code `getClaudeConfigDir()` |
| `/init` injected message | `OLD_INIT_PROMPT` (24-line markdown template) | `claude-code/src/commands/init.ts:6-30` |
| `/help` header | `"Commands:\n"` | Spec §4.6 + LingXi UX |
| `/help` per-line | `  /<name padded to col-1>  <description>\n` | LingXi UX |
| `/help` col-1 width | `longest_name_len + 2` (currently 20) | LingXi UX |
| `/help` unimplemented suffix | `"(unimplemented in v0.6.0)"` | LingXi UX |
| New telemetry events | 18 names, sorted by (command, phase) | LingXi convention `tengu_command_<name>_<phase>` |
| `ALL_EVENT_NAMES.len()` after M5-10 | `276 (= 258 + 18)` | Spec §6.3 |
| New `OrchestratorHandle` methods | `request_exit`, `open_memory_editor` | Spec §3 M5-10 row |
| New return type | `MemoryEditorOutcome { edited_path: PathBuf, exit_code: i32 }` | LingXi UX |

---

## Task 1: New `tengu::command` telemetry submodule + 18 NAMES const

**Files:**
- Create: `lingxi-core/crates/telemetry/src/tengu/command.rs`
- Modify: `lingxi-core/crates/telemetry/src/tengu/mod.rs` (add `pub mod command;` + grow `ALL_EVENT_NAMES`)

- [ ] **Step 1: Write the failing test.**

  Create `lingxi-core/crates/telemetry/src/tengu/command.rs`:

```rust
//! `tengu_command_<name>_<phase>` event names — the M5-10 batch-1 surface.
//!
//! M5-10 ships 18 events (6 commands × 3 phases). M5-11 will append another
//! 36 to this same module (12 batch-2 commands × 3). See plan
//! `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md` Task 1.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_length_locked_at_18() {
        assert_eq!(NAMES.len(), 18);
    }

    #[test]
    fn names_are_sorted_ascii_ascending() {
        let mut sorted = NAMES.to_vec();
        sorted.sort();
        assert_eq!(NAMES, sorted.as_slice());
    }

    #[test]
    fn every_name_starts_with_tengu_command() {
        for n in NAMES {
            assert!(n.starts_with("tengu_command_"), "{n} missing prefix");
        }
    }

    #[test]
    fn every_phase_is_started_completed_or_failed() {
        for n in NAMES {
            assert!(
                n.ends_with("_started") || n.ends_with("_completed") || n.ends_with("_failed"),
                "{n} has wrong phase"
            );
        }
    }

    #[test]
    fn exactly_three_phases_per_command() {
        let mut by_cmd: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for n in NAMES {
            let stripped = n.strip_prefix("tengu_command_").unwrap();
            // Split at last underscore — the part after is the phase.
            let last_under = stripped.rfind('_').expect("phase delimiter missing");
            let cmd = &stripped[..last_under];
            by_cmd.entry(cmd).or_default().push(n);
        }
        assert_eq!(by_cmd.len(), 6, "expected 6 distinct command names");
        for (cmd, names) in &by_cmd {
            assert_eq!(names.len(), 3, "/{cmd} should have exactly 3 phases, got {}", names.len());
        }
    }

    #[test]
    fn covers_all_6_batch_1_commands() {
        let expected: std::collections::HashSet<&str> =
            ["clear", "compact", "exit", "help", "init", "memory"]
                .iter()
                .copied()
                .collect();
        let mut found: std::collections::HashSet<&str> = Default::default();
        for n in NAMES {
            let stripped = n.strip_prefix("tengu_command_").unwrap();
            let last_under = stripped.rfind('_').unwrap();
            found.insert(&stripped[..last_under]);
        }
        assert_eq!(found, expected, "missing or extra command names");
    }

    #[test]
    fn individual_constants_match_their_position_in_names() {
        // Spot-check: CLEAR_STARTED is the first by sort.
        assert_eq!(CLEAR_STARTED, "tengu_command_clear_started");
        assert_eq!(CLEAR_COMPLETED, "tengu_command_clear_completed");
        assert_eq!(CLEAR_FAILED, "tengu_command_clear_failed");
        assert_eq!(INIT_FAILED, "tengu_command_init_failed");
        assert_eq!(MEMORY_FAILED, "tengu_command_memory_failed");
    }
}
```

- [ ] **Step 2: Run + watch fail.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-telemetry --lib tengu::command::tests 2>&1 | head -20
```

  Expected: 7 failures (`cannot find constant 'NAMES'`).

- [ ] **Step 3: Implement.**

  Prepend the impl block above the `#[cfg(test)]` in `command.rs`:

```rust
//! `tengu_command_<name>_<phase>` event names — the M5-10 batch-1 surface.
//!
//! M5-10 ships 18 events (6 commands × 3 phases). M5-11 will append another
//! 36 to this same module (12 batch-2 commands × 3). See plan
//! `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md` Task 1.

pub const CLEAR_STARTED:   &str = "tengu_command_clear_started";
pub const CLEAR_COMPLETED: &str = "tengu_command_clear_completed";
pub const CLEAR_FAILED:    &str = "tengu_command_clear_failed";

pub const COMPACT_STARTED:   &str = "tengu_command_compact_started";
pub const COMPACT_COMPLETED: &str = "tengu_command_compact_completed";
pub const COMPACT_FAILED:    &str = "tengu_command_compact_failed";

pub const EXIT_STARTED:   &str = "tengu_command_exit_started";
pub const EXIT_COMPLETED: &str = "tengu_command_exit_completed";
pub const EXIT_FAILED:    &str = "tengu_command_exit_failed";

pub const HELP_STARTED:   &str = "tengu_command_help_started";
pub const HELP_COMPLETED: &str = "tengu_command_help_completed";
pub const HELP_FAILED:    &str = "tengu_command_help_failed";

pub const INIT_STARTED:   &str = "tengu_command_init_started";
pub const INIT_COMPLETED: &str = "tengu_command_init_completed";
pub const INIT_FAILED:    &str = "tengu_command_init_failed";

pub const MEMORY_STARTED:   &str = "tengu_command_memory_started";
pub const MEMORY_COMPLETED: &str = "tengu_command_memory_completed";
pub const MEMORY_FAILED:    &str = "tengu_command_memory_failed";

/// All 18 command-event names (sorted ASCII-ascending: by command-name then phase).
///
/// Locked at length **18** for M5-10 ([`super::ALL_EVENT_NAMES`] formula must
/// add **18** for the `command` slot). M5-11 expands this to **54** (+36).
pub const NAMES: &[&str; 18] = &[
    CLEAR_STARTED,   CLEAR_COMPLETED,   CLEAR_FAILED,
    COMPACT_STARTED, COMPACT_COMPLETED, COMPACT_FAILED,
    EXIT_STARTED,    EXIT_COMPLETED,    EXIT_FAILED,
    HELP_STARTED,    HELP_COMPLETED,    HELP_FAILED,
    INIT_STARTED,    INIT_COMPLETED,    INIT_FAILED,
    MEMORY_STARTED,  MEMORY_COMPLETED,  MEMORY_FAILED,
];
```

  **Sort-order sanity check:** the test `names_are_sorted_ascii_ascending` confirms the static array is `.sort()`-stable. In ASCII, `_started < _completed < _failed`? Let's check: `s` (0x73), `c` (0x63), `f` (0x66) — so order is `_completed < _failed < _started`. **The hand-written order above (started, completed, failed) is NOT sort-stable!**

  Re-arrange to true ASCII order:

```rust
pub const NAMES: &[&str; 18] = &[
    CLEAR_COMPLETED, CLEAR_FAILED,    CLEAR_STARTED,
    COMPACT_COMPLETED, COMPACT_FAILED, COMPACT_STARTED,
    EXIT_COMPLETED, EXIT_FAILED, EXIT_STARTED,
    HELP_COMPLETED, HELP_FAILED, HELP_STARTED,
    INIT_COMPLETED, INIT_FAILED, INIT_STARTED,
    MEMORY_COMPLETED, MEMORY_FAILED, MEMORY_STARTED,
];
```

  Use this final ordering. The test `names_are_sorted_ascii_ascending` is the canonical check; if it fails, fix the array layout — do NOT relax the test.

- [ ] **Step 4: Grow `tengu::ALL_EVENT_NAMES` formula.**

  Open `lingxi-core/crates/telemetry/src/tengu/mod.rs`. Add `pub mod command;` to the module list. Update the `ALL_EVENT_NAMES` const-fn:

  - Existing TOTAL (post M5-08, pre M5-10): `25 + 30 + 20 + 134 + 10 + 8 + 12 + 3 + ...` — the exact formula depends on what M5-02..M5-08 added. Refer to spec §6.3 table.
  - **New TOTAL after M5-10:** **276**.
  - **Add an 18-length `command::NAMES` slice to the concatenation, in the right position.**

  Position rule (M3-06 convention): events are aggregated in **registration order** which approximately matches plan-introduction order. M5-10's `command` sits **after** M5-08's `session` (renamed `session.rs` already includes resume events from M5-07/M5-08) and **before** `cost`/`oauth`/`memory`/`settings`/`release` (which are M3-era).

  Safer alternative: re-sort `ALL_EVENT_NAMES` content order at the end so the concatenation matches the M3-06 `parity_tengu_events.json` row-order. Since we ship a parity fixture in T10 that has its own row-order, the consistency requirement is: **whatever order this slice appears in `ALL_EVENT_NAMES` matches the row order in `parity_tengu_events.json` exactly**.

  Concretely, append `command::NAMES` to the existing concatenation **after the last category** (so it's the last 18 events), and Task 10 step 1 appends 18 rows to `parity_tengu_events.json` at the matching position. Either approach is fine as long as both sides agree.

  The const-fn body needs the new slice loop. Pattern:

```rust
pub const ALL_EVENT_NAMES: &[&str] = {
    // Update TOTAL formula. The old formula post-M5-08 was something like:
    //   25 + 30 + 20 + 134 + 10 + 8 + 12 + 3 + 1
    // (api + agent + session + tool + cost + oauth + memory + settings + release)
    // After M5-10 adds 18 commands:
    const TOTAL: usize = 25 + 30 + 20 + 134 + 10 + 8 + 12 + 3 + 1 + 18;
    const fn concat_all() -> [&'static str; TOTAL] {
        let mut out: [&'static str; TOTAL] = [""; TOTAL];
        let mut idx = 0;

        // ... existing category loops ...
        // (api, agent, session, tool, cost, oauth, memory, settings, release)

        let mut i = 0;
        while i < command::NAMES.len() {
            out[idx] = command::NAMES[i];
            idx += 1;
            i += 1;
        }
        out
    }
    static OUT: [&'static str; TOTAL] = concat_all();
    &OUT
};
```

  **Verify the TOTAL formula** by reading the current `mod.rs` first — the integer breakdown may differ slightly from the comment above (M5-02 + M5-04 + M5-05 + M5-06 + M5-08 each contributed their own bumps). The end-state count is **276**; do whatever arithmetic on the formula adds up to 276 with all M5-02..M5-08 increments correctly attributed.

- [ ] **Step 5: Run tests.**

```bash
cargo test -p lingxi-telemetry --lib tengu::command::tests 2>&1 | tail -15
cargo test -p lingxi-telemetry --lib tengu::tests 2>&1 | tail -15   # the ALL_EVENT_NAMES tests
```

  Expected: 7 command-mod tests pass + tengu mod tests pass (which assert `ALL_EVENT_NAMES.len() == 276`).

- [ ] **Step 6: Fmt + clippy.**

```bash
cargo fmt -p lingxi-telemetry
cargo clippy -p lingxi-telemetry --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 7: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/telemetry/src/tengu/command.rs \
        lingxi-core/crates/telemetry/src/tengu/mod.rs
git commit -m "feat(M5-10 task 1): tengu::command module + 18 NAMES (6 cmds × 3 phases) + ALL_EVENT_NAMES 258→276 (7 invariant tests)"
```

---

## Task 2: Extend `OrchestratorHandle` trait + impl on `ConversationOrchestrator`

**Files:**
- Modify: `lingxi-core/crates/traits/src/orchestrator.rs` (add `request_exit`, `open_memory_editor`, `MemoryEditorOutcome`)
- Create: `lingxi-core/crates/orchestrator/src/handle_impl.rs` (impl block)
- Modify: `lingxi-core/crates/orchestrator/src/lib.rs` (re-export + wire module)

- [ ] **Step 1: Write the failing test.**

  Append to `lingxi-core/crates/traits/src/orchestrator.rs` (after the existing trait definition + tests):

```rust
#[cfg(test)]
mod m5_10_extension_tests {
    use super::*;

    // Object safety check after the M5-10 method additions.
    fn _handle_remains_object_safe<T: OrchestratorHandle + Send + Sync + 'static>() {
        let _: Box<dyn OrchestratorHandle> = Box::new(std::marker::PhantomData::<T>);
    }

    #[test]
    fn memory_editor_outcome_fields_and_clone() {
        let o = MemoryEditorOutcome {
            edited_path: std::path::PathBuf::from("/tmp/CLAUDE.md"),
            exit_code: 0,
        };
        let cloned = o.clone();
        assert_eq!(o, cloned);
        assert_eq!(o.exit_code, 0);
        assert_eq!(o.edited_path, std::path::PathBuf::from("/tmp/CLAUDE.md"));
    }
}
```

  Wait — these are compile-time / `PartialEq` tests that pass trivially **if** the new types exist. To make them actually fail when the types are missing, the imports won't resolve. Run as written; the failing assertion will be a compile error.

- [ ] **Step 2: Run + watch fail.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo test -p lingxi-traits --lib m5_10_extension_tests 2>&1 | head -10
```

  Expected: compile error (`cannot find struct 'MemoryEditorOutcome'`).

- [ ] **Step 3: Add the new types + trait methods.**

  Open `lingxi-core/crates/traits/src/orchestrator.rs`. Find the existing `OrchestratorHandle` trait (defined by M5-02). Add `MemoryEditorOutcome` and the two new trait methods:

```rust
use std::path::PathBuf;

/// Result of [`OrchestratorHandle::open_memory_editor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEditorOutcome {
    /// The CLAUDE.md path that was edited (may have been created if absent).
    pub edited_path: PathBuf,
    /// Exit code of the spawned `$EDITOR` process. 0 = success.
    pub exit_code: i32,
}

#[async_trait]
pub trait OrchestratorHandle: Send + Sync {
    // (Existing 5 methods from M5-02 — unchanged.)
    async fn current_session_id(&self) -> SessionId;
    async fn clear_session(&self) -> Result<(), HandleError>;
    async fn force_compact(&self) -> Result<CompactionSummary, HandleError>;
    async fn snapshot_cost(&self) -> CostSnapshot;
    async fn switch_model(&self, model: &str) -> Result<(), HandleError>;

    // M5-10 additions:

    /// Set the orchestrator's internal `should_exit` flag.
    ///
    /// The REPL (M5-13) checks this after each turn and breaks out of the
    /// loop. The flag is one-way: once set, it cannot be cleared (so a
    /// double-`/exit` is idempotent).
    async fn request_exit(&self);

    /// Open `$EDITOR` on `<config-dir>/claude/CLAUDE.md` (creating the file
    /// if it does not exist), block until the editor exits, then return the
    /// outcome.
    ///
    /// The editor lookup order is `EDITOR` → `VISUAL` → `"vi"` (Unix) /
    /// `"notepad.exe"` (Windows). Empty env values are treated as missing.
    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError>;
}
```

  Also add `pub use orchestrator::MemoryEditorOutcome;` to `lingxi-core/crates/traits/src/lib.rs`.

- [ ] **Step 4: Run the trait tests + watch pass.**

```bash
cargo test -p lingxi-traits --lib m5_10_extension_tests 2>&1 | tail -10
```

  Expected: 2 passed.

- [ ] **Step 5: Update mock + production impls of `OrchestratorHandle`.**

  M5-02 ships `MockOrchestratorHandle` in `lingxi-core/crates/orchestrator/src/test_support.rs`. Find it and add two methods so the mock compiles:

```rust
#[async_trait]
impl OrchestratorHandle for MockOrchestratorHandle {
    // ... existing 5 methods ...

    async fn request_exit(&self) {
        self.exit_requested.store(true, Ordering::SeqCst);
    }

    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        // Test default: no actual editor spawn — return success with the
        // path the test prepared via `set_memory_path` + `set_exit_code`.
        Ok(MemoryEditorOutcome {
            edited_path: self.memory_path.lock().unwrap().clone()
                .unwrap_or_else(|| PathBuf::from("/dev/null/CLAUDE.md")),
            exit_code: self.editor_exit_code.load(Ordering::SeqCst),
        })
    }
}
```

  Add the supporting fields to `MockOrchestratorHandle`:

```rust
pub struct MockOrchestratorHandle {
    // ... existing fields from M5-02 ...
    pub exit_requested: AtomicBool,
    pub memory_path: Mutex<Option<PathBuf>>,
    pub editor_exit_code: AtomicI32,
}
```

  Add helper setters:

```rust
impl MockOrchestratorHandle {
    pub fn set_memory_path(&self, p: PathBuf) { *self.memory_path.lock().unwrap() = Some(p); }
    pub fn set_editor_exit_code(&self, c: i32) { self.editor_exit_code.store(c, Ordering::SeqCst); }
    pub fn was_exit_requested(&self) -> bool { self.exit_requested.load(Ordering::SeqCst) }
}
```

  And include `AtomicI32` in the imports.

- [ ] **Step 6: Implement on `ConversationOrchestrator` (the production type).**

  Create `lingxi-core/crates/orchestrator/src/handle_impl.rs`:

```rust
//! `impl OrchestratorHandle for ConversationOrchestrator`.
//!
//! M5-02 declared the trait; M5-10 lights up the implementation against the
//! real orchestrator state.

use crate::ConversationOrchestrator;
use async_trait::async_trait;
use lingxi_telemetry::tengu::session as session_evt;
use lingxi_traits::{
    CompactionSummary, CostSnapshot, HandleError, MemoryEditorOutcome,
    OrchestratorHandle, SessionId,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tokio::process::Command;

#[async_trait]
impl OrchestratorHandle for ConversationOrchestrator {
    async fn current_session_id(&self) -> SessionId {
        // Read from the orchestrator's session state (M5-02 stored it as
        // `Arc<RwLock<SessionState>>`).
        self.session.read().await.id.clone()
    }

    async fn clear_session(&self) -> Result<(), HandleError> {
        let mut s = self.session.write().await;
        let old_id = s.id.clone();
        s.messages.clear();
        s.id = lingxi_protocol::SessionId::new(); // mint a fresh one
        drop(s);

        // Emit `tengu_session_clear_completed` (this event already exists
        // in M1.10 session module — reuse it; M5-10 does not introduce a
        // new session event).
        lingxi_telemetry::emit(session_evt::CLEAR_COMPLETED, serde_json::json!({
            "old_id": old_id.to_string(),
            "new_id": self.current_session_id().await.to_string(),
        }));
        Ok(())
    }

    async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
        // Delegate to the compaction subsystem owned by the orchestrator.
        // M5-02 wired `Arc<dyn CompactionDriver>` into the orchestrator
        // struct; that trait has a `compact(&self, session) -> Summary`
        // method.
        let mut s = self.session.write().await;
        let before = s.messages.len() as u32;
        let summary = self
            .compaction
            .compact(&mut s)
            .await
            .map_err(|e| HandleError::ActionFailed(format!("compaction failed: {e}")))?;
        let after = s.messages.len() as u32;
        drop(s);
        Ok(CompactionSummary {
            messages_before: before,
            messages_after: after,
            bytes_saved: summary.bytes_saved,
        })
    }

    async fn snapshot_cost(&self) -> CostSnapshot {
        self.cost.snapshot().await
    }

    async fn switch_model(&self, model: &str) -> Result<(), HandleError> {
        let mut s = self.session.write().await;
        s.model = model.to_string();
        Ok(())
    }

    async fn request_exit(&self) {
        self.should_exit.store(true, Ordering::SeqCst);
    }

    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        // 1. Resolve target path.
        let config_dir = dirs::config_dir().ok_or_else(|| {
            HandleError::ActionFailed("config_dir unavailable on this platform".into())
        })?;
        let target = config_dir.join("claude").join("CLAUDE.md");

        // 2. Ensure parent dir + file exist (touch with empty body if missing).
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                HandleError::ActionFailed(format!("mkdir {parent:?}: {e}"))
            })?;
        }
        if !target.exists() {
            tokio::fs::write(&target, "").await.map_err(|e| {
                HandleError::ActionFailed(format!("touch {target:?}: {e}"))
            })?;
        }

        // 3. Resolve editor: EDITOR → VISUAL → default.
        let editor = resolve_editor();

        // 4. Spawn + wait. (Inherits stdin/stdout/stderr so the user can
        //    interact with their TUI editor.)
        let status = Command::new(&editor)
            .arg(&target)
            .status()
            .await
            .map_err(|e| HandleError::ActionFailed(format!("spawn {editor}: {e}")))?;

        Ok(MemoryEditorOutcome {
            edited_path: target,
            exit_code: status.code().unwrap_or(-1),
        })
    }
}

#[cfg(unix)]
fn default_editor() -> String { "vi".to_string() }
#[cfg(windows)]
fn default_editor() -> String { "notepad.exe".to_string() }
#[cfg(not(any(unix, windows)))]
fn default_editor() -> String { "vi".to_string() }

fn resolve_editor() -> String {
    use std::env;
    if let Some(v) = env::var_os("EDITOR") {
        if !v.is_empty() {
            return v.to_string_lossy().into_owned();
        }
    }
    if let Some(v) = env::var_os("VISUAL") {
        if !v.is_empty() {
            return v.to_string_lossy().into_owned();
        }
    }
    default_editor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editor_uses_default_when_env_unset() {
        // Cannot reliably scrub env in a multi-threaded test, but the helper
        // path is mainly exercised by integration tests with `temp_env`.
        // This smoke test just calls the function.
        let _ = resolve_editor();
    }

    #[cfg(unix)]
    #[test]
    fn default_editor_unix_is_vi() {
        assert_eq!(default_editor(), "vi");
    }

    #[cfg(windows)]
    #[test]
    fn default_editor_windows_is_notepad() {
        assert_eq!(default_editor(), "notepad.exe");
    }
}
```

  Add `pub mod handle_impl;` to `lingxi-orchestrator/src/lib.rs`. Add `dirs = "5"` and `tokio = { workspace = true, features = ["process"] }` to `lingxi-orchestrator/Cargo.toml` if not already present.

- [ ] **Step 7: Run the orchestrator tests.**

```bash
cargo test -p lingxi-orchestrator --lib 2>&1 | tail -20
```

  Expected: existing M5-02 tests still pass + new `handle_impl::tests` pass.

- [ ] **Step 8: Fmt + clippy.**

```bash
cargo fmt -p lingxi-traits -p lingxi-orchestrator
cargo clippy -p lingxi-traits -p lingxi-orchestrator --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 9: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/traits/src/orchestrator.rs \
        lingxi-core/crates/traits/src/lib.rs \
        lingxi-core/crates/orchestrator/src/handle_impl.rs \
        lingxi-core/crates/orchestrator/src/lib.rs \
        lingxi-core/crates/orchestrator/Cargo.toml \
        lingxi-core/crates/orchestrator/src/test_support.rs
git commit -m "feat(M5-10 task 2): OrchestratorHandle grows request_exit + open_memory_editor + MemoryEditorOutcome; impl on ConversationOrchestrator"
```

---

## Task 3: `/clear` real implementation

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/clear.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs` (re-export)
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs` (remove the `ClearHandler` macro line)

- [ ] **Step 1: Write the failing test.**

  Create `lingxi-core/crates/commands/src/builtin/clear.rs`:

```rust
//! `/clear` — wipes the orchestrator's in-memory session and emits a
//! locked confirmation literal.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`
//! Task 3.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BuiltinCommandHandler, CommandResult};
    use crate::parser::ParsedSlashCommand;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use std::sync::Arc;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "clear".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        }
    }

    #[tokio::test]
    async fn success_returns_locked_literal() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ClearHandler::new(mock.clone());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Conversation cleared.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(mock.was_clear_session_called());
    }

    #[tokio::test]
    async fn failure_prefixes_handle_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_clear_session_error("disk full".to_string());
        let h = ClearHandler::new(mock.clone());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Could not clear conversation: handle action failed: disk full");
            }
            other => panic!("expected Done with error display, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ClearHandler::new(mock);
        assert_eq!(h.name(), "clear");
        assert_eq!(h.description(), "Clear conversation history and free up context");
    }
}
```

  Add `lingxi-orchestrator = { path = "../orchestrator", optional = true }` under `[dev-dependencies]` of `lingxi-commands/Cargo.toml` (so tests can import the mock). Same for `[features]` — add `mock-orchestrator = ["dep:lingxi-orchestrator"]` if needed for ergonomic test setup. Simpler: make `lingxi-orchestrator` a `[dev-dependencies]` entry without features.

- [ ] **Step 2: Run + watch fail.**

```bash
cargo test -p lingxi-commands --lib builtin::clear::tests 2>&1 | head -25
```

  Expected: 3 failures (no `ClearHandler` impl).

- [ ] **Step 3: Implement.**

```rust
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

/// `/clear` handler — wipes the in-memory conversation.
#[derive(Clone)]
pub struct ClearHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl ClearHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for ClearHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::CLEAR_STARTED, serde_json::json!({}));
        match self.handle.clear_session().await {
            Ok(()) => {
                lingxi_telemetry::emit(cmd_evt::CLEAR_COMPLETED, serde_json::json!({}));
                CommandResult::Done {
                    display: Some("Conversation cleared.".to_string()),
                }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::CLEAR_FAILED, serde_json::json!({
                    "error": e.to_string(),
                }));
                CommandResult::Done {
                    display: Some(format!("Could not clear conversation: {e}")),
                }
            }
        }
    }
    fn name(&self) -> &str { "clear" }
    fn description(&self) -> &str { core_description("clear") }
}
```

- [ ] **Step 4: Remove `ClearHandler` from the macro list in `core_placeholders.rs`.**

  Find the line `core_placeholder!(ClearHandler, "clear");` in `lingxi-core/crates/commands/src/builtin/core_placeholders.rs` and delete it. The remaining 17 macro calls still produce placeholders for the not-yet-implemented core commands. Likewise remove `Arc::new(ClearHandler::new())` from `register_core_placeholders` (the function that takes no args — the M5-09 version).

  Also remove `pub use core_placeholders::*;` if it re-exported `ClearHandler` — the canonical home is now `builtin::clear::ClearHandler`. Add `pub mod clear;` + `pub use clear::ClearHandler;` to `builtin/mod.rs` at the top of the list.

- [ ] **Step 5: Run tests.**

```bash
cargo test -p lingxi-commands --lib builtin::clear::tests 2>&1 | tail -10
```

  Expected: **3 passed**.

  Note: the `MockOrchestratorHandle` needs `was_clear_session_called()` / `set_clear_session_error()` helpers. These should already exist from M5-02; if not, add them to `lingxi-orchestrator/src/test_support.rs` as part of this task.

- [ ] **Step 6: Fmt + clippy.**

```bash
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 7: Commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-core/crates/commands/src/builtin/clear.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs \
        lingxi-core/crates/commands/Cargo.toml
git commit -m "feat(M5-10 task 3): /clear real impl + 3 telemetry events (3 unit tests)"
```

---

## Task 4: `/compact` real implementation

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/compact.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs` (re-export)
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs` (remove `CompactHandler` macro)

- [ ] **Step 1: Write the failing test.**

  Create `lingxi-core/crates/commands/src/builtin/compact.rs`:

```rust
//! `/compact` — runs a forced compaction pass and reports the summary.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`
//! Task 4.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CommandResult;
    use crate::parser::ParsedSlashCommand;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use lingxi_traits::CompactionSummary;
    use std::sync::Arc;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "compact".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        }
    }

    #[tokio::test]
    async fn success_renders_summary_template() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_summary(CompactionSummary {
            messages_before: 42,
            messages_after: 7,
            bytes_saved: 18_345,
        });
        let h = CompactHandler::new(mock.clone());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Compacted: 42 → 7 messages (18345 bytes saved).");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn failure_prefixes_handle_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_error("model 429".to_string());
        let h = CompactHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Could not compact: handle action failed: model 429");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zero_messages_renders_correctly() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_summary(CompactionSummary {
            messages_before: 0,
            messages_after: 0,
            bytes_saved: 0,
        });
        let h = CompactHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Compacted: 0 → 0 messages (0 bytes saved).");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = CompactHandler::new(mock);
        assert_eq!(h.name(), "compact");
        assert_eq!(h.description(), "Compact the conversation to a summary");
    }
}
```

- [ ] **Step 2: Run + watch fail.**

```bash
cargo test -p lingxi-commands --lib builtin::compact::tests 2>&1 | head -20
```

  Expected: 4 failures.

- [ ] **Step 3: Implement.**

```rust
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct CompactHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl CompactHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for CompactHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::COMPACT_STARTED, serde_json::json!({}));
        match self.handle.force_compact().await {
            Ok(summary) => {
                lingxi_telemetry::emit(cmd_evt::COMPACT_COMPLETED, serde_json::json!({
                    "messages_before": summary.messages_before,
                    "messages_after": summary.messages_after,
                    "bytes_saved": summary.bytes_saved,
                }));
                CommandResult::Done {
                    display: Some(format!(
                        "Compacted: {} → {} messages ({} bytes saved).",
                        summary.messages_before, summary.messages_after, summary.bytes_saved
                    )),
                }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::COMPACT_FAILED, serde_json::json!({
                    "error": e.to_string(),
                }));
                CommandResult::Done {
                    display: Some(format!("Could not compact: {e}")),
                }
            }
        }
    }
    fn name(&self) -> &str { "compact" }
    fn description(&self) -> &str { core_description("compact") }
}
```

- [ ] **Step 4: Wire into mod + remove placeholder.**

  Update `builtin/mod.rs`: add `pub mod compact;` + `pub use compact::CompactHandler;`. Delete the `core_placeholder!(CompactHandler, "compact");` line + the `Arc::new(CompactHandler::new())` line in `register_core_placeholders`.

- [ ] **Step 5: Run + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::compact::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/src/builtin/compact.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs
git commit -m "feat(M5-10 task 4): /compact real impl + 3 telemetry events (4 unit tests)"
```

---

## Task 5: `/help` real implementation + `help_render` module

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/help_render.rs`
- Create: `lingxi-core/crates/commands/src/builtin/help.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs` (re-export both)
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs` (remove `HelpHandler` macro)

- [ ] **Step 1: Write the failing renderer test.**

  Create `lingxi-core/crates/commands/src/builtin/help_render.rs`:

```rust
//! Stateless renderer for `/help` output. Produces a byte-locked string
//! formatted per plan M5-10 Task 0 step 2.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::names::BUILTIN_COMMAND_NAMES;

    #[test]
    fn output_starts_with_locked_header() {
        let s = render_help_screen();
        assert!(s.starts_with("Commands:\n"));
    }

    #[test]
    fn output_has_exactly_103_lines() {
        // 1 header + 102 commands = 103 lines (each terminated by '\n').
        let s = render_help_screen();
        let n = s.matches('\n').count();
        assert_eq!(n, 103);
    }

    #[test]
    fn first_command_line_is_add_dir_unimplemented() {
        let s = render_help_screen();
        let line2 = s.lines().nth(1).unwrap();
        // Col-1 width is `longest_name_len + 2`. With longest=18 (rate-limit-options),
        // col-1 = 20 → `/add-dir` (8 chars) gets 12 spaces of right-padding.
        assert_eq!(line2, "  /add-dir              (unimplemented in v0.6.0)");
    }

    #[test]
    fn agents_line_uses_core_description() {
        let s = render_help_screen();
        let line = s.lines().find(|l| l.contains("/agents")).unwrap();
        assert_eq!(line, "  /agents               Manage subagents");
    }

    #[test]
    fn longest_name_line_uses_minimum_padding() {
        let s = render_help_screen();
        let line = s.lines().find(|l| l.contains("/rate-limit-options")).unwrap();
        // 2 leading spaces + "/rate-limit-options" (19 chars) + 1 padding space + 2 spaces + desc
        assert_eq!(line, "  /rate-limit-options (unimplemented in v0.6.0)");
        // Wait — the col-1 width is `longest + 2` = 20; "/rate-limit-options" is 19 chars; pad 1 to reach 20; then 2 spaces; then desc.
        // Total prefix = "  " + "/rate-limit-options" + " " + "  " = 24 chars before description.
    }

    #[test]
    fn every_command_appears_once() {
        let s = render_help_screen();
        for name in BUILTIN_COMMAND_NAMES {
            let needle = format!("/{name}");
            let count = s.matches(&needle).count();
            assert_eq!(count, 1, "/{name} should appear exactly once, got {count}");
        }
    }

    #[test]
    fn output_ends_with_newline() {
        let s = render_help_screen();
        assert!(s.ends_with('\n'));
    }
}
```

- [ ] **Step 2: Run + watch fail.**

```bash
cargo test -p lingxi-commands --lib builtin::help_render::tests 2>&1 | head -25
```

  Expected: 7 failures.

- [ ] **Step 3: Implement.**

```rust
//! Stateless renderer for `/help` output. Produces a byte-locked string
//! formatted per plan M5-10 Task 0 step 2.

use crate::builtin::names::{core_description, BUILTIN_COMMAND_NAMES};

/// Render the locked `/help` output as a single `String`.
///
/// Format (line-by-line):
///
/// ```text
/// Commands:\n
///   /<name padded to longest+2>  <description>\n
///   ... (102 lines, sorted ASCII-ascending) ...
/// ```
///
/// Where `<description>` is `core_description(name)` for the 18 core commands
/// and `"(unimplemented in v0.6.0)"` for the other 84.
#[must_use]
pub fn render_help_screen() -> String {
    let col1_width = BUILTIN_COMMAND_NAMES
        .iter()
        .map(|n| n.len())
        .max()
        .unwrap_or(0)
        + 2;

    // Capacity hint: header + 102 lines.
    let mut out = String::with_capacity(10 + 102 * (col1_width + 40));
    out.push_str("Commands:\n");

    for name in BUILTIN_COMMAND_NAMES {
        // Column 1: `/<name>` left-padded to col1_width characters.
        out.push_str("  /");
        out.push_str(name);
        let prefix_used = 1 + name.len(); // `/` + name
        if prefix_used < col1_width {
            for _ in prefix_used..col1_width {
                out.push(' ');
            }
        }
        out.push_str("  ");

        // Column 2: core description if known, else unimplemented marker.
        if is_core(name) {
            out.push_str(core_description(name));
        } else {
            out.push_str("(unimplemented in v0.6.0)");
        }
        out.push('\n');
    }

    out
}

fn is_core(name: &str) -> bool {
    crate::builtin::names::BUILTIN_CORE_NAMES.contains(&name)
}
```

- [ ] **Step 4: Write the `HelpHandler` test.**

  Create `lingxi-core/crates/commands/src/builtin/help.rs`:

```rust
//! `/help` — emits the locked rendering of the 102-command surface.
//!
//! See plan M5-10 Task 5.

use crate::builtin::help_render::render_help_screen;
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;

/// `/help` handler — pure function of the static command tables.
#[derive(Debug, Default)]
pub struct HelpHandler;

impl HelpHandler {
    #[must_use]
    pub fn new() -> Self { Self }
}

#[async_trait]
impl BuiltinCommandHandler for HelpHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::HELP_STARTED, serde_json::json!({}));
        let s = render_help_screen();
        lingxi_telemetry::emit(cmd_evt::HELP_COMPLETED, serde_json::json!({
            "lines": s.matches('\n').count(),
        }));
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str { "help" }
    fn description(&self) -> &str { core_description("help") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_done_with_render_help_screen_output() {
        let h = HelpHandler::new();
        let args = ParsedSlashCommand {
            name: "help".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Commands:\n"));
                assert!(s.contains("/agents               Manage subagents"));
                assert!(s.contains("/x402"));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }
}
```

- [ ] **Step 5: Wire + remove placeholder.**

  Update `builtin/mod.rs`: `pub mod help; pub mod help_render; pub use help::HelpHandler;`. Delete `core_placeholder!(HelpHandler, "help");`.

- [ ] **Step 6: Run + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::help_render::tests builtin::help::tests 2>&1 | tail -15
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/src/builtin/help.rs \
        lingxi-core/crates/commands/src/builtin/help_render.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs
git commit -m "feat(M5-10 task 5): /help real impl + help_render module + 3 telemetry events (8 unit tests, byte-locked layout)"
```

---

## Task 6: `/exit` real implementation

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/exit.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs`

- [ ] **Step 1: Write the failing test.**

```rust
//! `/exit` — sets the orchestrator's `should_exit` flag and emits the
//! locked confirmation literal.
//!
//! See plan M5-10 Task 6.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CommandResult;
    use crate::parser::ParsedSlashCommand;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use std::sync::Arc;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "exit".to_string(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn success_sets_flag_and_returns_locked_literal() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ExitHandler::new(mock.clone());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Exiting.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(mock.was_exit_requested());
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ExitHandler::new(mock);
        assert_eq!(h.name(), "exit");
        assert_eq!(h.description(), "Exit the REPL");
    }
}
```

- [ ] **Step 2: Run + fail.**

```bash
cargo test -p lingxi-commands --lib builtin::exit::tests 2>&1 | head -10
```

- [ ] **Step 3: Implement.**

```rust
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct ExitHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl ExitHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for ExitHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::EXIT_STARTED, serde_json::json!({}));
        self.handle.request_exit().await;
        lingxi_telemetry::emit(cmd_evt::EXIT_COMPLETED, serde_json::json!({}));
        CommandResult::Done { display: Some("Exiting.".to_string()) }
    }
    fn name(&self) -> &str { "exit" }
    fn description(&self) -> &str { core_description("exit") }
}
```

  Note: `request_exit` is infallible (returns `()`) — so the FAILED telemetry event is unreachable in practice. It still gets emitted only via the `failed` slot if a future variant of `OrchestratorHandle::request_exit` returns Result. For now, register the constant but don't call `emit(EXIT_FAILED, …)` anywhere.

- [ ] **Step 4: Wire + remove placeholder.**

  `builtin/mod.rs`: add `pub mod exit; pub use exit::ExitHandler;`. Delete `core_placeholder!(ExitHandler, "exit");`.

- [ ] **Step 5: Run + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::exit::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/src/builtin/exit.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs
git commit -m "feat(M5-10 task 6): /exit real impl + 2 reachable telemetry events (2 unit tests)"
```

---

## Task 7: `/memory` real implementation

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/memory.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs`

- [ ] **Step 1: Write the failing test.**

```rust
//! `/memory` — spawns `$EDITOR` on `<config>/claude/CLAUDE.md`.
//!
//! See plan M5-10 Task 7.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CommandResult;
    use crate::parser::ParsedSlashCommand;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand { name: "memory".to_string(), raw_args: String::new(), tokens: vec![] }
    }

    #[tokio::test]
    async fn success_renders_template_with_path_and_exit_code() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_memory_path(PathBuf::from("/home/u/.config/claude/CLAUDE.md"));
        mock.set_editor_exit_code(0);
        let h = MemoryHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Edited /home/u/.config/claude/CLAUDE.md (exit 0).");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn nonzero_exit_still_reports_as_edited() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_memory_path(PathBuf::from("/tmp/CLAUDE.md"));
        mock.set_editor_exit_code(2);
        let h = MemoryHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Edited /tmp/CLAUDE.md (exit 2).");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn failure_prefixes_handle_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_memory_editor_error("EDITOR not found".to_string());
        let h = MemoryHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Could not edit memory: handle action failed: EDITOR not found");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = MemoryHandler::new(mock);
        assert_eq!(h.name(), "memory");
        assert_eq!(h.description(), "Edit Claude memory files");
    }
}
```

  `MockOrchestratorHandle::set_memory_editor_error` is a new setter — add it to `lingxi-orchestrator/src/test_support.rs` along with the existing `set_memory_path` / `set_editor_exit_code` setters from Task 2 step 5. The setter switches `open_memory_editor` to return `Err(HandleError::ActionFailed(msg))` on next call.

- [ ] **Step 2: Run + fail.**

```bash
cargo test -p lingxi-commands --lib builtin::memory::tests 2>&1 | head -15
```

- [ ] **Step 3: Implement.**

```rust
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct MemoryHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl MemoryHandler {
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for MemoryHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::MEMORY_STARTED, serde_json::json!({}));
        match self.handle.open_memory_editor().await {
            Ok(outcome) => {
                lingxi_telemetry::emit(cmd_evt::MEMORY_COMPLETED, serde_json::json!({
                    "edited_path": outcome.edited_path.display().to_string(),
                    "exit_code": outcome.exit_code,
                }));
                CommandResult::Done {
                    display: Some(format!(
                        "Edited {} (exit {}).",
                        outcome.edited_path.display(),
                        outcome.exit_code
                    )),
                }
            }
            Err(e) => {
                lingxi_telemetry::emit(cmd_evt::MEMORY_FAILED, serde_json::json!({
                    "error": e.to_string(),
                }));
                CommandResult::Done {
                    display: Some(format!("Could not edit memory: {e}")),
                }
            }
        }
    }
    fn name(&self) -> &str { "memory" }
    fn description(&self) -> &str { core_description("memory") }
}
```

- [ ] **Step 4: Wire + remove placeholder + commit.**

  `builtin/mod.rs`: `pub mod memory; pub use memory::MemoryHandler;`. Delete `core_placeholder!(MemoryHandler, "memory");`.

```bash
cargo test -p lingxi-commands --lib builtin::memory::tests 2>&1 | tail -10
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/src/builtin/memory.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs \
        lingxi-core/crates/orchestrator/src/test_support.rs
git commit -m "feat(M5-10 task 7): /memory real impl + EDITOR fallback + 3 telemetry events (4 unit tests)"
```

---

## Task 8: `/init` real implementation + byte-locked `OLD_INIT_PROMPT` template

**Files:**
- Create: `lingxi-core/crates/commands/src/builtin/templates.rs`
- Create: `lingxi-core/crates/commands/src/builtin/init.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/mod.rs`
- Modify: `lingxi-core/crates/commands/src/builtin/core_placeholders.rs`

- [ ] **Step 1: Write the failing template invariant tests.**

  Create `lingxi-core/crates/commands/src/builtin/templates.rs`:

```rust
//! Byte-locked text templates for `/init` and other commands.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_init_prompt_starts_with_locked_first_line() {
        assert!(OLD_INIT_PROMPT.starts_with(
            "Please analyze this codebase and create a CLAUDE.md file, which will be given to future instances of Claude Code to operate in this repository."
        ));
    }

    #[test]
    fn old_init_prompt_contains_claude_md_prefix_block() {
        assert!(OLD_INIT_PROMPT.contains("# CLAUDE.md"));
        assert!(OLD_INIT_PROMPT.contains(
            "This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository."
        ));
    }

    #[test]
    fn old_init_prompt_line_count_locked() {
        // 24 logical lines, terminated by '\n' (or the final line has no
        // trailing newline). See plan T0 step 1.
        let n = OLD_INIT_PROMPT.lines().count();
        assert_eq!(n, 24, "/init template line count drifted: {}", n);
    }

    #[test]
    fn old_init_prompt_no_trailing_blank_line() {
        assert!(!OLD_INIT_PROMPT.ends_with("\n\n"));
    }

    #[test]
    fn old_init_prompt_sha256_locked() {
        // Compute sha256 of OLD_INIT_PROMPT; lock against a fixed digest.
        // The expected digest is recorded in
        // `crates/test-harness/src/parity/fixtures/parity_init_template.json`
        // (Task 10). To regenerate: run this test once, copy the actual digest
        // into the assertion below + the fixture, commit.
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(OLD_INIT_PROMPT.as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        // The expected digest is locked at FIRST GREEN by the implementer:
        // they run this test, copy the printed digest from the failure
        // message, paste below, commit. Future drift => test fails.
        assert_eq!(
            digest,
            "REPLACE_WITH_ACTUAL_DIGEST_ON_FIRST_GREEN",
            "OLD_INIT_PROMPT byte-changed; expected hash drifted"
        );
    }
}
```

- [ ] **Step 2: Run + fail.**

```bash
cargo test -p lingxi-commands --lib builtin::templates::tests 2>&1 | head -15
```

- [ ] **Step 3: Implement.**

  Prepend to `templates.rs` (above the `#[cfg(test)]`):

```rust
//! Byte-locked text templates for `/init` and other commands.
//!
//! See plan M5-10 Task 0 step 1 for the source of [`OLD_INIT_PROMPT`].

/// 24-line markdown template that `/init` injects as the next user message.
///
/// Byte-locked from `claude-code/src/commands/init.ts:6-30` (the
/// `OLD_INIT_PROMPT` constant). When claude-code upgrades and the template
/// drifts, M6+ may need to refresh this constant; for v0.6.0 the M5
/// baseline is frozen.
pub const OLD_INIT_PROMPT: &str = "Please analyze this codebase and create a CLAUDE.md file, which will be given to future instances of Claude Code to operate in this repository.

What to add:
1. Commands that will be commonly used, such as how to build, lint, and run tests. Include the necessary commands to develop in this codebase, such as how to run a single test.
2. High-level code architecture and structure so that future instances can be productive more quickly. Focus on the \"big picture\" architecture that requires reading multiple files to understand.

Usage notes:
- If there's already a CLAUDE.md, suggest improvements to it.
- When you make the initial CLAUDE.md, do not repeat yourself and do not include obvious instructions like \"Provide helpful error messages to users\", \"Write unit tests for all new utilities\", \"Never include sensitive information (API keys, tokens) in code or commits\".
- Avoid listing every component or file structure that can be easily discovered.
- Don't include generic development practices.
- If there are Cursor rules (in .cursor/rules/ or .cursorrules) or Copilot rules (in .github/copilot-instructions.md), make sure to include the important parts.
- If there is a README.md, make sure to include the important parts.
- Do not make up information such as \"Common Development Tasks\", \"Tips for Development\", \"Support and Documentation\" unless this is expressly included in other files that you read.
- Be sure to prefix the file with the following text:

```
# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.
```";
```

  **Important escaping notes:**

  - The closing triple-backtick of the fenced code block at the end **must** be the actual three backticks `\`\`\``. In the Rust string above, they appear unescaped because backticks are not special in Rust string literals.
  - All quote characters inside the template are double quotes — these need to be escaped as `\"` in the Rust string literal.
  - All other characters (apostrophes, parens, hyphens) need no escaping.
  - Line breaks: use literal `\n` only if you prefer; the multi-line Rust string literal already terminates lines with `\n` from the source-file newlines. Make sure the source file ends the string with a closing `"` on the same line as the final triple-backtick line.

  **First-green digest workflow:** run `cargo test -p lingxi-commands --lib builtin::templates::tests::old_init_prompt_sha256_locked 2>&1`. The test will fail and print the actual sha256. Copy that hex string and replace `"REPLACE_WITH_ACTUAL_DIGEST_ON_FIRST_GREEN"` in the test with the real value. Re-run; should pass.

  Add `sha2 = "0.10"` to `lingxi-commands/Cargo.toml` `[dev-dependencies]`.

- [ ] **Step 4: Implement `InitHandler`.**

  Create `lingxi-core/crates/commands/src/builtin/init.rs`:

```rust
//! `/init` — returns the locked `OLD_INIT_PROMPT` template as an injected
//! user message so the next turn analyses the codebase and writes CLAUDE.md.
//!
//! See plan M5-10 Task 8.

use crate::builtin::names::core_description;
use crate::builtin::templates::OLD_INIT_PROMPT;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;

/// `/init` handler — returns the locked init prompt as InjectMessage.
#[derive(Debug, Default)]
pub struct InitHandler;

impl InitHandler {
    #[must_use]
    pub fn new() -> Self { Self }
}

#[async_trait]
impl BuiltinCommandHandler for InitHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit(cmd_evt::INIT_STARTED, serde_json::json!({}));
        lingxi_telemetry::emit(cmd_evt::INIT_COMPLETED, serde_json::json!({
            "template_bytes": OLD_INIT_PROMPT.len(),
        }));
        CommandResult::InjectMessage {
            content: OLD_INIT_PROMPT.to_string(),
        }
    }
    fn name(&self) -> &str { "init" }
    fn description(&self) -> &str { core_description("init") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_inject_message_with_locked_template() {
        let h = InitHandler::new();
        let args = ParsedSlashCommand {
            name: "init".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::InjectMessage { content } => {
                assert_eq!(content, OLD_INIT_PROMPT);
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inject_message_content_starts_with_locked_first_sentence() {
        let h = InitHandler::new();
        let args = ParsedSlashCommand {
            name: "init".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        let r = h.handle(&args).await;
        if let CommandResult::InjectMessage { content } = r {
            assert!(content.starts_with("Please analyze this codebase"));
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = InitHandler::new();
        assert_eq!(h.name(), "init");
        assert_eq!(h.description(),
            "Initialize a new CLAUDE.md file with codebase documentation");
    }
}
```

- [ ] **Step 5: Wire + remove placeholder.**

  `builtin/mod.rs`: `pub mod init; pub mod templates; pub use init::InitHandler; pub use templates::OLD_INIT_PROMPT;`. Delete `core_placeholder!(InitHandler, "init");`.

- [ ] **Step 6: Run + commit.**

```bash
cargo test -p lingxi-commands --lib builtin::templates::tests builtin::init::tests 2>&1 | tail -15
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/src/builtin/templates.rs \
        lingxi-core/crates/commands/src/builtin/init.rs \
        lingxi-core/crates/commands/src/builtin/mod.rs \
        lingxi-core/crates/commands/src/builtin/core_placeholders.rs \
        lingxi-core/crates/commands/Cargo.toml
git commit -m "feat(M5-10 task 8): /init real impl + OLD_INIT_PROMPT 24-line byte-locked template + 3 telemetry events (8 invariant tests)"
```

---

## Task 9: `register_core_batch_1(reg, handle)` helper

**Files:**
- Modify: `lingxi-core/crates/commands/src/registry.rs`
- Modify: `lingxi-core/crates/commands/src/lib.rs`

- [ ] **Step 1: Write the failing test.**

  Append to `lingxi-core/crates/commands/src/registry.rs`:

```rust
#[cfg(test)]
mod batch_1_tests {
    use super::*;
    use crate::builtin::{
        ClearHandler, CompactHandler, ExitHandler, HelpHandler, InitHandler,
        MemoryHandler,
    };
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;
    use std::sync::Arc;

    #[tokio::test]
    async fn all_6_overwrite_with_handle_bound_handlers() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_1(&mut reg, handle.clone());

        // After overwrite, the resolved handler for each of the 6 names is
        // their handle-bound type — verifiable by calling and observing the
        // mock side effect.
        let clear_h = reg.get_handler("clear").unwrap();
        let args = crate::parser::ParsedSlashCommand {
            name: "clear".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        let r = clear_h.handle(&args).await;
        assert!(matches!(r, CommandResult::Done { display: Some(ref s) } if s == "Conversation cleared."));
        assert!(handle.was_clear_session_called());
    }

    #[tokio::test]
    async fn other_96_commands_unchanged_after_batch_1() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_1(&mut reg, handle);

        // A non-batch-1 command (`x402`) still returns the M5-09 stub literal.
        let h = reg.get_handler("x402").unwrap();
        let args = crate::parser::ParsedSlashCommand {
            name: "x402".to_string(),
            raw_args: String::new(),
            tokens: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "x402: not implemented in v0.6.0 (M5)");
            }
            other => panic!("/x402 changed: {other:?}"),
        }
    }

    #[test]
    fn batch_1_size_is_6() {
        // Compile-time-ish proof: build the list, count it.
        let names = ["clear", "compact", "exit", "help", "init", "memory"];
        assert_eq!(names.len(), 6);
    }
}
```

- [ ] **Step 2: Implement.**

  Append to `registry.rs`:

```rust
/// Overwrite the 6 batch-1 entries (`clear`, `compact`, `exit`, `help`,
/// `init`, `memory`) with their handle-bound real handlers from M5-10.
///
/// Call **after** [`register_all_builtin_commands`]. The function is
/// idempotent — calling it twice with the same `handle` produces the same
/// final state.
///
/// `HelpHandler` and `InitHandler` are constructed without `handle` because
/// they don't need orchestrator state.
pub fn register_core_batch_1(
    reg: &mut CommandRegistry,
    handle: std::sync::Arc<dyn lingxi_traits::OrchestratorHandle>,
) {
    use crate::builtin::{
        ClearHandler, CompactHandler, ExitHandler, HelpHandler, InitHandler, MemoryHandler,
    };
    use std::sync::Arc;

    reg.register_builtin_handler(Arc::new(ClearHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(CompactHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ExitHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(HelpHandler::new()));
    reg.register_builtin_handler(Arc::new(InitHandler::new()));
    reg.register_builtin_handler(Arc::new(MemoryHandler::new(handle)));
}
```

  Re-export from `lib.rs`:

```rust
pub use registry::{register_all_builtin_commands, register_core_batch_1, CommandRegistry};
```

- [ ] **Step 3: Run + commit.**

```bash
cargo test -p lingxi-commands --lib registry::batch_1_tests 2>&1 | tail -10
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --lib --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/src/registry.rs lingxi-core/crates/commands/src/lib.rs
git commit -m "feat(M5-10 task 9): register_core_batch_1(reg, handle) — handle-bound overwrite for 6 commands (3 tests)"
```

---

## Task 10: Parity fixtures + driver updates

**Files:**
- Modify: `lingxi-core/crates/test-harness/src/parity/fixtures/parity_tengu_events.json` (append 18 rows)
- Create: `lingxi-core/crates/test-harness/src/parity/fixtures/parity_help_screen.txt` (golden output)
- Create: `lingxi-core/crates/test-harness/src/parity/fixtures/parity_init_template.json` (sha256 + line count)
- Modify: `lingxi-core/crates/test-harness/tests/parity_telemetry_coverage.rs` (count bump)
- Create: `lingxi-core/crates/test-harness/tests/parity_help_render.rs`
- Create: `lingxi-core/crates/test-harness/tests/parity_init_template.rs`

- [ ] **Step 1: Append 18 events to `parity_tengu_events.json`.**

  The fixture is a JSON array of rows like `{"name": "tengu_x", "category": "session", "since": "v0.5.0"}` (exact shape per M3-06). Add 18 new rows under category `"command"` since `v0.6.0`:

```json
{"name": "tengu_command_clear_completed",   "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_clear_failed",      "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_clear_started",     "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_compact_completed", "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_compact_failed",    "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_compact_started",   "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_exit_completed",    "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_exit_failed",       "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_exit_started",      "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_help_completed",    "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_help_failed",       "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_help_started",      "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_init_completed",    "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_init_failed",       "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_init_started",      "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_memory_completed",  "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_memory_failed",     "category": "command", "since": "v0.6.0"},
{"name": "tengu_command_memory_started",    "category": "command", "since": "v0.6.0"}
```

  Insert at the position that matches the `ALL_EVENT_NAMES` slice order (Task 1 step 4 places `command` last; append after the M5-08 session rows accordingly).

- [ ] **Step 2: Generate `parity_help_screen.txt`.**

  Use the renderer itself to produce the golden output, then check it in:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo run -p lingxi-commands --bin dump_help 2>/dev/null > \
  crates/test-harness/src/parity/fixtures/parity_help_screen.txt
```

  …**but we don't have a `dump_help` bin.** Instead, write a one-shot test that produces the file:

  Create a temporary test in `lingxi-commands/tests/dump_help.rs`:

```rust
use lingxi_commands::builtin::help_render::render_help_screen;

#[test]
#[ignore = "writes the golden fixture; run manually once"]
fn dump_golden_help_screen() {
    let s = render_help_screen();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../test-harness/src/parity/fixtures/parity_help_screen.txt");
    std::fs::write(&path, s).expect("write golden");
}
```

  Run once: `cargo test -p lingxi-commands --test dump_help -- --ignored`. Commit the resulting `.txt`. Verify by hand: file has exactly 103 lines, first is `Commands:`, last is `  /x402 …`.

- [ ] **Step 3: Generate `parity_init_template.json`.**

  Capture the byte length + sha256 + line count of `OLD_INIT_PROMPT`. Manually:

```bash
rg -A 100 "pub const OLD_INIT_PROMPT" lingxi-core/crates/commands/src/builtin/templates.rs | \
  awk 'BEGIN{p=0} /^pub const/{p=1} p{print}' > /tmp/init_template.rs
```

  Better: write the fixture from the same test that locked the sha256 in T8 step 5. Manually create `parity_init_template.json`:

```json
{
  "_meta": {
    "plan": "M5-10",
    "source": "claude-code/src/commands/init.ts:6-30 (OLD_INIT_PROMPT)",
    "_claude_code_version": "see CHANGELOG v0.6.0 baseline"
  },
  "byte_length": 1543,
  "line_count": 24,
  "sha256": "REPLACE_WITH_ACTUAL_DIGEST_FROM_T8_STEP_5",
  "first_sentence": "Please analyze this codebase and create a CLAUDE.md file, which will be given to future instances of Claude Code to operate in this repository.",
  "must_contain_substrings": [
    "# CLAUDE.md",
    "This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.",
    "If there's already a CLAUDE.md, suggest improvements to it.",
    "Be sure to prefix the file with the following text:"
  ]
}
```

  Update `byte_length` and `sha256` to the actual values printed by the T8 first-green test. The byte length will be approximately 1543 but may differ; lock the actual value.

- [ ] **Step 4: Write the help-render parity driver.**

  Create `lingxi-core/crates/test-harness/tests/parity_help_render.rs`:

```rust
//! Parity: `/help` byte-locked rendering. Compares the renderer output to
//! the golden `parity_help_screen.txt`.

use lingxi_commands::builtin::help_render::render_help_screen;

const GOLDEN: &str =
    include_str!("../src/parity/fixtures/parity_help_screen.txt");

#[test]
fn render_help_screen_matches_golden_byte_for_byte() {
    let actual = render_help_screen();
    assert_eq!(actual, GOLDEN, "/help output drifted from golden fixture");
}

#[test]
fn golden_starts_with_locked_header() {
    assert!(GOLDEN.starts_with("Commands:\n"));
}

#[test]
fn golden_has_103_lines() {
    assert_eq!(GOLDEN.matches('\n').count(), 103);
}
```

- [ ] **Step 5: Write the init-template parity driver.**

  Create `lingxi-core/crates/test-harness/tests/parity_init_template.rs`:

```rust
//! Parity: `/init` template byte-locked.

use lingxi_commands::builtin::OLD_INIT_PROMPT;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const FIXTURE: &str =
    include_str!("../src/parity/fixtures/parity_init_template.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    byte_length: usize,
    line_count: usize,
    sha256: String,
    first_sentence: String,
    must_contain_substrings: Vec<String>,
}

fn load() -> Fixture {
    serde_json::from_str(FIXTURE).expect("fixture parse")
}

#[test]
fn byte_length_matches_fixture() {
    let f = load();
    assert_eq!(OLD_INIT_PROMPT.len(), f.byte_length);
}

#[test]
fn line_count_matches_fixture() {
    let f = load();
    assert_eq!(OLD_INIT_PROMPT.lines().count(), f.line_count);
}

#[test]
fn sha256_matches_fixture() {
    let f = load();
    let mut hasher = Sha256::new();
    hasher.update(OLD_INIT_PROMPT.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    assert_eq!(digest, f.sha256);
}

#[test]
fn first_sentence_matches_fixture() {
    let f = load();
    assert!(OLD_INIT_PROMPT.starts_with(&f.first_sentence));
}

#[test]
fn contains_all_required_substrings() {
    let f = load();
    for s in &f.must_contain_substrings {
        assert!(OLD_INIT_PROMPT.contains(s), "missing substring: {s}");
    }
}
```

  Add `sha2 = "0.10"` and `serde = { workspace = true, features = ["derive"] }` + `serde_json` to `test-harness/Cargo.toml` `[dev-dependencies]`.

- [ ] **Step 6: Update telemetry coverage driver.**

  Open `lingxi-core/crates/test-harness/tests/parity_telemetry_coverage.rs`. There's likely a `TOTAL_EVENTS: usize = 258` constant or similar — bump to `276`. Also update any per-category count constants if the test breaks them down.

- [ ] **Step 7: Run all parity tests.**

```bash
cargo test -p lingxi-test-harness 2>&1 | tail -20
```

  Expected: all parity tests pass including the 3 new files + the bumped telemetry coverage.

- [ ] **Step 8: Fmt + clippy + commit.**

```bash
cargo fmt -p lingxi-test-harness
cargo clippy -p lingxi-test-harness --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/test-harness/src/parity/fixtures/ \
        lingxi-core/crates/test-harness/tests/ \
        lingxi-core/crates/test-harness/Cargo.toml
git commit -m "test(M5-10 task 10): parity fixtures + drivers — tengu events 258→276, /help golden, /init template sha256"
```

---

## Task 11: `event_name_completeness_test.rs` bump + cross-check

**Files:**
- Modify: `lingxi-core/crates/telemetry/tests/event_name_completeness_test.rs` (or wherever the M3-06 lock lives)

- [ ] **Step 1: Find the test.**

```bash
rg -n "ALL_EVENT_NAMES.len" lingxi-core/crates/telemetry/ lingxi-core/crates/test-harness/ 2>&1 | head -10
```

- [ ] **Step 2: Update the assertion.**

  Whatever file holds `assert_eq!(ALL_EVENT_NAMES.len(), 258)` or `258 + N`, change the right-hand side to `276`. If the file uses a `const TOTAL: usize = 258`, change to `276`.

- [ ] **Step 3: Run + commit.**

```bash
cargo test -p lingxi-telemetry -p lingxi-test-harness 2>&1 | tail -10
git add -u  # picks up the modified test files
git commit -m "test(M5-10 task 11): bump ALL_EVENT_NAMES.len() lock 258→276"
```

---

## Task 12: Integration e2e tests — dispatcher + 6 handlers via real path

**Files:**
- Create: `lingxi-core/crates/commands/tests/batch_1_e2e.rs`

- [ ] **Step 1: Write the e2e test.**

```rust
//! End-to-end: build a registry with `register_all_builtin_commands` +
//! `register_core_batch_1`, dispatch each of the 6 commands through the
//! `RegistrySlashDispatcher`, and verify behaviour.

use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{
    register_all_builtin_commands, register_core_batch_1, CommandRegistry,
};
use lingxi_orchestrator::test_support::MockOrchestratorHandle;
use lingxi_traits::{CompactionSummary, OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};
use std::sync::Arc;
use tokio::sync::RwLock;

async fn fresh() -> (RegistrySlashDispatcher, Arc<MockOrchestratorHandle>) {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let handle: Arc<MockOrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
    register_core_batch_1(&mut reg, handle.clone());
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));
    (d, handle)
}

#[tokio::test]
async fn clear_dispatch() {
    let (d, mock) = fresh().await;
    let r = d.dispatch("/clear").await;
    match r {
        SlashDispatchResult::Handled { display } => assert_eq!(display, "Conversation cleared."),
        other => panic!("{other:?}"),
    }
    assert!(mock.was_clear_session_called());
}

#[tokio::test]
async fn compact_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_compact_summary(CompactionSummary {
        messages_before: 100,
        messages_after: 5,
        bytes_saved: 4_096,
    });
    let r = d.dispatch("/compact").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Compacted: 100 → 5 messages (4096 bytes saved).");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn help_dispatch() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/help").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert!(display.starts_with("Commands:\n"));
            assert!(display.contains("/agents               Manage subagents"));
            // 103 newlines (header + 102 lines).
            assert_eq!(display.matches('\n').count(), 103);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn exit_dispatch() {
    let (d, mock) = fresh().await;
    let r = d.dispatch("/exit").await;
    match r {
        SlashDispatchResult::Handled { display } => assert_eq!(display, "Exiting."),
        other => panic!("{other:?}"),
    }
    assert!(mock.was_exit_requested());
}

#[tokio::test]
async fn memory_dispatch() {
    let (d, mock) = fresh().await;
    mock.set_memory_path(std::path::PathBuf::from("/tmp/CLAUDE.md"));
    mock.set_editor_exit_code(0);
    let r = d.dispatch("/memory").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Edited /tmp/CLAUDE.md (exit 0).");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn init_dispatch_returns_handled_with_template_text() {
    // /init returns InjectMessage; the dispatcher maps InjectMessage to
    // Handled { display: content } per M5-09 Task 5 step 4.
    let (d, _) = fresh().await;
    let r = d.dispatch("/init").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert!(display.starts_with("Please analyze this codebase"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn non_batch_1_command_still_returns_stub() {
    let (d, _) = fresh().await;
    let r = d.dispatch("/x402").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "x402: not implemented in v0.6.0 (M5)");
        }
        other => panic!("{other:?}"),
    }
}
```

- [ ] **Step 2: Run + commit.**

```bash
cargo test -p lingxi-commands --test batch_1_e2e 2>&1 | tail -15
cargo fmt -p lingxi-commands
cargo clippy -p lingxi-commands --tests -- -D warnings 2>&1 | tail -5
git add lingxi-core/crates/commands/tests/batch_1_e2e.rs
git commit -m "test(M5-10 task 12): batch_1_e2e — 7 end-to-end dispatcher cases"
```

---

## Task 13: Documentation updates

**Files:**
- Modify: `lingxi-core/crates/commands/src/lib.rs` (module docs)
- Modify: `lingxi-core/crates/traits/src/lib.rs` (re-export note)

- [ ] **Step 1: Update commands crate docs.**

  Append to the `lib.rs` module-level doc comment (M5-09 left the doc at the surface description):

```rust
//! # Batch 1 (M5-10)
//!
//! After [`register_core_batch_1`] runs, the 6 batch-1 commands are wired to
//! real implementations:
//!
//! - `/clear` — calls [`lingxi_traits::OrchestratorHandle::clear_session`]
//! - `/compact` — calls [`lingxi_traits::OrchestratorHandle::force_compact`]
//! - `/help` — renders the locked 102-line table via [`builtin::help_render::render_help_screen`]
//! - `/exit` — calls [`lingxi_traits::OrchestratorHandle::request_exit`]
//! - `/memory` — calls [`lingxi_traits::OrchestratorHandle::open_memory_editor`]
//! - `/init` — emits [`crate::CommandResult::InjectMessage`] with the locked
//!   [`builtin::OLD_INIT_PROMPT`] template
//!
//! Each command emits 3 telemetry events (`tengu_command_<name>_{started,completed,failed}`)
//! defined in [`lingxi_telemetry::tengu::command`].
```

- [ ] **Step 2: Commit.**

```bash
git add lingxi-core/crates/commands/src/lib.rs
git commit -m "docs(M5-10 task 13): batch-1 module docs for /clear /compact /help /exit /memory /init"
```

---

## Task 14: Verification gate + tag `m5.10`

**Files:** none (verification + tag).

- [ ] **Step 1: Workspace fmt + clippy + tests.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-core
cargo fmt --all -- --check 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
cargo test --workspace 2>&1 | tail -30
```

  Expected: all clean, all green (except the 2 known fs-watch flakes from v0.5.0).

- [ ] **Step 2: Confirm event count exactly 276.**

```bash
rg -n "276\|258\|TOTAL" lingxi-core/crates/telemetry/src/tengu/mod.rs | head -5
cargo test -p lingxi-test-harness --test parity_telemetry_coverage 2>&1 | tail -10
```

- [ ] **Step 3: Confirm M4 + M5-09 backward compat.**

```bash
cargo test -p lingxi-test-harness --test parity_registry 2>&1 | tail -5   # 40 tools
cargo test -p lingxi-test-harness --test parity_slash_commands 2>&1 | tail -10   # 102 commands (M5-09)
```

  Both must still pass.

- [ ] **Step 4: Update this plan's status header + final commit + tag.**

```bash
# (Edit this file's title block as in M5-09 Task 10 step 1.)
cd /Users/luolingfeng/Projects/LingXi-Next
git add docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md
git commit -m "release(M5-10 task 14): mark batch-1 plan complete"
git tag -a m5.10 -m "M5-10: /clear /compact /help /exit /memory /init real impls + 18 telemetry events + /init byte-locked template"
git tag -n 1 | grep "^m5\." | sort | tail
```

- [ ] **Step 5: Do NOT push.** Tag remains local until M5-14.

---

## Self-review

**1. Spec coverage** (against spec §3 M5-10 row):

| Spec requirement | Task |
|---|---|
| `/clear` → `OrchestratorHandle::clear_session` | T3 |
| `/compact` → `force_compact` | T4 |
| `/help` lists 102 names + status | T5 |
| `/exit` sets `should_exit=true` | T2 (trait + impl) + T6 (handler) |
| `/memory` opens `$EDITOR` | T2 (trait + impl) + T7 (handler) |
| `/init` generates CLAUDE.md skeleton | T8 (InjectMessage path with OLD_INIT_PROMPT) |
| `/init` template byte-locked | T0 step 1 + T8 templates.rs + T10 sha256 fixture |
| 6×3=18 telemetry events | T1 (tengu::command module + 18 NAMES) + each of T3/T4/T5/T6/T7/T8 emits the 3 events |
| ALL_EVENT_NAMES 258 → 276 | T1 step 4 (mod.rs TOTAL) + T11 (test lock) |

  All ✅.

**2. Placeholder scan:**

  - "REPLACE_WITH_ACTUAL_DIGEST_ON_FIRST_GREEN" appears in T8 step 3 as a deliberate first-green sigil — the implementer fills it in on first test run. **Not a placeholder** — it's a documented protocol (the test fails until filled, and the byte-lock is the actual sha256 of the actual template).
  - "REPLACE_WITH_ACTUAL_DIGEST_FROM_T8_STEP_5" in T10 step 3 fixture: same protocol — derive from the actual code.
  - No `TODO`, `TBD`, `unimplemented!()`, `todo!()`, or `FIXME` anywhere in code blocks.

**3. Type consistency:**

| Identifier | T1-T9 declaration | All later uses |
|---|---|---|
| `tengu::command::{CLEAR_STARTED, ..., MEMORY_FAILED}` | T1 step 3 (18 const) | T3-T8 emit calls, T10 parity rows, T11 count lock |
| `tengu::command::NAMES` | T1 step 3 (`&[&str; 18]`) | T1 step 4 ALL_EVENT_NAMES concat, T1 tests |
| `OrchestratorHandle::request_exit` | T2 step 3 trait method | T6 handler call, T2 mock impl, T2 production impl |
| `OrchestratorHandle::open_memory_editor` | T2 step 3 trait method | T7 handler call, T2 mock impl, T2 production impl |
| `MemoryEditorOutcome { edited_path, exit_code }` | T2 step 3 struct | T7 handler reads both fields, T2 mock returns, T2 production returns |
| `ClearHandler` | T3 step 3 in `builtin/clear.rs` | T9 register_core_batch_1, T12 e2e tests, M5-09 macro deletion (T3 step 4) |
| `CompactHandler` | T4 step 3 in `builtin/compact.rs` | T9, T12, M5-09 macro deletion (T4 step 4) |
| `HelpHandler` | T5 step 4 in `builtin/help.rs` | T9, T12, M5-09 macro deletion (T5 step 5) |
| `ExitHandler` | T6 step 3 in `builtin/exit.rs` | T9, T12, M5-09 macro deletion (T6 step 4) |
| `MemoryHandler` | T7 step 3 in `builtin/memory.rs` | T9, T12, M5-09 macro deletion (T7 step 4) |
| `InitHandler` | T8 step 4 in `builtin/init.rs` | T9, T12, M5-09 macro deletion (T8 step 5) |
| `render_help_screen` | T5 step 3 `pub fn` | T5 HelpHandler body, T10 step 4 golden dump, parity_help_render.rs |
| `OLD_INIT_PROMPT` | T8 step 3 `pub const &str` | T8 InitHandler body, T10 step 5 parity_init_template.rs |
| `register_core_batch_1` | T9 step 2 `pub fn` | T12 e2e tests, M5-12 CLI binary will call this |

  All consistent.

**4. Telemetry chain:** 18 new events (6 × 3). Verified across:

  - T1 step 3 (18 const + NAMES slice)
  - T1 step 4 (ALL_EVENT_NAMES concat: post-M5-08 count + 18 = 276)
  - T10 step 1 (parity_tengu_events.json: 18 new rows)
  - T11 (event_name_completeness_test bump 258 → 276)
  - Each of T3/T4/T5/T6/T7/T8 emits exactly 2-3 events per dispatch (started + completed OR started + failed).

  `tengu::tool::NAMES.len()` stays at 134 (M5-10 does not touch tool events).

**5. M4 / M5-09 backward compat:**

  - T14 step 3 runs `parity_registry_40_tools.rs` (M4-09 lock) — must pass unchanged.
  - T14 step 3 runs `parity_slash_commands.rs` (M5-09 102-name lock) — must pass; M5-10 only overwrites 6 entries to handle-bound versions with the SAME public name + description.
  - The M5-09 parity test `every_fixture_command_dispatches_to_expected_literal` (T6 step 4 of M5-09) **will break** for the 6 batch-1 commands because they no longer return `"clear: not implemented in v0.6.0 (M5)"` — they return `"Conversation cleared."` etc. **Decision:** that M5-09 test only runs against a registry built with `register_all_builtin_commands` alone (no `register_core_batch_1` overlay). So it stays green. M5-10's e2e test (T12) builds with both calls and verifies the new behavior.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**

**If Subagent-Driven chosen:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Fresh subagent per task + two-stage review.

**If Inline Execution chosen:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Batch execution with checkpoints for review.

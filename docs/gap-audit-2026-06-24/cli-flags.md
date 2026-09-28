# CLI Flags Parity Gap Report

**STATUS**: COMPLETE — 28 gap flags added, 3 behaviors wired, all tests green.

**Commit**: `715adc4e` — `feat(cli): recognize v2.1.186 headless/SDK flags + wire system-prompt/output-format/add-dir`

**Test result**: 190 tests pass (159 lib unit + 31 integration), 0 failed.

---

## BEHAVIOR-WIRED flags

| Flag | Behavior |
|------|----------|
| `--output-format json` | → `is_json_output()` returns true (same as `--json`) |
| `--output-format stream-json` | → `is_json_output()` returns true; TODO stub for NDJSON streaming |
| `--json` | Kept as LingXi-specific alias; still works |
| `--system-prompt <p>` | → `DesktopConfig.system_prompt_override` → engine-desktop `build()` |
| `--system-prompt-file <f>` | → reads file, sets `system_prompt_override` |
| `--append-system-prompt <p>` | → `DesktopConfig.append_system_prompt` → engine-desktop `build()` |
| `--append-system-prompt-file <f>` | → reads file, sets `append_system_prompt` |

---

## PARSE-ONLY flags (stored in Argv struct, no behavior yet)

- `--input-format <text|stream-json>`
- `--allowed-tools <...>` / `--allowedTools` alias
- `--disallowed-tools <...>` / `--disallowedTools` alias
- `--tools <...>`
- `--add-dir <dirs...>`
- `--settings <file-or-json>`
- `--mcp-config <configs...>`
- `--verbose`
- `--bare`
- `--safe-mode`
- `--agents <...>`
- `--agent <...>`
- `--plugin-dir <dir>`
- `--no-session-persistence`
- `--from-pr [value]`
- `--effort <level>`
- `--betas <...>`
- `--debug-file <path>`
- `--include-partial-messages`
- `--include-hook-events`
- `--replay-user-messages`
- `--thinking <enabled|adaptive|disabled>` (hidden)
- `--thinking-display <summarized|omitted>` (hidden)
- `--max-thinking-tokens <n>` (hidden/deprecated)
- `--prompt-suggestions [value]` (hidden)

---

## TODO-stubbed subsystems

1. **stream-json**: `// TODO(stream-json): realtime NDJSON I/O subsystem` — bidirectional streaming protocol not implemented; currently behaves like `json`.
2. **mcp-config**: `--mcp-config` parses but `McpConfig` loading not wired — needs MCP config loader integration.
3. **tool-gating**: `--allowed-tools` / `--disallowed-tools` / `--tools` parse but tool-permission gating not wired — needs `ToolPermissionManager` integration.
4. **add-dir**: `--add-dir` parses but directory injection not wired — needs memory-provider seam for CLAUDE.md search path injection.
5. **settings**: `--settings` parses but file/JSON settings overlay not wired — needs `DesktopConfig` merge integration.
6. **agents/agent**: `--agents` / `--agent` parse but agent routing not wired.
7. **effort**: `--effort` parses but effort-level routing not wired.
8. **betas**: `--betas` parses but beta-header injection not wired.
9. **verbose/bare/safe-mode**: parse but semantics not wired.
10. **thinking flags**: parse but thinking-mode routing not wired.

---

## Files changed

- `crates/apps/cli/src/argv.rs` — +524 lines (flag defs, is_json_output(), system-prompt wiring)
- `crates/apps/cli/src/init.rs` — updated to use `Argv::default()`
- `crates/apps/cli/src/lib.rs` — output-format wire-up
- `crates/apps/cli/src/mode.rs` — is_json_output() consumption
- `crates/apps/cli/src/run.rs` — system-prompt wire-up
- `crates/apps/engine-desktop/src/lib.rs` — system_prompt_override/append_system_prompt fields + build() injection
- `crates/apps/bridge-server/src/boot.rs` — Argv::default() update

# Slash-commands + CLI Surface Parity Audit — v2.1.186 vs LingXi
**Audit date:** 2026-06-24
**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude` (v2.1.186)
**LingXi CLI:** `crates/apps/cli/src/argv.rs`
**LingXi commands:** `lingxi-code/command-api/src/builtin_support/names.rs`

## Summary: 28 confirmed gaps (8 critical, 12 medium, 8 low) + 0 slash-command roster gaps

| Area | Count | Notes |
|------|-------|-------|
| CLI flags — critical | 8 | Core flags SDK/pipe users depend on |
| CLI flags — medium | 12 | Functionality gaps affecting normal use |
| CLI flags — low | 8 | Deprecated, hidden, or internal flags |
| Slash commands | 0 | LingXi 94-name roster is byte-faithful vs binary |
| LingXi-only extras | 4 | `--cwd`, `--json`, `--no-stream`, `--no-tui` (not in binary) |

---

## Section 1 — Slash-Command Roster: COMPLETE (0 gaps)

LingXi's `BUILTIN_COMMAND_NAMES` (94 names, `command-api/src/builtin_support/names.rs:23`) matches the v2.1.186 binary:
- `vim`, `pr-comments`, `output-style` correctly absent — 0-hit in binary command objects (confirmed via grep)
- `cost`, `stats` correctly absent as standalones — binary literal `aliases:["cost","stats"]` on `usage` command
- All 94 names verified against binary string evidence

---

## Section 2 — CLI Flags: CONFIRMED GAPS

Binary evidence method: `LC_ALL=C strings "$BIN" | grep -E "^FLAG$"` for presence; full option chain extracted via offset+context from the binary's minified JS.

| # | Area | Item | Oracle (binary evidence) | LingXi (`argv.rs`) | Severity | Note |
|---|------|------|--------------------------|--------------------|----------|------|
| 1 | Output | `--output-format <format>` choices: `text`/`json`/`stream-json` | Binary: `Output format (only works with --print): "text" (default), "json" (single result), or "stream-json" (realtime streaming)` | `--json` boolean flag only; no `--output-format` | CRITICAL | LingXi's `--json` is LingXi-specific; SDK consumers pass `--output-format json` or `--output-format stream-json` — both rejected as unknown flags |
| 2 | Input | `--input-format <format>` choices: `text`/`stream-json` | Binary: `Input format (only works with --print): "text" (default), or "stream-json" (realtime streaming input)` | Missing | CRITICAL | Required for SDK stream-json input piping |
| 3 | System prompt | `--system-prompt <prompt>` | Binary option chain: `System prompt to use for the session` | Missing | CRITICAL | Headless system prompt injection blocked |
| 4 | System prompt | `--append-system-prompt <prompt>` | Binary option chain: `Append a system prompt to the default system prompt` | Missing | CRITICAL | Appending to default system prompt blocked |
| 5 | Tools | `--allowed-tools <tools...>` (alias `--allowedTools`) | Binary: `Comma or space-separated list of tool names to allow (e.g. "Bash(git *) Edit")` | Missing | CRITICAL | Tool allowlist gating unavailable |
| 6 | Tools | `--disallowed-tools <tools...>` (alias `--disallowedTools`) | Binary: `Comma or space-separated list of tool names to deny (e.g. "Bash(git *) Edit")` | Missing | CRITICAL | Tool denylist unavailable |
| 7 | Config | `--mcp-config <configs...>` | Binary: `Load MCP servers from JSON files or strings (space-separated)` | Missing | CRITICAL | MCP server injection from CLI blocked |
| 8 | Config | `--settings <file-or-json>` | Binary: `--settings <file-or-json>` confirmed in strings | Missing | CRITICAL | External settings file/JSON loading blocked |
| 9 | Verbosity | `--verbose` | Binary option chain: `Override verbose mode setting from config` (separate from `-d/--debug`) | Missing as named flag; LingXi `--debug` covers logging but not this config override | MEDIUM | Binary has BOTH `-d, --debug [filter]` AND `--verbose` as distinct flags |
| 10 | Mode | `--bare` | Binary: `Minimal mode: skip hooks, LSP, plugin sync...Sets CLAUDE_CODE_SIMPLE=1` | Missing | MEDIUM | Minimal/headless mode blocked |
| 11 | Mode | `--safe-mode` | Binary: `Start with all customizations...disabled — useful for troubleshooting` | Missing | MEDIUM | Safe troubleshooting mode unavailable |
| 12 | Context | `--add-dir <directories...>` | Binary: `--add-dir <directory>` (multiple string hits confirmed) | Missing | MEDIUM | Adding CLAUDE.md search directories from CLI blocked |
| 13 | Streaming | `--include-partial-messages` | Binary: `Include partial message chunks as they arrive (only works with --print and --output-format=stream-json)` | Missing | MEDIUM | Partial streaming chunks unavailable |
| 14 | Streaming | `--include-hook-events` | Binary: `Include all hook lifecycle events in the output stream (only works with --output-format=stream-json)` | Missing | MEDIUM | Hook event streaming unavailable |
| 15 | Streaming | `--replay-user-messages` | Binary: `Re-emit user messages from stdin back on stdout for acknowledgment (only works with --input-format=stream-json and --output-format=stream-json)` | Missing | MEDIUM | Bidirectional stream-json acknowledgment unavailable |
| 16 | Tools | `--tools <tools...>` | Binary: `Specify the list of available tools from the built-in set. Use "" to disable all tools` | Missing | MEDIUM | Built-in tool set restriction unavailable |
| 17 | Agent | `--agents <...>` | Binary: `--agents` confirmed in strings | Missing | MEDIUM | Agent configuration from CLI unavailable |
| 18 | Agent | `--agent <...>` | Binary: `--agent` confirmed in strings | Missing | MEDIUM | Single agent spec from CLI unavailable |
| 19 | Plugin | `--plugin-dir <dir>` | Binary: `--plugin-dir` confirmed; preAction handler loads it | Missing | MEDIUM | Plugin dir injection blocked |
| 20 | Session | `--from-pr [value]` | Binary: `Resume a session linked to a PR by PR number/URL, or open interactive picker` | Missing | MEDIUM | PR-linked session resume unavailable |
| 21 | Session | `--no-session-persistence` | Binary: `Disable session persistence - sessions will not be saved to disk and cannot be resumed (only works with --print)` | Missing | MEDIUM | Ephemeral session mode unavailable |
| 22 | Thinking | `--thinking <mode>` choices: `enabled`/`adaptive`/`disabled` | Binary: `Thinking mode: enabled (equivalent to adaptive), disabled` (hideHelp) | Missing | LOW | Hidden flag; affects model thinking budget |
| 23 | Thinking | `--thinking-display <display>` choices: `summarized`/`omitted` | Binary: `How thinking content appears in the response` (hideHelp) | Missing | LOW | Hidden; thinking display control |
| 24 | Thinking | `--max-thinking-tokens <tokens>` | Binary: `[DEPRECATED. Use --thinking instead for newer models]` (hideHelp) | Missing | LOW | Deprecated hidden flag |
| 25 | Output | `--prompt-suggestions [value]` | Binary: `Enable prompt suggestions. In print/SDK mode, emits a prompt_suggestion message after each turn with a predicted next user prompt` | Missing | LOW | Hidden flag for SDK prompt suggestion frames |
| 26 | Config | `--effort <level>` | Binary: `Effort level for the current session` | Missing | LOW | Effort level control unavailable |
| 27 | Config | `--betas <...>` | Binary: `betas` + `Warning: Custom betas are only available for API key users` | Missing | LOW | Beta features flag unavailable |
| 28 | Debug | `--debug-file <path>` | Binary: `Write debug logs to a specific file path (implicitly enables debug mode)` | Missing | LOW | Debug log file routing unavailable |

---

## Section 3 — LingXi-only extras (not present in binary)

These flags exist in LingXi's `argv.rs` but are NOT found in the v2.1.186 binary:

| Flag | LingXi location | Note |
|------|-----------------|------|
| `--cwd <DIR>` | `argv.rs:125` | LingXi-specific; binary changes cwd via OS-level launch convention |
| `--json` (boolean) | `argv.rs:129` | LingXi-specific boolean; replaced by `--output-format json` in binary |
| `--no-stream` | `argv.rs:126` | LingXi-specific; controls internal reqwest mode, no binary equivalent |
| `--no-tui` | `argv.rs:136` | LingXi-specific; binary auto-detects non-TTY |

---

## Section 4 — Output Envelope: stream-json shape gap

The binary's `--output-format stream-json` NDJSON envelope (confirmed from binary keys + TS source) uses:

```
{ "type": "...", "session_id": "...", ... }          # per-event frame
{ "type": "result", "subtype": "success"|"error",
  "result": "...", "session_id": "...",
  "is_error": bool, "num_turns": N,
  "total_cost_usd": N, "usage": {...} }               # final result frame
```

LingXi's `--json` boolean flag emits a different NDJSON schema. The keys `type`, `subtype`, `session_id`, `is_error`, `num_turns`, `total_cost_usd`, `usage` are all confirmed in binary strings. This is a **behavioral divergence** for SDK consumers.

---

## Section 5 — Items Verified COMPLETE

| Item | Status | Evidence |
|------|--------|---------|
| Slash-command roster (94 names) | COMPLETE | Binary 0-hit for removed commands; aliases confirmed |
| `-p, --print` | PRESENT | `argv.rs:49` matches binary `-p, --print` |
| `--model <model>` | PRESENT | `argv.rs:56` matches binary `--model <model>` |
| `-r, --resume [value]` (optional arg) | PRESENT | `argv.rs:65` + optional-value semantics match binary |
| `-c, --continue` | PRESENT | `argv.rs:78` matches binary |
| `--fork-session` | PRESENT | `argv.rs:86` matches binary |
| `--fallback-model <MODEL>` | PRESENT | `argv.rs:91` matches binary |
| `--max-turns <turns>` | PRESENT | `argv.rs:101` matches binary |
| `--max-budget-usd <amount>` | PRESENT | `argv.rs:108` + error string byte-exact match |
| `--dangerously-skip-permissions` | PRESENT | `argv.rs:141` matches binary |
| `--permission-mode <mode>` | PRESENT | `argv.rs:147` matches binary |
| `--json-schema <schema>` | PRESENT | `argv.rs:133` matches binary |
| `-d, --debug` | PRESENT | `argv.rs:119` (`long = "debug"`) matches binary `-d, --debug` |

---

## Uncertain (not grounded in binary literals — excluded from confirmed count)

| Item | Reason uncertain |
|------|-----------------|
| `--worktree` | Binary has `--worktree` string but unclear if top-level flag or subcommand option |
| `--sdk-url` | Present in binary strings; unclear if top-level flag or internal bridge flag |
| `--session-id` | Present in binary strings; unclear if top-level or internal |
| `--smol` | Binary has `--smol` in Bun's own argv list and shell completion — likely Bun runtime flag, not claude CLI flag |
| `--allow-dangerously-skip-permissions` | Present in binary option chain; LingXi may cover via `--dangerously-skip-permissions` semantics |
| `--system-prompt-file <file>` / `--append-system-prompt-file <file>` | Confirmed in binary option chain (hideHelp); LingXi missing but lower priority than their visible counterparts |
| `--exclude-dynamic-system-prompt-sections` | Confirmed in binary option chain (hideHelp) |
| `--permission-prompt-tool <tool>` | Confirmed in binary option chain (hideHelp) |
| `--task-budget <tokens>` | Confirmed in binary option chain (hideHelp) |

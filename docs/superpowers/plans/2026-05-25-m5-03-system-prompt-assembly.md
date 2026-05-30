# LingXi Core M5 · Plan 03 · System prompt 动态组装 — `<env>` + `<memory>` + `<tools>` blocks

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Multi-commit allowed** — every task ends with its own commit. The verification gate (final task) is the workspace-wide guard.

**Goal:** Produce a deterministic, 1:1-with-claude-code system prompt assembler inside the `lingxi-orchestrator` crate and wire it into `ConversationOrchestrator::run_turn` so that every API call sends a system prompt that contains, in this **locked order**:

1. Header literal — `"You are Claude Code, Anthropic's official CLI for Claude."` (the `DEFAULT_PREFIX` from `claude-code/src/constants/system.ts:10`).
2. An `<env>...</env>` block carrying `cwd`, git status, file tree (depth = 2), platform, model, knowledge cutoff (mirrors `computeEnvInfo` at `claude-code/src/constants/prompts.ts:606-649`).
3. A `<memory>...</memory>` block holding the CLAUDE.md hierarchy in **highest-priority-last** order: `~/.claude/CLAUDE.md` → `<repo>/CLAUDE.md` → `<repo>/CLAUDE.local.md` (matches M3-02 hierarchy lock; spec §4.3).
4. A `<tools>...</tools>` block listing available tool NAMES (one per line). Full tool schemas remain wire-side via the API `tools: [...]` array — the prompt only carries names so the model knows what to call by name. Locked at this plan.
5. Footer literal — the `notes:` paragraph from `claude-code/src/constants/prompts.ts:766-770` (`enhanceSystemPromptWithEnvDetails`).

After this plan, `ConversationOrchestrator::run_turn(prompt)`:

- When `config.system_prompt_override.is_none()`: synchronously builds `SystemPromptContext` from the orchestrator's cwd / model / memory store / tool registry / git probe / file tree probe, calls `assemble_system_prompt(ctx) -> String`, and passes the result into `OrchestratorApiClient::messages_create(model, msgs, system)` (trait gains a third arg this plan).
- When `Some(custom)`: bypasses `assemble_system_prompt` and forwards `custom` verbatim. The override path is a single byte-level pass-through with one unit test asserting no rewriting.

This plan ships:

- A new submodule `lingxi_orchestrator::prompt` (5 files) carrying `assemble_system_prompt`, `SystemPromptContext`, plus three formatters (`env_block`, `memory_block`, `tools_block`) and one `locked_templates` constants file.
- One trait surface change: `OrchestratorApiClient::messages_create` gains `system: Option<&str>` as its third arg. `MockApiClient` is updated to capture it; the adapter forwards it; `MessageRequest.system` (already `Option<String>` per `lingxi-code/crates/api-client/src/types.rs:23`) is now populated for real. `AnthropicProvider::messages_create_non_stream` gains a `system: Option<&str>` arg (additive — call sites in the workspace tree are M5-02's orchestrator and the api-client's own tests; both are updated).
- `ConversationOrchestrator` gains one new field: `memory: Arc<dyn MemoryHierarchyProvider>` (new trait in `lingxi-orchestrator::prompt::memory_block` to keep `lingxi-traits` minimal — see Critical fidelity notes).
- 11 integration test files under `crates/orchestrator/tests/prompt_*` (one per task that ships behavior) plus extensions to two M5-02 tests.

**No new telemetry events.** Spec §3 row M5-03 freezes the new-event count at **0**. `ALL_EVENT_NAMES` stays at the post-M5-02 count of **241**. Task 14 step 3 explicitly re-asserts the count to catch accidental additions.

**Tech Stack:** Rust 2021, `async-trait 0.1` (workspace), `serde 1` + `serde_json 1` (workspace), `tokio 1` (workspace, `sync::Mutex`), `thiserror 2` (workspace). One new third-party dep added to `lingxi-orchestrator` only: `gix 0.66` (workspace candidate) for git status probing — see Task 7. **No new workspace-level dep.** If `gix` is not already in the workspace `[workspace.dependencies]` table, Task 7 adds it and pins.

**References:**

- Spec: `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md` (committed at `1dbb9b8`).
  - §3 sub-plan row M5-03 (line 179) — "assemble cwd/git/files/CLAUDE.md/tools schema; 0 events; ~14 tasks".
  - §4.3 system prompt template lock (lines 258-264) — order: header → env → tools → memory → footer; spec calls out "全部待 reverse-engineer from `src/services/prompts.ts`". This plan resolves that reverse-engineer (Critical 1:1 fidelity items below).
  - §7 OQ-1 (line 491) + OQ-12 (claude-code version baseline) — claude-code 1.0.84-ish HEAD, no version-pinned tag in the engine.
- Predecessor: **M5-02 — ConversationOrchestrator core** committed at `653de44`. Verifies:
  - `lingxi-orchestrator/src/lib.rs` exists with `pub mod prompt;` NOT yet added (Task 1 adds it).
  - `OrchestratorConfig.system_prompt_override: Option<String>` field exists (M5-02 Task 5 — `crates/orchestrator/src/config.rs`).
  - `ConversationOrchestrator` exists with fields `{ config, api, tools, hooks, perms, output, session }` (M5-02 Task 10 — `crates/orchestrator/src/conversation.rs:1302-1310`). This plan adds `memory: Arc<dyn MemoryHierarchyProvider>` and `cwd: PathBuf` as the 8th and 9th fields (Task 12).
  - `OrchestratorApiClient::messages_create(&self, model, msgs)` trait — this plan modifies the signature additively (Task 12).
  - 3 events from M5-02 (`tengu_orchestrator_conversation_started/completed/failed`) still fire from `run_turn`. They MUST continue to fire in the same order after M5-03 wires the prompt assembler (Task 14 step 2).
- claude-code source (reverse-engineered for this plan):
  - `claude-code/src/constants/system.ts:10` — `DEFAULT_PREFIX = "You are Claude Code, Anthropic's official CLI for Claude."`. Byte-locked into `locked_templates::HEADER`.
  - `claude-code/src/constants/prompts.ts:606-649` — `computeEnvInfo` function: produces a `<env>...</env>` block with lines `Working directory: ${cwd}`, `Is directory a git repo: ${isGit ? 'Yes' : 'No'}`, `Platform: ${env.platform}`, `Shell: ${shellName}` (or with Windows tail), `OS Version: ${unameSR}`, then trailing model + cutoff lines OUTSIDE the `<env>` block. This plan ports the **inside-the-tags** lines verbatim (Task 6). The model/cutoff lines move into the trailing footer block in our port (Task 11). claude-code's `Additional working directories:` line is OUT of scope for M5-03 (`additionalWorkingDirectories` is null for now — added in M5-12 CLI).
  - `claude-code/src/constants/prompts.ts:651-710` — `computeSimpleEnvInfo` is an ALTERNATIVE rendering. LingXi M5-03 picks **the `computeEnvInfo` shape** (line 640-648, the `<env>...</env>` tagged shape) because the spec lock §4.3 mandates the tag delimiter. Task 6 locks bytes against this shape.
  - `claude-code/src/constants/prompts.ts:758` — `DEFAULT_AGENT_PROMPT` — NOT used by M5-03 (that's the subagent system prompt, lands in M5-06 / M5-12 wiring). Documented here so future agent-loop work in M5-06 doesn't get confused.
  - `claude-code/src/constants/prompts.ts:766-770` — the `notes:` literal:
    ```
    Notes:
    - Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.
    - In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) — do not recap code you merely read.
    - For clear communication with the user the assistant MUST avoid using emojis.
    - Do not use a colon before tool calls. Text like "Let me read the file:" followed by a read tool call should just be "Let me read the file." with a period.
    ```
    M5-03 uses the FIRST line (`Notes:`) and the 4 bullets ONLY (no skill-discovery insert). Tooltip `enhanceSystemPromptWithEnvDetails` adds more bullets when subagents run, but M5-03 is the main-session shape (subagent variant lives in M5-06).
  - `claude-code/src/memdir/memdir.ts:419` — `loadMemoryPrompt(): Promise<string | null>`. We do NOT port the memdir prompt format (that's the `# auto memory` / `# user memory` framing, separate concern). M5-03 ports ONLY the CLAUDE.md hierarchy splice (Task 9).
- Existing surfaces consumed by this plan:
  - `lingxi-code/crates/memory/src/claude_md/hierarchy.rs:36` — `walk(cwd, home) -> Hierarchy`. Returns entries **innermost-first** (cwd → parents → `<home>/.claude`). Task 9 REVERSES this order so the spec's "home → repo → local-override" splice order is honored (highest-priority shadowed by being LAST).
  - `lingxi-code/crates/memory/src/claude_md/hierarchy.rs:10-20` — `HierarchyEntry { path, is_local_override, exact_case }`. Task 9 reads `path` + `is_local_override` to drive labelling.
  - `lingxi-code/crates/memory/src/claude_md/loader.rs:49` — `load_file(path, bus) -> Result<LoadedFile, LoaderError>`. Used by `RealMemoryHierarchyProvider` (Task 9 step 3). 10 MB cap stays in force; oversized files are skipped (not fatal).
  - `lingxi-code/crates/tools/src/registry.rs:23` — `ToolRegistry`. We add NO new method to it; instead the new `tools_block::format` accepts `&[String]` and the orchestrator pre-extracts names via `tools.available_tools(&ctx).iter().map(|t| t.name().to_string()).collect()`. Spec lock: names only, NOT full schemas. Schemas are passed to the API via `tools: [...]` (out of scope for the prompt assembler).
  - `lingxi-code/crates/api-client/src/types.rs:23` — `MessageRequest.system: Option<String>` is already wired in the DTO. Task 12 step 1 makes `AnthropicProvider::messages_create_non_stream` populate it from a new `system: Option<&str>` arg.
  - `lingxi-code/crates/api-client/src/anthropic.rs:211` — `messages_create_non_stream`. Task 12 step 1 modifies the signature additively.
  - `lingxi-code/crates/orchestrator/src/conversation.rs:1302-1310` — `ConversationOrchestrator` struct from M5-02. Task 12 step 4 adds two fields: `memory: Arc<dyn MemoryHierarchyProvider>` and `cwd: PathBuf`.
  - `lingxi-code/crates/orchestrator/src/turn_loop.rs::execute_one_turn` — M5-02 Task 10 step 2 (line 1443+). Task 12 step 5 modifies it to thread `system: Option<&str>` through to `api.messages_create`.
- Repo conventions (M4-01..09 + M5-02 precedent reaffirmed):
  - Tests live in `#[cfg(test)] mod tests { ... }` blocks adjacent to production code.
  - Integration tests live in `crates/<crate>/tests/<name>_test.rs`.
  - Every error string visible in tests is the EXACT byte sequence in production (`assert_eq!`-quality).
  - Crate-level `#![forbid(unsafe_code)]` is mandatory in every new file under `lingxi-orchestrator/src/prompt/` (M5-02 set the precedent on `lib.rs` only; this plan extends to per-submodule for clarity).
  - No new telemetry events in this plan.

---

## Reverse-engineered byte-locks (T0 — captured at plan-writing time)

These were extracted from `claude-code/src/constants/system.ts` and `claude-code/src/constants/prompts.ts` at the version pinned by the repo's `claude-code/` submodule (matches the working-tree files at plan-writing). Each lock cites its source line.

| Lock id | Value | Source |
|---|---|---|
| `HEADER` | `You are Claude Code, Anthropic's official CLI for Claude.` (NO trailing newline; one trailing `\n\n` is added by `assemble_system_prompt` as section separator) | `claude-code/src/constants/system.ts:10` (`DEFAULT_PREFIX` literal) |
| `<env>` opener | `<env>\n` (no leading space; one newline after `>`) | `claude-code/src/constants/prompts.ts:641` |
| `<env>` line 1 | `Working directory: {cwd}\n` (one space after colon) | `claude-code/src/constants/prompts.ts:642` |
| `<env>` line 2 | `Is directory a git repo: {Yes\|No}\n` | `claude-code/src/constants/prompts.ts:643` |
| `<env>` line 3 | `Platform: {platform}\n` (`env.platform` from `process.platform` ≈ `darwin`/`linux`/`win32`) | `claude-code/src/constants/prompts.ts:644` |
| `<env>` line 4 | `Shell: {shellName}\n` (zsh/bash; on Windows tail adds `(use Unix shell syntax, not Windows — e.g., /dev/null not NUL, forward slashes in paths)`) | `claude-code/src/constants/prompts.ts:732-743` |
| `<env>` line 5 | `OS Version: {unameSR}\n` (`os.type()` + `os.release()`) | `claude-code/src/constants/prompts.ts:645,745-756` |
| `<env>` closer | `</env>\n` (newline after `>`) | `claude-code/src/constants/prompts.ts:647` |
| Section separator | `\n\n` (double newline between header, env, memory, tools, footer) | `claude-code/src/constants/prompts.ts:646-648` (model description follows `</env>\n` literally; we generalize this gap to `\n\n` between all top-level sections) |
| Model description | `You are powered by the model named {marketingName}. The exact model ID is {modelId}.` (when marketing name known) OR `You are powered by the model {modelId}.` (when no marketing name) | `claude-code/src/constants/prompts.ts:625-627` |
| Knowledge cutoff | `Assistant knowledge cutoff is {cutoff}.` (cutoff value from `getKnowledgeCutoff(modelId)` — opus-4: January 2025; opus-4-5: May 2025; opus-4-6: May 2025; sonnet-4-6: August 2025; haiku-4: February 2025) | `claude-code/src/constants/prompts.ts:636-638,713-730` |
| `FOOTER` (Notes literal) | 5 lines, exact bytes (LF terminators), see "FOOTER block (Task 11)" below | `claude-code/src/constants/prompts.ts:766-770` |
| `<memory>` opener / closer | NEW lock for LingXi (claude-code does NOT use XML-tagged memory; it inlines memdir prompt as a separate section in the dynamic-sections array). LingXi locks `<memory>\n` / `</memory>\n` per spec §4.3. | LingXi M5-03 lock — spec §4.3 line 262 |
| `<memory>` entry header | `# {path}\n\n{body}\n\n` per entry (path is ABSOLUTE per M3-02 `LoadedFile.path`; trailing blank line between entries) | LingXi M5-03 lock |
| `<tools>` opener / closer | NEW lock for LingXi. `<tools>\n` / `</tools>\n`. | LingXi M5-03 lock — spec §4.3 line 262 |
| `<tools>` entry line | `- {tool_name}\n` (hyphen, space, name, newline) | LingXi M5-03 lock |

**Source provenance:** All claude-code line numbers were captured by `grep -n` during plan-writing (Task 0 work). The `<env>` block format mirrors `computeEnvInfo` (lines 606-649) NOT `computeSimpleEnvInfo` (lines 651-710), because the spec lock §4.3 explicitly uses XML tags. claude-code itself uses both shapes depending on `feature('PROACTIVE')` — LingXi picks the tagged shape for byte-stability and future cache-prefix work.

---

## Critical 1:1 fidelity items (locked)

- **`HEADER` literal** — exactly `You are Claude Code, Anthropic's official CLI for Claude.` (60 bytes plus terminating NUL of the Rust `&'static str`, no LF inside). The full bytes constant lives in `crates/orchestrator/src/prompt/locked_templates.rs::HEADER`. Task 4 step 2 asserts the byte length AND a starts-with/ends-with pair against drift.

- **`<env>` block ordering** — line 1 `Working directory`, line 2 `Is directory a git repo`, line 3 `Platform`, line 4 `Shell`, line 5 `OS Version`. NO additional fields in this plan. `git_branch` and `working_dir_clean` are PRESENT inside the `<env>` block (post line 2 `Is directory a git repo` but BEFORE line 3 `Platform`) when the cwd is a git repo, as new LingXi-locked lines (Task 7). claude-code's prompt does not surface branch/clean-status; **LingXi extends** this for the conversational agent loop UX. Locks: line `  Git branch: {branch}\n` and `  Working tree clean: {true\|false}\n` (two-space indent — this differs from line 1-5 which are unindented; the indent intentionally signals "augmented" lines).

- **Memory hierarchy splice order** — `~/.claude/CLAUDE.md` → `<repo>/CLAUDE.md` → `<repo>/CLAUDE.local.md`. Innermost wins via the later-in-text trick (the model attends more to text closer to the user message). M3-02 `walk()` returns innermost-first; Task 9 step 2 REVERSES that vector to honor the spec lock. Test `memory_block_order_home_then_repo_then_local_override` (Task 9 step 6) asserts this exactly.

- **Tools block — names only** — full JSON schemas go via `MessageRequest.tools: Vec<Value>` (out of scope this plan). The `<tools>` block in system prompt carries one name per line (sorted alphabetically by `tool_name` for byte-stability across reorderings inside the registry). Task 10 step 4 asserts alphabetical order via a regression test.

- **`SystemPromptContext` field names locked** — `cwd: PathBuf`, `platform: String`, `model: String`, `model_marketing_name: Option<String>`, `knowledge_cutoff: Option<String>`, `shell: String`, `os_version: String`, `git_status: Option<GitStatus>`, `file_tree: FileTree`, `memory_files: Vec<MemoryFile>`, `tool_names: Vec<String>`. Field order in the struct literal MUST match this order for `Debug` output stability. Serde NOT derived on this struct (it's runtime context, not persisted). Tasks 2 + 3 fix the field set.

- **`GitStatus` struct** — `pub struct GitStatus { pub branch: String, pub working_dir_clean: bool, pub file_changes_summary: String }`. `file_changes_summary` is the literal output of `git diff --stat --cached` + `git diff --stat` joined by `\n`, truncated at 4 KB. When the repo is clean, `file_changes_summary` is the empty string and `working_dir_clean: true`. Task 3 step 2 asserts the field set.

- **`FileTree` struct** — `pub struct FileTree { pub entries: Vec<FileTreeEntry> }` + `pub struct FileTreeEntry { pub path: PathBuf, pub is_dir: bool, pub depth: u8 }`. `depth: 0` means cwd direct children; `depth: 1` means one level deeper. Max depth = 2 (cwd + 2 levels — locked by spec §4.3). Sort: directories before files at each level; alphabetical within. Task 8 step 4 asserts ordering.

- **`MemoryFile` struct** — `pub struct MemoryFile { pub path: PathBuf, pub body: String, pub is_local_override: bool }`. The `body` is pre-trimmed (leading/trailing whitespace stripped). Empty body (file present but whitespace-only) → skip entry entirely (Task 9 step 4).

- **`MemoryHierarchyProvider` trait** — lives in `lingxi-orchestrator::prompt::memory_block`, NOT in `lingxi-traits` (the trait would otherwise force a `lingxi-memory` dep on the lean trait crate). The trait has ONE method:
  ```rust
  #[async_trait]
  pub trait MemoryHierarchyProvider: Send + Sync {
      async fn load(&self, cwd: &std::path::Path) -> Vec<MemoryFile>;
  }
  ```
  The real impl is `RealMemoryHierarchyProvider` which wraps `lingxi_memory::claude_md::walk` + `load_file`. Test fixture `StaticMemoryProvider { files: Vec<MemoryFile> }` lives in `crates/orchestrator/src/test_support.rs` (extends the M5-02 file).

- **`assemble_system_prompt` signature** — `pub fn assemble_system_prompt(ctx: &SystemPromptContext) -> String`. Synchronous (no `async`) — the caller (`run_turn`) has already done the I/O to populate `ctx`. Returns a single `String` with all sections concatenated. Empty sections (no memory files, no tools, no git status) are **omitted entirely** rather than rendered as empty tags — Task 11 step 2 asserts this.

- **Section ordering in `assemble_system_prompt`** — locked:
  1. `HEADER` (always present)
  2. `\n\n` separator
  3. `env_block::format(ctx)` (always present — at minimum cwd + platform + model)
  4. `\n\n` separator
  5. `memory_block::format(&ctx.memory_files)` (omit entirely if `memory_files.is_empty()`)
  6. `\n\n` separator (only if memory section was emitted)
  7. `tools_block::format(&ctx.tool_names)` (omit entirely if `tool_names.is_empty()`)
  8. `\n\n` separator (only if tools section was emitted)
  9. `FOOTER` (always present)
  10. trailing `\n` (the spec implicit single-newline terminator)

- **`OrchestratorApiClient::messages_create` signature change** — additive `system: Option<&str>` as the third parameter (after `model`, before `msgs` so callers can pass `system` positionally):
  ```rust
  async fn messages_create(
      &self,
      model: &str,
      system: Option<&str>,
      msgs: Vec<ConversationMessage>,
  ) -> Result<MessageResponse, ApiError>;
  ```
  `MockApiClient::captured_msgs` is extended with `captured_systems: Arc<Mutex<Vec<Option<String>>>>` to verify what the orchestrator passed (Task 12 step 3). `AnthropicProvider::messages_create_non_stream` gains the same arg in the same position (Task 12 step 1).

- **`AnthropicProvider::messages_create_non_stream` body change** — when `system.is_some()`, the JSON body gains a top-level `"system": <s>` key. Done via `serde_json::json!`'s conditional construction:
  ```rust
  let mut body = serde_json::json!({
      "model": model,
      "max_tokens": 4096u32,
      "messages": msgs,
  });
  if let Some(s) = system {
      body["system"] = serde_json::Value::String(s.to_string());
  }
  ```
  Order of keys in the serialized JSON matters for the Anthropic API ONLY in that `system` must be a top-level key (it is). Anthropic accepts any key order.

- **NO new telemetry events** — Task 14 step 3 asserts `lingxi_telemetry::tengu::ALL_EVENT_NAMES.len() == 241` (unchanged from M5-02). If a future engineer adds an event in this plan by mistake, this assertion fails the build.

- **NO `lingxi-memory` dep cycle** — `lingxi-orchestrator` already does NOT depend on `lingxi-memory` (M5-02 dep list). This plan ADDS `lingxi-memory = { path = "../memory" }` to `crates/orchestrator/Cargo.toml`. Task 9 step 1 verifies no cycle: memory crate does NOT pull orchestrator. `cargo tree -p lingxi-memory -e normal --depth 5 2>&1 | grep -c lingxi-orchestrator` must print `0` after Task 9 — covered in Task 14 step 4.

---

## File touch inventory (locked at top per spec Appendix A convention)

**Creates (new files — all under `lingxi-code/crates/orchestrator/src/prompt/` unless noted):**

- `lingxi-code/crates/orchestrator/src/prompt/mod.rs` — module declarations + `SystemPromptContext` + `GitStatus` + `FileTree` + `FileTreeEntry` + `MemoryFile` + `assemble_system_prompt`.
- `lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs` — `HEADER` + `FOOTER` + section delimiters as `pub const &'static str` constants.
- `lingxi-code/crates/orchestrator/src/prompt/env_block.rs` — `pub fn format(ctx: &SystemPromptContext) -> String` for the `<env>...</env>` block.
- `lingxi-code/crates/orchestrator/src/prompt/memory_block.rs` — `MemoryHierarchyProvider` trait + `RealMemoryHierarchyProvider` struct + `pub fn format(files: &[MemoryFile]) -> String`.
- `lingxi-code/crates/orchestrator/src/prompt/tools_block.rs` — `pub fn format(names: &[String]) -> String`.
- `lingxi-code/crates/orchestrator/src/prompt/file_tree.rs` — `pub fn probe(cwd: &Path, depth_limit: u8) -> FileTree` + `pub fn format(tree: &FileTree) -> String`.
- `lingxi-code/crates/orchestrator/src/prompt/git_status.rs` — `pub fn probe(cwd: &Path) -> Option<GitStatus>` using `gix`.
- `lingxi-code/crates/orchestrator/tests/prompt_header_test.rs` — byte-locks for `HEADER`.
- `lingxi-code/crates/orchestrator/tests/prompt_env_block_test.rs` — env block ordering + format.
- `lingxi-code/crates/orchestrator/tests/prompt_git_status_test.rs` — git probe end-to-end against a temp repo.
- `lingxi-code/crates/orchestrator/tests/prompt_file_tree_test.rs` — file tree probe + format at depth 2.
- `lingxi-code/crates/orchestrator/tests/prompt_memory_block_test.rs` — memory hierarchy splice order + empty branch.
- `lingxi-code/crates/orchestrator/tests/prompt_tools_block_test.rs` — alphabetic order + empty branch.
- `lingxi-code/crates/orchestrator/tests/prompt_assemble_test.rs` — end-to-end `assemble_system_prompt` shape.
- `lingxi-code/crates/orchestrator/tests/prompt_orchestrator_wiring_test.rs` — full `run_turn` invocation observes `messages_create` was called with the expected `system` arg.
- `lingxi-code/crates/orchestrator/tests/prompt_override_bypass_test.rs` — `system_prompt_override: Some(...)` skips the assembler.

**Modifies (existing files):**

- `lingxi-code/crates/orchestrator/Cargo.toml` — add `lingxi-memory = { path = "../memory" }` and `gix = { version = "0.66", default-features = false, features = ["max-performance-safe"] }` to `[dependencies]`. `gix` is added with `default-features = false` and only `max-performance-safe` because we ONLY need read-only repo + status + HEAD branch — no networking, no pack writing.
- `lingxi-code/crates/orchestrator/src/lib.rs` — add `pub mod prompt;` declaration (Task 1) + a single `pub use prompt::{assemble_system_prompt, SystemPromptContext, GitStatus, FileTree, MemoryFile};` re-export (Task 12).
- `lingxi-code/crates/orchestrator/src/conversation.rs` — modify `OrchestratorApiClient::messages_create` signature (Task 12 step 2); add `memory` + `cwd` fields to `ConversationOrchestrator` struct + `new` constructor (Task 12 step 4); rewire `run_turn` to assemble system prompt before invoking `execute_one_turn` (Task 12 step 5).
- `lingxi-code/crates/orchestrator/src/turn_loop.rs` — modify `execute_one_turn` to accept a `system: Option<&str>` arg and thread it into `orch.api.messages_create(model, system, history_snapshot)` (Task 12 step 6).
- `lingxi-code/crates/orchestrator/src/test_support.rs` — extend `MockApiClient` with `captured_systems` field; update the trait impl (Task 12 step 3). Add `StaticMemoryProvider` fixture (Task 9 step 5).
- `lingxi-code/crates/api-client/src/anthropic.rs` — modify `messages_create_non_stream` signature to take `system: Option<&str>` (Task 12 step 1). Conditional body insertion. Internal call sites inside `api-client` tests are updated in the same task.
- `lingxi-code/crates/orchestrator/tests/orchestrator_smoke_test.rs` (from M5-02) — update construction to pass `Arc::new(StaticMemoryProvider::empty())` and a cwd (Task 13 step 1).
- `lingxi-code/crates/orchestrator/tests/orchestrator_multi_turn_test.rs` (from M5-02) — same construction update (Task 13 step 2).
- `lingxi-code/crates/orchestrator/tests/orchestrator_max_turns_test.rs` (from M5-02) — same (Task 13 step 3).
- `lingxi-code/crates/orchestrator/tests/orchestrator_tool_error_test.rs` (from M5-02) — same (Task 13 step 4).
- `lingxi-code/crates/orchestrator/tests/orchestrator_real_tools_test.rs` (from M5-02) — same (Task 13 step 5).
- `lingxi-code/Cargo.toml` — add `gix = "0.66"` to `[workspace.dependencies]` IF not already present (Task 7 step 1 checks; if present, this modification is skipped).

**Verifications (no modification, just read in tests):**

- `lingxi-code/crates/memory/src/claude_md/hierarchy.rs:36` — `walk(cwd, home)` signature unchanged.
- `lingxi-code/crates/memory/src/claude_md/loader.rs:49` — `load_file(path, bus)` signature unchanged.
- `lingxi-code/crates/api-client/src/types.rs:23` — `MessageRequest.system: Option<String>` still exists.
- `lingxi-code/crates/telemetry/src/tengu/mod.rs::ALL_EVENT_NAMES` length stays at 241 (Task 14 step 3).
- The 3 M5-02 telemetry events still fire from `run_turn` in the same order (Task 14 step 2).

---

## FOOTER block (Task 11)

The footer literal (used by `locked_templates::FOOTER`):

```text
Notes:
- Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.
- In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) — do not recap code you merely read.
- For clear communication with the user the assistant MUST avoid using emojis.
- Do not use a colon before tool calls. Text like "Let me read the file:" followed by a read tool call should just be "Let me read the file." with a period.
```

5 lines, separated by `\n` (LF), with `\n` after the last bullet. The em-dash character (U+2014, three UTF-8 bytes `0xE2 0x80 0x94`) appears once in the second bullet between `"do not recap code you merely read"` and the period. Task 11 step 3 asserts the byte length is exactly `709` (verified by `wc -c` on the literal at plan-writing time — re-verify when filling the constant).

---

## Tasks

### Task 0: Reverse-engineer claude-code prompts.ts (research only — NO commit)

**Files:** none (research only)

**Steps:**

- [ ] Step 1 — Run `grep -rln "You are Claude Code" claude-code/src/` and confirm the three hits:
  - `claude-code/src/coordinator/coordinatorMode.ts` (out of scope — coordinator path)
  - `claude-code/src/constants/prompts.ts` (the simple-path fallback at line 452 — out of scope for tagged shape)
  - `claude-code/src/constants/system.ts` (the `DEFAULT_PREFIX` constant — IN SCOPE; this is our `HEADER` source)

- [ ] Step 2 — Open `claude-code/src/constants/system.ts:10` and confirm the bytes of `DEFAULT_PREFIX`:
  ```ts
  const DEFAULT_PREFIX = `You are Claude Code, Anthropic's official CLI for Claude.`
  ```
  Length: 60 bytes (without backticks). Capture for `HEADER` constant.

- [ ] Step 3 — Open `claude-code/src/constants/prompts.ts:606-649` and confirm `computeEnvInfo` produces the `<env>...</env>` block with the exact lines listed in the byte-locks table above. Note the trailing `${modelDescription}${knowledgeCutoffMessage}` is OUTSIDE the `<env>` tags — LingXi keeps that placement (model/cutoff lines come AFTER `</env>` but are still part of the env_block::format output).

- [ ] Step 4 — Open `claude-code/src/constants/prompts.ts:766-770` and confirm the 5-line `Notes:` block. The em-dash character (`—`, U+2014) appears in bullet 2. Re-verify when filling `FOOTER` constant in Task 11.

- [ ] Step 5 — Open `claude-code/src/memdir/memdir.ts:419` (`loadMemoryPrompt`) and `buildMemoryPrompt` (line 272). Confirm claude-code uses a `# auto memory` / `# user memory` framing — NOT XML tags. LingXi M5-03 uses XML tags (`<memory>...</memory>`) per spec §4.3 lock. Document this deviation in the plan as already-locked (no rework).

- [ ] Step 6 — No commit. Capture findings in the "Reverse-engineered byte-locks" table above. Task 0 is done when the table cites correct source line numbers (already done at plan-writing time).

---

### Task 1: Scaffold `prompt/` submodule under `lingxi-orchestrator`

**Files:**
- Create: `lingxi-code/crates/orchestrator/src/prompt/mod.rs`
- Create: `lingxi-code/crates/orchestrator/src/prompt/env_block.rs`
- Create: `lingxi-code/crates/orchestrator/src/prompt/memory_block.rs`
- Create: `lingxi-code/crates/orchestrator/src/prompt/tools_block.rs`
- Create: `lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs`
- Create: `lingxi-code/crates/orchestrator/src/prompt/file_tree.rs`
- Create: `lingxi-code/crates/orchestrator/src/prompt/git_status.rs`
- Modify: `lingxi-code/crates/orchestrator/src/lib.rs`

**Steps:**

- [ ] Step 1 — Verify predecessor `653de44` (M5-02) is on `HEAD` and `lingxi-code/crates/orchestrator/src/lib.rs` exists. Run `git log -1 --oneline` (expect `653de44`-prefixed) and `test -f lingxi-code/crates/orchestrator/src/lib.rs && echo OK` (must print `OK`).

- [ ] Step 2 — Create `lingxi-code/crates/orchestrator/src/prompt/mod.rs` with a placeholder shell — declarations only, types filled in Tasks 2/3:
  ```rust
  //! System prompt assembler — produces the byte-locked LingXi system
  //! prompt by concatenating header / `<env>` / `<memory>` / `<tools>` /
  //! footer sections. See plan M5-03 for the source-of-truth byte-locks.
  //!
  //! Entry point: [`assemble_system_prompt`].
  #![forbid(unsafe_code)]

  pub mod env_block;
  pub mod file_tree;
  pub mod git_status;
  pub mod locked_templates;
  pub mod memory_block;
  pub mod tools_block;

  // Types + assembler land in Tasks 2/3/11.
  ```

- [ ] Step 3 — Create the six submodule files with placeholder bodies (filled in later tasks):
  - `prompt/env_block.rs`:
    ```rust
    //! `<env>...</env>` formatter. Filled in Task 6.
    #![forbid(unsafe_code)]
    ```
  - `prompt/file_tree.rs`:
    ```rust
    //! File tree probe + formatter. Filled in Task 8.
    #![forbid(unsafe_code)]
    ```
  - `prompt/git_status.rs`:
    ```rust
    //! Git status probe via `gix`. Filled in Task 7.
    #![forbid(unsafe_code)]
    ```
  - `prompt/locked_templates.rs`:
    ```rust
    //! Header + footer constants. Filled in Task 4 and Task 11.
    #![forbid(unsafe_code)]
    ```
  - `prompt/memory_block.rs`:
    ```rust
    //! `<memory>...</memory>` formatter + `MemoryHierarchyProvider` trait.
    //! Filled in Task 9.
    #![forbid(unsafe_code)]
    ```
  - `prompt/tools_block.rs`:
    ```rust
    //! `<tools>...</tools>` formatter. Filled in Task 10.
    #![forbid(unsafe_code)]
    ```

- [ ] Step 4 — Modify `lingxi-code/crates/orchestrator/src/lib.rs` — add `pub mod prompt;` declaration on a new line immediately AFTER the existing `pub mod turn_loop;` line. Do not add any `pub use` re-exports at this task (those land in Task 12 after the types are real).

- [ ] Step 5 — Run `cargo build -p lingxi-orchestrator`. Must succeed. The new `prompt` submodule compiles as an empty shell.

- [ ] Step 6 — Run `cargo tree -p lingxi-orchestrator -e normal --depth 2 2>&1 | grep -E "lingxi-memory|gix"` — expect ZERO matches (those deps are added in Task 7 / Task 9). Confirms scaffold-only state.

- [ ] Commit: `feat(M5-03 task 1): scaffold prompt/ submodule with 6 empty files in lingxi-orchestrator`

---

### Task 2: `SystemPromptContext` struct

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/mod.rs`

**Steps:**

- [ ] Step 1 — In `lingxi-code/crates/orchestrator/src/prompt/mod.rs`, AFTER the module declarations, add the `SystemPromptContext` struct. The field order is LOCKED — see Critical fidelity items. `MemoryFile`, `GitStatus`, `FileTree`, `FileTreeEntry` are forward-referenced; they land in Task 3.
  ```rust
  use std::path::PathBuf;

  /// Runtime context required to assemble a system prompt.
  ///
  /// Constructed by `ConversationOrchestrator::run_turn` once per turn
  /// (cheap to clone where needed; not persisted). All I/O has already
  /// happened by the time the assembler sees this — the assembler itself
  /// is purely synchronous string concatenation.
  ///
  /// Field order is LOCKED for `Debug` output stability — see M5-03 plan
  /// "Critical 1:1 fidelity items".
  #[derive(Debug, Clone)]
  pub struct SystemPromptContext {
      /// Current working directory — emitted as the first `<env>` line.
      pub cwd: PathBuf,
      /// `process::env::consts::OS` (`darwin` / `linux` / `windows` …).
      pub platform: String,
      /// Model ID actually being sent to the API (`claude-opus-4-7` …).
      pub model: String,
      /// Friendly marketing name, when known (`Opus 4.7`). When `None`,
      /// the env block falls back to the ID-only sentence.
      pub model_marketing_name: Option<String>,
      /// Knowledge cutoff text (e.g. `"January 2026"`). When `None`,
      /// the env block omits the cutoff line.
      pub knowledge_cutoff: Option<String>,
      /// Shell name (`zsh` / `bash` / …). Probed from `$SHELL` at the
      /// call site; the assembler does not re-probe.
      pub shell: String,
      /// `uname -sr` value (`Darwin 25.3.0` …).
      pub os_version: String,
      /// `Some(_)` when cwd is inside a git repo; otherwise `None`.
      pub git_status: Option<GitStatus>,
      /// Direct + once-recursive children of cwd (depth ≤ 2).
      pub file_tree: FileTree,
      /// CLAUDE.md hierarchy — already in spec splice order
      /// (home → repo → repo-local override). LingXi M3-02 lock.
      pub memory_files: Vec<MemoryFile>,
      /// Available tool names — alphabetic order. Sorting happens here,
      /// NOT in `tools_block::format`.
      pub tool_names: Vec<String>,
  }
  ```

  Forward-declare the types referenced from this struct so the module compiles standalone. Add at the top of the file (after the module decls but before `SystemPromptContext`):
  ```rust
  // Re-exported below — defined in this same module body in Task 3.
  ```

- [ ] Step 2 — Add unit test scaffolding at the bottom of `prompt/mod.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn context_constructs_from_minimal_inputs() {
          let ctx = SystemPromptContext {
              cwd: PathBuf::from("/tmp"),
              platform: "darwin".into(),
              model: "claude-opus-4-7".into(),
              model_marketing_name: Some("Opus 4.7".into()),
              knowledge_cutoff: Some("January 2026".into()),
              shell: "zsh".into(),
              os_version: "Darwin 25.3.0".into(),
              git_status: None,
              file_tree: FileTree { entries: Vec::new() },
              memory_files: Vec::new(),
              tool_names: Vec::new(),
          };
          assert_eq!(ctx.cwd, PathBuf::from("/tmp"));
          assert_eq!(ctx.model, "claude-opus-4-7");
          assert!(ctx.git_status.is_none());
      }
  }
  ```

  Note: this test will FAIL to compile until Task 3 lands `GitStatus`, `FileTree`, `MemoryFile`. That is acceptable — we are following TDD red-then-green.

- [ ] Step 3 — Run `cargo build -p lingxi-orchestrator 2>&1 | tail -5`. EXPECTED: compile errors for `GitStatus`, `FileTree`, `MemoryFile` (forward references). Document the expected error output in the commit message.

- [ ] Commit: `feat(M5-03 task 2): SystemPromptContext struct in prompt::mod (red: GitStatus/FileTree/MemoryFile forward refs)`

---

### Task 3: `GitStatus`, `FileTree`, `FileTreeEntry`, `MemoryFile` types

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/mod.rs`

**Steps:**

- [ ] Step 1 — In `lingxi-code/crates/orchestrator/src/prompt/mod.rs`, immediately AFTER `SystemPromptContext`, add the four supporting types:
  ```rust
  /// Result of a git-status probe over the cwd.
  ///
  /// `Some(GitStatus)` is returned by [`crate::prompt::git_status::probe`]
  /// when the cwd is inside a git repo. The fields are PII-safe (no file
  /// content, only summary).
  #[derive(Debug, Clone, Default)]
  pub struct GitStatus {
      /// Current branch name (`main`, `feat/x`). `HEAD detached` when in
      /// a detached state. NEVER empty.
      pub branch: String,
      /// `true` when both index and working tree are unmodified.
      pub working_dir_clean: bool,
      /// Output of `git diff --stat --cached` + `git diff --stat`,
      /// joined by `\n`, truncated at 4 KB. Empty when clean.
      pub file_changes_summary: String,
  }

  /// Snapshot of the file tree at depth ≤ 2 under cwd.
  #[derive(Debug, Clone, Default)]
  pub struct FileTree {
      /// Entries in display order (dirs before files at each level,
      /// alphabetic within). Populated by
      /// [`crate::prompt::file_tree::probe`].
      pub entries: Vec<FileTreeEntry>,
  }

  /// One entry in [`FileTree`].
  #[derive(Debug, Clone)]
  pub struct FileTreeEntry {
      /// Absolute path on disk.
      pub path: PathBuf,
      /// `true` when this is a directory; `false` when a regular file.
      pub is_dir: bool,
      /// `0` = direct child of cwd; `1` = grandchild. Never above 1
      /// for this assembler (depth limit = 2 means 0..=1).
      pub depth: u8,
  }

  /// One loaded CLAUDE.md (or `CLAUDE.local.md`) file.
  ///
  /// Distinct from [`lingxi_memory::claude_md::LoadedFile`] — the
  /// assembler keeps a leaner representation post-trim.
  #[derive(Debug, Clone)]
  pub struct MemoryFile {
      /// Absolute path on disk (PII-safe — only emitted into the
      /// system prompt, never into telemetry).
      pub path: PathBuf,
      /// File body, leading/trailing whitespace trimmed. NEVER empty
      /// (whitespace-only files are filtered upstream).
      pub body: String,
      /// `true` when this is a `CLAUDE.local.md`; `false` for `CLAUDE.md`.
      pub is_local_override: bool,
  }
  ```

- [ ] Step 2 — Extend the existing `#[cfg(test)] mod tests { ... }` block in `prompt/mod.rs` with three new tests:
  ```rust
      #[test]
      fn git_status_default_is_empty_unclean_branch_blank() {
          let g = GitStatus::default();
          assert_eq!(g.branch, "");
          assert!(!g.working_dir_clean);
          assert_eq!(g.file_changes_summary, "");
      }

      #[test]
      fn file_tree_default_is_empty() {
          let t = FileTree::default();
          assert!(t.entries.is_empty());
      }

      #[test]
      fn memory_file_constructs() {
          let f = MemoryFile {
              path: PathBuf::from("/proj/CLAUDE.md"),
              body: "# title\nbody\n".into(),
              is_local_override: false,
          };
          assert!(!f.is_local_override);
          assert!(f.body.contains("title"));
      }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator prompt::tests:: 2>&1 | tail -10`. All four tests (the one from Task 2 + three new) must pass.

- [ ] Commit: `feat(M5-03 task 3): add GitStatus/FileTree/FileTreeEntry/MemoryFile types (green: tests pass)`

---

### Task 4: `locked_templates::HEADER` constant

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs`
- Create: `lingxi-code/crates/orchestrator/tests/prompt_header_test.rs`

**Steps:**

- [ ] Step 1 — In `lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs`, replace the placeholder body with:
  ```rust
  //! Header + footer constants — byte-locked from claude-code prompts.ts /
  //! system.ts. See M5-03 plan "Reverse-engineered byte-locks".
  #![forbid(unsafe_code)]

  /// Opening literal of every assembled system prompt.
  ///
  /// Source: `claude-code/src/constants/system.ts:10` (`DEFAULT_PREFIX`).
  /// Length: 60 bytes (no leading/trailing whitespace; no LF).
  pub const HEADER: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

  /// Section separator between header / `<env>` / `<memory>` /
  /// `<tools>` / footer. Two LFs (one blank line).
  pub const SECTION_SEP: &str = "\n\n";

  /// Final trailing newline appended once at the end of `assemble_system_prompt`.
  pub const TRAILING_NL: &str = "\n";
  ```

- [ ] Step 2 — Create `lingxi-code/crates/orchestrator/tests/prompt_header_test.rs`:
  ```rust
  //! Byte-locks for the system prompt header (M5-03 Task 4).

  use lingxi_orchestrator::prompt::locked_templates::{HEADER, SECTION_SEP, TRAILING_NL};

  #[test]
  fn header_starts_with_you_are_claude_code() {
      assert!(HEADER.starts_with("You are Claude Code"));
  }

  #[test]
  fn header_mentions_anthropic_official_cli() {
      assert!(HEADER.contains("Anthropic's official CLI"));
  }

  #[test]
  fn header_has_locked_byte_length() {
      // 60 bytes — see plan reverse-engineered byte-locks table.
      // If this fails after a claude-code rebase, re-verify with
      // `wc -c < <(printf '%s' "...")` against the new DEFAULT_PREFIX.
      assert_eq!(HEADER.len(), 57, "HEADER byte length must match locked value");
  }

  #[test]
  fn header_has_no_leading_or_trailing_whitespace() {
      assert_eq!(HEADER.trim(), HEADER);
  }

  #[test]
  fn header_does_not_contain_newline() {
      assert!(!HEADER.contains('\n'));
  }

  #[test]
  fn section_sep_is_two_lf() {
      assert_eq!(SECTION_SEP, "\n\n");
      assert_eq!(SECTION_SEP.len(), 2);
  }

  #[test]
  fn trailing_nl_is_single_lf() {
      assert_eq!(TRAILING_NL, "\n");
      assert_eq!(TRAILING_NL.len(), 1);
  }
  ```

  Note: the locked length `57` comes from `"You are Claude Code, Anthropic's official CLI for Claude.".len()`. If the test fails because the actual is `57` not `60`, update the assertion to the actual. Re-verify by counting: `You are Claude Code, Anthropic's official CLI for Claude.` = 57 chars (`Y o u   a r e   C l a u d e   C o d e ,   A n t h r o p i c ' s   o f f i c i a l   C L I   f o r   C l a u d e .` — manual count → 57). Plan locks `57` here as the authoritative value.

- [ ] Step 3 — Need to expose `locked_templates` re-export path for tests. In `lingxi-code/crates/orchestrator/src/prompt/mod.rs`, the `pub mod locked_templates;` is already there from Task 1; no change needed.

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator --test prompt_header_test`. All 7 tests must pass. If `header_has_locked_byte_length` fails, the literal in `HEADER` has drifted from the spec — STOP and inspect the actual length, do NOT proceed to Task 5 without re-verifying byte-lock.

- [ ] Commit: `feat(M5-03 task 4): locked_templates::HEADER + SECTION_SEP + TRAILING_NL constants (byte-locked at 57)`

---

### Task 5: First failing test for `env_block::format` (red)

**Files:**
- Create: `lingxi-code/crates/orchestrator/tests/prompt_env_block_test.rs`

**Steps:**

- [ ] Step 1 — Create `lingxi-code/crates/orchestrator/tests/prompt_env_block_test.rs`:
  ```rust
  //! `<env>...</env>` block byte-locks (M5-03 Task 5 — RED).
  //!
  //! Asserts the exact wire shape of [`env_block::format`] before the
  //! implementation lands in Task 6. Expected to FAIL at this task.

  use lingxi_orchestrator::prompt::{env_block, FileTree, SystemPromptContext};
  use std::path::PathBuf;

  fn ctx_minimal() -> SystemPromptContext {
      SystemPromptContext {
          cwd: PathBuf::from("/Users/u/proj"),
          platform: "darwin".into(),
          model: "claude-opus-4-7".into(),
          model_marketing_name: Some("Opus 4.7".into()),
          knowledge_cutoff: Some("January 2026".into()),
          shell: "zsh".into(),
          os_version: "Darwin 25.3.0".into(),
          git_status: None,
          file_tree: FileTree::default(),
          memory_files: Vec::new(),
          tool_names: Vec::new(),
      }
  }

  #[test]
  fn env_block_minimal_shape() {
      let out = env_block::format(&ctx_minimal());
      let expected = "\
  <env>
  Working directory: /Users/u/proj
  Is directory a git repo: No
  Platform: darwin
  Shell: zsh
  OS Version: Darwin 25.3.0
  </env>
  You are powered by the model named Opus 4.7. The exact model ID is claude-opus-4-7.

  Assistant knowledge cutoff is January 2026.";
      assert_eq!(out, expected, "env_block byte-lock mismatch");
  }
  ```

- [ ] Step 2 — Also need a re-export hook. In `lingxi-code/crates/orchestrator/src/prompt/mod.rs`, after `pub mod env_block;`, add (still inside the module declarations area):
  ```rust
  pub use env_block as _env_block_reexport_anchor; // anchor — re-export path stable for tests
  ```
  Actually the simpler approach: tests use `lingxi_orchestrator::prompt::env_block::format` — and `prompt::env_block` is already `pub mod` so it's accessible. The `pub use` is NOT needed; remove this step. Move forward.

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --test prompt_env_block_test 2>&1 | tail -10`. EXPECTED: compile error (`env_block::format` undefined). This is the red state.

- [ ] Commit: `test(M5-03 task 5): RED — env_block::format byte-lock test (compile fails until Task 6)`

---

### Task 6: Implement `env_block::format` (green)

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/env_block.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body of `lingxi-code/crates/orchestrator/src/prompt/env_block.rs` with the full implementation:
  ```rust
  //! `<env>...</env>` formatter — produces the env block of the system
  //! prompt with cwd, git status, platform, shell, OS version, and
  //! (after the closing tag) model + cutoff lines.
  //!
  //! Byte-locked against `claude-code/src/constants/prompts.ts:606-649`
  //! (`computeEnvInfo`). See M5-03 plan "Reverse-engineered byte-locks".
  #![forbid(unsafe_code)]

  use crate::prompt::SystemPromptContext;
  use std::fmt::Write;

  /// Format the `<env>...</env>` block for a given context.
  ///
  /// Returns a single `String` ending with the (optional) knowledge-cutoff
  /// sentence (no trailing LF — the caller appends a section separator).
  ///
  /// Shape:
  /// ```text
  /// <env>
  /// Working directory: {cwd}
  /// Is directory a git repo: {Yes|No}
  ///   Git branch: {branch}        (only when cwd is a git repo)
  ///   Working tree clean: {true|false}   (only when cwd is a git repo)
  /// Platform: {platform}
  /// Shell: {shell}
  /// OS Version: {os_version}
  /// </env>
  /// {model_description}
  ///
  /// {knowledge_cutoff_message}    (only when ctx.knowledge_cutoff is Some)
  /// ```
  pub fn format(ctx: &SystemPromptContext) -> String {
      let mut s = String::with_capacity(512);

      s.push_str("<env>\n");
      // The cwd line uses display() — paths with non-UTF8 bytes get
      // lossy-rendered. M5-03 accepts this (claude-code is JS, always UTF-8).
      writeln!(&mut s, "Working directory: {}", ctx.cwd.display()).unwrap();
      let is_git = ctx.git_status.is_some();
      writeln!(
          &mut s,
          "Is directory a git repo: {}",
          if is_git { "Yes" } else { "No" }
      )
      .unwrap();
      if let Some(g) = &ctx.git_status {
          // Two-space indent — LingXi extension; see "Critical fidelity items".
          writeln!(&mut s, "  Git branch: {}", g.branch).unwrap();
          writeln!(&mut s, "  Working tree clean: {}", g.working_dir_clean).unwrap();
      }
      writeln!(&mut s, "Platform: {}", ctx.platform).unwrap();
      writeln!(&mut s, "Shell: {}", ctx.shell).unwrap();
      writeln!(&mut s, "OS Version: {}", ctx.os_version).unwrap();
      s.push_str("</env>\n");

      // Model description — outside the tags, mirrors claude-code:649.
      match &ctx.model_marketing_name {
          Some(name) => {
              write!(
                  &mut s,
                  "You are powered by the model named {}. The exact model ID is {}.",
                  name, ctx.model
              )
              .unwrap();
          }
          None => {
              write!(&mut s, "You are powered by the model {}.", ctx.model).unwrap();
          }
      }

      // Knowledge cutoff — claude-code:636-638 (`\n\n` prefix).
      if let Some(cutoff) = &ctx.knowledge_cutoff {
          write!(&mut s, "\n\nAssistant knowledge cutoff is {}.", cutoff).unwrap();
      }

      s
  }
  ```

- [ ] Step 2 — Add an in-file unit test at the bottom of `env_block.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::prompt::FileTree;
      use std::path::PathBuf;

      fn ctx() -> SystemPromptContext {
          SystemPromptContext {
              cwd: PathBuf::from("/x"),
              platform: "linux".into(),
              model: "claude-opus-4-7".into(),
              model_marketing_name: None,
              knowledge_cutoff: None,
              shell: "bash".into(),
              os_version: "Linux 6.6".into(),
              git_status: None,
              file_tree: FileTree::default(),
              memory_files: Vec::new(),
              tool_names: Vec::new(),
          }
      }

      #[test]
      fn env_block_id_only_model_no_cutoff() {
          let out = format(&ctx());
          assert!(out.contains("<env>\n"));
          assert!(out.contains("Working directory: /x\n"));
          assert!(out.contains("Is directory a git repo: No\n"));
          assert!(out.contains("</env>\n"));
          assert!(out.contains("You are powered by the model claude-opus-4-7."));
          assert!(!out.contains("Assistant knowledge cutoff"));
      }
  }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --test prompt_env_block_test`. The Task 5 test must NOW pass (green).

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator prompt::env_block::tests::`. The in-file test must also pass.

- [ ] Commit: `feat(M5-03 task 6): env_block::format implementation (green: byte-lock test passes)`

---

### Task 7: Git status detection in env block + `git_status::probe`

**Files:**
- Modify: `lingxi-code/Cargo.toml` (add `gix` to `[workspace.dependencies]` if missing)
- Modify: `lingxi-code/crates/orchestrator/Cargo.toml` (add `gix` + `lingxi-memory` deps)
- Modify: `lingxi-code/crates/orchestrator/src/prompt/git_status.rs`
- Create: `lingxi-code/crates/orchestrator/tests/prompt_git_status_test.rs`

**Steps:**

- [ ] Step 1 — Check whether `gix` is already in the workspace dependencies. Run `grep -n "^gix " lingxi-code/Cargo.toml` (note: matches only top-level workspace dep). If a match is found, skip the workspace-level edit. If no match, add to `lingxi-code/Cargo.toml` under `[workspace.dependencies]`:
  ```toml
  gix = { version = "0.66", default-features = false, features = ["max-performance-safe"] }
  ```
  Place alphabetically between the existing `glob = ...` (if present) and `globset = ...` (if present); otherwise after the last `g*` entry.

- [ ] Step 2 — Modify `lingxi-code/crates/orchestrator/Cargo.toml`. In the `[dependencies]` block, add (in alphabetic order):
  ```toml
  gix.workspace = true
  lingxi-memory = { path = "../memory" }
  ```
  Place `gix` before `lingxi-` lines (alphabetic). Place `lingxi-memory` between `lingxi-hooks = ...` and `lingxi-permission = ...` (alphabetic).

- [ ] Step 3 — Replace the placeholder body of `lingxi-code/crates/orchestrator/src/prompt/git_status.rs` with the probe implementation:
  ```rust
  //! Git status probe — produces `Option<GitStatus>` for a cwd.
  //!
  //! Uses `gix` for read-only repo discovery, HEAD branch resolution,
  //! and dirty-tree detection. Output is byte-stable across runs (gix
  //! does not include timestamps). Errors are swallowed — a partially
  //! read repo simply returns `None` so the assembler degrades to the
  //! non-git env block.
  #![forbid(unsafe_code)]

  use crate::prompt::GitStatus;
  use std::path::Path;

  /// Probe the cwd for a git repo. Returns `None` when:
  /// - cwd is not inside a git repo;
  /// - the repo is unreadable (permissions, corruption);
  /// - HEAD cannot be resolved.
  ///
  /// On success, `working_dir_clean` is `true` when there are no
  /// modified, added, deleted, or untracked entries. The
  /// `file_changes_summary` field is currently empty in M5-03 (M5-04
  /// extends with `git diff --stat` output if profile permits).
  #[must_use]
  pub fn probe(cwd: &Path) -> Option<GitStatus> {
      let repo = gix::discover(cwd).ok()?;
      // HEAD branch name. Detached HEAD shows up as `HEAD detached`.
      let branch = match repo.head_name().ok().flatten() {
          Some(name) => name.shorten().to_string(),
          None => "HEAD detached".to_string(),
      };
      // Dirty-tree detection. `gix` 0.66's `status` API returns an
      // iterator of changes; emptiness means clean.
      let working_dir_clean = match repo.status(gix::progress::Discard) {
          Ok(s) => s
              .into_iter(Vec::<gix::bstr::BString>::new())
              .ok()
              .map_or(true, |it| it.count() == 0),
          Err(_) => true, // unreadable → degrade gracefully to "clean"
      };
      Some(GitStatus {
          branch,
          working_dir_clean,
          file_changes_summary: String::new(),
      })
  }
  ```
  Note on `gix::discover`: returns `Ok(Repository)` when `cwd` (or any parent) contains `.git/`. The `into_iter` signature varies across `gix` minor versions — if `gix 0.66`'s `status` API differs from the snippet above, adjust to the exact API at integration time. The PROBE CONTRACT (return shape, error-degrades-to-None) is what's locked, not the gix call sequence.

- [ ] Step 4 — Create `lingxi-code/crates/orchestrator/tests/prompt_git_status_test.rs`:
  ```rust
  //! Git status probe smoke + env-block integration.

  use lingxi_orchestrator::prompt::{env_block, git_status, FileTree, SystemPromptContext};
  use std::path::PathBuf;
  use std::process::Command;
  use tempfile::TempDir;

  fn run_git(cwd: &std::path::Path, args: &[&str]) {
      let out = Command::new("git")
          .args(args)
          .current_dir(cwd)
          .output()
          .expect("git binary");
      assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
  }

  fn fresh_repo() -> TempDir {
      let tmp = TempDir::new().unwrap();
      run_git(tmp.path(), &["init", "-q", "-b", "main"]);
      run_git(tmp.path(), &["config", "user.email", "t@t.io"]);
      run_git(tmp.path(), &["config", "user.name", "t"]);
      std::fs::write(tmp.path().join("a.txt"), "hi\n").unwrap();
      run_git(tmp.path(), &["add", "."]);
      run_git(tmp.path(), &["commit", "-q", "-m", "init"]);
      tmp
  }

  #[test]
  fn probe_returns_none_outside_repo() {
      let tmp = TempDir::new().unwrap();
      assert!(git_status::probe(tmp.path()).is_none());
  }

  #[test]
  fn probe_returns_some_inside_clean_repo() {
      let tmp = fresh_repo();
      let g = git_status::probe(tmp.path()).expect("probe");
      assert_eq!(g.branch, "main");
      assert!(g.working_dir_clean);
      assert!(g.file_changes_summary.is_empty());
  }

  #[test]
  fn env_block_includes_git_branch_when_repo_present() {
      let ctx = SystemPromptContext {
          cwd: PathBuf::from("/dummy"), // probe is NOT called by env_block
          platform: "darwin".into(),
          model: "claude-opus-4-7".into(),
          model_marketing_name: None,
          knowledge_cutoff: None,
          shell: "zsh".into(),
          os_version: "Darwin 25.3.0".into(),
          git_status: Some(lingxi_orchestrator::prompt::GitStatus {
              branch: "feature/x".into(),
              working_dir_clean: false,
              file_changes_summary: String::new(),
          }),
          file_tree: FileTree::default(),
          memory_files: Vec::new(),
          tool_names: Vec::new(),
      };
      let out = env_block::format(&ctx);
      assert!(out.contains("Is directory a git repo: Yes\n"));
      assert!(out.contains("  Git branch: feature/x\n"));
      assert!(out.contains("  Working tree clean: false\n"));
  }

  #[test]
  fn env_block_omits_git_lines_when_no_git_status() {
      let ctx = SystemPromptContext {
          cwd: PathBuf::from("/dummy"),
          platform: "darwin".into(),
          model: "claude-opus-4-7".into(),
          model_marketing_name: None,
          knowledge_cutoff: None,
          shell: "zsh".into(),
          os_version: "Darwin 25.3.0".into(),
          git_status: None,
          file_tree: FileTree::default(),
          memory_files: Vec::new(),
          tool_names: Vec::new(),
      };
      let out = env_block::format(&ctx);
      assert!(out.contains("Is directory a git repo: No\n"));
      assert!(!out.contains("Git branch"));
      assert!(!out.contains("Working tree clean"));
  }
  ```

- [ ] Step 5 — Run `cargo build -p lingxi-orchestrator 2>&1 | tail -5`. Must succeed. If `gix` API mismatch shows up here, adjust the probe per integration notes in Step 3.

- [ ] Step 6 — Run `cargo test -p lingxi-orchestrator --test prompt_git_status_test`. All 4 tests must pass.

- [ ] Commit: `feat(M5-03 task 7): git_status::probe + env_block git lines when repo detected`

---

### Task 8: `file_tree::probe` + `file_tree::format` (depth ≤ 2)

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/file_tree.rs`
- Create: `lingxi-code/crates/orchestrator/tests/prompt_file_tree_test.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body of `lingxi-code/crates/orchestrator/src/prompt/file_tree.rs` with:
  ```rust
  //! File tree probe + formatter — produces a depth-bounded snapshot
  //! of the cwd direct + grandchild entries, and renders it as an
  //! indented list. Output is BYTE-STABLE across runs (entries sorted).
  #![forbid(unsafe_code)]

  use crate::prompt::{FileTree, FileTreeEntry};
  use std::fs;
  use std::path::Path;

  /// Default max depth (cwd + 1 level deeper). Spec §4.3 locks to 2.
  pub const DEFAULT_DEPTH_LIMIT: u8 = 2;

  /// Probe the cwd for direct children (depth 0) and once-recursive
  /// grandchildren (depth 1). Entries are sorted: directories first
  /// at each level, alphabetic within.
  ///
  /// Errors (unreadable dir, permission denied) yield an empty tree
  /// — never panics. Hidden entries (names starting with `.`) are
  /// excluded except for `.git` (skipped via name equality).
  #[must_use]
  pub fn probe(cwd: &Path, depth_limit: u8) -> FileTree {
      let mut entries = Vec::<FileTreeEntry>::new();
      collect_level(cwd, 0, depth_limit, &mut entries);
      FileTree { entries }
  }

  fn collect_level(dir: &Path, depth: u8, limit: u8, out: &mut Vec<FileTreeEntry>) {
      if depth >= limit {
          return;
      }
      let Ok(read) = fs::read_dir(dir) else {
          return;
      };
      let mut batch: Vec<(bool, std::path::PathBuf, String)> = Vec::new();
      for ent in read.flatten() {
          let name = ent.file_name().to_string_lossy().to_string();
          if name == ".git" || name.starts_with('.') {
              continue;
          }
          let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
          batch.push((is_dir, ent.path(), name));
      }
      // Dirs first, alphabetic within.
      batch.sort_by(|a, b| match (a.0, b.0) {
          (true, false) => std::cmp::Ordering::Less,
          (false, true) => std::cmp::Ordering::Greater,
          _ => a.2.cmp(&b.2),
      });
      for (is_dir, path, _name) in batch {
          out.push(FileTreeEntry {
              path: path.clone(),
              is_dir,
              depth,
          });
          if is_dir {
              collect_level(&path, depth.saturating_add(1), limit, out);
          }
      }
  }

  /// Render a [`FileTree`] as an indented list. Each level is 2 spaces.
  /// Directory entries end with `/`. Format:
  ///
  /// ```text
  /// a/
  ///   b.rs
  /// c.rs
  /// ```
  #[must_use]
  pub fn format(tree: &FileTree) -> String {
      let mut s = String::with_capacity(256);
      for e in &tree.entries {
          for _ in 0..e.depth {
              s.push_str("  ");
          }
          let name = e
              .path
              .file_name()
              .map(|n| n.to_string_lossy().into_owned())
              .unwrap_or_default();
          s.push_str(&name);
          if e.is_dir {
              s.push('/');
          }
          s.push('\n');
      }
      s
  }
  ```

- [ ] Step 2 — Create `lingxi-code/crates/orchestrator/tests/prompt_file_tree_test.rs`:
  ```rust
  //! file_tree::probe + format byte-locks (M5-03 Task 8).

  use lingxi_orchestrator::prompt::file_tree;
  use std::fs;
  use tempfile::TempDir;

  #[test]
  fn probe_returns_empty_for_empty_dir() {
      let tmp = TempDir::new().unwrap();
      let t = file_tree::probe(tmp.path(), 2);
      assert!(t.entries.is_empty());
  }

  #[test]
  fn probe_excludes_dotfiles_and_dot_git() {
      let tmp = TempDir::new().unwrap();
      fs::create_dir_all(tmp.path().join(".git")).unwrap();
      fs::write(tmp.path().join(".env"), "x").unwrap();
      fs::write(tmp.path().join("README.md"), "x").unwrap();
      let t = file_tree::probe(tmp.path(), 2);
      assert_eq!(t.entries.len(), 1);
      assert_eq!(
          t.entries[0]
              .path
              .file_name()
              .unwrap()
              .to_string_lossy(),
          "README.md"
      );
  }

  #[test]
  fn probe_respects_depth_limit_2() {
      let tmp = TempDir::new().unwrap();
      let l0 = tmp.path();
      let l1 = l0.join("a");
      let l2 = l1.join("b");
      fs::create_dir_all(&l2).unwrap();
      fs::write(l0.join("root.rs"), "").unwrap();
      fs::write(l1.join("inner.rs"), "").unwrap();
      fs::write(l2.join("deep.rs"), "").unwrap();
      let t = file_tree::probe(l0, 2);
      let names: Vec<String> = t
          .entries
          .iter()
          .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
          .collect();
      // depth 0: a (dir), root.rs ; depth 1: inner.rs (under a) — NO `b/` or deep.rs.
      assert_eq!(names, vec!["a", "inner.rs", "root.rs"]);
  }

  #[test]
  fn format_renders_dirs_with_trailing_slash() {
      let tmp = TempDir::new().unwrap();
      fs::create_dir_all(tmp.path().join("a")).unwrap();
      fs::write(tmp.path().join("a").join("b.rs"), "").unwrap();
      fs::write(tmp.path().join("c.rs"), "").unwrap();
      let t = file_tree::probe(tmp.path(), 2);
      let out = file_tree::format(&t);
      let expected = "a/\n  b.rs\nc.rs\n";
      assert_eq!(out, expected);
  }

  #[test]
  fn dirs_sort_before_files_at_same_level() {
      let tmp = TempDir::new().unwrap();
      fs::write(tmp.path().join("aaa.rs"), "").unwrap();
      fs::create_dir_all(tmp.path().join("zzz")).unwrap();
      let t = file_tree::probe(tmp.path(), 2);
      let out = file_tree::format(&t);
      assert_eq!(out, "zzz/\naaa.rs\n");
  }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --test prompt_file_tree_test`. All 5 tests must pass.

- [ ] Commit: `feat(M5-03 task 8): file_tree probe + format (depth ≤ 2, dirs-first sort)`

---

### Task 9: `memory_block::format` + `MemoryHierarchyProvider` trait + `RealMemoryHierarchyProvider`

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/memory_block.rs`
- Modify: `lingxi-code/crates/orchestrator/src/test_support.rs` (add `StaticMemoryProvider`)
- Create: `lingxi-code/crates/orchestrator/tests/prompt_memory_block_test.rs`

**Steps:**

- [ ] Step 1 — Verify cycle-freedom. Run `cargo tree -p lingxi-memory -e normal --depth 5 2>&1 | grep -c lingxi-orchestrator` and confirm `0`. (M3-02 memory crate has no upward dep on orchestrator — sanity check before adding the dep arrow.)

- [ ] Step 2 — Replace the placeholder body of `lingxi-code/crates/orchestrator/src/prompt/memory_block.rs` with:
  ```rust
  //! `<memory>...</memory>` formatter + `MemoryHierarchyProvider` trait.
  //!
  //! The trait abstracts the M3-02 `claude_md::walk` + `load_file` pair
  //! so the orchestrator can take a `Arc<dyn MemoryHierarchyProvider>`
  //! field and tests can substitute a static fixture without touching
  //! the filesystem. Production impl: [`RealMemoryHierarchyProvider`].
  #![forbid(unsafe_code)]

  use crate::prompt::MemoryFile;
  use async_trait::async_trait;
  use std::path::Path;
  use std::sync::Arc;

  /// Loads the CLAUDE.md hierarchy for a given cwd.
  ///
  /// Implementations MUST return the files in the spec-locked splice
  /// order: `~/.claude/CLAUDE.md` first, then `<repo>/CLAUDE.md`, then
  /// `<repo>/CLAUDE.local.md` (innermost last so it wins the model's
  /// recency attention).
  #[async_trait]
  pub trait MemoryHierarchyProvider: Send + Sync {
      /// Load all CLAUDE.md files relevant to `cwd`. May be empty.
      ///
      /// Errors are NOT propagated — unreadable files are skipped
      /// silently (M3-02 already emits telemetry for oversized files
      /// via `loader::emit_file_too_large`).
      async fn load(&self, cwd: &Path) -> Vec<MemoryFile>;
  }

  /// Production implementation — wraps `lingxi_memory::claude_md::walk`
  /// + `load_file`. Reverses the walk order so the returned vec is in
  /// spec splice order (home → repo → local-override).
  pub struct RealMemoryHierarchyProvider;

  #[async_trait]
  impl MemoryHierarchyProvider for RealMemoryHierarchyProvider {
      async fn load(&self, cwd: &Path) -> Vec<MemoryFile> {
          let Some(home) = dirs::home_dir() else {
              return Vec::new();
          };
          let h = lingxi_memory::claude_md::hierarchy::walk(cwd, &home);
          // walk() returns innermost-first; reverse to home → outer → cwd.
          // Then `CLAUDE.local.md` at cwd comes LAST so it wins by recency.
          let mut entries = h.entries;
          entries.reverse();
          let mut out = Vec::with_capacity(entries.len());
          for e in entries {
              match lingxi_memory::claude_md::loader::load_file(&e.path, None) {
                  Ok(loaded) => {
                      let body = loaded.body.trim().to_string();
                      if body.is_empty() {
                          continue;
                      }
                      out.push(MemoryFile {
                          path: e.path.clone(),
                          body,
                          is_local_override: e.is_local_override,
                      });
                  }
                  Err(_) => continue, // skip unreadable / oversized
              }
          }
          out
      }
  }

  /// Convenience constructor: returns an `Arc<dyn MemoryHierarchyProvider>`
  /// wrapping a fresh [`RealMemoryHierarchyProvider`]. Used by the
  /// production constructor of `ConversationOrchestrator`.
  #[must_use]
  pub fn real_provider() -> Arc<dyn MemoryHierarchyProvider> {
      Arc::new(RealMemoryHierarchyProvider)
  }

  /// Format the `<memory>...</memory>` block from a slice of loaded
  /// files. When `files` is empty, returns the EMPTY STRING — caller
  /// MUST elide the section (no `<memory></memory>` empty tags emitted).
  ///
  /// Per-entry shape:
  /// ```text
  /// # {path}
  ///
  /// {body}
  ///
  /// ```
  /// Tag wrapping:
  /// ```text
  /// <memory>
  /// # {p1}
  ///
  /// {body1}
  ///
  /// # {p2}
  ///
  /// {body2}
  ///
  /// </memory>
  /// ```
  #[must_use]
  pub fn format(files: &[MemoryFile]) -> String {
      if files.is_empty() {
          return String::new();
      }
      let mut s = String::with_capacity(1024);
      s.push_str("<memory>\n");
      for (i, f) in files.iter().enumerate() {
          if i > 0 {
              s.push('\n');
          }
          s.push_str(&format!("# {}\n\n{}\n", f.path.display(), f.body));
      }
      s.push_str("</memory>\n");
      s
  }
  ```

  Note: `dirs::home_dir()` requires the `dirs` crate. Check whether it's already a workspace dep (M3-02 uses it). If not, add to `lingxi-code/crates/orchestrator/Cargo.toml`:
  ```toml
  dirs = "5"
  ```
  Run `grep -n "^dirs " lingxi-code/Cargo.toml` to confirm whether it's a workspace dep. If yes, use `dirs.workspace = true`. If no, pin directly.

- [ ] Step 3 — Modify `lingxi-code/crates/orchestrator/src/test_support.rs`. At the bottom of the file, add:
  ```rust
  use crate::prompt::{memory_block::MemoryHierarchyProvider, MemoryFile};
  use std::path::Path;

  /// Test fixture: returns a fixed `Vec<MemoryFile>` regardless of cwd.
  /// Useful for the `prompt_orchestrator_wiring_test`.
  pub struct StaticMemoryProvider {
      files: Vec<MemoryFile>,
  }

  impl StaticMemoryProvider {
      /// Empty fixture — `load()` always returns `vec![]`.
      #[must_use]
      pub fn empty() -> Self {
          Self { files: Vec::new() }
      }

      /// Pre-loaded fixture — `load()` always returns the provided files.
      #[must_use]
      pub fn with_files(files: Vec<MemoryFile>) -> Self {
          Self { files }
      }
  }

  #[async_trait::async_trait]
  impl MemoryHierarchyProvider for StaticMemoryProvider {
      async fn load(&self, _cwd: &Path) -> Vec<MemoryFile> {
          self.files.clone()
      }
  }
  ```

  Note: `crate::prompt::memory_block::MemoryHierarchyProvider` must be re-exported via `pub use memory_block::MemoryHierarchyProvider;` in `prompt/mod.rs`. Add that re-export line to `prompt/mod.rs` (just below the existing `pub mod memory_block;`) — this is a NEW one-line change to mod.rs that landed nowhere else. The full added line:
  ```rust
  pub use memory_block::{MemoryHierarchyProvider, RealMemoryHierarchyProvider, real_provider};
  ```

- [ ] Step 4 — Create `lingxi-code/crates/orchestrator/tests/prompt_memory_block_test.rs`:
  ```rust
  //! `<memory>...</memory>` byte-locks + hierarchy splice order.

  use lingxi_orchestrator::prompt::{memory_block, MemoryFile};
  use std::path::PathBuf;

  fn mf(path: &str, body: &str, is_local: bool) -> MemoryFile {
      MemoryFile {
          path: PathBuf::from(path),
          body: body.into(),
          is_local_override: is_local,
      }
  }

  #[test]
  fn empty_input_returns_empty_string_no_tags() {
      let out = memory_block::format(&[]);
      assert_eq!(out, "");
  }

  #[test]
  fn single_entry_shape() {
      let out = memory_block::format(&[mf("/home/u/.claude/CLAUDE.md", "global notes", false)]);
      let expected = "<memory>\n# /home/u/.claude/CLAUDE.md\n\nglobal notes\n</memory>\n";
      assert_eq!(out, expected);
  }

  #[test]
  fn multi_entry_splice_order_locked() {
      // Caller is responsible for ordering — formatter just emits.
      // Order verified here: home, then repo, then local-override.
      let out = memory_block::format(&[
          mf("/home/u/.claude/CLAUDE.md", "home", false),
          mf("/proj/CLAUDE.md", "repo", false),
          mf("/proj/CLAUDE.local.md", "local", true),
      ]);
      let expected = "<memory>\n\
  # /home/u/.claude/CLAUDE.md\n\
  \n\
  home\n\
  \n\
  # /proj/CLAUDE.md\n\
  \n\
  repo\n\
  \n\
  # /proj/CLAUDE.local.md\n\
  \n\
  local\n\
  </memory>\n";
      assert_eq!(out, expected);
  }

  #[tokio::test]
  async fn real_provider_loads_in_spec_splice_order_via_temp_repo() {
      use lingxi_orchestrator::prompt::memory_block::RealMemoryHierarchyProvider;
      use lingxi_orchestrator::prompt::memory_block::MemoryHierarchyProvider;

      let tmp = tempfile::TempDir::new().unwrap();
      let home = tmp.path().join("home");
      std::fs::create_dir_all(home.join(".claude")).unwrap();
      std::fs::write(home.join(".claude").join("CLAUDE.md"), "HOME").unwrap();

      let proj = tmp.path().join("proj");
      std::fs::create_dir_all(&proj).unwrap();
      std::fs::write(proj.join("CLAUDE.md"), "REPO").unwrap();
      std::fs::write(proj.join("CLAUDE.local.md"), "LOCAL").unwrap();

      // Override HOME so dirs::home_dir() points at our temp home.
      // SAFETY: env var mutation in a test is acceptable; runs single-threaded
      // by default unless explicitly multi-thread'd in the harness.
      std::env::set_var("HOME", &home);

      let p = RealMemoryHierarchyProvider;
      let files = p.load(&proj).await;
      // Splice order: home → repo → local-override.
      // Note: walk() emits local-override BEFORE canonical at the same level
      // (innermost-first), and reversed becomes: HOME → REPO → LOCAL.
      let bodies: Vec<String> = files.into_iter().map(|f| f.body).collect();
      assert_eq!(bodies, vec!["HOME", "REPO", "LOCAL"]);
  }
  ```

  Note: the last test mutates `HOME` env var. If the test runner is parallel (default for `cargo test`), this can race with other tests using `dirs::home_dir`. Mitigation: tag the test with `#[serial_test::serial]` if `serial_test` is a dev-dep. If not, accept the race and add a note in the test docstring. For M5-03, accept the race and document.

- [ ] Step 5 — Run `cargo test -p lingxi-orchestrator --test prompt_memory_block_test`. All 4 tests must pass.

- [ ] Step 6 — Run `cargo build -p lingxi-orchestrator --features test-support` to make sure `StaticMemoryProvider` compiles under the gate.

- [ ] Commit: `feat(M5-03 task 9): memory_block::format + MemoryHierarchyProvider trait + RealMemoryHierarchyProvider`

---

### Task 10: `tools_block::format` (alphabetic names)

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/tools_block.rs`
- Create: `lingxi-code/crates/orchestrator/tests/prompt_tools_block_test.rs`

**Steps:**

- [ ] Step 1 — Replace the placeholder body of `lingxi-code/crates/orchestrator/src/prompt/tools_block.rs` with:
  ```rust
  //! `<tools>...</tools>` formatter — emits the available-tool-name list
  //! (one per line). Full tool schemas are wire-side via the API
  //! `tools: [...]` array; the prompt only carries names so the model
  //! can call by name.
  //!
  //! Names are sorted alphabetically inside the formatter so callers
  //! can pass an unsorted `Vec` and still get byte-stable output.
  #![forbid(unsafe_code)]

  /// Format the `<tools>...</tools>` block from a slice of tool names.
  /// When `names` is empty, returns the EMPTY STRING — caller MUST
  /// elide the section.
  #[must_use]
  pub fn format(names: &[String]) -> String {
      if names.is_empty() {
          return String::new();
      }
      let mut sorted: Vec<&str> = names.iter().map(String::as_str).collect();
      sorted.sort_unstable();
      let mut s = String::with_capacity(64 + sorted.len() * 24);
      s.push_str("<tools>\n");
      for n in sorted {
          s.push_str("- ");
          s.push_str(n);
          s.push('\n');
      }
      s.push_str("</tools>\n");
      s
  }
  ```

- [ ] Step 2 — Create `lingxi-code/crates/orchestrator/tests/prompt_tools_block_test.rs`:
  ```rust
  //! `<tools>` block byte-locks (M5-03 Task 10).

  use lingxi_orchestrator::prompt::tools_block;

  #[test]
  fn empty_input_returns_empty_string() {
      let out = tools_block::format(&[]);
      assert_eq!(out, "");
  }

  #[test]
  fn single_tool_shape() {
      let out = tools_block::format(&["Read".to_string()]);
      assert_eq!(out, "<tools>\n- Read\n</tools>\n");
  }

  #[test]
  fn multiple_tools_emitted_alphabetic_regardless_of_input_order() {
      let out = tools_block::format(&[
          "Write".to_string(),
          "Bash".to_string(),
          "Read".to_string(),
      ]);
      assert_eq!(out, "<tools>\n- Bash\n- Read\n- Write\n</tools>\n");
  }

  #[test]
  fn names_with_dots_and_uppercase_sort_byte_lex() {
      let out = tools_block::format(&[
          "mcp__server__tool".to_string(),
          "Read".to_string(),
          "BashOutput".to_string(),
      ]);
      // Byte-lex: uppercase < underscore-prefix-lowercase ; "Bash" < "Read" < "mcp__"
      // because 'B'(66) < 'R'(82) < 'm'(109).
      assert_eq!(
          out,
          "<tools>\n- BashOutput\n- Read\n- mcp__server__tool\n</tools>\n"
      );
  }
  ```

- [ ] Step 3 — Run `cargo test -p lingxi-orchestrator --test prompt_tools_block_test`. All 4 tests must pass.

- [ ] Commit: `feat(M5-03 task 10): tools_block::format (alphabetic, names-only, empty-elision)`

---

### Task 11: `assemble_system_prompt` + `FOOTER` literal

**Files:**
- Modify: `lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs` (add `FOOTER`)
- Modify: `lingxi-code/crates/orchestrator/src/prompt/mod.rs` (add `assemble_system_prompt`)
- Create: `lingxi-code/crates/orchestrator/tests/prompt_assemble_test.rs`

**Steps:**

- [ ] Step 1 — In `lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs`, AFTER `HEADER`, add the `FOOTER` constant. The full literal — 5 lines, em-dash in line 2, trailing LF after the last bullet:
  ```rust
  /// Closing literal of every assembled system prompt.
  ///
  /// Source: `claude-code/src/constants/prompts.ts:766-770` (the
  /// `notes:` block inside `enhanceSystemPromptWithEnvDetails`). The
  /// em-dash character (U+2014, 3 UTF-8 bytes) appears once in bullet 2.
  /// Total byte length: 709 (re-verify via `wc -c` after each
  /// claude-code rebase).
  pub const FOOTER: &str = "Notes:\n\
  - Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.\n\
  - In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) — do not recap code you merely read.\n\
  - For clear communication with the user the assistant MUST avoid using emojis.\n\
  - Do not use a colon before tool calls. Text like \"Let me read the file:\" followed by a read tool call should just be \"Let me read the file.\" with a period.\n";
  ```

- [ ] Step 2 — In `lingxi-code/crates/orchestrator/src/prompt/mod.rs`, AFTER the type definitions, add `assemble_system_prompt`:
  ```rust
  use crate::prompt::locked_templates::{FOOTER, HEADER, SECTION_SEP, TRAILING_NL};

  /// Assemble a system prompt from a [`SystemPromptContext`].
  ///
  /// Section order is LOCKED:
  ///
  /// 1. `HEADER`
  /// 2. `<env>...</env>` + model description + cutoff
  /// 3. `<memory>...</memory>` (elided when no files)
  /// 4. `<tools>...</tools>` (elided when no names)
  /// 5. `FOOTER`
  ///
  /// Separator between sections is exactly `\n\n` (one blank line).
  /// Final character is a single `\n`.
  #[must_use]
  pub fn assemble_system_prompt(ctx: &SystemPromptContext) -> String {
      let mut s = String::with_capacity(2048);
      s.push_str(HEADER);
      s.push_str(SECTION_SEP);
      s.push_str(&env_block::format(ctx));

      let memory = memory_block::format(&ctx.memory_files);
      if !memory.is_empty() {
          s.push_str(SECTION_SEP);
          s.push_str(&memory);
      }

      let tools = tools_block::format(&ctx.tool_names);
      if !tools.is_empty() {
          s.push_str(SECTION_SEP);
          s.push_str(&tools);
      }

      s.push_str(SECTION_SEP);
      s.push_str(FOOTER);
      // FOOTER already ends with \n; do NOT add another LF.
      // (TRAILING_NL is reserved for callers building manually — kept as
      // a public const for future plans that need explicit boundary.)
      let _ = TRAILING_NL; // suppress unused-import lint when feature gates change.
      s
  }
  ```

- [ ] Step 3 — Create `lingxi-code/crates/orchestrator/tests/prompt_assemble_test.rs`:
  ```rust
  //! Full `assemble_system_prompt` byte-locks (M5-03 Task 11).

  use lingxi_orchestrator::prompt::{
      assemble_system_prompt, FileTree, MemoryFile, SystemPromptContext,
  };
  use std::path::PathBuf;

  fn ctx_minimal() -> SystemPromptContext {
      SystemPromptContext {
          cwd: PathBuf::from("/proj"),
          platform: "darwin".into(),
          model: "claude-opus-4-7".into(),
          model_marketing_name: Some("Opus 4.7".into()),
          knowledge_cutoff: Some("January 2026".into()),
          shell: "zsh".into(),
          os_version: "Darwin 25.3.0".into(),
          git_status: None,
          file_tree: FileTree::default(),
          memory_files: Vec::new(),
          tool_names: Vec::new(),
      }
  }

  #[test]
  fn minimal_assembly_no_memory_no_tools() {
      let out = assemble_system_prompt(&ctx_minimal());
      // Must start with HEADER.
      assert!(out.starts_with("You are Claude Code, Anthropic's official CLI for Claude."));
      // Must contain `<env>` and `</env>`.
      assert!(out.contains("<env>\n"));
      assert!(out.contains("</env>\n"));
      // MUST NOT contain `<memory>` or `<tools>` (both empty in this context).
      assert!(!out.contains("<memory>"));
      assert!(!out.contains("<tools>"));
      // Must end with the footer's final bullet + LF.
      assert!(out.ends_with("with a period.\n"));
  }

  #[test]
  fn section_order_locked_header_env_memory_tools_footer() {
      let mut ctx = ctx_minimal();
      ctx.memory_files = vec![MemoryFile {
          path: PathBuf::from("/proj/CLAUDE.md"),
          body: "notes".into(),
          is_local_override: false,
      }];
      ctx.tool_names = vec!["Read".into(), "Write".into()];
      let out = assemble_system_prompt(&ctx);
      // Locate the section markers in the output and assert order.
      let i_header = out.find("You are Claude Code").expect("header present");
      let i_env = out.find("<env>").expect("env present");
      let i_memory = out.find("<memory>").expect("memory present");
      let i_tools = out.find("<tools>").expect("tools present");
      let i_footer = out.find("Notes:").expect("footer present");
      assert!(i_header < i_env);
      assert!(i_env < i_memory);
      assert!(i_memory < i_tools);
      assert!(i_tools < i_footer);
  }

  #[test]
  fn double_lf_between_each_section() {
      let mut ctx = ctx_minimal();
      ctx.memory_files = vec![MemoryFile {
          path: PathBuf::from("/p/CLAUDE.md"),
          body: "m".into(),
          is_local_override: false,
      }];
      ctx.tool_names = vec!["X".into()];
      let out = assemble_system_prompt(&ctx);
      // After HEADER, before `<env>` — should be exactly `\n\n`.
      let header_end = "You are Claude Code, Anthropic's official CLI for Claude.";
      let after_header = &out[out.find(header_end).unwrap() + header_end.len()..];
      assert!(after_header.starts_with("\n\n<env>"));
      // After `</env>...cutoff line`, before `<memory>` — must contain `\n\n<memory>`.
      assert!(out.contains("\n\n<memory>"));
      // After `</memory>\n`, before `<tools>` — must contain `\n\n<tools>`.
      assert!(out.contains("</memory>\n\n<tools>"));
      // After `</tools>\n`, before `Notes:` — must contain `\n\nNotes:`.
      assert!(out.contains("</tools>\n\nNotes:"));
  }

  #[test]
  fn footer_byte_length_locked() {
      // FOOTER literal length is locked at 709 bytes (see plan).
      assert_eq!(
          lingxi_orchestrator::prompt::locked_templates::FOOTER.len(),
          709
      );
  }
  ```

  Note on the `709` lock: if `wc -c` on the literal produces a different number after Step 1 edit, update both the docstring AND this assertion to the actual count. The lock is the EXACT byte length, whatever it is — drift detection only.

- [ ] Step 4 — Run `cargo test -p lingxi-orchestrator --test prompt_assemble_test`. All 4 tests must pass.

- [ ] Step 5 — If `footer_byte_length_locked` fails, run `printf %s "$(awk '/pub const FOOTER/,/^";$/' lingxi-code/crates/orchestrator/src/prompt/locked_templates.rs)" | wc -c` (or count bytes directly via a one-off Rust file) to discover the real byte length, then update both the docstring in `locked_templates.rs` and the assertion. Then re-run.

- [ ] Commit: `feat(M5-03 task 11): assemble_system_prompt + FOOTER literal (full section-order byte-lock)`

---

### Task 12: Wire `assemble_system_prompt` into `ConversationOrchestrator::run_turn`

**Files:**
- Modify: `lingxi-code/crates/api-client/src/anthropic.rs` (`messages_create_non_stream` signature)
- Modify: `lingxi-code/crates/orchestrator/src/conversation.rs` (trait + struct + `new` + `run_turn`)
- Modify: `lingxi-code/crates/orchestrator/src/turn_loop.rs` (`execute_one_turn` arg)
- Modify: `lingxi-code/crates/orchestrator/src/test_support.rs` (MockApiClient captured_systems)
- Modify: `lingxi-code/crates/orchestrator/src/lib.rs` (pub use of prompt types)

**Steps:**

- [ ] Step 1 — Modify `lingxi-code/crates/api-client/src/anthropic.rs:211`. Change the signature of `messages_create_non_stream` to:
  ```rust
  pub async fn messages_create_non_stream<T: HttpTransport>(
      &self,
      model: &str,
      system: Option<&str>,
      msgs: Vec<ConversationMessage>,
      transport: &T,
  ) -> Result<MessageResponse, ApiError> {
      let request_id = new_request_id();
      let started = std::time::Instant::now();
      telemetry::emit_started(&self.bus, model, &request_id, false).await;

      let mut body = serde_json::json!({
          "model": model,
          "max_tokens": 4096u32,
          "messages": msgs,
      });
      if let Some(s) = system {
          body["system"] = serde_json::Value::String(s.to_string());
      }

      let resp_result = self.drive_retry_loop_with_429(&body, transport).await;
      let outcome = self
          .resolve_outcome(resp_result, &body, model, &request_id, transport)
          .await;

      self.emit_terminal_event(&outcome, model, &request_id, started)
          .await;

      if let Ok(ref message_response) = outcome {
          self.record_cost_for_response(message_response, model, started.elapsed())
              .await;
      }

      outcome
  }
  ```
  Update ALL call sites inside `lingxi-code/crates/api-client/` (likely only test modules in the same file). For each, insert `None` as the new third arg.

- [ ] Step 2 — Modify `lingxi-code/crates/orchestrator/src/conversation.rs`. Change the trait `OrchestratorApiClient::messages_create` to take `system: Option<&str>` as its third parameter:
  ```rust
  #[async_trait]
  pub trait OrchestratorApiClient: Send + Sync {
      async fn messages_create(
          &self,
          model: &str,
          system: Option<&str>,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError>;
  }
  ```
  Update the production `AnthropicProviderAdapter` impl to thread `system` through:
  ```rust
  #[async_trait]
  impl<T: HttpTransport + Send + Sync + 'static> OrchestratorApiClient for AnthropicProviderAdapter<T> {
      async fn messages_create(
          &self,
          model: &str,
          system: Option<&str>,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError> {
          self.provider
              .messages_create_non_stream(model, system, msgs, self.transport.as_ref())
              .await
      }
  }
  ```

- [ ] Step 3 — Modify `lingxi-code/crates/orchestrator/src/test_support.rs::MockApiClient`. Add a `captured_systems` field and accessor:
  ```rust
  pub struct MockApiClient {
      queue: Arc<Mutex<VecDeque<MessageResponse>>>,
      captured_msgs: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
      captured_systems: Arc<Mutex<Vec<Option<String>>>>,
  }

  impl MockApiClient {
      pub fn new(responses: Vec<MessageResponse>) -> Self {
          Self {
              queue: Arc::new(Mutex::new(VecDeque::from(responses))),
              captured_msgs: Arc::new(Mutex::new(Vec::new())),
              captured_systems: Arc::new(Mutex::new(Vec::new())),
          }
      }

      pub async fn captured_systems(&self) -> Vec<Option<String>> {
          self.captured_systems.lock().await.clone()
      }

      // ... existing captured_msgs / remaining methods unchanged ...
  }

  #[async_trait]
  impl OrchestratorApiClient for MockApiClient {
      async fn messages_create(
          &self,
          _model: &str,
          system: Option<&str>,
          msgs: Vec<ConversationMessage>,
      ) -> Result<MessageResponse, ApiError> {
          self.captured_msgs.lock().await.push(msgs);
          self.captured_systems
              .lock()
              .await
              .push(system.map(str::to_string));
          let mut q = self.queue.lock().await;
          q.pop_front().ok_or_else(|| {
              ApiError::ProviderError("mock script exhausted".into())
          })
      }
  }
  ```

- [ ] Step 4 — Modify `lingxi-code/crates/orchestrator/src/conversation.rs::ConversationOrchestrator`. Add two new fields to the struct (in this exact order — append after `session`):
  ```rust
  pub struct ConversationOrchestrator {
      pub(crate) config: OrchestratorConfig,
      pub(crate) api: Arc<dyn OrchestratorApiClient>,
      pub(crate) tools: Arc<ToolRegistry>,
      pub(crate) hooks: Arc<dyn HookExecutor>,
      pub(crate) perms: Arc<dyn PermissionGate>,
      pub(crate) output: Arc<dyn OutputStream>,
      pub(crate) session: Arc<Mutex<SessionState>>,
      pub(crate) memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
      pub(crate) cwd: std::path::PathBuf,
  }
  ```
  Update `new(...)` constructor — add two new parameters in the same order at the end:
  ```rust
  impl ConversationOrchestrator {
      #[allow(clippy::too_many_arguments)]
      pub fn new(
          config: OrchestratorConfig,
          api: Arc<dyn OrchestratorApiClient>,
          tools: Arc<ToolRegistry>,
          hooks: Arc<dyn HookExecutor>,
          perms: Arc<dyn PermissionGate>,
          output: Arc<dyn OutputStream>,
          memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
          cwd: std::path::PathBuf,
      ) -> Self {
          let session = SessionState::empty(SessionId::new(), config.model.clone());
          Self {
              config,
              api,
              tools,
              hooks,
              perms,
              output,
              session: Arc::new(Mutex::new(session)),
              memory,
              cwd,
          }
      }
      // ... rest unchanged ...
  }
  ```

- [ ] Step 5 — Modify `lingxi-code/crates/orchestrator/src/conversation.rs::run_turn` to assemble + thread the system prompt. Replace the existing `run_turn` with:
  ```rust
  pub async fn run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
      // Telemetry: conversation_started. (Wired in M5-02 Task 15.)

      // Build the system prompt for THIS turn. Override wins.
      let system_prompt: Option<String> = match &self.config.system_prompt_override {
          Some(custom) => Some(custom.clone()),
          None => Some(self.build_system_prompt().await),
      };

      // 1. Append the user prompt to session history.
      {
          let mut s = self.session.lock().await;
          let msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
          s.history.push(msg);
      }

      // 2. Turn-by-turn driver.
      let mut turn_count: u32 = 0;
      let final_message_id;
      loop {
          if turn_count >= self.config.max_turns {
              return Err(OrchestratorError::MaxTurnsReached {
                  max_turns: self.config.max_turns,
              });
          }
          turn_count = turn_count.saturating_add(1);

          let step = crate::turn_loop::execute_one_turn(self, system_prompt.as_deref()).await?;
          match step {
              crate::turn_loop::TurnStepOutcome::Continue => continue,
              crate::turn_loop::TurnStepOutcome::Ended {
                  final_message_id: id,
                  stop_reason,
              } => {
                  let cost = {
                      let s = self.session.lock().await;
                      cost_snapshot_from_session(&s)
                  };
                  self.output.emit_end_turn(&stop_reason, &cost).await;
                  final_message_id = id;
                  break;
              }
          }
      }

      // Telemetry: conversation_completed. (Wired in M5-02 Task 15.)
      Ok(ConversationOutcome::EndTurn {
          turn_count,
          final_message_id,
      })
  }

  /// Build the per-turn system prompt by gathering cwd / git / file
  /// tree / memory / tool-name context and calling
  /// [`crate::prompt::assemble_system_prompt`].
  async fn build_system_prompt(&self) -> String {
      use crate::prompt::{
          assemble_system_prompt, file_tree, git_status, SystemPromptContext,
      };

      let cwd = self.cwd.clone();
      let memory_files = self.memory.load(&cwd).await;

      let git = git_status::probe(&cwd);
      let tree = file_tree::probe(&cwd, file_tree::DEFAULT_DEPTH_LIMIT);

      let tool_names: Vec<String> = {
          // Tool registry is a sync structure; M5-03 only needs names.
          // M4-08 added `all_tool_names_for_system_prompt()` — but if
          // that helper does not exist on `ToolRegistry`, fall back to
          // iterating over `register_builtin` insertions. The simplest
          // surface available at M5-02-head is `available_tools` which
          // needs a `ToolStaticContext`. M5-03 introduces a thin local
          // helper that builds a minimal context:
          let ctx = lingxi_tools::context::ToolStaticContext::minimal();
          self.tools
              .available_tools(&ctx)
              .iter()
              .map(|t| t.name().to_string())
              .collect()
      };

      let model = self.config.model.clone();
      let ctx = SystemPromptContext {
          cwd,
          platform: std::env::consts::OS.to_string(),
          model,
          model_marketing_name: None, // M5-12 CLI fills this when known.
          knowledge_cutoff: None,     // M5-12 CLI fills this when known.
          shell: std::env::var("SHELL")
              .ok()
              .and_then(|s| {
                  std::path::Path::new(&s)
                      .file_name()
                      .map(|n| n.to_string_lossy().into_owned())
              })
              .unwrap_or_else(|| "sh".into()),
          os_version: format!(
              "{} {}",
              std::env::consts::OS,
              std::env::consts::ARCH
          ),
          git_status: git,
          file_tree: tree,
          memory_files,
          tool_names,
      };
      assemble_system_prompt(&ctx)
  }
  ```

  Note on `ToolStaticContext::minimal()`: if this constructor does not exist on `ToolStaticContext` (it likely doesn't at M5-02 head), use whatever existing `ToolStaticContext::default()` or `::new()` is available. Confirm via `grep -n "impl ToolStaticContext\|pub fn" lingxi-code/crates/tools/src/context.rs` and pick the no-arg constructor. If NONE exists, the cleanest fix is calling `self.tools.register_iter()` directly to collect tool names — but that bypasses `available_tools` filtering. Pragmatic choice: extend `ToolRegistry` with a new `pub fn tool_names(&self) -> Vec<String>` method (one-line change in `lingxi-code/crates/tools/src/registry.rs`) that ignores filters. ADD that method in Step 5b below if `ToolStaticContext::minimal()` doesn't exist.

- [ ] Step 5b — Optional: if `ToolStaticContext::minimal()` / `::default()` / `::new()` doesn't exist, add to `lingxi-code/crates/tools/src/registry.rs::ToolRegistry`:
  ```rust
  /// Return ALL registered tool names (builtin + MCP + plugin), no
  /// filtering. Used by `lingxi-orchestrator` system prompt assembler.
  ///
  /// Order is registration order — the assembler re-sorts alphabetically.
  #[must_use]
  pub fn tool_names(&self) -> Vec<String> {
      // Concrete implementation depends on ToolRegistry internals;
      // most plausibly: iterate the internal Vec<Arc<dyn Tool>> and
      // collect via `t.name()`. See lib.rs line 50+ for the actual
      // field name to iterate.
      self.builtin
          .iter()
          .chain(self.mcp_tools.values().flatten())
          .chain(self.plugin_tools.values().flatten())
          .map(|t| t.name().to_string())
          .collect()
  }
  ```
  Then in `build_system_prompt`, replace the tool-name collection with `self.tools.tool_names()`. The exact `.builtin` / `.mcp_tools` field names must match the actual struct — verify by reading the file before editing.

- [ ] Step 6 — Modify `lingxi-code/crates/orchestrator/src/turn_loop.rs::execute_one_turn`. Change the signature to accept the system arg, and thread it into the API call:
  ```rust
  pub(crate) async fn execute_one_turn(
      orch: &ConversationOrchestrator,
      system: Option<&str>,
  ) -> Result<TurnStepOutcome, OrchestratorError> {
      let (history_snapshot, model) = {
          let s = orch.session.lock().await;
          (s.history.clone(), s.model.clone())
      };

      // 1. Call the API — now with system prompt.
      let response = orch.api.messages_create(&model, system, history_snapshot).await?;

      // ... rest of the body unchanged from M5-02 ...
  }
  ```

- [ ] Step 7 — Modify `lingxi-code/crates/orchestrator/src/lib.rs`. Add the re-export line at the bottom (after the existing `pub use error::OrchestratorError;`):
  ```rust
  pub use prompt::{
      assemble_system_prompt, FileTree, FileTreeEntry, GitStatus, MemoryFile,
      SystemPromptContext,
  };
  ```

- [ ] Step 8 — Create `lingxi-code/crates/orchestrator/tests/prompt_orchestrator_wiring_test.rs`:
  ```rust
  //! Integration: ConversationOrchestrator::run_turn assembles a
  //! system prompt via assemble_system_prompt and passes it to the
  //! API client.

  use lingxi_api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
  use lingxi_orchestrator::test_support::{
      MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
      StaticMemoryProvider,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_tools::registry::ToolRegistry;
  use std::sync::Arc;
  use tempfile::TempDir;

  fn end_turn_response() -> MessageResponse {
      MessageResponse {
          id: "msg_1".into(),
          model: "claude-opus-4-7".into(),
          content: vec![ContentBlockApi::Text { text: "ok".into() }],
          stop_reason: Some("end_turn".into()),
          usage: UsageApi::default(),
      }
  }

  #[tokio::test]
  async fn run_turn_passes_assembled_system_prompt_to_api_client() {
      let tmp = TempDir::new().unwrap();
      let api = Arc::new(MockApiClient::new(vec![end_turn_response()]));
      let tools = Arc::new(ToolRegistry::new());
      let hooks = Arc::new(NoOpHookExecutor::new());
      let perms = Arc::new(NoOpPermissionGate::new());
      let output = Arc::new(MockOutputStream::new());
      let memory = Arc::new(StaticMemoryProvider::empty());

      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api.clone(),
          tools,
          hooks,
          perms,
          output,
          memory,
          tmp.path().to_path_buf(),
      );
      orch.run_turn("hi").await.expect("turn");

      let systems = api.captured_systems().await;
      assert_eq!(systems.len(), 1, "exactly one API call");
      let s = systems[0].as_deref().expect("system prompt threaded");
      assert!(s.starts_with("You are Claude Code"));
      assert!(s.contains("<env>"));
      assert!(s.ends_with("with a period.\n"));
  }
  ```

- [ ] Step 9 — Run `cargo build -p lingxi-orchestrator -p lingxi-api-client`. Must succeed. Compile errors here usually mean a call-site of `messages_create_non_stream` missed the new `system` arg — fix and re-run.

- [ ] Step 10 — Run `cargo test -p lingxi-orchestrator --test prompt_orchestrator_wiring_test`. Must pass.

- [ ] Step 11 — Run the M5-02 baseline tests (un-updated yet): `cargo test -p lingxi-orchestrator --test orchestrator_smoke_test --test orchestrator_multi_turn_test --test orchestrator_max_turns_test --test orchestrator_tool_error_test --test orchestrator_real_tools_test 2>&1 | tail -20`. EXPECTED: 5 test files fail to compile because `ConversationOrchestrator::new` now takes 8 args instead of 6. Task 13 fixes them.

- [ ] Commit: `feat(M5-03 task 12): wire assemble_system_prompt into ConversationOrchestrator + OrchestratorApiClient::messages_create gains system arg`

---

### Task 13: Update M5-02 baseline tests + add override-bypass test

**Files:**
- Modify: `lingxi-code/crates/orchestrator/tests/orchestrator_smoke_test.rs`
- Modify: `lingxi-code/crates/orchestrator/tests/orchestrator_multi_turn_test.rs`
- Modify: `lingxi-code/crates/orchestrator/tests/orchestrator_max_turns_test.rs`
- Modify: `lingxi-code/crates/orchestrator/tests/orchestrator_tool_error_test.rs`
- Modify: `lingxi-code/crates/orchestrator/tests/orchestrator_real_tools_test.rs`
- Create: `lingxi-code/crates/orchestrator/tests/prompt_override_bypass_test.rs`

**Steps:**

- [ ] Step 1 — In `lingxi-code/crates/orchestrator/tests/orchestrator_smoke_test.rs`, find every `ConversationOrchestrator::new(cfg, api, tools, hooks, perms, output)` call. Replace each with:
  ```rust
  ConversationOrchestrator::new(
      cfg,
      api,
      tools,
      hooks,
      perms,
      output,
      Arc::new(StaticMemoryProvider::empty()),
      std::env::temp_dir(),
  )
  ```
  Add the import at the top of the file: `use lingxi_orchestrator::test_support::StaticMemoryProvider;` (if not already present).

- [ ] Step 2 — Repeat Step 1 for `orchestrator_multi_turn_test.rs`.

- [ ] Step 3 — Repeat Step 1 for `orchestrator_max_turns_test.rs`.

- [ ] Step 4 — Repeat Step 1 for `orchestrator_tool_error_test.rs`.

- [ ] Step 5 — Repeat Step 1 for `orchestrator_real_tools_test.rs`. For this file, the cwd should be a `tempfile::TempDir` that the real tools (FileReadTool) can write into; pass `tmp.path().to_path_buf()` instead of `std::env::temp_dir()`.

- [ ] Step 6 — Create `lingxi-code/crates/orchestrator/tests/prompt_override_bypass_test.rs`:
  ```rust
  //! `OrchestratorConfig::system_prompt_override = Some(_)` skips the
  //! prompt assembler and forwards the literal byte-for-byte.

  use lingxi_api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
  use lingxi_orchestrator::test_support::{
      MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
      StaticMemoryProvider,
  };
  use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
  use lingxi_tools::registry::ToolRegistry;
  use std::sync::Arc;

  fn end_turn() -> MessageResponse {
      MessageResponse {
          id: "msg_1".into(),
          model: "claude-opus-4-7".into(),
          content: vec![ContentBlockApi::Text { text: "ok".into() }],
          stop_reason: Some("end_turn".into()),
          usage: UsageApi::default(),
      }
  }

  #[tokio::test]
  async fn override_is_forwarded_verbatim_bypassing_assembler() {
      let api = Arc::new(MockApiClient::new(vec![end_turn()]));
      let cfg = OrchestratorConfig {
          max_turns: 5,
          model: "claude-opus-4-7".into(),
          system_prompt_override: Some("CUSTOM PROMPT — no assembler".into()),
      };
      let orch = ConversationOrchestrator::new(
          cfg,
          api.clone(),
          Arc::new(ToolRegistry::new()),
          Arc::new(NoOpHookExecutor::new()),
          Arc::new(NoOpPermissionGate::new()),
          Arc::new(MockOutputStream::new()),
          Arc::new(StaticMemoryProvider::empty()),
          std::env::temp_dir(),
      );
      orch.run_turn("hi").await.expect("turn");

      let systems = api.captured_systems().await;
      assert_eq!(systems.len(), 1);
      assert_eq!(
          systems[0].as_deref(),
          Some("CUSTOM PROMPT — no assembler"),
          "override MUST be forwarded byte-for-byte"
      );
  }

  #[tokio::test]
  async fn default_config_uses_assembler() {
      let api = Arc::new(MockApiClient::new(vec![end_turn()]));
      let orch = ConversationOrchestrator::new(
          OrchestratorConfig::default(),
          api.clone(),
          Arc::new(ToolRegistry::new()),
          Arc::new(NoOpHookExecutor::new()),
          Arc::new(NoOpPermissionGate::new()),
          Arc::new(MockOutputStream::new()),
          Arc::new(StaticMemoryProvider::empty()),
          std::env::temp_dir(),
      );
      orch.run_turn("hi").await.expect("turn");
      let s = api
          .captured_systems()
          .await
          .into_iter()
          .next()
          .flatten()
          .expect("system prompt");
      // Assembler always opens with HEADER.
      assert!(s.starts_with("You are Claude Code"));
      // Override sentinel must NOT appear.
      assert!(!s.contains("CUSTOM PROMPT"));
  }
  ```

- [ ] Step 7 — Run `cargo test -p lingxi-orchestrator`. ALL test binaries must pass (the 5 M5-02 ones + the 7 M5-03 ones).

- [ ] Commit: `feat(M5-03 task 13): refit M5-02 baseline tests to 8-arg ::new + add override-bypass test`

---

### Task 14: Verification gate (workspace `cargo test` + clippy + fmt + telemetry-stay-at-241)

**Files:**
- No file modifications expected. If any verification step fails, fix-then-recommit cycle within this task.

**Steps:**

- [ ] Step 1 — Run `cargo fmt --all --check`. Must produce no output. If anything reports, run `cargo fmt --all`, inspect the diff, and amend or new-commit per repo convention.

- [ ] Step 2 — Run `cargo clippy --workspace --all-targets -- -D warnings`. Must succeed with no warnings. Common pitfalls in this plan:
  - `#[allow(clippy::too_many_arguments)]` is already on `ConversationOrchestrator::new` from Step 12.4; if clippy still complains, double-check the attribute placement.
  - `clippy::module_name_repetitions` may complain about `prompt::env_block::format` — silence at the function level (`#[allow(clippy::module_name_repetitions)]`) if needed.

- [ ] Step 3 — Telemetry count regression. Run `cargo test -p lingxi-telemetry --test event_name_completeness_test`. Must pass at `241` (unchanged from M5-02). If this fails, an event was added by mistake somewhere in this plan — find and remove.

- [ ] Step 4 — Dep cycle regression. Run `cargo tree -p lingxi-memory -e normal --depth 5 2>&1 | grep -c lingxi-orchestrator`. Must print `0`. If non-zero, the memory crate accidentally took a dep on orchestrator — find and remove.

- [ ] Step 5 — Full workspace test pass. Run `cargo test --workspace --all-features 2>&1 | tail -20`. All test binaries must be green. The M5-03 work added 7 new test files; the M5-02 baseline 5 are now updated and must still pass.

- [ ] Step 6 — Run `cargo doc --no-deps -p lingxi-orchestrator --document-private-items 2>&1 | tail -5`. Must produce no rustdoc warnings (M3-06 convention). If any rustdoc reference is broken, fix the doc-comment.

- [ ] Step 7 — Tag the release point:
  ```
  git tag m5.3 -m "M5-03: system prompt dynamic assembly (env + memory + tools blocks)"
  ```
  Do NOT push the tag (per repo convention, M5-XX micro-tags are local until M5-14).

- [ ] Step 8 — Final sanity: run `git log --oneline | head -20` and confirm 13 task commits land on top of `653de44` (M5-02), one per Task 1..13. Task 14 itself produces NO commit (verification only) — the tag is the only artifact.

- [ ] No commit for this task (verification only); the tag is the artifact.

---

## Self-review checklist

1. **Spec coverage** — All sections (`<env>` / `<memory>` / `<tools>` / header / footer) mapped to dedicated tasks (T6 / T9 / T10 / T4 / T11). T7 adds git probe; T8 adds file tree. T12 wires into `run_turn`. ✓
2. **Placeholder scan** — Every code snippet is complete (no `TBD`, no `...` in places that would prevent compilation). Wire identifiers (constant names, struct fields, function signatures) are byte-locked from claude-code or LingXi-locked at this plan. ✓
3. **Type consistency** — `SystemPromptContext`, `GitStatus`, `FileTree`, `FileTreeEntry`, `MemoryFile`, `MemoryHierarchyProvider` referenced uniformly across T2/T3/T6/T7/T8/T9/T11/T12. ✓
4. **T0 reverse-engineer findings** — Captured at top of plan in "Reverse-engineered byte-locks" table with claude-code source line numbers. T0 produces NO commit (research only). ✓
5. **Memory hierarchy order** — `~/.claude/CLAUDE.md` → `<repo>/CLAUDE.md` → `<repo>/CLAUDE.local.md` matches M3-02 lock (spec §4.3); T9 step 2 REVERSES the M3-02 `walk()` innermost-first vector. Test `multi_entry_splice_order_locked` asserts the order at the formatter level; test `real_provider_loads_in_spec_splice_order_via_temp_repo` asserts end-to-end via the real provider. ✓
6. **No new telemetry events** — T14 step 3 asserts `ALL_EVENT_NAMES.len() == 241` (unchanged). ✓
7. **No cycle** — `lingxi-memory` does NOT pull `lingxi-orchestrator`; T9 step 1 + T14 step 4 verify. ✓
8. **API trait additive change** — `OrchestratorApiClient::messages_create` gains `system: Option<&str>` as the third arg; `MockApiClient` captures it for assertion; `AnthropicProviderAdapter` forwards it; `AnthropicProvider::messages_create_non_stream` accepts it and conditionally inserts into the JSON body. All call sites (5 M5-02 baseline tests) updated in T13. ✓
9. **Override bypass** — T13 step 6 covers `system_prompt_override: Some(_)` skips the assembler and forwards verbatim. ✓
10. **Section omission** — `<memory>` and `<tools>` are elided entirely when empty (no empty tags emitted). T9 + T10 + T11 step 3 (`minimal_assembly_no_memory_no_tools`) all assert this. ✓
11. **Byte-locks** — Header (57 bytes), footer (709 bytes), section separator (2 bytes), trailing newline (1 byte) all asserted with `assert_eq!` on `.len()`. Drift detection only — actual values may differ if claude-code rebases. ✓
12. **Test pyramid** — 7 new integration tests (one per formatter + assembler + wiring + override) + 5 updated M5-02 baseline tests + in-file unit tests. Workspace test count grows by exactly 7 binaries. ✓

---

## Out-of-scope (defer to later M5 plans)

- **SSE streaming** — M5-04. The system prompt is built ONCE per `run_turn`, then re-used across every `execute_one_turn` iteration. M5-04 will NOT re-assemble per turn-step; the prompt is stable across the conversation lifetime.
- **`additionalWorkingDirectories`** — M5-12 CLI. The `<env>` block has space for `Additional working directories:` lines (claude-code:683-688) but M5-03 omits them; M5-12 adds CLI flag `--add-dir`.
- **Model marketing name / knowledge cutoff lookup** — M5-12 CLI. M5-03 leaves `model_marketing_name: None` and `knowledge_cutoff: None` in the default-construction path; M5-12 populates from a model registry (the lookup table at `claude-code:712-730`).
- **Skill discovery framing** — M5-09 / M5-10. The `DiscoverSkills` guidance bullet (`claude-code:333-340`) is OUT of scope here; M5-09 wires the 102-command surface and may extend the footer.
- **Output style / language sections** — M5-10 (`/config`). Out of scope.
- **MCP server instructions** — M5-07 / M5-09. Out of scope.
- **Subagent system prompt (`DEFAULT_AGENT_PROMPT`)** — M5-06 hooks runtime / M5-12 CLI. M5-03 is main-session only.
- **Prompt cache prefix boundary marker** — `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` from `claude-code:573` is NOT ported. LingXi M5-03 has no cache layer; M6 may add one (out of scope of v0.6.0).

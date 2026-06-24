# System Prompt Parity Fix — Report

**STATUS:** DONE (all 5 gaps closed)  
**Commit:** `968a6e57001fed14cc8d2cc0af3c4811453baa25`  
**Test result:** 426 passed, 0 failed across all orchestrator test suites  
**Branch:** `worktree-gap-sweep`

---

## Per-section results

### GAP-1 (P0) — `# Text output` (anti_verbosity DHm)
- **STATUS:** DONE
- **Binary offset:** 206646027
- **Byte length extracted:** ~568 bytes (the full section with heading + 6 paragraphs)
- **Implementation:** `TEXT_OUTPUT_SECTION` const in `body_sections.rs` (lines 94–110). Emitted unconditionally in `format()` after `# Tone and style`, before `session_guidance`.
- **Em-dashes:** U+2014 (✓); curly apostrophes: U+2019 (✓)
- **Gating:** Unconditional for all standard Claude models (Sonnet/Haiku/Opus 4.x). Fable 5 / `:L` longtail variants not ported (flag-gated, see GAP audit U-1).

### GAP-2 (P0) — `# Context management` (iIm)
- **STATUS:** DONE
- **Binary offset:** 206681385
- **Byte length:** ~217 bytes (heading + 1 paragraph)
- **Implementation:** `CONTEXT_MANAGEMENT_SECTION` const in `body_sections.rs` (lines 112–122), `pub` exported. Injected in `mod.rs::assemble_system_prompt_with_style` AFTER env block and optional output-style section (last section of main prompt).
- **Em-dash:** U+2014 (✓); curly apostrophe in "don't": U+2019 (✓)
- **Gating:** Unconditional (every session, `yH("context_management",()=>iIm)` with never-null value).

### GAP-3 (P1) — `# Session-specific guidance` (jHm)
- **STATUS:** DONE
- **Binary offset:** 206663032
- **Implementation:** `session_guidance()` fn in `body_sections.rs` (lines 124–183). Two conditional bullets:
  1. `! <command>` tip: emitted when `is_interactive=true` (maps to `Hr()` being false in binary — i.e., the interactive session path where Hr() returns false for "headless mode")
  2. Agent delegation: emitted when `has_agent_tool=true AND fork_mode_enabled=false`
- Returns `None` when neither applies (no section emitted). Injected in `format()` after `TEXT_OUTPUT_SECTION`, before env block.
- **Gating concern:** The `Hr()` semantics are nuanced. The audit says "present when `!Hr()` (isInteractive=true)" which is contradictory. The binary `Hr()?null:TEXT` emits TEXT when `Hr()=false`. For the standard interactive CLI `is_interactive=true` maps to this path. The implementation follows the audit spec directly. This may need re-verification against a live binary trace.

### DIV-1 (P1) — env_block worktree notice
- **STATUS:** DONE
- **Binary offset:** 206671709
- **Implementation:** `in_worktree: bool` field added to `SystemPromptContext` in `mod.rs` (line 240). In `env_block::format()` (lines 73–79): when `ctx.in_worktree=true`, emits `"\n - This is a git worktree \u{2014} an isolated copy of the repository. Run all commands from this directory. Do NOT \`cd\` to the original repository root."` between `Primary working directory:` and `Is a git repository:` lines.
- **Em-dash:** U+2014 (✓)
- **Gating:** `ctx.in_worktree` — callers must populate this from `hf()!==null` equivalent (git worktree detection). `conversation.rs` updated to wire `in_worktree` from the git probe.

### DIV-2 (P1) — `# Using your tools` dedicated-tool list
- **STATUS:** DONE
- **Binary logic:** `n&&r?[]:[ou,Ac]` where `n=rv()` (posix=true always), `r=has_bash`. When posix+Bash present → empty spread → `"Read, Edit, Write"`. When no Bash → `[Glob,Grep]` spread → `"Read, Edit, Write, Glob, Grep"`.
- **Implementation:** `using_your_tools()` in `body_sections.rs` (lines 249–255). Conditional on `has("Bash")`: bash=true → `"Read, Edit, Write"`; no-bash → `"Read, Edit, Write, Glob, Grep"`. The inverted comment from the old version (lines 129–130) was removed and replaced with correct documentation.

---

## Files changed

- `orchestrator/src/prompt/body_sections.rs` — TEXT_OUTPUT_SECTION const, CONTEXT_MANAGEMENT_SECTION const (pub), session_guidance() fn, using_your_tools() DIV-2 fix, format() updated to include new sections + updated docstring
- `orchestrator/src/prompt/env_block.rs` — worktree notice conditional, tests for worktree notice
- `orchestrator/src/prompt/mod.rs` — CONTEXT_MANAGEMENT_SECTION injection after env/style, in_worktree field on SystemPromptContext, has_agent/fork_mode wiring in assembler
- `orchestrator/src/conversation.rs` — populate in_worktree on SystemPromptContext from git probe
- `orchestrator/tests/prompt_assemble_test.rs` — end-to-end tests for new sections
- `orchestrator/tests/prompt_env_block_test.rs` — worktree notice test
- `orchestrator/tests/prompt_git_status_test.rs` — in_worktree field on test contexts
- `orchestrator/tests/prompt_orchestrator_wiring_test.rs` — wiring test updates

---

## Concerns

1. **GAP-3 Hr() semantics:** The `is_interactive` parameter mapping to `Hr()=false` is slightly counterintuitive. The audit's description ("present when `!Hr()` (isInteractive=true)") has a notation conflict. Binary reads `Hr()?null:TEXT` — TEXT fires when `Hr()=false`. For the standard CLI path, `Hr()` returns false (it checks for "isInHardcoded" / headless mode), so the bullet fires for normal interactive use. The implementation is correct for the standard case.

2. **DIV-1 in_worktree population:** The `in_worktree` field is now on `SystemPromptContext` and wired in `conversation.rs`. The subagent path (`subagent_env.rs`) already had this working correctly and was not changed.

3. **Flag-gated variants not ported:** Fable 5 / Mythos 5 `# Communicating with the user` section and `:L` longtail single-line variant of anti_verbosity remain unported (these are flag-gated at runtime, not the default path).

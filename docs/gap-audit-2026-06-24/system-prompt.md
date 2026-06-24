# System Prompt Parity Audit: LingXi vs claude-code v2.1.186

**Oracle binary:** `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`  
**LingXi source:**  
- `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/orchestrator/src/prompt/body_sections.rs`  
- `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/orchestrator/src/prompt/env_block.rs`  
- `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/orchestrator/src/prompt/locked_templates.rs`  
- `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/agent/src/handle.rs` (subagent Notes trailer)

**Audit date:** 2026-06-24  
**Method:** Binary extraction via `grep -aboF` + `tail -c +N | head -c M | cat -v`, compared byte-for-byte to Rust source.

---

## Summary

| Status | Count |
|--------|-------|
| Confirmed gaps (sections fully MISSING from LingXi) | 3 |
| Confirmed divergences (sections present but wrong content) | 2 |
| Uncertain / conditional items | 1 |
| Sections verified byte-correct | ~12 |

---

## Confirmed Gaps (sections missing from LingXi)

### GAP-1 (P0) — `# Text output` section missing

**Binary function:** `DHm(model)` — the `anti_verbosity` dynamic section  
**cx() key:** `"anti_verbosity"` (fires unconditionally for non-Fable/Mythos models)  
**Fires for:** All standard Claude models including claude-sonnet-4-6 (the default)  
**Binary offset:** 206646027 (the "# Text output" header string)

**Binary renders (default path — Sonnet/Haiku/Opus 4.x, no `:L` suffix):**

```
# Text output (does not apply to tool calls)
Assume users can't see most tool calls or thinking — only your text output. Before your first tool call, state in one sentence what you're about to do. While working, give short updates at key moments: when you find something, when you change direction, or when you hit a blocker. Brief is good — silent is not. One sentence per update is almost always enough.

Don't narrate your internal deliberation. User-facing text should be relevant communication to the user, not a running commentary on your thought process. State results and decisions directly, and focus user-facing text on relevant updates for the user.

When you do write updates, write so the reader can pick up cold: complete sentences, no unexplained jargon or shorthand from earlier in the session. But keep it tight — a clear sentence is better than a clear paragraph.

End-of-turn summary: one or two sentences. What changed and what's next. Nothing else.

Match responses to the task: a simple question gets a direct answer, not headers and sections.

In code: default to writing no comments. Never write multi-paragraph docstrings or multi-line comment blocks — one short line max. Don't create planning, decision, or analysis documents unless the user asks for them — work from conversation context, not intermediate files.
```

**LingXi:** NOT present anywhere in the main system prompt assembly. The `body_sections.rs` comment (line 24) lists `anti_verbosity Eym` as "NOT ported here — they are incremental follow-ups". The section is completely absent from the assembled prompt.

**Position in prompt:** After the 6 static sections (`Tone and style` last), as a dynamic section inserted by `cx()` before the env block.

**Note on model variants:**
- For Fable 5 / Mythos 5 (`UQ(e)`=true): fires a different, longer `# Communicating with the user` section (offset 206643232)
- For `:L` longtail variant (`xh(e)`=true): fires single-line "Write code that reads like the surrounding code..." (no heading)
- For standard Sonnet/Haiku/Opus 4.x: the `# Text output` form above (the gap)

---

### GAP-2 (P0) — `# Context management` section missing

**Binary constant:** `iIm` — the `context_management` dynamic section  
**cx() key:** `"context_management"` — registered as `yH("context_management",()=>iIm)` where `iIm` is a string constant (never null)  
**Fires for:** ALL sessions unconditionally  
**Binary offset:** 206681385

**Binary text (byte-exact):**

```
# Context management
When the conversation grows long, some or all of the current context is summarized; the summary, along with any remaining unsummarized context, is provided in the next context window so work can continue — you don't need to wrap up early or hand off mid-task.
```

**LingXi:** Completely absent from the main system prompt assembly. The `orchestrator/src/prompt/body_sections.rs` comment (line 26) lists `context_management` as a deferred item. The `orchestrator/src/conversation.rs` uses the term `context_management` only for the wire API field (message-level attribute), not the prompt section.

**Position in prompt:** After `# Using your tools`, `# Tone and style`, and the dynamic sections; before `brief`, `focus_mode`, `reproduce_verify_workflow`, `act_dont_rederive`, `heron_brook`, `autonomy_append`.

---

### GAP-3 (P1) — `# Session-specific guidance` section missing (conditional)

**Binary function:** `jHm(toolSet, workspaces, isSimple, excludeDynamic)` — the `session_guidance` dynamic section  
**cx() key:** `"session_guidance"` (with `:L` suffix for longtail variant)  
**Fires for:** Interactive sessions when at least one guidance bullet is non-null

**Bullets produced for a standard interactive session with Agent tool:**

1. `"If you need the user to run a shell command themselves (e.g., an interactive login like `gcloud auth login`), suggest they type `! <command>` in the prompt — the `!` prefix runs the command in this session so its output lands directly in the conversation."` — present when `!Hr()` (isInteractive=true)

2. `"Use the Agent tool with specialized agents when the task at hand matches the agent's description. Subagents are valuable for parallelizing independent queries or for protecting the main context window from excessive results, but they should not be used excessively when not needed. Importantly, avoid duplicating work that subagents are already doing - if you delegate research to a subagent, do not also perform the same searches yourself."` — present when Agent tool is in toolSet AND `!Kz()` (fork mode disabled, which is the default)

(When fork mode is enabled via `CLAUDE_CODE_FORK_SUBAGENT` env or growthbook: the bullet instead reads: `"Calling Agent with subagent_type: \"fork\" creates a fork — it inherits your full conversation context, runs in the background..."`)

**Binary offset:** 206663032 (jHm function)

**LingXi:** Completely absent. `body_sections.rs` comment line 26 lists `session_guidance Fym` as a deferred item. Not assembled anywhere in the main prompt.

**Position in prompt:** After `env_info_simple` (`# Environment`), before `language`.

---

## Confirmed Divergences (sections present but wrong)

### DIV-1 (P1) — Main `# Environment` block missing worktree notice

**Binary function:** `nIm(modelId, excludeDynamic, additionalDirs)` (the `env_info_simple` / `Kym` assembler)

**Binary element array** (relevant section):
```javascript
u = [
  `Primary working directory: ${l}`,
  c ? "This is a git worktree — an isolated copy of the repository. Run all commands from this directory. Do NOT `cd` to the original repository root." : null,
  `Is a git repository: ${r}`,
  ...
]
```
Where `c = hf()!==null` — non-null when running in a git worktree context.

**Binary offset for worktree notice text:** 206671709

**LingXi `env_block.rs`:**
```rust
write!(&mut s, "\n - Primary working directory: {}", ctx.cwd.display()).unwrap();
let is_git = ctx.git_status.is_some();
write!(&mut s, "\n - Is a git repository: {is_git}").unwrap();
```
The worktree notice between `Primary working directory:` and `Is a git repository:` is **not implemented** in `env_block.rs`. The `SystemPromptContext` struct has no `in_worktree` field; the format function has no conditional for the notice.

**Impact:** When the user runs LingXi from inside a git worktree, the main prompt does not inform the model of the worktree context (the subagent path via `subagent_env.rs` does implement this correctly).

**File:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/orchestrator/src/prompt/env_block.rs` — missing between lines 71–76

---

### DIV-2 (P1) — `# Using your tools` dedicated-tool list includes Glob/Grep when it should not

**Binary function:** `KHm(toolSet)`, the `# Using your tools` assembler

**Binary logic** (offset 206660705):
```javascript
let n = rv();        // rv() = true for posix non-Windows-shell-mode (hardcoded via ot("true"))
let r = e.has(Lo);  // Lo = "Bash" — true when Bash tool present
let s = [Rs, ma, Ec, ...(n && r ? [] : [ou, Ac])].join(", ");
// Rs="Read", ma="Edit", Ec="Write", ou="Glob", Ac="Grep"
// When rv()=true AND has_bash=true (the standard posix + Bash case):
//   n && r = true → spread [] (empty) → s = "Read, Edit, Write"
// When NOT posix OR no Bash:
//   n && r = false → spread [Glob, Grep] → s = "Read, Edit, Write, Glob, Grep"
```

**Binary produces** (standard interactive CLI with Bash present):  
`" - Prefer dedicated tools over Bash when one fits (Read, Edit, Write) — reserve Bash for shell-only operations."`

**LingXi `body_sections.rs` line 146:**
```rust
let dedicated = "Read, Edit, Write, Glob, Grep";
```
The comment on lines 129–130 says "LingXi is posix-default, so the Glob, Grep pair is always included." This is **incorrect** — the binary logic explicitly EXCLUDES Glob and Grep from the dedicated list when on posix AND Bash is present (the standard case), because on posix+bash the user can use Bash for glob/grep operations.

**LingXi produces:**  
`" - Prefer dedicated tools over Bash when one fits (Read, Edit, Write, Glob, Grep) — reserve Bash for shell-only operations."`

**File:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code/orchestrator/src/prompt/body_sections.rs` — line 146 (the `dedicated` constant) and the comment at lines 129–130

---

## Verified Byte-Correct Sections

All verified by binary extraction at offsets in the 206M range (the main J0 `cx()` path):

| Section | LingXi Location | Binary Offset | Status |
|---------|----------------|---------------|--------|
| `HEADER` "You are Claude Code…" | `locked_templates.rs:9` | 196484522 | ✓ |
| Opening paragraph `qHm` (no-style path) | `body_sections.rs:110–122` | 206651649 | ✓ |
| Defensive-security `ZHO` text | `body_sections.rs:41` | 206642674 | ✓ |
| `NEVER generate or guess URLs` | `body_sections.rs:120` | 206651949 | ✓ |
| `# System` section (all 6 bullets) | `body_sections.rs:46–54` | 206652167 | ✓ |
| `# Doing tasks` section (all bullets, order) | `body_sections.rs:60–76` | 206653490 | ✓ |
| `# Executing actions with care` (default/non-compact) | `body_sections.rs:82–92` | 206657510 | ✓ |
| `# Tone and style` (4 bullets) | `body_sections.rs:95–101` | 206666908 | ✓ |
| `# Environment` block structure | `env_block.rs:61–141` | 206671400 | ✓ (see DIV-1) |
| `Notes:` subagent trailer (5 bullets) | `agent/src/handle.rs:564–569` | 206675179 | ✓ |
| `FOOTER` constant | `locked_templates.rs:31–36` | same text | ✓ (same as Notes trailer) |
| Subagent `<env>` block `tIm` | `subagent_env.rs:117–174` | 206671100 | ✓ |

---

## Uncertain / Out-of-Scope Items

### U-1 — Flag-gated dynamic sections (not audited)

The following dynamic sections fire only behind feature flags (GrowthBook / env vars), all defaulting off, and were NOT systematically audited. They are listed in `body_sections.rs` comment line 24–28 as deferred follow-ups:

- `action_caution` (`PHm`) — fires only for `:L` longtail models (`xh(e)=true`)
- `task_continuity` (`OHm`) — fires only when conditions of `Mii(e)` and `!GOo()` met
- `fable_identity` (`LHm`) — fires only for Fable 5 / Mythos 5
- `tool_param_json` (`MHm`) — fires only when `nSi()` or specific combos
- `investigate_first` (`dIm`) — fires only for claude-opus-4-7 with env var set
- `scratchpad` (`u4n`) — fires when scratchpad setting is on
- `brief` (`aIm`) — fires only when `WOo.isBriefEnabled()=true`
- `focus_mode` (`uIm`) — fires only when focus mode is enabled
- `reproduce_verify_workflow` (`QHm`) — fires only when `XHm()=true`
- `act_dont_rederive` (`eIm`) — fires only when `ZHm()=true`
- `heron_brook` (`FHm`) — fires only when heron_brook client data / flag set
- `autonomy_append` (`BHm`) — fires only with specific amber_sextant flag and model conditions
- `language` (`UHm`) — fires only when user language is set (non-null)
- `bg-session` (`sIm`) — fires only in background session mode

These are all correctly listed as unimplemented in LingXi's `body_sections.rs` comment. Each fires rarely in practice. The three critical always-on sections (GAP-1, GAP-2, GAP-3) are more impactful.

---

## Fix Specifications

### Fix GAP-1: Add `# Text output` section (anti_verbosity DHm)

The simplest approach: add a new constant `TEXT_OUTPUT_SECTION` to `body_sections.rs` and emit it as the first dynamic section after `# Tone and style` in the assembler. The content depends on the model:

- **Standard models (Sonnet, Haiku, Opus 4.x without `:L`):** the full section shown in GAP-1 above
- **Fable 5 / Mythos 5:** different `# Communicating with the user` section (binary offset 206643232)
- **`:L` longtail:** single line "Write code that reads like the surrounding code..." (no heading)

For the standard case (MVP), add as a static constant in `body_sections.rs` and emit it unconditionally. The assembler `mod.rs` / `assemble_system_prompt_with_style` should inject it as the first dynamic section (after the 6 static body sections, before the env block — matching the cx() ordering: statics → dynamic sections → env).

### Fix GAP-2: Add `# Context management` section

Add to `body_sections.rs`:
```rust
const CONTEXT_MANAGEMENT_SECTION: &str = "# Context management\n\
When the conversation grows long, some or all of the current context is summarized; the summary, along with any remaining unsummarized context, is provided in the next context window so work can continue \u{2014} you don't need to wrap up early or hand off mid-task.";
```
Emit unconditionally in `assemble_system_prompt_with_style`, in the correct position (after anti_verbosity, action_caution, task_continuity, fable_identity, tool_param_json, investigate_first, session_guidance, memory, env_info_simple, language, output_style, bg-session, scratchpad — i.e., after env block in the current LingXi structure).

### Fix GAP-3: Add `# Session-specific guidance` section (optional bullets)

Add `jHm`-equivalent in `body_sections.rs` or as a new dynamic section builder. For the standard interactive + has-Agent case, the two bullets are:
1. `"If you need the user to run a shell command themselves (e.g., an interactive login like \`gcloud auth login\`), suggest they type \`! <command>\` in the prompt — the \`!\` prefix runs the command in this session so its output lands directly in the conversation."`
2. `"Use the Agent tool with specialized agents when the task at hand matches the agent's description. Subagents are valuable for parallelizing independent queries or for protecting the main context window from excessive results, but they should not be used excessively when not needed. Importantly, avoid duplicating work that subagents are already doing - if you delegate research to a subagent, do not also perform the same searches yourself."`

The section header is `# Session-specific guidance`.

### Fix DIV-1: Add worktree notice to main env_block.rs

Add `in_worktree: bool` to `SystemPromptContext`. Populate it at the call site (check if hf()/worktree is active). In `env_block::format`, after writing the Primary working directory line:
```rust
if ctx.in_worktree {
    s.push_str("\n - This is a git worktree \u{2014} an isolated copy of the repository. Run all commands from this directory. Do NOT `cd` to the original repository root.");
}
```

### Fix DIV-2: Fix dedicated tool list in `# Using your tools`

In `body_sections.rs` `using_your_tools()` function, replace the hardcoded `"Read, Edit, Write, Glob, Grep"` with the conditional logic:
```rust
// rv()=true for posix non-search-tools-opt-in (default), r=has_bash
// Binary: when rv()=true AND has_bash=true -> exclude Glob/Grep from dedicated list
let include_glob_grep = !has("Bash"); // simplification: exclude Glob/Grep when Bash present (posix default)
let dedicated = if include_glob_grep {
    "Read, Edit, Write, Glob, Grep"
} else {
    "Read, Edit, Write"
};
```
This matches the binary's `n&&r?[]:[ou,Ac]` logic where `n=rv()` (always true for posix) and `r=has_bash`.

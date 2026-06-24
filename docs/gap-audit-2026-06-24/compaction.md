# Compaction Parity Audit — LingXi vs claude-code v2.1.186 Oracle Binary
<!-- Date: 2026-06-24 · Binary: /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude -->

**Gap count: P0=0, P1=3, P2=1, UNCERTAIN=0**

---

## Confirmed Gaps

| # | Area | Item | Oracle (binary evidence) | LingXi (file:line) | Severity | Note |
|---|------|------|--------------------------|---------------------|----------|------|
| 1 | Prompt | `getPartialCompactPrompt` / `partialCompactConversation` missing | Binary at offsets 198338631 (PARTIAL_COMPACT_PROMPT, `from` direction) and 198327907 (PARTIAL_COMPACT_UP_TO_PROMPT, `up_to` direction) — both prompts fully expanded with security-relevant analysis bullet | `compaction/src/prompt.rs` declares only `BASE_COMPACT_PROMPT` / `get_compact_prompt`. `partial.rs` has the tail-selection algorithm but `get_partial_compact_prompt` is NOT exported. `partialCompactConversation` has no Rust equivalent. | P1 | Affects the `/compact` command when the user clicks on a specific message to compact `from` or `up_to`. The partial direction prompts send a different LLM request. Noted as deferral at `prompt.rs:20-22`. |
| 2 | Prompt | `replVmCleared` parameter missing from `get_compact_user_summary_message` | Binary `UOt(e,t,n,r,o)` at offset 198337715: param `o` appends `"\n\nYour REPL VM state has been cleared as part of this compaction. Variables defined in REPL calls before this point are no longer accessible — redefine any you still need."` between the `recentMessagesPreserved` sentence and the continuation sentence. The `tengu_repl` GrowthBook flag (offset 202229959 etc.) controls the REPL VM feature. | `compaction/src/prompt.rs:287-319` — `get_compact_user_summary_message` takes `(summary, suppress_follow_up, transcript_path, recent_messages_preserved)` — no `repl_vm_cleared` param. `prompt.rs:279` explicitly notes this as "intentional deferral". | P1 | When REPL VM is cleared during compaction, the model is not informed that its prior REPL state is gone. Only relevant when the REPL subsystem is active (`tengu_repl` flag). Sequence: base → transcript → recentPreserved → **replVmCleared** → continuation. |
| 3 | Prompt | `BASE_COMPACT_PROMPT` has 2 trailing-space divergences | Binary at offset 198331930: (a) example block header `"6. All user messages: \n"` (trailing space); (b) closing instruction `"...thoroughness in your response. \n"` (trailing space). Total prompt: 5426 chars. | `compaction/src/prompt.rs:50-146` — `"6. All user messages:\n"` (line 116, no trailing space); closing line without trailing space. Rust prompt is 5424 chars (2 fewer than binary). | P2 | Cosmetic trailing whitespace. The model sees a marginally different string but behaviour is unchanged. Rust prompt was written from the older TS source (`prompt.ts` also lacks the spaces — these were added in the bundled binary). |
| 4 | Microcompact | `CACHED_MICROCOMPACT` path is an empty stub | Binary has full `cachedMicrocompact.ts` machinery: `createCacheEditsBlock`, `getToolResultsToDelete`, pinned-edits state. `tengu_slate_heron` GrowthBook flag controls time-based MC; `CACHED_MICROCOMPACT` compile-time flag gates the cache-editing path. Binary offset 198311600 shows the full `$Hn`/`H7r`/`YMd` variables and both the time-based and cached MC paths. | `compaction/src/cached_microcompact.rs` — only `pub struct CachedMicrocompact;` (3 lines). `microcompact.rs` has the time-based trigger shape but not the cache-editing API path. | P1 | Cached MC (cache-editing) reduces token churn without prompt-cache misses. With `CACHED_MICROCOMPACT` off (default in external builds) this path is unreachable, so the gap only manifests for Anthropic-internal users or when the flag is enabled. |

---

## What Looked Clean (binary-verified)

**Compaction prompt (BASE_COMPACT_PROMPT):** LingXi matches the binary on all substantive content — the security-relevant bullet in the analysis section, section 6's "Preserve any security-relevant…" sentence, all 9 numbered sections with correct text, the `<example>` block structure, and the Compact Instructions examples. The TS source (`prompt.ts:31-44`) is older and lacks these security additions; the binary (and LingXi) are up-to-date.

**Preamble / Trailer:** `NO_TOOLS_PREAMBLE` and `NO_TOOLS_TRAILER` are byte-identical at runtime. The binary stores `—` (escaped em dash) vs LingXi's literal `—` UTF-8 bytes — both produce U+2014 when the model receives the string.

**`get_compact_user_summary_message` (params e/t/n/r):** Binary `UOt` confirms the base message, transcript-path insertion, `recentMessagesPreserved` sentence, and the `suppressFollowUpQuestions` continuation sentence all match LingXi byte-for-byte (runtime).

**All threshold constants:**
- `AUTOCOMPACT_BUFFER_TOKENS` = 13,000 ✓
- `WARNING_THRESHOLD_BUFFER_TOKENS` = `ERROR_THRESHOLD_BUFFER_TOKENS` = 20,000 each ✓
- `MANUAL_COMPACT_BUFFER_TOKENS` = 3,000 ✓
- `MAX_OUTPUT_TOKENS_FOR_SUMMARY` = 20,000 ✓
- `MICROCOMPACT_MIN_TOKENS_SAVED` (`H7r`) = 20,000 ✓ (binary offset 198311600)
- Rapid-refill constants (`jho`/`Who`/`f6n`) = 3/3/3 ✓
- `POST_COMPACT_TOKEN_BUDGET` (`Y9p`) = 50,000 ✓
- `POST_COMPACT_MAX_FILES_TO_RESTORE` (`Dqn`) = 5 ✓
- `POST_COMPACT_MAX_TOKENS_PER_FILE` (`J9p`) = 5,000 ✓
- `POST_COMPACT_MAX_TOKENS_PER_SKILL` (`X9p`) = 5,000 ✓
- `POST_COMPACT_SKILLS_TOKEN_BUDGET` (`Q9p`) = 25,000 ✓

**Sentinel strings:** `BOUNDARY_CONTENT` = `"Conversation compacted"` ✓; `TIME_BASED_MC_CLEARED_MESSAGE` = `"[Old tool result content cleared]"` ✓; `SKILL_TRUNCATION_MARKER` = `"\n\n[... skill content truncated for compaction; use Read on the skill path if you need the full text]"` ✓; `RAPID_REFILL_THRASHING_MESSAGE` = byte-exact with `3`s resolved ✓.

**Token warning banner:** `"N% until auto-compact"` (dim) / `"Context low (N% remaining) · Run /compact to compact & continue"` (error or warning per `isAboveErrorThreshold`) — text and color split both match binary and TS source.

**`TimeBasedMCConfig` defaults:** `enabled=false`, `gapThresholdMinutes=60`, `keepRecent=5` — matches TS `TIME_BASED_MC_CONFIG_DEFAULTS` and the `tengu_slate_heron` GrowthBook key ✓.

**Compact boundary:** `"Conversation compacted"` sentinel, `CompactTrigger` strings `"manual"/"auto"`, `CompactBoundaryMetadata` serde shape ✓.

**Post-compact restoration logic:** Sort-desc, top-5 files, per-file 5k cap, greedy 50k total budget, DROP-not-truncate on overflow — all mirror binary `Pqn`/`Lqn` functions ✓.

**format_compact_summary pipeline:** `<analysis>` stripping, `<summary>` → `Summary:\n{trimmed}` rewrite, `\n\n+` collapse, final trim — byte-faithful via hand-rolled scan matching the JS regex first-match non-greedy semantics ✓.

**Proactive branch:** `feature('PROACTIVE') || feature('KAIROS')` in `getCompactUserSummaryMessage` is DCE'd — the "autonomous/proactive mode" text is NOT in the binary (`grep` returns 0 hits). LingXi correctly omits it.

---

## Uncertain / Not Verified

None. All items above were confirmed via binary extraction (`grep -aboF` + `tail -c +OFFSET`).

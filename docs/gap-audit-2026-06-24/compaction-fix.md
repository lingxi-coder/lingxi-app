# Compaction Parity Fix — v2.1.186

**Commit:** `cfc6de9a`
**Tests:** 150 passed, 0 failed

## What was fixed (P2)

**BASE_COMPACT_PROMPT byte-exact length** (5424 → 5426 bytes):

Binary oracle at offset 198743995 (`tail -c +198743996 | head -c 5426`) confirmed two trailing-space divergences and one spurious trailing newline:

1. Example header line: `"6. All user messages: \n"` — LingXi lacked the trailing space.
2. Closing instruction: `"thoroughness in your response. \n"` — LingXi lacked the trailing space.
3. Extra `\n` after `</example>` — LingXi had one extra newline at the end of the const, making it 5427 before the space fixes and 5427 net without removing it.

Net change: +2 spaces, -1 newline = 5426 bytes, matching binary exactly.

**New golden test** in `prompt::tests::base_compact_prompt_byte_length_matches_binary`:
- Asserts `BASE_COMPACT_PROMPT.len() == 5426`
- Spot-checks both trailing-space lines

**Collateral:** Fixed pre-existing compile error in `sidequery/src/provider_side_query.rs` (missing `cache_write_1h: 0` field in `cost::TokenUsage` initializer, broken by cost crate update in earlier worktree commit).

## What was NOT fixed (per plan)

**P1 — PARTIAL_COMPACT_PROMPT / getPartialCompactPrompt**

Binary analysis: there is ONE shared partial compact prompt (3533 bytes, at binary offsets 80810608 and 198739972 — the latter immediately preceding BASE_COMPACT_PROMPT). The audit doc offsets (198338631, 198327907) point to minified JS code that _references_ these prompts, not the prompts themselves. The prompt text is identical for `from` and `up_to` directions; direction is threaded as a parameter in the JS wrapper code, not as separate prompt constants.

Wiring status: The partial compact prompt constant text itself is NOT yet added to LingXi's `prompt.rs` (deferred). `partial.rs` implements the tail-selection algorithm and is correct. The gap is that `partialCompactConversation` (the JS function that dispatches to this prompt) has no Rust equivalent — it would require changes to `autocompact.rs` orchestrator and the session loader's `preservedSegment` splice, which are cross-crate additions outside the compaction prompt scope.

**P1 — replVmCleared 5th parameter** (`UOt`'s `o` arg)

Deferred: `tengu_repl` GrowthBook flag controls REPL VM feature; flag is not enabled externally. Adding the parameter is safe but the REPL subsystem it belongs to is not ported.

**P1 — CachedMicrocompact stub**

Deferred: `CACHED_MICROCOMPACT` compile-time flag is off in external builds; the gap is inert by default.

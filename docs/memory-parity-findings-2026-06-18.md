# Memory / CLAUDE.md parity findings & corrections — 2026-06-18

**Reference:** claude-code **v2.1.181** (installed binary at
`/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe`, a
bun-compiled binary that embeds greppable JS source). The byte-alignment target
commit is `6a25909`, but no 6a25909 checkout is available on this machine, so
version-sensitive details below are confirmed against v2.1.181 (newer than
6a25909) and flagged where the two may differ.

**Origin:** a Codex review listed 13 memory/CLAUDE.md divergences. This document
records which were verified TRUE, which were **refuted** against v2.1.181, the
outcome of the `memory-parity-fix` workflow (run `wf_6187c0c7-e00`, 20 agents),
and the two fixes applied afterward. Purpose: **stop refuted items from being
re-attempted** by another session.

---

## 1. Per-claim verdicts

| # | Claim (Codex) | Verdict vs v2.1.181 | Notes |
|---|---|---|---|
| 1 | mobile main flow loads empty memory (`StaticMemoryProvider::empty()`) | ✅ **TRUE** | `engine-mobile/src/host.rs` had no injection seam at all. **Fixed** (P0.2). |
| 2 | desktop engine library default empty | ✅ TRUE, low impact | CLI (`init.rs:301`) + bridge (`boot.rs:220`) already inject `real_provider()`; only `cfg.memory_provider=None` embedders are empty. No change needed. |
| 3 | relevant-memory prefetch unwired (returns empty) | ✅ **TRUE — the real P0** | `prefetch.rs` was a stub; `find_relevant` had only test callers. **Fixed** (P0.1). |
| 4 | `find.rs` is deterministic Jaccard, "not Claude's sideQuery" | ⚠️ **MISCHARACTERIZED** | The sideQuery selector **already existed** (`memory/src/selector.rs` — LLM-driven, Haiku-class, JSON). The real gap was that it was *unwired*, not the wrong algorithm. `find.rs` is a separate memdir ranker. **Fixed** by wiring the selector (P0.1). |
| 5 | session memory fully stubbed | ✅ TRUE | `session_memory.rs` (memory + compaction) were 1-line stubs. P1 implemented but **version-skewed** — see §3. |
| 6 | hierarchy missing settings source-gate + nested-worktree `skipProject` | ❌ **REFUTED for v2.1.181** | `skipProject`/`skipProjectMemory` = **0 hits**; only `settingSources` (6 hits) — a settings-*loader* concept, not a `walk()` param. The `memory` crate models no setting sources. Adding a fabricated `skip_project` flag would *diverge*. |
| 7 | rules-dir order not byte-identical (LingXi sorts; TS uses raw readdir) | ✅ TRUE, deliberate | `hierarchy.rs:253` `sort_by_key(file_name)` is an intentional determinism deviation. `processMdRules` confirmed present. Low priority; only needed for strict byte-parity. |
| 8 | `@import` missing `TEXT_FILE_EXTENSIONS` allowlist | ❌ **REFUTED for v2.1.181** | `TEXT_FILE_EXTENSIONS` = **0 hits**. `@import` resolution already mirrors claude-code via `path_in_working_path` (`loader.rs:231`). An extension allowlist would be invented divergence. |
| 9 | external-include approval never allowed for Managed/Project | ✅ TRUE | `hasClaudeMdExternalIncludesApproved` exists (7 hits). `memory_block.rs:85` hardcodes `include_external = (tier==User)`. The seam is **parity-faithful by default**; plumbing the dynamic opt-in is a composition-root/provider-construction change (deferred, cross-track). |
| 10 | `getClaudeMds` eager block missing TeamMem/AutoMem | ❌ **REFUTED for v2.1.181** | `TeamMem`(70)/`AutoMem`(25) **exist**, but **not** as eager `ClaudeMdTier` tiers: `team-memory-content`/`source="shared"` wrapper = **0 hits**. They live in a *separate* `memory::tier::MemoryTier {Project,User,Session,Team}` subsystem. `claude_md/mod.rs` doc-comment already states TeamMem/AutoMem are deliberately out of `ClaudeMdTier` scope. |
| 11 | conditional/nested memory = per-turn merged reminder, not per-attachment | ✅ TRUE, nuanced | P3.2 upgraded to per-attachment injection + per-file `InstructionsLoaded` fire (bundled into the surfacing track). |
| 12 | `/memory` is not a tier selector UX | ✅ TRUE | claude-code labels confirmed: **User memory** (`~/.claude/CLAUDE.md`), **Project memory** (`./CLAUDE.md`), **Local memory** (`./CLAUDE.local.md`). **NOT implemented** — P3.1 agent died (§3). |
| 13 | `secret_scan` is isolated/dead | ✅ TRUE + worse | Zero non-test callers. Adapter **deleted**. The event it emits, `tengu_memory_secret_redacted`, is **FABRICATED** — 0 hits in v2.1.181; the real team-mem event is `tengu_team_mem_secret_skipped` (SKIP-on-detection at push/egress, **not** redact-on-load). Invented by the stale `docs/superpowers/plans/2026-05-23-m3-02-memory.md`. |

### Supplementary findings (not in the Codex list)
- **`CLAUDE_CONFIG_DIR` not honored in memory paths.** Only `migrations/src/global_config.rs` honors it; `memory_block.rs:54` (`dirs::home_dir()`) and `handle_impl.rs:114` (`dirs::config_dir()`) ignore it. So the **entire user-tier CLAUDE.md hierarchy reads the wrong dir** when the env var is set, not just `/memory`. **Open** (config-home consolidation deferred).
- **`find.rs` Team tier vs `ClaudeMdTier`.** `find.rs:64` has `MemoryEntryTier::Team` (+ team boost, 14 passing tests) while `ClaudeMdTier` has no Team — this is **by design** (two different subsystems), not a bug. Reconciliation already complete.
- **MemorySelector was doubly-dead** (both `find_relevant` and `selector.rs` unwired at runtime). The selector half is now wired via P0.1.

---

## 2. `memory-parity-fix` workflow outcome (per track)

Run `wf_6187c0c7-e00`. The Contracts phase re-verified every plan item against
the v2.1.181 binary and refuted the items marked ❌ above before any code was
written.

| Track | Item | Status | Detail |
|---|---|---|---|
| **surfacing** | P0.1 + P3.2 | ✅ **DONE / review PASS** | Wires `prefetch → MemorySelector → render_surfacing_block → relevant_memory_reminder_message` in both turn drivers. **Inert by default** (no composition root arms it ⇒ `None` ⇒ ~4000 locked fixtures byte-identical). 7 orchestrator tests + surfacing tests. **Committed in `3477b472`** (swept up with the user's bytealign commit). |
| **foundation** | P2.1/2.2/2.3/2.4/2.5 | ✅ correctly **NO-OP** | Every concrete item refuted by the binary (see #6/#8/#10). The one valid task — consolidate the 4–5 duplicate `claude_config_home_dir` copies into one canonical helper — was **NOT done**. |
| **session** | P1 | ⚠️ DONE / **concerns** | `SessionMemoryExtractor` (memory) + `SessionMemoryCompactor` (compaction), gated solely on `SessionMemoryConfig.enabled` (default false ⇒ inert). **Version-skew** — see §3. |
| **mobile** | P0.2 | ✅ DONE (+1 bug, now fixed) | Adds `cfg.memory_provider` seam + real `HookExecutorImpl` + `fire_session_start`/`fire_instructions_loaded`. Introduced a double-fire bug — **fixed**, §4. |
| **ux** | P3.1 | ❌ **FAILED** | Impl agent died mid-response; selector never implemented. Left a dead `open_memory_editor_at` trait stub — **removed**, §4. |
| **cleanup** | P4 | ⚠️ partial | Deleted dead `memory/src/secret_scan.rs` (correct). The fabricated `tengu_memory_secret_redacted` event remains locked in `telemetry/src/tengu/memory.rs` + 3 test-harness fixtures (removal requires the telemetry crate's append-only registry, out of the track's ownership). |

**Process note:** the workflow ran in the live checkout **while ≥3 concurrent
`claude --dangerously-skip-permissions` sessions** were editing the same tree (a
large sandbox/agent parity refactor). The Verify agent observed that in-flight
work, misattributed it to "the implement phase," and patched its stragglers
(test initializers, `nix::getuid`, sandbox test callsites, `apps/cli/init.rs`).
**The sandbox/agent churn is NOT part of the memory work** — do not attribute it
to these tracks. Lesson: run mutating workflows in an isolated worktree, never
the shared checkout, when other sessions are active.

---

## 3. Open follow-ups (deferred)

1. **P3.1 `/memory` tier selector** — not implemented. Needs a `MemoryFileSelector`
   (User/Project/Local) in `commands/core/src/memory.rs` + a tier-aware path open
   in `handle_impl.rs`, routed through a config-home helper. Confirmed labels in §1 #12.
2. **P1 session-memory version-skew** — the implemented `<session_memory>…</session_memory>`
   extraction tag + `<configHome>/agents/session-memory/<id>.md` path are **spec-derived,
   0-hit in v2.1.181** (which uses a `current_session_memory` attachment + a `memory_recall`
   supervisor with select/synthesize modes). Inert by default, so nothing breaks — but
   **confirm the tag + path against 6a25909 before any composition root wires it on**, or
   extraction silently no-ops / mis-places the file. Numeric `initialization_threshold` /
   `update_threshold` defaults are unknown upstream; tests assert against the config field,
   never a literal.
3. **`secret_scan` fabricated-event scrub** — remove `tengu_memory_secret_redacted` atomically
   across `telemetry/src/tengu/memory.rs` (const + NAMES array + `SecretRedactedPayload` +
   module doc count), `telemetry/tests/memory_schema_test.rs`,
   `telemetry/tests/event_name_completeness_test.rs` (352→351 + the entire downstream
   category-offset cascade), and the 3 fixtures (`tengu_events.json:242`,
   `full_v0_4_0_smoke.json:26`, `parity_full_v0_4_0_smoke.rs:159`). Also annotate the stale
   plan `docs/superpowers/plans/2026-05-23-m3-02-memory.md:72-74`. NOTE this conflicts with
   the append-only registry invariant (spec §7) — a judgement call for the telemetry-owning track.
4. **Config-home consolidation** — collapse the 4–5 copies of `$CLAUDE_CONFIG_DIR ?? $HOME/.claude`
   logic (two signatures: `migrations::global_config::claude_config_home -> Option<PathBuf>`
   without USERPROFILE fallback, vs `tools/file/read.rs` + `tools/task/todo_store.rs` +
   `memory/src/session_memory.rs` `-> PathBuf` with USERPROFILE) into ONE canonical helper in
   a low-level crate. Then fix the `dirs::config_dir()` user-tier bug at `handle_impl.rs:114`
   and the `dirs::home_dir()` user-tier resolution at `memory_block.rs:54`.
5. **External-include dynamic approval (#9)** — thread `hasClaudeMdExternalIncludesApproved`
   from the composition root through `RealMemoryHierarchyProvider::load` to allow approved
   Managed/Project external `@import`s. Currently inert + parity-faithful by default.

---

## 4. Fixes applied 2026-06-18 (verified)

1. **Mobile double hook-fire** — `apps/engine-mobile/src/host.rs`. On mobile
   `claude_home == <cwd>/.claude`, so the user- and project-settings paths
   collided on one `settings.json`; the loop read it twice, registering (and
   firing) every hook twice. Now de-dups: read each distinct path once; on
   collision keep the Project tag (preserves desktop's "project read last wins").
   Strengthened the regression test `build_mobile_fires_session_start_against_a_registered_hook`
   from `.any()` to `count == 1`. **Verified: `cargo test -p engine-mobile --features uniffi --lib` → 19/19 pass.**
2. **Dead `open_memory_editor_at` stub** — `platform-api/src/orchestrator.rs`. The
   failed ux track left an orphan trait method (overridden nowhere, called
   nowhere — grep-confirmed) + a vacuous delegation test + a misleading doc
   comment. Removed all three. **Verified: `cargo build -p platform-api` clean.**

Both fixes touched **only** those two files. `cargo test --workspace` was **not**
run (it would race the concurrent sessions and the tree was actively churning).

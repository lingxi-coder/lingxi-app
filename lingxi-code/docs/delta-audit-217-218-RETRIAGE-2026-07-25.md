# delta-audit 217→218 — re-triage, 2026-07-25

Re-verification of the 11 items `delta-audit-217-218-2026-07-23.json` lists as
`open`, against `main` @ `72b281387`. Each was checked at its **behaviour
site**, not by symbol name — see the methodology warning at the bottom, which
cost three false results on the first pass.

Net: **4 already done, 1 needs no change, 6 genuinely open** — and of those 6,
three are one architectural cluster and one is blocked on a prerequisite.

## Closed since the audit (4)

| id | evidence |
|---|---|
| `HOOKS-ORIGIN-TRUST` (HIGH, security) | `agent/src/hooks_trust.rs` exists AND is wired at the subagent call site — `agent/src/runner.rs:150` `agent_hooks_origin_trusted(...)` with `report_untrusted_hooks(..., HooksTrustSurface::Subagent)`. |
| `HOOK-TRUST-AGENT-FRONTMATTER-ORIGIN` (HIGH, security) | Same gate wired on the main thread — `apps/engine-desktop/src/lib.rs:8144`, `HooksTrustSurface::MainThread`. Both surfaces the audit named are covered. |
| `tengu_repair_double_escaped_unicode` (MEDIUM) | `llm-client/src/unicode_repair.rs` — its own header names the port as claude-code `jYd` / `L6s` (2.1.218), "successor to 2.1.217's `sOo`", i.e. exactly this item. It rewrites argument values, not telemetry. |
| `canonicalModel` (LOW) | `apps/cli/src/stream_json.rs:874-908` emits `"canonicalModel"` with a `(cc 2.1.218)` provenance comment, plus a lock test at `:1504`. |

## No change required (1)

`SWEEP-CRITICAL-PERMISSION-SANDBOX-LITERALS` — the audit itself concluded the
port sites are already aligned.

## Genuinely open (6)

### The forked-skill cluster — one gap, not three

`frozenCommandDenies` (MEDIUM, security, XL), `forkedSkill` (MEDIUM, security,
L), `forkedSkillName` (MEDIUM, L).

All three depend on a subsystem that does not exist. `tools/skill/src/skill.rs`
says so in its own header: the forked-agent execution path
(`executeForkedSkill` → `runAgent`, `prepareForkedCommandContext`, progress
streaming, `createAgentId`) has **no Rust substrate**; the tool does inline
resolution and metadata surfacing only.

So this is a documented architectural gap, not three oversights. It should be
scheduled as one piece of work — and `frozenCommandDenies` in particular is a
permission-scoping mechanism, so it must land WITH the fork path, never after
it: a fork that executes before its deny-freeze exists would run with the
parent's live rules instead of the frozen set.

### Blocked on a prerequisite

`trust_root` (LOW, S) — an additive field on `set_cwd`'s `needs_trust`
response. But `set_cwd` does not exist as a control request at all (repo-wide,
the only `set_cwd` hit is an unrelated `set_current_dir` in a TUI test). The
audit flagged this as "subordinate to a pre-217 gap" and that holds: the field
cannot be added to a response the port never sends. Do the `set_cwd` trust flow
first.

### Independently actionable (2)

- `tengu_left_arrow_editing_guard` (LOW, M) — `tui/src/bottom_pane/mod.rs:1311`
  handles `KeyCode::Left` as an unconditional `composer.move_left()`. The
  oracle's gesture state machine (`idp()`/`sdp()` over
  `{editedEmptyAtMs, armedAtMs, lastLeftPressMs, …}`, with
  reject/absorb/arm decisions) is absent. The flag defaults TRUE with no
  firstParty check, so this path IS reachable in an external build.
- `tengu_refusal_fallback_notice_collapsed` (LOW, L) —
  `orchestrator/src/conversation.rs:3943` `maybe_swap_to_refusal_fallback` has
  the once-per-session latch but no banner-collapsing queue: zero hits for
  `retractedMessageUuids` / `suppressedCount` / `emittedVia` repo-wide. Only
  reachable when a refusal→fallback episode actually occurs.

## Methodology warning for the next pass

Two traps hit during this re-triage, both of which produced **false "absent"**
results:

1. **`grep -E` with `\|`.** In extended regex, alternation is `|`; `\|` matches
   a LITERAL pipe. A batch probe using `-E 'a\|b'` reported 0 hits for three
   items that were all present. Use `|` with `-E`, or drop `-E`.
2. **Grepping the oracle's symbol name.** `tengu_repair_double_escaped_unicode`
   is ported as the module `unicode_repair`, and `canonicalModel` sat behind a
   `build_model_usage_block` helper. A name that does not appear proves
   nothing; check the behaviour site the audit cites.

Both are the same underlying error: **a negative search result is not
evidence.** Every "absent" claim in this document was confirmed by reading the
site, not by a failed grep.

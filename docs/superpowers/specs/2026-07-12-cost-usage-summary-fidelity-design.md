# Design: `/usage` cost-summary fidelity (byte-exact `i6e` port)

**Date:** 2026-07-12
**Status:** Approved (sections 1–3), pending spec review
**Scope:** gap #2 of two (this spec). gap #1 (rate-limit `/usage-credits` migration) is a
separate, independent spec — NOT covered here.
**Parity target:** Claude Code 2.1.206 (`/Users/luolingfeng/.local/share/claude/versions/2.1.206`)
**Prior audit:** `COST_USAGE_AUDIT_2026-07-12` in `.omx/state/parity-206-loop.json`

## Problem

The port's `/usage` (and its `/cost`, `/stats` aliases) renders a LingXi-adapted,
simplified cost view. CC 2.1.206's canonical cost summary (`i6e()`) shows rows the
port lacks:

- `Total duration (API):` — summed per-call API time
- `Total code changes:` — session lines added / removed
- `Usage by model:` — per-model token + cost breakdown

Plus the `" (costs may be inaccurate due to usage of unknown models)"` note when an
unpriced model was used (multi-provider-relevant).

`/cost` and `/stats` remaining aliases of `/usage` is **already correct and MUST be
kept** (CC defines `name:"usage", aliases:["cost","stats"]`; recorded as a keep-guard
in `lingxi-accepted-divergences`). This spec closes the **content** gap, not the alias.

## Goal

Produce a **byte-exact port of CC's `i6e()`** cost-summary renderer as the canonical
cost summary, route the port's human-readable `/usage` surfaces through it, and add the
supporting data tracking. Faithful to CC 2.1.206.

## The exact CC format (extracted from 2.1.206)

`i6e()` (whole block rendered dimmed via `ht.dim`):

```
Total cost:            <FTu(cost)>[ (costs may be inaccurate due to usage of unknown models)]
Total duration (API):  <qs(apiMs)>
Total duration (wall): <qs(wallMs)>
Total code changes:    <A> line[s] added, <B> line[s] removed
<cbg block>
```

- Labels are space-padded so every value starts at **column 23**
  (`"Total cost:"` + 12 spaces; `"Total duration (wall): "` = 23).
- Singular/plural: `A === 1 ? "line" : "lines"` (evaluated independently for added and
  removed).
- The unknown-model note is appended to the cost value only when an unpriced model was
  used.

`qs(ms)` — duration formatter:
- `ms < 60000`: `0 → "0s"`; `ms < 1 → "${(ms/1000).toFixed(1)}s"`; else
  `"${floor(ms/1000)}s"`.
- `ms >= 60000`: days/hours/minutes/seconds breakdown with carry rounding
  (`i===60 → o++`, `o===60 → n++`, `n===24 → r++`). `i6e` calls `qs(ms)` with no options
  → the full breakdown form (e.g. `"1h 5m 3s"`). **The exact ≥1-min assembly string is
  pinned against the binary during implementation.**

`cbg()` — Usage-by-model block:
- Empty (no per-model usage):
  `"Usage:                 0 input, 0 output, 0 cache read, 0 cache write"`
- Non-empty: header `"Usage by model:"`, then, per model (aggregated by normalized name
  `so()`):
  ```
  <model>:  <Bu(in)> input, <Bu(out)> output, <Bu(cr)> cache read, <Bu(cw)> cache write[, <Bu(web)> web search] (<FTu(cost)>)
  ```
  where `${model}:` is `padStart(21)` (right-aligned to 21). The `, N web search` clause
  is present only when `webSearchRequests > 0`.

## Data landscape (what already exists)

| CC needs | Port today | Work |
|---|---|---|
| Total cost (`QS`) | `total_nano_usd` | none |
| Total duration (wall) (`Dxe`) | `session_duration` | `qs()` formatting |
| Total duration (API) (`UL`) | per-call `elapsed` computed at `turn_loop.rs:692` / `conversation.rs:5977` (telemetry only) | NEW accumulator |
| Total code changes (`RFe`/`xFe`) | `Edit`/`Write`/`MultiEdit` emit `structuredPatch` hunks (`edit.rs:823`, `write.rs:413`) | NEW cumulative counter (sum +/- lines per edit) |
| Usage by model (`cbg`/`jP`) | `cost/src/summary.rs::ModelCostSummary` already projects per-model `total_nano_usd` + all token types | render/projection |
| unknown-model note (`Cqo`) | not tracked | NEW flag |
| token/cost formatters (`Bu`/`FTu`) | `format_tokens` (TS-compact), `${:.2}` cost | reuse, pin vs binary |

No component is blocked; all data foundations exist.

## Design

### Component 1 — `CostSnapshot` extension (`platform-api/src/orchestrator.rs`)

Add fields:
- `api_duration: std::time::Duration`
- `code_lines_added: u64`, `code_lines_removed: u64`
- `by_model: Vec<ModelCostSummary>` (reuse the existing per-model projection)
- `unknown_models: bool`

### Component 2 — API-duration accumulator (`cost` crate + orchestrator)

Add a cumulative API-duration counter to `CostState`/`CostTracker`. At the existing
per-call `elapsed` sites (`turn_loop.rs:692`, `conversation.rs:5977`), add the elapsed to
the counter. `snapshot_cost_real` projects it into `CostSnapshot.api_duration`.

### Component 3 — code-change line counting (CUMULATIVE per-edit)

**Pinned against the binary:** CC keeps running counters — `RFe()=Pt.totalLinesAdded`,
`xFe()=Pt.totalLinesRemoved` — incremented after each edit via
`Bhn(added,removed){Pt.totalLinesAdded+=added; Pt.totalLinesRemoved+=removed}` (reset to
0 at session start). So it is CUMULATIVE per-edit line counts from each edit's diff, NOT a
net FileHistory snapshot diff.

The port's `Edit`/`Write`/`MultiEdit` tools already emit a `structuredPatch` hunk array
(`edit.rs:823`, `write.rs:413`; `tools/file/src/structured_patch.rs::build_structured_patch`,
a 1:1 jsdiff port). Implementation: when the **orchestrator** processes a file-edit tool
result carrying `structuredPatch`, count the `+`/`-` lines across its hunks and add them
to session counters (added, removed). This needs no edit-tool changes — the orchestrator
already sees tool results, mirroring where CC calls `Bhn`. Counters live in the session
cost/stats state (Component 2's home) and project into `CostSnapshot.code_lines_added` /
`code_lines_removed`.

### Component 4 — unknown-model flag (`cost` crate)

At summary time, check each `per_model_usage` model against `cost/src/pricing.rs`'s
pricing table; if any model has no pricing entry, `unknown_models = true`. Drives the
`" (costs may be inaccurate…)"` note.

### Component 5 — canonical `cost_summary()` renderer (shared)

A shared renderer (in a `cost`-crate render/summary module, reachable by both the CLI
command and the TUI) producing the byte-exact `i6e()` block:
- `cost_summary(&CostSnapshot) -> String`
- `qs(ms)` duration formatter — shared util, byte-exact port
- `cbg`-equivalent per-model block builder
- Sub-formatters `FTu` (cost), `Bu` (token) — pinned byte-exact vs binary; `so` model-name
  normalization.
- Dimming (`ht.dim`) is applied by the consuming surface where supported (the string
  itself is the plain block).

### Component 6 — surface routing

- **`/usage` command** (`commands/core/src/usage.rs::render_usage_snapshot`) →
  produce `cost_summary()`. Replaces the adapted flat text.
- **TUI `screen_view.rs::usage`** → the tabbed interactive shell renders the
  `cost_summary()` block as its content; tab navigation/shell preserved.
- **print-mode (`-p`)** → UNCHANGED. It emits the structured stream-json `result` frame
  (already aligned); the human `i6e()` text is not part of print-mode.

## Out of scope / boundaries

- **print-mode `result` frame** — unchanged (already aligned).
- **Interactive session-end auto-print** of `i6e` (CC's `IIn()`-gated stdout print) —
  OPTIONAL; a one-line `cost_summary()` print can be added later if desired. Not in this
  spec's required scope.
- **claude.ai plan-tier usage bars** — carve-out, not ported.
- **per-model `webSearchRequests`** — the port doesn't track per-model web-search count
  (Tavily carve-out); the `, N web search` clause is naturally omitted when count is 0,
  matching CC's conditional. No extra work.
- **gap #1** (rate-limit `/usage-credits`) — separate spec.

## Testing

- **Byte-exact golden-string tests** for `cost_summary()`: the padded block, per-model
  lines (`padStart(21)`), singular vs plural (`1 line` / `N lines`, added & removed
  independently), the empty-usage case (`"Usage: 0 input, …"`), and the unknown-model
  note present/absent.
- **`qs()` unit tests**: `0s`, sub-second, seconds, and minutes/hours/days with carry
  rounding — asserted against CC's exact outputs.
- **Formatter tests**: `FTu` (cost) and `Bu` (token) pinned against the binary.
- **Data-tracking unit tests**: API-duration accumulator (sums per-call elapsed);
  code-change line diff (snapshots → added/removed counts); unknown-model flag.
- **Integration**: a mock session (2 models + edits) → `/usage` renders the full
  byte-exact block.

## Verification recipe (per this port's convention)

1. Oracle diff each ported string/format against 2.1.206 (`grep -abo` / python-latin1
   slice; handle `${…}` interpolation and source-vs-runtime escapes).
2. `cargo build` clean for touched crates (`cost`, `traits`, `orchestrator`, `session`,
   `commands/core`, `tui`).
3. `cargo test` for those crates (+ the new byte-exact tests).
4. Confirm the default/unregistered and print-mode paths stay byte-identical.

## Invariants

- Never touch the 4 user dirty files.
- Keep `/cost` and `/stats` as aliases of `/usage` (keep-guard).
- No git remote → commit to `main` locally; commit trailer
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.

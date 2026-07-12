# `/usage` cost-summary fidelity (byte-exact i6e port) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render CC 2.1.206's byte-exact `i6e()` cost summary in the port's `/usage` surfaces, adding `Total duration (API)`, `Total code changes`, `Usage by model`, and the unknown-model note.

**Architecture:** A new pure renderer in the `cost` crate (`cost_summary()`/`format_duration_ms()`/`usage_by_model_block()`/`format_cost()`) plus three new session counters (API-duration, code-lines added/removed, unknown-model flag) accumulated in `CostState` and projected through `CostSnapshot`. The `/usage` command and TUI screen route their text through `cost_summary()`; print-mode is unchanged.

**Tech Stack:** Rust; crates `cost`, `traits`, `orchestrator`, `commands/core` (crate `command-core`? confirm with `grep '^name' commands/core/Cargo.toml`), `tui`.

## Global Constraints

- Parity target: Claude Code **2.1.206** (`/Users/luolingfeng/.local/share/claude/versions/2.1.206`).
- **Never modify** these 4 user dirty files: `llm-client/data/models-dev/openrouter.json`, `llm-client/src/catalog/presets.rs`, `tools/agent/src/agent.rs`, `tools/agent/src/agent_test.rs`.
- Keep `/cost` and `/stats` as aliases of `/usage` (do NOT touch the alias registration).
- No git remote → commit to `main` locally. Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Byte-exact strings verified against the binary. The whole `i6e` block is rendered dimmed by the CONSUMER; the renderer returns the plain string.
- Never run `cargo fmt`. Run `cargo test -p <crate>` for touched crates after each task.

## Pinned CC formats (from the 2.1.206 binary)

- Label padding (value column = 23): `"Total cost:"`+12sp, `"Total duration (API):"`+2sp, `"Total duration (wall):"`+1sp, `"Total code changes:"`+4sp.
- Singular/plural: `n == 1 ? "line" : "lines"` (added and removed independently).
- `qs(ms)`: `<60000` → `0→"0s"`, else `"{ms/1000 floored}s"`; `>=60000` → d/h/m/s with carry-rounding, forms `"{r}d {n}h {o}m"` / `"{n}h {o}m {i}s"` / `"{o}m {i}s"` / `"{i}s"`.
- `FTu(usd,4)`: `usd > 0.5` → `"$" + round(usd*100)/100` at 2dp; else `"$" + usd` at 4dp.
- `Bu(count)`: Intl compact, lowercased (`1000→"1k"`, `1500→"1.5k"`, `12345→"12.3k"`, `1_000_000→"1m"`) — matches the port's existing `format_tokens`.
- `cbg()`: empty → `"Usage:                 0 input, 0 output, 0 cache read, 0 cache write"`; else header `"Usage by model:"` then per model `"{label:>21}  {in} input, {out} output, {cr} cache read, {cw} cache write ({cost})"` where `{label}` = `"{model}:"` right-padded to width 21. (web-search clause omitted — port tracks no per-model web-search.)
- `RFe/xFe/UL/Cqo` are cumulative counters (`Pt.totalLinesAdded/Removed/totalAPIDuration/hasUnknownModelCost`), incremented per-edit / per-call, reset to 0 at session start.

---

### Task 1: `format_duration_ms` (qs port)

**Files:**
- Create: `cost/src/render.rs`
- Modify: `cost/src/lib.rs` (add `pub mod render;`)
- Test: inline `#[cfg(test)]` in `cost/src/render.rs`

**Interfaces:**
- Produces: `pub fn format_duration_ms(ms: u64) -> String`

- [ ] **Step 1: Write the failing test**

In a new `cost/src/render.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qs_matches_cc() {
        assert_eq!(format_duration_ms(0), "0s");
        assert_eq!(format_duration_ms(500), "0s");     // <1s floors to 0
        assert_eq!(format_duration_ms(1_500), "1s");   // floor(1.5)
        assert_eq!(format_duration_ms(59_000), "59s");
        assert_eq!(format_duration_ms(60_000), "1m 0s");
        assert_eq!(format_duration_ms(65_000), "1m 5s");
        assert_eq!(format_duration_ms(3_661_000), "1h 1m 1s");
        assert_eq!(format_duration_ms(90_061_000), "1d 1h 1m"); // days form drops seconds
        assert_eq!(format_duration_ms(59_500), "59s");  // round(59.5)=60 -> carry to 1m 0s? see note
    }
}
```
Note: `59_500ms` → seconds `round(59.5)=60` → `i=0,o=1` → `"1m 0s"`. Fix that assertion to `"1m 0s"` before running (kept here to force thinking about carry).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cost render::tests::qs_matches_cc`
Expected: FAIL — `format_duration_ms` not found.

- [ ] **Step 3: Write minimal implementation**

At the top of `cost/src/render.rs`:
```rust
//! Byte-exact port of claude-code 2.1.206's cost-summary renderers
//! (`i6e`/`qs`/`cbg`/`FTu`/`Bu`). The whole block is rendered dimmed by the
//! consumer; these functions return plain strings.

/// Duration formatter — port of claude-code `qs(ms)` (no options). `ms` is an
/// integer, so the float sub-millisecond `.toFixed(1)` branch (`e < 1`) reduces
/// to the `e === 0` case already handled here.
#[must_use]
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 60_000 {
        if ms == 0 {
            return "0s".to_string();
        }
        return format!("{}s", ms / 1000);
    }
    let mut r = ms / 86_400_000;
    let mut n = (ms % 86_400_000) / 3_600_000;
    let mut o = (ms % 3_600_000) / 60_000;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut i = ((ms % 60_000) as f64 / 1000.0).round() as u64;
    if i == 60 {
        i = 0;
        o += 1;
    }
    if o == 60 {
        o = 0;
        n += 1;
    }
    if n == 24 {
        n = 0;
        r += 1;
    }
    if r > 0 {
        return format!("{r}d {n}h {o}m");
    }
    if n > 0 {
        return format!("{n}h {o}m {i}s");
    }
    if o > 0 {
        return format!("{o}m {i}s");
    }
    format!("{i}s")
}
```
And in `cost/src/lib.rs` add `pub mod render;` next to the other `pub mod` lines.

- [ ] **Step 4: Fix the `59_500` assertion to `"1m 0s"`, run to verify pass**

Run: `cargo test -p cost render::tests::qs_matches_cc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add cost/src/render.rs cost/src/lib.rs
git commit -m "feat(cost): format_duration_ms — byte-exact qs port (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `format_cost` (FTu port)

**Files:**
- Modify: `cost/src/render.rs`
- Test: inline in `cost/src/render.rs`

**Interfaces:**
- Produces: `pub fn format_cost(usd: f64) -> String`

- [ ] **Step 1: Write the failing test** (add to the `tests` mod):
```rust
#[test]
fn ftu_matches_cc() {
    assert_eq!(format_cost(0.0), "$0.0000");
    assert_eq!(format_cost(0.05), "$0.0500");
    assert_eq!(format_cost(0.5), "$0.5000");        // not > 0.5
    assert_eq!(format_cost(0.5001), "$0.50");       // > 0.5 -> 2dp rounded
    assert_eq!(format_cost(1.2345), "$1.23");
    assert_eq!(format_cost(12.999), "$13.00");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cost render::tests::ftu_matches_cc`
Expected: FAIL — `format_cost` not found.

- [ ] **Step 3: Implement**
```rust
/// Cost formatter — port of claude-code `FTu(usd, 4)`: `> $0.50` renders 2
/// decimals (rounded to cents via `dbg(e,100)=round(e*100)/100`); otherwise 4.
#[must_use]
pub fn format_cost(usd: f64) -> String {
    if usd > 0.5 {
        let cents = (usd * 100.0).round() / 100.0;
        format!("${cents:.2}")
    } else {
        format!("${usd:.4}")
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cost render::tests::ftu_matches_cc`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add cost/src/render.rs
git commit -m "feat(cost): format_cost — byte-exact FTu port (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `format_token_count` (Bu port)

**Files:**
- Modify: `cost/src/render.rs`
- Test: inline

**Interfaces:**
- Produces: `pub fn format_token_count(n: u64) -> String`

- [ ] **Step 1: Write the failing test** (golden values copied from the existing `commands/core/src/context.rs::format_tokens_matches_ts_compact_notation`):
```rust
#[test]
fn bu_matches_cc_compact() {
    assert_eq!(format_token_count(0), "0");
    assert_eq!(format_token_count(999), "999");
    assert_eq!(format_token_count(1_000), "1k");
    assert_eq!(format_token_count(1_500), "1.5k");
    assert_eq!(format_token_count(12_345), "12.3k");
    assert_eq!(format_token_count(50_000), "50k");
    assert_eq!(format_token_count(1_000_000), "1m");
    assert_eq!(format_token_count(1_500_000), "1.5m");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cost render::tests::bu_matches_cc_compact`
Expected: FAIL — not found.

- [ ] **Step 3: Implement** (a copy of the proven `format_tokens` logic — the port already has this exact algorithm in `commands/core/src/context.rs`; re-implement here so `cost` has no dep on `command-core`):
```rust
/// Token-count formatter — port of claude-code `Bu` (Intl compact notation,
/// lowercased, `maximumFractionDigits: 1`, trailing `.0` dropped). Identical
/// output to the port's existing `format_tokens`.
#[must_use]
pub fn format_token_count(n: u64) -> String {
    const UNITS: [(u64, char); 4] = [
        (1_000_000_000_000, 't'),
        (1_000_000_000, 'b'),
        (1_000_000, 'm'),
        (1_000, 'k'),
    ];
    for &(threshold, suffix) in &UNITS {
        if n >= threshold {
            #[allow(clippy::cast_precision_loss)]
            let rounded = ((n as f64 / threshold as f64) * 10.0).round() / 10.0;
            let s = format!("{rounded:.1}");
            let s = s.strip_suffix(".0").unwrap_or(&s);
            return format!("{s}{suffix}");
        }
    }
    n.to_string()
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cost render::tests::bu_matches_cc_compact`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add cost/src/render.rs
git commit -m "feat(cost): format_token_count — byte-exact Bu/compact port (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: `usage_by_model_block` (cbg port)

**Files:**
- Modify: `cost/src/render.rs`
- Test: inline

**Interfaces:**
- Consumes: `ModelCostSummary` from `cost::summary` (fields: `model_ref`, `total_nano_usd: u64`, `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`). Confirm exact field names with `grep -n "pub struct ModelCostSummary" -A8 cost/src/summary.rs` and adapt.
- Produces: `pub fn usage_by_model_block(by_model: &[ModelCostSummary]) -> String`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn cbg_empty_and_per_model() {
    // Empty → the aligned zero line.
    assert_eq!(
        usage_by_model_block(&[]),
        "Usage:                 0 input, 0 output, 0 cache read, 0 cache write"
    );
    let rows = vec![ModelCostSummary {
        model_ref: "claude-opus-4-8".into(),
        total_nano_usd: 1_230_000_000, // $1.23
        input_tokens: 5_000,
        output_tokens: 2_000,
        cache_read_input_tokens: 1_500,
        cache_creation_input_tokens: 0,
    }];
    let out = usage_by_model_block(&rows);
    assert!(out.starts_with("Usage by model:\n"));
    // label right-padded to 21, then the token/cost line.
    assert!(out.contains(
        "    claude-opus-4-8:  5k input, 2k output, 1.5k cache read, 0 cache write ($1.23)"
    ), "{out}");
}
```
(Adjust the expected `model_ref` label spacing so `"{model}:"` is right-aligned to width 21 — `"claude-opus-4-8:"` is 16 chars → 5 leading spaces. Verify by counting once implemented.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cost render::tests::cbg_empty_and_per_model`
Expected: FAIL — `usage_by_model_block` / `ModelCostSummary` import missing.

- [ ] **Step 3: Implement**
```rust
use crate::summary::ModelCostSummary;

/// `nano-USD → USD` (matches `cost/src/pricing.rs`'s `nano / 1e9`).
#[allow(clippy::cast_precision_loss)]
fn nano_to_usd(nano: u64) -> f64 {
    nano as f64 / 1_000_000_000.0
}

/// Model display name for the "Usage by model" label. Uses the model ref as-is
/// (the port's per-model key is already a display-usable ref); a catalog lookup
/// can refine this later without changing the format.
fn model_label(model_ref: &str) -> String {
    model_ref.to_string()
}

/// Usage-by-model block — port of claude-code `cbg()`. Empty usage renders the
/// aligned zero line; otherwise a header plus one right-aligned line per model.
/// The per-model web-search clause is omitted (no per-model web-search tracking).
#[must_use]
pub fn usage_by_model_block(by_model: &[ModelCostSummary]) -> String {
    if by_model.is_empty() {
        return "Usage:                 0 input, 0 output, 0 cache read, 0 cache write".to_string();
    }
    let mut r = "Usage by model:".to_string();
    for m in by_model {
        let label = format!("{}:", model_label(&m.model_ref));
        // right-align label to width 21 (claude-code `padStart(21)`).
        let padded = format!("{label:>21}");
        let line = format!(
            "  {} input, {} output, {} cache read, {} cache write ({})",
            format_token_count(m.input_tokens),
            format_token_count(m.output_tokens),
            format_token_count(m.cache_read_input_tokens),
            format_token_count(m.cache_creation_input_tokens),
            format_cost(nano_to_usd(m.total_nano_usd)),
        );
        r.push('\n');
        r.push_str(&padded);
        r.push_str(&line);
    }
    r
}
```

- [ ] **Step 4: Run, adjust the label-spacing assertion to the real output, verify pass**

Run: `cargo test -p cost render::tests::cbg_empty_and_per_model`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add cost/src/render.rs
git commit -m "feat(cost): usage_by_model_block — byte-exact cbg port (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: `cost_summary` (i6e port)

**Files:**
- Modify: `cost/src/render.rs`
- Test: inline

**Interfaces:**
- Consumes: a small input struct (defined here to avoid a `traits` dep from `cost` — the caller maps `CostSnapshot` → this):
  ```rust
  pub struct CostSummaryInput<'a> {
      pub total_usd: f64,
      pub unknown_models: bool,
      pub api_duration_ms: u64,
      pub wall_duration_ms: u64,
      pub code_lines_added: u64,
      pub code_lines_removed: u64,
      pub by_model: &'a [ModelCostSummary],
  }
  ```
- Produces: `pub fn cost_summary(input: &CostSummaryInput) -> String`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn i6e_full_block() {
    let out = cost_summary(&CostSummaryInput {
        total_usd: 1.2345,
        unknown_models: false,
        api_duration_ms: 65_000,
        wall_duration_ms: 3_661_000,
        code_lines_added: 1,
        code_lines_removed: 5,
        by_model: &[],
    });
    assert_eq!(
        out,
        "Total cost:            $1.23\n\
         Total duration (API):  1m 5s\n\
         Total duration (wall): 1h 1m 1s\n\
         Total code changes:    1 line added, 5 lines removed\n\
         Usage:                 0 input, 0 output, 0 cache read, 0 cache write"
    );
}

#[test]
fn i6e_unknown_models_note() {
    let out = cost_summary(&CostSummaryInput {
        total_usd: 0.05, unknown_models: true, api_duration_ms: 0, wall_duration_ms: 0,
        code_lines_added: 0, code_lines_removed: 0, by_model: &[],
    });
    assert!(out.starts_with(
        "Total cost:            $0.0500 (costs may be inaccurate due to usage of unknown models)\n"
    ), "{out}");
    // plural on zero: "0 lines added, 0 lines removed"
    assert!(out.contains("Total code changes:    0 lines added, 0 lines removed"), "{out}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cost render::tests::i6e`
Expected: FAIL — `cost_summary`/`CostSummaryInput` not found.

- [ ] **Step 3: Implement**
```rust
/// Inputs to [`cost_summary`], mapped by the caller from the session's cost
/// snapshot. Kept local so the `cost` crate needs no `traits` dependency.
pub struct CostSummaryInput<'a> {
    pub total_usd: f64,
    pub unknown_models: bool,
    pub api_duration_ms: u64,
    pub wall_duration_ms: u64,
    pub code_lines_added: u64,
    pub code_lines_removed: u64,
    pub by_model: &'a [ModelCostSummary],
}

/// Byte-exact port of claude-code `i6e()`. Returns the plain block; the consumer
/// applies dimming. Labels pad to column 23.
#[must_use]
pub fn cost_summary(input: &CostSummaryInput) -> String {
    let cost = if input.unknown_models {
        format!(
            "{} (costs may be inaccurate due to usage of unknown models)",
            format_cost(input.total_usd)
        )
    } else {
        format_cost(input.total_usd)
    };
    let api = format_duration_ms(input.api_duration_ms);
    let wall = format_duration_ms(input.wall_duration_ms);
    let added_unit = if input.code_lines_added == 1 { "line" } else { "lines" };
    let removed_unit = if input.code_lines_removed == 1 { "line" } else { "lines" };
    let by_model = usage_by_model_block(input.by_model);
    format!(
        "Total cost:            {cost}\n\
         Total duration (API):  {api}\n\
         Total duration (wall): {wall}\n\
         Total code changes:    {added} {added_unit} added, {removed} {removed_unit} removed\n\
         {by_model}",
        added = input.code_lines_added,
        removed = input.code_lines_removed,
    )
}
```
CAUTION: the `\` line-continuations in the `format!` string swallow the following line's leading whitespace, so the literal has NO indentation between `\n` and the next label — verify the test passes (it asserts the exact bytes).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cost render::tests::i6e`
Expected: PASS (both `i6e_full_block` and `i6e_unknown_models_note`).

- [ ] **Step 5: Commit**
```bash
git add cost/src/render.rs
git commit -m "feat(cost): cost_summary — byte-exact i6e block port (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: `CostState` session counters + `Bhn`/API-duration API

**Files:**
- Modify: `cost/src/tracker.rs` (the `CostState` struct + its impl)
- Test: inline in `cost/src/tracker.rs`

**Interfaces:**
- Produces (on `CostState`): fields `pub total_api_duration_ms: u64`, `pub total_lines_added: u64`, `pub total_lines_removed: u64`, `pub has_unknown_model_cost: bool`; methods `pub fn record_api_duration(&mut self, ms: u64)`, `pub fn record_code_change(&mut self, added: u64, removed: u64)`, `pub fn mark_unknown_model_cost(&mut self)`.

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn session_counters_accumulate() {
    let mut s = CostState::new(SessionId::from("s")); // match the real constructor
    s.record_api_duration(1000);
    s.record_api_duration(500);
    s.record_code_change(3, 1);
    s.record_code_change(0, 2);
    assert_eq!(s.total_api_duration_ms, 1500);
    assert_eq!(s.total_lines_added, 3);
    assert_eq!(s.total_lines_removed, 3);
    assert!(!s.has_unknown_model_cost);
    s.mark_unknown_model_cost();
    assert!(s.has_unknown_model_cost);
}
```
(Match `CostState::new` / construction to what `cost/src/tracker.rs` actually exposes — check with `grep -n "impl CostState" -A20 cost/src/tracker.rs`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cost tracker`
Expected: FAIL — fields/methods missing.

- [ ] **Step 3: Implement** — add the four fields to `CostState` (initialise to `0`/`false` wherever `CostState` is constructed, e.g. its `Default`/`new`), and:
```rust
impl CostState {
    /// Accumulate one API call's duration (claude-code `Pt.totalAPIDuration += e`).
    pub fn record_api_duration(&mut self, ms: u64) {
        self.total_api_duration_ms = self.total_api_duration_ms.saturating_add(ms);
    }
    /// Accumulate one edit's line changes (claude-code `Bhn(added, removed)`).
    pub fn record_code_change(&mut self, added: u64, removed: u64) {
        self.total_lines_added = self.total_lines_added.saturating_add(added);
        self.total_lines_removed = self.total_lines_removed.saturating_add(removed);
    }
    /// Flag that a model without pricing was used (claude-code `Pt.hasUnknownModelCost`).
    pub fn mark_unknown_model_cost(&mut self) {
        self.has_unknown_model_cost = true;
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cost tracker::` (plus `cargo test -p cost` to catch construction-site fallout)
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add cost/src/tracker.rs
git commit -m "feat(cost): session counters (api-duration, code-lines, unknown-model)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: count `structuredPatch` +/- lines (helper)

**Files:**
- Modify: `cost/src/render.rs` (a pure helper — `cost` already parses no JSON; take counts as input) — OR put it in `orchestrator` next to the tool-result path. Decision: put the pure counter in `orchestrator/src/cost_lines.rs` (new) since it consumes the tool-result JSON shape the orchestrator owns.
- Create: `orchestrator/src/cost_lines.rs`
- Modify: `orchestrator/src/lib.rs` (`mod cost_lines;`)
- Test: inline

**Interfaces:**
- Produces: `pub(crate) fn count_structured_patch_lines(structured_patch: &serde_json::Value) -> (u64, u64)` returning `(added, removed)`.

- [ ] **Step 1: Write the failing test**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_added_and_removed_across_hunks() {
        // structuredPatch hunks each carry a `lines` array; `+`/`-` prefixes.
        let patch = json!([
            { "lines": ["-old line", "+new line", " context", "+added2"] },
            { "lines": ["-gone"] }
        ]);
        assert_eq!(count_structured_patch_lines(&patch), (2, 2));
    }

    #[test]
    fn empty_or_non_array_is_zero() {
        assert_eq!(count_structured_patch_lines(&json!([])), (0, 0));
        assert_eq!(count_structured_patch_lines(&json!(null)), (0, 0));
    }
}
```
(Confirm the hunk `lines` field name against `tools/file/src/structured_patch.rs::StructuredPatchHunk` — `grep -n "pub struct StructuredPatchHunk" -A6 tools/file/src/structured_patch.rs` — and adapt the key if it differs.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p orchestrator cost_lines`
Expected: FAIL — module/function missing.

- [ ] **Step 3: Implement**
```rust
//! Count added/removed lines from a tool result's `structuredPatch` — the
//! per-edit input to the session's cumulative code-change counters
//! (claude-code `Bhn(added, removed)`).

/// Sum `+`/`-` lines across a `structuredPatch` hunk array. A `+`-prefixed line
/// is an addition, `-` a removal; context lines (` `) and anything else are
/// ignored. Non-array / absent input yields `(0, 0)`.
pub(crate) fn count_structured_patch_lines(structured_patch: &serde_json::Value) -> (u64, u64) {
    let Some(hunks) = structured_patch.as_array() else {
        return (0, 0);
    };
    let mut added = 0u64;
    let mut removed = 0u64;
    for hunk in hunks {
        let Some(lines) = hunk.get("lines").and_then(|l| l.as_array()) else {
            continue;
        };
        for line in lines {
            match line.as_str().and_then(|s| s.chars().next()) {
                Some('+') => added += 1,
                Some('-') => removed += 1,
                _ => {}
            }
        }
    }
    (added, removed)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator cost_lines`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add orchestrator/src/cost_lines.rs orchestrator/src/lib.rs
git commit -m "feat(orchestrator): count_structured_patch_lines helper

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: wire API-duration + code-change accumulation

**Files:**
- Modify: `orchestrator/src/turn_loop.rs:692` (per-call `elapsed`) and `orchestrator/src/conversation.rs:5977` (per-call `elapsed`) — call `record_api_duration`.
- Modify: the orchestrator's tool-result processing path (where file-edit tool results with `structuredPatch` are handled) — call `count_structured_patch_lines` + `record_code_change`.
- Test: `orchestrator/src/conversation.rs` test module (or `turn_loop_test.rs`).

**Interfaces:**
- Consumes: `CostState::record_api_duration`, `CostState::record_code_change`, `count_structured_patch_lines`.

- [ ] **Step 1: Locate the cost-state handle**

Run: `grep -n "cost_tracker\|CostTracker\|cost_state\|self.cost" orchestrator/src/conversation.rs | head`
Note the accessor used to reach the mutable `CostState`/`CostTracker` (e.g. `self.cost_tracker`).

- [ ] **Step 2: Write the failing test** (integration-style, in the conversation test module — adapt to the existing mock harness):
```rust
#[tokio::test]
async fn api_duration_and_code_changes_accumulate_into_snapshot() {
    // Build an orchestrator via the existing test_support harness, run a turn
    // whose assistant emits an Edit tool_use, and feed a tool result carrying a
    // structuredPatch of (+2, -1). Then:
    let snap = orch.snapshot_cost_real().await;
    assert!(snap.api_duration.as_millis() > 0);
    assert_eq!(snap.code_lines_added, 2);
    assert_eq!(snap.code_lines_removed, 1);
}
```
(If a full turn is heavy to drive, instead unit-test the two accumulation call-sites by asserting `CostState` after invoking the internal record methods the wiring calls — keep it to whatever the existing harness supports.)

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p orchestrator api_duration_and_code_changes`
Expected: FAIL (counts are 0).

- [ ] **Step 4: Implement the wiring**

At `turn_loop.rs:692` and `conversation.rs:5977`, right after `let elapsed = api_call_started.elapsed();`, add:
```rust
if let Some(tracker) = self.cost_tracker.as_ref() {
    tracker.lock().await.record_api_duration(elapsed.as_millis() as u64);
}
```
(Adapt the lock/accessor to the real `cost_tracker` type — `Arc<Mutex<CostState>>` or a `CostTracker` with an interior method. If `CostTracker` wraps the state, add a passthrough `record_api_duration`/`record_code_change` on `CostTracker`.)

In the tool-result path, when a file-edit tool result JSON has a `structuredPatch`:
```rust
if let Some(sp) = tool_result_json.get("structuredPatch") {
    let (added, removed) = crate::cost_lines::count_structured_patch_lines(sp);
    if added > 0 || removed > 0 {
        if let Some(tracker) = self.cost_tracker.as_ref() {
            tracker.lock().await.record_code_change(added, removed);
        }
    }
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p orchestrator api_duration_and_code_changes`
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
git add orchestrator/src/turn_loop.rs orchestrator/src/conversation.rs
git commit -m "feat(orchestrator): accumulate API duration + code-change lines

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: unknown-model flag from pricing

**Files:**
- Modify: the per-model cost recording path in `cost/src/tracker.rs` (where a model's cost is computed from `cost/src/pricing.rs`) — call `mark_unknown_model_cost()` when the model has no pricing entry.
- Test: inline in `cost/src/tracker.rs`.

**Interfaces:**
- Consumes: `cost::pricing` lookup (find with `grep -n "fn.*pricing\|fn lookup\|price_for\|pub fn" cost/src/pricing.rs | head`).

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn unknown_model_sets_flag() {
    let mut s = CostState::new(SessionId::from("s"));
    // record usage for a model with no pricing entry
    s.record_usage_for_model("some-unpriced-model", /* usage… per the real signature */);
    assert!(s.has_unknown_model_cost);
}
```
(Match the real per-model recording method name/signature.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p cost unknown_model_sets_flag`
Expected: FAIL.

- [ ] **Step 3: Implement** — in the per-model recording method, after the pricing lookup:
```rust
if pricing_lookup(model_ref).is_none() {
    self.mark_unknown_model_cost();
}
```
(Use the real pricing accessor; if pricing returns a zero/none sentinel rather than `Option`, branch on that.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cost unknown_model_sets_flag`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add cost/src/tracker.rs
git commit -m "feat(cost): mark unknown-model cost when a model lacks pricing

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 10: extend `CostSnapshot` + project the new fields

**Files:**
- Modify: `traits/src/orchestrator.rs` (`CostSnapshot` struct)
- Modify: `orchestrator/src/conversation.rs:2923` (`snapshot_cost_real`) and any other `CostSnapshot { … }` construction sites.
- Test: `orchestrator` snapshot test.

**Interfaces:**
- Produces (on `CostSnapshot`): `pub api_duration: std::time::Duration`, `pub code_lines_added: u64`, `pub code_lines_removed: u64`, `pub by_model: Vec<cost::summary::ModelCostSummary>`, `pub unknown_models: bool`.

- [ ] **Step 1: Write the failing test** (extend the existing `snapshot_cost_real` test, or add one):
```rust
#[tokio::test]
async fn snapshot_projects_new_cost_fields() {
    // after recording via the harness…
    let snap = orch.snapshot_cost_real().await;
    // defaults present and typed
    let _: std::time::Duration = snap.api_duration;
    let _: u64 = snap.code_lines_added;
    let _: bool = snap.unknown_models;
    let _: &Vec<_> = &snap.by_model;
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p orchestrator snapshot_projects_new_cost_fields`
Expected: FAIL (fields missing / construction sites incomplete).

- [ ] **Step 3: Implement** — add the fields to `CostSnapshot` (with doc comments — the crate warns on missing docs), then in `snapshot_cost_real` and every other `CostSnapshot { … }` literal, populate:
```rust
api_duration: std::time::Duration::from_millis(state.total_api_duration_ms),
code_lines_added: state.total_lines_added,
code_lines_removed: state.total_lines_removed,
by_model: cost::summary::by_model_summaries(&state), // reuse summary.rs projection; confirm fn name
unknown_models: state.has_unknown_model_cost,
```
For the empty/no-tracker early-return branch (`conversation.rs:2926`), use `Duration::ZERO`, `0`, `Vec::new()`, `false`. Fix ALL construction sites the compiler flags.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p orchestrator snapshot_projects_new_cost_fields` then `cargo build -p orchestrator -p traits`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**
```bash
git add traits/src/orchestrator.rs orchestrator/src/conversation.rs
git commit -m "feat: CostSnapshot carries api-duration, code-lines, by-model, unknown-models

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 11: route `/usage` command text through `cost_summary`

**Files:**
- Modify: `commands/core/src/usage.rs:48` (`render_usage_snapshot`)
- Test: `commands/core/src/usage.rs` test module

**Interfaces:**
- Consumes: `cost::render::{cost_summary, CostSummaryInput}` and `CostSnapshot`.

- [ ] **Step 1: Write the failing test** — replace/extend the existing `renders_flat_usage_snapshot`:
```rust
#[tokio::test]
async fn usage_renders_byte_exact_cost_summary() {
    let mock = Arc::new(MockOrchestratorHandle::new());
    mock.set_cost_snapshot(CostSnapshot {
        total_usd: 1.2345,
        api_duration: std::time::Duration::from_millis(65_000),
        session_duration: std::time::Duration::from_millis(3_661_000),
        code_lines_added: 1,
        code_lines_removed: 5,
        by_model: Vec::new(),
        unknown_models: false,
        // …other existing fields…
        ..Default::default() // if CostSnapshot derives Default; else fill all
    });
    let out = /* invoke the /usage handler and read its display text */;
    assert!(out.contains("Total duration (API):  1m 5s"), "{out}");
    assert!(out.contains("Total code changes:    1 line added, 5 lines removed"), "{out}");
    assert!(out.contains("Usage:                 0 input"), "{out}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p command-core usage_renders_byte_exact_cost_summary` (confirm crate name)
Expected: FAIL — old flat format.

- [ ] **Step 3: Implement** — rewrite `render_usage_snapshot`:
```rust
fn render_usage_snapshot(cost: &CostSnapshot) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let input = cost::render::CostSummaryInput {
        total_usd: cost.total_usd,
        unknown_models: cost.unknown_models,
        api_duration_ms: cost.api_duration.as_millis() as u64,
        wall_duration_ms: cost.session_duration.as_millis() as u64,
        code_lines_added: cost.code_lines_added,
        code_lines_removed: cost.code_lines_removed,
        by_model: &cost.by_model,
    };
    cost::render::cost_summary(&input)
}
```
Add `cost` to `commands/core/Cargo.toml` `[dependencies]` if not already present.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p command-core usage`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add commands/core/src/usage.rs commands/core/Cargo.toml
git commit -m "feat(usage): route /usage text through byte-exact cost_summary (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 12: render `cost_summary` in the TUI `/usage` screen

**Files:**
- Modify: `tui/src/bottom_pane/screen_view.rs:202` (`fn usage`)
- Test: `tui/src/bottom_pane/screen_view.rs` test module (there are existing `screen_view` tests asserting `Total cost:` etc.)

**Interfaces:**
- Consumes: `cost::render::{cost_summary, CostSummaryInput}`.

- [ ] **Step 1: Write the failing test** — assert the screen text now contains the new rows:
```rust
#[test]
fn usage_screen_shows_api_duration_and_by_model_rows() {
    let cost = CostSnapshot {
        total_usd: 2.0,
        api_duration: std::time::Duration::from_millis(5_000),
        session_duration: std::time::Duration::from_millis(9_000),
        code_lines_added: 0, code_lines_removed: 0,
        by_model: Vec::new(), unknown_models: false,
        ..Default::default()
    };
    let text = ScreenView::usage(&cost, UsageTab::Stats).rendered_text(); // use the existing text extractor the other tests use
    assert!(text.contains("Total duration (API):  5s"), "{text}");
    assert!(text.contains("Total code changes:    0 lines added, 0 lines removed"), "{text}");
}
```
(Match the existing test's rendering-extraction helper — the current tests already read `stats_body`/screen text.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tui usage_screen_shows`
Expected: FAIL.

- [ ] **Step 3: Implement** — in `fn usage`, build the stat rows from `cost::render::cost_summary(...)` (split into lines for the card, or render the block as the tab body), preserving the tabbed shell. Reuse the `CostSummaryInput` mapping from Task 11 (consider a small shared `fn to_summary_input(&CostSnapshot) -> CostSummaryInput` in `cost::render` to avoid duplicating the mapping — add it if the duplication is real). Keep `Total cost:`/`Total duration (wall):` rows sourced from the same block so nothing double-renders.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p tui screen_view` (run the whole module to catch the pre-existing `Total cost:` assertions)
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add tui/src/bottom_pane/screen_view.rs
git commit -m "feat(tui): /usage screen renders byte-exact cost_summary rows (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 13: full-suite verification + binary re-diff

**Files:** none (verification only)

- [ ] **Step 1: Build + test the touched crates**

Run:
```bash
cargo build -p cost -p traits -p orchestrator -p command-core -p tui
cargo test -p cost -p orchestrator -p command-core -p tui
```
Expected: clean build, all green.

- [ ] **Step 2: Byte re-diff the rendered block vs the binary**

Run a throwaway that prints `cost_summary` for a known input and confirm each line matches the pinned CC format (labels padded to col 23; `qs`/`FTu`/`Bu` outputs; singular/plural; empty `Usage:` line). Spot-check `Total duration (API):  1m 5s`, `Usage by model:` per-model line alignment.

- [ ] **Step 3: Confirm no regression on untouched surfaces**

Run: `cargo test -p cli` (print-mode `result` frame must be unchanged) and confirm `/cost`/`/stats` still resolve to `/usage` (`cargo test -p tui command::tests` — the alias tests).
Expected: green.

- [ ] **Step 4: Update the audit record**

Append a `COST_USAGE_FIDELITY_CLOSED_2026-07-12` note to `.omx/state/parity-206-loop.json` (commit list + "3 rows + unknown-model note now byte-exact via cost::render::cost_summary; /usage + TUI routed; print-mode unchanged").

- [ ] **Step 5: Commit**
```bash
git add .omx/state/parity-206-loop.json
git commit -m "chore: record /usage cost-summary fidelity closure (206)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes (author)

- **Spec coverage:** i6e/qs/cbg/FTu/Bu → Tasks 1–5; API-duration → Tasks 6,8; code-changes (cumulative per-edit) → Tasks 6,7,8; unknown-model → Tasks 6,9; CostSnapshot → Task 10; surface routing (`/usage` + TUI, print-mode unchanged) → Tasks 11,12; testing/verification → Task 13. `webSearchRequests` intentionally omitted (documented). Session-end auto-print intentionally out of scope.
- **Interface consistency:** `CostSummaryInput`, `cost_summary`, `format_duration_ms`, `format_cost`, `format_token_count`, `usage_by_model_block`, `record_api_duration`, `record_code_change`, `mark_unknown_model_cost`, `count_structured_patch_lines` — names are used identically across the tasks that produce and consume them.
- **Unknowns to confirm at implementation start (each task says so inline):** the `cost` crate's `CostState` constructor + per-model recording method signatures; `ModelCostSummary` exact field names; the `cost_tracker` handle type/lock on the orchestrator; the `command-core` crate name; whether `CostSnapshot` derives `Default`. These are lookups, not design gaps — resolve with the `grep` commands noted in each task.

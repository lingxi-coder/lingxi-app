# M9-02 Shared Primitives Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the two shared multi-agent presentation-styling primitives — a task-status → icon/color map (claude-code `taskStatusUtils` parity) and an agent-color → render-color map — that every M9 renderer/chrome will consume.

**Architecture:** One new file `tui/src/multiagent/style.rs` holding pure functions over the existing `crate::theme::Theme` (`iocraft::Color` fields) and the `TaskRow.status` wire strings produced by `TaskRegistryHandle`. No state, no engine wiring, no new dependencies.

**Tech Stack:** Rust 1.82, `iocraft::Color`, the existing `tui` crate + its `theme` module.

---

## Scope boundary (deviation from spec §3-M9-02, recorded deliberately)

The M9 design's §3 listed three items under M9-02: the agent-color map, the status icon/color map, **and** a "reusable select-list/detail-nav helper (reuse M7's if one exists; else extract)."

This plan ships the **two maps** and **defers the select-list helper to M9-05**. Rationale:
1. **No M7 precedent to reuse:** a survey of the `tui` crate shows M7 has *no* shared select-list — each overlay (`palette`, `completion`, `message_selector`) carries its own inline `selected: usize`. So "reuse M7's" is not an option.
2. **No consumer yet:** the first list-navigation consumer is M9-05's `BackgroundTasksDialog`. Extracting a generic select-list now (before any caller exists) is speculative abstraction — the worst kind. M9-05 will either follow M7's established per-component pattern or extract a helper grounded in the real dialog's needs.
3. The two maps, by contrast, have an immediate next-sub-plan consumer (M9-04 task rows) and are fully grounded against claude-code parity. The spec's "color/icon snapshot tables (all variants)" test intent maps exactly onto these two.

**Grounding facts (verified against the current tree):**
- `crate::theme::Theme` (`tui/src/theme.rs`) has `iocraft::Color` fields incl. `success`, `error`, `warning`, `dim`, `text`. `Theme` derives `PartialEq` (so `Color: PartialEq` — color equality is assertable in tests). `Color` is constructed via the private `rgb(r,g,b) -> Color::Rgb{r,g,b}` helper; named variants `Color::{Cyan,Magenta,Yellow,Green,Blue,Red}` are used in `theme.rs`'s ANSI map (so they exist).
- `TaskRow.status` (from M9-01) carries the **byte-locked lowercase** wire strings from `tasks/src/handle.rs::status_to_wire`: `"pending"`, `"running"`, `"completed"`, `"failed"`, `"killed"`.
- claude-code `components/tasks/taskStatusUtils.tsx` base mapping (no state-flag options): `running → figures.play (▶)`, `completed → figures.tick (✔)`, `failed||killed → figures.cross (✖)`, default → `figures.bullet (●)`; color: `completed → success`, `failed → error`, `killed → warning`, default(running/pending) → `background` (the dim/inactive color). The state-flag variants (idle/awaiting-approval/shutdown/has-error) arrive with richer task state in M9-04/05/06 and are out of M9-02 scope.
- `agent::display::AgentColor` has 10 variants: `Cyan, Magenta, Yellow, Green, Blue, Red, Orange, Purple, Pink, Teal`. Per M9 design §1, agent-color parity is **"equivalent look," not byte-identical RGB** — so M9-02 maps to sensible iocraft colors (named for the 6 ANSI, `Rgb` for the other 4) without chasing exact claude-code palette values.

**All commands run from `lingxi-code/`** (toolchain pins Rust 1.82.0).

---

## File Structure

**Create:**
- `lingxi-code/tui/src/multiagent/style.rs` — multi-agent presentation styling primitives: status icon/color map + agent-color map. Pure functions; one cohesive responsibility ("how multi-agent state is styled").

**Modify:**
- `lingxi-code/tui/src/multiagent/mod.rs` — add `pub mod style;` + re-exports.

**No other files change.** No new deps; no `agent`/`tasks`/`coordinator` edge (the agent-color enum is a TUI-local mirror, translated at the feed boundary in M9-06 — same posture as M9-01's `WorkerRow`).

---

### Task 1: Status + agent-color styling primitives

**Files:**
- Create: `lingxi-code/tui/src/multiagent/style.rs`
- Modify: `lingxi-code/tui/src/multiagent/mod.rs`

- [ ] **Step 1: Create `style.rs` with the status map + failing tests**

Create `lingxi-code/tui/src/multiagent/style.rs`:

```rust
//! Multi-agent presentation styling primitives. (M9-02)
//!
//! Pure maps from multi-agent state onto render styling:
//! - `task_status_icon` / `task_status_color` — claude-code `taskStatusUtils`
//!   parity over the byte-locked task status wire strings.
//! - `AgentColor` + `agent_color` — per-agent color (equivalent-look parity;
//!   exact RGB is a non-goal per the M9 design).

use crate::theme::Theme;
use iocraft::Color;

/// Icon for a task status (claude-code `getTaskStatusIcon`, base mapping —
/// state-flag variants land with richer task state in M9-04+). Unknown
/// statuses fall back to the bullet, matching claude-code's `default`.
#[must_use]
pub fn task_status_icon(status: &str) -> char {
    match status {
        "completed" => '✔',          // figures.tick
        "failed" | "killed" => '✖',  // figures.cross
        "running" => '▶',            // figures.play
        _ => '●',                    // figures.bullet (pending + unknown)
    }
}

/// Theme color for a task status (claude-code `getTaskStatusColor`, base
/// mapping). `background`/inactive maps onto the theme's `dim` color.
#[must_use]
pub fn task_status_color(status: &str, theme: &Theme) -> Color {
    match status {
        "completed" => theme.success,
        "failed" => theme.error,
        "killed" => theme.warning,
        _ => theme.dim, // running, pending, unknown → background/inactive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_icons_match_claude_code_figures() {
        assert_eq!(task_status_icon("completed"), '✔');
        assert_eq!(task_status_icon("failed"), '✖');
        assert_eq!(task_status_icon("killed"), '✖');
        assert_eq!(task_status_icon("running"), '▶');
        assert_eq!(task_status_icon("pending"), '●');
        assert_eq!(task_status_icon("totally-unknown"), '●');
    }

    #[test]
    fn status_colors_map_to_theme_semantics() {
        let t = Theme::dark();
        assert_eq!(task_status_color("completed", &t), t.success);
        assert_eq!(task_status_color("failed", &t), t.error);
        assert_eq!(task_status_color("killed", &t), t.warning);
        assert_eq!(task_status_color("running", &t), t.dim);
        assert_eq!(task_status_color("pending", &t), t.dim);
        assert_eq!(task_status_color("unknown", &t), t.dim);
    }
}
```

- [ ] **Step 2: Declare the module + re-export, run the status tests (verify pass)**

In `lingxi-code/tui/src/multiagent/mod.rs`, add `pub mod style;` (alongside the existing `pub mod` lines) and `pub use style::{task_status_color, task_status_icon};` (alongside the existing re-exports).

Run: `cd lingxi-code && cargo test -p tui multiagent::style`
Expected: PASS (`status_icons_match_claude_code_figures`, `status_colors_map_to_theme_semantics`).

- [ ] **Step 3: Add the agent-color map to `style.rs` (append after `task_status_color`, before the `#[cfg(test)]` block)**

```rust
/// TUI-local mirror of `agent::display::AgentColor` (10 colors). The feed
/// boundary (M9-06) translates the engine enum into this presentation copy,
/// keeping `tui` free of an `agent`-crate dependency (same posture as M9-01's
/// `WorkerRow` using plain strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentColor {
    /// Cyan.
    Cyan,
    /// Magenta.
    Magenta,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Red.
    Red,
    /// Orange.
    Orange,
    /// Purple.
    Purple,
    /// Pink.
    Pink,
    /// Teal.
    Teal,
}

/// Map an [`AgentColor`] to an iocraft render color. Parity is "equivalent
/// look", not byte-identical RGB (M9 design §1 non-goal): the 6 ANSI hues use
/// named colors; the other 4 use `Rgb`.
#[must_use]
pub fn agent_color(c: AgentColor) -> Color {
    match c {
        AgentColor::Cyan => Color::Cyan,
        AgentColor::Magenta => Color::Magenta,
        AgentColor::Yellow => Color::Yellow,
        AgentColor::Green => Color::Green,
        AgentColor::Blue => Color::Blue,
        AgentColor::Red => Color::Red,
        AgentColor::Orange => Color::Rgb { r: 255, g: 165, b: 0 },
        AgentColor::Purple => Color::Rgb { r: 160, g: 90, b: 220 },
        AgentColor::Pink => Color::Rgb { r: 255, g: 130, b: 180 },
        AgentColor::Teal => Color::Rgb { r: 0, g: 160, b: 160 },
    }
}
```

- [ ] **Step 4: Add the agent-color tests inside the existing `#[cfg(test)] mod tests` block**

```rust
    #[test]
    fn agent_colors_cover_all_ten_and_are_distinct() {
        let all = [
            AgentColor::Cyan,
            AgentColor::Magenta,
            AgentColor::Yellow,
            AgentColor::Green,
            AgentColor::Blue,
            AgentColor::Red,
            AgentColor::Orange,
            AgentColor::Purple,
            AgentColor::Pink,
            AgentColor::Teal,
        ];
        assert_eq!(all.len(), 10);
        // No two agents share a hue (Color: PartialEq via the theme derive).
        let mapped: Vec<Color> = all.iter().map(|c| agent_color(*c)).collect();
        for i in 0..mapped.len() {
            for j in (i + 1)..mapped.len() {
                assert_ne!(mapped[i], mapped[j], "agent colors {i} and {j} collide");
            }
        }
        // Spot-check a named + an rgb mapping.
        assert_eq!(agent_color(AgentColor::Cyan), Color::Cyan);
        assert_eq!(agent_color(AgentColor::Orange), Color::Rgb { r: 255, g: 165, b: 0 });
    }
```

- [ ] **Step 5: Update the re-export and run all style tests**

In `lingxi-code/tui/src/multiagent/mod.rs`, change the style re-export to:
```rust
pub use style::{agent_color, task_status_color, task_status_icon, AgentColor};
```

Run: `cd lingxi-code && cargo test -p tui multiagent::style`
Expected: PASS (3 tests: `status_icons_match_claude_code_figures`, `status_colors_map_to_theme_semantics`, `agent_colors_cover_all_ten_and_are_distinct`).

- [ ] **Step 6: Commit**

```bash
cd lingxi-code
git add tui/src/multiagent/style.rs tui/src/multiagent/mod.rs
git commit -m "feat(M9-02): multi-agent styling primitives (status icon/color + agent color)"
```

---

### Task 2: Workspace gate + tag `m9.2`

**Files:** none (verification + tag only)

- [ ] **Step 1: Format**

Run: `cd lingxi-code && cargo fmt -p tui && cargo fmt --check`
Expected: clean (exit 0). If `cargo fmt -p tui` changed anything, stage + commit: `git add tui/ && git commit -m "style(M9-02): cargo fmt"`.

- [ ] **Step 2: Clippy (scoped to the new code)**

Run: `cd lingxi-code && cargo clippy -p tui --all-targets -- -D warnings 2>&1 | grep -E 'tui/src/multiagent|^warning|^error' | head`
Expected: NO lines referencing `tui/src/multiagent`. (Pre-existing `tool-api` clippy debt is unrelated and tracked separately; M9-02 must add zero new `tui` warnings.)

- [ ] **Step 3: Test the crate**

Run: `cd lingxi-code && cargo test -p tui`
Expected: PASS (M9-01's multiagent tests + M7 suites + the 3 new `multiagent::style` tests; no regressions).

- [ ] **Step 4: Workspace build**

Run: `cd lingxi-code && cargo build --workspace --all-targets --offline`
Expected: `Finished` (exit 0). (M9-02 adds only a pure-function module + re-exports — no consumer construction sites change.)

- [ ] **Step 5: Annotated tag**

```bash
cd lingxi-code
git tag -a m9.2 -m "M9-02: multi-agent styling primitives (status icon/color + agent color maps)"
git tag --list m9.2
```
**Do not push.**

---

## Self-Review

**1. Spec coverage (design §3-M9-02):**
- agent-color map → Task 1 (`AgentColor` + `agent_color`). ✓
- task status icon/color map → Task 1 (`task_status_icon` + `task_status_color`). ✓
- "color/icon snapshot tables (all variants)" → Task 1 tests cover all 5 statuses (+ unknown) for both icon and color, and all 10 agent colors. ✓ (unit assertion tables rather than `insta` snapshots — appropriate for pure scalar maps; `insta` is reserved for rendered multi-line views.)
- reusable select-list helper → **relocated to M9-05** (documented under "Scope boundary"; no M7 precedent, no consumer yet). ✓ (deliberate)

**2. Placeholder scan:** No TBD/TODO/vague steps. Every code step is complete and compiling. ✓

**3. Type consistency:**
- `task_status_icon(&str) -> char`, `task_status_color(&str, &Theme) -> Color`, `agent_color(AgentColor) -> Color`, `AgentColor` (10 variants) — referenced identically in `style.rs` and the `mod.rs` re-export. ✓
- Status keys (`"completed"/"failed"/"killed"/"running"/"pending"`) match `tasks/src/handle.rs::status_to_wire` exactly. ✓
- `Theme` field names (`success`/`error`/`warning`/`dim`) match `tui/src/theme.rs`. ✓
- `Color::{Cyan,Magenta,Yellow,Green,Blue,Red}` + `Color::Rgb{r,g,b}` match iocraft's API as used in `theme.rs`. ✓

No gaps. Plan is internally consistent and grounded.

---

**End of plan.**

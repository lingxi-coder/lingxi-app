# iocraft Deletion — Execution Plan (Option A: gut `tui` in place)

Worktree: `.worktrees/iocraft-deletion`, branch `iocraft-deletion`, off main `f4ddad16f`.
Goal: `cargo tree -i iocraft` returns EMPTY; ratatui (`tui-rata`) is the only TUI;
the `tui` crate survives as iocraft-free non-UI infra so ~63 consumer refs keep resolving.

## Decision (user away → best-judgment): parity_tui oracles
Re-point byte-identical assertions to tui-rata renderers; DROP the deliberately
diverged ones (composer/footer/screens that tui-rata intentionally changed).
Document every dropped assertion in the commit. Re-ask the user if they return.

## Surviving infra in `tui` (must NOT break — verified consumers)
- `tui::session::Runtime` + builder chain (with_subscription/with_bash_runner/…)
- `tui::replay::rebuild_from_jsonl`
- `tui::state::{RenderedMessage, StatusSnapshot}` (tui-core shims)
- `tui::events::orchestrator_bridge::TurnEvent`, `tui::permission_bridge::{PermissionExchange,TuiPermissionGate}` (shims)
- `tui::bash_runner::{BashRunner, BashRunOutput}`
- `tui::recent_models::record_default_model`
- `tui::components::status_line_command::StatusLineConfig` (keep JUST this module)
- `tui::multiagent::{PollerFeed, MultiAgentFeed}` (non-UI feed; verify iocraft-free after UI delete)

## The 3 iocraft UIs cli calls directly → must port to tui-rata first
- `tui::startup_trust::{mount_trust_dialog, TrustDialogOutcome}` (311 LOC)
- `tui::startup_bypass::{mount_bypass_dialog, should_show_bypass_dialog, BypassDialogOutcome}` (272 LOC)
- `tui::session::run_resume_picker` (iocraft `element!`/TuiRoot + screens::resume)

## Phases (each ends green: `cargo build -p cli` + named `cargo test`)

### Phase B (FIRST — mechanical, fully headless-verifiable): remove the iocraft chat branch
- `apps/cli/src/mode.rs`: delete the `if use_ratatui_backend() {…} else {…}` fork —
  keep ONLY the `run_ratatui` path; drop `build_tui_runtime`/`mount_tui_runtime`
  (iocraft chat mount), `use_ratatui_backend`, `ratatui_selected`, `LINGXI_TUI_BACKEND`.
- Also the `--resume` mount (`resume_and_mount`) if it routes through `mount_tui_runtime`.
- Removes cli use of `tui::{run_tui_session, root::pump_turn, streaming::apply_event, BridgeOutputStream}`.
- Update the env doc + Warp notice copy that mentions `LINGXI_TUI_BACKEND=iocraft`.
- Verify: `cargo test -p cli`, `cargo build -p cli`.

### Phase A1: port resume picker to tui-rata
- New `tui_rata::resume::run_resume_picker(rows) -> Option<uuid::Uuid>` using
  tui-rata's picker/overlay + a bottom-pane view or a dedicated screen loop.
- Headless test: feed rows, drive key events, assert selection.
- Repoint `apps/cli/src/run.rs:1525` to `tui_rata::resume::run_resume_picker`.

### Phase A2: port trust + bypass startup dialogs to tui-rata
- New `tui_rata::startup::{mount_trust_dialog, mount_bypass_dialog}` (+ outcomes)
  using tui-rata's dialog/overlay widgets; keep the exact decision semantics
  (Decline/Esc → exit 1; Accept → persist skip-dangerous).
- Headless tests for accept/decline/esc outcomes.
- Repoint cli `mode.rs` trust_gate()/bypass block to the tui-rata versions.
- After A1+A2: cli/engine-desktop use `tui::` only for surviving infra.

### Phase C: parity_tui oracle tests (see Decision)
- `parity_tui_renderers.rs`, `parity_tui_renderers_m7.rs`: message-cell render-to-string
  — repoint to tui-rata's `history_cell` renderers where output is byte-identical
  (shared tui-core renderers). Drop composer/footer/screen assertions that diverged.
- `parity_tui_permission_dialogs.rs`, `parity_tui_screens.rs`, `parity_tui_repl_loop.rs`,
  `parity_tui_multiagent.rs`: repoint or delete per divergence; document drops.

### Phase D: delete the iocraft UI tree from `tui/src`
- Delete: `components/**`, `render_iocraft.rs`, `root.rs`, `screens/**`,
  `session.rs` render loop (keep `Runtime`/`run_resume_picker`-free session infra),
  `render/**` (iocraft), `theme*.rs`/`terminal.rs`/`error.rs`/`events/keymap.rs`/
  `telemetry.rs`/`recent_models.rs` iocraft bits, `multiagent/` UI (keep feed),
  `app.rs`, `startup_*.rs` (now in tui-rata), all `#[component]`/`element!` sites.
- Keep: `session::Runtime`, `replay`, `state` shim, `events/orchestrator_bridge` shim,
  `permission_bridge` shim, `bash_runner`, `recent_models::record_default_model`,
  `components/status_line_command`, `multiagent` non-UI feed.
- Prune `lib.rs` re-exports to the survivors.

### Phase E: drop iocraft + verify
- Remove `iocraft = "=0.8.3"` from `tui/Cargo.toml`; trim now-unused deps.
- `cargo tree -i iocraft` → EMPTY.
- `cargo build --workspace`; `cargo test -p tui -p tui-rata -p cli -p test-harness`.
- Update welcome banner / docs that referenced the iocraft backend.

## Rollback: worktree is isolated; `v0.13.0-tui-ratatui` tag (`5018e1c12`) is the
pre-migration restore point. Nothing on main until the user reviews + merges.

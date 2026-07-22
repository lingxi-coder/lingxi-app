//! Scheduling tools: CronCreate, CronDelete, CronList, RemoteTrigger.
//! Extracted in M8-P7. Cross-platform.
#![forbid(unsafe_code)]
#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::match_wildcard_for_single_variants,
    clippy::single_match_else,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::format_collect,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::manual_let_else
)]
pub mod cron_delete;
pub mod cron_list;
pub mod remote_trigger;
pub mod schedule_cron;
pub mod wakeup;
// `autonomous_loop` was relocated to the root-level `cron` crate to satisfy
// §8.1 dependency layering (command-core / tool-task must not depend on this
// tool crate). It is re-exported here so `tool_cron::autonomous_loop` and every
// `tool_cron::<Symbol>` path below keep working for existing callers.
pub use cron::autonomous_loop;
pub use cron::{
    begin_loop_tick, get_autonomous_loop_preamble, is_autonomous_loop_sentinel,
    is_loop_default_prompt_enabled, is_loop_default_sentinel, is_loop_dynamic_enabled,
    is_loop_file_sentinel, is_loop_keepalive_enabled, is_push_notif_enabled,
    log_autonomous_loop_activation, loop_consecutive_keepalives, loop_tick_in_flight_prompt,
    mark_loop_rescheduled, read_loop_file, reset_autonomous_loop_delivered,
    reset_loop_runtime_state, resolve_autonomous_loop_fire, resolve_loop_default_fire,
    resolve_loop_file_fire, set_loop_consecutive_keepalives, take_loop_rescheduled,
    take_loop_tick_in_flight_prompt, LoopFile, LoopRuntime, AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
    AUTONOMOUS_LOOP_PREAMBLE, AUTONOMOUS_LOOP_SENTINEL, LOOP_FILE_DYNAMIC_SENTINEL,
    LOOP_FILE_SENTINEL,
};
pub use cron_delete::CronDeleteTool;
pub use cron_list::CronListTool;
pub use remote_trigger::{ClaudeAiAuthProvider, RemoteTriggerTool};
pub use schedule_cron::CronCreateTool;
pub use wakeup::{
    arm_keepalive, arm_keepalive_with_runtime, clamp_delay_seconds, maybe_arm_keepalive,
    maybe_arm_keepalive_with_runtime, resolve_wakeup_prompt, KeepaliveOutcome, ScheduleWakeupTool,
    WakeupScheduler, WakeupSchedulerCell, SCHEDULE_WAKEUP_TOOL_NAME,
};
/// Register the cron scheduling tools against `reg`.
///
/// `RemoteTrigger` is registered WITHOUT an OAuth auth provider (`None`), so its
/// pre-flight "not authenticated" error fires until a host wires one. The
/// desktop composition root calls [`register_all_with_auth`] instead to hand the
/// tool a credential-store-backed [`ClaudeAiAuthProvider`]. Mobile (WIP) uses
/// this no-provider path.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    // The returned wakeup cell is dropped: hosts using this no-provider path
    // (mobile / offline) own no per-connection queue, so they never fill it and
    // `ScheduleWakeup` stays an honest no-op.
    let _ = register_all_with_auth(reg, ctx, None);
}

/// Register the cron scheduling tools, handing `RemoteTrigger` an in-process
/// [`ClaudeAiAuthProvider`] (`Some(..)` on desktop; `None` is equivalent to
/// [`register_all`]).
///
/// Returns the [`WakeupSchedulerCell`] for the registered `ScheduleWakeup` tool.
/// A host that owns a per-connection queue + spawner (the desktop bridge) fills
/// it after `build` via `cell.set(scheduler)`; other hosts drop it (no-op tool).
pub fn register_all_with_auth(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    auth: Option<std::sync::Arc<dyn ClaudeAiAuthProvider>>,
) -> WakeupSchedulerCell {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(CronCreateTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(CronDeleteTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(CronListTool::new(ctx.clone())));
    // `ScheduleWakeup` (/loop dynamic mode) — registered with an EMPTY set-once
    // wakeup cell; the cell clone is returned so the composition root can fill it
    // once the per-connection queue + spawner exist (`boot::assemble`).
    // Subagent gating is handled in `agent/src/tool_resolver.rs`
    // (`all_agent_disallowed_tools`, the binary `_qd`/`nHe` removal set which
    // includes `ScheduleWakeup`), so the tool is STRIPPED from a subagent's
    // resolved pool and only effective in the MAIN loop. (`NKE_BASE` in
    // `runner.rs` is only the companion advisory note appended to a refusal — it
    // performs no filtering; the actual removal is in `tool_resolver.rs`.)
    //
    // DECISION: registered on BOTH desktop and mobile through this shared path.
    // On mobile (and any host that owns no per-connection queue) the cell is
    // never filled, so the tool is an honest no-op (see `ScheduleWakeupTool::call`)
    // — harmless and avoids forking the registration API.
    let wakeup_tool = ScheduleWakeupTool::new(ctx.clone());
    let wakeup_cell = wakeup_tool.wakeup_cell();
    reg.register_builtin(Arc::new(wakeup_tool));
    reg.register_builtin(Arc::new(RemoteTriggerTool::new(ctx, auth)));
    wakeup_cell
}

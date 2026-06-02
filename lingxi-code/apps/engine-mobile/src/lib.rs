//! Mobile composition root (M8-P11).
//!
//! The mobile sibling of `engine-desktop`: the single declarative place that
//! decides the mobile capability set. It links a *different* subset of crates
//! via `Cargo.toml` — the cross-platform tools (file/task/web/plan/meta/cron/
//! ui/skill) plus the mobile-exclusive tools (camera/voice/share), the mobile
//! skill set, and the core + mobile command sets — while deliberately omitting
//! the desktop-only tools (shell/agent/mcp/lsp/team/worktree) and the
//! device-control tools. Same core agent logic, different assembly: no
//! `#[cfg(target_os)]` switching in any library crate.
//!
//! As with `engine-desktop`, the host binary owns the runtime wiring
//! (constructing the `BuiltinToolContext` from an `Arc<dyn Platform>` and
//! threading the orchestrator handle); this crate provides the pure
//! registry-assembly functions.

#![forbid(unsafe_code)]

use command_api::CommandRegistry;
use command_core::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2,
    register_core_batch_4, register_core_batch_5,
};
use skill_api::SkillRegistry;
use std::sync::Arc;
use tool_api::{BuiltinToolContext, ToolRegistry};
use traits::{AuthHandle, OrchestratorHandle};

/// Mobile engine knobs.
#[derive(Clone, Debug)]
pub struct MobileEngineConfig {
    /// Model id the mobile build defaults to.
    pub default_model: String,
}

impl Default for MobileEngineConfig {
    fn default() -> Self {
        Self {
            default_model: "claude-sonnet-4-20250514".to_string(),
        }
    }
}

/// Assemble the mobile builtin **tool** registry from a freshly-built
/// [`BuiltinToolContext`] (whose `camera`/`voice`/`share` handles come from the
/// mobile `Platform`).
#[must_use]
pub fn mobile_tool_registry(ctx: BuiltinToolContext) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_tools(&mut reg, ctx);
    reg
}

/// Register the mobile tool set into an existing registry.
pub fn register_mobile_tools(reg: &mut ToolRegistry, ctx: BuiltinToolContext) {
    // ----- cross-platform subset (also linked by engine-desktop) -----------
    tool_file::register_all(reg, ctx.clone());
    tool_task::register_all(reg, ctx.clone());
    tool_web::register_all(reg, ctx.clone());
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    tool_cron::register_all(reg, ctx.clone());
    tool_ui::register_all(reg, ctx.clone());
    tool_skill::register_all(reg, ctx.clone());
    // ----- mobile-exclusive tools ------------------------------------------
    tool_camera::register_all(reg, ctx.clone());
    tool_voice::register_all(reg, ctx.clone());
    tool_share::register_all(reg, ctx);
}

/// Assemble the mobile builtin **skill** registry.
#[must_use]
pub fn mobile_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_builtin::register_mobile(&mut reg);
    reg
}

/// Assemble the mobile slash-command registry: the core handlers plus the
/// mobile-only handlers (`/mobile` `/voice` `/share` `/camera`).
#[must_use]
pub fn mobile_command_registry(
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    command_mobile::register(&mut reg);
    reg
}

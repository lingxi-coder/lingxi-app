//! Desktop composition root (M8-P6).
//!
//! `engine-desktop` is the single declarative place that decides **which**
//! builtin tools and slash-commands (and, from P8, skills) ship in the
//! desktop build. It names each capability crate via a Cargo dependency edge
//! in `Cargo.toml` — the mobile composition root (`engine-mobile`, P11) will
//! depend on a different subset. There is no `#[cfg(target_os)]` switching:
//! the shipped capability set is chosen by *which composition root the app
//! links*, not by conditional compilation scattered through library crates.
//!
//! ## What lives here
//! - [`desktop_tool_registry`] / [`register_desktop_tools`] — assemble the 14
//!   desktop tool crates into a [`ToolRegistry`].
//! - [`desktop_command_registry`] — assemble the builtin slash-commands.
//! - [`DesktopEngineConfig`] — the knobs the composition root needs.
//!
//! ## Construction order (owned by the host binary)
//! The runtime wiring has an inherent cycle: tools must exist before the
//! orchestrator (it owns the [`ToolRegistry`]), and the command handlers bind
//! to an `Arc<dyn OrchestratorHandle>` produced *by* that orchestrator. So the
//! host binary (`apps/cli`) drives the order — build tools → build orchestrator
//! → build commands — and this crate provides the two pure assembly functions
//! it calls. A future consolidation can fold the orchestrator wiring into a
//! single `build(platform, config) -> Engine` here once the platform aggregate
//! trait (P10) lands; see the design doc §6.4.

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

/// Desktop engine knobs.
///
/// Intentionally small in P6 — it grows as P8/P9 fold skills + commands and a
/// full `build()` entrypoint into the composition root.
#[derive(Clone, Debug)]
pub struct DesktopEngineConfig {
    /// Model id the desktop build defaults to when argv omits `--model`.
    pub default_model: String,
}

impl Default for DesktopEngineConfig {
    fn default() -> Self {
        Self {
            default_model: "claude-sonnet-4-20250514".to_string(),
        }
    }
}

/// Assemble the desktop builtin **tool** registry from a freshly-built
/// [`BuiltinToolContext`].
///
/// This is the canonical desktop tool set: 9 cross-platform crates
/// (`tool-file/shell/task/web/plan/meta/cron/ui/skill`) + 5 desktop-only
/// crates (`tool-agent/team/worktree/mcp/lsp`). The mobile composition root
/// links only the cross-platform subset plus mobile-specific crates.
#[must_use]
pub fn desktop_tool_registry(ctx: BuiltinToolContext) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_desktop_tools(&mut reg, ctx);
    reg
}

/// Register the desktop tool set into an existing (empty) registry.
///
/// Each `tool_*::register_all` consumes a clone of `ctx`; the final crate
/// takes ownership to avoid a redundant clone.
pub fn register_desktop_tools(reg: &mut ToolRegistry, ctx: BuiltinToolContext) {
    // ----- cross-platform tool crates (also linked by engine-mobile, P11) ---
    tool_file::register_all(reg, ctx.clone());
    tool_shell::register_all(reg, ctx.clone());
    tool_web::register_all(reg, ctx.clone());
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    tool_cron::register_all(reg, ctx.clone());
    tool_ui::register_all(reg, ctx.clone());
    tool_skill::register_all(reg, ctx.clone());
    tool_task::register_all(reg, ctx.clone());
    // ----- desktop-only tool crates ----------------------------------------
    tool_agent::register_all(reg, ctx.clone());
    tool_team::register_all(reg, ctx.clone());
    tool_worktree::register_all(reg, ctx.clone());
    tool_mcp::register_all(reg, ctx.clone());
    tool_lsp::register_all(reg, ctx);
}

/// Assemble the desktop builtin **skill** registry.
///
/// Delegates to `skill_builtin::register_desktop`, the single place that names
/// the desktop builtin skill set. Empty in M8 (no Rust-bundled skills yet —
/// skills are markdown loaded from disk by the session loader); the mobile
/// composition root will call `skill_builtin::register_mobile` instead.
#[must_use]
pub fn desktop_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_builtin::register_desktop(&mut reg);
    reg
}

/// Assemble the desktop slash-command registry.
///
/// Mirrors the boot sequence the CLI used inline before P6:
/// [`register_all_builtin_commands`] seeds the builtin handlers, then
/// [`register_core_batch_1`] + [`register_core_batch_2`] overwrite the wired
/// core handlers with their orchestrator/auth-bound implementations.
#[must_use]
pub fn desktop_command_registry(
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    // Desktop-only command handlers (no-op in M8 — the names remain
    // command-core unimplemented stubs until future milestones fill them).
    command_desktop::register(&mut reg);
    reg
}

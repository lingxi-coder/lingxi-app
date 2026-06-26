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

// F3-03: the shared mobile session-host module — `MobileConfig` +
// `build_mobile(MobileConfig, Platform, listener, sink) -> MobileRuntime`. It
// lives under the `uniffi` feature because it wires the `client-adapter` sinks
// (`AdapterOutputStream` / `AdapterPermissionGate`) + the `ClientEventListener`,
// which are pulled ONLY under that feature (the FFI surface). Both FFI packager
// crates (`ios-framework` / `android-aar`) re-export this shared host (F3-04) so
// iOS and Android cannot drift.
#[cfg(feature = "uniffi")]
mod host;

// Audit fix (#14): the disk-backed Skill loader the FFI host wires so the mobile
// Skill tool resolves on-disk `.lingxi/commands` / `.lingxi/skills` under the
// app-private root. uniffi-gated — its `SkillLoader` impl uses `async-trait`
// (an FFI-only optional dep) and only the FFI host constructs a real loader.
#[cfg(feature = "uniffi")]
mod skill_loader;

#[cfg(feature = "uniffi")]
pub use host::{
    build_mobile, build_mobile_engine, build_mobile_engine_inner, build_mobile_inner,
    MobileBuildError, MobileConfig, MobileEngineError, MobileEngineHandle, MobileRuntime,
};

// F3-06: the host-only walking-skeleton support — a portable fake `Platform`
// shim (fs/http/clock stubs over a temp root), a recording `ClientEventListener`,
// a collecting `PermissionRequestSink`, and a streaming-injecting engine
// constructor. Lives behind the `uniffi` feature (it names the FFI-surface
// types) and is exposed so both the in-crate F3-03/F3-05 unit tests AND the
// `tests/skeleton_test.rs` integration test build the SAME off-device host. The
// real device `Platform` is `cfg(target_os)`-gated, so this shim is what proves
// the skeleton on CI — exactly the spec §8 "prove from a Swift/Kotlin unit test"
// smoke path, runnable on the host.
#[cfg(feature = "uniffi")]
pub mod test_support;

// F3-04: re-export the FFI-visible adapter types both packager crates name when
// they call `build_mobile_engine` (the foreign `ClientEventListener` they
// register and the `PermissionRequestSink` the gate emits to). Re-exporting them
// from the shared host crate keeps the FFI crates free of a direct
// `client-adapter` import for these types — the shared host is the single seam.
#[cfg(feature = "uniffi")]
pub use client_adapter::{ClientEventListener, ListenerSink, PermissionRequestSink};

// F3-04: this crate now DEFINES UniFFI-exported types (`MobileEngineHandle` as a
// `uniffi::Object`, `MobileEngineError` as a `uniffi::Error` — see `host`), so it
// must register their FFI metadata via the scaffolding macro. The aggregating
// cdylib crates (`ios-framework` / `android-aar`) re-export this scaffolding so
// the symbols land in the final library (the same pattern `client-adapter` uses
// for the `ClientEventListener` callback interface). Compiles ONLY under the
// `uniffi` feature; the default host build never includes it.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

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

/// Register the mobile tool set into an existing registry, with the `Skill` tool
/// INERT (the hermetic `EmptySkillLoader`). Used by tests and the non-FFI host
/// build. The FFI host instead calls [`register_mobile_tools_with_skill_loader`]
/// to wire a disk-backed loader (audit fix #14).
pub fn register_mobile_tools(reg: &mut ToolRegistry, ctx: BuiltinToolContext) {
    register_mobile_non_skill_tools(reg, ctx.clone());
    // Skill tool with the hermetic `EmptySkillLoader` (no on-disk discovery).
    tool_skill::register_all(reg, ctx);
}

/// Audit fix (#14): register the mobile tool set with a FUNCTIONAL `Skill` tool
/// backed by `skill_loader` (the disk-backed `MobileDiskSkillLoader`) instead of
/// the inert `EmptySkillLoader`, so model-invoked skills resolve against the
/// device's on-disk `.lingxi/commands` / `.lingxi/skills`. uniffi-gated because
/// the loader impl needs `async-trait` (an FFI-only optional dep) and only the
/// FFI host wires a real loader.
#[cfg(feature = "uniffi")]
pub fn register_mobile_tools_with_skill_loader(
    reg: &mut ToolRegistry,
    ctx: BuiltinToolContext,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
) {
    register_mobile_non_skill_tools(reg, ctx.clone());
    reg.register_builtin(Arc::new(tool_skill::SkillTool::with_loader(
        ctx,
        skill_loader,
    )));
}

/// Every mobile tool EXCEPT `Skill` (whose loader differs by build). Builtin wire
/// order is locale-sorted at enumeration time, so registration order is immaterial.
fn register_mobile_non_skill_tools(reg: &mut ToolRegistry, ctx: BuiltinToolContext) {
    // ----- cross-platform subset (also linked by engine-desktop) -----------
    tool_file::register_all(reg, ctx.clone());
    tool_task::register_all(reg, ctx.clone());
    tool_web::register_all(reg, ctx.clone(), None);
    tool_plan::register_all(reg, ctx.clone());
    tool_meta::register_all(reg, ctx.clone());
    // Audit fix (#7): the cron tools (Create/List/Delete/RemoteTrigger) are
    // registered, but mobile starts NO `cron::CronScheduler` (the desktop root is
    // the only place one runs) and wires no `task_registry` for it to fire into —
    // a backgrounded app has no long-running daemon. So a created cron job is
    // saved/listed/deletable but does NOT auto-fire on this platform; CronCreate's
    // result text says so (see schedule_cron.rs `scheduler_active`). RemoteTrigger
    // is independent of the local scheduler (it triggers a cloud-side run).
    tool_cron::register_all(reg, ctx.clone());
    tool_ui::register_all(reg, ctx.clone());
    // ----- mobile-exclusive tools ------------------------------------------
    // camera / voice / speech / notification / clipboard / share, folded into
    // the single `tool-mobile` crate.
    tool_mobile::register_all(reg, ctx.clone());
    // P3: Android-only Shell tool. Self-gates on ctx.android_shell.enabled;
    // iOS and desktop are unaffected (their ctx.android_shell is None).
    tool_shell_mobile::register_all(reg, ctx.clone());
    // P4: Android-only Git tool. Self-gates on ctx.android_git.as_ref().is_some_and(|g| g.enabled);
    // iOS and desktop are unaffected (their ctx.android_git is None).
    tool_git_mobile::register_all(reg, ctx);
}

/// Audit fix (#14): the FFI sibling of [`mobile_tool_registry`] that wires a
/// disk-backed `Skill` loader (the mobile composition root passes the
/// `MobileDiskSkillLoader` it built from the device's app-private root).
#[cfg(feature = "uniffi")]
#[must_use]
pub fn mobile_tool_registry_with_skill_loader(
    ctx: BuiltinToolContext,
    skill_loader: Arc<dyn tool_skill::skill::SkillLoader>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    register_mobile_tools_with_skill_loader(&mut reg, ctx, skill_loader);
    reg
}

/// Assemble the mobile builtin **skill** registry.
#[must_use]
pub fn mobile_skill_registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    skill_api::register_mobile(&mut reg);
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
    // Bundled programmatic skills (`/loop`), mirroring desktop. Gated on the cron
    // kill-switch (loop.ts:83); mobile starts no cron scheduler so a scheduled
    // job is inert, but the skill's listing/usage path is harmless and faithful.
    let cron_enabled = !traits::env::is_env_truthy(
        std::env::var("CLAUDE_CODE_DISABLE_CRON").ok().as_deref(),
    );
    command_core::register_bundled_skills(&mut reg, cron_enabled);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    // Mobile-only command handlers: currently none — the mobile command names
    // (/mobile, /voice, /share, /camera) are served as command-core
    // unimplemented stubs. Register real mobile handlers on `reg` directly here
    // when implemented.
    reg
}

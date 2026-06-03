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
    tool_speech::register_all(reg, ctx.clone());
    tool_notification::register_all(reg, ctx.clone());
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

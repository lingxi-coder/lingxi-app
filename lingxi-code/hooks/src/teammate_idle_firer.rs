//! Narrow hook-firer seam for the `TeammateIdle` lifecycle event.
//!
//! The `tasks` crate is a LEAF with no access to a live hook executor: the
//! [`InProcessTeammateHandler`](../../tasks/src/handlers/in_process_teammate.rs)
//! owns the teammate lifecycle (its streaming worker observes the persistent
//! runner's per-turn-set `Completed`, at which point the teammate parks awaiting
//! the next message — "about to go idle") but cannot reach `orch.hooks` without
//! a dependency cycle. This trait closes that seam exactly like
//! [`TaskCompletedFirer`](crate::TaskCompletedFirer): the handler holds an
//! `Option<Arc<dyn TeammateIdleFirer>>` (default `None` => strict no-op) and
//! calls [`fire`](TeammateIdleFirer::fire) best-effort each time a turn-set
//! completes and the teammate is about to idle.
//!
//! Defining the trait HERE (the `hooks` crate) — rather than in `tasks` — lets
//! the ORCHESTRATOR implement it over its `Arc<HookExecutorImpl>` (the
//! orchestrator depends on `hooks` but NOT on `tasks`, so it could never name a
//! `tasks`-defined trait). The composition root (`engine-desktop`) constructs
//! the impl and injects it where the `InProcessTeammateHandler` is built. This
//! mirrors [`TaskCompletedFirer`] / [`TaskCreatedFirer`](crate::TaskCreatedFirer).
//!
//! Parity: claude-code fires `executeTeammateIdleHooks` (`utils/hooks.ts:3709`)
//! from `stopHooks.ts:403` — after the Stop hooks pass and the teammate's
//! in-progress `TaskCompleted` hooks run — gated on `isTeammate()`, when the
//! teammate's query loop has stopped and it is about to park awaiting the next
//! message. The wire payload (`TeammateIdleHookInputSchema`,
//! `coreSchemas.ts:591-598`) carries `teammate_name` + `team_name` (both
//! required). The fire is best-effort: a blocking hook makes the teammate keep
//! working in claude-code, but the Rust seam degrades to "no intervention" so a
//! misbehaving hook never strands the teammate.

use std::sync::Arc;

use async_trait::async_trait;

/// The byte-faithful wire payload for a `TeammateIdle` fire, sourced from the
/// teammate handler at the per-turn-set idle transition. Field set mirrors
/// `TeammateIdleHookInputSchema` (`coreSchemas.ts:591-598`): `teammate_name` +
/// `team_name` are BOTH required. `team_name` rides as `""` when the firing
/// scope (the leaf `tasks` handler) cannot reach the team identity — faithful to
/// claude-code's `getTeamName() ?? ''` fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeammateIdleFire {
    /// Name of the teammate going idle (wire `teammate_name`, required).
    pub teammate_name: String,
    /// Team the teammate belongs to (wire `team_name`, required; `""` when the
    /// firing scope has no team identity).
    pub team_name: String,
}

/// One-method seam the `tasks` crate uses to fire a `TeammateIdle` hook without
/// owning a hook executor. The default handler holds `None` and performs a
/// strict no-op; the orchestrator provides the real impl.
#[async_trait]
pub trait TeammateIdleFirer: Send + Sync {
    /// Fire the `TeammateIdle` hook for a teammate about to park awaiting input.
    ///
    /// Best-effort: implementations MUST NOT propagate hook failures — a failing
    /// or absent hook is swallowed so the caller's turn-set loop is never
    /// disturbed.
    async fn fire(&self, fire: TeammateIdleFire);
}

/// Convenience alias for the optional firer the
/// [`InProcessTeammateHandler`](../../tasks/src/handlers/in_process_teammate.rs)
/// holds.
pub type OptionalTeammateIdleFirer = Option<Arc<dyn TeammateIdleFirer>>;

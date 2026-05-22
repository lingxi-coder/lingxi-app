//! Hook system — 28-event observability + intervention plane.
//!
//! Hooks fire around tool dispatch, session lifecycle, permission requests,
//! and many other engine touchpoints. Each registered hook can observe an
//! event and optionally return a [`HookResponse`] that blocks, approves, or
//! mutates the in-flight action.
//!
//! M1.4 ships the data model and an executor scaffold: the 28-variant event
//! enum, hook definition / source taxonomy, response aggregation, the
//! [`HookExecutorImpl`], and the [`SsrfGuard`]. The full Command / HTTP /
//! Agent executor bodies and the in-flight tracker land in Plan 09.
//!
//! See spec §9 (Hook System) and D17 (Runtime boundary — no direct tokio).

#![forbid(unsafe_code)]

pub mod async_registry;
pub mod builtin;
pub mod definition;
pub mod events;
pub mod executor;
pub mod registry;
pub mod response;
pub mod ssrf_guard;

pub use async_registry::AsyncHookRegistry;
pub use definition::{HookCondition, HookDefinition, HookExecutor, HookSource};
pub use events::{HookEvent, HookEventType};
pub use executor::{BuiltinHookHandler, HookExecutorImpl};
pub use registry::{HookContext, HookRegistry};
pub use response::{AggregateHookResult, HookDecision, HookOutcome, HookResponse, HookResult};
pub use ssrf_guard::{IpRange, SsrfError, SsrfGuard};

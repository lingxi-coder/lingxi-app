//! Dual-LLM multi-agent execution strategy (LingXi-only).
//!
//! This crate implements the orchestration strategy described in
//! `docs/multi-agent-dual-llm-design.md`: high-risk or user-requested tasks can
//! be implemented independently by two configured LLM backends, cross-reviewed,
//! revised, and arbitrated, with a single host-controlled finalizer applying
//! the winning patch to the main workspace.
//!
//! It is **LingXi-only** and does NOT alter any claude-code parity field or
//! behavior. It plugs into the existing Rust workspace (settings, provider
//! config, llm-client, agent loop, worktree manager) rather than spawning a
//! separate service.
//!
//! Phase 2 lands the configuration / routing / state scaffolding:
//!
//! - [`config`] — strongly-typed `multiAgent` settings parser with fail-fast
//!   diagnostics.
//! - [`router`] — single-agent vs dual-LLM route decision and precedence.
//! - [`state`] — the [`state::DualLlmPhase`] state machine and run-artifact
//!   path helpers.
//! - [`prompts`] — the four phase prompt templates embedded at compile time.
//!
// TODO(multi-agent): Phases 3-7 add worktrees/artifacts I/O, candidate runner,
// review/revision/arbiter, finalizer/verification, and CLI/TUI wiring.

#![forbid(unsafe_code)]

pub mod config;
pub mod prompts;
pub mod router;
pub mod state;

pub use config::ConfigError;
pub use config::MultiAgentConfig;
pub use config::MultiAgentMode;
pub use config::MultiAgentStrategyKind;
pub use router::route;
pub use router::ExecutionRoute;
pub use router::RouteInput;
pub use state::DualLlmPhase;
pub use state::RunArtifacts;

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
//! Phase 3 lands the worktree + artifact managers:
//!
//! - [`error`] — the [`error::MultiAgentError`] taxonomy (single source of
//!   truth for run failures).
//! - [`worktrees`] — direct, non-degrading, sequential candidate-worktree
//!   provisioning over [`traits::worktree::WorktreeManager`].
//! - [`artifacts`] — fatal-on-failure run-artifact I/O and fail-closed
//!   worktree cleanup.
//!
// TODO(multi-agent): Phases 4-7 add candidate runner, review/revision/arbiter,
// finalizer/verification, and CLI/TUI wiring.

#![forbid(unsafe_code)]

pub mod artifacts;
pub mod config;
pub mod error;
pub mod prompts;
pub mod router;
pub mod state;
pub mod worktrees;

pub use artifacts::cleanup_worktree_fail_closed;
pub use artifacts::ArtifactStore;
pub use artifacts::CleanupOutcome;
pub use config::ConfigError;
pub use config::MultiAgentConfig;
pub use config::MultiAgentMode;
pub use config::MultiAgentStrategyKind;
pub use error::MultiAgentError;
pub use router::route;
pub use router::ExecutionRoute;
pub use router::RouteInput;
pub use state::DualLlmPhase;
pub use state::RunArtifacts;
pub use worktrees::candidate_slug;
pub use worktrees::new_run_id;
pub use worktrees::CandidateWorktrees;
pub use worktrees::WorktreeProvisioner;

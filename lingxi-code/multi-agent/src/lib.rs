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
//! Phase 4 lands the candidate-implementation runner:
//!
//! - [`providers`] — resolve candidate / arbiter `profile/model` references
//!   through the existing `provider-config` → `llm-client` routing (no
//!   hard-coded vendor calls).
//! - [`orchestrator`] — the [`orchestrator::DualLlm`] run loop: build brief,
//!   create worktrees, run both candidates concurrently (each cwd = its own
//!   worktree), with timeout/cancel handling and a [`orchestrator::UsageSink`]
//!   cost/telemetry seam. The real candidate loop is a marked
//!   `// TODO(multi-agent):` seam ([`orchestrator::TodoLlmCandidateRunner`]).
//!
//! Phase 5 lands cross-review / revision / arbitration:
//!
//! - [`review`] — read-only cross-review via [`sidequery::SideQueryClient`]
//!   (empty tool set ⇒ reviewer cannot write), bounded by
//!   [`config::ReviewConfig::max_review_rounds`] (single source of truth); a
//!   review timeout writes a `*.error.md` marker and the run continues.
//! - [`revision`] — author-only revision: each author revises ONLY its own
//!   worktree ([`revision::Reviser`] seam); a revision timeout/error keeps the
//!   pre-revision patch.
//! - [`arbiter`] — single-shot structured arbitration with a 1-retry then
//!   deterministic-fallback ladder; `reject_both` requests the user (never a
//!   silent single-agent fallback). Deterministic decisions are tagged
//!   `decision_source=deterministic_fallback`.
//!
// TODO(multi-agent): Phases 6-7 add finalizer/verification and CLI/TUI wiring.

#![forbid(unsafe_code)]

pub mod arbiter;
pub mod artifacts;
pub mod config;
pub mod error;
pub mod orchestrator;
pub mod prompts;
pub mod providers;
pub mod review;
pub mod revision;
pub mod router;
pub mod state;
pub mod worktrees;

pub use arbiter::deterministic_fallback;
pub use arbiter::Arbiter;
pub use arbiter::Arbitration;
pub use arbiter::CandidateSummary;
pub use arbiter::Decision;
pub use arbiter::DecisionSource;
pub use arbiter::Slot;
pub use artifacts::cleanup_worktree_fail_closed;
pub use artifacts::ArtifactStore;
pub use artifacts::CleanupOutcome;
pub use config::ConfigError;
pub use config::MultiAgentConfig;
pub use config::MultiAgentMode;
pub use config::MultiAgentStrategyKind;
pub use error::MultiAgentError;
pub use orchestrator::CandidateOutcome;
pub use orchestrator::CandidateResult;
pub use orchestrator::CandidateRunContext;
pub use orchestrator::CandidateRunner;
pub use orchestrator::DualLlm;
pub use orchestrator::ImplementationRun;
pub use orchestrator::NullUsageSink;
pub use orchestrator::TodoLlmCandidateRunner;
pub use orchestrator::UsageSink;
pub use providers::ModelResolver;
pub use providers::ResolvedCandidate;
pub use review::CrossReviewer;
pub use review::ReviewDocument;
pub use review::ReviewOutcome;
pub use review::ReviewTarget;
pub use review::Severity;
pub use revision::revise_author;
pub use revision::Reviser;
pub use revision::RevisionContext;
pub use revision::RevisionResult;
pub use revision::TodoLlmReviser;
pub use router::route;
pub use router::ExecutionRoute;
pub use router::RouteInput;
pub use state::DualLlmPhase;
pub use state::RunArtifacts;
pub use worktrees::candidate_slug;
pub use worktrees::new_run_id;
pub use worktrees::CandidateWorktrees;
pub use worktrees::WorktreeProvisioner;

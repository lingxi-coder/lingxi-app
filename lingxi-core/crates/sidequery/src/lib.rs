//! Side queries (stateless one-shot LLM calls) and forked agents (full agent
//! loops that share the parent's prompt cache via byte-exact reuse of
//! [`CacheSafeParams`]).
//!
//! See spec §20 (Side Query & Forked Agent) for the architecture. The two
//! primitives live in the same crate because both are "non-main" entry
//! points to the LLM and need uniform telemetry tagging via [`QuerySource`].
//!
//! - [`side_query`]: one-shot, stateless, no tools-in-loop.
//! - [`forked_agent`]: full subagent loop, byte-exact cache reuse.
//! - [`cache_safe_params`]: the shared slot the host writes after every
//!   successful main-loop turn so forks can read the latest snapshot.

#![forbid(unsafe_code)]

pub mod cache_safe_params;
pub mod forked_agent;
pub mod purposes;
pub mod side_query;

pub use purposes::QuerySource;
// Re-exports for side_query / cache_safe_params / forked_agent are added in
// later tasks of this plan as each module gains its public types.

//! Offline-first fixtures and replay metrics for the Fusion evaluation handoff.
//!
//! This module is intentionally not wired into `fusion/src/lib.rs` yet. The
//! standalone `fusion_eval` example includes it so fixture work can evolve
//! without changing the production Fusion API or introducing a live-provider
//! dependency.

pub mod fixtures;
pub mod harness;

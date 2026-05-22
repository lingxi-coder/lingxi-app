//! Parity fixtures derived from the claude-code reference. Each fixture
//! records the input events and expected effect counts for a known scenario
//! (e.g. `streaming_text`, `read_file_roundtrip`, `auto_compact_triggered`).
//!
//! M1.23 ships the scaffold only — see plan
//! `docs/superpowers/plans/2026-05-22-lingxi-core-m1-17-tests-release.md`
//! Task 3 for the full 12-scenario list. Materializing the JSON fixtures and
//! the per-scenario driver tests lands in M2.

/// Placeholder for a parity fixture loaded from disk. M2 will replace this
/// with the real recording schema (events + expected effect summary).
#[derive(Debug, Default)]
pub struct ParityFixture;

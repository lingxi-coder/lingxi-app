//! Session-memory extraction (full implementation lands in Plan 10).
//!
//! Captures durable notes from a finished session and writes them to the
//! user-memory tier. Stubbed here so downstream crates can name the type.

/// Distils a finished session into reusable memory notes.
pub struct SessionMemoryExtractor;

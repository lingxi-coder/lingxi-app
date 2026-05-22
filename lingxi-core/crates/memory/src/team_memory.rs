//! Team-memory directory watcher (full implementation lands in Plan 10).
//!
//! Reloads shared team memory files when the underlying directory
//! changes and runs a secret-scan pass before surfacing them.

/// Watches a team-memory directory for additions and edits.
pub struct TeamMemoryWatcher;

/// Placeholder for the secret scanner that gates team-memory ingestion.
///
/// Plan 10 swaps this for the real `lingxi-secret` integration.
pub struct SecretScannerStub;

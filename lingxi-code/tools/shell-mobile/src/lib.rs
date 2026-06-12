//! Mobile-only `Shell` tool crate (Android, spec r3 §Shell tool).
//!
//! The tool itself (`ShellMobileTool`) is added in Task 3. This task
//! provides the network-intent advisory tokenizer (`net_intent`), which is
//! host-testable with no platform deps.

pub mod net_intent;

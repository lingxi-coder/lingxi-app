//! Compile-time audit proc-macro for the lingxi-telemetry tengu event tree.
//!
//! This crate exposes [`tengu_event_audit`], a function-like proc-macro
//! invoked from `crates/telemetry/src/tengu/mod.rs` at crate root. The macro
//! walks `../telemetry/src/tengu/*.rs` at compile time and emits
//! [`compile_error`] if any payload struct violates the schema discipline:
//! bare `String` field, missing `#[serde(deny_unknown_fields)]`, or a
//! payload enum without `#[non_exhaustive]`.
//!
//! Task 1 ships the stub (always-succeeds); Task 12 replaces the body with
//! the real walker.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;

/// Walks the tengu event tree at compile time and rejects schema regressions.
///
/// See spec §7 line 790-792 for the contract. Invoke at the bottom of
/// `crates/telemetry/src/tengu/mod.rs` with no arguments:
///
/// ```ignore
/// lingxi_telemetry_macros::tengu_event_audit!();
/// ```
#[proc_macro]
pub fn tengu_event_audit(input: TokenStream) -> TokenStream {
    // Task-1 stub: always passes. Task 12 swaps in the real walker.
    //
    // Expand to an empty token stream so the macro is item-position
    // legal (`()` is an expression, not an item — would fail to parse
    // at module root). Task 12's real walker likewise emits an empty
    // stream on success and `compile_error!()` on failure.
    let _ = input;
    TokenStream::new()
}

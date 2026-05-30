//! Byte-exact context fork (a.k.a. fork agent) — full implementation lands in
//! Plan 08 (`SideQuery` & Forked Agent). This module exposes the public
//! surface that other crates need to refer to today (boilerplate markers and
//! the [`ForkSpawner`] type tag).

/// Placeholder for the byte-exact cache fork spawner. The production impl is
/// scheduled for Plan 08; declaring the type here lets downstream crates take
/// a stable type today.
pub struct ForkSpawner;

/// Marker that precedes the fork-mode boilerplate in a transcript. Engine
/// code uses this to detect that a span of messages was injected by the fork
/// spawner rather than by the user.
pub const FORK_BOILERPLATE_TAG: &str = "<fork-boilerplate>";

/// Prefix used in front of fork-directive messages (the instructions handed
/// to the forked agent).
pub const FORK_DIRECTIVE_PREFIX: &str = "<fork-directive>";

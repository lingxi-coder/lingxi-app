//! `tool-mobile` — the mobile-exclusive builtin tools.
//!
//! Folds the former one-crate-per-tool split (`tool-share` / `tool-camera` /
//! `tool-voice` / `tool-notification` / `tool-clipboard` / `tool-speech`) into a
//! single crate of submodules. Every tool here is pure Rust that routes to an
//! `Arc<dyn …Control>` capability carried in [`tool_api::BuiltinToolContext`]
//! (a Swift/Kotlin impl injected via UniFFI). Linked only by the `engine-mobile`
//! composition root, which calls [`register_all`].

#![forbid(unsafe_code)]

pub mod camera;
pub mod clipboard;
pub mod notification;
pub mod share;
pub mod speech;
pub mod voice;

pub use camera::CameraTool;
pub use clipboard::ClipboardTool;
pub use notification::NotificationTool;
pub use share::ShareTool;
pub use speech::SpeechTool;
pub use voice::VoiceTool;

/// Register every mobile-exclusive builtin tool into `reg`.
///
/// Replaces the former per-crate `tool_<name>::register_all` calls the
/// composition root made; the ordering matches the previous wiring.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    camera::register_all(reg, ctx.clone());
    voice::register_all(reg, ctx.clone());
    speech::register_all(reg, ctx.clone());
    notification::register_all(reg, ctx.clone());
    clipboard::register_all(reg, ctx.clone());
    share::register_all(reg, ctx);
}

//! `ComputerControl` — screen-capture + input-automation seam (M8-P10).
//!
//! Implemented natively per platform: a desktop automation backend, or
//! Swift/Kotlin via `UniFFI` on mobile (the callback-interface annotations land
//! in P12). The `tool-computer-use` tool (P11b) routes through this trait, so
//! Rust drives the host's screen + input without knowing the backend.

use async_trait::async_trait;
use thiserror::Error;

/// A captured screen image (PNG-encoded) plus its pixel dimensions.
#[derive(Debug, Clone)]
pub struct Screenshot {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// PNG-encoded image bytes.
    pub png_bytes: Vec<u8>,
}

/// Failure modes for [`ComputerControl`] operations.
#[derive(Debug, Clone, Error)]
pub enum ComputerError {
    /// The OS denied screen-recording / accessibility permission.
    #[error("computer-control permission denied: {0}")]
    PermissionDenied(String),
    /// The operation is not supported on this platform/backend.
    #[error("computer-control unsupported: {0}")]
    Unsupported(String),
    /// Any other backend failure.
    #[error("computer-control error: {0}")]
    Other(String),
}

/// Screen capture + mouse/keyboard automation.
///
/// Coordinates are in screen pixels with the origin at the top-left.
#[async_trait]
pub trait ComputerControl: Send + Sync {
    /// Capture the current screen.
    async fn screenshot(&self) -> Result<Screenshot, ComputerError>;
    /// Return the primary display size in pixels `(width, height)`.
    async fn display_size(&self) -> Result<(u32, u32), ComputerError>;
    /// Move the cursor to `(x, y)`.
    async fn mouse_move(&self, x: u32, y: u32) -> Result<(), ComputerError>;
    /// Left-click at `(x, y)`.
    async fn left_click(&self, x: u32, y: u32) -> Result<(), ComputerError>;
    /// Right-click at `(x, y)`.
    async fn right_click(&self, x: u32, y: u32) -> Result<(), ComputerError>;
    /// Double-click at `(x, y)`.
    async fn double_click(&self, x: u32, y: u32) -> Result<(), ComputerError>;
    /// Type a string of text at the current focus.
    async fn type_text(&self, text: String) -> Result<(), ComputerError>;
    /// Press a named key (e.g. `"Return"`, `"cmd+a"`).
    async fn key(&self, key: String) -> Result<(), ComputerError>;
    /// Scroll by `(dx, dy)` ticks at `(x, y)`.
    async fn scroll(&self, x: u32, y: u32, dx: i32, dy: i32) -> Result<(), ComputerError>;
}

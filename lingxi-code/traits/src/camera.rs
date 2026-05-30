//! `CameraControl` — photo capture + library picker seam (M8-P10).
//!
//! Implemented natively in Swift/Kotlin via `UniFFI` (P12) and injected into the
//! mobile `Platform`. The `tool-camera` tool (P11) routes through it so Rust
//! can request a photo without knowing the native camera API.

use async_trait::async_trait;
use thiserror::Error;

/// Which camera to use for capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraPosition {
    /// Front ("selfie") camera.
    Front,
    /// Rear camera.
    Back,
}

/// Options for a photo capture.
#[derive(Debug, Clone)]
pub struct CapturePhotoOpts {
    /// Which camera to use.
    pub position: CameraPosition,
    /// Whether to present the native edit/crop UI after capture.
    pub allow_editing: bool,
}

/// A captured (or picked) image, JPEG-encoded.
#[derive(Debug, Clone)]
pub struct CapturedImage {
    /// JPEG-encoded image bytes.
    pub jpeg_bytes: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Failure modes for [`CameraControl`] operations.
#[derive(Debug, Clone, Error)]
pub enum CameraError {
    /// The user denied camera/photo-library permission.
    #[error("camera permission denied")]
    PermissionDenied,
    /// The user cancelled the capture/picker.
    #[error("camera capture cancelled")]
    Cancelled,
    /// No camera hardware is available.
    #[error("camera device unavailable")]
    DeviceUnavailable,
    /// Any other native failure.
    #[error("camera error: {0}")]
    Other(String),
}

/// Native camera + photo-library access.
#[async_trait]
pub trait CameraControl: Send + Sync {
    /// Capture a photo with the native camera UI.
    async fn capture_photo(&self, opts: CapturePhotoOpts) -> Result<CapturedImage, CameraError>;
    /// Pick an existing image from the photo library.
    async fn pick_from_library(&self) -> Result<CapturedImage, CameraError>;
}

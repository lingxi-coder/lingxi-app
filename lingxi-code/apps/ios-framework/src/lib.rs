//! `ios-framework` (M8-P12) — the iOS UniFFI packager.
//!
//! This crate is the FFI boundary between the Rust engine and the iOS app. The
//! Swift layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`]
//! / [`traits::SharingService`] callback interfaces (see the skeletons under
//! `swift/`), hands them across as a [`PlatformImpls`] record, and Rust uses
//! them to construct an `IosPlatform` and assemble the mobile engine — so Rust
//! drives the device's native capabilities by calling *back* into Swift. That
//! bidirectional flow is the whole point of the UniFFI seam.
//!
//! ## UniFFI status (M8 vs M9)
//! The opaque-handle + record + top-level-function shape below is *exactly*
//! what UniFFI 0.27+ exports. M8 keeps it plain Rust (no `uniffi` dep) so the
//! workspace builds under `--offline`; M9 makes it a real UniFFI crate by
//! adding the dep, sprinkling `#[uniffi::export]` / `#[derive(uniffi::Record)]`
//! / `#[derive(uniffi::Error)]`, calling `uniffi::setup_scaffolding!()`, and
//! running `uniffi-bindgen generate` (config in `uniffi.toml`). No shape change.

#![forbid(unsafe_code)]

use std::sync::Arc;
use thiserror::Error;
use traits::{CameraControl, Platform, SharingService, VoiceRecorder};

/// Errors surfaced to the Swift host. Mirrors the UniFFI `[Error]` enum M9
/// declares (folded from the legacy `uniffi-bridge::EngineError`).
#[derive(Debug, Clone, Error)]
pub enum MobileEngineError {
    /// The requested session id is not registered with this engine.
    #[error("session not found")]
    NotFound,
    /// The engine is in a state that does not allow the requested operation.
    #[error("invalid state")]
    InvalidState,
    /// This entry point only works when compiled for an iOS target.
    #[error("not running on iOS")]
    NotOnIos,
    /// Catch-all for engine-internal failures (message is log-safe).
    #[error("internal: {0}")]
    Internal(String),
}

/// The foreign (Swift) capability objects + config the engine needs to build an
/// `IosPlatform`. UniFFI marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_sandbox_root` is the app container path.
///
/// (M9: `#[derive(uniffi::Record)]`.)
pub struct PlatformImpls {
    /// Swift `CameraControl` impl.
    pub camera: Arc<dyn CameraControl>,
    /// Swift `VoiceRecorder` impl.
    pub voice: Arc<dyn VoiceRecorder>,
    /// Swift `SharingService` impl.
    pub share: Arc<dyn SharingService>,
    /// The app's writable sandbox container root.
    pub app_sandbox_root: String,
}

/// Opaque engine handle the Swift side holds. Owns the assembled mobile
/// `Platform` + skill set; session lifecycle (`create_session`,
/// `send_user_message`) is stubbed in M8 and wired to the orchestrator in M9.
///
/// (M9: `#[derive(uniffi::Object)]`.)
pub struct MobileEngineHandle {
    #[allow(dead_code)] // Held for the M9 orchestrator wiring.
    platform: Arc<dyn Platform>,
    skill_count: usize,
}

impl MobileEngineHandle {
    fn new(platform: Arc<dyn Platform>) -> Self {
        let skills = engine_mobile::mobile_skill_registry();
        Self {
            platform,
            skill_count: skills.len(),
        }
    }

    /// Number of builtin mobile skills assembled (lets the Swift smoke test
    /// confirm the engine wired up). (M9: `#[uniffi::export]`.)
    #[must_use]
    pub fn skill_count(&self) -> u32 {
        self.skill_count as u32
    }

    /// Create a conversation session for `model`. Stubbed in M8.
    /// (M9: `#[uniffi::export]` — returns a `SessionRef`.)
    pub fn create_session(&self, _model: String) -> Result<u64, MobileEngineError> {
        Err(MobileEngineError::Internal(
            "session lifecycle lands in M9".into(),
        ))
    }
}

/// Top-level UniFFI constructor: build the mobile engine from the Swift-supplied
/// platform callbacks. (M9: `#[uniffi::export]`.)
///
/// On non-iOS hosts this returns [`MobileEngineError::NotOnIos`] — the
/// `IosPlatform` is only linked under `cfg(target_os = "ios")` — so the crate
/// still compiles and is exercised on the host build.
pub fn build_mobile_engine(
    impls: PlatformImpls,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};
        let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
            app_sandbox_root: std::path::PathBuf::from(impls.app_sandbox_root),
            camera: impls.camera,
            voice: impls.voice,
            share: impls.share,
        }));
        Ok(Arc::new(MobileEngineHandle::new(platform)))
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = impls;
        Err(MobileEngineError::NotOnIos)
    }
}

//! `android-aar` (M8-P12) — the Android UniFFI packager.
//!
//! The FFI boundary between the Rust engine and the Android app. The Kotlin
//! layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`] /
//! [`traits::SharingService`] callback interfaces (skeletons under `kotlin/`),
//! hands them across as a [`PlatformImpls`] record, and Rust uses them to build
//! an `AndroidPlatform` and assemble the mobile engine — Rust calls *back* into
//! Kotlin for native capabilities.
//!
//! ## UniFFI status (M8 vs M9)
//! Same as `ios-framework`: the shape below is exactly what UniFFI exports;
//! M8 keeps it plain Rust (no `uniffi` dep) for `--offline` builds, M9 adds the
//! dep + annotations + `uniffi-bindgen generate … --language kotlin`.

#![forbid(unsafe_code)]

use std::sync::Arc;
use thiserror::Error;
use traits::{CameraControl, Platform, SharingService, VoiceRecorder};

/// Errors surfaced to the Kotlin host. (M9: `#[derive(uniffi::Error)]`.)
#[derive(Debug, Clone, Error)]
pub enum MobileEngineError {
    /// The requested session id is not registered with this engine.
    #[error("session not found")]
    NotFound,
    /// The engine is in a state that does not allow the requested operation.
    #[error("invalid state")]
    InvalidState,
    /// This entry point only works when compiled for an Android target.
    #[error("not running on Android")]
    NotOnAndroid,
    /// Catch-all for engine-internal failures (message is log-safe).
    #[error("internal: {0}")]
    Internal(String),
}

/// The foreign (Kotlin) capability objects + config needed to build an
/// `AndroidPlatform`. (M9: `#[derive(uniffi::Record)]`.)
pub struct PlatformImpls {
    /// Kotlin `CameraControl` impl (CameraX).
    pub camera: Arc<dyn CameraControl>,
    /// Kotlin `VoiceRecorder` impl (MediaRecorder).
    pub voice: Arc<dyn VoiceRecorder>,
    /// Kotlin `SharingService` impl (Intent.ACTION_SEND).
    pub share: Arc<dyn SharingService>,
    /// The app's private files-dir root.
    pub app_files_root: String,
}

/// Opaque engine handle the Kotlin side holds. (M9: `#[derive(uniffi::Object)]`.)
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

    /// Number of builtin mobile skills assembled. (M9: `#[uniffi::export]`.)
    #[must_use]
    pub fn skill_count(&self) -> u32 {
        self.skill_count as u32
    }

    /// Create a conversation session for `model`. Stubbed in M8.
    pub fn create_session(&self, _model: String) -> Result<u64, MobileEngineError> {
        Err(MobileEngineError::Internal(
            "session lifecycle lands in M9".into(),
        ))
    }
}

/// Top-level UniFFI constructor. (M9: `#[uniffi::export]`.)
///
/// On non-Android hosts this returns [`MobileEngineError::NotOnAndroid`] — the
/// `AndroidPlatform` is only linked under `cfg(target_os = "android")` — so the
/// crate still compiles + is exercised on the host build.
pub fn build_mobile_engine(
    impls: PlatformImpls,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new(AndroidPlatformInputs {
            app_files_root: std::path::PathBuf::from(impls.app_files_root),
            camera: impls.camera,
            voice: impls.voice,
            share: impls.share,
        }));
        Ok(Arc::new(MobileEngineHandle::new(platform)))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = impls;
        Err(MobileEngineError::NotOnAndroid)
    }
}

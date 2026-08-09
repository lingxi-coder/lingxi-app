//! Live device-capability handles for the local-apps bridge.
//!
//! `ProfileApps` is cached process-wide while engines rebuild per connection
//! (see `local_apps_profile`). The Swift/Kotlin-backed device objects belong
//! to ONE connection's platform, so — exactly like `SharedLlm` — the broker
//! must never pin them: it reads through this cell on every bridge call, and
//! [`crate::local_apps_profile::profile_apps`] swaps the whole set on every
//! (re)build, cached hit or not. A bare `OnceLock<Arc<dyn CameraControl>>`
//! here would dispatch a fresh connection's capture into a torn-down engine's
//! Swift object.

use std::sync::{Arc, RwLock};
use traits::{CameraControl, LocationProvider, NotificationService, VoiceRecorder};

/// One connection's device handles, as read from its `Platform`. Every slot
/// is optional — a Store build without a runtime, a stub platform, or a
/// desktop host simply exposes none, and the bridge fails typed instead.
#[derive(Clone, Default)]
pub(crate) struct DeviceCapabilities {
    pub(crate) camera: Option<Arc<dyn CameraControl>>,
    pub(crate) voice: Option<Arc<dyn VoiceRecorder>>,
    pub(crate) location: Option<Arc<dyn LocationProvider>>,
    pub(crate) notifications: Option<Arc<dyn NotificationService>>,
}

/// Mirror of `SharedLlm` for device handles: read fresh on every use,
/// swapped whole on every engine (re)build.
pub(crate) struct SharedDeviceCapabilities(RwLock<DeviceCapabilities>);

impl SharedDeviceCapabilities {
    pub(crate) fn new(devices: DeviceCapabilities) -> Self {
        Self(RwLock::new(devices))
    }

    /// The current handle set. Callers read per bridge call, never cache.
    pub(crate) fn current(&self) -> DeviceCapabilities {
        self.0
            .read()
            .expect("shared device capabilities poisoned")
            .clone()
    }

    /// Swap in a fresh connection's handles.
    pub(crate) fn replace(&self, devices: DeviceCapabilities) {
        *self
            .0
            .write()
            .expect("shared device capabilities poisoned") = devices;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use traits::{CameraError, CapturePhotoOpts, CapturedImage};

    struct FakeCamera;

    #[async_trait]
    impl CameraControl for FakeCamera {
        async fn capture_photo(
            &self,
            _opts: CapturePhotoOpts,
        ) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }

        async fn pick_from_library(&self) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
    }

    /// The stale-handle fix in one assertion: after `replace`, `current`
    /// serves the NEW connection's handles — a reader that pinned the old
    /// set would keep dispatching into a dead engine's platform objects.
    #[test]
    fn replace_swaps_the_live_handle_set() {
        let shared = SharedDeviceCapabilities::new(DeviceCapabilities::default());
        assert!(shared.current().camera.is_none());

        shared.replace(DeviceCapabilities {
            camera: Some(Arc::new(FakeCamera)),
            ..DeviceCapabilities::default()
        });
        assert!(shared.current().camera.is_some());
        assert!(shared.current().voice.is_none());
    }
}

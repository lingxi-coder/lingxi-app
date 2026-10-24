//! The app library/runtime is independent of MobileEngineHandle and account login.
#[cfg(feature = "uniffi")]
pub use harness_runtime::mobile::unified_local_apps::{
    MobileLocalAppError, MobileLocalAppServiceHandle,
};
#[cfg(feature = "uniffi")]
#[uniffi::export]
pub fn build_ios_local_app_service(
    config_home: String,
    runtime: Option<std::sync::Arc<super::linux_runtime::IosMobileLinuxRuntimeHandle>>,
) -> Result<std::sync::Arc<MobileLocalAppServiceHandle>, MobileLocalAppError> {
    MobileLocalAppServiceHandle::new(
        config_home.into(),
        runtime.map(|handle| handle.runtime.clone()),
        "ios".into(),
    )
}

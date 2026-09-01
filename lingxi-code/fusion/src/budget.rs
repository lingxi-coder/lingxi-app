//! Budget reservation (PR3). PR2 is a no-op so Fusion can run tests without
//! a session reservation API.

use crate::config::FusionRuntimeConfig;
use platform_api::FusionError;

/// Preflight reservation. Hard `nano_usd` accounting lands in PR3.
///
/// # Errors
///
/// Never in PR2.
pub fn preflight(_config: &FusionRuntimeConfig) -> Result<(), FusionError> {
    Ok(())
}

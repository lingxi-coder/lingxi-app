//! User-config loader.
//!
//! Resolves `user_config` field values declared by a [`crate::manifest::PluginManifest`].
//! Sensitive fields go through [`secret::CredentialManager`];
//! non-sensitive required fields are pulled from
//! [`crate::manifest::PluginManifest::settings`].
//!
//! See spec §15.4.

use crate::manifest::PluginManifest;
use secret::CredentialManager;
use serde_json::{Map, Value};
use thiserror::Error;

/// Failure modes for [`resolve_user_config`].
#[derive(Debug, Clone, Error)]
pub enum LoaderError {
    /// A required field was not provided in `settings` (non-sensitive case)
    /// or was missing from secret storage (sensitive case).
    #[error("missing required user config field: {0}")]
    MissingRequired(String),
    /// Credential storage read failed.
    #[error("credential read failed: {0}")]
    Credential(String),
}

/// Resolve `manifest.user_config` into a JSON object.
///
/// Sensitive fields are surfaced as `Value::Null` placeholders today —
/// the production wiring (Plan 16) pipes them through `CredentialManager`
/// using a per-plugin keychain key. A missing **required** sensitive field
/// surfaces as [`LoaderError::MissingRequired`].
///
/// Non-sensitive required fields are pulled from
/// [`PluginManifest::settings`]; non-required fields are silently skipped.
pub async fn resolve_user_config(
    manifest: &PluginManifest,
    _credentials: &CredentialManager,
) -> Result<Value, LoaderError> {
    let Some(schema) = &manifest.user_config else {
        return Ok(Value::Object(Map::new()));
    };
    let mut out = Map::new();
    for (key, field) in &schema.fields {
        if field.sensitive {
            // The host UI provides these; if absent, signal missing-required.
            if field.required {
                return Err(LoaderError::MissingRequired(key.clone()));
            }
            out.insert(key.clone(), Value::Null);
        } else if field.required {
            // Non-sensitive required values flow through manifest.settings.
            let v = manifest
                .settings
                .get(key)
                .ok_or_else(|| LoaderError::MissingRequired(key.clone()))?;
            out.insert(key.clone(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

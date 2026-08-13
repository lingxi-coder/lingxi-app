//! App-scoped persisted and session capability grants.
//!
//! Only `always` decisions are written to disk. `allow once` is consumed by
//! the caller for one operation and `allow session` lives in
//! [`SessionPermissions`], so restarting the host cannot accidentally turn a
//! temporary grant into a durable one.

use crate::error::AppError;
use crate::manifest::{validate_domain, AppLayout};
use crate::types::APPS_SCHEMA_VERSION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use traits::rooted_fs::{self, AtomicWriteOptions};
use traits::FsError;

const MAX_PERMISSIONS_BYTES: u64 = 512 * 1024;

/// Agent capability that requires user authorization before mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCapability {
    /// Insert, update, or delete native records.
    DataMutation,
    /// Click, fill, navigate, or otherwise control the app `WebView`.
    UiControl,
    /// Capture a photo with the device camera.
    Camera,
    /// Pick an image from the device photo library.
    PhotoLibrary,
    /// Record audio with the device microphone.
    Microphone,
    /// Read the device's current location, once per call.
    Location,
    /// Post local notifications on the app's behalf.
    Notifications,
    /// Send side-query requests to the user's configured LLM.
    Llm,
    /// Post events into the conversation-facing app mailbox.
    AgentNotify,
}

impl AppCapability {
    /// Every serialized capability in stable catalog order.
    ///
    /// The MCP schema serializes this list through Serde, so its advertised
    /// strings cannot drift from manifest and permission decoding.
    pub const ALL: [Self; 9] = [
        Self::DataMutation,
        Self::UiControl,
        Self::Camera,
        Self::PhotoLibrary,
        Self::Microphone,
        Self::Location,
        Self::Notifications,
        Self::Llm,
        Self::AgentNotify,
    ];
}

/// User decision returned by a capability prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Authorize exactly the pending operation.
    AllowOnce,
    /// Authorize subsequent matching operations until the host session ends.
    AllowSession,
    /// Persist the grant for this app until explicitly revoked.
    AlwaysAllow,
    /// Deny the pending operation without persisting a denial.
    Deny,
}

/// Durable per-app grants stored in `permissions.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPermissions {
    /// Persisted local-app schema version.
    pub schema_version: u32,
    /// Capabilities the user chose to always allow.
    #[serde(default)]
    pub always_allowed_capabilities: BTreeSet<AppCapability>,
    /// HTTPS hostnames the user chose to always allow for this app.
    #[serde(default)]
    pub always_allowed_domains: BTreeSet<String>,
}

impl Default for AppPermissions {
    fn default() -> Self {
        Self {
            schema_version: APPS_SCHEMA_VERSION,
            always_allowed_capabilities: BTreeSet::new(),
            always_allowed_domains: BTreeSet::new(),
        }
    }
}

impl AppPermissions {
    /// True when `capability` has a durable grant.
    #[must_use]
    pub fn allows(&self, capability: AppCapability) -> bool {
        self.always_allowed_capabilities.contains(&capability)
    }

    /// True when `domain` has a durable per-domain network grant.
    #[must_use]
    pub fn allows_domain(&self, domain: &str) -> bool {
        self.always_allowed_domains.contains(domain)
    }

    /// Add a durable capability grant.
    pub fn grant(&mut self, capability: AppCapability) {
        self.always_allowed_capabilities.insert(capability);
    }

    /// Revoke a durable capability grant.
    pub fn revoke(&mut self, capability: AppCapability) {
        self.always_allowed_capabilities.remove(&capability);
    }

    /// Add a durable domain grant after validating the hostname.
    pub fn grant_domain(&mut self, domain: impl Into<String>) -> Result<(), AppError> {
        let domain = domain.into();
        validate_domain(&domain)?;
        self.always_allowed_domains.insert(domain);
        Ok(())
    }

    /// Revoke a durable domain grant.
    pub fn revoke_domain(&mut self, domain: &str) {
        self.always_allowed_domains.remove(domain);
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != APPS_SCHEMA_VERSION {
            return Err(AppError::StorageCorrupt(format!(
                "permissions schemaVersion {} is unsupported (expected {APPS_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        for domain in &self.always_allowed_domains {
            validate_domain(domain).map_err(|error| {
                AppError::StorageCorrupt(format!("invalid persisted domain: {error}"))
            })?;
        }
        Ok(())
    }
}

/// In-memory grants that expire with the host session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionPermissions {
    capabilities: BTreeSet<(String, AppCapability)>,
    domains: BTreeSet<(String, String)>,
}

impl SessionPermissions {
    /// Grant `capability` to one app for this session.
    pub fn grant(&mut self, app_id: impl Into<String>, capability: AppCapability) {
        self.capabilities.insert((app_id.into(), capability));
    }

    /// Test an app-scoped session capability grant.
    #[must_use]
    pub fn allows(&self, app_id: &str, capability: AppCapability) -> bool {
        self.capabilities
            .contains(&(app_id.to_string(), capability))
    }

    /// Grant one network domain to an app for this session.
    pub fn grant_domain(
        &mut self,
        app_id: impl Into<String>,
        domain: impl Into<String>,
    ) -> Result<(), AppError> {
        let app_id = app_id.into();
        let domain = domain.into();
        crate::ids::validate_app_id(&app_id)?;
        validate_domain(&domain)?;
        self.domains.insert((app_id, domain));
        Ok(())
    }

    /// Test an app-scoped session domain grant.
    #[must_use]
    pub fn allows_domain(&self, app_id: &str, domain: &str) -> bool {
        self.domains
            .contains(&(app_id.to_string(), domain.to_string()))
    }

    /// Remove all temporary grants for one app.
    pub fn revoke_app(&mut self, app_id: &str) {
        self.capabilities.retain(|(id, _)| id != app_id);
        self.domains.retain(|(id, _)| id != app_id);
    }
}

/// Load persisted permissions, returning deny-by-default state when the file
/// has not been created yet.
pub fn load_permissions(layout: &AppLayout) -> Result<AppPermissions, AppError> {
    let body = match rooted_fs::read_to_string_limited(
        layout.root(),
        &layout.permissions_rel(),
        MAX_PERMISSIONS_BYTES,
    ) {
        Ok(body) => body,
        Err(FsError::NotFound(_)) => return Ok(AppPermissions::default()),
        Err(error) => return Err(AppError::from_fs("read app permissions", &error)),
    };
    let permissions: AppPermissions = serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("app permissions: {error}")))?;
    permissions.validate()?;
    Ok(permissions)
}

/// Atomically persist the app's durable grants.
pub fn save_permissions(layout: &AppLayout, permissions: &AppPermissions) -> Result<(), AppError> {
    permissions.validate()?;
    layout.initialize()?;
    let mut body = serde_json::to_vec_pretty(permissions)
        .map_err(|error| AppError::Io(format!("serialize app permissions: {error}")))?;
    body.push(b'\n');
    if body.len() as u64 > MAX_PERMISSIONS_BYTES {
        return Err(AppError::InvalidRequest(format!(
            "app permissions are {} bytes (limit {MAX_PERMISSIONS_BYTES})",
            body.len()
        )));
    }
    rooted_fs::atomic_write(
        layout.root(),
        &layout.permissions_rel(),
        &body,
        AtomicWriteOptions::default(),
    )
    .map_err(|error| AppError::from_fs("write app permissions", &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_permissions_round_trip_and_revoke() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        assert_eq!(
            load_permissions(&layout).unwrap(),
            AppPermissions::default()
        );

        let mut permissions = AppPermissions::default();
        permissions.grant(AppCapability::DataMutation);
        permissions.grant_domain("api.example.com").unwrap();
        save_permissions(&layout, &permissions).unwrap();
        assert_eq!(load_permissions(&layout).unwrap(), permissions);

        permissions.revoke(AppCapability::DataMutation);
        permissions.revoke_domain("api.example.com");
        assert!(!permissions.allows(AppCapability::DataMutation));
        assert!(!permissions.allows_domain("api.example.com"));
    }

    #[test]
    fn all_capability_wire_spellings_are_snake_case() {
        // These strings are the wire contract shared by plan JSON, the
        // persisted manifest, and permissions.json — pin them.
        assert_eq!(
            serde_json::to_value(AppCapability::ALL).unwrap(),
            serde_json::json!([
                "data_mutation",
                "ui_control",
                "camera",
                "photo_library",
                "microphone",
                "location",
                "notifications",
                "llm",
                "agent_notify"
            ])
        );
        for (capability, wire) in [
            (AppCapability::DataMutation, "\"data_mutation\""),
            (AppCapability::UiControl, "\"ui_control\""),
            (AppCapability::Camera, "\"camera\""),
            (AppCapability::PhotoLibrary, "\"photo_library\""),
            (AppCapability::Microphone, "\"microphone\""),
            (AppCapability::Location, "\"location\""),
            (AppCapability::Notifications, "\"notifications\""),
            (AppCapability::Llm, "\"llm\""),
            (AppCapability::AgentNotify, "\"agent_notify\""),
        ] {
            assert_eq!(serde_json::to_string(&capability).unwrap(), wire);
        }
    }

    #[test]
    fn device_capability_grants_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut permissions = AppPermissions::default();
        permissions.grant(AppCapability::Camera);
        permissions.grant(AppCapability::Llm);
        save_permissions(&layout, &permissions).unwrap();
        assert_eq!(load_permissions(&layout).unwrap(), permissions);
        permissions.revoke(AppCapability::Camera);
        assert!(!permissions.allows(AppCapability::Camera));
        assert!(permissions.allows(AppCapability::Llm));
    }

    #[test]
    fn session_grants_for_device_capabilities_are_app_scoped() {
        let mut session = SessionPermissions::default();
        session.grant("abcd1234", AppCapability::Microphone);
        assert!(session.allows("abcd1234", AppCapability::Microphone));
        assert!(!session.allows("other123", AppCapability::Microphone));
    }

    #[test]
    fn session_permissions_are_app_scoped() {
        let mut session = SessionPermissions::default();
        session.grant("abcd1234", AppCapability::UiControl);
        session.grant_domain("abcd1234", "api.example.com").unwrap();
        assert!(session.allows("abcd1234", AppCapability::UiControl));
        assert!(!session.allows("other123", AppCapability::UiControl));
        assert!(session.allows_domain("abcd1234", "api.example.com"));
        session.revoke_app("abcd1234");
        assert!(!session.allows("abcd1234", AppCapability::UiControl));
    }
}

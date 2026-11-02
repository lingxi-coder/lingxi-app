//! Project-independent, private account directory commands for the Desktop host.
use lingxi_core::host::{provider_accounts::ProviderAccountCommand, InMemorySecureStorage};
use serde::Deserialize;
use std::{io::Read, path::Path, sync::Arc};

pub struct BrokerAccountRefresher {
    pub data_root: std::path::PathBuf,
    pub credentials: std::sync::Weak<secret::CredentialManager>,
    pub sink: Arc<dyn client::adapter::ClientEventSink>,
    pub locks: tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[async_trait::async_trait]
impl secret::AccountCredentialRefresher for BrokerAccountRefresher {
    async fn refresh(
        &self,
        account: lingxi_core::host::provider_accounts::AccountMetadata,
        rejected_revision: u64,
    ) -> Result<Option<secret::AccountCredential>, secret::CredentialError> {
        let lock = self
            .locks
            .lock()
            .await
            .entry(account.account_id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        validate_current_account(&self.data_root, &account).await?;
        let credentials = self
            .credentials
            .upgrade()
            .ok_or(secret::CredentialError::Unavailable)?;
        let current = credentials
            .get_account_credential(&account.credential_ref)
            .await?;
        if let Some(current) = current {
            if current.identity_generation > account.identity_generation {
                return Err(secret::CredentialError::Unavailable);
            }
            if current.identity_generation == account.identity_generation
                && current.credential_revision > rejected_revision
            {
                return Ok(Some(current));
            }
        }
        self.sink
            .emit(
                client::protocol::events::ClientEvent::ProviderAccountRefreshRequested {
                    account_id: account.account_id.clone(),
                    identity_generation: account.identity_generation,
                    credential_revision: rejected_revision,
                },
            )
            .await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut next_directory_check =
            tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            match credentials
                .get_account_credential(&account.credential_ref)
                .await?
            {
                Some(current)
                    if current.identity_generation == account.identity_generation
                        && current.credential_revision > rejected_revision =>
                {
                    validate_current_account(&self.data_root, &account).await?;
                    return Ok(Some(current));
                }
                Some(current) if current.identity_generation > account.identity_generation => {
                    return Err(secret::CredentialError::Unavailable)
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= next_directory_check {
                validate_current_account(&self.data_root, &account).await?;
                next_directory_check =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(secret::CredentialError::RefreshFailed(
                    "Host account refresh timed out".into(),
                ));
            }
        }
    }
}

async fn validate_current_account(
    root: &Path,
    account: &lingxi_core::host::provider_accounts::AccountMetadata,
) -> Result<(), secret::CredentialError> {
    let root = root.to_path_buf();
    let account = account.clone();
    tokio::task::spawn_blocking(move || {
        let snapshot = harness_runtime::provider_accounts::load_account_snapshot(
            &root,
            &account.credential_ref.store_scope,
        )
        .map_err(|_| secret::CredentialError::Unavailable)?;
        if !snapshot.accounts.iter().any(|a| {
            a.account_id == account.account_id
                && a.identity_generation == account.identity_generation
                && a.enabled
                && a.credential_ref == account.credential_ref
        }) {
            return Err(secret::CredentialError::Unavailable);
        }
        Ok(())
    })
    .await
    .map_err(|_| secret::CredentialError::Unavailable)?
}

pub fn validate_current_snapshot(
    root: &Path,
    snapshot: &lingxi_core::host::provider_accounts::AccountSnapshot,
) -> Result<(), String> {
    let current =
        harness_runtime::provider_accounts::load_account_snapshot(root, &snapshot.store_scope)?;
    if current.config_revision != snapshot.config_revision
        || current.accounts != snapshot.accounts
        || current.provider_policies != snapshot.provider_policies
    {
        return Err("account configuration changed".into());
    }
    Ok(())
}

pub async fn validate_credential_revisions(
    credentials: &secret::CredentialManager,
    snapshot: &lingxi_core::host::provider_accounts::AccountSnapshot,
    incoming: &std::collections::BTreeMap<String, secret::AccountCredential>,
) -> Result<(), String> {
    for account in &snapshot.accounts {
        if let Some(next) = incoming.get(&account.account_id) {
            if let Some(current) = credentials
                .get_account_credential(&account.credential_ref)
                .await
                .map_err(|_| "account credential unavailable")?
            {
                if current.identity_generation > next.identity_generation
                    || (current.identity_generation == next.identity_generation
                        && current.credential_revision > next.credential_revision)
                {
                    return Err("stale account credential revision".into());
                }
                if current.identity_generation == next.identity_generation
                    && current.credential_revision == next.credential_revision
                {
                    let current_bytes = serde_json::to_vec(&current)
                        .map_err(|_| "account credential encoding failed")?;
                    let next_bytes = serde_json::to_vec(next)
                        .map_err(|_| "account credential encoding failed")?;
                    if current_bytes != next_bytes {
                        return Err("account credential revision conflict".into());
                    }
                }
            }
        }
    }
    Ok(())
}

const MAX_COMMAND_BYTES: u64 = 2 * 1024 * 1024;

/// Validate all material before changing any runtime credential cache.
pub fn validate_injection(
    snapshot: Option<&lingxi_core::host::provider_accounts::AccountSnapshot>,
    credentials: &std::collections::BTreeMap<String, secret::AccountCredential>,
) -> Result<(), String> {
    let Some(snapshot) = snapshot else {
        return if credentials.is_empty() {
            Ok(())
        } else {
            Err("account credentials require an account directory".into())
        };
    };
    provider_config::AccountRegistry::from_snapshot(snapshot.clone())?;
    if snapshot.accounts.len() > 64 || credentials.len() > 64 {
        return Err("too many provider accounts".into());
    }
    for (id, credential) in credentials {
        let account = snapshot
            .accounts
            .iter()
            .find(|a| &a.account_id == id)
            .ok_or("unknown account credential")?;
        if credential.identity_generation != account.identity_generation {
            return Err("account credential generation mismatch".into());
        }
        use lingxi_core::host::provider_accounts::AccountAuthKind;
        match (&credential.kind, account.auth_kind) {
            (secret::AccountCredentialKind::ApiKey { key }, AccountAuthKind::ApiKey)
                if valid_wire_secret(key.expose_secret()) => {}
            (
                secret::AccountCredentialKind::OAuth {
                    access_token,
                    refresh_token: None,
                    issuer,
                    ..
                },
                AccountAuthKind::OAuth,
            ) if valid_wire_secret(access_token.expose_secret())
                && issuer == "https://auth.openai.com" => {}
            _ => return Err("invalid private account credential".into()),
        }
    }
    Ok(())
}

fn valid_wire_secret(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 16384 && !value.chars().any(char::is_control)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountCommandEnvelope {
    pub request_json: String,
    #[serde(default)]
    pub metadata_only: bool,
    pub backend: String,
    pub credential_channel: String,
    #[serde(default)]
    pub app_version: Option<String>,
    /// Attribute-only Broker presence for a revision-matched model catalog read.
    #[serde(default)]
    pub credential_presence: Option<Vec<String>>,
    /// Explicit Broker-only publication after a durable credential commit.
    #[serde(default)]
    pub publish_credential_version: Option<provider_config::AccountCredentialVersion>,
    #[serde(default)]
    pub credential: Option<String>,
}

pub fn read_command(reader: impl Read) -> Result<AccountCommandEnvelope, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_COMMAND_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "account command input unavailable".to_string())?;
    if bytes.len() as u64 > MAX_COMMAND_BYTES {
        return Err("account command exceeds size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid account command envelope".into())
}

pub async fn execute(home: &Path, input: AccountCommandEnvelope) -> Result<String, String> {
    // The packaged Desktop Broker is the credential authority. This helper
    // cannot silently write a second copy to the native Rust credential store.
    if !input.metadata_only || input.backend != "desktop-broker" {
        return Err("Desktop account commands require the Broker metadata domain".into());
    }
    if !matches!(
        input.credential_channel.as_str(),
        "production" | "development"
    ) {
        return Err("invalid account credential channel".into());
    }
    let scope = harness_runtime::provider_accounts::account_store_scope(
        home,
        &format!("{}:{}", input.backend, input.credential_channel),
    );
    if let Some(version) = input.publish_credential_version {
        if !input.request_json.is_empty()
            || input.credential.is_some()
            || input.credential_presence.is_some()
            || input.app_version.is_some()
        {
            return Err("credential version publication cannot contain another operation".into());
        }
        harness_runtime::provider_accounts::publish_account_credential_version(
            home.to_path_buf(),
            scope,
            version,
        )
        .await?;
        return Ok(serde_json::json!({"published":true}).to_string());
    }
    if input.app_version.as_ref().is_some_and(|version| {
        version.is_empty() || version.len() > 128 || version.chars().any(char::is_control)
    }) {
        return Err("invalid account catalog application version".into());
    }
    let command: ProviderAccountCommand = serde_json::from_str(&input.request_json)
        .map_err(|_| "invalid account command".to_string())?;
    if input.credential_presence.is_some()
        && !matches!(
            &command.action,
            lingxi_core::host::provider_accounts::ProviderAccountAction::List
        )
    {
        return Err("credential presence is only accepted for account listing".into());
    }
    let expected_revision = command.expected_revision;
    let test_account = match &command.action {
        lingxi_core::host::provider_accounts::ProviderAccountAction::Test { account_id } => {
            Some(account_id.clone())
        }
        _ => None,
    };
    if input.credential.is_some() && test_account.is_none() {
        return Err("metadata commands do not accept authentication material".into());
    }
    let credentials = Arc::new(secret::CredentialManager::new(
        Arc::new(InMemorySecureStorage::new()),
        Arc::new(platform_posix::PosixClock::new()),
        Arc::new(platform_posix::PosixHttp::new()),
    ));
    let mut store = harness_runtime::provider_accounts::ProviderAccountStore::new(
        home.to_path_buf(),
        scope,
        credentials,
    );
    let needs_binding = input.credential_presence.is_some()
        || matches!(
            &command.action,
            lingxi_core::host::provider_accounts::ProviderAccountAction::Add { .. }
                | lingxi_core::host::provider_accounts::ProviderAccountAction::Test { .. }
        );
    let persisted = if needs_binding {
        let mut cfg = harness_runtime::desktop::DesktopConfig::default();
        cfg.lingxi_home = home.to_path_buf();
        let managed = harness_runtime::desktop::managed_settings_overlay(&cfg)
            .await
            .map_err(|_| "Managed provider settings are unavailable".to_string())?;
        let source =
            migrations::settings_update::read_settings_source(&home.join("settings.json"))?;
        let effective =
            crate::provider_settings::response(&source, managed.as_ref())?["effective"].clone();
        serde_json::from_value::<lingxi_core::settings::schema::SettingsJson>(effective)
            .map_err(|_| "Invalid Host provider settings".to_string())?
    } else {
        Default::default()
    };
    let region = match persisted.provider_region.unwrap_or_default() {
        lingxi_core::settings::ProviderRegion::International => llm_runtime::Region::International,
        lingxi_core::settings::ProviderRegion::ChinaMainland => llm_runtime::Region::ChinaMainland,
    };
    let configured = persisted.providers.unwrap_or_default();
    {
        let catalog_source = input.app_version.map(|version| {
            harness_runtime::StartupCatalog::new(
                harness_runtime::CatalogPlatform::Desktop,
                version,
                home,
            )
        });
        let inputs = provider_config::AssembleInputs {
            official_catalog: None,
            anthropic_api_base: "https://api.anthropic.com".into(),
            anthropic_models: llm_runtime::builtin_presets()
                .providers
                .into_iter()
                .find(|profile| profile.profile_name == "anthropic")
                .map(|profile| profile.models)
                .unwrap_or_default(),
            anthropic_has_api_key: false,
            anthropic_has_oauth: false,
            user_providers: configured,
            routing: None,
        };
        store = store.with_catalog(
            harness_runtime::provider_accounts::ProviderAccountCatalog::new(
                inputs,
                region,
                catalog_source,
                None,
            ),
        );
    }
    if let Some(id) = test_account {
        let snapshot =
            harness_runtime::provider_accounts::load_account_snapshot(home, store.store_scope())?;
        let account = snapshot
            .accounts
            .iter()
            .find(|a| a.account_id == id)
            .ok_or("unknown account")?;
        if snapshot.config_revision != command.expected_revision {
            return Err("account configuration changed".into());
        }
        let record = serde_json::from_str::<secret::AccountCredential>(
            &input
                .credential
                .ok_or("account credential required for test")?,
        )
        .map_err(|_| "invalid account credential")?;
        let values = std::collections::BTreeMap::from([(id, record)]);
        validate_injection(Some(&snapshot), &values)?;
        for (_, record) in values {
            store
                .credentials()
                .set_account_credential_ephemeral(&account.credential_ref, Some(record))
                .await
                .map_err(|_| "account credential unavailable")?;
        }
        let snapshot = store.execute(command, None).await?;
        return serde_json::to_string(&snapshot)
            .map_err(|_| "account snapshot encoding failed".into());
    }
    let mut snapshot = store.execute_metadata(command).await?;
    if let Some(presence) = input.credential_presence {
        if snapshot.config_revision != expected_revision {
            return Err("account configuration changed".into());
        }
        if presence.len() > 64 {
            return Err("too many account presence entries".into());
        }
        let count = presence.len();
        let presence: std::collections::BTreeSet<_> = presence.into_iter().collect();
        if presence.len() != count {
            return Err("duplicate account presence entry".into());
        }
        let accounts: Vec<_> = snapshot
            .accounts
            .iter()
            .filter(|account| presence.contains(&account.account_id))
            .cloned()
            .collect();
        let root = home.to_path_buf();
        let faults = tokio::task::spawn_blocking(move || {
            accounts
                .into_iter()
                .filter_map(|account| {
                    let reason =
                        match provider_config::read_account_credential_version(&root, &account) {
                            Ok(Some(_)) => return None,
                            Ok(None) => "credential_version_unpublished",
                            Err(_) => "credential_version_unavailable",
                        };
                    Some((account, reason))
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| "account credential version worker failed")?;
        for (account, reason) in faults {
            snapshot
                .health
                .retain(|health| health.account_id != account.account_id);
            snapshot
                .health
                .push(lingxi_core::host::provider_accounts::AccountHealth {
                account_id: account.account_id,
                identity_generation: account.identity_generation,
                status:
                    lingxi_core::host::provider_accounts::AccountHealthStatus::StorageUnavailable,
                reason: Some(reason.into()),
                cooldown_until_ms: None,
                health_revision: 0,
            });
        }
        snapshot = store.project_model_availability_with_presence(snapshot, presence)?;
    }
    serde_json::to_string(&snapshot).map_err(|_| "account snapshot encoding failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversize_and_unknown_envelopes_without_echoing_input() {
        assert_eq!(
            read_command(vec![b'x'; MAX_COMMAND_BYTES as usize + 1].as_slice())
                .err()
                .as_deref(),
            Some("account command exceeds size limit")
        );
        assert!(read_command(
            br#"{"request_json":"secret-private","backend":"desktop-broker","unknown":true}"#
                .as_slice()
        )
        .is_err());
    }

    fn snapshot() -> lingxi_core::host::provider_accounts::AccountSnapshot {
        serde_json::from_value(serde_json::json!({
            "storeScope":"test-scope", "configRevision":1, "health":[],
            "accounts":[{
                "accountId":"00000000-0000-4000-8000-000000000001",
                "providerId":"openai", "displayName":"Test account", "authKind":"oauth",
                "executionBindingId":"openai-chatgpt", "credentialRef":{
                    "storeScope":"test-scope", "secretRef":"acct.00000000-0000-4000-8000-000000000001"
                }, "enabled":true,"userPriority":0,"configRevision":1,"identityGeneration":1
            }]
        })).unwrap()
    }

    #[tokio::test]
    async fn account_binding_catalog_uses_effective_region_with_or_without_custom_definitions() {
        for custom in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let profiles = if custom {
                serde_json::json!({"custom": {
                    "type":"openai", "baseUrl":"http://127.0.0.1:9/v1",
                    "models":[{"id":"model-a"}]
                }})
            } else {
                serde_json::json!({})
            };
            let envelope = |request: serde_json::Value, region, presence| {
                std::fs::write(
                    home.path().join("settings.json"),
                    serde_json::json!({"providers":profiles,"providerRegion":region}).to_string(),
                )
                .unwrap();
                AccountCommandEnvelope {
                    request_json: request.to_string(),
                    metadata_only: true,
                    backend: "desktop-broker".into(),
                    credential_channel: "development".into(),
                    app_version: None,
                    credential_presence: presence,
                    publish_credential_version: None,
                    credential: None,
                }
            };
            let snapshot: lingxi_core::host::provider_accounts::AccountSnapshot =
                serde_json::from_str(
                    &execute(
                        home.path(),
                        envelope(
                            serde_json::json!({
                                "action":"add", "expectedRevision":0,"providerId":"zhipu",
                                "displayName":"Mainland account", "executionBindingId":"glm-coding",
                                "authKind":"api_key"
                            }),
                            lingxi_core::settings::ProviderRegion::ChinaMainland,
                            None,
                        ),
                    )
                    .await
                    .unwrap(),
                )
                .unwrap();
            let account = &snapshot.accounts[0];
            harness_runtime::provider_accounts::publish_account_credential_version(
                home.path().into(),
                snapshot.store_scope.clone(),
                provider_config::AccountCredentialVersion {
                    account_id: account.account_id.clone(),
                    identity_generation: account.identity_generation,
                    credential_revision: 1,
                },
            )
            .await
            .unwrap();
            for region in [
                lingxi_core::settings::ProviderRegion::ChinaMainland,
                lingxi_core::settings::ProviderRegion::International,
            ] {
                let listing: lingxi_core::host::provider_accounts::AccountSnapshot =
                    serde_json::from_str(
                        &execute(
                            home.path(),
                            envelope(
                                serde_json::json!({
                                    "action":"list", "expectedRevision":snapshot.config_revision
                                }),
                                region,
                                Some(vec![account.account_id.clone()]),
                            ),
                        )
                        .await
                        .unwrap(),
                    )
                    .unwrap();
                assert_eq!(
                    listing
                        .model_availability
                        .iter()
                        .any(|row| row.execution_binding_id == "glm-coding"
                            && row.eligible_account_ids.contains(&account.account_id)),
                    region == lingxi_core::settings::ProviderRegion::ChinaMainland
                );
            }
        }
    }

    #[tokio::test]
    async fn account_binding_catalog_reads_persisted_region_without_active_runtime() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("settings.json"),
            r#"{"providerRegion":"china_mainland"}"#,
        )
        .unwrap();
        let envelope = AccountCommandEnvelope {
            request_json: serde_json::json!({"action":"add","expectedRevision":0,
                "providerId":"zhipu","displayName":"Mainland account",
                "executionBindingId":"glm-coding","authKind":"api_key"})
            .to_string(),
            metadata_only: true,
            backend: "desktop-broker".into(),
            credential_channel: "development".into(),
            app_version: None,
            credential_presence: None,
            publish_credential_version: None,
            credential: None,
        };
        let snapshot: lingxi_core::host::provider_accounts::AccountSnapshot =
            serde_json::from_str(&execute(home.path(), envelope).await.unwrap()).unwrap();
        assert_eq!(snapshot.accounts[0].execution_binding_id, "glm-coding");
        assert!(read_command(br#"{"request_json":"{}","metadata_only":true,"backend":"desktop-broker","credential_channel":"development","provider_region":"unknown"}"#.as_slice()).is_err());
    }

    #[tokio::test]
    async fn account_catalog_version_rejects_invalid_values_without_echoing_input() {
        let home = tempfile::tempdir().unwrap();
        for version in [String::new(), "x".repeat(129), "private\nvalue".into()] {
            let input = AccountCommandEnvelope {
                request_json: r#"{"action":"list","expectedRevision":0}"#.into(),
                metadata_only: true,
                backend: "desktop-broker".into(),
                credential_channel: "development".into(),
                app_version: Some(version),
                credential_presence: None,
                publish_credential_version: None,
                credential: None,
            };
            assert_eq!(
                execute(home.path(), input).await.unwrap_err(),
                "invalid account catalog application version"
            );
        }
    }

    #[tokio::test]
    async fn metadata_model_projection_requires_current_presence_and_enabled_binding() {
        let home = tempfile::tempdir().unwrap();
        let profile = |enabled| {
            serde_json::json!({"custom":{"type":"openai","baseUrl":"http://127.0.0.1:9/v1","enabled":enabled,"models":[{"id":"model-a"}]}}).to_string()
        };
        let envelope = |request: serde_json::Value, enabled, presence: Option<Vec<String>>| {
            std::fs::write(
                home.path().join("settings.json"),
                format!(r#"{{"providers":{}}}"#, profile(enabled)),
            )
            .unwrap();
            AccountCommandEnvelope {
                request_json: request.to_string(),
                metadata_only: true,
                backend: "desktop-broker".into(),
                credential_channel: "development".into(),
                app_version: None,
                credential: None,
                credential_presence: presence,
                publish_credential_version: None,
            }
        };
        let added = execute(home.path(), envelope(serde_json::json!({"action":"add","expectedRevision":0,"providerId":"custom","displayName":"Test","executionBindingId":"custom","authKind":"api_key"}),false,None)).await.unwrap();
        let snapshot: lingxi_core::host::provider_accounts::AccountSnapshot =
            serde_json::from_str(&added).unwrap();
        let id = snapshot.accounts[0].account_id.clone();
        let missing: lingxi_core::host::provider_accounts::AccountSnapshot = serde_json::from_str(&execute(home.path(), envelope(serde_json::json!({"action":"list","expectedRevision":snapshot.config_revision}),true,Some(vec![id.clone()]))).await.unwrap()).unwrap();
        assert!(missing
            .model_availability
            .iter()
            .all(|row| !row.eligible_account_ids.contains(&id)));
        assert!(missing
            .health
            .iter()
            .any(|row| row.reason.as_deref() == Some("credential_version_unpublished")));
        harness_runtime::provider_accounts::publish_account_credential_version(
            home.path().into(),
            snapshot.store_scope.clone(),
            provider_config::AccountCredentialVersion {
                account_id: id.clone(),
                identity_generation: snapshot.accounts[0].identity_generation,
                credential_revision: 1,
            },
        )
        .await
        .unwrap();
        let listing =
            serde_json::json!({"action":"list","expectedRevision":snapshot.config_revision});
        let disabled: lingxi_core::host::provider_accounts::AccountSnapshot = serde_json::from_str(
            &execute(
                home.path(),
                envelope(listing.clone(), false, Some(vec![id.clone()])),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(disabled
            .model_availability
            .iter()
            .all(|row| !row.eligible_account_ids.contains(&id)));
        let enabled: lingxi_core::host::provider_accounts::AccountSnapshot = serde_json::from_str(
            &execute(home.path(), envelope(listing, true, Some(vec![id.clone()])))
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(enabled
            .model_availability
            .iter()
            .any(|row| row.model_id == "model-a" && row.unverified_account_ids.contains(&id)));
        let stale = execute(
            home.path(),
            envelope(
                serde_json::json!({"action":"list","expectedRevision":0}),
                true,
                Some(vec![id]),
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(stale, "account configuration changed");
    }

    #[tokio::test]
    async fn custom_definitions_preserve_builtin_anthropic_account_models() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("settings.json"), serde_json::json!({"providers":{"custom":{"type":"openai","baseUrl":"http://127.0.0.1:9/v1","enabled":false,"models":[{"id":"model-a"}]}}}).to_string()).unwrap();
        let envelope = |request: serde_json::Value, presence| AccountCommandEnvelope {
            request_json: request.to_string(),
            metadata_only: true,
            backend: "desktop-broker".into(),
            credential_channel: "development".into(),
            app_version: None,
            credential: None,
            credential_presence: presence,
            publish_credential_version: None,
        };
        let snapshot: lingxi_core::host::provider_accounts::AccountSnapshot = serde_json::from_str(
            &execute(
                home.path(),
                envelope(
                    serde_json::json!({
                        "action":"add", "expectedRevision":0, "providerId":"anthropic",
                        "displayName":"Builtin account", "executionBindingId":"anthropic",
                        "authKind":"api_key"
                    }),
                    None,
                ),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        let account = &snapshot.accounts[0];
        harness_runtime::provider_accounts::publish_account_credential_version(
            home.path().into(),
            snapshot.store_scope.clone(),
            provider_config::AccountCredentialVersion {
                account_id: account.account_id.clone(),
                identity_generation: account.identity_generation,
                credential_revision: 1,
            },
        )
        .await
        .unwrap();
        let listing: lingxi_core::host::provider_accounts::AccountSnapshot = serde_json::from_str(
            &execute(
                home.path(),
                envelope(
                    serde_json::json!({
                        "action":"list", "expectedRevision":snapshot.config_revision
                    }),
                    Some(vec![account.account_id.clone()]),
                ),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(listing
            .model_availability
            .iter()
            .any(|row| row.execution_binding_id == "anthropic"
                && row.eligible_account_ids.contains(&account.account_id)));
    }

    #[tokio::test]
    async fn explicit_version_publication_is_monotonic_and_does_not_mutate_account_metadata() {
        let home = tempfile::tempdir().unwrap();
        let envelope = |request_json: String, version| AccountCommandEnvelope {
            request_json,
            metadata_only: true,
            backend: "desktop-broker".into(),
            credential_channel: "development".into(),
            app_version: None,
            credential_presence: None,
            credential: None,
            publish_credential_version: version,
        };
        let snapshot: lingxi_core::host::provider_accounts::AccountSnapshot = serde_json::from_str(&execute(home.path(),envelope(serde_json::json!({"action":"add","expectedRevision":0,"providerId":"openai","displayName":"Version test","executionBindingId":"openai","authKind":"api_key"}).to_string(),None)).await.unwrap()).unwrap();
        let account = &snapshot.accounts[0];
        let path = home
            .path()
            .join("provider-accounts")
            .join(format!("{}.json", snapshot.store_scope));
        let metadata = std::fs::read(&path).unwrap();
        let version = |revision| provider_config::AccountCredentialVersion {
            account_id: account.account_id.clone(),
            identity_generation: account.identity_generation,
            credential_revision: revision,
        };
        for revision in [1, 2, 2] {
            assert_eq!(
                execute(
                    home.path(),
                    envelope(String::new(), Some(version(revision)))
                )
                .await
                .unwrap(),
                "{\"published\":true}"
            );
        }
        assert_eq!(
            provider_config::read_account_credential_version(home.path(), account).unwrap(),
            Some(2)
        );
        assert!(
            execute(home.path(), envelope(String::new(), Some(version(1))))
                .await
                .is_err()
        );
        assert!(
            execute(home.path(), envelope("{}".into(), Some(version(3))))
                .await
                .is_err()
        );
        let mut wrong = version(3);
        wrong.identity_generation += 1;
        assert!(execute(home.path(), envelope(String::new(), Some(wrong)))
            .await
            .is_err());
        assert_eq!(std::fs::read(path).unwrap(), metadata);
    }

    #[test]
    fn private_launch_rejects_refresh_tokens_and_wrong_account_generations() {
        let snapshot = snapshot();
        let id = snapshot.accounts[0].account_id.clone();
        let record = |generation, refresh| secret::AccountCredential {
            credential_revision: 1,
            identity_generation: generation,
            kind: secret::AccountCredentialKind::OAuth {
                access_token: lingxi_core::types::Secret::new("test-access".into()),
                refresh_token: refresh,
                expires_at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000),
                scopes: vec![],
                issuer: "https://auth.openai.com".into(),
                subject: None,
                organization_id: None,
                email: None,
                fedramp: false,
            },
        };
        let valid = std::collections::BTreeMap::from([(id.clone(), record(1, None))]);
        assert!(validate_injection(Some(&snapshot), &valid).is_ok());
        assert!(validate_injection(None, &valid).is_err());
        let rotated_wrong_identity =
            std::collections::BTreeMap::from([(id.clone(), record(2, None))]);
        assert!(validate_injection(Some(&snapshot), &rotated_wrong_identity).is_err());
        let refresh = std::collections::BTreeMap::from([(
            id,
            record(
                1,
                Some(lingxi_core::types::Secret::new("private-refresh".into())),
            ),
        )]);
        assert_eq!(
            validate_injection(Some(&snapshot), &refresh).unwrap_err(),
            "invalid private account credential"
        );
    }

    #[tokio::test]
    async fn rejects_secret_material_before_opening_a_store() {
        let result = execute(
            Path::new("/unused-account-test"),
            AccountCommandEnvelope {
                request_json: r#"{"action":"list","expectedRevision":0}"#.into(),
                metadata_only: true,
                backend: "desktop-broker".into(),
                credential: Some("private-value".into()),
                credential_channel: "development".into(),
                app_version: None,
                credential_presence: None,
                publish_credential_version: None,
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "metadata commands do not accept authentication material"
        );
    }

    #[tokio::test]
    async fn delayed_injection_cannot_restore_an_older_token_revision() {
        let snapshot = snapshot();
        let account = &snapshot.accounts[0];
        let credentials = secret::CredentialManager::new(
            Arc::new(InMemorySecureStorage::new()),
            Arc::new(platform_posix::PosixClock::new()),
            Arc::new(platform_posix::PosixHttp::new()),
        );
        let record = |revision, token: &str| secret::AccountCredential {
            credential_revision: revision,
            identity_generation: 1,
            kind: secret::AccountCredentialKind::ApiKey {
                key: lingxi_core::types::Secret::new(token.into()),
            },
        };
        credentials
            .set_account_credential_ephemeral(&account.credential_ref, Some(record(3, "current")))
            .await
            .unwrap();
        let delayed =
            std::collections::BTreeMap::from([(account.account_id.clone(), record(2, "old"))]);
        assert_eq!(
            validate_credential_revisions(&credentials, &snapshot, &delayed)
                .await
                .unwrap_err(),
            "stale account credential revision"
        );
        let conflict = std::collections::BTreeMap::from([(
            account.account_id.clone(),
            record(3, "different"),
        )]);
        assert_eq!(
            validate_credential_revisions(&credentials, &snapshot, &conflict)
                .await
                .unwrap_err(),
            "account credential revision conflict"
        );
        let same =
            std::collections::BTreeMap::from([(account.account_id.clone(), record(3, "current"))]);
        assert!(
            validate_credential_revisions(&credentials, &snapshot, &same)
                .await
                .is_ok()
        );
        let next =
            std::collections::BTreeMap::from([(account.account_id.clone(), record(4, "next"))]);
        assert!(
            validate_credential_revisions(&credentials, &snapshot, &next)
                .await
                .is_ok()
        );
    }

    #[test]
    fn delayed_launch_is_rejected_after_account_removal() {
        let root = tempfile::tempdir().unwrap();
        let snapshot = snapshot();
        let directory = root.path().join("provider-accounts");
        std::fs::create_dir(&directory).unwrap();
        let mut removed = snapshot.clone();
        removed.accounts.clear();
        removed.config_revision += 1;
        std::fs::write(
            directory.join("test-scope.json"),
            serde_json::to_vec(&removed).unwrap(),
        )
        .unwrap();
        assert_eq!(
            validate_current_snapshot(root.path(), &snapshot).unwrap_err(),
            "account configuration changed"
        );
        assert!(validate_current_snapshot(root.path(), &removed).is_ok());
    }

    #[tokio::test]
    async fn a_running_host_can_reinject_a_new_key_generation_without_rotating_it() {
        use secret::AccountCredentialRefresher;
        struct ReinjectSink {
            credentials: Arc<secret::CredentialManager>,
            reference: lingxi_core::host::provider_accounts::CredentialRef,
            calls: std::sync::atomic::AtomicUsize,
        }
        #[async_trait::async_trait]
        impl client::adapter::ClientEventSink for ReinjectSink {
            async fn emit(&self, event: client::protocol::events::ClientEvent) {
                if let client::protocol::events::ClientEvent::ProviderAccountRefreshRequested {
                    identity_generation,
                    credential_revision,
                    ..
                } = event
                {
                    assert_eq!((identity_generation, credential_revision), (2, 0));
                    self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    self.credentials
                        .set_account_credential_ephemeral(
                            &self.reference,
                            Some(secret::AccountCredential {
                                identity_generation: 2,
                                credential_revision: 1,
                                kind: secret::AccountCredentialKind::ApiKey {
                                    key: lingxi_core::types::Secret::new("new-test-key".into()),
                                },
                            }),
                        )
                        .await
                        .unwrap();
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut snapshot = snapshot();
        snapshot.config_revision = 2;
        snapshot.accounts[0].identity_generation = 2;
        snapshot.accounts[0].config_revision = 2;
        snapshot.accounts[0].auth_kind =
            lingxi_core::host::provider_accounts::AccountAuthKind::ApiKey;
        snapshot.accounts[0].execution_binding_id = "openai".into();
        std::fs::create_dir(dir.path().join("provider-accounts")).unwrap();
        std::fs::write(
            dir.path().join("provider-accounts/test-scope.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let credentials = Arc::new(secret::CredentialManager::new(
            Arc::new(InMemorySecureStorage::new()),
            Arc::new(platform_posix::PosixClock::new()),
            Arc::new(platform_posix::PosixHttp::new()),
        ));
        let reference = snapshot.accounts[0].credential_ref.clone();
        credentials
            .set_account_credential_ephemeral(
                &reference,
                Some(secret::AccountCredential {
                    identity_generation: 1,
                    credential_revision: 9,
                    kind: secret::AccountCredentialKind::ApiKey {
                        key: lingxi_core::types::Secret::new("old-test-key".into()),
                    },
                }),
            )
            .await
            .unwrap();
        let sink = Arc::new(ReinjectSink {
            credentials: credentials.clone(),
            reference,
            calls: Default::default(),
        });
        let authority = BrokerAccountRefresher {
            data_root: dir.path().into(),
            credentials: Arc::downgrade(&credentials),
            sink: sink.clone(),
            locks: Default::default(),
        };
        let refreshed = authority
            .refresh(snapshot.accounts[0].clone(), 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (refreshed.identity_generation, refreshed.credential_revision),
            (2, 1)
        );
        assert_eq!(sink.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

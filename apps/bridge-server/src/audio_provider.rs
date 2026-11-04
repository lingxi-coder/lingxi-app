//! Bounded stdin/stdout service for host audio without booting an Agent.

use base64::{engine::general_purpose::STANDARD, Engine};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, sync::Arc};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    profiles_json: Option<String>,
    region: Option<String>,
    /// Canonical private credential envelope; nested fields remain snake_case.
    credentials: Value,
    kind: String,
    request: Value,
    audio_base64: Option<String>,
    mime_type: Option<String>,
    text: Option<String>,
}

pub async fn run() -> anyhow::Result<()> {
    const LIMIT: u64 = 17 * 1024 * 1024;
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(LIMIT + 1)
        .read_to_end(&mut input)?;
    let result = if input.len() as u64 > LIMIT {
        audio_provider::failure("invalid_request", "audio service request is too large").to_string()
    } else {
        match serde_json::from_slice::<Envelope>(&input) {
            Ok(envelope) => execute(envelope).await,
            Err(_) => audio_provider::failure("invalid_request", "invalid audio service request")
                .to_string(),
        }
    };
    println!("{result}");
    Ok(())
}

async fn execute(envelope: Envelope) -> String {
    let profiles = match envelope.profiles_json {
        Some(profiles) => profiles,
        None => {
            let cfg = crate::boot::resolve_desktop_config(&crate::boot::BridgeArgs {
                packaged_credential_stdin_only: true,
                ..Default::default()
            });
            let cfg = match cfg {
                Ok(cfg) => cfg,
                Err(_) => {
                    return audio_provider::failure(
                        "unavailable",
                        "audio provider settings are unavailable",
                    )
                    .to_string()
                }
            };
            match serde_json::to_string(&cfg.provider_profiles.unwrap_or_default()) {
                Ok(profiles) => profiles,
                Err(_) => {
                    return audio_provider::failure(
                        "unavailable",
                        "audio provider settings are unavailable",
                    )
                    .to_string()
                }
            }
        }
    };
    let region = envelope.region.unwrap_or_else(|| {
        let cfg = crate::boot::resolve_desktop_config(&crate::boot::BridgeArgs {
            packaged_credential_stdin_only: true,
            ..Default::default()
        });
        let cfg = match cfg {
            Ok(cfg) => cfg,
            Err(_) => {
                return audio_provider::failure(
                    "unavailable",
                    "audio provider settings are unavailable",
                )
                .to_string()
            }
        };
        match harness_runtime::desktop::resolve_provider_region(
            &cfg,
            &crate::settings_bridge::empty_settings_source(),
        ) {
            Ok(llm_runtime::Region::ChinaMainland) => "china_mainland".into(),
            Ok(llm_runtime::Region::International) => "international".into(),
            Err(error) => settings_failure_json(&error),
        }
    });
    let host = account_host(&profiles, &region, envelope.credentials).await;
    let host = match host {
        Ok(host) => host,
        Err(error) => return audio_provider::failure("needs_configuration", &error).to_string(),
    };
    let request = envelope.request.to_string();
    match envelope.kind.as_str() {
        "capabilities" => host.capabilities(&request).await,
        "transcribe" => {
            let Some(audio) = envelope.audio_base64 else {
                return audio_provider::failure(
                    "invalid_request",
                    "transcription input is missing",
                )
                .to_string();
            };
            if audio.len() > 16 * 1024 * 1024 {
                return audio_provider::failure(
                    "media_too_large",
                    "transcription input is too large",
                )
                .to_string();
            }
            match STANDARD.decode(audio) {
                Ok(bytes) => {
                    host.transcribe(
                        &request,
                        bytes,
                        envelope.mime_type.as_deref().unwrap_or("audio/wav"),
                    )
                    .await
                }
                Err(_) => audio_provider::failure(
                    "invalid_request",
                    "invalid transcription audio encoding",
                )
                .to_string(),
            }
        }
        "synthesize" => {
            host.synthesize(&request, envelope.text.as_deref().unwrap_or(""))
                .await
        }
        _ => audio_provider::failure("invalid_request", "unknown audio service operation")
            .to_string(),
    }
}
async fn account_host(
    profiles: &str,
    region: &str,
    input: Value,
) -> Result<audio_provider::AudioProviderHost, String> {
    let bytes = serde_json::to_vec(&input).map_err(|_| "invalid private audio credentials")?;
    let mut envelope = crate::boot::read_credential_envelope(&mut std::io::Cursor::new(bytes))
        .map_err(|_| "invalid private audio credentials")?;
    let snapshot = envelope
        .provider_accounts
        .ok_or("managed audio accounts are unavailable")?;
    let home = envelope
        .provider_account_home
        .ok_or("managed audio account storage is unavailable")?;
    if !home.is_absolute() {
        return Err("managed audio account storage is unavailable".into());
    }
    let expected = snapshot.clone();
    let root = home.clone();
    tokio::task::spawn_blocking(move || {
        crate::provider_accounts::validate_current_snapshot(&root, &expected)
    })
    .await
    .map_err(|_| "managed audio account storage is unavailable")?
    .map_err(|_| "managed audio account configuration changed")?;
    let region = match region {
        "china_mainland" => llm_runtime::Region::ChinaMainland,
        "international" => llm_runtime::Region::International,
        _ => return Err("audio requires an explicit supported usage region".into()),
    };
    let user_providers = if profiles.trim().is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_str(profiles).map_err(|_| "invalid audio provider configuration")?
    };
    let mut assembled = provider_config::assemble_for_region(
        provider_config::AssembleInputs {
            official_catalog: None,
            anthropic_api_base: "https://api.anthropic.com".into(),
            anthropic_models: vec![],
            anthropic_has_api_key: false,
            anthropic_has_oauth: false,
            user_providers,
            routing: None,
        },
        region,
    );
    if !assembled.warnings.is_empty() {
        return Err("invalid audio provider configuration".into());
    }
    let credentials = Arc::new(secret::CredentialManager::new(
        Arc::new(lingxi_core::host::InMemorySecureStorage::new()),
        Arc::new(platform_posix::PosixClock::new()),
        Arc::new(platform_posix::PosixHttp::new()),
    ));
    for account in &snapshot.accounts {
        credentials
            .set_account_credential_ephemeral(
                &account.credential_ref,
                envelope
                    .provider_account_credentials
                    .remove(&account.account_id),
            )
            .await
            .map_err(|_| "managed audio account credential unavailable")?;
    }
    let pool =
        harness_runtime::account_pool_store::assemble_pool(&mut assembled, snapshot, home.clone())
            .map_err(|_| "managed audio account configuration is unavailable")?;
    let credential_ids = assembled
        .credential_sources
        .iter()
        .map(|source| (source.profile_name.clone(), source.credential_id.clone()))
        .collect();
    let source = Arc::new(
        provider_config::MultiCredentialProvider::new(credentials)
            .with_account_version_root(home)
            .with_account_directory(pool.directory()),
    );
    let model = llm_runtime::ModelRuntime::from_config(assembled.client_config)
        .map_err(|_| "invalid managed audio provider configuration")?
        .with_credential_provider(source)
        .with_account_pool(pool);
    let transport =
        http_client::provider_transport().map_err(|_| "audio transport is unavailable")?;
    let services = model
        .provider_services(region, Arc::new(transport))
        .map_err(|_| "managed audio provider services are unavailable")?;
    Ok(audio_provider::AudioProviderHost::from_provider_services(
        services,
        credential_ids,
    ))
}

fn settings_failure_json(error: &lingxi_core::settings::SettingsError) -> String {
    let error = Utf16JsonProjection::object(vec![
        ("kind".into(), Utf16JsonProjection::from("unavailable")),
        ("message".into(), error.message_projection()),
    ])
    .expect("audio settings error has unique JSON object fields");
    Utf16JsonProjection::object(vec![("error".into(), error)])
        .expect("audio settings error has a valid response object")
        .to_json_string()
        .expect("audio settings response serializes")
}

#[cfg(test)]
mod source_error_tests {
    use super::settings_failure_json;
    use lingxi_core::settings::SettingsError;
    use lingxi_core::types::utf16_json::Utf16JsonProjection;

    #[tokio::test]
    async fn managed_audio_requires_an_account_directory() {
        let envelope: super::Envelope = serde_json::from_value(serde_json::json!({
            "profilesJson":"{}", "region":"international",
            "credentials":{}, "kind":"capabilities", "request":{
                "operationId":"fixture-op", "kind":"speech", "cloud":{"binding":"explicit_profile","profileId":"openai"}
            }
        })).unwrap();
        let output = super::execute(envelope).await;
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["error"]["kind"], "needs_configuration");
    }

    #[test]
    fn audio_outer_envelope_preserves_canonical_private_nested_field_names() {
        let envelope: super::Envelope = serde_json::from_value(serde_json::json!({
            "profilesJson":"{}", "region":"international",
            "credentials":{"provider_account_home":"/fixture-brand-home","provider_accounts":null,"provider_account_credentials":{}},
            "kind":"capabilities", "request":{}
        })).unwrap();
        assert_eq!(
            envelope.credentials["provider_account_home"],
            "/fixture-brand-home"
        );
    }

    #[test]
    fn audio_requires_canonical_credentials_and_rejects_provider_key_map() {
        assert!(serde_json::from_value::<super::Envelope>(serde_json::json!({
            "profilesJson":"{}", "region":"international", "providerKeys":{"openai":"fixture-key"},
            "kind":"capabilities", "request":{}
        })).is_err());
        assert!(
            serde_json::from_value::<super::Envelope>(serde_json::json!({
                "profilesJson":"{}", "region":"international", "credentials":{},
                "providerKeys":{"openai":"fixture-key"}, "kind":"capabilities", "request":{}
            }))
            .is_err()
        );
    }

    #[test]
    fn settings_failure_stdout_json_keeps_source_surrogate() {
        let message = Utf16JsonProjection::root_string("�".into(), vec![0xd800]).unwrap();
        let diagnostics = Utf16JsonProjection::object(Vec::new()).unwrap();
        let error = SettingsError::source_failure("managed", message, diagnostics, None);
        let json = settings_failure_json(&error);

        assert_eq!(
            json,
            r#"{"error":{"kind":"unavailable","message":"\ud800"}}"#
        );
        let parsed = Utf16JsonProjection::parse(&json).unwrap();
        assert_eq!(parsed.string_units("/error/message"), Some(vec![0xd800]));
    }
}

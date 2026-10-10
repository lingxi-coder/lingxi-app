//! Bounded stdin/stdout service for host audio without booting an Agent.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, io::Read};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    profiles_json: Option<String>,
    region: Option<String>,
    #[serde(default)]
    provider_keys: BTreeMap<String, String>,
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
        match harness_runtime::desktop::resolve_provider_region(&cfg, &BTreeMap::new()) {
            llm_runtime::Region::ChinaMainland => "china".into(),
            llm_runtime::Region::International => "international".into(),
        }
    });
    let host = match audio_provider::AudioProviderHost::new(
        &profiles,
        &region,
        None,
        envelope.provider_keys,
    ) {
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

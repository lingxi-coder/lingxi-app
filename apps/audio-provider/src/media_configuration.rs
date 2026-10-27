//! Mapping from the persisted audio configuration v4 snapshot to the shared
//! media runtime's trusted route table.

use media_api::audio::AudioOperationKind;
use media_runtime::audio::AudioVoiceScope;
use media_runtime::{AudioOperationDefaults, AudioRouteMode, AudioRoutingSettings, CloudBinding};
use serde::Deserialize;
use std::collections::BTreeMap;

const AUDIO_CONFIGURATION_VERSION: u32 = 4;
const AUDIO_LANGUAGE_AUTO: &str = "auto";
const AUDIO_MIN_RATE: f32 = 0.5;
const AUDIO_MAX_RATE: f32 = 2.0;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioConfigurationV4 {
    schema_version: u32,
    recognition: RecognitionV4,
    speech: SpeechV4,
    language: String,
    rate: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecognitionV4 {
    source: String,
    offline_model_id: Option<String>,
    cloud: CloudV4,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpeechV4 {
    source: String,
    offline_model_id: Option<String>,
    voice: Option<VoiceV4>,
    cloud: CloudV4,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceV4 {
    source: String,
    id: String,
    model_id: Option<String>,
    profile_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudV4 {
    binding: String,
    profile_id: Option<String>,
    model_id: Option<String>,
}

/// Parse the exact saved v4 snapshot and map its route and operation defaults
/// once into the app-scoped media service. Unsupported/invalid data uses fresh
/// defaults; it never attempts an older settings decoder or data migration.
#[must_use]
pub fn audio_routing_settings_from_v4_json(input: &str) -> AudioRoutingSettings {
    audio_routing_settings_from_v4_json_checked(input).unwrap_or_default()
}

/// Strict form used by the host-only settings update RPC. It never replaces a
/// working snapshot when the caller sends malformed JSON or an unsupported
/// schema version.
pub fn audio_routing_settings_from_v4_json_checked(
    input: &str,
) -> Result<AudioRoutingSettings, String> {
    let configuration = serde_json::from_str::<AudioConfigurationV4>(input)
        .map_err(|_| "audio settings are not a valid v4 configuration".to_owned())?;
    if configuration.schema_version != AUDIO_CONFIGURATION_VERSION {
        return Err("audio settings schema version is not v4".to_owned());
    }

    let recognition = route_mode(
        &configuration.recognition.source,
        configuration.recognition.cloud,
    );
    let mut operations = BTreeMap::new();
    for operation in [AudioOperationKind::Listen, AudioOperationKind::Transcribe] {
        operations.insert(operation, recognition.clone());
    }

    let speech_source = configuration.speech.source.clone();
    let speech_offline_model_id = configuration.speech.offline_model_id.clone();
    let speech = route_mode(&configuration.speech.source, configuration.speech.cloud);
    let (speech, voice_scope) = validate_voice_scope(
        speech,
        &speech_source,
        speech_offline_model_id.as_deref(),
        configuration.speech.voice.as_ref(),
    );
    for operation in [AudioOperationKind::Synthesize, AudioOperationKind::Speak] {
        operations.insert(operation, speech.clone());
    }

    let configured_language = configuration.language.trim();
    let language = (!configured_language.is_empty()
        && !configured_language.eq_ignore_ascii_case(AUDIO_LANGUAGE_AUTO))
    .then(|| configured_language.to_owned());
    let rate = configuration
        .rate
        .is_finite()
        .then(|| (configuration.rate as f32).clamp(AUDIO_MIN_RATE, AUDIO_MAX_RATE));
    let voice = configuration
        .speech
        .voice
        .as_ref()
        .filter(|voice| !voice.id.trim().is_empty())
        .map(|voice| voice.id.clone());

    let mut defaults = BTreeMap::new();
    for operation in [AudioOperationKind::Listen, AudioOperationKind::Transcribe] {
        defaults.insert(
            operation,
            AudioOperationDefaults {
                language: language.clone(),
                rate: None,
                voice: None,
                voice_scope: None,
            },
        );
    }
    for operation in [AudioOperationKind::Synthesize, AudioOperationKind::Speak] {
        defaults.insert(
            operation,
            AudioOperationDefaults {
                language: language.clone(),
                rate,
                voice: voice.clone(),
                voice_scope: voice_scope.clone(),
            },
        );
    }
    Ok(AudioRoutingSettings {
        operations,
        defaults,
    })
}

fn validate_voice_scope(
    route: AudioRouteMode,
    saved_source: &str,
    offline_model_id: Option<&str>,
    voice: Option<&VoiceV4>,
) -> (AudioRouteMode, Option<AudioVoiceScope>) {
    let Some(voice) = voice else {
        return (route, None);
    };
    if voice.id.trim().is_empty() {
        return (invalid_route("the saved speech voice ID is empty"), None);
    }

    match (&route, voice.source.as_str()) {
        (AudioRouteMode::Cloud { binding, model }, "provider") => {
            if let CloudBinding::ExplicitProfile { profile_name } = binding {
                if voice.profile_id.as_deref() != Some(profile_name.as_str()) {
                    return (
                        invalid_route(
                            "the selected provider voice does not match its saved profile",
                        ),
                        None,
                    );
                }
            }
            if model
                .as_deref()
                .is_some_and(|selected| voice.model_id.as_deref() != Some(selected))
            {
                return (
                    invalid_route("the selected provider voice does not match its saved model"),
                    None,
                );
            }
            (
                route,
                Some(AudioVoiceScope {
                    profile_name: voice.profile_id.clone(),
                    model_id: voice.model_id.clone(),
                }),
            )
        }
        (AudioRouteMode::HostConfigured, "system" | "offline") => {
            let source_matches = saved_source == "automatic" || saved_source == voice.source;
            let offline_model_matches = voice.source != "offline"
                || voice.model_id.as_deref().is_some_and(|model| {
                    offline_model_id.map_or(true, |selected| selected == model)
                });
            if source_matches && offline_model_matches {
                (route, None)
            } else {
                (
                    invalid_route(
                        "the selected local speech voice does not match its saved source or model",
                    ),
                    None,
                )
            }
        }
        _ => (
            invalid_route(
                "the selected speech voice does not match its saved source, profile, or model",
            ),
            None,
        ),
    }
}

fn route_mode(source: &str, cloud: CloudV4) -> AudioRouteMode {
    match source {
        // The app's native AudioService owns system/offline resolution and
        // installed-model discovery. Keeping these choices as one constrained
        // host route avoids a second backend registry or a cloud fallback.
        "automatic" | "system" | "offline" => AudioRouteMode::HostConfigured,
        "provider" => {
            let binding = match cloud.binding.as_str() {
                "explicit_profile" => match cloud
                    .profile_id
                    .filter(|profile| !profile.trim().is_empty())
                {
                    Some(profile_name) => CloudBinding::ExplicitProfile { profile_name },
                    None => return invalid_route("an explicit cloud audio route has no profile"),
                },
                "follow_session" => CloudBinding::FollowSession,
                _ => return invalid_route("the saved cloud audio binding is invalid"),
            };
            AudioRouteMode::Cloud {
                binding,
                model: cloud.model_id,
            }
        }
        _ => invalid_route("the saved audio source is invalid"),
    }
}

fn invalid_route(reason: &str) -> AudioRouteMode {
    AudioRouteMode::Invalid {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_route_and_operation_defaults_are_preserved() {
        let settings = audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":4,"recognition":{"source":"provider","cloud":{"binding":"explicit_profile","profileId":"audio-a","modelId":"whisper"}},"speech":{"source":"provider","voice":{"source":"provider","id":"alloy","profileId":"audio-a","modelId":"tts-1"},"cloud":{"binding":"explicit_profile","profileId":"audio-a","modelId":"tts-1"}},"language":"en-US","rate":1.25}"#,
        );
        assert!(matches!(
            settings.operations.get(&AudioOperationKind::Transcribe),
            Some(AudioRouteMode::Cloud {
                binding: CloudBinding::ExplicitProfile { profile_name },
                model: Some(model),
            }) if profile_name == "audio-a" && model == "whisper"
        ));
        let speech = settings
            .defaults
            .get(&AudioOperationKind::Speak)
            .expect("speech defaults are mapped");
        assert_eq!(speech.language.as_deref(), Some("en-US"));
        assert_eq!(speech.rate, Some(1.25));
        assert_eq!(speech.voice.as_deref(), Some("alloy"));
        assert_eq!(
            speech.voice_scope,
            Some(AudioVoiceScope {
                profile_name: Some("audio-a".into()),
                model_id: Some("tts-1".into()),
            })
        );
    }

    #[test]
    fn follow_session_provider_voice_scope_is_checked_after_binding() {
        let settings = audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":4,"recognition":{"source":"automatic","cloud":{"binding":"follow_session"}},"speech":{"source":"provider","voice":{"source":"provider","id":"custom","profileId":"profile-a","modelId":"model-a"},"cloud":{"binding":"follow_session"}},"language":"auto","rate":1.0}"#,
        );
        assert!(matches!(
            settings.operations.get(&AudioOperationKind::Speak),
            Some(AudioRouteMode::Cloud {
                binding: CloudBinding::FollowSession,
                model: None,
            })
        ));
        assert_eq!(
            settings
                .defaults
                .get(&AudioOperationKind::Speak)
                .and_then(|defaults| defaults.voice_scope.as_ref()),
            Some(&AudioVoiceScope {
                profile_name: Some("profile-a".into()),
                model_id: Some("model-a".into()),
            })
        );
    }

    #[test]
    fn automatic_local_voice_keeps_system_and_offline_source_pins() {
        let settings = audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":4,"recognition":{"source":"automatic","cloud":{"binding":"follow_session"}},"speech":{"source":"automatic","offlineModelId":"offline-speech-1","voice":{"source":"offline","id":"offline-voice","modelId":"offline-speech-1"},"cloud":{"binding":"follow_session"}},"language":"en-US","rate":1.0}"#,
        );
        assert!(matches!(
            settings.operations.get(&AudioOperationKind::Speak),
            Some(AudioRouteMode::HostConfigured)
        ));
        assert_eq!(
            settings
                .defaults
                .get(&AudioOperationKind::Speak)
                .and_then(|defaults| defaults.voice.as_deref()),
            Some("offline-voice")
        );
        assert_eq!(
            settings
                .defaults
                .get(&AudioOperationKind::Speak)
                .and_then(|defaults| defaults.voice_scope.as_ref()),
            None
        );

        let system = audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":4,"recognition":{"source":"automatic","cloud":{"binding":"follow_session"}},"speech":{"source":"automatic","voice":{"source":"system","id":"system-voice"},"cloud":{"binding":"follow_session"}},"language":"en-US","rate":1.0}"#,
        );
        assert!(matches!(
            system.operations.get(&AudioOperationKind::Speak),
            Some(AudioRouteMode::HostConfigured)
        ));
        assert_eq!(
            system
                .defaults
                .get(&AudioOperationKind::Speak)
                .and_then(|defaults| defaults.voice.as_deref()),
            Some("system-voice")
        );
    }

    #[test]
    fn automatic_provider_voice_and_invalid_cloud_binding_fail_closed() {
        let automatic_provider_voice = audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":4,"recognition":{"source":"automatic","cloud":{"binding":"follow_session"}},"speech":{"source":"automatic","voice":{"source":"provider","id":"provider-voice","profileId":"profile-a","modelId":"tts-1"},"cloud":{"binding":"follow_session"}},"language":"auto","rate":9.0}"#,
        );
        assert!(matches!(
            automatic_provider_voice
                .operations
                .get(&AudioOperationKind::Speak),
            Some(AudioRouteMode::Invalid { .. })
        ));
        assert_eq!(
            automatic_provider_voice
                .defaults
                .get(&AudioOperationKind::Speak)
                .and_then(|defaults| defaults.language.as_deref()),
            None
        );
        assert_eq!(
            automatic_provider_voice
                .defaults
                .get(&AudioOperationKind::Speak)
                .and_then(|defaults| defaults.rate),
            Some(AUDIO_MAX_RATE)
        );

        let invalid_binding = audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":4,"recognition":{"source":"provider","cloud":{"binding":"unknown"}},"speech":{"source":"automatic","cloud":{"binding":"follow_session"}},"language":"auto","rate":1.0}"#,
        );
        assert!(matches!(
            invalid_binding.operations.get(&AudioOperationKind::Listen),
            Some(AudioRouteMode::Invalid { .. })
        ));
    }

    #[test]
    fn old_or_malformed_audio_settings_use_fresh_v4_defaults() {
        assert!(audio_routing_settings_from_v4_json(
            r#"{"schemaVersion":3,"recognition":{"source":"provider","cloud":{"binding":"follow_session"}},"speech":{"source":"automatic","cloud":{"binding":"follow_session"}},"language":"auto","rate":1.0}"#
        )
        .operations
        .is_empty());
        assert!(audio_routing_settings_from_v4_json("not-json")
            .operations
            .is_empty());
    }
}

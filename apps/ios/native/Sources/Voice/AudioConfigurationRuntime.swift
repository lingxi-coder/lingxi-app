import AVFoundation
import Foundation
import Speech

/// `Locale.identifier` uses underscores on some Apple releases. The generated
/// resolver consumes BCP-47 tags, so normalize only the platform-provided
/// locale input and leave an explicitly saved language untouched.
func resolveAudioLanguageForNativeDevice(
    configured: String,
    deviceLocale: String = Locale.autoupdatingCurrent.identifier
) -> String {
    resolveAudioLanguage(
        configured: configured,
        deviceLocale: deviceLocale.replacingOccurrences(of: "_", with: "-")
    )
}

/// Resolves an immutable device-local configuration snapshot at operation
/// admission. This keeps UI capture and engine callbacks on the same v4 route
/// semantics without having either surface mutate preferences as a side effect.
@MainActor
enum AudioConfigurationRuntime {
    static func snapshot() -> AudioConfigurationSnapshot {
        AudioConfigurationStore.shared.snapshot
    }

    static func route(
        kind: AudioProviderKind,
        snapshot: AudioConfigurationSnapshot,
        languageOverride: String? = nil,
        voiceOverride: String? = nil
    ) -> AudioRouteResolution {
        let language = resolveAudioLanguageForNativeDevice(
            configured: languageOverride ?? snapshot.configuration.language,
            deviceLocale: Locale.autoupdatingCurrent.identifier
        )
        let offlineModels = GeneratedVoiceModelCatalog.all.compactMap { model -> AudioOfflineModelAvailability? in
            let installed = VoiceModelFiles.modelRoot(for: model) != nil
            switch model.kind {
            case .stt:
                return AudioOfflineModelAvailability(
                    id: model.id,
                    kind: .recognition,
                    languages: model.languages,
                    installed: installed
                )
            case .tts:
                return AudioOfflineModelAvailability(
                    id: model.id,
                    kind: .speech,
                    languages: model.languages,
                    installed: installed,
                    voiceIds: model.voices.map(\.id)
                )
            default:
                return nil
            }
        }

        switch kind {
        case .recognition:
            let recognizer = SFSpeechRecognizer(locale: Locale(identifier: language))
            let readiness: AudioReadiness
            if recognizer?.isAvailable != true {
                readiness = .unavailable
            } else {
                switch SFSpeechRecognizer.authorizationStatus() {
                case .authorized: readiness = .available
                case .notDetermined: readiness = .permissionRequired
                case .denied, .restricted: readiness = .denied
                @unknown default: readiness = .unavailable
                }
            }
            return resolveAudioRoute(AudioRouteRequest(
                kind: .recognition,
                preference: snapshot.configuration.recognition,
                language: language,
                systemStatus: readiness,
                offlineModels: offlineModels,
                sessionContext: IOSAudioProviderService.shared.routeContext,
                providerCapabilities: IOSAudioProviderService.shared.routeCapabilities
            ))
        case .speech:
            let voices = AVSpeechSynthesisVoice.speechVoices()
            let readiness: AudioReadiness = voices.isEmpty
                ? (AVSpeechSynthesisVoice(language: language) == nil ? .unavailable : .available)
                : .available
            return resolveAudioRoute(AudioRouteRequest(
                kind: .speech,
                preference: speechPreferenceForCall(
                    snapshot.configuration.speech,
                    voiceOverride: voiceOverride
                ),
                language: language,
                systemStatus: readiness,
                offlineModels: offlineModels,
                systemVoiceIds: voices.map(\.identifier),
                voiceOverride: parseVoiceOverride(voiceOverride, preference: snapshot.configuration.speech),
                sessionContext: IOSAudioProviderService.shared.routeContext,
                providerCapabilities: IOSAudioProviderService.shared.routeCapabilities
            ))
        }
    }

    static func speechPreferenceForCall(
        _ saved: AudioSpeechPreference,
        voiceOverride: String?
    ) -> AudioSpeechPreference {
        guard isDefaultVoiceOverride(voiceOverride) else { return saved }
        return AudioSpeechPreference(
            source: saved.source,
            offlineModelId: saved.offlineModelId,
            cloud: saved.cloud
        )
    }

    static func parseVoiceOverride(_ value: String?, preference: AudioSpeechPreference) -> AudioVoiceSelection? {
        guard let value else { return nil }
        let normalizedValue = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if isDefaultVoiceOverride(normalizedValue) { return nil }
        return AudioVoiceSelection(source: preference.source == .offline ? .offline : .system, id: normalizedValue, modelId: preference.source == .offline ? preference.offlineModelId ?? preference.voice?.modelId : nil)
    }

    private static func isDefaultVoiceOverride(_ value: String?) -> Bool {
        guard let value else { return false }
        switch value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "default", "auto": return true
        default: return false
        }
    }
}

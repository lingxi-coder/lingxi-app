import SwiftUI

struct OfflineVoiceModelPicker: View {
    @Environment(\.locale) private var locale
    let capability: VoiceCapabilityModel
    let kind: GeneratedVoiceModelKind

    private var selection: Binding<String> {
        Binding(get: {
            let configuration = capability.configurationSnapshot.configuration
            return (kind == .stt ? configuration.recognition.offlineModelId : configuration.speech.offlineModelId) ?? ""
        }, set: { capability.setOfflineModel($0.isEmpty ? nil : $0, for: kind) })
    }

    var body: some View {
        SettingsRow(
            label: String(localized: kind == .stt ? "audio_offline_recognition_models" : "audio_offline_speech_models"),
            sub: String(localized: kind == .stt ? "audio_offline_recognition_hint" : "audio_offline_speech_hint"),
            chevron: false
        ) {
            Picker(String(localized: kind == .stt ? "audio_offline_recognition_models" : "audio_offline_speech_models"), selection: selection) {
                Text("audio_language_default_model").tag("")
                ForEach(capability.offlineModels(for: kind)) { status in
                    let name = status.model.displayName[locale.language.languageCode?.identifier ?? "en"]
                        ?? status.model.displayName["en"] ?? status.id
                    Text(name + " · " + (status.state.isReady ? String(localized: "audio_model_installed") : String(localized: "audio_model_not_installed")))
                        .tag(status.id)
                }
            }
            .labelsHidden()
            .frame(maxWidth: 190)
            .accessibilityIdentifier(kind == .stt ? "voice.offline-recognition-model" : "voice.offline-speech-model")
        }
    }
}

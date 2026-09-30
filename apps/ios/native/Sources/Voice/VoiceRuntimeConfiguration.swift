import Foundation
import Speech

struct VoicePreferencesSnapshot: Equatable, Sendable {
    static let currentSchemaVersion = 2
    static let automaticLanguage = "auto"
    static let defaultVoiceSelection = "system:default"

    let schemaVersion: Int
    let recognitionMode: VoiceRecognitionMode
    let language: String
    let voiceSelection: String
    let rate: Double
    let autoPlayReplies: Bool

    static func load(defaults: UserDefaults = .standard) -> VoicePreferencesSnapshot {
        let storedMode = defaults.string(forKey: Keys.recognitionMode)
            ?? defaults.string(forKey: Keys.legacyRecognitionMode)
        let mode: VoiceRecognitionMode
        switch storedMode {
        case VoiceRecognitionMode.automatic.rawValue:
            mode = .automatic
        case VoiceRecognitionMode.system.rawValue:
            mode = .system
        case VoiceRecognitionMode.onDevice.rawValue, "localOnly", "on-device", "ondevice":
            // Preserve the old explicit privacy choice during migration.
            mode = .onDevice
        default:
            // A fresh install has no legacy preference and should match
            // Android's system-first Automatic default.
            mode = .automatic
        }
        let language = defaults.string(forKey: Keys.language)
            ?? defaults.string(forKey: Keys.legacyLanguage)
            ?? automaticLanguage
        let storedVoice = defaults.string(forKey: Keys.voiceSelection)
        let legacyVoice = defaults.string(forKey: Keys.legacySystemVoice)
        let voiceSelection = normalizeVoiceSelection(storedVoice ?? legacyVoice)
        let rate: Double
        if defaults.object(forKey: Keys.rate) != nil {
            rate = defaults.double(forKey: Keys.rate)
        } else if defaults.object(forKey: Keys.legacyRate) != nil {
            rate = defaults.double(forKey: Keys.legacyRate)
        } else {
            rate = 1
        }
        let autoPlay = defaults.object(forKey: Keys.autoPlayReplies) != nil
            ? defaults.bool(forKey: Keys.autoPlayReplies)
            : defaults.bool(forKey: Keys.legacyAutoPlay)
        let snapshot = VoicePreferencesSnapshot(
            schemaVersion: currentSchemaVersion,
            recognitionMode: mode,
            language: language.isEmpty ? automaticLanguage : language,
            voiceSelection: voiceSelection,
            rate: min(2, max(0.5, rate)),
            autoPlayReplies: autoPlay
        )
        snapshot.persist(defaults: defaults)
        return snapshot
    }

    func persist(defaults: UserDefaults = .standard) {
        defaults.set(Self.currentSchemaVersion, forKey: Keys.schemaVersion)
        defaults.set(recognitionMode.rawValue, forKey: Keys.recognitionMode)
        defaults.set(language, forKey: Keys.language)
        defaults.set(voiceSelection, forKey: Keys.voiceSelection)
        defaults.set(rate, forKey: Keys.rate)
        defaults.set(autoPlayReplies, forKey: Keys.autoPlayReplies)
    }

    static func normalizeVoiceSelection(_ raw: String?) -> String {
        let value = raw?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if value.isEmpty || value == "default" { return defaultVoiceSelection }
        if value.hasPrefix("system:") || value.hasPrefix("sherpa:") { return value }
        return "system:\(value)"
    }

    enum Keys {
        static let schemaVersion = "voice.schemaVersion"
        static let recognitionMode = "voice.recognitionMode"
        static let language = "voice.language"
        static let voiceSelection = "voice.voiceSelection"
        static let rate = "voice.rate"
        static let autoPlayReplies = "voice.autoPlayReplies"
        static let legacyRecognitionMode = "voiceRecognitionMode"
        static let legacyLanguage = "voiceLanguage"
        static let legacySystemVoice = "systemVoiceIdentifier"
        static let legacyRate = "voiceSpeed"
        static let legacyAutoPlay = "voiceAutoPlay"
    }
}

enum VoiceRecognitionRoute: Equatable, Sendable {
    case system(languageIdentifier: String)
    case sherpa(languageIdentifier: String, modelID: String, modelDirectory: URL)
    case unavailable(String)
}

enum VoiceSpeechRoute: Equatable, Sendable {
    case system(languageIdentifier: String, voiceIdentifier: String?)
    case sherpa(
        languageIdentifier: String,
        modelID: String,
        voiceID: String,
        speakerID: Int32,
        modelDirectory: URL
    )
}

enum VoiceRuntimeResolver {
    static func systemRecognitionAvailable(
        serviceAvailable: Bool,
        authorization: SFSpeechRecognizerAuthorizationStatus
    ) -> Bool {
        guard serviceAvailable else { return false }
        return authorization != .denied && authorization != .restricted
    }

    static func effectiveLanguage(
        configured: String,
        override: String? = nil,
        locale: Locale = .autoupdatingCurrent
    ) -> String {
        VoiceCapabilityModel.resolvedRecognitionLocaleIdentifier(
            configuredLanguage: override ?? configured,
            currentLocale: locale
        )
    }

    static func recognitionRoute(
        preferences: VoicePreferencesSnapshot,
        languageOverride: String? = nil,
        systemRecognizerAvailable: Bool,
        modelRoot: (GeneratedOfflineModelEntry) -> URL? = VoiceModelFiles.modelRoot
    ) -> VoiceRecognitionRoute {
        let language = effectiveLanguage(
            configured: preferences.language,
            override: languageOverride
        )
        let sherpa = sherpaRecognitionModel(for: language)
        switch preferences.recognitionMode {
        case .system:
            guard systemRecognizerAvailable else {
                return .unavailable("The system recognizer is unavailable for \(language).")
            }
            return .system(languageIdentifier: language)
        case .onDevice:
            guard let sherpa, let root = modelRoot(sherpa) else {
                return .unavailable("A verified offline Sherpa model for \(language) is required.")
            }
            return .sherpa(languageIdentifier: language, modelID: sherpa.id, modelDirectory: root)
        case .automatic:
            if systemRecognizerAvailable {
                return .system(languageIdentifier: language)
            }
            if let sherpa, let root = modelRoot(sherpa) {
                return .sherpa(languageIdentifier: language, modelID: sherpa.id, modelDirectory: root)
            }
            return .unavailable("No system recognizer or verified offline model is available for \(language).")
        }
    }

    static func speechRoute(
        preferences: VoicePreferencesSnapshot,
        languageOverride: String? = nil,
        voiceOverride: String? = nil,
        modelRoot: (GeneratedOfflineModelEntry) -> URL? = VoiceModelFiles.modelRoot
    ) -> VoiceSpeechRoute {
        let language = effectiveLanguage(
            configured: preferences.language,
            override: languageOverride
        )
        let selection = VoicePreferencesSnapshot.normalizeVoiceSelection(
            voiceOverride ?? preferences.voiceSelection
        )
        if let parsed = parseSherpaVoice(selection),
           let model = GeneratedVoiceModelCatalog.byID(parsed.modelID),
           model.kind == .tts,
           model.languages.contains(language.split(separator: "-").first.map(String.init)?.lowercased() ?? ""),
           let effectiveVoice = model.voices.first(where: { $0.id == parsed.voiceID })
               ?? model.voices.first(where: {
                   $0.language == language.split(separator: "-").first.map(String.init)?.lowercased()
               })
               ?? model.voices.first,
           let root = modelRoot(model) {
            let speakerID = Int32(model.voices.firstIndex(where: { $0.id == effectiveVoice.id }) ?? 0)
            return .sherpa(
                languageIdentifier: language,
                modelID: model.id,
                voiceID: effectiveVoice.id,
                speakerID: speakerID,
                modelDirectory: root
            )
        }
        let systemID = selection.hasPrefix("system:")
            ? String(selection.dropFirst("system:".count))
            : selection
        return .system(
            languageIdentifier: language,
            voiceIdentifier: systemID == "default" || systemID.isEmpty ? nil : systemID
        )
    }

    static func sherpaRecognitionModel(for languageIdentifier: String) -> GeneratedOfflineModelEntry? {
        let language = languageIdentifier.replacingOccurrences(of: "_", with: "-")
            .split(separator: "-").first.map(String.init)?.lowercased() ?? ""
        return GeneratedVoiceModelCatalog.packFor(language).first { $0.kind == .stt }
    }

    static func parseSherpaVoice(_ selection: String) -> (modelID: String, voiceID: String)? {
        guard selection.hasPrefix("sherpa:") else { return nil }
        let payload = String(selection.dropFirst("sherpa:".count))
        guard let separator = payload.lastIndex(of: ":") else { return nil }
        let modelID = String(payload[..<separator])
        let voiceID = String(payload[payload.index(after: separator)...])
        guard !modelID.isEmpty, !voiceID.isEmpty else { return nil }
        return (modelID, voiceID)
    }
}

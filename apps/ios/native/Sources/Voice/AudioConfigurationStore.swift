import AVFoundation
import Foundation
import Observation

struct AudioConfigurationSnapshot: Equatable, Sendable {
    let configuration: AudioConfigurationV3
    let revision: UInt64
}

enum AudioConfigurationStoreError: Error, Equatable, LocalizedError {
    case revisionConflict(expected: UInt64, actual: UInt64)
    case persistenceFailure
    case revisionExhausted
    case corruptedStore

    var errorDescription: String? {
        switch self {
        case let .revisionConflict(expected, actual):
            "Audio settings changed while you were editing (expected revision \(expected), current revision \(actual))."
        case .persistenceFailure:
            "Audio settings could not be saved on this device."
        case .revisionExhausted:
            "Audio settings revision has reached its supported limit."
        case .corruptedStore:
            "The saved audio settings could not be read. The original settings were preserved."
        }
    }
}

/// Device-local, serialized v3 audio preferences with a revision attached to
/// each successful commit. Older preference keys remain available for recovery.
@Observable
@MainActor
final class AudioConfigurationStore {
    static let shared = AudioConfigurationStore()

    static let storageKey = "voice.audioConfiguration.v3"
    static let didChangeNotification = Notification.Name("LingxiAudioConfigurationStoreDidChange")
    private static let maximumSafeRevision: UInt64 = 9_007_199_254_740_991

    private struct PersistedValue: Codable {
        let configuration: AudioConfigurationV3
        let revision: UInt64
        let migrationComplete: Bool
    }

    private let defaults: UserDefaults
    private let voiceCatalog: [AudioVoiceCatalogEntry]
    private let writeValue: (Data) -> Bool

    private(set) var configuration: AudioConfigurationV3
    private(set) var revision: UInt64 {
        didSet {
            guard oldValue != revision else { return }
            NotificationCenter.default.post(name: Self.didChangeNotification, object: self)
        }
    }
    private(set) var migrationComplete: Bool
    private(set) var lastError: AudioConfigurationStoreError?

    init(
        defaults: UserDefaults = .standard,
        voiceCatalog: [AudioVoiceCatalogEntry]? = nil,
        writeValue: ((Data) -> Bool)? = nil
    ) {
        self.defaults = defaults
        let voiceCatalog = voiceCatalog ?? Self.currentVoiceCatalog()
        self.voiceCatalog = voiceCatalog
        self.writeValue = writeValue ?? { data in
            defaults.set(data, forKey: Self.storageKey)
            return defaults.data(forKey: Self.storageKey) == data
        }

        let storedData = defaults.data(forKey: Self.storageKey)
        if let storedData,
           let stored = try? JSONDecoder().decode(PersistedValue.self, from: storedData) {
            configuration = Self.normalized(stored.configuration)
            revision = stored.revision
            migrationComplete = stored.migrationComplete
            lastError = nil
            if !stored.migrationComplete {
                do {
                    try persist(configuration, revision: revision, migrationComplete: true)
                    migrationComplete = true
                } catch {}
            }
        } else if defaults.object(forKey: Self.storageKey) != nil {
            configuration = Self.unavailableAfterCorruption()
            revision = 0
            migrationComplete = false
            lastError = .corruptedStore
        } else {
            configuration = Self.legacyConfiguration(defaults: defaults, voiceCatalog: voiceCatalog)
            revision = 0
            migrationComplete = false
            lastError = nil
            do {
                try persist(configuration, revision: revision, migrationComplete: true)
                migrationComplete = true
            } catch {}
        }
    }

    var snapshot: AudioConfigurationSnapshot {
        AudioConfigurationSnapshot(configuration: configuration, revision: revision)
    }

    @discardableResult
    func save(
        _ requested: AudioConfigurationV3,
        expectedRevision: UInt64
    ) throws -> AudioConfigurationSnapshot {
        guard expectedRevision == revision else {
            let error = AudioConfigurationStoreError.revisionConflict(
                expected: expectedRevision,
                actual: revision
            )
            lastError = error
            throw error
        }
        guard revision < Self.maximumSafeRevision else {
            lastError = .revisionExhausted
            throw AudioConfigurationStoreError.revisionExhausted
        }

        let normalized = Self.normalized(requested)
        let nextRevision = revision + 1
        try persist(normalized, revision: nextRevision, migrationComplete: true)
        configuration = normalized
        revision = nextRevision
        migrationComplete = true
        lastError = nil
        return snapshot
    }

    func reload() {
        defer { NotificationCenter.default.post(name: Self.didChangeNotification, object: self) }
        guard let data = defaults.data(forKey: Self.storageKey) else {
            if defaults.object(forKey: Self.storageKey) != nil { lastError = .corruptedStore }
            return
        }
        guard let stored = try? JSONDecoder().decode(PersistedValue.self, from: data) else {
            lastError = .corruptedStore
            return
        }
        configuration = Self.normalized(stored.configuration)
        revision = stored.revision
        migrationComplete = stored.migrationComplete
        lastError = nil
    }

    private func persist(
        _ configuration: AudioConfigurationV3,
        revision: UInt64,
        migrationComplete: Bool
    ) throws {
        let value = PersistedValue(
            configuration: configuration,
            revision: revision,
            migrationComplete: migrationComplete
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        do {
            let data = try encoder.encode(value)
            guard writeValue(data) else { throw AudioConfigurationStoreError.persistenceFailure }
        } catch let error as AudioConfigurationStoreError {
            lastError = error
            throw error
        } catch {
            lastError = .persistenceFailure
            throw AudioConfigurationStoreError.persistenceFailure
        }
    }

    private static func normalized(_ configuration: AudioConfigurationV3) -> AudioConfigurationV3 {
        let voice: Any
        if let selected = configuration.speech.voice {
            var value: [String: Any] = ["source": selected.source.rawValue, "id": selected.id]
            if let modelId = selected.modelId { value["modelId"] = modelId }
            voice = value
        } else {
            voice = NSNull()
        }
        return AudioConfigurationNormalizer.normalize([
            "schemaVersion": configuration.schemaVersion,
            "recognition": [
                "source": configuration.recognition.source.rawValue,
                "offlineModelId": configuration.recognition.offlineModelId as Any? ?? NSNull(),
            ],
            "speech": [
                "source": configuration.speech.source.rawValue,
                "offlineModelId": configuration.speech.offlineModelId as Any? ?? NSNull(),
                "voice": voice,
            ],
            "language": configuration.language,
            "rate": configuration.rate.isFinite ? configuration.rate : audioDefaultRate,
            "autoPlayReplies": configuration.autoPlayReplies,
        ])
    }

    private static func unavailableAfterCorruption() -> AudioConfigurationV3 {
        let unavailable = AudioSource(rawValue: "corrupted_store")
        return AudioConfigurationV3(
            recognition: AudioRecognitionPreference(source: unavailable),
            speech: AudioSpeechPreference(source: unavailable),
            language: audioLanguageAuto,
            rate: audioDefaultRate,
            autoPlayReplies: false
        )
    }

    private static func legacyConfiguration(
        defaults: UserDefaults,
        voiceCatalog: [AudioVoiceCatalogEntry]
    ) -> AudioConfigurationV3 {
        let keys = VoicePreferencesSnapshot.Keys.self
        let mode = defaults.string(forKey: keys.recognitionMode)
            ?? defaults.string(forKey: keys.legacyRecognitionMode)
        let voice = defaults.string(forKey: keys.voiceSelection)
            ?? defaults.string(forKey: keys.legacySystemVoice)
        let language = defaults.string(forKey: keys.language)
            ?? defaults.string(forKey: keys.legacyLanguage)
        let rate: Any = defaults.object(forKey: keys.rate)
            ?? defaults.object(forKey: keys.legacyRate)
            ?? audioDefaultRate
        let autoPlay: Any = defaults.object(forKey: keys.autoPlayReplies)
            ?? defaults.object(forKey: keys.legacyAutoPlay)
            ?? false
        let schemaVersion = defaults.object(forKey: keys.schemaVersion) ?? 0
        var legacy: [String: Any] = [
            "schemaVersion": schemaVersion,
            "rate": rate,
            "autoPlayReplies": autoPlay,
        ]
        if let mode { legacy["recognitionMode"] = mode }
        if let voice { legacy["voiceSelection"] = voice }
        if let language { legacy["language"] = language }
        return AudioConfigurationNormalizer.migrateLegacy(legacy, voiceCatalog: voiceCatalog)
    }

    private static func currentVoiceCatalog() -> [AudioVoiceCatalogEntry] {
        let system = AVSpeechSynthesisVoice.speechVoices().map { voice in
            AudioVoiceCatalogEntry(
                source: .system,
                id: voice.identifier,
                label: voice.name
            )
        }
        let offline = GeneratedVoiceModelCatalog.all
            .filter { $0.kind == .tts }
            .flatMap { model in
                model.voices.map { voice in
                    AudioVoiceCatalogEntry(
                        source: .offline,
                        id: voice.id,
                        modelId: model.id,
                        label: voice.displayName,
                        aliases: ["\(model.id):\(voice.id)"]
                    )
                }
            }
        return system + offline
    }
}

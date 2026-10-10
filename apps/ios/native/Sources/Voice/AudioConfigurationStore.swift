import Foundation
import Observation

struct AudioConfigurationSnapshot: Equatable, Sendable {
    let configuration: AudioConfigurationV4
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

/// Device-local, serialized v4 audio preferences with a revision attached to
/// each successful commit. Only the current v4 storage key is read.
@Observable
@MainActor
final class AudioConfigurationStore {
    static let shared = AudioConfigurationStore()

    static let storageKey = "voice.audioConfiguration.v4"
    static let didChangeNotification = Notification.Name("LingxiAudioConfigurationStoreDidChange")
    private static let maximumSafeRevision: UInt64 = 9_007_199_254_740_991

    private struct PersistedValue: Codable {
        let configuration: AudioConfigurationV4
        let revision: UInt64
    }

    private let defaults: UserDefaults
    private let writeValue: (Data) -> Bool

    private(set) var configuration: AudioConfigurationV4
    private(set) var revision: UInt64 {
        didSet {
            guard oldValue != revision else { return }
            NotificationCenter.default.post(name: Self.didChangeNotification, object: self)
        }
    }
    private(set) var lastError: AudioConfigurationStoreError?

    init(
        defaults: UserDefaults = .standard,
        writeValue: ((Data) -> Bool)? = nil
    ) {
        self.defaults = defaults
        self.writeValue = writeValue ?? { data in
            defaults.set(data, forKey: Self.storageKey)
            return defaults.data(forKey: Self.storageKey) == data
        }

        if let data = defaults.data(forKey: Self.storageKey),
           let stored = try? JSONDecoder().decode(PersistedValue.self, from: data),
           stored.configuration.schemaVersion == 4, stored.revision <= Self.maximumSafeRevision {
            configuration = Self.normalized(stored.configuration)
            revision = stored.revision
            lastError = nil
        } else if let data = defaults.data(forKey: Self.storageKey), Self.hasNonCurrentSchema(data) {
            configuration = AudioConfigurationNormalizer.defaults
            revision = 0
            lastError = nil
            do { try persist(configuration, revision: revision) } catch {}
        } else if defaults.object(forKey: Self.storageKey) != nil {
            configuration = Self.unavailableAfterCorruption()
            revision = 0
            lastError = .corruptedStore
        } else {
            configuration = AudioConfigurationNormalizer.defaults
            revision = 0
            lastError = nil
            do { try persist(configuration, revision: revision) } catch {}
        }
    }

    var snapshot: AudioConfigurationSnapshot {
        AudioConfigurationSnapshot(configuration: configuration, revision: revision)
    }

    @discardableResult
    func save(
        _ requested: AudioConfigurationV4,
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
        try persist(normalized, revision: nextRevision)
        configuration = normalized
        revision = nextRevision
        lastError = nil
        return snapshot
    }

    func reload() {
        defer { NotificationCenter.default.post(name: Self.didChangeNotification, object: self) }
        guard let data = defaults.data(forKey: Self.storageKey) else {
            if defaults.object(forKey: Self.storageKey) != nil { lastError = .corruptedStore }
            return
        }
        if Self.hasNonCurrentSchema(data) {
            do { try save(AudioConfigurationNormalizer.defaults, expectedRevision: revision) } catch {}
            return
        }
        guard let stored = try? JSONDecoder().decode(PersistedValue.self, from: data), stored.configuration.schemaVersion == 4, stored.revision <= Self.maximumSafeRevision else {
            lastError = .corruptedStore
            return
        }
        configuration = Self.normalized(stored.configuration)
        revision = stored.revision
        lastError = nil
    }

    private func persist(
        _ configuration: AudioConfigurationV4,
        revision: UInt64
    ) throws {
        let value = PersistedValue(
            configuration: configuration,
            revision: revision
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

    private static func hasNonCurrentSchema(_ data: Data) -> Bool {
        guard let envelope = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return false }
        return (envelope["configuration"] as? [String: Any])?["schemaVersion"] as? Int != 4
    }

    private static func normalized(_ configuration: AudioConfigurationV4) -> AudioConfigurationV4 {
        var finite = configuration
        if !finite.rate.isFinite { finite.rate = audioDefaultRate }
        guard let data = try? JSONEncoder().encode(finite),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return unavailableAfterCorruption() }
        return AudioConfigurationNormalizer.normalize(object)
    }

    private static func unavailableAfterCorruption() -> AudioConfigurationV4 {
        let unavailable = AudioSource(rawValue: "corrupted_store")
        return AudioConfigurationV4(
            recognition: AudioRecognitionPreference(source: unavailable),
            speech: AudioSpeechPreference(source: unavailable),
            language: audioLanguageAuto,
            rate: audioDefaultRate,
            autoPlayReplies: false
        )
    }

}

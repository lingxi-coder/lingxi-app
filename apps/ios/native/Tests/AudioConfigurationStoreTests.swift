import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class AudioConfigurationStoreTests: XCTestCase {
    func testFreshDefaultsIgnoreObsoleteStorageAndPreferenceKeys() throws {
        let defaults = makeDefaults()
        defaults.set(Data("{\"configuration\":{\"schemaVersion\":3},\"revision\":17}".utf8), forKey: "voice.audioConfiguration.v3")
        defaults.set("localOnly", forKey: "voiceRecognitionMode")
        defaults.set("system:old.voice", forKey: "systemVoiceIdentifier")
        let store = AudioConfigurationStore(defaults: defaults)
        XCTAssertEqual(store.configuration, AudioConfigurationNormalizer.defaults)
        XCTAssertEqual(store.revision, 0)
        let data = try XCTUnwrap(defaults.data(forKey: AudioConfigurationStore.storageKey))
        let value = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(Set(value.keys), ["configuration", "revision"])
    }

    func testNonCurrentSchemaStartsFreshV4WithoutReadingOldFieldsOrRevision() throws {
        let defaults = makeDefaults()
        let value: [String: Any] = ["configuration": ["schemaVersion": 3, "language": "ja-JP", "rate": 1.4], "revision": 17]
        defaults.set(try JSONSerialization.data(withJSONObject: value), forKey: AudioConfigurationStore.storageKey)
        let store = AudioConfigurationStore(defaults: defaults)
        XCTAssertEqual(store.configuration, AudioConfigurationNormalizer.defaults)
        XCTAssertEqual(store.revision, 0)
        XCTAssertEqual(AudioConfigurationStore(defaults: defaults).snapshot, store.snapshot)
    }

    func testFailedInitialWriteKeepsCurrentDefaultsWithoutMigrationState() {
        let store = AudioConfigurationStore(defaults: makeDefaults(), writeValue: { _ in false })
        XCTAssertEqual(store.configuration, AudioConfigurationNormalizer.defaults)
        XCTAssertEqual(store.revision, 0)
        XCTAssertEqual(store.lastError, .persistenceFailure)
    }

    func testFailedSaveDoesNotPublishConfigurationOrRevision() throws {
        var acceptsWrites = true
        let store = AudioConfigurationStore(defaults: makeDefaults(), writeValue: { _ in acceptsWrites })
        let original = store.snapshot
        var changed = original.configuration
        changed.language = "ja-JP"
        acceptsWrites = false
        XCTAssertThrowsError(try store.save(changed, expectedRevision: original.revision))
        XCTAssertEqual(store.snapshot, original)
        XCTAssertEqual(store.lastError, .persistenceFailure)
    }

    func testCorruptCurrentValueIsPreservedAndUnavailable() {
        let defaults = makeDefaults()
        let data = Data([0xFF, 0x01])
        defaults.set(data, forKey: AudioConfigurationStore.storageKey)
        let store = AudioConfigurationStore(defaults: defaults)
        XCTAssertEqual(store.lastError, .corruptedStore)
        XCTAssertEqual(store.configuration.recognition.source.rawValue, "corrupted_store")
        XCTAssertEqual(defaults.data(forKey: AudioConfigurationStore.storageKey), data)
    }
    func testV4SaveRetainsIndependentProfilesModelsAndScopedVoice() throws {
        let store = AudioConfigurationStore(defaults: makeDefaults())
        var configuration = store.configuration
        configuration.recognition = AudioRecognitionPreference(source: .provider, cloud: AudioCloudBinding(binding: "explicit_profile", profileId: "openai:a", modelId: "stt-model"))
        configuration.speech = AudioSpeechPreference(source: .provider, voice: AudioVoiceSelection(source: .provider, id: "voice-a", modelId: "tts-model", profileId: "openai:b"), cloud: AudioCloudBinding(binding: "explicit_profile", profileId: "openai:b", modelId: "tts-model"))
        configuration.conversation = AudioConversationPreference(mode: "realtime", interaction: "interruptible", cloud: AudioCloudBinding(binding: "explicit_profile", profileId: "gemini:c", modelId: "live-model"), voice: AudioVoiceSelection(source: .provider, id: "live-voice", modelId: "live-model", profileId: "gemini:c"))
        let saved = try store.save(configuration, expectedRevision: store.revision)
        XCTAssertEqual(saved.configuration, configuration)
    }

    func testConfigurationWritesAreRevisionCheckedAndPublishedOnlyAfterSave() throws {
        let defaults = makeDefaults()
        let store = AudioConfigurationStore(defaults: defaults)
        let initial = store.snapshot
        var changed = initial.configuration
        changed.language = "ja-JP"

        let saved = try store.save(changed, expectedRevision: initial.revision)
        XCTAssertEqual(saved.revision, initial.revision + 1)
        XCTAssertEqual(saved.configuration.language, "ja-JP")

        XCTAssertThrowsError(try store.save(initial.configuration, expectedRevision: initial.revision)) { error in
            XCTAssertEqual(
                error as? AudioConfigurationStoreError,
                .revisionConflict(expected: initial.revision, actual: saved.revision)
            )
        }
        XCTAssertEqual(store.snapshot, saved)
    }

    func testInvalidRateDoesNotEraseExplicitSourceModelOrVoice() throws {
        let store = AudioConfigurationStore(defaults: makeDefaults())
        var requested = store.configuration
        requested.recognition = AudioRecognitionPreference(
            source: .offline,
            offlineModelId: "sherpa.explicit-stt"
        )
        requested.speech = AudioSpeechPreference(
            source: .offline,
            offlineModelId: "sherpa.explicit-tts",
            voice: AudioVoiceSelection(
                source: .offline,
                id: "future-voice",
                modelId: "sherpa.explicit-tts"
            )
        )
        requested.rate = .nan

        let saved = try store.save(requested, expectedRevision: store.revision)

        XCTAssertEqual(saved.configuration.rate, audioDefaultRate)
        XCTAssertEqual(saved.configuration.recognition.source, .offline)
        XCTAssertEqual(saved.configuration.recognition.offlineModelId, "sherpa.explicit-stt")
        XCTAssertEqual(saved.configuration.speech.source, .offline)
        XCTAssertEqual(saved.configuration.speech.offlineModelId, "sherpa.explicit-tts")
        XCTAssertEqual(saved.configuration.speech.voice?.id, "future-voice")
    }

    private func makeDefaults() -> UserDefaults {
        let suiteName = "AudioConfigurationStoreTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return defaults
    }
}

import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class AudioConfigurationStoreTests: XCTestCase {
    func testLegacyLocalOnlyMigratesOnceAndPreservesRecoveryKeys() throws {
        let defaults = makeDefaults()
        defaults.set(2, forKey: VoicePreferencesSnapshot.Keys.schemaVersion)
        defaults.set("localOnly", forKey: VoicePreferencesSnapshot.Keys.recognitionMode)
        defaults.set("zh-CN", forKey: VoicePreferencesSnapshot.Keys.language)
        defaults.set(true, forKey: VoicePreferencesSnapshot.Keys.autoPlayReplies)

        let store = AudioConfigurationStore(defaults: defaults)

        XCTAssertTrue(store.migrationComplete)
        XCTAssertNil(store.lastError)
        XCTAssertEqual(store.configuration.recognition.source, .offline)
        XCTAssertNil(store.configuration.recognition.offlineModelId)
        XCTAssertEqual(store.configuration.speech.source, .automatic)
        XCTAssertNil(store.configuration.speech.voice)
        XCTAssertEqual(store.configuration.language, "zh-CN")
        XCTAssertTrue(store.configuration.autoPlayReplies)
        XCTAssertEqual(defaults.string(forKey: VoicePreferencesSnapshot.Keys.recognitionMode), "localOnly")

        let restored = AudioConfigurationStore(defaults: defaults)
        XCTAssertEqual(restored.snapshot, store.snapshot)
        XCTAssertEqual(restored.revision, store.revision)
    }

    func testLegacyUnknownVoiceRemainsAnExplicitUnavailableSelection() throws {
        let defaults = makeDefaults()
        defaults.set(2, forKey: VoicePreferencesSnapshot.Keys.schemaVersion)
        defaults.set("automatic", forKey: VoicePreferencesSnapshot.Keys.recognitionMode)
        defaults.set("system:missing.voice", forKey: VoicePreferencesSnapshot.Keys.voiceSelection)

        let store = AudioConfigurationStore(defaults: defaults, voiceCatalog: [])

        XCTAssertEqual(store.configuration.speech.source, .system)
        XCTAssertEqual(store.configuration.speech.voice?.source, .system)
        XCTAssertEqual(store.configuration.speech.voice?.id, "missing.voice")
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

    func testFailedInitialMigrationWriteDoesNotMarkMigrationComplete() {
        let defaults = makeDefaults()
        defaults.set(2, forKey: VoicePreferencesSnapshot.Keys.schemaVersion)
        defaults.set("on-device", forKey: VoicePreferencesSnapshot.Keys.recognitionMode)
        let store = AudioConfigurationStore(defaults: defaults, writeValue: { _ in false })

        XCTAssertFalse(store.migrationComplete)
        XCTAssertEqual(store.lastError, .persistenceFailure)
        XCTAssertEqual(store.configuration.recognition.source, .offline)
        XCTAssertNil(defaults.data(forKey: AudioConfigurationStore.storageKey))
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

    func testCorruptV3ValueIsPreservedInsteadOfRemigratingStaleLegacy() {
        let defaults = makeDefaults()
        let rawValue = Data([0xFF, 0x01, 0x02])
        defaults.set(rawValue, forKey: AudioConfigurationStore.storageKey)
        defaults.set("localOnly", forKey: VoicePreferencesSnapshot.Keys.recognitionMode)

        let store = AudioConfigurationStore(defaults: defaults)

        XCTAssertEqual(store.lastError, .corruptedStore)
        XCTAssertFalse(store.migrationComplete)
        XCTAssertEqual(store.configuration.recognition.source.rawValue, "corrupted_store")
        XCTAssertEqual(defaults.data(forKey: AudioConfigurationStore.storageKey), rawValue)
        XCTAssertEqual(defaults.string(forKey: VoicePreferencesSnapshot.Keys.recognitionMode), "localOnly")
    }

    func testUnrepresentableV3SourcesDoNotOverwriteLegacyRecoveryValues() throws {
        let defaults = makeDefaults()
        defaults.set("localOnly", forKey: VoicePreferencesSnapshot.Keys.recognitionMode)
        defaults.set("system:known.voice", forKey: VoicePreferencesSnapshot.Keys.voiceSelection)
        let store = AudioConfigurationStore(defaults: defaults)
        var requested = store.configuration
        requested.recognition = AudioRecognitionPreference(source: AudioSource(rawValue: "future-stt"))
        requested.speech = AudioSpeechPreference(source: AudioSource(rawValue: "future-tts"))

        _ = try store.save(requested, expectedRevision: store.revision)

        XCTAssertEqual(defaults.string(forKey: VoicePreferencesSnapshot.Keys.recognitionMode), "localOnly")
        XCTAssertEqual(defaults.string(forKey: VoicePreferencesSnapshot.Keys.voiceSelection), "system:known.voice")
        XCTAssertEqual(store.configuration.recognition.source.rawValue, "future-stt")
        XCTAssertEqual(store.configuration.speech.source.rawValue, "future-tts")
    }

    private func makeDefaults() -> UserDefaults {
        let suiteName = "AudioConfigurationStoreTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return defaults
    }
}

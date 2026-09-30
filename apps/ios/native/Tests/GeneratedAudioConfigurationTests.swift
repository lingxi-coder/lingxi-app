import Foundation
import XCTest
@testable import LingxiCode

final class GeneratedAudioConfigurationTests: XCTestCase {
    func testSharedAudioConfigurationFixtures() throws {
        let fixtureURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("../../../../resources/voice/audio-config-fixtures.json")
            .standardizedFileURL
        let fixtures = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(contentsOf: fixtureURL)) as? [String: Any]
        )

        for fixture in try XCTUnwrap(fixtures["normalization"] as? [[String: Any]]) {
            let actual = AudioConfigurationNormalizer.normalize(fixture["input"])
            let expected = AudioConfigurationNormalizer.normalize(fixture["expected"])
            XCTAssertEqual(actual, expected, fixture["name"] as? String ?? "normalization fixture")
        }

        for fixture in try XCTUnwrap(fixtures["migrations"] as? [[String: Any]]) {
            let actual = AudioConfigurationNormalizer.migrateLegacy(fixture["input"])
            let expected = AudioConfigurationNormalizer.normalize(fixture["expected"])
            XCTAssertEqual(actual, expected, fixture["name"] as? String ?? "migration fixture")
        }

        for fixture in try XCTUnwrap(fixtures["routes"] as? [[String: Any]]) {
            let actual = resolveRoute(from: try XCTUnwrap(fixture["input"] as? [String: Any]))
            let expected = try XCTUnwrap(fixture["expected"] as? [String: Any])
            assertRoute(actual, matches: expected, name: fixture["name"] as? String)
        }
    }

    func testSystemDefaultMigrationResolvesToProviderDefaultVoice() throws {
        let configuration = AudioConfigurationNormalizer.migrateLegacy([
            "schemaVersion": 2,
            "voiceSelection": "system:default",
        ])
        let voice = try XCTUnwrap(configuration.speech.voice)
        XCTAssertEqual(voice.source, .system)
        XCTAssertEqual(voice.id, "default")

        let route = resolveAudioRoute(AudioRouteRequest(
            kind: .speech,
            preference: configuration.speech,
            language: "en-US",
            systemStatus: .available,
            offlineModels: [],
            systemVoiceIds: []
        ))
        XCTAssertEqual(route.status, .ready)
        XCTAssertEqual(route.effective?.source, .system)
        XCTAssertNil(route.effective?.voiceId)
    }

    func testAutomaticFallbackIsLimitedToPreStartPermissionOrAvailabilityFailure() {
        XCTAssertTrue(isAudioFallbackAllowed(.permission, operationStarted: false))
        XCTAssertTrue(isAudioFallbackAllowed(.unavailable, operationStarted: false))
        XCTAssertFalse(isAudioFallbackAllowed(.permission, operationStarted: true))
        XCTAssertFalse(isAudioFallbackAllowed(.busy, operationStarted: false))
        XCTAssertFalse(isAudioFallbackAllowed(.cancelled, operationStarted: false))
        XCTAssertFalse(isAudioFallbackAllowed(.timeout, operationStarted: false))
        XCTAssertFalse(isAudioFallbackAllowed(.invalidRequest, operationStarted: false))
    }

    func testNativeLocaleIdentifiersNormalizeForAutomaticOfflineRecognition() {
        let cases = [
            (device: "en_US", expectedLanguage: "en-US", modelLanguages: ["en"]),
            (device: "zh_Hans_CN", expectedLanguage: "zh-Hans-CN", modelLanguages: ["zh"]),
        ]

        for value in cases {
            let language = resolveAudioLanguageForNativeDevice(
                configured: audioLanguageAuto,
                deviceLocale: value.device
            )
            XCTAssertEqual(language, value.expectedLanguage, value.device)

            let route = resolveAudioRoute(AudioRouteRequest(
                kind: .recognition,
                preference: AudioRecognitionPreference(source: .automatic),
                language: language,
                systemStatus: .unavailable,
                offlineModels: [AudioOfflineModelAvailability(
                    id: "installed-test-model",
                    kind: .recognition,
                    languages: value.modelLanguages,
                    installed: true
                )]
            ))
            XCTAssertEqual(route.status, .ready, value.device)
            XCTAssertEqual(route.effective?.source, .offline, value.device)
            XCTAssertEqual(route.fallbackReason, "systemUnavailable", value.device)
        }
    }

    func testNativeLocaleNormalizationPreservesExplicitConfiguredLanguage() {
        XCTAssertEqual(
            resolveAudioLanguageForNativeDevice(configured: "custom_Latn", deviceLocale: "en_US"),
            "custom_Latn"
        )
    }

    private func resolveRoute(from value: [String: Any]) -> AudioRouteResolution {
        let kind = AudioProviderKind(rawValue: value["kind"] as? String ?? "") ?? .recognition
        let preferenceValues = value["preference"] as? [String: Any] ?? [:]
        let preferenceSource = AudioSource(rawValue: preferenceValues["source"] as? String ?? "automatic")
        let modelID = preferenceValues["offlineModelId"] as? String
        let models = (value["offlineModels"] as? [[String: Any]] ?? []).map { model in
            AudioOfflineModelAvailability(
                id: model["id"] as? String ?? "",
                kind: AudioProviderKind(rawValue: model["kind"] as? String ?? "") ?? .recognition,
                languages: model["languages"] as? [String] ?? [],
                installed: model["installed"] as? Bool == true,
                voiceIds: model["voiceIds"] as? [String]
            )
        }
        let readiness = AudioReadiness(rawValue: value["systemStatus"] as? String ?? "unavailable")
            ?? .unavailable
        let voiceOverride = selection(value["voiceOverride"])
        let systemVoiceIDs = value["systemVoiceIds"] as? [String]

        if kind == .speech {
            let preference = AudioSpeechPreference(
                source: preferenceSource,
                offlineModelId: modelID,
                voice: selection(preferenceValues["voice"])
            )
            return resolveAudioRoute(AudioRouteRequest(
                kind: kind,
                preference: preference,
                language: value["language"] as? String ?? "",
                systemStatus: readiness,
                offlineModels: models,
                systemVoiceIds: systemVoiceIDs,
                voiceOverride: voiceOverride
            ))
        }

        let preference = AudioRecognitionPreference(source: preferenceSource, offlineModelId: modelID)
        return resolveAudioRoute(AudioRouteRequest(
            kind: kind,
            preference: preference,
            language: value["language"] as? String ?? "",
            systemStatus: readiness,
            offlineModels: models,
            systemVoiceIds: systemVoiceIDs,
            voiceOverride: voiceOverride
        ))
    }

    private func selection(_ value: Any?) -> AudioVoiceSelection? {
        guard let value = value as? [String: Any],
              let id = value["id"] as? String
        else { return nil }
        return AudioVoiceSelection(
            source: AudioSource(rawValue: value["source"] as? String ?? ""),
            id: id,
            modelId: value["modelId"] as? String
        )
    }

    private func assertRoute(
        _ actual: AudioRouteResolution,
        matches expected: [String: Any],
        name: String?
    ) {
        let message = name ?? "audio route fixture"
        let requested = expected["requested"] as? [String: Any] ?? [:]
        XCTAssertEqual(actual.requested.source.rawValue, requested["source"] as? String, message)
        XCTAssertEqual(actual.requested.offlineModelId, requested["offlineModelId"] as? String, message)
        XCTAssertEqual(actual.requested.voice, selection(requested["voice"]), message)

        if let expectedEffective = expected["effective"] as? [String: Any] {
            XCTAssertEqual(actual.effective?.source.rawValue, expectedEffective["source"] as? String, message)
            XCTAssertEqual(actual.effective?.modelId, expectedEffective["modelId"] as? String, message)
            XCTAssertEqual(actual.effective?.voiceId, expectedEffective["voiceId"] as? String, message)
        } else {
            XCTAssertNil(actual.effective, message)
        }

        XCTAssertEqual(actual.status.rawValue, expected["status"] as? String, message)
        XCTAssertEqual(actual.reason, expected["reason"] as? String, message)
        if let expectedFallbackReason = expected["fallbackReason"] as? String {
            XCTAssertEqual(actual.fallbackReason, expectedFallbackReason, message)
        } else {
            XCTAssertNil(actual.fallbackReason, message)
        }
    }
}

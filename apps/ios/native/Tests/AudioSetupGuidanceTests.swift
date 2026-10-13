import XCTest
@testable import LingxiCode

final class AudioSetupGuidanceTests: XCTestCase {
    @MainActor
    func testOfflineChoicesSeparateKindsAndRestoreSelections() {
        let suite = "AudioSetupGuidanceTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = VoiceCapabilityModel(defaults: defaults)
        XCTAssertTrue(model.offlineModels(for: .stt).allSatisfy { $0.model.kind == .stt })
        XCTAssertTrue(model.offlineModels(for: .tts).allSatisfy { $0.model.kind == .tts })
        model.setOfflineModel("sherpa.zipformer-zh-14m-mobile", for: .stt)
        model.setOfflineModel("sherpa.melo-zh-en", for: .tts)
        let restored = VoiceCapabilityModel(defaults: defaults)
        XCTAssertEqual(restored.configurationSnapshot.configuration.recognition.offlineModelId, "sherpa.zipformer-zh-14m-mobile")
        XCTAssertEqual(restored.configurationSnapshot.configuration.speech.offlineModelId, "sherpa.melo-zh-en")
        restored.setOfflineModel("sherpa.melo-zh-en", for: .stt)
        XCTAssertEqual(restored.configurationSnapshot.configuration.recognition.offlineModelId, "sherpa.zipformer-zh-14m-mobile")
        XCTAssertNotNil(restored.errorMessage)
    }

    func testRecoveriesNeverUseRawProviderErrors() {
        XCTAssertEqual(AudioSetupGuidance.fromError("HTTP 429 secret-token"), .quota)
        XCTAssertEqual(AudioSetupGuidance.fromError("permission_denied"), .permission)
        XCTAssertEqual(AudioSetupGuidance.fromError("unsupported provider"), .configuration)
        XCTAssertEqual(AudioSetupGuidance.fromError("model_missing"), .configuration)
        XCTAssertEqual(AudioSetupGuidance.fromError("no_speech"), .retry)
        XCTAssertNil(AudioSetupGuidance.fromError("cancelled"))
        XCTAssertFalse(AudioSetupGuidance.quota.detail.contains("secret-token"))
    }

    func testGuidanceOnlyReplacesThePanelAtTerminalRecoveryStates() {
        XCTAssertEqual(AudioSetupGuidance.forPhase(.configurationRequired, message: ""), .configuration)
        XCTAssertEqual(AudioSetupGuidance.forPhase(.failed, message: "429"), .quota)
        let phases: [VoiceInteractionPhase] = [.paused, .requestingPermission, .listening, .recognizing, .thinking, .speaking, .interrupting]
        for phase in phases {
            XCTAssertNil(AudioSetupGuidance.forPhase(phase, message: "unavailable"))
        }
    }
}

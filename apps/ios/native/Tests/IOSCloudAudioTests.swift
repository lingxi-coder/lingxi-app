import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class IOSCloudAudioTests: XCTestCase {
    func testCaptureDeadlineLeavesCompletionBudgetAndBoundsPayload() {
        XCTAssertEqual(IOSAudioService.captureDurationMs(timeoutBudgetMs: 1_000, maximumBytes: 1_000_000), 800)
        XCTAssertEqual(IOSAudioService.captureDurationMs(timeoutBudgetMs: nil, maximumBytes: 4_000), 900)
        XCTAssertEqual(IOSAudioService.captureDurationMs(timeoutBudgetMs: nil, maximumBytes: 1_000_000), 60_000)
    }

    func testCloudBindingsUseExactSessionOrExplicitProfile() {
        let service = IOSAudioProviderService()
        service.setSessionContext(sessionID: "session-a", profileID: "openai:work", accountScope: "account-a")
        XCTAssertEqual(service.profileID(for: AudioCloudBinding()), "openai:work")
        XCTAssertEqual(service.profileID(for: AudioCloudBinding(binding: "explicit_profile", profileId: "openai:personal")), "openai:personal")
        service.setSessionContext(sessionID: nil, profileID: nil)
        XCTAssertNil(service.profileID(for: AudioCloudBinding()))
        XCTAssertNil(service.profileID(for: AudioCloudBinding(binding: "invalid", profileId: "openai:work")))
    }

    func testCallbackOwnerCannotBorrowAnotherSessionsProfile() {
        let service = IOSAudioProviderService()
        service.setSessionContext(sessionID: "session-a", profileID: "openai:work", accountScope: "account-a")
        XCTAssertNoThrow(try service.validateOwner(.session(sessionID: "session-a"), binding: AudioCloudBinding()))
        XCTAssertThrowsError(try service.validateOwner(.session(sessionID: "session-b"), binding: AudioCloudBinding()))
        XCTAssertThrowsError(try service.validateOwner(.system(instanceID: "preview"), binding: AudioCloudBinding()))
        XCTAssertThrowsError(try service.validateOwner(.ui(instanceID: "preview"), binding: AudioCloudBinding()))
        for owner in [IOSAudioOwner.ui(instanceID: "preview"), .system(instanceID: "preview")] {
            XCTAssertNoThrow(try service.validateOwner(owner, binding: AudioCloudBinding(binding: "explicit_profile", profileId: "openai:work")))
        }
    }

    func testCurrentOwnersKeepInstanceAndOwnershipDomainDistinct() {
        XCTAssertNotEqual(IOSAudioOwner.ui(instanceID: "preview").stableKey,
                          IOSAudioOwner.ui(instanceID: "capture").stableKey)
        XCTAssertNotEqual(IOSAudioOwner.ui(instanceID: "preview").stableKey,
                          IOSAudioOwner.system(instanceID: "preview").stableKey)
    }

    func testCapabilitiesKeepRecognitionAndSpeechModelsIndependent() throws {
        let input = #"{"profileId":"openai:work","providerId":"openai","supported":true,"readiness":"ready","modelId":"asr-default","models":[{"id":"asr-default","voices":[]},{"id":"asr-other","voices":[]}]}"#
        let output = #"{"profileId":"openai:work","providerId":"openai","supported":false,"readiness":"unsupported","models":[],"reason":"not supported"}"#
        let recognition = try IOSAudioProviderService.parseCapability(input, kind: .recognition, profileID: nil)
        let speech = try IOSAudioProviderService.parseCapability(output, kind: .speech, profileID: nil)
        XCTAssertEqual(recognition.route.defaultModelId, "asr-default")
        XCTAssertEqual(recognition.route.modelIds, ["asr-default", "asr-other"])
        XCTAssertEqual(recognition.route.kind, .recognition)
        XCTAssertFalse(speech.route.supported)
        XCTAssertEqual(speech.reason, "not supported")
    }

    func testModelLessEndpointPreservesNullDescriptorWithoutInventingModel() throws {
        let json = #"{"profileId":"native-tts","providerId":"native","supported":true,"readiness":"ready","modelId":null,"models":[{"id":null,"voices":[{"id":"native-default"}]}]}"#
        let capability = try IOSAudioProviderService.parseCapability(json, kind: .speech, profileID: nil)
        XCTAssertNil(capability.route.defaultModelId)
        XCTAssertEqual(capability.route.modelIds.count, 1)
        XCTAssertNil(capability.route.modelIds[0])
        XCTAssertEqual(capability.voices, ["native-default"])
        let route = resolveAudioRoute(AudioRouteRequest(kind: .speech,
            preference: AudioSpeechPreference(source: .provider, cloud: AudioCloudBinding(binding: "explicit_profile", profileId: "native-tts")),
            language: "en-US", systemStatus: .unavailable, offlineModels: [], providerCapabilities: [capability.route]))
        XCTAssertEqual(route.status, .ready)
        XCTAssertNil(route.effective?.modelId)
    }
}

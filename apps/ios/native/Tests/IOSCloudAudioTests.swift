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
        XCTAssertThrowsError(try service.validateOwner(.session(sessionID: "session-a"), binding: AudioCloudBinding()))
        XCTAssertThrowsError(try service.validateOwner(.session(sessionID: "session-b"), binding: AudioCloudBinding()))
        XCTAssertThrowsError(try service.validateOwner(.system(instanceID: "preview"), binding: AudioCloudBinding()))
        XCTAssertThrowsError(try service.validateOwner(.ui(instanceID: "preview"), binding: AudioCloudBinding()))
        for owner in [IOSAudioOwner.ui(instanceID: "preview"), .system(instanceID: "preview")] {
            XCTAssertNoThrow(try service.validateOwner(owner, binding: AudioCloudBinding(binding: "explicit_profile", profileId: "openai:work")))
        }
    }

    func testFollowSessionRequiresSameTrustedAttachmentAndLogicalSession() throws {
        let attachmentA = try XCTUnwrap(IOSMediaAttachmentIdentity(
            hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 17
        ))
        let attachmentB = try XCTUnwrap(IOSMediaAttachmentIdentity(
            hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 18
        ))
        let otherHost = try XCTUnwrap(IOSMediaAttachmentIdentity(
            hostID: "d46e67b0-8adf-4b55-9fc9-93e586de4c14", generation: 17
        ))
        let context = IOSAudioSessionContext(
            sessionId: "session-a",
            profileId: "openai:work",
            accountScope: "account-a",
            engineAttachmentIdentity: attachmentA
        )
        let owner = IOSAudioOwner.attached(
            attachment: attachmentA,
            logicalOwner: .session(sessionID: "session-a")
        )

        XCTAssertTrue(IOSAudioProviderService.matchesTrustedFollowSessionOwner(
            owner: owner,
            context: context,
            engineAttachmentIdentity: attachmentA,
            engineIsAttached: true
        ))
        XCTAssertFalse(IOSAudioProviderService.matchesTrustedFollowSessionOwner(
            owner: owner,
            context: context,
            engineAttachmentIdentity: attachmentA,
            engineIsAttached: false
        ))
        XCTAssertFalse(IOSAudioProviderService.matchesTrustedFollowSessionOwner(
            owner: owner,
            context: context,
            engineAttachmentIdentity: attachmentB,
            engineIsAttached: true
        ))
        XCTAssertFalse(IOSAudioProviderService.matchesTrustedFollowSessionOwner(
            owner: owner,
            context: context,
            engineAttachmentIdentity: otherHost,
            engineIsAttached: true
        ))
        XCTAssertFalse(IOSAudioProviderService.matchesTrustedFollowSessionOwner(
            owner: .attached(attachment: attachmentA, logicalOwner: .session(sessionID: "session-b")),
            context: context,
            engineAttachmentIdentity: attachmentA,
            engineIsAttached: true
        ))
        XCTAssertFalse(IOSAudioProviderService.matchesTrustedFollowSessionOwner(
            owner: .attached(attachment: attachmentA, logicalOwner: .ui(instanceID: "session-a")),
            context: context,
            engineAttachmentIdentity: attachmentA,
            engineIsAttached: true
        ))
    }

    func testTrustedEnginePinRequiresMatchingHostRouteSessionBinding() throws {
        let hostID = "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f"
        let pin = try XCTUnwrap(IOSAudioProviderService.trustedAttachmentIdentity(
            routeSessionID: "session-a",
            ownerSessionID: "session-a",
            attachmentHostID: hostID,
            attachmentGeneration: 17
        ))
        XCTAssertEqual(pin, IOSMediaAttachmentIdentity(hostID: hostID, generation: 17))
        XCTAssertNil(IOSAudioProviderService.trustedAttachmentIdentity(
            routeSessionID: "session-b",
            ownerSessionID: "session-a",
            attachmentHostID: hostID,
            attachmentGeneration: 17
        ))
        XCTAssertNil(IOSAudioProviderService.trustedAttachmentIdentity(
            routeSessionID: "session-a",
            ownerSessionID: "session-a",
            attachmentHostID: "invalid",
            attachmentGeneration: 17
        ))
        XCTAssertNil(IOSAudioProviderService.trustedAttachmentIdentity(
            routeSessionID: "session-a",
            ownerSessionID: "session-a",
            attachmentHostID: hostID,
            attachmentGeneration: 0
        ))
    }

    func testAttachmentBucketsIsolateTwoEnginesForTheSameSession() throws {
        let attachmentA = try XCTUnwrap(IOSMediaAttachmentIdentity(
            hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 17
        ))
        let attachmentB = try XCTUnwrap(IOSMediaAttachmentIdentity(
            hostID: "d46e67b0-8adf-4b55-9fc9-93e586de4c14", generation: 17
        ))
        let ownerA = IOSAudioOwner.attached(
            attachment: attachmentA, logicalOwner: .session(sessionID: "same-session")
        )
        let ownerB = IOSAudioOwner.attached(
            attachment: attachmentB, logicalOwner: .session(sessionID: "same-session")
        )
        let contextA = IOSAudioSessionContext(
            sessionId: "same-session", profileId: "profile-a", accountScope: "account-a",
            engineAttachmentIdentity: attachmentA
        )
        let contextB = IOSAudioSessionContext(
            sessionId: "same-session", profileId: "profile-b", accountScope: "account-b",
            engineAttachmentIdentity: attachmentB
        )
        let contextBUpdated = IOSAudioSessionContext(
            sessionId: "same-session", profileId: "profile-b-updated", accountScope: "account-b",
            engineAttachmentIdentity: attachmentB
        )

        var confirmedRoutes = IOSAudioAttachmentSessionRoutes()
        XCTAssertTrue(confirmedRoutes.register(attachment: attachmentA, ownerID: "engine-A"))
        XCTAssertTrue(confirmedRoutes.register(attachment: attachmentA, ownerID: "engine-A"))
        XCTAssertFalse(confirmedRoutes.register(attachment: attachmentA, ownerID: "engine-other"))
        XCTAssertTrue(confirmedRoutes.register(attachment: attachmentB, ownerID: "engine-B"))
        XCTAssertTrue(confirmedRoutes.update(attachment: attachmentA, ownerID: "engine-A", context: contextA))
        XCTAssertTrue(confirmedRoutes.update(attachment: attachmentB, ownerID: "engine-B", context: contextB))
        XCTAssertEqual(confirmedRoutes.context(for: ownerA), contextA)
        XCTAssertEqual(confirmedRoutes.context(for: ownerB), contextB)
        XCTAssertEqual(confirmedRoutes.latestAttachment(ownerID: "engine-A"), attachmentA)
        XCTAssertEqual(confirmedRoutes.latestAttachment(ownerID: "engine-B"), attachmentB)

        // A confirmed revoke removes only A. A UI/owner switch and a B pin
        // update cannot overwrite A's context or remove B's route.
        XCTAssertTrue(confirmedRoutes.update(attachment: attachmentB, ownerID: "engine-B", context: contextBUpdated))
        XCTAssertFalse(confirmedRoutes.remove(attachment: attachmentB, ownerID: "engine-A"))
        XCTAssertEqual(confirmedRoutes.context(for: ownerA), contextA)
        XCTAssertEqual(confirmedRoutes.context(for: ownerB), contextBUpdated)
        XCTAssertEqual(confirmedRoutes.removeConfirmed(attachment: attachmentA), "engine-A")
        XCTAssertNil(confirmedRoutes.context(for: ownerA))
        XCTAssertEqual(confirmedRoutes.context(for: ownerB), contextBUpdated)

        // Unknown teardown retains A. Updating B during that interval remains
        // confined to B's host-issued attachment bucket.
        var unknownDrainRoutes = IOSAudioAttachmentSessionRoutes()
        XCTAssertTrue(unknownDrainRoutes.register(attachment: attachmentA, ownerID: "engine-A"))
        XCTAssertTrue(unknownDrainRoutes.register(attachment: attachmentB, ownerID: "engine-B"))
        XCTAssertTrue(unknownDrainRoutes.update(attachment: attachmentA, ownerID: "engine-A", context: contextA))
        XCTAssertTrue(unknownDrainRoutes.update(attachment: attachmentB, ownerID: "engine-B", context: contextB))
        XCTAssertTrue(unknownDrainRoutes.update(attachment: attachmentB, ownerID: "engine-B", context: contextBUpdated))
        XCTAssertEqual(unknownDrainRoutes.context(for: ownerA), contextA)
        XCTAssertEqual(unknownDrainRoutes.context(for: ownerB), contextBUpdated)
        // A source/settings owner may already have released its selection;
        // only the later host-confirmed receipt removes A's retained bucket.
        XCTAssertEqual(unknownDrainRoutes.removeConfirmed(attachment: attachmentA), "engine-A")
        XCTAssertNil(unknownDrainRoutes.context(for: ownerA))
        XCTAssertEqual(unknownDrainRoutes.context(for: ownerB), contextBUpdated)
    }


    func testAttachedOwnerDoesNotCreateTrustWhenProviderHasNoEnginePin() throws {
        let service = IOSAudioProviderService()
        let attachment = try XCTUnwrap(IOSMediaAttachmentIdentity(
            hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 17
        ))
        service.setSessionContext(
            sessionID: "session-a",
            profileID: "openai:work",
            accountScope: "account-a",
            engineAttachmentIdentity: attachment
        )
        let owner = IOSAudioOwner.attached(
            attachment: attachment,
            logicalOwner: .session(sessionID: "session-a")
        )

        XCTAssertThrowsError(try service.validateOwner(owner, binding: AudioCloudBinding()))
    }

    func testCurrentOwnersKeepInstanceAndOwnershipDomainDistinct() {
        XCTAssertNotEqual(IOSAudioOwner.ui(instanceID: "preview").stableKey,
                          IOSAudioOwner.ui(instanceID: "capture").stableKey)
        XCTAssertNotEqual(IOSAudioOwner.ui(instanceID: "preview").stableKey,
                          IOSAudioOwner.system(instanceID: "preview").stableKey)

        let firstAttachment = IOSMediaAttachmentIdentity(
            hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 17
        )!
        let replacementAttachment = IOSMediaAttachmentIdentity(
            hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 18
        )!
        let first = IOSAudioOwner.attached(
            attachment: firstAttachment,
            logicalOwner: .session(sessionID: "session-a")
        )
        let replacement = IOSAudioOwner.attached(
            attachment: replacementAttachment,
            logicalOwner: .session(sessionID: "session-a")
        )
        XCTAssertNotEqual(first, replacement)
        XCTAssertNotEqual(first.stableKey, replacement.stableKey)
    }

    func testAttachmentIdentityRequiresCanonicalV4UuidAndNonzeroGeneration() {
        XCTAssertNil(IOSMediaAttachmentIdentity(hostID: "not-a-uuid", generation: 1))
        XCTAssertNil(IOSMediaAttachmentIdentity(hostID: "4C1CA0D5-61CD-4C9D-9D9A-23C8832EBC8F", generation: 1))
        XCTAssertNil(IOSMediaAttachmentIdentity(hostID: "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f", generation: 0))
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

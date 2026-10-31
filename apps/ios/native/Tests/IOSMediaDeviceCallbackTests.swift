import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

@MainActor
final class IOSMediaDeviceCallbackTests: XCTestCase {
    func testAttachedMediaOwnerDtoPreservesAttachmentAndLogicalOwner() throws {
        let hostID = "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f"
        let owner = try IOSMediaOwnerMapper.media(from: .attached(
            attachmentHostId: hostID,
            attachmentGeneration: 17,
            logicalKind: .ui,
            logicalOwnerId: "preview|camera"
        ))
        let attachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: hostID, generation: 17))
        XCTAssertEqual(owner, .attached(
            attachment: attachment,
            logicalOwner: .ui(instanceID: "preview|camera")
        ))
    }

    func testMediaOpenIdentityUsesTypedOwnerAndOperationTuple() throws {
        let hostID = "4c1ca0d5-61cd-4c9d-9d9a-23c8832ebc8f"
        let firstAttachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: hostID, generation: 17))
        let replacementAttachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: hostID, generation: 18))
        let operation = IOSAudioOperationIdentity(
            id: "fdb72f73-cc3b-43df-a9a3-7d305f45cc00",
            generation: 2,
            serviceEpoch: 3
        )
        let first = IOSMediaDeviceService.OpenKey(
            owner: .attached(attachment: firstAttachment, logicalOwner: .session(sessionID: "session-a")),
            identity: operation
        )
        let replacement = IOSMediaDeviceService.OpenKey(
            owner: .attached(attachment: replacementAttachment, logicalOwner: .session(sessionID: "session-a")),
            identity: operation
        )

        XCTAssertNotEqual(first, replacement)
    }

    func testNativeMediaAdapterAdvertisesOnlyItsImplementedSessionModes() {
        let callback = IOSMediaDeviceCallbackAdapter(service: .shared)
        XCTAssertEqual(callback.supportedSessionModes(), [.audio, .video, .audioVideo])
    }

    func testObservationIntentDoesNotEnableMediaPlayback() {
        XCTAssertFalse(iosMediaIntentAllowsPlayback(.observation))
        XCTAssertTrue(iosMediaIntentAllowsPlayback(.conversation))
        XCTAssertEqual(iosMediaAudioSessionPurpose(.observation), .recognition)
        XCTAssertEqual(iosMediaAudioSessionPurpose(.conversation), .flowDuplex)
    }

    func testNativeMediaOutputAcceptsSharedPcmSampleRateRange() {
        for sampleRateHz: UInt32 in [8_000, 16_000, 22_050, 24_000, 44_100, 48_000, 96_000, 192_000] {
            XCTAssertTrue(isSupportedMediaPcmSampleRate(sampleRateHz), "Expected \(sampleRateHz) Hz to be accepted")
        }
        for sampleRateHz: UInt32 in [0, 7_999, 192_001] {
            XCTAssertFalse(isSupportedMediaPcmSampleRate(sampleRateHz), "Expected \(sampleRateHz) Hz to be rejected")
        }
    }

    func testNativeAudioCallbackFenceWaitsForAlreadyEnteredCallback() async throws {
        let fence = IOSRealtimeAudioCallbackFence()
        XCTAssertTrue(fence.enter())
        fence.close()
        XCTAssertFalse(fence.enter())
        var settled = false
        let waiter = Task { @MainActor in
            await fence.wait()
            settled = true
        }
        try await Task.sleep(nanoseconds: 20_000_000)
        XCTAssertFalse(settled)
        fence.leave()
        await waiter.value
        XCTAssertTrue(settled)
    }

    func testUnknownLeaseCannotReadInputAndResponseEchoesRequestID() async {
        let requestID = UUID().uuidString.lowercased()
        let response = await IOSMediaDeviceService.shared.handle(
            MediaDeviceRequestEnvelopeDto(
                requestId: requestID,
                request: .nextInput(leaseId: UUID().uuidString.lowercased())
            )
        )

        XCTAssertEqual(response.requestId, requestID)
        guard case let .error(error) = response.result else {
            return XCTFail("An unknown lease must not expose a media frame")
        }
        XCTAssertEqual(error.kind, .notFound)
    }

    func testInvalidOpenRejectedBeforeNativeDispatchHasNoResourceReceipt() async {
        let requestID = UUID().uuidString.lowercased()
        let open = MediaDeviceOpenRequestDto(
            identity: MediaOperationIdDto(id: UUID().uuidString.lowercased(), generation: 1, serviceEpoch: 1),
            owner: .session(sessionId: "open-preflight"),
            initiator: nil,
            timeoutBudgetMs: nil,
            maxPayloadBytes: 0,
            request: MediaSessionRequestDto(mode: .audio, intent: .observation, audioSource: nil, videoSource: nil)
        )
        let response = await IOSMediaDeviceService.shared.handle(
            MediaDeviceRequestEnvelopeDto(requestId: requestID, request: .open(request: open))
        )
        XCTAssertEqual(response.requestId, requestID)
        guard case let .openFailedWithoutResource(error) = response.result else {
            return XCTFail("Pre-dispatch rejection must return the explicit no-resource receipt")
        }
        XCTAssertEqual(error.kind, .invalidRequest)
    }

    func testUnknownVideoLeaseIsNotReportedAsRendered() async {
        let requestID = UUID().uuidString.lowercased()
        let response = await IOSMediaDeviceService.shared.handle(
            MediaDeviceRequestEnvelopeDto(
                requestId: requestID,
                request: .writeVideo(request: MediaDeviceVideoWriteRequestDto(
                    leaseId: UUID().uuidString.lowercased(),
                    frame: .rgba8(widthPx: 1, heightPx: 1, strideBytes: 4, bytes: Data([0, 0, 0, 255]))
                ))
            )
        )

        XCTAssertEqual(response.requestId, requestID)
        guard case let .error(error) = response.result else {
            return XCTFail("Video write must not be acknowledged without a renderer")
        }
        XCTAssertEqual(error.kind, MediaErrorKindDto.notFound)
    }

    func testUnknownStopDoesNotClaimAClosedLeaseReceipt() async {
        let requestID = UUID().uuidString.lowercased()
        let response = await IOSMediaDeviceService.shared.handle(
            MediaDeviceRequestEnvelopeDto(requestId: requestID, request: .stop(leaseId: UUID().uuidString.lowercased()))
        )
        XCTAssertEqual(response.requestId, requestID)
        guard case let .error(error) = response.result else {
            return XCTFail("An unknown lease has no confirmed stop receipt")
        }
        XCTAssertEqual(error.kind, .notFound)
    }

    func testMediaRuntimeReceivesBareV4AudioSettings() throws {
        let suiteName = "ios-media-settings-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let store = AudioConfigurationStore(defaults: defaults)

        let json = try XCTUnwrap(store.mediaRuntimeConfigurationJSON)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        XCTAssertEqual(object["schemaVersion"] as? Int, 4)
        XCTAssertNotNil(object["recognition"])
        XCTAssertNotNil(object["speech"])
        XCTAssertNil(object["configuration"])
        XCTAssertNil(object["revision"])
    }

    func testNativeMediaHostRejectsInvalidAudioSettingsInsteadOfUsingDefaults() {
        XCTAssertThrowsError(try IOSNativeMediaHostComposition(audioConfigurationJSON: "{}"))
    }

    func testMediaReceiptLedgerDoesNotReturnInterruptedFramesAgainToFlush() {
        var ledger = IOSMediaPlaybackReceiptLedger()
        ledger.accept(100, generation: 2)
        ledger.recordPlayed(37, generation: 2, renderedAtUnixMs: 120)

        let interrupted = ledger.take(generation: 2)
        XCTAssertEqual(interrupted.playedSampleFrames, 37)
        XCTAssertEqual(interrupted.renderedAtUnixMs, 120)

        let settledFlush = ledger.take(generation: 2)
        XCTAssertEqual(settledFlush.playedSampleFrames, 0)
        XCTAssertNil(settledFlush.renderedAtUnixMs)

        ledger.accept(20, generation: 3)
        ledger.recordPlayed(20, generation: 3, renderedAtUnixMs: 140)
        XCTAssertEqual(ledger.take(generation: 3).playedSampleFrames, 20)
    }
}
#endif

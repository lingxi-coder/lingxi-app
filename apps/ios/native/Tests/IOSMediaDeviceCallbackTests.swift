import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

@MainActor
final class IOSMediaDeviceCallbackTests: XCTestCase {
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

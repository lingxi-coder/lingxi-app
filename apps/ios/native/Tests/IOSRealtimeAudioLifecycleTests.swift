import XCTest
@testable import LingxiCode

final class IOSRealtimeAudioLifecycleTests: XCTestCase {
    func testCallbackMailboxPreservesOrderedEvents() async throws {
        let listener = IOSRealtimeAudioListenerAdapter()
        await listener.onEvent(eventJson: #"{"type":"session_ready"}"#)
        await listener.onEvent(eventJson: #"{"type":"transcript","text":"hello"}"#)
        listener.finish()
        var values: [String] = []
        for try await event in listener.events { values.append(event) }
        XCTAssertEqual(values, [#"{"type":"session_ready"}"#, #"{"type":"transcript","text":"hello"}"#])
    }

    func testCallbackOverflowTerminatesInsteadOfSilentlyDroppingAudio() async {
        let listener = IOSRealtimeAudioListenerAdapter()
        for index in 0 ..< 17 { await listener.onEvent(eventJson: "event-\(index)") }
        do {
            for try await _ in listener.events {}
            XCTFail("overflow must report a failure to the owning conversation")
        } catch {
            guard case .mediaTooLarge = error as? AudioServiceFailure else {
                return XCTFail("expected a bounded-media overflow failure")
            }
        }
    }

    func testOversizedCallbackFailsBeforeQueueingPayload() async {
        let listener = IOSRealtimeAudioListenerAdapter()
        await listener.onEvent(eventJson: String(repeating: "x", count: 1_500_001))
        do {
            for try await _ in listener.events { XCTFail("oversized input must not be queued") }
            XCTFail("oversized callback must fail")
        } catch {
            guard case .mediaTooLarge = error as? AudioServiceFailure else {
                return XCTFail("expected a bounded-media overflow failure")
            }
        }
    }
}

@MainActor
private final class RealtimeStopDevice: IOSRealtimeAudioDeviceDriving {
    var onPlaybackCompleted: ((String?) -> Void)?
    var hasPendingOutput = false
    var currentPlaybackItemID: String?
    var stopCalls = 0
    private var stopContinuation: CheckedContinuation<Bool, Never>?

    func owns(_ event: VoiceAudioSessionCoordinator.Invalidation) async -> Bool { false }
    func start(inputSampleRateHz: UInt32, outputSampleRateHz: UInt32) async throws -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { $0.finish() }
    }
    func stop() async -> Bool {
        stopCalls += 1
        return await withCheckedContinuation { stopContinuation = $0 }
    }
    func finishStop(_ result: Bool) {
        let continuation = stopContinuation
        stopContinuation = nil
        continuation?.resume(returning: result)
    }
    func enqueueOutput(pcm: Data, sampleRateHz: UInt32, itemID: String?) throws {}
    func markAllOutputCompleted() {}
    func interruptPlayback(itemID: String?) -> UInt64 { 0 }
}

extension IOSRealtimeAudioLifecycleTests {
    @MainActor
    func testUnconfirmedStopKeepsAdmissionClosedAndCanBeRetried() async throws {
        let device = RealtimeStopDevice()
        let service = IOSRealtimeAudioService(device: device, stopTimeout: .milliseconds(30), observeInvalidation: false)
        let first = Task { await service.stop(abort: true) }
        while device.stopCalls == 0 { await Task.yield() }
        let second = Task { await service.stop(abort: true) }
        let firstResult = await first.value
        let secondResult = await second.value
        XCTAssertFalse(firstResult)
        XCTAssertFalse(secondResult)
        XCTAssertEqual(device.stopCalls, 1)
        XCTAssertTrue(service.isActive)
        XCTAssertEqual(service.phase, .failed)

        // Finish the original flight before retrying; its failure must not be
        // mistaken for a receipt, and must not start a second device teardown.
        device.finishStop(false)
        try await Task.sleep(for: .milliseconds(10))
        XCTAssertTrue(service.isActive)
        let retry = Task { await service.stop(abort: true) }
        while device.stopCalls < 2 { await Task.yield() }
        device.finishStop(true)
        let retryResult = await retry.value
        XCTAssertTrue(retryResult)
        XCTAssertFalse(service.isActive)
        XCTAssertEqual(service.phase, .idle)
    }
}

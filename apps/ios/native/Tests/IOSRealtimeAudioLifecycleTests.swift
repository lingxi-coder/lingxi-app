import XCTest
@testable import LingxiCode

final class IOSRealtimeAudioLifecycleTests: XCTestCase {
    func testToolPermissionSurvivesProviderCompletionAndContinuation() {
        var ownership = IOSRealtimeTurnOwnership()
        ownership.beginInput()
        XCTAssertTrue(ownership.hasPendingWork, "The input is owned before the native start callback arrives")
        ownership.beginAgentTurn(turnID: "agent-1")
        ownership.beginToolWork()
        XCTAssertTrue(ownership.hasPendingWork, "A permission-gated tool outlives its provider response")
        // The same native Agent turn owns every provider continuation.
        ownership.markActive()
        XCTAssertFalse(ownership.completeAgentTurn(turnID: "stale-agent"), "A stale terminal cannot release the continuation")
        XCTAssertTrue(ownership.hasPendingWork)
        XCTAssertTrue(ownership.completeAgentTurn(turnID: "agent-1"))
        XCTAssertFalse(ownership.hasPendingWork)
        ownership.beginInput()
        XCTAssertFalse(ownership.completeAgentTurn(turnID: "agent-1"), "A late terminal cannot release a new input")
        ownership.beginAgentTurn(turnID: nil)
        XCTAssertFalse(ownership.completeAgentTurn(turnID: nil), "An uncorrelated terminal cannot release the owner")
        ownership.beginAgentTurn(turnID: "agent-1")
        XCTAssertFalse(ownership.completeAgentTurn(turnID: "agent-1"), "Agent turn IDs cannot be reused")
        ownership.beginAgentTurn(turnID: "agent-2")
        XCTAssertTrue(ownership.completeAgentTurn(turnID: "agent-2"))
    }

    func testNavigationWaitsForSubmittedTurnAndPlaybackToDrain() {
        XCTAssertTrue(IOSRealtimeAudioDrainPolicy.canClose(turnInFlight: false,
            hasUnconfirmedToolWork: false,
            pendingPlayback: false, pendingAcknowledgements: false))
        XCTAssertFalse(IOSRealtimeAudioDrainPolicy.canClose(turnInFlight: true,
            hasUnconfirmedToolWork: false,
            pendingPlayback: false, pendingAcknowledgements: false))
        // A provider response can finish while an Agent tool is waiting for
        // permission; only an authoritative Agent terminal can release it.
        XCTAssertFalse(IOSRealtimeAudioDrainPolicy.canClose(turnInFlight: false,
            hasUnconfirmedToolWork: true,
            pendingPlayback: false, pendingAcknowledgements: false))
        XCTAssertFalse(IOSRealtimeAudioDrainPolicy.canClose(turnInFlight: false,
            hasUnconfirmedToolWork: false,
            pendingPlayback: true, pendingAcknowledgements: false))
        XCTAssertFalse(IOSRealtimeAudioDrainPolicy.canClose(turnInFlight: false,
            hasUnconfirmedToolWork: false,
            pendingPlayback: false, pendingAcknowledgements: true))
        XCTAssertTrue(IOSRealtimeAudioDrainPolicy.canClose(turnInFlight: false,
            hasUnconfirmedToolWork: false,
            pendingPlayback: false, pendingAcknowledgements: false))
    }

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
    var startCalls = 0
    var stopCalls = 0
    private var stopContinuation: CheckedContinuation<Bool, Never>?

    func owns(_ event: VoiceAudioSessionCoordinator.Invalidation) async -> Bool { false }
    func start(inputSampleRateHz: UInt32, outputSampleRateHz: UInt32) async throws -> AsyncThrowingStream<Data, Error> {
        startCalls += 1
        return AsyncThrowingStream { $0.finish() }
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

#if DEBUG
private actor RealtimeSessionCalls {
    private(set) var commits = 0
    private(set) var closes = 0
    private(set) var aborts = 0
    func committed() { commits += 1 }
    func closed() { closes += 1 }
    func aborted() { aborts += 1 }
    func snapshot() -> (Int, Int, Int) { (commits, closes, aborts) }
}

private final class RealtimeCommitSession: IosRealtimeAudioSession, @unchecked Sendable {
    let calls = RealtimeSessionCalls()
    required init(unsafeFromHandle handle: UInt64) { super.init(unsafeFromHandle: handle) }
    init() { super.init(noHandle: .init()) }
    override func commitInput() async throws { await calls.committed() }
    override func close() async throws { await calls.closed() }
    override func abort() async { await calls.aborted() }
}
#endif

extension IOSRealtimeAudioLifecycleTests {
    #if DEBUG
    @MainActor
    func testSubmitTapBeforeNavigationKeepsTurnUntilTerminal() async {
        let device = RealtimeStopDevice()
        let session = RealtimeCommitSession()
        let service = IOSRealtimeAudioService(device: device, observeInvalidation: false)
        service.installSessionForTesting(session)
        service.commitInput()
        service.finishCurrentTurnAndClose()
        let clock = ContinuousClock()
        let commitDeadline = clock.now.advanced(by: .seconds(1))
        var beforeTerminal = await session.calls.snapshot()
        while beforeTerminal.0 == 0, clock.now < commitDeadline {
            await Task.yield()
            beforeTerminal = await session.calls.snapshot()
        }
        XCTAssertEqual(beforeTerminal.0, 1)
        XCTAssertEqual(beforeTerminal.1, 0)
        XCTAssertEqual(beforeTerminal.2, 0)
        XCTAssertEqual(device.stopCalls, 0)

        await service.receiveForTesting(#"{"type":"agent_response_started","turnId":"tapped"}"#)
        await service.receiveForTesting(#"{"type":"agent_turn_completed","turnId":"tapped"}"#)
        let stopDeadline = clock.now.advanced(by: .seconds(1))
        while device.stopCalls == 0, clock.now < stopDeadline { await Task.yield() }
        XCTAssertEqual(device.stopCalls, 1)
        guard device.stopCalls == 1 else { return }
        device.finishStop(true)
        let closeDeadline = clock.now.advanced(by: .seconds(1))
        while service.isActive, clock.now < closeDeadline { await Task.yield() }
        XCTAssertFalse(service.isActive)
        let afterTerminal = await session.calls.snapshot()
        XCTAssertEqual(afterTerminal.0, 1)
        XCTAssertEqual(afterTerminal.1, 1)
        XCTAssertEqual(afterTerminal.2, 0)
    }

    @MainActor
    func testQueuedCloseRechecksAnAgentTurnThatStartsBeforeStop() async {
        let device = RealtimeStopDevice()
        let service = IOSRealtimeAudioService(device: device, observeInvalidation: false)
        service.requestCloseAfterDrainForTesting()
        await service.receiveForTesting(#"{"type":"agent_response_started","turnId":"new-agent"}"#)
        for _ in 0 ..< 10 { await Task.yield() }
        XCTAssertEqual(device.stopCalls, 0)
        await service.receiveForTesting(#"{"type":"agent_turn_completed","turnId":"new-agent"}"#)
        while device.stopCalls == 0 { await Task.yield() }
        device.finishStop(true)
        while service.phase != .idle { await Task.yield() }
    }

    @MainActor
    func testDuplicateSessionReadyDoesNotRestartDeviceDuringTurn() async {
        let device = RealtimeStopDevice()
        let service = IOSRealtimeAudioService(device: device, observeInvalidation: false)
        let ready = #"{"type":"session_ready","inputFormat":{"encoding":"pcm16","channels":1,"sampleRateHz":24000},"outputFormat":{"encoding":"pcm16","channels":1,"sampleRateHz":24000}}"#
        await service.receiveForTesting(ready)
        while device.startCalls == 0 { await Task.yield() }
        await service.receiveForTesting(#"{"type":"agent_response_started","turnId":"agent-1"}"#)
        await service.receiveForTesting(ready)
        for _ in 0 ..< 10 { await Task.yield() }
        XCTAssertEqual(device.startCalls, 1)
        XCTAssertEqual(service.phase, .thinking)
        let stopping = Task { await service.stop(abort: true) }
        while device.stopCalls == 0 { await Task.yield() }
        device.finishStop(true)
        let stopped = await stopping.value
        XCTAssertTrue(stopped)
    }

    @MainActor
    func testLateInputTranscriptDoesNotReopenCompletedAgentTurn() async {
        let service = IOSRealtimeAudioService(device: RealtimeStopDevice(), observeInvalidation: false)
        await service.receiveForTesting(#"{"type":"agent_response_started","turnId":"agent-1"}"#)
        await service.receiveForTesting(#"{"type":"turn_completed","turnId":null}"#)
        await service.receiveForTesting(#"{"type":"agent_turn_completed","turnId":"agent-1"}"#)
        XCTAssertEqual(service.phase, .listening)
        await service.receiveForTesting(#"{"type":"transcript","role":"user","final":true,"text":"late words"}"#)
        XCTAssertEqual(service.phase, .listening)
        XCTAssertEqual(service.caption, "late words")
    }
    #endif

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

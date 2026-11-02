import XCTest
@testable import LingxiCode

final class RemoteModeAdmissionTests: XCTestCase {
    @MainActor
    func testHostSwitchWaitsForEveryAdmittedRequest() async {
        let drain = RemoteRequestDrain()
        drain.begin()
        drain.begin()
        var switched = false
        let waiter = Task { @MainActor in
            await drain.waitUntilEmpty()
            switched = true
        }
        await Task.yield()
        XCTAssertFalse(switched)
        drain.end()
        await Task.yield()
        XCTAssertFalse(switched)
        drain.end()
        await waiter.value
        XCTAssertTrue(switched)
    }

    @MainActor
    func testNativeConnectsRemainSerializedWhileAnEarlierConnectIsPending() async {
        let serial = RemoteConnectSerial()
        await serial.acquire()
        var nextEntered = false
        let next = Task { @MainActor in
            await serial.acquire()
            nextEntered = true
            serial.release()
        }
        await Task.yield()
        XCTAssertFalse(nextEntered)
        serial.release()
        await next.value
        XCTAssertTrue(nextEntered)
    }

    @MainActor
    func testLateConnectCannotOpenAnOlderHostBeforeLatestIntent() async {
        let serial = RemoteConnectSerial()
        var fence = RemoteConnectionFence()
        let first = fence.begin("B")
        await serial.acquire()
        fence.nativeCallStarted()
        let latest = fence.begin("C")
        var latestEntered = false
        let next = Task { @MainActor in
            await serial.acquire()
            latestEntered = true
            fence.nativeCallStarted()
            fence.nativeCallSucceeded("C")
            fence.finishSuccess(latest)
            serial.release()
        }
        await Task.yield()
        XCTAssertFalse(latestEntered)
        fence.nativeCallSucceeded("B")
        fence.finishSuccess(first)
        XCTAssertTrue(fence.admissionClosed)
        serial.release()
        await next.value
        XCTAssertEqual(fence.requestedHostID, "C")
        XCTAssertEqual(fence.confirmedHostID, "C")
        XCTAssertFalse(fence.admissionClosed)
    }

    @MainActor
    func testDetachWaitsForAcceptedWorkBeforeNextHostCanConnect() async {
        let serial = RemoteConnectSerial()
        let drain = RemoteRequestDrain()
        var events: [String] = []
        let (started, continuation) = AsyncStream<Void>.makeStream()
        drain.begin()
        let detach = Task { @MainActor in
            await serial.acquire()
            continuation.yield(())
            await drain.waitUntilEmpty()
            events.append("detach")
            serial.release()
        }
        var iterator = started.makeAsyncIterator()
        _ = await iterator.next()
        let switchHost = Task { @MainActor in
            await serial.acquire()
            events.append("connect")
            serial.release()
        }
        await Task.yield()
        XCTAssertTrue(events.isEmpty)
        drain.end()
        await detach.value
        await switchHost.value
        XCTAssertEqual(events, ["detach", "connect"])
    }

    func testCancelledQueuedConnectRestoresOnlyAProvenReadyHost() {
        var fence = RemoteConnectionFence()
        let first = fence.begin("A")
        fence.nativeCallStarted()
        fence.nativeCallSucceeded("A")
        fence.finishSuccess(first)
        let cancelled = fence.begin("A")
        fence.finishFailure(cancelled, readyHostID: "A")
        XCTAssertFalse(fence.admissionClosed)
        XCTAssertFalse(fence.needsRetry)

        let stillConnecting = fence.begin("A")
        fence.finishFailure(stillConnecting, readyHostID: nil)
        XCTAssertTrue(fence.needsRetry)
        fence.observeReadyHost("A")
        XCTAssertFalse(fence.admissionClosed)
    }

    func testNativeConnectFailureRequiresRetryAndStaleSuccessCannotOpenAdmission() {
        var fence = RemoteConnectionFence()
        let first = fence.begin("A")
        fence.nativeCallStarted()
        fence.nativeCallSucceeded("A")
        fence.finishSuccess(first)
        let failed = fence.begin("B")
        fence.nativeCallStarted()
        fence.finishFailure(failed, readyHostID: "A")
        XCTAssertTrue(fence.admissionClosed)
        XCTAssertTrue(fence.needsRetry)
        fence.observeReadyHost("A")
        XCTAssertTrue(fence.admissionClosed)

        let old = fence.begin("B")
        fence.nativeCallStarted()
        let latest = fence.begin("C")
        fence.nativeCallSucceeded("B")
        fence.finishSuccess(old)
        XCTAssertTrue(fence.admissionClosed)
        fence.nativeCallStarted()
        fence.nativeCallSucceeded("C")
        fence.finishSuccess(latest)
        XCTAssertFalse(fence.admissionClosed)
        XCTAssertEqual(fence.requestedHostID, "C")
    }

    func testTeamApprovalIDsRequireTheExactReservedNonceShape() {
        let nonce = "0123456789abcdefghijklmnopqrstuv"
        XCTAssertTrue(RemoteApprovalCapability.accepts("team-approval:" + nonce))
        for invalid in ["team-approval:", "team-approval:" + nonce + "x", "team-approval:" + String(nonce.dropLast()),
                        "team-approval:" + String(nonce.dropLast()) + ".", "TEAM-APPROVAL:" + nonce] {
            XCTAssertFalse(RemoteApprovalCapability.accepts(invalid), invalid)
        }
    }

    func testUnknownUpdatesReplayInOrderAfterModeIsKnown() {
        var gate = RemoteModeAdmission<String>()
        guard case .waiting = gate.receive("snapshot", announcedModes: []) else { return XCTFail() }
        guard case .waiting = gate.receive("event", announcedModes: []) else { return XCTFail() }
        guard case let .replay(mode, updates) = gate.receive("mode", announcedModes: [.chat]) else { return XCTFail() }
        XCTAssertEqual(mode, .chat)
        XCTAssertEqual(updates, ["snapshot", "event", "mode"])
        guard case let .apply(update) = gate.receive("partial", announcedModes: []) else { return XCTFail() }
        XCTAssertEqual(update, "partial")
    }

    func testConflictingModesFailClosedPermanently() {
        var gate = RemoteModeAdmission<String>()
        guard case .replay = gate.receive("chat", announcedModes: [.chat]) else { return XCTFail() }
        guard case .failed = gate.receive("code", announcedModes: [.code]) else { return XCTFail() }
        guard case .failed = gate.receive("partial", announcedModes: []) else { return XCTFail() }

        var conflictingSnapshot = RemoteModeAdmission<String>()
        guard case .failed = conflictingSnapshot.receive("conflict", announcedModes: [.chat, .code]) else { return XCTFail() }
    }

    func testBufferOverflowFailsInsteadOfDroppingUpdates() {
        var gate = RemoteModeAdmission<String>(capacity: 2)
        guard case .waiting = gate.receive("one", announcedModes: []) else { return XCTFail() }
        guard case .waiting = gate.receive("two", announcedModes: []) else { return XCTFail() }
        guard case .failed = gate.receive("three", announcedModes: []) else { return XCTFail() }
        guard case .failed = gate.receive("mode", announcedModes: [.chat]) else { return XCTFail() }
    }

    func testByteBudgetIsCumulativeAndOnlyMeasuresBufferedUpdates() {
        var gate = RemoteModeAdmission<String>(byteCapacity: 10)
        guard case .waiting = gate.receive("one", announcedModes: [], estimatedBytes: { 6 }) else { return XCTFail() }
        guard case .failed = gate.receive("two", announcedModes: [], estimatedBytes: { 5 }) else { return XCTFail() }

        var ready = RemoteModeAdmission<String>(byteCapacity: 1)
        guard case .replay = ready.receive("large known snapshot", announcedModes: [.chat],
                                           estimatedBytes: { XCTFail("not buffered"); return 100 }) else { return XCTFail() }
        guard case .apply = ready.receive("large live update", announcedModes: [],
                                          estimatedBytes: { XCTFail("not buffered"); return 100 }) else { return XCTFail() }
    }

    func testMemoryEstimateIncludesNestedTextAndBuffersWithoutCopying() {
        struct Snapshot { let messages: [String]; let raw: [UInt16] }
        let snapshot = Snapshot(messages: [String(repeating: "a", count: 900)], raw: Array(repeating: 0, count: 100))
        XCTAssertGreaterThan(RemoteUpdateMemoryEstimate.bytes(of: snapshot, limit: 1_024), 1_024)
        XCTAssertLessThanOrEqual(RemoteUpdateMemoryEstimate.bytes(of: snapshot, limit: 2_048), 2_048)
    }

    func testInvalidationDiscardsPendingUpdates() {
        var gate = RemoteModeAdmission<String>()
        guard case .waiting = gate.receive("before account change", announcedModes: []) else { return XCTFail() }
        gate.invalidate()
        guard case .failed = gate.receive("mode", announcedModes: [.chat]) else { return XCTFail() }
    }
}

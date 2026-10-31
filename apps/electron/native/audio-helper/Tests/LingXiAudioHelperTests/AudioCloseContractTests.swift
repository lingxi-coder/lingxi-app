import Foundation
import Testing
@testable import LingXiAudioHelper

private final class CloseTestOutput: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [[String: Any]] = []
    func append(_ data: Data) {
        guard let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        lock.lock(); storage.append(value); lock.unlock()
    }
    var values: [[String: Any]] { lock.lock(); defer { lock.unlock() }; return storage }
}

private actor CloseTestHold {
    private var continuation: CheckedContinuation<Void, Never>?
    private var released = false
    func wait() async {
        if released { return }
        await withCheckedContinuation { continuation = $0 }
    }
    func release() { released = true; continuation?.resume(); continuation = nil }
}

private actor CloseTestSessionHolder {
    private var session: ListeningSession?
    func set(_ value: ListeningSession) { session = value }
    func stop() async { _ = await session?.stop() }
}

private func closeTestJSON(_ value: [String: Any]) throws -> String {
    try #require(String(data: JSONSerialization.data(withJSONObject: value), encoding: .utf8))
}

private func closeTestWait(_ output: CloseTestOutput, id: String) async throws -> [String: Any] {
    for _ in 0..<300 {
        if let value = output.values.first(where: { $0["id"] as? String == id }) { return value }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    throw NSError(domain: "AudioCloseContractTests", code: 1, userInfo: [NSLocalizedDescriptionKey: "missing \(id)"])
}

@Test func nativeCallbackFenceJoinsAcceptedWorkAndFailsClosedOnOverflow() async {
    let fence = NativeCallbackFence(capacity: 2)
    #expect(fence.enter())
    #expect(fence.enter())
    fence.close()
    #expect(!fence.enter())
    let joined = Task { await fence.join() }
    fence.leave(); fence.leave()
    #expect(await joined.value)
    let overflow = NativeCallbackFence(capacity: 1)
    #expect(overflow.enter())
    #expect(!overflow.enter())
    overflow.close(); overflow.leave()
    #expect(!(await overflow.join()))
}

@Test func onLimitCanAwaitTheSameStopWithoutSelfJoin() async {
    let holder = CloseTestSessionHolder()
    let started = AsyncStream<Void>.makeStream()
    let session = await MainActor.run {
        ListeningSession(owner: HelperOwner(kind: "session", id: "limit-test"), languageIdentifier: "en-US",
            route: .captureOnly, outputSampleRate: 16_000, maximumSampleCount: 1_024,
            cancellation: AudioCancellationFlag(), onPartial: { _ in }, onLevel: { _ in },
            onSilence: {}, onLimit: { started.continuation.yield(()); await holder.stop() })
    }
    await holder.set(session)
    await MainActor.run { session.signalCaptureLimit() }
    var iterator = started.stream.makeAsyncIterator()
    _ = await iterator.next()
    _ = await session.stop()
    #expect(await session.closeCallbacksDrained())
}

@Test func cancelledTaskMustSettleAndMatchOwnerBeforeReceipt() async throws {
    let output = CloseTestOutput()
    let started = AsyncStream<Void>.makeStream()
    let hold = CloseTestHold()
    let executor: HelperAudioOperationExecutor = { _, _ in
        started.continuation.yield(())
        await hold.wait()
        return .status(recording: false, playing: false)
    }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("audio-close-\(UUID().uuidString)")
    let state = HelperStateStore(storageRoot: root, writer: LineWriter(emit: { output.append($0) }), operationExecutor: executor)
    let snapshot = await state.snapshot(message: nil)
    let identity: [String: Any] = ["id": UUID().uuidString.lowercased(), "generation": 1,
        "service_epoch": try #require(snapshot.capabilities?.serviceEpoch)]
    let owner: [String: Any] = ["type": "session", "session_id": "s"]
    let context: [String: Any] = ["helper_instance": try #require(snapshot.helper.instanceId),
        "connection_id": UUID().uuidString.lowercased(), "resource_incarnation": UUID().uuidString.lowercased(),
        "flight_id": UUID().uuidString.lowercased()]
    let request: [String: Any] = ["id": "request", "kind": "engine_request", "closeContext": context,
        "request": ["identity": identity, "owner": owner, "max_payload_bytes": 1_024, "operation": ["type": "status"]]]
    func cancel(_ id: String, _ owner: [String: Any]) -> [String: Any] {
        ["id": id, "kind": "command", "command": ["type": "cancel_operation", "identity": identity, "owner": owner, "closeContext": context]]
    }
    let (lines, continuation) = AsyncStream<String>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { await handleHelperInputEnvelope($0, state: state) },
            cancel: { await cancelHelperInputEnvelope($0, state: state) }, reject: { await state.rejectOverloaded($0, reason: $1) })
    }
    continuation.yield(try closeTestJSON(request))
    var iterator = started.stream.makeAsyncIterator(); _ = await iterator.next()
    continuation.yield(try closeTestJSON(cancel("wrong-owner", ["type": "session", "session_id": "other"])))
    let wrong = try await closeTestWait(output, id: "wrong-owner")
    let wrongResponse = try #require(wrong["result"] as? [String: Any])
    #expect((wrongResponse["closeReceipt"] as? [String: Any])?["disposition"] as? String == "unknown")
    continuation.yield(try closeTestJSON(cancel("close", owner)))
    continuation.yield(try closeTestJSON(["id": "snapshot", "kind": "command", "command": ["type": "get_snapshot"]]))
    _ = try await closeTestWait(output, id: "snapshot")
    #expect(!output.values.contains(where: { $0["id"] as? String == "close" }))
    await hold.release()
    _ = try await closeTestWait(output, id: "request")
    let closed = try await closeTestWait(output, id: "close")
    let closedResponse = try #require(closed["result"] as? [String: Any])
    #expect((closedResponse["closeReceipt"] as? [String: Any])?["disposition"] as? String == "unknown")
    let ids = output.values.compactMap { $0["id"] as? String }
    #expect(try #require(ids.firstIndex(of: "request")) < #require(ids.firstIndex(of: "close")))
    continuation.finish(); await dispatcher.value
}

@Test func helperProvesNotAdmittedForARecordingRejectedBeforeNativeSetup() async throws {
    let output = CloseTestOutput()
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("audio-not-admitted-\(UUID().uuidString)")
    let state = HelperStateStore(storageRoot: root, writer: LineWriter(emit: { output.append($0) }))
    let snapshot = await state.snapshot(message: nil)
    let identity = HelperAudioOperationIdentity(id: UUID().uuidString.lowercased(), generation: 1,
        serviceEpoch: try #require(snapshot.capabilities?.serviceEpoch))
    let owner = HelperAudioOwner(type: "session", sessionID: "s", instanceID: nil)
    let context = HelperAudioCloseContext(helperInstance: try #require(snapshot.helper.instanceId), connectionID: UUID().uuidString.lowercased(),
        resourceIncarnation: UUID().uuidString.lowercased(), flightID: UUID().uuidString.lowercased())
    await state.handleEngineRequest(id: "too-small", identity: identity, owner: owner,
        operation: ["type": "start_recording", "sample_rate_hz": 16_000, "format": "wav"],
        configuration: AudioConfigurationNormalizer.defaults, configurationRevision: 0, maxPayloadBytes: 1, closeContext: context)
    let response = try await closeTestWait(output, id: "too-small")
    let result = try #require(response["result"] as? [String: Any])
    #expect(result["admissionWitness"] == nil)
    let command: [String: Any] = ["identity": ["id": identity.id, "generation": identity.generation, "service_epoch": identity.serviceEpoch],
        "owner": ["type": "session", "session_id": "s"],
        "closeContext": ["helper_instance": context.helperInstance, "connection_id": context.connectionID,
                         "resource_incarnation": context.resourceIncarnation, "flight_id": context.flightID]]
    var wrongOwner = command
    wrongOwner["owner"] = ["type": "session", "session_id": "other"]
    await state.beginVerifiedCancellation(wrongOwner)
    #expect(await state.verifiedCancellationReceipt(wrongOwner)?.disposition == "unknown")
    await state.beginVerifiedCancellation(command)
    let receipt = try #require(await state.verifiedCancellationReceipt(command))
    #expect(receipt.disposition == "not_admitted")
    #expect(receipt.identity == identity)
    #expect(receipt.context == context)
    #expect(await state.acknowledgeVerifiedCancellation(command))
    await state.beginVerifiedCancellation(command)
    #expect(await state.verifiedCancellationReceipt(command)?.disposition == "unknown")
}

@Test func cancellingAnOldStopCannotRollBackANewerRecordingForTheSameOwner() {
    let owner = HelperAudioOwner(type: "session", sessionID: "same-owner", instanceID: nil)
    let oldOrigin = HelperAudioOperationIdentity(id: "old", generation: 1, serviceEpoch: 7)
    let stop = HelperAudioOperationIdentity(id: "stop", generation: 2, serviceEpoch: 7)
    let newOrigin = HelperAudioOperationIdentity(id: "new", generation: 3, serviceEpoch: 7)
    #expect(recordingOriginToRollback(for: stop, operation: "stop_recording", owner: owner,
        targetRecordingOrigin: oldOrigin, recordingOrigin: oldOrigin, recordingOwner: owner) == oldOrigin)
    #expect(recordingOriginToRollback(for: stop, operation: "stop_recording", owner: owner,
        targetRecordingOrigin: oldOrigin, recordingOrigin: newOrigin, recordingOwner: owner) == nil)
}

private actor CloseTestMultipleHolds {
    private var continuations: [CheckedContinuation<Void, Never>] = []
    private var released = false
    func wait() async {
        if released { return }
        await withCheckedContinuation { continuations.append($0) }
    }
    func release() {
        released = true
        let waiting = continuations; continuations.removeAll()
        for continuation in waiting { continuation.resume() }
    }
}

@Test func schedulerDistinguishesOverloadFromDuplicateMediaAndEnvelopeIDs() async throws {
    let holds = CloseTestMultipleHolds()
    let (lines, input) = AsyncStream<String>.makeStream()
    let (started, starts) = AsyncStream<Void>.makeStream()
    let (rejected, rejections) = AsyncStream<String>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { _ in starts.yield(()); await holds.wait() }, cancel: { _ in },
            reject: { value, reason in
                let id = value["id"] as? String ?? "?"
                rejections.yield("\(id):\(reason)")
            })
    }
    func open(_ id: String, _ requestID: String) -> [String: Any] {
        ["id": id, "kind": "media_request", "request": ["request_id": requestID,
            "request": ["type": "open", "request": ["owner": ["type": "session", "session_id": "s"]]]]]
    }
    input.yield(try closeTestJSON(open("open-envelope", "open-request")))
    for index in 1..<16 {
        input.yield(try closeTestJSON(["id": "busy-\(index)", "kind": "command", "command": ["type": "get_snapshot"]]))
    }
    var startsIterator = started.makeAsyncIterator()
    for _ in 0..<16 { #expect(await startsIterator.next() != nil) }
    input.yield(try closeTestJSON(open("open-envelope", "another-request")))
    input.yield(try closeTestJSON(open("same-request", "open-request")))
    input.yield(try closeTestJSON(open("new-request", "new-request")))
    var iterator = rejected.makeAsyncIterator()
    #expect(await iterator.next() == "open-envelope:duplicate")
    #expect(await iterator.next() == "same-request:duplicate")
    #expect(await iterator.next() == "new-request:overloaded")
    await holds.release(); input.finish(); await dispatcher.value
    starts.finish(); rejections.finish()
}

@Test func mediaOverloadProofOnlyCoversAnUnadmittedUniqueRequest() async throws {
    let holds = CloseTestMultipleHolds()
    let output = CloseTestOutput()
    let state = HelperStateStore(storageRoot: FileManager.default.temporaryDirectory.appendingPathComponent("media-overload-\(UUID().uuidString)"), writer: LineWriter(emit: { output.append($0) }))
    let (lines, input) = AsyncStream<String>.makeStream()
    let (started, starts) = AsyncStream<Void>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { _ in starts.yield(()); await holds.wait() }, cancel: { _ in },
            reject: { envelope, reason in await state.rejectOverloaded(envelope, reason: reason) })
    }
    func open(_ id: String, _ requestID: String) -> [String: Any] {
        ["id": id, "kind": "media_request", "request": ["request_id": requestID,
            "request": ["type": "open", "request": ["owner": ["type": "session", "session_id": "s"]]]]]
    }
    input.yield(try closeTestJSON(open("active", "active-request")))
    for index in 1..<16 {
        input.yield(try closeTestJSON(["id": "busy-\(index)", "kind": "command", "command": ["type": "get_snapshot"]]))
    }
    var startsIterator = started.makeAsyncIterator()
    for _ in 0..<16 { #expect(await startsIterator.next() != nil) }
    input.yield(try closeTestJSON(open("same-request", "active-request")))
    input.yield(try closeTestJSON(open("fresh", "fresh-request")))
    let response = try await closeTestWait(output, id: "fresh")
    let body = try #require(response["response"] as? [String: Any])
    #expect(body["request_id"] as? String == "fresh-request")
    #expect((body["result"] as? [String: Any])?["type"] as? String == "open_failed_without_resource")
    #expect(!output.values.contains(where: { $0["id"] as? String == "same-request" }))
    await holds.release(); input.finish(); await dispatcher.value; starts.finish()
}

@Test func mediaTerminalProofWaitsForTheTargetTaskToReturn() async throws {
    let hold = CloseTestMultipleHolds()
    let output = CloseTestOutput()
    let state = HelperStateStore(storageRoot: FileManager.default.temporaryDirectory.appendingPathComponent("media-task-settle-\(UUID().uuidString)"), writer: LineWriter(emit: { output.append($0) }))
    let (lines, input) = AsyncStream<String>.makeStream()
    let (started, starts) = AsyncStream<Void>.makeStream()
    let (rejected, rejects) = AsyncStream<HelperInputRejection>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { envelope in
            await handleHelperInputEnvelope(envelope, state: state)
            starts.yield(())
            await hold.wait()
        }, cancel: { _ in }, reject: { envelope, reason in
            rejects.yield(reason)
            await state.rejectOverloaded(envelope, reason: reason)
        }, mediaCompleted: { id in await state.publishMediaTerminalResponse(id: id) })
    }
    let media: [String: Any] = ["request_id": "unique-open", "request": ["type": "open", "request": [
        "identity": ["id": "operation", "generation": 1, "service_epoch": 1],
        "owner": ["type": "session", "session_id": "s"], "max_payload_bytes": 1024,
        "request": ["mode": "audio", "intent": "conversation", "audio_source": ["id": "invalid-source"]]]]]
    input.yield(try closeTestJSON(["id": "open-envelope", "kind": "media_request", "request": media]))
    var startsIterator = started.makeAsyncIterator(); _ = await startsIterator.next()
    #expect(output.values.isEmpty, "the actor has a proof but its target task has not returned")
    input.yield(try closeTestJSON(["id": "duplicate-envelope", "kind": "media_request", "request": media]))
    var rejectIterator = rejected.makeAsyncIterator(); #expect(await rejectIterator.next() == .duplicate)
    #expect(output.values.isEmpty)
    await hold.release()
    let response = try await closeTestWait(output, id: "open-envelope")
    let body = try #require(response["response"] as? [String: Any])
    #expect(body["request_id"] as? String == "unique-open")
    #expect((body["result"] as? [String: Any])?["type"] as? String == "open_failed_without_resource")
    #expect(!output.values.contains(where: { $0["id"] as? String == "duplicate-envelope" }))
    input.finish(); await dispatcher.value; starts.finish(); rejects.finish()
}

@Test func mediaStopWaitsForEarlierLeaseTasksAndFencesNewOnes() async throws {
    let hold = CloseTestMultipleHolds()
    let (lines, input) = AsyncStream<String>.makeStream()
    let (started, starts) = AsyncStream<String>.makeStream()
    let (rejected, rejects) = AsyncStream<HelperInputRejection>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { envelope in
            let id = envelope["id"] as? String ?? "?"
            starts.yield(id)
            if id == "reader" { await hold.wait() }
        }, cancel: { _ in }, reject: { _, reason in rejects.yield(reason) })
    }
    func request(_ id: String, _ operation: String, _ lease: String) -> [String: Any] {
        ["id": id, "kind": "media_request", "request": ["request_id": id, "request": ["type": operation, "lease_id": lease]]]
    }
    input.yield(try closeTestJSON(request("reader", "next_input", "lease-a")))
    var startsIterator = started.makeAsyncIterator(); #expect(await startsIterator.next() == "reader")
    input.yield(try closeTestJSON(request("stop", "stop", "lease-a")))
    input.yield(try closeTestJSON(request("late", "next_input", "lease-a")))
    input.yield(try closeTestJSON(request("other", "next_input", "lease-b")))
    var rejectIterator = rejected.makeAsyncIterator(); #expect(await rejectIterator.next() == .duplicate)
    #expect(await startsIterator.next() == "other", "stop must wait for the old lease task")
    await hold.release()
    #expect(await startsIterator.next() == "stop")
    input.finish(); await dispatcher.value; starts.finish(); rejects.finish()
}

@Test func saturatedLeaseStillAdmitsABoundedStopThatJoinsItsReaders() async throws {
    let hold = CloseTestMultipleHolds()
    let (lines, input) = AsyncStream<String>.makeStream()
    let (started, starts) = AsyncStream<String>.makeStream()
    let (rejected, rejects) = AsyncStream<HelperInputRejection>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { envelope in
            let id = envelope["id"] as? String ?? "?"
            starts.yield(id)
            if id.hasPrefix("reader-") { await hold.wait() }
        }, cancel: { _ in }, reject: { _, reason in rejects.yield(reason) })
    }
    func request(_ id: String, _ operation: String) -> [String: Any] {
        ["id": id, "kind": "media_request", "request": ["request_id": id, "request": ["type": operation, "lease_id": "lease-a"]]]
    }
    for index in 0..<16 { input.yield(try closeTestJSON(request("reader-\(index)", "next_input"))) }
    var startsIterator = started.makeAsyncIterator()
    for _ in 0..<16 { #expect((await startsIterator.next())?.hasPrefix("reader-") == true) }
    input.yield(try closeTestJSON(request("stop", "stop")))
    input.yield(try closeTestJSON(request("late", "next_input")))
    var rejectIterator = rejected.makeAsyncIterator(); #expect(await rejectIterator.next() == .duplicate)
    await hold.release()
    #expect(await startsIterator.next() == "stop")
    input.finish(); await dispatcher.value; starts.finish(); rejects.finish()
}

@Test func stopCannotEnterStateBeforeTheOriginSchedulerTaskReturns() async throws {
    let hold = CloseTestHold()
    let (lines, input) = AsyncStream<String>.makeStream()
    let (signals, output) = AsyncStream<String>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { envelope in
            let id = envelope["id"] as? String ?? "?"
            output.yield(id)
            if id == "capture-start" { await hold.wait() }
        }, cancel: { _ in })
    }
    let owner: [String: Any] = ["type": "session", "session_id": "same-owner"]
    func request(_ id: String, _ operation: String, _ generation: Int) -> [String: Any] {
        ["id": id, "kind": "engine_request", "request": ["identity": ["id": id, "generation": generation, "service_epoch": 1],
            "owner": owner, "operation": ["type": operation]]]
    }
    input.yield(try closeTestJSON(request("capture-start", "start_recording", 1)))
    var iterator = signals.makeAsyncIterator()
    #expect(await iterator.next() == "capture-start")
    input.yield(try closeTestJSON(request("capture-stop", "stop_recording", 2)))
    input.yield(try closeTestJSON(["id": "snapshot", "kind": "command", "command": ["type": "get_snapshot"]]))
    #expect(await iterator.next() == "snapshot")
    await hold.release()
    #expect(await iterator.next() == "capture-stop")
    input.finish(); await dispatcher.value; output.finish()
}

@Test func cancellationBeforeActorAdmissionCanStillReturnJoinedNotAdmitted() async throws {
    let output = CloseTestOutput()
    let hold = CloseTestHold()
    let (started, startSignal) = AsyncStream<Void>.makeStream()
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("audio-cancel-before-actor-\(UUID().uuidString)")
    let state = HelperStateStore(storageRoot: root, writer: LineWriter(emit: { output.append($0) }))
    let snapshot = await state.snapshot(message: nil)
    let identity: [String: Any] = ["id": UUID().uuidString.lowercased(), "generation": 1,
        "service_epoch": try #require(snapshot.capabilities?.serviceEpoch)]
    let owner: [String: Any] = ["type": "session", "session_id": "s"]
    let context: [String: Any] = ["helper_instance": try #require(snapshot.helper.instanceId),
        "connection_id": UUID().uuidString.lowercased(), "resource_incarnation": UUID().uuidString.lowercased(),
        "flight_id": UUID().uuidString.lowercased()]
    let request: [String: Any] = ["id": "pre-actor", "kind": "engine_request", "closeContext": context,
        "request": ["identity": identity, "owner": owner, "max_payload_bytes": 1,
                    "operation": ["type": "start_recording", "sample_rate_hz": 16_000, "format": "wav"]]]
    let cancel: [String: Any] = ["id": "pre-actor-close", "kind": "command", "command":
        ["type": "cancel_operation", "identity": identity, "owner": owner, "closeContext": context]]
    let (lines, input) = AsyncStream<String>.makeStream()
    let dispatcher = Task {
        await readInputLines(lines, handle: { envelope in
            if envelope["id"] as? String == "pre-actor" { startSignal.yield(()); await hold.wait() }
            await handleHelperInputEnvelope(envelope, state: state)
        }, cancel: { await cancelHelperInputEnvelope($0, state: state) },
           reject: { await state.rejectOverloaded($0, reason: $1) })
    }
    input.yield(try closeTestJSON(request))
    var iterator = started.makeAsyncIterator(); _ = await iterator.next()
    input.yield(try closeTestJSON(cancel))
    input.yield(try closeTestJSON(["id": "check-pre-actor", "kind": "command", "command": ["type": "get_snapshot"]]))
    _ = try await closeTestWait(output, id: "check-pre-actor")
    #expect(!output.values.contains(where: { $0["id"] as? String == "pre-actor-close" }))
    await hold.release()
    _ = try await closeTestWait(output, id: "pre-actor")
    let response = try await closeTestWait(output, id: "pre-actor-close")
    let value = try #require(response["result"] as? [String: Any])
    #expect((value["closeReceipt"] as? [String: Any])?["disposition"] as? String == "not_admitted")
    input.finish(); await dispatcher.value; startSignal.finish()
}

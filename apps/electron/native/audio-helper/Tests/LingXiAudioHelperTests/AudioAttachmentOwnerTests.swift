import Foundation
import Testing
@testable import LingXiAudioHelper

private actor AttachmentHold {
    private var waiting: CheckedContinuation<Void, Never>?
    private var released = false
    func wait() async { if released { return }; await withCheckedContinuation { waiting = $0 } }
    func release() { released = true; waiting?.resume(); waiting = nil }
}
private actor AttachmentIndex {
    private var value = 0
    func next() -> Int { value += 1; return value }
}
private final class AttachmentOutput: @unchecked Sendable {
    private let lock = NSLock(); private var storage: [[String: Any]] = []
    func append(_ data: Data) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        lock.lock(); storage.append(object); lock.unlock()
    }
    var values: [[String: Any]] { lock.lock(); defer { lock.unlock() }; return storage }
}
private func attached(_ generation: UInt64) -> HelperAudioOwner {
    HelperAudioOwner(type: "attached", attachmentHostID: "12345678-1234-4234-8234-123456789abc", attachmentGeneration: generation, logicalKind: "session", logicalOwnerID: "same-session")
}
private func waitOutput(_ output: AttachmentOutput, _ id: String) async throws {
    for _ in 0..<300 {
        if output.values.contains(where: { $0["id"] as? String == id }) { return }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    throw NSError(domain: "AudioAttachmentOwnerTests", code: 1)
}

@Test func audioOwnerAttachmentKeepsBothScopesAndEndsOnlyItsOwnWork() async throws {
    let first = attached(1), second = attached(2)
    #expect(helperAudioOwnerKey(first) != helperAudioOwnerKey(second))
    let raw: [String: Any] = ["type": "attached", "attachment_host_id": first.attachmentHostID!, "attachment_generation": 1, "logical_kind": "session", "logical_owner_id": "same-session"]
    #expect(parseHelperAudioOwner(raw) == first)
    let holdA = AttachmentHold(), holdB = AttachmentHold(), counter = AttachmentIndex(), output = AttachmentOutput()
    let (started, starts) = AsyncStream<Int>.makeStream()
    let executor: HelperAudioOperationExecutor = { operation, _ in
        guard operation == "speak" else { return nil }
        let index = await counter.next(); starts.yield(index)
        if index == 1 { await holdA.wait() } else { await holdB.wait() }
        return .status(recording: false, playing: false)
    }
    let state = HelperStateStore(storageRoot: FileManager.default.temporaryDirectory.appendingPathComponent("attached-audio-\(UUID().uuidString)"), writer: LineWriter(emit: { output.append($0) }), operationExecutor: executor)
    let epoch = try #require((await state.snapshot(message: nil)).capabilities?.serviceEpoch)
    func identity() -> HelperAudioOperationIdentity { HelperAudioOperationIdentity(id: UUID().uuidString.lowercased(), generation: 1, serviceEpoch: epoch) }
    let firstTask = Task { await state.handleEngineRequest(id: "work-a", identity: identity(), owner: first, operation: ["type": "speak"], configuration: AudioConfigurationNormalizer.defaults, configurationRevision: 0, maxPayloadBytes: 1024) }
    var iterator = started.makeAsyncIterator(); #expect(await iterator.next() == 1)
    let secondTask = Task { await state.handleEngineRequest(id: "work-b", identity: identity(), owner: second, operation: ["type": "speak"], configuration: AudioConfigurationNormalizer.defaults, configurationRevision: 0, maxPayloadBytes: 1024) }
    #expect(await iterator.next() == 2)
    let endA = Task { await state.handleEngineRequest(id: "end-a", identity: identity(), owner: first, operation: ["type": "end_owner"], configuration: AudioConfigurationNormalizer.defaults, configurationRevision: 0, maxPayloadBytes: 1024) }
    try await Task.sleep(nanoseconds: 30_000_000)
    #expect(!output.values.contains(where: { $0["id"] as? String == "end-a" }))
    await holdA.release(); await firstTask.value; await endA.value
    try await waitOutput(output, "end-a")
    let responseA = try #require(output.values.first(where: { $0["id"] as? String == "end-a" })?["result"] as? [String: Any])
    #expect((responseA["result"] as? [String: Any])?["type"] as? String == "owner_ended")
    #expect(!output.values.contains(where: { $0["id"] as? String == "work-b" }))
    let endB = Task { await state.handleEngineRequest(id: "end-b", identity: identity(), owner: second, operation: ["type": "end_owner"], configuration: AudioConfigurationNormalizer.defaults, configurationRevision: 0, maxPayloadBytes: 1024) }
    try await Task.sleep(nanoseconds: 30_000_000)
    #expect(!output.values.contains(where: { $0["id"] as? String == "end-b" }))
    await holdB.release(); await secondTask.value; await endB.value
    try await waitOutput(output, "end-b")
    let responseB = try #require(output.values.first(where: { $0["id"] as? String == "end-b" })?["result"] as? [String: Any])
    #expect((responseB["result"] as? [String: Any])?["type"] as? String == "owner_ended")
    starts.finish()
}

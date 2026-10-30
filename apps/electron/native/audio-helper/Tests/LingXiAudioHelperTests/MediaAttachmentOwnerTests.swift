import Foundation
import Testing
@testable import LingXiAudioHelper

private func attachedOwner() -> [String: Any] {
    ["type": "attached", "attachment_host_id": "12345678-1234-4234-8234-123456789abc",
     "attachment_generation": 1, "logical_kind": "session", "logical_owner_id": "session-a"]
}

private func mediaOpen(_ owner: [String: Any]) -> [String: Any] {
    ["request_id": "request-a", "request": ["type": "open", "request": [
        "identity": ["id": "operation-a", "generation": 1, "service_epoch": 10],
        "owner": owner, "max_payload_bytes": 1024,
        "request": ["mode": "audio", "intent": "conversation", "audio_source": ["id": "media-runtime:microphone"]],
    ]]]
}

private actor CompletionProbe {
    private(set) var completed = false
    func finish() { completed = true }
}

private final class MediaOutputProbe: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [Data] = []
    func append(_ value: Data) { lock.lock(); defer { lock.unlock() }; storage.append(value) }
    var values: [Data] { lock.lock(); defer { lock.unlock() }; return storage }
}

struct MediaAttachmentOwnerTests {
    @Test
    func mediaOpenRetainsTheCompleteAttachmentAndRejectsOtherOwnerKeys() throws {
        let parsed = try parseHelperMediaDeviceRequest(mediaOpen(attachedOwner()))
        guard case let .open(open) = parsed.operation else { Issue.record("expected media open"); return }
        #expect(open.owner.attachmentHostID == "12345678-1234-4234-8234-123456789abc")
        #expect(open.owner.attachmentGeneration == 1)
        #expect(open.owner.logicalKind == "session")
        #expect(open.owner.logicalOwnerID == "session-a")
        #expect(parseHelperAudioOwner(attachedOwner()) == open.owner)
        let key = helperAudioOwnerKey(open.owner)
        for (field, value) in [("attachment_host_id", "12345678-1234-4234-8234-123456789abd" as Any),
                               ("attachment_generation", 2), ("logical_kind", "ui"), ("logical_owner_id", "session-b")] {
            var changed = attachedOwner(); changed[field] = value
            let candidate = try #require(parseHelperAudioOwner(changed, allowAttachment: true))
            #expect(helperAudioOwnerKey(candidate) != key)
        }
        #expect(helperAudioOwnerKey(HelperAudioOwner(type: "session", sessionID: "session-a")) != key)
    }

    @Test
    func mediaOpenRejectsMalformedOrAmbiguousAttachmentFields() {
        var candidates: [[String: Any]] = []
        for (field, value) in [("attachment_host_id", "not-a-uuid" as Any),
                               ("attachment_host_id", "12345678-1234-4234-8234-123456789abc\n"), ("attachment_generation", 0),
                               ("attachment_generation", true), ("attachment_generation", 1.5),
                               ("attachment_generation", 9_007_199_254_740_992 as UInt64),
                               ("logical_kind", "attached"), ("logical_owner_id", ""),
                               ("logical_owner_id", String(repeating: "界", count: 86))] {
            var changed = attachedOwner(); changed[field] = value; candidates.append(changed)
        }
        var extra = attachedOwner(); extra["extra"] = "ambiguous"; candidates.append(extra)
        var missing = attachedOwner(); missing.removeValue(forKey: "logical_owner_id"); candidates.append(missing)
        for candidate in candidates {
            #expect(throws: HelperError.self) { try parseHelperMediaDeviceRequest(mediaOpen(candidate)) }
        }
    }

    @Test
    func callbackFenceJoinsAlreadyAdmittedWorkAndRejectsLateAdmission() async throws {
        let fence = HelperMediaCallbackFence()
        #expect(fence.admit())
        #expect(fence.admit()) // Includes the asynchronous playback callback hop.
        fence.close()
        #expect(!fence.admit())
        let probe = CompletionProbe()
        let draining = Task { await fence.waitForDrain(); await probe.finish() }
        fence.finish()
        try await Task.sleep(nanoseconds: 20_000_000)
        #expect(await !probe.completed)
        fence.finish()
        await draining.value
        #expect(await probe.completed)
    }

    @Test
    func terminalMediaResponsesEncodeTypedNoResourceAndExactCloseIdentity() throws {
        let owner = try #require(parseHelperAudioOwner(attachedOwner(), allowAttachment: true))
        let identity = HelperAudioOperationIdentity(id: "operation-a", generation: 1, serviceEpoch: 10)
        let receipt = HelperMediaCloseReceipt(leaseID: "lease-a", openRequestID: "open-a", owner: owner, identity: identity)
        let stopped = HelperMediaOutputEnvelope(id: "flight-stop", response: .init(requestID: "stop-a", result: .stopped(receipt)))
        let json = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(stopped)) as? [String: Any])
        let proof = try #require(json["close_receipt"] as? [String: Any])
        #expect(proof["lease_id"] as? String == "lease-a")
        #expect(proof["open_request_id"] as? String == "open-a")
        #expect((proof["owner"] as? [String: Any])?["attachment_generation"] as? Int == 1)
        #expect((proof["identity"] as? [String: Any])?["service_epoch"] as? Int == 10)
        let failure = HelperMediaDeviceResponseDto(requestID: "open-a", result: .openFailedWithoutResource(.init(kind: "busy", message: "not admitted")))
        let failureJSON = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(failure)) as? [String: Any])
        #expect((failureJSON["result"] as? [String: Any])?["type"] as? String == "open_failed_without_resource")
    }

    @Test
    func unadmittedOpenProofReservesRequestIdentityExactlyOnce() async throws {
        let output = MediaOutputProbe()
        let state = HelperStateStore(storageRoot: FileManager.default.temporaryDirectory.appending(path: "media-proof-\(UUID().uuidString)"),
                                     writer: LineWriter(emit: { output.append($0) }))
        let envelope: [String: Any] = ["id": "envelope-1", "kind": "media_request", "request": mediaOpen(attachedOwner())]
        #expect(await state.rejectUnadmittedMediaOpen(envelope))
        #expect(await !state.rejectUnadmittedMediaOpen(envelope))
        let response = try #require(output.values.first)
        let json = try #require(JSONSerialization.jsonObject(with: response) as? [String: Any])
        let body = try #require(json["response"] as? [String: Any])
        #expect(body["request_id"] as? String == "request-a")
        #expect((body["result"] as? [String: Any])?["type"] as? String == "open_failed_without_resource")
        #expect(output.values.count == 1)
    }

    @Test
    func malformedDuplicateOpenNeverProvesAnExistingIdentityHasNoResource() async throws {
        let output = MediaOutputProbe()
        let state = HelperStateStore(storageRoot: FileManager.default.temporaryDirectory.appending(path: "media-malformed-\(UUID().uuidString)"),
                                     writer: LineWriter(emit: { output.append($0) }))
        let first: [String: Any] = ["id": "first", "kind": "media_request", "request": mediaOpen(attachedOwner())]
        #expect(await state.rejectUnadmittedMediaOpen(first))
        var malformed = mediaOpen(["type": "attached"])
        await state.handleMediaDeviceRequest(id: "duplicate", rawRequest: malformed)
        malformed["request_id"] = "fresh-request"
        await state.handleMediaDeviceRequest(id: "fresh", rawRequest: malformed)
        #expect(output.values.count == 2, "terminal proof must wait for the scheduler watcher")
        await state.publishMediaTerminalResponse(id: "fresh")
        let values = try output.values.map { try #require(JSONSerialization.jsonObject(with: $0) as? [String: Any]) }
        func resultType(_ id: String) -> String? {
            guard let envelope = values.first(where: { $0["id"] as? String == id }),
                  let response = envelope["response"] as? [String: Any],
                  let result = response["result"] as? [String: Any] else { return nil }
            return result["type"] as? String
        }
        #expect(resultType("duplicate") == "error")
        #expect(resultType("fresh") == "open_failed_without_resource")
    }

}

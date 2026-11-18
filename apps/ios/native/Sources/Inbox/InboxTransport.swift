import CryptoKit
import FirebaseAuth
import FirebaseFirestore
import Foundation
import Security

enum InboxTransportError: Error {
    case response(InboxAuthorityFailure)
    case invalidData

    var authority: InboxAuthorityFailure? {
        if case let .response(failure) = self, failure.authorityDenied { return failure }
        return nil
    }
}

@MainActor
struct InboxTransport {
    let uid: String

    func request<T: Decodable>(_ body: [String: Any], endpoint: String = "/v1/inbox") async throws -> T {
        guard let user = Auth.auth().currentUser, user.uid == uid else { throw CancellationError() }
        let token = try await user.getIDToken()
        var request = URLRequest(url: URL(string: "https://lingxios.net" + endpoint)!)
        request.httpMethod = "POST"
        request.timeoutInterval = 20
        request.setValue("Bearer " + token, forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await Self.session.data(for: request)
        guard uid == Auth.auth().currentUser?.uid, !Task.isCancelled else { throw CancellationError() }
        guard let http = response as? HTTPURLResponse else { throw InboxTransportError.invalidData }
        guard (200..<300).contains(http.statusCode) else {
            throw InboxTransportError.response(InboxAuthorityFailure(status: http.statusCode, data: data))
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    struct Ack: Decodable {}
    struct Item: Decodable { let item: InboxRow }
    struct AccountPreferences: Decodable { let dmReceipts: Bool }
    struct AssistantSource: Decodable { let enabled: Bool; let generation: Int }
    struct PeerReceipt: Decodable { let readThrough: Int? }
    struct ReadBatchStatus: Decodable {
        enum State: String, Decodable { case queued, running, complete, expired }
        let jobId: String
        let state: State
    }
    struct Notices: Decodable {
        struct Notice: Decodable {
            let id: String
            let key: String
            let generation: Int
            let through: Int
            let kind: String
            let lastActivityAt: Int64
            let deliverAfter: Int64
            let deliverable: Bool
            let messageId: String?
            let taskId: String?
        }
        let items: [Notice]
    }

    func noticeMessage(_ preview: InboxNotificationPreview) async throws -> TeamMessage? {
        let ref = preview.row.ref
        guard ref.kind != .assistant, let before = preview.target.exclusiveBefore else { return nil }
        let body: [String: Any] = [
            "action": "history", "generation": ref.generation, "conversationId": ref.conversationId,
            "before": before, ref.kind == .service ? "serviceId" : "teamId": ref.sourceId
        ]
        let history: HostedServiceHistory = try await request(body, endpoint: "/v1/teams/messages")
        return history.messages.reversed().first { message in
            message.recalled != true && preview.target.matches(seq: message.seq, messageID: message.id,
                taskIDs: (message.taskIds ?? []) + (message.taskId.map { [$0] } ?? []))
        }
    }

    private static let session = URLSession(configuration: .ephemeral, delegate: TeamMessageRedirectPolicy(), delegateQueue: nil)

    struct OperationResult {
        var row: InboxRow?
        var source: AssistantSource?
    }

    func operation(_ operation: InboxOperation) async throws -> OperationResult {
        var body: [String: Any] = [
            "action": operation.action.rawValue, "key": operation.key, "generation": operation.generation
        ]
        var binding: AssistantSource?
        switch operation.action {
        case .markRead:
            body["operationId"] = operation.id
            body["through"] = operation.through
            body["manualUnreadRevision"] = operation.manualUnreadRevision
            body["observedPendingActionRevision"] = operation.observedPendingActionRevision
            body["observedFailureRevision"] = operation.observedFailureRevision
        case .markUnread:
            body["operationId"] = operation.id
            body["expectedManualUnreadRevision"] = operation.expectedManualUnreadRevision
        case .preferences:
            body["expectedPreferencesRevision"] = operation.expectedPreferencesRevision
            if let patch = operation.preferences {
                body["patch"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(patch))
            }
        case .peerReceipts:
            body = ["action": "accountPreferences", "dmReceipts": operation.enabled ?? false]
        case .assistantSync:
            guard let ref = InboxRef(key: operation.key, generation: operation.generation) else {
                throw InboxTransportError.invalidData
            }
            body = ["action": "assistantSource", "sourceId": ref.sourceId,
                    "sourceSecret": try sourceSecret(ref.sourceId), "enabled": operation.enabled ?? false]
            if let revision = operation.consentRevision { body["consentRevision"] = revision }
        case .assistantSummary:
            guard let ref = InboxRef(key: operation.key, generation: operation.generation), let summary = operation.summary else {
                throw InboxTransportError.invalidData
            }
            let activity = summary.activity
            let secret = try sourceSecret(ref.sourceId)
            var registration: [String: Any] = [
                "action": "assistantSource", "sourceId": ref.sourceId, "sourceSecret": secret, "enabled": true
            ]
            if let revision = operation.consentRevision { registration["consentRevision"] = revision }
            let source: AssistantSource = try await request(registration)
            guard source.enabled else { return OperationResult(source: source) }
            binding = source
            body = ["action": "assistantPublish", "sourceId": ref.sourceId,
                    "sourceSecret": secret, "conversationId": ref.conversationId,
                    "generation": source.generation, "baseline": activity.baselineCursor,
                    "sourceRevision": activity.sourceRevision, "sourceCursor": activity.cursor,
                    "lastActivityAt": activity.lastMeaningfulAt, "state": summary.state,
                    "pendingActionCount": activity.pendingActionCount, "sourceLabel": "iOS",
                    "previewKind": activity.previewKind.map { $0 as Any } ?? NSNull()]
            if let pendingAt = activity.pendingActionAt { body["pendingActionAt"] = pendingAt }
            if let revision = activity.pendingActionRevision { body["pendingActionRevision"] = revision }
            if let failureAt = activity.failureAt { body["failureAt"] = failureAt }
            if let revision = activity.failureRevision { body["failureRevision"] = revision }
            if let revision = operation.conversationConsentRevision { body["conversationConsentRevision"] = revision }
        case .assistantRemove:
            guard let ref = InboxRef(key: operation.key, generation: operation.generation),
                  let revision = operation.conversationConsentRevision else { throw InboxTransportError.invalidData }
            body = ["action": "assistantRemove", "sourceId": ref.sourceId,
                    "sourceSecret": try sourceSecret(ref.sourceId), "conversationId": ref.conversationId,
                    "generation": operation.generation, "sourceRevision": operation.sourceRevision ?? 0,
                    "conversationConsentRevision": revision]
        }
        switch operation.action {
        case .markRead, .markUnread, .preferences:
            let result: Item = try await request(body)
            return OperationResult(row: result.item.valid ? result.item : nil)
        case .assistantSync:
            let source: AssistantSource = try await request(body)
            return OperationResult(source: source)
        case .assistantSummary, .assistantRemove, .peerReceipts:
            let _: Ack = try await request(body)
            return OperationResult(source: binding)
        }
    }

    private func sourceSecret(_ sourceID: String) throws -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: (Bundle.main.bundleIdentifier ?? "com.lingxi.code") + ".inbox-source",
            kSecAttrAccount as String: InboxFiles.accountKey(uid) + "." + sourceID,
            kSecAttrSynchronizable as String: false
        ]
        var value: CFTypeRef?
        let status = SecItemCopyMatching(query.merging([kSecReturnData as String: true]) { _, new in new } as CFDictionary, &value)
        if status == errSecSuccess, let data = value as? Data, let secret = String(data: data, encoding: .utf8) { return secret }
        guard status == errSecItemNotFound else { throw InboxTransportError.invalidData }
        let secret = PushInstallationIdentity.create().secret
        let attributes: [String: Any] = [
            kSecValueData as String: Data(secret.utf8),
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        guard SecItemAdd(query.merging(attributes) { _, new in new } as CFDictionary, nil) == errSecSuccess else {
            throw InboxTransportError.invalidData
        }
        return secret
    }
}

struct InboxFiles: Sendable {
    static let cacheBudget = 4 * 1024 * 1024
    let stateURL: URL?
    let cacheURL: URL?

    init(uid: String?) {
        let component = Self.accountKey(uid ?? "guest")
        stateURL = URL.applicationSupportDirectory.appending(path: "UnifiedInbox/" + component + "/state.json")
        cacheURL = URL.cachesDirectory.appending(path: "UnifiedInbox/" + component + "/rows.json")
    }

    static func accountKey(_ account: String) -> String {
        SHA256.hash(data: Data(account.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    func load() throws -> InboxAccountState {
        guard let stateURL, FileManager.default.fileExists(atPath: stateURL.path) else { return InboxAccountState() }
        return try JSONDecoder().decode(InboxAccountState.self, from: Data(contentsOf: stateURL))
    }

    func cached(now: Int64) -> [InboxCachedRow] {
        guard let cacheURL, let values = try? Data(contentsOf: cacheURL), values.count <= Self.cacheBudget else { return [] }
        return (try? JSONDecoder().decode([InboxCachedRow].self, from: values))?.filter { $0.isAuthorized(at: now) } ?? []
    }

    func save(_ state: InboxAccountState) throws {
        guard let stateURL else { return }
        try Self.write(JSONEncoder().encode(state), to: stateURL)
    }

    func saveCache(_ rows: [InboxCachedRow]) {
        guard let cacheURL else { return }
        let encoder = JSONEncoder()
        var retained: [InboxCachedRow] = []
        var bytes = 2
        for row in rows {
            guard let size = try? encoder.encode(row).count else { continue }
            bytes += size + 1
            if bytes > Self.cacheBudget { break }
            retained.append(row)
        }
        guard let data = try? encoder.encode(retained), (try? Self.write(data, to: cacheURL)) != nil else { return }
        let root = cacheURL.deletingLastPathComponent().deletingLastPathComponent()
        let folders = (try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? []
        let entries = folders.compactMap { folder -> (URL, Int, Date)? in
            let file = folder.appending(path: "rows.json")
            guard let values = try? file.resourceValues(forKeys: [.contentModificationDateKey, .fileSizeKey]),
                  let bytes = values.fileSize, let date = values.contentModificationDate else { return nil }
            return (file, bytes, date)
        }
        var totalBytes = entries.reduce(0) { $0 + $1.1 }
        for (file, size, _) in entries.sorted(by: { $0.2 < $1.2 }) where totalBytes > Self.cacheBudget && file != cacheURL {
            if (try? FileManager.default.removeItem(at: file)) != nil { totalBytes -= size }
        }
    }

    private static func write(_ data: Data, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
}

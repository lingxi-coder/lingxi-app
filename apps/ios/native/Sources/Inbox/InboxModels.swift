import Foundation

enum InboxKind: String, Codable, Sendable { case assistant, team, service }

enum InboxCategory: String, Codable, CaseIterable, Sendable {
    case assistant, friend, team, service
}

enum InboxNotificationMode: String, Codable, Sendable {
    case all, mentions, none
}

struct InboxRef: Codable, Equatable, Hashable, Identifiable, Sendable {
    let kind: InboxKind
    let sourceId: String
    let conversationId: String
    let generation: Int

    var id: String { key }
    var key: String { "\(kind.rawValue):\(sourceId):\(conversationId)" }
    var valid: Bool { generation > 0 && Self.safe(sourceId) && Self.safe(conversationId) }

    init(kind: InboxKind, sourceId: String, conversationId: String, generation: Int) {
        self.kind = kind
        self.sourceId = sourceId
        self.conversationId = conversationId
        self.generation = generation
    }

    init?(key: String, generation: Int) {
        let parts = key.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count == 3, let kind = InboxKind(rawValue: String(parts[0])) else { return nil }
        self.init(kind: kind, sourceId: String(parts[1]), conversationId: String(parts[2]), generation: generation)
        guard valid else { return nil }
    }

    static func safe(_ value: String) -> Bool {
        (1...128).contains(value.utf8.count) && value.utf8.allSatisfy { byte in
            (48...57).contains(byte) || (65...90).contains(byte) || (97...122).contains(byte) || byte == 45 || byte == 95
        }
    }
}

struct InboxRow: Codable, Equatable, Identifiable, Sendable {
    var schemaVersion = 1
    var key: String
    var ref: InboxRef
    var category: InboxCategory
    var title: String
    var preview = ""
    var context: String?
    var sourceLabel: String?
    var peerUid: String?
    var remoteHostId: String?
    var lastActivityAt: Int64 = 0
    var sourceCursor = 0
    var sourceRevision = 0
    var readThrough = 0
    var unreadCount = 0
    var mentionCount = 0
    var pendingActionCount = 0
    var pendingActionRevision: Int?
    var failureRevision: Int?
    var attentionKind: String?
    var state = "idle"
    var manualUnread = false
    var manualUnreadRevision = 0
    var stateRevision = 0
    var preferencesRevision: Int?
    var offline = false
    var pinOrder: Int64?
    var notificationMode = InboxNotificationMode.mentions
    var archived = false

    var id: String { key }
    var isUnread: Bool { manualUnread || unreadCount > 0 }
    var needsAttention: Bool { pendingActionCount > 0 || mentionCount > 0 || attentionKind != nil }
    var valid: Bool {
        schemaVersion == 1 && ref.valid && key == ref.key &&
        (ref.kind.rawValue == category.rawValue || ref.kind == .team && category == .friend) &&
        sourceCursor >= 0 && (0...sourceCursor).contains(readThrough) &&
        sourceRevision >= 0 && stateRevision >= 0 && manualUnreadRevision >= 0
    }

    func frozenRead(through: Int? = nil) -> InboxRead {
        InboxRead(key: key, generation: ref.generation,
                  through: min(sourceCursor, max(0, through ?? sourceCursor)),
                  manualUnreadRevision: manualUnreadRevision,
                  observedPendingActionRevision: pendingActionRevision,
                  observedFailureRevision: failureRevision)
    }

    static func ordered(_ left: Self, _ right: Self) -> Bool {
        let lhs = left.pinOrder ?? Int64.max
        let rhs = right.pinOrder ?? Int64.max
        if lhs != rhs { return lhs < rhs }
        if left.lastActivityAt != right.lastActivityAt { return left.lastActivityAt > right.lastActivityAt }
        return left.key < right.key
    }
}

enum InboxFilter: String, CaseIterable, Identifiable {
    case all, unread, attention
    var id: Self { self }

    func accepts(_ row: InboxRow, archivedOnly: Bool) -> Bool {
        if archivedOnly && !row.archived { return false }
        switch self {
        case .all: return row.archived == archivedOnly
        case .unread: return row.isUnread
        case .attention: return row.needsAttention
        }
    }
}

enum InboxTitleSearch {
    static func normalize(_ title: String) -> String {
        title.precomposedStringWithCompatibilityMapping.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    static func prefix(_ input: String) -> String? {
        var units = normalize(input).utf16.prefix(120)
        if let last = units.last, (0xD800...0xDBFF).contains(last) { units = units.dropLast() }
        return units.isEmpty ? nil : String(decoding: units, as: UTF16.self)
    }
}

struct InboxCursor: Codable, Equatable, Sendable {
    let lastActivityAt: Int64
    let key: String
    var pinOrder: Int64?
    var titleSort: String?

    var requestValues: [String: Any] {
        var values: [String: Any] = ["lastActivityAt": lastActivityAt, "key": key]
        if let titleSort { values["titleSort"] = titleSort }
        else { values["pinOrder"] = pinOrder.map { $0 as Any } ?? NSNull() }
        return values
    }
}

struct InboxListRequest {
    let filter: InboxFilter
    let category: InboxCategory?
    let archived: Bool
    let searchPrefix: String?
    let after: InboxCursor?

    var requestValues: [String: Any] {
        var values: [String: Any] = ["action": "list", "filter": filter.rawValue, "archived": archived]
        values["category"] = category?.rawValue ?? "messages"
        if let searchPrefix { values["searchPrefix"] = searchPrefix }
        if let after { values["after"] = after.requestValues }
        return values
    }
}

struct InboxPage: Codable, Sendable {
    let items: [InboxRow]
    let next: InboxCursor?
}

struct InboxRead: Codable, Equatable, Sendable {
    let key: String
    let generation: Int
    let through: Int
    let manualUnreadRevision: Int
    var observedPendingActionRevision: Int?
    var observedFailureRevision: Int?
}

struct InboxPreferencePatch: Codable, Equatable, Sendable {
    var pinOrder: Int64?
    var clearPin: Bool?
    var notificationMode: InboxNotificationMode?
    var archived: Bool?

    private enum CodingKeys: String, CodingKey { case pinOrder, notificationMode, archived }

    init(pinOrder: Int64? = nil, clearPin: Bool? = nil, notificationMode: InboxNotificationMode? = nil, archived: Bool? = nil) {
        self.pinOrder = pinOrder
        self.clearPin = clearPin
        self.notificationMode = notificationMode
        self.archived = archived
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        pinOrder = try values.decodeIfPresent(Int64.self, forKey: .pinOrder)
        clearPin = values.contains(.pinOrder) && pinOrder == nil ? true : nil
        notificationMode = try values.decodeIfPresent(InboxNotificationMode.self, forKey: .notificationMode)
        archived = try values.decodeIfPresent(Bool.self, forKey: .archived)
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        if clearPin == true { try values.encodeNil(forKey: .pinOrder) }
        else { try values.encodeIfPresent(pinOrder, forKey: .pinOrder) }
        try values.encodeIfPresent(notificationMode, forKey: .notificationMode)
        try values.encodeIfPresent(archived, forKey: .archived)
    }
}

struct InboxOperation: Codable, Equatable, Identifiable, Sendable {
    enum Action: String, Codable, Sendable { case markRead, markUnread, preferences, assistantSummary, assistantRemove, assistantSync, peerReceipts }
    var id = UUID().uuidString
    var action: Action
    var key: String
    var generation: Int
    var through: Int?
    var manualUnreadRevision: Int?
    var expectedManualUnreadRevision: Int?
    var expectedPreferencesRevision: Int?
    var observedPendingActionRevision: Int?
    var observedFailureRevision: Int?
    var preferences: InboxPreferencePatch?
    var summary: InboxAssistantSummary?
    var enabled: Bool?
    var consentRevision: Int?
    var conversationConsentRevision: Int?
    var sourceRevision: Int?

    init(markingUnread row: InboxRow) {
        action = .markUnread
        key = row.key
        generation = row.ref.generation
        expectedManualUnreadRevision = row.manualUnreadRevision
    }

    init(preferences: InboxPreferencePatch, row: InboxRow) {
        action = .preferences
        key = row.key
        generation = row.ref.generation
        self.preferences = preferences
        expectedPreferencesRevision = row.preferencesRevision ?? 0
    }

    init(_ read: InboxRead) {
        action = .markRead
        key = read.key
        generation = read.generation
        through = read.through
        manualUnreadRevision = read.manualUnreadRevision
        observedPendingActionRevision = read.observedPendingActionRevision
        observedFailureRevision = read.observedFailureRevision
    }

    init(action: Action, ref: InboxRef, preferences: InboxPreferencePatch? = nil, summary: InboxAssistantSummary? = nil) {
        self.action = action
        key = ref.key
        generation = ref.generation
        self.preferences = preferences
        self.summary = summary
    }

    init(action: Action, enabled: Bool) {
        self.action = action
        key = ""
        generation = 0
        self.enabled = enabled
    }

    /// A reminder and a read are ordering barriers, even when other threads are interleaved.
    func coalescing(_ newer: Self) -> Self? {
        guard key == newer.key, generation == newer.generation, action == newer.action else { return nil }
        var result = newer
        switch action {
        case .markRead:
            guard manualUnreadRevision == newer.manualUnreadRevision,
                  observedPendingActionRevision == newer.observedPendingActionRevision,
                  observedFailureRevision == newer.observedFailureRevision else { return nil }
            result.through = max(through ?? 0, newer.through ?? 0)
        case .preferences:
            guard expectedPreferencesRevision == newer.expectedPreferencesRevision else { return nil }
            let previous = preferences
            var next = newer.preferences ?? InboxPreferencePatch()
            if next.pinOrder == nil && next.clearPin != true {
                next.pinOrder = previous?.pinOrder
                next.clearPin = previous?.clearPin
            }
            next.notificationMode = next.notificationMode ?? previous?.notificationMode
            next.archived = next.archived ?? previous?.archived
            result.preferences = next
        case .assistantSummary, .assistantRemove, .assistantSync, .peerReceipts: break
        case .markUnread: return nil
        }
        return result
    }
}

struct InboxCachedRow: Codable, Equatable, Sendable {
    static let lease: Int64 = 300_000
    static let renewalLead: Int64 = 30_000
    let row: InboxRow
    let verifiedAt: Int64

    func isAuthorized(at now: Int64) -> Bool {
        row.valid && verifiedAt >= 0 && verifiedAt <= now && now - verifiedAt < Self.lease
    }

    static func renewAt(verifiedAt: Int64, attemptedAt: Int64?) -> Int64 {
        max(verifiedAt + lease - renewalLead, (attemptedAt ?? 0) + renewalLead)
    }
}

struct InboxAuthorityFailure: Equatable, Sendable {
    let status: Int
    let code: String?

    init(status: Int, data: Data) {
        self.status = status
        struct Envelope: Decodable {
            struct Failure: Decodable { let code: String }
            let error: Failure
        }
        code = (try? JSONDecoder().decode(Envelope.self, from: data))?.error.code
    }

    var accountUnauthorized: Bool { status == 401 }
    var authorityDenied: Bool {
        [401, 403, 404, 410].contains(status) || code == "STALE_MEMBERSHIP"
    }

    func revokes(_ ref: InboxRef, rejected: InboxRef) -> Bool {
        guard authorityDenied else { return false }
        if accountUnauthorized { return true }
        if code == "STALE_MEMBERSHIP" {
            return ref.kind == rejected.kind && ref.sourceId == rejected.sourceId && ref.generation == rejected.generation
        }
        return ref == rejected
    }
}

enum InboxStorageError: Error { case queueFull }

struct InboxAssistantActivity: Codable, Equatable, Sendable {
    let sourceId: String
    let cursor: Int
    let baselineCursor: Int
    let lastMeaningfulAt: Int64
    let previewKind: String?
    let pendingActionCount: Int
    let failed: Bool
    let sourceRevision: Int
    var latestResultUuids: [String]?
    var pendingActionKind: ActionKind?
    var pendingActionAt: Int64?
    var pendingActionRevision: Int?
    var failureAt: Int64?
    var failureRevision: Int?

    enum ActionKind: String, Codable, Sendable { case approval, input }
}

struct InboxAssistantReadEntry: Codable, Equatable, Sendable {
    let cursor: Int
    let resultUuids: [String]
}

struct InboxAssistantReadPage: Decodable, Sendable {
    let items: [InboxAssistantReadEntry]
    let cursor: Int
    let baselineCursor: Int
    let hasMore: Bool
}

struct InboxAssistantExposure: Equatable {
    let ref: InboxRef
    let sourceCursor: Int
    let sourceRevision: Int
    let readThrough: Int
    let visible: Set<UUID>
}

/// The frontier follows contiguous committed semantic results. Scroll
/// exposure only contributes UUIDs for the current presentation.
struct InboxAssistantReadWindow {
    private var observed: [UUID] = []
    private(set) var through = 0

    mutating func confirm(_ visible: Set<UUID>) {
        let old = Set(observed)
        observed.append(contentsOf: visible.subtracting(old).sorted { $0.uuidString < $1.uuidString })
        observed = Array(observed.suffix(200))
    }

    mutating func advance(_ entries: [InboxAssistantReadEntry], after baseline: Int) -> Int {
        through = max(through, baseline)
        let visible = Set(observed)
        var consumed: Set<UUID> = []
        for entry in entries where entry.cursor > through {
            guard entry.cursor == through + 1,
                  let raw = entry.resultUuids.last, let result = UUID(uuidString: raw), visible.contains(result) else { break }
            through = entry.cursor
            consumed.insert(result)
        }
        observed.removeAll { consumed.contains($0) }
        return through
    }
}


struct InboxAssistantSummary: Codable, Equatable, Sendable {
    let activity: InboxAssistantActivity

    var state: String {
        if activity.pendingActionCount > 0 {
            return activity.pendingActionKind == .approval ? "waiting_for_approval" : "waiting_for_input"
        }
        if activity.failed { return "failed" }
        return activity.previewKind == nil ? "idle" : "completed"
    }

    func updating(_ input: InboxRow) -> InboxRow {
        let read = min(activity.cursor, max(input.readThrough, activity.baselineCursor))
        var row = input
        row.lastActivityAt = max(input.lastActivityAt, activity.lastMeaningfulAt, activity.pendingActionAt ?? 0,
                                 activity.failed ? activity.failureAt ?? 0 : 0)
        row.sourceCursor = activity.cursor
        row.sourceRevision = activity.sourceRevision
        row.readThrough = read
        row.unreadCount = max(0, activity.cursor - read)
        row.pendingActionCount = activity.pendingActionCount
        row.pendingActionRevision = activity.pendingActionRevision
        row.failureRevision = activity.failureRevision
        row.attentionKind = activity.failed ? "failure" : nil
        row.state = state
        return row
    }
}

struct InboxAssistantSharing: Codable, Equatable, Sendable {
    let enabled: Bool
    let revision: Int
}

struct InboxReadBatch: Codable, Equatable, Sendable {
    var id = UUID().uuidString
    let requestedAt: Int64

    var createRequest: [String: Any] { ["action": "markAllReadSnapshot", "operationId": id, "requestedAt": requestedAt, "category": "messages"] }
    var statusRequest: [String: String] { ["action": "readBatchStatus", "jobId": id] }
}

struct InboxAccountState: Codable, Equatable, Sendable {
    struct Personal: Codable, Equatable, Sendable {
        var generation: Int
        var readThrough = 0
        var manualUnread = false
        var manualUnreadRevision = 0
        var pinOrder: Int64?
        var notificationMode = InboxNotificationMode.mentions
        var archived = false
        var preferencesRevision: Int?
    }

    var queue: [InboxOperation] = []
    var personal: [String: Personal] = [:]
    var drafts: [String: Data] = [:]
    var delivered: [String: Int64] = [:]
    var assistantSync = false
    var assistantGenerations: [String: Int] = [:]
    var assistantConsentRevision: Int?
    var assistantSharing: [String: InboxAssistantSharing]?
    var readBatch: InboxReadBatch?
    var peerReceipts = false

    static let maxEntries = 10_000
    static let maxQueueBytes = 16 * 1024 * 1024
    static let deliveryLifetime: Int64 = 7 * 24 * 60 * 60 * 1_000

    mutating func enqueue(_ ops: [InboxOperation], inFlight: String? = nil,
                          maxEntries: Int = Self.maxEntries, maxBytes: Int = Self.maxQueueBytes) throws {
        var candidate = queue
        for op in ops {
            if let index = candidate.lastIndex(where: { $0.key == op.key && $0.generation == op.generation }),
               candidate[index].id != inFlight, let merged = candidate[index].coalescing(op) {
                candidate[index] = merged
            } else {
                candidate.append(op)
            }
        }
        guard candidate.count <= maxEntries, try JSONEncoder().encode(candidate).count <= maxBytes else {
            throw InboxStorageError.queueFull
        }
        queue = candidate
    }

    func sharesAssistant(_ key: String) -> Bool { assistantSharing?[key]?.enabled ?? true }

    @discardableResult
    mutating func setSharing(_ enabled: Bool, for key: String) -> InboxAssistantSharing {
        let previous = assistantSharing?[key]
        let choice = InboxAssistantSharing(enabled: enabled, revision: (previous?.revision ?? 0) + 1)
        var choices = assistantSharing ?? [:]
        choices[key] = choice
        assistantSharing = choices
        return choice
    }

    mutating func beginReadBatch(_ batch: InboxReadBatch, localRows: [InboxRow]) {
        for row in localRows where row.isUnread {
            applyLocal(InboxOperation(row.frozenRead()), row: row)
        }
        readBatch = batch
    }

    mutating func applyLocal(_ operation: InboxOperation, row: InboxRow) {
        var value = personal[row.key].flatMap { $0.generation == row.ref.generation || row.ref.kind == .assistant ? $0 : nil }
            ?? Personal(generation: row.ref.generation, readThrough: row.readThrough,
                        manualUnread: row.manualUnread, manualUnreadRevision: row.manualUnreadRevision,
                        pinOrder: row.pinOrder, notificationMode: row.notificationMode, archived: row.archived,
                        preferencesRevision: row.preferencesRevision)
        value.generation = row.ref.generation
        switch operation.action {
        case .markRead:
            value.readThrough = max(value.readThrough, operation.through ?? 0)
            if value.manualUnreadRevision == operation.manualUnreadRevision { value.manualUnread = false }
        case .markUnread:
            if operation.expectedManualUnreadRevision == nil ||
                operation.expectedManualUnreadRevision == max(value.manualUnreadRevision, row.manualUnreadRevision) {
                value.manualUnreadRevision = max(value.manualUnreadRevision, row.manualUnreadRevision) + 1
                value.manualUnread = true
            }
        case .preferences:
            let revision = max(value.preferencesRevision ?? 0, row.preferencesRevision ?? 0)
            if let patch = operation.preferences,
               operation.expectedPreferencesRevision == nil || operation.expectedPreferencesRevision == revision {
                value.preferencesRevision = revision + 1
                if patch.clearPin == true { value.pinOrder = nil }
                if let pinOrder = patch.pinOrder { value.pinOrder = pinOrder }
                if let mode = patch.notificationMode { value.notificationMode = mode }
                if let archived = patch.archived { value.archived = archived }
            }
        case .assistantSummary, .assistantRemove, .assistantSync, .peerReceipts: break
        }
        personal[row.key] = value
    }

    mutating func discardRejected(_ operation: InboxOperation, rejected: InboxRef, failure: InboxAuthorityFailure) {
        queue.removeAll { pending in
            if pending.id == operation.id { return true }
            guard let ref = InboxRef(key: pending.key, generation: pending.generation) else { return false }
            return failure.revokes(ref, rejected: rejected)
        }
    }

    mutating func admitDelivery(_ event: String, now: Int64, expiresAt: Int64) -> Bool {
        delivered = delivered.filter { $0.value > now }
        guard now < expiresAt, delivered[event] == nil else { return false }
        delivered[event] = now + Self.deliveryLifetime
        return true
    }
}

/// A visible message may advance the cumulative cursor only when every
/// earlier sequence after the current read cursor has also been exposed.
struct InboxReadVisibility {
    private var seen: Set<Int> = []
    private var through = 0

    mutating func observe(_ sequences: [Int], after baseline: Int) -> Int {
        through = max(through, baseline)
        seen = seen.filter { $0 > through }
        seen.formUnion(sequences.filter { $0 > through })
        while seen.remove(through + 1) != nil { through += 1 }
        // Keep the nearest continuity gaps if the reader browses far ahead.
        if seen.count > 200 { seen = Set(seen.sorted().prefix(200)) }
        return through
    }
}

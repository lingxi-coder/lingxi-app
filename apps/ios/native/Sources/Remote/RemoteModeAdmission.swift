import Foundation

enum RemoteApprovalCapability {
    static func accepts(_ sessionID: String) -> Bool {
        let prefix = "team-approval:"
        guard sessionID.hasPrefix(prefix) else { return false }
        let nonce = sessionID.dropFirst(prefix.count).utf8
        return nonce.count == 32 && nonce.allSatisfy {
            (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95
        }
    }
}

/// A Team capability answers one currently pending request at most once.
/// Delivery failure releases the reservation so a still-pending request can retry.
struct RemoteApprovalAdmission {
    private(set) var delivered = false
    private var inFlight = false

    mutating func begin(isPending: Bool) -> Bool {
        guard isPending, !delivered, !inFlight else { return false }
        inFlight = true
        return true
    }

    mutating func finish(delivered: Bool) {
        guard inFlight else { return }
        inFlight = false
        if delivered { self.delivered = true }
    }
}

@MainActor
final class RemoteRequestDrain {
    private var active = 0
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func begin() { active += 1 }

    func end() {
        precondition(active > 0)
        active -= 1
        if active == 0 {
            let pending = waiters
            waiters.removeAll()
            for waiter in pending { waiter.resume() }
        }
    }

    func waitUntilEmpty() async {
        guard active > 0 else { return }
        await withCheckedContinuation { waiters.append($0) }
    }
}

@MainActor
final class RemoteConnectSerial {
    private var held = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func acquire() async {
        if !held {
            held = true
            return
        }
        await withCheckedContinuation { waiters.append($0) }
    }

    func release() {
        precondition(held)
        if waiters.isEmpty { held = false }
        else { waiters.removeFirst().resume() }
    }
}

/// Tracks the selected host and the last native connect proven to have finished.
struct RemoteConnectionFence {
    private(set) var intent: UInt64 = 0
    private(set) var requestedHostID: String?
    private(set) var confirmedHostID: String?
    private(set) var admissionClosed = true
    private(set) var needsRetry = false

    mutating func reset() {
        intent &+= 1
        requestedHostID = nil
        confirmedHostID = nil
        admissionClosed = true
        needsRetry = false
    }

    mutating func begin(_ hostID: String) -> UInt64 {
        intent &+= 1
        requestedHostID = hostID
        admissionClosed = true
        needsRetry = false
        return intent
    }

    mutating func nativeCallStarted() { confirmedHostID = nil }
    mutating func nativeCallSucceeded(_ hostID: String) { confirmedHostID = hostID }

    mutating func finishSuccess(_ operation: UInt64) {
        guard operation == intent else { return }
        admissionClosed = false
        needsRetry = false
    }

    mutating func finishFailure(_ operation: UInt64, readyHostID: String?) {
        guard operation == intent else { return }
        let restorable = confirmedHostID != nil && confirmedHostID == requestedHostID && confirmedHostID == readyHostID
        admissionClosed = !restorable
        needsRetry = !restorable
    }

    mutating func observeReadyHost(_ hostID: String) {
        guard needsRetry, confirmedHostID == requestedHostID, confirmedHostID == hostID else { return }
        admissionClosed = false
        needsRetry = false
    }
}

/// Holds remote updates until the desktop identifies this session's fixed mode.
/// All updates are replayed in arrival order after that identification.
struct RemoteModeAdmission<Update> {
    enum Decision {
        case waiting
        case replay(mode: SessionMode, updates: [Update])
        case apply(Update)
        case failed
    }

    private let capacity: Int
    private let byteCapacity: Int
    private var mode: SessionMode?
    private var pending: [Update] = []
    private var pendingBytes = 0
    private var failed = false

    init(capacity: Int = 128, byteCapacity: Int = 4 * 1024 * 1024) {
        self.capacity = capacity
        self.byteCapacity = byteCapacity
    }

    mutating func receive(_ update: Update, announcedModes: Set<SessionMode>,
                          estimatedBytes: () -> Int = { 0 }) -> Decision {
        guard !failed else { return .failed }
        guard announcedModes.count <= 1 else { return fail() }
        let announced = announcedModes.first
        if let mode {
            guard announced == nil || announced == mode else { return fail() }
            return .apply(update)
        }
        if let announced {
            mode = announced
            pending.append(update)
            let replay = pending
            pending.removeAll()
            pendingBytes = 0
            return .replay(mode: announced, updates: replay)
        }
        guard pending.count < capacity else { return fail() }
        let bytes = estimatedBytes()
        guard bytes >= 0, bytes <= byteCapacity - pendingBytes else { return fail() }
        pending.append(update)
        pendingBytes += bytes
        return .waiting
    }

    mutating func invalidate() {
        _ = fail()
    }

    private mutating func fail() -> Decision {
        pending.removeAll()
        pendingBytes = 0
        failed = true
        return .failed
    }
}

/// A conservative traversal avoids copying a potentially large FFI snapshot
/// just to find its size. It stops as soon as the budget is exceeded.
enum RemoteUpdateMemoryEstimate {
    static let limit = 4 * 1024 * 1024

    static func bytes(of value: Any, limit: Int = limit) -> Int {
        var remaining = limit
        return consume(value, remaining: &remaining) ? limit - remaining : limit + 1
    }

    private static func consume(_ value: Any, remaining: inout Int) -> Bool {
        guard charge(64, remaining: &remaining) else { return false }
        switch value {
        case let string as String: return charge(string.utf8.count, remaining: &remaining)
        case let data as Data: return charge(data.count, remaining: &remaining)
        case let bytes as [UInt8]: return chargeElements(bytes.count, stride: 1, remaining: &remaining)
        case let units as [UInt16]: return chargeElements(units.count, stride: 2, remaining: &remaining)
        case let units as [UInt32]: return chargeElements(units.count, stride: 4, remaining: &remaining)
        case let units as [UInt64]: return chargeElements(units.count, stride: 8, remaining: &remaining)
        case let units as [Int8]: return chargeElements(units.count, stride: 1, remaining: &remaining)
        case let units as [Int16]: return chargeElements(units.count, stride: 2, remaining: &remaining)
        case let units as [Int32]: return chargeElements(units.count, stride: 4, remaining: &remaining)
        case let units as [Int64]: return chargeElements(units.count, stride: 8, remaining: &remaining)
        default:
            for child in Mirror(reflecting: value).children {
                guard consume(child.value, remaining: &remaining) else { return false }
            }
            return true
        }
    }

    private static func chargeElements(_ count: Int, stride: Int, remaining: inout Int) -> Bool {
        guard count <= remaining / stride else { return false }
        return charge(count * stride, remaining: &remaining)
    }

    private static func charge(_ bytes: Int, remaining: inout Int) -> Bool {
        guard bytes <= remaining else { return false }
        remaining -= bytes
        return true
    }
}

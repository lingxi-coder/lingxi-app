import Foundation

/// Invalidates pending catalog requests before a current-host forget suspends.
@MainActor
final class RemoteAppRequestEpoch {
    private(set) var value: UInt64 = 0
    private final class ForgetBarrier {
        var pending = 0
        var departed: Bool
        init(departed: Bool) { self.departed = departed }
    }
    private var forgetting: [String: ForgetBarrier] = [:]

    func advance() { value &+= 1 }
    func reset() { forgetting.removeAll(); advance() }
    func permits(hostID: String) -> Bool { forgetting[hostID] == nil }

    func observe(hostID: String, ready: Bool) {
        for (host, barrier) in forgetting where host != hostID || !ready {
            barrier.departed = true
            if barrier.pending == 0 { forgetting.removeValue(forKey: host) }
        }
    }

    func forget(hostID: String, currentHostID: String?, action: () async -> Void) async {
        let barrier = forgetting[hostID] ?? ForgetBarrier(departed: hostID != currentHostID)
        if barrier.departed && hostID == currentHostID { barrier.departed = false }
        forgetting[hostID] = barrier
        barrier.pending += 1
        if hostID == currentHostID { advance() }
        await action()
        // A reset can replace this account's barrier while the action awaits.
        guard forgetting[hostID] === barrier else { return }
        barrier.pending -= 1
        if barrier.pending == 0 && barrier.departed { forgetting.removeValue(forKey: hostID) }
    }
}

/// Keeps an already presented browser independent of the catalog connection,
/// while rejecting a launch that completes after its originating view went away.
struct RemoteAppBrowserLifetime {
    struct Ticket: Identifiable {
        let id = UUID()
        let url: URL
    }

    private(set) var ticket: Ticket?
    private(set) var generation: UInt64 = 0

    mutating func invalidatePending() { generation &+= 1 }

    mutating func present(_ url: URL, generation: UInt64) {
        guard generation == self.generation else { return }
        ticket = Ticket(url: url)
    }

    mutating func dismiss() { ticket = nil }
}

/// Validates the transient desktop launch ticket before handing it to iOS.
enum RemoteAppLaunchURL {
    static func validate(_ raw: String, expiresAt: UInt64, now: Date = Date()) -> URL? {
        guard Double(expiresAt) > now.timeIntervalSince1970 * 1_000,
              let parts = URLComponents(string: raw),
              parts.scheme?.lowercased() == "https", let host = parts.host, !host.isEmpty,
              parts.user == nil, parts.password == nil, let fragment = parts.fragment, !fragment.isEmpty,
              let url = parts.url else { return nil }
        return url
    }
}

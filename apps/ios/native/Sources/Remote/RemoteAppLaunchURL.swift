import Foundation

/// Invalidates pending catalog requests before a current-host forget suspends.
@MainActor
final class RemoteAppRequestEpoch {
    private(set) var value: UInt64 = 0
    private var forgettingHost: String?

    func advance() { value &+= 1 }
    func reset() { forgettingHost = nil; advance() }
    func permits(hostID: String) -> Bool { forgettingHost != hostID }

    func observe(hostID: String, ready: Bool) {
        if let forgettingHost, forgettingHost != hostID || !ready { self.forgettingHost = nil }
    }

    func forget(hostID: String, currentHostID: String?, action: () async -> Void) async {
        if hostID == currentHostID { forgettingHost = hostID; advance() }
        await action()
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

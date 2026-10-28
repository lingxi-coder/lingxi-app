import Foundation

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

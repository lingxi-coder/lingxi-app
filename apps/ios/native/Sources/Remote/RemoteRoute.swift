import Foundation

/// The Bots tree is the root; obsolete Bot-page routes intentionally do not decode.
enum RemoteRoute: Hashable, Codable {
    case host(String)
    case session(host: String, session: String, title: String)

    var isSession: Bool { if case .session = self { true } else { false } }

    static func stack(host: String, session: String?, title: String) -> [RemoteRoute] {
        if let session { [.session(host: host, session: session, title: title)] }
        else { [.host(host)] }
    }
}

struct BotsNavigationState: Codable {
    var uid: String
    var routes: [RemoteRoute]
}

import Foundation

/// The three Friends destinations a link can name (docs/FRIENDS-CLIENTS.md D16). Foundation only, so the same
/// file runs in the macOS XCTest harness. Parsing never redeems a code, accepts a request or opens a chat:
/// `.add` only starts the confirmation flow (D8), and wiring these cases to screens belongs to the Friends UI.
enum FriendLink: Equatable, Sendable {
    /// `friend/add?code=<32 URL-safe characters>`: look the owner up, then ask before `redeem`.
    case add(code: String)
    /// `friend/requests`: the Friends root, Requests section.
    case requests
    /// `friend/chat?uid=<1-128 URL-safe characters>`: the chat with one friend.
    case chat(uid: String)

    /// The Friends destination of an already parsed link, or nil for any other route.
    init?(_ link: CanonicalDeepLink) {
        switch link.route {
        case "friend/add":
            guard let code = link.params["code"] else { return nil }
            self = .add(code: code)
        case "friend/requests":
            self = .requests
        case "friend/chat":
            guard let uid = link.params["uid"] else { return nil }
            self = .chat(uid: uid)
        default:
            return nil
        }
    }

    init?(raw: String) {
        guard let link = CanonicalDeepLink.parse(raw) else { return nil }
        self.init(link)
    }
}

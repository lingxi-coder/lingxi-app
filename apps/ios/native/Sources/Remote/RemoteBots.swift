import Foundation

#if canImport(harness_runtimeFFI)
/// A Bot of one computer; the chats a screen shows are kept under it.
struct RemoteBotKey: Hashable {
    let hostID: String
    let botID: String
}

/// The Bots as the Bots listener reported them last: each computer's list, and each Bot's chats.
/// They are filed under the computer that reported them, the way the sessions are kept for the
/// computer being dialled, so that a report from one the app has left cannot land on another's screen.
struct RemoteBotDirectory: Equatable {
    private var listByHost: [String: RemoteBotsStateFfi] = [:]
    private var chatsByBot: [RemoteBotKey: RemoteBotThreadsStateFfi] = [:]

    /// What the computer said of its Bots; `unknown` until it has said anything.
    func bots(host: String) -> RemoteBotsStateFfi { listByHost[host] ?? .unknown }

    /// What was listed of a Bot's chats; nothing until a screen showing them has had them listed.
    func chats(host: String, bot: String) -> RemoteBotThreadsStateFfi {
        chatsByBot[RemoteBotKey(hostID: host, botID: bot)] ?? RemoteBotThreadsStateFfi(threads: nil, error: nil)
    }

    mutating func set(_ state: RemoteBotsStateFfi, host: String) { listByHost[host] = state }

    mutating func set(_ state: RemoteBotThreadsStateFfi, host: String, bot: String) {
        chatsByBot[RemoteBotKey(hostID: host, botID: bot)] = state
    }

    mutating func removeAll() {
        listByHost.removeAll()
        chatsByBot.removeAll()
    }
}

/// What a Bot's screen shows, decided from what the computer last said of its Bots and of this Bot's
/// chats, so that the view only draws it.
struct RemoteBotScreen: Equatable {
    enum Phase: Equatable {
        /// The computer has not said which Bots it has.
        case loading
        /// The Bot cannot be reached: it is not in the computer's list (its Remote Control switch went off,
        /// or it was deleted), the computer does not know Bots, or listing its chats said so.
        case gone
        case ready(RemoteBotSummaryFfi)
    }

    let phase: Phase
    /// The Bot's chats, newest first, as the computer listed them; none until it has.
    let chats: [RemoteSessionSummaryFfi]
    /// The chats have not been listed yet, and listing them has not failed.
    let loadingChats: Bool
    /// The chats were listed and there are none.
    let noChats: Bool
    /// The last listing failed. Any chats shown are from an earlier one.
    let listFailed: Bool

    init(bot botID: String, bots: RemoteBotsStateFfi, chats listing: RemoteBotThreadsStateFfi) {
        switch bots {
        case .unknown:
            phase = .loading
        case .unsupported:
            phase = .gone
        case let .bots(list):
            if let bot = list.first(where: { $0.botId == botID }), !Self.isUnreachable(listing.error) {
                phase = .ready(bot)
            } else {
                phase = .gone
            }
        }
        chats = listing.threads ?? []
        loadingChats = listing.threads == nil && listing.error == nil
        noChats = listing.threads?.isEmpty == true
        listFailed = listing.error != nil
    }

    /// Whether a failed listing says that the Bot cannot be reached any more, as opposed to
    /// something that a retry may mend.
    static func isUnreachable(_ error: RemoteSessionErrorFfi?) -> Bool {
        error?.code == "unknown_session" || error?.code == "not_remote_enabled"
    }
}

/// Why starting a chat with a Bot failed, as the screen tells it.
enum RemoteBotCreateFailure: Equatable {
    /// The Bot went away.
    case gone
    case capacity
    case forbidden
    case other

    init(_ error: Error) {
        guard case let RemoteFfiError.Rpc(code, _) = error else {
            self = .other
            return
        }
        switch code {
        case "unknown_session", "not_remote_enabled": self = .gone
        case "capacity": self = .capacity
        case "forbidden": self = .forbidden
        default: self = .other
        }
    }

    var message: String {
        switch self {
        case .gone: String(localized: "remote_bot_gone")
        case .capacity: String(localized: "remote_err_capacity")
        case .forbidden: String(localized: "remote_err_forbidden")
        case .other: String(localized: "remote_err_generic")
        }
    }
}

/// The label over a prompt in the transcript.
enum RemotePromptOriginLabel {
    /// A prompt that a Bot handed over arrives as `scheduled`, and the Bot's name tells the two apart.
    /// `botName` is asked only for a scheduled prompt; without a name it reads as scheduled, as before Bots.
    static func text(for origin: RemotePromptOriginFfi, deviceName: String?, botName: () -> String?) -> String {
        switch origin {
        case .desktop:
            return String(localized: "remote_origin_desktop")
        case .scheduled:
            if let name = botName() { return String(localized: "remote_origin_bot \(name)") }
            return String(localized: "remote_origin_scheduled")
        case .device:
            return deviceName ?? String(localized: "remote_origin_other_device")
        }
    }
}

/// The Bot that handed over each prompt of a session, asked of the Rust client once per prompt: the
/// transcript is rebuilt from its records whenever the history changes, and every question crosses the FFI.
struct RemotePromptBotNames {
    private var names: [UInt64: String?] = [:]

    mutating func name(forTurn turnID: UInt64, lookup: () -> String?) -> String? {
        if let known = names[turnID] { return known }
        let found = lookup()
        names.updateValue(found, forKey: turnID)
        return found
    }

    mutating func removeAll() { names.removeAll() }
}
#endif

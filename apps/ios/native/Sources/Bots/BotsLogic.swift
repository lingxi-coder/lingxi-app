import Foundation

/// The snapshot uses app-owned values so its logic and storage can run without the engine bindings.
enum BotsStatus: String, Codable, Sendable { case idle, running, requiresAction, dormant }
enum BotsPendingAction: String, Codable, Sendable { case permission, question, computerAccess }

struct BotsChat: Codable, Equatable, Sendable, Identifiable {
    var id: String
    var title: String = ""
    var status: BotsStatus = .idle
    var pendingAction: BotsPendingAction?
}

struct BotsItem: Codable, Equatable, Sendable, Identifiable {
    var id: String
    var name: String = ""
    var description: String = ""
    var status: BotsStatus = .idle
    var waitingThreads: Int = 0
    var lastActivityAtMs: UInt64?
    var chats: [BotsChat]?
}

struct BotsSnapshot: Codable, Equatable, Sendable {
    var uid: String
    var hostID: String
    var savedAtMs: UInt64
    var bots: [BotsItem]
}

enum BotsLogic {
    static let firstChats = 5
    static let maxHosts = 20
    static let maxBots = 100
    static let maxChats = 50
    static let connectionGrace: Duration = .seconds(60)

    struct Host: Equatable, Sendable {
        var id: String
        var name: String = ""
        var online = false
        var paired = false
        var lastSeenMs: UInt64 = 0
    }
    struct ChatWindow: Equatable {
        var shown: Int
        var hidden: Int
        var showMore: Bool
        var showLess: Bool
    }
    enum Live: Equatable { case unknown, unsupported, bots(Int) }
    enum Kind: String { case loading, unsupported, noBots, bots, connectToLoad, offlineNoSnapshot, notPaired }
    enum Source: String { case live, cached, none }
    enum Note: String { case savedNote, offlineChatsNote }
    struct HostBody: Equatable {
        var kind: Kind
        var source: Source
        var dimmed = false
        var actionable = false
        var notes: [Note] = []
    }

    static func chatWindow(total: Int, expanded: Bool, first: Int = firstChats) -> ChatWindow {
        let total = max(0, total), first = max(0, first)
        let shown = expanded ? total : min(total, first)
        return ChatWindow(shown: shown, hidden: total - shown,
                          showMore: !expanded && total > shown, showLess: expanded && total > first)
    }

    static func compareNames(_ a: String, _ b: String) -> ComparisonResult {
        a.compare(b, options: [.numeric, .caseInsensitive], locale: Locale(identifier: "en_US_POSIX"))
    }
    static func sortBots(_ bots: [BotsItem]) -> [BotsItem] {
        bots.sorted { a, b in
            if a.lastActivityAtMs != b.lastActivityAtMs, (a.lastActivityAtMs ?? 0) != (b.lastActivityAtMs ?? 0) {
                return (a.lastActivityAtMs ?? 0) > (b.lastActivityAtMs ?? 0)
            }
            let name = compareNames(a.name, b.name)
            return name == .orderedSame ? a.id < b.id : name == .orderedAscending
        }
    }
    static func sortHosts(_ hosts: [Host], connectedHostID: String?) -> [Host] {
        hosts.sorted { a, b in
            if (a.id == connectedHostID) != (b.id == connectedHostID) { return a.id == connectedHostID }
            if a.online != b.online { return a.online }
            if a.lastSeenMs != b.lastSeenMs { return a.lastSeenMs > b.lastSeenMs }
            let name = compareNames(a.name, b.name)
            return name == .orderedSame ? a.id < b.id : name == .orderedAscending
        }
    }
    static func hostBody(connected: Bool, online: Bool, paired: Bool, live: Live, snapshotBots: Int?) -> HostBody {
        if connected {
            let kind: Kind
            switch live {
            case .unknown: kind = .loading
            case .unsupported: kind = .unsupported
            case .bots(let n): kind = n > 0 ? .bots : .noBots
            }
            return HostBody(kind: kind, source: .live, actionable: true)
        }
        if paired, let count = snapshotBots {
            let rows = count > 0
            return HostBody(kind: rows ? .bots : .noBots, source: .cached, dimmed: rows, actionable: online,
                            notes: [.savedNote] + (rows && !online ? [.offlineChatsNote] : []))
        }
        return HostBody(kind: !online ? .offlineNoSnapshot : !paired ? .notPaired : .connectToLoad,
                        source: .none, actionable: online)
    }
    static func applyBots(prior: BotsSnapshot?, uid: String, hostID: String, bots: [BotsItem], nowMs: UInt64) -> BotsSnapshot {
        let previous = prior.flatMap { $0.uid == uid && $0.hostID == hostID ? $0 : nil }
        let chats = Dictionary(previous?.bots.map { ($0.id, $0.chats) } ?? [], uniquingKeysWith: { _, b in b })
        let items = bots.prefix(maxBots).map { bot in
            var item = bot
            item.chats = chats[bot.id] ?? nil
            return item
        }
        return BotsSnapshot(uid: uid, hostID: hostID, savedAtMs: nowMs, bots: items)
    }
    static func applyChats(prior: BotsSnapshot?, botID: String, chats: [BotsChat], nowMs: UInt64) -> BotsSnapshot? {
        guard var snapshot = prior, let index = snapshot.bots.firstIndex(where: { $0.id == botID }) else { return prior }
        snapshot.bots[index].chats = Array(chats.prefix(maxChats))
        snapshot.savedAtMs = nowMs
        return snapshot
    }
    static func evictHosts(_ snapshots: [BotsSnapshot]) -> [BotsSnapshot] {
        Array(snapshots.sorted {
            $0.savedAtMs == $1.savedAtMs ? $0.hostID < $1.hostID : $0.savedAtMs > $1.savedAtMs
        }.prefix(maxHosts))
    }
    static func autoConnectTarget(signedIn: Bool, connectedHostID: String?, lastHostID: String?, hosts: [Host]) -> String? {
        guard signedIn, connectedHostID == nil, let lastHostID,
              hosts.contains(where: { $0.id == lastHostID && $0.online && $0.paired }) else { return nil }
        return lastHostID
    }
}

#if canImport(harness_runtimeFFI)
extension BotsLogic.Host {
    init(_ host: RemoteHostFfi) {
        self.init(id: host.hostId, name: host.name, online: host.online, paired: host.paired, lastSeenMs: host.lastSeenMs)
    }
}
extension BotsItem {
    init(_ bot: RemoteBotSummaryFfi) {
        self.init(id: bot.botId, name: bot.name, description: bot.description,
                  status: bot.status == .running ? .running : bot.status == .requiresAction ? .requiresAction : .idle,
                  waitingThreads: Int(bot.waitingThreads), lastActivityAtMs: bot.lastActivityAtMs)
    }
}
extension BotsChat {
    init(_ chat: RemoteSessionSummaryFfi) {
        let status: BotsStatus = switch chat.status {
        case .running: .running
        case .requiresAction: .requiresAction
        case .dormant: .dormant
        case .idle: .idle
        }
        let pending: BotsPendingAction? = switch chat.pendingAction {
        case .permission: .permission
        case .question: .question
        case .computerAccess: .computerAccess
        case nil: nil
        }
        self.init(id: chat.sessionId, title: chat.title, status: status, pendingAction: pending)
    }
}
#endif

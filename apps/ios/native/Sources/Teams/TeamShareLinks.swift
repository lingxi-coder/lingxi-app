import Foundation

/// Shared conversation links (`team/chat`, `team/bot`), the same rules as `packages/teams/share.ts`: a link opens the
/// conversation in the LingXi app where it is installed and on the web otherwise.
enum TeamShareLinks {
    /// The link of one conversation. A Bot's direct chat is shared as the Bot, since each person has their own chat with
    /// it; a friend chat is its pair team's conversation, which both friends can open. A friendship's Bot chat (D22) has
    /// no link: its pair team is not one Teams opens. Nil for a Bot chat without its Bot, or an id a link cannot carry.
    static func link(teamID: String, conversation: TeamMessageConversation) -> URL? {
        if conversation.kind == "bot_dm" {
            guard !teamID.hasPrefix("f_"),
                  let bot = conversation.participants.first(where: { $0.hasPrefix("shared_bot:") })?.dropFirst("shared_bot:".count)
            else { return nil }
            return build("team/bot", ["team": teamID, "bot": String(bot)])
        }
        return build("team/chat", ["team": teamID, "conversation": conversation.id])
    }

    private static func build(_ route: String, _ params: [String: String]) -> URL? {
        var parts = URLComponents(string: "https://lingxios.net/open/\(route)")
        parts?.queryItems = params.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
        guard let url = parts?.url, CanonicalDeepLink.parse(url.absoluteString)?.route == route else { return nil }
        return url
    }

    /// What a shared team link asks the Teams tab for. A friendship's links (pair team `f_…`) go to Friends instead.
    enum Request: Equatable, Sendable {
        case conversation(teamID: String, conversationID: String)
        case bot(teamID: String, botID: String)

        var teamID: String {
            switch self { case let .conversation(teamID, _), let .bot(teamID, _): teamID }
        }

        /// Nil for any other route, and for a pair team's link, which is a `FriendLink.pair`.
        init?(_ link: CanonicalDeepLink) {
            guard let team = link.params["team"], !team.hasPrefix("f_") else { return nil }
            switch link.route {
            case "team/chat": guard let id = link.params["conversation"] else { return nil }; self = .conversation(teamID: team, conversationID: id)
            case "team/bot": guard let id = link.params["bot"] else { return nil }; self = .bot(teamID: team, botID: id)
            default: return nil
            }
        }
    }

    /// The next step toward a team link. A team the viewer is not in is `.unavailable`, never another team; a link never
    /// creates a chat: a Bot without one of the viewer's opens its team's details, where starting one is the viewer's choice.
    enum Step: Equatable {
        case wait, unavailable, selectTeam, open(String), details
    }

    static func step(_ request: Request, uid: String?, listed: Bool, teamIDs: [String], selectedTeamID: String,
                     cacheTeamID: String?, botsLoaded: Bool, bots: [TeamBotCard], conversations: [TeamMessageConversation]) -> Step {
        guard let uid, listed else { return .wait }
        guard teamIDs.contains(request.teamID) else { return .unavailable }
        guard selectedTeamID == request.teamID else { return .selectTeam }
        guard cacheTeamID == request.teamID else { return .wait }
        switch request {
        case let .conversation(_, id): return .open(id)
        case let .bot(_, botID):
            guard botsLoaded else { return .wait }
            guard bots.contains(where: { $0.sharedBotId == botID }) else { return .unavailable }
            return TeamsListLogic.findBotDm(conversations, uid: uid, sharedBotId: botID, bots: bots).map { .open($0.id) } ?? .details
        }
    }
}

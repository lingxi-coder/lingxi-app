import Foundation

// The conversation list rules of the Teams tab (spec §8.1; mirrors `packages/teams/workspace.ts`).
// Pure: Foundation only, no UI, no Firebase, no clock, no locale of its own, so the same file runs in the
// macOS XCTest harness against `mobile-team-bot-vectors.json` (`sort`, `group`, `labels`, `participants`,
// `finds`, `previews`, `search`, `unreadCount`, `defaultConversationTitle`). Android has the identical
// contract in `teams/TeamsListLogic.kt`.

/// The words the list rules put into a label or a preview. The UI builds them from resources for the UI
/// language; the tests use the two fixed sets below. The two separators are code constants, not resources:
/// `、` and `：` when the UI language is Chinese, `, ` and `: ` otherwise.
struct TeamsListStrings: Equatable, Sendable {
    var channel: String
    var directMessage: String
    var group: String
    var member: String
    var you: String
    var bot: String
    var noMessages: String
    var recalled: String
    var nonText: String
    var listSeparator: String
    var authorSeparator: String

    static let en = TeamsListStrings(
        channel: "Channel", directMessage: "Direct message", group: "Group", member: "Member", you: "You", bot: "Bot",
        noMessages: "No messages yet", recalled: "Message recalled", nonText: "[Non-text message]",
        listSeparator: ", ", authorSeparator: ": ")
    static let zh = TeamsListStrings(
        channel: "频道", directMessage: "私信", group: "群聊", member: "成员", you: "你", bot: "Bot",
        noMessages: "暂无消息", recalled: "消息已撤回", nonText: "[非文本消息]",
        listSeparator: "、", authorSeparator: "：")

    /// Chinese is any `zh-*` language tag.
    static func isChinese(languageTag: String) -> Bool { languageTag.lowercased().hasPrefix("zh") }
}

/// A person or a Bot, in the shape avatars take.
struct TeamIdentity: Equatable, Sendable {
    enum Kind: String, Sendable { case user; case bot = "shared_bot" }
    let kind: Kind
    /// The uid of a person; the `botId` of a Bot whose card is known, else its key.
    let id: String
    let name: String
    let isSelf: Bool
}

struct TeamConversationGroups: Equatable, Sendable {
    var channels: [TeamMessageConversation]
    var direct: [TeamMessageConversation]
    var botDirect: [TeamMessageConversation]
}

enum TeamsListLogic {
    /// `max(0, lastSeq - lastReadSeq)`; edits and recalls never advance `lastSeq`.
    static func unreadCount(_ conversation: TeamMessageConversation) -> Int {
        max(0, conversation.lastSeq - conversation.lastReadSeq)
    }

    /// Newest `lastMessage.at` first. Rows without a time follow, by `lastSeq` (newest first), then the
    /// numeric-collated title, then the id (ordinal). Negative = `a` first.
    static func compareConversations(_ a: TeamMessageConversation, _ b: TeamMessageConversation) -> Int {
        let x = a.lastMessage?.atMs, y = b.lastMessage?.atMs
        if let x, let y, x != y { return y > x ? 1 : -1 }
        if (x == nil) != (y == nil) { return x == nil ? 1 : -1 }
        if a.lastSeq != b.lastSeq { return b.lastSeq > a.lastSeq ? 1 : -1 }
        let byTitle = naturalCompare(a.title, b.title)
        if byTitle != 0 { return byTitle }
        return a.id == b.id ? 0 : (a.id < b.id ? -1 : 1)
    }

    /// The sections of the list: channels, people (dm and group) and Bot DMs. A Bot's owner can read other
    /// members' DMs with that Bot; those follow the viewer's own Bot DMs.
    static func groupConversations(_ conversations: [TeamMessageConversation], uid: String) -> TeamConversationGroups {
        var channels: [TeamMessageConversation] = [], direct: [TeamMessageConversation] = [], botDirect: [TeamMessageConversation] = []
        for conversation in conversations {
            switch conversation.kind {
            case "channel": channels.append(conversation)
            case "bot_dm": botDirect.append(conversation)
            default: direct.append(conversation)
            }
        }
        func mine(_ conversation: TeamMessageConversation) -> Bool {
            conversation.participants.isEmpty || conversation.participants.contains("user:\(uid)")
        }
        channels.sort { compareConversations($0, $1) < 0 }
        direct.sort { compareConversations($0, $1) < 0 }
        botDirect.sort { a, b in
            let x = mine(a), y = mine(b)
            return x != y ? x : compareConversations(a, b) < 0
        }
        return TeamConversationGroups(channels: channels, direct: direct, botDirect: botDirect)
    }

    /// What to call a conversation. A channel is its title. A dm or group without a chosen title (empty, or
    /// just the participants' names) is named after the other participants. A Bot DM is the Bot's name,
    /// followed by the people for the Bot's owner reading someone else's DM.
    static func conversationLabel(_ conversation: TeamMessageConversation, members: [TeamMember], uid: String, bots: [TeamBotCard], strings: TeamsListStrings) -> String {
        let title = trim(conversation.title)
        let people = userIds(conversation)
        if conversation.kind == "channel" { return title.isEmpty ? strings.channel : title }
        if conversation.kind == "bot_dm" {
            let card = cardFor(conversation.effectiveSharedBotId, bots)
            let bot = [card?.name ?? "", title, strings.bot].first { !$0.isEmpty } ?? strings.bot
            let others = people.filter { $0 != uid }
            if people.contains(uid) || others.isEmpty { return bot }
            return bot + " · " + others.map { memberName($0, members, uid, strings) }.joined(separator: strings.listSeparator)
        }
        let names = people.map { memberName($0, members, uid, strings) }
        if !title.isEmpty, !isGeneratedTitle(title, names) { return title }
        let others = people.filter { $0 != uid }.map { memberName($0, members, uid, strings) }.joined(separator: strings.listSeparator)
        if !others.isEmpty { return others }
        if !title.isEmpty { return title }
        return conversation.kind == "dm" ? strings.directMessage : strings.group
    }

    /// Everyone in a conversation as avatar identities: other people, then Bots, then the viewer.
    static func conversationParticipants(_ conversation: TeamMessageConversation, members: [TeamMember], bots: [TeamBotCard], uid: String, strings: TeamsListStrings) -> [TeamIdentity] {
        let people = userIds(conversation).map {
            TeamIdentity(kind: .user, id: $0, name: memberName($0, members, uid, strings), isSelf: $0 == uid)
        }
        let robots = conversation.participants.filter { $0.hasPrefix("shared_bot:") }.map { key -> TeamIdentity in
            let card = cardFor(String(key.dropFirst(11)), bots)
            return TeamIdentity(kind: .bot, id: card?.sharedBotId ?? String(key.dropFirst(11)), name: [card?.name ?? "", strings.bot].first { !$0.isEmpty } ?? strings.bot, isSelf: false)
        }
        return people.filter { !$0.isSelf } + robots + people.filter(\.isSelf)
    }

    /// The viewer's own Bot DM with a Bot, if there is one. (A Bot's owner also sees other people's.) With `bots`,
    /// a DM from before the Bot changed maintainer does not count: the Bot answers in a new one.
    static func findBotDm(_ conversations: [TeamMessageConversation], uid: String, sharedBotId: String, bots: [TeamBotCard] = []) -> TeamMessageConversation? {
        conversations.first {
            $0.kind == "bot_dm" && $0.participants.contains("shared_bot:\(sharedBotId)") && $0.participants.contains("user:\(uid)")
                && !botDmHandedOver(kind: $0.kind, participants: $0.participants, readers: $0.readers, bots: bots)
        }
    }

    /// A Bot DM made before its Bot was handed to another maintainer. It stays as it was for the people who could
    /// read it, and the Bot takes no new request in it: the new maintainer was never one of its readers. Unknown
    /// readers say nothing.
    static func botDmHandedOver(kind: String, participants: [String], readers: [String]?, bots: [TeamBotCard]) -> Bool {
        guard kind == "bot_dm", let readers,
              let maintainer = bots.first(where: { participants.contains("shared_bot:\($0.sharedBotId)") })?.maintainer.uid else { return false }
        return !readers.contains(maintainer) || readers.contains { $0 != maintainer && !participants.contains("user:\($0)") }
    }

    /// The viewer's existing two-person DM with someone.
    static func findDm(_ conversations: [TeamMessageConversation], uid: String, peerUid: String) -> TeamMessageConversation? {
        conversations.first {
            $0.kind == "dm" && $0.participants.count == 2 && $0.participants.contains("user:\(uid)") && $0.participants.contains("user:\(peerUid)")
        }
    }

    /// The stored title for a dm or group made without one: every participant's name, the creator first.
    /// `conversationLabel` recognises it and shows only the other people. Always English (", ", "You",
    /// "Member"), because it is stored; cut at `limit` characters (code points).
    static func defaultConversationTitle(_ uids: [String], members: [TeamMember], uid: String, limit: Int = 120) -> String {
        let names = ([uid] + uids.filter { $0 != uid }).map { memberName($0, members, uid, .en) }
        var title = ""
        for name in names {
            let next = title.isEmpty ? name : title + ", " + name
            if next.unicodeScalars.count > limit {
                return title.isEmpty ? String(String.UnicodeScalarView(name.unicodeScalars.prefix(limit))) : title
            }
            title = next
        }
        return title
    }

    /// One line of the newest message. No `lastMessage`: no messages yet. Recalled and non-text messages get
    /// a label. "You: " prefixes the viewer's own message; `author` names the sender in groups and channels.
    static func conversationPreview(_ conversation: TeamMessageConversation, uid: String, strings: TeamsListStrings, author: ((TeamLastMessage) -> String)? = nil) -> String {
        pointerPreview(conversation.lastMessage, uid: uid, strings: strings, author: conversation.kind == "group" || conversation.kind == "channel" ? author : nil)
    }

    /// The message part of that line (a shared Bot's Markdown reads as plain text), from the newest-message pointer alone: a friend's card carries one too
    /// (docs/FRIENDS-CLIENTS.md D3), so a Friends row reads like a Teams row. `author` names someone else's
    /// message where the caller wants a prefix (groups and channels); the viewer's own always reads "You: ".
    static func pointerPreview(_ last: TeamLastMessage?, uid: String, strings: TeamsListStrings, author: ((TeamLastMessage) -> String)? = nil) -> String {
        guard let last else { return strings.noMessages }
        let mine = last.authorKind == "user" && last.authorUid == uid
        var prefix = ""
        if mine {
            prefix = strings.you + strings.authorSeparator
        } else if let author {
            prefix = author(last) + strings.authorSeparator
        }
        if last.recalled { return prefix + strings.recalled }
        let text = TeamsBotText.rendersMarkdown(authorKind: last.authorKind, recalled: last.recalled) ? TeamsBotText.plainText(last.preview) : last.preview
        return prefix + (text.isEmpty ? strings.nonText : text)
    }

    /// The search box: the trimmed query matches when empty, or when the label contains it ignoring case.
    static func conversationMatches(label: String, query: String) -> Bool {
        let needle = trim(query).lowercased()
        return needle.isEmpty || label.lowercased().range(of: needle) != nil
    }

    /// Numeric-collated, case- and diacritic-insensitive comparison (`Intl.Collator(numeric, base)`):
    /// "room 2" before "room 10". Locale-independent on purpose (§8.0: pure functions do not read the locale).
    static func naturalCompare(_ a: String, _ b: String) -> Int {
        switch a.compare(b, options: [.numeric, .caseInsensitive, .diacriticInsensitive], range: nil, locale: nil) {
        case .orderedAscending: -1
        case .orderedDescending: 1
        case .orderedSame: 0
        }
    }

    // MARK: helpers

    private static func userIds(_ conversation: TeamMessageConversation) -> [String] {
        conversation.participants.filter { $0.hasPrefix("user:") }.map { String($0.dropFirst(5)) }
    }

    private static func cardFor(_ key: String?, _ bots: [TeamBotCard]) -> TeamBotCard? {
        key.flatMap { key in bots.first { $0.sharedBotId == key } }
    }

    /// A person's display name, falling back to "You" / "Member" for people the directory does not list.
    private static func memberName(_ id: String, _ members: [TeamMember], _ uid: String, _ strings: TeamsListStrings) -> String {
        if let name = members.first(where: { $0.uid == id })?.displayName, !name.isEmpty { return name }
        return id == uid ? strings.you : strings.member
    }

    /// Titles made of the participants' own names are generated defaults, not names a person chose.
    private static func isGeneratedTitle(_ title: String, _ names: [String]) -> Bool {
        let parts = title.unicodeScalars.split(whereSeparator: { $0 == "," || $0 == "、" })
            .map { trim(String(String.UnicodeScalarView($0))) }
            .filter { !$0.isEmpty }
        if parts.isEmpty || parts.count != names.count { return false }
        var rest = names
        for part in parts {
            guard let index = rest.firstIndex(of: part) else { return false }
            rest.remove(at: index)
        }
        return true
    }

    private static func trim(_ text: String) -> String { JSText.trim(text) }
}

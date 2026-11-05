import Foundation

// The composer rules of a conversation (spec §8.3; mirrors `packages/teams/compose.ts` and the lock logic of
// `Composer.tsx`). Pure: Foundation only. Text indices are UTF-16 code-unit offsets (`String.utf16`), as in
// JavaScript and Kotlin; `snippet` counts code points; the send limit counts UTF-8 bytes. Runs in the macOS
// XCTest harness against `mobile-team-bot-vectors.json` › `compose`. Android has the identical contract in
// `teams/TeamsComposeLogic.kt`.

/// Why the composer cannot be used. `none` = the composer is shown.
enum ComposerLock: Equatable, Sendable {
    case none, ownerReading, botUnverified, botChecking, botUnavailable
}

/// The `@query` being typed right before the caret; `start` is the UTF-16 offset of the `@`.
struct MentionAt: Equatable, Sendable {
    let start: Int
    let query: String
}

enum TeamsComposeLogic {
    /// The server's limit for one message body, in UTF-8 bytes.
    static let messageBytes = 8192
    /// The most Bots one message can ask.
    static let maxTargets = 8
    /// Longest `@query` the picker follows, in code points.
    private static let maxQueryLength = 40

    static func byteLength(_ text: String) -> Int { text.utf8.count }

    /// Non-blank and within the server's limit; the untrimmed text is measured.
    static func canSendText(_ text: String) -> Bool {
        !JSText.isBlank(text) && byteLength(text) <= messageBytes
    }

    /// The `@query` right before `caret`: the text before the caret ends with `@` and up to 40 characters that
    /// are neither whitespace nor `@`, and the `@` follows the start of the text, whitespace, `(` or `（`.
    static func mentionAt(_ text: String, caret: Int) -> MentionAt? {
        let units = Array(text.utf16)
        let end = min(max(caret, 0), units.count)
        let scalars = Array(String(decoding: units[0..<end], as: UTF16.self).unicodeScalars)
        var runStart = scalars.count
        while runStart > 0, !AvatarIdentity.isSpace(scalars[runStart - 1]), scalars[runStart - 1] != "@" { runStart -= 1 }
        let queryScalars = scalars[runStart...]
        guard queryScalars.count <= maxQueryLength, runStart > 0, scalars[runStart - 1] == "@" else { return nil }
        let atIndex = runStart - 1
        if atIndex > 0 {
            let before = scalars[atIndex - 1]
            guard AvatarIdentity.isSpace(before) || before == "(" || before == "（" else { return nil }
        }
        let query = String(String.UnicodeScalarView(queryScalars))
        return MentionAt(start: end - query.utf16.count - 1, query: query)
    }

    /// The text without the `@query` that ran from `start` to `caret`, and without the double space it leaves.
    static func withoutMention(_ text: String, start: Int, caret: Int) -> String {
        let units = Array(text.utf16)
        let from = min(max(start, 0), units.count)
        let to = min(max(caret, from), units.count)
        var before = Array(units[0..<from]), after = Array(units[to...])
        if before.last == 0x20, after.first == 0x20 { after.removeFirst() }
        before.append(contentsOf: after)
        return String(decoding: before, as: UTF16.self)
    }

    /// Bots that can be asked now: only groups and channels take explicit targets (a Bot DM asks its Bot
    /// automatically, a person-to-person DM has none), and the Bot must be usable, take part in the
    /// conversation, not be chosen yet, and match what was typed (trimmed, case-insensitive).
    static func mentionCandidates(bots: [TeamBotCard], conversation: TeamMessageConversation, chosenKeys: some Collection<String>, query: String) -> [TeamBotCard] {
        guard conversation.kind == "group" || conversation.kind == "channel" else { return [] }
        let needle = JSText.trim(query).lowercased()
        return bots.filter {
            $0.selectable && conversation.participants.contains($0.key) && !chosenKeys.contains($0.key)
                && (needle.isEmpty || $0.name.lowercased().contains(needle))
        }
    }

    /// One line of a message for a quote or a reply bar: whitespace runs collapsed, trimmed, cut at `length`
    /// code points plus an ellipsis.
    static func snippet(_ text: String, length: Int = 90) -> String {
        let flat = JSText.trim(JSText.collapseSpaces(text))
        let scalars = flat.unicodeScalars
        guard scalars.count > length else { return flat }
        return String(String.UnicodeScalarView(scalars.prefix(length))) + "…"
    }

    /// Why the composer is replaced by an explanation. A Bot's owner reading someone else's Bot DM cannot
    /// send; a Bot DM needs a usable Bot card among its participants, and while the cards are unknown the
    /// reason says whether they are being checked or could not be verified.
    static func composerLock(uid: String, kind: String, participants: [String], bots: [TeamBotCard], botsLoaded: Bool, botsUnverified: Bool) -> ComposerLock {
        if !participants.isEmpty, !participants.contains("user:\(uid)") { return .ownerReading }
        if kind == "bot_dm", !participants.contains(where: { key in key.hasPrefix("bot:") && bots.contains { $0.key == key && $0.selectable } }) {
            if botsUnverified { return .botUnverified }
            if !botsLoaded { return .botChecking }
            return .botUnavailable
        }
        return .none
    }
}

import Foundation

/// Who looks like what (spec §7 and §8.5). One person or Bot must look the same on every screen of the app
/// and on Desktop: people get initials on a colour taken from their id (`ui.tsx` of the shared Teams
/// package), Bots get the shipped art by their `botId` (`agentAvatarIndex` in `AgentAvatar.swift`),
/// teams a tile with the initial of their name (`TeamAvatar.tsx`).
///
/// Pure: Foundation only, so the same file runs in the macOS XCTest harness against the shared vectors
/// (`avatars.color`, `.initials`, `.teamBotAvatarId` in `mobile-team-bot-vectors.json`). Android has the
/// identical contract in `theme/AvatarIdentity.kt`.
enum AvatarIdentity {
    /// White initials on each of these are at least 4.5:1, so the same colours serve light and dark mode.
    static let avatarColors: [String] = [
        "#4a5fc1", "#2e7d6b", "#b04a6e", "#8a5a1f", "#6b4fa3", "#2a7aa3",
        "#9a4b2d", "#4f7a28", "#a1427d", "#5a6b78", "#3f6fb5", "#a8571a",
    ]

    /// FNV-1a, 32 bit, over the UTF-16 code units of `id` (the same hash Desktop runs over JavaScript strings).
    static func fnv1a(_ id: String) -> UInt32 {
        var hash: UInt32 = 0x811c_9dc5
        for unit in id.utf16 {
            hash ^= UInt32(unit)
            hash = hash &* 0x0100_0193
        }
        return hash
    }

    static func avatarColorIndex(_ id: String) -> Int {
        Int(fnv1a(id) % UInt32(avatarColors.count))
    }

    /// `#rrggbb` of the colour for `id` (lower-case, as in the shared vectors).
    static func avatarColorHex(_ id: String) -> String {
        avatarColors[avatarColorIndex(id)]
    }

    /// The colour for `id` as 0...1 sRGB components, for the view layer.
    static func avatarColorComponents(_ id: String) -> (red: Double, green: Double, blue: Double) {
        let hex = avatarColorHex(id).dropFirst()
        let value = UInt32(hex, radix: 16) ?? 0
        return (Double((value >> 16) & 0xFF) / 255, Double((value >> 8) & 0xFF) / 255, Double(value & 0xFF) / 255)
    }

    /// Two letters for a person ("Ann Lee" is AL), the first character alone for CJK names, "?" for no name.
    /// Words are separated by JavaScript's `\s` (Desktop's rule); "character" means Unicode code point.
    static func initials(_ name: String) -> String {
        let words = name.unicodeScalars.split(whereSeparator: isSpace)
        guard let head = words.first?.first else { return "?" }
        if isCJK(head) { return String(head) }
        let second = words.dropFirst().first?.first
        return (String(head) + (second.map { String($0) } ?? "")).uppercased()
    }

    /// The tile letter of a team: the first code point of its (trimmed) name, upper-cased, or "?".
    static func teamInitial(_ name: String) -> String {
        guard let first = name.unicodeScalars.drop(while: isSpace).first else { return "?" }
        return String(first).uppercased()
    }

    /// The id a Bot's avatar is drawn from when only its key `bot:<owner>:<host>:<botId>` is known (the Bot
    /// card has not loaded yet): the text after the last colon. Any other id is used as it is. The prefix and
    /// the colon are compared as UTF-16 code units, as `String.startsWith` / `lastIndexOf` do on Desktop.
    static func teamBotAvatarId(_ identityId: String) -> String {
        let units = Array(identityId.utf16)
        guard units.starts(with: Array("bot:".utf16)), let colon = units.lastIndex(of: 0x3A) else { return identityId }
        return String(decoding: units[(colon + 1)...], as: UTF16.self)
    }

    /// JavaScript's `\s`: ASCII whitespace plus the Unicode space separators, the BOM and the line and
    /// paragraph separators.
    static func isSpace(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x09...0x0D, 0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF: true
        default: false
        }
    }

    private static func isCJK(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x3400...0x9FFF, 0xF900...0xFAFF, 0x3040...0x30FF, 0xAC00...0xD7AF: true
        default: false
        }
    }
}

import Foundation

// When the app marks a conversation read by itself (decision D12; spec §6.3 and §8.6). Pure: Foundation
// only. Runs in the macOS XCTest harness against `mobile-team-bot-vectors.json` › `mobile.autoMarkRead`.
// Android has the identical contract in `teams/TeamsReadLogic.kt`.
enum TeamsReadLogic {
    /// The newest message must stay in the viewport this long. `nil` disables automatic mark-read.
    static let autoMarkReadMs: Int64? = 1000

    /// The `seq` to send as `lastReadSeq`, or `nil` for "do nothing". Never offline, never from the cache,
    /// never in the background, and only when the newest *rendered* message is ahead of the watermark and has
    /// been at the bottom of the viewport for at least `thresholdMs` (`atBottomMs` = how long it has been
    /// there, `nil` = not at the bottom).
    static func autoMarkReadSeq(
        online: Bool,
        fromCache: Bool,
        foreground: Bool,
        atBottomMs: Int64?,
        newestRenderedSeq: Int,
        lastReadSeq: Int,
        thresholdMs: Int64? = TeamsReadLogic.autoMarkReadMs
    ) -> Int? {
        guard let thresholdMs, online, !fromCache, foreground else { return nil }
        guard let atBottomMs, atBottomMs >= thresholdMs else { return nil }
        return newestRenderedSeq > lastReadSeq ? newestRenderedSeq : nil
    }
}

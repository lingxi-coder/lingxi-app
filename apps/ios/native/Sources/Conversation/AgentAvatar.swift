import SwiftUI

/// Use UTF-16 units rather than Swift Characters to match Desktop and Android.
///
/// Desktop normalises the id first (`AgentAvatar.tsx`: tool results may carry a bare UUID while the agent
/// list uses `agent:<UUID>`), so the same agent, and the same Bot, has the same face on every device
/// (spec §7.2). The prefix test compares code units, as `String.startsWith` does there; `hasPrefix` would
/// compare grapheme clusters and disagree for an id such as `"agent:" + U+0301`.
func agentAvatarIndex(_ agentID: String) -> Int {
    let prefix = "agent:"
    let key = agentID.utf16.starts(with: prefix.utf16) ? agentID : prefix + agentID
    var hash: Int64 = 0
    for unit in key.utf16 { hash = (hash * 31 + Int64(unit)) % 2147483647 }
    return Int(hash % 28)
}

/// How an avatar grows with Dynamic Type (docs/TEAMS-UI-POLISH.md U2, U7): its base size times the scale of the body text, never
/// below the base and never above 1.5 times it. The base sizes are the avatar scale 20 / 24 / 32 / 40 / 56. A view reads the scale
/// with `@ScaledMetric(relativeTo: .body) var percent: CGFloat = 100` (100 at the default text size) and asks `side`.
enum AvatarScale {
    static let maxPercent: CGFloat = 150

    static func side(_ base: CGFloat, percent: CGFloat) -> CGFloat {
        base * min(max(percent, 100), maxPercent) / 100
    }

    /// The corner radius of a Bot, a channel and a team: 0.24 times the side, continuous (U2). A person is a circle.
    static func cornerRadius(side: CGFloat) -> CGFloat { side * 0.24 }
}

/// A Bot's face (or a sub-agent's): the shipped art chosen by `agentID`, clipped to a continuous rounded square so that a Bot reads
/// as a Bot next to the circles of people (U2).
struct AgentAvatar: View {
    @Environment(\.theme) private var theme
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    let agentID: String
    var size: CGFloat = 20

    var body: some View {
        let side = AvatarScale.side(size, percent: percent)
        Image(String(format: "AgentAvatar%02d", agentAvatarIndex(agentID)))
            .resizable()
            .interpolation(.high)
            .frame(width: side, height: side)
            .clipShape(RoundedRectangle(cornerRadius: AvatarScale.cornerRadius(side: side), style: .continuous))
            .environment(\.colorScheme, theme.isDark ? .dark : .light)
            .accessibilityHidden(true)
    }
}

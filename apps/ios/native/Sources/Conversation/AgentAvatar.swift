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

struct AgentAvatar: View {
    @Environment(\.theme) private var theme
    let agentID: String
    var size: CGFloat = 20

    var body: some View {
        Image(String(format: "AgentAvatar%02d", agentAvatarIndex(agentID)))
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
            .environment(\.colorScheme, theme.isDark ? .dark : .light)
            .accessibilityHidden(true)
    }
}

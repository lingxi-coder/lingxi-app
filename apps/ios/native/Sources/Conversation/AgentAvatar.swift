import SwiftUI

/// Use UTF-16 units rather than Swift Characters to match Desktop and Android.
func agentAvatarIndex(_ agentID: String) -> Int {
    var hash: Int64 = 0
    for unit in agentID.utf16 { hash = (hash * 31 + Int64(unit)) % 2147483647 }
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

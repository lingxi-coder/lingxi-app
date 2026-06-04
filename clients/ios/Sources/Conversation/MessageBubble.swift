import SwiftUI

// MARK: - Message bubble (user right-aligned, AI left with avatar)
struct MessageBubble: View {
    @Environment(\.theme) private var t
    let message: Message
    /// When the most-recent AI reply is dimmed during voice flow ("上下文已记入").
    var dimmed: Bool = false
    /// Tapping the assistant bubble's share affordance surfaces the native share
    /// sheet for this reply's text (mirrors Android `MessageBubble onShare`).
    var onShare: (String) -> Void = { _ in }

    var body: some View {
        if message.role == .user {
            HStack {
                Spacer(minLength: 0)
                Text(message.text)
                    .font(.system(size: 15.5))
                    .lineSpacing(15.5 * 0.5)
                    .foregroundColor(t.text)
                    .padding(.horizontal, 16).padding(.vertical, 12)
                    .background(t.surface)
                    .clipShape(BubbleShape(topRightSharp: true))
                    .overlay(BubbleShape(topRightSharp: true).stroke(t.border, lineWidth: 0.5))
                    .frame(maxWidth: 300, alignment: .trailing)
            }
            .padding(.bottom, 22)
        } else {
            HStack(alignment: .top, spacing: 11) {
                AssistantAvatar()
                VStack(alignment: .leading, spacing: 8) {
                    if let tag = message.tag { Pill(text: tag, color: t.accent) }
                    AIText(markdown: message.text)
                    // Share affordance: surfaces the native chooser for this
                    // reply's text through the same ShareImpl the engine bridges
                    // onto `traits::SharingService` — so a bubble share and a
                    // `tool-share` invocation are the identical launch path.
                    Button(action: { onShare(message.text) }) {
                        LXIcon(name: .share, size: 15, color: t.text3, stroke: 1.8)
                            .frame(width: 28, height: 28)
                            .contentShape(RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain)
                }
                Spacer(minLength: 0)
            }
            .opacity(dimmed ? 0.4 : 1)
            .padding(.bottom, 26)
        }
    }
}

// AI bubble shape: rounded 18 with one corner sharpened to 6.
struct BubbleShape: Shape {
    var topRightSharp: Bool
    func path(in rect: CGRect) -> Path {
        let big: CGFloat = 18, small: CGFloat = 6
        return Path(roundedRect: rect, cornerRadii: RectangleCornerRadii(
            topLeading: big,
            bottomLeading: big,
            bottomTrailing: big,
            topTrailing: topRightSharp ? small : big))
    }
}

struct AssistantAvatar: View {
    @Environment(\.theme) private var t
    var size: CGFloat = 30
    var corner: CGFloat = 9
    var glyph: CGFloat = 15
    var body: some View {
        RoundedRectangle(cornerRadius: corner)
            .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
            .frame(width: size, height: size)
            .overlay(LXIcon(name: .sparkle, size: glyph, color: .white, stroke: 2))
            .shadow(color: t.accent.tint(0.30), radius: 6, y: 4)
    }
}

// Renders **bold** spans (the only markdown the prototype uses).
struct AIText: View {
    @Environment(\.theme) private var t
    let markdown: String
    var body: some View {
        Text(attributed)
            .font(.system(size: 15.5))
            .lineSpacing(15.5 * 0.6)
            .foregroundColor(t.text)
            .fixedSize(horizontal: false, vertical: true)
    }
    private var attributed: AttributedString {
        // Per-line so paragraph breaks render, with **bold** inline.
        var out = AttributedString()
        let lines = markdown.components(separatedBy: "\n")
        for (i, line) in lines.enumerated() {
            out += parseBold(line)
            if i < lines.count - 1 { out += AttributedString("\n") }
        }
        return out
    }
    private func parseBold(_ s: String) -> AttributedString {
        var result = AttributedString()
        var rest = Substring(s)
        while let open = rest.range(of: "**") {
            result += AttributedString(String(rest[rest.startIndex..<open.lowerBound]))
            let after = rest[open.upperBound...]
            if let close = after.range(of: "**") {
                var bold = AttributedString(String(after[after.startIndex..<close.lowerBound]))
                bold.font = .system(size: 15.5, weight: .semibold)
                result += bold
                rest = after[close.upperBound...]
            } else {
                result += AttributedString(String(after)); rest = Substring("")
            }
        }
        result += AttributedString(String(rest))
        return result
    }
}

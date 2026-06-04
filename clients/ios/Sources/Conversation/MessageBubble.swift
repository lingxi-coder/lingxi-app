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

// MARK: - Assistant markdown rendering (PR-4 item 5)
//
// Renders the markdown subset assistant replies actually emit: fenced code
// blocks (```), inline code (`…`), **bold**, and bullet / numbered lists — in
// addition to paragraph breaks. Anything else falls through as plain text.
//
// The text is first split into BLOCKS (fenced code vs. everything else); each
// non-code block is then rendered line-by-line as a paragraph or a list item,
// with `**bold**` + `` `inline code` `` parsed inline.

/// One parsed block of assistant markdown.
private enum MDBlock: Equatable {
    /// A fenced code block (``` … ```), `lang` is the optional info string.
    case code(String, lang: String?)
    /// A run of ordinary text lines (paragraphs + list items).
    case text([MDLine])
}

/// One line within a text block.
private enum MDLine: Equatable {
    case paragraph(String)
    case bullet(String)
    case numbered(marker: String, String)
}

struct AIText: View {
    @Environment(\.theme) private var t
    let markdown: String

    private static let bodySize: CGFloat = 15.5

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(Self.parseBlocks(markdown).enumerated()), id: \.offset) { _, block in
                switch block {
                case let .code(code, _):
                    codeBlock(code)
                case let .text(lines):
                    textBlock(lines)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // A fenced code block: monospaced, in a tinted rounded panel.
    private func codeBlock(_ code: String) -> some View {
        Text(code)
            .font(.system(size: 13.5, design: .monospaced))
            .foregroundColor(t.text)
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(t.surface)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }

    // A text block: paragraphs and list items, each with inline spans.
    private func textBlock(_ lines: [MDLine]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                switch line {
                case let .paragraph(s):
                    inlineText(s)
                case let .bullet(s):
                    listRow(marker: "•", s)
                case let .numbered(marker, s):
                    listRow(marker: marker, s)
                }
            }
        }
    }

    private func listRow(marker: String, _ s: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(marker)
                .font(.system(size: Self.bodySize))
                .foregroundColor(t.text2)
                .frame(minWidth: 14, alignment: .trailing)
            inlineText(s)
        }
    }

    private func inlineText(_ s: String) -> some View {
        Text(Self.parseInline(s, size: Self.bodySize))
            .font(.system(size: Self.bodySize))
            .lineSpacing(Self.bodySize * 0.6)
            .foregroundColor(t.text)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: block parsing

    /// Split the source into fenced-code vs. text blocks.
    fileprivate static func parseBlocks(_ md: String) -> [MDBlock] {
        var blocks: [MDBlock] = []
        let lines = md.components(separatedBy: "\n")
        var i = 0
        var pendingText: [String] = []

        func flushText() {
            guard !pendingText.isEmpty else { return }
            // Drop leading/trailing blank lines a fence may have left behind.
            while pendingText.first?.trimmingCharacters(in: .whitespaces).isEmpty == true { pendingText.removeFirst() }
            while pendingText.last?.trimmingCharacters(in: .whitespaces).isEmpty == true { pendingText.removeLast() }
            if !pendingText.isEmpty { blocks.append(.text(pendingText.map(parseLine))) }
            pendingText = []
        }

        while i < lines.count {
            let line = lines[i]
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                flushText()
                let lang = String(trimmed.dropFirst(3)).trimmingCharacters(in: .whitespaces)
                var code: [String] = []
                i += 1
                while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                    code.append(lines[i]); i += 1
                }
                // Skip the closing fence (if present).
                if i < lines.count { i += 1 }
                blocks.append(.code(code.joined(separator: "\n"), lang: lang.isEmpty ? nil : lang))
            } else {
                pendingText.append(line)
                i += 1
            }
        }
        flushText()
        return blocks
    }

    /// Classify one text line as a paragraph, bullet, or numbered item.
    fileprivate static func parseLine(_ raw: String) -> MDLine {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        // Bullets: -, *, or • followed by a space.
        for marker in ["- ", "* ", "• "] where trimmed.hasPrefix(marker) {
            return .bullet(String(trimmed.dropFirst(marker.count)))
        }
        // Numbered: `1.` / `1)` followed by a space.
        if let dot = trimmed.firstIndex(where: { $0 == "." || $0 == ")" }) {
            let head = trimmed[trimmed.startIndex..<dot]
            let afterIdx = trimmed.index(after: dot)
            if !head.isEmpty, head.allSatisfy(\.isNumber),
               afterIdx < trimmed.endIndex, trimmed[afterIdx] == " " {
                let body = String(trimmed[trimmed.index(after: afterIdx)...])
                return .numbered(marker: "\(head).", body)
            }
        }
        return .paragraph(trimmed)
    }

    // MARK: inline parsing (**bold** + `code`)

    /// Parse inline `**bold**` and `` `inline code` `` spans within one line.
    static func parseInline(_ s: String, size: CGFloat) -> AttributedString {
        var result = AttributedString()
        let chars = Array(s)
        var idx = 0
        var plain = ""

        func flushPlain() {
            if !plain.isEmpty { result += AttributedString(plain); plain = "" }
        }

        while idx < chars.count {
            // Inline code: `…` (single backtick, no nesting).
            if chars[idx] == "`" {
                if let close = nextIndex(of: "`", in: chars, from: idx + 1) {
                    flushPlain()
                    var code = AttributedString(String(chars[(idx + 1)..<close]))
                    code.font = .system(size: size - 1, design: .monospaced)
                    result += code
                    idx = close + 1
                    continue
                }
            }
            // Bold: **…**
            if chars[idx] == "*", idx + 1 < chars.count, chars[idx + 1] == "*" {
                if let close = nextDoubleStar(in: chars, from: idx + 2) {
                    flushPlain()
                    var bold = AttributedString(String(chars[(idx + 2)..<close]))
                    bold.font = .system(size: size, weight: .semibold)
                    result += bold
                    idx = close + 2
                    continue
                }
            }
            plain.append(chars[idx])
            idx += 1
        }
        flushPlain()
        return result
    }

    private static func nextIndex(of ch: Character, in chars: [Character], from start: Int) -> Int? {
        var i = start
        while i < chars.count { if chars[i] == ch { return i }; i += 1 }
        return nil
    }

    private static func nextDoubleStar(in chars: [Character], from start: Int) -> Int? {
        var i = start
        while i + 1 < chars.count {
            if chars[i] == "*", chars[i + 1] == "*" { return i }
            i += 1
        }
        return nil
    }
}

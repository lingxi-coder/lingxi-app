import SwiftUI

// MARK: - Message bubble (user right-aligned, AI left with avatar)
struct MessageBubble: View, Equatable {
    @Environment(\.theme) private var t
    let message: Message
    var detail: ConversationMessageDetail? = nil
    /// When the most-recent AI reply is dimmed during voice flow ("上下文已记入").
    var dimmed: Bool = false
    /// Tapping the assistant bubble's share affordance surfaces the native share
    /// sheet for this reply's text (mirrors Android `MessageBubble onShare`).
    var onShare: (String) -> Void = { _ in }

    static func == (lhs: MessageBubble, rhs: MessageBubble) -> Bool {
        lhs.message == rhs.message &&
            lhs.detail == rhs.detail &&
            lhs.dimmed == rhs.dimmed
    }

    var body: some View {
        if message.role == .user {
            HStack(alignment: .top, spacing: 0) {
                Spacer(minLength: 0)
                Text(message.text)
                    // Dynamic Type: scale the body relative to .body so the
                    // transcript honors the user's text-size setting while
                    // keeping the design's 15.5pt baseline.
                    .font(.scaledSystem(15.5, relativeTo: .body))
                    .lineSpacing(15.5 * 0.5)
                    .foregroundColor(t.text)
                    .padding(.horizontal, 16).padding(.vertical, 12)
                    .background(t.surface)
                    .clipShape(BubbleShape(topRightSharp: true))
                    .overlay(BubbleShape(topRightSharp: true).stroke(t.border, lineWidth: 0.5))
                    .frame(maxWidth: 320, alignment: .trailing)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.bottom, 22)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("conversation.message.user")
        } else {
            HStack(alignment: .top, spacing: 11) {
                AssistantAvatar()
                VStack(alignment: .leading, spacing: 8) {
                    if let tag = message.tag { Pill(text: tag, color: t.accent) }
                    if let detail, !detail.blocks.isEmpty {
                        StructuredAIBlocks(detail: detail, fallback: message.text)
                    } else {
                        AIText(markdown: message.text)
                            .equatable()
                    }
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
                    .accessibilityLabel("chat_share_reply")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .opacity(dimmed ? 0.4 : 1)
            .padding(.bottom, 26)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("conversation.message.assistant")
        }
    }
}

private struct StructuredAIBlocks: View {
    @Environment(\.theme) private var t
    let detail: ConversationMessageDetail
    let fallback: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(detail.blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case let .text(text):
                    AIText(markdown: text)
                        .equatable()
                case let .thinking(text, _):
                    panel(title: String(localized: "chat_thinking"), body: text, icon: .brain)
                case .redactedThinking:
                    panel(title: String(localized: "chat_thinking"), body: String(localized: "chat_redacted_thinking"), icon: .brain)
                case let .compactBoundary(messagesBefore, messagesAfter, _):
                    compactBoundary(before: messagesBefore, after: messagesAfter)
                case let .toolUse(_, tool, inputSummary, _):
                    panel(title: String(localized: "chat_tool_call_title \(tool)"), body: inputSummary, icon: .workflow)
                case let .toolResult(_, tool, isError, summary, _, oldString, newString, filePath):
                    VStack(alignment: .leading, spacing: 6) {
                        panel(
                            title: isError ? String(localized: "chat_tool_failed \(tool)") : String(localized: "chat_tool_returned \(tool)"),
                            body: summary,
                            icon: isError ? .warning : .check
                        )
                        if let filePath, let oldString, let newString {
                            diffPreview(path: filePath, oldString: oldString, newString: newString)
                        }
                    }
                }
            }
        }
        .overlay {
            if detail.blocks.isEmpty && !fallback.isEmpty {
                AIText(markdown: fallback)
                    .equatable()
            }
        }
    }

    private func panel(title: String, body: String, icon: LXIconName) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                LXIcon(name: icon, size: 12, color: t.text3, stroke: 1.7)
                Text(title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(t.text3)
            }
            Text(body)
                .font(.system(size: 12.5))
                .foregroundColor(t.text2)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }
        .padding(10)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }

    private func compactBoundary(before: Int, after: Int) -> some View {
        HStack(spacing: 8) {
            LXIcon(name: .workflow, size: 12, color: t.text3, stroke: 1.7)
            Text(String(localized: "chat_compacted \(before) \(after)"))
                .font(.system(size: 12))
                .foregroundColor(t.text3)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(t.surface)
        .clipShape(Capsule())
        .overlay(Capsule().stroke(t.border, lineWidth: 0.5))
    }

    private func diffPreview(path: String, oldString: String, newString: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(path)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundColor(t.text4)
            Text("- \(oldString)\n+ \(newString)")
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundColor(t.text2)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }
        .padding(10)
        .background(t.windowBg.opacity(0.55))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
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
            .accessibilityHidden(true)
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

/// One top-level markdown block, rendered independently.
///
/// `Equatable` on its block value so a streaming message only pays for the
/// block that changed — see `AIText.body`.
private struct MDBlockView: View, Equatable {
    @Environment(\.theme) private var t
    let block: MDBlock

    private static let bodySize: CGFloat = 15.5

    static func == (lhs: MDBlockView, rhs: MDBlockView) -> Bool {
        lhs.block == rhs.block
    }

    var body: some View {
        switch block {
        case let .code(code, _):
            codeBlock(code)
        case let .text(lines):
            textBlock(lines)
        }
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
            // Per LINE, not per block: `parseBlocks` folds every run of
            // non-fenced lines into a SINGLE `.text` block, so block-level
            // equality alone does nothing for ordinary prose — the one text
            // block changes on every streamed chunk. Line granularity is what
            // actually stops the whole message re-building its
            // `AttributedString`s, and it keeps the layout byte-identical
            // because the spacing still comes from this VStack.
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                MDLineView(line: line).equatable()
            }
        }
    }

}

/// One rendered markdown line. `Equatable` on the line so a streaming message
/// only rebuilds the `AttributedString` for the line that actually changed.
private struct MDLineView: View, Equatable {
    @Environment(\.theme) private var t
    let line: MDLine

    private static let bodySize: CGFloat = 15.5

    static func == (lhs: MDLineView, rhs: MDLineView) -> Bool {
        lhs.line == rhs.line
    }

    var body: some View {
        switch line {
        case let .paragraph(s):
            inlineText(s)
        case let .bullet(s):
            listRow(marker: "•", s)
        case let .numbered(marker, s):
            listRow(marker: marker, s)
        }
    }

    private func listRow(marker: String, _ s: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(marker)
                .font(.scaledSystem(Self.bodySize, relativeTo: .body))
                .foregroundColor(t.text2)
                .frame(minWidth: 14, alignment: .trailing)
            inlineText(s)
        }
    }

    private func inlineText(_ s: String) -> some View {
        // Dynamic Type: the assistant body scales relative to .body. The inline
        // spans carry their own fixed-point fonts (bold / code) inside the
        // AttributedString; the outer .font sets the scalable default size.
        Text(AIText.parseInline(s, size: Self.bodySize))
            .font(.scaledSystem(Self.bodySize, relativeTo: .body))
            .lineSpacing(Self.bodySize * 0.6)
            .foregroundColor(t.text)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

}

struct AIText: View, Equatable {
    @Environment(\.theme) private var t
    let markdown: String

    private static let bodySize: CGFloat = 15.5

    static func == (lhs: AIText, rhs: AIText) -> Bool {
        lhs.markdown == rhs.markdown
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // One Equatable view PER BLOCK, not one view for the whole
            // document. While a message streams, `markdown` changes on every
            // chunk, so this body re-runs — but only the block that actually
            // changed (the last, still-open one) re-evaluates its own body and
            // rebuilds its `AttributedString`s. Every earlier block compares
            // equal and is skipped.
            //
            // Without this, `parseInline` rebuilt an `AttributedString`
            // character by character for every line of the WHOLE message on
            // every chunk — quadratic in the length of the stream. The UI
            // stuttered worse the longer a response ran and went smooth the
            // instant it stopped, which is the signature of per-change work
            // rather than per-scroll work.
            //
            // Splitting on `parseBlocks`' own boundaries is what makes this
            // safe: it closes a fenced code block only at its closing fence,
            // so no block is ever cut through the middle of a construct.
            ForEach(Array(Self.parseBlocks(markdown).enumerated()), id: \.offset) { _, block in
                MDBlockView(block: block).equatable()
            }
        }
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

    // MARK: inline parsing (**bold** + `code` + links)

    /// Parse inline `**bold**`, `` `inline code` `` and `[label](url)` spans.
    /// SwiftUI routes attributed links through the scene's `openURL` action, so
    /// Android-compatible `lingxi://open_terminal` links reach RootView too.
    static func parseInline(_ s: String, size: CGFloat) -> AttributedString {
        var result = AttributedString()
        let chars = Array(s)
        var idx = 0
        var plain = ""

        func flushPlain() {
            if !plain.isEmpty { result += AttributedString(plain); plain = "" }
        }

        while idx < chars.count {
            // Link: [label](url). A malformed or non-URL target remains text.
            if chars[idx] == "[",
               let labelEnd = nextIndex(of: "]", in: chars, from: idx + 1),
               labelEnd + 1 < chars.count,
               chars[labelEnd + 1] == "(",
               let targetEnd = nextIndex(of: ")", in: chars, from: labelEnd + 2) {
                let label = String(chars[(idx + 1)..<labelEnd])
                let target = String(chars[(labelEnd + 2)..<targetEnd])
                if !label.isEmpty, let url = URL(string: target) {
                    flushPlain()
                    var link = parseInline(label, size: size)
                    link.link = url
                    result += link
                    idx = targetEnd + 1
                    continue
                }
            }
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

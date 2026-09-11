import SwiftUI
import UIKit

private struct MessageImages: View {
    @Environment(\.theme) private var t
    let images: [MessageImage]
    @State private var selectedImage: MessageImage?

    var body: some View {
        ScrollView(.horizontal) {
            HStack(alignment: .bottom, spacing: 7) {
                ForEach(images) { image in
                    Button { selectedImage = image } label: { imageView(image) }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Preview attached image")
                }
            }
        }
        .scrollIndicators(.hidden)
        .frame(maxWidth: 320, alignment: .trailing)
        .fullScreenCover(item: $selectedImage) { image in
            NavigationStack {
                Group {
                    if let decoded = decodedImage(image.url) {
                        Image(uiImage: decoded).resizable().scaledToFit()
                    } else if let url = URL(string: image.url) {
                        AsyncImage(url: url) { content in content.resizable().scaledToFit() }
                            placeholder: { ProgressView() }
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(t.windowBg)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_close") { selectedImage = nil }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func imageView(_ image: MessageImage) -> some View {
        if let decoded = decodedImage(image.url) {
            Image(uiImage: decoded)
                .resizable()
                .scaledToFill()
                .frame(width: 116, height: 116)
                .clipped()
                .background(t.surfaceActive)
                .clipShape(RoundedRectangle(cornerRadius: 11))
                .overlay(RoundedRectangle(cornerRadius: 11).stroke(t.border, lineWidth: 0.5))
                .accessibilityLabel("Attached image")
        } else if let url = URL(string: image.url) {
            AsyncImage(url: url) { phase in
                switch phase {
                case let .success(content):
                    content.resizable().scaledToFill()
                default:
                    placeholder
                }
            }
            .frame(width: 116, height: 116)
            .clipped()
            .background(t.surfaceActive)
            .clipShape(RoundedRectangle(cornerRadius: 11))
            .overlay(RoundedRectangle(cornerRadius: 11).stroke(t.border, lineWidth: 0.5))
            .accessibilityLabel("Attached image")
        } else {
            placeholder
        }
    }

    private var placeholder: some View {
        Text("Image")
            .font(.caption)
            .foregroundStyle(t.text3)
            .frame(width: 116, height: 116)
            .background(t.surfaceActive)
            .clipShape(RoundedRectangle(cornerRadius: 11))
    }

    private func decodedImage(_ url: String) -> UIImage? {
        guard url.hasPrefix("data:"), let comma = url.firstIndex(of: ",") else { return nil }
        let encoded = String(url[url.index(after: comma)...])
        guard let data = Data(base64Encoded: encoded, options: .ignoreUnknownCharacters) else { return nil }
        return UIImage(data: data)
    }
}

// MARK: - Message bubble (user right-aligned, AI left)
struct MessageBubble: View, Equatable {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let message: Message
    var detail: ConversationMessageDetail? = nil
    var expandedToolBlocks: Set<String> = []
    var onToggleToolBlock: (String) -> Void = { _ in }
    /// When the most-recent AI reply is dimmed during voice flow ("上下文已记入").
    var dimmed: Bool = false
    /// Tapping the assistant bubble's share affordance surfaces the native share
    /// sheet for this reply's text (mirrors Android `MessageBubble onShare`).
    var onShare: (String) -> Void = { _ in }
    /// Fold state for a long USER bubble, owned by `ConversationTimelineView`.
    /// Row-local `@State` would be discarded when the `LazyVStack` releases an
    /// off-screen row, silently re-collapsing a prompt the user expanded.
    var isUserExpanded: Bool = false
    var onToggleUserExpanded: () -> Void = {}
    var isAssistantExpanded: Bool = false
    var onToggleAssistantExpanded: () -> Void = {}

    static func == (lhs: MessageBubble, rhs: MessageBubble) -> Bool {
        lhs.message == rhs.message &&
            lhs.detail == rhs.detail &&
            lhs.expandedToolBlocks == rhs.expandedToolBlocks &&
            // Without this the fold toggle would change no observed property
            // and SwiftUI would skip the re-render entirely.
            lhs.isUserExpanded == rhs.isUserExpanded &&
            lhs.isAssistantExpanded == rhs.isAssistantExpanded &&
            lhs.dimmed == rhs.dimmed
    }

    var body: some View {
        if message.role == .user {
            VStack(alignment: .trailing, spacing: 4) {
                if !message.images.isEmpty {
                    MessageImages(images: message.images)
                }
                HStack(alignment: .top, spacing: 0) {
                    Spacer(minLength: 0)
                    if !message.text.isEmpty {
                        userContent
                        // Dynamic Type: scale the body relative to .body so the
                        // transcript honors the user's text-size setting while
                        // keeping the design's 15.5pt baseline.
                        .font(.scaledSystem(15.5, relativeTo: .body))
                        .lineSpacing(15.5 * 0.5)
                        // `lineLimit`, not the assistant branch's
                        // `frame(maxHeight:)`: this bubble is one `Text` inside
                        // a `BubbleShape`, so a height clip would square off the
                        // rounded bottom corners. `lineLimit` also gives a
                        // trailing ellipsis for free.
                        .frame(maxHeight: userIsCollapsed ? 260 : nil, alignment: .top)
                        .clipped()
                        .foregroundColor(t.text)
                        .padding(.horizontal, 16).padding(.vertical, 12)
                        .background(t.surface)
                        .clipShape(.rect(cornerRadius: 18))
                        .frame(maxWidth: 700, alignment: .trailing)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                // A subagent's prompt arrives as the first USER bubble of its
                // child transcript, and those run to thousands of characters —
                // the whole transcript became a wall of text. Same affordance,
                // same policy, and the same two strings as the assistant side.
                if userIsCollapsible {
                    Button {
                        withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.18)) {
                            onToggleUserExpanded()
                        }
                    } label: {
                        HStack(spacing: 4) {
                            LXIcon(name: .chevron, size: 11, color: t.accent, stroke: 2)
                                .rotationEffect(.degrees(isUserExpanded ? 180 : 0))
                            Text(isUserExpanded
                                ? String(localized: "chat_run_collapse")
                                : String(localized: "chat_run_expand"))
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundColor(t.accent)
                        }
                        .frame(minHeight: 28)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("conversation.message.user.toggle")
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.bottom, 22)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("conversation.message.user")
        } else {
            HStack(alignment: .top, spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    if let tag = message.tag { Pill(text: tag, color: t.accent) }
                    if !message.images.isEmpty { MessageImages(images: message.images) }
                    assistantContent
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(
                            maxHeight: assistantIsCollapsed ? 360 : nil,
                            alignment: .top
                        )
                        .clipped()
                    // Share affordance: surfaces the native chooser for this
                    // reply's text through the same ShareImpl the engine bridges
                    // onto `traits::SharingService` — so a bubble share and a
                    // `tool-share` invocation are the identical launch path.
                    HStack(spacing: 4) {
                        if assistantIsCollapsible {
                            Button {
                                withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.18)) {
                                    onToggleAssistantExpanded()
                                }
                            } label: {
                                HStack(spacing: 4) {
                                    LXIcon(name: .chevron, size: 11, color: t.accent, stroke: 2)
                                        .rotationEffect(.degrees(isAssistantExpanded ? 180 : 0))
                                    Text(isAssistantExpanded
                                        ? String(localized: "chat_run_collapse")
                                        : String(localized: "chat_run_expand"))
                                        .font(.system(size: 11.5, weight: .medium))
                                        .foregroundColor(t.accent)
                                }
                                .frame(minHeight: 28)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("conversation.message.assistant.toggle")
                        }
                        Button(action: { onShare(message.text) }) {
                            LXIcon(name: .share, size: 15, color: t.text3, stroke: 1.8)
                                .frame(width: 28, height: 28)
                                .contentShape(RoundedRectangle(cornerRadius: 8))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("chat_share_reply")
                    }
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

    @ViewBuilder
    private var userContent: some View {
        if message.text.contains("\n") {
            AIText(markdown: message.text)
        } else {
            Text(AIText.parseInline(message.text, size: 15.5))
                .textSelection(.enabled)
        }
    }

    @ViewBuilder
    private var assistantContent: some View {
        if let detail, !detail.blocks.isEmpty {
            StructuredAIBlocks(
                detail: detail,
                fallback: message.text,
                expandedToolBlocks: expandedToolBlocks,
                onToggleToolBlock: onToggleToolBlock
            )
        } else {
            AIText(markdown: message.text)
                .equatable()
        }
    }

    private var userIsCollapsible: Bool {
        AssistantMessageCollapsePolicy.shouldCollapse(message.text)
    }

    private var userIsCollapsed: Bool {
        userIsCollapsible && !isUserExpanded
    }

    private var assistantIsCollapsible: Bool {
        AssistantMessageCollapsePolicy.shouldCollapse(message.text)
    }

    private var assistantIsCollapsed: Bool {
        assistantIsCollapsible && !isAssistantExpanded
    }
}

/// Stable, cross-platform threshold for keeping long streaming replies from
/// taking over the transcript. Character and explicit-line limits complement
/// each other: CJK prose can be visually tall without newline delimiters, while
/// logs and generated code often contain many short lines.
enum AssistantMessageCollapsePolicy {
    private static let characterLimit = 640
    private static let lineLimit = 20

    /// Lines revealed when a USER bubble is collapsed. 15 lines at 15.5pt with
    /// 0.5 line spacing is ~349pt, which lands next to the assistant branch's
    /// 360pt reveal so both sides fold to about the same height.
    static let collapsedLineLimit = 15

    static func shouldCollapse(_ text: String) -> Bool {
        text.count >= characterLimit || text.lazy.filter { $0 == "\n" }.prefix(lineLimit).count >= lineLimit
    }
}

private struct StructuredAIBlocks: View {
    @Environment(\.theme) private var t
    let detail: ConversationMessageDetail
    let fallback: String
    let expandedToolBlocks: Set<String>
    let onToggleToolBlock: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(detail.blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case let .text(text):
                    AIText(markdown: text)
                        .equatable()
                case .thinking, .redactedThinking:
                    // The timeline owns the ephemeral Thinking indicator.
                    // Restored reasoning remains in the model, never in a bubble.
                    EmptyView()
                case let .compactBoundary(messagesBefore, messagesAfter, _):
                    compactBoundary(before: messagesBefore, after: messagesAfter)
                case let .toolUse(id, tool, inputSummary, inputJson, header):
                    // The engine's derived header, localized. `inputSummary` is
                    // the older-engine fallback, never a re-derivation.
                    StructuredToolBlock(
                        id: id,
                        title: header.map(ToolDisplayText.title)
                            ?? String(localized: "chat_tool_call_title \(tool)"),
                        summary: header?.subLine?.text ?? inputSummary,
                        detail: inputJson,
                        icon: ToolDisplayText.icon(header: header, tool: tool),
                        iconColor: ToolDisplayText.iconColor(
                            header: header,
                            tool: tool,
                            palette: t
                        ),
                        isExpanded: expandedToolBlocks.contains(id),
                        onToggle: { onToggleToolBlock(id) }
                    )
                case let .toolResult(id, tool, isError, summary, _, _, _, _, display):
                    StructuredToolBlock(
                        id: id,
                        title: isError
                            ? String(localized: "chat_tool_failed \(tool)")
                            : String(localized: "chat_tool_returned \(tool)"),
                        summary: display.flatMap(ToolDisplayText.headline) ?? summary,
                        detail: display?.body,
                        diff: display?.diff,
                        detailWasTruncated: display?.bodyTruncated ?? false,
                        icon: isError ? .warning : ToolDisplayText.icon(header: nil, tool: tool),
                        iconColor: isError
                            ? t.danger
                            : ToolDisplayText.iconColor(header: nil, tool: tool, palette: t),
                        isExpanded: expandedToolBlocks.contains(id),
                        onToggle: { onToggleToolBlock(id) }
                    )
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

}

private struct StructuredToolBlock: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let id: String
    let title: String
    let summary: String
    let detail: String?
    let diff: ConversationStructuredDiff?
    let detailWasTruncated: Bool
    let icon: LXIconName
    let iconColor: Color
    let isExpanded: Bool
    let onToggle: () -> Void

    init(
        id: String,
        title: String,
        summary: String,
        detail: String? = nil,
        diff: ConversationStructuredDiff? = nil,
        detailWasTruncated: Bool = false,
        icon: LXIconName,
        iconColor: Color,
        isExpanded: Bool,
        onToggle: @escaping () -> Void
    ) {
        self.id = id
        self.title = title
        self.summary = summary
        self.detail = detail
        self.diff = diff
        self.detailWasTruncated = detailWasTruncated
        self.icon = icon
        self.iconColor = iconColor
        self.isExpanded = isExpanded
        self.onToggle = onToggle
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            if hasDetail {
                Button {
                    if reduceMotion {
                        onToggle()
                    } else {
                        withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.18)) {
                            onToggle()
                        }
                    }
                } label: {
                    summaryRow(showsDisclosure: true)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("conversation.structured-tool.\(id).toggle")
                .accessibilityLabel(accessibilityTitle)
                .accessibilityValue(isExpanded
                    ? String(localized: "chat_tool_show_less")
                    : String(localized: "chat_tool_show_more_label"))
            } else {
                summaryRow(showsDisclosure: false)
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("conversation.structured-tool.\(id)")
            }

            if isExpanded {
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(.system(size: 11.5, design: .monospaced))
                        .foregroundStyle(t.text2)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .padding(.leading, 26)
                }
                if let diff {
                    DiffView(diff: diff, showsFilePath: true)
                        .padding(.leading, 26)
                }
                if detailWasTruncated {
                    Text("chat_tool_body_truncated")
                        .font(.system(size: 11))
                        .foregroundStyle(t.text4)
                        .padding(.leading, 26)
                }
            }
        }
        .padding(.vertical, 3)
    }

    private var hasDetail: Bool {
        (detail?.isEmpty == false) || diff != nil || detailWasTruncated
    }

    private var accessibilityTitle: String {
        summary.isEmpty ? title : "\(title), \(summary)"
    }

    private func summaryRow(showsDisclosure: Bool) -> some View {
        HStack(alignment: .top, spacing: 8) {
            LXIcon(name: icon, size: 18, color: iconColor, stroke: 1.65)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(t.text)
                    .multilineTextAlignment(.leading)
                if !summary.isEmpty {
                    Text(summary)
                        .font(.system(size: 12))
                        .foregroundStyle(t.text3)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
            }
            Spacer(minLength: 4)
            if showsDisclosure {
                LXIcon(name: .chevron, size: 11, color: t.text4, stroke: 1.8)
                    .rotationEffect(.degrees(isExpanded ? 180 : 0))
                    .padding(.top, 3)
            }
        }
        .contentShape(Rectangle())
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

// MARK: - Assistant markdown rendering (PR-4 item 5)
//
// Renders the markdown subset assistant replies actually emit: fenced code
// blocks (```), inline code (`…`), **bold**, bullet / numbered lists, and GFM
// pipe tables — in addition to paragraph breaks. Anything else falls through
// as plain text.
//
// The text is first split into blocks (fenced code, tables, and everything
// else); ordinary text is then rendered line-by-line as a paragraph or list
// item, with `**bold**` + `` `inline code` `` parsed inline.

/// One parsed block of assistant markdown.
enum MDBlock: Equatable {
    /// A fenced code block (``` … ```), `lang` is the optional info string.
    case code(String, lang: String?)
    /// A run of ordinary text lines (paragraphs + list items).
    case text([MDLine])
    /// A GitHub-Flavored Markdown pipe table.
    case table(MDTable)
}

/// One line within a text block.
enum MDLine: Equatable {
    case paragraph(String)
    case bullet(String)
    case numbered(marker: String, String)
    case heading(level: Int, String)
    case quote(String)
    case task(checked: Bool, String)
    case nestedList(indent: Int, marker: String, String)
    case rule
}

struct MDTable: Equatable {
    let headers: [String]
    let alignments: [MDTableAlignment]
    let rows: [[String]]
}

enum MDTableAlignment: Equatable {
    case leading
    case center
    case trailing
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
        case let .code(code, language):
            codeBlock(code, language: language)
        case let .text(lines):
            textBlock(lines)
        case let .table(table):
            MDTableView(table: table)
                .equatable()
        }
    }

    // A fenced code block: monospaced, in a tinted rounded panel.
    private func codeBlock(_ code: String, language: String?) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language.flatMap { $0.isEmpty ? nil : $0 } ?? "Code")
                    .font(.system(size: 11.5, weight: .medium, design: .monospaced))
                    .foregroundStyle(t.text3)
                Spacer()
                Button {
                    UIPasteboard.general.string = code
                } label: {
                    Label("Copy", systemImage: "doc.on.doc")
                        .font(.system(size: 11.5))
                }
                .buttonStyle(.plain)
                .foregroundStyle(t.text3)
                .accessibilityLabel("Copy code")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            Divider().overlay(t.border)
            ScrollView(.horizontal) {
                Text(code)
                    .font(.system(size: 13.5, design: .monospaced))
                    .foregroundStyle(t.text)
                    .fixedSize(horizontal: true, vertical: false)
                    .textSelection(.enabled)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
            }
        }
        .background(t.surface)
        .clipShape(.rect(cornerRadius: 10))
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

/// A compact, horizontally scrollable table for narrow phone transcripts.
/// Fixed column widths keep every row aligned while still allowing long tables
/// to scroll instead of compressing their content into unreadable slivers.
private struct MDTableView: View, Equatable {
    @Environment(\.theme) private var t
    let table: MDTable

    private static let fontSize: CGFloat = 13.5
    private static let minimumColumnWidth: CGFloat = 96
    private static let maximumColumnWidth: CGFloat = 220
    private static let cellHorizontalPadding: CGFloat = 10

    static func == (lhs: MDTableView, rhs: MDTableView) -> Bool {
        lhs.table == rhs.table
    }

    var body: some View {
        let widths = columnWidths
        let separators = separatorOffsets(for: widths)
        ScrollView(.horizontal) {
            VStack(alignment: .leading, spacing: 0) {
                tableRow(
                    table.headers,
                    isHeader: true,
                    columnWidths: widths,
                    separatorOffsets: separators
                )
                ForEach(Array(table.rows.enumerated()), id: \.offset) { _, row in
                    tableRow(
                        row,
                        isHeader: false,
                        columnWidths: widths,
                        separatorOffsets: separators
                    )
                }
            }
            .clipShape(.rect(cornerRadius: 9))
            .overlay(
                RoundedRectangle(cornerRadius: 9)
                    .stroke(t.border, lineWidth: 0.5)
            )
        }
        .scrollIndicators(.hidden)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }

    private var columnWidths: [CGFloat] {
        table.headers.indices.map { column in
            let cells = [table.headers[column]] + table.rows.map { row in
                column < row.count ? row[column] : ""
            }
            let widest = cells.map(Self.visualCharacterCount).max() ?? 0
            return min(
                Self.maximumColumnWidth,
                max(Self.minimumColumnWidth, CGFloat(widest) * 7.2 + 24)
            )
        }
    }

    private func separatorOffsets(for widths: [CGFloat]) -> [CGFloat] {
        var offset: CGFloat = 0
        return widths.dropLast().map { width in
            offset += width + Self.cellHorizontalPadding * 2
            return offset
        }
    }

    private func tableRow(
        _ cells: [String],
        isHeader: Bool,
        columnWidths: [CGFloat],
        separatorOffsets: [CGFloat]
    ) -> some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(table.headers.indices, id: \.self) { column in
                let text = column < cells.count ? cells[column] : ""
                Text(AIText.parseInline(text, size: Self.fontSize))
                    .font(.scaledSystem(
                        Self.fontSize,
                        weight: isHeader ? .semibold : .regular,
                        relativeTo: .body
                    ))
                    .foregroundStyle(isHeader ? t.text : t.text2)
                    .multilineTextAlignment(textAlignment(for: column))
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(
                        width: columnWidths[column],
                        alignment: frameAlignment(for: column)
                    )
                    .padding(.horizontal, Self.cellHorizontalPadding)
                    .padding(.vertical, 8)
            }
        }
        .background(isHeader ? t.surface : t.windowBg.opacity(0.35))
        .overlay(alignment: .topLeading) {
            GeometryReader { proxy in
                ZStack(alignment: .topLeading) {
                    ForEach(separatorOffsets.indices, id: \.self) { index in
                        Rectangle()
                            .fill(t.border)
                            .frame(width: 0.5, height: proxy.size.height)
                            .offset(x: separatorOffsets[index])
                    }
                }
                .frame(
                    width: proxy.size.width,
                    height: proxy.size.height,
                    alignment: .topLeading
                )
            }
            .allowsHitTesting(false)
        }
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(t.border)
                .frame(height: 0.5)
        }
    }

    private func frameAlignment(for column: Int) -> Alignment {
        switch table.alignments[column] {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }

    private func textAlignment(for column: Int) -> TextAlignment {
        switch table.alignments[column] {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }

    private static func visualCharacterCount(_ text: String) -> Int {
        text.reduce(into: 0) { width, character in
            width += character.unicodeScalars.allSatisfy(\.isASCII) ? 1 : 2
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
        case let .heading(level, s):
            Text(AIText.parseInline(s, size: Self.bodySize))
                .font(.system(size: max(16, 25 - CGFloat(level) * 2), weight: .semibold))
                .foregroundStyle(t.text)
                .padding(.top, 8)
                .accessibilityAddTraits(.isHeader)
        case let .quote(s):
            HStack(spacing: 10) {
                Rectangle().fill(t.border).frame(width: 3)
                inlineText(s)
            }
            .fixedSize(horizontal: false, vertical: true)
            .padding(.vertical, 4)
        case let .task(checked, s):
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: checked ? "checkmark.square" : "square")
                    .foregroundStyle(t.text3)
                inlineText(s)
            }
        case let .nestedList(indent, marker, s):
            listRow(marker: marker, s).padding(.leading, CGFloat(min(indent, 12)) * 8)
        case .rule:
            Divider().padding(.vertical, 8)
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

    /// Split the source into fenced-code, GFM table, and text blocks.
    static func parseBlocks(_ md: String) -> [MDBlock] {
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
            } else if i + 1 < lines.count,
                      let tableHeader = parseTableHeader(line, delimiter: lines[i + 1]) {
                flushText()
                i += 2
                var rows: [[String]] = []
                while i < lines.count,
                      !isTableBodyBoundary(lines[i]),
                      let cells = tableCells(in: lines[i], requiresSeparator: false) {
                    rows.append(normalize(cells, to: tableHeader.headers.count))
                    i += 1
                }
                blocks.append(.table(MDTable(
                    headers: tableHeader.headers,
                    alignments: tableHeader.alignments,
                    rows: rows
                )))
            } else {
                pendingText.append(line)
                i += 1
            }
        }
        flushText()
        return blocks
    }

    private static func parseTableHeader(
        _ header: String,
        delimiter: String
    ) -> (headers: [String], alignments: [MDTableAlignment])? {
        guard let headers = tableCells(in: header),
              let delimiterCells = tableCells(in: delimiter),
              !headers.isEmpty,
              headers.count == delimiterCells.count else {
            return nil
        }

        var alignments: [MDTableAlignment] = []
        alignments.reserveCapacity(delimiterCells.count)
        for cell in delimiterCells {
            let marker = cell.trimmingCharacters(in: .whitespaces)
            let hasLeadingColon = marker.first == ":"
            let hasTrailingColon = marker.last == ":"
            let dashes = marker.trimmingCharacters(in: CharacterSet(charactersIn: ":"))
            guard dashes.count >= 3, dashes.allSatisfy({ $0 == "-" }) else {
                return nil
            }
            if hasLeadingColon && hasTrailingColon {
                alignments.append(.center)
            } else if hasTrailingColon {
                alignments.append(.trailing)
            } else {
                alignments.append(.leading)
            }
        }
        return (headers, alignments)
    }

    /// Split one pipe row while preserving escaped `\|` characters inside a
    /// cell. Leading and trailing pipes are optional in GFM table syntax.
    private static func tableCells(
        in line: String,
        requiresSeparator: Bool = true
    ) -> [String]? {
        let source = line.trimmingCharacters(in: .whitespaces)
        let characters = Array(source)
        var cells: [String] = []
        var cell = ""
        var foundSeparator = false
        var pendingBackslashes = 0
        var index = 0

        while index < characters.count {
            let character = characters[index]
            if character == "\\" {
                pendingBackslashes += 1
            } else if character == "|" {
                cell.append(String(repeating: "\\", count: pendingBackslashes / 2))
                if pendingBackslashes.isMultiple(of: 2) {
                    cells.append(cell.trimmingCharacters(in: .whitespaces))
                    cell = ""
                    foundSeparator = true
                } else {
                    cell.append("|")
                }
                pendingBackslashes = 0
            } else {
                cell.append(String(repeating: "\\", count: pendingBackslashes))
                pendingBackslashes = 0
                cell.append(character)
            }
            index += 1
        }
        cell.append(String(repeating: "\\", count: pendingBackslashes))
        cells.append(cell.trimmingCharacters(in: .whitespaces))

        guard foundSeparator || !requiresSeparator else { return nil }
        if source.first == "|", cells.first?.isEmpty == true {
            cells.removeFirst()
        }
        if source.last == "|", cells.last?.isEmpty == true {
            cells.removeLast()
        }
        return cells
    }

    /// GFM tables continue through ordinary non-empty lines (a one-cell row is
    /// valid) and stop when another block-level construct begins.
    private static func isTableBodyBoundary(_ line: String) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return true }
        if line.hasPrefix("    ") || line.hasPrefix("\t") { return true }
        if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") || trimmed.hasPrefix(">") {
            return true
        }

        let hashCount = trimmed.prefix(while: { $0 == "#" }).count
        if (1 ... 6).contains(hashCount) {
            let suffix = trimmed.dropFirst(hashCount)
            if suffix.isEmpty || suffix.first?.isWhitespace == true { return true }
        }

        if let marker = trimmed.first, "-+*".contains(marker) {
            let suffix = trimmed.dropFirst()
            if suffix.isEmpty || suffix.first?.isWhitespace == true { return true }
        }

        let digits = trimmed.prefix(while: \.isNumber)
        if !digits.isEmpty, digits.count <= 9 {
            let suffix = trimmed.dropFirst(digits.count)
            if let marker = suffix.first, marker == "." || marker == ")" {
                let afterMarker = suffix.dropFirst()
                if afterMarker.isEmpty || afterMarker.first?.isWhitespace == true { return true }
            }
        }

        let thematicMarker = trimmed.filter { !$0.isWhitespace }
        if thematicMarker.count >= 3,
           let marker = thematicMarker.first,
           "-_*".contains(marker),
           thematicMarker.allSatisfy({ $0 == marker }) {
            return true
        }
        return false
    }

    private static func normalize(_ cells: [String], to columnCount: Int) -> [String] {
        if cells.count >= columnCount {
            return Array(cells.prefix(columnCount))
        }
        return cells + Array(repeating: "", count: columnCount - cells.count)
    }

    /// Classify one text line as a paragraph, bullet, or numbered item.
    fileprivate static func parseLine(_ raw: String) -> MDLine {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        let indent = raw.prefix(while: { $0 == " " || $0 == "\t" }).reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
        let hashes = trimmed.prefix(while: { $0 == "#" }).count
        if (1...6).contains(hashes), trimmed.dropFirst(hashes).hasPrefix(" ") {
            return .heading(level: hashes, String(trimmed.dropFirst(hashes + 1)))
        }
        if trimmed.hasPrefix("> ") { return .quote(String(trimmed.dropFirst(2))) }
        let markerText = trimmed.filter { !$0.isWhitespace }
        if markerText.count >= 3, let marker = markerText.first, "-_*".contains(marker), markerText.allSatisfy({ $0 == marker }) {
            return .rule
        }
        for marker in ["- [x] ", "- [X] ", "- [ ] "] where trimmed.hasPrefix(marker) {
            return .task(checked: marker != "- [ ] ", String(trimmed.dropFirst(marker.count)))
        }
        // Bullets: -, *, or • followed by a space.
        for marker in ["- ", "* ", "• "] where trimmed.hasPrefix(marker) {
            let text = String(trimmed.dropFirst(marker.count))
            return indent > 0 ? .nestedList(indent: indent, marker: "•", text) : .bullet(text)
        }
        // Numbered: `1.` / `1)` followed by a space.
        if let dot = trimmed.firstIndex(where: { $0 == "." || $0 == ")" }) {
            let head = trimmed[trimmed.startIndex..<dot]
            let afterIdx = trimmed.index(after: dot)
            if !head.isEmpty, head.allSatisfy(\.isNumber),
               afterIdx < trimmed.endIndex, trimmed[afterIdx] == " " {
                let body = String(trimmed[trimmed.index(after: afterIdx)...])
                return indent > 0 ? .nestedList(indent: indent, marker: "\(head).", body) : .numbered(marker: "\(head).", body)
            }
        }
        return .paragraph(trimmed)
    }

    // Foundation's CommonMark parser preserves nested emphasis, links, escaping
    // and inline code; the native Text renderer applies their presentation intents.
    static func parseInline(_ s: String, size: CGFloat) -> AttributedString {
        guard var result = try? AttributedString(markdown: s, options: .init(
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible
        )) else { return AttributedString(s) }
        for run in result.runs {
            if run.inlinePresentationIntent?.contains(.code) == true {
                result[run.range].font = .system(size: size - 1, design: .monospaced)
            }
        }
        return result
    }
}

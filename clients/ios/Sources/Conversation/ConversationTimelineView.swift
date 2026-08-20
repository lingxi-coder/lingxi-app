import SwiftUI

/// Codex-style compact transcript timeline.  Execution runs are represented by
/// their reasoning and tool rows; the old bordered Agent Run card is never
/// rendered here.  State for expandable rows is owned by this list, keyed by
/// the stable run/tool identifiers supplied by the engine projection.
struct ConversationTimelineView: View {
    @Environment(\.theme) private var t

    let groups: [ConversationTimelineGroup]
    let messageDetails: [UUID: ConversationMessageDetail]
    let expandedToolCalls: Set<String>
    let onToggleToolCall: (String) -> Void
    let onShareMessage: (String) -> Void

    @State private var expandedReasoningIDs: Set<String> = []
    /// Folded user bubbles, keyed by the engine-supplied row id.
    ///
    /// Owned by the LIST, not the row: `MessageBubble` lives in a `LazyVStack`,
    /// which releases off-screen rows and takes their `@State` with them — a
    /// prompt the user expanded would silently re-collapse after scrolling
    /// past it. Android keeps the same fold in `rememberSaveable(message.id)`,
    /// so row-local state would also make the two platforms disagree about how
    /// long the user's action lasts.
    @State private var expandedUserMessageIDs: Set<String> = []
    @State private var expandedBatchIDs: Set<String> = []

    var body: some View {
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(timelineSegments) { segment in
                segmentView(segment)
            }
        }
    }

    private var timelineSegments: [Segment] {
        groups.flatMap(segments(in:))
    }

    @ViewBuilder
    private func segmentView(_ segment: Segment) -> some View {
        switch segment {
        case let .message(rowID, message):
            MessageBubble(
                message: message,
                detail: messageDetails[message.id],
                expandedToolBlocks: ConversationToolExpansionKey.structuredToolIDs(
                    in: expandedToolCalls,
                    messageID: message.id
                ),
                onToggleToolBlock: { toolID in
                    onToggleToolCall(
                        ConversationToolExpansionKey.structured(
                            messageID: message.id,
                            toolID: toolID
                        )
                    )
                },
                onShare: onShareMessage,
                isUserExpanded: expandedUserMessageIDs.contains(rowID),
                onToggleUserExpanded: { toggleUserMessage(rowID) }
            )
            .id(rowID)
        case let .commandOutput(rowID, output):
            SlashCommandOutputCard(output: output)
                .id(rowID)
        case let .reasoning(id, text, isRunning):
            ConversationThoughtRow(
                id: id,
                text: text,
                isRunning: isRunning,
                isExpanded: expandedReasoningIDs.contains(id),
                onToggle: { toggleReasoning(id) }
            )
        case let .tool(id, trace):
            ToolCallView(
                trace: trace,
                isExpanded: expandedToolCalls.contains(trace.id),
                compact: true,
                onToggle: { onToggleToolCall(trace.id) }
            )
            .id(id)
        case let .toolBatch(id, tools):
            ConversationToolBatchRow(
                id: id,
                tools: tools,
                expandedToolCalls: expandedToolCalls,
                isExpanded: expandedBatchIDs.contains(id),
                onToggleBatch: { toggleBatch(id) },
                onToggleTool: onToggleToolCall
            )
        case let .notice(id, notice):
            ConversationTimelineNoticeRow(notice: notice)
                .id(id)
        }
    }

    private enum Segment: Identifiable {
        case message(id: String, message: Message)
        case commandOutput(id: String, output: ConversationCommandOutput)
        case reasoning(id: String, text: String, isRunning: Bool)
        case tool(id: String, trace: ConversationToolTrace)
        case toolBatch(id: String, tools: [ConversationToolTrace])
        case notice(id: String, notice: ConversationExecutionNotice)

        var id: String {
            switch self {
            case let .message(id, _), let .commandOutput(id, _), let .reasoning(id, _, _), let .tool(id, _),
                 let .toolBatch(id, _), let .notice(id, _):
                return id
            }
        }
    }

    private func segments(in group: ConversationTimelineGroup) -> [Segment] {
        var result: [Segment] = []
        var pendingTools: [(String, ConversationToolTrace)] = []

        func flushTools() {
            guard !pendingTools.isEmpty else { return }
            if pendingTools.count >= 2 {
                let id = "timeline-batch:\(group.id)"
                result.append(.toolBatch(id: id, tools: pendingTools.map(\.1)))
            } else if let one = pendingTools.first {
                result.append(.tool(id: one.0, trace: one.1))
            }
            pendingTools.removeAll(keepingCapacity: true)
        }

        for row in group.rows {
            switch row {
            case let .tool(runID, trace):
                pendingTools.append(("tool:\(runID ?? "none"):\(trace.id)", trace))
            case let .message(message):
                flushTools()
                result.append(.message(id: "message:\(message.id.uuidString)", message: message))
            case let .commandOutput(output):
                flushTools()
                result.append(.commandOutput(id: "command-output:\(output.id)", output: output))
            case let .reasoning(_, activityID, text):
                flushTools()
                let id = "thought:\(group.id):\(activityID)"
                result.append(.reasoning(
                    id: id,
                    text: text,
                    isRunning: group.status == .running
                ))
            case let .notice(runID, notice):
                flushTools()
                result.append(.notice(id: "notice:\(group.id):\(runID ?? "none"):\(notice.id)", notice: notice))
            }
        }
        flushTools()
        return result
    }

    private func toggleUserMessage(_ id: String) {
        if expandedUserMessageIDs.contains(id) {
            expandedUserMessageIDs.remove(id)
        } else {
            expandedUserMessageIDs.insert(id)
        }
    }

    private func toggleReasoning(_ id: String) {
        if expandedReasoningIDs.contains(id) {
            expandedReasoningIDs.remove(id)
        } else {
            expandedReasoningIDs.insert(id)
        }
    }

    private func toggleBatch(_ id: String) {
        if expandedBatchIDs.contains(id) {
            expandedBatchIDs.remove(id)
        } else {
            expandedBatchIDs.insert(id)
        }
    }
}

private struct ConversationThoughtRow: View {
    @Environment(\.theme) private var t

    let id: String
    let text: String
    let isRunning: Bool
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        Button(action: onToggle) {
            HStack(alignment: .top, spacing: 8) {
                LXIcon(name: .brain, size: 14, color: t.text3, stroke: 1.7)
                    .padding(.top, 2)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 5) {
                        Text(thoughtLabel)
                            .font(.system(size: 12.5, weight: .medium))
                            .foregroundStyle(t.text2)
                        Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(t.text4)
                    }
                    if !isExpanded, let preview = firstLine, !preview.isEmpty {
                        Text(preview)
                            .font(.system(size: 11.5))
                            .foregroundStyle(t.text4)
                            .lineLimit(1)
                    }
                    if isExpanded {
                        Text(text)
                            .font(.system(size: 12.5))
                            .foregroundStyle(t.text3)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(thoughtLabel)
        .accessibilityValue(isExpanded
            ? String(localized: "chat_agent_details_collapse")
            : String(localized: "chat_agent_details_expand"))
        .accessibilityIdentifier("conversation.timeline.thought.\(id)")
    }

    private var firstLine: String? {
        text.split(whereSeparator: { $0.isNewline }).first.map(String.init)
    }

    private var thoughtLabel: String {
        isRunning
            ? String(localized: "chat_thinking")
            : String(localized: "chat_thought")
    }
}

private struct ConversationToolBatchRow: View {
    @Environment(\.theme) private var t

    let id: String
    let tools: [ConversationToolTrace]
    let expandedToolCalls: Set<String>
    let isExpanded: Bool
    let onToggleBatch: () -> Void
    let onToggleTool: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onToggleBatch) {
                HStack(spacing: 8) {
                    LXIcon(name: batchIcon, size: 14, color: t.text3, stroke: 1.6)
                    Text(String(localized: "chat_tool_batch_count \(tools.count)"))
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundStyle(t.text2)
                    Text(summary)
                        .font(.system(size: 11.5))
                        .foregroundStyle(t.text4)
                        .lineLimit(1)
                    if let terminalStatus {
                        Text(terminalStatus.label)
                            .font(.system(size: 10.5, weight: .medium))
                            .foregroundStyle(terminalStatus == .failed ? t.danger : t.text3)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(t.text4)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "chat_tool_batch_count \(tools.count)"))
            .accessibilityValue(isExpanded
                ? String(localized: "chat_agent_details_collapse")
                : String(localized: "chat_agent_details_expand"))
            .accessibilityIdentifier("conversation.timeline.tool-batch.\(id)")

            if isExpanded {
                ForEach(tools) { trace in
                    ToolCallView(
                        trace: trace,
                        isExpanded: expandedToolCalls.contains(trace.id),
                        compact: true,
                        onToggle: { onToggleTool(trace.id) }
                    )
                    .id("\(id):\(trace.id)")
                }
            }
        }
    }

    private var summary: String {
        tools.compactMap { $0.header.map { ToolDisplayText.verbLabel($0) } ?? $0.tool }
            .joined(separator: " · ")
    }

    private var terminalStatus: ConversationToolStatus? {
        if tools.contains(where: { $0.status == .failed }) { return .failed }
        if tools.contains(where: { $0.status == .cancelled }) { return .cancelled }
        return nil
    }

    private var batchIcon: LXIconName {
        guard let first = tools.first.map({ ToolDisplayText.icon(header: $0.header, tool: $0.tool) }) else {
            return .workflow
        }
        let isUniform = tools.dropFirst().allSatisfy {
            ToolDisplayText.icon(header: $0.header, tool: $0.tool).rawValue == first.rawValue
        }
        return isUniform ? first : .workflow
    }
}

private struct ConversationTimelineNoticeRow: View {
    @Environment(\.theme) private var t
    let notice: ConversationExecutionNotice

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
            Text(notice.text)
                .font(.system(size: 12))
                .foregroundStyle(t.text3)
                .lineLimit(2)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("conversation.timeline.notice.\(notice.id)")
    }

    private var color: Color {
        switch notice.kind {
        case .info: return t.accent
        case .warning: return t.text3
        case .error: return t.danger
        }
    }
}

import SwiftUI

/// Desktop-style compact transcript. Disclosure state is owned by the session
/// model so recycled rows retain the user's choices. Historical reasoning stays
/// in the execution ledger; only its current Thinking status enters this view.
struct ConversationTimelineView: View {
    @Environment(\.theme) private var t

    let groups: [ConversationTimelineGroup]
    var liveToolIDs: Set<String>? = nil
    var hasLiveOwner: Bool = true
    let messageDetails: [UUID: ConversationMessageDetail]
    let expandedToolCalls: Set<String>
    let onToggleToolCall: (String) -> Void
    var transcriptAgents: [ConversationAgentSummary] = []
    var transcriptAgentAnchors: [String: String] = [:]
    var activeAgentID: String = ConversationModel.mainAgentID
    var onSelectAgent: (String) -> Void = { _ in }
    var streaming = false
    var showThinking = true

    var body: some View {
        let segments = timelineSegments
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(Array(segments.enumerated()), id: \.element.id) { index, segment in
                segmentView(segment)
                    .padding(.top, index == 0 ? 0 : Self.spacing(before: segments[index - 1], current: segment))
            }
        }
    }

    /// Desktop separates narration/activity rows by 18pt, while adjacent tool
    /// rows stay in a compact 8pt rhythm. Applying this at the segment boundary
    /// keeps the spacing stable as a tool group grows during streaming.
    private static func spacing(before: Segment, current: Segment) -> CGFloat {
        if case .toolBatch = before, case .toolBatch = current { return 8 }
        return 18
    }

    private var timelineSegments: [Segment] {
        Self.projectedSegments(
            groups: groups,
            transcriptAgents: transcriptAgents,
            transcriptAgentAnchors: transcriptAgentAnchors,
            liveToolIDs: liveToolIDs,
            hasLiveOwner: hasLiveOwner,
            streaming: streaming,
            showThinking: showThinking
        )
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
                isUserExpanded: expandedToolCalls.contains("user:\(rowID)"),
                onToggleUserExpanded: { onToggleToolCall("user:\(rowID)") },
                isAssistantExpanded: expandedToolCalls.contains("assistant:\(rowID)"),
                onToggleAssistantExpanded: { onToggleToolCall("assistant:\(rowID)") }
            )
            .id(rowID)
        case let .commandOutput(rowID, output):
            SlashCommandOutputCard(output: output)
                .id(rowID)
        case let .reasoning(_, _, isRunning):
            ConversationThoughtRow(
                isRunning: isRunning
            )
        case let .thinkingSlot(isVisible):
            ConversationThoughtRow(isRunning: isVisible)
                .opacity(isVisible ? 1 : 0)
                .accessibilityHidden(!isVisible)
                .allowsHitTesting(false)
        case let .toolBatch(id, tools):
            ConversationToolBatchRow(
                id: id,
                tools: tools,
                expandedToolCalls: expandedToolCalls,
                isExpanded: expandedToolCalls.contains(id),
                onToggleBatch: { onToggleToolCall(id) },
                onToggleTool: onToggleToolCall
            )
        case let .notice(id, notice):
            ConversationTimelineNoticeRow(notice: notice)
                .id(id)
        case let .agents(id, agents):
            ConversationTranscriptAgents(
                agents: agents,
                activeAgentID: activeAgentID,
                onSelect: onSelectAgent
            )
            .id(id)
        }
    }

    /// The `ForEach` id a tool group projects to, whatever its tool count.
    static func toolSegmentID(firstToolID: String) -> String {
        "timeline-tools:\(firstToolID)"
    }

    /// The row ids a group projects to. `Segment` is private, so this is the
    /// testable projection of it; it calls the same `segments(in:)` the view
    /// renders from, so the two cannot drift.
    static func segmentIDs(in group: ConversationTimelineGroup) -> [String] {
        segments(in: group).map(\.id)
    }

    /// Test seam for the exact projection rendered by this view. The returned
    /// ids include agent rows at their concrete placement, so callers can
    /// verify ordering and exactly-once attachment without instantiating a
    /// SwiftUI hierarchy.
    static func projectedSegmentIDs(
        groups: [ConversationTimelineGroup],
        transcriptAgents: [ConversationAgentSummary],
        transcriptAgentAnchors: [String: String],
        liveToolIDs: Set<String>? = nil,
        hasLiveOwner: Bool = true,
        streaming: Bool = false,
        showThinking: Bool = true
    ) -> [String] {
        projectedSegments(
            groups: groups,
            transcriptAgents: transcriptAgents,
            transcriptAgentAnchors: transcriptAgentAnchors,
            liveToolIDs: liveToolIDs,
            hasLiveOwner: hasLiveOwner,
            streaming: streaming,
            showThinking: showThinking
        ).map(\.id)
    }

    private static func projectedSegments(
        groups: [ConversationTimelineGroup],
        transcriptAgents: [ConversationAgentSummary],
        transcriptAgentAnchors: [String: String],
        liveToolIDs: Set<String>?,
        hasLiveOwner: Bool,
        streaming: Bool,
        showThinking: Bool
    ) -> [Segment] {
        let projectedGroups = ConversationDesktopTimeline.groups(
            groups,
            liveToolIDs: liveToolIDs,
            hasLiveOwner: hasLiveOwner
        )
        let visibleGroups = showThinking
            ? projectedGroups
            : projectedGroups.compactMap { group in
                let rows = group.rows.filter { row in
                    if case .reasoning = row { return false }
                    return true
                }
                guard !rows.isEmpty else { return nil }
                return ConversationTimelineGroup(id: group.id, runID: group.runID, rows: rows, status: group.status)
            }
        let children = transcriptAgents.filter { $0.id != ConversationModel.mainAgentID }
        var result: [Segment] = []

        func anchorID(for row: ConversationTimelineRow) -> String {
            if case let .tool(_, trace) = row { return trace.id }
            return row.id
        }

        func containsAnchor(_ anchor: String, in group: ConversationTimelineGroup) -> Bool {
            group.rows.contains { anchorID(for: $0) == anchor } || group.id == anchor
        }

        let top = children.filter { agent in
            guard let anchor = transcriptAgentAnchors[agent.id], !anchor.isEmpty else { return true }
            return !visibleGroups.contains { containsAnchor(anchor, in: $0) }
        }
        if !top.isEmpty { result.append(.agents(id: "agents:start", agents: top)) }

        for group in visibleGroups {
            // Only concrete row ids split a group. In particular, a run id is
            // never treated as a wildcard that could match multiple groups.
            let anchoredRowIDs = Set(children.compactMap { transcriptAgentAnchors[$0.id] })
                .filter { anchor in group.rows.contains { anchorID(for: $0) == anchor } }
            var chunks: [[ConversationTimelineRow]] = [[]]
            for row in group.rows {
                chunks[chunks.count - 1].append(row)
                if anchoredRowIDs.contains(anchorID(for: row)) { chunks.append([]) }
            }
            if chunks.last?.isEmpty == true { chunks.removeLast() }

            for (chunkIndex, rows) in chunks.enumerated() {
                let chunk = ConversationTimelineGroup(
                    id: "\(group.id):chunk:\(chunkIndex)",
                    runID: group.runID,
                    rows: rows,
                    status: group.status
                )
                result.append(contentsOf: Self.segments(in: chunk))
                let attached = children.filter { agent in
                    guard let anchor = transcriptAgentAnchors[agent.id], !anchor.isEmpty else { return false }
                    if rows.contains(where: { anchorID(for: $0) == anchor }) { return true }
                    // A group-id anchor is already concrete. Attach it after
                    // that group's final chunk, exactly once.
                    return chunkIndex == chunks.count - 1 && group.id == anchor
                }
                if !attached.isEmpty {
                    var anchorIDs: [String] = []
                    for anchor in attached.compactMap({ transcriptAgentAnchors[$0.id] })
                        where !anchorIDs.contains(anchor) {
                        anchorIDs.append(anchor)
                    }
                    let stableID = "agents:after:\(anchorIDs.joined(separator: ","))"
                    result.append(.agents(id: stableID, agents: attached))
                }
            }
        }
        if !groups.isEmpty || !result.isEmpty || (streaming && hasLiveOwner) {
            let hasRunningTool = result.contains { segment in
                if case let .toolBatch(_, tools) = segment { return tools.contains { $0.status == .running } }
                return false
            }
            let hasRunningThought = result.contains { segment in
                if case let .reasoning(_, _, isRunning) = segment { return isRunning }
                return false
            }
            // Keep one intrinsic-height slot at the end, including while tools
            // run and after completion. Toggling Thinking must not resize the
            // transcript or leave gaps where historical reasoning used to be.
            result.removeAll { segment in
                if case .reasoning = segment { return true }
                return false
            }
            result.append(.thinkingSlot(isVisible: showThinking && hasLiveOwner && !hasRunningTool && (streaming || hasRunningThought)))
        }
        return result
    }

    private enum Segment: Identifiable {
        case message(id: String, message: Message)
        case commandOutput(id: String, output: ConversationCommandOutput)
        case reasoning(id: String, text: String, isRunning: Bool)
        case thinkingSlot(isVisible: Bool)
        case toolBatch(id: String, tools: [ConversationToolTrace])
        case notice(id: String, notice: ConversationExecutionNotice)
        case agents(id: String, agents: [ConversationAgentSummary])

        var id: String {
            switch self {
            case .thinkingSlot:
                return "thought:bottom-slot"
            case let .message(id, _), let .commandOutput(id, _), let .reasoning(id, _, _),
                 let .toolBatch(id, _), let .notice(id, _), let .agents(id, _):
                return id
            }
        }
    }

    private static func segments(in group: ConversationTimelineGroup) -> [Segment] {
        var result: [Segment] = []
        var pendingTools: [ConversationToolTrace] = []

        func flushTools() {
            guard !pendingTools.isEmpty else { return }
            // ONE id for both shapes. A live group grows from one tool to two
            // mid-turn; if the single-tool id and the batch id differ, that
            // growth reads to SwiftUI as "remove a row, insert a different
            // row" and the subtree is rebuilt under the reader. The group id
            // is keyed on the group's FIRST tool, so it is stable as more
            // tools are appended.
            let id = Self.toolSegmentID(firstToolID: pendingTools[0].id)
            result.append(.toolBatch(id: id, tools: pendingTools))
            pendingTools.removeAll(keepingCapacity: true)
        }

        for row in group.rows {
            switch row {
            case let .tool(_, trace):
                pendingTools.append(trace)
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

}

private struct ConversationThoughtRow: View {
    @Environment(\.theme) private var t
    let isRunning: Bool

    var body: some View {
        Text("chat_thinking")
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(t.text3)
            .runtimeTextSweep(isActive: isRunning, highlightColor: t.text)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .accessibilityIdentifier("conversation.timeline.thinking")
    }
}

private struct ConversationToolBatchRow: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState private var isFocused: Bool

    let id: String
    let tools: [ConversationToolTrace]
    let expandedToolCalls: Set<String>
    let isExpanded: Bool
    let onToggleBatch: () -> Void
    let onToggleTool: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !activeTools.isEmpty {
                ForEach(activeTools) { trace in
                    ToolCallView(trace: trace, isExpanded: expandedToolCalls.contains(trace.id), compact: true,
                                 onToggle: { onToggleTool(trace.id) })
                }
            } else if !ordinaryTools.isEmpty {
            Button(action: onToggleBatch) {
                HStack(spacing: 8) {
                    LXIcon(name: batchIcon, size: 14, color: t.text3, stroke: 1.6)
                    HStack(spacing: 8) {
                        Text(summary)
                            .font(.system(size: 11.5))
                            .foregroundStyle(t.text4)
                            .lineLimit(1)
                    }
                    .runtimeTextSweep(isActive: containsRunningTool, highlightColor: t.text)
                    if failedToolCount > 0 {
                        Text("· \(failedToolCount) failed")
                            .font(.system(size: 10.5, weight: .medium))
                            .foregroundStyle(t.danger)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(t.text4)
                        .timelineChevron(
                            isHighlighted: isFocused,
                            isExpanded: isExpanded,
                            reduceMotion: reduceMotion
                        )
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(t.accent.opacity(isFocused || isExpanded ? 0.10 : 0))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .focused($isFocused)
            .accessibilityLabel(String(localized: "chat_tool_batch_count \(tools.count)"))
            .accessibilityValue(isExpanded
                ? String(localized: "chat_agent_details_collapse")
                : String(localized: "chat_agent_details_expand"))
            .accessibilityIdentifier("conversation.timeline.tool-batch.\(id)")

            if isExpanded {
                ForEach(ordinaryTools) { trace in
                    ToolCallView(
                        trace: trace,
                        isExpanded: expandedToolCalls.contains(trace.id),
                        compact: true,
                        onToggle: { onToggleTool(trace.id) }
                    )
                    .id("\(id):\(trace.id)")
                }
                .padding(.leading, 12)
            }
            }
            ForEach(tools.filter { $0.planDocument != nil }) { trace in
                if let document = trace.planDocument { PlanDocumentCard(document: document) }
            }
        }
    }

    private var ordinaryTools: [ConversationToolTrace] { tools.filter { $0.planDocument == nil } }
    private var activeTools: [ConversationToolTrace] { ConversationDesktopTimeline.activeTools(ordinaryTools) }

    private var summary: String {
        ConversationDesktopTimeline.summary(ordinaryTools)
    }

    private var failedToolCount: Int {
        tools.reduce(into: 0) { count, tool in
            if tool.status == .failed { count += 1 }
        }
    }

    private var containsRunningTool: Bool {
        tools.contains(where: { $0.status == .running })
    }

    private var batchIcon: LXIconName {
        guard let last = tools.last else { return .workflow }
        return ToolDisplayText.icon(header: last.header, tool: last.tool)
    }
}

/// Reveal disclosure marks on focus or expansion without shifting layout.
enum ConversationTimelineChevronPresentation {
    static let restingOpacity = 0.0
    static let highlightedOpacity = 1.0
    static let restingScale: CGFloat = 0.92
    static let highlightedScale: CGFloat = 1.0

    static func opacity(isHighlighted: Bool, isExpanded: Bool = false) -> Double {
        isHighlighted || isExpanded ? highlightedOpacity : restingOpacity
    }

    static func scale(isHighlighted: Bool) -> CGFloat {
        isHighlighted ? highlightedScale : restingScale
    }
}

extension View {
    func timelineChevron(isHighlighted: Bool, isExpanded: Bool, reduceMotion: Bool) -> some View {
        opacity(ConversationTimelineChevronPresentation.opacity(isHighlighted: isHighlighted, isExpanded: isExpanded))
            .scaleEffect(ConversationTimelineChevronPresentation.scale(isHighlighted: isHighlighted || isExpanded))
            .animation(reduceMotion ? nil : .easeOut(duration: 0.16), value: isHighlighted || isExpanded)
            .accessibilityHidden(true)
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

/// Desktop display projection keeps the durable activity ledger intact.
enum ConversationDesktopTimeline {
    static func groups(_ sourceGroups: [ConversationTimelineGroup], liveToolIDs: Set<String>? = nil,
                       hasLiveOwner: Bool = true) -> [ConversationTimelineGroup] {
        let groups = sourceGroups.map { group in
            ConversationTimelineGroup(id: group.id, runID: group.runID, rows: group.rows.map { row in
                guard case let .tool(runID, trace) = row, trace.status == .running,
                      let liveToolIDs, !liveToolIDs.contains(trace.id) else { return row }
                var settled = trace
                settled.status = .unknown
                return .tool(runID: runID, trace: settled)
            }, status: group.status)
        }
        var result: [ConversationTimelineGroup] = []
        let hasActiveTools = groups.contains { group in
            group.rows.contains { row in
                if case let .tool(_, trace) = row { return trace.status == .running }
                return false
            }
        }
        for (index, group) in groups.enumerated() {
            let rows = group.rows.filter { row in
                if case .reasoning = row {
                    return index == groups.count - 1 && group.status == .running && hasLiveOwner && !hasActiveTools
                }
                return true
            }
            guard !rows.isEmpty else { continue }
            let visible = ConversationTimelineGroup(id: group.id, runID: group.runID, rows: rows, status: group.status)
            if visible.isToolGroup, let previous = result.last, previous.isToolGroup,
               previous.runID == visible.runID {
                result[result.count - 1] = ConversationTimelineGroup(
                    id: previous.id, runID: previous.runID,
                    rows: previous.rows + visible.rows, status: visible.status)
            } else {
                result.append(visible)
            }
        }
        return result
    }

    static func activeTools(_ tools: [ConversationToolTrace]) -> [ConversationToolTrace] {
        tools.filter { $0.status == .running }
    }

    static func summary(_ tools: [ConversationToolTrace]) -> String {
        guard let last = tools.last else { return "" }
        let title = last.header.map(ToolDisplayText.title) ?? last.tool
        guard let detail = last.header?.subLine, !detail.text.isEmpty else { return title }
        return "\(title) · \(detail.prefix)\(detail.text)"
    }
}

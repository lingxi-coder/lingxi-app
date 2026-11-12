import SwiftUI

/// Desktop-style compact transcript. Disclosure state is owned by the session
/// model so recycled rows retain the user's choices. Historical reasoning stays
/// in the execution ledger; only its current Thinking status enters this view.
struct ConversationTimelineView: View {
    @Environment(\.theme) private var t
    @ScaledMetric(relativeTo: .body) private var spacingScale: CGFloat = 1

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
    /// Session whose store owns the inline widgets of this transcript.
    var visualizationSessionId: String = ""
    var highlightedRowID: String? = nil
    var onVisualizationFollowup: (VisualizationFollowup) -> Void = { _ in }

    var body: some View {
        let segments = timelineSegments
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(Array(segments.enumerated()), id: \.element.id) { index, segment in
                segmentView(segment)
                    .background(t.accent.opacity(highlightedRowID == segment.id ? 0.12 : 0), in: .rect(cornerRadius: 10))
                    .padding(.top, index == 0 ? 0 : Self.spacing(before: segments[index - 1], current: segment) * spacingScale)
            }
        }
        .scrollTargetLayout()
    }

    /// Keep related rows close on iPhone without depending on activity visibility.
    private static func spacing(before: Segment, current: Segment) -> CGFloat {
        if before.isUser || current.isUser { return 16 }
        if before.isActivity || current.isActivity { return 8 }
        return 12
    }

    static func projectedSegmentSpacings(
        groups: [ConversationTimelineGroup],
        transcriptAgents: [ConversationAgentSummary] = [],
        transcriptAgentAnchors: [String: String] = [:],
        streaming: Bool = false,
        showThinking: Bool = true
    ) -> [CGFloat] {
        let segments = projectedSegments(
            groups: groups, transcriptAgents: transcriptAgents, transcriptAgentAnchors: transcriptAgentAnchors,
            liveToolIDs: nil, hasLiveOwner: true, streaming: streaming, showThinking: showThinking
        )
        return segments.enumerated().map { index, segment in
            index == 0 ? 0 : spacing(before: segments[index - 1], current: segment)
        }
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
        case let .skillInstructions(rowID, message, name):
            ConversationSkillInstructionsRow(name: name, text: message.text,
                isExpanded: expandedToolCalls.contains("skill:\(rowID)"),
                onToggle: { onToggleToolCall("skill:\(rowID)") })
                .id(rowID)
        case let .message(rowID, message) where message.visualization != nil:
            VisualizationCardView(
                sessionId: visualizationSessionId,
                visualization: message.visualization!,
                onFollowup: onVisualizationFollowup
            )
            .id(rowID)
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
                isAssistantExpanded: message.streamed
                    ? !expandedToolCalls.contains("collapsed-assistant:\(rowID)")
                    : expandedToolCalls.contains("assistant:\(rowID)"),
                onToggleAssistantExpanded: {
                    onToggleToolCall(message.streamed ? "collapsed-assistant:\(rowID)" : "assistant:\(rowID)")
                }
            )
            .id(rowID)
        case let .commandOutput(rowID, output):
            SlashCommandOutputCard(output: output)
                .id(rowID)
        case .reasoning:
            EmptyView() // Intermediate ledger segment, folded into an activity before rendering.
        case let .activity(id, tools, reasoning, isRunning):
            ConversationToolBatchRow(id: id, tools: tools, expandedToolCalls: expandedToolCalls,
                isExpanded: expandedToolCalls.contains(id), onToggleBatch: { onToggleToolCall(id) },
                onToggleTool: onToggleToolCall, reasoning: reasoning, active: isRunning, unified: true)
                .id(id)
        case let .toolBatch(id, tools):
            ConversationToolBatchRow(
                id: id,
                tools: tools,
                expandedToolCalls: expandedToolCalls,
                isExpanded: expandedToolCalls.contains(id),
                onToggleBatch: { onToggleToolCall(id) },
                onToggleTool: onToggleToolCall
            )
            .id(id)
        case let .notice(id, notice):
            ConversationTimelineNoticeRow(notice: notice)
                .id(id)
        case let .agents(id, agents):
            ConversationTranscriptAgents(
                groupExpanded: expandedToolCalls.contains(id),
                onToggleGroup: { onToggleToolCall(id) },
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
            hasLiveOwner: hasLiveOwner, includeReasoning: true
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
                let represented = rows.filter { row in
                    guard case let .tool(_, trace) = row, trace.status == .completed,
                          let spawned = trace.spawnedAgentID else { return true }
                    let key = spawned.replacingOccurrences(of: "agent:", with: "")
                    return !children.contains { $0.id.replacingOccurrences(of: "agent:", with: "") == key && transcriptAgentAnchors[$0.id] == trace.id }
                }
                result.append(contentsOf: Self.segments(in: ConversationTimelineGroup(
                    id: chunk.id, runID: chunk.runID, rows: represented, status: chunk.status)))
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
        // Merge only adjacent visible launch rows; prose/tools preserve their boundaries.
        var merged: [Segment] = []
        for segment in result {
            if case let .agents(_, agents) = segment,
               let last = merged.last, case let .agents(id, previous) = last {
                merged[merged.count - 1] = .agents(id: id, agents: previous + agents)
            } else { merged.append(segment) }
        }
        result = merged
        var previousTool: ConversationToolTrace?
        for index in result.indices {
            switch result[index] {
            case let .toolBatch(_, tools):
                previousTool = tools.last
            case .reasoning:
                continue
            case let .message(id, message):
                if let tool = previousTool, ConversationSkillInstructions.matches(message, tool: tool) {
                    result[index] = .skillInstructions(id: id, message: message, name: tool.header?.primary ?? tool.tool)
                }
                previousTool = nil
            default:
                previousTool = nil
            }
        }
        // Reasoning and ordinary calls are states of one activity, keyed by
        // the preceding visible boundary rather than the first tool's arrival.
        var activities: [Segment] = []
        var calls: [ConversationToolTrace] = []
        var reasoning: [String] = []
        var active = false
        func flushActivity() {
            guard !calls.isEmpty || !reasoning.isEmpty || active else { return }
            let id = "activity:after:\(activities.last?.id ?? "start")"
            activities.append(.activity(id: id, tools: calls, reasoning: reasoning.joined(separator: "\n\n"), isRunning: active))
            calls = []; reasoning = []; active = false
        }
        for segment in result {
            switch segment {
            case let .reasoning(_, text, isRunning):
                if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { reasoning.append(text) }
                active = active || (showThinking && hasLiveOwner && isRunning)
            case let .toolBatch(_, tools) where tools.allSatisfy({ $0.groupingKey != nil && $0.planDocument == nil && $0.localApp == nil && $0.questionAnswers == nil }):
                calls.append(contentsOf: tools)
                active = active || (hasLiveOwner && tools.contains { $0.status == .running })
            default:
                flushActivity()
                activities.append(segment)
            }
        }
        if streaming && hasLiveOwner && showThinking {
            if !calls.isEmpty || !reasoning.isEmpty || activities.last?.isUser == true || activities.isEmpty { active = true }
        }
        flushActivity()
        result = activities
        return result
    }

    private enum Segment: Identifiable {
        case message(id: String, message: Message)
        case skillInstructions(id: String, message: Message, name: String)
        case commandOutput(id: String, output: ConversationCommandOutput)
        case reasoning(id: String, text: String, isRunning: Bool)
        case activity(id: String, tools: [ConversationToolTrace], reasoning: String, isRunning: Bool)
        case toolBatch(id: String, tools: [ConversationToolTrace])
        case notice(id: String, notice: ConversationExecutionNotice)
        case agents(id: String, agents: [ConversationAgentSummary])

        var isUser: Bool {
            if case let .message(_, message) = self { return message.role == .user }
            return false
        }

        var isActivity: Bool {
            switch self {
            case .toolBatch, .reasoning, .activity, .agents: return true
            default: return false
            }
        }

        var id: String {
            switch self {
            case let .message(id, _), let .skillInstructions(id, _, _), let .commandOutput(id, _), let .reasoning(id, _, _),
                 let .toolBatch(id, _), let .activity(id, _, _, _), let .notice(id, _), let .agents(id, _):
                return id
            }
        }
    }

    /// Resolve links through the same projection that renders tools and merged agents.
    static func toolDestinationID(toolID: String, groups: [ConversationTimelineGroup],
                                  transcriptAgents: [ConversationAgentSummary], transcriptAgentAnchors: [String: String]) -> String? {
        let segments = projectedSegments(groups: groups, transcriptAgents: transcriptAgents,
            transcriptAgentAnchors: transcriptAgentAnchors, liveToolIDs: nil, hasLiveOwner: true,
            streaming: false, showThinking: true)
        return segments.first { segment in
            switch segment {
            case let .toolBatch(_, tools), let .activity(_, tools, _, _): return tools.contains { $0.id == toolID }
            case let .agents(_, agents): return agents.contains { transcriptAgentAnchors[$0.id] == toolID }
            default: return false
            }
        }?.id
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
                if let last = pendingTools.last,
                   last.groupingKey == nil || last.groupingKey != trace.groupingKey { flushTools() }
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

private struct ConversationSkillInstructionsRow: View {
    @Environment(\.theme) private var t
    let name: String
    let text: String
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button(action: onToggle) {
                HStack(spacing: 8) {
                    LXIcon(name: .sparkles, size: 16, color: t.text3, stroke: 1.6)
                    Text(name).font(.footnote).foregroundStyle(t.text3)
                }.frame(minHeight: 40).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(isExpanded ? String(localized: "chat_agent_details_collapse") : String(localized: "chat_agent_details_expand"))
            if isExpanded { AIText(markdown: text).padding(.leading, 24) }
        }
    }
}

private struct ConversationToolBatchRow: View {
    @Environment(\.theme) private var t
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState private var isFocused: Bool
    @State private var isHovering = false

    let id: String
    let tools: [ConversationToolTrace]
    let expandedToolCalls: Set<String>
    let isExpanded: Bool
    let onToggleBatch: () -> Void
    let onToggleTool: (String) -> Void
    var reasoning: String = ""
    var active = false
    var unified = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if tools.contains(where: { $0.questionAnswers != nil || $0.localApp != nil }) {
                ForEach(ordinaryTools) { trace in
                    ToolCallView(trace: trace, isExpanded: expandedToolCalls.contains(trace.id), compact: true,
                                 onToggle: { onToggleTool(trace.id) })
                }
            } else if !unified, ordinaryTools.count == 1, let trace = ordinaryTools.first {
                ToolCallView(trace: trace, isExpanded: expandedToolCalls.contains(trace.id), compact: true,
                             onToggle: { onToggleTool(trace.id) })
            } else if unified || !ordinaryTools.isEmpty {
            Button(action: onToggleBatch) {
                HStack(spacing: 8) {
                    LXIcon(name: batchIcon, size: 14, color: t.text3, stroke: 1.6)
                    HStack(spacing: 8) {
                        Text(summary)
                            .font(.caption)
                            .foregroundStyle(t.text3)
                            .lineLimit(1)
                    }
                    .runtimeTextSweep(isActive: active || containsRunningTool, highlightColor: t.text)
                    if containsRunningTool {
                        Text(String(localized: "chat_tool_running_count \(activeTools.count)"))
                            .font(.caption).foregroundStyle(t.text3)
                    }
                    if (unified || ordinaryTools.allSatisfy({ $0.isTaskWorkflow })), ordinaryTools.contains(where: { $0.status == .failed }) {
                        Text("\(ordinaryTools.filter { $0.status == .failed }.count) \(String(localized: "chat_tool_calls_unsuccessful"))")
                            .font(.caption).foregroundStyle(t.danger).lineLimit(1).layoutPriority(1)
                    } else if !ordinaryTools.isEmpty && !containsRunningTool && ordinaryTools.allSatisfy({ $0.status == .failed }) {
                        Text(String(localized: "chat_tool_calls_unsuccessful"))
                            .font(.caption).foregroundStyle(t.text3)
                    }
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(t.text4)
                        .timelineChevron(
                            isHighlighted: true,
                            isExpanded: isExpanded,
                            reduceMotion: reduceMotion
                        )
                }
                .frame(minHeight: 44)
                .padding(.horizontal, 10)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .focused($isFocused)
            .onHover { isHovering = $0 }
            .accessibilityLabel(unified ? summary : String(localized: "chat_tool_batch_count \(tools.count)"))
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
            if isExpanded && !reasoning.isEmpty { AIText(markdown: reasoning).padding(.leading, 24) }
            ForEach(tools.filter { $0.planDocument != nil }) { trace in
                if let document = trace.planDocument { PlanDocumentCard(document: document) }
            }
        }
    }

    private var ordinaryTools: [ConversationToolTrace] { tools.filter { $0.planDocument == nil } }
    private var activeTools: [ConversationToolTrace] { ConversationDesktopTimeline.activeTools(ordinaryTools) }

    private var summary: String {
        if unified && (ordinaryTools.isEmpty || (active && !containsRunningTool && !reasoning.isEmpty)) {
            return reasoning.isEmpty ? String(localized: "chat_thinking") : String(reasoning.suffix(240)).replacingOccurrences(of: "\n", with: " ").replacingOccurrences(of: "\r", with: " ")
        }
        return ConversationDesktopTimeline.summary(ordinaryTools)
    }

    private var containsRunningTool: Bool {
        tools.contains(where: { $0.status == .running })
    }

    private var batchIcon: LXIconName {
        guard let first = tools.first else { return .sparkles }
        if tools.allSatisfy({ $0.isTaskWorkflow }) { return .listChecks }
        return ToolDisplayText.icon(header: first.header, tool: first.tool)
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
                .font(.caption)
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
                       hasLiveOwner: Bool = true, includeReasoning: Bool = false) -> [ConversationTimelineGroup] {
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
        var typedGroups: [ConversationTimelineGroup] = []
        for group in groups {
            var chunk: [ConversationTimelineRow] = []
            var chunkIndex = 0
            func flushChunk() {
                guard !chunk.isEmpty else { return }
                let suffix = chunkIndex == 0 ? "" : ":\(chunk[0].id)"
                typedGroups.append(ConversationTimelineGroup(id: group.id + suffix, runID: group.runID, rows: chunk, status: group.status))
                chunkIndex += 1
                chunk.removeAll(keepingCapacity: true)
            }
            for row in group.rows {
                if case let .tool(_, trace) = row, let last = chunk.last, case let .tool(_, previous) = last,
                   previous.groupingKey == nil || previous.groupingKey != trace.groupingKey { flushChunk() }
                chunk.append(row)
            }
            flushChunk()
        }
        for (index, group) in typedGroups.enumerated() {
            let rows = group.rows.filter { row in
                if case .reasoning = row {
                    return includeReasoning || (index == typedGroups.count - 1 && group.status == .running && hasLiveOwner && !hasActiveTools)
                }
                return true
            }
            guard !rows.isEmpty else { continue }
            let visible = ConversationTimelineGroup(id: group.id, runID: group.runID, rows: rows, status: group.status)
            if visible.isToolGroup, let previous = result.last, previous.isToolGroup,
               previous.runID == visible.runID,
               case let .tool(_, previousTool)? = previous.rows.last,
               case let .tool(_, nextTool)? = visible.rows.first,
               let key = previousTool.groupingKey, key == nextTool.groupingKey {
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
        guard let first = tools.first else { return "" }
        if tools.count == 1 {
            if first.tool == "LocalApp" {
                let argument = first.header?.primary ?? (first.status == .failed ? nil : first.display.flatMap { ToolDisplayText.headline($0, trace: first) })
                return [String(localized: "chat_local_app_kind"), argument].compactMap { $0 }.joined(separator: " ")
            }
            return first.header.map(ToolDisplayText.title) ?? first.tool
        }
        let keys = Set(tools.compactMap(\.groupingKey))
        if keys.count > 1 {
            var labels: [String] = []
            for trace in tools {
                let label = trace.header.map(ToolDisplayText.verbLabel) ?? trace.tool
                if !labels.contains(label) { labels.append(label) }
            }
            return "\(labels.joined(separator: ", ")) · \(String(localized: "chat_tool_calls_count \(tools.count)"))"
        }
        let label = tools.allSatisfy({ $0.isTaskWorkflow })
            ? String(localized: tools.allSatisfy({ ConversationToolTrace.isTaskProgressTool($0.tool) }) ? "chat_tool_group_tasks" : "chat_tool_group_coordination")
            : first.header.map { header in
            header.verb == .shell ? String(localized: "chat_tool_group_shell") : ToolDisplayText.verbLabel(header)
        } ?? first.tool
        return "\(label) · \(String(localized: "chat_tool_calls_count \(tools.count)"))"
    }
}

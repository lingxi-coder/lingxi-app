// TasksStatusPanel.swift — the pinned background-tasks widget.
//
// The mobile analog of Claude Code's workflow footer: background tasks remain
// compact by default, but a workflow task can expand into structured
// phase/agent progress instead of collapsing everything into one description
// line or relying on transcript polling.

import SwiftUI

struct TasksStatusPanel: View {
    @Environment(\.theme) private var theme
    let tasks: [BackgroundTaskSnapshot]
    let onResume: (String) -> Void
    let showsContainer: Bool
    let workflowResumeState: WorkflowResumeState
    @State private var collapsed = false
    @State private var expandedTaskIDs: Set<String> = []

    static let visibleFinishedLimit = 3
    private static let maxPanelHeight: CGFloat = 240

    struct TaskCounts: Equatable {
        let total: Int
        let queued: Int
        let running: Int
        let succeeded: Int
        let failed: Int
        let paused: Int
        let cancelled: Int
    }

    static func taskCounts(_ tasks: [BackgroundTaskSnapshot]) -> TaskCounts {
        TaskCounts(
            total: tasks.count,
            queued: tasks.filter { $0.status == .pending }.count,
            running: tasks.filter { $0.status == .running }.count,
            succeeded: tasks.filter { $0.status == .completed }.count,
            failed: tasks.filter { $0.status == .failed }.count,
            paused: tasks.filter { $0.status == .paused }.count,
            cancelled: tasks.filter { $0.status == .cancelled }.count
        )
    }

    private var active: [BackgroundTaskSnapshot] { tasks.filter { !$0.status.isTerminal } }
    private var finished: [BackgroundTaskSnapshot] { tasks.filter { $0.status.isTerminal } }
    private var counts: TaskCounts { Self.taskCounts(tasks) }

    init(
        tasks: [BackgroundTaskSnapshot],
        onResume: @escaping (String) -> Void = { _ in },
        showsContainer: Bool = true,
        workflowResumeState: WorkflowResumeState = .idle
    ) {
        self.tasks = tasks
        self.onResume = onResume
        self.showsContainer = showsContainer
        self.workflowResumeState = workflowResumeState
    }

    @ViewBuilder
    private var panelContent: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if !collapsed {
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(active) { taskCard(for: $0) }
                        ForEach(finished.suffix(Self.visibleFinishedLimit)) { taskCard(for: $0) }
                        if finished.count > Self.visibleFinishedLimit {
                            Text("chat_tasks_more_completed \(finished.count - Self.visibleFinishedLimit)")
                                .font(.caption2)
                                .foregroundStyle(theme.text4)
                                .padding(.leading, 22)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: Self.maxPanelHeight)
                .scrollBounceBehavior(.basedOnSize)
            }
        }
    }

    var body: some View {
        if showsContainer {
            panelContent
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(theme.surface)
                        .overlay(
                            RoundedRectangle(cornerRadius: 12, style: .continuous)
                                .stroke(theme.border, lineWidth: 1)
                        )
                )
                .accessibilityIdentifier("chat.tasks-panel")
        } else {
            panelContent
        }
    }

    struct WorkflowSection: Identifiable, Equatable {
        let id: String
        let title: String
        let subtitle: String?
        let agents: [ConversationWorkflowAgentSnapshot]
        let logs: [ConversationWorkflowLogSnapshot]
    }

    static func workflowSections(
        for run: ConversationWorkflowRunSnapshot,
        nowMs: UInt64 = currentWallClockMs()
    ) -> [WorkflowSection] {
        var sections: [WorkflowSection] = []
        var consumedAgentIDs = Set<String>()
        var consumedLogIDs = Set<String>()

        for phase in run.sortedPhases {
            let agents = run.sortedAgents.filter { agent in
                let matchesIndex = phase.index != nil && agent.phaseIndex == phase.index
                let matchesTitle = !matchesIndex
                    && agent.phaseIndex == nil
                    && agent.phaseTitle == phase.title
                let include = matchesIndex || matchesTitle
                if include { consumedAgentIDs.insert(agent.id) }
                return include
            }
            let logs = run.logs.filter { log in
                let matchesIndex = phase.index != nil && log.phaseIndex == phase.index
                let matchesTitle = !matchesIndex
                    && log.phaseIndex == nil
                    && log.phaseTitle == phase.title
                let include = matchesIndex || matchesTitle
                if include { consumedLogIDs.insert(log.id) }
                return include
            }
            let subtitle = phase.message
                ?? logs.last?.message
                ?? agents.first(where: { !$0.state.isTerminal })?.activityLine
            sections.append(WorkflowSection(
                id: phase.id,
                title: phase.title,
                subtitle: subtitle,
                agents: agents,
                logs: logs
            ))
        }

        let ungroupedAgents = run.sortedAgents.filter { !consumedAgentIDs.contains($0.id) }
        let ungroupedLogs = run.logs.filter { !consumedLogIDs.contains($0.id) }
        if !ungroupedAgents.isEmpty || !ungroupedLogs.isEmpty {
            sections.append(WorkflowSection(
                id: "workflow-ungrouped",
                title: String(localized: "chat_workflow_activity"),
                subtitle: ungroupedLogs.last?.message ?? ungroupedAgents.first?.activityLine,
                agents: ungroupedAgents,
                logs: ungroupedLogs
            ))
        }

        if sections.isEmpty, !run.sortedAgents.isEmpty {
            sections.append(WorkflowSection(
                id: "workflow-agents",
                title: run.currentPhaseTitle ?? String(localized: "chat_workflow_title"),
                subtitle: nil,
                agents: run.sortedAgents,
                logs: []
            ))
        }

        _ = nowMs // kept explicit so tests can fix time without warning churn
        return sections
    }

    static func workflowCompactSummary(
        for task: BackgroundTaskSnapshot,
        nowMs: UInt64 = currentWallClockMs()
    ) -> String? {
        guard let workflow = task.workflow else { return nil }
        var parts: [String] = []
        if workflow.totalAgents > 0 {
            parts.append(String(localized:
                "chat_workflow_agents_done \(workflow.succeededAgents) \(workflow.totalAgents)"))
            if workflow.runningAgents > 0 {
                parts.append(String(localized:
                    "chat_workflow_agents_running \(workflow.runningAgents)"))
            }
            if workflow.queuedAgents > 0 {
                parts.append(String(localized:
                    "chat_workflow_agents_queued \(workflow.queuedAgents)"))
            }
            if workflow.failedAgents > 0 {
                parts.append(String(localized:
                    "chat_workflow_agents_failed \(workflow.failedAgents)"))
            }
        }
        if let phase = workflow.currentPhaseTitle, !phase.isEmpty {
            parts.append(phase)
        }
        if task.status.isTerminal {
            parts.append(task.status.label)
        }
        _ = nowMs
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    static func workflowAgentMetrics(
        _ agent: ConversationWorkflowAgentSnapshot,
        nowMs: UInt64 = currentWallClockMs()
    ) -> String {
        var parts: [String] = [agent.state.label]
        if let model = agent.model, !model.isEmpty {
            if let fallback = agent.fallbackModel, !fallback.isEmpty, fallback != model {
                parts.append("\(model) → \(fallback)")
            } else {
                parts.append(model)
            }
        }
        if let tokens = agent.tokens {
            parts.append(String(localized: "chat_workflow_tokens \(tokens)"))
        }
        if let toolCalls = agent.toolCalls {
            parts.append(String(localized: "chat_workflow_tools \(toolCalls)"))
        }
        if let duration = agent.durationMs(nowMs: nowMs) {
            parts.append(formatDuration(duration))
        }
        if let attempt = agent.attempt, attempt > 1 {
            if let reason = agent.lastAttemptReason, !reason.isEmpty {
                parts.append(String(localized: "chat_workflow_retry_reason \(attempt) \(reason)"))
            } else {
                parts.append(String(localized: "chat_workflow_retry \(attempt)"))
            }
        }
        return parts.joined(separator: " · ")
    }

    private static func currentWallClockMs() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1_000)
    }

    static func formatDuration(_ durationMs: UInt64) -> String {
        let seconds = durationMs / 1_000
        let minutes = seconds / 60
        let remaining = seconds % 60
        if minutes == 0 {
            return "\(remaining)s"
        }
        return "\(minutes)m \(String(format: "%02d", remaining))s"
    }

    private var header: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) { collapsed.toggle() }
        } label: {
            HStack(spacing: 6) {
                if counts.running > 0 {
                    ProgressView().controlSize(.mini)
                } else {
                    Image(systemName: "checklist")
                        .font(.caption)
                        .foregroundStyle(theme.text3)
                }
                Text(
                    "chat_tasks_summary_detail \(counts.total) \(counts.succeeded) \(counts.running) \(counts.queued) \(counts.failed) \(counts.paused) \(counts.cancelled)"
                )
                .font(.caption)
                .foregroundStyle(theme.text2)
                Spacer(minLength: 0)
                Image(systemName: collapsed ? "chevron.down" : "chevron.up")
                    .font(.caption2)
                    .foregroundStyle(theme.text4)
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("chat.tasks-panel.toggle")
        .accessibilityHint(Text(collapsed ? "chat_tasks_expand_hint" : "chat_tasks_collapse_hint"))
    }

    @ViewBuilder
    private func taskCard(for task: BackgroundTaskSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            if let workflow = task.workflow {
                workflowTaskCard(task: task, workflow: workflow)
            } else {
                plainTaskRow(task)
            }
            if task.canResume, task.status == .paused {
                Button {
                    onResume(task.id)
                } label: {
                    Label(String(localized: "chat_workflow_resume"), systemImage: "play.fill")
                        .font(.caption2.weight(.semibold))
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.mini)
                .padding(.leading, 22)
                .disabled(isResuming(task.id))
            }
            if case let .failed(taskID, message) = workflowResumeState, taskID == task.id {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(theme.danger)
                    .lineLimit(3)
                    .padding(.leading, 22)
            }
        }
    }

    private func isResuming(_ taskID: String) -> Bool {
        if case let .resuming(activeTaskID) = workflowResumeState {
            return activeTaskID == taskID
        }
        return false
    }

    private func plainTaskRow(_ task: BackgroundTaskSnapshot) -> some View {
        HStack(spacing: 8) {
            statusIcon(task.status)
                .frame(width: 14)
            Text(task.descriptionText.isEmpty ? task.id : task.descriptionText)
                .font(.caption)
                .lineLimit(1)
                .strikethrough(task.status == .completed)
                .foregroundStyle(task.status.isTerminal ? theme.text4 : theme.text)
            Spacer(minLength: 0)
        }
    }

    private func workflowTaskCard(
        task: BackgroundTaskSnapshot,
        workflow: ConversationWorkflowRunSnapshot
    ) -> some View {
        let expanded = expandedTaskIDs.contains(task.id)
        let title = task.descriptionText.isEmpty ? task.id : task.descriptionText
        let summary = Self.workflowCompactSummary(for: task) ?? task.status.label

        return VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) {
                    if expanded {
                        expandedTaskIDs.remove(task.id)
                    } else {
                        expandedTaskIDs.insert(task.id)
                    }
                }
            } label: {
                HStack(alignment: .top, spacing: 8) {
                    statusIcon(task.status)
                        .frame(width: 14, height: 14)
                        .padding(.top, 2)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(task.status.isTerminal ? theme.text3 : theme.text)
                            .lineLimit(1)
                        Text(summary)
                            .font(.caption2)
                            .foregroundStyle(theme.text3)
                            .lineLimit(2)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(theme.text4)
                        .padding(.top, 2)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Self.workflowSections(for: workflow)) { section in
                        workflowSection(section)
                    }
                }
                .padding(.leading, 22)
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .padding(.vertical, 2)
    }

    private func workflowSection(_ section: WorkflowSection) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(section.title)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                if !section.agents.isEmpty {
                    Text("\(section.agents.filter { $0.state.isSuccessLike }.count)/\(section.agents.count)")
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(theme.text4)
                }
            }
            if let subtitle = section.subtitle, !subtitle.isEmpty {
                Text(subtitle)
                    .font(.caption2)
                    .foregroundStyle(theme.text3)
                    .lineLimit(2)
            }
            ForEach(section.agents) { agent in
                workflowAgentRow(agent)
            }
            if section.agents.isEmpty, let log = section.logs.last?.message, !log.isEmpty {
                Text(log)
                    .font(.caption2)
                    .foregroundStyle(theme.text3)
                    .lineLimit(2)
            }
        }
    }

    private func workflowAgentRow(_ agent: ConversationWorkflowAgentSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Circle()
                    .fill(agentStateColor(agent.state))
                    .frame(width: 6, height: 6)
                Text(agent.displayTitle)
                    .font(.caption)
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.leading, 2)

            Text(Self.workflowAgentMetrics(agent))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(theme.text3)
                .padding(.leading, 14)
                .lineLimit(2)

            if let activity = agent.activityLine, !activity.isEmpty {
                Text(activity)
                    .font(.caption2)
                    .foregroundStyle(agent.state == .error ? theme.danger : theme.text4)
                    .padding(.leading, 14)
                    .lineLimit(2)
            }
        }
    }

    @ViewBuilder
    private func statusIcon(_ status: BackgroundTaskSnapshot.Status) -> some View {
        switch status {
        case .running:
            ProgressView().controlSize(.mini)
        case .pending:
            Image(systemName: "clock")
                .font(.caption2)
                .foregroundStyle(theme.text3)
        case .paused:
            Image(systemName: "pause.circle")
                .font(.caption2)
                .foregroundStyle(theme.text3)
        case .completed:
            Image(systemName: "checkmark")
                .font(.caption2)
                .foregroundStyle(theme.ok)
        case .failed:
            Image(systemName: "xmark")
                .font(.caption2)
                .foregroundStyle(theme.danger)
        case .cancelled:
            Image(systemName: "minus.circle")
                .font(.caption2)
                .foregroundStyle(theme.text4)
        }
    }

    private func agentStateColor(_ state: ConversationWorkflowAgentState) -> Color {
        switch state {
        case .done, .cached:
            return theme.ok
        case .error:
            return theme.danger
        case .start:
            return theme.text4
        case .progress:
            return theme.accent
        }
    }
}

private extension BackgroundTaskSnapshot.Status {
    var label: String {
        switch self {
        case .pending: return String(localized: "chat_plan_state_pending")
        case .running: return String(localized: "chat_status_running")
        case .paused: return String(localized: "settings_status_paused")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        }
    }
}

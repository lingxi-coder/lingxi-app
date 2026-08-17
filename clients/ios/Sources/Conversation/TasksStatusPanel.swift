// Flat workflow steps pinned above the composer.

import SwiftUI

struct TasksStatusPanel: View {
    @Environment(\.theme) private var theme
    let tasks: [BackgroundTaskSnapshot]
    let onResume: (String) -> Void
    let showsContainer: Bool
    let workflowResumeState: WorkflowResumeState

    private static let maxPanelHeight: CGFloat = 240

    enum WorkflowStepState: String, Equatable, Hashable {
        case pending
        case running
        case completed
        case failed
        case paused
        case cancelled

        var label: String {
            switch self {
            case .pending: return String(localized: "chat_plan_state_pending")
            case .running: return String(localized: "chat_status_running")
            case .completed: return String(localized: "chat_status_completed")
            case .failed: return String(localized: "chat_status_failed")
            case .paused: return String(localized: "settings_status_paused")
            case .cancelled: return String(localized: "chat_status_cancelled")
            }
        }
    }

    struct WorkflowStep: Identifiable, Equatable, Hashable {
        let id: String
        let title: String
        let state: WorkflowStepState
        let canResume: Bool
    }

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

    static func workflowTaskIsSuccessfullyComplete(_ task: BackgroundTaskSnapshot) -> Bool {
        task.workflow != nil && task.status == .completed
    }

    static func shouldShowWorkflow(
        _ tasks: [BackgroundTaskSnapshot],
        resumeState: WorkflowResumeState = .idle
    ) -> Bool {
        let hasVisibleTask = tasks.contains {
            $0.workflow != nil && !workflowTaskIsSuccessfullyComplete($0)
        }
        if hasVisibleTask { return true }
        if case let .failed(taskID, _) = resumeState {
            return tasks.contains { $0.id == taskID && $0.workflow != nil }
        }
        return false
    }

    static func visibleWorkflowTasks(
        _ tasks: [BackgroundTaskSnapshot],
        resumeState: WorkflowResumeState = .idle
    ) -> [BackgroundTaskSnapshot] {
        tasks.filter { task in
            guard task.workflow != nil else { return false }
            if !workflowTaskIsSuccessfullyComplete(task) { return true }
            if case let .failed(taskID, _) = resumeState, taskID == task.id { return true }
            return false
        }
    }

    static func workflowSteps(
        for task: BackgroundTaskSnapshot,
        resumeState _: WorkflowResumeState = .idle,
        nowMs: UInt64 = currentWallClockMs()
    ) -> [WorkflowStep] {
        guard let workflow = task.workflow else { return [] }
        let phases = workflow.sortedPhases
        let activeAgent = workflow.sortedAgents.first { !$0.state.isTerminal }
        let activePosition = activeAgent.flatMap { agent in
            phases.firstIndex { phaseMatches($0, agent: agent) }
        } ?? phases.firstIndex {
            $0.title == workflow.currentPhaseTitle
        } ?? phases.indices.last
        let terminalPosition = activePosition ?? phases.indices.last ?? 0

        if phases.isEmpty {
            let title = task.descriptionText.isEmpty
                ? (workflow.currentPhaseTitle ?? task.id)
                : task.descriptionText
            return [WorkflowStep(
                id: "workflow-\(task.id)-fallback",
                title: title,
                state: fallbackState(for: task, workflow: workflow),
                canResume: task.canResume && task.status == .paused
            )]
        }

        let result = phases.enumerated().map { position, phase in
            let agents = workflow.sortedAgents.filter { phaseMatches(phase, agent: $0) }
            let state = state(
                for: task,
                phasePosition: position,
                terminalPosition: terminalPosition,
                phaseAgents: agents
            )
            return WorkflowStep(
                id: "workflow-\(task.id)-\(phase.id)",
                title: phase.title,
                state: state,
                canResume: false
            )
        }

        guard task.canResume, task.status == .paused else {
            _ = nowMs
            return result
        }
        guard let pausedIndex = result.lastIndex(where: { $0.state == .paused }) else {
            _ = nowMs
            return result
        }
        return result.enumerated().map { index, step in
            guard index == pausedIndex else { return step }
            return WorkflowStep(id: step.id, title: step.title, state: step.state, canResume: true)
        }
    }

    private static func phaseMatches(
        _ phase: ConversationWorkflowPhaseSnapshot,
        agent: ConversationWorkflowAgentSnapshot
    ) -> Bool {
        let matchesIndex = phase.index != nil && agent.phaseIndex == phase.index
        let matchesTitle = !matchesIndex
            && agent.phaseIndex == nil
            && agent.phaseTitle == phase.title
        return matchesIndex || matchesTitle
    }

    private static func state(
        for task: BackgroundTaskSnapshot,
        phasePosition: Int,
        terminalPosition: Int,
        phaseAgents: [ConversationWorkflowAgentSnapshot]
    ) -> WorkflowStepState {
        if task.status == .completed { return .completed }
        if phasePosition == terminalPosition {
            switch task.status {
            case .paused: return .paused
            case .failed: return .failed
            case .cancelled: return .cancelled
            case .pending, .running, .completed: break
            }
        }

        if phaseAgents.contains(where: { $0.state == .error }) { return .failed }
        if phaseAgents.contains(where: { $0.state == .progress }) { return .running }
        if phaseAgents.contains(where: { $0.state == .start }) { return .pending }
        if !phaseAgents.isEmpty && phaseAgents.allSatisfy({ $0.state.isSuccessLike }) {
            return .completed
        }

        switch task.status {
        case .pending:
            return .pending
        case .running:
            if phasePosition < terminalPosition { return .completed }
            return phasePosition == terminalPosition ? .running : .pending
        case .paused:
            return phasePosition < terminalPosition ? .completed : phasePosition == terminalPosition ? .paused : .pending
        case .completed:
            return .completed
        case .failed:
            return phasePosition < terminalPosition ? .completed : phasePosition == terminalPosition ? .failed : .pending
        case .cancelled:
            return phasePosition < terminalPosition ? .completed : phasePosition == terminalPosition ? .cancelled : .pending
        }
    }

    private static func fallbackState(
        for task: BackgroundTaskSnapshot,
        workflow: ConversationWorkflowRunSnapshot
    ) -> WorkflowStepState {
        switch task.status {
        case .paused: return .paused
        case .completed: return .completed
        case .failed: return .failed
        case .cancelled: return .cancelled
        case .pending, .running: break
        }
        if workflow.failedAgents > 0 { return .failed }
        if workflow.runningAgents > 0 { return .running }
        if workflow.queuedAgents > 0 { return .pending }
        switch task.status {
        case .pending: return .pending
        case .running: return .running
        case .paused: return .paused
        case .completed: return .completed
        case .failed: return .failed
        case .cancelled: return .cancelled
        }
    }

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

    private var visibleTasks: [BackgroundTaskSnapshot] {
        Self.visibleWorkflowTasks(tasks, resumeState: workflowResumeState)
    }

    @ViewBuilder
    private var content: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.caption)
                    .foregroundStyle(theme.text3)
                Text("chat_workflow_title")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.text2)
                Spacer(minLength: 0)
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(visibleTasks.enumerated()), id: \.element.id) { taskIndex, task in
                        if taskIndex > 0 {
                            Divider()
                                .overlay(theme.border.opacity(0.7))
                                .padding(.vertical, 6)
                        }
                        workflowRows(for: task, showContext: visibleTasks.count > 1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: Self.maxPanelHeight)
            .scrollBounceBehavior(.basedOnSize)
        }
        .accessibilityIdentifier("chat.tasks-panel")
    }

    var body: some View {
        if visibleTasks.isEmpty {
            EmptyView()
        } else if showsContainer {
            content
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
        } else {
            content
        }
    }

    @ViewBuilder
    private func workflowRows(
        for task: BackgroundTaskSnapshot,
        showContext: Bool
    ) -> some View {
        if showContext {
            Text(task.descriptionText.isEmpty ? task.id : task.descriptionText)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.text3)
                .lineLimit(1)
                .padding(.bottom, 3)
        }
        ForEach(Self.workflowSteps(for: task, resumeState: workflowResumeState)) { step in
            workflowStepRow(step, task: task)
        }
        if case let .failed(taskID, message) = workflowResumeState, taskID == task.id {
            Text(message)
                .font(.caption2)
                .foregroundStyle(theme.danger)
                .lineLimit(3)
                .padding(.leading, 22)
                .padding(.top, 3)
        }
    }

    private func workflowStepRow(
        _ step: WorkflowStep,
        task: BackgroundTaskSnapshot
    ) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            statusIcon(step.state)
                .frame(width: 14)
            Text(step.title)
                .font(.caption)
                .fontWeight(step.state == .running ? .semibold : .regular)
                .foregroundStyle(step.state == .completed ? theme.text4 : theme.text)
                .strikethrough(step.state == .completed)
                .lineLimit(2)
            Spacer(minLength: 4)
            Text(step.state.label)
                .font(.caption2)
                .foregroundStyle(color(for: step.state))
                .lineLimit(1)
            if step.canResume {
                Button {
                    onResume(task.id)
                } label: {
                    Image(systemName: "play.fill")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(theme.accent)
                        .frame(width: 24, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(isResuming(task.id))
                .accessibilityLabel(String(localized: "chat_workflow_resume"))
            }
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(step.title), \(step.state.label)")
    }

    private func isResuming(_ taskID: String) -> Bool {
        if case let .resuming(activeTaskID) = workflowResumeState {
            return activeTaskID == taskID
        }
        return false
    }

    @ViewBuilder
    private func statusIcon(_ state: WorkflowStepState) -> some View {
        switch state {
        case .running:
            ProgressView().controlSize(.mini)
        case .pending:
            Image(systemName: "clock")
                .font(.caption2)
                .foregroundStyle(theme.text3)
        case .completed:
            Image(systemName: "checkmark")
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.ok)
        case .failed:
            Image(systemName: "xmark")
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.danger)
        case .paused:
            Image(systemName: "pause.circle")
                .font(.caption2)
                .foregroundStyle(theme.text3)
        case .cancelled:
            Image(systemName: "minus.circle")
                .font(.caption2)
                .foregroundStyle(theme.text4)
        }
    }

    private func color(for state: WorkflowStepState) -> Color {
        switch state {
        case .running: return theme.accent
        case .completed: return theme.ok
        case .failed: return theme.danger
        case .pending, .paused: return theme.text3
        case .cancelled: return theme.text4
        }
    }

    // Compatibility projections retained for reducer/display tests that still
    // validate the wire-to-model workflow grouping independently of this flat UI.
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

        _ = nowMs
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
        UInt64(Date().timeIntervalSince1970 * 1000)
    }

    static func formatDuration(_ durationMs: UInt64) -> String {
        let seconds = durationMs / 1000
        let minutes = seconds / 60
        let remaining = seconds % 60
        if minutes == 0 { return "\(remaining)s" }
        return "\(minutes)m \(String(format: "%02d", remaining))s"
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

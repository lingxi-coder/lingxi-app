import SwiftUI

/// The execution footer is intentionally a flat projection of three independent
/// sources of work. The projection owns visibility so ChatView never leaves an
/// empty padded shell behind when a source reaches a successful terminal state.
struct ExecutionStatusPanel: View {
    enum Group: String, CaseIterable, Hashable, Identifiable {
        case agents
        case workflow
        case todos

        var id: String { rawValue }
    }

    @Environment(\.theme) private var theme

    let agents: [ConversationAgentSummary]
    @Binding var selectedAgentID: String?
    let latestActivity: String?
    let isReadOnly: Bool
    let tasks: [BackgroundTaskSnapshot]
    let planTasks: [ConversationPlanTask]
    let workflowResumeState: WorkflowResumeState
    let onSelectAgent: (String?) -> Void
    let onResumeWorkflow: (String) -> Void

    static func visibleGroups(
        agents: [ConversationAgentSummary],
        tasks: [BackgroundTaskSnapshot],
        todos: [ConversationPlanTask],
        workflowResumeState: WorkflowResumeState = .idle,
        selectedAgentID: String? = "main"
    ) -> [Group] {
        var groups: [Group] = []
        if shouldShowAgents(
            agents,
            selectedAgentID: selectedAgentID
        ) {
            groups.append(.agents)
        }
        if TasksStatusPanel.shouldShowWorkflow(tasks, resumeState: workflowResumeState) {
            groups.append(.workflow)
        }
        if todos.contains(where: { $0.state != .completed }) {
            groups.append(.todos)
        }
        return groups
    }

    static func shouldShowAgents(
        _ agents: [ConversationAgentSummary],
        selectedAgentID: String? = "main"
    ) -> Bool {
        let children = agents.filter { $0.id != ConversationModel.mainAgentID }
        guard !children.isEmpty else { return false }

        let allChildrenCompleted = children.allSatisfy {
            AgentStatusPresentation(rawValue: $0.status) == .completed
        }
        guard !allChildrenCompleted else {
            // Keep the selector alive while a child transcript is open so the
            // user can always return to the main conversation.
            return selectedAgentID != nil && selectedAgentID != ConversationModel.mainAgentID
        }
        return true
    }

    private var groups: [Group] {
        Self.visibleGroups(
            agents: agents,
            tasks: tasks,
            todos: planTasks,
            workflowResumeState: workflowResumeState,
            selectedAgentID: selectedAgentID
        )
    }

    var body: some View {
        if groups.count == 1, let group = groups.first {
            groupContent(group)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
        } else if !groups.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(groups.enumerated()), id: \.element) { index, group in
                    if index > 0 {
                        Divider()
                            .overlay(theme.border)
                            .padding(.vertical, 8)
                    }
                    groupContent(group)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(theme.surface)
                    .overlay(
                        RoundedRectangle(cornerRadius: 12, style: .continuous)
                            .stroke(theme.border, lineWidth: 1)
                    )
            )
        } else {
            EmptyView()
        }
    }

    @ViewBuilder
    private func groupContent(_ group: Group) -> some View {
        switch group {
        case .agents:
            AgentStatusDock(
                agents: agents,
                selectedAgentID: $selectedAgentID,
                latestActivity: latestActivity,
                isReadOnly: isReadOnly,
                onSelect: onSelectAgent
            )
        case .workflow:
            TasksStatusPanel(
                tasks: tasks,
                onResume: onResumeWorkflow,
                showsContainer: false,
                workflowResumeState: workflowResumeState
            )
        case .todos:
            PlanTasksPanel(tasks: planTasks, showsContainer: false)
        }
    }

    init(
        agents: [ConversationAgentSummary],
        selectedAgentID: Binding<String?>,
        latestActivity: String?,
        isReadOnly: Bool,
        tasks: [BackgroundTaskSnapshot],
        planTasks: [ConversationPlanTask],
        workflowResumeState: WorkflowResumeState = .idle,
        onSelectAgent: @escaping (String?) -> Void,
        onResumeWorkflow: @escaping (String) -> Void
    ) {
        self.agents = agents
        _selectedAgentID = selectedAgentID
        self.latestActivity = latestActivity
        self.isReadOnly = isReadOnly
        self.tasks = tasks
        self.planTasks = planTasks
        self.workflowResumeState = workflowResumeState
        self.onSelectAgent = onSelectAgent
        self.onResumeWorkflow = onResumeWorkflow
    }
}

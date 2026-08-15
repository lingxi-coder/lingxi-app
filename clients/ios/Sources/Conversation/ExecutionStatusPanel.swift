import SwiftUI

/// One execution surface for the conversation. Agents, durable workflows,
/// background tasks, and the model plan share one disclosure card while each
/// reducer keeps its own stable row identity and interaction semantics.
struct ExecutionStatusPanel: View {
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

    @State private var collapsed = false

    private var hasChildAgents: Bool {
        agents.contains { $0.id != ConversationModel.mainAgentID }
    }

    private var attentionKey: String {
        tasks
            .filter { $0.status == .paused || $0.status == .failed }
            .map { "\($0.id):\($0.status)" }
            .sorted()
            .joined(separator: "|")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { collapsed.toggle() }
            } label: {
                HStack(spacing: 7) {
                    Image(systemName: tasks.contains(where: { !$0.status.isTerminal })
                        ? "arrow.triangle.2.circlepath"
                        : "checklist")
                        .font(.caption)
                        .foregroundStyle(theme.text3)
                    Text(String(localized: "chat_execution_status"))
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(theme.text)
                    Text(summary)
                        .font(.caption2)
                        .foregroundStyle(theme.text3)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Image(systemName: collapsed ? "chevron.down" : "chevron.up")
                        .font(.caption2)
                        .foregroundStyle(theme.text4)
                }
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("chat.execution-status.toggle")

            if !collapsed {
                if hasChildAgents {
                    sectionLabel("chat_execution_agents")
                    AgentStatusDock(
                        agents: agents,
                        selectedAgentID: $selectedAgentID,
                        latestActivity: latestActivity,
                        isReadOnly: isReadOnly,
                        showsDetails: false,
                        onSelect: onSelectAgent
                    ) { EmptyView() }
                }
                if !tasks.isEmpty {
                    sectionLabel("chat_execution_tasks")
                    TasksStatusPanel(
                        tasks: tasks,
                        onResume: onResumeWorkflow,
                        showsContainer: false,
                        workflowResumeState: workflowResumeState
                    )
                }
                if !planTasks.isEmpty {
                    sectionLabel("chat_execution_plan")
                    PlanTasksPanel(tasks: planTasks, showsContainer: false)
                }
            }
        }
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
        .accessibilityIdentifier("chat.execution-status")
        .onChange(of: attentionKey) { _, _ in
            withAnimation(.easeInOut(duration: 0.15)) { collapsed = false }
        }
    }

    private var summary: String {
        let active = tasks.filter { !$0.status.isTerminal }.count
        let agentsCount = max(0, agents.count - 1)
        return "\(agentsCount) · \(active) · \(planTasks.count)"
    }

    private func sectionLabel(_ key: String) -> some View {
        Text(LocalizedStringKey(key))
            .font(.caption2.weight(.semibold))
            .foregroundStyle(theme.text3)
            .padding(.top, 2)
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
        self._selectedAgentID = selectedAgentID
        self.latestActivity = latestActivity
        self.isReadOnly = isReadOnly
        self.tasks = tasks
        self.planTasks = planTasks
        self.workflowResumeState = workflowResumeState
        self.onSelectAgent = onSelectAgent
        self.onResumeWorkflow = onResumeWorkflow
    }
}

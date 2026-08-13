import SwiftUI

/// A compact, always-visible status surface for the currently selected agent.
///
/// The dock deliberately keeps its collapsed footprint to a single row so the
/// transcript remains the primary surface.  The agent picker and execution
/// details are presented only on demand.  `ConversationModel` owns the
/// selection; this view is intentionally a pure projection and can therefore
/// be reused by previews and the mock source.
struct AgentStatusDock<Details: View>: View {
    @Environment(\.theme) private var t

    let agents: [ConversationAgentSummary]
    @Binding var selectedAgentID: String?
    let latestActivity: String?
    let isReadOnly: Bool
    let showsDetails: Bool
    let onSelect: (String?) -> Void
    let details: () -> Details

    @State private var isExpanded = false
    @State private var showingPicker = false

    init(
        agents: [ConversationAgentSummary],
        selectedAgentID: Binding<String?>,
        latestActivity: String? = nil,
        isReadOnly: Bool = false,
        showsDetails: Bool = true,
        onSelect: @escaping (String?) -> Void,
        @ViewBuilder details: @escaping () -> Details
    ) {
        self.agents = agents
        self._selectedAgentID = selectedAgentID
        self.latestActivity = latestActivity
        self.isReadOnly = isReadOnly
        self.showsDetails = showsDetails
        self.onSelect = onSelect
        self.details = details
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Button {
                    showingPicker = true
                } label: {
                    HStack(spacing: 8) {
                        Circle()
                            .fill(statusColor)
                            .frame(width: 8, height: 8)
                        VStack(alignment: .leading, spacing: 1) {
                            HStack(spacing: 5) {
                                Text(selectedName)
                                    .font(.system(size: 12.5, weight: .semibold))
                                    .foregroundStyle(t.text)
                                    .lineLimit(1)
                                if isReadOnly {
                                    Text(String(localized: "chat_agent_read_only"))
                                        .font(.system(size: 10, weight: .medium))
                                        .foregroundStyle(t.text3)
                                        .padding(.horizontal, 5)
                                        .padding(.vertical, 2)
                                        .background(t.windowBg.opacity(0.55))
                                        .clipShape(Capsule())
                                }
                            }
                            Text(displayActivity)
                                .font(.system(size: 11.5))
                                .foregroundStyle(t.text3)
                                .lineLimit(1)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "chat_agent_picker_label"))
                .accessibilityValue(selectedName)
                .accessibilityIdentifier("conversation.agent-picker")

                Spacer(minLength: 4)

                if showsDetails {
                    Button {
                        withAnimation(.easeInOut(duration: 0.16)) { isExpanded.toggle() }
                    } label: {
                        Image(systemName: isExpanded ? "chevron.down" : "chevron.up")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(t.text3)
                            .frame(width: 36, height: 36)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(isExpanded
                        ? String(localized: "chat_agent_details_collapse")
                        : String(localized: "chat_agent_details_expand"))
                    .accessibilityIdentifier("conversation.agent-status.toggle")
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 5)

            if showsDetails, isExpanded {
                details()
                    .frame(maxHeight: 240)
                    .transition(.opacity.combined(with: .move(edge: .bottom)))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(t.surface.opacity(0.82))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(t.border.opacity(0.8), lineWidth: 0.5)
        )
        .sheet(isPresented: $showingPicker) {
            AgentPickerSheet(
                agents: agents,
                selectedAgentID: selectedAgentID,
                onSelect: { id in
                    onSelect(id)
                    showingPicker = false
                }
            )
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
        }
    }

    private var selectedAgent: ConversationAgentSummary? {
        let id = selectedAgentID ?? ConversationModel.mainAgentID
        return agents.first(where: { $0.id == id })
    }

    private var selectedName: String {
        selectedAgent?.name ?? String(localized: "chat_agent_main")
    }

    private var displayActivity: String {
        if let latestActivity, !latestActivity.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return latestActivity
        }
        if let activity = selectedAgent?.latestActivity,
           !activity.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return activity
        }
        return selectedAgent.map { AgentStatusPresentation(rawValue: $0.status).label }
            ?? String(localized: "chat_agent_status_idle")
    }

    private var statusColor: Color {
        guard let selectedAgent else { return t.accent }
        return AgentStatusPresentation(rawValue: selectedAgent.status).color(using: t)
    }
}

/// Agent list shown from the compact dock.  The main agent is always first,
/// followed by running agents and then historical agents in their source order.
private struct AgentPickerSheet: View {
    @Environment(\.theme) private var t

    let agents: [ConversationAgentSummary]
    let selectedAgentID: String?
    let onSelect: (String?) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section(String(localized: "chat_agent_group_current")) {
                    agentRow(
                        nil,
                        name: mainAgent?.name ?? String(localized: "chat_agent_main"),
                        type: mainAgent?.agentType,
                        status: mainAgent?.status ?? "idle",
                        activity: mainAgent?.latestActivity
                    )
                }

                if !runningAgents.isEmpty {
                    Section(String(localized: "chat_agent_group_running")) {
                        ForEach(runningAgents) { agent in row(for: agent) }
                    }
                }

                if !historicalAgents.isEmpty {
                    Section(String(localized: "chat_agent_group_history")) {
                        ForEach(historicalAgents) { agent in row(for: agent) }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle(String(localized: "chat_agent_picker_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(String(localized: "common_done")) { dismiss() }
                }
            }
        }
    }

    private var runningAgents: [ConversationAgentSummary] {
        agents.filter { $0.id != ConversationModel.mainAgentID && isRunning($0) }
    }

    private var historicalAgents: [ConversationAgentSummary] {
        agents.filter { $0.id != ConversationModel.mainAgentID && !isRunning($0) }
    }

    private var mainAgent: ConversationAgentSummary? {
        agents.first { $0.id == ConversationModel.mainAgentID }
    }

    private func row(for agent: ConversationAgentSummary) -> some View {
        agentRow(
            agent.id,
            name: agent.name,
            type: agent.agentType,
            status: agent.status,
            activity: agent.latestActivity
        )
    }

    private func agentRow(
        _ id: String?,
        name: String,
        type: String?,
        status: String,
        activity: String?
    ) -> some View {
        Button {
            onSelect(id)
        } label: {
            HStack(spacing: 10) {
                Circle()
                    .fill(color(for: status))
                    .frame(width: 8, height: 8)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(name)
                            .font(.system(size: 14, weight: .medium))
                            .foregroundStyle(t.text)
                        if let type, !type.isEmpty {
                            Text(type)
                                .font(.system(size: 11))
                                .foregroundStyle(t.text4)
                        }
                    }
                    Text(activityText(activity, fallback: AgentStatusPresentation(rawValue: status).label))
                        .font(.system(size: 11.5))
                        .foregroundStyle(t.text3)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                if selectedAgentID == id {
                    Image(systemName: "checkmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(t.accent)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(minHeight: 44)
        .accessibilityIdentifier("conversation.agent-row." + (id ?? "main"))
    }

    private func isRunning(_ agent: ConversationAgentSummary) -> Bool {
        AgentStatusPresentation(rawValue: agent.status) == .running
    }

    private func color(for status: String) -> Color {
        AgentStatusPresentation(rawValue: status).color(using: t)
    }

    private func activityText(_ activity: String?, fallback: String) -> String {
        guard let activity else { return fallback }
        let value = activity.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? fallback : value
    }
}

enum AgentStatusPresentation: Equatable {
    case running
    case idle
    case completed
    case failed
    case cancelled
    case killed

    init(rawValue: String) {
        let value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if value.contains("kill") || value.contains("terminated") || value.contains("dead") {
            self = .killed
        } else if value.contains("fail") || value.contains("error") {
            self = .failed
        } else if value.contains("cancel") || value.contains("stop") {
            self = .cancelled
        } else if value.contains("complete") || value.contains("done") || value.contains("success") {
            self = .completed
        } else if value.contains("run") || value.contains("active") || value.contains("work") || value.contains("pending") {
            self = .running
        } else {
            self = .idle
        }
    }

    var label: String {
        switch self {
        case .running: return String(localized: "chat_status_running")
        case .idle: return String(localized: "chat_agent_status_idle")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .killed: return String(localized: "chat_agent_status_killed")
        }
    }

    func color(using theme: Palette) -> Color {
        switch self {
        case .running: return theme.accent
        case .idle: return theme.text3
        case .completed: return theme.ok
        case .failed: return theme.danger
        case .cancelled, .killed: return theme.statusTesting
        }
    }
}

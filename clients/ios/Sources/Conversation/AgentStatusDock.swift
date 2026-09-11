import SwiftUI

private extension ConversationAgentSummary {
    var modelDisplayLabel: String? {
        guard let model = model?.trimmingCharacters(in: .whitespacesAndNewlines),
              !model.isEmpty else { return nil }
        let reference = modelProfile
            .flatMap { $0.isEmpty ? nil : "\($0)/\(model)" }
            ?? model
        let item = ModelDisplay.item(for: reference)
        guard item.providerId != "other" else { return item.shortName }
        return "\(ModelDisplay.providerName(for: item.providerId)) · \(item.shortName)"
    }
}

/// An always-expanded, inline agent list. Selection stays in the same view so
/// switching transcripts never opens a second-level picker or sheet.
struct AgentStatusDock: View {
    @Environment(\.theme) private var theme

    private static let maximumVisibleRows = 5
    private static let maxListHeight: CGFloat = 220

    let agents: [ConversationAgentSummary]
    @Binding var selectedAgentID: String?
    let latestActivity: String?
    let isReadOnly: Bool
    let onSelect: (String?) -> Void

    private var orderedAgents: [ConversationAgentSummary] {
        let main = agents.filter { $0.id == ConversationModel.mainAgentID }
        let children = agents
            .filter { $0.id != ConversationModel.mainAgentID }
            .sorted { lhs, rhs in
                let lhsRunning = AgentStatusPresentation(rawValue: lhs.status) == .running
                let rhsRunning = AgentStatusPresentation(rawValue: rhs.status) == .running
                if lhsRunning != rhsRunning { return lhsRunning }
                if lhs.updatedAtMs != rhs.updatedAtMs { return lhs.updatedAtMs > rhs.updatedAtMs }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
        return main + children
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "person.2")
                    .font(.caption)
                    .foregroundStyle(theme.text3)
                Text("chat_agent_picker_title")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.text2)
                Spacer(minLength: 0)
            }
            .padding(.bottom, 3)
            .accessibilityIdentifier("conversation.agent-status")

            boundedAgentRows
        }
    }

    @ViewBuilder
    private var boundedAgentRows: some View {
        if orderedAgents.count > Self.maximumVisibleRows {
            ScrollView {
                agentRows
            }
            .frame(maxHeight: Self.maxListHeight)
            .scrollBounceBehavior(.basedOnSize)
        } else {
            agentRows
        }
    }

    private var agentRows: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(orderedAgents.enumerated()), id: \.element.id) { index, agent in
                if index > 0 {
                    Divider()
                        .overlay(theme.border.opacity(0.7))
                }
                agentRow(agent)
            }
        }
    }

    private func agentRow(_ agent: ConversationAgentSummary) -> some View {
        let isMain = agent.id == ConversationModel.mainAgentID
        let selectionID: String? = isMain ? nil : agent.id
        let activity = isMain ? (latestActivity ?? agent.latestActivity) : agent.latestActivity

        return Button {
            onSelect(selectionID)
        } label: {
            HStack(spacing: 9) {
                AgentAvatar(agentID: agent.id, size: 30)
                    .overlay(alignment: .bottomTrailing) {
                        Circle()
                            .fill(AgentStatusPresentation(rawValue: agent.status).color(using: theme))
                            .frame(width: 8, height: 8)
                            .overlay(Circle().stroke(theme.windowBg, lineWidth: 1.5))
                    }
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(agent.name)
                            .font(.system(size: 13.5, weight: .medium))
                            .foregroundStyle(theme.text)
                            .lineLimit(1)
                        if !agent.agentType.isEmpty {
                            Text(agent.agentType)
                                .font(.system(size: 10.5))
                                .foregroundStyle(theme.text4)
                                .lineLimit(1)
                        }
                        if isReadOnly && selectionID == selectedAgentID {
                            Text(String(localized: "chat_agent_read_only"))
                                .font(.system(size: 9.5, weight: .medium))
                                .foregroundStyle(theme.text3)
                                .padding(.horizontal, 4)
                                .padding(.vertical, 1)
                                .background(theme.windowBg.opacity(0.55))
                                .clipShape(Capsule())
                        }
                    }
                    Text(activityText(
                        activity,
                        fallback: AgentStatusPresentation(rawValue: agent.status).label,
                        model: agent.modelDisplayLabel
                    ))
                    .font(.system(size: 11))
                    .foregroundStyle(theme.text3)
                    .lineLimit(1)
                }
                Spacer(minLength: 4)
                if selectedAgentID == selectionID {
                    Image(systemName: "checkmark")
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(theme.accent)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(minHeight: 40)
        .accessibilityIdentifier("conversation.agent-row." + (isMain ? "main" : agent.id))
    }

    private func activityText(_ activity: String?, fallback: String, model: String?) -> String {
        let value = activity?.trimmingCharacters(in: .whitespacesAndNewlines)
        let status = value.flatMap { $0.isEmpty ? nil : $0 } ?? fallback
        guard let model else { return status }
        return "\(status) · \(model)"
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

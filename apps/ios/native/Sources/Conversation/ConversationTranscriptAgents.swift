import SwiftUI

/// Compact transcript disclosure rows matching Desktop's inline subagent
/// status treatment. These rows remain in the transcript after completion and
/// open the child transcript in a detail sheet.
struct ConversationTranscriptAgents: View {
    @Environment(\.theme) private var t
    @FocusState private var focusedAgentID: String?

    let agents: [ConversationAgentSummary]
    let activeAgentID: String
    let onSelect: (String) -> Void

    private var childAgents: [ConversationAgentSummary] {
        agents.filter { $0.id != ConversationModel.mainAgentID }
    }

    static func rowIdentifier(for agentID: String) -> String {
        "conversation.agent-row.\(agentID)"
    }

    static func projectedRowIDs(for agents: [ConversationAgentSummary]) -> [String] {
        agents.filter { $0.id != ConversationModel.mainAgentID }.map { rowIdentifier(for: $0.id) }
    }

    var body: some View {
        if !childAgents.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                ForEach(childAgents) { agent in
                    let isFocused = focusedAgentID == agent.id
                    // Selecting a child opens its transcript sheet, so the
                    // selected row is the expanded state of this disclosure.
                    let isExpanded = activeAgentID == agent.id
                    Button { onSelect(agent.id) } label: {
                        HStack(spacing: 6) {
                            AgentAvatar(agentID: agent.id, size: 18)
                            Text(agent.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? agent.agentType : agent.name)
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundStyle(isFocused || isExpanded ? t.text : t.text2)
                                .lineLimit(1)
                                .runtimeTextSweep(
                                    isActive: AgentStatusPresentation(rawValue: agent.status) == .running,
                                    highlightColor: t.text
                                )
                            Text(statusText(agent.status))
                                .font(.system(size: 10.5))
                                .foregroundStyle(statusColor(agent.status))
                                .lineLimit(1)
                            if let activity = agent.latestActivity?.trimmingCharacters(in: .whitespacesAndNewlines), !activity.isEmpty {
                                Text(activity)
                                    .font(.system(size: 10.5))
                                    .foregroundStyle(t.text3)
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 0)
                            Image(systemName: "chevron.right")
                                .font(.system(size: 9, weight: .semibold))
                                .foregroundStyle(t.accent)
                                .opacity(isFocused || isExpanded ? 1 : 0)
                        }
                        .frame(minHeight: 28)
                        .padding(.horizontal, 6)
                        .background {
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(t.accent.opacity(isFocused || isExpanded ? 0.12 : 0))
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .focused($focusedAgentID, equals: agent.id)
                    .accessibilityIdentifier(Self.rowIdentifier(for: agent.id))
                    .accessibilityLabel(accessibilityLabel(for: agent))
                    .accessibilityValue(isExpanded ? String(localized: "chat_agent_details_collapse") : "")
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("conversation.transcript-agents")
        }
    }

    private func statusText(_ status: String) -> String {
        switch AgentStatusPresentation(rawValue: status) {
        case .running: return "Running"
        case .completed: return "Done"
        case .failed: return "Failed"
        case .cancelled: return "Cancelled"
        case .killed: return "Killed"
        case .idle: return "Idle"
        }
    }

    private func statusColor(_ status: String) -> Color {
        AgentStatusPresentation(rawValue: status) == .failed ? t.danger : t.text3
    }

    private func accessibilityLabel(for agent: ConversationAgentSummary) -> String {
        var values = [agent.name, statusText(agent.status)]
        if let activity = agent.latestActivity?.trimmingCharacters(in: .whitespacesAndNewlines), !activity.isEmpty {
            values.append(activity)
        }
        return values.joined(separator: " · ")
    }
}

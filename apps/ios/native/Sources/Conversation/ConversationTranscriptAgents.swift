import SwiftUI

/// Compact transcript disclosure rows matching Desktop's inline subagent
/// status treatment. These rows remain in the transcript after completion and
/// open the child transcript in a detail sheet.
struct ConversationTranscriptAgents: View {
    @Environment(\.theme) private var t
    @FocusState private var focusedAgentID: String?
    var groupExpanded: Bool = false
    var onToggleGroup: () -> Void = {}

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

    private func name(_ agent: ConversationAgentSummary) -> String {
        agent.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? agent.agentType : agent.name
    }

    private func displayStatus(_ agent: ConversationAgentSummary) -> String {
        if AgentStatusPresentation(rawValue: agent.status) == .running { return String(localized: "chat_agent_started_working") }
        if ["withheld", "flagged", "pending"].contains(agent.reportDelivery ?? "") { return agent.reportStatusText ?? statusText(agent.status) }
        if agent.status == "completed", agent.reportDelivery == "delivered" { return String(localized: "chat_agent_finished") }
        if agent.status == "completed", agent.reportDelivery == nil { return agent.reportStatusText ?? statusText(agent.status) }
        return statusText(agent.status)
    }

    private func needsAttention(_ agent: ConversationAgentSummary) -> Bool {
        agent.reportDelivery == "withheld" || AgentStatusPresentation(rawValue: agent.status) == .failed
    }

    private func summary(_ agents: [ConversationAgentSummary]) -> Text {
        let states = Dictionary(grouping: agents, by: displayStatus).sorted {
            let left = $0.value.contains(where: needsAttention)
            let right = $1.value.contains(where: needsAttention)
            return left != right ? left : $0.key < $1.key
        }
        return states.enumerated().reduce(Text("")) { text, item in
            let (index, state) = item
            let prefix = index == 0 ? "" : " · "
            let count = states.count == 1 ? "" : "\(state.value.count) "
            return text + Text(prefix) + Text(count + state.key)
                .foregroundColor(state.value.contains(where: needsAttention) ? t.danger : t.text3)
        }
    }

    var body: some View {
        if !childAgents.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                if childAgents.count > 1 {
                    Button(action: onToggleGroup) {
                        HStack(spacing: 9) {
                            HStack(spacing: 2) {
                                ForEach(Array(childAgents.prefix(3))) { agent in
                                    AgentAvatar(agentID: agent.id, size: 18)
                                }
                                if childAgents.count > 3 {
                                    Text("+\(childAgents.count - 3)").font(.caption2)
                                }
                            }
                            Text(ListFormatter.localizedString(byJoining: childAgents.map(name)))
                                .lineLimit(1).truncationMode(.tail)
                            summary(childAgents).lineLimit(1).layoutPriority(1)
                            Image(systemName: groupExpanded ? "chevron.down" : "chevron.right")
                                .font(.caption2.weight(.semibold))
                        }
                        .font(.caption).foregroundStyle(t.text3)
                        .frame(minHeight: 44).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("conversation.agent-group")
                    .accessibilityValue(groupExpanded ? String(localized: "chat_agent_details_collapse") : String(localized: "chat_agent_details_expand"))
                }
                if childAgents.count == 1 || groupExpanded {
                    ForEach(childAgents) { agent in
                        Button { onSelect(agent.id) } label: {
                            HStack(spacing: 9) {
                                AgentAvatar(agentID: agent.id, size: 18)
                                Text(name(agent)).lineLimit(1).truncationMode(.tail)
                                Text(displayStatus(agent))
                                    .foregroundStyle(agent.reportDelivery == "withheld" ? t.danger : statusColor(agent.status))
                                    .lineLimit(1).layoutPriority(1)
                                Image(systemName: "chevron.right")
                                    .font(.caption2.weight(.semibold)).foregroundStyle(t.text3)
                            }
                            .font(.caption).foregroundStyle(activeAgentID == agent.id ? t.text : t.text3)
                            .frame(minHeight: 44).contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .focused($focusedAgentID, equals: agent.id)
                        .accessibilityIdentifier(Self.rowIdentifier(for: agent.id))
                        .accessibilityLabel(accessibilityLabel(for: agent))
                        .padding(.leading, childAgents.count > 1 ? 24 : 0)
                    }
                }
            }
            .padding(.horizontal, 10)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("conversation.transcript-agents")
        }
    }

    private func statusText(_ status: String) -> String {
        switch AgentStatusPresentation(rawValue: status) {
        case .running: return String(localized: "chat_status_running")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        case .killed: return "Killed"
        case .idle: return "Idle"
        }
    }

    private func statusColor(_ status: String) -> Color {
        AgentStatusPresentation(rawValue: status) == .failed ? t.danger : t.text3
    }

    private func accessibilityLabel(for agent: ConversationAgentSummary) -> String {
        var values = [agent.name, displayStatus(agent)]
        if let activity = agent.latestActivity?.trimmingCharacters(in: .whitespacesAndNewlines), !activity.isEmpty, activity != "SubagentHandback" {
            values.append(activity)
        }
        return values.joined(separator: " · ")
    }
}

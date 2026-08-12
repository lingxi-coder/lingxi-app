import SwiftUI

struct ConversationExecutionRunCard: View {
    @Environment(\.theme) private var t
    // Pinned status must not consume the conversation viewport by default.
    // The header still exposes liveness/status and expands on demand.
    @State private var collapsed = true
    let run: ConversationExecutionRun
    /// Which tool rows are expanded, keyed by tool-use id. Owned by
    /// `ConversationModel` — never by the row, which is recycled on scroll.
    var expandedToolCalls: Set<String> = []
    var onToggleToolCall: (String) -> Void = { _ in }
    var onOpenShellTask: ((ConversationShellLaunchRequest) -> Void)? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { collapsed.toggle() }
            } label: {
                HStack(spacing: 8) {
                    LXIcon(name: .workflow, size: 14, color: statusColor, stroke: 1.8)
                    Text("chat_agent_run")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(statusColor)
                    StatusChip(text: run.status.label, accent: statusColor)
                    Spacer(minLength: 6)
                    if let cost = run.costFormatted, !cost.isEmpty {
                        Text(cost)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(t.text3)
                    }
                    Image(systemName: collapsed ? "chevron.down" : "chevron.up")
                        .font(.caption2)
                        .foregroundStyle(t.text4)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("conversation.agent-run.toggle")
            .accessibilityLabel(collapsed
                ? String(localized: "chat_run_expand")
                : String(localized: "chat_run_collapse"))

            if !collapsed {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 12) {
                        if !run.reasoning.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            blockPanel(icon: .brain, title: String(localized: "chat_thinking"), body: run.reasoning)
                        }

                        if !run.notices.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                ForEach(run.notices) { notice in
                                    HStack(alignment: .top, spacing: 8) {
                                        Circle()
                                            .fill(color(for: notice.kind))
                                            .frame(width: 6, height: 6)
                                            .padding(.top, 6)
                                        Text(notice.text)
                                            .font(.system(size: 12.5))
                                            .foregroundColor(t.text2)
                                            .fixedSize(horizontal: false, vertical: true)
                                    }
                                }
                            }
                        }

                        if let retry = run.retry {
                            HStack(spacing: 8) {
                                LXIcon(name: .clock, size: 12, color: t.text3, stroke: 1.6)
                                Text(String(localized: "chat_retry_attempt \(retry.message) \(retry.attempt) \(retry.maxRetries) \(retry.delayMs)"))
                                    .font(.system(size: 12.5))
                                    .foregroundColor(t.text2)
                            }
                        }

                        if !run.tools.isEmpty {
                            // One renderer for every tool row, live or restored: it consumes
                            // the engine's derived header/display and falls back to the old
                            // summaries only when an older engine ships neither.
                            VStack(alignment: .leading, spacing: 8) {
                                ForEach(run.tools) { tool in
                                    ToolCallView(
                                        trace: tool,
                                        isExpanded: expandedToolCalls.contains(tool.id),
                                        onToggle: { onToggleToolCall(tool.id) }
                                    )
                                }
                            }
                        }

                        if !run.shellCards.isEmpty {
                            VStack(alignment: .leading, spacing: 10) {
                                ForEach(run.shellCards) { card in
                                    ConversationShellCardView(card: card, onOpenInTerminal: onOpenShellTask)
                                }
                            }
                        }

                        if let usage = run.usage {
                            HStack(spacing: 10) {
                                UsageChip(label: String(localized: "chat_usage_input"), value: "\(usage.inputTokens)")
                                UsageChip(label: String(localized: "chat_usage_output"), value: "\(usage.outputTokens)")
                                if usage.cacheReadTokens > 0 {
                                    UsageChip(label: String(localized: "chat_usage_cache_read"), value: "\(usage.cacheReadTokens)")
                                }
                                if usage.cacheCreationTokens > 0 {
                                    UsageChip(label: String(localized: "chat_usage_cache_write"), value: "\(usage.cacheCreationTokens)")
                                }
                            }
                        }

                        if !run.compactions.isEmpty {
                            VStack(alignment: .leading, spacing: 5) {
                                ForEach(run.compactions) { item in
                                    Text(String(localized: "chat_compaction \(item.messagesBefore) \(item.messagesAfter) \(item.bytesSaved)"))
                                        .font(.system(size: 12))
                                        .foregroundColor(t.text3)
                                }
                            }
                        }

                        if run.activeWorkers > 0 || !run.workers.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                Text(run.coordinatorTeam.map { String(localized: "chat_team_active_agents \($0) \(run.activeWorkers)") } ?? String(localized: "chat_active_agents \(run.activeWorkers)"))
                                    .font(.system(size: 12.5, weight: .medium))
                                    .foregroundColor(t.text2)
                                ForEach(run.workers) { worker in
                                    HStack(spacing: 8) {
                                        Text(worker.name)
                                            .font(.system(size: 12.5, weight: .medium))
                                            .foregroundColor(t.text)
                                        Text(worker.agentType)
                                            .font(.system(size: 11.5))
                                            .foregroundColor(t.text4)
                                        Spacer(minLength: 4)
                                        Text(worker.status)
                                            .font(.system(size: 11.5))
                                            .foregroundColor(t.text3)
                                    }
                                }
                            }
                        }
                    }
                }
                .frame(maxHeight: 360)
                .scrollIndicators(.visible)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(t.surface.opacity(0.72))
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(statusColor.opacity(0.35), lineWidth: 0.5))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conversation.agent-run")
    }

    private func blockPanel(icon: LXIconName, title: String, body: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                LXIcon(name: icon, size: 12, color: t.text3, stroke: 1.7)
                Text(title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(t.text3)
            }
            Text(body)
                .font(.system(size: 12.5))
                .foregroundColor(t.text2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(10)
        .background(t.windowBg.opacity(0.55))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }

    private var statusColor: Color {
        switch run.status.tone {
        case .running: return t.accent
        case .completed: return t.ok
        case .failed: return t.danger
        case .cancelled: return t.statusTesting
        case .maxTurns: return t.accent2
        case .restored: return t.text3
        }
    }

    private func color(for kind: ConversationExecutionNotice.Kind) -> Color {
        switch kind {
        case .info: return t.accent
        case .warning: return t.text3
        case .error: return t.danger
        }
    }
}

private struct ConversationShellCardView: View {
    @Environment(\.theme) private var t
    let card: ConversationShellCard
    let onOpenInTerminal: ((ConversationShellLaunchRequest) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("Shell")
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundColor(t.text)
                StatusChip(text: card.status.label, accent: statusColor)
                if let duration = ConversationExecutionParsing.formatDuration(card.durationMs) {
                    Text(duration)
                        .font(.system(size: 11.5))
                        .foregroundColor(t.text4)
                }
                Spacer(minLength: 6)
                if let exit = card.exitCode {
                    Text("exit \(exit)")
                        .font(.system(size: 11.5))
                        .foregroundColor(t.text3)
                }
            }

            if !card.command.isEmpty {
                mono(card.command)
            }
            if let cwd = card.cwd, !cwd.isEmpty {
                Text(cwd)
                    .font(.system(size: 11.5))
                    .foregroundColor(t.text4)
            }
            if let onOpenInTerminal, !card.command.isEmpty {
                Button("chat_open_in_terminal") {
                    onOpenInTerminal(
                        ConversationShellLaunchRequest(
                            taskId: card.taskId,
                            command: card.command,
                            cwd: card.requestedCwd
                        )
                    )
                }
                .font(.system(size: 12.5, weight: .medium))
                .buttonStyle(.plain)
                .foregroundColor(t.accent)
                .accessibilityIdentifier("conversation.shell.open-terminal")
            }
            if !card.stdout.isEmpty {
                labeledOutput("stdout", card.stdout)
            }
            if !card.stderr.isEmpty {
                labeledOutput("stderr", card.stderr)
            }
            if card.truncated {
                Text("chat_output_truncated")
                    .font(.system(size: 11.5))
                    .foregroundColor(t.text3)
            }
        }
        .padding(10)
        .background(t.windowBg.opacity(0.55))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
        .accessibilityIdentifier("conversation.shell.\(card.taskId)")
    }

    private func labeledOutput(_ label: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundColor(t.text4)
            mono(text)
        }
    }

    private func mono(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12, design: .monospaced))
            .foregroundColor(t.text2)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var statusColor: Color {
        switch card.status {
        case .running: return t.accent
        case .completed: return t.ok
        case .failed, .timedOut: return t.danger
        case .cancelled: return t.text3
        }
    }
}

private struct StatusChip: View {
    @Environment(\.theme) private var t
    let text: String
    let accent: Color

    var body: some View {
        Text(text)
            .font(.system(size: 11.5, weight: .medium))
            .foregroundColor(accent)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(accent.opacity(0.12))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(accent.opacity(0.25), lineWidth: 0.5))
    }
}

private struct UsageChip: View {
    @Environment(\.theme) private var t
    let label: String
    let value: String

    var body: some View {
        Text("\(label) \(value)")
            .font(.system(size: 11.5))
            .foregroundColor(t.text3)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(t.windowBg.opacity(0.55))
            .clipShape(Capsule())
    }
}

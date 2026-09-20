import SwiftUI

/// A focused read-only view of the current session's execution context.
///
/// The conversation model already owns the structured execution trace, so this
/// screen deliberately derives its sections from that one source of truth. It
/// stays useful for an empty session too: the overview still identifies the
/// workspace and model, while the execution sections explain that there is no
/// data yet.
struct SessionDetailsView: View {
    @Environment(\.theme) private var t

    let session: SessionRef
    let workspacePath: String
    let onOpenTerminal: () -> Void

    @State private var selectedTask: SessionTaskRow?
    @ObservedObject private var convo: ConversationModel

    init(
        session: SessionRef,
        source: any ConversationSource,
        workspacePath: String,
        onOpenTerminal: @escaping () -> Void
    ) {
        self.session = session
        self.workspacePath = workspacePath
        self.onOpenTerminal = onOpenTerminal
        self.convo = source.model
    }

    private var runs: [ConversationExecutionRun] {
        convo.items.compactMap { item in
            guard case let .run(run) = item else { return nil }
            return run
        }
    }

    private var workers: [ConversationCoordinatorWorker] {
        var seen = Set<String>()
        return runs.flatMap(\.workers).filter { seen.insert($0.id).inserted }
    }

    private var tasks: [SessionTaskRow] {
        runs.flatMap { run in
            let toolRows = run.tools.map { tool in
                SessionTaskRow(
                    id: "tool:\(run.id):\(tool.id)",
                    title: tool.tool,
                    detail: [tool.header.map(ToolDisplayText.title), tool.inputSummary,
                             tool.display?.body ?? tool.outputSummary]
                        .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "\n\n"),
                    status: tool.status.label,
                    accent: taskColor(tool.status)
                )
            }
            let shellRows = run.shellCards.map { shell in
                SessionTaskRow(
                    id: "shell:\(run.id):\(shell.taskId)",
                    title: shell.command.isEmpty ? "Shell" : shell.command,
                    detail: shell.cwd,
                    status: shell.status.label,
                    accent: shellColor(shell.status)
                )
            }
            return toolRows + shellRows
        }
    }

    private var planEntries: [PlanDocument] {
        runs.flatMap { $0.tools.compactMap(\.planDocument) } + convo.messages.flatMap { message -> [PlanDocument] in
            guard message.role != .user else { return [] }
            if let detail = convo.messageDetails[message.id], !detail.blocks.isEmpty {
                return detail.blocks.flatMap { block -> [PlanDocument] in
                    switch block {
                    case let .text(text):
                        return PlanDocument.segments(text).compactMap { if case let .plan(plan) = $0 { return plan }; return nil }
                    case let .toolUse(_, tool, _, input, _):
                        return PlanDocument.tool(tool, json: input).map { [$0] } ?? []
                    default: return []
                    }
                }
            }
            return PlanDocument.segments(message.text).compactMap { if case let .plan(plan) = $0 { return plan }; return nil }
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                overviewCard
                executionSection(
                    title: String(localized: "session_details_agents"),
                    subtitle: String(localized: "session_details_agents_subtitle"),
                    icon: "person.2.fill"
                ) {
                    if workers.isEmpty {
                        emptyRow(
                            icon: "person.2",
                            title: String(localized: "session_details_no_agents")
                        )
                    } else {
                        ForEach(workers) { worker in
                            workerRow(worker)
                        }
                    }
                }
                executionSection(
                    title: String(localized: "session_details_tasks"),
                    subtitle: String(localized: "session_details_tasks_subtitle"),
                    icon: "checklist"
                ) {
                    if tasks.isEmpty {
                        emptyRow(
                            icon: "checklist",
                            title: String(localized: "session_details_no_tasks")
                        )
                    } else {
                        ForEach(tasks) { task in
                            Button { selectedTask = task } label: { taskRow(task) }
                                .buttonStyle(.plain)
                                .accessibilityHint("Open task details")
                        }
                    }
                }
                executionSection(
                    title: String(localized: "session_details_plan"),
                    subtitle: String(localized: "session_details_plan_subtitle"),
                    icon: "list.bullet.clipboard"
                ) {
                    if planEntries.isEmpty {
                        emptyRow(
                            icon: "list.bullet.clipboard",
                            title: String(localized: "session_details_no_plan")
                        )
                    } else {
                        ForEach(Array(planEntries.enumerated()), id: \.offset) { index, entry in
                            PlanDocumentCard(document: entry)
                        }
                    }
                }
            }
            .padding(.horizontal, 18)
            .padding(.top, 16)
            .padding(.bottom, 28)
        }
        .scrollIndicators(.hidden)
        .background {
            t.windowBg.ignoresSafeArea()
        }
        .navigationTitle("session_details_title")
        .sheet(item: $selectedTask) { task in
            NavigationStack {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        Label(task.status, systemImage: "circle.fill")
                            .foregroundStyle(task.accent)
                        Text(task.title).font(.headline)
                        if let detail = task.detail { Text(detail).font(.system(.body, design: .monospaced)) }
                    }
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                }
                .background(t.windowBg)
                .navigationTitle("session_details_tasks")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_close") { selectedTask = nil }
                    }
                }
            }
            .environment(\.theme, t)
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
            .accessibilityIdentifier("conversation.task-detail-sheet")
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: onOpenTerminal) {
                    Label("settings_linux_section_terminal", systemImage: "terminal")
                }
                .foregroundStyle(t.accent)
                .accessibilityIdentifier("session-details.open-terminal")
            }
        }
        .accessibilityIdentifier("session-details.root")
    }

    private var overviewCard: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 12) {
                ZStack {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .fill(LinearGradient(
                            colors: [t.accent, t.accent2],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        ))
                    Image(systemName: "waveform.path.ecg")
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundStyle(.white)
                }
                .frame(width: 52, height: 52)

                VStack(alignment: .leading, spacing: 5) {
                    Text(session.title)
                        .font(.system(size: 20, weight: .bold, design: .rounded))
                        .foregroundStyle(t.text)
                        .lineLimit(2)
                    Text(session.id)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(t.text4)
                        .lineLimit(1)
                        .textSelection(.enabled)
                }
                Spacer(minLength: 0)
            }

            Divider().overlay(t.border)

            VStack(alignment: .leading, spacing: 10) {
                detailLine(icon: "folder.fill", label: String(localized: "session_details_workspace"), value: workspacePath)
                detailLine(icon: "cpu", label: String(localized: "session_details_model"), value: modelName)
                detailLine(icon: "bubble.left.and.bubble.right.fill", label: String(localized: "session_details_messages"), value: "\(convo.messages.count)")
            }

            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 10) {
                metric(value: "\(runs.count)", label: String(localized: "session_details_runs"), color: t.accent)
                metric(value: "\(workers.count)", label: String(localized: "session_details_agent_count"), color: t.accent2)
                metric(value: "\(tasks.count)", label: String(localized: "session_details_task_count"), color: t.accent3)
                metric(value: statusLabel, label: String(localized: "session_details_status"), color: statusColor)
            }
        }
        .padding(16)
        .background(t.surface, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 22, style: .continuous)
                .stroke(t.borderStrong.opacity(0.7), lineWidth: 1)
        }
    }

    private var modelName: String {
        convo.activeModelId.isEmpty ? convo.model.name : convo.activeModelId
    }

    private var statusLabel: String {
        if convo.streaming { return String(localized: "chat_status_running") }
        return runs.last?.status.label ?? String(localized: "session_details_status_ready")
    }

    private var statusColor: Color {
        if convo.streaming { return t.accent }
        switch runs.last?.status {
        case .completed: return t.ok
        case .failed: return t.danger
        default: return t.text3
        }
    }

    private func detailLine(icon: String, label: String, value: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(t.text3)
                .frame(width: 18)
            Text(label)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(t.text3)
            Spacer(minLength: 8)
            Text(value)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(t.text2)
                .multilineTextAlignment(.trailing)
                .lineLimit(2)
                .textSelection(.enabled)
        }
    }

    private func metric(value: String, label: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(value)
                .font(.system(size: 18, weight: .bold, design: .rounded))
                .foregroundStyle(color)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(label)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(t.text4)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(11)
        .background(t.windowBg.opacity(0.72), in: RoundedRectangle(cornerRadius: 13, style: .continuous))
    }

    private func executionSection<Content: View>(
        title: String,
        subtitle: String,
        icon: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(t.accent)
                    .frame(width: 30, height: 30)
                    .background(t.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 16, weight: .bold, design: .rounded))
                        .foregroundStyle(t.text)
                    Text(subtitle)
                        .font(.system(size: 11.5))
                        .foregroundStyle(t.text4)
                }
            }
            VStack(alignment: .leading, spacing: 8, content: content)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(t.surface.opacity(0.72), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .stroke(t.border, lineWidth: 0.7)
                }
        }
    }

    private func workerRow(_ worker: ConversationCoordinatorWorker) -> some View {
        HStack(spacing: 10) {
            AgentAvatar(agentID: worker.id, size: 32)
            VStack(alignment: .leading, spacing: 2) {
                Text(worker.name)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(t.text)
                Text(worker.agentType)
                    .font(.system(size: 11))
                    .foregroundStyle(t.text4)
            }
            Spacer()
            Text(worker.status)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(t.text3)
        }
    }

    private func taskRow(_ task: SessionTaskRow) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Circle()
                .fill(task.accent)
                .frame(width: 8, height: 8)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 3) {
                Text(task.title)
                    .font(.system(size: 12.5, weight: .semibold, design: .monospaced))
                    .foregroundStyle(t.text)
                    .lineLimit(2)
                if let detail = task.detail, !detail.isEmpty {
                    Text(detail)
                        .font(.system(size: 11))
                        .foregroundStyle(t.text4)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 4)
            Text(task.status)
                .font(.system(size: 10.5, weight: .medium))
                .foregroundStyle(task.accent)
        }
    }

    private func emptyRow(icon: String, title: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 14))
                .foregroundStyle(t.text4)
            Text(title)
                .font(.system(size: 12.5))
                .foregroundStyle(t.text4)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 4)
    }

    private func taskColor(_ status: ConversationToolStatus) -> Color {
        switch status {
        case .running: return t.accent
        case .completed: return t.ok
        case .failed: return t.danger
        case .cancelled, .unknown: return t.text3
        }
    }

    private func shellColor(_ status: ConversationShellStatus) -> Color {
        switch status {
        case .running: return t.accent
        case .completed: return t.ok
        case .failed, .timedOut: return t.danger
        case .cancelled: return t.text3
        }
    }
}

private struct SessionTaskRow: Identifiable {
    let id: String
    let title: String
    let detail: String?
    let status: String
    let accent: Color
}

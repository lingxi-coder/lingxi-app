import SwiftUI
import UIKit

/// The native iOS counterpart to Desktop's Runtime Center. Each case is a
/// real projection of data already published by ConversationModel; ChatView
/// only adds populated cases to its menu.
enum ConversationSummaryCategory: String, Identifiable {
    case changes
    case agents
    case resources
    case plan

    var id: String { rawValue }

    var title: String {
        switch self {
        case .changes: return "Changes"
        case .agents: return "Agents & background tasks"
        case .resources: return "Resources"
        case .plan: return "Plan"
        }
    }

    var systemImage: String {
        switch self {
        case .changes: return "arrow.triangle.2.circlepath"
        case .agents: return "person.2"
        case .resources: return "shippingbox"
        case .plan: return "list.bullet.clipboard"
        }
    }
}

/// A medium/large bottom sheet with one detail route per Runtime Center
/// category. Agent selection dismisses this overview before the parent
/// presents the child transcript sheet.
struct ConversationSummarySheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.theme) private var t
    @State private var selectedResource: Resource?

    let category: ConversationSummaryCategory
    let session: SessionRef
    let selectedAgentID: String
    let onSelectAgent: (String) -> Void
    let onResumeWorkflow: (String) -> Void
    @ObservedObject private var convo: ConversationModel

    init(
        category: ConversationSummaryCategory,
        session: SessionRef,
        source: any ConversationSource,
        selectedAgentID: String,
        onSelectAgent: @escaping (String) -> Void,
        onResumeWorkflow: @escaping (String) -> Void
    ) {
        self.category = category
        self.session = session
        self.selectedAgentID = selectedAgentID
        self.onSelectAgent = onSelectAgent
        self.onResumeWorkflow = onResumeWorkflow
        self.convo = source.model
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    contextHeader
                    categoryContent
                }
                .padding(.horizontal, 18)
                .padding(.top, 14)
                .padding(.bottom, 28)
            }
            .scrollIndicators(.hidden)
            .background(t.windowBg.ignoresSafeArea())
            .navigationTitle(category.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close") { dismiss() }
                }
            }
        }
        .accessibilityIdentifier("conversation.summary-sheet.\(category.rawValue)")
        .tint(t.accent)
        .environment(\.openPlanDocument, nil)
        .sheet(item: $selectedResource) { resource in
            NavigationStack {
                AttachmentPreview(resource: resource)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("common_close") { selectedResource = nil }
                                .accessibilityIdentifier("conversation.resource-detail.close")
                        }
                    }
            }
            .environment(\.theme, t)
            .tint(t.accent)
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
            .accessibilityIdentifier("conversation.resource-detail-sheet")
        }
    }

    private var contextHeader: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(session.title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(t.text)
                .lineLimit(2)
            Text(session.id)
                .font(.system(size: 10.5, design: .monospaced))
                .foregroundStyle(t.text4)
                .lineLimit(1)
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, 2)
    }

    @ViewBuilder
    private var categoryContent: some View {
        switch category {
        case .changes:
            changesContent
        case .agents:
            agentsContent
        case .resources:
            resourcesContent
        case .plan:
            planContent
        }
    }

    private var changesContent: some View {
        let changes = Self.changes(in: convo)
        return VStack(alignment: .leading, spacing: 12) {
            if changes.isEmpty {
                emptyRow("No file changes in this session.", systemImage: "doc.text.magnifyingglass")
            } else {
                ForEach(Array(changes.enumerated()), id: \.offset) { _, change in
                    VStack(alignment: .leading, spacing: 5) {
                        HStack(spacing: 7) {
                            Image(systemName: "doc.text.magnifyingglass")
                                .foregroundStyle(t.accent)
                            Text(change.path)
                                .font(.system(size: 13, weight: .medium, design: .monospaced))
                                .foregroundStyle(t.text)
                                .lineLimit(2)
                            Spacer(minLength: 0)
                            Text("+\(change.diff.additions)  -\(change.diff.removals)")
                                .font(.system(size: 10.5, design: .monospaced))
                                .foregroundStyle(t.text3)
                        }
                        DiffView(diff: change.diff)
                    }
                    .padding(12)
                    .background(t.surface, in: .rect(cornerRadius: 14))
                    .overlay { RoundedRectangle(cornerRadius: 14).stroke(t.border, lineWidth: 0.7) }
                }
            }
        }
    }

    private var agentsContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            let agents = convo.agentSummaries
            ForEach(agents) { agent in
                Button {
                    onSelectAgent(agent.id)
                    dismiss()
                } label: {
                    HStack(spacing: 10) {
                        AgentAvatar(agentID: agent.id, size: 30)
                            .overlay(alignment: .bottomTrailing) {
                                Circle()
                                    .fill(AgentStatusPresentation(rawValue: agent.status).color(using: t))
                                    .frame(width: 8, height: 8)
                                    .overlay(Circle().stroke(t.windowBg, lineWidth: 1.5))
                            }
                        VStack(alignment: .leading, spacing: 2) {
                            Text(agent.name)
                                .font(.system(size: 13.5, weight: .medium))
                                .foregroundStyle(t.text)
                                .lineLimit(1)
                            Text(agent.latestActivity?.nilIfBlank ?? AgentStatusPresentation(rawValue: agent.status).label)
                                .font(.system(size: 11))
                                .foregroundStyle(t.text3)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 4)
                        if selectedAgentID == agent.id {
                            Image(systemName: "checkmark")
                                .font(.system(size: 12.5, weight: .semibold))
                                .foregroundStyle(t.accent)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .frame(minHeight: 42)
                .accessibilityIdentifier("conversation.summary.agent.\(agent.id)")
            }

            if !convo.backgroundTasks.isEmpty {
                Divider().overlay(t.border)
                ForEach(convo.backgroundTasks) { task in
                    backgroundTaskRow(task)
                }
            }
        }
    }

    private func backgroundTaskRow(_ task: BackgroundTaskSnapshot) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: task.status.isTerminal ? "checkmark.circle" : "clock.arrow.circlepath")
                .foregroundStyle(taskColor(task.status))
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 3) {
                Text(task.descriptionText.nilIfBlank ?? task.id)
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundStyle(t.text)
                    .lineLimit(2)
                Text(statusLabel(task.status))
                    .font(.system(size: 11))
                    .foregroundStyle(taskColor(task.status))
                if let error = task.errorText?.nilIfBlank {
                    Text(error)
                        .font(.system(size: 11))
                        .foregroundStyle(t.danger)
                        .lineLimit(3)
                }
                resumeFailure(for: task.id)
            }
            Spacer(minLength: 0)
            if task.canResume {
                Button {
                    onResumeWorkflow(task.id)
                } label: {
                    if isResuming(task.id) {
                        ProgressView().controlSize(.small)
                    } else {
                        Label("Resume", systemImage: "play.fill")
                            .font(.system(size: 11.5, weight: .medium))
                    }
                }
                .buttonStyle(.borderedProminent)
                .tint(t.accent)
                .disabled(isResuming(task.id))
                .accessibilityIdentifier("conversation.summary.resume.\(task.id)")
            }
        }
        .padding(10)
        .background(t.surface, in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.7) }
    }

    private func isResuming(_ taskID: String) -> Bool {
        if case let .resuming(activeTaskID) = convo.workflowResumeState {
            return activeTaskID == taskID
        }
        return false
    }

    @ViewBuilder
    private func resumeFailure(for taskID: String) -> some View {
        if case let .failed(failedTaskID, message) = convo.workflowResumeState,
           failedTaskID == taskID,
           !message.isEmpty {
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(t.danger)
                .lineLimit(3)
        }
    }

    private var resourcesContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            let resources = Self.resources(in: convo)
            if resources.isEmpty {
                emptyRow("No resources in this session.", systemImage: "shippingbox")
            } else {
                ForEach(resources) { resource in
                    Button {
                        selectedResource = resource
                    } label: {
                        resourceRow(resource)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("conversation.summary.resource.\(resource.id)")
                }
            }
        }
    }

    private func resourceRow(_ resource: Resource) -> some View {
        HStack(spacing: 10) {
            Image(systemName: resource.icon)
                .foregroundStyle(t.accent)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(resource.title).font(.system(size: 13.5, weight: .medium)).foregroundStyle(t.text)
                Text(resource.detail).font(.system(size: 11)).foregroundStyle(t.text3)
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.caption2.weight(.semibold))
                .foregroundStyle(t.text4)
        }
        .padding(10)
        .background(t.surface, in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.7) }
    }

    private var planContent: some View {
        VStack(alignment: .leading, spacing: 12) {
            let documents = Self.planDocuments(in: convo)
            if documents.isEmpty {
                emptyRow("No submitted plan in this session.", systemImage: "list.bullet.clipboard")
            } else {
                ForEach(Array(documents.enumerated()), id: \.offset) { _, document in
                    PlanDocumentCard(document: document)
                }
            }
        }
    }

    private func emptyRow(_ title: String, systemImage: String) -> some View {
        HStack(spacing: 9) {
            Image(systemName: systemImage)
                .foregroundStyle(t.text3)
            Text(title)
                .font(.system(size: 12.5))
                .foregroundStyle(t.text3)
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(t.surface, in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.7) }
    }

    private func taskColor(_ status: BackgroundTaskSnapshot.Status) -> Color {
        switch status {
        case .running, .pending: return t.accent
        case .completed: return t.ok
        case .failed: return t.danger
        case .paused: return t.text3
        case .cancelled: return t.text4
        }
    }

    private struct Change: Identifiable {
        let id: String
        let path: String
        let diff: ConversationStructuredDiff
    }

    fileprivate struct Resource: Identifiable, Hashable {
        let id: String
        let icon: String
        let title: String
        let detail: String
        let url: String
    }

    static func hasChanges(_ convo: ConversationModel) -> Bool { !changes(in: convo).isEmpty }

    static func hasAgents(_ convo: ConversationModel) -> Bool {
        convo.agentSummaries.contains { $0.id != ConversationModel.mainAgentID } || !convo.backgroundTasks.isEmpty
    }

    static func hasResources(_ convo: ConversationModel) -> Bool {
        !resources(in: convo).isEmpty
    }

    private static func changes(in convo: ConversationModel) -> [Change] {
        var result: [Change] = []
        var seen = Set<String>()
        for item in convo.selectedAgentItems {
            let traces: [ConversationToolTrace]
            switch item {
            case let .run(run): traces = run.tools
            case let .toolCall(trace): traces = [trace]
            default: traces = []
            }
            for trace in traces {
                guard let diff = trace.display?.diff else { continue }
                let path = diff.filePath?.nilIfBlank ?? trace.header?.primary ?? trace.tool
                let change = Change(id: trace.id, path: path, diff: diff)
                if seen.insert(change.id).inserted { result.append(change) }
            }
        }
        return result
    }

    private static func resources(in convo: ConversationModel) -> [Resource] {
        convo.selectedAgentMessages.enumerated().flatMap { messageIndex, message in
            message.images.enumerated().map { imageIndex, image in
                Resource(
                    id: "message:\(message.id.uuidString):image:\(imageIndex)",
                    icon: image.mediaType.hasPrefix("image/") ? "photo" : "paperclip",
                    title: image.mediaType.hasPrefix("image/") ? "Image attachment" : "File attachment",
                    detail: "\(image.mediaType) · message \(messageIndex + 1)",
                    url: image.url
                )
            }
        }
    }

    static func planDocuments(in convo: ConversationModel) -> [PlanDocument] {
        var documents = convo.selectedAgentItems.compactMap { item -> [PlanDocument]? in
            guard case let .run(run) = item else { return nil }
            return run.tools.compactMap(\.planDocument)
        }.flatMap { $0 }
        let details = convo.selectedAgentID == ConversationModel.mainAgentID
            ? convo.messageDetails
            : convo.selectedAgentMessageDetails
        documents.append(contentsOf: convo.selectedAgentMessages.flatMap { message -> [PlanDocument] in
            guard message.role != .user else { return [] }
            if let detail = details[message.id], !detail.blocks.isEmpty {
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
        })
        return documents
    }

    private func statusLabel(_ status: BackgroundTaskSnapshot.Status) -> String {
        switch status {
        case .pending: return String(localized: "chat_plan_state_pending")
        case .running: return String(localized: "chat_status_running")
        case .paused: return String(localized: "settings_status_paused")
        case .completed: return String(localized: "chat_status_completed")
        case .failed: return String(localized: "chat_status_failed")
        case .cancelled: return String(localized: "chat_status_cancelled")
        }
    }
}

private struct AttachmentPreview: View {
    @Environment(\.theme) private var t
    let resource: ConversationSummarySheet.Resource

    var body: some View {
        Group {
            if let image = decodedImage(resource.url) {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
            } else if resource.icon == "photo", let url = URL(string: resource.url) {
                AsyncImage(url: url) { content in
                    content.resizable().scaledToFit()
                } placeholder: {
                    ProgressView()
                }
            } else {
                ScrollView {
                    Text(resource.url)
                        .font(.system(size: 12, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding()
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(t.windowBg.ignoresSafeArea())
        .navigationTitle(resource.title)
        .navigationBarTitleDisplayMode(.inline)
    }

    private func decodedImage(_ url: String) -> UIImage? {
        guard url.hasPrefix("data:"), let comma = url.firstIndex(of: ",") else { return nil }
        let encoded = String(url[url.index(after: comma)...])
        guard let data = Data(base64Encoded: encoded, options: .ignoreUnknownCharacters) else { return nil }
        return UIImage(data: data)
    }
}

private extension String {
    var nilIfBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

import SwiftUI

/// Resolves identifiers exclusively against the currently loaded local runtime.
struct DeepLinkRuntimeDetail: View {
    let target: CanonicalDeepLink
    let source: any ConversationSource
    @ObservedObject var convo: ConversationModel
    @Environment(\.dismiss) private var dismiss
    private var kind: String { target.parts[1] }
    private var identifier: String { target.parts[2] }
    private var tools: [ConversationToolTrace] {
        convo.items.flatMap { item -> [ConversationToolTrace] in
            if case let .run(run) = item { return run.tools }
            if case let .toolCall(tool) = item { return [tool] }
            return []
        }
    }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(identifier).font(.caption.monospaced()).textSelection(.enabled)
                    detail
                }.padding()
            }
            .navigationTitle(kind.capitalized)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() } } }
        }
    }
    @ViewBuilder private var detail: some View {
        switch kind {
        case "task":
            if let task = convo.backgroundTasks.first(where: { $0.id == identifier }) {
                Text(task.descriptionText).font(.headline)
                Text(String(describing: task.status))
                if let error = task.errorText { Text(error).foregroundStyle(.red) }
                if task.canResumeWorkflow { Button("Resume") { source.resumeWorkflow(task.id) } }
            } else if let task = convo.planTasks.first(where: { $0.id == identifier }) {
                Text(task.subject).font(.headline)
                Text(task.state.label)
            } else { missing }
        case "plan":
            if let document = tools.first(where: { $0.id == identifier })?.planDocument {
                PlanDocumentDetail(document: document)
            } else if let message = convo.messages.first(where: { $0.transcriptUUID?.uuidString.lowercased() == identifier }),
                      let document = PlanDocument.segments(message.text).compactMap({ if case let .plan(document) = $0 { return document }; return nil }).first {
                PlanDocumentDetail(document: document)
            } else { missing }
        case "review":
            if let diff = tools.first(where: { $0.id == identifier })?.display?.diff { DiffView(diff: diff) }
            else { missing }
        case "activity":
            if !identifier.isEmpty && identifier == convo.activeSessionId {
                ConversationTimelineView(groups: ConversationRenderLayout.timelineGroups(convo.items), messageDetails: convo.messageDetails, expandedToolCalls: convo.expandedToolCalls,
                    onToggleToolCall: { convo.toggleTranscriptDisclosure($0) })
                    .accessibilityIdentifier("deeplink.runtime.activity")
            } else { missing }
        default: missing
        }
    }
    private var missing: some View {
        ContentUnavailableView("Local item unavailable", systemImage: "link", description: Text("This identifier is not present in the loaded local conversation."))
    }
}

import SwiftUI

struct InboxNoticePreviewView: View {
    @Environment(InboxStore.self) private var inbox
    @Environment(\.dismiss) private var dismiss
    let preview: InboxNotificationPreview
    var loadAssistant: (() async -> [String])?
    let openConversation: () -> Void

    @State private var message: TeamMessage?
    @State private var assistantText: [String] = []
    @State private var loading = true

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if loading {
                        ProgressView("Loading conversation update")
                    } else if let message {
                        if let name = message.author.displayName, !name.isEmpty {
                            Text(verbatim: name).font(.headline)
                        }
                        if !message.text.isEmpty { Text(verbatim: message.text).textSelection(.enabled) }
                        if message.text.isEmpty && !message.files.isEmpty {
                            Label("Attachment", systemImage: "paperclip")
                        }
                    } else if !assistantText.isEmpty {
                        ForEach(Array(assistantText.enumerated()), id: \.offset) { _, text in
                            Text(verbatim: text).textSelection(.enabled)
                        }
                    } else {
                        Label("This update is unavailable", systemImage: "text.bubble")
                    }
                    Button("Open conversation", action: openConversation)
                        .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
            }
            .navigationTitle(preview.row.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
        }
        .task(id: preview.id) { await load() }
    }

    private func load() async {
        defer { loading = false }
        guard let uid = inbox.uid, await inbox.authorized(preview.row.ref) != nil else { return }
        if preview.row.ref.kind == .assistant {
            let text = await loadAssistant?() ?? []
            guard !Task.isCancelled, inbox.uid == uid, inbox.notificationPreview?.id == preview.id else { return }
            assistantText = text
        } else {
            let message = try? await InboxTransport(uid: uid).noticeMessage(preview)
            guard !Task.isCancelled, inbox.uid == uid, inbox.notificationPreview?.id == preview.id else { return }
            self.message = message
        }
    }
}

import SwiftUI

struct TeamMessagesPage: View {
    let uid: String
    let detail: TeamDetail
    let onClose: () -> Void
    @State private var model = TeamMessagingModel()
    @State private var text = ""
    @State private var title = ""
    @State private var kind = "dm"
    @State private var parties: Set<String> = []
    @State private var creationID = UUID().uuidString
    @State private var editing: TeamMessage?
    @State private var reply: TeamMessage?
    @State private var recall: TeamMessage?
    @State private var operationError = ""
    @State private var busy = false
    @State private var shown = 100

    var body: some View {
        List {
            Section {
                Text("teams_disclosure").font(.footnote).foregroundStyle(.secondary)
                Text(LocalizedStringKey(model.offline ? "teams_messages_offline" : "teams_messages_connected")).font(.footnote).foregroundStyle(.secondary)
                if !model.error.isEmpty || !operationError.isEmpty { Text(LocalizedStringKey(errorKey(operationError.isEmpty ? model.error : operationError))).font(.caption).foregroundStyle(.red) }
            }
            if !model.revoked {
                if let creation = model.cache?.creation {
                    Section { Text(creation.title); Text(LocalizedStringKey(creation.error.map(errorKey) ?? (creation.attempted == true ? "teams_message_confirming" : "teams_message_pending"))).font(.caption); Button("teams_message_discard") { perform { try await model.discardCreation() } }.disabled((creation.attempted == true && creation.error == nil) || (creation.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000) }
                }
                conversations
                if let conversation = model.cache?.conversations.first(where: { $0.id == model.selected }) {
                    transcript(conversation)
                    composer
                }
                creation
            }
        }
        .navigationTitle(Text("teams_conversations"))
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("teams_cancel", action: onClose) }
            ToolbarItem(placement: .primaryAction) { Button("teams_refresh", systemImage: "arrow.clockwise") { model.refresh() } }
        }
        .task { model.start(uid: uid, team: detail.team) }
        .onDisappear { model.stop() }
        .onChange(of: model.selected) { _, _ in editing = nil; reply = nil; text = ""; shown = 100 }
        .confirmationDialog(Text("teams_recall_confirm"), isPresented: Binding(get: { recall != nil }, set: { if !$0 { recall = nil } }), titleVisibility: .visible) {
            Button("teams_message_recall", role: .destructive) { if let recall { perform { try await model.queue(operation: "recall", text: "", messageId: recall.id) } }; recall = nil }
            Button("teams_cancel", role: .cancel) { recall = nil }
        }
    }
    private var conversations: some View {
        Section("teams_conversations") {
            ForEach(model.cache?.conversations ?? []) { conversation in
                Button { model.select(conversation.id) } label: {
                    HStack { Text(conversation.title); Spacer(); if conversation.unread > 0 { Text(conversation.unread, format: .number).font(.caption).monospacedDigit() }; if model.selected == conversation.id { Image(systemName: "checkmark") } }
                }
            }
        }
    }
    private func transcript(_ conversation: TeamMessageConversation) -> some View {
        let messages = model.cache?.threads[conversation.id]?.messages ?? []
        return Section {
            Button("teams_mark_read") { perform { try await model.markRead() } }.disabled(model.offline || busy)
            if messages.count > shown { Button("teams_messages_earlier") { shown += 100 } }
            ForEach(Array(messages.suffix(shown))) { message in
                VStack(alignment: .leading, spacing: 8) {
                    HStack { Text(name(message.author.uid)).font(.caption.bold()); Spacer(); if let date = message.createdAt.flatMap({ Self.timestamp.date(from: $0) }) { Text(date, style: .time).font(.caption).foregroundStyle(.secondary) } }
                    if let replyID = message.replyTo {
                        let source = messages.first { $0.id == replyID }
                        Text(source?.recalled == true ? String(localized: "teams_message_recalled") : source?.text ?? String(localized: "teams_message_reply")).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                    Text(message.recalled == true ? String(localized: "teams_message_recalled") : message.text).textSelection(.enabled)
                    if message.editedAt != nil { Text("teams_message_edited").font(.caption2).foregroundStyle(.secondary) }
                    if message.recalled != true {
                        HStack {
                            Button("teams_message_reply") { reply = message; editing = nil }
                            if message.author.uid == uid && message.author.kind == "user" {
                                Button("teams_message_edit") { editing = message; reply = nil; text = message.text }
                                Button("teams_message_recall", role: .destructive) { recall = message }
                            }
                        }.font(.caption).buttonStyle(.borderless)
                    }
                }.padding(.vertical, 8).accessibilityIdentifier("teams.message.\(message.id)")
            }
            ForEach((model.cache?.outbox ?? []).filter { $0.conversationId == conversation.id }) { intent in
                VStack(alignment: .leading) {
                    Text(LocalizedStringKey(intent.error.map(errorKey) ?? (intent.attempted == true ? "teams_message_confirming" : "teams_message_pending"))).font(.caption).foregroundStyle(.secondary)
                    Text(intent.operation == "recall" ? String(localized: "teams_message_recall") : intent.text)
                    Button("teams_message_discard", role: .destructive) { perform { try await model.discard(intent.id) } }.disabled((intent.attempted == true && intent.error == nil) || (intent.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000)
                }
            }
        } header: { Text(conversation.title) }
    }
    private var composer: some View {
        Section {
            if editing != nil || reply != nil {
                HStack { Text(LocalizedStringKey(editing != nil ? "teams_message_edit" : "teams_message_reply")); Spacer(); Button("teams_cancel") { editing = nil; reply = nil; text = "" } }
            }
            TextField("teams_message_input", text: $text, axis: .vertical).lineLimit(3...8).disabled(busy)
            Button(LocalizedStringKey(editing != nil ? "teams_message_save" : "teams_message_send")) {
                let submitted = text, target = editing?.id, replyID = reply?.id
                perform { try await model.queue(operation: target == nil ? "create" : "edit", text: submitted, messageId: target, replyTo: replyID); if text == submitted { text = "" }; if editing?.id == target { editing = nil }; if reply?.id == replyID { reply = nil } }
            }.disabled(busy || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || text.utf8.count > 8192)
        }
    }
    private var creation: some View {
        Section {
            DisclosureGroup("teams_conversation_new") {
                TextField("teams_conversation_title", text: $title)
                Picker("teams_conversation_type", selection: $kind) { Text("teams_conversation_dm").tag("dm"); Text("teams_conversation_group").tag("group"); Text("teams_conversation_channel").tag("channel") }
                ForEach(detail.members.filter { $0.uid != uid }) { member in
                    Toggle(member.displayName, isOn: Binding(get: { parties.contains(member.uid) }, set: { value in if kind == "dm" { parties = value ? [member.uid] : [] } else if value { parties.insert(member.uid) } else { parties.remove(member.uid) } }))
                }
                Button("teams_conversation_create") {
                    perform { try await model.create(kind: kind, title: title.trimmingCharacters(in: .whitespacesAndNewlines), uids: parties.sorted(), id: creationID); creationID = UUID().uuidString; title = ""; parties = [] }
                }.disabled(busy || model.offline || title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || title.count > 120 || parties.isEmpty || (kind == "dm" && parties.count != 1))
            }
        }.onChange(of: kind) { _, _ in parties = [] }
    }
    @MainActor private static let timestamp: ISO8601DateFormatter = { let value = ISO8601DateFormatter(); value.formatOptions = [.withInternetDateTime, .withFractionalSeconds]; return value }()
    private func errorKey(_ code: String) -> String {
        if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED"].contains(code) { return "teams_stale_error" }
        if code.contains("STORAGE") { return "teams_messages_storage_error" }
        if ["MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "INVALID_INPUT"].contains(code) { return "teams_message_failed" }
        return "teams_error"
    }
    private func name(_ uid: String) -> String { detail.members.first { $0.uid == uid }?.displayName ?? String(localized: "teams_member") }
    private func perform(_ action: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }; busy = true; operationError = ""
        Task { defer { busy = false }; do { try await action() } catch { operationError = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE" } }
    }
}

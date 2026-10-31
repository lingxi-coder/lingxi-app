import SwiftUI

struct TeamMessagesPage: View {
    let uid: String
    let detail: TeamDetail
    let onClose: () -> Void
    @State private var model: TeamMessagingModel
    @State private var text = ""
    @State private var title = ""
    @State private var kind = "dm"
    @State private var parties: Set<String> = []
    @State private var botParties: Set<String> = []
    @State private var targets: Set<String> = []
    @State private var unbinding: TeamBotCard?
    @State private var creationID = UUID().uuidString
    @State private var editing: TeamMessage?
    @State private var reply: TeamMessage?
    @State private var recall: TeamMessage?
    @State private var operationError = ""
    @State private var busy = false
    @State private var shown = 100

    @MainActor
    init(uid: String, detail: TeamDetail, onClose: @escaping () -> Void, model: TeamMessagingModel? = nil) {
        self.uid = uid
        self.detail = detail
        self.onClose = onClose
        _model = State(initialValue: model ?? TeamMessagingModel())
    }

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
                botCards
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
        .onChange(of: model.selected) { _, _ in editing = nil; reply = nil; text = ""; targets = []; shown = 100 }
        .confirmationDialog("Unbind this Bot?", isPresented: Binding(get: { unbinding != nil }, set: { if !$0 { unbinding = nil } }), titleVisibility: .visible) {
            Button("Unbind Bot", role: .destructive) { if let bot = unbinding { perform { try await model.unbind(bot) } }; unbinding = nil }
            Button("teams_cancel", role: .cancel) { unbinding = nil }
        } message: { Text("New requests and pending execution for this binding will be revoked. Shared conversation history remains visible.") }
        .confirmationDialog(Text("teams_recall_confirm"), isPresented: Binding(get: { recall != nil }, set: { if !$0 { recall = nil } }), titleVisibility: .visible) {
            Button("teams_message_recall", role: .destructive) { if let recall { perform { try await model.queue(operation: "recall", text: "", messageId: recall.id) } }; recall = nil }
            Button("teams_cancel", role: .cancel) { recall = nil }
        }
    }
    private var botCards: some View {
        Section {
            Text("The platform can read shared messages. A Bot's owner can read conversations that include their Bot.").font(.footnote).foregroundStyle(.secondary)
            Text("Bind and configure Bots on LingXi Desktop. Keep the owner's Desktop running to process requests.").font(.footnote).foregroundStyle(.secondary)
            ForEach(model.bots) { bot in
                VStack(alignment: .leading, spacing: 10) {
                    TeamBotCardView(bot: bot, owner: name(bot.ownerUid))
                    if bot.enabled {
                        Button("Chat with Bot") {
                            perform { try await model.create(kind: "bot_dm", title: bot.name, uids: [], botKeys: [bot.key], id: creationID); creationID = UUID().uuidString }
                        }.disabled(busy || model.offline || !bot.selectable || model.cache?.creation != nil)
                    }
                    if bot.ownerUid == uid {
                        Button("Unbind Bot", role: .destructive) { unbinding = bot }.disabled(busy || model.offline || !bot.enabled)
                    }
                }.padding(.vertical, 6).accessibilityIdentifier("teams.bot.\(bot.key)")
            }
        } header: { Text("Team Bots") }
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
        let visible = Array(messages.suffix(shown))
        let messageById = Dictionary(uniqueKeysWithValues: messages.map { ($0.id, $0) })
        let latestByTrigger = visible.reduce(into: [String: String]()) { result, message in
            for trigger in message.botTriggers(in: conversation) { result[trigger] = message.id }
        }
        return Section {
            Button("teams_mark_read") { perform { try await model.markRead() } }.disabled(model.offline || busy)
            if messages.count > shown { Button("teams_messages_earlier") { shown += 100; model.showBotHistory(limit: shown) } }
            ForEach(visible) { message in
                VStack(alignment: .leading, spacing: 8) {
                    HStack { Text(message.author.kind == "bot" ? botName(message.author.botKey) : name(message.author.uid)).font(.caption.bold()); Spacer(); if let date = message.createdAt.flatMap({ Self.timestamp.date(from: $0) }) { Text(date, style: .time).font(.caption).foregroundStyle(.secondary) } }
                    if let replyID = message.replyTo {
                        let source = messageById[replyID]
                        Text(source?.recalled == true ? String(localized: "teams_message_recalled") : source?.text ?? String(localized: "teams_message_reply")).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                    Text(message.recalled == true ? String(localized: "teams_message_recalled") : message.text).textSelection(.enabled)
                    ForEach(message.botTriggers(in: conversation), id: \.self) { trigger in
                        botControls(message, trigger: trigger, latest: latestByTrigger[trigger] == message.id)
                    }
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
            ForEach((model.cache?.botOutbox ?? []).filter { $0.conversationId == conversation.id }) { intent in
                VStack(alignment: .leading, spacing: 8) {
                    Text(intent.action == "botAnswer" ? "Bot answer" : "Bot settlement").font(.caption.bold())
                    Text(LocalizedStringKey(intent.error.map(errorKey) ?? (intent.attempted == true ? "teams_message_confirming" : "teams_message_pending"))).font(.caption)
                    if let answers = intent.answers {
                        ForEach(answers.keys.sorted(), id: \.self) { prompt in Text(prompt + "\n" + (answers[prompt] ?? "")) }
                    }
                    if let text = intent.text { Text(text) }
                    Button("teams_message_discard", role: .destructive) { perform { try await model.discardBotIntent(intent.id) } }
                        .disabled(!intent.canDiscard(now: Date().timeIntervalSince1970 * 1000))
                }
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
            if editing == nil, let conversation = model.cache?.conversations.first(where: { $0.id == model.selected }) {
                if conversation.kind == "bot_dm" {
                    Text("Sending here requests a reply from this Bot.").font(.caption).foregroundStyle(.secondary)
                } else if conversation.kind == "group" || conversation.kind == "channel" {
                    DisclosureGroup("Request replies from Bots") {
                        Text("Select Bots explicitly. Typing @name alone does not start a Bot.").font(.caption).foregroundStyle(.secondary)
                        ForEach(model.bots.filter { $0.selectable && conversation.participants.contains($0.key) }) { bot in
                            Toggle(bot.name, isOn: Binding(get: { targets.contains(bot.key) }, set: { if $0 { targets.insert(bot.key) } else { targets.remove(bot.key) } }))
                        }
                    }
                }
            }
            if !unavailableTargets.isEmpty {
                if selectedConversation?.kind == "bot_dm" {
                    Text("This Bot is unavailable. New requests cannot be sent. You can keep this draft or clear it.").font(.caption).foregroundStyle(.secondary)
                    Button("Clear draft") { text = ""; targets = []; reply = nil }
                } else {
                    Text("A selected Bot is unavailable. Clear that selection before sending.").font(.caption).foregroundStyle(.secondary)
                    Button("Clear unavailable Bot selection") { targets.subtract(unavailableTargets) }
                }
            }
            TextField("teams_message_input", text: $text, axis: .vertical).lineLimit(3...8).disabled(busy)
            Button(LocalizedStringKey(editing != nil ? "teams_message_save" : "teams_message_send")) {
                let submitted = text, target = editing?.id, replyID = reply?.id, submittedTargets = targets
                perform { try await model.queue(operation: target == nil ? "create" : "edit", text: submitted, messageId: target, replyTo: replyID, targets: target == nil ? submittedTargets.sorted() : []); if targets == submittedTargets { targets = [] }; if text == submitted { text = "" }; if editing?.id == target { editing = nil }; if reply?.id == replyID { reply = nil } }
            }.disabled(busy || (editing == nil && !unavailableTargets.isEmpty) || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || text.utf8.count > 8192)
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
                if kind != "dm" {
                    ForEach(model.bots.filter(\.selectable)) { bot in
                        Toggle("Bot: " + bot.name, isOn: Binding(get: { botParties.contains(bot.key) }, set: { if $0 { botParties.insert(bot.key) } else { botParties.remove(bot.key) } }))
                            .disabled(!botParties.contains(bot.key) && botParties.count >= 8)
                    }
                }
                if !unavailableBotParties.isEmpty {
                    Text("A selected Bot is unavailable. Clear that selection before creating this conversation.").font(.caption).foregroundStyle(.secondary)
                    Button("Clear unavailable Bot selection") { botParties.subtract(unavailableBotParties) }
                }
                Button("teams_conversation_create") {
                    perform { try await model.create(kind: kind, title: title.trimmingCharacters(in: .whitespacesAndNewlines), uids: parties.sorted(), botKeys: kind == "dm" ? [] : botParties.sorted(), id: creationID); creationID = UUID().uuidString; title = ""; parties = []; botParties = [] }
                }.disabled(busy || model.offline || !unavailableBotParties.isEmpty || title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || title.count > 120 || (parties.isEmpty && botParties.isEmpty) || (kind == "dm" && parties.count != 1))
            }
        }.onChange(of: kind) { _, _ in parties = []; botParties = [] }
    }
    private var selectedConversation: TeamMessageConversation? { model.cache?.conversations.first { $0.id == model.selected } }
    private var unavailableTargets: Set<String> {
        guard let conversation = selectedConversation else { return targets }
        return TeamBotSelection.unavailable(TeamBotSelection.targets(in: conversation, selected: targets), cards: model.bots, participants: conversation.participants)
    }
    private var unavailableBotParties: Set<String> { TeamBotSelection.unavailable(botParties, cards: model.bots) }
    private func botName(_ key: String?) -> String { model.bots.first { $0.key == key }?.name ?? "Bot" }
    private func botControls(_ message: TeamMessage, trigger: String, latest: Bool) -> some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let now = context.date.timeIntervalSince1970 * 1000
            VStack(alignment: .leading, spacing: 8) {
                if let question = message.question, model.canAnswer(message, now: now) {
                    TeamBotQuestionView(question: question, busy: busy) { answers in perform { try await model.answer(message, answers: answers) } }.id(question.id)
                }
                if latest, let state = model.botStates[trigger] {
                    Text("Bot status: " + statusLabel(state, now: now)).font(.caption).foregroundStyle(.secondary)
                    if model.canRecover(trigger, now: now) {
                        TeamBotRecoveryView(busy: busy) { result in perform { try await model.recover(trigger: trigger, text: result) } }.id(trigger)
                    }
                }
            }
        }
    }
    private func statusLabel(_ state: TeamBotState, now: Double) -> String {
        if state.needsRecovery(now: now) { return "Needs manual settlement by the owner" }
        switch state.state {
        case "queued": return "Waiting for Desktop"
        case "claimed": return "Preparing"
        case "running": return state.answered ? "Running" : (state.questionId == nil ? "Running" : "Waiting for the original requester")
        case "completed", "recovered": return "Completed"
        case "revoked": return "Revoked"
        case "failed": return "Failed"
        case "cancelled": return "Cancelled"
        case "missed": return "Expired while offline"
        default: return "Unavailable"
        }
    }
    @MainActor private static let timestamp: ISO8601DateFormatter = { let value = ISO8601DateFormatter(); value.formatOptions = [.withInternetDateTime, .withFractionalSeconds]; return value }()
    private func errorKey(_ code: String) -> String {
        if ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED"].contains(code) { return "teams_stale_error" }
        if code.contains("STORAGE") { return "teams_messages_storage_error" }
        if ["MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "BOT_USE_REQUIRED", "BOT_PARTICIPANT_REQUIRED", "INVALID_BOT_TARGET", "QUESTION_RECIPIENT_REQUIRED", "ANSWER_CONFLICT", "INVALID_ANSWER", "RECOVERY_NOT_REQUIRED", "BOT_RESULT_CONFLICT", "BOT_REVISION_CONFLICT", "BOT_OWNER_REQUIRED", "MESSAGE_IN_FLIGHT", "INVALID_INPUT"].contains(code) { return "teams_message_failed" }
        return "teams_error"
    }
    private func name(_ uid: String) -> String { detail.members.first { $0.uid == uid }?.displayName ?? String(localized: "teams_member") }
    private func perform(_ action: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }; busy = true; operationError = ""
        Task { defer { busy = false }; do { try await action() } catch { operationError = (error as? TeamMessageFailure)?.code ?? "SERVICE_UNAVAILABLE" } }
    }
}

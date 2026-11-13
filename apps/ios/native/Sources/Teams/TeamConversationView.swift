import SwiftUI

/// An open conversation (spec §6.3): header, banners, the message stream, the composer. A pushed screen on
/// phones (the tab bar leaves with it) and the detail pane on wide layouts. The rules are
/// `TeamsStreamLogic` (rows, grouping, the window), `TeamsComposeLogic` (composer) and `TeamsReadLogic`
/// (when the app marks the conversation read by itself, decision D12).
///
/// Over a workspace pinned to a friendship (`workspace.pair`, docs/FRIENDS-CLIENTS.md D9) it is a friend's chat: the
/// header is the friend, and the details button and the platform notice are left out. Everything else is the same.
struct TeamConversationView: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.dismiss) private var dismiss
    @Environment(\.locale) private var locale
    let conversationID: String
    /// Called when the conversation is gone (the viewer is not in it any more). By default the screen pops itself;
    /// the wide layout's pane clears its selection instead.
    var onGone: (() -> Void)?
    /// The wide layout opens the conversation details as a side pane; by default they are a sheet.
    var onShowDetails: (() -> Void)?

    /// `lastReadSeq` as it was when the conversation opened: the "new messages" divider stays where it was
    /// even though reading moves the mark. `Int.max` = no divider (the mark never arrived).
    @State private var anchorSeq: Int?
    /// The oldest message the reader asked for with "show earlier messages".
    @State private var startID: String?
    @State private var atBottom = true
    /// The newest `seq` seen while at the bottom; later messages by others count for the pill.
    @State private var seenSeq = 0
    @State private var placed = false
    @State private var position = ScrollPosition(edge: .bottom)
    @State private var showDetails = false
    @State private var showNotice = false
    @State private var recalling: TeamMessage?
    @State private var editing: TeamMessage?
    @State private var savedText: String?
    @State private var draft = TeamComposerDraft()
    @State private var draftLoaded = false
    @FocusState private var composerFocused: Bool

    var body: some View {
        Group {
            if let context = TeamConversationContext.make(workspace: workspace, conversationID: conversationID) {
                loaded(context)
            } else {
                missing
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: workspace.pair == nil ? .teams : .friends)
        // The model is replaced when the team or its membership generation changes; the new one must open this thread.
        .task(id: workspace.messaging.map(ObjectIdentifier.init)) { workspace.openConversation(conversationID) }
        .onAppear {
            if !draftLoaded { draft = workspace.draft(for: conversationID); draftLoaded = true }
            showNotice = workspace.pair == nil && !workspace.platformNoticeShown
            workspace.markPlatformNoticeShown()
        }
        .onChange(of: draft) { _, value in
            if draftLoaded, editing == nil { workspace.setDraft(value, for: conversationID) }
        }
        .onDisappear {
            if draftLoaded, editing == nil { workspace.setDraft(draft, for: conversationID) }
        }
        // Without `.contain` the identifier is copied onto every child (composer input, send, rows).
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.conversation.\(conversationID)")
    }

    // MARK: Not there (yet)

    @ViewBuilder
    private var missing: some View {
        let model = workspace.messaging
        if let creation = model?.cache?.creation, creation.conversationId == conversationID {
            VStack(spacing: 12) {
                Text(creation.title).font(.headline)
                Text(LocalizedStringKey(creation.error.map(TeamsErrors.key) ?? (creation.attempted == true ? "teams_message_confirming" : "teams_message_pending")))
                    .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                Button("teams_message_discard", role: .destructive) {
                    Task { if await workspace.run({ try await model?.discardCreation() }) { dismiss() } }
                }
                .disabled((creation.attempted == true && creation.error == nil) || (creation.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000)
            }
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if model?.directoryReady == true && model?.fromCache == false {
            // Gone (the viewer was removed from it, or it was never theirs): back to the list.
            TeamsStateView(systemImage: "bubble.left.and.bubble.right", title: "teams_no_conversations")
                .task { if let onGone { onGone() } else { dismiss() } }
        } else {
            VStack(spacing: 12) {
                ProgressView()
                Text("teams_loading_messages").font(.footnote).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: Loaded

    private func loaded(_ context: TeamConversationContext) -> some View {
        let model = context.model
        let conversation = context.conversation
        let unread = model.unread(of: conversation)
        let messages = model.cache?.threads[conversation.id]?.messages ?? []
        let participants = TeamsListLogic.conversationParticipants(conversation, members: context.members, bots: context.bots, uid: context.uid, strings: context.strings)
        let carrying = carriesFiles(context)
        return VStack(spacing: 0) {
            banners(context)
            stream(context, messages: messages)
            TeamComposerView(
                context: context, draft: $draft, editing: editing, replying: replying(context, messages: messages), busy: workspace.messageBusy,
                files: carrying ? workspace.files(for: conversationID) : [], refusals: carrying ? workspace.refusals(for: conversationID) : [],
                onChooseFiles: carrying ? { workspace.addFiles($0, to: conversationID) } : nil,
                onRemoveFile: { workspace.removeFile($0, from: conversationID) }, onDismissRefusals: { workspace.dismissRefusals(for: conversationID) },
                focus: $composerFocused, onSend: { Task { await send(context) } },
                onCancelEdit: endEdit, onCancelReply: { draft.replyToID = nil })
        }
        .toolbar {
            ToolbarItem(placement: .principal) {
                HStack(spacing: 8) {
                    ConversationLead(conversation: conversation, label: context.label, participants: participants, size: 32)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(context.label).font(.headline).lineLimit(1)
                        if let subtitle = context.subtitle { Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("teams.conversation.header")
            }
            if context.pair == nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { if let onShowDetails { onShowDetails() } else { showDetails = true } } label: { Image(systemName: "info.circle") }
                        .accessibilityLabel(Text("teams_conversation_details"))
                        .accessibilityIdentifier("teams.conversation.info")
                }
            }
            // A friendship's chats read themselves (D12/D13), so they have no manual mark, as on Android.
            if unread > 0, context.pair == nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("teams_mark_read", systemImage: "checkmark.circle") { Task { await workspace.markRead(conversation.id) } }
                            .disabled(model.offline || workspace.messageBusy)
                    } label: { Image(systemName: "ellipsis.circle") }
                    .accessibilityIdentifier("teams.conversation.more")
                }
            }
        }
        .sheet(isPresented: $showDetails) {
            TeamConversationDetailsSheet(conversationID: conversation.id)
        }
        .confirmationDialog(Text("teams_recall_confirm"), isPresented: Binding(get: { recalling != nil }, set: { if !$0 { recalling = nil } }), titleVisibility: .visible, presenting: recalling) { message in
            Button("teams_message_recall", role: .destructive) {
                Task { await workspace.run { try await model.queue(operation: "recall", text: "", messageId: message.id) } }
            }
            Button("teams_cancel", role: .cancel) {}
        } message: { _ in
            Text("teams_recall_confirm_detail")
        }
        // The divider's anchor: the read mark as it was on opening, once the marks are known.
        .task(id: model.readsReady) {
            guard anchorSeq == nil else { return }
            if model.readsReady {
                anchorSeq = conversation.lastReadSeq
            } else {
                // Without a read mark there is no divider; never wait for it forever.
                do { try await Task.sleep(for: .seconds(4)) } catch { return }
                if anchorSeq == nil { anchorSeq = Int.max }
            }
        }
        // Automatic mark-read (D12): the newest message has been in view long enough, online, in the foreground.
        .task(id: AutoReadKey(
            ready: placed && model.readsReady, atBottom: atBottom, newest: messages.last?.seq ?? 0, lastRead: conversation.lastReadSeq,
            offline: model.offline, fromCache: model.fromCache, foreground: scenePhase == .active)) {
            guard placed, model.readsReady, atBottom, let threshold = TeamsReadLogic.autoMarkReadMs else { return }
            do { try await Task.sleep(for: .milliseconds(threshold)) } catch { return }
            if let seq = TeamsReadLogic.autoMarkReadSeq(
                online: !model.offline, fromCache: model.fromCache, foreground: scenePhase == .active, atBottomMs: threshold,
                newestRenderedSeq: messages.last?.seq ?? 0, lastReadSeq: conversation.lastReadSeq) {
                await workspace.autoMarkRead(conversationID: conversation.id, lastReadSeq: seq)
            }
        }
    }

    private struct AutoReadKey: Equatable {
        let ready: Bool
        let atBottom: Bool
        let newest: Int
        let lastRead: Int
        let offline: Bool
        let fromCache: Bool
        let foreground: Bool
    }

    private func banners(_ context: TeamConversationContext) -> some View {
        let model = context.model
        return VStack(spacing: 0) {
            if !model.error.isEmpty {
                TeamsBanner(kind: .error, title: LocalizedStringKey(TeamsErrors.key(model.error)), actionTitle: "mcp_reconnect") { workspace.reconnect() }
            } else if model.directoryReady && model.offline {
                TeamsBanner(kind: .offline, title: "teams_offline_title", detail: "teams_messages_offline", actionTitle: "mcp_reconnect") { workspace.reconnect() }
            }
            if !workspace.actionError.isEmpty, workspace.actionError != model.error {
                TeamsBanner(kind: .error, title: LocalizedStringKey(TeamsErrors.key(workspace.actionError)), actionTitle: "common_close") { workspace.clearActionError() }
            }
        }
    }

    // MARK: Stream

    private struct ScrollMetrics: Equatable {
        let nearBottom: Bool
        let container: CGFloat
    }

    private func stream(_ context: TeamConversationContext, messages: [TeamMessage]) -> some View {
        let conversation = context.conversation
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
        let streamMessages = messages.map(StreamMessage.init)
        let anchor = anchorSeq ?? Int.max
        let start = TeamsStreamLogic.visibleStart(streamMessages, startId: startID, lastReadSeq: anchor, uid: context.uid)
        let rows = TeamsStreamLogic.buildStreamRows(Array(streamMessages[start...]), lastReadSeq: anchor, uid: context.uid, nowMs: nowMs, zone: .current)
        let byID = Dictionary(messages.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let thread = context.model.cache?.threads[conversation.id]
        let newest = messages.last?.seq ?? 0
        let unseen = messages.filter { $0.seq > seenSeq && !context.isMine($0) }.count
        let locked = TeamsComposeLogic.composerLock(
            uid: context.uid, kind: conversation.kind, participants: conversation.participants, readers: conversation.readers,
            bots: context.bots, botsLoaded: context.model.botsLoaded, botsUnverified: context.model.botsUnverified) != .none
        let outbox = (context.model.cache?.outbox ?? []).filter { $0.conversationId == conversation.id }
        return ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if showNotice {
                    TeamsNote(text: "teams_disclosure", systemImage: "lock.shield")
                        .padding(.horizontal, 16).padding(.vertical, 10)
                }
                if start > 0 {
                    Button("teams_messages_earlier") { showEarlier(messages: messages, start: start, rows: rows) }
                        .font(.subheadline.weight(.semibold))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                }
                if messages.isEmpty {
                    Text(LocalizedStringKey(thread == nil && conversation.lastSeq > 0 ? "teams_loading_messages" : "teams_empty_thread"))
                        .font(.footnote).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity).padding(.vertical, 24)
                }
                ForEach(rows, id: \.key) { row in
                    switch row {
                    case let .date(_, atMs, kind):
                        StreamDateSeparator(text: TeamsTimeText.dayLabel(kind, atMs: atMs, locale: locale)).id(row.key)
                    case .unread:
                        StreamUnreadDivider().id(row.key)
                    case let .message(item, grouped):
                        if let message = byID[item.id] {
                            TeamMessageCell(
                                message: message, grouped: grouped, context: context, quote: quote(of: message, byID: byID, context: context),
                                onReply: locked ? nil : { startReply($0) },
                                onEdit: context.isMine(message) && context.participant && message.decision == nil ? { beginEdit($0) } : nil,
                                onRecall: context.isMine(message) && context.participant && message.decision == nil ? { recalling = $0 } : nil)
                            .id(row.key)
                        }
                    }
                }
                ForEach(outbox) { intent in
                    TeamOutboxCell(intent: intent, context: context) {
                        Task { await workspace.run { try await context.model.discard(intent.id) } }
                    }
                }
            }
            .scrollTargetLayout()
        }
        .scrollPosition($position)
        .scrollDismissesKeyboard(.interactively)
        .onScrollGeometryChange(for: ScrollMetrics.self) { geometry in
            ScrollMetrics(nearBottom: geometry.contentSize.height - geometry.visibleRect.maxY < 48, container: geometry.containerSize.height)
        } action: { old, new in
            // The keyboard or a taller composer shrinks the viewport: keep the newest message in view if it was.
            if placed, old.nearBottom, old.container != new.container {
                position.scrollTo(edge: .bottom)
            } else if atBottom != new.nearBottom {
                atBottom = new.nearBottom
            }
        }
        .onChange(of: atBottom) { _, now in
            if now { seenSeq = newest }
        }
        .onChange(of: messages.last?.id) { _, _ in
            guard placed else { return }
            if atBottom || (messages.last.map(context.isMine) ?? false) {
                position.scrollTo(edge: .bottom)
                seenSeq = newest
            }
        }
        .onChange(of: outbox.count) { old, new in
            if placed, new > old { position.scrollTo(edge: .bottom) }
        }
        // Opens at the first unread message (with a little context above it), else at the newest.
        .task(id: PlaceKey(anchored: anchorSeq != nil, loaded: thread != nil)) {
            guard !placed, anchorSeq != nil, thread != nil else { return }
            await Task.yield()
            if let divider = rows.firstIndex(of: .unread) {
                var index = divider, seen = 0
                while index > 0, seen < TeamsStreamLogic.unreadContext {
                    index -= 1
                    if case .message = rows[index] { seen += 1 }
                }
                // `atBottom` follows the scroll geometry: a thread that fits on screen does not move and stays at the
                // bottom (forcing it false here would never be undone, and the conversation would never be read).
                position.scrollTo(id: rows[index].key, anchor: .top)
            } else {
                position.scrollTo(edge: .bottom)
            }
            // The history on screen is what the reader has seen: the pill counts what arrives from here on.
            seenSeq = newest
            placed = true
        }
        .overlay(alignment: .bottomTrailing) {
            if placed && !atBottom {
                Button {
                    withAnimation { position.scrollTo(edge: .bottom) }
                    seenSeq = newest
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "arrow.down")
                        if unseen > 0 { Text("teams_new_messages_pill \(unseen)").font(.footnote.weight(.semibold)) }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(.regularMaterial, in: Capsule())
                    .overlay(Capsule().stroke(Color.primary.opacity(0.1)))
                }
                .buttonStyle(.plain)
                .padding(12)
                .accessibilityLabel(Text(LocalizedStringKey(unseen > 0 ? "teams_new_messages_pill \(unseen)" : "teams_jump_latest")))
                .accessibilityIdentifier("teams.stream.jump")
            }
        }
        .accessibilityIdentifier("teams.stream")
    }

    private struct PlaceKey: Equatable {
        let anchored: Bool
        let loaded: Bool
    }

    /// Shows the 100 messages before the oldest one on screen, and keeps the reader where they were.
    private func showEarlier(messages: [TeamMessage], start: Int, rows: [StreamRow]) {
        let keep = rows.first { if case .message = $0 { true } else { false } }?.key
        let newStart = max(0, start - TeamsStreamLogic.windowSize)
        startID = messages[newStart].id
        workspace.messaging?.refreshTasks()
        if let keep {
            Task {
                await Task.yield()
                position.scrollTo(id: keep, anchor: .top)
            }
        }
    }

    private func quote(of message: TeamMessage, byID: [String: TeamMessage], context: TeamConversationContext) -> TeamMessageQuote? {
        guard let replyID = message.replyTo else { return nil }
        // A reply to a message that is not loaded says only that it is a reply (as Android does); the spec's snippet is 90 characters.
        guard let target = byID[replyID] else { return TeamMessageQuote(author: "", text: String(localized: "teams_message_reply")) }
        return TeamMessageQuote(
            author: context.identity(of: target).name,
            text: target.recalled == true ? String(localized: "teams_message_recalled") : said(target))
    }

    // MARK: Composer actions

    private func replying(_ context: TeamConversationContext, messages: [TeamMessage]) -> (author: String, text: String)? {
        guard let id = draft.replyToID, let target = messages.first(where: { $0.id == id }) else { return nil }
        return (context.identity(of: target).name, target.recalled == true ? String(localized: "teams_message_recalled") : said(target))
    }

    /// A one-line snippet of a message; one of files alone is said by their names.
    private func said(_ message: TeamMessage) -> String {
        TeamsComposeLogic.snippet(TeamAttachments.said(text: message.text, names: message.files.map(\.name)))
    }

    /// The wait the message being written would answer: it replies to a Bot's question that still waits for the viewer.
    private func answering(_ context: TeamConversationContext) -> String? {
        guard editing == nil, let id = draft.replyToID,
              let question = context.model.cache?.threads[context.conversation.id]?.messages.first(where: { $0.id == id }) else { return nil }
        return context.answerableWait(of: question)
    }

    /// Whether the message being written carries the draft's files: a new message does, an edit and an answer to a Bot's question do not.
    private func carriesFiles(_ context: TeamConversationContext) -> Bool { editing == nil && answering(context) == nil }

    private func startReply(_ message: TeamMessage) {
        if editing != nil { endEdit() }
        draft.replyToID = message.id
        composerFocused = true
    }

    /// An edit borrows the box: the draft is set aside and comes back when the edit ends.
    private func beginEdit(_ message: TeamMessage) {
        if editing == nil { savedText = draft.text }
        editing = message
        draft.replyToID = nil
        draft.targets = []
        draft.text = message.text
        composerFocused = true
    }

    private func endEdit() {
        editing = nil
        draft.text = savedText ?? ""
        savedText = nil
    }

    private func send(_ context: TeamConversationContext) async {
        let body = draft.text, reply = draft.replyToID, targets = draft.targets, changing = editing
        // A reply to a Bot's question that still waits for the viewer is its answer: it continues that task and asks no Bot.
        let waitId = answering(context)
        // The files are this conversation's draft's, read at the moment of sending: no other conversation's are ever sent here.
        let files = changing == nil && waitId == nil ? workspace.files(for: conversationID) : []
        guard TeamAttachments.canSend(text: body, files: files) else { return }
        let sent = await workspace.run {
            let model = context.model
            if let waitId { try await model.queue(operation: "create", text: body, replyTo: reply, waitId: waitId); return }
            // A Bot the viewer picked may have been revoked since: the draft is never sent with a stale target.
            if changing == nil, targets.contains(where: { key in !model.bots.contains { $0.sharedBotId == key && $0.selectable } }) {
                throw TeamMessageFailure(code: "BOT_USE_REQUIRED")
            }
            guard model.selected == conversationID else { throw TeamMessageFailure(code: "INVALID_INPUT") }
            try await model.queue(operation: changing == nil ? "create" : "edit", text: body, messageId: changing?.id, replyTo: reply, targets: changing == nil ? targets : [], files: files)
        }
        guard sent else { return }
        workspace.filesSent(files.map(\.uploadId), in: conversationID)
        if changing != nil {
            // The set-aside draft comes back (Desktop: `onClearEditing` restores the text kept while editing).
            endEdit()
        } else {
            draft.targets = []
            // Text typed while the message was being saved stays.
            if draft.text == body { draft.text = "" }
        }
        if reply != nil, draft.replyToID == reply { draft.replyToID = nil }
    }
}

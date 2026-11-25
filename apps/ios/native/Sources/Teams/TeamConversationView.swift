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
    @Environment(InboxStore.self) private var inbox
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.dismiss) private var dismiss
    @Environment(\.locale) private var locale
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(AppNavigationModel.self) private var navigation
    let conversationID: String
    /// Called when the conversation is gone (the viewer is not in it any more). By default the screen pops itself;
    /// the wide layout's pane clears its selection instead.
    var onGone: (() -> Void)?
    /// The wide layout opens the conversation details as a side pane; by default they are a sheet.
    var onShowDetails: (() -> Void)?
    var onShowProfile: (() -> Void)?

    /// `lastReadSeq` as it was when the conversation opened: the "new messages" divider stays where it was
    /// even though reading moves the mark. `Int.max` = no divider (the mark never arrived).
    @State private var anchorSeq: Int?
    @State private var visibleMessageIDs: Set<String> = []
    @State private var seenMessages = InboxReadVisibility()
    @State private var peerRead: Int?
    /// The oldest message the reader asked for with "show earlier messages".
    @State private var startID: String?
    @State private var atBottom = true
    /// The newest `seq` seen while at the bottom; later messages by others count for the pill.
    @State private var seenSeq = 0
    @State private var placed = false
    @State private var position = ScrollPosition(edge: .bottom)
    @State private var showDetails = false
    @State private var showDataset = false
    @State private var showNotice = false
    @State private var recalling: TeamMessage?
    @State private var editing: TeamMessage?
    @State private var savedText: String?
    @State private var savedMentions: [TeamMentionEntity]?
    @State private var draft = TeamComposerDraft()
    @State private var draftLoaded = false
    @State private var ownedDraftKey = ""
    /// The app's voice input, made the first time the microphone is used here. What it hears goes into the box.
    @State private var voice: VoiceInteractionController?
    @State private var composerFocused = false

    var body: some View {
        Group {
            if let context = TeamConversationContext.make(workspace: workspace, conversationID: conversationID) {
                loaded(context)
            } else {
                missing
                    .navigationTitle(workspace.messaging?.cache?.creation.flatMap { $0.conversationId == conversationID ? $0.title : nil }
                                     ?? workspace.team?.name ?? "")
                    .toolbar {
                        if let model = workspace.messaging,
                           !model.error.isEmpty || (model.directoryReady && model.offline) {
                            ToolbarItem(placement: .topBarTrailing) {
                                let status = model.error.isEmpty ? LocalizedStringKey("teams_offline_title") : LocalizedStringKey(TeamsErrors.key(model.error))
                                Menu {
                                    Text(model.error.isEmpty ? LocalizedStringKey("teams_messages_offline") : status)
                                    Button("teams_reconnect", systemImage: "arrow.clockwise") { workspace.reconnect() }
                                } label: { Image(systemName: model.error.isEmpty ? "wifi.slash" : "exclamationmark.circle") }
                                .accessibilityLabel(Text(status))
                            }
                        }
                    }
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: workspace.pair == nil ? .teams : .friends)
        // The model is replaced when the team or its membership generation changes; the new one must open this thread.
        .task(id: ThreadVisibility(model: workspace.messaging.map(ObjectIdentifier.init), visible: selectedTabIsVisible)) {
            if selectedTabIsVisible { workspace.openConversation(conversationID) }
            else if workspace.messaging?.selected == conversationID { workspace.openConversation("") }
        }
        .onAppear {
            if !draftLoaded || ownedDraftKey != workspace.draftKey(conversationID) { loadDraft() }
            showNotice = workspace.pair == nil && !workspace.platformNoticeShown
            workspace.markPlatformNoticeShown()
        }
        .onChange(of: draft) { _, value in
            if draftLoaded, editing == nil, ownedDraftKey == workspace.draftKey(conversationID) { workspace.setDraft(value, for: conversationID) }
        }
        .onChange(of: activeInboxRef, initial: true) { old, next in
            if let next { inbox.setCurrent(next) }
            else if let old { inbox.setCurrent(nil, replacing: old) }
            seenMessages = InboxReadVisibility()
        }
        .onDisappear {
            if let ref = inboxRef { inbox.setCurrent(nil, replacing: ref) }
            if draftLoaded, editing == nil, ownedDraftKey == workspace.draftKey(conversationID) { workspace.setDraft(draft, for: conversationID) }
            voice?.close()
        }
        // Another account, another team, or the viewer's membership changed: what was being said is dropped, not moved.
        .onChange(of: workspace.draftKey(conversationID)) { _, _ in
            voice?.close()
            editing = nil; savedText = nil; savedMentions = nil
            loadDraft()
        }
        .onChange(of: workspace.draft(for: conversationID)) { _, value in
            if ownedDraftKey == workspace.draftKey(conversationID), editing == nil, draft != value { draft = value }
        }
        // Without `.contain` the identifier is copied onto every child (composer input, send, rows).
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.conversation.\(conversationID)")
    }

    private var inboxRef: InboxRef? {
        guard workspace.uid == inbox.uid, let team = workspace.messagingTeam else { return nil }
        let ref = InboxRef(kind: .team, sourceId: team.id, conversationId: conversationID, generation: team.generation)
        return ref.valid ? ref : nil
    }

    private struct ThreadVisibility: Equatable {
        let model: ObjectIdentifier?
        let visible: Bool
    }

    private var selectedTabIsVisible: Bool { navigation.tabs.selected == (workspace.pair == nil ? .teams : .friends) }

    private var activeInboxRef: InboxRef? {
        guard selectedTabIsVisible,
              scenePhase == .active, workspace.messaging?.selected == conversationID else { return nil }
        return inboxRef
    }

    private func openDetails() {
        if let onShowDetails { onShowDetails() } else { showDetails = true }
    }

    private func loadDraft() {
        ownedDraftKey = workspace.draftKey(conversationID)
        draft = workspace.draft(for: conversationID)
        draftLoaded = true
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
                Button(role: .destructive) {
                    Task { if await workspace.run({ try await model?.discardCreation() }) { dismiss() } }
                } label: {
                    Text("teams_message_discard").minimumHitTarget()
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
        let connectionError = !model.error.isEmpty
        let connectionOffline = model.directoryReady && model.offline
        return VStack(spacing: 0) {
            banners(context)
            if let notice = inbox.inlineNotice, notice.ref == activeInboxRef {
                Label("inbox_notice", systemImage: "bell").font(.caption).padding(5)
            }
            if let peerRead, let lastSent = messages.last(where: context.isMine), peerRead >= lastSent.seq {
                Text("inbox_read_receipt").font(.caption).foregroundStyle(.secondary)
            }
            if conversation.kind == "bot_dm", let bot = context.bots.first(where: { conversation.participants.contains("shared_bot:\($0.sharedBotId)") }) {
                TeamBotMemberInfo(bot: bot, conversation: conversation, unverified: model.botsUnverified).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal).padding(.vertical, 6)
            }
            stream(context, messages: messages)
            voicePanel
            if context.participant && !connectionOffline && editing == nil {
                Button("teams_dataset_title") { showDataset = true }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal)
            }
            TeamComposerView(
                context: context, draft: $draft, editing: editing, answeringWait: answering(context) != nil, replying: replying(context, messages: messages), busy: workspace.messageBusy,
                files: carrying ? workspace.files(for: conversationID) : [], refusals: carrying ? workspace.refusals(for: conversationID) : [],
                onChooseFiles: carrying ? { workspace.addFiles($0, to: conversationID) } : nil,
                onRemoveFile: { workspace.removeFile($0, from: conversationID) }, onDismissRefusals: { workspace.dismissRefusals(for: conversationID) },
                listening: voice?.capturePhase == .listening, onVoice: toggleVoice,
                focused: $composerFocused, onSend: { Task { await send(context) } },
                onCancelEdit: endEdit, onCancelReply: { draft.replyToID = nil })
        }
        .sheet(isPresented: $showDataset) { TeamDatasetShareSheet(context: context) }
        .toolbar {
            ToolbarItem(placement: .principal) {
                let identity = HStack(spacing: 8) {
                    ConversationLead(conversation: conversation, label: context.label, participants: participants, size: 32)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(context.label).font(.headline).lineLimit(1)
                        if let subtitle = context.subtitle { Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                    }
                }
                Group {
                    if context.pair == nil {
                        Button(action: openDetails) { identity }
                            .buttonStyle(.plain)
                            .accessibilityHint(Text("teams_conversation_details"))
                    } else if let onShowProfile {
                        Button(action: onShowProfile) { identity }
                            .buttonStyle(.plain)
                            .accessibilityHint(Text("friends_menu_view_profile"))
                    } else {
                        identity
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("teams.conversation.header")
            }
            if let link = TeamShareLinks.link(teamID: context.team.id, conversation: conversation) {
                ToolbarItem(placement: .topBarTrailing) {
                    ShareLink(item: link, subject: Text(verbatim: context.label)) {
                        Image(systemName: "square.and.arrow.up")
                    }
                    .accessibilityLabel(Text("teams_share_link"))
                    .accessibilityIdentifier("teams.conversation.share")
                }
            }
            if context.pair == nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: openDetails) { Image(systemName: "info.circle") }
                        .accessibilityLabel(Text("teams_conversation_details"))
                        .accessibilityIdentifier("teams.conversation.info")
                }
            }
            // A friendship's chats read themselves, but connection recovery still belongs in the top bar.
            let canMarkRead = unread > 0 && context.pair == nil
            if canMarkRead || connectionError || connectionOffline {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        if connectionError || connectionOffline {
                            Text(connectionError ? LocalizedStringKey(TeamsErrors.key(model.error)) : "teams_messages_offline")
                            Button("teams_reconnect", systemImage: "arrow.clockwise") { workspace.reconnect() }
                        }
                        if canMarkRead {
                            Button("teams_mark_read", systemImage: "checkmark.circle") { Task { await workspace.markRead(conversation.id) } }
                                .disabled(model.offline || workspace.messageBusy)
                        }
                    } label: {
                        Image(systemName: connectionError ? "exclamationmark.circle" : connectionOffline ? "wifi.slash" : "ellipsis.circle")
                    }
                    .accessibilityLabel(Text(connectionError ? LocalizedStringKey(TeamsErrors.key(model.error)) : connectionOffline ? "teams_offline_title" : "teams_conversation_menu"))
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
            Button("common_cancel", role: .cancel) {}
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
        .task(id: ReadVisibilityKey(ref: activeInboxRef, ids: visibleMessageIDs,
                                   lastRead: conversation.lastReadSeq, canRead: placed && model.readsReady && !model.fromCache)) {
            guard let ref = activeInboxRef, placed, model.readsReady, !model.fromCache,
                  let row = inbox.row(ref) else { return }
            let visible = messages.filter { visibleMessageIDs.contains("message:" + $0.id) }.map(\.seq)
            guard !visible.isEmpty else { return }
            do { try await Task.sleep(for: .milliseconds(300)) } catch { return }
            guard activeInboxRef == ref, !Task.isCancelled else { return }
            let baseline = max(row.readThrough, conversation.lastReadSeq)
            let through = seenMessages.observe(visible, after: baseline)
            inbox.sawContent(row, through: through)
        }
        .task(id: PeerReceiptKey(ref: activeInboxRef, sourceRevision: inbox.activeRow?.sourceRevision, enabled: inbox.dmReceipts)) {
            peerRead = nil
            guard conversation.kind == "dm" || conversation.kind == "friend_chat", let ref = activeInboxRef else { return }
            while inbox.dmReceipts, !Task.isCancelled {
                peerRead = await inbox.peerReceipt(ref)
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
            }
        }
    }

    private struct ReadVisibilityKey: Equatable {
        let ref: InboxRef?
        let ids: Set<String>
        let lastRead: Int
        let canRead: Bool
    }

    private struct PeerReceiptKey: Equatable {
        let ref: InboxRef?
        let sourceRevision: Int?
        let enabled: Bool
    }

    private func banners(_ context: TeamConversationContext) -> some View {
        let model = context.model
        return VStack(spacing: 0) {
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
                if start > 0 || context.model.hasOlderMessages {
                    Button {
                        if start > 0 {
                            showEarlier(messages: messages, start: start, rows: rows)
                        } else {
                            Task {
                                if let previous = await context.model.loadOlder() {
                                    startID = context.model.cache?.threads[conversationID]?.messages.first?.id
                                    await Task.yield()
                                    position.scrollTo(id: "message:" + previous, anchor: .top)
                                }
                            }
                        }
                    } label: {
                        Text("teams_messages_earlier").font(.subheadline.weight(.semibold)).frame(maxWidth: .infinity).minimumHitTarget(minWidth: 0)
                    }
                }
                if messages.isEmpty {
                    if thread == nil && conversation.lastSeq > 0 {
                        Text("teams_loading_messages")
                            .font(.footnote).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity).padding(.vertical, 24)
                    } else {
                        // Nothing here yet: the composer below is the action (U9).
                        TeamsStateView(systemImage: "bubble.left.and.bubble.right", title: "teams_preview_none", message: "teams_empty_thread_hint")
                            .frame(maxWidth: .infinity).padding(.vertical, 24)
                    }
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
                    TeamOutboxCell(intent: intent) {
                        Task { await workspace.run { try await context.model.discard(intent.id) } }
                    }
                }
            }
            .padding(.vertical, 8)
            .scrollTargetLayout()
        }
        .background(Color(uiColor: .systemGroupedBackground))
        .defaultScrollAnchor(.bottom, for: .alignment)
        .scrollPosition($position)
        .onScrollTargetVisibilityChange(idType: String.self, threshold: 0.1) { ids in
            let next = Set(ids.filter { $0.hasPrefix("message:") })
            if visibleMessageIDs != next { visibleMessageIDs = next }
        }
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
                    withAnimation(.lxMotion(reduceMotion)) { position.scrollTo(edge: .bottom) }
                    if let ref = activeInboxRef, let row = inbox.row(ref) { inbox.markRead([row]) }
                    seenSeq = newest
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "arrow.down")
                        if unseen > 0 { Text("teams_new_messages_pill \(unseen)").font(.footnote.weight(.semibold)) }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(.regularMaterial, in: Capsule())
                    .overlay(Capsule().stroke(Color.primary.opacity(0.1)))
                    .minimumHitTarget()
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

    // MARK: Voice

    /// What the voice input shows while it is used (as in a chat): how to set it up when it cannot listen — the
    /// box stays usable for typing — or what it is hearing.
    @ViewBuilder
    private var voicePanel: some View {
        if let voice {
            let settings = { navigation.showSettings(.voice, origin: navigation.selectedTab) }
            if let guidance = AudioSetupGuidance.forPhase(voice.phase, message: voice.statusDetail) {
                AudioSetupBanner(guidance: guidance, onConfigure: settings, onDismiss: voice.close)
            } else if voice.isPresented {
                InlineVoicePanel(controller: voice, onConfigure: settings)
            }
        }
    }

    /// The microphone button: starts listening, or ends it so the words are recognized. The words are added to the
    /// box of the conversation they were spoken in, for the viewer to read, change and send; they are never sent
    /// from here, and ones that arrive after the account, the team or the membership changed are dropped.
    private func toggleVoice() {
        let controller = voice ?? VoiceInteractionController()
        voice = controller
        switch controller.capturePhase {
        case .idle:
            let key = workspace.draftKey(conversationID)
            composerFocused = false
            controller.startDictation { heard in
                guard workspace.draftKey(conversationID) == key else { return }
                let next = TeamsComposeLogic.appendingTranscript(heard, to: draft.text)
                // Dictation appends to this draft, so any existing ranges stay at their offsets.
                draft.text = next
            }
        case .listening:
            controller.finishListening()
        case .finishing:
            break
        }
    }

    private func startReply(_ message: TeamMessage) {
        if editing != nil { endEdit() }
        draft.replyToID = message.id
        composerFocused = true
    }

    /// An edit borrows the box: the draft is set aside and comes back when the edit ends.
    private func beginEdit(_ message: TeamMessage) {
        // Words still being spoken were for the message set aside, not for the one being changed.
        voice?.cancelDictation()
        if editing == nil { savedText = draft.text; savedMentions = draft.mentionEntities }
        editing = message
        draft.replyToID = nil
        draft.targets = []
        draft.text = message.text
        draft.mentionEntities = message.mentionEntities
        composerFocused = true
    }

    private func endEdit() {
        voice?.cancelDictation()
        editing = nil
        draft.text = savedText ?? ""
        draft.mentionEntities = savedMentions
        savedText = nil; savedMentions = nil
    }

    private func send(_ context: TeamConversationContext) async {
        let ownerKey = workspace.draftKey(conversationID)
        guard ownedDraftKey == ownerKey else { return }
        let submittedDraft = draft
        let body = submittedDraft.text, reply = submittedDraft.replyToID, targets = submittedDraft.targets, changing = editing
        // A reply to a Bot's question that still waits for the viewer is its answer: it continues that task and asks no Bot.
        let waitId = answering(context)
        let entities = TeamMentionEntity.valid(submittedDraft.mentionEntities ?? [], in: body)
            .filter { context.conversation.participants.contains("user:\($0.userId)") }
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
            try await model.queue(operation: changing == nil ? "create" : "edit", text: body, messageId: changing?.id, replyTo: reply, targets: changing == nil ? targets : [], files: files, mentionEntities: !entities.isEmpty || changing?.mentionEntities?.isEmpty == false ? entities : nil)
        }
        guard sent, ownedDraftKey == ownerKey, workspace.draftKey(conversationID) == ownerKey, workspace.messaging === context.model else { return }
        workspace.filesSent(files.map(\.uploadId), in: conversationID)
        if changing != nil {
            // The set-aside draft comes back (Desktop: `onClearEditing` restores the text kept while editing).
            endEdit()
        } else {
            if draft.targets == targets { draft.targets = [] }
            // Both text and mentions can change while the message is being saved.
            if draft.text == body && draft.mentionEntities == submittedDraft.mentionEntities {
                draft.text = ""
                draft.mentionEntities = nil
            }
        }
        if reply != nil, draft.replyToID == reply { draft.replyToID = nil }
    }
}

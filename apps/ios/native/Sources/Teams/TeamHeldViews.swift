import SwiftUI

extension TeamConversationContext {
    /// The Bots of this Chat whose held replies the viewer decides on: the ones they maintain that are in it.
    var heldBots: [TeamBotCard] { TeamHeldRules.bots(conversation, cards: bots, viewer: viewer) }
}

/// What a screen has to say for the held replies it shows: the Bots, the Chat (none for every Chat), whether there is a connection, and what tells it the Chat's replies moved.
struct TeamHeldWanted: Equatable {
    let model: ObjectIdentifier?
    let bots: [String]
    let conversationId: String?
    let online: Bool
    let watch: String
}

/// The replies that wait for this person on one screen (`packages/teams/held-hooks.ts`): the Bots' replies in one Chat or in all of them, read when the screen opens, when `watch`
/// changes (the screen says it with whatever tells it the Chat's tasks moved) and when the connection returns, never on a timer of their own. The time left is kept while the screen
/// runs `keepTime`. A change of model, Chat or Bots starts the list over, and what was read for the old one is never shown for the new one.
@MainActor @Observable
final class TeamHeldKeeper {
    private(set) var list = TeamHeldList(status: .ready)
    @ObservationIgnored private var board: TeamHeldBoard?
    @ObservationIgnored private var key = ""
    /// What the list was last read for: a change of it, or the connection coming back, is a reason to read it again.
    @ObservationIgnored private var seen = (watch: "", online: false)
    @ObservationIgnored private weak var model: TeamMessagingModel?

    /// Starts keeping the replies of `bots` (the screen's Chat when it has one), or reads them again when `watch` changed or the connection returned. Nothing is read for no Bot, and
    /// nothing is read while `online` is false.
    func sync(model: TeamMessagingModel, bots: [String], conversationId: String?, online: Bool, watch: String) {
        let next = "\(ObjectIdentifier(model).hashValue)|\(conversationId ?? "")|\(bots.joined(separator: ","))"
        if next != key {
            board?.close(); board = nil
            key = next; self.model = model; seen = (watch, online)
            guard !bots.isEmpty else { list = TeamHeldList(status: .ready); return }
            let board = TeamHeldBoard(
                read: { [model] in try await model.listHeldReplies(bots, conversationId: conversationId) },
                decide: TeamHeldDecide(approve: { [model] taskId, text, commandId in try await model.approveReply(taskId, text: text, commandId: commandId) },
                                       reject: { [model] taskId, commandId in try await model.rejectReply(taskId, commandId: commandId) }),
                changed: { [weak self] in self?.list = $0 })
            self.board = board
            list = board.list
            if online { Task { await board.load() } }
            return
        }
        let before = seen
        seen = (watch, online)
        if online, before.watch != watch || !before.online { reload() }
    }
    /// Nothing is kept or read for this screen any more.
    func stop() {
        board?.close(); board = nil; key = ""
        list = TeamHeldList(status: .ready)
    }
    /// Keeps the time until the screen's task is cancelled.
    func keepTime() async { await board?.keepTime() }
    func reload() {
        guard let board else { return }
        Task { await board.load() }
    }
    /// Lets a reply go, as the Bot made it or with `text`; the Chat's tasks are read again once the service took the decision.
    func approve(_ taskId: String, text: String? = nil) {
        guard let board else { return }
        Task { if await board.approve(taskId, text: text) { model?.refreshTasks() } }
    }
    func reject(_ taskId: String) {
        guard let board else { return }
        Task { if await board.reject(taskId) { model?.refreshTasks() } }
    }
    func dismiss(_ taskId: String) { board?.dismiss(taskId) }
}

/// A reply of a Bot that waits for its maintainer, as that person reads it (`packages/teams/HeldReplyCard.tsx`): the Bot's words and the document that goes with them, the time they
/// have to decide, and what they can do. They approve the reply as it is, change its text and send that, or reject it after being asked once more. Nobody else is shown any of it.
///
/// Once the reply is no longer there to decide (it was decided, its time ran out, a newer message dropped it) the card says so in its place, without the words and without the
/// buttons, and goes when it is closed. `inList`: the card of the list of a Bot's replies, which is already under the Chat it belongs to and leaves out the line that says what it is.
struct TeamHeldCard: View {
    let row: TeamHeldRow
    /// The time the card is shown at, from the list: the time left is as new as it is.
    let now: Double
    /// The Bot that made the reply, named where the card sits under a message that asked it; a card that is already under the Bot's name leaves it out.
    var botName: String? = nil
    var inList = false
    let keeper: TeamHeldKeeper
    @State private var editing: String?
    @State private var rejecting = false
    @FocusState private var focused: Bool

    private var reply: TeamHeldReply { row.reply }
    private var id: String { reply.taskId }
    private var busy: Bool { row.busy != nil }

    var body: some View {
        Group {
            if let ended = row.over(at: now) { over(ended) } else { held }
        }
        .padding(inList ? 0 : 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background { if !inList { RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color(uiColor: .secondarySystemBackground)) } }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.held.\(id)")
    }

    // MARK: Over

    /// The time to decide is over, or the service said the reply is not there: what became of it, and Close once the service said it.
    private func over(_ code: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label { Text(LocalizedStringKey(TeamHeldRules.errorKey(code))).font(.footnote) } icon: { Image(systemName: "clock.badge.xmark").foregroundStyle(.secondary) }
                .accessibilityIdentifier("teams.held.over.\(id)")
            if row.ended != nil {
                Button("common_close") { keeper.dismiss(id) }
                    .buttonStyle(.borderless).font(.footnote)
                    .accessibilityIdentifier("teams.held.close.\(id)")
            }
        }
    }

    // MARK: Waiting

    private var held: some View {
        let edit = editing.map { TeamHeldRules.edit(draft: reply.text, edited: $0) }
        return VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 4) {
                if !inList { Label("teams_held_title", systemImage: "clock").font(.footnote.weight(.semibold)) }
                if let botName { Text(verbatim: botName).font(.subheadline.weight(.semibold)) }
                HStack(spacing: 6) {
                    Text("teams_held_expires \(TeamsBotText.expiryText(at: reply.expiresAt, now: now))").font(.caption).foregroundStyle(.secondary)
                    if reply.unasked {
                        Text("teams_held_unasked")
                            .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                    }
                }
                .accessibilityIdentifier("teams.held.expires.\(id)")
                if let draft = editing { editor(draft, edit) } else { AIText(markdown: reply.text).teamBotLinkPolicy().accessibilityIdentifier("teams.held.text.\(id)") }
                if let document = reply.document {
                    Label { Text("teams_held_document \(document.title)") } icon: { Image(systemName: "doc.text") }
                        .font(.caption).foregroundStyle(.secondary)
                        .accessibilityIdentifier("teams.held.document.\(id)")
                }
            }
            if editing != nil, let problem = edit.flatMap(Self.problem) {
                Text(LocalizedStringKey(problem)).font(.caption).foregroundStyle(.secondary)
                    .accessibilityIdentifier("teams.held.problem.\(id)")
            }
            // The buttons stack under a large text size, as the buttons of the run card do.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) { buttons(edit) }
                VStack(alignment: .leading, spacing: 0) { buttons(edit) }
            }
            .disabled(busy)
            if let error = row.error {
                Text(LocalizedStringKey(TeamHeldRules.errorKey(error))).font(.footnote).foregroundStyle(.red)
                    .accessibilityIdentifier("teams.held.error.\(id)")
            }
        }
        // The question is answered once it is asked: what comes of the decision is told on the card, which is the one thing on screen that can say it.
        .confirmationDialog("teams_held_reject_confirm", isPresented: $rejecting, titleVisibility: .visible) {
            Button("teams_held_reject", role: .destructive) { keeper.reject(id) }
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("teams_held_reject_detail")
        }
    }

    private func sendable(_ edit: TeamHeldEdit?) -> Bool {
        if case .send? = edit { return true }
        return false
    }

    private static func problem(_ edit: TeamHeldEdit) -> String? {
        switch edit {
        case .send: nil
        case .empty: "teams_held_empty_text"
        case .tooLong: "teams_held_too_long"
        }
    }

    private func editor(_ draft: String, _ edit: TeamHeldEdit?) -> some View {
        TextField("teams_held_edit_label", text: Binding(get: { editing ?? draft }, set: { editing = $0 }), axis: .vertical)
            .lineLimit(3...12)
            .textFieldStyle(.roundedBorder)
            .focused($focused)
            .disabled(busy)
            .task { focused = true }
            .accessibilityIdentifier("teams.held.editor.\(id)")
    }

    @ViewBuilder private func buttons(_ edit: TeamHeldEdit?) -> some View {
        if editing != nil {
            Button {
                if case .send(let text)? = edit { keeper.approve(id, text: text) }
            } label: { Text("teams_message_send").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.borderedProminent)
                .disabled(!sendable(edit))
                .accessibilityIdentifier("teams.held.send.\(id)")
            Button { editing = nil } label: { Text("common_cancel").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("teams.held.cancel.\(id)")
        } else {
            Button { keeper.approve(id) } label: { Text("teams_approval_approve").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("teams.held.approve.\(id)")
            Button { editing = reply.text } label: { Text("teams_message_edit").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("teams.held.edit.\(id)")
            Button(role: .destructive) { rejecting = true } label: { Text("teams_held_reject").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("teams.held.reject.\(id)")
        }
    }
}

/// A reply nobody asked for has no task in the Chat (the service keeps such a task out of every member's list), so it is a row of its own at the end of the stream, from the Bot
/// that made it.
struct TeamHeldCell: View {
    let row: TeamHeldRow
    let now: Double
    let context: TeamConversationContext
    let keeper: TeamHeldKeeper

    var body: some View {
        let card = context.card(forKey: row.reply.sharedBotId)
        let identity = TeamIdentity(kind: .bot, id: card?.sharedBotId ?? row.reply.sharedBotId, name: card?.name ?? context.strings.bot, isSelf: false)
        HStack(alignment: .top, spacing: 8) {
            IdentityAvatar(identity: identity, size: 32)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(verbatim: identity.name).font(.subheadline.weight(.semibold)).lineLimit(1)
                    Text(context.strings.bot)
                        .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                }
                TeamHeldCard(row: row, now: now, keeper: keeper)
            }
            .frame(maxWidth: 700, alignment: .leading)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.top, 8).padding(.bottom, 1)
        .frame(maxWidth: 832)
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.held.row.\(row.reply.taskId)")
    }
}

/// Every reply of one Bot that waits for its maintainer, grouped by the Chat it was made in, the Chat of the newest reply first; each is the card of the Chat, with the same
/// decisions, and each Chat can be opened. When the service says the person does not maintain the Bot after all, the list closes.
struct TeamHeldListSheet: View {
    let bot: TeamBotCard
    let keeper: TeamHeldKeeper
    /// Takes the person to the Chat a reply was made in.
    let onOpenChat: (String) -> Void
    let onClose: () -> Void

    var body: some View {
        let list = keeper.list
        NavigationStack {
            List {
                Section {
                    TeamsNote(text: "teams_held_intro", systemImage: "info.circle")
                    Text(verbatim: bot.name).font(.subheadline.weight(.semibold)).accessibilityIdentifier("teams.held.list.bot")
                }
                if list.status == .loading {
                    Section { HStack(spacing: 10) { ProgressView(); Text("teams_loading").foregroundStyle(.secondary) }.accessibilityIdentifier("teams.held.list.loading") }
                }
                if list.status == .ready && list.rows.isEmpty {
                    Section { Text("teams_held_none").font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("teams.held.list.empty") }
                }
                ForEach(TeamHeldRules.byChat(list.rows)) { chat in
                    Section {
                        ForEach(chat.rows) { row in
                            TeamHeldCard(row: row, now: list.now, inList: true, keeper: keeper)
                                .padding(.vertical, 4)
                        }
                    } header: {
                        HStack(alignment: .firstTextBaseline) {
                            Text(verbatim: chat.title ?? String(localized: "teams_triage_chat_untitled")).font(.subheadline.weight(.semibold)).foregroundStyle(.primary)
                            Spacer(minLength: 8)
                            Button("friends_open_chat") { onOpenChat(chat.id) }.font(.subheadline).textCase(nil)
                                .accessibilityIdentifier("teams.held.list.open.\(chat.id)")
                        }
                        .textCase(nil)
                    }
                }
                if list.status == .failed, let error = list.error {
                    Section {
                        Text(LocalizedStringKey(TeamHeldRules.errorKey(error))).foregroundStyle(.red).accessibilityIdentifier("teams.held.list.error")
                        if list.rows.isEmpty { Button("common_retry") { keeper.reload() }.accessibilityIdentifier("teams.held.list.retry") }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("teams_held_waiting")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_close", action: onClose) }
                ToolbarItem(placement: .primaryAction) {
                    Button("teams_list_refresh", systemImage: "arrow.clockwise") { keeper.reload() }
                        .disabled(list.status == .loading)
                        .accessibilityIdentifier("teams.held.list.refresh")
                }
            }
            .onChange(of: list.status) { _, status in if status == .denied { onClose() } }
        }
        .accessibilityIdentifier("teams.held.list")
    }
}

/// "Replies waiting" on the card of a Bot its viewer maintains: a button with the number of replies of the Bot that wait for them across every Chat, only while there are some.
struct TeamHeldEntryLabel: View {
    let count: Int

    var body: some View {
        HStack(spacing: 6) {
            Label("teams_held_waiting", systemImage: "tray.full")
            Text(verbatim: count > 99 ? "99+" : String(count))
                .font(.caption2.weight(.bold)).monospacedDigit()
                .padding(.horizontal, 6).padding(.vertical, 1)
                .background(Color.accentColor.opacity(0.18), in: Capsule())
                .accessibilityHidden(true)
        }
        .minimumHitTarget(minWidth: 0)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: TeamsStreamLogic.spokenLine([TeamsText.localized("teams_held_waiting"), String(count)])))
    }
}

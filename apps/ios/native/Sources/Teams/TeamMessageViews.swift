import SwiftUI
import UIKit

/// What the pieces of an open conversation need to know about it, gathered once per update by
/// `TeamConversationView`: who the viewer is, the team, the model, the people and Bots of the team (for names
/// and faces) and the words for labels.
struct TeamConversationContext {
    let uid: String
    let team: TeamSummary
    let model: TeamMessagingModel
    let conversation: TeamMessageConversation
    let label: String
    let members: [TeamMember]
    let bots: [TeamBotCard]
    let strings: TeamsListStrings
    /// Set in a friend chat (docs/FRIENDS-CLIENTS.md D9): the friend stands in for the conversation's name. Only for the friend
    /// chat itself: a Bot DM on the same pair workspace is an ordinary Bot DM (D22), headed by the Bot.
    let pair: PairTeam?
    /// The replies of this Chat's Bots that wait for the viewer, kept by the open Chat. `nil` where nothing is read for them (the details of the Chat).
    var held: TeamHeldKeeper? = nil

    /// The signed-in member as commands and tasks name them.
    var viewer: TeamMemberRef { TeamMemberRef(uid: uid, generation: team.generation) }
    /// The wait a reply to `message` would answer: `message` is a Bot's question whose task still waits for the viewer.
    @MainActor
    func answerableWait(of message: TeamMessage) -> String? {
        guard let waitId = message.waitId, let taskId = message.taskId, message.author.kind != "user" else { return nil }
        let pending = Set((model.cache?.outbox ?? []).compactMap(\.waitId))
        return TeamTaskActions.answerableWait(model.tasks.first { $0.taskId == taskId }, viewer: viewer, pendingWaitIds: pending) == waitId ? waitId : nil
    }
    /// Where the Bot's request under `message` stands for the viewer, and the name of the member who decides or decided it.
    @MainActor
    func approval(of message: TeamMessage) -> (state: TeamApprovalState, decider: String) {
        let state = TeamTaskActions.approvalState(waitId: message.waitId ?? "", task: model.tasks.first { $0.taskId == message.taskId }, viewer: viewer,
                                                  messages: model.cache?.threads[conversation.id]?.messages ?? [])
        switch state {
        case .waiting(let uid), .approved(let uid), .declined(let uid): return (state, userName(uid))
        case .decide, .closed: return (state, "")
        }
    }
    /// What a message says. The service writes an approval request and the reply to it in English; they are shown in the viewer's language.
    func text(of message: TeamMessage) -> String {
        if let request = message.approvalRequest { return String(localized: "teams_approval_request \(request.title)") }
        if message.author.kind == "user", message.waitId != nil, let approved = message.decision?.approved {
            return String(localized: approved ? "teams_approval_reply_approved" : "teams_approval_reply_declined")
        }
        return message.text
    }
    /// The viewer may send here (a Bot's owner reading someone else's Bot DM only observes).
    var participant: Bool { conversation.participants.isEmpty || conversation.participants.contains("user:\(uid)") }

    func userName(_ id: String) -> String {
        let name = members.first { $0.uid == id }?.displayName ?? ""
        if !name.isEmpty { return name }
        return id == uid ? strings.you : strings.member
    }

    func card(forKey key: String?) -> TeamBotCard? {
        guard let key else { return nil }
        return bots.first { $0.sharedBotId == key }
    }

    /// The id a Bot's face is drawn from: its `botId`, else the last segment of its key.
    func avatarID(forSharedBotId key: String) -> String {
        card(forKey: key)?.avatarId ?? key
    }

    /// Who wrote a message, as an avatar identity.
    func identity(of message: TeamMessage) -> TeamIdentity {
        if message.author.kind == "user" {
            return TeamIdentity(kind: .user, id: message.author.uid, name: userName(message.author.uid), isSelf: message.author.uid == uid)
        }
        if message.author.kind == "task", let id = message.author.taskId, let name = message.author.displayName {
            return TeamIdentity(kind: .task, id: id, name: name, isSelf: false)
        }
        let card = card(forKey: message.author.sharedBotId)
        let name = message.author.displayName ?? card?.name ?? ""
        return TeamIdentity(kind: .bot, id: card?.sharedBotId ?? message.author.sharedBotId ?? message.author.uid, name: name.isEmpty ? strings.bot : name, isSelf: false)
    }

    func isMine(_ message: TeamMessage) -> Bool { message.author.kind == "user" && message.author.uid == uid }

    /// Who wrote a message, as its author line reads: "You" on the viewer's own messages (docs/TEAMS-UI-POLISH.md U3), else the
    /// name of the person or the Bot.
    func authorName(of message: TeamMessage) -> String {
        isMine(message) ? TeamsText.you : identity(of: message).name
    }

    /// The same member-safe, freshness-aware status shown by the Bot directory.
    @MainActor
    func presenceText(_ card: TeamBotCard) -> String {
        TeamsText.localized(TeamBotMemberText.availabilityKey(model.botsUnverified ? .unverified : card.memberAvailability))
    }

    /// The second line of the header (Desktop `conversationSubtitle`): a Bot DM says whether the Bot is reachable
    /// and whose it is, a dm says what it is, a group or channel how many are in it; a friend's chat names their `@username`.
    /// `nil` when there is nothing to say.
    @MainActor
    var subtitle: String? {
        if let pair { return pair.handle.map { "@" + $0 } }
        switch conversation.kind {
        case "bot_dm":
            guard let card = card(forKey: conversation.effectiveSharedBotId) else { return nil }
            return TeamsStreamLogic.joinedLine([presenceText(card), String(localized: "teams_bot_owner \(userName(card.maintainer.uid))")])
        case "dm":
            return strings.directMessage
        default:
            let members = conversation.participants.filter { $0.hasPrefix("user:") }.count
            let bots = conversation.participants.filter { $0.hasPrefix("shared_bot:") }.count
            let line = TeamsStreamLogic.joinedLine([members > 0 ? TeamsText.membersCount(members) : nil, bots > 0 ? TeamsText.botsCount(bots) : nil])
            return line.isEmpty ? nil : line
        }
    }

    /// The context of an open conversation, or `nil` while the viewer, the team, the model or the conversation is
    /// not there (yet).
    @MainActor
    static func make(workspace: TeamsWorkspace, conversationID: String, held: TeamHeldKeeper? = nil) -> TeamConversationContext? {
        guard let model = workspace.messaging, let uid = workspace.uid, let team = workspace.team,
              let conversation = model.cache?.conversations.first(where: { $0.id == conversationID }) else { return nil }
        let strings = TeamsListStrings.current
        let members = workspace.directory
        let bots = model.bots
        let pair = conversationID == FriendsLogic.chatConversationID ? workspace.pair : nil
        return TeamConversationContext(
            uid: uid, team: team, model: model, conversation: conversation,
            label: pair?.team.name ?? TeamsListLogic.conversationLabel(conversation, members: members, uid: uid, bots: bots, strings: strings),
            members: members, bots: bots, strings: strings, pair: pair, held: held)
    }
}

/// The message a reply answers, as a line: its author and a one-line snippet.
struct TeamMessageQuote: Equatable {
    let author: String
    let text: String
}

// MARK: - Separators

struct StreamDateSeparator: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 12).padding(.vertical, 4)
            .background(Color(uiColor: .secondarySystemBackground), in: Capsule())
            .frame(maxWidth: .infinity)
            .padding(.vertical, 10)
            .accessibilityAddTraits(.isHeader)
    }
}

/// The "New messages" divider before the first unread message written by someone else.
struct StreamUnreadDivider: View {
    var body: some View {
        HStack(spacing: 8) {
            line
            Text("teams_new_messages_divider").font(.caption.weight(.semibold)).foregroundStyle(Color.accentColor)
            line
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("teams.unread.divider")
    }

    private var line: some View { Rectangle().fill(Color.accentColor.opacity(0.5)).frame(height: 1) }
}

// MARK: - Message

/// One message of the stream: the avatar and name on the first message of a run, the time, the reply quote,
/// the text (or the recall notice), the edit mark, and the Bot cards that belong to it. Long-press opens the
/// message actions (reply, edit and recall for the viewer's own text, copy); nothing on a recalled message.
///
/// For VoiceOver the author, time, quote and text are one element with the message actions.
struct TeamMessageCell: View {
    @Environment(\.locale) private var locale
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    @State private var openedData: TeamDataRef?
    @State private var openedReport: TeamReportLink?
    @State private var openedDataset = false
    let message: TeamMessage
    let grouped: Bool
    let context: TeamConversationContext
    let quote: TeamMessageQuote?
    /// The triggers whose run card sits under this message (the newest message of each trigger).
    let onReply: ((TeamMessage) -> Void)?
    let onEdit: ((TeamMessage) -> Void)?
    let onRecall: ((TeamMessage) -> Void)?

    private var recalled: Bool { message.recalled == true }

    var body: some View {
        let identity = context.identity(of: message)
        let name = context.authorName(of: message)
        let own = context.isMine(message)
        let bot = identity.kind == .bot
        HStack(alignment: .top, spacing: 8) {
            if own {
                Spacer(minLength: 40)
            } else if grouped {
                Color.clear.frame(width: AvatarScale.side(32, percent: percent), height: 1)
                    .accessibilityHidden(true)
            } else {
                IdentityAvatar(identity: identity, size: 32)
            }
            VStack(alignment: own ? .trailing : .leading, spacing: 4) {
                if bot {
                    content(identity, name: name)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    content(identity, name: name)
                        .modifier(TeamBubbleSurface(own: own, grouped: grouped))
                }
                cards
            }
            .frame(maxWidth: bot ? 700 : 520, alignment: own ? .trailing : .leading)
            .layoutPriority(1)
            if !own { Spacer(minLength: bot ? 0 : 12) }
        }
        .padding(.horizontal, 12)
        .padding(.top, grouped ? 1 : 8)
        .padding(.bottom, 1)
        .sheet(item: $openedData) { reference in
            TeamDataRecordSheet(reference: reference, conversationId: context.conversation.id, model: context.model)
        }
        .sheet(item: $openedReport) { reference in
            TeamReportSheet(link: reference, conversation: context.conversation.id, model: context.model)
        }
        .sheet(isPresented: $openedDataset) {
            if !recalled, let reference = message.resourceRef, let grant = message.resourceGrant {
                TeamDatasetSheet(reference: reference, grant: grant, conversation: context.conversation.id, model: context.model)
            }
        }
        .onChange(of: recalled) { _, recalled in if recalled { openedDataset = false } }
        .frame(maxWidth: 832)
        .frame(maxWidth: .infinity)
        .contextMenu {
            if !recalled {
                if let onReply { Button("teams_message_reply", systemImage: "arrowshape.turn.up.left") { onReply(message) } }
                if let onEdit { Button("teams_message_edit", systemImage: "pencil") { onEdit(message) } }
                if !message.text.isEmpty { Button("chat_message_copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = message.text } }
                if let onRecall { Button("teams_message_recall", systemImage: "trash", role: .destructive) { onRecall(message) } }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.message.\(message.id)")
    }

    private var edited: Bool { message.editedAt != nil && !recalled }

    /// What the message carries. The author line, the quote, the text and the edit mark are one element for VoiceOver, with the message
    /// actions. The files are buttons (open, take back) and what the Bot read is a line of its own, so they stay elements of their own
    /// beside it, like the Bot cards.
    private func content(_ identity: TeamIdentity, name: String) -> some View {
        let clock = TeamsStreamLogic.formatClock(atMs: message.createdAtMs, locale: locale)
        return Grid(alignment: .leading, verticalSpacing: 3) {
            GridRow { words(identity, name: name, clock: clock) }
            if !message.files.isEmpty {
                GridRow { TeamMessageFilesView(message: message, context: context) }
            }
            if !recalled, let reference = message.dataRef, reference.valid {
                GridRow {
                    Button("teams_data_open") { openedData = reference }
                        .accessibilityIdentifier("teams.message.data.\(message.id)")
                }
            }
            if !recalled, case let .pageVersion(page, version)? = message.resourceRef,
               let url = datasetPageURL(page, version: version) {
                GridRow {
                    Link("teams_dataset_open_page", destination: url)
                        .accessibilityIdentifier("teams.message.page." + message.id)
                }
            }
            if !recalled, let reference = message.resourceRef, message.resourceGrant != nil {
                switch reference {
                case .pageVersion:
                    EmptyView()
                case .dataset:
                    GridRow { datasetButton("teams_dataset_open_live") }
                default:
                    GridRow { datasetButton("teams_dataset_open_fixed") }
                }
            }
            if !recalled, let reference = TeamReportLink(message.text) {
                GridRow { Button("teams_report_open") { openedReport = reference }
                    .accessibilityIdentifier("teams.report.open." + message.id) }
            }
            if !recalled, let read = message.readFiles, !read.isEmpty {
                GridRow {
                    Label { Text("teams_files_read \(read.map(\.name).joined(separator: ", "))") } icon: { Image(systemName: "paperclip") }
                        .font(.caption2).foregroundStyle(.secondary)
                        .accessibilityIdentifier("teams.message.read.\(message.id)")
                }
            }
            if edited || !clock.isEmpty {
                GridRow {
                    HStack(spacing: 5) {
                        if edited { Text("teams_message_edited") }
                        if !clock.isEmpty { Text(verbatim: clock).monospacedDigit() }
                    }
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                    .gridCellAnchor(identity.kind == .bot ? .leading : .trailing)
                }
            }
        }
    }

    private func datasetButton(_ title: LocalizedStringKey) -> some View {
        Button(title) { openedDataset = true }
            .accessibilityIdentifier("teams.message.dataset." + message.id)
    }

    private func datasetPageURL(_ page: String, version: String) -> URL? {
        let origin = LocalAppDatasetAccess.origin ?? URL(string: "https://lingxios.net")!
        guard let target = URL(string: "/chat", relativeTo: origin)?.absoluteURL,
              var parts = URLComponents(url: target, resolvingAgainstBaseURL: false) else { return nil }
        parts.queryItems = [URLQueryItem(name: "datasetPage", value: page), URLQueryItem(name: "datasetVersion", value: version)]
        return parts.url
    }

    private func words(_ identity: TeamIdentity, name: String, clock: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            if !grouped { header(identity, name: name) }
            if let quote { quoteLine(quote) }
            if recalled {
                Text("teams_message_recalled").italic().foregroundStyle(.secondary)
            } else if message.text.isEmpty && !message.files.isEmpty {
                // A message of files alone has no text line.
                EmptyView()
            } else if TeamsBotText.rendersMarkdown(authorKind: message.author.kind, recalled: recalled) {
                // A shared Bot writes Markdown: the chat's renderer, web links only. Images stay inert.
                AIText(markdown: context.text(of: message))
                    .teamBotLinkPolicy()
            } else {
                let body = context.text(of: message)
                if let entities = message.mentionEntities, !entities.isEmpty {
                    Text(mentionedText(body, entities: entities))
                } else {
                    Text(body)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text(verbatim: spokenLabel(identity, name: name, clock: clock)))
        .accessibilityIdentifier("teams.message.body.\(message.id)")
        .modifier(MessageActionsAccessibility(message: message, recalled: recalled, onReply: onReply, onEdit: onEdit, onRecall: onRecall))
    }

    private func mentionedText(_ text: String, entities: [TeamMentionEntity]) -> AttributedString {
        let units = Array(text.utf16)
        let valid = TeamMentionEntity.valid(entities, in: units)
        guard !valid.isEmpty else { return AttributedString(text) }
        var styled = AttributedString(), end = 0
        for entity in valid {
            styled += AttributedString(String(decoding: units[end..<entity.offset], as: UTF16.self))
            end = entity.offset + entity.length
            var mention = AttributedString(String(decoding: units[entity.offset..<end], as: UTF16.self))
            mention.font = .body.weight(.semibold)
            styled += mention
        }
        styled += AttributedString(String(decoding: units[end...], as: UTF16.self))
        return styled
    }

    /// "Ann, Bot, 10:32, Sam, Hello there, Sure, Edited": author, Bot tag, time, the quote, the text, the edit mark.
    private func spokenLabel(_ identity: TeamIdentity, name: String, clock: String) -> String {
        let said = context.text(of: message)
        let text = recalled ? TeamsText.localized("teams_message_recalled")
            : TeamsBotText.rendersMarkdown(authorKind: message.author.kind, recalled: recalled) ? TeamsBotText.plainText(said) : said
        return TeamsStreamLogic.spokenLine([
            name,
            identity.kind == .bot ? context.strings.bot : nil,
            clock,
            quote.map { TeamsStreamLogic.spokenLine([$0.author, $0.text]) },
            text,
            edited ? TeamsText.localized("teams_message_edited") : nil,
        ])
    }

    private func header(_ identity: TeamIdentity, name: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(verbatim: name).font(.subheadline.weight(.semibold)).lineLimit(1)
            if identity.kind == .bot {
                Text(context.strings.bot)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                if let card = context.card(forKey: message.author.sharedBotId) { Text("teams_bot_owner \(context.userName(card.maintainer.uid))").font(.caption2).foregroundStyle(.secondary).lineLimit(1) }
            }
        }
    }

    private func quoteLine(_ quote: TeamMessageQuote) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Rectangle().fill(Color.accentColor).frame(width: 2)
            Text(quoteText(quote))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(Color.accentColor.opacity(0.06), in: RoundedRectangle(cornerRadius: 4))
    }

    private func quoteText(_ quote: TeamMessageQuote) -> AttributedString {
        var result = AttributedString(quote.author.isEmpty ? "" : quote.author + " ")
        result.inlinePresentationIntent = .stronglyEmphasized
        result.append(AttributedString(quote.text))
        return result
    }

    @ViewBuilder
    private var cards: some View {
        if let request = message.approvalRequest, let waitId = message.waitId, let taskId = message.taskId {
            let standing = context.approval(of: message)
            TeamApprovalCard(approval: request, waitId: waitId, taskId: taskId, state: standing.state, deciderName: standing.decider)
                .padding(.top, 4)
        }
        let ids = message.taskId.map { [$0] } ?? message.taskIds ?? []
        ForEach(ids, id: \.self) { taskId in
            if let task = context.model.tasks.first(where: { $0.taskId == taskId }) {
                let bot = context.bots.first { $0.sharedBotId == task.sharedBotId }
                // The reply of a Bot the viewer maintains waits for them, who read it: it is theirs to decide. Everyone else sees the card that says it awaits approval.
                if task.state == "awaiting_approval", let held = context.held, let row = held.list.row(taskId) {
                    TeamHeldCard(row: row, now: held.list.now, botName: bot?.name ?? context.strings.bot, keeper: held)
                        .padding(.top, 4)
                } else {
                    TeamTaskCard(task: task, botName: bot?.name ?? context.strings.bot, viewer: context.viewer, maintainer: bot?.maintainer,
                                 cards: context.bots, tasks: context.model.tasks,
                                 recipientName: task.wait.map { context.userName($0.recipient.uid) } ?? "",
                                 onReply: context.answerableWait(of: message) == nil ? nil : onReply.map { reply in { reply(message) } },
                                 reason: context.model.triageReasons[taskId])
                        .padding(.top, 4)
                }
            } else {
                Button {
                    Task { try? await context.model.refreshTask(taskId) }
                } label: {
                    Text("teams_task_view").minimumHitTarget(minWidth: 0, alignment: .leading)
                }
            }
        }
        if ids.isEmpty, let artifactId = message.artifactId {
            Button {
                Task { try? await context.model.openArtifact(artifactId) }
            } label: {
                Text("teams_artifact_open").minimumHitTarget(minWidth: 0, alignment: .leading)
            }
        }
    }

}

/// The message actions for VoiceOver and Switch Control (the context menu is gesture-only): reply, edit and recall where they apply,
/// and Copy. A recalled message has none of them.
private struct MessageActionsAccessibility: ViewModifier {
    let message: TeamMessage
    let recalled: Bool
    let onReply: ((TeamMessage) -> Void)?
    let onEdit: ((TeamMessage) -> Void)?
    let onRecall: ((TeamMessage) -> Void)?

    func body(content: Content) -> some View {
        if recalled {
            content
        } else {
            content
                .accessibilityHint(Text("teams_message_actions"))
                .accessibilityActions {
                    if let onReply { Button("teams_message_reply") { onReply(message) } }
                    if let onEdit { Button("teams_message_edit") { onEdit(message) } }
                    if !message.text.isEmpty { Button("chat_message_copy") { UIPasteboard.general.string = message.text } }
                    if let onRecall { Button("teams_message_recall") { onRecall(message) } }
                }
        }
    }
}

private struct TeamBubbleSurface: ViewModifier {
    let own: Bool
    let grouped: Bool

    func body(content: Content) -> some View {
        let shape = UnevenRoundedRectangle(
            topLeadingRadius: grouped && !own ? 6 : 16,
            bottomLeadingRadius: 16,
            bottomTrailingRadius: 16,
            topTrailingRadius: grouped && own ? 6 : 16
        )
        content
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background {
                shape.fill(Color(uiColor: .secondarySystemGroupedBackground))
                    .overlay { if own { shape.fill(Color.accentColor.opacity(0.06)) } }
            }
            .overlay { shape.strokeBorder(Color.primary.opacity(0.08)) }
            .contentShape(shape)
    }
}

// MARK: - Unsent commands

/// A message, edit or recall that is saved on this device and not confirmed yet. It keeps its identity
/// through retries; it can be discarded until the server may have seen it.
struct TeamOutboxCell: View {
    let intent: TeamMessageIntent
    let onDiscard: () -> Void

    var body: some View {
        let status = intent.error.map(TeamsErrors.key) ?? (intent.attempted == true ? "teams_message_confirming" : "teams_message_pending")
        let inFlight = (intent.attempted == true && intent.error == nil) || (intent.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000
        HStack {
            Spacer(minLength: 40)
            VStack(alignment: .leading, spacing: 3) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: TeamsText.you).font(.subheadline.weight(.semibold))
                    Text(LocalizedStringKey(status)).font(.caption).foregroundStyle(intent.error == nil ? Color.secondary : Color.orange)
                    switch intent.operation {
                    case "recall": Text("teams_message_recall").foregroundStyle(.secondary)
                    case "edit": Text(String(localized: "teams_message_edit") + ": " + intent.text).foregroundStyle(.secondary)
                    default:
                        if !intent.text.isEmpty { Text(intent.text).foregroundStyle(.secondary) }
                        if let names = intent.attachmentNames, !names.isEmpty {
                            Label(names.joined(separator: ", "), systemImage: "paperclip").font(.footnote).foregroundStyle(.secondary)
                        }
                    }
                }
                .accessibilityElement(children: .combine)
                if intent.error == "MENTION_TARGET_REQUIRED", !intent.text.isEmpty {
                    Button("chat_message_copy") { UIPasteboard.general.string = intent.text }
                        .font(.footnote).minimumHitTarget(minWidth: 0, alignment: .leading)
                }
                Button(role: .destructive, action: onDiscard) {
                    Text("teams_message_discard").font(.footnote).minimumHitTarget(minWidth: 0, alignment: .leading)
                }
                .disabled(inFlight)
            }
            .modifier(TeamBubbleSurface(own: true, grouped: false))
            .frame(maxWidth: 520, alignment: .trailing)
            .layoutPriority(1)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 1)
        .frame(maxWidth: 832)
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.outbox.\(intent.id)")
    }
}
private struct TeamDataRecordSheet: View {
    private struct Save: Identifiable {
        let id = UUID()
        let text: String
        let operation: String
    }

    @Environment(\.dismiss) private var dismiss
    @State private var savedReference: TeamDataRef?
    @State private var content = ""
    @State private var fields: [String: String] = [:]
    @State private var views: [TeamDataTemplateView] = []
    @State private var selectedView = ""
    @State private var editable = false
    @State private var editing = false
    @State private var loaded = false
    @State private var error: String?
    @State private var save: Save?
    @State private var lastAttempt: Save?
    @State private var conflict = false
    @State private var reloading: UUID?
    let reference: TeamDataRef
    let conversationId: String
    let model: TeamMessagingModel

    private var currentReference: TeamDataRef { savedReference ?? reference }

    var body: some View {
        NavigationStack {
            Form {
                if let error { Text(error).foregroundStyle(.red) }
                if conflict {
                    Button("teams_data_latest") { reloading = UUID() }.disabled(reloading != nil)
                }
                if loaded {
                    if editing {
                        TextEditor(text: $content)
                            .font(.system(.body, design: .monospaced))
                            .frame(minHeight: 160)
                            .disabled(save != nil)
                            .accessibilityLabel("teams_data_json")
                            .accessibilityIdentifier("teams.data.editor")
                    } else {
                        if !views.isEmpty {
                            Picker("teams_data_view", selection: $selectedView) {
                                Text("teams_data_json").tag("")
                                ForEach(views, id: \.view) { view in Text(verbatim: view.view).tag(view.view) }
                            }
                            .accessibilityIdentifier("teams.data.view")
                        }
                        if let view = views.first(where: { $0.view == selectedView }) {
                            ForEach(view.definition.nodes.indices, id: \.self) { index in
                                TeamDataTemplateNodeView(node: view.definition.nodes[index], fields: fields, depth: 0)
                            }
                        } else {
                            Text(verbatim: content).font(.system(.body, design: .monospaced)).textSelection(.enabled)
                        }
                    }
                } else if error == nil {
                    ProgressView("teams_data_loading")
                }
            }
            .navigationTitle(currentReference.record)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close") { dismiss() }.accessibilityIdentifier("teams.data.close")
                }
                if editable && loaded {
                    ToolbarItem(placement: .confirmationAction) {
                        Button(editing ? "teams_data_save" : "teams_message_edit") { activate() }.disabled(save != nil)
                            .accessibilityIdentifier("teams.data.save")
                    }
                }
            }
            .task(id: currentReference) { await load() }
            .task(id: save?.id) { if let save { await write(save) } }
            .task(id: reloading) { if reloading != nil { await reload() } }
        }
    }

    private func activate() {
        guard editing else { editing = true; return }
        let operation = lastAttempt?.text == content ? lastAttempt?.operation : nil
        let intent = Save(text: content, operation: operation ?? UUID().uuidString)
        lastAttempt = intent
        save = intent
    }

    private func load() async {
        loaded = false
        content = ""
        fields = [:]; views = []; selectedView = ""
        do {
            let record = try await model.dataRecord(conversationId: conversationId, reference: currentReference)
            try Task.checkCancellation()
            content = record.value; fields = record.fields; views = record.views
            editable = record.editable; loaded = true; error = nil
        } catch where !Task.isCancelled {
            self.error = String(localized: "teams_data_unavailable")
        } catch {}
    }

    private func write(_ intent: Save) async {
        defer { save = nil }
        guard let value = (try? JSONSerialization.jsonObject(with: Data(intent.text.utf8))) as? [String: Any] else {
            error = String(localized: "teams_data_invalid")
            return
        }
        conflict = false
        do {
            let written = try await model.writeDataRecord(conversationId: conversationId, reference: currentReference,
                                                          value: value, operation: intent.operation)
            try Task.checkCancellation()
            savedReference = written; editing = false; error = nil; lastAttempt = nil
        } catch where !Task.isCancelled {
            showFailure(error)
        } catch {}
    }

    private func reload() async {
        defer { reloading = nil }
        do {
            let latest = try await model.latestDataRecord(conversationId: conversationId, reference: currentReference)
            try Task.checkCancellation()
            guard latest != currentReference else { return }
            conflict = false; editing = false; lastAttempt = nil; error = nil
            savedReference = latest
        } catch where !Task.isCancelled {
            showFailure(error)
        } catch {}
    }

    private func showFailure(_ failure: Error) {
        let code = (failure as? TeamMessageFailure)?.code ?? ""
        if ["DATA_ACCESS_REQUIRED", "INVALID_SESSION", "CONVERSATION_REQUIRED", "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP"].contains(code) {
            loaded = false; editable = false; editing = false; lastAttempt = nil
            content = ""; fields = [:]; views = []; selectedView = ""
        }
        conflict = code == "DATA_CONFLICT"
        error = conflict
            ? String(localized: "teams_data_conflict")
            : String(localized: "teams_data_unavailable")
    }
}

private struct TeamDataTemplateNodeView: View {
    let node: TeamDataTemplateView.Node
    let fields: [String: String]
    let depth: Int
    var emphasis = ""

    var body: some View {
        if depth <= 10 {
            switch node.kind {
            case "text":
                label(node.text ?? "")
            case "field":
                label(fields[node.field ?? ""] ?? "—")
            case "element":
                if let tag = node.tag, let children = node.children,
                   ["section", "div", "h3", "p", "span", "strong", "small", "ul", "li"].contains(tag) {
                    let emphasis = tag == "h3" || tag == "strong" ? tag : emphasis
                    let horizontal = node.style?["display"] == "flex" && node.style?["flexDirection"] != "column"
                    let layout = horizontal ? AnyLayout(HStackLayout()) : AnyLayout(VStackLayout(alignment: .leading))
                    layout {
                        ForEach(children.indices, id: \.self) { index in
                            TeamDataTemplateNodeView(node: children[index], fields: fields, depth: depth + 1, emphasis: emphasis)
                        }
                    }
                }
            default:
                EmptyView()
            }
        }
    }

    private func label(_ text: String) -> some View {
        Text(verbatim: text)
            .font(emphasis == "h3" ? .headline : .body)
            .fontWeight(emphasis == "strong" ? .bold : nil)
            .textSelection(.enabled)
    }
}

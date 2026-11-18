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
    static func make(workspace: TeamsWorkspace, conversationID: String) -> TeamConversationContext? {
        guard let model = workspace.messaging, let uid = workspace.uid, let team = workspace.team,
              let conversation = model.cache?.conversations.first(where: { $0.id == conversationID }) else { return nil }
        let strings = TeamsListStrings.current
        let members = workspace.directory
        let bots = model.bots
        let pair = conversationID == FriendsLogic.chatConversationID ? workspace.pair : nil
        return TeamConversationContext(
            uid: uid, team: team, model: model, conversation: conversation,
            label: pair?.team.name ?? TeamsListLogic.conversationLabel(conversation, members: members, uid: uid, bots: bots, strings: strings),
            members: members, bots: bots, strings: strings, pair: pair)
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
                Text(context.text(of: message))
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text(verbatim: spokenLabel(identity, name: name, clock: clock)))
        .accessibilityIdentifier("teams.message.body.\(message.id)")
        .modifier(MessageActionsAccessibility(message: message, recalled: recalled, onReply: onReply, onEdit: onEdit, onRecall: onRecall))
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
                TeamTaskCard(task: task, botName: bot?.name ?? context.strings.bot, viewer: context.viewer, maintainer: bot?.maintainer,
                             recipientName: task.wait.map { context.userName($0.recipient.uid) } ?? "",
                             onReply: context.answerableWait(of: message) == nil ? nil : onReply.map { reply in { reply(message) } })
                    .padding(.top, 4)
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

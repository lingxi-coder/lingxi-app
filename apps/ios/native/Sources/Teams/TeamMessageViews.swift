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

    /// Whether a Bot can be reached now: `teams_bot_disabled`, `remote_online` or `teams_bot_offline_queued`.
    func presenceKey(_ card: TeamBotCard) -> String {
        card.state != "published" ? "teams_bot_disabled" : card.ready ? "remote_online" : "teams_bot_offline_queued"
    }

    /// The second line of the header (Desktop `conversationSubtitle`): a Bot DM says whether the Bot is reachable
    /// and whose it is, a dm says what it is, a group or channel how many are in it. `nil` when there is nothing to say.
    var subtitle: String? {
        switch conversation.kind {
        case "bot_dm":
            guard let card = card(forKey: conversation.effectiveSharedBotId) else { return nil }
            return String(localized: String.LocalizationValue(presenceKey(card))) + " · " + String(localized: "teams_bot_owner \(userName(card.maintainer.uid))")
        case "dm":
            return strings.directMessage
        default:
            return conversation.participants.isEmpty ? nil : TeamsText.membersCount(conversation.participants.count)
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
        return TeamConversationContext(
            uid: uid, team: team, model: model, conversation: conversation,
            label: TeamsListLogic.conversationLabel(conversation, members: members, uid: uid, bots: bots, strings: strings),
            members: members, bots: bots, strings: strings)
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
struct TeamMessageCell: View {
    @Environment(\.locale) private var locale
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
        HStack(alignment: .top, spacing: 10) {
            if grouped {
                Color.clear.frame(width: 32, height: 1)
            } else {
                IdentityAvatar(identity: identity, size: 32)
            }
            VStack(alignment: .leading, spacing: 3) {
                if !grouped { header(identity) }
                if let quote { quoteLine(quote) }
                if recalled {
                    Text("teams_message_recalled").italic().foregroundStyle(.secondary)
                } else {
                    Text(message.text)
                }
                if message.editedAt != nil && !recalled {
                    Text("teams_message_edited").font(.caption2).foregroundStyle(.secondary)
                }
                cards
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 16)
        .padding(.top, grouped ? 1 : 10)
        .padding(.bottom, 1)
        .contentShape(Rectangle())
        .contextMenu {
            if !recalled {
                if let onReply { Button("teams_message_reply", systemImage: "arrowshape.turn.up.left") { onReply(message) } }
                if let onEdit { Button("teams_message_edit", systemImage: "pencil") { onEdit(message) } }
                Button("chat_message_copy", systemImage: "doc.on.doc") { UIPasteboard.general.string = message.text }
                if let onRecall { Button("teams_message_recall", systemImage: "trash", role: .destructive) { onRecall(message) } }
            }
        }
        .modifier(MessageActionsAccessibility(message: message, onReply: recalled ? nil : onReply, onEdit: recalled ? nil : onEdit, onRecall: recalled ? nil : onRecall))
        .accessibilityIdentifier("teams.message.\(message.id)")
    }

    private func header(_ identity: TeamIdentity) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(identity.name).font(.subheadline.weight(.semibold)).lineLimit(1)
            if identity.kind == .bot {
                Text(context.strings.bot)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                if let card = context.card(forKey: message.author.sharedBotId) { Text("teams_bot_owner \(context.userName(card.maintainer.uid))").font(.caption2).foregroundStyle(.secondary).lineLimit(1) }
            }
            let clock = TeamsStreamLogic.formatClock(atMs: message.createdAtMs, locale: locale)
            if !clock.isEmpty { Text(clock).font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func quoteLine(_ quote: TeamMessageQuote) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Rectangle().fill(Color.secondary.opacity(0.45)).frame(width: 2)
            Text(quoteText(quote))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    private func quoteText(_ quote: TeamMessageQuote) -> AttributedString {
        var result = AttributedString(quote.author.isEmpty ? "" : quote.author + " ")
        result.inlinePresentationIntent = .stronglyEmphasized
        result.append(AttributedString(quote.text))
        return result
    }

    @ViewBuilder
    private var cards: some View {
        let ids = message.taskId.map { [$0] } ?? message.taskIds ?? []
        ForEach(ids, id: \.self) { taskId in
            if let task = context.model.tasks.first(where: { $0.taskId == taskId }) {
                TeamTaskCard(task: task, botName: context.bots.first { $0.sharedBotId == task.sharedBotId }?.name ?? context.strings.bot)
                    .padding(.top, 4)
            } else {
                Button("teams_task_view") { Task { try? await context.model.refreshTask(taskId) } }
            }
        }
        if ids.isEmpty, let artifactId = message.artifactId {
            Button("teams_artifact_open") { Task { try? await context.model.openArtifact(artifactId) } }
        }
    }

}

/// The message actions for VoiceOver and Switch Control (the context menu is gesture-only).
private struct MessageActionsAccessibility: ViewModifier {
    let message: TeamMessage
    let onReply: ((TeamMessage) -> Void)?
    let onEdit: ((TeamMessage) -> Void)?
    let onRecall: ((TeamMessage) -> Void)?

    func body(content: Content) -> some View {
        content
            .accessibilityElement(children: .contain)
            .accessibilityHint(Text("teams_message_actions"))
            .accessibilityActions {
                if let onReply { Button("teams_message_reply") { onReply(message) } }
                if let onEdit { Button("teams_message_edit") { onEdit(message) } }
                if let onRecall { Button("teams_message_recall") { onRecall(message) } }
            }
    }
}

// MARK: - Unsent commands

/// A message, edit or recall that is saved on this device and not confirmed yet. It keeps its identity
/// through retries; it can be discarded until the server may have seen it.
struct TeamOutboxCell: View {
    let intent: TeamMessageIntent
    let context: TeamConversationContext
    let onDiscard: () -> Void

    var body: some View {
        let identity = TeamIdentity(kind: .user, id: context.uid, name: context.userName(context.uid), isSelf: true)
        let status = intent.error.map(TeamsErrors.key) ?? (intent.attempted == true ? "teams_message_confirming" : "teams_message_pending")
        let inFlight = (intent.attempted == true && intent.error == nil) || (intent.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000
        HStack(alignment: .top, spacing: 10) {
            IdentityAvatar(identity: identity, size: 32)
            VStack(alignment: .leading, spacing: 3) {
                Text(identity.name).font(.subheadline.weight(.semibold))
                Text(LocalizedStringKey(status)).font(.caption).foregroundStyle(intent.error == nil ? Color.secondary : Color.orange)
                switch intent.operation {
                case "recall": Text("teams_message_recall").foregroundStyle(.secondary)
                case "edit": Text(String(localized: "teams_message_edit") + ": " + intent.text).foregroundStyle(.secondary)
                default: Text(intent.text).foregroundStyle(.secondary)
                }
                Button("teams_message_discard", role: .destructive, action: onDiscard)
                    .font(.caption)
                    .disabled(inFlight)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 16)
        .padding(.top, 10)
        .opacity(0.9)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.outbox.\(intent.id)")
    }
}

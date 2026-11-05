import SwiftUI

/// The details of one conversation (spec §6.3): who is in it, and for a Bot DM whose Bot it is and who can read
/// it. A sheet over the conversation, opened from the info button in its header.
struct TeamConversationDetailsSheet: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.dismiss) private var dismiss
    let conversationID: String
    /// In the wide layout the details are a side pane: Close replaces Done and calls this instead of dismissing a sheet.
    var onClose: (() -> Void)?

    var body: some View {
        NavigationStack {
            Group {
                if let context = TeamConversationContext.make(workspace: workspace, conversationID: conversationID) {
                    details(context)
                } else {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .navigationTitle("Conversation details") // TODO(strings): teams_conversation_details
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(onClose == nil ? "common_done" : "common_close") { if let onClose { onClose() } else { dismiss() } }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .accessibilityIdentifier("teams.conversation.details")
    }

    /// One participant: a person (with their team role) or a Bot (with its card, when it is known).
    private struct Participant: Identifiable {
        let id: String
        let identity: TeamIdentity
        let card: TeamBotCard?
        let role: String?
    }

    /// Everyone in the conversation in the order of `TeamsListLogic.conversationParticipants`: other people, Bots,
    /// then the viewer. A channel without a participant list is open to the whole team.
    private func participants(_ context: TeamConversationContext) -> [Participant] {
        let conversation = context.conversation
        let identities = TeamsListLogic.conversationParticipants(conversation, members: context.members, bots: context.bots, uid: context.uid, strings: context.strings)
        let role = { (uid: String) in context.members.first { $0.uid == uid }?.role }
        if identities.isEmpty && conversation.kind == "channel" {
            return context.members.map { member in
                let name = member.displayName.isEmpty ? context.strings.member : member.displayName
                return Participant(id: "user:" + member.uid, identity: TeamIdentity(kind: .user, id: member.uid, name: name, isSelf: member.uid == context.uid), card: nil, role: member.role)
            }
        }
        let botKeys = conversation.participants.filter { $0.hasPrefix("bot:") }
        var nextBot = 0
        var result: [Participant] = []
        for identity in identities {
            if identity.kind == .bot {
                let key = nextBot < botKeys.count ? botKeys[nextBot] : identity.id
                nextBot += 1
                result.append(Participant(id: key, identity: identity, card: context.card(forKey: key), role: nil))
            } else {
                result.append(Participant(id: "user:" + identity.id, identity: identity, card: nil, role: role(identity.id)))
            }
        }
        return result
    }

    private func details(_ context: TeamConversationContext) -> some View {
        let conversation = context.conversation
        let model = context.model
        let people = participants(context)
        let unread = model.unread(of: conversation)
        let participantsForLead = TeamsListLogic.conversationParticipants(conversation, members: context.members, bots: context.bots, uid: context.uid, strings: context.strings)
        return List {
            Section {
                HStack(spacing: 14) {
                    ConversationLead(conversation: conversation, label: context.label, participants: participantsForLead, size: 56)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(context.label).font(.title3.weight(.semibold))
                        if let subtitle = context.subtitle { Text(subtitle).font(.subheadline).foregroundStyle(.secondary) }
                    }
                }
                .padding(.vertical, 4)
                .accessibilityElement(children: .combine)
            }
            if conversation.kind == "bot_dm" {
                Section { TeamsNote(text: "teams_composer_note_bot_dm", systemImage: "eye") }
            } else if people.contains(where: { $0.identity.kind == .bot }) {
                Section { TeamsNote(text: "teams_new_bot_owner_note", systemImage: "eye") }
            }
            if !people.isEmpty {
                Section {
                    ForEach(people) { person in
                        row(person, context)
                    }
                } header: {
                    Text("teams_members")
                }
            }
            if unread > 0 {
                Section {
                    Button("teams_mark_read", systemImage: "checkmark.circle") {
                        Task { await workspace.markRead(conversation.id) }
                    }
                    .disabled(model.offline || workspace.messageBusy)
                    .accessibilityIdentifier("teams.conversation.details.markread")
                }
            }
        }
    }

    private func row(_ person: Participant, _ context: TeamConversationContext) -> some View {
        let identity = person.identity
        return HStack(spacing: 12) {
            IdentityAvatar(identity: identity, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(identity.name).font(.body.weight(.semibold)).lineLimit(1)
                    if identity.kind == .bot {
                        Text(context.strings.bot)
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                    }
                }
                if let card = person.card {
                    Text("teams_bot_owner \(context.userName(card.ownerUid))").font(.caption).foregroundStyle(.secondary)
                    Text(LocalizedStringKey(context.presenceKey(card))).font(.caption).foregroundStyle(card.enabled && card.online ? Color.green : Color.secondary)
                } else if let role = person.role {
                    HStack(spacing: 4) {
                        if identity.isSelf { Text(context.strings.you); Text("·") }
                        Text(TeamsText.role(role))
                    }
                    .font(.caption).foregroundStyle(.secondary)
                } else if identity.isSelf {
                    Text(context.strings.you).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("teams.participant.\(person.id)")
    }
}

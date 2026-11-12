import SwiftUI

/// The faces of the Teams UI (spec §7): a person is initials on a colour taken from their id, a Bot is the
/// shipped art chosen by its `botId` (`AgentAvatar`), a channel a `#` tile, a team the initial of its name. One
/// person or Bot looks the same on every screen and on Desktop. All of them are decorative: the name next
/// to the avatar carries the meaning, so VoiceOver skips them.

extension Color {
    /// The tile colour of a person or team id (`AvatarIdentity.avatarColors`, white initials on each).
    init(avatarID id: String) {
        let rgb = AvatarIdentity.avatarColorComponents(id)
        self.init(.sRGB, red: rgb.red, green: rgb.green, blue: rgb.blue, opacity: 1)
    }
}

/// A person: initials on a colour from the uid. The faces of this file take a base size of the avatar scale (20 / 24 / 32 / 40 / 56) and
/// grow with Dynamic Type up to 1.5 times it (`AvatarScale`); their initials follow the face, not a text style.
struct PersonAvatar: View {
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    let id: String
    let name: String
    var size: CGFloat = 40

    var body: some View {
        let side = AvatarScale.side(size, percent: percent)
        Text(AvatarIdentity.initials(name))
            .font(.system(size: side * 0.4, weight: .semibold))
            .foregroundStyle(.white)
            .lineLimit(1)
            .minimumScaleFactor(0.5)
            .frame(width: side, height: side)
            .background(Color(avatarID: id), in: Circle())
            .accessibilityHidden(true)
    }
}

/// A channel: a rounded square with a `#`.
struct ChannelAvatar: View {
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    var size: CGFloat = 40

    var body: some View {
        let side = AvatarScale.side(size, percent: percent)
        Image(systemName: "number")
            .font(.system(size: side * 0.45, weight: .semibold))
            .foregroundStyle(.secondary)
            .frame(width: side, height: side)
            .background(Color(uiColor: .tertiarySystemFill), in: RoundedRectangle(cornerRadius: AvatarScale.cornerRadius(side: side), style: .continuous))
            .accessibilityHidden(true)
    }
}

/// A team: the initial of its name on a rounded square coloured from the team id.
struct TeamAvatar: View {
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    let teamID: String
    let name: String
    var size: CGFloat = 40

    var body: some View {
        let side = AvatarScale.side(size, percent: percent)
        Text(AvatarIdentity.teamInitial(name))
            .font(.system(size: side * 0.5, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: side, height: side)
            .background(Color(avatarID: teamID), in: RoundedRectangle(cornerRadius: AvatarScale.cornerRadius(side: side), style: .continuous))
            .accessibilityHidden(true)
    }
}

/// A person or a Bot from a `TeamIdentity`. A Bot is drawn from its `botId`; when only its key is known the id
/// is the text after the key's last colon (`AvatarIdentity.teamBotAvatarId`). A person is a circle, a Bot a rounded square (U2).
struct IdentityAvatar: View {
    let identity: TeamIdentity
    var size: CGFloat = 40

    var body: some View {
        switch identity.kind {
        case .user: PersonAvatar(id: identity.id, name: identity.name, size: size)
        case .bot: AgentAvatar(agentID: AvatarIdentity.teamBotAvatarId(identity.id), size: size)
        }
    }
}

/// Up to `maxCount` avatars overlapped, and a "+N" for the rest (the lead of a group). The faces scale themselves (`size` is their
/// base); the stack's own geometry scales by the same rule.
struct AvatarStack: View {
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    let identities: [TeamIdentity]
    var maxCount = 2
    var size: CGFloat = 40

    var body: some View {
        let shown = Array(identities.prefix(maxCount))
        let rest = identities.count - shown.count
        let side = AvatarScale.side(size, percent: percent)
        let step = side * 0.62
        ZStack(alignment: .leading) {
            ForEach(Array(shown.enumerated()), id: \.offset) { index, identity in
                IdentityAvatar(identity: identity, size: size)
                    .overlay { ring(identity, side: side) }
                    .offset(x: CGFloat(index) * step)
            }
            if rest > 0 {
                Text(verbatim: "+\(rest)")
                    .font(.system(size: side * 0.34, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .offset(x: CGFloat(shown.count) * step + side * 0.18)
            }
        }
        .frame(width: side + CGFloat(max(shown.count - 1, 0)) * step + (rest > 0 ? side * 0.7 : 0), height: side, alignment: .leading)
        .accessibilityHidden(true)
    }

    /// The gap between two overlapped faces follows the shape of the face on top: a circle for a person, the rounded square for a Bot.
    @ViewBuilder
    private func ring(_ identity: TeamIdentity, side: CGFloat) -> some View {
        switch identity.kind {
        case .user:
            Circle().stroke(Color(uiColor: .systemBackground), lineWidth: 1.5)
        case .bot:
            RoundedRectangle(cornerRadius: AvatarScale.cornerRadius(side: side), style: .continuous)
                .stroke(Color(uiColor: .systemBackground), lineWidth: 1.5)
        }
    }
}

/// The lead of a conversation row or header (spec §7.4): a channel is a `#` tile; a dm is the other person (the
/// viewer when alone); a Bot DM is the Bot; a group is a stack of the other people one step down the avatar scale (32 in a profile header, 24 in a
/// row, 20 in a conversation header).
struct ConversationLead: View {
    @ScaledMetric(relativeTo: .body) private var percent: CGFloat = 100
    let conversation: TeamMessageConversation
    let label: String
    /// `TeamsListLogic.conversationParticipants`: other people, then Bots, then the viewer.
    let participants: [TeamIdentity]
    var size: CGFloat = 40

    var body: some View {
        switch conversation.kind {
        case "channel":
            ChannelAvatar(size: size)
        case "bot_dm":
            IdentityAvatar(identity: participants.first { $0.kind == .bot } ?? fallback, size: size)
        case "group":
            let others = participants.filter { !$0.isSelf }
            let side = AvatarScale.side(size, percent: percent)
            AvatarStack(identities: others.isEmpty ? (participants.isEmpty ? [fallback] : participants) : others, maxCount: 2, size: size >= 56 ? 32 : size >= 40 ? 24 : 20)
                .frame(width: side, height: side, alignment: .leading)
        default:
            IdentityAvatar(identity: participants.first { !$0.isSelf && $0.kind == .user } ?? participants.first { $0.isSelf } ?? fallback, size: size)
        }
    }

    /// `{ kind: bot_dm ? bot : user, id: conversation.id, name: label }`.
    private var fallback: TeamIdentity {
        TeamIdentity(kind: conversation.kind == "bot_dm" ? .bot : .user, id: conversation.id, name: label, isSelf: false)
    }
}

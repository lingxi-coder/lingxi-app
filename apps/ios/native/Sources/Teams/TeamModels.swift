import Foundation

// The value types of the Teams HTTP API (`GET /v1/teams`, `GET /v1/teams/{id}`). Foundation only, so the
// pure Teams logic (labels, previews) and its macOS tests can use them without the Firebase-bound workspace.

struct TeamSummary: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let memberCount: Int
    let role: String
    let generation: Int
}

struct TeamMember: Decodable, Identifiable, Equatable, Sendable {
    let uid: String
    let displayName: String
    let role: String
    let generation: Int
    var id: String { uid }
}

struct TeamInvite: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let role: String
    let expiresAt: String
    let uses: Int
    let maxUses: Int
    let revoked: Bool
}

struct TeamDetail: Decodable, Equatable, Sendable {
    let team: TeamSummary
    let members: [TeamMember]
    let invites: [TeamInvite]
}

/// The one team of a friend chat (docs/FRIENDS-CLIENTS.md D9): the summary synthesized from a friend's card and the two
/// people it is made of, so a `TeamsWorkspace` pinned to it needs no `GET /v1/teams` and no team-detail call.
struct PairTeam: Equatable, Sendable {
    let team: TeamSummary
    /// The friend, then the viewer once their profile name is known.
    let members: [TeamMember]
    let friendUID: String
    /// The friend's `@username`, when they have one.
    let handle: String?
}

/// What a role may do in a team (Desktop `TeamDetailsDrawer`: `drawerTabs`, `canManageMember`). The server
/// decides; these rules only decide what is offered.
enum TeamRoles {
    /// Owners manage everyone but themselves and other owners; admins manage plain members.
    static func canManage(viewerRole: String, viewerUID: String, memberUID: String, memberRole: String) -> Bool {
        memberUID != viewerUID && memberRole != "owner" && (viewerRole == "owner" || (viewerRole == "admin" && memberRole == "member"))
    }

    /// Plain members cannot create or revoke invitations, so they have no Invites tab.
    static func canInvite(role: String) -> Bool { role != "member" }

    /// Only an owner revokes an invitation for admins.
    static func canRevoke(viewerRole: String, inviteRole: String) -> Bool { viewerRole == "owner" || inviteRole != "admin" }
}

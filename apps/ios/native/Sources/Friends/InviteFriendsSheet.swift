import Observation
import SwiftUI

/// The state of one Invite friends sheet (docs/FRIENDS-CLIENTS.md D26, D30): who was invited from it, which invitations are running,
/// and what failed under each row. A sheet owns one with `@State`. Nothing is stored anywhere (the sender cannot read offers), so
/// opening the sheet again starts empty and lists the person again: inviting again refreshes the same offer.
@MainActor @Observable
final class FriendInvites {
    /// A friend invited from this sheet, with the name the row showed.
    struct Invited: Equatable {
        let uid: String
        let name: String
    }

    /// Who was invited from this sheet, in the order of the invitations; their rows say `friends_invite_sent`.
    private(set) var invited: [Invited] = []
    /// Friends with an invitation in flight (their row is disabled).
    private(set) var busy: Set<String> = []
    /// The code of the last failed invitation, under the friend's row.
    private(set) var failures: [String: String] = [:]
    /// People the server says are in the team already: their rows are dropped here (D30), and the member list refreshes.
    private(set) var members: Set<String> = []
    /// The `requestId` of each invitation whose outcome is unknown, by friend (D26, D30): minted at the tap, reused only by the retry
    /// after an unknown outcome, dropped by any verdict, never written anywhere.
    @ObservationIgnored private var attempts: [String: String] = [:]

    func isInvited(_ uid: String) -> Bool { invited.contains { $0.uid == uid } }

    /// `offerTeam` for one friend: `nil` when the invitation was sent, else the failure code (also kept under the row).
    @discardableResult
    func invite(_ card: FriendCard, to teamID: String, in friends: FriendsWorkspace) async -> String? {
        guard !busy.contains(card.uid), !isInvited(card.uid) else { return nil }
        let uid = card.uid
        let requestID = attempts[uid] ?? FriendsLogic.newRequestID()
        attempts[uid] = requestID
        busy.insert(uid); failures[uid] = nil
        defer { busy.remove(uid) }
        if let code = await friends.offerTeam(teamID, to: uid, requestID: requestID) {
            failures[uid] = code
            if FriendsLogic.isDefinitive(code) { attempts[uid] = nil }
            if code == "ALREADY_MEMBER" { members.insert(uid) }
            return code
        }
        attempts[uid] = nil
        invited.append(Invited(uid: uid, name: card.displayName))
        return nil
    }
}

/// Invite friends (docs/FRIENDS-CLIENTS.md D26), a sheet over Team details for an owner or an admin: the friend cards that are not in
/// the team yet, in the Friends order and narrowed by the filter, each with Send. The cards are the app-wide Friends workspace's (no
/// listener of its own); the members are the ones the Teams view already holds. An invitation is always for the role `member`, carries
/// no message and grants nothing until the friend joins. When the sheet closes, `onInvited` hands over the names that were invited,
/// for the one-shot `friends_invite_sent_to` under the button.
struct InviteFriendsSheet: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.dismiss) private var dismiss
    @Environment(\.locale) private var locale
    let teamID: String
    let teamName: String
    let onInvited: ([String]) -> Void

    @State private var invites = FriendInvites()
    @State private var query = ""

    var body: some View {
        NavigationStack {
            list
                .navigationTitle(String(localized: "friends_invite_title \(teamName)"))
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("common_done") { dismiss() } } }
                .searchable(text: $query, prompt: Text("friends_filter_placeholder"))
        }
        .presentationDetents([.large])
        // A new account forgets what was open (D19).
        .onChange(of: friends.authority) { _, _ in dismiss() }
        .onDisappear { if !invites.invited.isEmpty { onInvited(invites.invited.map(\.name)) } }
        .accessibilityIdentifier("friends.invite")
    }

    /// The team's people as the Teams view holds them; unknown while another team is the selected one.
    private var memberUIDs: Set<String> {
        let held = workspace.selectedTeamID == teamID ? Set(workspace.directory.map(\.uid)) : []
        return held.union(invites.members)
    }

    @ViewBuilder
    private var list: some View {
        let rows = FriendsLogic.invitePicker(friends.list, memberUIDs: memberUIDs, query: query, locale: locale)
        List {
            if friends.loading {
                HStack { Spacer(); ProgressView(); Spacer() }
                    .listRowSeparator(.hidden)
                    .accessibilityLabel(Text("friends_loading"))
                    .accessibilityIdentifier("friends.invite.loading")
            } else if friends.cardsFailed && friends.list.isEmpty {
                TeamsStateView(systemImage: "exclamationmark.triangle", title: "friends_title", message: "friends_load_error") {
                    Button("common_retry") { friends.reload() }
                        .buttonStyle(.borderedProminent)
                }
                .listRowSeparator(.hidden).listRowBackground(Color.clear)
                .accessibilityIdentifier("friends.invite.failed")
            } else {
                Section {
                    if rows.isEmpty { emptyState }
                    ForEach(rows) { card in
                        InviteFriendRow(
                            card: card, sent: invites.isInvited(card.uid), busy: invites.busy.contains(card.uid), failure: invites.failures[card.uid],
                            onSend: { send(card) })
                    }
                } footer: {
                    Text("friends_invite_disclosure")
                }
            }
        }
        .listStyle(.insetGrouped)
    }

    /// Nothing to list: no friend to invite at all, or no friend that matches the filter.
    private var emptyState: some View {
        let filtering = !JSText.trim(query).isEmpty
        return TeamsStateView(
            systemImage: filtering ? "magnifyingglass" : "person.2",
            title: filtering ? "friends_filter_empty" : "friends_invite_empty")
            .listRowSeparator(.hidden).listRowBackground(Color.clear)
            .accessibilityIdentifier(filtering ? "friends.invite.nomatch" : "friends.invite.empty")
    }

    private func send(_ card: FriendCard) {
        Task {
            // A person who joined meanwhile leaves the list, and the member list is read again (D30).
            if await invites.invite(card, to: teamID, in: friends) == "ALREADY_MEMBER" { workspace.scheduleRefresh() }
        }
    }
}

/// One friend that can be invited: the face, the name, the `@username` and Send, which turns into the disabled `friends_invite_sent`
/// once it went through. A failure shows under the row (D30). The row is disabled while its invitation runs.
private struct InviteFriendRow: View {
    let card: FriendCard
    let sent: Bool
    let busy: Bool
    /// The code of the last failed invitation of this friend.
    let failure: String?
    let onSend: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                PersonAvatar(id: card.uid, name: card.displayName, size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: card.displayName).font(.body.weight(.semibold)).lineLimit(1)
                    if let handle = card.handle { Text(verbatim: "@" + handle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                }
                Spacer(minLength: 8)
                Button(action: onSend) {
                    if busy { ProgressView() } else { Text(LocalizedStringKey(sent ? "friends_invite_sent" : "friends_invite_send")) }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(sent || busy)
                .accessibilityIdentifier("friends.invite.send.\(card.uid)")
            }
            if let failure {
                Text(LocalizedStringKey(FriendsLogic.offerErrorKey(.offerTeam, failure))).font(.footnote).foregroundStyle(.red)
                    .accessibilityIdentifier("friends.invite.error")
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("friends.invite.row.\(card.uid)")
    }
}

import SwiftUI

/// The short message the workspace keeps once (`friends_now_friends`, `friends_chat_unavailable`): a strip above the
/// Friends list or a friend's chat that goes away by itself or with Close.
struct FriendsNotice: View {
    @Environment(FriendsWorkspace.self) private var friends

    var body: some View {
        if let text = friends.notice {
            HStack(spacing: 10) {
                Image(systemName: "info.circle").foregroundStyle(.secondary).accessibilityHidden(true)
                Text(verbatim: text).font(.subheadline.weight(.semibold)).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                Button("common_close") { friends.clearNotice() }
                    .font(.subheadline.weight(.semibold))
                    .buttonStyle(.borderless)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(uiColor: .secondarySystemBackground))
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("friends.notice")
            .task(id: text) {
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
                friends.clearNotice()
            }
        }
    }
}

/// What a lookup found (D6): who they are and the one action their relation allows. Add friend and the member sheet
/// share it; the confirmation, the request id and the note are the `FriendLookup`'s.
struct FriendResultCard: View {
    @Environment(FriendsWorkspace.self) private var friends
    let lookup: FriendLookup
    /// Opens the chat with a friend: from "Open chat", and after a confirmation that made them friends.
    let onOpenChat: (String) -> Void

    var body: some View {
        if let result = lookup.result {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 12) {
                    PersonAvatar(id: result.uid, name: result.displayName, size: 44)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verbatim: result.displayName).font(.headline).lineLimit(2)
                        if let handle = result.handle { Text(verbatim: "@" + handle).font(.subheadline).foregroundStyle(.secondary) }
                    }
                    Spacer(minLength: 0)
                }
                .accessibilityElement(children: .combine)
                actions(result)
                if !lookup.error.isEmpty {
                    Text(LocalizedStringKey(FriendsLogic.errorKey(lookup.error))).font(.footnote).foregroundStyle(.red)
                        .accessibilityIdentifier("friends.result.error")
                }
            }
            .padding(.vertical, 4)
            .onChange(of: lookup.outcome) { _, outcome in
                guard outcome == .friends else { return }
                friends.show(notice: String(localized: "friends_now_friends \(result.displayName)"))
                onOpenChat(result.uid)
            }
            .accessibilityIdentifier("friends.result")
        }
    }

    @ViewBuilder
    private func actions(_ result: FriendLookupResult) -> some View {
        switch lookup.outcome == .requested ? FriendAction.sent : FriendsLogic.action(for: result.relation) {
        case .add:
            if !lookup.isCode {
                TextField("friends_note_placeholder", text: note, axis: .vertical)
                    .lineLimit(1...3)
                    .accessibilityIdentifier("friends.result.note")
            }
            confirmButton("friends_add")
        case .accept:
            confirmButton("friends_request_accept")
        case .sent:
            Button {} label: { Text("friends_request_sent").frame(maxWidth: .infinity) }
                .buttonStyle(.bordered)
                .disabled(true)
            if lookup.outcome == .requested {
                TeamsNote(text: "friends_request_sent_to \(result.displayName)", systemImage: "paperplane")
            }
        case .friends:
            Label("friends_relation_friends", systemImage: "checkmark.circle").foregroundStyle(.secondary)
            Button { onOpenChat(result.uid) } label: { Text("friends_open_chat").frame(maxWidth: .infinity) }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("friends.result.open")
        }
    }

    /// At most 200 characters (UTF-16 units, as the server counts them).
    private var note: Binding<String> {
        Binding(get: { lookup.note }, set: { lookup.note = FriendsLogic.clipped($0, to: FriendsLogic.noteMax) })
    }

    private func confirmButton(_ title: LocalizedStringKey) -> some View {
        Button {
            Task { await lookup.confirm(in: friends) }
        } label: {
            HStack(spacing: 8) {
                if lookup.confirming { ProgressView() }
                Text(title)
            }
            .frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .disabled(lookup.confirming)
        .accessibilityIdentifier("friends.result.confirm")
    }
}

/// A friend link that was opened (D8): looks the owner up and asks before adding them. Nothing is redeemed by opening
/// it; the code lives in this sheet only, and closing the sheet, confirming or failing drops it.
struct FriendLinkSheet: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.dismiss) private var dismiss
    let code: String
    /// Opens the chat with the new (or already) friend and closes the sheet.
    let onOpenChat: (String) -> Void
    @State private var lookup = FriendLookup()

    var body: some View {
        NavigationStack {
            content
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() } } }
        }
        .presentationDetents([.medium])
        .task(id: code) { await lookup.lookup(.code(code), in: friends) }
        .onChange(of: friends.authority) { _, _ in dismiss() }
        .onChange(of: lookup.outcome) { _, outcome in
            guard outcome == .friends, let result = lookup.result else { return }
            friends.show(notice: String(localized: "friends_now_friends \(result.displayName)"))
            onOpenChat(result.uid)
        }
        .accessibilityIdentifier("friends.link")
    }

    @ViewBuilder
    private var content: some View {
        if let result = lookup.result {
            VStack(spacing: 16) {
                PersonAvatar(id: result.uid, name: result.displayName, size: 64)
                if result.relation == .friends {
                    Text(verbatim: result.displayName).font(.title3.weight(.semibold)).multilineTextAlignment(.center)
                    Label("friends_relation_friends", systemImage: "checkmark.circle").foregroundStyle(.secondary)
                    Button { onOpenChat(result.uid) } label: { Text("friends_open_chat").frame(maxWidth: .infinity) }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("friends.link.open")
                } else {
                    Text("friends_confirm_title \(result.displayName)").font(.title3.weight(.semibold)).multilineTextAlignment(.center)
                    Text("friends_confirm_link_body").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    Button {
                        Task { await lookup.confirm(in: friends) }
                    } label: {
                        HStack(spacing: 8) {
                            if lookup.confirming { ProgressView() }
                            Text("friends_add")
                        }
                        .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(lookup.confirming)
                    .accessibilityIdentifier("friends.link.confirm")
                    Button("common_cancel") { dismiss() }
                }
                if !lookup.error.isEmpty { failure }
            }
        } else if !lookup.error.isEmpty {
            VStack(spacing: 16) {
                failure
                Button("common_close") { dismiss() }
                    .buttonStyle(.borderedProminent)
            }
        } else {
            ProgressView().accessibilityLabel(Text("teams_loading"))
        }
    }

    private var failure: some View {
        Text(LocalizedStringKey(FriendsLogic.linkErrorKey(lookup.error)))
            .font(.subheadline).foregroundStyle(.red).multilineTextAlignment(.center)
            .accessibilityIdentifier("friends.link.error")
    }
}

/// "Add friend" on a Team member (D15): `lookup {teamId, uid}` into the same result card as Add friend, note included.
struct FriendMemberSheet: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.dismiss) private var dismiss
    let teamID: String
    let memberUID: String
    /// Opens the chat with the new (or already) friend; the sheet closes first.
    let onOpenChat: (String) -> Void
    @State private var lookup = FriendLookup()

    var body: some View {
        NavigationStack {
            List {
                Section {
                    if lookup.result != nil {
                        FriendResultCard(lookup: lookup) { uid in
                            dismiss()
                            onOpenChat(uid)
                        }
                    } else if !lookup.error.isEmpty {
                        Text(LocalizedStringKey(FriendsLogic.errorKey(lookup.error))).foregroundStyle(.red)
                        Button("common_retry") { Task { await run() } }
                    } else {
                        HStack {
                            Spacer()
                            ProgressView().accessibilityLabel(Text("teams_loading"))
                            Spacer()
                        }
                    }
                }
            }
            .navigationTitle("friends_add")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() } } }
        }
        .presentationDetents([.medium])
        .task { await run() }
        .onChange(of: friends.authority) { _, _ in dismiss() }
        .accessibilityIdentifier("friends.member")
    }

    private func run() async {
        await lookup.lookup(.team(teamId: teamID, uid: memberUID), in: friends)
    }
}

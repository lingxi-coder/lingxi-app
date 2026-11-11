import SwiftUI

/// Blocked people (docs/FRIENDS-CLIENTS.md D12): the entries of `users/{uid}/blocks`, newest first, each with the name the
/// backend kept when the block was made and Unblock. Nothing is remembered on the device.
struct BlockedPeopleView: View {
    @Environment(FriendsWorkspace.self) private var friends

    var body: some View {
        List {
            if !friends.blocksLoaded && !friends.blocksFailed {
                HStack {
                    Spacer()
                    ProgressView().accessibilityLabel(Text("friends_loading"))
                    Spacer()
                }
                .listRowSeparator(.hidden)
            } else if friends.blocksFailed && friends.blocks.isEmpty {
                TeamsStateView(systemImage: "exclamationmark.triangle", title: "friends_blocked_title", message: "friends_load_error") {
                    Button("common_retry") { friends.reload() }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("friends.blocked.retry")
                }
                .listRowSeparator(.hidden).listRowBackground(Color.clear)
            } else {
                Section {
                    if !friends.actionError.isEmpty {
                        Text(LocalizedStringKey(FriendsLogic.errorKey(friends.actionError))).foregroundStyle(.red)
                            .accessibilityIdentifier("friends.blocked.error")
                    }
                    if friends.blocks.isEmpty {
                        Text("friends_blocked_empty").foregroundStyle(.secondary)
                            .accessibilityIdentifier("friends.blocked.empty")
                    }
                    ForEach(friends.blockedPeople) { block in row(block) }
                } footer: {
                    Text("friends_unblock_footer")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("friends_blocked_title")
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: .friends)
        .onAppear { friends.clearActionError() }
        .accessibilityIdentifier("friends.blocked")
    }

    private func row(_ block: FriendBlock) -> some View {
        // The entry has no name when it was made before the backend kept one (D12): `Member`.
        let name = block.displayName.isEmpty ? String(localized: "teams_member") : block.displayName
        return HStack(spacing: 12) {
            PersonAvatar(id: block.uid, name: name, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: name).font(.body.weight(.semibold)).lineLimit(1)
                if let handle = block.handle { Text(verbatim: "@" + handle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            Spacer(minLength: 8)
            Button("friends_unblock") {
                Task { await friends.end("unblock", with: block.uid) }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(friends.isBusy(block.uid))
            .accessibilityIdentifier("friends.unblock.\(block.uid)")
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("friends.block.\(block.uid)")
    }
}

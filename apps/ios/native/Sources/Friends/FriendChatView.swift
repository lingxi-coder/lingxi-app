import SwiftUI

/// A friend's chat (docs/FRIENDS-CLIENTS.md D9, D10): the Team conversation view and message layer, unchanged, over the
/// `TeamsWorkspace` the Friends workspace pinned to this friendship's pair team. The view leaves out what a Team has
/// and a friend does not (details, platform notice) by reading `workspace.pair`; this wrapper adds the friend's menu
/// (View profile, Remove, Block), the Bots button (D21), the notice, and the unavailable state once the friendship has ended.
struct FriendChatView: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.scenePhase) private var scenePhase
    let uid: String
    /// Back to the list: after Remove or Block, and from the unavailable state.
    let onClose: () -> Void
    /// The Bots button: the Bots of this friendship (D21).
    let onBots: () -> Void

    private enum Pending: Identifiable {
        case remove, block
        var id: Self { self }
    }

    @State private var pending: Pending?
    @State private var viewing = false
    /// The Teams layer found no `friend_chat` for the viewer any more: the friendship is over.
    @State private var gone = false

    var body: some View {
        content
            .safeAreaInset(edge: .top, spacing: 0) { banners }
            .hidesTabBarWhenPushed(in: .friends)
            .confirmationDialog(
                pending.map { question($0, name: friends.friend(uid)?.displayName ?? "") } ?? Text(verbatim: ""),
                isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } }),
                titleVisibility: .visible, presenting: pending
            ) { action in
                Button(LocalizedStringKey(action == .remove ? "friends_menu_remove" : "friends_menu_block"), role: .destructive) { confirm(action) }
                Button("common_cancel", role: .cancel) {}
            } message: { action in
                Text(LocalizedStringKey(action == .remove ? "friends_remove_body" : "friends_block_body"))
            }
            .sheet(isPresented: $viewing) { FriendProfileSheet(uid: uid) }
            // The chat on screen counts as read in the list and the badge (D13); it stops counting when it is hidden or the app leaves.
            .onAppear {
                friends.clearActionError()
                friends.reading(uid, visible: scenePhase == .active)
                // Back from the Bots or a Bot DM: the workspace selects the friend chat again (D22).
                friends.chat(for: uid)?.workspace.openConversation(FriendsLogic.chatConversationID)
            }
            .onDisappear { friends.reading(uid, visible: false) }
            .onChange(of: scenePhase) { _, phase in friends.reading(uid, visible: phase == .active) }
            .onChange(of: friends.authority) { _, _ in pending = nil; viewing = false }
            .onChange(of: friends.chat(for: uid)?.teamID) { _, _ in gone = false }
            // A chat refreshes the friend's name and username once it is known to be theirs (D10).
            .task(id: friends.friend(uid)?.uid) { await friends.refreshProfile(of: uid) }
    }

    /// What the screen shows: the chat, the wait for it, or the end of the friendship.
    private enum Phase {
        case chat(FriendCard, FriendsWorkspace.Chat)
        case loading
        case unavailable
    }

    private var phase: Phase {
        let friend = friends.friend(uid)
        let chat = friends.chat(for: uid)
        if gone || chat?.ended == true { return .unavailable }
        if let friend, let chat { return .chat(friend, chat) }
        // The card is there but the chat is not open yet, or the new friendship's card has not arrived.
        return friend != nil || friends.expecting.contains(uid) ? .loading : .unavailable
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case let .chat(friend, chat):
            TeamConversationView(conversationID: FriendsLogic.chatConversationID, onGone: { gone = true })
                .environment(chat.workspace)
                .id(chat.teamID + ":" + String(chat.generation))
                .toolbar { toolbar(friend) }
        case .loading:
            FriendChatLoadingView()
        case .unavailable:
            FriendChatUnavailableView(onClose: onClose)
        }
    }

    // MARK: Bots button and menu (D10, D21)

    @ToolbarContentBuilder
    private func toolbar(_ friend: FriendCard) -> some ToolbarContent {
        // Always there, with no Bots too, so that a friend can find it (D21).
        ToolbarItem(placement: .topBarTrailing) {
            Button(action: onBots) { Image(systemName: "cpu") }
                .accessibilityLabel(Text("friends_bots_title"))
                .accessibilityIdentifier("friends.chat.bots")
        }
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Button("friends_menu_view_profile", systemImage: "person.text.rectangle") { viewing = true }
                Button("friends_menu_remove", systemImage: "person.badge.minus", role: .destructive) { pending = .remove }
                Button("friends_menu_block", systemImage: "hand.raised", role: .destructive) { pending = .block }
            } label: {
                Image(systemName: "person.crop.circle")
            }
            .disabled(friends.isBusy(uid))
            .accessibilityLabel(Text(verbatim: friend.displayName))
            .accessibilityIdentifier("friends.chat.menu")
        }
    }

    private func question(_ action: Pending, name: String) -> Text {
        switch action {
        case .remove: Text("friends_remove_title \(name)")
        case .block: Text("friends_block_title \(name)")
        }
    }

    private func confirm(_ action: Pending) {
        Task {
            if await friends.end(action == .remove ? "remove" : "block", with: uid) { onClose() }
        }
    }

    @ViewBuilder
    private var banners: some View {
        VStack(spacing: 0) {
            if !friends.actionError.isEmpty {
                TeamsBanner(kind: .error, title: LocalizedStringKey(FriendsLogic.errorKey(friends.actionError)), actionTitle: "common_close") { friends.clearActionError() }
                    .accessibilityIdentifier("friends.chat.banner")
            }
            FriendsNotice()
        }
    }
}

/// The wait for a friend's chat: the card is there but the workspace is not open yet, or the new friendship's card has not arrived.
/// The chat, the Bots and a Bot DM share it.
struct FriendChatLoadingView: View {
    var body: some View {
        VStack(spacing: 12) {
            ProgressView()
            Text("teams_loading_messages").font(.footnote).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationBarTitleDisplayMode(.inline)
        .accessibilityIdentifier("friends.chat.loading")
    }
}

/// The end of the friendship under an open screen (D9, D25): the whole friend area, the Bots and a Bot DM included, is replaced by
/// this and Back, which closes it.
struct FriendChatUnavailableView: View {
    let onClose: () -> Void

    var body: some View {
        TeamsStateView(systemImage: "person.crop.circle.badge.xmark", title: "friends_chat_unavailable") {
            Button("remote_back", action: onClose)
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("friends.chat.back")
        }
        .navigationBarTitleDisplayMode(.inline)
        .accessibilityIdentifier("friends.chat.unavailable")
    }
}

/// View profile (D10): the friend's name and `@username`, and since when they are friends. The name and username are the
/// friend's own profile if it differs from the card (`refreshProfile`), else the card's.
private struct FriendProfileSheet: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.dismiss) private var dismiss
    @Environment(\.locale) private var locale
    let uid: String

    var body: some View {
        NavigationStack {
            List {
                if let friend = friends.friend(uid) {
                    Section {
                        VStack(spacing: 8) {
                            PersonAvatar(id: friend.uid, name: friend.displayName, size: 72)
                            Text(verbatim: friend.displayName).font(.title2.weight(.semibold)).multilineTextAlignment(.center)
                            if let handle = friend.handle { Text(verbatim: "@" + handle).foregroundStyle(.secondary) }
                            if friend.sinceMs > 0 {
                                Text("friends_since \(since(friend))").font(.footnote).foregroundStyle(.secondary)
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .accessibilityElement(children: .combine)
                    }
                }
            }
            .navigationTitle("friends_menu_view_profile")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("common_done") { dismiss() } } }
        }
        .presentationDetents([.medium])
        .task { await friends.refreshProfile(of: uid) }
        .onChange(of: friends.friend(uid) == nil) { _, gone in if gone { dismiss() } }
        .accessibilityIdentifier("friends.profile.sheet")
    }

    private func since(_ friend: FriendCard) -> String {
        Date(timeIntervalSince1970: Double(friend.sinceMs) / 1000).formatted(Date.FormatStyle(date: .abbreviated, time: .omitted).locale(locale))
    }
}

import SwiftUI

/// The Bots of a friendship (docs/FRIENDS-CLIENTS.md D21 to D23): the Teams Bot cards of the pair team, read by the friend chat's
/// own pair-pinned `TeamsWorkspace` (`messaging.bots`), without the audience line, since a friendship has no audience to choose.
/// Message opens the Bot DM; Stop sharing pauses the viewer's own Bot (`TeamMessagingModel.pause`). iOS cannot publish a Bot, so
/// under the list there is only the note about who can read a Bot's chats and the hint where a Bot is shared from. The end of the
/// friendship replaces the screen by `friends_chat_unavailable` and Back (D25).
struct FriendBotsView: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.isWideLayout) private var wide
    let uid: String
    /// Message on a Bot: opens its Bot DM, a conversation of the pair workspace (D22).
    let onOpenBot: (String) -> Void
    /// Back to the friend chat. Phones have the system back button; the wide layout's pane needs this one.
    let onBack: () -> Void
    /// The friendship is over: Back from the unavailable state closes the whole friend area.
    let onClose: () -> Void

    /// The Bot whose Stop sharing is waiting for its confirmation.
    @State private var stopping: TeamBotCard?
    @State private var listOf: TeamBotCard?
    /// The Bot whose auto-reply log is open, and the Bots the service said this person does not maintain after all: their entry is not offered again.
    @State private var triageOf: TeamBotCard?
    @State private var triageDenied: Set<String> = []

    var body: some View {
        content
            .navigationTitle("friends_bots_title")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if wide {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(action: onBack) { Label("remote_back", systemImage: "chevron.backward").labelStyle(.titleAndIcon) }
                            .accessibilityIdentifier("friends.bots.back")
                    }
                }
            }
            .hidesTabBarWhenPushed(in: .friends)
            // An account change, or the end of the friendship, drops what the person was about to confirm.
            .onChange(of: friends.authority) { _, _ in stopping = nil; listOf = nil; triageOf = nil; triageDenied = [] }
            .accessibilityIdentifier("friends.bots")
    }

    /// The panel, the wait for the chat's workspace, or the end of the friendship.
    @ViewBuilder
    private var content: some View {
        let friend = friends.friend(uid)
        if friend != nil, let chat = friends.chat(for: uid) {
            if chat.ended {
                FriendChatUnavailableView(onClose: onClose)
            } else {
                panel(chat)
            }
        } else if friend != nil || friends.expecting.contains(uid) {
            FriendChatLoadingView()
        } else {
            FriendChatUnavailableView(onClose: onClose)
        }
    }

    // MARK: The list (D21)

    private func panel(_ chat: FriendsWorkspace.Chat) -> some View {
        let workspace = chat.workspace
        let model = workspace.messaging
        let bots = model?.bots ?? []
        let status: LocalizedStringKey? = model?.botsUnverified == true ? "teams_bots_unverified" : model?.botsLoaded == true ? nil : "teams_bots_loading"
        let offline = model?.offline ?? true
        let empty = bots.isEmpty && status == nil
        return List {
            Section {
                if let status { TeamsNote(text: status, systemImage: "exclamationmark.triangle") }
                ForEach(bots) { bot in
                    let existing = workspace.uid.flatMap { TeamsListLogic.findBotDm(model?.cache?.conversations ?? [], uid: $0, sharedBotId: bot.sharedBotId) }
                    TeamBotCardView(
                        bot: bot, ownerName: name(of: bot.maintainer.uid, in: workspace),
                        isOwner: bot.maintainer == TeamMemberRef(uid: workspace.uid ?? "", generation: chat.generation),
                        busy: workspace.messageBusy || (offline && existing == nil),
                        showsAudience: false,
                        onMessage: { Task { if let id = await workspace.openBotDM(bot) { onOpenBot(id) } } },
                        onStopSharing: { stopping = bot },
                        onList: TeamSharedListRules.canOpen(bot, viewer: TeamMemberRef(uid: workspace.uid ?? "", generation: chat.generation)) && !offline ? { listOf = bot } : nil,
                        onTriageLog: TeamTriageRules.maintains(bot, viewer: TeamMemberRef(uid: workspace.uid ?? "", generation: chat.generation)) && bot.autoReply != nil && !triageDenied.contains(bot.sharedBotId) && !offline ? { triageOf = bot } : nil)
                }
                if empty {
                    // What is empty, and what to do about it (U9): the hint is the body here, so it is not repeated below.
                    TeamsStateView(systemImage: "cpu", title: "friends_bots_none", message: "friends_bots_share_hint")
                        .listRowSeparator(.hidden).listRowBackground(Color.clear)
                        .accessibilityIdentifier("friends.bots.none")
                }
            }
            // Under the list, always: a Bot runs on its owner's Desktop and its owner can read every chat with it (D21).
            Section {
                TeamsNote(text: "friends_bots_note", systemImage: "eye")
                if !empty { TeamsNote(text: "friends_bots_share_hint", systemImage: "desktopcomputer") }
            }
        }
        .listStyle(.insetGrouped)
        .toolbar {
            if let model, !model.error.isEmpty || (model.directoryReady && model.offline) {
                ToolbarItem(placement: .topBarTrailing) {
                    let status = model.error.isEmpty ? LocalizedStringKey("teams_offline_title") : LocalizedStringKey(TeamsErrors.key(model.error))
                    Menu {
                        Text(model.error.isEmpty ? LocalizedStringKey("teams_messages_offline") : status)
                        Button("teams_reconnect", systemImage: "arrow.clockwise") { workspace.reconnect() }
                    } label: { Image(systemName: model.error.isEmpty ? "wifi.slash" : "exclamationmark.circle") }
                    .accessibilityLabel(Text(status))
                }
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if !workspace.actionError.isEmpty, workspace.actionError != model?.error {
                TeamsBanner(kind: .error, title: LocalizedStringKey(TeamsErrors.key(workspace.actionError)), actionTitle: "common_close") { workspace.clearActionError() }
                    .accessibilityIdentifier("friends.bots.banner")
            }
        }
        .sheet(item: $listOf) { bot in
            if let model = workspace.messaging { TeamSharedListSheet(bot: bot, model: model, members: workspace.directory) { listOf = nil } }
        }
        .sheet(item: $triageOf) { bot in
            if let model = workspace.messaging { TeamTriageLogSheet(bot: bot, model: model, onDenied: { triageDenied.insert($0) }) { triageOf = nil } }
        }
        .confirmationDialog(
            stopping.map { Text("teams_bot_stop_title \($0.name)") } ?? Text(verbatim: ""),
            isPresented: Binding(get: { stopping != nil }, set: { if !$0 { stopping = nil } }),
            titleVisibility: .visible, presenting: stopping
        ) { bot in
            Button("friends_share_bot_stop", role: .destructive) { stop(bot, in: workspace) }
                .disabled(model?.offline ?? true)
            Button("common_cancel", role: .cancel) {}
        } message: { _ in
            Text("teams_bot_stop_detail")
        }
    }

    /// The person's name from the pair directory (the friend, and the viewer once their profile is known).
    private func name(of id: String, in workspace: TeamsWorkspace) -> String {
        let name = workspace.directory.first { $0.uid == id }?.displayName ?? ""
        if !name.isEmpty { return name }
        let strings = TeamsListStrings.current
        return id == workspace.uid ? strings.you : strings.member
    }

    /// D23 Stop sharing: the Teams `pauseSharedBot`, queued like any Bot command; the Bot cards refresh afterwards.
    private func stop(_ bot: TeamBotCard, in workspace: TeamsWorkspace) {
        Task { await workspace.run { try await workspace.messaging?.pause(bot) } }
    }
}

/// A Bot DM of a friendship (docs/FRIENDS-CLIENTS.md D22): the Teams conversation view on the friend chat's own pair-pinned
/// workspace, headed by the Bot and its presence instead of the friend (the friend header and menu belong to `friend_chat` only). Back
/// returns to the friend chat, which selects `friend_chat` again; the end of the friendship replaces the screen like the chat and the
/// Bots (D25).
struct FriendBotChatView: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.isWideLayout) private var wide
    let uid: String
    /// The Bot DM, a conversation of the pair workspace.
    let conversationID: String
    /// Back to the friend chat; also what happens when the Bot DM is not there (any more).
    let onBack: () -> Void
    /// The friendship is over: Back from the unavailable state closes the whole friend area.
    let onClose: () -> Void

    var body: some View {
        content
            .toolbar {
                if wide {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(action: onBack) { Label("remote_back", systemImage: "chevron.backward").labelStyle(.titleAndIcon) }
                            .accessibilityIdentifier("friends.botchat.back")
                    }
                }
            }
            .hidesTabBarWhenPushed(in: .friends)
    }

    @ViewBuilder
    private var content: some View {
        let friend = friends.friend(uid)
        if friend != nil, let chat = friends.chat(for: uid) {
            if chat.ended {
                FriendChatUnavailableView(onClose: onClose)
            } else {
                TeamConversationView(conversationID: conversationID, onGone: onBack)
                    .environment(chat.workspace)
                    .id(chat.teamID + ":" + String(chat.generation))
            }
        } else if friend != nil || friends.expecting.contains(uid) {
            FriendChatLoadingView()
        } else {
            FriendChatUnavailableView(onClose: onClose)
        }
    }
}

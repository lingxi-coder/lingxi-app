import SwiftUI

#if canImport(harness_runtimeFFI)
/// A session in a list: a computer's shared sessions and a Bot's chats look alike.
struct RemoteSessionRow: View {
    let session: RemoteSessionSummaryFfi
    /// A computer's sessions keep their project line even when it is empty, as they always did; a Bot's
    /// chats mostly have no project, and do without the line.
    var hidesEmptyProject = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(session.title.isEmpty ? String(localized: "remote_untitled") : session.title)
            if !(hidesEmptyProject && session.projectLabel.isEmpty) {
                Text(session.projectLabel).font(.caption).foregroundStyle(.secondary)
            }
            Text(status).font(.caption).foregroundStyle(.secondary)
        }
    }

    private var status: LocalizedStringKey {
        switch session.status {
        case .running: "remote_status_running"
        case .requiresAction: "remote_status_requires_action"
        case .idle: "remote_status_idle"
        case .dormant: "remote_status_dormant"
        }
    }
}

/// A Bot: its name, what it is for, where it stands, and how many of its chats wait for the reader.
struct RemoteBotRow: View {
    let bot: RemoteBotSummaryFfi
    /// In a list the description is one line; on the Bot's own screen it is shown whole.
    var wholeDescription = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(bot.name)
            if !bot.description.isEmpty {
                Text(bot.description).font(.caption).foregroundStyle(.secondary).lineLimit(wholeDescription ? nil : 1)
            }
            Text(status).font(.caption).foregroundStyle(.secondary)
            if bot.waitingThreads > 0 {
                Text(String(localized: "remote_bot_waiting \(Int(bot.waitingThreads))")).font(.caption).foregroundStyle(.orange)
            }
        }
    }

    // A Bot is in one of a session's states, less the one that only a session can be in.
    private var status: LocalizedStringKey {
        switch bot.status {
        case .running: "remote_status_running"
        case .requiresAction: "remote_status_requires_action"
        case .idle: "remote_status_idle"
        }
    }
}

/// The Bots of a computer, above its sessions.
struct RemoteBotsSection: View {
    let bots: [RemoteBotSummaryFfi]
    let open: (RemoteBotSummaryFfi) -> Void

    var body: some View {
        Section {
            ForEach(bots, id: \.botId) { bot in
                Button { open(bot) } label: { RemoteBotRow(bot: bot) }
                    .accessibilityIdentifier("remote.bot.\(bot.botId)")
            }
        } header: { Text("remote_bots") }
    }
}

/// One Bot: what it is, its chats newest first, and a way to start a new one. A chat opens in the
/// session screen, which returns here.
@MainActor
struct RemoteBotView: View {
    let manager: RemoteControlManager
    let hostID: String
    let botID: String
    let openChat: (_ sessionID: String, _ title: String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var creating = false
    @State private var createFailure: RemoteBotCreateFailure?
    /// Asking for the chats failed before the computer could answer (nothing was listed or failed in the state).
    @State private var activationFailed = false
    @State private var visible = false

    private var key: RemoteBotKey { RemoteBotKey(hostID: hostID, botID: botID) }
    private var connected: Bool {
        manager.connection?.hostId == hostID && manager.connection?.phase == .ready
    }

    var body: some View {
        let screen = RemoteBotScreen(
            bot: botID,
            bots: manager.botDirectory.bots(host: hostID),
            chats: manager.botDirectory.chats(host: hostID, bot: botID))
        Group {
            switch screen.phase {
            case .loading:
                ProgressView("remote_loading")
            case .gone:
                ContentUnavailableView {
                    Label("remote_bots", systemImage: "person.crop.circle.badge.questionmark")
                } description: { Text("remote_bot_gone") } actions: {
                    Button("remote_back") { dismiss() }
                }
            case let .ready(bot):
                content(bot, screen)
            }
        }
        .navigationTitle(title(screen))
        .task { await show() }
        .onAppear { visible = true }
        .onDisappear {
            visible = false
            manager.hideBot(key)
        }
    }

    private func title(_ screen: RemoteBotScreen) -> String {
        if case let .ready(bot) = screen.phase { return bot.name }
        return String(localized: "remote_bots")
    }

    private func content(_ bot: RemoteBotSummaryFfi, _ screen: RemoteBotScreen) -> some View {
        List {
            if !connected {
                Section { Text("remote_reconnecting").font(.caption).foregroundStyle(.secondary) }
            }
            Section {
                RemoteBotRow(bot: bot, wholeDescription: true)
                Button(action: createChat) {
                    HStack {
                        Label("remote_bot_new_chat", systemImage: "square.and.pencil")
                        if creating { ProgressView() }
                    }.frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .disabled(creating || !connected)
                .accessibilityIdentifier("remote.bot.new-chat")
                if let createFailure {
                    Text(createFailure.message).font(.caption).foregroundStyle(.red)
                }
            }
            Section {
                if screen.loadingChats && !activationFailed { ProgressView("remote_loading") }
                if screen.listFailed || activationFailed {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("remote_err_generic").foregroundStyle(.red)
                        Button("common_retry", action: retry).buttonStyle(.borderless)
                    }
                }
                if screen.noChats { Text("remote_bot_no_chats").foregroundStyle(.secondary) }
                ForEach(screen.chats, id: \.sessionId) { chat in
                    Button {
                        openChat(chat.sessionId, chat.title.isEmpty ? String(localized: "remote_untitled") : chat.title)
                    } label: { RemoteSessionRow(session: chat, hidesEmptyProject: true) }
                        .accessibilityIdentifier("remote.session.\(chat.sessionId)")
                }
            }
        }
    }

    private func show() async {
        activationFailed = false
        do { try await manager.showBot(key) }
        catch is CancellationError { }
        catch { activationFailed = true }
    }

    private func retry() {
        activationFailed = false
        Task {
            do { try await manager.reloadBot(key) }
            catch is CancellationError { }
            catch { activationFailed = true }
        }
    }

    private func createChat() {
        guard !creating else { return }
        creating = true
        createFailure = nil
        Task {
            do {
                let session = try await manager.createBotChat(key)
                creating = false
                // The chat exists on the computer whether or not this screen waited for it; one that was
                // left does not take the reader to it afterwards.
                if visible { openChat(session, String(localized: "remote_untitled")) }
            } catch {
                creating = false
                createFailure = RemoteBotCreateFailure(error)
            }
        }
    }
}
#endif

import SwiftUI

#if canImport(harness_runtimeFFI)
@MainActor
struct BotsHostSection: View {
    let manager: RemoteControlManager
    let host: RemoteHostFfi
    let openComputer: () -> Void
    let openChat: (String, String) -> Void
    let canNavigate: () -> Bool
    @State private var connectionError = false

    private var store: BotsSnapshotStore { manager.snapshots }
    private var connected: Bool { manager.connectedHostID == host.hostId }
    private var live: BotsLogic.Live {
        switch manager.botDirectory.bots(host: host.hostId) {
        case .unknown: .unknown
        case .unsupported: .unsupported
        case .bots(let bots): .bots(bots.count)
        }
    }
    private var snapshot: BotsSnapshot? { store.snapshots[host.hostId] }
    private var facts: BotsLogic.HostBody {
        BotsLogic.hostBody(connected: connected, online: host.online, paired: host.paired, live: live,
                           snapshotBots: snapshot?.bots.count)
    }
    private var bots: [BotsItem] {
        if facts.source == .live, case let .bots(bots) = manager.botDirectory.bots(host: host.hostId) {
            return BotsLogic.sortBots(bots.map(BotsItem.init))
        }
        return BotsLogic.sortBots(snapshot?.bots ?? [])
    }

    var body: some View {
        Section {
            if !store.collapsedHosts.contains(host.hostId) {
                content
                if manager.connection?.hostId == host.hostId, manager.connection?.phase != .ready {
                    RemoteConnectionSection(state: manager.connection, retry: { Task { await manager.retry() } },
                                            repair: { Task { try? await manager.repair(host.hostId) } })
                }
                if connectionError { Text("remote_fail_network").foregroundStyle(.red) }
            }
        } header: {
            HStack {
                Button { store.toggleHost(host.hostId) } label: {
                    HStack {
                        Image(systemName: store.collapsedHosts.contains(host.hostId) ? "chevron.right" : "chevron.down")
                        Image(systemName: "desktopcomputer").accessibilityHidden(true)
                        VStack(alignment: .leading) {
                            Text(host.name).font(.headline).foregroundStyle(.primary)
                            Text(connected ? "mobile_bots_connected" : !host.paired ? "remote_not_paired" : host.online ? "remote_online" : "remote_offline")
                                .font(.caption)
                            if !connected, host.lastSeenMs > 0 {
                                Text("mobile_bots_last_seen \(relative(host.lastSeenMs))").font(.caption)
                            }
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("bots.host.\(host.hostId)")
                .accessibilityValue(Text(store.collapsedHosts.contains(host.hostId) ? "chat_run_expand" : "chat_run_collapse"))
                Button(action: openComputer) { Image(systemName: "info.circle").frame(minWidth: 44, minHeight: 44) }
                    .accessibilityLabel("mobile_bots_computer_details")
                    .accessibilityIdentifier("bots.computer.\(host.hostId)")
            }.textCase(nil)
        }
        .onChange(of: bots, initial: true) { _, bots in
            if connected, let first = bots.first { store.expandDefault(host.hostId + "/" + first.id) }
        }
    }

    @ViewBuilder private var content: some View {
        if facts.notes.contains(.savedNote), let snapshot {
            Text("mobile_bots_saved_note \(relative(snapshot.savedAtMs))").font(.caption).foregroundStyle(.secondary)
        }
        if facts.notes.contains(.offlineChatsNote) {
            Text("mobile_bots_offline_chats_note").font(.caption).foregroundStyle(.secondary)
        }
        switch facts.kind {
        case .loading: ProgressView("remote_loading")
        case .unsupported: Text("mobile_bots_unsupported").foregroundStyle(.secondary)
        case .noBots: Text("mobile_bots_none").foregroundStyle(.secondary)
        case .offlineNoSnapshot:
            Text("remote_offline")
            Text("remote_offline_help").font(.caption).foregroundStyle(.secondary)
        case .notPaired, .connectToLoad:
            Text(facts.kind == .notPaired ? "remote_not_paired" : "mobile_bots_connect_to_load")
            Button("remote_connect", action: connect).accessibilityIdentifier("bots.connect.\(host.hostId)")
        case .bots:
            ForEach(bots) { bot in
                BotsTreeBot(manager: manager, host: host, bot: bot, dimmed: facts.dimmed,
                            connect: connect, openChat: openChat, canNavigate: canNavigate)
            }
        }
    }
    private func connect() {
        connectionError = false
        Task {
            do { try await manager.connect(host.hostId) }
            catch is CancellationError { }
            catch { connectionError = true }
        }
    }
    private func relative(_ milliseconds: UInt64) -> String {
        Date(timeIntervalSince1970: Double(milliseconds) / 1000).formatted(.relative(presentation: .named))
    }
}

@MainActor
private struct BotsTreeBot: View {
    let manager: RemoteControlManager
    let host: RemoteHostFfi
    let bot: BotsItem
    let dimmed: Bool
    let connect: () -> Void
    let openChat: (String, String) -> Void
    let canNavigate: () -> Bool
    @State private var showAll = false
    @State private var activationFailed = false
    @State private var creating = false
    @State private var failure: String?
    @State private var visible = false
    private var key: RemoteBotKey { RemoteBotKey(hostID: host.hostId, botID: bot.id) }
    private var expansionKey: String { host.hostId + "/" + bot.id }
    private var expanded: Bool { manager.snapshots.expandedBots.contains(expansionKey) }
    private var ready: Bool { manager.connection?.hostId == host.hostId && manager.connection?.phase == .ready }
    private var subscribed: Bool { visible && expanded && ready }
    private var listing: RemoteBotThreadsStateFfi { manager.botDirectory.chats(host: host.hostId, bot: bot.id) }
    private var gone: Bool { ready && RemoteBotScreen.isUnreachable(listing.error) }
    private var chats: [BotsChat]? { ready ? listing.threads?.map(BotsChat.init) : bot.chats }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button(action: toggle) {
                HStack(spacing: 10) {
                    AgentAvatar(agentID: bot.id, size: 40).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(bot.name).foregroundStyle(.primary)
                        if !bot.description.isEmpty { Text(bot.description).font(.caption).lineLimit(1).foregroundStyle(.secondary) }
                        if bot.status == .running { Label("remote_status_running", systemImage: "circle.dotted").font(.caption) }
                        if bot.status == .requiresAction { Text("remote_status_requires_action").font(.caption).foregroundStyle(.orange) }
                    }
                    Spacer(minLength: 4)
                    VStack(alignment: .trailing) {
                        if let at = bot.lastActivityAtMs {
                            Text(Date(timeIntervalSince1970: Double(at) / 1000), style: .relative).font(.caption2)
                        }
                        if bot.waitingThreads > 0 {
                            Text(TabLogic.tabBadgeText(bot.waitingThreads) ?? "").font(.caption.bold())
                                .foregroundStyle(.orange).accessibilityLabel(Text("remote_bot_waiting \(bot.waitingThreads)"))
                        }
                    }
                    Image(systemName: expanded ? "chevron.down" : "chevron.right").font(.caption)
                }.contentShape(Rectangle()).opacity(dimmed ? 0.55 : 1)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("bots.bot.\(host.hostId).\(bot.id)")
            .accessibilityValue(Text(expanded ? "chat_run_collapse" : "chat_run_expand"))
            if expanded { chatRows.padding(.leading, 24) }
        }
        .onAppear { visible = true }
        .task(id: subscribed) {
            if subscribed {
                activationFailed = false
                do { try await manager.showBot(key) }
                catch is CancellationError { }
                catch { activationFailed = true }
            } else { manager.hideBot(key) }
        }
        .onDisappear { visible = false; manager.hideBot(key) }
    }

    @ViewBuilder private var chatRows: some View {
        if gone { Text("remote_bot_gone").font(.caption).foregroundStyle(.secondary) }
        else {
            Button {
                Task { await createChat() }
            } label: {
                HStack { Label("remote_bot_new_chat", systemImage: "plus"); if creating { ProgressView() } }
            }
            .disabled(creating || !(ready || host.online && host.paired))
            .accessibilityIdentifier("bots.new-chat.\(host.hostId).\(bot.id)")
            if ready && chats == nil && listing.error == nil && !activationFailed {
                ProgressView("mobile_bots_chats_loading")
            }
            if ready && (listing.error != nil || activationFailed) {
                Text("mobile_bots_chats_failed").font(.caption)
                Button("common_retry") {
                    Task {
                        do { try await manager.reloadBot(key); activationFailed = false }
                        catch { activationFailed = true }
                    }
                }
            }
            if let chats {
                if chats.isEmpty { Text("remote_bot_no_chats").font(.caption).foregroundStyle(.secondary) }
                let window = BotsLogic.chatWindow(total: chats.count, expanded: showAll)
                ForEach(Array(chats.prefix(window.shown))) { chat in
                    Button { open(chat) } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(chat.title.isEmpty ? String(localized: "remote_untitled") : chat.title)
                            if let pending = chat.pendingAction {
                                Text(pending == .permission ? "remote_needs_permission" : pending == .question ? "remote_needs_question" : "remote_needs_computer_access")
                                    .font(.caption).foregroundStyle(.orange)
                            } else if chat.status == .running { Text("remote_status_running").font(.caption) }
                        }.frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle()).opacity(dimmed ? 0.55 : 1)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("remote.session.\(chat.id)")
                }
                if window.showMore { Button("mobile_bots_show_more \(window.hidden)") { showAll = true } }
                if window.showLess { Button("chat_tool_show_less") { showAll = false } }
            }
            if let failure { Text(failure).font(.caption).foregroundStyle(.red) }
        }
    }
    private func toggle() {
        manager.snapshots.toggleBot(expansionKey)
        if expanded {
            if !ready && host.online && host.paired { connect() }
        } else { showAll = false; manager.hideBot(key) }
    }
    private func open(_ chat: BotsChat) {
        guard ready || host.online else { failure = String(localized: "remote_fail_no_answer"); return }
        if !host.paired && !ready { connect(); return }
        openChat(chat.id, chat.title.isEmpty ? String(localized: "remote_untitled") : chat.title)
    }
    private func createChat() async {
        guard !creating else { return }
        creating = true; failure = nil
        defer { creating = false }
        do {
            if !ready { try await manager.connect(host.hostId); try await manager.waitForReady(host.hostId) }
            // connect returns only after its ready/pairing attempt; pairing cannot send the command.
            guard manager.connection?.hostId == host.hostId, manager.connection?.phase == .ready else { return }
            guard canNavigate() else { return }
            let session = try await manager.createBotChat(key)
            guard !Task.isCancelled, canNavigate() else { return }
            openChat(session, String(localized: "remote_untitled"))
        } catch is CancellationError { }
        catch { failure = RemoteBotCreateFailure(error).message }
    }
}
#endif

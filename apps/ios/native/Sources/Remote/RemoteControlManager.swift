import Foundation
import Observation
import UIKit
#if canImport(FirebaseAuth)
import FirebaseAuth
#endif

#if canImport(harness_runtimeFFI)
@MainActor
@Observable
final class RemoteControlManager {
    private let mediaHostComposition: IOSNativeMediaHostComposition?
    private let mediaHostInitializationError: String?
    private(set) var uid: String?
    private(set) var hosts: [RemoteHostFfi] = []
    private(set) var hostsLoaded = false
    private(set) var connection: RemoteConnectionStateFfi?
    private(set) var sessions: [RemoteSessionSummaryFfi] = []
    private(set) var botDirectory = RemoteBotDirectory()
    private(set) var error: String?
    let snapshots = BotsSnapshotStore()
    private(set) var waitingChats = 0
    @ObservationIgnored private var teardown: Task<Void, Never>?
    @ObservationIgnored private var requestedHostID: String?
    @ObservationIgnored private var watchingHosts = false
    @ObservationIgnored private var foreground = true
    @ObservationIgnored private var lifecycleEpoch = 0
    @ObservationIgnored private var lifecycleTask: Task<Void, Never>?
    @ObservationIgnored private var botsVisible = false
    @ObservationIgnored private var sessionOpen = false
    @ObservationIgnored private var autoConnectPending = true
    @ObservationIgnored private var client: IosRemoteControlClient?
    @ObservationIgnored private var generation: UInt64 = 0
    @ObservationIgnored private let connectionEpoch = RemoteAppRequestEpoch()
    private final class SourceReference {
        weak var value: RemoteConversationSource?
        init(_ value: RemoteConversationSource) { self.value = value }
    }
    @ObservationIgnored private var sources: [String: SourceReference] = [:]
    /// The Bots whose chats a screen has had listed on this connection (see `showBot`).
    @ObservationIgnored private var shownBots: Set<RemoteBotKey> = []

    init(
        mediaHostComposition: IOSNativeMediaHostComposition? = nil,
        mediaHostInitializationError: String? = nil
    ) {
        self.mediaHostComposition = mediaHostComposition
        self.mediaHostInitializationError = mediaHostInitializationError
    }

    func configure(uid: String?, watchHosts: Bool = true) async {
        if uid == self.uid, client != nil {
            if watchHosts { await startWatchingHosts() }
            return
        }
        teardown?.cancel(); teardown = nil
        generation &+= 1
        connectionEpoch.reset()
        let mine = generation
        let previous = client
        client = nil
        self.uid = uid
        hosts = []
        hostsLoaded = false
        connection = nil
        requestedHostID = nil
        sessions = []
        botDirectory.removeAll()
        shownBots.removeAll()
        error = nil
        waitingChats = 0
        watchingHosts = false
        autoConnectPending = true
        for source in sources.values { source.value?.invalidate() }
        sources.removeAll()
        previous?.setBotsListener(listener: nil)
        await previous?.stopWatchingHosts()
        await previous?.disconnect()
        guard mine == generation, !Task.isCancelled else { return }
        await snapshots.configure(uid: uid)
        guard let uid, mine == generation, !Task.isCancelled else { return }
        guard let mediaHostComposition else {
            self.error = mediaHostInitializationError
                ?? "The app media host was not composed before Remote Control started."
            return
        }
        do {
            let created = try await buildIosRemoteControlClient(
                databaseUrl: "https://lingxi-5e4b2-default-rtdb.firebaseio.com", uid: uid,
                deviceName: UIDevice.current.name,
                appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.1.0",
                tokens: RemoteTokenProvider(uid: uid), keys: RemoteKeyStore(uid: uid),
                listener: RemoteListener(manager: self, generation: mine),
                mediaHost: mediaHostComposition.nativeHost,
                mediaConfigProvider: mediaHostComposition.remoteMediaConfigProvider
            )
            guard mine == generation, !Task.isCancelled else {
                await created.disconnect()
                return
            }
            created.setBotsListener(listener: RemoteBotsListener(manager: self, generation: mine))
            created.setPushInstallationId(id: PushRegistration.shared.remoteInstallationId)
            client = created
            if watchHosts { await startWatchingHosts() }
        } catch {
            guard mine == generation else { return }
            self.error = String(localized: "remote_hosts_error")
        }
    }

    func activeClient() throws -> IosRemoteControlClient {
        guard let client, uid == FirebaseAccount.shared.user?.uid else { throw RemoteFfiError.NotConnected }
        return client
    }

    func connect(_ hostID: String) async throws {
        guard foreground else { throw RemoteFfiError.NotConnected }
        teardown?.cancel(); teardown = nil
        let client = try activeClient()
        if requestedHostID != hostID {
            connectionEpoch.advance()
            sessions = []
            botDirectory.removeAll()
            shownBots.removeAll()
            let obsolete = sources.filter { $0.value.value?.hostID != hostID }
            for (id, source) in obsolete { source.value?.invalidate(); sources.removeValue(forKey: id) }
        }
        requestedHostID = hostID
        try await client.connect(hostId: hostID)
    }

    /// Presence remains cheap and independent of the one live connection.
    private func startWatchingHosts() async {
        guard foreground, !watchingHosts, let client else { return }
        watchingHosts = true
        await client.watchHosts()
    }

    func setSelected(_ selected: Bool) { setPresentation(selected: selected, sessionOpen: sessionOpen) }

    func setPresentation(selected: Bool, sessionOpen: Bool) {
        botsVisible = selected
        self.sessionOpen = sessionOpen
        teardown?.cancel(); teardown = nil
        if selected {
            Task { await startWatchingHosts(); autoConnectIfReady() }
        } else if !sessionOpen {
            teardown = Task { [weak self] in
                do { try await Task.sleep(for: BotsLogic.connectionGrace) } catch { return }
                guard let self, !self.botsVisible, !self.sessionOpen else { return }
                await self.closeConnection()
            }
        }
    }

    func setForeground(_ active: Bool) async {
        foreground = active
        lifecycleEpoch += 1
        let epoch = lifecycleEpoch, mine = generation, previous = lifecycleTask
        let task = Task { [weak self] in
            await previous?.value
            guard let self, epoch == self.lifecycleEpoch, mine == self.generation else { return }
            if active {
                self.autoConnectPending = true
                if self.botsVisible {
                    await self.startWatchingHosts()
                    guard epoch == self.lifecycleEpoch, mine == self.generation else { return }
                    let openSources = self.sources.values.compactMap(\.value)
                    if openSources.isEmpty { self.autoConnectIfReady() }
                    else {
                        for source in openSources {
                            Task { [weak self] in
                                guard let self, self.foreground, epoch == self.lifecycleEpoch, mine == self.generation else { return }
                                await source.start()
                            }
                        }
                    }
                }
            } else {
                self.teardown?.cancel(); self.teardown = nil
                self.watchingHosts = false
                let closingClient = self.client
                await self.closeConnection(preservingSessions: true)
                guard epoch == self.lifecycleEpoch, mine == self.generation else { return }
                await closingClient?.stopWatchingHosts()
                guard epoch == self.lifecycleEpoch, mine == self.generation else { return }
                await self.snapshots.flush()
            }
        }
        lifecycleTask = task
        await task.value
    }

    private func autoConnectIfReady() {
        guard foreground, botsVisible, hostsLoaded, autoConnectPending else { return }
        autoConnectPending = false
        let activeHost = requestedHostID ?? connection.flatMap { $0.phase == .idle ? nil : $0.hostId }
        guard let target = BotsLogic.autoConnectTarget(signedIn: uid != nil, connectedHostID: activeHost,
            lastHostID: snapshots.lastHostID, hosts: hosts.map(BotsLogic.Host.init)) else { return }
        Task { try? await connect(target) }
    }

    func closeConnection(preservingSessions: Bool = false) async {
        requestedHostID = nil
        connectionEpoch.advance()
        if preservingSessions {
            for source in sources.values { source.value?.suspendConnection() }
        } else {
            for source in sources.values { source.value?.invalidate() }
            sources.removeAll()
        }
        shownBots.removeAll()
        sessions = []; botDirectory.removeAll(); connection = nil; waitingChats = 0
        await client?.disconnect()
    }

    private func appRequestContext(hostID: String) throws -> (IosRemoteControlClient, UInt64, UInt64) {
        let client = try activeClient()
        guard connection?.hostId == hostID, connection?.phase == .ready,
              connectionEpoch.permits(hostID: hostID) else { throw RemoteFfiError.NotConnected }
        return (client, generation, connectionEpoch.value)
    }

    private func checkAppRequest(hostID: String, generation: UInt64, epoch: UInt64) throws {
        guard !Task.isCancelled, generation == self.generation, epoch == connectionEpoch.value,
              uid == FirebaseAccount.shared.user?.uid,
              connection?.hostId == hostID, connection?.phase == .ready else { throw CancellationError() }
    }

    func listApps(hostID: String) async throws -> [RemoteLocalAppFfi] {
        let (client, generation, epoch) = try appRequestContext(hostID: hostID)
        let apps = try await client.listApps()
        try checkAppRequest(hostID: hostID, generation: generation, epoch: epoch)
        return apps
    }

    func openApp(hostID: String, app: RemoteLocalAppFfi) async throws -> URL {
        let (client, generation, epoch) = try appRequestContext(hostID: hostID)
        let ticket = try await client.openApp(appId: app.appId)
        try checkAppRequest(hostID: hostID, generation: generation, epoch: epoch)
        guard ticket.appId == app.appId, ticket.revision == app.revision,
              let url = RemoteAppLaunchURL.validate(ticket.url, expiresAt: ticket.expiresAt) else {
            throw RemoteFfiError.InvalidArgument(message: "Invalid or expired app launch")
        }
        return url
    }

    func retryHosts() async {
        guard let client else { await configure(uid: FirebaseAccount.shared.user?.uid); return }
        error = nil
        hostsLoaded = false
        await client.stopWatchingHosts()
        await client.watchHosts()
    }

    func retry() async { await client?.retry() }
    func repair(_ hostID: String) async throws { try await activeClient().repairHost(hostId: hostID) }
    func forget(_ hostID: String) async {
        let mine = generation, owner = uid
        let client = client
        let currentHostID = connection?.phase == .idle ? nil : connection?.hostId
        await connectionEpoch.forget(hostID: hostID, currentHostID: currentHostID) {
            await client?.forgetHost(hostId: hostID)
        }
        guard mine == generation, owner == uid else { return }
        await snapshots.forget(hostID)
    }
    func nudge() async {
        client?.setPushInstallationId(id: PushRegistration.shared.remoteInstallationId)
        await client?.nudge()
    }

    func follow(_ source: RemoteConversationSource) async throws {
        guard foreground else { throw RemoteFfiError.NotConnected }
        let client = try activeClient()
        let mine = generation
        try await connect(source.hostID)
        guard foreground, mine == generation, !Task.isCancelled else { throw CancellationError() }
        sources[source.sessionID] = SourceReference(source)
        try await client.attachSession(sessionId: source.sessionID)
        guard sources[source.sessionID]?.value === source, mine == generation else { throw CancellationError() }
        try await client.setSessionForeground(sessionId: source.sessionID, foreground: true)
    }

    func unfollow(_ source: RemoteConversationSource) async {
        guard sources[source.sessionID]?.value === source else { return }
        sources.removeValue(forKey: source.sessionID)
        try? await client?.detachSession(sessionId: source.sessionID)
    }

    /// A Bot's screen is up: list the Bot's chats and keep them current until `hideBot`. The computer is
    /// dialled first when another one is current, as `follow` does for a session; screens are counted per
    /// connection, so what a replaced connection held is gone, and this is what activates them again.
    func showBot(_ key: RemoteBotKey) async throws {
        let mine = generation
        guard connection?.hostId == key.hostID, connection?.phase == .ready else { throw RemoteFfiError.NotConnected }
        let epoch = connectionEpoch.value
        guard mine == generation, !Task.isCancelled else { throw CancellationError() }
        let client = try activeClient()
        guard shownBots.insert(key).inserted else { return }
        do { try await client.activateBotThreads(botId: key.botID) } catch {
            if mine == generation, epoch == connectionEpoch.value { shownBots.remove(key) }
            throw error
        }
        // The screen left while the call was on its way, and its `hideBot` found nothing to undo.
        if mine == generation, epoch == connectionEpoch.value, !shownBots.contains(key) {
            client.deactivateBotThreads(botId: key.botID)
        }
    }

    /// The Bot's screen is gone.
    func hideBot(_ key: RemoteBotKey) {
        guard shownBots.remove(key) != nil else { return }
        client?.deactivateBotThreads(botId: key.botID)
    }

    /// List the Bot's chats again: the client has no call for it, so the screen is taken off and put on.
    func reloadBot(_ key: RemoteBotKey) async throws {
        guard shownBots.contains(key) else { return try await showBot(key) }
        guard connection?.hostId == key.hostID, connection?.phase == .ready else { throw RemoteFfiError.NotConnected }
        let client = try activeClient()
        let mine = generation, epoch = connectionEpoch.value
        client.deactivateBotThreads(botId: key.botID)
        do { try await client.activateBotThreads(botId: key.botID) } catch {
            if mine == generation, epoch == connectionEpoch.value { shownBots.remove(key) }
            throw error
        }
        if mine == generation, epoch == connectionEpoch.value, !shownBots.contains(key) { client.deactivateBotThreads(botId: key.botID) }
    }

    /// Start a chat with the Bot and return its session id, for the session screen to follow.
    func createBotChat(_ key: RemoteBotKey) async throws -> String {
        let (client, mine, epoch) = try appRequestContext(hostID: key.hostID)
        let session = try await client.createBotThread(botId: key.botID)
        try checkAppRequest(hostID: key.hostID, generation: mine, epoch: epoch)
        return session
    }

    func waitForReady(_ hostID: String) async throws {
        let mine = generation
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(120))
        while clock.now < deadline {
            try Task.checkCancellation()
            guard mine == generation, requestedHostID == hostID else { throw CancellationError() }
            if connection?.hostId == hostID, connection?.phase == .ready { return }
            if connection?.hostId == hostID, connection?.phase == .failed || connection?.phase == .idle { throw RemoteFfiError.NotConnected }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw RemoteFfiError.NotConnected
    }

    /// The Bot a chat belongs to, when this connection has been told.
    func botOfThread(_ sessionID: String) -> String? {
        client?.botOfThread(sessionId: sessionID)
    }

    func close() async {
        teardown?.cancel(); teardown = nil
        generation &+= 1
        connectionEpoch.reset()
        let previous = client
        client = nil
        uid = nil
        for source in sources.values { source.value?.invalidate() }
        sources.removeAll()
        hosts = []
        sessions = []
        botDirectory.removeAll()
        shownBots.removeAll()
        connection = nil
        waitingChats = 0
        requestedHostID = nil
        previous?.setBotsListener(listener: nil)
        await previous?.stopWatchingHosts()
        await previous?.disconnect()
    }

    fileprivate func receive(_ state: RemoteConnectionStateFfi, generation: UInt64) {
        guard generation == self.generation, state.hostId == requestedHostID else { return }
        connectionEpoch.observe(hostID: state.hostId, ready: state.phase == .ready)
        if state.hostId != connection?.hostId || state.phase != .ready || connection?.phase != .ready {
            connectionEpoch.advance()
        }
        connection = state
        if state.phase == .ready { snapshots.connected(state.hostId) }
        updateBadge()
    }
    fileprivate func receive(_ hosts: [RemoteHostFfi], generation: UInt64) {
        guard generation == self.generation else { return }
        self.hosts = hosts
        hostsLoaded = true
        error = nil
        autoConnectIfReady()
    }
    fileprivate func hostsFailed(generation: UInt64) {
        guard generation == self.generation else { return }
        error = String(localized: "remote_hosts_error")
    }
    fileprivate func receive(_ sessions: [RemoteSessionSummaryFfi], hostID: String, generation: UInt64) {
        guard generation == self.generation, hostID == connection?.hostId else { return }
        self.sessions = sessions
    }
    fileprivate func receive(_ update: RemoteSessionUpdateFfi, generation: UInt64) {
        guard generation == self.generation else { return }
        sources[update.sessionId]?.value?.receive(update)
    }
    func receive(bots state: RemoteBotsStateFfi, hostID: String, generation: UInt64) {
        guard generation == self.generation, hostID == connection?.hostId else { return }
        botDirectory.set(state, host: hostID)
        if case let .bots(bots) = state {
            snapshots.receive(bots: bots.map(BotsItem.init), hostID: hostID, nowMs: UInt64(Date().timeIntervalSince1970 * 1000))
        }
        updateBadge()
    }
    func receive(botThreads state: RemoteBotThreadsStateFfi, hostID: String, botID: String, generation: UInt64) {
        guard generation == self.generation, hostID == connection?.hostId else { return }
        botDirectory.set(state, host: hostID, bot: botID)
        if let chats = state.threads {
            snapshots.receive(chats: chats.map(BotsChat.init), hostID: hostID, botID: botID,
                              nowMs: UInt64(Date().timeIntervalSince1970 * 1000))
        }
    }

    var connectedHostID: String? {
        guard let connection else { return nil }
        switch connection.phase {
        case .ready, .reconnecting: return connection.hostId
        case .connecting: return connection.everReady ? connection.hostId : nil
        default: return nil
        }
    }

    private func updateBadge() {
        guard let state = connection, state.phase == .ready || state.phase == .reconnecting,
              case let .bots(bots) = botDirectory.bots(host: state.hostId) else { waitingChats = 0; return }
        waitingChats = TabLogic.botsBadgeCount(waitingThreads: bots.map { Int($0.waitingThreads) })
    }
}

/// Each Rust client receives its own generation-tagged callback object. Awaiting
/// the main actor keeps the Rust callback queue ordered across account changes.
@MainActor
private final class RemoteListener: IosRemoteListener {
    private weak var manager: RemoteControlManager?
    private let generation: UInt64
    init(manager: RemoteControlManager, generation: UInt64) {
        self.manager = manager
        self.generation = generation
    }
    func onConnectionState(state: RemoteConnectionStateFfi) async { manager?.receive(state, generation: generation) }
    func onHosts(hosts: [RemoteHostFfi]) async { manager?.receive(hosts, generation: generation) }
    func onHostsFailed(message: String) async { manager?.hostsFailed(generation: generation) }
    func onSessions(hostId: String, sessions: [RemoteSessionSummaryFfi]) async {
        manager?.receive(sessions, hostID: hostId, generation: generation)
    }
    func onSessionUpdate(update: RemoteSessionUpdateFfi) async { manager?.receive(update, generation: generation) }
}

/// The same for the Bots, which come through a listener of their own (`setBotsListener`).
@MainActor
private final class RemoteBotsListener: IosRemoteBotsListener {
    private weak var manager: RemoteControlManager?
    private let generation: UInt64
    init(manager: RemoteControlManager, generation: UInt64) {
        self.manager = manager
        self.generation = generation
    }
    func onBots(hostId: String, state: RemoteBotsStateFfi) async {
        manager?.receive(bots: state, hostID: hostId, generation: generation)
    }
    func onBotThreads(hostId: String, botId: String, state: RemoteBotThreadsStateFfi) async {
        manager?.receive(botThreads: state, hostID: hostId, botID: botId, generation: generation)
    }
}

@MainActor
private final class RemoteTokenProvider: IosRemoteTokenProvider {
    private let uid: String
    init(uid: String) { self.uid = uid }
    func idToken(forceRefresh: Bool) async throws -> String {
        #if canImport(FirebaseAuth)
        guard let user = Auth.auth().currentUser, user.uid == uid else {
            throw RemoteFfiError.Token(message: "Account changed")
        }
        let token = try await user.getIDToken(forcingRefresh: forceRefresh)
        guard Auth.auth().currentUser?.uid == uid else { throw RemoteFfiError.Token(message: "Account changed") }
        return token
        #else
        throw RemoteFfiError.Token(message: "Firebase unavailable")
        #endif
    }
}
#endif

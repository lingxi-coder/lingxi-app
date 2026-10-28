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
    private(set) var error: String?
    @ObservationIgnored private var client: IosRemoteControlClient?
    @ObservationIgnored private var generation: UInt64 = 0
    @ObservationIgnored private var connectionEpoch: UInt64 = 0
    private final class SourceReference {
        weak var value: RemoteConversationSource?
        init(_ value: RemoteConversationSource) { self.value = value }
    }
    @ObservationIgnored private var sources: [String: SourceReference] = [:]

    init(
        mediaHostComposition: IOSNativeMediaHostComposition? = nil,
        mediaHostInitializationError: String? = nil
    ) {
        self.mediaHostComposition = mediaHostComposition
        self.mediaHostInitializationError = mediaHostInitializationError
    }

    func configure(uid: String?) async {
        guard uid != self.uid || client == nil else { return }
        generation &+= 1
        let mine = generation
        let previous = client
        client = nil
        self.uid = uid
        hosts = []
        hostsLoaded = false
        connection = nil
        sessions = []
        error = nil
        for source in sources.values { source.value?.invalidate() }
        sources.removeAll()
        await previous?.stopWatchingHosts()
        await previous?.disconnect()
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
            created.setPushInstallationId(id: PushRegistration.shared.remoteInstallationId)
            client = created
            await created.watchHosts()
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
        let client = try activeClient()
        if connection?.hostId != hostID {
            connectionEpoch &+= 1
            sessions = []
            for source in sources.values { source.value?.invalidate() }
            sources.removeAll()
        }
        try await client.connect(hostId: hostID)
    }

    private func appRequestContext(hostID: String) throws -> (IosRemoteControlClient, UInt64, UInt64) {
        let client = try activeClient()
        guard connection?.hostId == hostID, connection?.phase == .ready else { throw RemoteFfiError.NotConnected }
        return (client, generation, connectionEpoch)
    }

    private func checkAppRequest(hostID: String, generation: UInt64, epoch: UInt64) throws {
        guard !Task.isCancelled, generation == self.generation, epoch == connectionEpoch,
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
    func forget(_ hostID: String) async { await client?.forgetHost(hostId: hostID) }
    func nudge() async {
        client?.setPushInstallationId(id: PushRegistration.shared.remoteInstallationId)
        await client?.nudge()
    }

    func follow(_ source: RemoteConversationSource) async throws {
        let client = try activeClient()
        let mine = generation
        try await connect(source.hostID)
        guard mine == generation, !Task.isCancelled else { throw CancellationError() }
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

    func close() async {
        generation &+= 1
        let previous = client
        client = nil
        uid = nil
        for source in sources.values { source.value?.invalidate() }
        sources.removeAll()
        hosts = []
        sessions = []
        connection = nil
        await previous?.stopWatchingHosts()
        await previous?.disconnect()
    }

    fileprivate func receive(_ state: RemoteConnectionStateFfi, generation: UInt64) {
        guard generation == self.generation else { return }
        if state.hostId != connection?.hostId || state.phase != .ready || connection?.phase != .ready {
            connectionEpoch &+= 1
        }
        connection = state
    }
    fileprivate func receive(_ hosts: [RemoteHostFfi], generation: UInt64) {
        guard generation == self.generation else { return }
        self.hosts = hosts
        hostsLoaded = true
        error = nil
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

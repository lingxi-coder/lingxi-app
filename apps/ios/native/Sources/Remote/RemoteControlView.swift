import SwiftUI

#if canImport(harness_runtimeFFI)
/// The Bots tab owns the tree and a stack whose Back destination is always that tree.
@MainActor
struct RemoteControlView: View {
    let openAccount: () -> Void
    @Environment(RemoteControlManager.self) private var manager
    @Environment(TabCoordinator.self) private var tabs
    @Environment(\.isWideLayout) private var wide
    @State private var path: [RemoteRoute] = []
    @State private var wideRoute: RemoteRoute?
    @State private var configuredUID: String?
    @SceneStorage("bots.navigation") private var savedNavigation = ""

    var body: some View {
        Group {
            if wide {
                HStack(spacing: 0) {
                    NavigationStack { tree }.frame(width: 360)
                    Divider()
                    NavigationStack {
                        if let wideRoute { destination(wideRoute) }
                        else { ContentUnavailableView("remote_sessions_title", systemImage: "bubble.left.and.bubble.right") }
                    }.frame(maxWidth: .infinity)
                }
            } else {
                NavigationStack(path: $path) {
                    tree.navigationDestination(for: RemoteRoute.self) { destination($0).hidesTabBarWhenPushed(in: .bots) }
                }
            }
        }
        .task(id: FirebaseAccount.shared.user?.uid) {
            let uid = FirebaseAccount.shared.user?.uid
            if configuredUID != uid { path = []; wideRoute = nil; configuredUID = nil }
            await manager.configure(uid: uid)
            guard !Task.isCancelled, uid == FirebaseAccount.shared.user?.uid else { return }
            if configuredUID != uid, let uid {
                configuredUID = uid
                restoreNavigation(uid: uid)
            }
            applyPendingRequest()
            updatePresentation()
        }
        .onChange(of: tabs.botsRequest) { _, _ in applyPendingRequest() }
        .onChange(of: path) { _, _ in saveNavigation(); updatePresentation() }
        .onChange(of: wideRoute) { _, _ in saveNavigation(); updatePresentation() }
        .onChange(of: tabs.selected) { _, _ in updatePresentation() }
        .onChange(of: tabs.popToRootRequests[.bots]) { _, _ in path = []; wideRoute = nil }
        .onChange(of: wide) { _, value in
            if value { wideRoute = path.last; path = [] }
            else if let wideRoute { path = [wideRoute]; self.wideRoute = nil }
            updatePresentation()
        }
    }

    private var tree: some View {
        ScrollViewReader { proxy in
            Group {
                if FirebaseAccount.shared.user == nil {
                    ContentUnavailableView {
                        Label("remote_signed_out_title", systemImage: "person.crop.circle")
                    } description: { Text("remote_signed_out_body") } actions: {
                        Button("remote_sign_in", action: openAccount)
                    }
                } else if let error = manager.error, manager.hosts.isEmpty {
                    ContentUnavailableView {
                        Label("remote_computers", systemImage: "desktopcomputer")
                    } description: { Text(error) } actions: {
                        Button("common_retry") { Task { await manager.retryHosts() } }
                    }
                } else if !manager.hostsLoaded {
                    ProgressView("remote_loading")
                } else if manager.hosts.isEmpty {
                    ContentUnavailableView("remote_no_computers", systemImage: "desktopcomputer", description: Text("remote_no_computers_help"))
                } else {
                    List {
                        Color.clear.frame(height: 0).id("bots.top").listRowSeparator(.hidden)
                        if let error = manager.error {
                            Text(error).foregroundStyle(.secondary)
                            Button("common_retry") { Task { await manager.retryHosts() } }
                        }
                        ForEach(sortedHosts, id: \.hostId) { host in
                            BotsHostSection(manager: manager, host: host,
                                openComputer: { open(.host(host.hostId)) },
                                openChat: { session, title in open(.session(host: host.hostId, session: session, title: title)) },
                                canNavigate: { tabs.selected == .bots && configuredUID == manager.uid })
                        }
                    }
                    .accessibilityIdentifier("bots.tree")
                }
            }
            .navigationTitle("remote_bots")
            .onChange(of: tabs.scrollToTopRequests[.bots]) { _, _ in proxy.scrollTo("bots.top", anchor: .top) }
        }
    }

    private var sortedHosts: [RemoteHostFfi] {
        let byID = Dictionary(manager.hosts.map { ($0.hostId, $0) }, uniquingKeysWith: { _, b in b })
        return BotsLogic.sortHosts(manager.hosts.map(BotsLogic.Host.init), connectedHostID: manager.connectedHostID).compactMap { byID[$0.id] }
    }
    @ViewBuilder private func destination(_ route: RemoteRoute) -> some View {
        switch route {
        case .host(let id):
            RemoteHostView(manager: manager, hostID: id) { session in
                open(.session(host: id, session: session.sessionId, title: session.title))
            }
        case let .session(host, session, title):
            if let uid = configuredUID {
                RemoteSessionView(hostID: host, sessionID: session, title: title, uid: uid, manager: manager)
                    .id(uid + ":" + host + ":" + session)
            }
        }
    }
    private func open(_ route: RemoteRoute) {
        if wide { wideRoute = route } else { path.append(route) }
    }
    private func applyPendingRequest() {
        guard configuredUID != nil, let request = tabs.botsRequest else { return }
        tabs.botsRequest = nil
        let routes = RemoteRoute.stack(host: request.hostID, session: request.sessionID, title: String(localized: "remote_untitled"))
        if wide { wideRoute = routes.last } else { path = routes }
    }
    private func updatePresentation() {
        tabs.reportDepth(.bots, wide ? 0 : path.count)
        let route = wide ? wideRoute : path.last
        manager.setPresentation(selected: tabs.selected == .bots, sessionOpen: route?.isSession == true)
    }
    private func saveNavigation() {
        guard let uid = configuredUID, let data = try? JSONEncoder().encode(BotsNavigationState(uid: uid, routes: wide ? wideRoute.map { [$0] } ?? [] : path)) else { return }
        savedNavigation = String(decoding: data, as: UTF8.self)
    }
    private func restoreNavigation(uid: String) {
        guard let data = savedNavigation.data(using: .utf8), let saved = try? JSONDecoder().decode(BotsNavigationState.self, from: data), saved.uid == uid else { return }
        if wide { wideRoute = saved.routes.last } else { path = saved.routes }
    }
}

@MainActor
struct RemoteHostView: View {
    let manager: RemoteControlManager
    let hostID: String
    let select: (RemoteSessionSummaryFfi) -> Void
    @Environment(\.scenePhase) private var scenePhase
    @State private var error: String?
    @State private var appBrowser = RemoteAppBrowserLifetime()
    private var host: RemoteHostFfi? { manager.hosts.first { $0.hostId == hostID } }

    var body: some View {
        List {
            RemoteConnectionSection(state: manager.connection?.hostId == hostID ? manager.connection : nil, retry: { Task { await manager.retry() } },
                repair: { Task { try? await manager.repair(hostID) } })
            if let error { Text(error).foregroundStyle(.red) }
            if manager.connection?.phase == .ready, manager.connection?.hostId == hostID {
                RemoteAppsSection(manager: manager, hostID: hostID,
                    browserGeneration: { appBrowser.generation },
                    presentBrowser: { url, generation in appBrowser.present(url, generation: generation) },
                    invalidateBrowserLaunch: { appBrowser.invalidatePending() })
                if manager.sessions.isEmpty {
                    Section { Text("remote_no_sessions_help").foregroundStyle(.secondary) } header: { Text("remote_no_sessions") }
                }
                Section {
                    ForEach(manager.sessions, id: \.sessionId) { session in
                        Button { select(session) } label: { RemoteSessionRow(session: session) }
                            .accessibilityIdentifier("remote.session.\(session.sessionId)")
                    }
                } header: { Text("remote_sessions_title") }
            } else if host?.online == false {
                Text("remote_offline_help").foregroundStyle(.secondary)
            }
            if host?.paired == true {
                Section {
                    Button("remote_pair_again") { Task { try? await manager.repair(hostID) } }
                    Button("remote_forget", role: .destructive) { Task { await manager.forget(hostID) } }
                } footer: { Text("remote_forget_help") }
            }
        }
        .navigationTitle(host?.name ?? String(localized: "remote_title"))
        .sheet(item: Binding(get: { appBrowser.ticket }, set: { if $0 == nil { appBrowser.dismiss() } })) { ticket in
            LocalAppSystemBrowser(url: ticket.url)
        }
        .task { do { try await manager.connect(hostID) } catch { self.error = String(localized: "remote_err_generic") } }
        .onChange(of: scenePhase) { _, phase in if phase == .active { Task { await manager.nudge() } } }
    }
}

/// The desktop owns these apps and their runtime. Tickets live only until opened.
@MainActor
private struct RemoteAppsSection: View {
    let manager: RemoteControlManager
    let hostID: String
    let browserGeneration: () -> UInt64
    let presentBrowser: (URL, UInt64) -> Void
    let invalidateBrowserLaunch: () -> Void
    @Environment(\.openURL) private var openURL
    @State private var apps: [RemoteLocalAppFfi] = []
    @State private var loading = false
    @State private var opening: String?
    @State private var error: String?
    @State private var request: UInt64 = 0
    @State private var listRequest: UInt64 = 0

    var body: some View {
        Section {
            Text("remote_apps_intro")
                .font(.footnote).foregroundStyle(.secondary)
            if let error { Text(error).foregroundStyle(.red) }
            if loading { ProgressView() }
            else if apps.isEmpty { Text("remote_apps_empty").foregroundStyle(.secondary) }
            ForEach(apps, id: \.appId) { app in
                HStack {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(app.name)
                        if !app.summary.isEmpty { Text(app.summary).font(.caption).foregroundStyle(.secondary) }
                        HStack(spacing: 4) { Text("remote_apps_desktop"); Text("·"); Text(phase(app.phase)) }
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if opening == app.appId { ProgressView() }
                    else {
                        Button("common_open") { Task { await open(app, inApp: false) } }
                            .buttonStyle(.borderless)
                        Menu {
                            Button("remote_apps_open_default") { Task { await open(app, inApp: false) } }
                            Button("remote_apps_open_in_app") { Task { await open(app, inApp: true) } }
                        } label: { Image(systemName: "ellipsis.circle") }
                            .accessibilityLabel("remote_apps_open_choice")
                    }
                }
            }
            Button("remote_apps_refresh") { Task { await refresh() } }.disabled(loading)
        } header: { Text("remote_apps_title") }
        .task { await refresh() }
        .onDisappear { request &+= 1; invalidateBrowserLaunch() }
    }

    private func phase(_ value: String) -> LocalizedStringKey {
        switch value {
        case "installing": "remote_apps_phase_installing"
        case "building": "remote_apps_phase_building"
        case "starting": "remote_apps_phase_starting"
        case "running": "remote_apps_phase_running"
        case "stopping": "remote_apps_phase_stopping"
        case "failed": "remote_apps_phase_failed"
        default: "remote_apps_phase_stopped"
        }
    }

    private func refresh() async {
        listRequest &+= 1
        let mine = request
        let list = listRequest
        loading = true; error = nil
        do {
            let result = try await manager.listApps(hostID: hostID)
            guard !Task.isCancelled, mine == request, list == listRequest else { return }
            apps = result; loading = false
        } catch {
            guard !Task.isCancelled, mine == request, list == listRequest else { return }
            apps = []; loading = false; self.error = String(localized: "remote_apps_load_error")
        }
    }

    private func open(_ app: RemoteLocalAppFfi, inApp: Bool) async {
        guard opening == nil else { return }
        opening = app.appId; error = nil
        let mine = request
        let browserRequest = browserGeneration()
        defer { opening = nil }
        do {
            let url = try await manager.openApp(hostID: hostID, app: app)
            guard !Task.isCancelled, mine == request else { return }
            if inApp { presentBrowser(url, browserRequest) }
            else { openURL(url) { accepted in if !accepted { error = String(localized: "remote_apps_browser_error") } } }
        } catch is CancellationError { }
        catch { if mine == request { self.error = String(localized: "remote_apps_open_error") } }
    }
}

struct RemoteConnectionSection: View {
    let state: RemoteConnectionStateFfi?
    let retry: () -> Void
    let repair: () -> Void
    var body: some View {
        Section {
            switch state?.phase {
            case .ready:
                Label(state?.transport == .webrtc ? "remote_direct_help" : "remote_tunnel_help", systemImage: "lock.shield")
            case .pairing:
                Text("remote_pairing_title").font(.headline)
                Text(state?.pairingCode ?? "").font(.largeTitle.monospacedDigit()).textSelection(.enabled)
                    .accessibilityLabel(Text("remote_pairing_code"))
                Text("remote_pairing_waiting").foregroundStyle(.secondary)
            case .failed:
                Text(failureMessage(state?.failure?.kind)).foregroundStyle(.secondary)
                Button("common_retry", action: retry)
                if state?.failure?.kind == .revoked {
                    Button("remote_pair_again", action: repair)
                    Text("remote_pair_again_note").font(.caption)
                }
            case .reconnecting: ProgressView("remote_reconnecting")
            default: ProgressView("remote_loading")
            }
        }
    }

    private func failureMessage(_ kind: RemoteFailureKindFfi?) -> LocalizedStringKey {
        switch kind {
        case .revoked: "remote_fail_revoked"
        case .verification: "remote_fail_verification"
        case .pairingCodeMismatch: "remote_fail_pairing_mismatch"
        case .upgradeRequired: "remote_fail_upgrade"
        case .disabled: "remote_fail_disabled"
        case .denied: "remote_fail_denied"
        case .noAnswer: "remote_fail_no_answer"
        case .unknownHost: "remote_fail_unknown_host"
        case .capacity: "remote_fail_capacity"
        case .busy: "remote_fail_busy"
        case .expired: "remote_fail_expired"
        default: "remote_fail_network"
        }
    }
}

@MainActor
private struct RemoteSessionModeLabel: View {
    @ObservedObject var model: ConversationModel

    var body: some View {
        if model.sessionModeKnown {
            VStack(spacing: 2) {
                Text(model.sessionMode.title)
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(8)
            .accessibilityIdentifier("remote.session-mode")
        }
    }
}

private struct RemoteSessionLoadingError: View {
    @ObservedObject var model: ConversationModel

    var body: some View {
        if let error = model.error {
            Text(error.message).font(.caption).foregroundStyle(.secondary).padding(8)
        }
    }
}

struct RemoteSessionView: View {
    let title: String
    let manager: RemoteControlManager
    @Environment(\.scenePhase) private var scenePhase
    @State private var source: RemoteConversationSource
    @State private var draft: RemoteDraftOwner
    @State private var voice = VoiceInteractionController()

    init(hostID: String, sessionID: String, title: String, uid: String, manager: RemoteControlManager) {
        self.title = title
        self.manager = manager
        let source = RemoteConversationSource(hostID: hostID, sessionID: sessionID, uid: uid, manager: manager)
        _source = State(initialValue: source)
        _draft = State(initialValue: source.draftOwner)
    }

    var body: some View {
        VStack(spacing: 0) {
            if manager.needsSessionReconnect(hostID: source.hostID) {
                HStack {
                    Text("remote_err_generic")
                    Button("common_retry") { Task { await source.start() } }
                }.font(.caption).padding(8)
            }
            if source.phase != .live {
                Text(source.phase == .closed ? "remote_session_closed" : source.phase == .loading ? "remote_session_loading" : "remote_session_offline")
                    .font(.caption).foregroundStyle(.secondary).padding(8)
            }
            if source.hasOmittedContent { Text("remote_omitted_note").font(.caption).padding(8) }
            RemoteSessionModeLabel(model: source.model)
            if source.computerAccessPending { Text("remote_computer_access_note").font(.caption).padding(8) }
            if source.answeredElsewhere { Text("remote_answered_elsewhere").font(.caption).padding(8) }
            if source.hasOlder {
                Button(source.loadingOlder ? "remote_loading_older" : "remote_load_older", action: source.loadOlder)
                    .disabled(source.loadingOlder).padding(6)
            }
            if !source.isApprovalCapability {
                RemoteFilesView(ownerUID: manager.uid, manager: manager, hostID: source.hostID, sessionID: source.sessionID,
                    enabled: source.model.sessionModeKnown && source.phase == .live && source.hasReadyConnection && manager.connection?.transport == .webrtc)
                    .id("\(manager.uid ?? "")/\(source.hostID)/\(source.sessionID)")
            }
            if !source.model.sessionModeKnown {
                RemoteSessionLoadingError(model: source.model)
            }
            if source.model.sessionModeKnown {
                let owner = draft
                ChatView(session: SessionRef(id: source.sessionID, title: title),
                    draft: Binding(get: { owner.draft.text }, set: { owner.draft.text = $0 }),
                    voiceInteraction: voice, source: source, draftRevision: { owner.draftRevision },
                    draftSourceCodeUnits: Binding(get: { owner.draft.sourceCodeUnits }, set: { owner.draft.sourceCodeUnits = $0 }),
                    attachmentBinding: Binding(get: { owner.draft.attachment }, set: { owner.draft.attachment = $0 }),
                    sessionMode: source.sessionMode, workspaceName: title,
                    workspaceKey: ConversationScope.remote(hostID: source.hostID, sessionID: source.sessionID).workspaceKey)
                    .disabled(source.phase != .live || !source.hasReadyConnection)
                    .background {
                        if source.phase == .live && source.hasReadyConnection {
                            EnginePermissionPromptHost(model: source.model,
                                onApprove: source.approvePermission, onDeny: source.denyPermission)
                        }
                    }
            }
            if source.isApprovalCapability {
                Text("remote_team_approval").padding()
                    .background {
                        if source.phase == .live && source.hasReadyConnection {
                            EnginePermissionPromptHost(model: source.model,
                                onApprove: source.approvePermission, onDeny: source.denyPermission)
                        }
                    }
            }
            ForEach(source.unsentPrompts) { prompt in
                HStack {
                    Text(prompt.text).lineLimit(2)
                    Spacer()
                    Button("remote_restore_unsent") { restoreUnsent(prompt) }
                        .disabled(!draft.draft.text.isEmpty || draft.draft.sourceCodeUnits != nil || draft.draft.attachment != nil)
                }.font(.caption).padding(10)
            }
            ForEach(source.outgoing, id: \.id) { prompt in
                HStack {
                    Text(prompt.text).lineLimit(2)
                    Spacer()
                    if prompt.status == .failed {
                        Button("common_retry") { source.retryPrompt(prompt.id) }
                        Button("common_close") { source.dismissPrompt(prompt.id) }
                    } else { ProgressView() }
                }.font(.caption).padding(10)
            }
        }
        // The reused chat draws its background beyond the safe area. Keep its
        // scrolling transcript inside this navigation destination's bounds.
        .clipped()
        .navigationTitle(title)
        .task { await source.start() }
        .onDisappear { Task { await source.stop() } }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { source.handleForeground() }
            else if phase == .background { source.handleBackground() }
        }
    }

    private func restoreUnsent(_ prompt: RemoteUnsentPrompt) {
        if draft.draft.restore(prompt) { source.removeUnsentPrompt(prompt.id) }
    }
}
#endif

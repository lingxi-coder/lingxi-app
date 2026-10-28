import SwiftUI

struct RemoteControlDestination: Identifiable {
    let id = UUID()
    var hostID: String?
    var sessionID: String?
}

#if canImport(harness_runtimeFFI)
private enum RemoteRoute: Hashable {
    case host(String)
    case session(host: String, session: String, title: String)
}

@MainActor
struct RemoteControlView: View {
    let destination: RemoteControlDestination
    let openAccount: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var manager: RemoteControlManager
    @State private var path: [RemoteRoute] = []
    @State private var configuredUID: String?

    init(
        destination: RemoteControlDestination,
        openAccount: @escaping () -> Void,
        mediaHostComposition: IOSNativeMediaHostComposition?,
        mediaHostInitializationError: String?
    ) {
        self.destination = destination
        self.openAccount = openAccount
        _manager = State(initialValue: RemoteControlManager(
            mediaHostComposition: mediaHostComposition,
            mediaHostInitializationError: mediaHostInitializationError
        ))
    }

    var body: some View {
        NavigationStack(path: $path) {
            Group {
                if FirebaseAccount.shared.user == nil {
                    ContentUnavailableView {
                        Label("remote_signed_out_title", systemImage: "person.crop.circle")
                    } description: { Text("remote_signed_out_body") } actions: {
                        Button("remote_sign_in") { dismiss(); openAccount() }
                    }
                } else if let error = manager.error {
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
                        Section { Text("remote_computers_intro").foregroundStyle(.secondary) }
                        ForEach(manager.hosts, id: \.hostId) { host in
                            Button { path.append(.host(host.hostId)) } label: {
                                HStack {
                                    Image(systemName: "desktopcomputer")
                                    VStack(alignment: .leading) {
                                        Text(host.name)
                                        Text(host.online ? "remote_online" : "remote_offline").font(.caption).foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    if host.paired { Image(systemName: "checkmark.shield").accessibilityLabel(Text("remote_paired")) }
                                    Image(systemName: "chevron.right").foregroundStyle(.secondary)
                                }
                            }.accessibilityIdentifier("remote.host.\(host.hostId)")
                        }
                    }
                }
            }
            .navigationTitle("remote_title")
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() } } }
            .navigationDestination(for: RemoteRoute.self) { route in
                switch route {
                case let .host(id):
                    RemoteHostView(manager: manager, hostID: id) { session in
                        path.append(.session(host: id, session: session.sessionId, title: session.title))
                    }
                case let .session(host, session, title):
                    if let uid = configuredUID {
                        RemoteSessionView(hostID: host, sessionID: session, title: title, uid: uid, manager: manager)
                            .id(uid + ":" + host + ":" + session)
                    }
                }
            }
        }
        .task(id: FirebaseAccount.shared.user?.uid) {
            let uid = FirebaseAccount.shared.user?.uid
            // Full-screen UIKit capture hides this navigation stack and starts
            // its task again on return. Reset routes only for an account change,
            // so the session and its captured draft survive the camera.
            guard configuredUID != uid else { return }
            path = []
            configuredUID = nil
            await manager.configure(uid: uid)
            guard !Task.isCancelled, uid == FirebaseAccount.shared.user?.uid else { return }
            configuredUID = uid
            if uid != nil, let host = destination.hostID {
                path = [.host(host)]
                if let session = destination.sessionID {
                    path.append(.session(host: host, session: session, title: String(localized: "remote_untitled")))
                }
            }
        }
    }
}

@MainActor
private struct RemoteHostView: View {
    let manager: RemoteControlManager
    let hostID: String
    let select: (RemoteSessionSummaryFfi) -> Void
    @Environment(\.scenePhase) private var scenePhase
    @State private var error: String?
    @State private var appBrowser = RemoteAppBrowserLifetime()
    private var host: RemoteHostFfi? { manager.hosts.first { $0.hostId == hostID } }

    var body: some View {
        List {
            RemoteConnectionSection(state: manager.connection, retry: { Task { await manager.retry() } },
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
                ForEach(manager.sessions, id: \.sessionId) { session in
                    Button { select(session) } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(session.title.isEmpty ? String(localized: "remote_untitled") : session.title)
                            Text(session.projectLabel).font(.caption).foregroundStyle(.secondary)
                            Text(status(session.status)).font(.caption).foregroundStyle(.secondary)
                        }
                    }.accessibilityIdentifier("remote.session.\(session.sessionId)")
                }
            } else if host?.online == false {
                Text("remote_offline_help").foregroundStyle(.secondary)
            }
            if host?.paired == true {
                Section {
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

    private func status(_ value: RemoteSessionStatusFfi) -> LocalizedStringKey {
        switch value {
        case .running: "remote_status_running"
        case .requiresAction: "remote_status_requires_action"
        case .idle: "remote_status_idle"
        case .dormant: "remote_status_dormant"
        }
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

private struct RemoteConnectionSection: View {
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
private struct RemoteSessionView: View {
    let title: String
    let manager: RemoteControlManager
    @Environment(\.scenePhase) private var scenePhase
    @State private var source: RemoteConversationSource
    @State private var draft = ""
    @State private var voice = VoiceInteractionController()

    init(hostID: String, sessionID: String, title: String, uid: String, manager: RemoteControlManager) {
        self.title = title
        self.manager = manager
        _source = State(initialValue: RemoteConversationSource(hostID: hostID, sessionID: sessionID, uid: uid, manager: manager))
    }

    var body: some View {
        VStack(spacing: 0) {
            if source.phase != .live {
                Text(source.phase == .closed ? "remote_session_closed" : source.phase == .loading ? "remote_session_loading" : "remote_session_offline")
                    .font(.caption).foregroundStyle(.secondary).padding(8)
            }
            if source.hasOmittedContent { Text("remote_omitted_note").font(.caption).padding(8) }
            if source.computerAccessPending { Text("remote_computer_access_note").font(.caption).padding(8) }
            if source.answeredElsewhere { Text("remote_answered_elsewhere").font(.caption).padding(8) }
            if source.hasOlder {
                Button(source.loadingOlder ? "remote_loading_older" : "remote_load_older", action: source.loadOlder)
                    .disabled(source.loadingOlder).padding(6)
            }
            RemoteFilesView(ownerUID: manager.uid, manager: manager, hostID: source.hostID, sessionID: source.sessionID,
                enabled: source.phase == .live && manager.connection?.transport == .webrtc)
                .id("\(manager.uid ?? "")/\(source.hostID)/\(source.sessionID)")
            ChatView(session: SessionRef(id: source.sessionID, title: title), draft: $draft,
                voiceInteraction: voice, source: source, workspaceName: title,
                workspaceKey: ConversationScope.remote(hostID: source.hostID, sessionID: source.sessionID).workspaceKey)
                .disabled(source.phase != .live)
                .background {
                    if source.phase == .live {
                        EnginePermissionPromptHost(model: source.model,
                            onApprove: source.approvePermission, onDeny: source.denyPermission)
                    }
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
}
#endif

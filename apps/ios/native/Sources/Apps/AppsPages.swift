import SwiftUI
import UniformTypeIdentifiers

private struct LocalAppSourceDocument: FileDocument {
    static var readableContentTypes: [UTType] { [.zip] }
    let bytes: Data
    init(bytes: Data) { self.bytes = bytes }
    init(configuration: ReadConfiguration) throws {
        guard let data = configuration.file.regularFileContents else { throw MarketFailure("INVALID_ZIP") }
        bytes = data
    }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: bytes) }
}

struct AppCover: View {
    let name: String
    let category: String
    var size: CGFloat = 56
    var body: some View {
        Text(AppsModel.initial(name)).font(.system(size:size * 0.42,weight:.semibold))
            .foregroundStyle(.tint).frame(width:size,height:size)
            .background(Color.accentColor.opacity(0.12),in:.rect(cornerRadius:size * 0.24)).accessibilityHidden(true)
    }
}

struct LocalAppDevelopmentRequest: Sendable {
    let appId: String?
    let name: String
    let sourceDirectory: String
    let sourceID: String
    let sourceRequestID: String?
    let authoring: LocalAppAuthoring?
    let prompt: String
}

/// Catalog key for a Local App run phase reported by the host.
func localAppPhaseKey(_ phase: String) -> String {
    switch phase {
    case "installing": "remote_apps_phase_installing"
    case "building": "remote_apps_phase_building"
    case "starting": "remote_apps_phase_starting"
    case "running": "remote_apps_phase_running"
    case "stopping": "remote_apps_phase_stopping"
    case "failed": "remote_apps_phase_failed"
    default: "remote_apps_phase_stopped"
    }
}

struct AppsPage: View {
    @Environment(\.theme) private var t
    @Environment(\.openURL) private var openURL
    let library: UnifiedLocalApps
    let market: UnifiedAppMarket
    let projectStore: ProjectStore
    let onOpenApp: (String) -> Void
    let onShowDetails: (String) -> Void
    let currentMode: () -> SessionMode
    let captureImportPresentation: () -> LocalAppActionPresentation
    let onCreate: (String, String) async throws -> Void
    let onRecoverCreation: (LocalAppSourceCreateIntent) -> Void
    var onResumeBotHandoff: ((BotMarketHandoff) -> Void)? = nil
    @State private var query = ""
    @State private var creating = false
    @State private var importing = false
    @State private var importingMode: SessionMode?
    @State private var importPresentation: LocalAppActionPresentation?
    @State private var requiresCode = false
    @State private var error: String?
    private var filtered: [UnifiedAppRecord] { library.records.filter { query.isEmpty || $0.name.localizedStandardContains(query) } }
    var body: some View {
        List {
            if let pending = BotMarketHandoff.pending(), let onResumeBotHandoff {
                Section("bot_market_download") { Button("bot_market_resume_download") { onResumeBotHandoff(pending) } }
            }
            if let message = error ?? library.error { Text(message).foregroundStyle(.red).textSelection(.enabled) }
            if let message = projectStore.pendingLocalAppError { Text(message).foregroundStyle(.red).textSelection(.enabled) }
            if !projectStore.pendingLocalAppCreations.isEmpty {
                Section("local_app_authoring_recover") {
                    ForEach(projectStore.pendingLocalAppCreations) { intent in
                        Button { if developmentMode() != nil { onRecoverCreation(intent) } } label: {
                            Label(intent.name, systemImage: "arrow.clockwise")
                        }.accessibilityIdentifier("apps.recover-creation.\(intent.requestId)")
                    }
                }
            }
            ForEach(filtered) { app in
                Button { onShowDetails(app.appId) } label: {
                    HStack(spacing: 12) {
                        AppCover(name: app.name, category: "", size: 46)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(app.name).foregroundStyle(t.text)
                            Text(LocalizedStringKey(localAppPhaseKey(library.runs[app.appId]?.phase ?? "stopped"))).font(.caption).foregroundStyle(t.text3)
                        }
                        Spacer()
                        Button("common_open") { onOpenApp(app.appId) }.buttonStyle(.borderless)
                    }.padding(.vertical, 4)
                }
            }
            if filtered.isEmpty { ContentUnavailableView("local_app_empty_title", systemImage: "square.grid.2x2", description: Text("local_app_empty_body")) }
        }
        .listStyle(.plain)
        .background(t.windowBg)
        .navigationTitle("apps_title").navigationBarTitleDisplayMode(.inline)
        .searchable(text: $query, prompt: "apps_search_hint")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("local_app_market_open", systemImage: "storefront") { openMarket() }
                    .labelStyle(.iconOnly)
                    .accessibilityIdentifier("apps.market.open")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("local_app_create") { if developmentMode() != nil { creating = true } }
                    Button("local_app_import_source") {
                        if let mode = developmentMode() {
                            importingMode = mode
                            importPresentation = captureImportPresentation()
                            importing = true
                        }
                    }
                }
                    label: { Image(systemName: "plus") }.accessibilityLabel("local_app_create_or_import")
            }
        }
        .task { await library.refresh(); await projectStore.refreshPendingLocalAppCreations() }
        .sheet(isPresented: $creating) { LocalAppCreateSheet(onCreate: onCreate) }
        .fileImporter(isPresented:$importing,allowedContentTypes:[.folder, .zip]) { result in
            let admittedMode = importingMode
            let presentation = importPresentation
            importingMode = nil
            importPresentation = nil
            Task { @MainActor in
                do {
                    guard let mode = admittedMode else { throw LocalAppServiceFailure("requiresCode", String(localized: "local_app_development_requires_code")) }
                    let url = try result.get(); let scoped = url.startAccessingSecurityScopedResource(); defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                    let imported = try await library.importSource(url, mode: mode)
                    await library.refresh()
                    if presentation?.mayPresent() == true { onShowDetails(imported.record.appId) }
                } catch {
                    if let presentation {
                        presentation.deliverFailure(error.localizedDescription) { self.error = $0 }
                    } else { self.error = error.localizedDescription }
                }
            }
        }
        .accessibilityIdentifier("apps.page")
        .alert(String(localized: "local_app_development_requires_code"), isPresented: $requiresCode) {
            Button("common_ok", role: .cancel) { }
        }
    }
    private func developmentMode() -> SessionMode? {
        let mode = currentMode()
        guard mode == .code else { requiresCode = true; return nil }
        return mode
    }
    private func openMarket() {
        guard let url = market.browseURL else { error = String(localized: "local_app_market_unavailable"); return }
        openURL(url) { accepted in if !accepted { error = String(localized: "remote_apps_browser_error") } }
    }
}

private struct LocalAppCreateSheet: View {
    @Environment(\.dismiss) private var dismiss
    let onCreate: (String, String) async throws -> Void
    @State private var name = ""
    @State private var prompt = ""
    @State private var error: String?
    @State private var creating = false
    var body: some View {
        NavigationStack {
            Form {
                TextField("local_app_name",text:$name)
                Text("local_app_source_location \(MobileUserStorage.directories().appsGuestPath)").font(.footnote).foregroundStyle(.secondary)
                TextField("local_app_goal",text:$prompt,axis:.vertical).lineLimit(3...8)
                Text("local_app_create_hint").font(.footnote).foregroundStyle(.secondary)
                if let error { Text(error).foregroundStyle(.red) }
            }
            .navigationTitle("local_app_create")
            .toolbar {
                ToolbarItem(placement:.cancellationAction) { Button("common_cancel") { dismiss() }.disabled(creating) }
                ToolbarItem(placement:.confirmationAction) { Button("local_app_start_development") {
                    creating = true
                    Task { @MainActor in
                        defer { creating = false }
                        do {
                            try await onCreate(name, prompt)
                            dismiss()
                        } catch { self.error = error.localizedDescription }
                    }
                }.disabled(creating || name.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty) }
            }
        }
        .interactiveDismissDisabled(creating)
    }
}

struct LocalAppConsentSheet: View {
    @Environment(\.dismiss) private var dismiss
    let manifest: UnifiedAppManifest
    let digest: String
    let title: String
    var onCancel: (() -> Void)? = nil
    var canCancel = true
    let onApprove: () -> Void
    @State private var submitted = false
    var body: some View {
        NavigationStack {
            Form {
                Section(manifest.name) {
                    Text(manifest.summary ?? "")
                    if manifest.executesCode { Text("local_app_consent_executes") }
                    Text("local_app_capabilities_requested \((manifest.capabilities ?? []).joined(separator: ", "))")
                    Text("local_app_consent_scope").font(.footnote)
                }
                Section("local_app_release_digest") { Text(digest).font(.caption.monospaced()).textSelection(.enabled) }
                if let remix = manifest.remix, remix.allowed { Section("local_app_remix_license") { Text(remix.license ?? "") } }
            }
            .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
            .interactiveDismissDisabled(submitted)
            .toolbar {
                ToolbarItem(placement:.cancellationAction) { Button("common_cancel") { onCancel?(); dismiss() }.disabled(!canCancel) }
                ToolbarItem(placement:.confirmationAction) { Button("permission_allow") { submitted = true; onApprove() }.disabled(submitted) }
            }
        }
    }
}

struct LocalAppDetailPage: View {
    let library: UnifiedLocalApps
    let market: UnifiedAppMarket
    let appId: String
    let onOpenApp: (String) -> Void
    let onDevelop: (UnifiedAppRecord) -> Void
    let onRestore: (UnifiedAppRecord) -> Void
    let onRemix: (UnifiedAppRecord) -> Void
    let currentMode: () -> SessionMode
    let captureActionPresentation: () -> LocalAppActionPresentation
    @State private var releases: [UnifiedAppRelease] = []
    @State private var logs = ""
    @State private var dataPath = ""
    @State private var error: String?
    @State private var consent: UnifiedAppRelease?
    @State private var agentRelease: UnifiedAppRelease?
    @State private var deleting = false
    @State private var publication: LocalAppMarketPublication?
    @State private var preparingPublication = false
    @State private var exportingSource = false
    @State private var sourceDocument: LocalAppSourceDocument?
    @State private var restoring = false
    @State private var remixing = false
    @State private var pendingRemix = LocalAppRemixSelection<UnifiedAppRecord>()
    @State private var requiresCode = false
    @State private var sheetRequiresCode = false
    var body: some View {
        List {
            if let app = library.record(appId) {
                Section {
                    Text(app.name).font(.title2.weight(.semibold)); Text(app.summary).foregroundStyle(.secondary)
                    let run = library.runs[appId]
                    LabeledContent("session_details_status", value: TeamsText.localized(localAppPhaseKey(run?.phase ?? "stopped")))
                    Button(run?.phase == "running" ? "local_app_open_running" : "terminal_run") { requestRun(app) }
                    Button("composer_stop") { operation("stop") }.disabled(run?.isLive != true)
                    Button("local_app_continue_development") { if developmentMode() != nil { onDevelop(app) } }
                    if app.marketOrigin == nil { Button("local_app_restore_source") { if developmentMode() != nil { restoring = true } } }
                    if app.marketOrigin?.remixedFrom == nil, app.marketOrigin != nil {
                        Button("local_app_market_remix") {
                            if developmentMode() != nil { pendingRemix.capture(app); remixing = true }
                        }
                    }
                    Button("local_app_repair") { operation("repair") }
                }
                Section("plugins_detail_version") {
                    ForEach(releases) { release in
                        HStack {
                            Text("local_app_revision \(release.revision)"); Spacer()
                            if release.revision == app.currentRevision { Text("chat_agent_group_current").foregroundStyle(.secondary) }
                            else { Button("local_app_run_release") { requestRelease(release, app: app) } }
                        }
                    }
                }
                Section("settings_parity_permissions") {
                    Text("local_app_grant_hint").font(.footnote)
                    Button("local_app_revoke_grant",role:.destructive) { operation("revoke") }
                    if let release = releases.first(where: { $0.revision == app.currentRevision }) {
                        if (release.manifest.capabilities ?? []).contains("agent") {
                            Button("local_app_agent_settings") { agentRelease = release }
                        }
                    }
                    Text("local_app_browser_permissions_hint").font(.footnote)
                }
                Section("settings_section_data") {
                    Text(dataPath).font(.caption.monospaced()).textSelection(.enabled)
                    Text("local_app_data_hint").font(.footnote)
                    Button("local_app_export_source") { Task { do {
                        let source = try await library.sourceArchive(appId, revision: app.currentRevision)
                        sourceDocument = .init(bytes: source.bytes); exportingSource = true
                    } catch { self.error = error.localizedDescription } } }
                    Text("local_app_export_hint").font(.footnote).foregroundStyle(.secondary)
                    Text("local_app_browser_data_hint").font(.footnote).foregroundStyle(.secondary)
                }
                Section("local_app_logs") { Text(logs.isEmpty ? String(localized: "local_app_logs_empty") : logs).font(.caption.monospaced()).textSelection(.enabled); Button("local_app_logs_refresh") { Task { await load() } } }
                Section("local_app_publish") {
                    Button("local_app_publish_market") {
                        if let mode = developmentMode() {
                            let presentation = captureActionPresentation()
                            Task {
                                preparingPublication = true; defer { preparingPublication = false }
                                do {
                                    let prepared = try await market.preparePublication(app, mode: mode)
                                    market.parkPreparedPublication(prepared)
                                    if presentation.mayPresent() {
                                        publication = market.takePreparedPublication(appID: app.appId)
                                    }
                                } catch {
                                    presentation.deliverFailure(error.localizedDescription) { self.error = $0 }
                                }
                            }
                        }
                    }.disabled(preparingPublication || market.publishing)
                    if market.hasPreparedPublication(appID: app.appId) {
                        Button("local_app_publication_review_prepared") {
                            if developmentMode() != nil { publication = market.takePreparedPublication(appID: app.appId) }
                        }.disabled(market.publishing)
                    }
                    if preparingPublication { ProgressView("local_app_publish_preparing") }
                    if let url = market.managementURL { Link("local_app_publish_manage", destination: url) }
                    if let status = market.publishStatus { Text(status).font(.footnote) }
                }
                Section { Button("local_app_remove",role:.destructive) { deleting = true } }
            } else { ContentUnavailableView("local_app_unavailable_title",systemImage:"square.dashed",description:Text("local_app_unavailable_body")) }
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
        }
        .navigationTitle("apps_details").navigationBarTitleDisplayMode(.inline)
        .task(id:appId) { await load() }
        .fileExporter(isPresented: $exportingSource, document: sourceDocument, contentType: .zip, defaultFilename: "local-app-source") { result in
            sourceDocument = nil
            if case .failure(let failure) = result { error = failure.localizedDescription }
        }
        .sheet(item: $publication) { prepared in
            NavigationStack {
                List {
                    Section(prepared.name) {
                        Text(prepared.summary)
                        Text("local_app_publish_review_hint").font(.footnote)
                        Text(prepared.source.digest).font(.caption.monospaced()).textSelection(.enabled)
                    }
                    Section("local_app_publish_files") { ForEach(prepared.source.files, id: \.self) { Text($0).font(.caption.monospaced()) } }
                }
                .navigationTitle("local_app_publish_review_title").navigationBarTitleDisplayMode(.inline)
                .interactiveDismissDisabled(market.publishing)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { publication = nil }.disabled(market.publishing) }
                    ToolbarItem(placement: .confirmationAction) { Button("local_app_publish") {
                        if let mode = developmentMode(inSheet: true) {
                            let presentation = captureActionPresentation()
                            Task {
                                do { try await market.publish(prepared, mode: mode); publication = nil }
                                catch {
                                    market.parkPreparedPublication(prepared)
                                    presentation.deliverFailure(error.localizedDescription) { self.error = $0 }
                                    publication = nil
                                }
                            }
                        }
                    }.disabled(market.publishing) }
                }
                .alert(String(localized: "local_app_development_requires_code"), isPresented: $sheetRequiresCode) {
                    Button("common_ok", role: .cancel) { }
                }
            }
        }
        .sheet(item: $agentRelease) { release in
            LocalAppAgentSettingsView(
                client: library.client,
                appID: appId,
                release: release,
                captureConfigurationScope: {
                    library.configurationScopeSnapshot(appID: appId, revision: release.revision)
                },
                onConfigurationChanged: { snapshot in
                    await library.cancelBrowserAgentScope(snapshot)
                }
            )
        }
        .sheet(item:$consent) { release in LocalAppConsentSheet(manifest:release.manifest,digest:release.digest,title: String(localized: "local_app_consent_run_title")) { Task { do { try await library.grant(appId,digest:release.digest); consent = nil; await runRelease(release) } catch { self.error = error.localizedDescription; consent = nil } } } }
        .confirmationDialog("local_app_restore_confirm", isPresented: $restoring, titleVisibility: .visible) {
            Button("local_app_restore_continue") { if developmentMode() != nil, let app = library.record(appId) { onRestore(app) } }
            Button("common_cancel", role: .cancel) { }
        } message: { Text("local_app_restore_hint") }
        .confirmationDialog(String(localized: "local_app_market_remix_confirm"), isPresented: $remixing, titleVisibility: .visible) {
            Button("local_app_market_remix") {
                let selected = pendingRemix.take()
                if developmentMode() != nil, let selected { onRemix(selected) }
            }
            Button("common_cancel", role: .cancel) { pendingRemix.clear() }
        }
        .confirmationDialog("local_app_remove_confirm",isPresented:$deleting,titleVisibility:.visible) { Button("settings_parity_remove",role:.destructive) { operation("remove") }; Button("common_cancel",role:.cancel) {} } message: { Text("local_app_remove_hint") }
        .alert(String(localized: "local_app_development_requires_code"), isPresented: $requiresCode) {
            Button("common_ok", role: .cancel) { }
        }
    }
    private func developmentMode(inSheet: Bool = false) -> SessionMode? {
        let mode = currentMode()
        guard mode == .code else {
            if inSheet { sheetRequiresCode = true } else { requiresCode = true }
            return nil
        }
        return mode
    }
    private func requestRun(_ app: UnifiedAppRecord) {
        guard let release = releases.first(where: { $0.revision == app.currentRevision }) else { error = String(localized: "local_app_release_not_loaded"); return }
        requestRelease(release, app: app)
    }
    private func requestRelease(_ release: UnifiedAppRelease, app: UnifiedAppRecord) {
        if release.manifest.executesCode && !app.executionGrants.contains(release.digest) { consent = release }
        else { Task { await runRelease(release) } }
    }
    private func runRelease(_ release: UnifiedAppRelease) async {
        do {
            try await library.action("start", id: appId, extra: ["revision": .number(Double(release.revision))])
            onOpenApp(appId)
        } catch { self.error = error.localizedDescription; await library.refresh() }
    }
    private func operation(_ value: String) { Task { do { try await library.action(value,id:appId); await load() } catch { self.error = error.localizedDescription } } }
    private func load() async {
        do {
            await library.refresh(); releases = try await library.releases(appId)
            dataPath = try UnifiedLocalApps.guestDirectory(await library.layout(appId).dataPath)
            if let run = library.runs[appId] { logs = try await library.logs(appId,runID:run.runId) }
        } catch { self.error = error.localizedDescription }
    }
}

/// Trusted run controls; downloaded application pages live in the system-selected browser.
struct AppRuntimeView: View {
    @Environment(\.openURL) private var openURL
    let library: UnifiedLocalApps
    let info: InstalledAppInfoStore
    let appId: String
    let onClose: () -> Void
    let onShowDetails: (String) -> Void
    @State private var launch: LocalAppLaunch?
    @State private var error: String?
    @State private var hasAgent = false
    @State private var opened = false
    @State private var busy = false
    @State private var consent: UnifiedAppRelease?
    @State private var inAppBrowser: LocalAppBrowserTicket?
    @State private var requestedInApp = false
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text(library.record(appId)?.name ?? String(localized: "chat_local_app_kind")).font(.title2)
                Text("local_app_browser_intro")
                Text("local_app_browser_lifetime")
                    .font(.footnote).foregroundStyle(.secondary)
                if busy { ProgressView() }
                if let error {
                    Text(error)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let wake = library.browserAgentWake(for: appId) {
                    Text("local_app_agent_wake_pending \(wake.runID)")
                        .font(.footnote.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    Text("local_app_agent_in_app_wake_unsupported")
                        .font(.footnote).foregroundStyle(.secondary)
                    Text("local_app_agent_default_wake_help")
                        .font(.footnote).foregroundStyle(.secondary)
                    Button("local_app_agent_continue_default") {
                        guard !busy, let token = library.browserAgentWakeToken(for: wake) else { return }
                        busy = true
                        Task { await returnToBrowser(wake, token: token) }
                    }.disabled(busy).accessibilityIdentifier("local-app-agent-continue-default")
                    Button("local_app_agent_cancel_request", role: .cancel) {
                        guard !busy, let token = library.browserAgentWakeToken(for: wake) else { return }
                        busy = true
                        Task {
                            await library.cancelBrowserAgentHandoff(wake, token: token)
                            busy = false
                        }
                    }.disabled(busy)
                } else {
                    Button(launch == nil ? "local_app_launch_default" : "local_app_return_default") {
                        Task { await load(openBrowser: true) }
                    }.disabled(busy)
                    Button("remote_apps_open_in_app") { Task { await load(openBrowser: true, inApp: true) } }.disabled(busy)
                    if hasAgent {
                        Text("local_app_agent_in_app_unsupported").font(.footnote).foregroundStyle(.secondary)
                    }
                }
                if hasAgent, let revision = library.record(appId)?.currentRevision {
                    LocalAppAgentTasks(client: library.client, appID: appId, revision: revision)
                }
                Button("local_app_settings_agent") { onShowDetails(appId) }
                Button("local_app_stop_all", role: .destructive) { Task {
                    do { try await library.stop(appId); onClose() } catch { self.error = error.localizedDescription }
                } }
                Button("local_app_close_manager", action: onClose)
                Text("local_app_close_manager_hint")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityIdentifier("app-runtime")
        }
        .task(id: appId) { await load(openBrowser: !opened && !library.consumeBrowserReview(appId)) }
        .sheet(item: $inAppBrowser) { ticket in LocalAppSystemBrowser(url: ticket.url) }
        .sheet(item: $consent) { release in
            LocalAppConsentSheet(manifest: release.manifest, digest: release.digest, title: String(localized: "local_app_consent_run_title")) {
                Task { do { try await library.grant(appId, digest: release.digest); consent = nil; await load(openBrowser: true, inApp: requestedInApp) }
                catch { self.error = error.localizedDescription; consent = nil } }
            }
        }
        .background {
            if hasAgent {
                LocalAppAgentInteractions(client: library.client, appID: appId)
            }
        }
    }
    private func load(openBrowser: Bool, inApp: Bool = false) async {
        guard !busy else { return }
        requestedInApp = inApp
        busy = true; error = nil
        defer { busy = false }
        do {
            await library.refresh()
            guard let app = library.record(appId) else { throw LocalAppServiceFailure("APP_NOT_FOUND", String(localized: "local_app_not_found")) }
            let release = try await library.release(appId, revision: app.currentRevision)
            hasAgent = (release.manifest.capabilities ?? []).contains("agent")
            if release.manifest.executesCode && !app.executionGrants.contains(release.digest) { consent = release; return }
            if openBrowser, library.browserAgentWake(for: appId) != nil {
                // Pending input belongs to its originating browser. The user
                // must explicitly confirm the default-browser continuation.
                return
            }
            let value = try await library.openExternalBrowser(appId)
            guard LocalAppExternalBrowserURL.matches(value.url, origin: value.origin) else {
                throw LocalAppServiceFailure("INVALID_APP_ORIGIN", String(localized: "local_app_invalid_origin"))
            }
            launch = value; info.markOpened(app); opened = true
            if openBrowser {
                if inApp {
                    inAppBrowser = LocalAppBrowserTicket(url: value.url)
                } else {
                    let scope = LocalAppBrowserDeliveryScope(appID: appId, revision: value.run.revision)
                    let acquisition = library.openBrowserPage(scope: scope) {
                        openURL(value.url) { accepted in
                            guard !accepted else { return }
                            Task { @MainActor in
                                library.endBrowserPageDelivery(scope)
                                self.error = String(localized: "local_app_browser_open_failed")
                            }
                        }
                    }
                    guard acquisition != .unavailable else {
                        throw LocalAppServiceFailure(
                            "BROWSER_DELIVERY_UNAVAILABLE",
                            String(localized: "local_app_browser_background_unavailable")
                        )
                    }
                }
            }
        } catch { self.error = error.localizedDescription }
    }
    private func returnToBrowser(_ wake: LocalAppAgentWakeRequest, token: UUID) async {
        defer { busy = false }
        await prepareAndReturnToBrowser(wake, token: token)
    }
    private func prepareAndReturnToBrowser(_ wake: LocalAppAgentWakeRequest, token: UUID) async {
        do {
            let handoff = try await library.prepareBrowserAgentHandoff(wake, token: token)
            guard library.isBrowserAgentHandoffReady(wake, token: handoff.token) else {
                await library.cancelBrowserAgentHandoff(wake, token: handoff.token, reason: .acquireFailure)
                return
            }
            launch = handoff.launch
            if let app = library.record(appId) { info.markOpened(app) }
            opened = true
            openURL(handoff.readyURL) { accepted in
                Task { @MainActor in
                    let handled = await library.finishBrowserAgentHandoff(
                        wake, token: handoff.token, accepted: accepted
                    )
                    if !accepted && handled {
                        self.error = String(localized: "local_app_browser_return_failed")
                    }
                }
            }
        } catch { self.error = error.localizedDescription }
    }
}

struct MarketInstallPage: View {
    let library: UnifiedLocalApps
    let market: UnifiedAppMarket
    let appId: String
    let onOpenApp: (String) -> Void
    let currentMode: () -> SessionMode
    let onRemix: (MarketDistribution) -> Void
    @State private var app: MarketApp?
    @State private var distribution: MarketDistribution?
    @State private var error: String?
    @State private var consenting = false
    @State private var remixing = false
    @State private var pendingRemix = LocalAppRemixSelection<MarketDistribution>()
    @State private var requiresCode = false
    @State private var loading = false
    var body: some View {
        List {
            if loading { ProgressView() }
            if let app {
                Section { AppCover(name: app.name, category: app.category); Text(app.name).font(.title2.weight(.semibold)); Text(app.summary) }
            }
            if let release = distribution {
                Section("local_app_install_release") {
                    Text("\(Text("local_app_revision \(release.sequence)")) · \(Text(release.manifest.executesCode ? "local_app_kind_linux" : "local_app_kind_static"))")
                    Text("local_app_install_build_hint \(release.dependencies.count)")
                    Text("local_app_capabilities \((release.manifest.capabilities ?? []).joined(separator: ", "))")
                    Text(release.releaseDigest).font(.caption.monospaced()).textSelection(.enabled)
                    Button("local_app_install") { consenting = true }.disabled(market.progress != nil || market.installCommitting)
                    if release.manifest.remix?.allowed == true {
                        Button("local_app_market_remix") {
                            if currentMode() == .code { pendingRemix.capture(release); remixing = true }
                            else { requiresCode = true }
                        }.disabled(market.progress != nil || market.installCommitting)
                    }
                }
            }
            if let progress = market.progress { ProgressView(value: progress) }
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            Button("teams_list_refresh") { Task { await load() } }
        }
        .navigationTitle("local_app_install_title").navigationBarTitleDisplayMode(.inline)
        .task(id: appId) { await load() }
        .sheet(isPresented: $consenting) {
            if let distribution {
                LocalAppConsentSheet(manifest: distribution.manifest, digest: distribution.releaseDigest,
                    title: String(localized: "local_app_install_consent_title"), onCancel: { market.cancelInstall() }, canCancel: !market.installCommitting) {
                    Task {
                        do {
                            let installed = try await market.install(distribution, approved: true)
                            consenting = false
                            onOpenApp(installed.appId)
                        } catch { self.error = error.localizedDescription; consenting = false }
                    }
                }
            }
        }
        .confirmationDialog(String(localized: "local_app_market_remix_confirm"), isPresented: $remixing, titleVisibility: .visible) {
            Button("local_app_market_remix") {
                let selected = pendingRemix.take()
                guard currentMode() == .code else { requiresCode = true; return }
                if let selected { onRemix(selected) }
            }
            Button("common_cancel", role: .cancel) { pendingRemix.clear() }
        }
        .alert(String(localized: "local_app_development_requires_code"), isPresented: $requiresCode) {
            Button("common_ok", role: .cancel) { }
        }
    }
    private func load() async {
        distribution = nil; app = nil; error = nil; loading = true
        defer { loading = false }
        do {
            await library.refresh()
            try Task.checkCancellation()
            if let installed = library.records.first(where: { $0.marketOrigin?.appId == appId && $0.marketOrigin?.remixedFrom == nil }) {
                onOpenApp(installed.appId); return
            }
            app = try await market.detail(appId)
            let release = try await market.distribution(appId)
            try Task.checkCancellation()
            distribution = release; error = nil
        } catch is CancellationError { }
        catch { self.error = error.localizedDescription }
    }
}

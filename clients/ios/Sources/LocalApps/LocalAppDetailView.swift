import SwiftUI

private enum LocalAppDetailSection: String, CaseIterable, Identifiable {
    case sessions
    case overview
    case preview
    case data
    case code
    case history
    case permissions

    var id: String { rawValue }

    var label: String {
        switch self {
        case .sessions: String(localized: "local_apps_section_sessions")
        case .overview: String(localized: "local_apps_section_overview")
        case .preview: String(localized: "local_apps_section_preview")
        case .data: String(localized: "local_apps_section_data")
        case .code: String(localized: "local_apps_section_code")
        case .history: String(localized: "local_apps_section_history")
        case .permissions: String(localized: "local_apps_section_permissions")
        }
    }
}

struct LocalAppDetailView: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    let appID: String
    @Binding var path: [LocalAppsRoute]
    /// `(appID, sessionUUID)` — RootView dismisses the cover and resumes the
    /// session inside the app's conversation scope.
    var onOpenAppSession: (String, String) -> Void = { _, _ in }
    /// RootView dismisses the cover and starts a fresh conversation in the
    /// app's scope.
    var onNewAppSession: (String) -> Void = { _ in }

    /// The session catalog is the app's primary surface now — each app IS a
    /// conversation scope, so the conversations come first.
    @State private var section: LocalAppDetailSection = .sessions

    private var app: LocalAppSummary? { store.app(id: appID) }
    private var runtime: LocalAppRuntimeStatus { store.runtimes[appID] ?? .stopped }

    var body: some View {
        content
            .navigationTitle(app?.name ?? String(localized: "local_apps_detail_title"))
            .navigationBarTitleDisplayMode(.inline)
            .task {
                await store.refresh()
                await store.getDetails(appID: appID)
                await store.listCheckpoints(appID: appID)
            }
    }

    /// The one place the engine's app-activity signals are visible.
    ///
    /// `llm.chat` bills the USER's model quota, so an app calling it while
    /// the user is looking at something else must not be silent; and an app
    /// that posted an event has told the assistant something the user may
    /// want to ask about. Both are engine state the store already tracks —
    /// without this they were computed and never shown.
    @ViewBuilder
    private var activityBar: some View {
        let callingAI = store.llmActiveAppIDs.contains(appID)
        let unread = store.unreadAgentEvents[appID] ?? 0
        if callingAI || unread > 0 {
            HStack(spacing: 8) {
                if callingAI {
                    ProgressView().controlSize(.small)
                    Text("local_apps_activity_calling_ai")
                        .accessibilityIdentifier("local-apps.activity.calling-ai")
                }
                if callingAI, unread > 0 {
                    Text("·").foregroundStyle(.secondary)
                }
                if unread > 0 {
                    Image(systemName: "tray.full")
                    Text("local_apps_activity_unread_events \(unread)")
                        .accessibilityIdentifier("local-apps.activity.unread-events")
                }
                Spacer()
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
            .padding(.horizontal)
            .padding(.vertical, 6)
            .background(.bar)
        }
    }

    @ViewBuilder
    private var content: some View {
        Group {
            if let app {
                VStack(spacing: 0) {
                    LocalAppDetailSectionPicker(selection: $section)
                    Divider()
                    activityBar
                    sectionContent(app)
                }
                .background(theme.windowBg)
                .toolbar {
                    ToolbarItemGroup(placement: .primaryAction) {
                        if case .running = runtime {
                            Button("composer_stop", systemImage: "stop.fill") {
                                Task { await store.stop(appID: appID) }
                            }
                        } else {
                            Button("common_start", systemImage: "play.fill") {
                                Task { await store.start(appID: appID) }
                            }
                            .disabled(app.workflow != .ready)
                        }
                        Menu {
                            Button("local_apps_restart", systemImage: "arrow.clockwise") {
                                Task { await store.restart(appID: appID) }
                            }
                        } label: {
                            Label("local_apps_more", systemImage: "ellipsis.circle")
                        }
                    }
                }
            } else {
                ContentUnavailableView("local_apps_not_found", systemImage: "questionmark.app")
            }
        }
    }

    @ViewBuilder
    private func sectionContent(_ app: LocalAppSummary) -> some View {
        switch section {
        case .sessions:
            LocalAppSessionsSection(
                store: store,
                appID: appID,
                onOpenSession: { onOpenAppSession(appID, $0) },
                onNewSession: { onNewAppSession(appID) }
            )
        case .overview:
            LocalAppOverviewSection(
                app: app,
                runtime: runtime,
                distribution: store.distributionMode,
                onOpenPreview: { path.append(.preview(appID)) }
            )
        case .preview:
            LocalAppEmbeddedPreview(store: store, appID: appID)
        case .data:
            LocalAppDataSection(collections: store.collections[appID] ?? [])
        case .code:
            LocalAppCodeSection(app: app)
        case .history:
            LocalAppHistorySection(store: store, appID: appID)
        case .permissions:
            LocalAppPermissionsSection(store: store, appID: appID)
        }
    }
}

/// The app's workspace-scoped session catalog (`ListAppSessions` →
/// `AppSessionsChanged`): the pinned init session first with an「初始化」
/// badge, the rest modified-descending, paged by「加载更多」.
struct LocalAppSessionsSection: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    let appID: String
    let onOpenSession: (String) -> Void
    let onNewSession: () -> Void

    private var page: LocalAppSessionPage? { store.sessionPages[appID] }

    var body: some View {
        List {
            Section {
                Button {
                    onNewSession()
                } label: {
                    Label("local_apps_session_new", systemImage: "square.and.pencil")
                        .foregroundStyle(theme.accent)
                }
                .accessibilityIdentifier("local-apps.sessions.new")
            }
            Section {
                ForEach(page?.rows ?? []) { row in
                    Button {
                        onOpenSession(row.uuid)
                    } label: {
                        LocalAppSessionRowView(row: row)
                    }
                    .accessibilityIdentifier("local-apps.sessions.row.\(row.uuid)")
                }
                if page?.nextOffset != nil {
                    Button("local_apps_sessions_load_more") {
                        Task { await store.loadMoreSessions(appID: appID) }
                    }
                    .accessibilityIdentifier("local-apps.sessions.load-more")
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if page?.rows.isEmpty != false {
                ContentUnavailableView(
                    "local_apps_sessions_empty",
                    systemImage: "bubble.left.and.bubble.right"
                )
                .allowsHitTesting(false)
            }
        }
        .task {
            await store.listSessions(appID: appID)
        }
        .refreshable {
            await store.listSessions(appID: appID)
        }
    }
}

private struct LocalAppSessionRowView: View {
    @Environment(\.theme) private var theme
    let row: LocalAppSessionRow

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text(row.title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                if row.isInit {
                    Text("local_apps_session_init_badge")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(theme.accent)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(theme.accent.opacity(0.14), in: Capsule())
                }
            }
            Text("drawer_session_subtitle \(row.relativeTime) \(row.messageCount)")
                .font(.caption)
                .foregroundStyle(theme.text4)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(.rect)
    }
}

private struct LocalAppDetailSectionPicker: View {
    @Environment(\.theme) private var theme
    @Binding var selection: LocalAppDetailSection

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(LocalAppDetailSection.allCases) { item in
                    Button(item.label) { selection = item }
                        .buttonStyle(.bordered)
                        .tint(selection == item ? theme.accent : theme.text4)
                }
            }
            .padding(.horizontal)
        }
        .scrollIndicators(.hidden)
        .padding(.vertical, 8)
    }
}

private struct LocalAppOverviewSection: View {
    @Environment(\.theme) private var theme
    let app: LocalAppSummary
    let runtime: LocalAppRuntimeStatus
    let distribution: LocalAppsDistributionMode
    let onOpenPreview: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 14) {
                    Image(systemName: localAppIconSystemName)
                        .font(.largeTitle)
                        .foregroundStyle(theme.accent)
                        .frame(width: 64, height: 64)
                        .background(theme.accent.opacity(0.12), in: .rect(cornerRadius: 16))
                    VStack(alignment: .leading, spacing: 5) {
                        Text(app.name).font(.title2.bold())
                        Text(app.workflow.label).foregroundStyle(theme.text3)
                        Label(runtime.label, systemImage: runtimeSystemImage)
                            .font(.caption)
                            .foregroundStyle(runtimeColor)
                    }
                }
                .padding()
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(theme.surface, in: .rect(cornerRadius: 16))

                VStack(alignment: .leading, spacing: 10) {
                    LabeledContent("local_apps_runtime_mode", value: distribution.runtimeLabel)
                    LabeledContent("local_apps_workspace", value: app.workspaceRelativePath)
                    LabeledContent("local_apps_brief", value: app.brief)
                    LabeledContent("local_apps_updated_at") {
                        Text(app.updatedAt, format: .relative(presentation: .named))
                    }
                }
                .padding()
                .background(theme.surface, in: .rect(cornerRadius: 16))

                Button("local_apps_open_preview", systemImage: "safari", action: onOpenPreview)
                    .buttonStyle(.borderedProminent)
                    .disabled(runtime.url == nil)
            }
            .padding()
        }
    }

    private var runtimeSystemImage: String {
        switch runtime {
        case .running: "circle.fill"
        case .starting, .stopping: "clock"
        case .failed: "exclamationmark.triangle"
        case .stopped, .suspended: "circle"
        }
    }

    private var runtimeColor: Color {
        switch runtime {
        case .running: theme.ok
        case .failed: theme.danger
        case .starting, .stopping: .orange
        case .stopped, .suspended: theme.text3
        }
    }
}

private struct LocalAppEmbeddedPreview: View {
    @Bindable var store: LocalAppsStore
    let appID: String

    var body: some View {
        if let url = store.runtimes[appID]?.url {
            LocalAppWebView(
                appID: appID,
                url: url,
                onBridgeRequest: { request in
                    Task { await store.executeBridge(request) }
                }
            )
        } else {
            ContentUnavailableView {
                Label("local_apps_preview_not_running", systemImage: "safari")
            } description: {
                Text("local_apps_preview_not_running_detail")
            } actions: {
                Button("common_start") { Task { await store.start(appID: appID) } }
            }
        }
    }
}

private struct LocalAppDataSection: View {
    let collections: [LocalAppDataCollection]

    var body: some View {
        List {
            Section {
                Label("local_apps_data_sqlite", systemImage: "lock.shield")
                Text("local_apps_data_sqlite_detail")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            ForEach(collections) { collection in
                Section(collection.label) {
                    ForEach(collection.fields) { field in
                        LabeledContent(field.label, value: field.fieldType.rawValue)
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
    }
}

private struct LocalAppCodeSection: View {
    let app: LocalAppSummary

    @State private var browser: LocalAppCodeBrowser

    init(app: LocalAppSummary) {
        self.app = app
        _browser = State(initialValue: LocalAppCodeBrowser(workspaceRelativePath: app.workspaceRelativePath))
    }

    var body: some View {
        List {
            Section("local_apps_workspace") {
                Text(app.workspaceRelativePath)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
            }
            Section("local_apps_source") {
                ForEach(browser.files) { file in
                    Button {
                        browser.open(file)
                    } label: {
                        HStack {
                            Label(file.relativePath, systemImage: "doc.text")
                                .foregroundStyle(.primary)
                            Spacer()
                            Text(ByteCountFormatter.string(fromByteCount: Int64(file.size), countStyle: .file))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
            Section {
                Text("local_apps_source_detail")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .overlay {
            if browser.isLoading {
                ProgressView("local_apps_source_loading")
            } else if browser.files.isEmpty {
                ContentUnavailableView("local_apps_source_empty", systemImage: "doc.text.magnifyingglass")
            }
        }
        .task { browser.refresh() }
        .sheet(
            isPresented: Binding(
                get: { browser.selectedPath != nil },
                set: { if !$0 { browser.closeEditor() } }
            )
        ) {
            NavigationStack {
                TextEditor(text: Binding(
                    get: { browser.editorText },
                    set: browser.updateEditorText
                ))
                .font(.system(.body, design: .monospaced))
                .padding(.horizontal, 8)
                .navigationTitle(browser.selectedPath ?? String(localized: "local_apps_source"))
                .navigationBarTitleDisplayMode(.inline)
                .safeAreaInset(edge: .bottom) {
                    if let message = browser.statusMessage {
                        Text(message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .padding(8)
                            .frame(maxWidth: .infinity)
                            .background(.bar)
                    }
                }
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_close") { browser.closeEditor() }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("voice_save_button") { browser.save() }
                            .disabled(browser.isSaving)
                    }
                }
            }
        }
        .alert(
            "local_apps_source_error_title",
            isPresented: Binding(
                get: { browser.errorMessage != nil },
                set: { if !$0 { browser.clearError() } }
            )
        ) {
            Button("common_ok", role: .cancel, action: browser.clearError)
        } message: {
            Text(browser.errorMessage ?? String(localized: "common_unknown_error"))
        }
    }
}

private struct LocalAppHistorySection: View {
    @Bindable var store: LocalAppsStore
    let appID: String

    @State private var pendingRestore: LocalAppCheckpoint?

    var body: some View {
        List(store.checkpoints[appID] ?? []) { checkpoint in
            Button {
                pendingRestore = checkpoint
            } label: {
                VStack(alignment: .leading, spacing: 4) {
                    Text(checkpoint.label)
                    Text(checkpoint.createdAt, format: .relative(presentation: .named))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .overlay {
            if (store.checkpoints[appID] ?? []).isEmpty {
                ContentUnavailableView("local_apps_no_checkpoints", systemImage: "clock.arrow.circlepath")
            }
        }
        .confirmationDialog(
            "local_apps_restore_checkpoint_title",
            isPresented: Binding(
                get: { pendingRestore != nil },
                set: { if !$0 { pendingRestore = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("local_apps_restore_code", role: .destructive) {
                guard let checkpoint = pendingRestore else { return }
                pendingRestore = nil
                Task { _ = await store.restore(appID: appID, checkpointID: checkpoint.id) }
            }
            Button("common_cancel", role: .cancel) { pendingRestore = nil }
        } message: {
            Text("local_apps_restore_checkpoint_detail")
        }
    }
}

private struct LocalAppPermissionsSection: View {
    @Bindable var store: LocalAppsStore
    let appID: String
    @State private var confirmReset = false

    var body: some View {
        List {
            Section("local_apps_section_agent") {
                Label("local_apps_permission_read_default", systemImage: "eye")
                Label("local_apps_permission_mutation_requires", systemImage: "hand.raised")
                Button("local_apps_permissions_reset", role: .destructive) { confirmReset = true }
            }
            Section("local_apps_section_network") {
                Label("local_apps_permission_network_default", systemImage: "network.slash")
                Text("local_apps_permission_network_detail")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            Section("local_apps_section_app_id") {
                Text(appID).textSelection(.enabled)
            }
        }
        .confirmationDialog(
            "local_apps_permissions_reset_title",
            isPresented: $confirmReset,
            titleVisibility: .visible
        ) {
            Button("local_apps_permissions_reset_confirm", role: .destructive) {
                Task { _ = await store.resetPermissions(appID: appID) }
            }
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("local_apps_permissions_reset_detail")
        }
    }
}

struct LocalAppPreviewView: View {
    @Bindable var store: LocalAppsStore
    let appID: String

    private var previewURL: URL? {
        store.runtimes[appID]?.url
    }

    var body: some View {
        Group {
            if let url = previewURL {
                LocalAppWebView(
                    appID: appID,
                    url: url,
                    onBridgeRequest: { request in
                        Task { await store.executeBridge(request) }
                    }
                )
            } else {
                ContentUnavailableView("local_apps_preview_not_ready", systemImage: "hourglass")
            }
        }
        .navigationTitle("local_apps_section_preview")
        .navigationBarTitleDisplayMode(.inline)
    }
}

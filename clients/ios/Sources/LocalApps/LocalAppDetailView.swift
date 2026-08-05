import SwiftUI

private enum LocalAppDetailSection: String, CaseIterable, Identifiable {
    case overview
    case preview
    case data
    case code
    case history
    case permissions

    var id: String { rawValue }

    var label: String {
        switch self {
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

    @State private var section: LocalAppDetailSection = .overview

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

    @ViewBuilder
    private var content: some View {
        Group {
            if let app {
                VStack(spacing: 0) {
                    LocalAppDetailSectionPicker(selection: $section)
                    Divider()
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
                            .disabled(app.workflow != .ready && app.workflow != .awaitingPreviewConfirmation)
                        }
                        Menu {
                            if app.workflow == .generationFailed || app.workflow == .validationFailed {
                                Button("local_apps_retry_generate", systemImage: "arrow.trianglehead.2.clockwise.rotate.90") {
                                    Task { await store.retryGeneration(appID: appID) }
                                }
                            }
                            Button("local_apps_restart", systemImage: "arrow.clockwise") {
                                Task { await store.restart(appID: appID) }
                            }
                            Button("local_apps_continue_design", systemImage: "slider.horizontal.3") {
                                path.append(.designer(appID))
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
        case .overview:
            LocalAppOverviewSection(
                app: app,
                runtime: runtime,
                progress: store.generationProgress[appID],
                distribution: store.distributionMode,
                onOpenPreview: { path.append(.preview(appID)) }
            )
        case .preview:
            LocalAppEmbeddedPreview(store: store, appID: appID)
        case .data:
            LocalAppDataSection(template: store.template(for: app))
        case .code:
            LocalAppCodeSection(app: app)
        case .history:
            LocalAppHistorySection(store: store, appID: appID)
        case .permissions:
            LocalAppPermissionsSection(store: store, appID: appID)
        }
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
    let progress: LocalAppGenerationProgress?
    let distribution: LocalAppsDistributionMode
    let onOpenPreview: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 14) {
                    Image(systemName: app.templateKind.systemImage)
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

                if let progress {
                    LocalAppGenerationCard(progress: progress, workflow: app.workflow)
                }

                VStack(alignment: .leading, spacing: 10) {
                    LabeledContent("local_apps_runtime_mode", value: distribution.runtimeLabel)
                    LabeledContent("local_apps_workspace", value: app.workspaceRelativePath)
                    LabeledContent("local_apps_template", value: app.templateKind.rawValue)
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
        if let url = store.runtimes[appID]?.url ?? store.previews[appID]?.url {
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
    let template: LocalAppTemplate?

    var body: some View {
        List {
            Section {
                Label("local_apps_data_sqlite", systemImage: "lock.shield")
                Text("local_apps_data_sqlite_detail")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            ForEach(template?.collections ?? []) { collection in
                Section(collection.name) {
                    ForEach(collection.fields) { field in
                        LabeledContent(field.name, value: field.type.rawValue)
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

    @State private var feedback = ""
    @State private var showFeedback = false

    private var previewURL: URL? {
        store.runtimes[appID]?.url ?? store.previews[appID]?.url
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
                .safeAreaInset(edge: .bottom) {
                    if store.previews[appID] != nil {
                        HStack {
                            Button("local_apps_feedback") { showFeedback = true }
                                .buttonStyle(.bordered)
                            Spacer()
                            Button("local_apps_approve_preview") {
                                Task { _ = await store.approvePreview(appID: appID) }
                            }
                            .buttonStyle(.borderedProminent)
                        }
                        .padding()
                        .background(.bar)
                    }
                }
            } else {
                ContentUnavailableView("local_apps_preview_not_ready", systemImage: "hourglass")
            }
        }
        .navigationTitle("local_apps_preview_confirm_title")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $showFeedback) {
            NavigationStack {
                Form {
                    TextEditor(text: $feedback)
                        .frame(minHeight: 140)
                }
                .navigationTitle("local_apps_feedback")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_cancel") { showFeedback = false }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("local_apps_submit") {
                            Task {
                                if await store.requestRevision(appID: appID, feedback: feedback) {
                                    showFeedback = false
                                }
                            }
                        }
                        .disabled(feedback.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
        }
    }
}

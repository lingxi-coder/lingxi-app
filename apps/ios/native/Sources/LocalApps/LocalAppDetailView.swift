import SwiftUI

private func localAppMcpDigestSummary(_ value: String) -> String {
    guard value.count > 12 else { return value }
    return String(value.prefix(12))
}

private enum LocalAppDetailSection: String, CaseIterable, Identifiable {
    case sessions
    case overview
    case mcp
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
        case .mcp: "MCP"
        case .preview: String(localized: "local_apps_section_preview")
        case .data: String(localized: "local_apps_section_data")
        case .code: String(localized: "local_apps_section_code")
        case .history: String(localized: "local_apps_section_history")
        case .permissions: String(localized: "local_apps_section_permissions")
        }
    }
}

/// Advanced Local App controls deliberately live behind the workspace's
/// overflow menu. The runtime and repair conversation are the everyday flow;
/// source, MCP, data and recovery tools remain available without competing for
/// the same first screen.
private enum LocalAppManagementRoute: Hashable {
    case overview
    case mcp
    case data
    case code
    case history
    case permissions
}

struct LocalAppManagementSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var store: LocalAppsStore
    let appID: String
    let activeConversationID: String
    let onDeleted: (String) -> Void

    @State private var route: [LocalAppManagementRoute] = []
    @State private var confirmDelete = false

    private var app: LocalAppSummary? { store.app(id: appID) }
    private var runtime: LocalAppRuntimeStatus { store.runtimes[appID] ?? .stopped }

    var body: some View {
        NavigationStack(path: $route) {
            List {
                if let app {
                    Section("local_apps_section_overview") {
                        NavigationLink(value: LocalAppManagementRoute.overview) {
                            Label(app.displayName, systemImage: localAppIconSystemName)
                        }
                        LabeledContent("local_apps_runtime_mode", value: runtime.label)
                    }
                    Section {
                        NavigationLink(value: LocalAppManagementRoute.mcp) {
                            Label("MCP", systemImage: "slider.horizontal.3")
                        }
                        NavigationLink(value: LocalAppManagementRoute.data) {
                            Label("local_apps_section_data", systemImage: "cylinder.split.1x2")
                        }
                        NavigationLink(value: LocalAppManagementRoute.code) {
                            Label("local_apps_section_code", systemImage: "chevron.left.forwardslash.chevron.right")
                        }
                        NavigationLink(value: LocalAppManagementRoute.history) {
                            Label("local_apps_section_history", systemImage: "clock.arrow.circlepath")
                        }
                        NavigationLink(value: LocalAppManagementRoute.permissions) {
                            Label("local_apps_section_permissions", systemImage: "hand.raised")
                        }
                    }
                    if app.scaffolded {
                        Section {
                            Button("local_apps_widget_add", systemImage: "rectangle.on.rectangle") {
                                store.requestWidgetSetup(appID: appID)
                            }
                            .accessibilityIdentifier("local-apps.management.add-widget")
                        }
                    }
                    Section {
                        Button("local_apps_delete", role: .destructive) { confirmDelete = true }
                    }
                } else {
                    ContentUnavailableView("local_apps_not_found", systemImage: "questionmark.app")
                }
            }
            .navigationTitle("local_apps_more")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_done") { dismiss() }
                }
            }
            .navigationDestination(for: LocalAppManagementRoute.self) { managementDestination($0) }
            .confirmationDialog(
                "local_apps_delete_confirm \(app?.displayName ?? String(localized: "local_apps_title"))",
                isPresented: $confirmDelete,
                titleVisibility: .visible
            ) {
                Button("local_apps_delete", role: .destructive) { deleteApp() }
                Button("common_cancel", role: .cancel) {}
            } message: {
                Text("local_apps_delete_detail")
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .sheet(
            item: Binding(
                get: { store.pendingWidgetSetup },
                set: { _ in }
            )
        ) { setup in
            LocalAppWidgetSetupSheet(appName: setup.appName) {
                store.completeWidgetSetup()
            }
        }
        .task(id: appID) { await store.getDetails(appID: appID) }
    }

    @ViewBuilder
    private func managementDestination(_ destination: LocalAppManagementRoute) -> some View {
        switch destination {
        case .overview:
            if let app {
                LocalAppOverviewSection(
                    app: app,
                    runtime: runtime,
                    distribution: store.distributionMode,
                    onOpenPreview: {}
                )
                .navigationTitle("local_apps_section_overview")
            }
        case .mcp:
            LocalAppMcpSection(store: store, appID: appID, activeConversationID: activeConversationID)
                .navigationTitle("MCP")
        case .data:
            LocalAppDataSection(collections: store.collections[appID] ?? [])
                .navigationTitle("local_apps_section_data")
        case .code:
            if let app {
                LocalAppCodeSection(app: app)
                    .navigationTitle("local_apps_section_code")
            }
        case .history:
            LocalAppHistorySection(store: store, appID: appID)
                .navigationTitle("local_apps_section_history")
        case .permissions:
            LocalAppPermissionsSection(store: store, appID: appID)
                .navigationTitle("local_apps_section_permissions")
        }
    }

    private func deleteApp() {
        Task {
            guard await store.delete(appID: appID) else { return }
            dismiss()
            onDeleted(appID)
        }
    }
}

struct LocalAppDetailView: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    let appID: String
    @Binding var path: [LocalAppsRoute]
    var activeConversationID: String = ""
    /// `(appID, sessionUUID, mode)` — RootView dismisses the cover and resumes
    /// the session inside the matching app capability profile.
    var onOpenAppSession: (String, String, SessionMode) -> Void = { _, _, _ in }
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
            .navigationTitle(app?.displayName ?? String(localized: "local_apps_detail_title"))
            .navigationBarTitleDisplayMode(.inline)
            .task {
                // Library/create events normally already supplied the summary.
                // Keep the refresh only as a deep-link recovery path; details
                // already carries checkpoints, so a second list is redundant.
                if store.app(id: appID) == nil {
                    await store.refresh()
                }
                await store.getDetails(appID: appID)
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
                            .disabled(!app.workflow.isPublished)
                        }
                        Menu {
                            Button("local_apps_restart", systemImage: "arrow.clockwise") {
                                Task { await store.restart(appID: appID) }
                            }
                            // The home-screen widget entry's PERMANENT home.
                            // It used to be a toggle inside the create form,
                            // which the conversational flow deletes — without
                            // this the feature would simply disappear from the
                            // product. Hidden for a shell, which is excluded
                            // from the widget snapshot and so has nothing to
                            // put on the home screen.
                            if app.scaffolded {
                                Button(
                                    "local_apps_widget_add",
                                    systemImage: "rectangle.on.rectangle"
                                ) {
                                    store.requestWidgetSetup(appID: appID)
                                }
                                .accessibilityIdentifier("local-apps.detail.add-widget")
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
                onOpenSession: { onOpenAppSession(appID, $0.uuid, $0.mode) },
                onNewSession: { onNewAppSession(appID) }
            )
        case .overview:
            LocalAppOverviewSection(
                app: app,
                runtime: runtime,
                distribution: store.distributionMode,
                onOpenPreview: { path.append(.preview(appID)) }
            )
        case .mcp:
            LocalAppMcpSection(
                store: store,
                appID: appID,
                activeConversationID: activeConversationID
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

private struct LocalAppMcpSection: View {
    @Bindable var store: LocalAppsStore
    let appID: String
    let activeConversationID: String

    @State private var authoringGoal = ""
    @State private var pendingServiceEnabled: Bool?
    @State private var pendingToolEnabled: [String: Bool] = [:]

    private var inventory: LocalAppManagedMcpInventory {
        store.managedMcpInventory(appID: appID)
    }

    private var commandError: String? {
        store.managedMcpCommandError(appID: appID)
    }

    private var isPending: Bool {
        store.isManagedMcpPending(appID: appID)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                LocalAppMcpSummaryCard(inventory: inventory)
                LocalAppMcpControlsCard(
                    inventory: inventory,
                    commandError: commandError,
                    isPending: isPending,
                    serviceEnabled: Binding(
                        get: { pendingServiceEnabled ?? inventory.enabled },
                        set: updateServiceEnabled
                    ),
                    conversationPinned: Binding(
                        get: { inventory.pinnedToCurrentConversation },
                        set: updateConversationPinned
                    ),
                    canPinConversation: inventory.enabled && !activeConversationID.isEmpty
                )
                LocalAppMcpAuthoringCard(
                    goal: $authoringGoal,
                    status: inventory.status,
                    isPending: isPending,
                    onStart: startAuthoring
                )
                LocalAppMcpToolsCard(
                    inventory: inventory,
                    isPending: isPending,
                    pendingToolEnabled: $pendingToolEnabled,
                    onToggle: updateToolEnabled
                )
            }
            .padding()
        }
        .task {
            if authoringGoal.isEmpty {
                authoringGoal = store.app(id: appID)?.displayBrief ?? ""
            }
            await store.refreshManagedMcpInventory()
        }
        .onChange(of: inventory.enabled) { _, enabled in
            if pendingServiceEnabled == enabled {
                pendingServiceEnabled = nil
            }
        }
        .onChange(of: inventory.enabledTools) { _, enabledTools in
            pendingToolEnabled = pendingToolEnabled.filter { key, value in
                enabledTools.contains(key) != value
            }
        }
    }

    private func updateServiceEnabled(_ enabled: Bool) {
        pendingServiceEnabled = enabled
        store.clearManagedMcpCommandError(appID: appID)
        Task {
            let sent = await store.setManagedMcpEnabled(appID: appID, enabled: enabled)
            if !sent {
                pendingServiceEnabled = nil
            }
        }
    }

    private func updateToolEnabled(_ toolName: String, enabled: Bool) {
        pendingToolEnabled[toolName] = enabled
        store.clearManagedMcpCommandError(appID: appID)
        Task {
            let sent = await store.setManagedMcpToolEnabled(
                appID: appID,
                toolName: toolName,
                enabled: enabled
            )
            if !sent {
                pendingToolEnabled.removeValue(forKey: toolName)
            }
        }
    }

    private func updateConversationPinned(_ pinned: Bool) {
        guard !activeConversationID.isEmpty else { return }
        store.clearManagedMcpCommandError(appID: appID)
        Task {
            _ = await store.setManagedMcpConversationPinned(
                conversationID: activeConversationID,
                appID: appID,
                pinned: pinned
            )
        }
    }

    private func startAuthoring() {
        let goal = authoringGoal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { return }
        store.clearManagedMcpCommandError(appID: appID)
        Task {
            _ = await store.startManagedMcpAuthoring(appID: appID, userGoal: goal)
        }
    }
}

private struct LocalAppMcpSummaryCard: View {
    @Environment(\.theme) private var theme
    let inventory: LocalAppManagedMcpInventory

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "slider.horizontal.3")
                    .font(.title2)
                    .foregroundStyle(theme.accent)
                    .frame(width: 52, height: 52)
                    .background(theme.accent.opacity(0.12), in: .rect(cornerRadius: 14))
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 8) {
                        Text(inventory.serverName)
                            .font(.headline)
                            .textSelection(.enabled)
                        LocalAppPublicationBadgeView(badge: inventory.statusBadge)
                    }
                    Text(inventory.status.summary)
                        .font(.subheadline)
                        .foregroundStyle(theme.text3)
                    HStack(spacing: 6) {
                        LocalAppPublicationBadgeView(badge: inventory.publicationBadge)
                        LocalAppPublicationBadgeView(badge: inventory.uiVerification.badge)
                        LocalAppPublicationBadgeView(badge: inventory.mcpVerification.badge)
                    }
                }
                Spacer(minLength: 0)
            }

            VStack(alignment: .leading, spacing: 10) {
                LabeledContent("App", value: inventory.appName)
                LabeledContent("Status", value: inventory.status.title)
                LabeledContent("Tools") {
                    Text("\(inventory.enabledTools.count) of \(inventory.toolCount)")
                }
                LabeledContent("Settings revision") {
                    Text("\(inventory.settingsRevision)")
                }
                if !inventory.buildID.isEmpty {
                    LabeledContent("Build", value: inventory.buildID)
                }
                if !inventory.catalogDigest.isEmpty {
                    LabeledContent("Catalog", value: localAppMcpDigestSummary(inventory.catalogDigest))
                }
                if let widget = inventory.widget {
                    LabeledContent("Widget") {
                        Text(widget.title ?? widget.resourceURI ?? "Configured")
                            .multilineTextAlignment(.trailing)
                    }
                } else {
                    LabeledContent("Widget", value: "Not configured")
                }
            }
        }
        .padding()
        .background(theme.surface, in: .rect(cornerRadius: 16))
    }
}

private struct LocalAppMcpControlsCard: View {
    @Environment(\.theme) private var theme
    let inventory: LocalAppManagedMcpInventory
    let commandError: String?
    let isPending: Bool
    let serviceEnabled: Binding<Bool>
    let conversationPinned: Binding<Bool>
    let canPinConversation: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Assistant Exposure")
                        .font(.headline)
                    Text("Expose this app's managed MCP server to the assistant in compatible conversations.")
                        .font(.footnote)
                        .foregroundStyle(theme.text3)
                }
                Spacer(minLength: 12)
                Toggle("", isOn: serviceEnabled)
                    .labelsHidden()
                    .disabled(isPending || inventory.status == .needsSetup)
            }

            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Pin to current conversation")
                        .font(.subheadline.weight(.semibold))
                    Text("Keep this app in the current conversation's bounded Local App MCP set.")
                        .font(.footnote)
                        .foregroundStyle(theme.text3)
                }
                Spacer(minLength: 12)
                Toggle("", isOn: conversationPinned)
                    .labelsHidden()
                    .disabled(isPending || !canPinConversation)
            }

            if isPending {
                Label("Waiting for the host to apply the MCP change…", systemImage: "clock")
                    .font(.footnote)
                    .foregroundStyle(.orange)
            }

            if let commandError {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Last error")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(theme.danger)
                    Text(commandError)
                        .font(.footnote)
                        .foregroundStyle(theme.text3)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(theme.danger.opacity(0.08), in: .rect(cornerRadius: 12))
            }
        }
        .padding()
        .background(theme.surface, in: .rect(cornerRadius: 16))
    }
}

private struct LocalAppMcpAuthoringCard: View {
    @Environment(\.theme) private var theme
    @Binding var goal: String
    let status: LocalAppManagedMcpStatus
    let isPending: Bool
    let onStart: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(String(localized: "local_apps_mcp_customize_title"))
                .font(.headline)
            Text(String(localized: "local_apps_mcp_customize_detail"))
                .font(.footnote)
                .foregroundStyle(theme.text3)

            TextEditor(text: $goal)
                .frame(minHeight: 104)
                .padding(8)
                .background(theme.windowBg, in: .rect(cornerRadius: 12))
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(theme.border, lineWidth: 1)
                )

            HStack {
                Text(
                    status == .needsSetup
                        ? String(localized: "local_apps_mcp_needs_setup_detail") : status.summary
                )
                .font(.footnote)
                .foregroundStyle(theme.text3)
                Spacer()
                // Mirrors Android's split (`LocalAppsScreen.kt`): a fresh app
                // GENERATES a design, a previously-authored one is REVISED —
                // one shared "Start Customizing" label made every visit after
                // the first one say the wrong verb.
                Button(
                    status == .needsSetup
                        ? String(localized: "local_apps_mcp_start_authoring")
                        : String(localized: "local_apps_mcp_update_authoring"),
                    action: onStart
                )
                .buttonStyle(.borderedProminent)
                .disabled(isPending || goal.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding()
        .background(theme.surface, in: .rect(cornerRadius: 16))
    }
}

private struct LocalAppMcpToolsCard: View {
    @Environment(\.theme) private var theme
    let inventory: LocalAppManagedMcpInventory
    let isPending: Bool
    @Binding var pendingToolEnabled: [String: Bool]
    let onToggle: (String, Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Tools")
                    .font(.headline)
                Spacer()
                Text("\(inventory.toolCount)")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.text3)
            }

            if inventory.tools.isEmpty {
                ContentUnavailableView {
                    Label("No managed MCP tools yet", systemImage: "wrench.and.screwdriver")
                } description: {
                    Text("Run MCP authoring to create an app-specific tool surface before exposing it to the assistant.")
                }
            } else {
                LazyVStack(spacing: 10) {
                    ForEach(inventory.tools) { tool in
                        LocalAppMcpToolRow(
                            tool: tool,
                            isEnabled: Binding(
                                get: { pendingToolEnabled[tool.name] ?? inventory.isToolEnabled(tool.name) },
                                set: { onToggle(tool.name, $0) }
                            ),
                            isPending: isPending
                        )
                    }
                }
            }
        }
        .padding()
        .background(theme.surface, in: .rect(cornerRadius: 16))
    }
}

private struct LocalAppMcpToolRow: View {
    @Environment(\.theme) private var theme
    let tool: LocalAppManagedMcpTool
    let isEnabled: Binding<Bool>
    let isPending: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(tool.name)
                        .font(.system(size: 14, weight: .semibold))
                    Text(tool.title ?? tool.description ?? "No description")
                        .font(.footnote)
                        .foregroundStyle(theme.text3)
                }
                Spacer(minLength: 12)
                Toggle("", isOn: isEnabled)
                    .labelsHidden()
                    .disabled(isPending)
            }
            VStack(alignment: .leading, spacing: 6) {
                Text(tool.ceilingSummary)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.accent)
                Text(tool.semanticFlowSummary)
                    .font(.system(size: 11.5, design: .monospaced))
                    .foregroundStyle(theme.text3)
                    .lineLimit(4)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.windowBg, in: .rect(cornerRadius: 12))
    }
}

/// The app's workspace-scoped session catalog (`ListAppSessions` →
/// `AppSessionsChanged`): the pinned init session first with an「初始化」
/// badge, the rest modified-descending, paged by「加载更多」.
struct LocalAppSessionsSection: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    let appID: String
    let onOpenSession: (LocalAppSessionRow) -> Void
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
                        onOpenSession(row)
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
                        HStack(spacing: 8) {
                            Text(app.displayName).font(.title2.bold())
                            LocalAppPublicationBadgeView(badge: app.workflow.statusBadge)
                            if let uiVerification = app.uiVerification {
                                LocalAppPublicationBadgeView(badge: uiVerification.badge)
                            }
                            if let mcpVerification = app.mcpVerification {
                                LocalAppPublicationBadgeView(badge: mcpVerification.badge)
                            }
                        }
                        Text(app.draftStatusLine ?? app.workflow.label)
                            .foregroundStyle(theme.text3)
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
                    // A shell's brief is `""` — the interview has not happened
                    // yet — so the row is omitted rather than rendered empty.
                    if let brief = app.displayBrief {
                        LabeledContent("local_apps_brief", value: brief)
                    }
                    if let profileStatus = app.runtimeProfileStatus {
                        LabeledContent("local_apps_runtime_profile_health_title") {
                            Label(profileStatus.title, systemImage: profileStatus.systemImageName)
                                .foregroundStyle(runtimeProfileStatusColor(profileStatus))
                                .multilineTextAlignment(.trailing)
                        }
                    }
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

    private func runtimeProfileStatusColor(_ status: LocalAppRuntimeProfileStatus) -> Color {
        switch status {
        case .verified: .green
        case .dependenciesDirty, .migrationAvailable, .rebuildRequired: .orange
        case .coreDependencyDrift, .runtimeBundleMissing, .runtimeContractCorrupt: .red
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
    /// The close control pops this route. `dismiss` keeps the construction site
    /// (LocalAppsLibraryView.destination) at two arguments and works whether the
    /// route was pushed from the detail screen or seeded by the widget deep link.
    @Environment(\.dismiss) private var dismiss
    @Bindable var store: LocalAppsStore
    let appID: String

    private var previewURL: URL? {
        store.runtimes[appID]?.url
    }

    /// Every non-running state used to collapse into one hourglass with no
    /// reason and no action, so a FAILED runtime looked identical to one that
    /// was still booting and the user had no way to retry.
    private var placeholder: LocalAppPreviewPlaceholder? {
        LocalAppPreviewPlaceholder.forStatus(store.runtimes[appID])
    }

    private var startButton: some View {
        Button("common_start") { Task { await store.start(appID: appID) } }
    }

    var body: some View {
        Group {
            if let url = previewURL {
                // The running app draws its own header, so a host navigation bar
                // would be a second stacked bar over it. Give the page the screen
                // below the status bar and float the run controls on top.
                ZStack(alignment: .bottomLeading) {
                    LocalAppWebView(
                        appID: appID,
                        url: url,
                        onBridgeRequest: { request in
                            Task { await store.executeBridge(request) }
                        }
                    )
                    // LocalAppWebView already ignores `.bottom` for its own
                    // consumers; widening it to the horizontal edges here (rather
                    // than inside that view) keeps LocalAppEmbeddedPreview — the
                    // detail page's tab — laid out below the picker exactly as it
                    // is today.
                    //
                    // ⚠️ Do NOT "tidy" this back to `.all`. The TOP inset is kept
                    // deliberately, because the page cannot recover from losing it:
                    // a page's only source of a top inset is the injected
                    // readInsets() probe over env(safe-area-inset-top), and
                    // lingxi-provider.jsx writes that reading onto
                    // document.documentElement.style as a FIXED px string that
                    // foundation.css maps to --ion-safe-area-top. An inline style
                    // outranks the :root env() fallback, so once a 0px reading is
                    // written the live env() can never win it back. The only re-read
                    // is resync() on resize / orientationchange / visualViewport
                    // resize — and under viewport-fit=cover the layout viewport does
                    // NOT change when a WKWebView's safeAreaInsets do. A page that
                    // paints before the first reading arrives would therefore keep a
                    // 0px top inset for the whole session and park its own Ionic
                    // header under the status-bar clock. Keeping the top inset makes
                    // env(safe-area-inset-top) legitimately 0 and matches Android,
                    // whose whole local-apps route is wrapped in
                    // windowInsetsPadding(WindowInsets.systemBars) at RootScreen.kt
                    // and so structurally cannot go under the status bar either.
                    // The host navigation bar stays hidden; there is still one bar.
                    .ignoresSafeArea(.container, edges: [.horizontal, .bottom])

                    // Bottom-LEADING, not trailing: Ionic's IonFab defaults to
                    // vertical="bottom" horizontal="end", which resolves to
                    // bottom:10px; right:calc(10px + var(--ion-safe-area-right,0px))
                    // with a 56px button — so the bottom-trailing corner is exactly
                    // where a hosted app parks its own furniture, and the host
                    // control would sit on top of it. The ZStack itself stays inside
                    // the safe area, so the control clears the home indicator
                    // without reading insets.
                    //
                    // No `isRunning` parameter. `previewURL` is
                    // `store.runtimes[appID]?.url`, and `LocalAppRuntimeStatus.url`
                    // returns non-nil ONLY out of `case .running(url)` — so inside
                    // `if let url = previewURL` the status IS `.running`, and the
                    // old argument was literally `previewURL != nil` at a site
                    // already guarded by `previewURL != nil`. Its start branch was
                    // unreachable. Re-deriving the flag from `store.runtimes[appID]`
                    // would not fix that: it is the same fact spelled differently,
                    // and it would still be a constant `true` here. So running-ness
                    // is not passed at all — the control shows pause because the
                    // thing it floats over is, by construction of the enum, a live
                    // runtime, which is a derivation that cannot disagree with what
                    // is rendered. Starting a STOPPED runtime belongs to the
                    // placeholder arms below, where the user actually is when there
                    // is nothing to pause; Android says the same thing at its own
                    // else branch ("no app content to fill the screen, so no pill —
                    // the centred placeholder keeps its own start button").
                    LocalAppRunControl(
                        onPause: { Task { await store.stop(appID: appID) } },
                        onExit: { dismiss() }
                    )
                    .padding(16)
                }
            } else {
                switch placeholder {
                case .transient, .none:
                    ContentUnavailableView {
                        Label("local_apps_preview_not_ready", systemImage: "hourglass")
                    } description: {
                        ProgressView()
                    }
                case let .failed(reason):
                    ContentUnavailableView {
                        Label("local_apps_runtime_failed_title", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(reason)
                    } actions: {
                        Button("common_retry") { Task { await store.start(appID: appID) } }
                    }
                case let .suspended(reason):
                    ContentUnavailableView {
                        Label("local_apps_runtime_suspended_plain", systemImage: "pause.circle")
                    } description: {
                        if let reason { Text(reason) }
                    } actions: {
                        startButton
                    }
                case .idle:
                    ContentUnavailableView {
                        Label("local_apps_preview_not_running", systemImage: "safari")
                    } description: {
                        Text("local_apps_preview_not_running_detail")
                    } actions: {
                        startButton
                    }
                }
            }
        }
        .navigationTitle("local_apps_section_preview")
        .navigationBarTitleDisplayMode(.inline)
        // Only the running surface goes full-bleed. Deleting the title outright
        // and hiding the bar in every state would strand the user on the
        // placeholders: they carry no pill, so the host bar's back button is
        // their only way out once the runtime is stopped.
        .toolbar(previewURL == nil ? .visible : .hidden, for: .navigationBar)
    }
}

/// The floating run control for the running app surface.
///
/// Not named `Pill` — `Pill` is already a text chip in
/// Components/SharedComponents.swift. "Pill" here is a shape, not a type.
///
/// Every accessibility identifier sits on a leaf `Button`. An identifier on the
/// enclosing capsule would replace its children's identifiers at runtime, so the
/// ids greppable here would not exist on device.
private struct LocalAppRunControl: View {
    @Environment(\.theme) private var theme

    let onPause: () -> Void
    let onExit: () -> Void

    @State private var expanded = false

    /// Every button, collapsed one included, is this square.
    ///
    /// 44, not 40. 40 is under Apple's 44x44pt minimum, and it is also under the
    /// `--platform-control-min` the host itself publishes to hosted pages for
    /// iOS (`controlDensity: 44` in local-apps/templates/*/lib/platform-adapter.js)
    /// — the host was holding its own guests to a bar it did not meet. There is
    /// no fallback for a missed tap here: with the navigation bar hidden this is
    /// the only host affordance on the screen, and for an app generated from the
    /// CANVAS scaffold it is the only affordance at all — that template's
    /// game-screen.jsx renders a `<canvas>` plus overlays and contains no
    /// IonPage, IonHeader or IonToolbar anywhere in the template.
    private static let tapTarget: CGFloat = 44

    /// Icon-only, but the `Label` keeps its localized title as the VoiceOver
    /// label, and the square frame plus `contentShape` gives a real tap target.
    private func glyph(_ title: LocalizedStringKey, _ systemName: String) -> some View {
        Label(title, systemImage: systemName)
            .labelStyle(.iconOnly)
            .frame(width: Self.tapTarget, height: Self.tapTarget)
            .contentShape(Rectangle())
    }

    var body: some View {
        HStack(spacing: 2) {
            // FIRST, not last. The capsule is anchored bottom-LEADING, so the
            // leading-most child sits on fixed pixels and everything after it
            // grows away from the corner. With the toggle last, expanding slid
            // it away by one button and dropped the newly composed first child
            // onto the exact rect the finger had just tapped — and on this
            // client that first child was CLOSE, so tapping the same pixel twice
            // popped the route instead of collapsing the control. Keeping the
            // toggle leading-most makes its hit rect identical in both states —
            // the frame is fixed at 44 and `.padding(.horizontal, 4)` puts its
            // leading edge 4pt inside the capsule no matter how many siblings
            // follow — so tap-tap always means expand-then-collapse.
            Button {
                withAnimation(.snappy(duration: 0.2)) { expanded.toggle() }
            } label: {
                // `local_apps_more` ("More"), not the surface's own title: the
                // Label's title IS the VoiceOver label under `.iconOnly`, and with
                // the host navigation bar hidden this is the only host control on
                // the screen — a noun there announces as "本地应用，按钮". Android's
                // collapsed button already uses this same key.
                //
                // One STATIC glyph, like Android's `Icons.Rounded.MoreVert`. The
                // chevron pair it replaces made the resting control read as a
                // drawer handle on iOS and as a menu on Android, for the same
                // "More" label.
                glyph("local_apps_more", "ellipsis")
            }
            .accessibilityIdentifier("local-apps.run.controls")

            if expanded {
                // Stopping the runtime IS the pause: the URL goes nil and the
                // placeholder comes back. There is no freeze-the-frame pause.
                Button(action: onPause) {
                    glyph("local_apps_run_pause", "pause.fill")
                }
                .accessibilityIdentifier("local-apps.run.pause")

                Button(action: onExit) {
                    glyph("local_apps_run_exit", "xmark")
                }
                .accessibilityIdentifier("local-apps.run.exit")
            }
        }
        .buttonStyle(.plain)
        .font(.system(size: 15, weight: .semibold))
        .foregroundStyle(theme.text)
        .padding(.horizontal, 4)
        .background(theme.surface.opacity(0.92), in: Capsule())
        .overlay(Capsule().strokeBorder(theme.border, lineWidth: 1))
        .shadow(color: .black.opacity(0.22), radius: 10, y: 4)
        // Resting state stays out of the app's way; a tap brings it forward.
        // 0.6 is Android's resting alpha (RunPill's `alpha(if (expanded) 1f else 0.6f)`);
        // 0.45 made the only affordance on a full-bleed canvas harder to find than
        // its twin on the other client.
        .opacity(expanded ? 1 : 0.6)
    }
}

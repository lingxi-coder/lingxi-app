import SwiftUI

enum LocalAppsRoute: Hashable {
    /// The brief-input create screen — there is no template catalog to pick
    /// from, only a one-line brief to collect.
    case create
    case details(String)
    case preview(String)
}

struct LocalAppsRootView: View {
    @Bindable var store: LocalAppsStore
    let initialAppID: String?
    let availableModels: [String]
    let activeModelID: String
    let onDismiss: () -> Void
    /// Called with `(appID, sessionUUID)` when the user taps a row of the
    /// app's session catalog. RootView dismisses this cover and switches the
    /// conversation scope to the app, resuming that session.
    var onOpenAppSession: (String, String) -> Void = { _, _ in }
    /// The create-flow landing: like `onOpenAppSession`, but RootView also
    /// queues the kickoff message that starts the create-local-app flow in
    /// the (empty) init session.
    var onOpenCreatedAppSession: (String, String, String, String?) -> Void = { _, _, _, _ in }
    /// Called with `appID` for「新会话」. RootView dismisses this cover and
    /// starts a fresh conversation in the app's scope.
    var onNewAppSession: (String) -> Void = { _ in }

    @State private var path: [LocalAppsRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            LocalAppsLibraryScreen(
                store: store,
                path: $path,
                availableModels: availableModels,
                activeModelID: activeModelID,
                onDismiss: onDismiss,
                onOpenAppSession: onOpenCreatedAppSession
            )
                .navigationDestination(for: LocalAppsRoute.self) { route in
                    destination(route)
                }
        }
        .task {
            await store.refresh()
            if let initialAppID {
                // An inbound UI-automation request should land straight on
                // the live preview rather than one tap short at `.details`.
                let shouldOpenPreview = store.hasPendingUIRequest(appID: initialAppID)
                path = [shouldOpenPreview ? .preview(initialAppID) : .details(initialAppID)]
            }
        }
        .alert(
            "local_apps_error_title",
            isPresented: Binding(
                get: { store.errorMessage != nil },
                set: { if !$0 { store.clearError() } }
            )
        ) {
            Button("common_ok", role: .cancel, action: store.clearError)
        } message: {
            Text(store.errorMessage ?? String(localized: "common_unknown_error"))
        }
        // Every capability request a user can trigger is raised from inside this
        // cover, and the root presenter (RootView) cannot present while the cover
        // owns the controller — so the prompt has to be anchored here too.
        .sheet(
            item: Binding(
                get: { store.pendingPermission },
                set: { _ in }
            )
        ) { prompt in
            LocalAppPermissionSheet(store: store, prompt: prompt)
        }
        .sheet(
            item: Binding(
                get: { store.pendingProfileProposal },
                set: { _ in }
            )
        ) { proposal in
            LocalAppProfileProposalSheet(store: store, proposal: proposal)
        }
    }

    @ViewBuilder
    private func destination(_ route: LocalAppsRoute) -> some View {
        switch route {
        case .create:
            LocalAppCreateView(
                store: store,
                path: $path,
                availableModels: availableModels,
                activeModelID: activeModelID
            )
        case let .details(appID):
            LocalAppDetailView(
                store: store,
                appID: appID,
                path: $path,
                onOpenAppSession: onOpenAppSession,
                onNewAppSession: onNewAppSession
            )
        case let .preview(appID):
            LocalAppPreviewView(store: store, appID: appID)
        }
    }
}

struct LocalAppProfileProposalSheet: View {
    @Bindable var store: LocalAppsStore
    let proposal: LocalAppProfileProposal

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Label("local_apps_profile_proposal_title", systemImage: "text.badge.checkmark")
                        .font(.title3.bold())
                    Text(proposal.reason)
                        .foregroundStyle(.secondary)
                    Text(proposal.instructions)
                        .font(.body)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding()
            }
            .navigationTitle("local_apps_profile_proposal_navigation_title")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_cancel") { store.resolveProfileProposal(false) }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("local_apps_profile_proposal_apply") { store.resolveProfileProposal(true) }
                }
            }
        }
    }
}

struct LocalAppPermissionSheet: View {
    @Bindable var store: LocalAppsStore
    let prompt: LocalAppPermissionPrompt

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 18) {
                Label(prompt.title, systemImage: "hand.raised.fill")
                    .font(.title3.bold())
                Text(prompt.reason)
                    .foregroundStyle(.secondary)
                if let domain = prompt.domain {
                    LabeledContent("local_apps_domain") {
                        Text(domain)
                            .font(.system(.body, design: .monospaced))
                            .textSelection(.enabled)
                    }
                }
                Spacer()
                VStack(spacing: 10) {
                    permissionButton(.once, prominent: true)
                    permissionButton(.session)
                    permissionButton(.always)
                    permissionButton(.deny)
                }
            }
            .padding(24)
            .navigationTitle("local_apps_permissions_title")
            .navigationBarTitleDisplayMode(.inline)
        }
        .interactiveDismissDisabled()
        .presentationDetents([.medium])
        .accessibilityIdentifier("local-apps.permission.\(prompt.id)")
    }

    @ViewBuilder
    private func permissionButton(
        _ decision: LocalAppCapabilityDecision,
        prominent: Bool = false
    ) -> some View {
        if prominent {
            permissionAction(decision)
                .buttonStyle(.borderedProminent)
        } else {
            permissionAction(decision)
                .buttonStyle(.bordered)
        }
    }

    private func permissionAction(_ decision: LocalAppCapabilityDecision) -> some View {
        Button {
            Task { await store.resolvePendingPermission(decision) }
        } label: {
            Text(decision.label).frame(maxWidth: .infinity)
        }
        .tint(decision == .deny ? .red : nil)
        .accessibilityIdentifier("local-apps.permission.\(decision.rawValue)")
    }
}

private struct LocalAppsLibraryScreen: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    @Binding var path: [LocalAppsRoute]
    let availableModels: [String]
    let activeModelID: String
    let onDismiss: () -> Void
    /// Threaded from the root cover: `(appID, sessionUUID, brief)` →
    /// dismiss + switch into the app scope (the create flow's init-chat
    /// landing, kickoff seeded with the brief).
    var onOpenAppSession: (String, String, String, String?) -> Void = { _, _, _, _ in }

    @State private var pendingDelete: LocalAppSummary?

    var body: some View {
        Group {
            if store.filteredApps.isEmpty {
                emptyState
            } else {
                List(store.filteredApps) { app in
                    LocalAppLibraryRow(
                        app: app,
                        runtime: store.runtimes[app.id] ?? .stopped,
                        onOpen: { open(app) },
                        onStart: { Task { await store.start(appID: app.id) } },
                        onStop: { Task { await store.stop(appID: app.id) } },
                        onDelete: { pendingDelete = app }
                    )
                }
                .listStyle(.plain)
                .refreshable { await store.refresh() }
            }
        }
        .searchable(text: $store.searchQuery, prompt: "local_apps_search_prompt")
        .navigationTitle("local_apps_title")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("common_close", action: onDismiss)
            }
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    path.append(.create)
                } label: {
                    Label("local_apps_create", systemImage: "plus")
                }
                .accessibilityIdentifier("local-apps.create")
            }
        }
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 8) {
                Image(systemName: store.distributionMode == .full ? "server.rack" : "doc.badge.gearshape")
                Text("local_apps_runtime_footer \(store.distributionMode.runtimeLabel)")
            }
            .font(.caption)
            .foregroundStyle(theme.text3)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity)
            .background(.bar)
        }
        .onAppear(perform: openCreatedAppIfNeeded)
        .onChange(of: store.createdAppID) { _, _ in openCreatedAppIfNeeded() }
        .onChange(of: store.createdAppSession) { _, _ in openCreatedInitChatIfNeeded() }
        .onAppear { openCreatedInitChatIfNeeded() }
        .confirmationDialog(
            "local_apps_delete_confirm \(pendingDelete?.name ?? String(localized: "local_apps_title"))",
            isPresented: Binding(
                get: { pendingDelete != nil },
                set: { if !$0 { pendingDelete = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("local_apps_delete", role: .destructive) {
                guard let app = pendingDelete else { return }
                pendingDelete = nil
                Task { _ = await store.delete(appID: app.id) }
            }
            Button("common_cancel", role: .cancel) { pendingDelete = nil }
        } message: {
            Text("local_apps_delete_detail")
        }
    }

    private var emptyState: some View {
        ContentUnavailableView {
            Label("local_apps_empty", systemImage: "square.grid.2x2")
        } description: {
            Text(store.isRefreshing ? String(localized: "local_apps_empty_loading") : String(localized: "local_apps_empty_hint"))
        } actions: {
            Button("local_apps_create") { path.append(.create) }
                .buttonStyle(.borderedProminent)
        }
    }

    private func open(_ app: LocalAppSummary) {
        path.append(.details(app.id))
    }

    private func openCreatedAppIfNeeded() {
        // Fallback landing only (init-session pin never arrived): show the
        // app's details/sessions page instead of the init chat.
        guard let appID = store.consumeCreatedAppID() else { return }
        path = [.details(appID)]
    }

    private func openCreatedInitChatIfNeeded() {
        // v3 target behavior: a fresh create lands DIRECTLY in the app's
        // init conversation — dismiss the cover and switch into the app
        // scope resuming the pinned init session.
        guard let created = store.consumeCreatedAppSession() else { return }
        onOpenAppSession(
            created.appID,
            created.initSessionID,
            created.brief,
            created.modelOverride
        )
    }
}

private struct LocalAppLibraryRow: View {
    @Environment(\.theme) private var theme
    let app: LocalAppSummary
    let runtime: LocalAppRuntimeStatus
    let onOpen: () -> Void
    let onStart: () -> Void
    let onStop: () -> Void
    let onDelete: () -> Void

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 14) {
                Image(systemName: localAppIconSystemName)
                    .font(.title3)
                    .foregroundStyle(theme.accent)
                    .frame(width: 42, height: 42)
                    .background(theme.accent.opacity(0.12), in: .rect(cornerRadius: 11))

                VStack(alignment: .leading, spacing: 4) {
                    Text(app.name)
                        .font(.headline)
                        .foregroundStyle(theme.text)
                    HStack(spacing: 6) {
                        Circle()
                            .fill(runtimeColor)
                            .frame(width: 7, height: 7)
                        Text("\(app.workflow.label) · \(runtime.label)")
                            .font(.caption)
                            .foregroundStyle(theme.text3)
                            .lineLimit(1)
                    }
                }
                Spacer()
                Menu {
                    if case .running = runtime {
                        Button("composer_stop", systemImage: "stop.fill", action: onStop)
                    } else {
                        Button("common_start", systemImage: "play.fill", action: onStart)
                    }
                    Button("common_delete", systemImage: "trash", role: .destructive, action: onDelete)
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .frame(width: 36, height: 36)
                }
                .accessibilityLabel("local_apps_row_actions \(app.name)")
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("local-apps.row.\(app.id)")
    }

    private var runtimeColor: Color {
        switch runtime {
        case .running: theme.ok
        case .starting, .stopping: .orange
        case .failed: theme.danger
        case .stopped, .suspended: theme.text4
        }
    }
}

/// Collects the one-line brief `createApp(brief:)` needs and nothing else —
/// no display name, no template. Replaces the deleted static template
/// picker + its create sheet (local-apps#questionnaire, Task 2/5/13): there
/// is no more catalog to choose from, only a brief for the LLM to author a
/// questionnaire from. Task 16 ("iOS 创建入口与常驻迭代输入") owns the real,
/// polished create entry point; this is the minimal working replacement that
/// keeps the create flow honest (it sends a REAL brief, not the app name
/// relabeled — see `LocalAppsStore.createApp(brief:)`) until then.
struct LocalAppCreateView: View {
    @Bindable var store: LocalAppsStore
    @Binding var path: [LocalAppsRoute]
    let availableModels: [String]
    let activeModelID: String

    @State var brief = ""
    @State private var gitEnabled = true
    @State private var creating = false
    @State private var modelOverride: String?
    @State private var showingModelPicker = false

    init(
        store: LocalAppsStore,
        path: Binding<[LocalAppsRoute]>,
        availableModels: [String] = [],
        activeModelID: String = ""
    ) {
        self.store = store
        self._path = path
        self.availableModels = availableModels
        self.activeModelID = activeModelID
    }

    /// The submit predicate, as a pure function of the text.
    ///
    /// It lives here rather than inline in `canSubmit` because `brief` is
    /// `@State`: assigning to it on a bare struct outside a view hierarchy
    /// does NOT take effect, so a test that pokes `view.brief` and reads
    /// `view.canSubmit` is really only ever reading the initial `""`. That
    /// made the whitespace test pass vacuously (it asserts `false` on a value
    /// that was already empty) while the non-empty test failed — the first
    /// run of the iOS suite is what surfaced it. Tests call this directly.
    static func canSubmit(brief: String) -> Bool {
        !brief.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Instance mirror of the toolbar button's guard below (minus the
    /// transient `creating` flag) — same source of truth `body` disables on,
    /// not a parallel description of it. `createApp(brief:)` itself repeats
    /// this same empty check server-side, so this is belt-and-suspenders,
    /// not the only gate.
    var canSubmit: Bool { Self.canSubmit(brief: brief) }

    var body: some View {
        Form {
            Section("local_apps_create_brief_section") {
                TextEditor(text: $brief)
                    .frame(minHeight: 120)
                Text("local_apps_create_brief_detail")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Toggle("local_apps_create_git_version_control", isOn: $gitEnabled)
            }
            Section("local_apps_create_model_section") {
                Button {
                    showingModelPicker = true
                } label: {
                    LabeledContent("local_apps_create_model_label") {
                        Text(modelSelectionLabel)
                            .foregroundStyle(.secondary)
                    }
                }
                .disabled(availableModels.isEmpty)
                if modelOverride != nil {
                    Button("local_apps_create_model_follow_current") {
                        modelOverride = nil
                    }
                }
                Text("local_apps_create_model_detail")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .navigationTitle("local_apps_create")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button(creating ? "local_apps_creating" : "local_apps_create") {
                    Task { await create() }
                }
                .disabled(creating || !canSubmit)
                .accessibilityIdentifier("local-apps.create.submit")
            }
        }
        .sheet(isPresented: $showingModelPicker) {
            ModelPickerSheet(
                availableModels: availableModels,
                activeModelId: modelOverride ?? activeModelID,
                recentModels: [],
                onSelect: { reference in
                    modelOverride = reference
                    showingModelPicker = false
                },
                onDismiss: { showingModelPicker = false }
            )
        }
    }

    private var modelSelectionLabel: String {
        guard let modelOverride else {
            if activeModelID.isEmpty {
                return String(localized: "local_apps_create_model_follow_current")
            }
            return String(localized: "local_apps_create_model_follow_current_value \(ModelDisplay.shortName(for: activeModelID))")
        }
        let item = ModelDisplay.item(for: modelOverride)
        return "\(ModelDisplay.providerName(for: item.providerId)) · \(item.shortName)"
    }

    private func create() async {
        creating = true
        let succeeded = await store.createApp(
            brief: brief,
            gitEnabled: gitEnabled,
            modelOverride: modelOverride
        )
        creating = false
        if succeeded, !path.isEmpty {
            path.removeLast()
        }
    }
}

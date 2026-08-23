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
    let availableModelDetails: [String: ModelRuntimeDetails]
    let activeModelID: String
    let onDismiss: () -> Void
    /// Called with `(appID, sessionUUID)` when the user taps a row of the
    /// app's session catalog. RootView dismisses this cover and switches the
    /// conversation scope to the app, resuming that session.
    var onOpenAppSession: (String, String) -> Void = { _, _ in }
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
                availableModelDetails: availableModelDetails,
                activeModelID: activeModelID,
                onDismiss: onDismiss
            )
            .navigationDestination(for: LocalAppsRoute.self) { route in
                destination(route)
            }
        }
        .task {
            await store.refresh()
            if let initialAppID {
                let launchDestination = store.consumeLaunchDestination(appID: initialAppID)
                let shouldOpenPreview = launchDestination == .preview
                    || store.hasPendingUIRequest(appID: initialAppID)
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
                availableModelDetails: availableModelDetails,
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
    let availableModelDetails: [String: ModelRuntimeDetails]
    let activeModelID: String
    let onDismiss: () -> Void
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
        .sheet(
            item: Binding(
                get: { store.pendingWidgetSetup },
                set: { _ in }
            )
        ) { setup in
            LocalAppWidgetSetupSheet(appName: setup.appName) {
                // Clearing this un-gates `RootView.landCreatedAppIfReady`,
                // which takes the user into the new app's conversation.
                store.completeWidgetSetup()
                // …and drains the library's own fallback landing. Without this
                // `createdAppID` stays armed for the process lifetime: the
                // `onChange` above ran while `pendingWidgetSetup` was still
                // set, so it returned at the guard WITHOUT consuming, and
                // nothing re-invokes it once the sheet closes. The next plain
                // visit to the library would then hit `onAppear`, drain the
                // stale id, and drop the user on that old app's details page.
                openCreatedAppIfNeeded()
            }
        }
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
        // The library's own landing: show the new app's details page. It only
        // ever wins if the cover is still up, which means `RootView` has not
        // taken the user into the app's conversation — the primary landing.
        guard store.pendingWidgetSetup == nil else { return }
        guard let appID = store.consumeCreatedAppID() else { return }
        path = [.details(appID)]
    }
}

private struct LocalAppWidgetSetupSheet: View {
    let appName: String
    let onContinue: () -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 18) {
                Label("local_apps_widget_setup_title", systemImage: "rectangle.on.rectangle")
                    .font(.title3.bold())
                Text("local_apps_widget_setup_message \(appName)")
                    .foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 10) {
                    Text("local_apps_widget_setup_step_1")
                    Text("local_apps_widget_setup_step_2")
                    Text("local_apps_widget_setup_step_3")
                    Text("local_apps_widget_setup_step_4 \(appName)")
                }
                .font(.body)
                Spacer()
            }
            .padding()
            .navigationTitle("local_apps_widget_setup_title")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("local_apps_widget_setup_continue") {
                        onContinue()
                        dismiss()
                    }
                }
            }
        }
        .interactiveDismissDisabled()
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

/// The create sheet: a one-line brief, then a name and a surface to confirm.
///
/// Two steps, in ONE sheet, because both of the second step's fields are fixed
/// at creation — a surface is immutable once scaffolded and apps have no rename
/// — so neither may be decided by something the user never saw. The host
/// proposes both from the brief (`ProposeAppIdentity`, one headless model call,
/// no conversation); this screen shows the proposal and gives the user the last
/// word.
///
/// The app is then created OUTRIGHT. There is no intake conversation: the app's
/// first conversation opens in the app's own scope, so its cwd is the app
/// workspace from the first message.
struct LocalAppCreateView: View {
    @Bindable var store: LocalAppsStore
    @Binding var path: [LocalAppsRoute]
    let availableModels: [String]
    let availableModelDetails: [String: ModelRuntimeDetails]
    let activeModelID: String

    /// Which half of the sheet is on screen.
    enum Step: Equatable {
        /// Typing the brief.
        case brief
        /// Confirming the proposed name and surface.
        case identity
    }

    @State var brief = ""
    @State private var step: Step = .brief
    @State private var name = ""
    @State private var surface: LocalAppSurface = .dom
    @State private var proposing = false
    @State private var gitEnabled = true
    @State private var addWidget = false
    @State private var creating = false
    @State private var modelOverride: String?
    @State private var showingModelPicker = false

    init(
        store: LocalAppsStore,
        path: Binding<[LocalAppsRoute]>,
        availableModels: [String] = [],
        availableModelDetails: [String: ModelRuntimeDetails] = [:],
        activeModelID: String = ""
    ) {
        self.store = store
        self._path = path
        self.availableModels = availableModels
        self.availableModelDetails = availableModelDetails
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
            switch step {
            case .brief: briefStep
            case .identity: identityStep
            }
        }
        .navigationTitle("local_apps_create")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { toolbarContent }
        .sheet(isPresented: $showingModelPicker) {
            ModelPickerSheet(
                availableModels: availableModels,
                detailsByReference: availableModelDetails,
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

    @ViewBuilder
    private var briefStep: some View {
        Section("local_apps_create_brief_section") {
            TextEditor(text: $brief)
                .frame(minHeight: 120)
            Text("local_apps_create_brief_detail")
                .font(.footnote)
                .foregroundStyle(.secondary)
            Toggle("local_apps_create_git_version_control", isOn: $gitEnabled)
            Toggle("local_apps_create_add_widget", isOn: $addWidget)
            Text("local_apps_create_add_widget_detail")
                .font(.footnote)
                .foregroundStyle(.secondary)
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

    @ViewBuilder
    private var identityStep: some View {
        Section("local_apps_create_name_label") {
            TextField("local_apps_create_name_label", text: $name)
                .accessibilityIdentifier("local-apps.create.name")
            Text("local_apps_create_name_hint")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        Section("local_apps_create_surface_section") {
            Picker("local_apps_create_surface_section", selection: $surface) {
                ForEach(LocalAppSurface.allCases, id: \.self) { option in
                    Text(option.label).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .accessibilityIdentifier("local-apps.create.surface")
            Text(surface.detail)
                .font(.footnote)
                .foregroundStyle(.secondary)
            Text("local_apps_create_surface_immutable")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        Section("local_apps_create_brief_section") {
            Text(brief)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(placement: .confirmationAction) {
            switch step {
            case .brief:
                Button(proposing ? "local_apps_create_naming" : "local_apps_create_next") {
                    Task { await advanceToIdentity() }
                }
                .disabled(proposing || !canSubmit)
                .accessibilityIdentifier("local-apps.create.next")
            case .identity:
                Button(creating ? "local_apps_creating" : "local_apps_create") {
                    Task { await create() }
                }
                .disabled(creating || !canSubmit)
                .accessibilityIdentifier("local-apps.create.submit")
            }
        }
        ToolbarItem(placement: .cancellationAction) {
            if step == .identity {
                Button("local_apps_create_back") { step = .brief }
                    .disabled(creating)
            }
        }
    }

    private var modelSelectionLabel: String {
        guard let modelOverride else {
            if activeModelID.isEmpty {
                return String(localized: "local_apps_create_model_follow_current")
            }
            return String(localized: "local_apps_create_model_follow_current_value \(ModelDisplay.shortName(for: activeModelID))")
        }
        let item = ModelDisplay.item(for: modelOverride, detailsByReference: availableModelDetails)
        return "\((item.details?.preferredProviderLabel ?? ModelDisplay.providerName(for: item.providerId))) · \(item.shortName)"
    }

    /// Ask the host to name and shape the app, then show what it said.
    ///
    /// Advances even when the proposal is the derived fallback: the fields are
    /// editable, so a model that is unreachable costs the user a moment of
    /// typing rather than blocking the create outright.
    private func advanceToIdentity() async {
        proposing = true
        let proposal = await store.proposeIdentity(brief: brief)
        proposing = false
        name = proposal.name
        surface = proposal.surface
        step = .identity
    }

    private func create() async {
        creating = true
        let succeeded = await store.createApp(
            brief: brief,
            name: name,
            surface: surface,
            gitEnabled: gitEnabled,
            modelOverride: modelOverride,
            addWidget: addWidget
        )
        creating = false
        if succeeded, !path.isEmpty {
            path.removeLast()
        }
    }
}

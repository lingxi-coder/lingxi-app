import SwiftUI

enum LocalAppsRoute: Hashable {
    case details(String)
    case preview(String)

    /// The app this route is showing. Both cases carry exactly one, and
    /// `.preview` is only ever pushed from `LocalAppDetailView` for the SAME
    /// app it is already showing, so every route on the stack at once shares
    /// one id — see `LocalAppsRootView`'s collapse-on-disappear below, which
    /// relies on that.
    var appID: String {
        switch self {
        case let .details(id): id
        case let .preview(id): id
        }
    }
}

struct LocalAppPublicationBadgeView: View {
    @Environment(\.theme) private var theme
    let badge: LocalAppStatusBadge

    var body: some View {
        Label(badge.label, systemImage: badge.systemImageName)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(tint.opacity(0.12), in: Capsule())
            .accessibilityLabel(badge.accessibilityLabel)
    }

    private var tint: Color {
        switch badge.tintName {
        case "green":
            theme.ok
        case "orange":
            .orange
        default:
            theme.text3
        }
    }
}

struct LocalAppsRootView: View {
    @Bindable var store: LocalAppsStore
    let initialAppID: String?
    var activeConversationID: String = ""
    let onDismiss: () -> Void
    /// Called with `(appID, sessionUUID, mode)` when the user taps a row of the
    /// app's session catalog. The mode is part of the resume identity: Rust
    /// rejects a transcript opened under the other capability profile.
    var onOpenAppSession: (String, String, SessionMode) -> Void = { _, _, _ in }
    /// Called with `appID` for「新会话」. RootView starts a fresh conversation
    /// in the app's scope and dismisses this cover once that is accepted.
    var onNewAppSession: (String) -> Void = { _ in }
    /// Called with `appID` when a DRAFT card whose init-session pin never
    /// landed is tapped. Deliberately NOT `onNewAppSession`: this one carries
    /// the create kickoff, so the interview re-arms instead of the tap minting
    /// yet another silent empty session in the app's scope. See
    /// `RootView.startDraftAppInterview`.
    var onStartDraftInterview: (String) -> Void = { _ in }

    @State private var path: [LocalAppsRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            LocalAppsLibraryScreen(
                store: store,
                path: $path,
                onDismiss: onDismiss,
                onOpenAppSession: onOpenAppSession,
                onNewAppSession: onNewAppSession,
                onStartDraftInterview: onStartDraftInterview
            )
            .navigationDestination(for: LocalAppsRoute.self) { route in
                destination(route)
            }
        }
        // This cover is the ONLY consumer of the library's create-landing
        // fallback (`createdAppID`) — see `clearLibraryFallbackArm()`.
        // Dismissing it between the '+' tap and `AppCreated` must not leave
        // that claim armed for a much later, unrelated visit to walk into.
        .onDisappear { store.clearLibraryFallbackArm() }
        // Mirrors Android's collapse in `LocalAppsViewModel.kt`
        // (`appIdOnScreen()` against `liveIds`). `pendingDelete`'s
        // confirmation dialog is anchored on THIS screen (the list), not on
        // `.details`/`.preview`, so a delete never pops the stack on its
        // own — without this, deleting the app currently open in one of
        // those two routes left the stack pushed on a route for an app that
        // no longer exists.
        .onChange(of: store.apps.map(\.id)) { _, liveIDs in
            guard let onScreen = path.first?.appID, !liveIDs.contains(onScreen) else { return }
            path.removeAll()
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
                get: { store.pendingDependencyChangeConfirmation },
                set: { _ in }
            )
        ) { prompt in
            LocalAppDependencyChangeConfirmationSheet(store: store, prompt: prompt)
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
        case let .details(appID):
            LocalAppDetailView(
                store: store,
                appID: appID,
                path: $path,
                activeConversationID: activeConversationID,
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
                    if prompt.allowsPersistentGrant {
                        permissionButton(.session)
                        permissionButton(.always)
                    }
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

struct LocalAppDependencyChangeConfirmationSheet: View {
    @Bindable var store: LocalAppsStore
    let prompt: LocalAppDependencyChangeConfirmationPrompt

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    Label(
                        String(localized: "local_apps_dependency_change_title"),
                        systemImage: "shippingbox.and.arrow.backward.fill"
                    )
                    .font(.title3.bold())
                    Text(localizedDependencyConfirmationReason(prompt.reason))
                        .foregroundStyle(.secondary)

                    VStack(alignment: .leading, spacing: 12) {
                        ForEach(Array(prompt.changes.enumerated()), id: \.offset) { _, change in
                            DependencyChangeRow(change: change)
                        }
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        policyRow(
                            title: String(localized: "local_apps_dependency_change_license_risk"),
                            value: localizedDependencyRisk(prompt.licenseRisk)
                        )
                        policyRow(
                            title: String(localized: "local_apps_dependency_change_sbom_risk"),
                            value: localizedDependencyRisk(prompt.sbomRisk)
                        )
                        policyRow(
                            title: String(localized: "local_apps_dependency_change_scripts"),
                            value: prompt.lifecycleScriptsBlocked
                                ? String(localized: "local_apps_dependency_change_blocked")
                                : String(localized: "local_apps_dependency_change_allowed")
                        )
                        policyRow(
                            title: String(localized: "local_apps_dependency_change_native_addons"),
                            value: prompt.nativeAddonsBlocked
                                ? String(localized: "local_apps_dependency_change_blocked")
                                : String(localized: "local_apps_dependency_change_allowed")
                        )
                    }
                    .padding(14)
                    .background(.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 14))

                    Text(localizedDependencyRollbackPolicy(prompt.rollbackPolicy))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(24)
            }
            .navigationTitle("local_apps_dependency_change_navigation_title")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_cancel") {
                        resolve(false)
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("local_apps_dependency_change_approve") {
                        resolve(true)
                    }
                }
            }
        }
        .interactiveDismissDisabled()
        .presentationDetents([.large])
        .accessibilityIdentifier("local-apps.dependency-change.\(prompt.id)")
    }

    private func resolve(_ approved: Bool) {
        Task { await store.resolvePendingDependencyChangeConfirmation(approved) }
    }

    @ViewBuilder
    private func policyRow(title: String, value: String) -> some View {
        LabeledContent(title) {
            Text(value)
                .multilineTextAlignment(.trailing)
        }
    }
}

private struct DependencyChangeRow: View {
    let change: LocalAppDependencyChange

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(alignment: .firstTextBaseline) {
                Text(change.kind.title)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                Text(change.package)
                    .font(.headline)
                    .textSelection(.enabled)
                if let version = change.version {
                    Text("@\(version)")
                        .font(.system(.subheadline, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
            Text(
                String(
                    format: String(localized: "local_apps_dependency_change_cache_download"),
                    localizedDependencyStatus(change.cacheStatus),
                    localizedDependencyStatus(change.downloadStatus)
                )
            )
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
    }
}

private func localizedDependencyRisk(_ raw: String) -> String {
    switch raw {
    case "unknown_until_resolution":
        return String(localized: "local_apps_dependency_change_unknown_until_resolution")
    default:
        return raw
    }
}

private func localizedDependencyConfirmationReason(_ raw: String) -> String {
    switch raw {
    case "pre_resolution_no_network":
        return String(localized: "local_apps_dependency_change_pre_resolution_no_network")
    default:
        return raw
    }
}

private func localizedDependencyRollbackPolicy(_ raw: String) -> String {
    switch raw {
    case "rollback_on_validation_failure":
        return String(localized: "local_apps_dependency_change_rollback_on_failure")
    default:
        return raw
    }
}

private func localizedDependencyStatus(_ raw: String) -> String {
    switch raw {
    case "may_be_required":
        return String(localized: "local_apps_dependency_change_download_may_be_required")
    case "not_required":
        return String(localized: "local_apps_dependency_change_download_not_required")
    case "not_needed":
        return String(localized: "local_apps_dependency_change_cache_not_needed")
    default:
        return raw
    }
}

func localizedRuntimeProfileStatus(_ raw: String) -> String {
    switch raw {
    case "bundled":
        String(localized: "local_apps_runtime_profile_status_bundled")
    case "cached":
        String(localized: "local_apps_runtime_profile_status_cached")
    case "download_required":
        String(localized: "local_apps_runtime_profile_status_download_required")
    case "unavailable":
        String(localized: "local_apps_runtime_profile_status_unavailable")
    case "gated":
        String(localized: "local_apps_runtime_profile_status_gated")
    default:
        raw
    }
}

private struct LocalAppsLibraryScreen: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    @Binding var path: [LocalAppsRoute]
    let onDismiss: () -> Void
    let onOpenAppSession: (String, String, SessionMode) -> Void
    let onNewAppSession: (String) -> Void
    let onStartDraftInterview: (String) -> Void
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
                    Task { await createShellApp() }
                } label: {
                    Label("local_apps_create", systemImage: "plus")
                }
                .disabled(store.isCreateInFlight)
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
            "local_apps_delete_confirm \(pendingDelete?.displayName ?? String(localized: "local_apps_title"))",
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
            Button("local_apps_create") { Task { await createShellApp() } }
                .buttonStyle(.borderedProminent)
                .disabled(store.isCreateInFlight)
                .accessibilityIdentifier("local-apps.create.empty-state")
        }
    }

    /// Tapping a card.
    ///
    /// A SHELL has no detail page worth showing — no brief, no surface, no
    /// runtime — and the one thing the user wants from it is the conversation
    /// that is going to define it. So a draft card resumes the app's pinned
    /// session, or — if the engine's best-effort mint failed and there is no
    /// pin — starts a fresh conversation in the app's scope WITH the create
    /// kickoff, which is what re-arms the interview instead of dropping the
    /// user into an empty composer. A formed app opens its details as before.
    private func open(_ app: LocalAppSummary) {
        guard app.isDraftShell else {
            path.append(.details(app.id))
            return
        }
        if let sessionID = app.initSessionId {
            onOpenAppSession(app.id, sessionID, .code)
        } else {
            // No pin: the engine's best-effort init-session mint failed, so
            // there is nothing to resume. Re-arm the interview with the create
            // kickoff rather than minting a silent anchor — a shell with no
            // brief and no kickoff gives the agent nothing to act on, and
            // tapping the card again would just mint another one.
            onStartDraftInterview(app.id)
        }
    }

    /// The "+" button: create the empty shell and let the landing take the
    /// user into its conversation. There is no form to push any more.
    ///
    /// The button's `.disabled` above reads `store.isCreateInFlight`
    /// directly rather than a local flag around this `await`: that `await`
    /// only spans the round-trip to the engine, but the create itself
    /// resolves out of band on `AppCreated` / `AppOperationFailed` roughly
    /// 30s later, and `isCreateInFlight` stays true for that whole window —
    /// see its doc comment on `LocalAppsStore`.
    private func createShellApp() async {
        // `armLibraryFallback` spelled out rather than left to the default:
        // this screen IS the fallback landing's only consumer
        // (`openCreatedAppIfNeeded` below), and it is the only caller that can
        // truthfully claim the cover is mounted to consume it. The drawer's
        // create passes `false` for exactly that reason.
        _ = await store.createShellApp(armLibraryFallback: true)
    }

    private func openCreatedAppIfNeeded() {
        // The library's own landing. It only ever wins if the cover is still
        // up, which means `RootView` has not taken the user into the app's
        // conversation — the primary landing.
        guard store.pendingWidgetSetup == nil else { return }
        guard let appID = store.consumeCreatedAppID() else { return }
        // A SHELL is left entirely to the primary landing. This fallback runs
        // on `AppCreated`, which is one event too early: the pin is minted
        // afterwards, so opening the conversation from here would start a
        // FRESH one — without the kickoff, and orphaning the session the
        // engine is about to pin. `RootView.landCreatedAppIfReady` waits for
        // the record that carries the pin and sends the kickoff with it.
        // Consuming the id and doing nothing is what keeps this fallback from
        // racing it. A shell whose primary landing never fires is still one
        // tap away in the list.
        if store.app(id: appID)?.isDraftShell == true { return }
        path = [.details(appID)]
    }
}

struct LocalAppWidgetSetupSheet: View {
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
                    // `displayName`, never `name`: a shell's name is an engine
                    // placeholder the user never chose.
                    HStack(spacing: 8) {
                        Text(app.displayName)
                            .font(.headline)
                            .foregroundStyle(theme.text)
                            .lineLimit(1)
                        LocalAppPublicationBadgeView(badge: app.workflow.statusBadge)
                        if let uiVerification = app.uiVerification {
                            LocalAppPublicationBadgeView(badge: uiVerification.badge)
                        }
                        if let mcpVerification = app.mcpVerification {
                            LocalAppPublicationBadgeView(badge: mcpVerification.badge)
                        }
                    }
                    HStack(spacing: 6) {
                        Circle()
                            .fill(runtimeColor)
                            .frame(width: 7, height: 7)
                        // A shell has no workflow state or runtime worth
                        // naming — it is being created.
                        Text(app.draftStatusLine ?? "\(app.workflow.label) · \(runtime.label)")
                            .font(.caption)
                            .foregroundStyle(theme.text3)
                            .lineLimit(1)
                    }
                    if let profileStatus = app.runtimeProfileStatus {
                        Label(profileStatus.title, systemImage: profileStatus.systemImageName)
                            .font(.caption2.weight(.medium))
                            .foregroundStyle(runtimeProfileStatusColor(profileStatus))
                            .lineLimit(1)
                    }
                }
                Spacer()
                Menu {
                    // Nothing to start or stop until the scaffold lands; delete
                    // works exactly as before, so an abandoned shell can be
                    // thrown away.
                    if !app.isDraftShell {
                        if case .running = runtime {
                            Button("composer_stop", systemImage: "stop.fill", action: onStop)
                        } else {
                            Button("common_start", systemImage: "play.fill", action: onStart)
                        }
                    }
                    Button("common_delete", systemImage: "trash", role: .destructive, action: onDelete)
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .frame(width: 36, height: 36)
                }
                .accessibilityLabel("local_apps_row_actions \(app.displayName)")
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

private func runtimeProfileStatusColor(_ status: LocalAppRuntimeProfileStatus) -> Color {
    switch status {
    case .verified: .green
    case .dependenciesDirty, .migrationAvailable, .rebuildRequired: .orange
    case .coreDependencyDrift, .runtimeBundleMissing, .runtimeContractCorrupt: .red
    }
}

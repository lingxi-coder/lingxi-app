import SwiftUI

enum LocalAppsRoute: Hashable {
    case templates
    case designer(String)
    case details(String)
    case preview(String)
}

struct LocalAppsRootView: View {
    @Bindable var store: LocalAppsStore
    let initialAppID: String?
    let onDismiss: () -> Void

    @State private var path: [LocalAppsRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            LocalAppsLibraryScreen(store: store, path: $path, onDismiss: onDismiss)
                .navigationDestination(for: LocalAppsRoute.self) { route in
                    destination(route)
                }
        }
        .task {
            await store.refresh()
            if let initialAppID {
                path = [store.hasPendingUIRequest(appID: initialAppID) ? .preview(initialAppID) : .details(initialAppID)]
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
    }

    @ViewBuilder
    private func destination(_ route: LocalAppsRoute) -> some View {
        switch route {
        case .templates:
            LocalAppTemplatePickerView(store: store, path: $path)
        case let .designer(appID):
            LocalAppDesignerView(store: store, appID: appID, path: $path)
        case let .details(appID):
            LocalAppDetailView(store: store, appID: appID, path: $path)
        case let .preview(appID):
            LocalAppPreviewView(store: store, appID: appID)
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
                Menu {
                    Button("local_apps_templates_all") { store.templateFilter = nil }
                    ForEach(LocalAppTemplateKind.allCases, id: \.rawValue) { kind in
                        Button(templateFilterLabel(kind)) { store.templateFilter = kind }
                    }
                } label: {
                    Label("local_apps_filter", systemImage: "line.3.horizontal.decrease.circle")
                }
                Button {
                    path.append(.templates)
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
        .onChange(of: store.createdAppIDForDesigner) { _, _ in openCreatedAppIfNeeded() }
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
            Button("local_apps_templates_title") { path.append(.templates) }
                .buttonStyle(.borderedProminent)
        }
    }

    private func open(_ app: LocalAppSummary) {
        switch app.workflow {
        case .collectingSpec, .awaitingSpecConfirmation:
            path.append(.designer(app.id))
        case .awaitingPreviewConfirmation:
            path.append(.preview(app.id))
        default:
            path.append(.details(app.id))
        }
    }

    private func openCreatedAppIfNeeded() {
        guard let appID = store.consumeCreatedAppID() else { return }
        path = [.designer(appID)]
    }

    private func templateFilterLabel(_ kind: LocalAppTemplateKind) -> String {
        store.templates.first(where: { $0.kind == kind })?.name ?? kind.rawValue
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
                Image(systemName: app.templateKind.systemImage)
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

struct LocalAppTemplatePickerView: View {
    @Environment(\.theme) private var theme
    @Bindable var store: LocalAppsStore
    @Binding var path: [LocalAppsRoute]

    @State private var selectedTemplate: LocalAppTemplate?

    var body: some View {
        Group {
            if store.templates.isEmpty {
                ContentUnavailableView {
                    Label("local_apps_templates_loading", systemImage: "square.stack.3d.up")
                } description: {
                    Text("local_apps_templates_loading_detail")
                } actions: {
                    Button("common_retry") { Task { await store.refresh() } }
                }
            } else {
                ScrollView {
                    LazyVStack(spacing: 12) {
                        ForEach(store.templates) { template in
                            Button {
                                selectedTemplate = template
                            } label: {
                                HStack(spacing: 14) {
                                    Image(systemName: template.kind.systemImage)
                                        .font(.title2)
                                        .foregroundStyle(theme.accent)
                                        .frame(width: 48, height: 48)
                                        .background(theme.accent.opacity(0.12), in: .rect(cornerRadius: 13))
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(template.name)
                                            .font(.headline)
                                            .foregroundStyle(theme.text)
                                        Text(template.description)
                                            .font(.subheadline)
                                            .foregroundStyle(theme.text3)
                                            .multilineTextAlignment(.leading)
                                        Text("local_apps_template_steps \(template.orderedSteps.count) \(template.version)")
                                            .font(.caption)
                                            .foregroundStyle(theme.text4)
                                    }
                                    Spacer()
                                    Image(systemName: "chevron.right")
                                        .foregroundStyle(theme.text4)
                                }
                                .padding(14)
                                .background(theme.surface, in: .rect(cornerRadius: 16))
                                .overlay {
                                    RoundedRectangle(cornerRadius: 16)
                                        .stroke(theme.border, lineWidth: 0.5)
                                }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding()
                }
                .background(theme.windowBg)
            }
        }
        .navigationTitle("local_apps_templates_title")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $selectedTemplate) { template in
            LocalAppCreateSheet(store: store, template: template, path: $path)
        }
    }
}

private struct LocalAppCreateSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var store: LocalAppsStore
    let template: LocalAppTemplate
    @Binding var path: [LocalAppsRoute]

    @State private var name = ""
    @State private var creating = false

    var body: some View {
        NavigationStack {
            Form {
                Section("local_apps_title") {
                    TextField("local_apps_name", text: $name)
                        .textInputAutocapitalization(.never)
                    LabeledContent("local_apps_template", value: template.name)
                    Text(template.description)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section {
                    Text("local_apps_create_detail")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .navigationTitle("local_apps_create")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_cancel", action: dismiss.callAsFunction)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(creating ? "local_apps_creating" : "local_apps_create") {
                        Task { await create() }
                    }
                    .disabled(creating || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }

    private func create() async {
        creating = true
        let succeeded = await store.createApp(name: name, template: template)
        creating = false
        if succeeded {
            dismiss()
            if !path.isEmpty { path.removeLast() }
        }
    }
}

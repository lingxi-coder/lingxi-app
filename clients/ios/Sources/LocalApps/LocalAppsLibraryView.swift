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
            "应用错误",
            isPresented: Binding(
                get: { store.errorMessage != nil },
                set: { if !$0 { store.clearError() } }
            )
        ) {
            Button("好", role: .cancel, action: store.clearError)
        } message: {
            Text(store.errorMessage ?? "未知错误")
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
                    LabeledContent("域名") {
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
            .navigationTitle("应用授权")
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
        .searchable(text: $store.searchQuery, prompt: "搜索应用或状态")
        .navigationTitle("应用")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("关闭", action: onDismiss)
            }
            ToolbarItemGroup(placement: .primaryAction) {
                Menu {
                    Button("全部模板") { store.templateFilter = nil }
                    ForEach(LocalAppTemplateKind.allCases, id: \.rawValue) { kind in
                        Button(templateFilterLabel(kind)) { store.templateFilter = kind }
                    }
                } label: {
                    Label("筛选", systemImage: "line.3.horizontal.decrease.circle")
                }
                Button {
                    path.append(.templates)
                } label: {
                    Label("创建应用", systemImage: "plus")
                }
                .accessibilityIdentifier("local-apps.create")
            }
        }
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 8) {
                Image(systemName: store.distributionMode == .full ? "server.rack" : "doc.badge.gearshape")
                Text("\(store.distributionMode.runtimeLabel) · 依赖由 Lingxi 固定管理")
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
            "删除 \(pendingDelete?.name ?? "应用")？",
            isPresented: Binding(
                get: { pendingDelete != nil },
                set: { if !$0 { pendingDelete = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("删除应用", role: .destructive) {
                guard let app = pendingDelete else { return }
                pendingDelete = nil
                Task { _ = await store.delete(appID: app.id) }
            }
            Button("取消", role: .cancel) { pendingDelete = nil }
        } message: {
            Text("代码、构建产物和该应用的本地数据将被删除。")
        }
    }

    private var emptyState: some View {
        ContentUnavailableView {
            Label("还没有本地应用", systemImage: "square.grid.2x2")
        } description: {
            Text(store.isRefreshing ? "正在读取应用库…" : "从模板开始设计，Agent 会在确认后生成应用。")
        } actions: {
            Button("选择模板") { path.append(.templates) }
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
                        Button("停止", systemImage: "stop.fill", action: onStop)
                    } else {
                        Button("启动", systemImage: "play.fill", action: onStart)
                    }
                    Button("删除", systemImage: "trash", role: .destructive, action: onDelete)
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .frame(width: 36, height: 36)
                }
                .accessibilityLabel("\(app.name) 操作")
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
                    Label("正在读取模板", systemImage: "square.stack.3d.up")
                } description: {
                    Text("模板结构由 Rust AppService 下发，客户端不会使用内置字段副本。")
                } actions: {
                    Button("重试") { Task { await store.refresh() } }
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
                                        Text("\(template.orderedSteps.count) 个设计步骤 · v\(template.version)")
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
        .navigationTitle("选择模板")
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
                Section("应用") {
                    TextField("应用名称", text: $name)
                        .textInputAutocapitalization(.never)
                    LabeledContent("模板", value: template.name)
                    Text(template.description)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section {
                    Text("创建后进入五步设计向导。生成只有在你确认设计后才会开始。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .navigationTitle("创建应用")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消", action: dismiss.callAsFunction)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(creating ? "创建中…" : "创建") {
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

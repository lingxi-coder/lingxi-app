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
        case .overview: "概览"
        case .preview: "预览"
        case .data: "数据"
        case .code: "代码"
        case .history: "历史"
        case .permissions: "权限"
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
            .navigationTitle(app?.name ?? "应用详情")
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
                            Button("停止", systemImage: "stop.fill") {
                                Task { await store.stop(appID: appID) }
                            }
                        } else {
                            Button("启动", systemImage: "play.fill") {
                                Task { await store.start(appID: appID) }
                            }
                            .disabled(app.workflow != .ready && app.workflow != .awaitingPreviewConfirmation)
                        }
                        Menu {
                            if app.workflow == .generationFailed || app.workflow == .validationFailed {
                                Button("重试生成", systemImage: "arrow.trianglehead.2.clockwise.rotate.90") {
                                    Task { await store.retryGeneration(appID: appID) }
                                }
                            }
                            Button("重新启动", systemImage: "arrow.clockwise") {
                                Task { await store.restart(appID: appID) }
                            }
                            Button("继续设计", systemImage: "slider.horizontal.3") {
                                path.append(.designer(appID))
                            }
                        } label: {
                            Label("更多", systemImage: "ellipsis.circle")
                        }
                    }
                }
            } else {
                ContentUnavailableView("应用不存在", systemImage: "questionmark.app")
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
                    LabeledContent("运行模式", value: distribution.runtimeLabel)
                    LabeledContent("工作区", value: app.workspaceRelativePath)
                    LabeledContent("模板", value: app.templateKind.rawValue)
                    LabeledContent("更新时间") {
                        Text(app.updatedAt, format: .relative(presentation: .named))
                    }
                }
                .padding()
                .background(theme.surface, in: .rect(cornerRadius: 16))

                Button("打开预览", systemImage: "safari", action: onOpenPreview)
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
                Label("预览尚未运行", systemImage: "safari")
            } description: {
                Text("启动应用后，页面将从 Rust 回环服务加载。")
            } actions: {
                Button("启动") { Task { await store.start(appID: appID) } }
            }
        }
    }
}

private struct LocalAppDataSection: View {
    let template: LocalAppTemplate?

    var body: some View {
        List {
            Section {
                Label("SQLite 由 Rust AppService 独占管理", systemImage: "lock.shield")
                Text("页面和 Agent 只能通过受控 collection API 查询和修改，不接受原始 SQL。")
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
            Section("工作区") {
                Text(app.workspaceRelativePath)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
            }
            Section("源码") {
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
                Text("可查看和编辑完整文本源码；符号链接、构建产物和工作区外路径不会显示。修改固定依赖可以保存，但构建验证会拒绝不受支持的版本或安装操作。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .overlay {
            if browser.isLoading {
                ProgressView("正在读取源码…")
            } else if browser.files.isEmpty {
                ContentUnavailableView("暂无可编辑源码", systemImage: "doc.text.magnifyingglass")
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
                .navigationTitle(browser.selectedPath ?? "源码")
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
                        Button("关闭") { browser.closeEditor() }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("保存") { browser.save() }
                            .disabled(browser.isSaving)
                    }
                }
            }
        }
        .alert(
            "源码错误",
            isPresented: Binding(
                get: { browser.errorMessage != nil },
                set: { if !$0 { browser.clearError() } }
            )
        ) {
            Button("好", role: .cancel, action: browser.clearError)
        } message: {
            Text(browser.errorMessage ?? "未知错误")
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
                ContentUnavailableView("暂无检查点", systemImage: "clock.arrow.circlepath")
            }
        }
        .confirmationDialog(
            "恢复代码检查点？",
            isPresented: Binding(
                get: { pendingRestore != nil },
                set: { if !$0 { pendingRestore = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("恢复代码", role: .destructive) {
                guard let checkpoint = pendingRestore else { return }
                pendingRestore = nil
                Task { _ = await store.restore(appID: appID, checkpointID: checkpoint.id) }
            }
            Button("取消", role: .cancel) { pendingRestore = nil }
        } message: {
            Text("恢复前会自动创建 pre_restore 检查点。应用数据库不会回滚。")
        }
    }
}

private struct LocalAppPermissionsSection: View {
    @Bindable var store: LocalAppsStore
    let appID: String
    @State private var confirmReset = false

    var body: some View {
        List {
            Section("Agent 控制") {
                Label("读取数据与检查 UI 默认允许", systemImage: "eye")
                Label("修改数据或控制 UI 首次需要授权", systemImage: "hand.raised")
                Button("撤销全部授权", role: .destructive) { confirmReset = true }
            }
            Section("网络") {
                Label("默认禁止外网", systemImage: "network.slash")
                Text("仅可访问设计清单声明的 HTTPS 域名，每个域名首次访问单独授权。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            Section("应用标识") {
                Text(appID).textSelection(.enabled)
            }
        }
        .confirmationDialog(
            "撤销该应用的全部授权？",
            isPresented: $confirmReset,
            titleVisibility: .visible
        ) {
            Button("撤销会话与持久授权", role: .destructive) {
                Task { _ = await store.resetPermissions(appID: appID) }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("下次修改数据、控制界面或访问域名时会重新询问。")
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
                            Button("反馈修改") { showFeedback = true }
                                .buttonStyle(.bordered)
                            Spacer()
                            Button("批准预览") {
                                Task { _ = await store.approvePreview(appID: appID) }
                            }
                            .buttonStyle(.borderedProminent)
                        }
                        .padding()
                        .background(.bar)
                    }
                }
            } else {
                ContentUnavailableView("预览尚未准备好", systemImage: "hourglass")
            }
        }
        .navigationTitle("预览确认")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $showFeedback) {
            NavigationStack {
                Form {
                    TextEditor(text: $feedback)
                        .frame(minHeight: 140)
                }
                .navigationTitle("反馈修改")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("取消") { showFeedback = false }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("提交") {
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

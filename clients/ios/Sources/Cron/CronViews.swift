import SwiftUI

struct CronRootView: View {
    @State private var repository: CronRepository
    let initialRoute: CronRoute?
    let onDismiss: (() -> Void)?

    init(
        repository: CronRepository,
        initialRoute: CronRoute? = nil,
        onDismiss: (() -> Void)? = nil
    ) {
        _repository = State(initialValue: repository)
        self.initialRoute = initialRoute
        self.onDismiss = onDismiss
    }

    var body: some View {
        @Bindable var repository = repository
        NavigationStack(path: $repository.routePath) {
            CronListView(repository: repository)
                .navigationTitle("定时任务")
                .navigationDestination(for: CronRoute.self) { route in
                    switch route {
                    case .list:
                        CronListView(repository: repository)
                    case .task(let scopeID, _):
                        CronTaskEditorView(repository: repository, scopeID: scopeID)
                    case .run(let runID):
                        CronRunDetailView(repository: repository, runID: runID)
                    }
                }
                .toolbar {
                    if let onDismiss {
                        ToolbarItem(placement: .topBarLeading) {
                            Button("关闭", action: onDismiss)
                        }
                    }
                }
        }
        .task {
            await repository.handleLaunch()
            repository.resetRoutes()
            switch initialRoute {
            case .task(let scopeID, let taskID):
                if let taskID {
                    repository.beginEdit(scopeID: scopeID, taskID: taskID)
                } else {
                    repository.beginCreate(scopeID: scopeID)
                }
            case .run(let runID):
                repository.openRun(runID)
            case .list, .none:
                break
            }
        }
        .accessibilityIdentifier("cron.root")
    }
}

private struct CronListView: View {
    @Bindable var repository: CronRepository

    var body: some View {
        List {
            schedulingSection
            tasksSection
            diagnosticsSection
            resultCategoriesSection
            messageSection
            errorSection
            historySection
        }
        .overlay {
            if repository.state.loading {
                ProgressView("巡检中…")
            }
        }
        .refreshable {
            await repository.refresh()
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("立即巡检") {
                    Task { await repository.reconcile(reason: "manual-refresh") }
                }
            }
        }
    }

    private var schedulingSection: some View {
        Section("调度") {
            LabeledContent("模式", value: repository.state.diagnostics.schedulingModeTitle)
            LabeledContent("任务标识", value: repository.state.diagnostics.backgroundTaskIdentifier)
            LabeledContent("下一次最早执行", value: repository.state.diagnostics.nextEarliestRunText)
            LabeledContent("最近巡检", value: repository.state.diagnostics.lastReconciledText)
            Text(repository.state.diagnostics.schedulingNote)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private var diagnosticsSection: some View {
        Section("诊断") {
            LabeledContent("当前作用域", value: repository.state.diagnostics.activeScopeName)
            LabeledContent("作用域 ID", value: repository.state.diagnostics.activeScopeID)
            LabeledContent("任务总数", value: "\(repository.state.diagnostics.taskCount)")
            LabeledContent("历史总数", value: "\(repository.state.diagnostics.historyCount)")
            LabeledContent("活动运行", value: "\(repository.state.diagnostics.activeRunCount)")
            Text(repository.state.diagnostics.activeRunSummary)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var resultCategoriesSection: some View {
        if !repository.state.diagnostics.resultCategories.isEmpty {
            Section("最近结果分类") {
                ForEach(repository.state.diagnostics.resultCategories) { summary in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(summary.title)
                            .foregroundStyle(.primary)
                        Text(summary.detail)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var messageSection: some View {
        if let message = repository.state.lastActionMessage, !message.isEmpty {
            Section {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var errorSection: some View {
        if let error = repository.state.errorMessage, !error.isEmpty {
            Section {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.red)
            }
        }
    }

    private var tasksSection: some View {
        Section("任务") {
            Button("新建定时任务") {
                repository.beginCreate()
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("cron.add")
            ForEach(displayScopes) { scope in
                if let tasks = groupedTasks[scope.scopeID], !tasks.isEmpty {
                    NavigationLink(value: CronRoute.task(scopeID: scope.scopeID, taskID: nil)) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(scope.projectName)
                            Text("\(tasks.count) 个任务")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                    }
                    ForEach(tasks) { scoped in
                        taskRow(scoped, scopeID: scope.scopeID)
                    }
                }
            }
        }
    }

    private var historySection: some View {
        Section("运行历史") {
            if repository.state.history.isEmpty {
                Text("暂无运行记录。系统调度不是精确闹钟，但始终可以立即运行。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(repository.state.history.prefix(50)) { run in
                    Button {
                        repository.openRun(run.runID)
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("\(run.projectName) · \(run.prompt.firstCronLine ?? run.taskID)")
                                .foregroundStyle(.primary)
                            Text("\(run.resultCategory.label) · \(formatCronEpoch(run.scheduledAtMs))")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    private var displayScopes: [CronScope] {
        repository.state.scopes
    }

    private var groupedTasks: [String: [CronScopedTask]] {
        Dictionary(grouping: repository.state.tasks, by: { $0.scope.scopeID })
    }

    @ViewBuilder
    private func taskRow(_ scoped: CronScopedTask, scopeID: String) -> some View {
        Button {
            repository.beginEdit(scopeID: scopeID, taskID: scoped.task.id)
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Text(scoped.task.prompt.firstCronLine ?? scoped.task.id)
                    .foregroundStyle(.primary)
                Text(scoped.task.human + (scoped.task.recurring ? "" : " · 仅一次"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Text(runSummary(for: scoped))
                    .font(.caption)
                    .foregroundStyle(scoped.activeRun == nil ? Color.secondary : Color.orange)
            }
        }
        .buttonStyle(.plain)
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button("立即运行") {
                Task { await repository.runNow(scopeID: scopeID, taskID: scoped.task.id) }
            }
            .tint(.blue)
            Button(role: .destructive) {
                Task { await repository.deleteTask(scopeID: scopeID, taskID: scoped.task.id) }
            } label: {
                Text("删除")
            }
        }
    }

    private func runSummary(for task: CronScopedTask) -> String {
        if let active = task.activeRun {
            return "\(active.status.label) · 下次 \(task.task.nextFireMs.map(formatCronEpoch) ?? "待定")"
        }
        if let last = task.lastRun {
            return "最近：\(last.resultCategory.label) · 下次 \(task.task.nextFireMs.map(formatCronEpoch) ?? "待定")"
        }
        return "下次 \(task.task.nextFireMs.map(formatCronEpoch) ?? "待定")"
    }
}

private struct CronTaskEditorView: View {
    @Bindable var repository: CronRepository
    let scopeID: String

    var body: some View {
        Form {
            Section {
                Picker("作用域", selection: $repository.draft.scopeID) {
                    ForEach(repository.state.scopes) { scope in
                        Text(scope.projectName).tag(scope.scopeID)
                    }
                }
                .disabled(repository.draft.isEditing)
                TextField("提示词 / 任务", text: $repository.draft.prompt, axis: .vertical)
                    .lineLimit(3, reservesSpace: true)
                TextField("Cron（分 时 日 月 周）", text: $repository.draft.cron)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Toggle("重复执行", isOn: $repository.draft.recurring)
                Text("iOS 只提供“最早不早于”的系统调度，不保证准点启动；也可随时立即运行。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            if let error = repository.state.errorMessage, !error.isEmpty {
                Section {
                    Text(error)
                        .foregroundStyle(.red)
                }
            }
            Section {
                Button(repository.draft.isEditing ? "保存" : "创建并调度") {
                    Task { await repository.saveDraft() }
                }
                .disabled(repository.draft.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ||
                    repository.draft.cron.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                if let taskID = repository.draft.taskID {
                    Button("立即运行") {
                        Task { await repository.runNow(scopeID: repository.draft.scopeID, taskID: taskID) }
                    }
                    Button("删除", role: .destructive) {
                        Task { await repository.deleteTask(scopeID: repository.draft.scopeID, taskID: taskID) }
                    }
                }
            }
        }
        .navigationTitle(repository.draft.isEditing ? "编辑任务" : "新建任务")
        .onAppear {
            if repository.draft.scopeID.isEmpty {
                repository.draft.scopeID = scopeID
            }
        }
    }
}

private struct CronRunDetailView: View {
    @Bindable var repository: CronRepository
    let runID: String

    var body: some View {
        Form {
            if let run = repository.state.history.first(where: { $0.runID == runID }) {
                Section("概览") {
                    LabeledContent("项目", value: run.projectName)
                    LabeledContent("状态", value: run.resultCategory.label)
                    LabeledContent("计划执行", value: formatCronEpoch(run.scheduledAtMs))
                    LabeledContent("触发时间", value: formatCronEpoch(run.triggeredAtMs))
                    if let startedAt = run.startedAtMs {
                        LabeledContent("开始时间", value: formatCronEpoch(startedAt))
                    }
                    if let finishedAt = run.finishedAtMs {
                        LabeledContent("结束时间", value: formatCronEpoch(finishedAt))
                    }
                    LabeledContent("运行 ID", value: run.runID)
                }
                Section("任务") {
                    Text(run.prompt)
                        .textSelection(.enabled)
                }
                if let result = run.resultText, !result.isEmpty {
                    Section("结果") {
                        Text(result)
                            .textSelection(.enabled)
                    }
                }
                if let error = run.errorMessage, !error.isEmpty {
                    Section("错误") {
                        Text(run.errorKind.map { "\($0.label)：\(error)" } ?? error)
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                    }
                }
            } else {
                Text("未找到该次运行记录。")
                    .foregroundStyle(.secondary)
            }
        }
        .navigationTitle("运行详情")
    }
}

private extension String {
    var firstCronLine: String? {
        split(whereSeparator: \.isNewline).first.map(String.init)
    }
}

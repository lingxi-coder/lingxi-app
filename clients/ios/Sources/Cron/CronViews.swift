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
                .navigationTitle("settings_title_cron")
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
                            Button("common_close", action: onDismiss)
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
                ProgressView("cron_reconciling_progress")
            }
        }
        .refreshable {
            await repository.refresh()
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("cron_reconcile_now_button") {
                    Task { await repository.reconcile(reason: "manual-refresh") }
                }
            }
        }
    }

    private var schedulingSection: some View {
        Section("cron_section_scheduling") {
            LabeledContent("cron_mode_label", value: repository.state.diagnostics.schedulingModeTitle)
            LabeledContent("cron_task_identifier_label", value: repository.state.diagnostics.backgroundTaskIdentifier)
            LabeledContent("cron_next_earliest_run_label", value: repository.state.diagnostics.nextEarliestRunText)
            LabeledContent("cron_last_reconciled_label", value: repository.state.diagnostics.lastReconciledText)
            Text(repository.state.diagnostics.schedulingNote)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private var diagnosticsSection: some View {
        Section("settings_linux_diagnose") {
            LabeledContent("cron_active_scope_label", value: repository.state.diagnostics.activeScopeName)
            LabeledContent("cron_scope_id_label", value: repository.state.diagnostics.activeScopeID)
            LabeledContent("cron_task_count_label", value: "\(repository.state.diagnostics.taskCount)")
            LabeledContent("cron_history_count_label", value: "\(repository.state.diagnostics.historyCount)")
            LabeledContent("cron_active_runs_label", value: "\(repository.state.diagnostics.activeRunCount)")
            Text(repository.state.diagnostics.activeRunSummary)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var resultCategoriesSection: some View {
        if !repository.state.diagnostics.resultCategories.isEmpty {
            Section("cron_recent_result_categories_section") {
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
        Section("settings_linux_section_tasks") {
            Button("cron_new_task_button") {
                repository.beginCreate()
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("cron.add")
            ForEach(displayScopes) { scope in
                if let tasks = groupedTasks[scope.scopeID], !tasks.isEmpty {
                    NavigationLink(value: CronRoute.task(scopeID: scope.scopeID, taskID: nil)) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(scope.projectName)
                            Text("cron_scope_task_count \(tasks.count)")
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
        Section("cron_run_history_section") {
            if repository.state.history.isEmpty {
                Text("cron_no_run_history")
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
                Text(scoped.task.human + (scoped.task.recurring ? "" : " · " + String(localized: "cron_once_only")))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Text(runSummary(for: scoped))
                    .font(.caption)
                    .foregroundStyle(scoped.activeRun == nil ? Color.secondary : Color.orange)
            }
        }
        .buttonStyle(.plain)
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button("cron_run_now_button") {
                Task { await repository.runNow(scopeID: scopeID, taskID: scoped.task.id) }
            }
            .tint(.blue)
            Button(role: .destructive) {
                Task { await repository.deleteTask(scopeID: scopeID, taskID: scoped.task.id) }
            } label: {
                Text("common_delete")
            }
        }
    }

    private func runSummary(for task: CronScopedTask) -> String {
        let nextText = task.task.nextFireMs.map(formatCronEpoch) ?? String(localized: "cron_pending")
        if let active = task.activeRun {
            return String(localized: "cron_run_summary_active \(active.status.label) \(nextText)")
        }
        if let last = task.lastRun {
            return String(localized: "cron_run_summary_last \(last.resultCategory.label) \(nextText)")
        }
        return String(localized: "cron_run_summary_next \(nextText)")
    }
}

private struct CronTaskEditorView: View {
    @Bindable var repository: CronRepository
    let scopeID: String

    var body: some View {
        Form {
            Section {
                Picker("cron_scope_label", selection: $repository.draft.scopeID) {
                    ForEach(repository.state.scopes) { scope in
                        Text(scope.projectName).tag(scope.scopeID)
                    }
                }
                .disabled(repository.draft.isEditing)
                TextField("cron_prompt_field_label", text: $repository.draft.prompt, axis: .vertical)
                    .lineLimit(3, reservesSpace: true)
                TextField("cron_expression_field_label", text: $repository.draft.cron)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Toggle("cron_recurring_toggle", isOn: $repository.draft.recurring)
                Text("cron_ios_scheduling_note")
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
                Button(repository.draft.isEditing ? "voice_save_button" : "cron_create_and_schedule_button") {
                    Task { await repository.saveDraft() }
                }
                .disabled(repository.draft.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ||
                    repository.draft.cron.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                if let taskID = repository.draft.taskID {
                    Button("cron_run_now_button") {
                        Task { await repository.runNow(scopeID: repository.draft.scopeID, taskID: taskID) }
                    }
                    Button("common_delete", role: .destructive) {
                        Task { await repository.deleteTask(scopeID: repository.draft.scopeID, taskID: taskID) }
                    }
                }
            }
        }
        .navigationTitle(repository.draft.isEditing ? "cron_edit_task_title" : "cron_new_task_title")
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
                Section("local_apps_section_overview") {
                    LabeledContent("cron_run_project_label", value: run.projectName)
                    LabeledContent("settings_linux_section_status", value: run.resultCategory.label)
                    LabeledContent("cron_scheduled_at_label", value: formatCronEpoch(run.scheduledAtMs))
                    LabeledContent("cron_triggered_at_label", value: formatCronEpoch(run.triggeredAtMs))
                    if let startedAt = run.startedAtMs {
                        LabeledContent("cron_started_at_label", value: formatCronEpoch(startedAt))
                    }
                    if let finishedAt = run.finishedAtMs {
                        LabeledContent("cron_finished_at_label", value: formatCronEpoch(finishedAt))
                    }
                    LabeledContent("cron_run_id_label", value: run.runID)
                }
                Section("settings_linux_section_tasks") {
                    Text(run.prompt)
                        .textSelection(.enabled)
                }
                if let result = run.resultText, !result.isEmpty {
                    Section("cron_result_section") {
                        Text(result)
                            .textSelection(.enabled)
                    }
                }
                if let error = run.errorMessage, !error.isEmpty {
                    Section("cron_error_section") {
                        Text(run.errorKind.map { "\($0.label)：\(error)" } ?? error)
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                    }
                }
            } else {
                Text("cron_run_not_found")
                    .foregroundStyle(.secondary)
            }
        }
        .navigationTitle("cron_run_detail_title")
    }
}

private extension String {
    var firstCronLine: String? {
        split(whereSeparator: \.isNewline).first.map(String.init)
    }
}

import SwiftUI

struct CronRootView: View {
    @State private var repository: CronRepository
    let initialRoute: CronRoute?
    let onDismiss: (() -> Void)?
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var closeAlert = false

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
        Group {
            if sizeClass == .regular {
                NavigationSplitView {
                    taskList
                        .navigationTitle("Scheduled")
                        .navigationSplitViewColumnWidth(min: 320, ideal: 420, max: 550)
                } detail: {
                    NavigationStack {
                        if let route = repository.routePath.last { destination(route) }
                        else { ContentUnavailableView("Scheduled tasks", systemImage: "clock", description: Text("Select a task or create a new one.")) }
                    }
                }
            } else {
                NavigationStack(path: $repository.routePath) {
                    taskList
                        .navigationTitle("Scheduled")
                        .navigationDestination(for: CronRoute.self) { destination($0) }
                }
            }
        }
        .task {
            repository.resetRoutes()
            await repository.refresh()
            guard repository.routePath.isEmpty else { return }
            switch initialRoute {
            case .task(let scopeID, let taskID):
                if let taskID { repository.beginEdit(scopeID: scopeID, taskID: taskID) }
                else { repository.beginCreate(scopeID: scopeID) }
            case .run(let runID): repository.openRun(runID)
            case .list, .none: break
            }
        }
        .interactiveDismissDisabled(repository.hasUnsavedChanges)
        .alert("Discard unsaved changes?", isPresented: $closeAlert) {
            Button("Discard", role: .destructive) { repository.discardDraft(); onDismiss?() }
            Button("Keep editing", role: .cancel) {}
        }
        .accessibilityIdentifier("cron.root")
    }

    private var taskList: some View {
        CronListView(repository: repository)
            .toolbar {
                if let onDismiss {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Close") {
                            if repository.hasUnsavedChanges { closeAlert = true } else { onDismiss() }
                        }
                    }
                }
            }
    }

    @ViewBuilder
    private func destination(_ route: CronRoute) -> some View {
        switch route {
        case .list: CronListView(repository: repository)
        case .task(let scopeID, _): CronTaskEditorView(repository: repository, scopeID: scopeID)
        case .run(let runID): CronRunDetailView(repository: repository, runID: runID)
        }
    }
}

private struct CronListView: View {
    @Bindable var repository: CronRepository
    @State private var filter = "all"
    @State private var query = ""
    @State private var pendingRoute: CronRoute?
    @State private var discardAlert = false

    private var visibleTasks: [CronScopedTask] {
        repository.state.tasks.filter {
            (filter == "all" || $0.task.status.rawValue == filter) &&
            (query.isEmpty || ($0.task.configuration.name ?? "").localizedCaseInsensitiveContains(query) ||
                $0.task.prompt.localizedCaseInsensitiveContains(query) || $0.scope.projectName.localizedCaseInsensitiveContains(query))
        }
    }

    var body: some View {
        List {
            Section {
                Picker("Status", selection: $filter) {
                    Text("All").tag("all")
                    ForEach(CronTaskStatus.allCases, id: \.self) { Text($0.label).tag($0.rawValue) }
                }
                .pickerStyle(.segmented)
                .listRowSeparator(.hidden)
            }
            Section {
                ForEach(visibleTasks) { scoped in
                    Button {
                        select(.task(scopeID: scoped.scope.scopeID, taskID: scoped.task.id))
                    } label: {
                        HStack(alignment: .top, spacing: 12) {
                            Image(systemName: scoped.task.status == .completed ? "checkmark.circle" : scoped.task.status == .paused ? "pause.circle" : "circle")
                                .foregroundStyle(scoped.task.status == .active ? Color.accentColor : Color.secondary)
                            VStack(alignment: .leading, spacing: 6) {
                                Text(scoped.task.configuration.name ?? scoped.task.prompt.firstCronLine ?? scoped.task.id)
                                    .foregroundStyle(.primary).lineLimit(2)
                                Text("\(scoped.task.human) · \(scoped.scope.projectName)").font(.subheadline).foregroundStyle(.secondary)
                                if let reason = scoped.task.configuration.statusReason ?? scoped.task.unsupportedReason {
                                    Text(reason).font(.caption).foregroundStyle(.orange)
                                } else if let next = scoped.task.nextFireMs, scoped.task.status == .active {
                                    Text("Next run: \(formatCronEpoch(next))").font(.caption).foregroundStyle(.secondary)
                                }
                                if let run = scoped.activeRun ?? scoped.lastRun {
                                    Text(run.status.label).font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                        .padding(.vertical, 8)
                    }
                    .swipeActions(allowsFullSwipe: false) {
                        if scoped.task.status != .completed {
                            Button(scoped.task.status == .active ? "Pause" : "Resume") {
                                Task { await repository.setStatus(scopeID: scoped.scope.scopeID, taskID: scoped.task.id, status: scoped.task.status == .active ? .paused : .active) }
                            }.tint(.orange)
                        }
                        Button("Delete", role: .destructive) {
                            Task { await repository.deleteTask(scopeID: scoped.scope.scopeID, taskID: scoped.task.id) }
                        }
                    }
                }
                if visibleTasks.isEmpty {
                    if query.isEmpty {
                        ContentUnavailableView("No scheduled tasks", systemImage: "clock", description: Text("Create a task to run saved instructions on a schedule."))
                    } else { ContentUnavailableView.search(text: query) }
                }
            }
            Section {
                Text(repository.state.scheduling.note).font(.footnote).foregroundStyle(.secondary)
                if let error = repository.state.errorMessage { Text(error).foregroundStyle(.red) }
            }
        }
        .alert("Discard unsaved changes?", isPresented: $discardAlert) {
            Button("Discard", role: .destructive) { if let route = pendingRoute { open(route) }; pendingRoute = nil }
            Button("Keep editing", role: .cancel) { pendingRoute = nil }
        }
        .searchable(text: $query, prompt: "Search scheduled tasks")
        .refreshable { await repository.refresh() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                // The ACTIVE scope, not the global one: with a project open, pinning
                // every new task to `globalCronScopeID` runs it in
                // `<root>/scheduled/workspace` instead of the project workspace.
                Button("Create", systemImage: "plus") { select(.task(scopeID: repository.state.activeScopeID, taskID: nil)) }
                    .accessibilityIdentifier("cron.add")
            }
        }
    }

    private func select(_ route: CronRoute) {
        if repository.hasUnsavedChanges && !repository.routePath.isEmpty {
            pendingRoute = route
            discardAlert = true
        } else { open(route) }
    }

    private func open(_ route: CronRoute) {
        if case .task(let scope, let id) = route {
            repository.resetRoutes()
            if let id { repository.beginEdit(scopeID: scope, taskID: id) }
            else { repository.beginCreate(scopeID: scope) }
        }
    }

}

private struct CronTaskEditorView: View {
    @Bindable var repository: CronRepository
    let scopeID: String
    @State private var discardAlert = false
    @State private var modelPicker = false
    @State private var saving = false

    private var reasoningOptions: [ReasoningSelectionDto] {
        repository.modelDetails[repository.draft.automation.model ?? ""]?.reasoning.options.map(\.selection) ?? [.automatic]
    }
    private var taskHistory: [CronRunRecord] {
        repository.state.history.filter { $0.scopeID == repository.draft.scopeID && $0.taskID == repository.draft.taskID }
    }
    private var name: Binding<String> {
        Binding(get: { repository.draft.automation.name ?? "" }, set: { repository.draft.automation.name = $0.isEmpty ? nil : $0 })
    }
    private var target: Binding<String> {
        Binding(get: { repository.draft.automation.targetSessionId ?? "" }, set: { id in
            repository.draft.automation.targetSessionId = id.isEmpty ? nil : id
            if let session = repository.sessionChoices.first(where: { $0.id == id }) { repository.draft.scopeID = session.scopeID }
        })
    }
    private var reasoning: Binding<String> {
        Binding(get: { repository.draft.automation.reasoning.selection }, set: { repository.draft.automation.reasoning = CronReasoning(selection: $0) })
    }

    var body: some View {
        Form {
            Section {
                TextField("Task name", text: name)
                TextField("Instructions", text: $repository.draft.prompt, axis: .vertical).lineLimit(4...12)
            }
            detailsSection
            Section("Frequency") {
                TextField("Cron expression", text: $repository.draft.cron).textInputAutocapitalization(.never).autocorrectionDisabled()
                Toggle("Repeat on schedule", isOn: $repository.draft.recurring)
                LabeledContent("Time zone", value: TimeZone.current.identifier)
                Picker("Notifications", selection: $repository.draft.automation.notificationPolicy) {
                    ForEach(CronNotificationPolicy.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                Text("cron_ios_scheduling_note").font(.footnote).foregroundStyle(.secondary)
            }
            if let taskID = repository.draft.taskID, repository.draft.automation.status == .active {
                Section {
                    Button("Run now") { Task { await repository.runNow(scopeID: repository.draft.scopeID, taskID: taskID) } }
                        .disabled(repository.hasUnsavedChanges)
                }
            }
            if !taskHistory.isEmpty {
                Section("Run history") {
                    ForEach(taskHistory) { run in
                        Button { repository.openRun(run.runID) } label: {
                            LabeledContent(run.status.label, value: formatCronEpoch(run.scheduledAtMs))
                        }
                    }
                }
            }
            if let error = repository.state.errorMessage { Section { Text(error).foregroundStyle(.red) } }
        }
        .navigationTitle(repository.draft.isEditing ? "Edit task" : "New task")
        .navigationBarBackButtonHidden()
        .interactiveDismissDisabled(repository.hasUnsavedChanges)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") {
                    if repository.hasUnsavedChanges { discardAlert = true } else { repository.popRoute() }
                }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Save") {
                    saving = true
                    Task { await repository.saveDraft(); saving = false }
                }.disabled(saving || repository.draft.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || repository.draft.automation.model == nil)
            }
        }
        .alert("Discard unsaved changes?", isPresented: $discardAlert) {
            Button("Discard", role: .destructive) { repository.discardDraft() }
            Button("Keep editing", role: .cancel) {}
        }
        .sheet(isPresented: $modelPicker) {
            ModelPickerSheet(availableModels: repository.modelChoices, detailsByReference: repository.modelDetails,
                             activeModelId: repository.draft.automation.model ?? "", recentModels: [], onSelect: { model in
                repository.draft.automation.model = model
                let supported = reasoningOptions.map(reasoningID)
                if !supported.contains(reasoning.wrappedValue) {
                    let fallback = repository.modelDetails[model]?.reasoning.providerDefault ?? .automatic
                    repository.draft.automation.reasoning = CronReasoning(selection: reasoningID(fallback))
                }
                modelPicker = false
            }, onDismiss: { modelPicker = false })
        }
    }

    private var detailsSection: some View {
            Section("Details") {
                Picker("Status", selection: $repository.draft.automation.status) {
                    ForEach(CronTaskStatus.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                Picker("Runs in", selection: $repository.draft.automation.runMode) {
                    ForEach(CronRunMode.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                if repository.draft.automation.runMode == .selectedSession {
                    Picker("Chat", selection: target) {
                        Text("Select chat").tag("")
                        ForEach(repository.sessionChoices.filter { !repository.draft.isEditing || $0.scopeID == repository.draft.scopeID }) { Text($0.title).tag($0.id) }
                    }
                }
                Picker("Project", selection: $repository.draft.scopeID) {
                    ForEach(repository.state.scopes) { scope in Text(scope.projectID == nil ? "None" : scope.projectName).tag(scope.scopeID) }
                }.disabled(repository.draft.isEditing || repository.draft.automation.runMode == .selectedSession)
                Button { modelPicker = true } label: {
                    LabeledContent("Model", value: repository.modelDetails[repository.draft.automation.model ?? ""]?.displayName ?? repository.draft.automation.model ?? "Select model")
                }
                Picker("Reasoning", selection: reasoning) {
                    ForEach(reasoningOptions, id: \.self) { option in
                        Text(ModelDetailFormat.reasoningSelectionLabel(option)).tag(reasoningID(option))
                    }
                }
                if repository.draft.automation.reasoning.type == "token_budget",
                   let range = repository.modelDetails[repository.draft.automation.model ?? ""]?.reasoning.budgetRange {
                    TextField("Token budget", value: Binding(
                        get: { repository.draft.automation.reasoning.tokens ?? range.minTokens },
                        set: { repository.draft.automation.reasoning.tokens = min(range.maxTokens, max(range.minTokens, $0)) }
                    ), format: .number).keyboardType(.numberPad)
                }
                if repository.draft.isEditing {
                    Button("Copy to project…") { repository.beginCopy() }
                }
                if let reason = repository.draft.automation.statusReason { Text(reason).foregroundStyle(.orange) }
            }
    }

    private func reasoningID(_ selection: ReasoningSelectionDto) -> String {
        switch selection {
        case .automatic: return "automatic"
        case .disabled: return "disabled"
        case .enabled: return "enabled"
        case .level(let id): return id
        case .tokenBudget(let tokens): return "budget:\(tokens)"
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
                    if let model = run.actualModel { LabeledContent("Model", value: model) }
                    if run.sessionID != nil { Button("Open chat") { repository.openResultSession(run) } }
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
        .navigationBarBackButtonHidden()
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Back") { repository.popRoute() }
            }
        }
    }
}

private extension String {
    var firstCronLine: String? {
        split(whereSeparator: \.isNewline).first.map(String.init)
    }
}

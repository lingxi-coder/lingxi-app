import SwiftUI

struct CronRootView: View {
    @State private var repository: CronRepository
    let initialRoute: CronRoute?
    /// Opens the new-task form with these instructions already filled in.
    let initialPrompt: String?
    let onDismiss: (() -> Void)?
    @State private var closeAlert = false
    @State private var linkedResourceMissing = false

    init(
        repository: CronRepository,
        initialRoute: CronRoute? = nil,
        initialPrompt: String? = nil,
        onDismiss: (() -> Void)? = nil
    ) {
        _repository = State(initialValue: repository)
        self.initialRoute = initialRoute
        self.initialPrompt = initialPrompt
        self.onDismiss = onDismiss
    }

    var body: some View {
        Group {
            // RootView owns navigation on both phone and tablet. Keep one
            // navigation bar and let the repository retain editor/run history.
            if linkedResourceMissing {
                ContentUnavailableView(String(localized: "cron_link_unavailable"), systemImage: "link", description: Text(String(localized: "cron_link_missing")))
                    .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close") { onDismiss?() } } }
            } else if let route = repository.routePath.last {
                destination(route)
            } else {
                taskList.navigationTitle(String(localized: "settings_title_cron"))
            }
        }
        .navigationBarBackButtonHidden()
        .task {
            repository.resetRoutes()
            await repository.refresh()
            guard repository.routePath.isEmpty else { return }
            switch initialRoute {
            case .task(let scopeID, let taskID):
                if let taskID {
                    guard repository.state.tasks.contains(where: { $0.scope.scopeID == scopeID && $0.task.id == taskID }) else { linkedResourceMissing = true; return }
                    repository.beginEdit(scopeID: scopeID, taskID: taskID)
                }
                else { repository.beginCreate(scopeID: scopeID, prompt: initialPrompt ?? "") }
            case .run(let runID):
                guard repository.state.history.contains(where: { $0.runID == runID }) else { linkedResourceMissing = true; return }
                repository.openRun(runID)
            case .list, .none: break
            }
        }
        .interactiveDismissDisabled(repository.hasUnsavedChanges)
        .alert(String(localized: "cron_discard_title"), isPresented: $closeAlert) {
            Button(String(localized: "cron_discard"), role: .destructive) { repository.discardDraft(); onDismiss?() }
            Button(String(localized: "cron_keep_editing"), role: .cancel) {}
        }
        .accessibilityIdentifier("cron.root")
    }

    private var taskList: some View {
        CronListView(repository: repository)
            .toolbar {
                if let onDismiss {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(String(localized: "common_close")) {
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
                $0.task.prompt.localizedCaseInsensitiveContains(query) || $0.scope.displayName.localizedCaseInsensitiveContains(query))
        }
    }

    var body: some View {
        List {
            Section {
                Picker(String(localized: "settings_linux_section_status"), selection: $filter) {
                    Text(String(localized: "drawer_filter_all")).tag("all")
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
                                Text("\(scoped.task.human) · \(scoped.scope.displayName)").font(.subheadline).foregroundStyle(.secondary)
                                if let reason = scoped.task.configuration.statusReason ?? scoped.task.unsupportedReason {
                                    Text(reason).font(.caption).foregroundStyle(.orange)
                                } else if let next = scoped.task.nextFireMs, scoped.task.status == .active {
                                    Text(String(localized: "cron_run_summary_next \(formatCronEpoch(next))")).font(.caption).foregroundStyle(.secondary)
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
                            Button(scoped.task.status == .active ? String(localized: "cron_pause") : String(localized: "cron_resume")) {
                                Task { await repository.setStatus(scopeID: scoped.scope.scopeID, taskID: scoped.task.id, status: scoped.task.status == .active ? .paused : .active) }
                            }.tint(.orange)
                        }
                        Button(String(localized: "common_delete"), role: .destructive) {
                            Task { await repository.deleteTask(scopeID: scoped.scope.scopeID, taskID: scoped.task.id) }
                        }
                    }
                }
                if visibleTasks.isEmpty {
                    if query.isEmpty {
                        ContentUnavailableView(String(localized: "cron_no_tasks_yet"), systemImage: "clock", description: Text(String(localized: "cron_empty_help")))
                    } else { ContentUnavailableView.search(text: query) }
                }
            }
            Section {
                Text(repository.state.scheduling.note).font(.footnote).foregroundStyle(.secondary)
                if let error = repository.state.errorMessage { Text(error).foregroundStyle(.red) }
            }
        }
        .alert(String(localized: "cron_discard_title"), isPresented: $discardAlert) {
            Button(String(localized: "cron_discard"), role: .destructive) { if let route = pendingRoute { open(route) }; pendingRoute = nil }
            Button(String(localized: "cron_keep_editing"), role: .cancel) { pendingRoute = nil }
        }
        .searchable(text: $query, prompt: String(localized: "cron_search_tasks"))
        .refreshable { await repository.refresh() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                // The ACTIVE scope, not the global one: with a project open, pinning
                // every new task to `globalCronScopeID` runs it in
                // `<root>/scheduled/workspace` instead of the project workspace.
                Button(String(localized: "common_create"), systemImage: "plus") { select(.task(scopeID: repository.state.activeScopeID, taskID: nil)) }
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
        (repository.modelDetails[repository.draft.automation.model ?? ""]?.reasoning.options ?? [])
            .filter { option in
                switch option.selection {
                case .automatic, .disabled: return false
                default: return true
                }
            }
            .map(\.selection)
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
                TextField(String(localized: "cron_task_name"), text: name)
                TextField(String(localized: "cron_instructions"), text: $repository.draft.prompt, axis: .vertical).lineLimit(4...12)
            }
            detailsSection
            Section(String(localized: "cron_frequency")) {
                TextField(String(localized: "cron_expression_field_label"), text: $repository.draft.cron).textInputAutocapitalization(.never).autocorrectionDisabled()
                Toggle(String(localized: "cron_recurring_toggle"), isOn: $repository.draft.recurring)
                LabeledContent(String(localized: "cron_time_zone"), value: TimeZone.current.identifier)
                Picker(String(localized: "settings_notifications"), selection: $repository.draft.automation.notificationPolicy) {
                    ForEach(CronNotificationPolicy.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                Text("cron_ios_scheduling_note").font(.footnote).foregroundStyle(.secondary)
            }
            if let taskID = repository.draft.taskID, repository.draft.automation.status == .active {
                Section {
                    Button(String(localized: "cron_run_now_button")) { Task { await repository.runNow(scopeID: repository.draft.scopeID, taskID: taskID) } }
                        .disabled(repository.hasUnsavedChanges)
                }
            }
            if !taskHistory.isEmpty {
                Section(String(localized: "cron_run_history_section")) {
                    ForEach(taskHistory) { run in
                        Button { repository.openRun(run.runID) } label: {
                            LabeledContent(run.status.label, value: formatCronEpoch(run.scheduledAtMs))
                        }
                    }
                }
            }
            if let error = repository.state.errorMessage { Section { Text(error).foregroundStyle(.red) } }
        }
        .navigationTitle(repository.draft.isEditing ? String(localized: "cron_edit_task_title") : String(localized: "cron_new_task_title"))
        .navigationBarBackButtonHidden()
        .interactiveDismissDisabled(repository.hasUnsavedChanges)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button(String(localized: "common_cancel")) {
                    if repository.hasUnsavedChanges { discardAlert = true } else { repository.popRoute() }
                }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(String(localized: "settings_save")) {
                    saving = true
                    Task { await repository.saveDraft(); saving = false }
                }.disabled(saving || repository.draft.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || repository.draft.automation.model == nil)
            }
        }
        .alert(String(localized: "cron_discard_title"), isPresented: $discardAlert) {
            Button(String(localized: "cron_discard"), role: .destructive) { repository.discardDraft() }
            Button(String(localized: "cron_keep_editing"), role: .cancel) {}
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
            Section(String(localized: "session_details_button")) {
                Picker(String(localized: "settings_linux_section_status"), selection: $repository.draft.automation.status) {
                    ForEach(CronTaskStatus.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                Picker(String(localized: "cron_runs_in"), selection: $repository.draft.automation.runMode) {
                    ForEach(CronRunMode.allCases, id: \.self) { Text($0.label).tag($0) }
                }
                if repository.draft.automation.runMode == .selectedSession {
                    Picker(String(localized: "drawer_tab_chat"), selection: target) {
                        Text(String(localized: "cron_select_chat")).tag("")
                        ForEach(repository.sessionChoices.filter { !repository.draft.isEditing || $0.scopeID == repository.draft.scopeID }) { Text($0.title).tag($0.id) }
                    }
                }
                Picker(String(localized: "cron_run_project_label"), selection: $repository.draft.scopeID) {
                    ForEach(repository.state.scopes) { scope in Text(scope.displayName).tag(scope.scopeID) }
                }.disabled(repository.draft.isEditing || repository.draft.automation.runMode == .selectedSession)
                Button { modelPicker = true } label: {
                    LabeledContent(String(localized: "session_details_model"), value: repository.modelDetails[repository.draft.automation.model ?? ""]?.displayName ?? repository.draft.automation.model ?? String(localized: "composer_select_model"))
                }
                Picker(String(localized: "model_detail_reasoning"), selection: reasoning) {
                    ForEach(reasoningOptions, id: \.self) { option in
                        Text(ModelDetailFormat.reasoningSelectionLabel(option)).tag(reasoningID(option))
                    }
                }
                if repository.draft.automation.reasoning.type == "token_budget",
                   let range = repository.modelDetails[repository.draft.automation.model ?? ""]?.reasoning.budgetRange {
                    TextField(String(localized: "cron_token_budget"), value: Binding(
                        get: { repository.draft.automation.reasoning.tokens ?? range.minTokens },
                        set: { repository.draft.automation.reasoning.tokens = min(range.maxTokens, max(range.minTokens, $0)) }
                    ), format: .number).keyboardType(.numberPad)
                }
                if repository.draft.isEditing {
                    Button(String(localized: "cron_copy_project")) { repository.beginCopy() }
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
                Section("cron_run_section_overview") {
                    LabeledContent("cron_run_project_label", value: run.projectID == nil ? String(localized: "common_none") : run.projectName)
                    LabeledContent("settings_linux_section_status", value: run.resultCategory.label)
                    LabeledContent("cron_scheduled_at_label", value: formatCronEpoch(run.scheduledAtMs))
                    LabeledContent("cron_triggered_at_label", value: formatCronEpoch(run.triggeredAtMs))
                    if let startedAt = run.startedAtMs {
                        LabeledContent("cron_started_at_label", value: formatCronEpoch(startedAt))
                    }
                    if let finishedAt = run.finishedAtMs {
                        LabeledContent("cron_finished_at_label", value: formatCronEpoch(finishedAt))
                    }
                    if let model = run.actualModel { LabeledContent(String(localized: "session_details_model"), value: model) }
                    if run.sessionID != nil { Button(String(localized: "cron_open_chat")) { repository.openResultSession(run) } }
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
                        Text(run.errorKind.map { String(localized: "cron_error_detail \($0.label) \(error)") } ?? error)
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
                Button(String(localized: "onboarding_back")) { repository.popRoute() }
            }
        }
    }
}

private extension String {
    var firstCronLine: String? {
        split(whereSeparator: \.isNewline).first.map(String.init)
    }
}

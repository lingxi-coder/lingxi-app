import SwiftUI
import UniformTypeIdentifiers

/// The projects list: where workspaces are created and imported. Replaces the
/// per-project cards the sidebar used to carry.
struct ProjectsPage: View {
    @Environment(\.theme) private var t
    @Bindable var projectStore: ProjectStore
    /// Pin time per workspace key (`project.<id>`).
    let workspacePinnedAt: [String: Date]
    let mode: SessionMode
    let onOpenProject: (String) -> Void

    @State private var createName = ""
    @State private var showCreateAlert = false
    @State private var showFolderPicker = false

    private var projects: [ProjectSnapshot] {
        RecentSessionsBuilder.sortedProjects(projectStore.projects.filter { $0.record.visibleModes.contains(mode) }, pinnedAt: workspacePinnedAt)
    }

    var body: some View {
        List {
            if projects.isEmpty {
                Text("drawer_no_projects_detail")
                    .font(.footnote)
                    .foregroundStyle(t.text4)
                    .listRowBackground(Color.clear)
                    .accessibilityIdentifier("projects.empty")
            }
            ForEach(projects) { project in
                Button { onOpenProject(project.id) } label: {
                    HStack(spacing: 12) {
                        LXIcon(name: .folder, size: 18, color: t.text3, stroke: 1.8)
                        VStack(alignment: .leading, spacing: 3) {
                            HStack(spacing: 6) {
                                Text(project.record.name)
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundStyle(t.text)
                                    .lineLimit(1)
                                if workspacePinnedAt[ConversationScope.project(project.id).workspaceKey] != nil {
                                    Image(systemName: "pin.fill")
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(t.text4)
                                }
                            }
                            Text("\(project.record.syncState.label) · \(project.sessions.filter { !$0.isArchived && $0.mode == mode }.count)")
                                .font(.caption)
                                .foregroundStyle(t.text4)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 1.7)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("projects.row.\(project.id)")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(t.windowBg.ignoresSafeArea())
        .navigationTitle(String(localized: "drawer_entry_projects"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("drawer_new_local_project") {
                        createName = ""
                        showCreateAlert = true
                    }
                    Button("drawer_import_folder") { showFolderPicker = true }
                } label: {
                    Image(systemName: "plus")
                }
                .accessibilityLabel("drawer_new_or_import_project")
                .accessibilityIdentifier("projects.create")
            }
        }
        .alert(String(localized: "drawer_new_local_project"), isPresented: $showCreateAlert) {
            TextField("drawer_project_name_placeholder", text: $createName)
            Button("common_create") { createProject() }
                .disabled(createName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("drawer_workspace_saved_note")
        }
        .fileImporter(
            isPresented: $showFolderPicker,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false,
            onCompletion: importFolder
        )
        .accessibilityIdentifier("projects.page")
    }

    private func createProject() {
        let name = createName
        Task { @MainActor in
            do {
                let project = try await projectStore.createInternal(name: name, mode: mode)
                onOpenProject(project.id)
            } catch {
                projectStore.errorMessage = error.localizedDescription
            }
        }
    }

    private func importFolder(_ result: Result<[URL], Error>) {
        switch result {
        case let .success(urls):
            guard let url = urls.first else { return }
            Task { @MainActor in
                do {
                    let project = try await projectStore.importExternal(name: url.lastPathComponent, directoryURL: url, mode: mode)
                    onOpenProject(project.id)
                } catch {
                    projectStore.errorMessage = error.localizedDescription
                }
            }
        case let .failure(error):
            projectStore.errorMessage = error.localizedDescription
        }
    }
}

/// One project: its sessions, its scheduled tasks, its terminal and — for
/// imported folders — the sync operations.
struct ProjectPage: View {
    @Environment(\.theme) private var t
    @Environment(\.dismiss) private var dismiss
    @Bindable var projectStore: ProjectStore
    let projectID: String
    let activeScope: ConversationScope
    let mode: SessionMode
    let cronState: CronRepositoryState
    let isPinned: Bool
    let activeSession: String
    let onTogglePinned: () -> Void
    /// nil while the list shows every mode: the caller keeps its current mode.
    let onNewSession: (SessionMode?) -> Void
    let onOpenSession: (SessionMode, String, String?) -> Void
    /// `(scopeID, taskID)`; `(nil, nil)` opens the full task list.
    let onOpenCron: (String?, String?) -> Void
    let onOpenTerminal: () -> Void
    let onRecoverAuthoring: () -> Void

    @ObservedObject private var convo: ConversationModel
    @State private var showFolderPicker = false
    @State private var selectedAuthoringIntent: LocalAppAuthoringIntent?

    init(
        projectStore: ProjectStore,
        projectID: String,
        activeScope: ConversationScope,
        mode: SessionMode,
        source: any ConversationSource,
        cronState: CronRepositoryState,
        isPinned: Bool,
        activeSession: String,
        onTogglePinned: @escaping () -> Void,
        onNewSession: @escaping (SessionMode?) -> Void,
        onOpenSession: @escaping (SessionMode, String, String?) -> Void,
        onOpenCron: @escaping (String?, String?) -> Void,
        onOpenTerminal: @escaping () -> Void,
        onRecoverAuthoring: @escaping () -> Void = {}
    ) {
        self.projectStore = projectStore
        self.projectID = projectID
        self.activeScope = activeScope
        self.mode = mode
        self.cronState = cronState
        self.isPinned = isPinned
        self.activeSession = activeSession
        self.onTogglePinned = onTogglePinned
        self.onNewSession = onNewSession
        self.onOpenSession = onOpenSession
        self.onOpenCron = onOpenCron
        self.onOpenTerminal = onOpenTerminal
        self.onRecoverAuthoring = onRecoverAuthoring
        convo = source.model
    }

    private var project: ProjectSnapshot? {
        projectStore.projects.first { $0.id == projectID }
    }

    var body: some View {
        Group {
            if let project {
                content(project)
            } else {
                Color.clear.onAppear { dismiss() }
            }
        }
        .background(t.windowBg.ignoresSafeArea())
        .task(id: "\(projectID):\(projectStore.localAppIntentRevision)") {
            do {
                let intent = try await projectStore.selectedLocalAppAuthoringIntent(projectID: projectID)
                guard !Task.isCancelled else { return }
                selectedAuthoringIntent = intent
            } catch {
                guard !Task.isCancelled else { return }
                selectedAuthoringIntent = nil
                projectStore.errorMessage = error.localizedDescription
            }
        }
        .navigationTitle(project?.record.name ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: onTogglePinned) {
                    Image(systemName: isPinned ? "pin.fill" : "pin")
                }
                .accessibilityLabel(String(localized: isPinned ? "drawer_session_unpin" : "drawer_session_pin"))
                .accessibilityIdentifier("project.pin")
            }
        }
        .fileImporter(
            isPresented: $showFolderPicker,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case let .success(urls):
                if let url = urls.first {
                    projectStore.reauthorize(projectId: projectID, directoryURL: url)
                }
            case let .failure(error):
                projectStore.errorMessage = error.localizedDescription
            }
        }
        .accessibilityIdentifier("project.page")
    }

    @ViewBuilder
    private func content(_ project: ProjectSnapshot) -> some View {
        // The bound project's live catalog is fresher than the cached index.
        let scope = ConversationScope.project(projectID)
        let live = activeScope == scope ? convo.engineSessions.filter { projectStore.allowsSession($0.id, in: scope) } : []
        let cached = project.sessions.filter { projectStore.allowsSession($0.sessionId, in: scope) }
        let scopeRows = WorkspaceGroupBuilder.mergedSessionRows(live: live, cached: cached)
        let rows = RecentSessionsBuilder.sessions(in: scopeRows, filter: mode == .chat ? .chat : .code)
        let tasks = cronState.tasks.filter { $0.scope.projectID == projectID }
        List {
            Section {
                HStack(spacing: 8) {
                    Button { onNewSession(mode) } label: {
                        Label("chat_new_chat", systemImage: "square.and.pencil")
                            .font(.subheadline.weight(.semibold))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 10)
                            .background(t.accent.opacity(0.14), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .foregroundStyle(t.accent)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("project.new-session")
                    Button(action: onOpenTerminal) {
                        Label("settings_linux_section_terminal", systemImage: "terminal")
                            .font(.subheadline.weight(.semibold))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 10)
                            .background(t.surfaceActive, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .foregroundStyle(t.text)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("project.terminal")
                }
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
                Text(project.workspace.guestPath)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(t.text4)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 0, trailing: 4))
            }

            Section {
                if LocalAppAuthoringRecoveryVisibility.shouldOffer(intent: selectedAuthoringIntent, project: project) {
                    Button(action: onRecoverAuthoring) {
                        Label("local_app_authoring_recover", systemImage: "arrow.clockwise")
                    }
                    .accessibilityIdentifier("project.resume-authoring")
                }
                if rows.isEmpty {
                    Text("drawer_no_sessions")
                        .font(.footnote)
                        .foregroundStyle(t.text4)
                }
                ForEach(rows) { row in
                    Button { onOpenSession(row.mode, row.id, row.transcriptPath) } label: {
                        HStack(spacing: 10) {
                            row.mode.glyph(t, size: 15)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(row.title)
                                    .font(.subheadline.weight(row.id == activeSession ? .semibold : .medium))
                                    .foregroundStyle(t.text)
                                    .lineLimit(1)
                                Text("\(row.mode.title) · \(row.relativeTime) · \(row.messageCount)")
                                    .font(.caption)
                                    .foregroundStyle(t.text4)
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 6)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("project.session.\(row.id)")
                }
            }

            Section {
                ForEach(tasks) { task in
                    Button { onOpenCron(task.scope.scopeID, task.task.id) } label: {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(task.task.prompt)
                                .font(.subheadline.weight(.medium))
                                .foregroundStyle(t.text)
                                .lineLimit(1)
                            Text(task.task.human).font(.caption).foregroundStyle(t.text4).lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("project.cron.\(task.id)")
                }
                Button { onOpenCron(nil, nil) } label: {
                    HStack {
                        Text("project_page_manage").foregroundStyle(t.accent)
                        Spacer()
                        LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 1.7)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("project.cron.manage")
            } header: {
                Text("project_page_crons")
            }

            if project.record.storageKind == .externalBookmarkMirror {
                Section {
                    Text(project.record.syncState.label).font(.footnote).foregroundStyle(t.text3)
                    Button("drawer_reimport") { projectStore.reimport(projectId: projectID) }
                        .accessibilityIdentifier("project.reimport")
                    Button("drawer_export_back") { projectStore.export(projectId: projectID) }
                        .accessibilityIdentifier("project.export")
                    Button("drawer_reauthorize") { showFolderPicker = true }
                        .accessibilityIdentifier("project.reauthorize")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
    }
}

enum LocalAppAuthoringRecoveryVisibility {
    static func shouldOffer(intent: LocalAppAuthoringIntent?, project: ProjectSnapshot) -> Bool {
        guard let intent, intent.projectId == project.record.id else { return false }
        guard let sessionID = intent.authoringSessionId else { return true }
        return !project.sessions.contains { canonicalSessionID($0.sessionId) == canonicalSessionID(sessionID) }
    }
}

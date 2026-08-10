import SwiftUI
import UniformTypeIdentifiers

/// The workspace sidebar — the `NavigationSplitView` sidebar column. The
/// highest-frequency workspace action (Terminal) lives in the header so it is
/// always reachable without scrolling past the session list.
///
/// Dismissal belongs to the split view, not to this view: in compact width the
/// system back button and back-swipe pop it, and every navigation callback
/// already resets the column through `AppNavigationModel`. That is why there is
/// no close button and no scrim here.
struct Drawer: View {
    @Environment(\.theme) private var t
    @Bindable var projectStore: ProjectStore
    @Bindable var localAppsStore: LocalAppsStore
    @Binding var activeSession: String

    let source: any ConversationSource
    let openSettings: () -> Void
    let openTerminal: () -> Void
    let openApps: (String?) -> Void
    let onSelectProject: (String?) -> Void
    let onSelectSession: (String?, String) -> Void
    let onNewChat: (String?) -> Void

    @ObservedObject private var convo: ConversationModel
    @State private var section: Section = .chats
    @State private var query = ""
    @State private var openProjects = Set<String>()
    @State private var createProjectName = ""
    @State private var showCreateAlert = false
    @State private var showFolderPicker = false
    @State private var pickerMode: FolderPickerMode = .importProject
    @FocusState private var searchFocused: Bool

    private enum Section: String { case chats, projects, apps }
    private enum FolderPickerMode { case importProject, reauthorize(String) }

    init(
        projectStore: ProjectStore,
        localAppsStore: LocalAppsStore,
        activeSession: Binding<String>,
        source: any ConversationSource,
        openSettings: @escaping () -> Void,
        openTerminal: @escaping () -> Void,
        openApps: @escaping (String?) -> Void,
        onSelectProject: @escaping (String?) -> Void,
        onSelectSession: @escaping (String?, String) -> Void,
        onNewChat: @escaping (String?) -> Void
    ) {
        self.projectStore = projectStore
        self.localAppsStore = localAppsStore
        _activeSession = activeSession
        self.source = source
        self.openSettings = openSettings
        self.openTerminal = openTerminal
        self.openApps = openApps
        self.onSelectProject = onSelectProject
        self.onSelectSession = onSelectSession
        self.onNewChat = onNewChat
        convo = source.model
    }

    private var engineSessions: [EngineSession] {
        convo.engineSessions.filter { matches($0.title, $0.relativeTime) }
    }

    private var projects: [ProjectSnapshot] {
        projectStore.projects.filter { project in
            matches(project.record.name, project.record.syncState.label)
                || project.sessions.contains { matches($0.title, $0.relativeTime) }
        }
    }

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var projectOperationInFlight: Bool {
        projectStore.operation != nil
    }

    var body: some View {
        panel
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background { t.sidebarBg.ignoresSafeArea() }
            .onAppear {
                source.listSessions()
                if let active = projectStore.activeProjectId { openProjects.insert(active) }
                Task { await localAppsStore.refresh() }
            }
            // Titles the compact back button that returns to this column.
            .navigationTitle(String(localized: "app_name"))
            .navigationBarTitleDisplayMode(.inline)
            .navigationSplitViewColumnWidth(min: 300, ideal: 330, max: 420)
        .alert(String(localized: "drawer_new_local_project"), isPresented: $showCreateAlert) {
            TextField("drawer_project_name_placeholder", text: $createProjectName)
            Button("common_create") { createInternalProject() }
                .disabled(createProjectName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("common_cancel", role: .cancel) {}
        } message: {
            Text("drawer_workspace_saved_note")
        }
        .fileImporter(
            isPresented: $showFolderPicker,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false,
            onCompletion: handleFolderSelection
        )
    }

    private var panel: some View {
        VStack(spacing: 0) {
            activeScopeCaption
            scopePills
            searchBar
            sectionTabs
            sectionBody
            accountRow
        }
    }

    /// The app name stays in the navigation title; this header makes the active
    /// workspace and its primary developer action visible at a glance.
    private var activeScopeCaption: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 10) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(projectStore.activeProject?.record.name ?? String(localized: "drawer_global_session"))
                        .font(.system(size: 16, weight: .bold, design: .rounded))
                        .foregroundStyle(t.text)
                        .lineLimit(1)
                    Text(currentWorkspaceGuestPath)
                        .font(.system(size: 10.5, design: .monospaced))
                        .foregroundStyle(t.text4)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Button(action: openTerminal) {
                    Label("settings_linux_section_terminal", systemImage: "terminal.fill")
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(t.accent)
                        .padding(.horizontal, 11)
                        .padding(.vertical, 8)
                        .background(t.accent.opacity(0.14), in: Capsule())
                        .overlay(Capsule().stroke(t.accent.opacity(0.28), lineWidth: 0.7))
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.terminal.top")
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 12)
        .background(t.surface.opacity(0.72), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(t.border, lineWidth: 0.7)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    private var currentWorkspaceGuestPath: String {
        projectStore.activeProject?.workspace.guestPath ?? LXISHDefaultWorkspace.guestHome
    }

    private var scopePills: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                scopePill(id: nil, name: String(localized: "common_global"), icon: "◎")
                ForEach(projectStore.projects) { project in
                    scopePill(id: project.id, name: project.record.name, icon: "◇")
                }
            }
            .padding(.horizontal, 18)
        }
        .padding(.bottom, 12)
    }

    private func scopePill(id: String?, name: String, icon: String) -> some View {
        let active = projectStore.activeProjectId == id
        return Button {
            onSelectProject(id)
        } label: {
            HStack(spacing: 5) { Text(icon); Text(name).lineLimit(1) }
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(active ? t.accent : t.text3)
                .padding(.horizontal, 12).padding(.vertical, 7)
                .background(active ? t.accent.opacity(0.16) : t.surface)
                .clipShape(Capsule())
                .overlay(Capsule().stroke(active ? t.accent.opacity(0.4) : t.border, lineWidth: 0.5))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id == nil ? "drawer.scope.global" : "drawer.scope.project.\(name)")
        .accessibilityValue(active ? String(localized: "drawer_current_project_a11y") : "")
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            LXIcon(name: .search, size: 16, color: t.text4, stroke: 2)
            TextField("drawer_search_placeholder", text: $query)
                .font(.scaledSystem(14, relativeTo: .subheadline))
                .foregroundColor(t.text)
                .focused($searchFocused)
                .submitLabel(.search)
                .autocorrectionDisabled()
            if !query.isEmpty {
                Button {
                    query = ""
                    searchFocused = false
                } label: {
                    LXIcon(name: .x, size: 14, color: t.text4, stroke: 2).frame(width: 22, height: 22)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "drawer_clear_search_a11y"))
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(searchFocused ? t.accent.opacity(0.5) : t.border, lineWidth: 0.5))
        .padding(.horizontal, 18).padding(.bottom, 14)
    }

    private var sectionTabs: some View {
        HStack(spacing: 4) {
            tab(.chats, .message, String(localized: "drawer_tab_chats"), engineSessions.count)
            tab(.projects, .folder, String(localized: "drawer_tab_projects"), projects.count)
            tab(.apps, .skill, String(localized: "drawer_tab_apps"), localApps.count)
        }
        .padding(.horizontal, 14).padding(.bottom, 8)
    }

    private func tab(_ id: Section, _ icon: LXIconName, _ label: String, _ count: Int) -> some View {
        let active = section == id
        return Button { section = id } label: {
            HStack(spacing: 5) {
                LXIcon(name: icon, size: 14, color: active ? t.text : t.text3, stroke: 1.8)
                Text(label).font(.system(size: 13, weight: active ? .semibold : .medium))
                Text("\(count)").font(.system(size: 10.5, weight: .semibold)).foregroundColor(active ? t.accent : t.text4)
            }
            .foregroundColor(active ? t.text : t.text3)
            .frame(maxWidth: .infinity).padding(.vertical, 9)
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 9))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawer.tab.\(id.rawValue)")
    }

    private var sectionBody: some View {
        ScrollView(showsIndicators: false) {
            VStack(alignment: .leading, spacing: 6) {
                if currentSectionEmpty {
                    emptyState
                } else {
                    switch section {
                    case .chats: chatsSection
                    case .projects: projectsSection
                    case .apps: appsSection
                    }
                }
            }
            .padding(.horizontal, 12).padding(.top, 4).padding(.bottom, 8)
        }
        .frame(maxHeight: .infinity)
    }

    private var currentSectionEmpty: Bool {
        switch section {
        case .chats: engineSessions.isEmpty
        case .projects: projects.isEmpty
        case .apps: localApps.isEmpty
        }
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            LXIcon(name: searching ? .search : section == .projects ? .folder : section == .apps ? .skill : .message,
                   size: 24, color: t.text4, stroke: 1.8)
            Text(searching ? String(localized: "drawer_no_match") : emptyCopy)
                .font(.system(size: 13)).foregroundColor(t.text4).multilineTextAlignment(.center)
            if !searching, section == .chats { newChatButton(projectID: projectStore.activeProjectId) }
            if !searching, section == .projects {
                projectCreationMenu
            }
            if !searching, section == .apps {
                dashedButton(String(localized: "drawer_create_app")) { openApps(nil) }
                    // Same identifier as the populated section's own create row
                    // (they never render together), so the apps route has one
                    // addressable entry point whether or not any app exists.
                    .accessibilityIdentifier("drawer.apps.create")
            }
        }
        .frame(maxWidth: .infinity).padding(.top, 38).padding(.horizontal, 10)
    }

    private var emptyCopy: String {
        switch section {
        case .chats: return String(localized: "drawer_empty_chats")
        case .projects: return String(localized: "drawer_empty_projects")
        case .apps: return String(localized: "drawer_empty_apps")
        }
    }

    private var localApps: [LocalAppSummary] {
        localAppsStore.apps.filter { matches($0.name, $0.workflow.label) }
    }

    private var appsSection: some View {
        LocalAppsDrawerSection(
            apps: localApps,
            onOpenLibrary: { openApps(nil) },
            onOpenApp: { openApps($0) }
        )
    }

    private var chatsSection: some View {
        VStack(spacing: 2) {
            if !searching { newChatButton(projectID: projectStore.activeProjectId) }
            ForEach(engineSessions) { session in
                sessionButton(session.id, title: session.title, subtitle: String(localized: "drawer_session_subtitle \(session.relativeTime) \(session.messageCount)"), projectID: projectStore.activeProjectId)
            }
        }
    }

    private func newChatButton(projectID: String?) -> some View {
        Button {
            onNewChat(projectID)
        } label: {
            Label("chat_new_conversation", systemImage: "square.and.pencil")
                .font(.system(size: 13.5, weight: .medium)).foregroundColor(t.accent)
                .frame(maxWidth: .infinity, alignment: .leading).padding(10)
                .background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
    }

    private var projectsSection: some View {
        VStack(spacing: 6) {
            ForEach(projects) { project in projectCard(project) }
            if !searching {
                projectCreationMenu
            }
        }
    }

    private var projectCreationMenu: some View {
        Menu {
            Button("drawer_new_local_project") {
                createProjectName = ""
                showCreateAlert = true
            }
            Button("drawer_import_folder") {
                pickerMode = .importProject
                showFolderPicker = true
            }
        } label: {
            Label("drawer_new_or_import_project", systemImage: "plus")
                .font(.system(size: 13)).foregroundColor(t.text3)
                .frame(maxWidth: .infinity).padding(11)
                .overlay(
                    RoundedRectangle(cornerRadius: 10)
                        .stroke(t.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                )
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawer.project.create")
    }

    private func projectCard(_ project: ProjectSnapshot) -> some View {
        let active = project.id == projectStore.activeProjectId
        let expanded = searching || openProjects.contains(project.id)
        let conflicts = projectStore.conflicts.filter { $0.projectId == project.id }
        return VStack(alignment: .leading, spacing: 4) {
            Button {
                if !active { onSelectProject(project.id) }
                if expanded { openProjects.remove(project.id) } else { openProjects.insert(project.id) }
            } label: {
                HStack(spacing: 10) {
                    LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 2)
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                    LXIcon(name: .folder, size: 17, color: active ? t.accent : t.text3, stroke: 1.8)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(project.record.name).font(.system(size: 14, weight: .semibold)).foregroundColor(t.text)
                        Text(project.record.syncState.label).font(.caption).foregroundColor(syncColor(project.record.syncState))
                    }
                    Spacer()
                    Text("\(project.sessions.count)").font(.caption).foregroundColor(t.text4)
                }
                .padding(10).background(active ? t.surfaceActive : .clear).clipShape(RoundedRectangle(cornerRadius: 10))
            }
            .buttonStyle(.plain)
            if expanded {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(project.sessions.filter { matches($0.title, $0.relativeTime) }) { session in
                        sessionButton(session.sessionId, title: session.title,
                                      subtitle: String(localized: "drawer_session_subtitle \(session.relativeTime) \(session.messageCount)"), projectID: project.id)
                    }
                    Button {
                        onNewChat(project.id)
                    } label: {
                        Label("drawer_project_new_session", systemImage: "plus").font(.caption).foregroundColor(t.text3).padding(8)
                    }
                    .buttonStyle(.plain)
                    projectActions(project)
                    if !conflicts.isEmpty { conflictActions(project, count: conflicts.count) }
                }
                .padding(.leading, 26)
            }
        }
    }

    private func projectActions(_ project: ProjectSnapshot) -> some View {
        HStack(spacing: 8) {
            if project.record.storageKind == .externalBookmarkMirror {
                Button("drawer_reimport") { projectStore.reimport(projectId: project.id) }
                    .buttonStyle(.bordered)
                Button("drawer_export_back") { projectStore.export(projectId: project.id) }
                    .buttonStyle(.bordered)
                if project.record.syncState == .authorizationLost {
                    Button("drawer_reauthorize") {
                        pickerMode = .reauthorize(project.id)
                        showFolderPicker = true
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .font(.caption)
        .disabled(projectOperationInFlight)
    }

    private func conflictActions(_ project: ProjectSnapshot, count: Int) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("drawer_conflicts_count \(count)").font(.caption).foregroundColor(t.danger)
            HStack {
                Button("drawer_keep_internal") { projectStore.resolveConflicts(projectId: project.id, resolution: .keepInternal) }
                Button("drawer_keep_external") { projectStore.resolveConflicts(projectId: project.id, resolution: .keepExternal) }
            }
            .buttonStyle(.bordered)
            .font(.caption)
            .disabled(projectOperationInFlight)
        }
    }

    private func sessionButton(_ id: String, title: String, subtitle: String, projectID: String?) -> some View {
        let active = id == activeSession && projectID == projectStore.activeProjectId
        return Button {
            onSelectSession(projectID, id)
        } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 13.5, weight: active ? .semibold : .medium)).foregroundColor(active ? t.text : t.text2).lineLimit(1)
                Text(subtitle).font(.caption).foregroundColor(t.text4).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding(9)
            .background(active ? t.surfaceActive : .clear).clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
    }

    private func dashedButton(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(label, systemImage: "plus")
                .font(.system(size: 13)).foregroundColor(t.text3)
                .frame(maxWidth: .infinity).padding(11)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
        }
        .buttonStyle(.plain)
    }

    private var accountRow: some View {
        Button(action: openSettings) {
            HStack(spacing: 12) {
                Circle().fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 36, height: 36)
                    .overlay(LXIcon(name: .cog, size: 16, color: .white, stroke: 1.8))
                Text("settings_title_main").font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                Spacer()
                LXIcon(name: .chevronR, size: 14, color: t.text4, stroke: 1.7)
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawer.settings")
        .padding(.horizontal, 12).padding(.vertical, 10)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .top)
    }

    private func matches(_ haystacks: String...) -> Bool {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return needle.isEmpty || haystacks.contains { $0.lowercased().contains(needle) }
    }

    private func syncColor(_ state: ProjectSyncState) -> Color {
        switch state {
        case .synced, .localOnly: return t.ok
        case .conflict, .error: return t.danger
        case .authorizationLost: return .orange
        case .changesPending, .syncing: return t.text3
        }
    }

    private func createInternalProject() {
        Task { @MainActor in
            do {
                let project = try await projectStore.createInternal(name: createProjectName)
                onSelectProject(project.id)
            } catch {
                projectStore.errorMessage = error.localizedDescription
            }
        }
    }

    private func handleFolderSelection(_ result: Result<[URL], Error>) {
        guard case let .success(urls) = result, let url = urls.first else {
            if case let .failure(error) = result { projectStore.errorMessage = error.localizedDescription }
            return
        }
        switch pickerMode {
        case .importProject:
            Task { @MainActor in
                do {
                    let project = try await projectStore.importExternal(name: url.lastPathComponent, directoryURL: url)
                    onSelectProject(project.id)
                } catch {
                    projectStore.errorMessage = error.localizedDescription
                }
            }
        case .reauthorize(let projectID):
            projectStore.reauthorize(projectId: projectID, directoryURL: url)
        }
    }
}

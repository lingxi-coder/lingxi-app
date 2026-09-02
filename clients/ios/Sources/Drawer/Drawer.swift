import SwiftUI
import UniformTypeIdentifiers

typealias WorkspaceSessionMode = SessionMode

enum DrawerSection: String, CaseIterable, Hashable, Sendable {
    case chat
    case code
    case cron

    var sessionMode: WorkspaceSessionMode? {
        switch self {
        case .chat: .chat
        case .code: .code
        case .cron: nil
        }
    }
}

enum WorkspaceGroupKind: String, Hashable, Sendable {
    case global
    case project
    case localApp
}

struct WorkspaceSessionRow: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let mode: WorkspaceSessionMode
    let modifiedAt: Date?
    let relativeTime: String
    let messageCount: Int
    let isInit: Bool
}

struct WorkspaceCronRow: Identifiable, Equatable, Sendable {
    let scopeID: String
    let taskID: String
    let title: String
    let detail: String
    let nextFireAt: Date?

    var id: String { "\(scopeID):\(taskID)" }
}

struct WorkspaceGroup: Identifiable, Equatable, Sendable {
    let scope: ConversationScope
    let key: String
    let kind: WorkspaceGroupKind
    let title: String
    let subtitle: String?
    let updatedAt: Date?
    let pinnedAt: Date?
    let sessions: [WorkspaceSessionRow]
    let cronRows: [WorkspaceCronRow]

    var id: String { key }
    var latestSessionAt: Date? { sessions.compactMap(\.modifiedAt).max() }
    var latestCronAt: Date? { cronRows.compactMap(\.nextFireAt).max() }
}

struct WorkspaceGroupSeed: Equatable, Sendable {
    let scope: ConversationScope
    let kind: WorkspaceGroupKind
    let title: String
    let subtitle: String?
    let updatedAt: Date?
    let sessions: [WorkspaceSessionRow]
}

enum WorkspaceGroupBuilder {
    static func seededConversationGroups(
        section: DrawerSection,
        query: String,
        groups: [WorkspaceGroupSeed],
        pinnedAt: [String: Date] = [:]
    ) -> [WorkspaceGroup] {
        guard let mode = section.sessionMode else { return [] }
        let needle = normalized(query)
        return groups.map {
            WorkspaceGroup(
                scope: $0.scope,
                key: $0.scope.workspaceKey,
                kind: $0.kind,
                title: $0.title,
                subtitle: $0.subtitle,
                updatedAt: $0.updatedAt,
                pinnedAt: pinnedAt[$0.scope.workspaceKey],
                sessions: filteredSessions($0.sessions, mode: mode),
                cronRows: []
            )
        }
        .compactMap { filteredConversationGroup($0, query: needle) }
        .sorted { lhs, rhs in compareGroups(lhs, rhs, latestAt: { $0.latestSessionAt }) }
    }

    static func conversationGroups(
        section: DrawerSection,
        query: String,
        activeScope: ConversationScope,
        liveSessions: [EngineSession],
        globalSessions: [ProjectSessionSummary],
        projects: [ProjectSnapshot],
        localApps: [LocalAppSummary],
        localAppSessionPages: [String: LocalAppSessionPage],
        pinnedAt: [String: Date] = [:]
    ) -> [WorkspaceGroup] {
        let globalSeed = WorkspaceGroupSeed(
            scope: .global,
            kind: .global,
            title: String(localized: "common_global"),
            subtitle: nil,
            updatedAt: nil,
            sessions: (activeScope == .global && !liveSessions.isEmpty ? liveSessions.map(engineRow) : globalSessions.map(projectRow))
        )
        let projectSeeds = projects.map { project in
            let scope = ConversationScope.project(project.id)
            return WorkspaceGroupSeed(
                scope: scope,
                kind: .project,
                title: project.record.name,
                subtitle: project.record.syncState.label,
                updatedAt: project.record.updatedAt,
                sessions: (activeScope == scope && !liveSessions.isEmpty ? liveSessions.map(engineRow) : project.sessions.map(projectRow))
            )
        }
        let appSeeds = localApps.map { app in
            WorkspaceGroupSeed(
                scope: .localApp(app.id),
                kind: .localApp,
                title: app.displayName,
                subtitle: app.draftStatusLine ?? app.workflow.label,
                updatedAt: app.updatedAt,
                sessions: (localAppSessionPages[app.id]?.rows ?? []).map(localAppRow)
            )
        }
        return seededConversationGroups(
            section: section,
            query: query,
            groups: [globalSeed] + projectSeeds + appSeeds,
            pinnedAt: pinnedAt
        )
    }

    static func cronGroups(
        query: String,
        projects: [ProjectSnapshot],
        cronState: CronRepositoryState,
        appSandboxRoot: String,
        pinnedAt: [String: Date] = [:]
    ) -> [WorkspaceGroup] {
        let needle = normalized(query)
        let tasksByScope = Dictionary(grouping: cronState.tasks, by: { $0.scope.scopeID })
        let scopes = [CronScope.global(appSandboxRoot: appSandboxRoot)]
            + projects.map {
                CronScope(
                    scopeID: "project.\($0.id)",
                    projectID: $0.id,
                    projectName: $0.record.name,
                    projectCwd: $0.workspace.hostURL.path,
                    guestWorkspacePath: $0.workspace.guestPath
                )
            }
        return scopes.map { scope in
            WorkspaceGroup(
                scope: scope.projectID.map(ConversationScope.project) ?? .global,
                key: scope.scopeID,
                kind: scope.projectID == nil ? .global : .project,
                title: scope.projectName,
                subtitle: scope.guestWorkspacePath,
                updatedAt: nil,
                pinnedAt: pinnedAt[scope.scopeID],
                sessions: [],
                cronRows: (tasksByScope[scope.scopeID] ?? []).map {
                    WorkspaceCronRow(
                        scopeID: scope.scopeID,
                        taskID: $0.task.id,
                        title: $0.task.prompt,
                        detail: $0.task.human,
                        nextFireAt: $0.task.nextFireMs.map { Date(timeIntervalSince1970: TimeInterval($0) / 1_000) }
                    )
                }
            )
        }
        .filter { includeCronGroup($0, query: needle) }
        .sorted { lhs, rhs in compareGroups(lhs, rhs, latestAt: { $0.latestCronAt }) }
    }

    private static func engineRow(_ session: EngineSession) -> WorkspaceSessionRow {
        WorkspaceSessionRow(
            id: session.id,
            title: session.title,
            mode: session.mode,
            modifiedAt: session.modifiedAt,
            relativeTime: session.relativeTime,
            messageCount: session.messageCount,
            isInit: false
        )
    }

    private static func projectRow(_ session: ProjectSessionSummary) -> WorkspaceSessionRow {
        WorkspaceSessionRow(
            id: session.sessionId,
            title: session.title,
            mode: session.mode,
            modifiedAt: session.modifiedAt,
            relativeTime: session.relativeTime,
            messageCount: session.messageCount,
            isInit: false
        )
    }

    private static func localAppRow(_ session: LocalAppSessionRow) -> WorkspaceSessionRow {
        WorkspaceSessionRow(
            id: session.uuid,
            title: session.title,
            mode: session.mode,
            modifiedAt: session.modifiedAt,
            relativeTime: session.relativeTime,
            messageCount: session.messageCount,
            isInit: session.isInit
        )
    }

    private static func filteredSessions(_ sessions: [WorkspaceSessionRow], mode: WorkspaceSessionMode) -> [WorkspaceSessionRow] {
        sessions.filter { $0.mode == mode }
            .sorted { lhs, rhs in
                switch (lhs.modifiedAt, rhs.modifiedAt) {
                case let (l?, r?) where l != r:
                    return l > r
                case (.some, .none):
                    return true
                case (.none, .some):
                    return false
                default:
                    return lhs.title.localizedStandardCompare(rhs.title) == .orderedAscending
                }
            }
    }

    private static func filteredConversationGroup(
        _ group: WorkspaceGroup,
        query: String
    ) -> WorkspaceGroup? {
        guard !query.isEmpty else { return group }
        if matches(group.title, query) || matches(group.subtitle, query) { return group }
        let matchingSessions = group.sessions.filter { matches($0.title, query) }
        guard !matchingSessions.isEmpty else { return nil }
        return WorkspaceGroup(
            scope: group.scope,
            key: group.key,
            kind: group.kind,
            title: group.title,
            subtitle: group.subtitle,
            updatedAt: group.updatedAt,
            pinnedAt: group.pinnedAt,
            sessions: matchingSessions,
            cronRows: group.cronRows
        )
    }

    private static func includeCronGroup(_ group: WorkspaceGroup, query: String) -> Bool {
        guard !query.isEmpty else { return true }
        if matches(group.title, query) || matches(group.subtitle, query) { return true }
        return group.cronRows.contains { matches($0.title, query) || matches($0.detail, query) }
    }

    private static func compareGroups(
        _ lhs: WorkspaceGroup,
        _ rhs: WorkspaceGroup,
        latestAt: (WorkspaceGroup) -> Date?
    ) -> Bool {
        if lhs.kind == .global || rhs.kind == .global {
            return lhs.kind == .global && rhs.kind != .global
        }
        switch (lhs.pinnedAt, rhs.pinnedAt) {
        case let (l?, r?) where l != r:
            return l > r
        case (.some, .none):
            return true
        case (.none, .some):
            return false
        default:
            break
        }
        switch (latestAt(lhs), latestAt(rhs)) {
        case let (l?, r?) where l != r:
            return l > r
        case (.some, .none):
            return true
        case (.none, .some):
            return false
        default:
            break
        }
        switch (lhs.updatedAt, rhs.updatedAt) {
        case let (l?, r?) where l != r:
            return l > r
        case (.some, .none):
            return true
        case (.none, .some):
            return false
        default:
            break
        }
        let titleOrder = lhs.title.localizedStandardCompare(rhs.title)
        if titleOrder != .orderedSame { return titleOrder == .orderedAscending }
        return lhs.key < rhs.key
    }

    private static func normalized(_ query: String) -> String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func matches(_ value: String?, _ query: String) -> Bool {
        guard let value, !query.isEmpty else { return false }
        return value.localizedStandardContains(query)
    }
}

struct Drawer: View {
    @Environment(\.theme) private var t
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Bindable var projectStore: ProjectStore
    @Bindable var localAppsStore: LocalAppsStore
    let activeScope: ConversationScope
    @Binding var activeSession: String

    let source: any ConversationSource
    let cronState: CronRepositoryState
    let activeMode: WorkspaceSessionMode
    let workspacePinnedAt: [String: Date]
    let collapsedWorkspaceKeys: Set<String>
    let openSettings: () -> Void
    let openTerminal: () -> Void
    let openApps: (String?) -> Void
    let closeSidebar: () -> Void
    let createApp: () -> Void
    let onSelectProject: (String?) -> Void
    let onSelectSession: (String?, String) -> Void
    let onNewChat: (String?) -> Void
    let onSelectAppSession: (String, String) -> Void
    let onNewAppChat: (String) -> Void
    let onModeChanged: ((WorkspaceSessionMode) -> Void)?
    let onToggleWorkspacePinned: (String) -> Void
    let onSetWorkspaceCollapsed: (Bool, String) -> Void
    let onContinueInMode: (ConversationScope, String, WorkspaceSessionMode, WorkspaceSessionMode) -> Void

    @ObservedObject private var convo: ConversationModel
    @State private var section: DrawerSection
    @State private var query = ""
    @State private var createProjectName = ""
    @State private var showCreateAlert = false
    @State private var showFolderPicker = false
    @State private var pickerMode: FolderPickerMode = .importProject
    @FocusState private var searchFocused: Bool

    private enum FolderPickerMode { case importProject, reauthorize(String) }

    init(
        projectStore: ProjectStore,
        localAppsStore: LocalAppsStore,
        activeScope: ConversationScope = .global,
        activeSession: Binding<String>,
        source: any ConversationSource,
        cronState: CronRepositoryState = .init(),
        activeMode: WorkspaceSessionMode = .code,
        workspacePinnedAt: [String: Date] = [:],
        collapsedWorkspaceKeys: Set<String> = [],
        openSettings: @escaping () -> Void,
        openTerminal: @escaping () -> Void,
        openApps: @escaping (String?) -> Void,
        closeSidebar: @escaping () -> Void,
        createApp: @escaping () -> Void,
        onSelectProject: @escaping (String?) -> Void,
        onSelectSession: @escaping (String?, String) -> Void,
        onNewChat: @escaping (String?) -> Void,
        onSelectAppSession: @escaping (String, String) -> Void = { _, _ in },
        onNewAppChat: @escaping (String) -> Void = { _ in },
        onModeChanged: ((WorkspaceSessionMode) -> Void)? = nil,
        onToggleWorkspacePinned: @escaping (String) -> Void = { _ in },
        onSetWorkspaceCollapsed: @escaping (Bool, String) -> Void = { _, _ in },
        onContinueInMode: @escaping (ConversationScope, String, WorkspaceSessionMode, WorkspaceSessionMode) -> Void = { _, _, _, _ in }
    ) {
        self.projectStore = projectStore
        self.localAppsStore = localAppsStore
        self.activeScope = activeScope
        _activeSession = activeSession
        self.source = source
        self.cronState = cronState
        self.activeMode = activeMode
        self.workspacePinnedAt = workspacePinnedAt
        self.collapsedWorkspaceKeys = collapsedWorkspaceKeys
        self.openSettings = openSettings
        self.openTerminal = openTerminal
        self.openApps = openApps
        self.closeSidebar = closeSidebar
        self.createApp = createApp
        self.onSelectProject = onSelectProject
        self.onSelectSession = onSelectSession
        self.onNewChat = onNewChat
        self.onSelectAppSession = onSelectAppSession
        self.onNewAppChat = onNewAppChat
        self.onModeChanged = onModeChanged
        self.onToggleWorkspacePinned = onToggleWorkspacePinned
        self.onSetWorkspaceCollapsed = onSetWorkspaceCollapsed
        self.onContinueInMode = onContinueInMode
        _section = State(initialValue: activeMode == .chat ? .chat : .code)
        convo = source.model
    }

    private var activeApp: LocalAppSummary? {
        activeScope.appID.flatMap { localAppsStore.app(id: $0) }
    }

    private var currentWorkspaceGuestPath: String {
        projectStore.activeProject?.workspace.guestPath ?? LXISHDefaultWorkspace.guestHome
    }

    private var conversationGroups: [WorkspaceGroup] {
        WorkspaceGroupBuilder.conversationGroups(
            section: section,
            query: query,
            activeScope: activeScope,
            liveSessions: convo.engineSessions,
            globalSessions: projectStore.globalSessions,
            projects: projectStore.projects,
            localApps: localAppsStore.apps,
            localAppSessionPages: localAppsStore.sessionPages,
            pinnedAt: workspacePinnedAt
        )
    }

    private var cronGroups: [WorkspaceGroup] {
        WorkspaceGroupBuilder.cronGroups(
            query: query,
            projects: projectStore.projects,
            cronState: cronState,
            appSandboxRoot: ConversationSourceFactory.appSandboxRoot(),
            pinnedAt: workspacePinnedAt
        )
    }

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            searchBar
            tabs
            bodyContent
            accountRow
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { t.sidebarBg.ignoresSafeArea() }
        .onAppear {
            source.listSessions()
            Task {
                await localAppsStore.refresh()
                for app in localAppsStore.apps {
                    await localAppsStore.listSessions(appID: app.id)
                }
            }
        }
        .onChange(of: activeMode) { _, mode in
            section = mode == .chat ? .chat : .code
        }
        .navigationTitle(String(localized: "app_name"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if horizontalSizeClass == .compact {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: closeSidebar) {
                        Label("drawer_close_sidebar_a11y", systemImage: "xmark")
                            .labelStyle(.iconOnly)
                    }
                    .accessibilityIdentifier("drawer.close")
                }
            }
        }
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

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(activeScope.isLocalApp
                    ? (activeApp?.displayName ?? activeScope.appID ?? "")
                    : (projectStore.activeProject?.record.name ?? String(localized: "drawer_global_session")))
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
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("drawer.terminal.top")
        }
        .padding(16)
        .background(t.surface.opacity(0.72), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(t.border, lineWidth: 0.7))
        .padding(.horizontal, 12)
        .padding(.top, 8)
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
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(searchFocused ? t.accent.opacity(0.5) : t.border, lineWidth: 0.5))
        .padding(.horizontal, 18)
        .padding(.bottom, 14)
    }

    private var tabs: some View {
        HStack(spacing: 4) {
            tab(.chat, .message, String(localized: "drawer_tab_chat"), count: count(for: .chat))
            tab(.code, .terminal, String(localized: "drawer_tab_code"), count: count(for: .code))
            tab(.cron, .clock, String(localized: "drawer_tab_crons"), count: cronGroups.count)
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 8)
    }

    private func tab(_ id: DrawerSection, _ icon: LXIconName, _ label: String, count: Int) -> some View {
        let active = section == id
        return Button {
            section = id
            if let mode = id.sessionMode {
                onModeChanged?(mode)
            }
        } label: {
            HStack(spacing: 5) {
                LXIcon(name: icon, size: 14, color: active ? t.text : t.text3, stroke: 1.8)
                Text(label).font(.system(size: 13, weight: active ? .semibold : .medium))
                Text("\(count)").font(.system(size: 10.5, weight: .semibold)).foregroundColor(active ? t.accent : t.text4)
            }
            .foregroundColor(active ? t.text : t.text3)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 9)
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 9))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawer.tab.\(id.rawValue)")
    }

    private func count(for section: DrawerSection) -> Int {
        WorkspaceGroupBuilder.conversationGroups(
            section: section,
            query: query,
            activeScope: activeScope,
            liveSessions: convo.engineSessions,
            globalSessions: projectStore.globalSessions,
            projects: projectStore.projects,
            localApps: localAppsStore.apps,
            localAppSessionPages: localAppsStore.sessionPages,
            pinnedAt: workspacePinnedAt
        ).count
    }

    private var bodyContent: some View {
        ScrollView(showsIndicators: false) {
            VStack(alignment: .leading, spacing: 10) {
                if section != .cron {
                    conversationActions
                    ForEach(conversationGroups) { group in
                        conversationGroupCard(group)
                    }
                } else {
                    ForEach(cronGroups) { group in
                        cronGroupCard(group)
                    }
                }
                if visibleGroupCount == 0 {
                    emptyState
                }
            }
            .padding(.horizontal, 12)
            .padding(.top, 4)
            .padding(.bottom, 8)
        }
        .frame(maxHeight: .infinity)
    }

    private var visibleGroupCount: Int {
        section == .cron ? cronGroups.count : conversationGroups.count
    }

    private var conversationActions: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                projectCreationMenu
                dashedButton(String(localized: "drawer_create_app"), action: createApp)
                    .accessibilityIdentifier("drawer.apps.create")
            }
            Button {
                openApps(nil)
            } label: {
                Label(String(localized: "drawer_apps_library"), systemImage: localAppIconSystemName)
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundStyle(t.text3)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(t.surface, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("drawer.apps.library")
        }
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            LXIcon(name: searching ? .search : section == .cron ? .clock : .folder, size: 24, color: t.text4, stroke: 1.8)
            Text(searching ? String(localized: "drawer_no_match") : String(localized: "drawer_no_workspaces"))
                .font(.system(size: 13))
                .foregroundColor(t.text4)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 24)
    }

    private func conversationGroupCard(_ group: WorkspaceGroup) -> some View {
        let collapsed = collapsedWorkspaceKeys.contains(group.key)
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                glyph(for: group.kind)
                VStack(alignment: .leading, spacing: 2) {
                    Text(group.title).font(.system(size: 14, weight: .semibold)).foregroundStyle(t.text)
                    if let subtitle = group.subtitle, !subtitle.isEmpty {
                        Text(subtitle).font(.caption).foregroundStyle(t.text4).lineLimit(1)
                    }
                }
                Spacer()
                Text("\(group.sessions.count)")
                    .font(.caption)
                    .foregroundStyle(t.text4)
                Button {
                    onToggleWorkspacePinned(group.key)
                } label: {
                    Image(systemName: group.pinnedAt == nil ? "pin" : "pin.fill")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(group.pinnedAt == nil ? t.text4 : t.accent)
                        .frame(width: 28, height: 28)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.workspace.pin.\(group.key)")
                Button {
                    onSetWorkspaceCollapsed(!collapsed, group.key)
                } label: {
                    Image(systemName: collapsed ? "chevron.down" : "chevron.up")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(t.text4)
                        .frame(width: 28, height: 28)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.workspace.collapse.\(group.key)")
                if let appID = group.scope.appID {
                    Button(String(localized: "drawer_workspace_details")) { openApps(appID) }
                        .font(.caption)
                        .buttonStyle(.borderless)
                } else {
                    Button(String(localized: "common_open")) { selectWorkspace(group.scope) }
                        .font(.caption)
                        .buttonStyle(.borderless)
                }
                Button {
                    startNewConversation(in: group.scope)
                } label: {
                    Image(systemName: "square.and.pencil")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(t.accent)
                        .frame(width: 30, height: 30)
                        .background(t.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.workspace.new.\(group.key)")
            }

            if collapsed {
                EmptyView()
            } else if group.sessions.isEmpty {
                Text(String(localized: "drawer_empty_chats"))
                    .font(.caption)
                    .foregroundStyle(t.text4)
                    .padding(.leading, 30)
            } else {
                ForEach(group.sessions) { row in
                    sessionRow(row, scope: group.scope)
                }
            }
        }
        .padding(12)
        .background(group.scope == activeScope ? t.surfaceActive : t.surface, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(t.border, lineWidth: 0.6))
        .accessibilityIdentifier("drawer.workspace.\(group.key)")
    }

    private func cronGroupCard(_ group: WorkspaceGroup) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                glyph(for: group.kind)
                VStack(alignment: .leading, spacing: 2) {
                    Text(group.title).font(.system(size: 14, weight: .semibold)).foregroundStyle(t.text)
                    if let subtitle = group.subtitle, !subtitle.isEmpty {
                        Text(subtitle).font(.caption).foregroundStyle(t.text4).lineLimit(1)
                    }
                }
                Spacer()
                Text("\(group.cronRows.count)").font(.caption).foregroundStyle(t.text4)
                Button {
                    onToggleWorkspacePinned(group.key)
                } label: {
                    Image(systemName: group.pinnedAt == nil ? "pin" : "pin.fill")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(group.pinnedAt == nil ? t.text4 : t.accent)
                        .frame(width: 28, height: 28)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.workspace.pin.\(group.key)")
            }
            if group.cronRows.isEmpty {
                Text(String(localized: "drawer_no_crons"))
                    .font(.caption)
                    .foregroundStyle(t.text4)
                    .padding(.leading, 30)
            } else {
                ForEach(group.cronRows) { row in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.title).font(.system(size: 13.5, weight: .medium)).foregroundStyle(t.text2)
                        Text(row.detail).font(.caption).foregroundStyle(t.text4)
                    }
                    .padding(.leading, 30)
                }
            }
        }
        .padding(12)
        .background(t.surface, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(t.border, lineWidth: 0.6))
        .accessibilityIdentifier("drawer.workspace.\(group.key)")
    }

    private func sessionRow(_ row: WorkspaceSessionRow, scope: ConversationScope) -> some View {
        let active = scope == activeScope && row.id == activeSession
        let title = row.isInit ? "\(row.title) · \(String(localized: "local_apps_session_init_badge"))" : row.title
        let targetMode: WorkspaceSessionMode = row.mode == .chat ? .code : .chat
        return Button {
            selectSession(row.id, in: scope)
        } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 13.5, weight: active ? .semibold : .medium)).foregroundColor(active ? t.text : t.text2).lineLimit(1)
                Text(String(localized: "drawer_session_subtitle \(row.relativeTime) \(row.messageCount)"))
                    .font(.caption)
                    .foregroundColor(t.text4)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(9)
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(
                String(localized: targetMode == .chat ? "drawer_session_continue_chat" : "drawer_session_continue_code")
            ) {
                onContinueInMode(scope, row.id, row.mode, targetMode)
            }
        }
        .accessibilityIdentifier("drawer.session.\(scope.workspaceKey).\(row.id)")
    }

    private func glyph(for kind: WorkspaceGroupKind) -> some View {
        Group {
            switch kind {
            case .global:
                Text("◎")
            case .project:
                LXIcon(name: .folder, size: 15, color: t.text3, stroke: 1.8)
            case .localApp:
                Image(systemName: localAppIconSystemName)
                    .font(.system(size: 13))
                    .foregroundStyle(t.accent)
            }
        }
        .frame(width: 18, height: 18)
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
                .font(.system(size: 13))
                .foregroundColor(t.text3)
                .frame(maxWidth: .infinity)
                .padding(11)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawer.project.create")
    }

    private func dashedButton(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(label, systemImage: "plus")
                .font(.system(size: 13))
                .foregroundColor(t.text3)
                .frame(maxWidth: .infinity)
                .padding(11)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
        }
        .buttonStyle(.plain)
    }

    private var accountRow: some View {
        Button(action: openSettings) {
            HStack(spacing: 12) {
                Circle()
                    .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 36, height: 36)
                    .overlay(LXIcon(name: .cog, size: 16, color: .white, stroke: 1.8))
                Text("settings_title_main").font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                Spacer()
                LXIcon(name: .chevronR, size: 14, color: t.text4, stroke: 1.7)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("drawer.settings")
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .top)
    }

    private func selectWorkspace(_ scope: ConversationScope) {
        if let mode = section.sessionMode {
            onModeChanged?(mode)
        }
        switch scope {
        case .global:
            onSelectProject(nil)
        case let .project(id):
            onSelectProject(id)
        case let .localApp(id):
            openApps(id)
        }
    }

    private func selectSession(_ sessionID: String, in scope: ConversationScope) {
        if let mode = section.sessionMode {
            onModeChanged?(mode)
        }
        switch scope {
        case .global:
            onSelectSession(nil, sessionID)
        case let .project(id):
            onSelectSession(id, sessionID)
        case let .localApp(id):
            onSelectAppSession(id, sessionID)
        }
    }

    private func startNewConversation(in scope: ConversationScope) {
        if let mode = section.sessionMode {
            onModeChanged?(mode)
        }
        switch scope {
        case .global:
            onNewChat(nil)
        case let .project(id):
            onNewChat(id)
        case let .localApp(id):
            onNewAppChat(id)
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
            if case let .failure(error) = result {
                projectStore.errorMessage = error.localizedDescription
            }
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
        case let .reauthorize(projectID):
            projectStore.reauthorize(projectId: projectID, directoryURL: url)
        }
    }
}

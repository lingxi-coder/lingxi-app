import SwiftUI
import UniformTypeIdentifiers

/// The real workspace drawer. Projects and scheduled tasks come from their
/// repositories; presets remain creation choices and are never rendered as
/// persisted rows.
struct Drawer: View {
    @Environment(\.theme) private var t
    @Bindable var projectStore: ProjectStore
    @Bindable var cronRepository: CronRepository
    @Binding var activeSession: String

    let source: any ConversationSource
    let onClose: () -> Void
    let openSettings: () -> Void
    let openTerminal: () -> Void
    let openCron: (String?, String?) -> Void
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

    private enum Section: String { case chats, projects, crons }
    private enum FolderPickerMode { case importProject, reauthorize(String) }

    init(
        projectStore: ProjectStore,
        cronRepository: CronRepository,
        activeSession: Binding<String>,
        source: any ConversationSource,
        onClose: @escaping () -> Void,
        openSettings: @escaping () -> Void,
        openTerminal: @escaping () -> Void,
        openCron: @escaping (String?, String?) -> Void,
        onSelectProject: @escaping (String?) -> Void,
        onSelectSession: @escaping (String?, String) -> Void,
        onNewChat: @escaping (String?) -> Void
    ) {
        self.projectStore = projectStore
        self.cronRepository = cronRepository
        _activeSession = activeSession
        self.source = source
        self.onClose = onClose
        self.openSettings = openSettings
        self.openTerminal = openTerminal
        self.openCron = openCron
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

    private var cronTasks: [CronScopedTask] {
        cronRepository.state.tasks.filter {
            matches($0.scope.projectName, $0.task.prompt, $0.task.cron, $0.task.human)
        }
    }

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var projectOperationInFlight: Bool {
        projectStore.operation != nil
    }

    var body: some View {
        ZStack(alignment: .leading) {
            Button(action: onClose) { Color.black.opacity(0.4).ignoresSafeArea() }
                .buttonStyle(.plain)
                .accessibilityLabel("关闭抽屉")

            panel
                .frame(width: 330)
                .frame(maxHeight: .infinity)
                .background(t.sidebarBg)
                .overlay(Rectangle().frame(width: 0.5).foregroundColor(t.border), alignment: .trailing)
                .shadow(color: .black.opacity(0.3), radius: 15, x: 8)
                .transition(.move(edge: .leading))
                .onAppear {
                    source.listSessions()
                    if let active = projectStore.activeProjectId { openProjects.insert(active) }
                    Task { await cronRepository.refresh() }
                }
        }
        .alert("新建本地项目", isPresented: $showCreateAlert) {
            TextField("项目名称", text: $createProjectName)
            Button("创建") { createInternalProject() }
                .disabled(createProjectName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("取消", role: .cancel) {}
        } message: {
            Text("工作区会保存在 App 管理目录中。")
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
            Color.clear.frame(height: 54)
            header
            scopePills
            searchBar
            sectionTabs
            sectionBody
            shortcuts
            accountRow
        }
    }

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("灵犀").font(.system(size: 17, weight: .bold)).foregroundColor(t.text)
                Text(projectStore.activeProject?.record.name ?? "全局会话")
                    .font(.caption).foregroundColor(t.text4).lineLimit(1)
            }
            Spacer()
            Button(action: onClose) {
                LXIcon(name: .x, size: 20, color: t.text3, stroke: 1.8).frame(width: 36, height: 36)
            }
            .accessibilityLabel("关闭抽屉")
        }
        .padding(.horizontal, 18).padding(.top, 8).padding(.bottom, 12)
    }

    private var scopePills: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                scopePill(id: nil, name: "全局", icon: "◎")
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
        .accessibilityIdentifier(id == nil ? "drawer.scope.global" : "drawer.scope.project.\(name)")
        .accessibilityValue(active ? "当前项目" : "")
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            LXIcon(name: .search, size: 16, color: t.text4, stroke: 2)
            TextField("搜索会话、项目或定时任务", text: $query)
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
                .accessibilityLabel("清除搜索")
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
            tab(.chats, .message, "对话", engineSessions.count)
            tab(.projects, .folder, "项目", projects.count)
            tab(.crons, .clock, "定时", cronTasks.count)
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
                    case .crons: cronsSection
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
        case .crons: cronTasks.isEmpty
        }
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            LXIcon(name: searching ? .search : section == .projects ? .folder : section == .crons ? .clock : .message,
                   size: 24, color: t.text4, stroke: 1.8)
            Text(searching ? "没有匹配结果" : emptyCopy)
                .font(.system(size: 13)).foregroundColor(t.text4).multilineTextAlignment(.center)
            if !searching, section == .chats { newChatButton(projectID: projectStore.activeProjectId) }
            if !searching, section == .projects {
                projectCreationMenu
            }
            if !searching, section == .crons {
                dashedButton("新建定时任务") {
                    openCron(projectStore.activeProjectId ?? globalCronScopeID, nil)
                }
            }
        }
        .frame(maxWidth: .infinity).padding(.top, 38).padding(.horizontal, 10)
    }

    private var emptyCopy: String {
        switch section {
        case .chats: return "当前作用域暂无会话"
        case .projects: return "还没有真实项目工作区"
        case .crons: return "还没有定时任务"
        }
    }

    private var chatsSection: some View {
        VStack(spacing: 2) {
            if !searching { newChatButton(projectID: projectStore.activeProjectId) }
            ForEach(engineSessions) { session in
                sessionButton(session.id, title: session.title, subtitle: "\(session.relativeTime) · \(session.messageCount) 条", projectID: projectStore.activeProjectId)
            }
        }
    }

    private func newChatButton(projectID: String?) -> some View {
        Button {
            onNewChat(projectID)
            onClose()
        } label: {
            Label("新对话", systemImage: "square.and.pencil")
                .font(.system(size: 13.5, weight: .medium)).foregroundColor(t.accent)
                .frame(maxWidth: .infinity, alignment: .leading).padding(10)
                .background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10))
        }
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
            Button("新建本地项目") {
                createProjectName = ""
                showCreateAlert = true
            }
            Button("导入外部文件夹") {
                pickerMode = .importProject
                showFolderPicker = true
            }
        } label: {
            Label("新建或导入项目", systemImage: "plus")
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
            if expanded {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(project.sessions.filter { matches($0.title, $0.relativeTime) }) { session in
                        sessionButton(session.sessionId, title: session.title,
                                      subtitle: "\(session.relativeTime) · \(session.messageCount) 条", projectID: project.id)
                    }
                    Button {
                        onNewChat(project.id)
                        onClose()
                    } label: {
                        Label("项目新会话", systemImage: "plus").font(.caption).foregroundColor(t.text3).padding(8)
                    }
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
                Button("导入更新") { projectStore.reimport(projectId: project.id) }
                    .buttonStyle(.bordered)
                Button("同步回目录") { projectStore.export(projectId: project.id) }
                    .buttonStyle(.bordered)
                if project.record.syncState == .authorizationLost {
                    Button("重新授权") {
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
            Text("\(count) 个文件冲突").font(.caption).foregroundColor(t.danger)
            HStack {
                Button("保留设备版本") { projectStore.resolveConflicts(projectId: project.id, resolution: .keepInternal) }
                Button("保留外部版本") { projectStore.resolveConflicts(projectId: project.id, resolution: .keepExternal) }
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
            onClose()
        } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 13.5, weight: active ? .semibold : .medium)).foregroundColor(active ? t.text : t.text2).lineLimit(1)
                Text(subtitle).font(.caption).foregroundColor(t.text4).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding(9)
            .background(active ? t.surfaceActive : .clear).clipShape(RoundedRectangle(cornerRadius: 8))
        }
    }

    private var cronsSection: some View {
        VStack(spacing: 8) {
            ForEach(cronTasks) { scoped in
                Button {
                    openCron(scoped.scope.scopeID, scoped.task.id)
                    onClose()
                } label: {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Circle().fill(scoped.activeRun == nil ? t.accent : Color.orange).frame(width: 8, height: 8)
                            Text(scoped.task.prompt.split(whereSeparator: \.isNewline).first.map(String.init) ?? scoped.task.id)
                                .font(.system(size: 14, weight: .semibold)).foregroundColor(t.text).lineLimit(1)
                            Spacer()
                            Text(scoped.scope.projectName).font(.caption).foregroundColor(t.text4)
                        }
                        Text(scoped.task.human).font(.system(size: 12, design: .monospaced)).foregroundColor(t.text3)
                        Text(scoped.lastRun.map { "最近：\($0.status.label)" } ?? "尚未运行")
                            .font(.caption).foregroundColor(t.text4)
                    }
                    .padding(12).background(t.surface).clipShape(RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
                }
                .buttonStyle(.plain)
            }
            if !searching {
                dashedButton("新建定时任务") {
                    openCron(projectStore.activeProjectId ?? globalCronScopeID, nil)
                }
            }
        }
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

    private var shortcuts: some View {
        HStack(spacing: 4) {
            Button(action: openTerminal) { shortcut(.workflow, "终端") }
                .accessibilityIdentifier("drawer.shortcut.terminal")
            Button { openCron(nil, nil) } label: { shortcut(.clock, "定时任务") }
                .accessibilityIdentifier("drawer.shortcut.cron")
        }
        .padding(.horizontal, 12).padding(.top, 4)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .top)
    }

    private func shortcut(_ icon: LXIconName, _ label: String) -> some View {
        HStack(spacing: 8) {
            LXIcon(name: icon, size: 15, color: t.text3, stroke: 1.7)
            Text(label).font(.system(size: 13.5, weight: .medium)).foregroundColor(t.text3)
        }
        .frame(maxWidth: .infinity).padding(.vertical, 11)
    }

    private var accountRow: some View {
        Button(action: openSettings) {
            HStack(spacing: 12) {
                Circle().fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 36, height: 36)
                    .overlay(LXIcon(name: .cog, size: 16, color: .white, stroke: 1.8))
                Text("设置").font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                Spacer()
                LXIcon(name: .chevronR, size: 14, color: t.text4, stroke: 1.7)
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
        }
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

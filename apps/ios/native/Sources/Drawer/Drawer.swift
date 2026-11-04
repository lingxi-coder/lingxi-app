import SwiftUI

/// The signed-in profile shown in the sidebar's bottom bar.
struct DrawerAccountSummary: Equatable, Sendable {
    let displayName: String
    let email: String?
}

/// The sidebar: three entries (scheduled tasks, projects, ...), one recent-
/// sessions list with a mode filter, and the account / terminal / settings bar.
///
/// Mode is a property of a session, not a place: rows from both modes live in
/// the same list and opening one carries its mode (and workspace) along.
struct Drawer: View {
    @Environment(\.theme) private var t
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Bindable var projectStore: ProjectStore
    let activeScope: ConversationScope
    @Binding var activeSession: String

    let source: any ConversationSource
    let cronState: CronRepositoryState
    let sessionPinnedAt: [String: Date]
    let newCronResults: Int
    let account: DrawerAccountSummary?
    let openSettings: () -> Void
    let openAccount: () -> Void
    let openRemoteControl: () -> Void
    let openTerminal: () -> Void
    let openCron: (String?, String?) -> Void
    let openProjects: () -> Void
    let openProject: (String) -> Void
    let closeSidebar: () -> Void
    let onOpenSession: (ConversationScope, SessionMode, String) -> Void
    let onToggleSessionPinned: (String) -> Void
    let onContinueInMode: (ConversationScope, String, SessionMode, SessionMode) -> Void
    /// The independent app library remains available without a chat engine.
    let appLibrary: UnifiedLocalApps?
    let openApps: () -> Void
    let openTeams: () -> Void
    let openApp: (String) -> Void

    @ObservedObject private var convo: ConversationModel
    private let appInfo: InstalledAppInfoStore
    @Binding private var filter: RecentsFilter
    @State private var query = ""
    @FocusState private var searchFocused: Bool

    init(
        projectStore: ProjectStore,
        activeScope: ConversationScope = .global,
        activeSession: Binding<String>,
        source: any ConversationSource,
        cronState: CronRepositoryState = .init(),
        filter: Binding<RecentsFilter>,
        sessionPinnedAt: [String: Date] = [:],
        newCronResults: Int = 0,
        account: DrawerAccountSummary? = nil,
        openSettings: @escaping () -> Void,
        openAccount: @escaping () -> Void = {},
        openRemoteControl: @escaping () -> Void = {},
        openTerminal: @escaping () -> Void,
        openCron: @escaping (String?, String?) -> Void = { _, _ in },
        openProjects: @escaping () -> Void = {},
        openProject: @escaping (String) -> Void = { _ in },
        closeSidebar: @escaping () -> Void,
        onOpenSession: @escaping (ConversationScope, SessionMode, String) -> Void,
        onToggleSessionPinned: @escaping (String) -> Void = { _ in },
        onContinueInMode: @escaping (ConversationScope, String, SessionMode, SessionMode) -> Void = { _, _, _, _ in },
        appLibrary: UnifiedLocalApps? = nil,
        appInfo: InstalledAppInfoStore,
        openApps: @escaping () -> Void = {},
        openTeams: @escaping () -> Void = {},
        openApp: @escaping (String) -> Void = { _ in }
    ) {
        self.projectStore = projectStore
        self.activeScope = activeScope
        _activeSession = activeSession
        self.source = source
        self.cronState = cronState
        _filter = filter
        self.sessionPinnedAt = sessionPinnedAt
        self.newCronResults = newCronResults
        self.account = account
        self.openSettings = openSettings
        self.openAccount = openAccount
        self.openRemoteControl = openRemoteControl
        self.openTerminal = openTerminal
        self.openCron = openCron
        self.openProjects = openProjects
        self.openProject = openProject
        self.closeSidebar = closeSidebar
        self.onOpenSession = onOpenSession
        self.onToggleSessionPinned = onToggleSessionPinned
        self.onContinueInMode = onContinueInMode
        self.appLibrary = appLibrary
        self.appInfo = appInfo
        self.openApps = openApps
        self.openTeams = openTeams
        self.openApp = openApp
        convo = source.model
    }

    // MARK: derived state

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Every workspace's session catalog. Scheduled-run sessions are never
    /// listed here — their results live under 定时任务.
    private var seeds: [WorkspaceGroupSeed] {
        let globalSeed = WorkspaceGroupSeed(
            scope: .global,
            kind: .global,
            title: String(localized: "common_global"),
            subtitle: nil,
            updatedAt: nil,
            sessions: WorkspaceGroupBuilder.mergedSessionRows(
                live: activeScope == .global ? convo.engineSessions : [],
                cached: projectStore.globalSessions
            )
        )
        let projectSeeds = projectStore.projects.map { project in
            let scope = ConversationScope.project(project.id)
            return WorkspaceGroupSeed(
                scope: scope,
                kind: .project,
                title: project.record.name,
                subtitle: project.record.syncState.label,
                updatedAt: project.record.updatedAt,
                sessions: WorkspaceGroupBuilder.mergedSessionRows(
                    live: activeScope == scope ? convo.engineSessions : [],
                    cached: project.sessions
                )
            )
        }
        return [globalSeed] + projectSeeds
    }

    private var awaitingConfirmation: Bool {
        var awaiting = !convo.pendingQuestions.isEmpty
        #if canImport(harness_runtimeFFI)
            awaiting = awaiting || !convo.pendingPermissions.isEmpty
        #endif
        return awaiting
    }

    /// Only the bound session can be running or waiting for the user.
    private func status(of sessionID: String) -> RecentStatus {
        guard !sessionID.isEmpty, sessionID == activeSession else { return .idle }
        if awaitingConfirmation { return .awaitingConfirmation }
        return convo.streaming ? .running : .idle
    }

    private var groups: [RecentGroup] {
        RecentSessionsBuilder.groups(
            seeds: seeds,
            // The chips are hidden while searching, so they must not narrow it.
            filter: searching ? .all : filter,
            query: query,
            pinnedAt: sessionPinnedAt,
            statusFor: status(of:)
        )
    }

    private var matchingProjects: [ProjectSnapshot] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return [] }
        return RecentSessionsBuilder.sortedProjects(projectStore.projects, pinnedAt: [:])
            .filter { $0.record.name.localizedStandardContains(needle) }
    }

    private var installedApps: [MyApp] {
        AppsModel.myApps(appLibrary?.records ?? [], info: appInfo.apps)
    }

    private var matchingCrons: [CronScopedTask] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return [] }
        return cronState.tasks.filter {
            $0.task.prompt.localizedStandardContains(needle)
                || $0.task.human.localizedStandardContains(needle)
                || $0.scope.projectName.localizedStandardContains(needle)
        }
    }

    // MARK: body

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 6) {
                    if searching {
                        searchResults
                    } else {
                        entries
                        filterChips
                        recents
                    }
                }
                .padding(.horizontal, 12)
                .padding(.top, 4)
                .padding(.bottom, 8)
            }
            .frame(maxHeight: .infinity)
            bottomBar
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { t.sidebarBg.ignoresSafeArea() }
        .onAppear { source.listSessions() }
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
    }

    // MARK: search

    private var searchBar: some View {
        HStack(spacing: 8) {
            LXIcon(name: .search, size: 16, color: t.text4, stroke: 2)
            TextField("drawer_search_all_placeholder", text: $query)
                .font(.body)
                .foregroundColor(t.text)
                .focused($searchFocused)
                .submitLabel(.search)
                .autocorrectionDisabled()
                .accessibilityIdentifier("drawer.search")
            if !query.isEmpty {
                Button {
                    query = ""
                    searchFocused = false
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(t.text2)
                        .frame(minWidth: 44, minHeight: 44)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "drawer_clear_search_a11y"))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(searchFocused ? t.accent.opacity(0.5) : t.border, lineWidth: 0.5))
        .padding(.horizontal, 18)
        .padding(.top, 8)
        .padding(.bottom, 12)
    }

    @ViewBuilder
    private var searchResults: some View {
        let sessionGroups = groups
        let sessions = sessionGroups.flatMap(\.sessions)
        let projects = matchingProjects
        let crons = matchingCrons
        let apps = AppsModel.matchingApps(installedApps, query: query)
        if sessions.isEmpty && apps.isEmpty && projects.isEmpty && crons.isEmpty {
            emptyState(text: String(localized: "drawer_no_match"), icon: .search)
        }
        if !sessions.isEmpty {
            sectionHeader(String(localized: "drawer_search_section_sessions"))
            ForEach(sessions) { sessionRow($0) }
        }
        if !apps.isEmpty {
            sectionHeader(String(localized: "apps_title"))
            ForEach(apps) { app in
                Button { openApp(app.appId) } label: {
                    HStack(spacing: 10) {
                        AppCover(name: app.name, category: app.category, size: 22)
                        Text(app.name)
                            .font(.footnote.weight(.medium))
                            .foregroundColor(t.text2)
                            .lineLimit(1)
                        Spacer()
                    }
                    .padding(9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.app.\(app.appId)")
            }
        }
        if !projects.isEmpty {
            sectionHeader(String(localized: "drawer_entry_projects"))
            ForEach(projects) { project in
                Button { openProject(project.id) } label: {
                    HStack(spacing: 10) {
                        LXIcon(name: .folder, size: 15, color: t.text3, stroke: 1.8)
                        Text(project.record.name)
                            .font(.footnote.weight(.medium))
                            .foregroundColor(t.text2)
                            .lineLimit(1)
                        Spacer()
                    }
                    .padding(9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.project.\(project.id)")
            }
        }
        if !crons.isEmpty {
            sectionHeader(String(localized: "settings_title_cron"))
            ForEach(crons) { cron in
                Button { openCron(cron.scope.scopeID, cron.task.id) } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(cron.task.prompt)
                            .font(.footnote.weight(.medium))
                            .foregroundColor(t.text2)
                            .lineLimit(1)
                        Text(cron.task.human).font(.caption).foregroundColor(t.text4).lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.cron.\(cron.id)")
            }
        }
    }

    // MARK: entries

    private var entries: some View {
        VStack(spacing: 2) {
            Button(action: openRemoteControl) {
                Label("remote_title", systemImage: "desktopcomputer")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20).padding(.vertical, 10)
            }.buttonStyle(.plain).accessibilityIdentifier("drawer.remote-control")

            appsEntry
            entryRow(
                icon: .clock,
                title: String(localized: "settings_title_cron"),
                detail: newCronResults > 0
                    ? String(localized: "drawer_cron_new_results \(newCronResults)")
                    : nil,
                highlighted: newCronResults > 0,
                identifier: "drawer.entry.cron"
            ) { openCron(nil, nil) }
            entryRow(
                icon: .folder,
                title: String(localized: "drawer_entry_projects"),
                detail: projectStore.projects.isEmpty ? nil : "\(projectStore.projects.count)",
                highlighted: false,
                identifier: "drawer.entry.projects"
            ) { openProjects() }
        }
        .padding(.bottom, 6)
    }

    /// 应用, with the last apps opened one tap away.
    private var appsEntry: some View {
        VStack(alignment: .leading, spacing: 4) {
        Button(action: openTeams) {
            Label("teams_title", systemImage: "person.2").font(.subheadline.weight(.medium)).frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 11)
        }.buttonStyle(.plain).accessibilityIdentifier("drawer.entry.teams")
        HStack(spacing: 8) {
            Button(action: openApps) {
                HStack(spacing: 12) {
                    Image(systemName: "square.grid.2x2")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(t.text2)
                        .frame(width: 22)
                    Text("apps_title").font(.subheadline.weight(.medium)).foregroundColor(t.text)
                    Spacer(minLength: 8)
                }
                .padding(.vertical, 11)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("drawer.entry.apps")
            ForEach(AppsModel.recentApps(installedApps)) { app in
                Button { openApp(app.appId) } label: {
                    AppCover(name: app.name, category: app.category, size: 26)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(app.name)
                .accessibilityIdentifier("drawer.recent-app.\(app.appId)")
            }
            Button(action: openApps) {
                LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 1.7)
                    .padding(.vertical, 11)
            }
            .buttonStyle(.plain)
            .accessibilityHidden(true)
        }
        }.padding(.horizontal, 10)
    }

    private func entryRow(
        icon: LXIconName,
        title: String,
        detail: String?,
        highlighted: Bool,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                LXIcon(name: icon, size: 17, color: t.text2, stroke: 1.8).frame(width: 22)
                Text(title).font(.subheadline.weight(.medium)).foregroundColor(t.text)
                Spacer(minLength: 8)
                if let detail {
                    Text(detail)
                        .font(.caption.weight(highlighted ? .semibold : .regular))
                        .foregroundColor(highlighted ? t.accent : t.text4)
                }
                LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 1.7)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 11)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(identifier)
    }

    // MARK: recents

    private var filterChips: some View {
        HStack(spacing: 6) {
            ForEach(RecentsFilter.allCases, id: \.self) { chip in
                let selected = filter == chip
                Button {
                    filter = chip
                } label: {
                    Text(chip.title)
                        .font(.footnote.weight(selected ? .semibold : .medium))
                        .foregroundColor(selected ? t.text : t.text3)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(selected ? t.surfaceActive : .clear, in: Capsule())
                        .overlay(Capsule().stroke(selected ? t.borderStrong : t.border, lineWidth: 0.6))
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.filter.\(chip.rawValue)")
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
            Spacer()
        }
        .padding(.horizontal, 6)
        .padding(.bottom, 4)
    }

    @ViewBuilder
    private var recents: some View {
        let sections = groups
        if sections.isEmpty {
            emptyState(text: String(localized: "drawer_no_sessions"), icon: .message)
        }
        ForEach(sections) { section in
            sectionHeader(section.bucket.title)
            ForEach(section.sessions) { sessionRow($0) }
        }
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title)
            .font(.caption.weight(.semibold))
            .foregroundColor(t.text4)
            .padding(.horizontal, 10)
            .padding(.top, 10)
            .padding(.bottom, 2)
    }

    private func emptyState(text: String, icon: LXIconName) -> some View {
        VStack(spacing: 10) {
            LXIcon(name: icon, size: 24, color: t.text4, stroke: 1.8)
            Text(text)
                .font(.footnote)
                .foregroundColor(t.text4)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 24)
    }

    private func sessionRow(_ row: RecentSession) -> some View {
        let active = row.scope == activeScope && row.id == activeSession
        let targetMode: SessionMode = row.mode == .chat ? .code : .chat
        return Button {
            onOpenSession(row.scope, row.mode, row.id)
        } label: {
            HStack(spacing: 10) {
                row.mode.glyph(t, size: 15)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(row.title)
                            .font(.footnote.weight(active ? .semibold : .medium))
                            .foregroundColor(active ? t.text : t.text2)
                            .lineLimit(1)
                        if row.pinnedAt != nil {
                            Image(systemName: "pin.fill")
                                .font(.caption.weight(.semibold))
                                .foregroundColor(t.text4)
                        }
                    }
                    Text("\(row.mode.title) · \(row.workspaceName) · \(row.relativeTime)")
                        .font(.caption)
                        .foregroundColor(t.text4)
                        .lineLimit(1)
                }
                Spacer(minLength: 6)
                statusMarker(row.status)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(9)
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(String(localized: row.pinnedAt == nil ? "drawer_session_pin" : "drawer_session_unpin")) {
                onToggleSessionPinned(row.id)
            }
            Button(
                String(localized: targetMode == .chat ? "drawer_session_continue_chat" : "drawer_session_continue_code")
            ) {
                onContinueInMode(row.scope, row.id, row.mode, targetMode)
            }
        }
        .accessibilityIdentifier("drawer.session.\(row.workspaceKey).\(row.id)")
    }

    @ViewBuilder
    private func statusMarker(_ status: RecentStatus) -> some View {
        switch status {
        case .idle:
            EmptyView()
        case .running:
            HStack(spacing: 4) {
                ProgressView().controlSize(.mini)
                Text("chat_status_running").font(.caption.weight(.medium)).foregroundColor(t.accent)
            }
        case .awaitingConfirmation:
            Text("drawer_status_awaiting")
                .font(.caption.weight(.semibold))
                .foregroundColor(t.statusTesting)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(t.statusTesting.opacity(0.15), in: Capsule())
        }
    }

    // MARK: bottom bar

    private var bottomBar: some View {
        HStack(spacing: 4) {
            Button(action: openAccount) {
                HStack(spacing: 12) {
                    Circle()
                        .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                        .frame(width: 36, height: 36)
                        .overlay {
                            if let initial = account?.displayName.trimmingCharacters(in: .whitespaces).first {
                                Text(String(initial).uppercased())
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundColor(.white)
                            } else {
                                LXIcon(name: .sparkle, size: 16, color: .white, stroke: 1.8)
                            }
                        }
                    VStack(alignment: .leading, spacing: 1) {
                        Text(account?.displayName ?? String(localized: "drawer_account_signed_out_title"))
                            .font(.subheadline.weight(.medium))
                            .foregroundColor(t.text)
                            .lineLimit(1)
                        Text(account?.email ?? String(localized: "drawer_account_signed_out_subtitle"))
                            .font(.caption)
                            .foregroundColor(t.text4)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("drawer.account")
            bottomIconButton(icon: .terminal, label: "settings_linux_section_terminal", identifier: "drawer.terminal", action: openTerminal)
            bottomIconButton(icon: .cog, label: "settings_title_main", identifier: "drawer.settings", action: openSettings)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .top)
    }

    private func bottomIconButton(
        icon: LXIconName,
        label: LocalizedStringKey,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            LXIcon(name: icon, size: 20, color: t.text2, stroke: 1.7)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityIdentifier(identifier)
    }
}

extension RecentsFilter {
    var title: String {
        switch self {
        case .all: String(localized: "drawer_filter_all")
        case .chat: String(localized: "session_mode_chat")
        case .code: String(localized: "session_mode_code")
        }
    }
}

extension RecentBucket {
    var title: String {
        switch self {
        case .pinned: String(localized: "drawer_recents_pinned")
        case .today: String(localized: "drawer_recents_today")
        case .yesterday: String(localized: "session_relative_yesterday")
        case .lastWeek: String(localized: "drawer_recents_week")
        case .earlier: String(localized: "drawer_recents_earlier")
        }
    }
}

extension SessionMode {
    var title: String {
        switch self {
        case .chat: String(localized: "session_mode_chat")
        case .code: String(localized: "session_mode_code")
        }
    }

    /// The mode's identifying colour: accent for Chat, the teal accent for Code.
    func color(_ palette: Palette) -> Color {
        switch self {
        case .chat: palette.accent
        case .code: palette.accent3
        }
    }

    @ViewBuilder
    func glyph(_ palette: Palette, size: CGFloat) -> some View {
        LXIcon(name: self == .chat ? .message : .terminal, size: size, color: color(palette), stroke: 1.8)
            .frame(width: size + 4, height: size + 4)
    }
}

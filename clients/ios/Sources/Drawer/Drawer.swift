import SwiftUI

// MARK: - Drawer (slide-in left panel)
struct Drawer: View {
    @Environment(\.theme) private var t
    @Binding var activeWs: String
    @Binding var activeSession: String
    let onClose: () -> Void
    let openSettings: () -> Void
    /// The conversation source — drives REAL session history (`engineSessions`)
    /// and the resume/new-chat actions. The drawer reads `source.model` for the
    /// live session list (an `@ObservedObject`) and calls `source.resumeSession`
    /// / `source.startNewConversation` on the user's choice.
    let source: any ConversationSource
    /// Triggered when the user picks the REAL engine session `uuid` (so the
    /// parent can mirror the selection into its own `activeSession` state). The
    /// drawer also drives `source.resumeSession` itself; this lets RootView keep
    /// its title-bar / @AppStorage in sync.
    let onSelectEngineSession: (String) -> Void
    /// Triggered for "New chat" so the parent can reset its own session state in
    /// step with `source.startNewConversation`.
    let onNewChat: () -> Void

    @ObservedObject private var convo: ConversationModel

    init(activeWs: Binding<String>,
         activeSession: Binding<String>,
         source: any ConversationSource,
         onClose: @escaping () -> Void,
         openSettings: @escaping () -> Void,
         onSelectEngineSession: @escaping (String) -> Void,
         onNewChat: @escaping () -> Void) {
        self._activeWs = activeWs
        self._activeSession = activeSession
        self.source = source
        self.onClose = onClose
        self.openSettings = openSettings
        self.onSelectEngineSession = onSelectEngineSession
        self.onNewChat = onNewChat
        self.convo = source.model
    }

    private enum Section: String { case chats, projects, crons }
    @State private var section: Section = .chats
    @State private var openProjects: Set<String> = ["p1"]
    /// The live drawer search query (real `TextField` — was a static label). It
    /// filters the chats / projects / crons lists below by a case-insensitive
    /// substring over the visible fields.
    @State private var query: String = ""
    @FocusState private var searchFocused: Bool

    /// REAL engine sessions, newest-first, filtered by the search query. When
    /// non-empty the chats section renders these in place of the mock chats; when
    /// empty (engine unavailable / no history) the drawer falls back to MockData.
    private var engineSessions: [EngineSession] {
        convo.engineSessions.filter { matches($0.title, $0.relativeTime) }
    }
    /// True when the engine has reported real history — drives the chats section
    /// between the real list and the MockData fallback.
    private var hasEngineSessions: Bool { !convo.engineSessions.isEmpty }

    /// `s` trimmed + lowercased contains the trimmed query (empty query ⇒ match
    /// everything). The shared predicate every section filter runs through.
    private func matches(_ haystacks: String...) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return true }
        return haystacks.contains { $0.lowercased().contains(q) }
    }

    private var chats: [Chat] {
        MockData.chats.filter { $0.wsId == activeWs && matches($0.title, $0.preview, $0.activity) }
    }
    /// A project matches when its own name/desc match OR any of its sessions do;
    /// when only sessions match we still show the project (so the row is reachable).
    private var projects: [Project] {
        MockData.projects.filter { p in
            p.wsId == activeWs &&
            (matches(p.name, p.desc) || p.sessions.contains { matches($0.title, $0.preview, $0.activity) })
        }
    }
    private var crons: [Cron] {
        MockData.crons.filter { $0.wsId == activeWs && matches($0.title, $0.cron, $0.next, $0.desc) }
    }

    /// True while the user is actively searching — drives the "no results" copy.
    private var searching: Bool { !query.trimmingCharacters(in: .whitespaces).isEmpty }

    var body: some View {
        ZStack(alignment: .leading) {
            Color.black.opacity(0.4)
                .ignoresSafeArea()
                .onTapGesture { onClose() }

            panel
                .frame(width: 320)
                .frame(maxHeight: .infinity)
                .background(t.sidebarBg)
                .overlay(Rectangle().frame(width: 0.5).foregroundColor(t.border), alignment: .trailing)
                .shadow(color: .black.opacity(0.3), radius: 15, x: 8)
                .transition(.move(edge: .leading))
                // Refresh the REAL session catalog when the drawer opens so the
                // chats list reflects sessions created since the last pull. A
                // no-op on the mock; the engine re-submits `ListSessions`.
                .onAppear { source.listSessions() }
        }
    }

    private var panel: some View {
        VStack(spacing: 0) {
            Color.clear.frame(height: 54) // status bar offset
            header
            workspacePills
            searchBar
            sectionTabs
            sectionBody
            shortcuts
            accountRow
        }
    }

    // MARK: header
    private var header: some View {
        HStack(spacing: 10) {
            Text("灵犀").font(.system(size: 17, weight: .bold)).foregroundColor(t.text)
            Spacer()
            Button(action: onClose) {
                LXIcon(name: .x, size: 20, color: t.text3, stroke: 1.8).frame(width: 36, height: 36)
            }
            .accessibilityLabel("关闭抽屉")
        }
        .padding(.horizontal, 18).padding(.top, 8).padding(.bottom, 12)
    }

    // MARK: workspace pills
    private var workspacePills: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(MockData.workspaces) { w in
                    let active = w.id == activeWs
                    Button { activeWs = w.id } label: {
                        HStack(spacing: 5) { Text(w.icon); Text(w.name) }
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(active ? w.color : t.text3)
                            .padding(.horizontal, 12).padding(.vertical, 7)
                            .background(active ? w.color.tint(0.20) : t.surface)
                            .clipShape(Capsule())
                            .overlay(Capsule().stroke(active ? w.color.tint(0.35) : t.border, lineWidth: 0.5))
                    }
                }
            }
            .padding(.horizontal, 18)
        }
        .padding(.bottom, 12)
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            LXIcon(name: .search, size: 16, color: t.text4, stroke: 2)
                .accessibilityHidden(true)
            TextField("", text: $query,
                      prompt: Text("搜索会话").foregroundColor(t.text4))
                .font(.scaledSystem(14, relativeTo: .subheadline))
                .foregroundColor(t.text)
                .focused($searchFocused)
                .submitLabel(.search)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .accessibilityLabel("搜索会话")
            if !query.isEmpty {
                Button {
                    query = ""
                    searchFocused = false
                } label: {
                    LXIcon(name: .x, size: 14, color: t.text4, stroke: 2)
                        .frame(width: 22, height: 22)
                        .contentShape(Rectangle())
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

    // MARK: section tabs
    private var sectionTabs: some View {
        HStack(spacing: 4) {
            tab(.chats, .message, "对话", hasEngineSessions ? engineSessions.count : chats.count)
            tab(.projects, .folder, "项目", projects.count)
            tab(.crons, .clock, "定时", crons.count)
        }
        .padding(.horizontal, 14).padding(.bottom, 8)
    }

    private func tab(_ id: Section, _ icon: LXIconName, _ label: String, _ count: Int) -> some View {
        let active = section == id
        return Button { section = id } label: {
            HStack(spacing: 5) {
                LXIcon(name: icon, size: 14, color: active ? t.text : t.text3, stroke: 1.8)
                Text(label).font(.system(size: 13, weight: active ? .semibold : .medium))
                    .foregroundColor(active ? t.text : t.text3)
                Text("\(count)").font(.system(size: 10.5, weight: .semibold))
                    .foregroundColor(active ? t.accent : t.text4)
            }
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 4).padding(.vertical, 9)
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).stroke(active ? t.border : .clear, lineWidth: 0.5))
        }
    }

    // MARK: section body
    private var sectionBody: some View {
        ScrollView(showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                if currentSectionEmpty && searching {
                    noResults
                } else {
                    switch section {
                    case .chats:    chatsSection
                    case .projects: projectsSection
                    case .crons:    cronsSection
                    }
                }
            }
            .padding(.horizontal, 12).padding(.top, 4).padding(.bottom, 8)
        }
        .frame(maxHeight: .infinity)
    }

    /// True when the active section has no rows under the current filter.
    private var currentSectionEmpty: Bool {
        switch section {
        case .chats:    return hasEngineSessions ? engineSessions.isEmpty : chats.isEmpty
        case .projects: return projects.isEmpty
        case .crons:    return crons.isEmpty
        }
    }

    /// A centered "no results" line shown when a search matches nothing in the
    /// active section (so the section doesn't render an empty body / lone "新建").
    private var noResults: some View {
        VStack(spacing: 8) {
            LXIcon(name: .search, size: 22, color: t.text4, stroke: 1.8)
                .accessibilityHidden(true)
            Text("未找到与“\(query.trimmingCharacters(in: .whitespaces))”匹配的结果")
                .font(.system(size: 13))
                .foregroundColor(t.text4)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 40).padding(.horizontal, 16)
    }

    @ViewBuilder
    private var chatsSection: some View {
        if hasEngineSessions {
            engineSessionsSection
        } else {
            mockChatsSection
        }
    }

    /// REAL engine history: a "New chat" affordance + one row per resumable
    /// session (newest-first, already filtered). Tapping a row resumes it; "New
    /// chat" starts a fresh session. Shown only when the engine has reported
    /// sessions (otherwise the mock list renders).
    private var engineSessionsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !searching { newChatButton }
            ForEach(engineSessions) { s in engineSessionRow(s) }
        }
        .padding(.top, 4)
    }

    /// "New chat" — submits `NewSession` (resets the transcript on
    /// `SessionStarted`) and lets the parent mirror the reset, then closes.
    private var newChatButton: some View {
        Button {
            source.startNewConversation()
            onNewChat()
            onClose()
        } label: {
            HStack(spacing: 8) {
                LXIcon(name: .plus, size: 14, color: t.accent, stroke: 2)
                Text("新对话").font(.system(size: 13.5, weight: .medium)).foregroundColor(t.accent)
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(t.surface)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
        }
        .accessibilityLabel("新对话")
        .padding(.horizontal, 2).padding(.bottom, 8)
    }

    private func engineSessionRow(_ s: EngineSession) -> some View {
        let active = s.id == convo.activeSessionId
        return Button {
            // Resume on the source AND select locally (so the UI reflects the
            // choice even if engine-side resume is still a follow-up).
            source.resumeSession(s.id)
            onSelectEngineSession(s.id)
            onClose()
        } label: {
            ZStack(alignment: .leading) {
                if active { Capsule().fill(t.accent).frame(width: 2.5).padding(.vertical, 12) }
                VStack(alignment: .leading, spacing: 3) {
                    Text(s.title).font(.scaledSystem(14, weight: active ? .semibold : .medium, relativeTo: .subheadline))
                        .foregroundColor(active ? t.text : t.text2).lineLimit(1)
                    Text("\(s.relativeTime) · \(s.messageCount) 条").font(.scaledSystem(12, relativeTo: .caption))
                        .foregroundColor(t.text4).lineLimit(1)
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 10))
        }
        .accessibilityLabel("\(s.title)，\(s.messageCount) 条消息，\(s.relativeTime)")
        .padding(.bottom, 2)
    }

    private var mockChatsSection: some View {
        let grouped = Dictionary(grouping: chats, by: { $0.group })
        let order = ["今天", "昨天", "本周"]
        return VStack(alignment: .leading, spacing: 0) {
            ForEach(order.filter { grouped[$0] != nil }, id: \.self) { group in
                VStack(alignment: .leading, spacing: 2) {
                    Text(group.uppercased())
                        .font(.system(size: 11, weight: .semibold)).tracking(0.6)
                        .foregroundColor(t.text4)
                        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 4)
                    ForEach(grouped[group] ?? []) { s in chatRow(s) }
                }
                .padding(.bottom, 8)
            }
            if !searching {
                (Text("临时对话 30 天后自动归档 · ") + Text("转为项目").foregroundColor(t.accent))
                    .font(.system(size: 11.5)).foregroundColor(t.text4).lineSpacing(4)
                    .padding(.horizontal, 14).padding(.top, 14).padding(.bottom, 4)
            }
        }
    }

    private func chatRow(_ s: Chat) -> some View {
        let active = s.id == activeSession
        return Button { activeSession = s.id; onClose() } label: {
            ZStack(alignment: .leading) {
                if active { Capsule().fill(t.accent).frame(width: 2.5).padding(.vertical, 12) }
                VStack(alignment: .leading, spacing: 3) {
                    Text(s.title).font(.scaledSystem(14, weight: active ? .semibold : .medium, relativeTo: .subheadline))
                        .foregroundColor(active ? t.text : t.text2).lineLimit(1)
                    Text("\(s.activity) · \(s.preview)").font(.scaledSystem(12, relativeTo: .caption))
                        .foregroundColor(t.text4).lineLimit(1)
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 10))
        }
        .padding(.bottom, 2)
    }

    private var projectsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(projects) { p in projectRow(p) }
            if !searching { dashedButton("新建项目") }
        }
        .padding(.top, 4)
    }

    private func projectRow(_ p: Project) -> some View {
        // While searching, auto-expand matched projects so the matching session
        // is visible, and narrow the session list to the matches (the project
        // name/desc matching still shows all its sessions).
        let nameMatches = matches(p.name, p.desc)
        let visibleSessions = searching && !nameMatches
            ? p.sessions.filter { matches($0.title, $0.preview, $0.activity) }
            : p.sessions
        let isOpen = searching ? true : openProjects.contains(p.id)
        let hasActive = p.sessions.contains { $0.id == activeSession }
        return VStack(alignment: .leading, spacing: 2) {
            Button {
                if openProjects.contains(p.id) { openProjects.remove(p.id) } else { openProjects.insert(p.id) }
            } label: {
                HStack(spacing: 10) {
                    LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 2)
                        .rotationEffect(.degrees(isOpen ? 90 : 0))
                        .accessibilityHidden(true)
                    projectIcon(p)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(p.name).font(.system(size: 14, weight: .semibold)).foregroundColor(t.text).lineLimit(1)
                        Text(p.desc).font(.system(size: 11.5)).foregroundColor(t.text4).lineLimit(1)
                    }
                    Spacer()
                    Text("\(p.sessions.count)").font(.system(size: 11, weight: .medium)).foregroundColor(t.text4)
                }
                .padding(.horizontal, 12).padding(.vertical, 10)
                .background(hasActive && !isOpen ? t.surfaceActive : .clear)
                .clipShape(RoundedRectangle(cornerRadius: 10))
            }
            .accessibilityLabel("\(p.name)，\(p.sessions.count) 个会话")
            .accessibilityHint(isOpen ? "收起项目" : "展开项目")
            if isOpen {
                ZStack(alignment: .leading) {
                    Rectangle().fill(t.border).frame(width: 1).padding(.vertical, 4).padding(.leading, 22)
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(visibleSessions) { s in sessionRow(p, s) }
                        if !searching {
                            HStack(spacing: 5) {
                                LXIcon(name: .plus, size: 11, color: t.text4, stroke: 2)
                                Text("新会话").font(.system(size: 12.5)).foregroundColor(t.text4)
                            }
                            .padding(.leading, 16).padding(.trailing, 12).padding(.vertical, 7)
                        }
                    }
                    .padding(.leading, 22)
                }
            }
        }
    }

    private func projectIcon(_ p: Project) -> some View {
        Text(p.icon).font(.system(size: 13, weight: .bold)).foregroundColor(p.color)
            .frame(width: 28, height: 28)
            .background(p.color.mix(with: t.surface, amount: 0.18))
            .clipShape(RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).stroke(p.color.tint(0.28), lineWidth: 0.5))
    }

    private func sessionRow(_ p: Project, _ s: ProjectSession) -> some View {
        let active = s.id == activeSession
        return Button { activeSession = s.id; onClose() } label: {
            ZStack(alignment: .leading) {
                if active { Capsule().fill(p.color).frame(width: 2.5).padding(.vertical, 8).offset(x: -3) }
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 5) {
                        if s.pinned { LXIcon(name: .pin, size: 11, color: p.color, stroke: 2.2) }
                        Text(s.title).font(.system(size: 13.5, weight: active ? .semibold : .medium))
                            .foregroundColor(active ? t.text : t.text2).lineLimit(1)
                    }
                    Text("\(s.activity) · \(s.msgs) 条").font(.system(size: 11.5)).foregroundColor(t.text4).lineLimit(1)
                }
                .padding(.leading, 16).padding(.trailing, 12).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.leading, 4)
            .background(active ? t.surfaceActive : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
    }

    private var cronsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(crons) { c in cronCard(c) }
            if !searching { dashedButton("新建定时任务") }
        }
        .padding(.top, 4)
    }

    private func cronCard(_ c: Cron) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Circle().fill(c.enabled ? t.accent : t.text4).frame(width: 8, height: 8)
                    .overlay(c.enabled ? Circle().stroke(t.accent.tint(0.18), lineWidth: 3).scaleEffect(1.6) : nil)
                Text(c.title).font(.system(size: 14, weight: .semibold)).foregroundColor(t.text).lineLimit(1)
                Spacer()
                LXIcon(name: c.enabled ? .pause : .play, size: 13, color: t.text4, stroke: 1.8)
            }
            .padding(.bottom, 5)
            Text(c.desc).font(.system(size: 12)).foregroundColor(t.text3).lineSpacing(2).padding(.bottom, 7)
            HStack(spacing: 7) {
                Text(c.cron).font(.system(size: 11, design: .monospaced)).fontWeight(.medium)
                    .foregroundColor(t.text2)
                    .padding(.horizontal, 7).padding(.vertical, 3)
                    .background(t.surfaceActive).clipShape(RoundedRectangle(cornerRadius: 5))
                Text("→").font(.system(size: 11, design: .monospaced)).foregroundColor(t.text4)
                Text(c.next).font(.system(size: 11, design: .monospaced)).fontWeight(.medium)
                    .foregroundColor(c.enabled ? t.accent : t.text4)
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
        .opacity(c.enabled ? 1 : 0.55)
    }

    private func dashedButton(_ label: String) -> some View {
        HStack(spacing: 6) {
            LXIcon(name: .plus, size: 13, color: t.text3, stroke: 2)
            Text(label).font(.system(size: 13)).foregroundColor(t.text3)
        }
        .frame(maxWidth: .infinity)
        .padding(11)
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, style: StrokeStyle(lineWidth: 1, dash: [4,3])))
        .padding(.horizontal, 4).padding(.top, 10)
    }

    // MARK: shortcuts + account
    private var shortcuts: some View {
        HStack(spacing: 4) {
            shortcut(.book, "知识库", 24)
            shortcut(.brain, "记忆", 42)
        }
        .padding(.horizontal, 12).padding(.top, 4)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .top)
    }

    private func shortcut(_ icon: LXIconName, _ label: String, _ count: Int) -> some View {
        HStack(spacing: 8) {
            LXIcon(name: icon, size: 15, color: t.text3, stroke: 1.7)
            Text(label).font(.system(size: 13.5, weight: .medium)).foregroundColor(t.text3)
            Spacer()
            Text("\(count)").font(.system(size: 11.5)).foregroundColor(t.text4)
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
    }

    private var accountRow: some View {
        VStack(spacing: 0) {
            Button(action: openSettings) {
                HStack(spacing: 12) {
                    Circle().fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                        .frame(width: 36, height: 36)
                        .overlay(Text("Y").font(.system(size: 14, weight: .semibold)).foregroundColor(.white))
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Yuxin Yang").font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                        Text("Pro · 5.5 / 8 段").font(.system(size: 11.5)).foregroundColor(t.text4)
                    }
                    Spacer()
                    LXIcon(name: .cog, size: 18, color: t.text3, stroke: 1.6)
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .overlay(Rectangle().frame(height: 0.5).foregroundColor(t.border), alignment: .top)
    }
}

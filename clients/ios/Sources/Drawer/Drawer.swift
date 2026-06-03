import SwiftUI

// MARK: - Drawer (slide-in left panel)
struct Drawer: View {
    @Environment(\.theme) private var t
    @Binding var activeWs: String
    @Binding var activeSession: String
    let onClose: () -> Void
    let openSettings: () -> Void

    private enum Section: String { case chats, projects, crons }
    @State private var section: Section = .chats
    @State private var openProjects: Set<String> = ["p1"]

    private var chats: [Chat] { MockData.chats.filter { $0.wsId == activeWs } }
    private var projects: [Project] { MockData.projects.filter { $0.wsId == activeWs } }
    private var crons: [Cron] { MockData.crons.filter { $0.wsId == activeWs } }

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
            Text("搜索会话").font(.system(size: 14)).foregroundColor(t.text4)
            Spacer()
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
        .padding(.horizontal, 18).padding(.bottom, 14)
    }

    // MARK: section tabs
    private var sectionTabs: some View {
        HStack(spacing: 4) {
            tab(.chats, .message, "对话", chats.count)
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
                switch section {
                case .chats:    chatsSection
                case .projects: projectsSection
                case .crons:    cronsSection
                }
            }
            .padding(.horizontal, 12).padding(.top, 4).padding(.bottom, 8)
        }
        .frame(maxHeight: .infinity)
    }

    private var chatsSection: some View {
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
            (Text("临时对话 30 天后自动归档 · ") + Text("转为项目").foregroundColor(t.accent))
                .font(.system(size: 11.5)).foregroundColor(t.text4).lineSpacing(4)
                .padding(.horizontal, 14).padding(.top, 14).padding(.bottom, 4)
        }
    }

    private func chatRow(_ s: Chat) -> some View {
        let active = s.id == activeSession
        return Button { activeSession = s.id; onClose() } label: {
            ZStack(alignment: .leading) {
                if active { Capsule().fill(t.accent).frame(width: 2.5).padding(.vertical, 12) }
                VStack(alignment: .leading, spacing: 3) {
                    Text(s.title).font(.system(size: 14, weight: active ? .semibold : .medium))
                        .foregroundColor(active ? t.text : t.text2).lineLimit(1)
                    Text("\(s.activity) · \(s.preview)").font(.system(size: 12))
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
            dashedButton("新建项目")
        }
        .padding(.top, 4)
    }

    private func projectRow(_ p: Project) -> some View {
        let isOpen = openProjects.contains(p.id)
        let hasActive = p.sessions.contains { $0.id == activeSession }
        return VStack(alignment: .leading, spacing: 2) {
            Button {
                if isOpen { openProjects.remove(p.id) } else { openProjects.insert(p.id) }
            } label: {
                HStack(spacing: 10) {
                    LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 2)
                        .rotationEffect(.degrees(isOpen ? 90 : 0))
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
            if isOpen {
                ZStack(alignment: .leading) {
                    Rectangle().fill(t.border).frame(width: 1).padding(.vertical, 4).padding(.leading, 22)
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(p.sessions) { s in sessionRow(p, s) }
                        HStack(spacing: 5) {
                            LXIcon(name: .plus, size: 11, color: t.text4, stroke: 2)
                            Text("新会话").font(.system(size: 12.5)).foregroundColor(t.text4)
                        }
                        .padding(.leading, 16).padding(.trailing, 12).padding(.vertical, 7)
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
            dashedButton("新建定时任务")
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

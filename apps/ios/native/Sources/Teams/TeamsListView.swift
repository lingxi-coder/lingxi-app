import SwiftUI

/// The root of the Teams tab (spec §6.1): the conversations of the selected team in three collapsible
/// groups, with the unread state, a one-line preview and the time of each. The data is the selected team's
/// `TeamMessagingModel`; the rules (grouping, order, labels, previews, search) are `TeamsListLogic`.
struct TeamsListView: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.locale) private var locale
    let uid: String
    let team: TeamSummary
    /// The conversation open beside the list (wide layout); highlighted. Empty on phones.
    var selectedConversation = ""
    /// Increases when the tab is tapped again at its root.
    var scrollToTop = 0
    let onOpen: (String) -> Void
    let onSwitchTeam: () -> Void
    let onNew: (TeamsSheet) -> Void

    @State private var query = ""
    private static let topID = "teams.list.top"

    @MainActor
    private struct Context {
        let model: TeamMessagingModel?
        let strings: TeamsListStrings
        let members: [TeamMember]
        let bots: [TeamBotCard]
        let labels: [String: String]
        let online: Bool
        func unread(_ conversation: TeamMessageConversation) -> Int { model?.unread(of: conversation) ?? 0 }
    }

    var body: some View {
        let model = workspace.messaging
        let strings = TeamsListStrings.current
        let members = workspace.directory
        let bots = model?.bots ?? []
        let all = model?.cache?.conversations ?? []
        let labels = Dictionary(all.map { ($0.id, TeamsListLogic.conversationLabel($0, members: members, uid: uid, bots: bots, strings: strings)) }, uniquingKeysWith: { first, _ in first })
        let context = Context(model: model, strings: strings, members: members, bots: bots, labels: labels, online: model.map { !$0.offline } ?? false)
        let visible = all.filter { TeamsListLogic.conversationMatches(label: labels[$0.id] ?? "", query: query) }
        let groups = TeamsListLogic.groupConversations(visible, uid: uid)
        let ready = model?.directoryReady ?? false
        return ScrollViewReader { proxy in
            List {
                Color.clear.frame(height: 1)
                    .listRowInsets(EdgeInsets()).listRowSeparator(.hidden).listRowBackground(Color.clear)
                    .id(Self.topID)
                    .accessibilityHidden(true)
                if !bots.isEmpty {
                    Section("teams_assistants") {
                        ForEach(bots.filter(\.selectable)) { bot in
                            Button {
                                let id = UUID().uuidString
                                Task {
                                    let done = await workspace.run { try await model?.create(kind: "bot_dm", title: bot.name, uids: [], sharedBotIds: [bot.sharedBotId], id: id) }
                                    if done { onOpen(id) }
                                }
                            } label: {
                                HStack {
                                    AgentAvatar(agentID: bot.sharedBotId, size: 32)
                                    VStack(alignment: .leading) {
                                        Text(bot.name).font(.body.weight(.semibold))
                                        Text(LocalizedStringKey(bot.ready ? "remote_online" : "teams_bot_offline_queued")).font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                            .accessibilityIdentifier("teams.assistant.\(bot.sharedBotId)")
                        }
                    }
                }
                if let model, !model.tasks.isEmpty {
                    Section("teams_recent_tasks") {
                        ForEach(model.tasks) { task in
                            TeamTaskCard(task: task, botName: bots.first { $0.sharedBotId == task.sharedBotId }?.name ?? strings.bot, viewer: TeamMemberRef(uid: uid, generation: team.generation),
                                         maintainer: bots.first { $0.sharedBotId == task.sharedBotId }?.maintainer,
                                         recipientName: task.wait.flatMap { wait in members.first { $0.uid == wait.recipient.uid }?.displayName } ?? strings.member)
                        }
                    }
                }
                ForEach((model?.cache?.commands ?? []).filter { $0.error != nil }) { command in
                    VStack(alignment: .leading, spacing: 6) {
                        Text(LocalizedStringKey(TeamsErrors.key(command.error ?? "INVALID_INPUT"))).font(.caption).foregroundStyle(.secondary)
                        Button("teams_message_discard") { Task { await workspace.run { try await model?.discardCommand(command.id) } } }
                    }
                }
                if let creation = model?.cache?.creation { creationRow(creation) }
                if !ready && all.isEmpty && (model?.error.isEmpty ?? true) {
                    HStack { Spacer(); ProgressView(); Spacer() }
                        .listRowSeparator(.hidden)
                        .accessibilityLabel(Text("teams_loading"))
                } else if all.isEmpty && model?.cache?.creation == nil {
                    emptyState.listRowSeparator(.hidden).listRowBackground(Color.clear)
                } else if visible.isEmpty {
                    TeamsStateView(systemImage: "magnifyingglass", title: "teams_no_matches")
                        .listRowSeparator(.hidden).listRowBackground(Color.clear)
                } else {
                    section(.channels, "teams_group_channels", groups.channels, context)
                    section(.direct, "teams_group_direct", groups.direct, context)
                    section(.bots, "teams_group_bot_dm", groups.botDirect, context)
                }
            }
            .listStyle(.plain)
            .searchable(text: $query, prompt: Text("teams_search_placeholder"))
            .safeAreaInset(edge: .top, spacing: 0) { banners }
            .refreshable {
                await workspace.refresh()
                workspace.messaging?.refresh()
            }
            .onChange(of: scrollToTop) { _, _ in
                withAnimation { proxy.scrollTo(Self.topID, anchor: .top) }
            }
        }
        .sheet(item: Binding(get: { workspace.messaging?.artifact }, set: { if $0 == nil { workspace.messaging?.closeArtifact() } })) { artifact in
            TeamArtifactViewer(artifact: artifact) { workspace.messaging?.closeArtifact() }
        }
        .navigationTitle("teams_title")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { teamChip }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    NewConversationItems(onNew: onNew)
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .disabled(workspace.messaging == nil)
                .accessibilityLabel(Text("teams_conversation_new"))
                .accessibilityIdentifier("teams.compose")
            }
        }
        .accessibilityIdentifier("teams.list")
    }

    /// The selected team and the way to another (spec §6.2): its name, a chevron, and the unread count of
    /// the other teams.
    private var teamChip: some View {
        let others = workspace.unreadByTeam.reduce(0) { $0 + ($1.key == team.id ? 0 : $1.value) }
        return Button(action: onSwitchTeam) {
            HStack(spacing: 6) {
                TeamAvatar(teamID: team.id, name: team.name, size: 24)
                Text(team.name).font(.subheadline.weight(.semibold)).lineLimit(1).frame(maxWidth: 150, alignment: .leading)
                Image(systemName: "chevron.up.chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                TeamsUnreadBadge(count: others)
            }
        }
        .accessibilityLabel(Text(verbatim: team.name))
        .accessibilityHint(Text("teams_switch_team"))
        .accessibilityIdentifier("teams.chip")
    }

    // MARK: Banners and states

    @ViewBuilder
    private var banners: some View {
        VStack(spacing: 0) {
            if let model = workspace.messaging {
                if !model.error.isEmpty {
                    TeamsBanner(kind: .error, title: LocalizedStringKey(TeamsErrors.key(model.error)), actionTitle: "mcp_reconnect") { workspace.reconnect() }
                        .accessibilityIdentifier("teams.banner.error")
                } else if model.directoryReady && model.fromCache {
                    TeamsBanner(kind: .offline, title: "teams_offline_title", detail: "teams_messages_offline", actionTitle: "mcp_reconnect") { workspace.reconnect() }
                        .accessibilityIdentifier("teams.banner.offline")
                }
                if !workspace.actionError.isEmpty, workspace.actionError != model.error {
                    TeamsBanner(kind: .error, title: LocalizedStringKey(TeamsErrors.key(workspace.actionError)), actionTitle: "common_close") { workspace.clearActionError() }
                }
            }
        }
    }

    private var emptyState: some View {
        TeamsStateView(systemImage: "bubble.left.and.bubble.right", title: "teams_no_conversations", message: "teams_no_conversations_hint") {
            Menu {
                NewConversationItems(onNew: onNew)
            } label: {
                Text("teams_conversation_new")
            }
            .buttonStyle(.borderedProminent)
            .disabled(workspace.messaging?.offline ?? true)
        }
    }

    /// A conversation that was created on this device and is not confirmed yet.
    private func creationRow(_ creation: TeamMessageCreation) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(creation.title).font(.body.weight(.semibold)).lineLimit(1)
            Text(LocalizedStringKey(creation.error.map(TeamsErrors.key) ?? (creation.attempted == true ? "teams_message_confirming" : "teams_message_pending")))
                .font(.caption).foregroundStyle(.secondary)
            Button("teams_message_discard", role: .destructive) {
                Task { await workspace.run { try await workspace.messaging?.discardCreation() } }
            }
            .font(.caption)
            .disabled((creation.attempted == true && creation.error == nil) || (creation.leaseUntil ?? 0) > Date().timeIntervalSince1970 * 1000)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }

    // MARK: Sections and rows

    @ViewBuilder
    private func section(_ kind: TeamsSection, _ title: LocalizedStringKey, _ rows: [TeamMessageConversation], _ context: Context) -> some View {
        if !rows.isEmpty {
            let collapsed = workspace.collapsed.contains(kind)
            Section {
                if !collapsed {
                    ForEach(rows) { conversation in
                        row(conversation, context)
                    }
                }
            } header: {
                Button {
                    withAnimation(.easeInOut(duration: 0.2)) { workspace.toggleSection(kind) }
                } label: {
                    HStack(spacing: 8) {
                        Text(title).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary).textCase(nil)
                        Spacer()
                        if collapsed { TeamsUnreadBadge(count: rows.reduce(0) { $0 + context.unread($1) }) }
                        Image(systemName: "chevron.down")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .rotationEffect(.degrees(collapsed ? -90 : 0))
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("teams.section.\(kind.rawValue)")
            }
        }
    }

    private func row(_ conversation: TeamMessageConversation, _ context: Context) -> some View {
        let label = context.labels[conversation.id] ?? ""
        let unread = context.unread(conversation)
        let strings = context.strings
        let preview = TeamsListLogic.conversationPreview(conversation, uid: uid, strings: strings) { last in
            if last.authorKind == "shared_bot" {
                let name = context.bots.first { $0.sharedBotId == last.authorSharedBotId }?.name ?? ""
                return name.isEmpty ? strings.bot : name
            }
            let name = context.members.first { $0.uid == last.authorUid }?.displayName ?? ""
            return name.isEmpty ? strings.member : name
        }
        let participants = TeamsListLogic.conversationParticipants(conversation, members: context.members, bots: context.bots, uid: uid, strings: strings)
        return Button {
            onOpen(conversation.id)
        } label: {
            TeamConversationRow(
                conversation: conversation, label: label, preview: preview, unread: unread, participants: participants,
                time: TeamsTimeText.listTime(atMs: conversation.lastMessage?.atMs, locale: locale))
        }
        .buttonStyle(.plain)
        .listRowBackground(conversation.id == selectedConversation ? Color.accentColor.opacity(0.12) : Color.clear)
        .swipeActions(edge: .leading, allowsFullSwipe: true) {
            if unread > 0 && context.online {
                Button("teams_mark_read", systemImage: "checkmark.circle") {
                    Task { await workspace.markRead(conversation.id) }
                }
                .tint(.accentColor)
            }
        }
        .accessibilityIdentifier("teams.row.\(conversation.id)")
    }
}

/// One conversation in the list: the lead, the label and time, the preview and the unread badge. Unread
/// rows have a bold label and preview.
struct TeamConversationRow: View {
    let conversation: TeamMessageConversation
    let label: String
    let preview: String
    let unread: Int
    let participants: [TeamIdentity]
    let time: String

    var body: some View {
        HStack(spacing: 12) {
            ConversationLead(conversation: conversation, label: label, participants: participants, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(label)
                        .font(.body.weight(unread > 0 ? .bold : .regular))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Text(time).font(.caption).foregroundStyle(unread > 0 ? Color.primary : Color.secondary)
                }
                HStack(alignment: .center, spacing: 8) {
                    Text(preview)
                        .font(.subheadline.weight(unread > 0 ? .semibold : .regular))
                        .foregroundStyle(unread > 0 ? Color.primary : Color.secondary)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    TeamsUnreadBadge(count: unread)
                }
            }
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

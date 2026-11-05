import SwiftUI

/// The sheets of the Teams tab.
enum TeamsSheet: String, Identifiable {
    case switcher, createTeam, joinTeam, newMessage, newChannel, newBot
    var id: String { rawValue }
}

/// The three ways to start a conversation (spec §6.5): the entries of the compose menu.
struct NewConversationItems: View {
    let onNew: (TeamsSheet) -> Void

    var body: some View {
        Button("teams_new_message_title", systemImage: "square.and.pencil") { onNew(.newMessage) }
        Button("teams_new_channel_title", systemImage: "number") { onNew(.newChannel) }
        Button("teams_new_bot_title", systemImage: "cpu") { onNew(.newBot) }
    }
}

// MARK: - Team switcher

/// The team chip's sheet (spec §6.2): every team with its role, member count and unread sum; the current one
/// checked. Footer actions: create, join, details of the current team.
struct TeamSwitcherSheet: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.dismiss) private var dismiss
    let onSelect: (String) -> Void
    let onCreate: () -> Void
    let onJoin: () -> Void
    let onDetails: () -> Void

    var body: some View {
        let unread = workspace.unreadByTeam
        NavigationStack {
            List {
                Section {
                    ForEach(workspace.teams) { team in
                        Button {
                            onSelect(team.id)
                        } label: {
                            HStack(spacing: 12) {
                                TeamAvatar(teamID: team.id, name: team.name, size: 36)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(team.name).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                                    HStack(spacing: 6) {
                                        Text(TeamsText.role(team.role))
                                        Text("·")
                                        Text(TeamsText.membersCount(team.memberCount))
                                    }
                                    .font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer(minLength: 8)
                                if team.id != workspace.selectedTeamID { TeamsUnreadBadge(count: unread[team.id] ?? 0) }
                                if team.id == workspace.selectedTeamID {
                                    Image(systemName: "checkmark").foregroundStyle(Color.accentColor).accessibilityLabel(Text("teams_switch_team"))
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .accessibilityIdentifier("teams.team.\(team.id)")
                    }
                }
                Section {
                    Button("teams_create", systemImage: "plus.circle", action: onCreate).accessibilityIdentifier("teams.switcher.create")
                    Button("teams_join", systemImage: "person.badge.plus", action: onJoin).accessibilityIdentifier("teams.switcher.join")
                    if workspace.team != nil {
                        Button("teams_team_details", systemImage: "info.circle", action: onDetails).accessibilityIdentifier("teams.switcher.details")
                    }
                }
            }
            .navigationTitle("teams_switch_team")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("common_done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

// MARK: - Create and join a team

/// Create a team (spec §6.2; Desktop `CreateTeamDialog`): a name of at most 80 characters.
struct CreateTeamSheet: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.dismiss) private var dismiss
    let onDone: () -> Void
    @State private var name = ""

    private var trimmed: String { JSText.trim(name) }
    private var valid: Bool { !trimmed.isEmpty && trimmed.count <= 80 }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("teams_name", text: $name)
                        .accessibilityIdentifier("teams.name")
                        .submitLabel(.done)
                } footer: {
                    Text("teams_disclosure")
                }
                if !workspace.error.isEmpty {
                    Section { Text(LocalizedStringKey(TeamsErrors.key(workspace.error))).foregroundStyle(.red).accessibilityIdentifier("teams.error") }
                }
                Section {
                    Button("teams_create") {
                        Task {
                            let created = await workspace.execute(["action": "create", "teamId": UUID().uuidString, "name": trimmed])
                            if created { onDone() }
                        }
                    }
                    .disabled(!valid || workspace.busy)
                }
            }
            .navigationTitle("teams_new")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("teams_cancel") { dismiss() } } }
            .onAppear { workspace.clearError() }
        }
        .presentationDetents([.medium])
    }
}

/// Join a team (spec §6.2): an invite link or the 32-character code. A link only opens this sheet prefilled;
/// nothing joins until the viewer confirms.
struct JoinTeamSheet: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.dismiss) private var dismiss
    let prefill: String
    let onDone: () -> Void
    @State private var text = ""

    private var code: String? { TeamInvitation.code(text) }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("teams_invite_input", text: $text)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .privacySensitive()
                        .accessibilityIdentifier("teams.invite")
                    if !JSText.isBlank(text) && code == nil {
                        Text("teams_invite_invalid").font(.footnote).foregroundStyle(.red)
                    }
                } footer: {
                    Text("teams_join_disclosure")
                }
                if !workspace.error.isEmpty {
                    Section { Text(LocalizedStringKey(TeamsErrors.key(workspace.error))).foregroundStyle(.red).accessibilityIdentifier("teams.error") }
                }
                Section {
                    Button("teams_join_confirm") {
                        guard let code else { return }
                        Task {
                            let joined = await workspace.execute(["action": "join", "code": code, "requestId": UUID().uuidString])
                            if joined { onDone() }
                        }
                    }
                    .disabled(code == nil || workspace.busy)
                }
            }
            .navigationTitle("teams_join")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("teams_cancel") { dismiss() } } }
            .onAppear { workspace.clearError(); text = prefill }
        }
        .presentationDetents([.medium])
    }
}

// MARK: - New conversation

/// New message (a person or a group), new channel and "message a Bot" (spec §6.5; Desktop
/// `NewConversationDialog` and `NewBotDmDialog`). Creating needs a connection; every chosen Bot must still be
/// usable when the conversation is created.
struct NewConversationSheet: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.dismiss) private var dismiss
    let mode: TeamsSheet
    let uid: String
    /// Opens the conversation that was created or found.
    let onOpen: (String) -> Void

    private static let maxBots = 8
    @State private var direct = "dm"
    @State private var title = ""
    @State private var parties: [String] = []
    @State private var botParties: [String] = []
    @State private var tried = false

    private var model: TeamMessagingModel? { workspace.messaging }
    private var offline: Bool { model?.offline ?? true }
    private var kind: String { mode == .newChannel ? "channel" : direct }
    private var people: [TeamMember] { workspace.directory.filter { $0.uid != uid } }
    private var usableBots: [TeamBotCard] { (model?.bots ?? []).filter(\.selectable) }
    private var trimmedTitle: String { JSText.trim(title) }
    /// A channel is always named; so is a group without people (Bots only).
    private var named: Bool { kind == "channel" || (kind == "group" && parties.isEmpty) }
    private var valid: Bool {
        kind == "dm" ? parties.count == 1 : (parties.count + botParties.count > 0 && (!named || !trimmedTitle.isEmpty) && trimmedTitle.count <= 120)
    }
    private func owner(_ id: String) -> String { workspace.directory.first { $0.uid == id }?.displayName ?? id }

    var body: some View {
        NavigationStack {
            Group {
                if mode == .newBot { botPicker } else { form }
            }
            .navigationTitle(LocalizedStringKey(mode == .newChannel ? "teams_new_channel_title" : mode == .newBot ? "teams_new_bot_title" : "teams_new_message_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button(mode == .newBot ? "common_close" : "teams_cancel") { dismiss() } } }
            .onAppear { workspace.clearActionError() }
        }
        .presentationDetents([.large])
    }

    // MARK: Message and channel

    private var form: some View {
        Form {
            if mode == .newMessage {
                Section {
                    Picker("teams_conversation_type", selection: $direct) {
                        Text("teams_conversation_dm").tag("dm")
                        Text("teams_conversation_group").tag("group")
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("teams.new.kind")
                    .onChange(of: direct) { _, _ in parties = []; botParties = [] }
                }
            }
            if kind != "dm" {
                Section {
                    TextField(LocalizedStringKey(kind == "channel" ? "teams_new_channel_name" : "teams_new_group_name_optional"), text: $title)
                        .accessibilityIdentifier("teams.new.title")
                }
            }
            Section {
                ForEach(people) { member in
                    Button { choose(member.uid) } label: {
                        HStack(spacing: 12) {
                            PersonAvatar(id: member.uid, name: member.displayName, size: 28)
                            Text(member.displayName).foregroundStyle(.primary)
                            Spacer()
                            Image(systemName: parties.contains(member.uid) ? (kind == "dm" ? "largecircle.fill.circle" : "checkmark.circle.fill") : (kind == "dm" ? "circle" : "circle"))
                                .foregroundStyle(parties.contains(member.uid) ? Color.accentColor : Color.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .accessibilityAddTraits(parties.contains(member.uid) ? .isSelected : [])
                }
                if people.isEmpty { Text("teams_new_no_members").font(.footnote).foregroundStyle(.secondary) }
            } header: {
                Text(LocalizedStringKey(kind == "dm" ? "teams_new_to" : "teams_members"))
            }
            if kind != "dm" && !usableBots.isEmpty {
                Section {
                    ForEach(usableBots) { bot in
                        let chosen = botParties.contains(bot.key)
                        Button { toggleBot(bot.key) } label: {
                            HStack(spacing: 12) {
                                AgentAvatar(agentID: bot.avatarId, size: 28)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(bot.name).foregroundStyle(.primary)
                                    Text("teams_bot_owner \(owner(bot.ownerUid))").font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                Image(systemName: chosen ? "checkmark.circle.fill" : "circle").foregroundStyle(chosen ? Color.accentColor : Color.secondary)
                            }
                            .contentShape(Rectangle())
                        }
                        .disabled(!chosen && botParties.count >= Self.maxBots)
                        .accessibilityAddTraits(chosen ? .isSelected : [])
                    }
                } header: {
                    Text("teams_new_participating_bots")
                }
            }
            if !botParties.isEmpty {
                Section { TeamsNote(text: "teams_new_bot_owner_note", systemImage: "eye") }
            }
            if offline {
                Section { TeamsNote(text: "teams_new_offline", systemImage: "wifi.slash") }
            }
            if tried && !workspace.actionError.isEmpty {
                Section { Text(LocalizedStringKey(TeamsErrors.key(workspace.actionError))).foregroundStyle(.red).accessibilityIdentifier("teams.error") }
            }
            Section {
                Button(LocalizedStringKey(kind == "dm" ? "teams_new_open_dm" : kind == "group" ? "teams_new_create_group" : "teams_new_create_channel")) {
                    Task { await submit() }
                }
                .disabled(workspace.messageBusy || offline || !valid)
                .accessibilityIdentifier("teams.new.submit")
            }
        }
    }

    private func choose(_ id: String) {
        if kind == "dm" { parties = [id] }
        else if let index = parties.firstIndex(of: id) { parties.remove(at: index) }
        else { parties.append(id) }
    }

    private func toggleBot(_ key: String) {
        if let index = botParties.firstIndex(of: key) { botParties.remove(at: index) } else if botParties.count < Self.maxBots { botParties.append(key) }
    }

    private func submit() async {
        guard valid, !offline else { return }
        tried = true
        let opened: String?
        if kind == "dm" {
            opened = await workspace.openDM(peerUID: parties[0])
        } else {
            let name = kind == "group" && trimmedTitle.isEmpty
                ? TeamsListLogic.defaultConversationTitle(parties, members: workspace.directory, uid: uid)
                : trimmedTitle
            opened = await workspace.createConversation(kind: kind, title: name, parties: parties, botParties: botParties)
        }
        if let opened { onOpen(opened) }
    }

    // MARK: Message a Bot

    private var botPicker: some View {
        List {
            Section {
                TeamsNote(text: "teams_bots_note", systemImage: "info.circle")
                if model?.botsUnverified == true { TeamsNote(text: "teams_bots_unverified", systemImage: "exclamationmark.triangle") }
            }
            Section {
                ForEach(usableBots) { bot in
                    botRow(bot)
                }
                if usableBots.isEmpty && model?.botsUnverified != true {
                    Text(LocalizedStringKey(model?.botsLoaded == true ? "teams_new_no_bots" : "teams_bots_loading")).font(.footnote).foregroundStyle(.secondary)
                }
            } header: {
                Text("teams_new_bots_pickable")
            }
            if offline {
                Section { TeamsNote(text: "teams_new_offline", systemImage: "wifi.slash") }
            }
            if tried && !workspace.actionError.isEmpty {
                Section { Text(LocalizedStringKey(TeamsErrors.key(workspace.actionError))).foregroundStyle(.red).accessibilityIdentifier("teams.error") }
            }
        }
    }

    private func botRow(_ bot: TeamBotCard) -> some View {
        let existing = TeamsListLogic.findBotDm(model?.cache?.conversations ?? [], uid: uid, botKey: bot.key)
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                AgentAvatar(agentID: bot.avatarId, size: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(bot.name).font(.body.weight(.semibold))
                    HStack(spacing: 6) {
                        Text(LocalizedStringKey(bot.online ? "remote_online" : "teams_bot_offline_queued"))
                        Text("·")
                        Text("teams_bot_owner \(owner(bot.ownerUid))")
                    }
                    .font(.caption).foregroundStyle(.secondary)
                }
            }
            HStack(spacing: 10) {
                Button(LocalizedStringKey(existing != nil ? "teams_new_bot_open" : "teams_new_bot_start")) {
                    Task { await openBot(bot, fresh: false) }
                }
                .buttonStyle(.borderedProminent)
                if existing != nil {
                    Button("remote_bot_new_chat") { Task { await openBot(bot, fresh: true) } }
                        .buttonStyle(.bordered)
                }
            }
            .controlSize(.small)
            .disabled(workspace.messageBusy || offline)
        }
        .padding(.vertical, 4)
        .accessibilityIdentifier("teams.new.bot.\(bot.key)")
    }

    private func openBot(_ bot: TeamBotCard, fresh: Bool) async {
        tried = true
        if let opened = await workspace.openBotDM(bot, fresh: fresh) { onOpen(opened) }
    }
}

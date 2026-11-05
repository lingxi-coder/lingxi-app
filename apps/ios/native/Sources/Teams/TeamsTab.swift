import SwiftUI

/// Where a push in the Teams tab goes.
enum TeamsRoute: Hashable {
    case conversation(String)
    case details
}

/// The Teams tab (spec §6): the selected team's conversations, a conversation, and the team's details. It reads
/// the shell state from the environment: `TabCoordinator` (join-link hand-off, depth, re-tap requests),
/// `AppNavigationModel` (the sign-in jump to Settings/Account, which remembers this tab as the way back) and
/// the app-level `TeamsWorkspace` (data; started by the app root, never by this tab).
///
/// - Phones: one `NavigationStack`: the list, a pushed conversation, a pushed details page. The tab bar leaves
///   with a push (`hidesTabBarWhenPushed`).
/// - Wide layouts (`isWideLayout`): the list, the open conversation and, on request, the details side by side
///   inside the tab; the tab bar stays.
struct TeamsTab: View {
    @Environment(AppNavigationModel.self) private var navigation
    @Environment(TabCoordinator.self) private var tabs
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.isWideLayout) private var wide
    @Environment(\.scenePhase) private var scenePhase

    @State private var path: [TeamsRoute] = []
    @State private var sheet: TeamsSheet?
    /// What the join sheet starts with: the code of a `team/join` link, else nothing. A link never joins by itself.
    @State private var joinPrefill = ""
    @State private var scrollToTop = 0
    /// Wide layout: the conversation in the middle pane, and whether the details pane is open.
    @State private var paneConversation = ""
    @State private var paneDetails: PaneDetails?

    private enum PaneDetails: Equatable {
        case team
        case conversation(String)
    }
    /// The open conversation as `<uid>\n<teamID>\n<conversationID>`. It survives the process being killed (spec §2.3);
    /// it is only restored for the same account and team, and only while the conversation is still in the directory.
    @SceneStorage("teams.openConversation") private var storedOpen = ""
    @State private var restoredOpen = false

    var body: some View {
        content
            .sheet(item: $sheet) { sheetContent($0) }
            .onChange(of: tabs.teamsJoinCode, initial: true) { _, _ in consumeJoinCode() }
            .onChange(of: workspace.uid) { _, uid in
                resetNavigation()
                sheet = nil
                if uid != nil { consumeJoinCode() }
            }
            .onChange(of: workspace.selectedTeamID) { _, _ in resetNavigation() }
            .onChange(of: path) { _, routes in
                reportDepth()
                guard !wide else { return }
                let open = routes.compactMap { route -> String? in if case let .conversation(id) = route { id } else { nil } }.first
                // Back at the list: the thread listener of the conversation that was open closes.
                if open == nil { workspace.openConversation("") }
                rememberOpen(open)
            }
            .onChange(of: wide, initial: true) { _, _ in
                reportDepth()
                restorePane()
            }
            .onChange(of: paneConversation) { _, id in
                guard wide else { return }
                if id.isEmpty { workspace.openConversation("") }
                // Details of the conversation that was open follow the one that is open now.
                if case .conversation = paneDetails { paneDetails = id.isEmpty ? nil : .conversation(id) }
                rememberOpen(id.isEmpty ? nil : id)
            }
            .onChange(of: workspace.messaging?.directoryReady ?? false, initial: true) { _, _ in
                restoreOpenConversation()
                restorePane()
            }
            .onChange(of: tabs.popToRootRequests[.teams]) { _, _ in path = [] }
            .onChange(of: tabs.scrollToTopRequests[.teams]) { _, _ in scrollToTop += 1 }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { workspace.foregrounded() }
            }
    }

    // MARK: Layouts

    @ViewBuilder
    private var content: some View {
        if wide, let uid = workspace.uid, let team = workspace.team {
            widePanes(uid: uid, team: team)
        } else {
            NavigationStack(path: $path) {
                root
                    .navigationDestination(for: TeamsRoute.self) { destination($0) }
            }
        }
    }

    /// The root of the phone stack: the list, or the reason there is none.
    @ViewBuilder
    private var root: some View {
        if let uid = workspace.uid {
            if let team = workspace.team {
                TeamsListView(
                    uid: uid, team: team, scrollToTop: scrollToTop,
                    onOpen: { open($0) }, onSwitchTeam: { sheet = .switcher }, onNew: { sheet = $0 })
            } else if workspace.loading || !workspace.selectedTeamID.isEmpty {
                loadingView
            } else if workspace.teams.isEmpty {
                if workspace.loadError.isEmpty { noTeamsView } else { failedView }
            } else {
                chooseView
            }
        } else {
            signedOutView
        }
    }

    private func widePanes(uid: String, team: TeamSummary) -> some View {
        HStack(spacing: 0) {
            NavigationStack {
                TeamsListView(
                    uid: uid, team: team, selectedConversation: paneConversation, scrollToTop: scrollToTop,
                    onOpen: { open($0) }, onSwitchTeam: { sheet = .switcher }, onNew: { sheet = $0 })
            }
            .frame(width: 360)
            Divider()
            NavigationStack {
                if paneConversation.isEmpty {
                    paneEmpty.toolbar(.hidden, for: .navigationBar)
                } else {
                    TeamConversationView(
                        conversationID: paneConversation, onGone: { paneConversation = "" },
                        onShowDetails: { paneDetails = .conversation(paneConversation) })
                        .id(team.id + ":" + paneConversation)
                }
            }
            .frame(maxWidth: .infinity)
            if let paneDetails {
                Divider()
                NavigationStack {
                    switch paneDetails {
                    case .team:
                        TeamDetailsPage(onOpenConversation: { open($0) }, onClose: { self.paneDetails = nil }, showsClose: true)
                    case let .conversation(id):
                        TeamConversationDetailsSheet(conversationID: id, onClose: { self.paneDetails = nil })
                    }
                }
                .frame(width: 360)
            }
        }
    }

    private var paneEmpty: some View {
        let none = workspace.messaging?.cache?.conversations.isEmpty == true
        return TeamsStateView(
            systemImage: "bubble.left.and.bubble.right",
            title: none ? "teams_no_conversations" : "Choose a conversation", // TODO(strings): teams_choose_conversation
            message: none ? "teams_no_conversations_hint" : nil)
    }

    @ViewBuilder
    private func destination(_ route: TeamsRoute) -> some View {
        switch route {
        case let .conversation(id):
            TeamConversationView(conversationID: id)
        case .details:
            TeamDetailsPage(onOpenConversation: { open($0) }, onClose: { path = [] })
        }
    }

    // MARK: States without a list

    private var signedOutView: some View {
        TeamsStateView(systemImage: "person.crop.circle", title: "remote_signed_out_title", message: "teams_signin") {
            Button("remote_sign_in") { navigation.showSettings(.account, origin: .teams) }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("teams.signin")
        }
        .navigationTitle("teams_title")
        .navigationBarTitleDisplayMode(.large)
        .accessibilityIdentifier("teams.signedout")
    }

    private var loadingView: some View {
        VStack(spacing: 12) {
            ProgressView()
            Text("teams_loading").font(.footnote).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationTitle("teams_title")
        .navigationBarTitleDisplayMode(.large)
        .accessibilityIdentifier("teams.loading")
    }

    private var failedView: some View {
        TeamsStateView(systemImage: "exclamationmark.triangle", title: "teams_load_failed", message: LocalizedStringKey(TeamsErrors.key(workspace.loadError))) {
            Button("common_retry") { workspace.scheduleRefresh() }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("teams.retry")
        }
        .navigationTitle("teams_title")
        .navigationBarTitleDisplayMode(.large)
        .accessibilityIdentifier("teams.failed")
    }

    private var noTeamsView: some View {
        TeamsStateView(systemImage: "person.2", title: "teams_empty") {
            VStack(spacing: 10) {
                Button("teams_create") { sheet = .createTeam }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("teams.empty.create")
                Button("teams_join") { joinPrefill = ""; sheet = .joinTeam }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("teams.empty.join")
            }
        }
        .navigationTitle("teams_title")
        .navigationBarTitleDisplayMode(.large)
        .accessibilityIdentifier("teams.empty")
    }

    private var chooseView: some View {
        TeamsStateView(systemImage: "person.2", title: "teams_choose_team") {
            Button("teams_switch_team") { sheet = .switcher }
                .buttonStyle(.borderedProminent)
        }
        .navigationTitle("teams_title")
        .navigationBarTitleDisplayMode(.large)
    }

    // MARK: Sheets

    @ViewBuilder
    private func sheetContent(_ sheet: TeamsSheet) -> some View {
        switch sheet {
        case .switcher:
            TeamSwitcherSheet(
                onSelect: { id in
                    self.sheet = nil
                    workspace.selectTeam(id)
                },
                onCreate: { self.sheet = .createTeam },
                onJoin: {
                    joinPrefill = ""
                    self.sheet = .joinTeam
                },
                onDetails: {
                    self.sheet = nil
                    showDetails()
                })
        case .createTeam:
            CreateTeamSheet { self.sheet = nil }
        case .joinTeam:
            JoinTeamSheet(prefill: joinPrefill) { self.sheet = nil }
        case .newMessage, .newChannel, .newBot:
            if let uid = workspace.uid {
                NewConversationSheet(mode: sheet, uid: uid) { id in
                    self.sheet = nil
                    open(id)
                }
            }
        }
    }

    // MARK: Navigation

    private func open(_ id: String) {
        if wide { paneConversation = id } else { path = [.conversation(id)] }
    }

    private func showDetails() {
        if wide { paneDetails = .team } else { path = [.details] }
    }

    private func reportDepth() {
        tabs.reportDepth(.teams, wide ? 0 : path.count)
    }

    /// Another account or another team: what was open belonged to the old one.
    private func resetNavigation() {
        path = []
        paneConversation = ""
        paneDetails = nil
    }

    private func rememberOpen(_ id: String?) {
        guard let id, let uid = workspace.uid, let team = workspace.team else { storedOpen = ""; return }
        storedOpen = [uid, team.id, id].joined(separator: "\n")
    }

    /// After the process was killed: the conversation that was open comes back, once the directory says it is still there.
    private func restoreOpenConversation() {
        guard !restoredOpen, !storedOpen.isEmpty, let model = workspace.messaging, model.directoryReady,
              let uid = workspace.uid, let team = workspace.team else { return }
        restoredOpen = true
        let parts = storedOpen.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        guard parts.count == 3, parts[0] == uid, parts[1] == team.id,
              model.cache?.conversations.contains(where: { $0.id == parts[2] }) == true else { return }
        open(parts[2])
    }

    /// The wide layout reopens the conversation that was open when the team was last on screen.
    private func restorePane() {
        guard wide, paneConversation.isEmpty, let team = workspace.team, let id = workspace.rememberedConversation(for: team.id),
              workspace.messaging?.cache?.conversations.contains(where: { $0.id == id }) == true else { return }
        paneConversation = id
    }

    /// A `team/join` link opens the join sheet with its code, once someone is signed in. It never joins by itself.
    private func consumeJoinCode() {
        guard workspace.uid != nil, let code = tabs.teamsJoinCode else { return }
        tabs.teamsJoinCode = nil
        joinPrefill = code
        sheet = .joinTeam
    }
}

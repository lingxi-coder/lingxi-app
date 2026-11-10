import SwiftUI

/// Where a push in the Friends tab goes.
enum FriendsRoute: Hashable {
    case chat(String)
    /// The Bots of a friendship (D21), over its chat.
    case bots(String)
    /// A Bot DM on the friendship's workspace (D22), over its chat.
    case botChat(uid: String, conversationID: String)
    case add
    case profile
    case blocked

    /// The friend whose chat workspace the screen needs: it stays open for as long as any screen of the friendship is.
    var chatUID: String? {
        switch self {
        case let .chat(uid), let .bots(uid), let .botChat(uid, _): uid
        case .add, .profile, .blocked: nil
        }
    }

    /// The phone's stack for this screen: the Bots and a Bot DM sit over the friend's chat, which Back returns to.
    var stack: [FriendsRoute] {
        switch self {
        case let .bots(uid), let .botChat(uid, _): [.chat(uid), self]
        case .chat, .add, .profile, .blocked: [self]
        }
    }
}

/// What the Friends tab shows over its root.
private enum FriendsSheet: Identifiable {
    /// The confirmation of an opened friend link (D8); it holds the code until it closes.
    case link(String)

    var id: String {
        switch self {
        case let .link(code): "link:" + code
        }
    }
}

/// The Friends tab (docs/FRIENDS-CLIENTS.md D1, D3): the team invitations, requests and friends, Add friend, Profile
/// settings, Blocked people, and a friend's chat with its Bots. It reads the shell state from the environment:
/// `TabCoordinator` (the `friend/*` link hand-off, depth, re-tap requests), `AppNavigationModel` (the sign-in jump to
/// Settings/Account, which remembers this tab as the way back), the app-level `FriendsWorkspace` (data; started by the
/// app root, never by this tab) and the app-level `TeamsWorkspace` (refreshed after a team offer is joined).
///
/// - Phones: one `NavigationStack`: the list, and the pushed screens above it (a friend's chat, with its Bots or a Bot DM
///   over it). The tab bar leaves with a push (`hidesTabBarWhenPushed`).
/// - Wide layouts (`isWideLayout`): the list and one screen (a chat, its Bots or a Bot DM, Add friend, Profile settings or
///   Blocked people) side by side inside the tab; the tab bar stays.
struct FriendsTab: View {
    @Environment(AppNavigationModel.self) private var navigation
    @Environment(TabCoordinator.self) private var tabs
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(TeamsWorkspace.self) private var teams
    @Environment(\.isWideLayout) private var wide
    @Environment(\.scenePhase) private var scenePhase

    @State private var path: [FriendsRoute] = []
    /// Wide layout: the screen beside the list.
    @State private var pane: FriendsRoute?
    @State private var sheet: FriendsSheet?
    @State private var scrollToTop = 0
    @State private var scrollToRequests = 0

    var body: some View {
        content
            .sheet(item: $sheet) { sheetContent($0) }
            // The profile is read on the first open per account (D2): it names the viewer's friend link and tells whether the name is still `Member`.
            .task(id: friends.uid) { if friends.uid != nil { await friends.loadProfile() } }
            .onChange(of: tabs.friendsRequest, initial: true) { _, _ in consume() }
            .onChange(of: friends.uid) { _, uid in
                resetNavigation()
                sheet = nil
                if uid != nil { consume() }
            }
            .onChange(of: friends.cardsLoaded) { _, _ in consume() }
            .onChange(of: friends.cardsFailed) { _, _ in consume() }
            .onChange(of: path) { _, _ in
                reportDepth()
                syncChat()
            }
            .onChange(of: pane) { _, _ in syncChat() }
            .onChange(of: wide, initial: true) { _, isWide in adopt(wide: isWide) }
            .onChange(of: tabs.popToRootRequests[.friends]) { _, _ in path = [] }
            .onChange(of: tabs.scrollToTopRequests[.friends]) { _, _ in scrollToTop += 1 }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { friends.foregrounded() }
            }
    }

    // MARK: Layouts

    @ViewBuilder
    private var content: some View {
        if wide, friends.uid != nil {
            widePanes
        } else {
            NavigationStack(path: $path) {
                root
                    .navigationDestination(for: FriendsRoute.self) { destination($0) }
            }
        }
    }

    /// The root of the phone stack: the list, or the reason there is none.
    @ViewBuilder
    private var root: some View {
        if friends.uid != nil {
            list(selected: "")
        } else {
            signedOutView
        }
    }

    private var widePanes: some View {
        HStack(spacing: 0) {
            NavigationStack {
                list(selected: pane?.chatUID ?? "")
            }
            .frame(width: 360)
            Divider()
            NavigationStack {
                if let pane {
                    destination(pane).id(pane)
                } else {
                    TeamsStateView(systemImage: "bubble.left.and.bubble.right", title: "teams_choose_conversation")
                        .toolbar(.hidden, for: .navigationBar)
                }
            }
            .frame(maxWidth: .infinity)
        }
    }

    private func list(selected: String) -> some View {
        FriendsListView(
            selectedChat: selected, scrollToTop: scrollToTop, scrollToRequests: scrollToRequests,
            onOpen: { open(.chat($0)) }, onAdd: { open(.add) }, onProfile: { open(.profile) }, onBlocked: { open(.blocked) },
            onJoined: joined)
    }

    @ViewBuilder
    private func destination(_ route: FriendsRoute) -> some View {
        switch route {
        case let .chat(uid):
            FriendChatView(uid: uid, onClose: close, onBots: { push(.bots(uid)) })
        case let .bots(uid):
            FriendBotsView(uid: uid, onOpenBot: { openBotChat(uid, $0) }, onBack: { open(.chat(uid)) }, onClose: close)
        case let .botChat(uid, conversationID):
            FriendBotChatView(uid: uid, conversationID: conversationID, onBack: { open(.chat(uid)) }, onClose: close)
        case .add:
            AddFriendView(onOpenChat: { open(.chat($0)) })
        case .profile:
            FriendProfileView()
        case .blocked:
            BlockedPeopleView()
        }
    }

    private var signedOutView: some View {
        TeamsStateView(systemImage: "person.crop.circle", title: "remote_signed_out_title", message: "friends_signin") {
            Button("remote_sign_in") { navigation.showSettings(.account, origin: .friends) }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("friends.signin")
        }
        .navigationTitle("friends_title")
        .navigationBarTitleDisplayMode(.large)
        .accessibilityIdentifier("friends.signedout")
    }

    @ViewBuilder
    private func sheetContent(_ sheet: FriendsSheet) -> some View {
        switch sheet {
        case let .link(code):
            FriendLinkSheet(code: code) { uid in
                self.sheet = nil
                open(.chat(uid))
            }
        }
    }

    // MARK: Navigation

    private func open(_ route: FriendsRoute) {
        if wide { pane = route } else { path = [route] }
    }

    /// A screen over the friend's chat (its Bots): pushed on phones, the pane's content on wide layouts.
    private func push(_ route: FriendsRoute) {
        if wide { pane = route } else { path.append(route) }
    }

    /// Message on a Bot opens its DM in place of the Bots, so that Back returns to the friend chat (D22).
    private func openBotChat(_ uid: String, _ conversationID: String) {
        let route = FriendsRoute.botChat(uid: uid, conversationID: conversationID)
        if wide {
            pane = route
        } else {
            if case .bots = path.last { path.removeLast() }
            path.append(route)
        }
    }

    /// A team offer was joined (D27): say so once, stay in Friends, and refresh the Teams list as a link join does, so the team appears
    /// there.
    private func joined(_ offer: TeamOffer) {
        friends.show(notice: String(localized: "friends_offer_joined \(offer.teamName)"))
        teams.scheduleRefresh()
    }

    private func close() {
        if wide { pane = nil } else { path = [] }
    }

    private func reportDepth() {
        tabs.reportDepth(.friends, wide ? 0 : path.count)
    }

    /// Another account: what was open belonged to the old one.
    private func resetNavigation() {
        path = []
        pane = nil
    }

    /// The layout changed: the screen that was open moves between the stack and the pane.
    private func adopt(wide isWide: Bool) {
        if isWide, !path.isEmpty {
            pane = path.last
            path = []
        } else if !isWide, let current = pane {
            path = current.stack
            pane = nil
        }
        reportDepth()
        syncChat()
    }

    /// A friend's chat keeps its listeners for as long as its screen is on the stack, visible or not; leaving it stops them.
    private func syncChat() {
        friends.openChat(wide ? pane?.chatUID : path.compactMap(\.chatUID).first)
    }

    /// A `friend/*` link or push, once someone is signed in (D8, D16). It never redeems by itself: `friend/add` only
    /// opens the confirmation.
    private func consume() {
        guard friends.uid != nil, let request = tabs.friendsRequest else { return }
        switch request {
        case let .add(code):
            tabs.friendsRequest = nil
            resetNavigation()
            sheet = .link(code)
        case .requests:
            tabs.friendsRequest = nil
            resetNavigation()
            scrollToRequests += 1
        case let .chat(uid):
            // The first snapshot of the cards says whether this is a friend.
            guard friends.cardsLoaded || friends.cardsFailed else { return }
            tabs.friendsRequest = nil
            if friends.friend(uid) != nil || friends.expecting.contains(uid) {
                open(.chat(uid))
            } else {
                resetNavigation()
                friends.show(notice: String(localized: "friends_chat_unavailable"))
            }
        }
    }
}

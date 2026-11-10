import SwiftUI

/// Where a push in the Friends tab goes.
enum FriendsRoute: Hashable {
    case chat(String)
    case add
    case profile
    case blocked

    var chatUID: String? {
        if case let .chat(uid) = self { uid } else { nil }
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

/// The Friends tab (docs/FRIENDS-CLIENTS.md D1, D3): the requests and friends, Add friend, Profile settings, Blocked
/// people, and a friend's chat. It reads the shell state from the environment: `TabCoordinator` (the `friend/*` link
/// hand-off, depth, re-tap requests), `AppNavigationModel` (the sign-in jump to Settings/Account, which remembers this
/// tab as the way back) and the app-level `FriendsWorkspace` (data; started by the app root, never by this tab).
///
/// - Phones: one `NavigationStack`: the list, and one pushed screen above it. The tab bar leaves with a push
///   (`hidesTabBarWhenPushed`).
/// - Wide layouts (`isWideLayout`): the list and one screen (a chat, Add friend, Profile settings or Blocked people)
///   side by side inside the tab; the tab bar stays.
struct FriendsTab: View {
    @Environment(AppNavigationModel.self) private var navigation
    @Environment(TabCoordinator.self) private var tabs
    @Environment(FriendsWorkspace.self) private var friends
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
            onOpen: { open(.chat($0)) }, onAdd: { open(.add) }, onProfile: { open(.profile) }, onBlocked: { open(.blocked) })
    }

    @ViewBuilder
    private func destination(_ route: FriendsRoute) -> some View {
        switch route {
        case let .chat(uid):
            FriendChatView(uid: uid, onClose: close)
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
            path = [current]
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

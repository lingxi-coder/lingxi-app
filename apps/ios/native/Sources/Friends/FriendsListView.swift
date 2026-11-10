import SwiftUI

/// The root of the Friends tab (docs/FRIENDS-CLIENTS.md D3, D4, D27): the name banner, the team invitations, the requests,
/// and one row per friend with the preview, the time and the unread count of its chat. Everything comes from the cards and
/// the offers the workspace listens to; the rules (order, filter, preview, expiry) are `FriendsLogic` and `TeamsListLogic`.
struct FriendsListView: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.locale) private var locale
    /// The chat open beside the list (wide layout); highlighted. Empty on phones.
    var selectedChat = ""
    /// Increase when the tab is tapped again at its root, and when `friend/requests` was opened.
    var scrollToTop = 0
    var scrollToRequests = 0
    let onOpen: (String) -> Void
    let onAdd: () -> Void
    let onProfile: () -> Void
    let onBlocked: () -> Void
    /// A team offer was joined (`friends_offer_joined`, and the Teams list refreshes): the tab does both.
    let onJoined: (TeamOffer) -> Void

    @State private var query = ""
    @State private var showAllOffers = false
    @State private var showAllReceived = false
    @State private var showAllSent = false
    /// `friend/requests` was opened: scroll to the team invitations or the requests as soon as there are any.
    @State private var scrollPending = false
    private static let topID = "friends.list.top"
    private static let offersID = "friends.list.offers"
    private static let requestsID = "friends.list.requests"

    var body: some View {
        let sections = FriendsLogic.sections(friends.list, locale: locale)
        let shown = sections.friends.filter { FriendsLogic.matches($0, query: query) }
        let offers = friends.sortedOffers
        let strings = TeamsListStrings.current
        return ScrollViewReader { proxy in
            List {
                Color.clear.frame(height: 1)
                    .listRowInsets(EdgeInsets()).listRowSeparator(.hidden).listRowBackground(Color.clear)
                    .id(Self.topID)
                    .accessibilityHidden(true)
                if friends.profile?.displayName == FriendsLogic.placeholderName { namePrompt }
                // The empty state also waits for the offers: they are a listener of their own, and an invitation is something to show.
                if friends.loading || (friends.list.isEmpty && !friends.cardsFailed && !friends.offersAnswered) {
                    HStack { Spacer(); ProgressView(); Spacer() }
                        .listRowSeparator(.hidden)
                        .accessibilityLabel(Text("friends_loading"))
                        .accessibilityIdentifier("friends.loading")
                } else if friends.list.isEmpty && offers.isEmpty {
                    if friends.cardsFailed { failedState } else { emptyState }
                } else {
                    teamInvitations(offers)
                    requests(sections)
                    if !sections.friends.isEmpty && shown.isEmpty {
                        TeamsStateView(systemImage: "magnifyingglass", title: "friends_filter_empty")
                            .listRowSeparator(.hidden).listRowBackground(Color.clear)
                    }
                    ForEach(shown) { friendRow($0, strings: strings) }
                }
            }
            .listStyle(.plain)
            .searchable(text: $query, prompt: Text("friends_filter_placeholder"))
            .safeAreaInset(edge: .top, spacing: 0) { banners }
            .onChange(of: scrollToTop) { _, _ in
                withAnimation { proxy.scrollTo(Self.topID, anchor: .top) }
            }
            .onChange(of: scrollToRequests) { _, _ in
                scrollPending = true
                scrollToRequestsIfReady(proxy)
            }
            .onChange(of: friends.cardsLoaded) { _, _ in scrollToRequestsIfReady(proxy) }
            .onChange(of: friends.offersAnswered) { _, _ in scrollToRequestsIfReady(proxy) }
            .onChange(of: hasRequests) { _, _ in scrollToRequestsIfReady(proxy) }
            // The offers are a function of the clock (D28): whenever this view is shown it is read again.
            .onAppear { friends.refreshClock() }
        }
        .navigationTitle("friends_title")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: onAdd) { Image(systemName: "person.badge.plus") }
                    .accessibilityLabel(Text("friends_add"))
                    .accessibilityIdentifier("friends.add")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("friends_profile_title", systemImage: "person.text.rectangle", action: onProfile)
                    Button("friends_blocked_title", systemImage: "hand.raised", action: onBlocked)
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .accessibilityLabel(Text("composer_more"))
                .accessibilityIdentifier("friends.more")
            }
        }
        .accessibilityIdentifier("friends.list")
    }

    /// The area `friend/requests` leads to: the team invitations, or else the requests.
    private var hasRequests: Bool { !friends.offers.isEmpty || friends.list.contains { $0.state != .friends } }

    /// Waits for the invitations and requests to arrive and scrolls to the first of them (D16); gives up once the cards and the
    /// offers are in and there are none.
    private func scrollToRequestsIfReady(_ proxy: ScrollViewProxy) {
        guard scrollPending else { return }
        if hasRequests {
            scrollPending = false
            let target = friends.offers.isEmpty ? Self.requestsID : Self.offersID
            withAnimation { proxy.scrollTo(target, anchor: .top) }
        } else if friends.cardsLoaded && friends.offersAnswered {
            scrollPending = false
        }
    }

    // MARK: Banners and states

    @ViewBuilder
    private var banners: some View {
        VStack(spacing: 0) {
            if friends.cardsFailed && (!friends.list.isEmpty || !friends.offers.isEmpty) {
                TeamsBanner(kind: .error, title: "friends_load_error", actionTitle: "common_retry") { friends.reload() }
                    .accessibilityIdentifier("friends.banner.error")
            }
            if !friends.actionError.isEmpty {
                TeamsBanner(kind: .error, title: LocalizedStringKey(FriendsLogic.errorKey(friends.actionError)), actionTitle: "common_close") { friends.clearActionError() }
                    .accessibilityIdentifier("friends.banner.action")
            }
            FriendsNotice()
        }
    }

    private var namePrompt: some View {
        HStack(spacing: 12) {
            Image(systemName: "person.crop.circle.badge.exclamationmark").font(.title3).foregroundStyle(.orange).accessibilityHidden(true)
            Text("friends_name_prompt").font(.subheadline)
            Spacer(minLength: 8)
            Button("friends_name_prompt_action", action: onProfile)
                .buttonStyle(.bordered)
                .controlSize(.small)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("friends.nameprompt")
    }

    private var failedState: some View {
        TeamsStateView(systemImage: "exclamationmark.triangle", title: "friends_title", message: "friends_load_error") {
            Button("common_retry") { friends.reload() }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("friends.retry")
        }
        .listRowSeparator(.hidden).listRowBackground(Color.clear)
        .accessibilityIdentifier("friends.failed")
    }

    private var emptyState: some View {
        TeamsStateView(systemImage: "person.2", title: "friends_empty_title", message: "friends_empty_body") {
            Button("friends_add", action: onAdd)
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("friends.empty.add")
        }
        .listRowSeparator(.hidden).listRowBackground(Color.clear)
        .accessibilityIdentifier("friends.empty")
    }

    // MARK: Team invitations (D27)

    /// A section of its own above the requests, with the same twenty-row limit, only when there is at least one offer.
    @ViewBuilder
    private func teamInvitations(_ offers: [TeamOffer]) -> some View {
        if !offers.isEmpty {
            Text("friends_section_offers")
                .font(.title3.weight(.semibold))
                .listRowSeparator(.hidden)
                .id(Self.offersID)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("friends.offers")
            ForEach(rows(offers, expanded: showAllOffers)) { offerRow($0) }
            showAll(offers.count, expanded: showAllOffers, identifier: "friends.offers.showall") { showAllOffers = true }
        }
    }

    private func offerRow(_ offer: TeamOffer) -> some View {
        FriendOfferRow(
            offer: offer, expiry: FriendsLogic.offerExpiry(expiresAtMs: offer.expiresAtMs, nowMs: friends.clockMs), locale: locale,
            busy: friends.isBusy(offer: offer), failure: friends.offerFailures[offer.teamId],
            onJoin: { Task { if await friends.acceptOffer(offer) { onJoined(offer) } } },
            onDecline: { Task { await friends.declineOffer(offer) } })
            .accessibilityIdentifier("friends.offer.\(offer.teamId)")
    }

    // MARK: Requests (D4)

    @ViewBuilder
    private func requests(_ sections: FriendsListSections) -> some View {
        if !sections.received.isEmpty || !sections.sent.isEmpty {
            Text("friends_section_requests")
                .font(.title3.weight(.semibold))
                .listRowSeparator(.hidden)
                .id(Self.requestsID)
                .accessibilityAddTraits(.isHeader)
            if !sections.received.isEmpty {
                subheading("friends_requests_received")
                ForEach(rows(sections.received, expanded: showAllReceived)) { requestRow($0) }
                showAll(sections.received.count, expanded: showAllReceived, identifier: "friends.requests.showall") { showAllReceived = true }
            }
            if !sections.sent.isEmpty {
                subheading("friends_requests_sent")
                ForEach(rows(sections.sent, expanded: showAllSent)) { requestRow($0) }
                showAll(sections.sent.count, expanded: showAllSent, identifier: "friends.requests.showall") { showAllSent = true }
            }
        }
    }

    private func subheading(_ title: LocalizedStringKey) -> some View {
        Text(title)
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.secondary)
            .listRowSeparator(.hidden)
            .accessibilityAddTraits(.isHeader)
    }

    /// Twenty rows, then a final row that shows the rest (incoming requests are not capped by the backend).
    private func rows<Item>(_ items: [Item], expanded: Bool) -> [Item] {
        expanded ? items : Array(items.prefix(FriendsLogic.sectionRows))
    }

    @ViewBuilder
    private func showAll(_ count: Int, expanded: Bool, identifier: String, action: @escaping () -> Void) -> some View {
        if !expanded && count > FriendsLogic.sectionRows {
            Button(action: action) { Text("friends_show_all \(count)") }
                .buttonStyle(.borderless)
                .accessibilityIdentifier(identifier)
        }
    }

    private func requestRow(_ card: FriendCard) -> some View {
        FriendRequestRow(
            card: card, busy: friends.isBusy(card.uid),
            onAccept: { Task { await friends.respond(.accept, to: card) } },
            onDecline: { Task { await friends.respond(.decline, to: card) } },
            onCancel: { Task { await friends.respond(.cancel, to: card) } })
            .accessibilityIdentifier("friends.request.\(card.uid)")
    }

    // MARK: Friends (D3)

    private func friendRow(_ card: FriendCard, strings: TeamsListStrings) -> some View {
        let unread = friends.unread(of: card)
        let preview = TeamsListLogic.pointerPreview(card.lastMessage, uid: friends.uid ?? "", strings: strings)
        return Button {
            onOpen(card.uid)
        } label: {
            FriendRow(card: card, unread: unread, preview: preview, time: TeamsTimeText.listTime(atMs: card.lastAtMs, locale: locale))
        }
        .buttonStyle(.plain)
        .listRowBackground(card.uid == selectedChat ? Color.accentColor.opacity(0.12) : Color.clear)
        .accessibilityIdentifier("friends.row.\(card.uid)")
    }
}

/// One friend in the list: the face, the name and time, the preview and the unread badge. Unread rows have a bold
/// name and preview.
private struct FriendRow: View {
    let card: FriendCard
    let unread: Int
    let preview: String
    let time: String

    var body: some View {
        HStack(spacing: 12) {
            PersonAvatar(id: card.uid, name: card.displayName, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(verbatim: card.displayName)
                        .font(.body.weight(unread > 0 ? .bold : .regular))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Text(time).font(.caption).foregroundStyle(unread > 0 ? Color.primary : Color.secondary)
                }
                HStack(alignment: .center, spacing: 8) {
                    Text(verbatim: preview)
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

/// One team invitation (D27): the team's face and name, who invited you, when it lapses, and Join and Decline, or Dismiss once it
/// has lapsed. The names are the team's and the sender's at the time of the offer, shown as plain text. The row is disabled while
/// one of its actions is in flight, and a failure shows under it (D30).
private struct FriendOfferRow: View {
    let offer: TeamOffer
    let expiry: OfferExpiry
    let locale: Locale
    let busy: Bool
    let failure: FriendsWorkspace.OfferFailure?
    let onJoin: () -> Void
    /// Decline on a live offer, Dismiss on an expired one: both are `declineOffer`.
    let onDecline: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            TeamAvatar(teamID: offer.teamId, name: offer.teamName, size: 40)
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: offer.teamName).font(.body.weight(.semibold)).lineLimit(1)
                Text(verbatim: String(localized: "friends_offer_invited_by \(offer.fromName)")).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                expiryLine
                HStack(spacing: 10) {
                    switch expiry {
                    case .live:
                        Button("friends_offer_join", action: onJoin).buttonStyle(.borderedProminent)
                            .accessibilityIdentifier("friends.offer.join")
                        Button("friends_request_decline", action: onDecline).buttonStyle(.bordered)
                            .accessibilityIdentifier("friends.offer.decline")
                    case .expired:
                        Button("friends_offer_dismiss", action: onDecline).buttonStyle(.bordered)
                            .accessibilityIdentifier("friends.offer.dismiss")
                    }
                }
                .controlSize(.small)
                .padding(.top, 2)
                if let failure {
                    Text(LocalizedStringKey(failure.key)).font(.footnote).foregroundStyle(.red)
                        .accessibilityIdentifier("friends.offer.error")
                }
            }
        }
        .padding(.vertical, 4)
        .disabled(busy)
        .accessibilityElement(children: .contain)
    }

    /// `friends_offer_expires(x)` with the system's relative time in the future, or `friends_offer_expired`.
    @ViewBuilder
    private var expiryLine: some View {
        switch expiry {
        case let .live(unit, value):
            Text(verbatim: String(localized: "friends_offer_expires \(FriendsLogic.relativeExpiry(unit: unit, value: value, locale: locale))"))
                .font(.caption).foregroundStyle(.secondary)
        case .expired:
            Text("friends_offer_expired").font(.caption).foregroundStyle(.secondary)
        }
    }
}

/// One request: the face, the name, the `@username`, the sender's note as plain text (two lines at most) and the
/// actions of its direction. The row is disabled while one of them is in flight.
private struct FriendRequestRow: View {
    let card: FriendCard
    let busy: Bool
    let onAccept: () -> Void
    let onDecline: () -> Void
    let onCancel: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            PersonAvatar(id: card.uid, name: card.displayName, size: 40)
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: card.displayName).font(.body.weight(.semibold)).lineLimit(1)
                if let handle = card.handle { Text(verbatim: "@" + handle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                if let note = card.note { Text(verbatim: note).font(.subheadline).foregroundStyle(.secondary).lineLimit(2) }
                HStack(spacing: 10) {
                    if card.state == .incoming {
                        Button("friends_request_accept", action: onAccept).buttonStyle(.borderedProminent)
                        Button("friends_request_decline", action: onDecline).buttonStyle(.bordered)
                    } else {
                        Text("friends_request_sent").font(.subheadline).foregroundStyle(.secondary)
                        Button("friends_request_cancel", action: onCancel).buttonStyle(.bordered)
                    }
                }
                .controlSize(.small)
                .padding(.top, 2)
            }
        }
        .padding(.vertical, 4)
        .disabled(busy)
        .accessibilityElement(children: .contain)
    }
}

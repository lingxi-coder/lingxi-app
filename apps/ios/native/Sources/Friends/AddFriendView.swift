import SwiftUI
import UIKit

/// Add friend (docs/FRIENDS-CLIENTS.md D6, D7): one field that finds someone by
/// `@username`, email or a friend link. Username fragments search as the person types, and selecting a result
/// opens the same confirmation card as an exact lookup.
struct AddFriendView: View {
    @Environment(FriendsWorkspace.self) private var friends
    /// Opens the chat with a new (or already) friend.
    let onOpenChat: (String) -> Void

    private enum LinkChange: Identifiable {
        case reset, off
        var id: Self { self }
    }

    @State private var text = ""
    @State private var lookup = FriendLookup()
    @State private var searchTask: Task<Void, Never>?
    @State private var copied = false
    @State private var change: LinkChange?
    @State private var linkBusy = false
    @State private var linkError = ""
    @FocusState private var focused: Bool

    private var proof: FriendProof? {
        let parsed = FriendsLogic.classify(text)
        if case let .handle(handle) = parsed { return .handle(handle.lowercased()) }
        return parsed
    }

    private var canSearch: Bool {
        guard let proof, !lookup.looking, !lookup.confirming else { return false }
        if case .handle = proof { return lookup.matches?.isEmpty != false && friends.handleSearchRetryAt == nil }
        return true
    }

    var body: some View {
        List {
            findSection
            if lookup.result != nil {
                Section { FriendResultCard(lookup: lookup, onOpenChat: onOpenChat) }
            }
            linkSection
        }
        .listStyle(.insetGrouped)
        .navigationTitle("friends_add")
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: .friends)
        .task(id: friends.uid) { await friends.loadProfile() }
        .onChange(of: proof, initial: true) { _, next in
            searchTask?.cancel()
            lookup.clear()
            if case .handle = next { search(afterTyping: true) }
        }
        .task(id: friends.handleSearchRetryAt) {
            guard let retryAt = friends.handleSearchRetryAt else { return }
            try? await ContinuousClock().sleep(until: retryAt)
            guard !Task.isCancelled, friends.endHandleSearchCooldown(at: retryAt) else { return }
            if case .handle = proof { search(afterTyping: true) }
        }
        .onDisappear { searchTask?.cancel() }
        .onChange(of: friends.authority) { _, _ in
            searchTask?.cancel()
            text = ""; lookup.clear(); change = nil
        }
        .onChange(of: friends.profile?.friendCode) { _, _ in copied = false }
        .confirmationDialog(
            Text(LocalizedStringKey(change == .off ? "friends_link_off_title" : "friends_link_reset_title")),
            isPresented: Binding(get: { change != nil }, set: { if !$0 { change = nil } }),
            titleVisibility: .visible, presenting: change
        ) { pending in
            Button(LocalizedStringKey(pending == .off ? "friends_link_off" : "friends_link_reset"), role: .destructive) {
                run(rotate: pending == .reset)
            }
            Button("common_cancel", role: .cancel) {}
        } message: { pending in
            Text(LocalizedStringKey(pending == .off ? "friends_link_off_body" : "friends_link_reset_body"))
        }
        .accessibilityIdentifier("friends.add.screen")
    }

    // MARK: My friend link (D6.1)

    private var linkSection: some View {
        Section {
            if let code = friends.profile?.friendCode {
                Text(verbatim: code.link)
                    .font(.footnote.monospaced())
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .privacySensitive()
                    .accessibilityIdentifier("friends.link.text")
                HStack(spacing: 20) {
                    Button {
                        UIPasteboard.general.string = code.link
                        copied = true
                    } label: {
                        Label("friends_link_copy", systemImage: "doc.on.doc").minimumHitTarget(minWidth: 0)
                    }
                    .accessibilityIdentifier("friends.link.copy")
                    ShareLink(item: String(localized: "friends_link_share_message") + " " + code.link) {
                        Label("friends_link_share", systemImage: "square.and.arrow.up").minimumHitTarget(minWidth: 0)
                    }
                    .accessibilityIdentifier("friends.link.share")
                }
                .buttonStyle(.borderless)
                if copied { Text("friends_link_copied").font(.footnote).foregroundStyle(.secondary) }
                HStack(spacing: 20) {
                    Button { change = .reset } label: { Text("friends_link_reset").minimumHitTarget(minWidth: 0) }
                        .accessibilityIdentifier("friends.link.reset")
                    Button(role: .destructive) { change = .off } label: { Text("friends_link_off").minimumHitTarget(minWidth: 0) }
                        .accessibilityIdentifier("friends.link.off")
                }
                .buttonStyle(.borderless)
                .disabled(linkBusy)
            } else if friends.profile != nil {
                Text("friends_link_is_off").foregroundStyle(.secondary)
                Button("friends_link_create") { run(rotate: true) }
                    .disabled(linkBusy)
                    .accessibilityIdentifier("friends.link.create")
            } else if !friends.profileError.isEmpty && !friends.profileLoading {
                VStack(alignment: .leading, spacing: 8) {
                    Text(LocalizedStringKey(FriendsLogic.errorKey(friends.profileError))).foregroundStyle(.red)
                    Button("common_retry") { Task { await friends.loadProfile() } }
                        .accessibilityIdentifier("friends.link.retry")
                }
            } else {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("teams_loading").foregroundStyle(.secondary)
                }
            }
            if !linkError.isEmpty {
                Text(LocalizedStringKey(FriendsLogic.errorKey(linkError))).font(.footnote).foregroundStyle(.red)
            }
        } header: {
            Text("friends_link_title")
        } footer: {
            Text("friends_link_disclosure")
        }
    }

    /// `rotateCode` (a new link; the old one stops working at once) or `revokeCode` (no link).
    private func run(rotate: Bool) {
        guard !linkBusy else { return }
        linkBusy = true; linkError = ""
        Task {
            linkError = await friends.changeFriendLink(rotate: rotate) ?? ""
            linkBusy = false
        }
    }

    // MARK: Find someone (D6.2, D7)

    private var findSection: some View {
        Section {
            TextField("friends_search_placeholder", text: $text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .focused($focused)
                .onSubmit { search() }
                .disabled(lookup.confirming)
                .accessibilityIdentifier("friends.search.field")
            Button {
                search()
            } label: {
                HStack(spacing: 8) {
                    if lookup.looking { ProgressView() }
                    Text("friends_search_action")
                }
            }
            .disabled(!canSearch)
            .accessibilityIdentifier("friends.search.action")
            if lookup.result != nil, lookup.matches != nil {
                Button("friends_search_back") { lookup.clearSelection() }
                    .disabled(lookup.confirming)
            }
            if lookup.result == nil, let matches = lookup.matches {
                if matches.isEmpty {
                    Text("friends_error_not_found").foregroundStyle(.secondary)
                } else {
                    ForEach(matches, id: \.uid) { result in
                        Button {
                            focused = false
                            lookup.select(result)
                        } label: {
                            HStack(spacing: 12) {
                                PersonAvatar(id: result.uid, name: result.displayName, size: 32)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(result.displayName).foregroundStyle(.primary)
                                    if let handle = result.handle {
                                        Text("@\(handle)").font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                        }
                    }
                }
            }
            if lookup.result == nil, !searchError.isEmpty {
                Text(LocalizedStringKey(FriendsLogic.errorKey(searchError))).font(.footnote).foregroundStyle(.red)
                    .accessibilityIdentifier("friends.search.error")
            }
        } header: {
            Text("friends_search_title")
        } footer: {
            if proof == nil { Text("friends_search_hint") }
        }
    }

    private var searchError: String {
        if case .handle = proof, friends.handleSearchRetryAt != nil { return "RATE_LIMITED" }
        return lookup.error
    }

    private func search(afterTyping: Bool = false) {
        guard let proof, canSearch, !afterTyping || lookup.matches == nil else { return }
        if !afterTyping { focused = false }
        searchTask?.cancel()
        searchTask = Task {
            if afterTyping { try? await Task.sleep(for: .milliseconds(350)) }
            guard !Task.isCancelled else { return }
            if case let .handle(handle) = proof {
                await lookup.searchHandles(handle, in: friends)
            } else {
                await lookup.lookup(proof, in: friends)
            }
        }
    }
}

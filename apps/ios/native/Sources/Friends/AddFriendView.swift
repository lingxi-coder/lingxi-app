import SwiftUI

/// Add friend (docs/FRIENDS-CLIENTS.md D6, D7): one field that finds someone by
/// `@username`, email or a friend link. Username fragments search as the person types, and selecting a result
/// opens the same confirmation card as an exact lookup.
struct AddFriendView: View {
    @Environment(FriendsWorkspace.self) private var friends
    /// Opens the chat with a new (or already) friend.
    let onOpenChat: (String) -> Void

    @State private var text = ""
    @State private var lookup = FriendLookup()
    @State private var searchTask: Task<Void, Never>?
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
        }
        .listStyle(.insetGrouped)
        .navigationTitle("friends_add")
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: .friends)
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
            text = ""; lookup.clear()
        }
        .accessibilityIdentifier("friends.add.screen")
    }

    // MARK: Find someone (D6.2, D7)

    private var findSection: some View {
        Section {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary).accessibilityHidden(true)
                TextField("friends_search_placeholder", text: $text)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.search)
                    .focused($focused)
                    .onSubmit { search() }
                    .disabled(lookup.confirming)
                    .accessibilityIdentifier("friends.search.field")
                if !text.isEmpty {
                    Button { text = "" } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(.secondary)
                            .minimumHitTarget(minWidth: 44)
                    }
                    .buttonStyle(.borderless)
                    .disabled(lookup.confirming)
                    .accessibilityLabel(Text("friends_search_clear"))
                }
            }
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
                                Spacer(minLength: 8)
                                Image(systemName: "chevron.forward")
                                    .font(.footnote.weight(.semibold))
                                    .foregroundStyle(.tertiary)
                                    .accessibilityHidden(true)
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

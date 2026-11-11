import SwiftUI
import UIKit

/// Add friend (docs/FRIENDS-CLIENTS.md D6, D7): the viewer's own friend link, and one field that finds someone by
/// `@username`, email or a friend link. A lookup runs only when the person asks (they are rate limited); its result is
/// a card with the one action their relation allows. The rules are `FriendsLogic`; the flow is `FriendLookup`.
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
    @State private var copied = false
    @State private var change: LinkChange?
    @State private var linkBusy = false
    @State private var linkError = ""
    @FocusState private var focused: Bool

    private var proof: FriendProof? { FriendsLogic.classify(text) }

    var body: some View {
        List {
            linkSection
            findSection
            if lookup.result != nil {
                Section { FriendResultCard(lookup: lookup, onOpenChat: onOpenChat) }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("friends_add")
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: .friends)
        .task(id: friends.uid) { await friends.loadProfile() }
        // Editing the field, or another account, forgets the card (D6, D19).
        .onChange(of: text) { _, _ in lookup.clear() }
        .onChange(of: friends.authority) { _, _ in text = ""; lookup.clear(); change = nil }
        .onChange(of: friends.profile?.friendCode) { _, _ in copied = false }
        .confirmationDialog(
            Text(LocalizedStringKey(change == .off ? "friends_link_off_confirm" : "friends_link_reset_confirm")),
            isPresented: Binding(get: { change != nil }, set: { if !$0 { change = nil } }),
            titleVisibility: .visible, presenting: change
        ) { pending in
            Button(LocalizedStringKey(pending == .off ? "friends_link_off" : "friends_link_reset"), role: .destructive) {
                run(rotate: pending == .reset)
            }
            Button("common_cancel", role: .cancel) {}
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
                        Label("friends_link_copy", systemImage: "doc.on.doc")
                    }
                    .accessibilityIdentifier("friends.link.copy")
                    ShareLink(item: String(localized: "friends_link_share_message") + " " + code.link) {
                        Label("friends_link_share", systemImage: "square.and.arrow.up")
                    }
                    .accessibilityIdentifier("friends.link.share")
                }
                .buttonStyle(.borderless)
                if copied { Text("friends_link_copied").font(.footnote).foregroundStyle(.secondary) }
                HStack(spacing: 20) {
                    Button("friends_link_reset") { change = .reset }
                        .accessibilityIdentifier("friends.link.reset")
                    Button("friends_link_off", role: .destructive) { change = .off }
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
                .onSubmit(search)
                .accessibilityIdentifier("friends.search.field")
            Button {
                search()
            } label: {
                HStack(spacing: 8) {
                    if lookup.looking { ProgressView() }
                    Text("friends_search_action")
                }
            }
            .disabled(proof == nil || lookup.looking)
            .accessibilityIdentifier("friends.search.action")
            if lookup.result == nil, !lookup.error.isEmpty {
                Text(LocalizedStringKey(FriendsLogic.errorKey(lookup.error))).font(.footnote).foregroundStyle(.red)
                    .accessibilityIdentifier("friends.search.error")
            }
        } header: {
            Text("friends_search_title")
        } footer: {
            if proof == nil { Text("friends_search_hint") }
        }
    }

    private func search() {
        guard let proof, !lookup.looking else { return }
        focused = false
        Task { await lookup.lookup(proof, in: friends) }
    }
}

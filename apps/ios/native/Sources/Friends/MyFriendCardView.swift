import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

struct MyFriendCardView: View {
    @Environment(FriendsWorkspace.self) private var friends
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let onProfile: () -> Void
    let onBlocked: () -> Void

    private enum LinkChange: Identifiable {
        case reset, off
        var id: Self { self }
    }

    @State private var copied = false
    @State private var change: LinkChange?
    @State private var linkBusy = false
    @State private var linkError = ""

    var body: some View {
        List {
            if let profile = friends.profile {
                HStack(spacing: 14) {
                    PersonAvatar(id: profile.uid, name: profile.displayName, size: 56)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(profile.displayName).font(.title3.weight(.semibold))
                        if let handle = profile.handle {
                            Text("@\(handle)").font(.subheadline).foregroundStyle(.secondary)
                        }
                    }
                }
                .accessibilityElement(children: .combine)
            }
            linkSection
            Section {
                Button(action: onProfile) {
                    Label("friends_profile_title", systemImage: "person.text.rectangle")
                }
                Button(action: onBlocked) {
                    Label("friends_blocked_title", systemImage: "hand.raised")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("friends_my_card")
        .navigationBarTitleDisplayMode(.inline)
        .hidesTabBarWhenPushed(in: .friends)
        .task(id: friends.uid) { await friends.loadProfile() }
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
        .accessibilityIdentifier("friends.me.card")
    }

    private var actionLayout: AnyLayout {
        dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 4))
            : AnyLayout(HStackLayout(spacing: 20))
    }


    private var linkSection: some View {
        Section {
            if let code = friends.profile?.friendCode {
                FriendLinkQR(link: code.link)
                Text(verbatim: code.link)
                    .font(.footnote.monospaced())
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .privacySensitive()
                    .accessibilityIdentifier("friends.link.text")
                actionLayout {
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
                actionLayout {
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

}

private struct FriendLinkQR: View {
    private static let context = CIContext()
    let link: String

    private var image: CGImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(link.utf8)
        filter.correctionLevel = "M"
        guard let code = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 6, y: 6)) else { return nil }
        return Self.context.createCGImage(code, from: code.extent)
    }

    var body: some View {
        if let image {
            Image(uiImage: UIImage(cgImage: image))
                .interpolation(.none)
                .resizable()
                .scaledToFit()
                .frame(width: 164, height: 164)
                .padding(24)
                .background(.white, in: RoundedRectangle(cornerRadius: 14))
                .frame(maxWidth: .infinity)
                .accessibilityLabel(Text("friends_link_qr"))
                .accessibilityIdentifier("friends.link.qr")
                .privacySensitive()
        }
    }
}

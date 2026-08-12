import SwiftUI

/// The transcript scroller shared by the conversation and by local-app
/// generation.
///
/// Both surfaces show the same thing — an assistant talking, at length, while
/// you watch — so they get the same scrolling behaviour rather than two
/// implementations that drift. What lives here is exactly the part that is
/// identical: the bottom anchor, the "stay pinned until the reader drags away
/// from the bottom" rule, and the scroll-on-focus nudge. What each
/// caller keeps is its own content and its own idea of when new content
/// arrived.
///
/// `followsLatest` is a binding rather than internal state because the caller
/// re-arms it on appear (a transcript reopened from the library starts pinned
/// to the newest message, regardless of where the reader left it). The bottom
/// anchor does not clear the flag on disappearance: a growing streaming row
/// can temporarily push that marker off-screen before the follow scroll runs.
struct TranscriptScroll<Follow: Equatable, Content: View>: View {
    /// Changes to this value mean "new content arrived" and trigger a scroll —
    /// but only when `followsLatest` is true. Callers compose whatever set of
    /// signals matters to them into one `Equatable` value.
    let follow: Follow

    /// Whether the reader is currently parked at the bottom. Re-armable by the
    /// caller and set back to true when the bottom anchor returns into view.
    @Binding var followsLatest: Bool

    /// Composer focus. A keyboard coming up must reveal the newest content
    /// even when the reader had scrolled away — this scroll is unconditional,
    /// unlike the `follow` one.
    var focused: Bool = false

    /// Identifier for UI tests, applied to the scroll view.
    var accessibilityIdentifier: String?

    @State private var followState = TranscriptScrollFollowState()

    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    content()
                    Color.clear
                        .frame(height: 1)
                        .id(Self.bottomAnchor)
                        .onAppear {
                            followsLatest = followState.bottomVisibilityChanged(
                                true,
                                followsLatest: followsLatest
                            )
                        }
                        .onDisappear {
                            followsLatest = followState.bottomVisibilityChanged(
                                false,
                                followsLatest: followsLatest
                            )
                        }
                }
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 16).padding(.top, 18).padding(.bottom, 8)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .modifier(OptionalAccessibilityIdentifier(identifier: accessibilityIdentifier))
            // A bottom marker can disappear simply because a streamed row grew
            // before the pending scroll has been applied. It is not evidence
            // that the user scrolled away, so follow state is changed only by
            // an upward user drag or by the marker returning into view.
            .simultaneousGesture(
                DragGesture(minimumDistance: 8)
                    .onChanged { value in
                        followsLatest = followState.dragChanged(
                            translationHeight: value.translation.height,
                            followsLatest: followsLatest
                        )
                    }
                    .onEnded { _ in
                        followsLatest = followState.dragEnded(
                            followsLatest: followsLatest
                        )
                    }
            )
            .onAppear {
                scrollToLatest(using: proxy, animated: false, requiresFollow: false)
            }
            .onChange(of: follow) { _, _ in
                guard followsLatest else { return }
                scrollToLatest(using: proxy, animated: true, requiresFollow: true)
            }
            .onChange(of: focused) { _, isFocused in
                guard isFocused else { return }
                scrollToLatest(using: proxy, animated: true, requiresFollow: false)
            }
        }
    }

    private func scrollToLatest(
        using proxy: ScrollViewProxy,
        animated: Bool,
        requiresFollow: Bool
    ) {
        // Wait for the streamed row's replacement layout to be committed. A
        // synchronous scroll can target the old bottom position, especially
        // when several text deltas arrive in one SwiftUI transaction.
        Task { @MainActor in
            await Task.yield()
            guard !requiresFollow || followsLatest else { return }
            if animated {
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
                }
            } else {
                proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
            }
        }
    }

    /// Stable id of the zero-height view pinned below the content. Scrolling to
    /// a marker rather than to the last item keeps the behaviour correct when
    /// the last item is itself growing (a streaming block).
    static var bottomAnchor: String { "bottom" }
}

struct TranscriptScrollFollowState {
    private(set) var isBottomVisible = false

    mutating func bottomVisibilityChanged(
        _ isVisible: Bool,
        followsLatest: Bool
    ) -> Bool {
        isBottomVisible = isVisible
        guard isVisible, !followsLatest else { return followsLatest }
        return true
    }

    func dragChanged(
        translationHeight: CGFloat,
        followsLatest: Bool
    ) -> Bool {
        guard translationHeight < 0, followsLatest else { return followsLatest }
        return false
    }

    func dragEnded(followsLatest: Bool) -> Bool {
        guard isBottomVisible, !followsLatest else { return followsLatest }
        return true
    }
}

/// Applies an accessibility identifier only when one was supplied, so a caller
/// that has no UI test to satisfy does not stamp an empty identifier.
private struct OptionalAccessibilityIdentifier: ViewModifier {
    let identifier: String?

    func body(content: Content) -> some View {
        if let identifier {
            content.accessibilityIdentifier(identifier)
        } else {
            content
        }
    }
}

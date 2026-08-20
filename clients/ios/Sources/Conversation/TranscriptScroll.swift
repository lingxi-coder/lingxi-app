import SwiftUI

/// The transcript scroller shared by the conversation and by local-app
/// generation.
///
/// Both surfaces show the same thing — an assistant talking, at length, while
/// you watch — so they get the same scrolling behaviour rather than two
/// implementations that drift. What lives here is exactly the part that is
/// identical: the bottom anchor, the "stay pinned until the reader drags away
/// from the bottom" rule with a delayed resume, and the scroll-on-focus nudge.
/// What each caller keeps is its own content and its own idea of when new content
/// arrived.
///
/// `followsLatest` is a binding rather than internal state because the caller
/// re-arms it when the visible session changes. The bottom anchor does not
/// clear the flag on disappearance: a growing streaming row or a modal
/// transition can temporarily push that marker off-screen before the follow
/// scroll runs.
struct TranscriptScroll<Follow: Equatable, Content: View>: View {
    /// Changes to this value mean "new content arrived" and trigger a scroll —
    /// but only when `followsLatest` is true. Callers compose whatever set of
    /// signals matters to them into one `Equatable` value.
    let follow: Follow

    /// Whether the reader is currently parked at the bottom. Re-armable by the
    /// caller, set back to true when the bottom anchor returns into view, or
    /// after the detached-reader cooldown when new content arrives.
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
                // NOT a LazyVStack. This stack has two children — the caller's
                // content and the bottom anchor — so it provides no laziness of
                // its own (`ConversationTimelineView` owns the real LazyVStack).
                // What it did provide was a single child of unknown height for
                // `.defaultScrollAnchor(.bottom)` to measure, which is how the
                // transcript came up blank.
                VStack(alignment: .leading, spacing: 0) {
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
            // Chat content is newest-at-the-bottom. Keep that as the default
            // anchor when SwiftUI re-lays out the container (for example when
            // a sheet changes the available presentation size) instead of
            // falling back to the first row.
            .defaultScrollAnchor(.bottom)
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
                            followsLatest: followsLatest,
                            now: Date()
                        )
                    }
                    .onEnded { _ in
                        followsLatest = followState.dragEnded(
                            followsLatest: followsLatest,
                            now: Date()
                        )
                    }
            )
            .onAppear {
                // A modal can temporarily recreate this surface. Restore the
                // bottom only when the reader was already following the latest
                // message; otherwise preserve the user's reading position.
                scrollToLatest(using: proxy, animated: false, requiresFollow: true)
            }
            .onChange(of: follow) { _, _ in
                if !followsLatest {
                    let resumed = followState.autoResumeIfTimedOut(
                        followsLatest: followsLatest,
                        now: Date(),
                        after: TranscriptScrollFollowState.automaticFollowDelay
                    )
                    guard resumed else { return }
                    followsLatest = true
                }
                // Unanimated on purpose. A streamed turn changes `follow` tens
                // of times per second; a 200ms animation per change is always
                // interrupted by the next one, and the pile-up is what reads as
                // the transcript jittering. The focus scroll below is a
                // discrete user action and stays animated.
                scrollToLatest(using: proxy, animated: false, requiresFollow: true)
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
    /// A detached reader gets a grace period before live updates may resume
    /// automatic following. The timeout is evaluated on the next content
    /// update rather than by a timer, so a quiet transcript never jumps by
    /// itself and modal presentation cannot fire a hidden scroll.
    static let automaticFollowDelay: TimeInterval = 30

    private(set) var isBottomVisible = false
    private var detachedAt: Date?

    mutating func bottomVisibilityChanged(
        _ isVisible: Bool,
        followsLatest: Bool
    ) -> Bool {
        isBottomVisible = isVisible
        if isVisible {
            detachedAt = nil
        }
        guard isVisible, !followsLatest else { return followsLatest }
        return true
    }

    mutating func dragChanged(
        translationHeight: CGFloat,
        followsLatest: Bool,
        now: Date = Date()
    ) -> Bool {
        guard translationHeight < 0, followsLatest else { return followsLatest }
        // Keep the first transition timestamp, then refresh it when the drag
        // ends so the cooldown starts after the user's last scroll gesture.
        if detachedAt == nil {
            detachedAt = now
        }
        return false
    }

    mutating func dragEnded(
        followsLatest: Bool,
        now: Date = Date()
    ) -> Bool {
        guard !followsLatest else {
            detachedAt = nil
            return followsLatest
        }
        guard isBottomVisible else {
            detachedAt = now
            return followsLatest
        }
        detachedAt = nil
        return true
    }

    mutating func autoResumeIfTimedOut(
        followsLatest: Bool,
        now: Date,
        after delay: TimeInterval
    ) -> Bool {
        guard !followsLatest,
              let detachedAt,
              now.timeIntervalSince(detachedAt) >= delay
        else { return false }

        self.detachedAt = nil
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

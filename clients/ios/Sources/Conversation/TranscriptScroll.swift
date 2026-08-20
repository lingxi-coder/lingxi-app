import SwiftUI

/// The transcript scroller shared by the conversation and by local-app
/// generation.
///
/// Both surfaces show the same thing — an assistant talking, at length, while
/// you watch — so they get the same scrolling behaviour rather than two
/// implementations that drift. What lives here is exactly the part that is
/// identical: the bottom anchor, the "stay pinned while the reader is at the
/// bottom" rule, and the scroll-on-focus nudge. What each caller keeps is its
/// own content and its own idea of when new content arrived.
///
/// `followsLatest` is a binding rather than internal state because the caller
/// re-arms it when the visible session changes. Its value is otherwise driven
/// entirely by `onScrollGeometryChange`: it is true exactly while the scroll
/// offset is within `TranscriptScrollFollowState.bottomSlack` of the bottom.
struct TranscriptScroll<Follow: Equatable, Content: View>: View {
    /// Changes to this value mean "new content arrived" and trigger a scroll —
    /// but only when `followsLatest` is true. Callers compose whatever set of
    /// signals matters to them into one `Equatable` value.
    let follow: Follow

    /// Whether the reader is currently parked at the bottom. Re-armable by the
    /// caller; otherwise kept in sync with the measured scroll offset by
    /// `onScrollGeometryChange`.
    @Binding var followsLatest: Bool

    /// Composer focus. A keyboard coming up must reveal the newest content
    /// even when the reader had scrolled away — this scroll is unconditional,
    /// unlike the `follow` one.
    var focused: Bool = false

    /// Identifier for UI tests, applied to the scroll view.
    var accessibilityIdentifier: String?

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
            // The reader's position is measured, not inferred. The previous
            // implementation read it from a 1pt marker's onAppear/onDisappear
            // inside a lazy stack — which fire on cell creation and recycling,
            // out of order during layout — plus the sign of a drag translation.
            .onScrollGeometryChange(for: Bool.self) { geometry in
                TranscriptScrollFollowState.isAtBottom(
                    contentSize: geometry.contentSize.height,
                    visibleMaxY: geometry.visibleRect.maxY,
                    slack: TranscriptScrollFollowState.bottomSlack
                )
            } action: { _, isAtBottom in
                followsLatest = isAtBottom
            }
            .modifier(OptionalAccessibilityIdentifier(identifier: accessibilityIdentifier))
            .onAppear {
                // A modal can temporarily recreate this surface. Restore the
                // bottom only when the reader was already following the latest
                // message; otherwise preserve the user's reading position.
                scrollToLatest(using: proxy, animated: false, requiresFollow: true)
            }
            .onChange(of: follow) { _, _ in
                // Unanimated on purpose. A streamed turn changes `follow` tens
                // of times per second; a 200ms animation per change is always
                // interrupted by the next one, and the pile-up is what reads as
                // the transcript jittering.
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
        // Decide BEFORE yielding. The geometry observer writes `followsLatest`
        // from the very layout this scroll is waiting for: content growing by
        // more than `bottomSlack` moves the offset out of the bottom band for
        // one tick, so a guard read after the yield sees `false` and cancels
        // the scroll that the new content asked for — permanently, since
        // nothing re-arms it.
        guard !requiresFollow || followsLatest else { return }
        // Wait for the streamed row's replacement layout to be committed. A
        // synchronous scroll can target the old bottom position, especially
        // when several text deltas arrive in one SwiftUI transaction.
        Task { @MainActor in
            await Task.yield()
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

enum TranscriptScrollFollowState {
    /// How far above the true bottom still counts as "at the bottom". Absorbs
    /// rounding and the one frame between a row growing and the follow scroll
    /// landing, without swallowing a real upward drag.
    static let bottomSlack: CGFloat = 24

    /// Whether the reader is parked at the newest content.
    ///
    /// Measured against `visibleRect`, NOT reconstructed from offset, container
    /// size and insets. Device measurement (iPhone 11, iOS 18) showed
    /// `containerSize` EXCLUDES the content insets while `visibleRect` includes
    /// them, and that this surface's only inset is `contentInsets.top`, never
    /// `.bottom` — so the reconstructed maximum was 92pt too high and this
    /// predicate answered false even with the reader sitting at the bottom.
    /// Asking `visibleRect` avoids the whole question.
    ///
    /// The gap goes negative while the scroll view rubber-bands past the end,
    /// and is zero when the content is too short to scroll, so both cases fall
    /// out of the comparison without a special case.
    static func isAtBottom(
        contentSize: CGFloat,
        visibleMaxY: CGFloat,
        slack: CGFloat
    ) -> Bool {
        contentSize - visibleMaxY <= slack
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

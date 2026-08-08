import SwiftUI

/// The transcript scroller shared by the conversation and by local-app
/// generation.
///
/// Both surfaces show the same thing — an assistant talking, at length, while
/// you watch — so they get the same scrolling behaviour rather than two
/// implementations that drift. What lives here is exactly the part that is
/// identical: the bottom anchor, the "stay pinned only while the reader is
/// already at the bottom" rule, and the scroll-on-focus nudge. What each
/// caller keeps is its own content and its own idea of when new content
/// arrived.
///
/// `followsLatest` is a binding rather than internal state because the caller
/// re-arms it on appear (a transcript reopened from the library starts pinned
/// to the newest message, regardless of where the reader left it).
struct TranscriptScroll<Follow: Equatable, Content: View>: View {
    /// Changes to this value mean "new content arrived" and trigger a scroll —
    /// but only when `followsLatest` is true. Callers compose whatever set of
    /// signals matters to them into one `Equatable` value.
    let follow: Follow

    /// Whether the reader is currently parked at the bottom. Driven by the
    /// bottom anchor's appear/disappear, and re-armable by the caller.
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
            ScrollView(showsIndicators: false) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    content()
                    Color.clear
                        .frame(height: 1)
                        .id(Self.bottomAnchor)
                        .onAppear {
                            if !followsLatest { followsLatest = true }
                        }
                        .onDisappear {
                            if followsLatest { followsLatest = false }
                        }
                }
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 16).padding(.top, 18).padding(.bottom, 8)
            }
            .scrollDismissesKeyboard(.interactively)
            .modifier(OptionalAccessibilityIdentifier(identifier: accessibilityIdentifier))
            .onChange(of: follow) { _, _ in
                guard followsLatest else { return }
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
                }
            }
            .onChange(of: focused) { _, isFocused in
                guard isFocused else { return }
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo(Self.bottomAnchor, anchor: .bottom)
                }
            }
        }
    }

    /// Stable id of the zero-height view pinned below the content. Scrolling to
    /// a marker rather than to the last item keeps the behaviour correct when
    /// the last item is itself growing (a streaming block).
    static var bottomAnchor: String { "bottom" }
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

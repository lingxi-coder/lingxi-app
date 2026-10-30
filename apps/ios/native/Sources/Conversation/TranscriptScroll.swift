import SwiftUI

/// Owns transcript scroll intent and the single native scroll position. Layout
/// changes keep an attached reader at the tail; only a user scroll can detach
/// or rejoin it. Composer focus never changes that reading intent.
struct TranscriptScroll<Follow: Equatable, Content: View>: View {
    let follow: Follow
    var focused: Bool = false
    /// Accepted local sends rejoin even when no transcript row changed yet.
    var latestRequestRevision: UInt64 = 0
    var accessibilityIdentifier: String?
    /// Populated only by the DEBUG history-reading UI fixture.
    var uiTestAccessibilityValue: String? = nil
    var maxContentWidth: CGFloat = 720
    let scrollScope: String
    var alignShortContentToTop = false
    var requestedRowID: String? = nil
    var requestedScrollRevision: UInt64 = 0
    @ViewBuilder var content: () -> Content

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var scrollPosition = ScrollPosition(idType: String.self)
    @State private var followState = TranscriptScrollFollowState()
    @State private var scrollTask: Task<Void, Never>?
    @State private var pendingRowID: String?
    @State private var pendingLatestAnimation: Bool?
    @State private var lastRowRequest: TranscriptScrollRowRequest?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                content()
            }
            .frame(maxWidth: maxContentWidth)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 16).padding(.top, 18).padding(.bottom, 8)
        }
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        // Native anchoring follows every frame of an animated row resize.
        // A deferred edge request alone can run before expansion has finished.
        // History reading and active touches retain their visible position.
        .defaultScrollAnchor(followState.canFollow ? .bottom : .top, for: .sizeChanges)
        .scrollPosition($scrollPosition, anchor: followState.followsLatest ? .bottom : .top)
        .modifier(ShortTranscriptAlignmentModifier(enabled: alignShortContentToTop))
        .scrollIndicators(.hidden)
        .scrollDismissesKeyboard(.interactively)
        .onScrollGeometryChange(for: TranscriptScrollMetrics.self) { geometry in
            TranscriptScrollMetrics(geometry)
        } action: { previous, current in
            if followState.geometryChanged(from: previous, to: current) {
                scrollToLatest(animated: false)
            }
        }
        .onScrollPhaseChange { previousPhase, phase, context in
            if phase == .tracking || (phase == .interacting && previousPhase == .idle) {
                // A newer gesture cancels a row jump waiting for layout too.
                pendingRowID = nil
                pendingLatestAnimation = nil
            }
            followState.phaseChanged(to: phase, isAtBottom: TranscriptScrollMetrics(context.geometry).isAtBottom)
            if followState.isUserInteracting { cancelPendingScroll() }
            if phase == .idle || phase == .decelerating {
                if pendingRowID != nil { performPendingRowScroll() }
                else if let pendingLatestAnimation {
                    scrollToLatest(animated: pendingLatestAnimation, explicit: true)
                } else if phase == .idle { scrollToLatest(animated: false) }
            }
        }
        .modifier(OptionalAccessibilityIdentifier(identifier: accessibilityIdentifier))
        .modifier(OptionalAccessibilityValue(value: uiTestAccessibilityValue))
        .onAppear {
            if !consumeRowRequest() {
                if pendingRowID != nil { performPendingRowScroll() }
                else { scrollToLatest(animated: false) }
            }
        }
        .onDisappear {
            cancelPendingScroll()
            followState.suspendInteraction()
        }
        .onChange(of: follow) { _, _ in
            // Streaming changes do not queue overlapping animations.
            scrollToLatest(animated: false)
        }
        .onChange(of: scrollScope) { _, _ in
            cancelPendingScroll()
            pendingRowID = nil
            pendingLatestAnimation = nil
            lastRowRequest = nil
            scrollPosition = ScrollPosition(idType: String.self)
            followState = TranscriptScrollFollowState()
            // An explicit message target wins over new-session initialization.
            if !consumeRowRequest() { scrollToLatest(animated: false) }
        }
        .onChange(of: requestedScrollRevision) { _, _ in
            _ = consumeRowRequest()
        }
        .onChange(of: latestRequestRevision) { _, _ in
            requestLatest(animated: false)
        }
        .onChange(of: focused) { _, _ in
            // Native keyboard geometry drives the final correction. This also
            // handles focus changes where the keyboard is already presented.
            scrollToLatest(animated: false)
        }
        .overlay(alignment: .bottomTrailing) {
            if !followState.followsLatest {
                Button {
                    requestLatest(animated: true)
                } label: {
                    Image(systemName: "chevron.down")
                        .font(.footnote.weight(.semibold))
                        .frame(width: 32, height: 32)
                        .background(.regularMaterial, in: Circle())
                        .overlay(Circle().strokeBorder(Color.primary.opacity(0.08)))
                }
                .buttonStyle(.plain)
                .padding(.trailing, 16)
                .padding(.bottom, 12)
                .accessibilityLabel("chat_jump_to_latest")
                .accessibilityIdentifier("conversation.jump-to-latest")
            }
        }
    }

    private func cancelPendingScroll() {
        scrollTask?.cancel()
        scrollTask = nil
    }

    private func requestLatest(animated: Bool) {
        pendingRowID = nil
        pendingLatestAnimation = animated
        followState.requestLatest()
        scrollToLatest(animated: animated, explicit: true)
    }

    private func scrollToLatest(animated: Bool, explicit: Bool = false) {
        guard followState.followsLatest,
              explicit ? followState.canApplyExplicitRequest : followState.canFollow else { return }
        cancelPendingScroll()
        let scope = scrollScope
        scrollTask = Task { @MainActor in
            // Coalesce updates after SwiftUI has laid out the current content.
            await Task.yield()
            guard !Task.isCancelled, scope == scrollScope, followState.followsLatest,
                  explicit ? followState.canApplyExplicitRequest : followState.canFollow else { return }
            pendingLatestAnimation = nil
            if animated && !reduceMotion {
                withAnimation(.easeOut(duration: 0.2)) {
                    scrollPosition.scrollTo(edge: .bottom)
                }
            } else {
                scrollPosition.scrollTo(edge: .bottom)
            }
        }
    }

    @discardableResult
    private func consumeRowRequest() -> Bool {
        guard let requestedRowID else { return false }
        let request = TranscriptScrollRowRequest(
            scope: scrollScope, rowID: requestedRowID, revision: requestedScrollRevision
        )
        guard request != lastRowRequest else { return false }
        lastRowRequest = request
        cancelPendingScroll()
        followState.requestRow()
        pendingLatestAnimation = nil
        pendingRowID = requestedRowID
        performPendingRowScroll()
        return true
    }

    private func performPendingRowScroll() {
        guard let rowID = pendingRowID, followState.canApplyExplicitRequest else { return }
        cancelPendingScroll()
        let scope = scrollScope
        scrollTask = Task { @MainActor in
            await Task.yield()
            guard !Task.isCancelled, scope == scrollScope, followState.canApplyExplicitRequest else { return }
            scrollPosition.scrollTo(id: rowID, anchor: .center)
            pendingRowID = nil
        }
    }

}

private struct TranscriptScrollRowRequest: Equatable {
    let scope: String
    let rowID: String
    let revision: UInt64
}

private struct ShortTranscriptAlignmentModifier: ViewModifier {
    let enabled: Bool

    @ViewBuilder
    func body(content: Content) -> some View {
        if enabled {
            content.defaultScrollAnchor(.top, for: .alignment)
        } else {
            content
        }
    }
}

struct TranscriptScrollMetrics: Equatable {
    let contentHeight: CGFloat
    let visibleMinY: CGFloat
    let visibleMaxY: CGFloat
    let viewportSize: CGSize
    let topInset: CGFloat
    let bottomInset: CGFloat

    init(_ geometry: ScrollGeometry) {
        contentHeight = geometry.contentSize.height
        visibleMinY = geometry.visibleRect.minY
        visibleMaxY = geometry.visibleRect.maxY
        viewportSize = geometry.containerSize
        topInset = geometry.contentInsets.top
        bottomInset = geometry.contentInsets.bottom
    }

    var isAtBottom: Bool {
        TranscriptScrollFollowState.isAtBottom(
            contentSize: contentHeight,
            visibleMaxY: visibleMaxY,
            slack: TranscriptScrollFollowState.bottomSlack
        )
    }

    func hasSameLayout(as other: Self) -> Bool {
        contentHeight == other.contentHeight && viewportSize == other.viewportSize
            && topInset == other.topInset && bottomInset == other.bottomInset
    }
}

struct TranscriptScrollFollowState {
    static let bottomSlack: CGFloat = 24

    private(set) var followsLatest = true
    private(set) var phase: ScrollPhase = .idle
    private var userDidScroll = false
    private var requestDuringInteraction = false

    var isUserInteracting: Bool {
        phase == .tracking || phase == .interacting || phase == .decelerating
    }

    var canFollow: Bool { followsLatest && !isUserInteracting }

    /// Explicit controls interrupt momentum but never fight a finger still down.
    var canApplyExplicitRequest: Bool { phase != .tracking && phase != .interacting }

    mutating func suspendInteraction() {
        phase = .idle
        userDidScroll = false
        requestDuringInteraction = false
    }

    mutating func requestLatest() {
        followsLatest = true
        requestDuringInteraction = isUserInteracting
    }

    mutating func requestRow() {
        followsLatest = false
        requestDuringInteraction = isUserInteracting
    }

    mutating func phaseChanged(to next: ScrollPhase, isAtBottom: Bool) {
        if next == .tracking {
            userDidScroll = false
            requestDuringInteraction = false
        }
        if next == .interacting || next == .decelerating { userDidScroll = true }
        if next == .idle {
            // Merely touching the view does not turn concurrent layout growth
            // into a detached reading position. A completed scroll does.
            if userDidScroll && !requestDuringInteraction { followsLatest = isAtBottom }
            userDidScroll = false
            requestDuringInteraction = false
        }
        phase = next
    }

    /// Returns whether a changed layout needs a tail correction. Geometry alone
    /// never changes intent, even if content growth exceeds the near-tail band.
    mutating func geometryChanged(from previous: TranscriptScrollMetrics, to current: TranscriptScrollMetrics) -> Bool {
        let layoutChanged = !current.hasSameLayout(as: previous)
        if !requestDuringInteraction, (phase == .interacting || phase == .decelerating), !layoutChanged,
           abs(current.visibleMinY - previous.visibleMinY) > 0.5 {
            followsLatest = current.isAtBottom
        }
        return canFollow && layoutChanged
    }

    /// visibleRect includes the native scroll insets. Reconstructing the bottom
    /// from containerSize instead loses those insets on iPhone.
    static func isAtBottom(contentSize: CGFloat, visibleMaxY: CGFloat, slack: CGFloat) -> Bool {
        contentSize - visibleMaxY <= slack
    }
}

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

private struct OptionalAccessibilityValue: ViewModifier {
    let value: String?

    func body(content: Content) -> some View {
        #if DEBUG
        if let value {
            content.accessibilityValue(value)
        } else {
            content
        }
        #else
        content
        #endif
    }
}

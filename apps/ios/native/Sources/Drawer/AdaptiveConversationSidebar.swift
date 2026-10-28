import SwiftUI
import UIKit

/// Native split navigation on iPad; a directly manipulated SwiftUI drawer on
/// iPhone. Both layouts share the existing navigation model as their state source.
struct AdaptiveConversationSidebar<Sidebar: View, Detail: View>: View {
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Bindable var navigation: AppNavigationModel
    let gesturesEnabled: Bool
    @ViewBuilder let sidebar: () -> Sidebar
    @ViewBuilder let detail: () -> Detail

    var body: some View {
        if sizeClass == .regular {
            NavigationSplitView(
                columnVisibility: $navigation.columnVisibility,
                preferredCompactColumn: $navigation.compactColumn,
                sidebar: sidebar,
                detail: detail
            )
        } else {
            ConversationSideDrawer(
                isOpen: Binding(
                    get: { navigation.compactColumn == .sidebar },
                    set: { if $0 { navigation.showSidebar() } else { navigation.closeSidebar() } }
                ),
                gesturesEnabled: gesturesEnabled,
                menu: { NavigationStack { sidebar() } },
                content: detail
            )
        }
    }
}

/// Content, scrim and menu share a clamped reveal distance. Drag updates have
/// no animation; only release and explicit open/close actions settle.
private struct ConversationSideDrawer<Menu: View, Content: View>: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Binding var isOpen: Bool
    let gesturesEnabled: Bool
    @ViewBuilder let menu: () -> Menu
    @ViewBuilder let content: () -> Content
    @GestureState private var drag = DrawerDrag()
    @AccessibilityFocusState private var menuFocused: Bool

    private var settlingAnimation: Animation? {
        reduceMotion ? nil : .snappy(duration: 0.3, extraBounce: 0)
    }

    var body: some View {
        GeometryReader { geometry in
            let width = min(360, max(1, geometry.size.width - 56))
            let reveal = min(max((isOpen ? width : 0) + drag.translation, 0), width)
            let progress = reveal / width

            ZStack(alignment: .leading) {
                content()
                    .allowsHitTesting(reveal == 0)
                    .accessibilityHidden(isOpen || drag.direction == .horizontal)

                Color.black.opacity(0.3 * progress)
                    .ignoresSafeArea()
                    .contentShape(Rectangle())
                    .onTapGesture { setOpen(false) }
                    .allowsHitTesting(reveal > 0)
                    .accessibilityHidden(true)

                menu()
                    .frame(width: width, height: geometry.size.height)
                    .offset(x: reveal - width)
                    .opacity(reveal > 0 ? 1 : 0)
                    .allowsHitTesting(isOpen && drag.direction != .horizontal)
                    .disabled(drag.direction == .horizontal)
                    .accessibilityElement(children: .contain)
                    // An offscreen drawer can still be exposed as a modal by
                    // the accessibility server. Make it modal only while it
                    // is open or actively being dragged.
                    .accessibilityAddTraits(isOpen || drag.direction == .horizontal ? .isModal : [])
                    .accessibilityHidden(reveal == 0)
                    .accessibilityFocused($menuFocused)
                    .accessibilityAction(.escape) { setOpen(false) }
            }
            // Native navigation chrome may extend into the system's trailing
            // safe area. A fixed hit rectangle would exclude those controls.
            // Simultaneous recognition preserves scrolling and row taps. Lock
            // intent once; vertical drags never become drawer drags.
            .simultaneousGesture(
                drawerGesture(width: width),
                including: gesturesEnabled ? .all : .subviews
            )
            .animation(settlingAnimation, value: isOpen)
            .animation(settlingAnimation, value: drag.direction == .horizontal)
        }
        .onChange(of: isOpen) { _, open in
            dismissKeyboard()
            menuFocused = open
        }
    }

    private func drawerGesture(width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 12, coordinateSpace: .local)
            .updating($drag) { value, state, transaction in
                guard gesturesEnabled else { return }
                if state.direction == .undecided {
                    guard isOpen || value.startLocation.x <= 24 else {
                        state.direction = .ignored
                        return
                    }
                    let horizontal = abs(value.translation.width) > abs(value.translation.height) * 1.2
                    state.direction = horizontal ? .horizontal : .ignored
                }
                guard state.direction == .horizontal else { return }
                transaction.disablesAnimations = true
                state.translation = value.translation.width
            }
            .onEnded { value in
                guard gesturesEnabled,
                      abs(value.translation.width) > abs(value.translation.height) * 1.2,
                      isOpen || value.startLocation.x <= 24
                else { return }
                let projectedReveal = (isOpen ? width : 0) + value.predictedEndTranslation.width
                setOpen(projectedReveal > width / 2)
            }
    }

    private func setOpen(_ open: Bool) {
        dismissKeyboard()
        withAnimation(settlingAnimation) { isOpen = open }
    }

    private func dismissKeyboard() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

private struct DrawerDrag {
    enum Direction { case undecided, horizontal, ignored }
    var direction: Direction = .undecided
    var translation: CGFloat = 0
}

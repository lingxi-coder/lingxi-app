import SwiftUI

extension View {
    /// A control that looks smaller keeps a hit area of at least 44 x 44 pt (HIG Mobility; docs/TEAMS-UI-POLISH.md U6). Apply it to the
    /// label inside the button, so that the whole area is the control. Text buttons that need no width pass `minWidth: 0`.
    func minimumHitTarget(minWidth: CGFloat = 44, alignment: Alignment = .center) -> some View {
        frame(minWidth: minWidth, minHeight: 44, alignment: alignment).contentShape(Rectangle())
    }
}

extension Animation {
    /// `animation` unless the person asked for Reduce Motion: `withAnimation(.lxMotion(reduceMotion)) { ... }` (U10).
    static func lxMotion(_ reduceMotion: Bool, _ animation: Animation = .default) -> Animation? {
        reduceMotion ? nil : animation
    }
}

/// A strip above a list or a stream: the data on screen is the saved copy, or something failed. The action
/// (Reconnect) is optional.
struct TeamsBanner: View {
    enum Kind { case offline, error }
    let kind: Kind
    let title: LocalizedStringKey
    var detail: LocalizedStringKey?
    var actionTitle: LocalizedStringKey?
    var action: (() -> Void)?

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: kind == .offline ? "wifi.slash" : "exclamationmark.triangle.fill")
                .foregroundStyle(kind == .offline ? Color.secondary : Color.orange)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.subheadline.weight(.semibold))
                if let detail {
                    Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.vertical, 10)
            Spacer(minLength: 8)
            if let actionTitle, let action {
                Button(action: action) {
                    Text(actionTitle).font(.subheadline.weight(.semibold)).minimumHitTarget(minWidth: 0)
                }
                .buttonStyle(.borderless)
            }
        }
        .padding(.horizontal, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(kind == .offline ? Color(uiColor: .secondarySystemBackground) : Color.orange.opacity(0.12))
        .accessibilityElement(children: .contain)
    }
}

/// The count on a row, a section header or the team switcher: `99+` above 99. The text is `.caption2.bold()`: bold text only needs
/// 3:1 against the accent (4.02:1 light, 3.65:1 dark) where semibold caption text would need 4.5:1 (U7).
struct TeamsUnreadBadge: View {
    @ScaledMetric(relativeTo: .caption2) private var minSide: CGFloat = 20
    let count: Int

    var body: some View {
        if let text = TeamsText.badge(count) {
            Text(text)
                .font(.caption2.bold())
                .monospacedDigit()
                .foregroundStyle(.white)
                .padding(.horizontal, 6)
                .frame(minWidth: minSide, minHeight: minSide)
                .background(Color.accentColor, in: Capsule())
                .accessibilityLabel(Text("tabs_badge_unread \(count)"))
        }
    }
}

/// A centred explanation with up to two actions (signed out, loading failed, nothing yet).
struct TeamsStateView<Actions: View>: View {
    let systemImage: String
    let title: LocalizedStringKey
    var message: LocalizedStringKey?
    @ViewBuilder var actions: Actions

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: systemImage)
        } description: {
            if let message { Text(message) }
        } actions: {
            actions
        }
    }
}

extension TeamsStateView where Actions == EmptyView {
    init(systemImage: String, title: LocalizedStringKey, message: LocalizedStringKey? = nil) {
        self.init(systemImage: systemImage, title: title, message: message) { EmptyView() }
    }
}

/// A line of caption text with an optional leading symbol, for notes under forms and cards.
struct TeamsNote: View {
    let text: LocalizedStringKey
    var systemImage: String?

    var body: some View {
        Label {
            Text(text).font(.footnote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        } icon: {
            if let systemImage { Image(systemName: systemImage).foregroundStyle(.secondary) }
        }
        .labelStyle(NoteLabelStyle(hasIcon: systemImage != nil))
    }

    private struct NoteLabelStyle: LabelStyle {
        let hasIcon: Bool
        func makeBody(configuration: Configuration) -> some View {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if hasIcon { configuration.icon.font(.footnote) }
                configuration.title
            }
        }
    }
}

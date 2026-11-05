import SwiftUI

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
            Spacer(minLength: 8)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .font(.subheadline.weight(.semibold))
                    .buttonStyle(.borderless)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(kind == .offline ? Color(uiColor: .secondarySystemBackground) : Color.orange.opacity(0.12))
        .accessibilityElement(children: .contain)
    }
}

/// The count on a row, a section header or the team switcher: `99+` above 99.
struct TeamsUnreadBadge: View {
    let count: Int

    var body: some View {
        if let text = TeamsText.badge(count) {
            Text(text)
                .font(.caption2.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(.white)
                .padding(.horizontal, 6)
                .frame(minWidth: 20, minHeight: 20)
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

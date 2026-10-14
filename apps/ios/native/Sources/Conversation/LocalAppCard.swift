import SwiftUI

private struct OpenLocalAppKey: EnvironmentKey {
    static let defaultValue: ((String) -> Void)? = nil
}

extension EnvironmentValues {
    /// Opens an app made in chat full screen; nil where nothing can open one.
    var openLocalApp: ((String) -> Void)? {
        get { self[OpenLocalAppKey.self] }
        set { self[OpenLocalAppKey.self] = newValue }
    }
}

/// A published app in the conversation: tap it to open the app.
struct LocalAppCard: View {
    let app: PublishedLocalApp

    var body: some View {
        LocalAppCardContent(app: app, chat: LocalApps.chat)
    }
}

private struct LocalAppCardContent: View {
    @Environment(\.theme) private var t
    @Environment(\.openLocalApp) private var openLocalApp
    let app: PublishedLocalApp
    let chat: ChatApps

    @State private var exists: Bool?

    var body: some View {
        // Until the library answers, the app is assumed to be there.
        let openable = openLocalApp != nil && exists != false
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        Button { openLocalApp?(app.id) } label: {
            HStack(spacing: 12) {
                AppCover(name: app.name, category: "", size: 44)
                VStack(alignment: .leading, spacing: 2) {
                    Text(app.name)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(t.text)
                        .lineLimit(1)
                    Text(exists == false
                        ? String(localized: "chat_local_app_deleted")
                        : "\(String(localized: "chat_local_app_kind")) · \(String(localized: "apps_version \(app.revision)"))")
                        .font(.system(size: 12.5))
                        .foregroundStyle(t.text3)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                if openable {
                    Text("common_open")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(t.accent)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 7)
                        .background(t.accent.opacity(0.12), in: Capsule())
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(t.surface, in: shape)
            .overlay(shape.stroke(t.border, lineWidth: 0.5))
            .contentShape(shape)
        }
        .buttonStyle(.plain)
        .disabled(!openable)
        // Asks again whenever the library changes.
        .task(id: chat.apps) { exists = await chat.exists(app.id) }
        .accessibilityIdentifier("conversation.local-app.\(app.id)")
    }
}

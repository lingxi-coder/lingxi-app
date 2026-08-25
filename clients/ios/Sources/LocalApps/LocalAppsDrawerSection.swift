import SwiftUI

struct LocalAppsDrawerSection: View {
    @Environment(\.theme) private var theme
    let apps: [LocalAppSummary]
    let onOpenLibrary: () -> Void
    let onOpenApp: (String) -> Void

    var body: some View {
        VStack(spacing: 6) {
            Button(action: onOpenLibrary) {
                Label("local_apps_create", systemImage: "plus")
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundStyle(theme.accent)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .background(theme.surface, in: .rect(cornerRadius: 10))
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("drawer.apps.create")

            ForEach(apps) { app in
                Button {
                    onOpenApp(app.id)
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: localAppIconSystemName)
                            .font(.system(size: 16))
                            .foregroundStyle(theme.accent)
                            .frame(width: 30, height: 30)
                            .background(theme.accent.opacity(0.1), in: .rect(cornerRadius: 8))
                        VStack(alignment: .leading, spacing: 2) {
                            // Same draft branch as the library card and the
                            // detail header: a shell shows the localized "New
                            // App" / "Creating…" pair, never its placeholder
                            // name.
                            Text(app.displayName)
                                .font(.system(size: 13.5, weight: .semibold))
                                .foregroundStyle(theme.text)
                                .lineLimit(1)
                            Text(app.draftStatusLine ?? app.workflow.label)
                                .font(.caption)
                                .foregroundStyle(theme.text4)
                        }
                        Spacer()
                        Image(systemName: "chevron.right")
                            .font(.caption)
                            .foregroundStyle(theme.text4)
                    }
                    .padding(9)
                    .background(theme.surface, in: .rect(cornerRadius: 10))
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("drawer.apps.row.\(app.id)")
            }

            Button("local_apps_view_all", systemImage: "square.grid.2x2", action: onOpenLibrary)
                .font(.system(size: 13))
                .foregroundStyle(theme.text3)
                .frame(maxWidth: .infinity)
                .padding(10)
                .buttonStyle(.plain)
        }
    }
}

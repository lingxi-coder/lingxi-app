import AppIntents
import SwiftUI
import WidgetKit

struct LocalAppWidgetEntity: AppEntity, Hashable, Sendable {
    static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "Local App")
    static let defaultQuery = LocalAppWidgetEntityQuery()

    let id: String
    let name: String
    let workflow: String

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(
            title: LocalizedStringResource(stringLiteral: name),
            subtitle: LocalizedStringResource(stringLiteral: workflow)
        )
    }
}

struct LocalAppWidgetEntityQuery: EntityQuery {
    func entities(for identifiers: [String]) async throws -> [LocalAppWidgetEntity] {
        let byID = Dictionary(
            uniqueKeysWithValues: LocalAppWidgetSnapshotStore.read().apps.map { app in
                (app.id, LocalAppWidgetEntity(id: app.id, name: app.name, workflow: app.workflow))
            }
        )
        return identifiers.compactMap { byID[$0] }
    }

    func suggestedEntities() async throws -> [LocalAppWidgetEntity] {
        LocalAppWidgetSnapshotStore.read().apps.map {
            LocalAppWidgetEntity(id: $0.id, name: $0.name, workflow: $0.workflow)
        }
    }
}

struct LocalAppWidgetConfigurationIntent: WidgetConfigurationIntent {
    static let title: LocalizedStringResource = "Select Local App"
    static let description = IntentDescription("Choose the Local App opened by this widget.")

    @Parameter(title: "Local App")
    var app: LocalAppWidgetEntity?

    init() {}

    init(app: LocalAppWidgetEntity?) {
        self.app = app
    }
}

struct LocalAppWidgetEntry: TimelineEntry {
    let date: Date
    let app: LocalAppWidgetSnapshot.App?

    var destination: URL? {
        guard let app else { return nil }
        return LocalAppWidgetSnapshotStore.makeOpenURL(appID: app.id)
    }
}

struct LocalAppWidgetProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> LocalAppWidgetEntry {
        LocalAppWidgetEntry(
            date: .now,
            app: LocalAppWidgetSnapshot.App(
                id: "00000000",
                name: "Local App",
                brief: "Open your local app",
                workflow: "ready",
                runtimeState: "stopped",
                updatedAtMs: 0
            )
        )
    }

    func snapshot(
        for configuration: LocalAppWidgetConfigurationIntent,
        in context: Context
    ) async -> LocalAppWidgetEntry {
        LocalAppWidgetEntry(date: .now, app: selectedApp(for: configuration))
    }

    func timeline(
        for configuration: LocalAppWidgetConfigurationIntent,
        in context: Context
    ) async -> Timeline<LocalAppWidgetEntry> {
        let entry = LocalAppWidgetEntry(date: .now, app: selectedApp(for: configuration))
        return Timeline(entries: [entry], policy: .after(.now.addingTimeInterval(15 * 60)))
    }

    private func selectedApp(for configuration: LocalAppWidgetConfigurationIntent) -> LocalAppWidgetSnapshot.App? {
        guard let selectedID = configuration.app?.id else { return nil }
        return LocalAppWidgetSnapshotStore.read().apps.first { $0.id == selectedID }
    }
}

struct LocalAppWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: LocalAppWidgetEntry

    var body: some View {
        Group {
            if let destination = entry.destination {
                Link(destination: destination) {
                    content
                        .containerBackground(.background, for: .widget)
                }
                .widgetURL(destination)
            } else {
                content
                    .containerBackground(.background, for: .widget)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if let app = entry.app {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "app.badge")
                        .font(.headline)
                    Text(app.name)
                        .font(.headline)
                        .lineLimit(1)
                }
                Text(app.brief)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(family == .systemSmall ? 3 : 2)
                Spacer(minLength: 0)
                Label(
                    statusLabel(workflow: app.workflow, runtimeState: app.runtimeState),
                    systemImage: statusSymbol(workflow: app.workflow, runtimeState: app.runtimeState)
                )
                    .font(.caption2)
                    .foregroundStyle(statusColor(workflow: app.workflow, runtimeState: app.runtimeState))
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            .padding()
        } else {
            VStack(spacing: 8) {
                Image(systemName: "app.badge")
                    .font(.title2)
                Text("Local App unavailable")
                    .font(.caption)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding()
        }
    }

    private func statusLabel(workflow: String, runtimeState: String) -> String {
        guard workflow == "ready" else {
            switch workflow {
            case "draft": return "Draft"
            case "building": return "Building"
            case "failed": return "Needs attention"
            default: return workflow.capitalized
            }
        }
        switch runtimeState {
        case "running": return "Running"
        case "starting": return "Starting"
        case "failed": return "Needs attention"
        case "stopped": return "Ready to launch"
        default: return runtimeState.capitalized
        }
    }

    private func statusSymbol(workflow: String, runtimeState: String) -> String {
        guard workflow == "ready" else {
            switch workflow {
            case "draft": return "pencil"
            case "building": return "hammer"
            case "failed": return "exclamationmark.triangle.fill"
            default: return "questionmark.circle"
            }
        }
        switch runtimeState {
        case "running": return "play.circle.fill"
        case "starting": return "clock"
        case "failed": return "exclamationmark.triangle.fill"
        default: return "bolt.fill"
        }
    }

    private func statusColor(workflow: String, runtimeState: String) -> Color {
        guard workflow == "ready" else {
            return workflow == "failed" ? .orange : .secondary
        }
        switch runtimeState {
        case "running": return .green
        case "failed": return .orange
        default: return .secondary
        }
    }
}

struct LocalAppWidget: Widget {
    let kind = LocalAppWidgetSnapshotStore.widgetKind

    var body: some WidgetConfiguration {
        AppIntentConfiguration(
            kind: kind,
            intent: LocalAppWidgetConfigurationIntent.self,
            provider: LocalAppWidgetProvider()
        ) { entry in
            LocalAppWidgetView(entry: entry)
        }
        .configurationDisplayName("Local App")
        .description("Open a Local App directly in Lingxi.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

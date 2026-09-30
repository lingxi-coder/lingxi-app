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
            subtitle: LocalizedStringResource(
                stringLiteral: LocalAppWidgetWorkflow(rawValue: workflow)?.label ?? "Needs attention"
            )
        )
    }
}

/// The widget receives the app workflow through the shared snapshot rather
/// than the generated client-protocol enum. Keep the projection closed over
/// the current publication states so the widget cannot resurrect the retired
/// single `ready` state or treat an unknown future state as launchable.
private enum LocalAppWidgetWorkflow: String {
    case draft
    case publishedUnverified = "published_unverified"
    case publishedVerified = "published_verified"

    var isPublished: Bool {
        switch self {
        case .draft:
            false
        case .publishedUnverified, .publishedVerified:
            true
        }
    }

    var label: String {
        switch self {
        case .draft:
            "Draft"
        case .publishedUnverified:
            "Published (unverified)"
        case .publishedVerified:
            "Published"
        }
    }

    var symbol: String {
        switch self {
        case .draft:
            "pencil"
        case .publishedUnverified:
            "exclamationmark.triangle.fill"
        case .publishedVerified:
            "checkmark.seal.fill"
        }
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
                workflow: LocalAppWidgetWorkflow.draft.rawValue,
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
        guard let state = LocalAppWidgetWorkflow(rawValue: workflow) else {
            return "Needs attention"
        }
        guard state.isPublished else {
            return state.label
        }
        let publication = state.label
        switch runtimeState {
        case "running": return "\(publication) · Running"
        case "starting": return "\(publication) · Starting"
        case "failed": return "\(publication) · Needs attention"
        case "stopped": return "\(publication) · Ready to launch"
        default: return "\(publication) · \(runtimeState.capitalized)"
        }
    }

    private func statusSymbol(workflow: String, runtimeState: String) -> String {
        guard let state = LocalAppWidgetWorkflow(rawValue: workflow) else {
            return "questionmark.circle"
        }
        guard state.isPublished else {
            return state.symbol
        }
        switch runtimeState {
        case "running": return "play.circle.fill"
        case "starting": return "clock"
        case "failed": return "exclamationmark.triangle.fill"
        default: return state.symbol
        }
    }

    private func statusColor(workflow: String, runtimeState: String) -> Color {
        guard let state = LocalAppWidgetWorkflow(rawValue: workflow) else {
            return .orange
        }
        guard state.isPublished else {
            return .secondary
        }
        switch runtimeState {
        case "running": return .green
        case "failed": return .orange
        default: return state == .publishedUnverified ? .orange : .secondary
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

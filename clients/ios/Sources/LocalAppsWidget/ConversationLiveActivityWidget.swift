import SwiftUI

#if canImport(ActivityKit)
    import ActivityKit
    import WidgetKit

    @available(iOS 18.0, *)
    struct ConversationLiveActivityWidget: Widget {
        var body: some WidgetConfiguration {
            ActivityConfiguration(for: ConversationLiveActivityAttributes.self) { context in
                ConversationLiveActivityView(state: context.state, sessionID: context.attributes.sessionID)
            } dynamicIsland: { context in
                DynamicIsland {
                    DynamicIslandExpandedRegion(.leading) {
                        Label("Lingxi", systemImage: "sparkles")
                            .font(.headline)
                    }
                    DynamicIslandExpandedRegion(.trailing) {
                        Text(shortStatus(context.state.status))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    DynamicIslandExpandedRegion(.bottom) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(context.state.title)
                                .font(.subheadline.weight(.semibold))
                                .lineLimit(1)
                            Text(context.state.subtitle)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(2)
                        }
                    }
                } compactLeading: {
                    Image(systemName: "sparkles")
                } compactTrailing: {
                    Text(shortStatus(context.state.status))
                        .font(.caption2)
                } minimal: {
                    Image(systemName: "sparkles")
                }
                .widgetURL(
                    ConversationDeepLink.makeURL(
                        sessionID: context.attributes.sessionID,
                        turnID: context.state.turnID
                    )
                )
            }
        }

        private func shortStatus(_ status: ConversationLiveActivitySnapshot.Status) -> String {
            switch status {
            case .running: return "Run"
            case .waiting: return "Wait"
            case .paused: return "Pause"
            case .completed: return "Done"
            case .failed: return "Fail"
            }
        }
    }

    @available(iOS 18.0, *)
    private struct ConversationLiveActivityView: View {
        let state: ConversationLiveActivityAttributes.ContentState
        let sessionID: String

        var body: some View {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Image(systemName: "sparkles")
                        .font(.headline)
                    Text(state.title)
                        .font(.headline)
                        .lineLimit(1)
                }
                Text(state.subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                Label(statusLabel, systemImage: statusSymbol)
                    .font(.caption2)
                    .foregroundStyle(statusColor)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()
            .activityBackgroundTint(Color(.systemBackground))
            .activitySystemActionForegroundColor(.accentColor)
            .widgetURL(ConversationDeepLink.makeURL(sessionID: sessionID, turnID: state.turnID))
        }

        private var statusLabel: String {
            switch state.status {
            case .running: return "Running"
            case .waiting: return "Waiting for you"
            case .paused: return "Paused safely"
            case .completed: return "Finished"
            case .failed: return "Needs attention"
            }
        }

        private var statusSymbol: String {
            switch state.status {
            case .running: return "ellipsis.circle"
            case .waiting: return "questionmark.circle"
            case .paused: return "pause.circle"
            case .completed: return "checkmark.circle"
            case .failed: return "exclamationmark.triangle"
            }
        }

        private var statusColor: Color {
            switch state.status {
            case .running: return .green
            case .waiting, .paused: return .orange
            case .completed: return .green
            case .failed: return .red
            }
        }
    }
#endif

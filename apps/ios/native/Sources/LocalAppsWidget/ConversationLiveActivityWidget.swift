import Foundation
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
            case .running: return conversationLiveActivityString("chat_live_activity_short_running", "Run")
            case .waiting: return conversationLiveActivityString("chat_live_activity_short_waiting", "Wait")
            case .paused: return conversationLiveActivityString("chat_live_activity_short_paused", "Pause")
            case .completed:
                return conversationLiveActivityString("chat_live_activity_short_completed", "Done")
            case .failed: return conversationLiveActivityString("chat_live_activity_short_failed", "Fail")
            }
        }
    }

    @available(iOS 18.0, *)
    private struct ConversationLiveActivityView: View {
        @Environment(\.colorSchemeContrast) private var contrast
        @Environment(\.isLuminanceReduced) private var isLuminanceReduced
        @ScaledMetric(relativeTo: .title2) private var symbolWidth = 28

        let state: ConversationLiveActivityAttributes.ContentState
        let sessionID: String

        var body: some View {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Image(systemName: "sparkles")
                    .font(.title2.weight(.medium))
                    .frame(width: symbolWidth)
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 6) {
                    Text(state.title)
                        .font(.headline)
                        .lineLimit(2)

                    if !state.subtitle.isEmpty {
                        Text(state.subtitle)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }

                    Label {
                        Text(statusLabel)
                    } icon: {
                        Image(systemName: statusSymbol)
                            .foregroundStyle(statusColor)
                    }
                    .font(.caption.weight(.medium))
                    .padding(.top, 4)
                }
            }
            .foregroundStyle(.primary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
            .accessibilityElement(children: .combine)
            // Preserve the system material so the activity blends with the Lock Screen.
            .activityBackgroundTint(nil)
            .activitySystemActionForegroundColor(nil)
            .widgetURL(ConversationDeepLink.makeURL(
                sessionID: sessionID,
                turnID: state.turnID,
                workspaceKey: state.workspaceKey,
                sessionMode: state.sessionMode
            ))
        }

        private var statusLabel: String {
            switch state.status {
            case .running:
                return conversationLiveActivityString("chat_live_activity_status_running", "Running")
            case .waiting:
                return conversationLiveActivityString(
                    "chat_live_activity_status_waiting",
                    "Waiting for you"
                )
            case .paused:
                return conversationLiveActivityString(
                    "chat_live_activity_status_paused",
                    "Paused safely"
                )
            case .completed:
                return conversationLiveActivityString(
                    "chat_live_activity_status_completed",
                    "Finished"
                )
            case .failed:
                return conversationLiveActivityString(
                    "chat_live_activity_status_failed",
                    "Needs attention"
                )
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
            // Keep state readable when the system dims or increases contrast.
            if isLuminanceReduced || contrast == .increased {
                return .primary
            }
            switch state.status {
            case .running: return .primary
            case .waiting, .paused: return .orange
            case .completed: return .green
            case .failed: return .red
            }
        }
    }

    /// Localize one Live Activity status chip.
    ///
    /// `state.title` / `state.subtitle` arrive from the APP already localized
    /// (`preferredLiveActivitySnapshot` builds them with `String(localized:)`),
    /// so the status chips beside them were the only hardcoded English left on
    /// this surface — a Chinese/Japanese/Korean device read
    /// "灵犀正在等待你 / Waiting for you".
    ///
    /// `NSLocalizedString(_:value:)` — not `String(localized:)` — on purpose:
    /// this code runs inside the widget EXTENSION, whose `Bundle.main` is the
    /// extension bundle. If `Resources/Localizable.xcstrings` is ever dropped
    /// from that target, the `value:` overload returns this English default
    /// rather than rendering the raw key at the user.
    private func conversationLiveActivityString(_ key: String, _ fallback: String) -> String {
        NSLocalizedString(key, value: fallback, comment: "Live Activity status chip")
    }
#endif

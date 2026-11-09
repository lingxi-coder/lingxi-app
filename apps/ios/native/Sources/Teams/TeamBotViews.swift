import SwiftUI

struct TeamBotCardView: View {
    let bot: TeamBotCard
    let ownerName: String
    let isOwner: Bool
    let busy: Bool
    let onMessage: () -> Void
    let onPause: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                AgentAvatar(agentID: bot.avatarId, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(bot.name).font(.body.weight(.semibold))
                    Text("teams_bot_owner \(ownerName)").font(.caption).foregroundStyle(.secondary)
                }
            }
            if !bot.description.isEmpty { Text(bot.description).font(.subheadline) }
            if (bot.state == "published") {
                Label(LocalizedStringKey(bot.ready ? "remote_online" : "teams_bot_offline_queued"), systemImage: bot.ready ? "circle.fill" : "moon.zzz")
                    .font(.caption).foregroundStyle(bot.ready ? Color.green : Color.secondary)
                Text(LocalizedStringKey(bot.usePolicy.mode == "all" ? "teams_bot_use_all" : "teams_bot_use_listed")).font(.caption).foregroundStyle(.secondary)
                if !bot.permitted { Text("teams_bot_not_permitted").font(.caption).foregroundStyle(.orange) }
            } else {
                Text("teams_bot_disabled").font(.caption.weight(.semibold)).foregroundStyle(.orange)
                Text("teams_bot_not_shared").font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 10) {
                if bot.selectable {
                    Button("teams_bot_message", action: onMessage).buttonStyle(.borderedProminent)
                }
                if isOwner && (bot.state == "published") {
                    Button("teams_bot_pause", role: .destructive, action: onPause).buttonStyle(.bordered)
                }
            }
            .controlSize(.small)
            .disabled(busy)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.bot.\(bot.sharedBotId)")
    }
}

struct TeamTaskCard: View {
    @Environment(TeamsWorkspace.self) private var workspace
    let task: TeamTask
    let botName: String
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(botName).font(.subheadline.weight(.semibold))
            Button {
                Task { await workspace.run { try await workspace.messaging?.refreshTask(task.taskId) } }
            } label: {
                Label(LocalizedStringKey(TeamsBotText.stateKey(task.state)), systemImage: "arrow.clockwise")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .buttonStyle(.borderless)
            Text(TeamsBotText.usageLine(task.usage, strings: .current))
                .font(.caption).foregroundStyle(.secondary)
                .accessibilityIdentifier("teams.task.usage.\(task.taskId)")
            if let artifactId = task.artifactId {
                Button("teams_artifact_open", systemImage: "doc.text") {
                    Task { await workspace.openArtifact(artifactId) }
                }
                .buttonStyle(.borderless)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.task.\(task.taskId)")
    }
}
/// Artifact links come from Bot output, so only web links may leave the app.
enum TeamArtifactLinkPolicy {
    static func allows(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" else { return false }
        return url.host?.isEmpty == false
    }
}

extension View {
    /// Text written by a Bot: links open only when they are http(s); everything else is discarded.
    func teamBotLinkPolicy() -> some View {
        environment(\.openURL, OpenURLAction { url in
            TeamArtifactLinkPolicy.allows(url) ? .systemAction : .discarded
        })
    }
}

/// Cloud-backed immutable document; reading always checks current Team membership.
/// The body is Markdown rendered by the same `AIText` as chat messages. Markdown images stay inert
/// (alt text only, nothing is fetched) and links open only when they are http(s).
struct TeamArtifactViewer: View {
    @Environment(\.theme) private var theme
    let artifact: TeamArtifact
    let onClose: () -> Void
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text(artifact.document.title).font(.title2.weight(.semibold)).foregroundStyle(theme.text)
                    AIText(markdown: artifact.document.body)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("teams.artifact.body")
                    Text("v\(artifact.revision) · \(artifact.digest)").font(.caption.monospaced()).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, alignment: .leading).padding()
            }
            .background(theme.windowBg)
            .teamBotLinkPolicy()
            .navigationTitle("teams_artifact_title")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("common_close", action: onClose) } }
        }
        .accessibilityIdentifier("teams.artifact.\(artifact.artifactId)")
    }
}

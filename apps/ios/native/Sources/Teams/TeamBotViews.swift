import SwiftUI

struct TeamBotCardView: View {
    let bot: TeamBotCard
    let ownerName: String
    let isOwner: Bool
    let busy: Bool
    /// The audience line (`teams_bot_use_all`, `teams_bot_use_listed`). A friendship has no audience to choose, so the Bots of a
    /// friend chat leave it out (docs/FRIENDS-CLIENTS.md D21).
    var showsAudience = true
    let onMessage: () -> Void
    /// The maintainer's one way to end a Bot's availability, on Teams and Friends alike: "Stop sharing" (`friends_share_bot_stop`,
    /// docs/TEAMS-UI-POLISH.md U1). The caller confirms it.
    let onStopSharing: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                AgentAvatar(agentID: bot.avatarId, size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text(bot.name).font(.body.weight(.semibold))
                    Text("teams_bot_owner \(ownerName)").font(.caption).foregroundStyle(.secondary)
                }
            }
            if !bot.description.isEmpty { Text(bot.description).font(.subheadline) }
            if (bot.state == "published") {
                Label(LocalizedStringKey(bot.ready ? "remote_online" : "teams_bot_offline_queued"), systemImage: bot.ready ? "circle.fill" : "moon.zzz")
                    .font(.caption).foregroundStyle(bot.ready ? Color.green : Color.secondary)
                if showsAudience {
                    Text(LocalizedStringKey(bot.usePolicy.mode == "all" ? "teams_bot_use_all" : "teams_bot_use_listed")).font(.caption).foregroundStyle(.secondary)
                }
                if !bot.permitted { Text("teams_bot_not_permitted").font(.caption).foregroundStyle(.orange) }
            } else {
                Text("teams_bot_disabled").font(.caption.weight(.semibold)).foregroundStyle(.orange)
                Text("teams_bot_not_shared").font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 10) {
                if bot.selectable {
                    Button(action: onMessage) { Text("teams_bot_message").minimumHitTarget(minWidth: 0) }
                        .buttonStyle(.borderedProminent)
                }
                if isOwner && (bot.state == "published") {
                    Button(role: .destructive, action: onStopSharing) { Text("friends_share_bot_stop").minimumHitTarget(minWidth: 0) }
                        .buttonStyle(.bordered)
                }
            }
            .disabled(busy)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.bot.\(bot.sharedBotId)")
    }
}

/// What one Bot request looks like in a conversation (docs/TEAMS-UI-POLISH.md U4): the state as a glyph and a word, the Bot's name, one
/// line of detail (the usage once the run has completed, what to do or what is going on before that) and at most one button. The text
/// block is one accessibility element, "Bot, state, detail"; the button stays reachable beside it.
struct TeamTaskCard: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.locale) private var locale
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .footnote) private var glyphSide: CGFloat = 16
    let task: TeamTask
    let botName: String

    private var run: TeamsBotText.RunState { TeamsBotText.runState(task.state) }

    var body: some View {
        let words = TeamsBotText.UsageStrings.current(locale: locale)
        let state = TeamsText.localized(run.labelKey)
        let completed = task.state == "completed"
        let detail = completed ? TeamsBotText.usageLine(task.usage, strings: words) : run.detailKey.map(TeamsText.localized)
        VStack(alignment: .leading, spacing: 4) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    glyph
                    Text(verbatim: state).font(.footnote.weight(.semibold)).foregroundStyle(.primary)
                }
                Text(verbatim: botName).font(.subheadline.weight(.semibold))
                if let detail {
                    Text(verbatim: detail)
                        .font(.footnote).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text(verbatim: TeamsStreamLogic.spokenLine([botName, state, detail])))
            .accessibilityValue(Text(verbatim: completed ? (TeamsBotText.tokenSplit(task.usage, strings: words) ?? "") : ""))
            // A finished run's detail is its usage line: the element keeps the identifier the line had.
            .accessibilityIdentifier(completed ? "teams.task.usage.\(task.taskId)" : "teams.task.state.\(task.taskId)")
            action
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.task.\(task.taskId)")
    }

    /// The state's glyph. It is decorative: the label next to it is the state. Processing is a spinner, and the static `ellipsis.circle`
    /// under Reduce Motion (U10).
    @ViewBuilder private var glyph: some View {
        if run.spins && !reduceMotion {
            ProgressView().controlSize(.mini).frame(width: glyphSide, height: glyphSide).accessibilityHidden(true)
        } else {
            Image(systemName: run.symbol)
                .font(.footnote)
                .foregroundStyle(tint)
                .frame(width: glyphSide, height: glyphSide)
                .accessibilityHidden(true)
        }
    }

    private var tint: Color {
        switch run.tone {
        case .ok: .green
        case .danger: .red
        case .muted: .secondary
        }
    }

    /// At most one button: a task that is still going offers "Refresh status"; a finished one that has a document offers "Open
    /// document". The other states say what to do in their detail and have none.
    @ViewBuilder private var action: some View {
        if run.active {
            Button {
                Task { await workspace.run { try await workspace.messaging?.refreshTask(task.taskId) } }
            } label: {
                Label("teams_run_refresh", systemImage: "arrow.clockwise").font(.footnote).minimumHitTarget(minWidth: 0, alignment: .leading)
            }
            .buttonStyle(.borderless)
        } else if task.state == "completed", let artifactId = task.artifactId {
            Button {
                Task { await workspace.openArtifact(artifactId) }
            } label: {
                Label("teams_artifact_open", systemImage: "doc.text").font(.footnote).minimumHitTarget(minWidth: 0, alignment: .leading)
            }
            .buttonStyle(.borderless)
        }
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
                    Text("apps_version \(artifact.revision)").font(.caption).foregroundStyle(.secondary)
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

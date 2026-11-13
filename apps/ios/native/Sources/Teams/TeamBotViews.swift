import SwiftUI

struct TeamBotCardView: View {
    let bot: TeamBotCard
    let ownerName: String
    let isOwner: Bool
    let busy: Bool
    /// The audience line (`teams_bot_use_all`, `teams_bot_use_listed`). A friendship has no audience to choose, so the Bots of a
    /// friend chat leave it out (docs/FRIENDS-CLIENTS.md D21).
    var showsAudience = true
    /// The owner's way to stop it: a Team pauses a Bot (`teams_bot_pause`), a friendship stops sharing it (`friends_share_bot_stop`, D23).
    var pauseTitle: LocalizedStringKey = "teams_bot_pause"
    let onMessage: () -> Void
    let onPause: () -> Void
    /// Opens the list the Bot shares. Set only for a viewer the service lets read it.
    var onList: (() -> Void)? = nil

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
                    Button("teams_bot_message", action: onMessage).buttonStyle(.borderedProminent)
                }
                if let onList {
                    Button("teams_list_open", systemImage: "checklist", action: onList).buttonStyle(.bordered)
                        .accessibilityIdentifier("teams.bot.list.\(bot.sharedBotId)")
                }
                if isOwner && (bot.state == "published") {
                    Button(pauseTitle, role: .destructive, action: onPause).buttonStyle(.bordered)
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
    /// The signed-in member, and the Bot's maintainer when its card is known: they decide who may cancel.
    let viewer: TeamMemberRef
    var maintainer: TeamMemberRef? = nil
    /// The name of the member a waiting task waits for (not needed when that is the viewer).
    var recipientName: String = ""
    /// Starts the answer to the Bot's question. Set only on the question itself, while it can still be answered by the viewer.
    var onReply: (() -> Void)? = nil
    @State private var cancelling = false
    @State private var reviewing = false

    private var canCancel: Bool { TeamTaskActions.canCancel(task, viewer: viewer, maintainer: maintainer) }
    private var canResolve: Bool { TeamOperationRules.canResolve(viewer: viewer, maintainer: maintainer) }
    private var waitLine: String? {
        guard task.state == "waiting", let wait = task.wait else { return nil }
        let expires = TeamsBotText.expiryText(at: wait.expiresAt, now: Date().timeIntervalSince1970 * 1000)
        return wait.recipient == viewer ? String(localized: "teams_task_waiting_you \(expires)") : String(localized: "teams_task_waiting_member \(recipientName) \(expires)")
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(botName).font(.subheadline.weight(.semibold))
            Button {
                Task { await workspace.run { try await workspace.messaging?.refreshTask(task.taskId) } }
            } label: {
                Label(LocalizedStringKey(task.cancelling ? "teams_task_cancelling" : task.state == "waiting" && task.wait?.kind == "tool_approval" ? "teams_task_waiting_approval" : TeamsBotText.stateKey(task.state)), systemImage: "arrow.clockwise")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .buttonStyle(.borderless)
            if let waitLine {
                Text(waitLine).font(.caption).foregroundStyle(.secondary)
                    .accessibilityIdentifier("teams.task.wait.\(task.taskId)")
            }
            // An outside action of unknown outcome: the maintainer confirms it where the review screen is.
            if task.state == "needs_reconciliation" {
                Text(canResolve ? "teams_task_needs_checking_maintainer" : "teams_task_needs_checking_hint").font(.caption).foregroundStyle(.secondary)
                    .accessibilityIdentifier("teams.task.checking.\(task.taskId)")
                if TeamOperationRules.canReview(task, viewer: viewer, maintainer: maintainer) {
                    Button(canResolve ? "teams_ops_review" : "teams_ops_view", systemImage: "checklist") { reviewing = true }
                        .buttonStyle(.bordered).controlSize(.small)
                        .accessibilityIdentifier("teams.task.review.\(task.taskId)")
                }
            }
            // A task cancelled before any model call has nothing to report.
            if !(task.state == "cancelled" && task.usage == nil) {
                Text(TeamsBotText.usageLine(task.usage, strings: .current))
                    .font(.caption).foregroundStyle(.secondary)
                    .accessibilityIdentifier("teams.task.usage.\(task.taskId)")
            }
            if onReply != nil || canCancel {
                HStack(spacing: 10) {
                    if let onReply {
                        Button("teams_message_reply", systemImage: "arrowshape.turn.up.left", action: onReply)
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier("teams.task.reply.\(task.taskId)")
                    }
                    if canCancel {
                        Button("teams_task_cancel", role: .destructive) {
                            guard !cancelling else { return }
                            cancelling = true
                            Task { await workspace.run { try await workspace.messaging?.cancelTask(task.taskId) }; cancelling = false }
                        }
                        .buttonStyle(.bordered)
                        .disabled(cancelling)
                        .accessibilityIdentifier("teams.task.cancel.\(task.taskId)")
                    }
                }
                .controlSize(.small)
            }
            if let artifactId = task.artifactId {
                Button("teams_artifact_open", systemImage: "doc.text") {
                    Task { await workspace.openArtifact(artifactId) }
                }
                .buttonStyle(.borderless)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.task.\(task.taskId)")
        .sheet(isPresented: $reviewing) {
            TeamOperationsSheet(taskId: task.taskId, model: workspace.messaging, canResolve: canResolve, members: workspace.directory) { reviewing = false }
        }
    }
}
/// A Bot's request to change its list, under the message that carries it: each change in words, and Approve and Decline for the member the
/// Bot works for while the task waits. Afterwards it says how the request ended. Nothing is saved to the list before an approval.
struct TeamApprovalCard: View {
    @Environment(TeamsWorkspace.self) private var workspace
    let approval: TeamListApproval
    let waitId: String
    let taskId: String
    let state: TeamApprovalState
    /// The name of the member who decides or decided (not used when that is the viewer deciding now).
    let deciderName: String
    @State private var sending = false
    /// One identity per decision, kept across retries so a lost answer cannot post the reply twice.
    @State private var ids: [Bool: TeamDecisionIds] = [:]

    private func line(_ change: TeamListApproval.Change) -> String {
        let item = change.item.map { "“\($0)”" } ?? String(localized: "teams_approval_missing_item")
        let text = "“\(change.text ?? "")”"
        switch (change.op, change.text != nil, change.done) {
        case ("add", _, _): return String(localized: "teams_approval_add \(text)")
        case ("remove", _, _): return String(localized: "teams_approval_remove \(item)")
        case (_, true, .some(true)): return String(localized: "teams_approval_change_done \(item) \(text)")
        case (_, true, .some(false)): return String(localized: "teams_approval_change_undone \(item) \(text)")
        case (_, true, nil): return String(localized: "teams_approval_change \(item) \(text)")
        case (_, false, .some(false)): return String(localized: "teams_approval_undone \(item)")
        default: return String(localized: "teams_approval_done \(item)")
        }
    }
    private var status: String? {
        switch state {
        case .decide: nil
        case .waiting: String(localized: "teams_approval_waiting_member \(deciderName)")
        case .approved: String(localized: "teams_approval_approved_by \(deciderName)")
        case .declined: String(localized: "teams_approval_declined_by \(deciderName)")
        case .closed: String(localized: "teams_approval_closed")
        }
    }
    private func decide(_ approve: Bool) {
        guard !sending else { return }
        sending = true
        let identity = ids[approve] ?? TeamDecisionIds(); ids[approve] = identity
        Task { await workspace.run { try await workspace.messaging?.decideWait(waitId, taskId: taskId, approve: approve, ids: identity) }; sending = false }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("teams_approval_title \(approval.title)").font(.subheadline.weight(.semibold))
            ForEach(Array(approval.changes.enumerated()), id: \.offset) { _, change in
                Label(line(change), systemImage: change.op == "add" ? "plus" : change.op == "remove" ? "minus" : "pencil")
                    .font(.footnote)
            }
            if let status {
                Text(status).font(.caption).foregroundStyle(.secondary)
                    .accessibilityIdentifier("teams.approval.status.\(waitId)")
            } else {
                Text("teams_approval_hint").font(.caption).foregroundStyle(.secondary)
                HStack(spacing: 10) {
                    Button("teams_approval_approve") { decide(true) }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("teams.approval.approve.\(waitId)")
                    Button("teams_approval_decline", role: .destructive) { decide(false) }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("teams.approval.decline.\(waitId)")
                }
                .controlSize(.small)
                .disabled(sending)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.approval.\(waitId)")
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

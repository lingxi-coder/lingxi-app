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
    /// Opens the list the Bot shares. Set only for a viewer the service lets read it.
    var onList: (() -> Void)? = nil

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
            // Three buttons at 44 pt do not fit side by side on a narrow phone or under a large text size: they stack then.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) { actions }
                VStack(alignment: .leading, spacing: 0) { actions }
            }
            .disabled(busy)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.bot.\(bot.sharedBotId)")
    }

    @ViewBuilder private var actions: some View {
        if bot.selectable {
            Button(action: onMessage) { Text("teams_bot_message").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.borderedProminent)
        }
        if let onList {
            Button(action: onList) { Label("teams_list_open", systemImage: "checklist").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("teams.bot.list.\(bot.sharedBotId)")
        }
        if isOwner && (bot.state == "published") {
            Button(role: .destructive, action: onStopSharing) { Text("friends_share_bot_stop").minimumHitTarget(minWidth: 0) }
                .buttonStyle(.bordered)
        }
    }
}

/// What one Bot request looks like in a conversation (docs/TEAMS-UI-POLISH.md U4): the state as a glyph and a word, the Bot's name, one
/// line of detail (the usage once the run has finished; before that what is going on or what to do) and the buttons it needs. The text
/// block is one accessibility element, "Bot, state, detail"; the buttons stay reachable beside it.
struct TeamTaskCard: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.locale) private var locale
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .footnote) private var glyphSide: CGFloat = 16
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

    private var run: TeamsBotText.RunState { TeamsBotText.runState(task.state) }
    private var canCancel: Bool { TeamTaskActions.canCancel(task, viewer: viewer, maintainer: maintainer) }
    private var canResolve: Bool { TeamOperationRules.canResolve(viewer: viewer, maintainer: maintainer) }
    /// An outside action of unknown outcome: the maintainer confirms it where the review screen is.
    private var canReview: Bool { task.state == "needs_reconciliation" && TeamOperationRules.canReview(task, viewer: viewer, maintainer: maintainer) }
    /// The usage line is the detail once the run has finished: Completed, and Cancelled when the Bot had already used the model (a task
    /// cancelled before any model call has nothing to report).
    private var showsUsage: Bool { task.state == "completed" || (task.state == "cancelled" && task.usage != nil) }
    private var waitLine: String? {
        guard task.state == "waiting", let wait = task.wait else { return nil }
        let expires = TeamsBotText.expiryText(at: wait.expiresAt, now: Date().timeIntervalSince1970 * 1000)
        return wait.recipient == viewer ? String(localized: "teams_task_waiting_you \(expires)") : String(localized: "teams_task_waiting_member \(recipientName) \(expires)")
    }

    /// The state's word: the table's, except while a cancellation is on its way and while a tool waits for an approval.
    private var stateKey: String {
        if task.cancelling { return "teams_task_cancelling" }
        if task.state == "waiting", task.wait?.kind == "tool_approval" { return "teams_task_waiting_approval" }
        return run.labelKey
    }

    /// The one line under the Bot's name. Waiting and Needs checking write their own, since they know the person: who the task waits
    /// for, and why the outcome of an action outside the chat is unknown.
    private func detailLine(_ words: TeamsBotText.UsageStrings) -> String? {
        switch task.state {
        case "waiting": waitLine
        case "needs_reconciliation": TeamsText.localized(canResolve ? "teams_task_needs_checking_maintainer" : "teams_task_needs_checking_hint")
        default: showsUsage ? TeamsBotText.usageLine(task.usage, strings: words) : run.detailKey.map(TeamsText.localized)
        }
    }

    /// The text block keeps the identifier of the line it carries.
    private var textIdentifier: String {
        switch task.state {
        case "waiting" where waitLine != nil: "teams.task.wait.\(task.taskId)"
        case "needs_reconciliation": "teams.task.checking.\(task.taskId)"
        default: showsUsage ? "teams.task.usage.\(task.taskId)" : "teams.task.state.\(task.taskId)"
        }
    }

    var body: some View {
        let words = TeamsBotText.UsageStrings.current(locale: locale)
        let state = TeamsText.localized(stateKey)
        let detail = detailLine(words)
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
            .accessibilityValue(Text(verbatim: showsUsage ? (TeamsBotText.tokenSplit(task.usage, strings: words) ?? "") : ""))
            .accessibilityIdentifier(textIdentifier)
            if hasActions { actions }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.task.\(task.taskId)")
        .sheet(isPresented: $reviewing) {
            TeamOperationsSheet(taskId: task.taskId, model: workspace.messaging, canResolve: canResolve, members: workspace.directory) { reviewing = false }
        }
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

    private var hasActions: Bool { run.active || onReply != nil || canCancel || canReview || task.artifactId != nil }

    /// The first eight states of the table have at most one button: a task that is still going offers "Refresh status", a finished one
    /// that has a document offers "Open document", the others say what to do in their detail. A waiting task adds Reply and Cancel task,
    /// Needs checking adds Review. The buttons stack under a large text size.
    private var actions: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) { buttons }
            VStack(alignment: .leading, spacing: 0) { buttons }
        }
    }

    @ViewBuilder private var buttons: some View {
        if run.active {
            Button {
                Task { await workspace.run { try await workspace.messaging?.refreshTask(task.taskId) } }
            } label: {
                Label("teams_run_refresh", systemImage: "arrow.clockwise").font(.footnote).minimumHitTarget(minWidth: 0, alignment: .leading)
            }
            .buttonStyle(.borderless)
        }
        if let onReply {
            Button(action: onReply) {
                Label("teams_message_reply", systemImage: "arrowshape.turn.up.left").minimumHitTarget(minWidth: 0)
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("teams.task.reply.\(task.taskId)")
        }
        if canCancel {
            Button(role: .destructive) {
                guard !cancelling else { return }
                cancelling = true
                Task { await workspace.run { try await workspace.messaging?.cancelTask(task.taskId) }; cancelling = false }
            } label: {
                Text("teams_task_cancel").minimumHitTarget(minWidth: 0)
            }
            .buttonStyle(.bordered)
            .disabled(cancelling)
            .accessibilityIdentifier("teams.task.cancel.\(task.taskId)")
        }
        if canReview {
            Button { reviewing = true } label: {
                Label(LocalizedStringKey(canResolve ? "teams_ops_review" : "teams_ops_view"), systemImage: "checklist").minimumHitTarget(minWidth: 0)
            }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("teams.task.review.\(task.taskId)")
        }
        if let artifactId = task.artifactId {
            Button {
                Task { await workspace.openArtifact(artifactId) }
            } label: {
                Label("teams_artifact_open", systemImage: "doc.text").font(.footnote).minimumHitTarget(minWidth: 0, alignment: .leading)
            }
            .buttonStyle(.borderless)
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
                    Button { decide(true) } label: { Text("teams_approval_approve").minimumHitTarget(minWidth: 0) }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("teams.approval.approve.\(waitId)")
                    Button(role: .destructive) { decide(false) } label: { Text("teams_approval_decline").minimumHitTarget(minWidth: 0) }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("teams.approval.decline.\(waitId)")
                }
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

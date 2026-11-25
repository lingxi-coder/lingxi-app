import SwiftUI

/// The small tag next to a name ("Can edit", "Limited in this chat"): the look of the "Bot" tag of a participant row.
struct TeamBotScopeTag: View {
    let text: LocalizedStringKey

    var body: some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
    }
}

/// What a Bot may use of its maintainer's computer (`packages/teams/BotScopeSummary.tsx`): the names of its Local Apps and its datasets, and a dataset
/// it may edit says so. Nothing else of them is known here. `limited` says a Chat uses less of it than the maintainer shared. A Bot that uses none of
/// it says that instead. It is a few lines of caption under the Bot's other details, so it takes their font and colour.
struct TeamBotScopeSummary: View {
    let scope: TeamBotScope
    var limited = false
    var identifier = "teams.bot.scope"

    init(scope: TeamBotScope, limited: Bool = false, identifier: String = "teams.bot.scope") {
        self.scope = scope
        self.limited = limited
        self.identifier = identifier
    }

    /// What `bot` may use in `conversation`: what its maintainer published, cut down to what the Chat allows. Without a Chat, all of what was published.
    init(bot: TeamBotCard, in conversation: TeamMessageConversation? = nil) {
        let effective = TeamBotScopeRules.effective(of: bot, in: conversation)
        self.init(scope: effective, limited: TeamBotScopeRules.narrowed(bot.scope, to: effective), identifier: "teams.bot.scope.\(bot.sharedBotId)")
    }

    @ViewBuilder private var headline: some View {
        if scope.isEmpty { Text("teams_bot_scope_none") } else { Text("teams_bot_scope_uses") }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            // The tag stays on the line of the sentence when it fits and goes under it when it does not.
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    headline
                    if limited { TeamBotScopeTag(text: "teams_bot_scope_limited") }
                }
                VStack(alignment: .leading, spacing: 2) {
                    headline
                    if limited { TeamBotScopeTag(text: "teams_bot_scope_limited") }
                }
            }
            if !scope.localApps.isEmpty {
                Text("teams_bot_scope_apps").fontWeight(.semibold)
                Text(verbatim: scope.localApps.map(\.name).joined(separator: TeamsListStrings.current.listSeparator)).padding(.leading, 12)
            }
            if !scope.datasets.isEmpty {
                Text("teams_bot_scope_datasets").fontWeight(.semibold)
                ForEach(scope.datasets) { row in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(verbatim: row.dataset)
                        if row.write { TeamBotScopeTag(text: "teams_bot_scope_edit") }
                    }
                    .padding(.leading, 12)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(identifier)
    }
}

extension TeamConversationContext {
    /// The Bots of this Chat that a person in it can limit: those that share something of their maintainer's computer. A friend chat has no such Bots.
    var limitableBots: [TeamBotCard] { pair == nil ? TeamBotScopeRules.limitable(conversation, cards: bots, uid: uid) : [] }
}

/// "What Bots can use in this chat" (`packages/teams/BotScopeDialog.tsx`): one section per Bot of the Chat that shares something of its maintainer's
/// computer, with what it shares as switches, on where the Chat allows it. Saving names what stays allowed; allowing everything again lifts the limit.
/// A change someone made meanwhile, or the maintainer, is not written over: what the Bot may use now is shown again, with the reason.
struct TeamBotScopeEditor: View {
    @Environment(TeamsWorkspace.self) private var workspace
    let conversationID: String
    /// The last save or reset: its Bot, and the code it failed with (empty when it was written).
    @State private var outcome: Outcome?
    /// What is ticked, for the Bots whose ticks were changed.
    @State private var drafts: [String: Draft] = [:]
    /// The revision of a Bot's entry that its last write made. The Bot's switches wait until the listener brings it: another change before then would
    /// be sent from the old revision and conflict (as the auto-reply switches wait).
    @State private var written: [String: Int] = [:]

    private struct Outcome: Equatable {
        let sharedBotId: String
        let code: String
    }

    /// What a person ticked, and what it was ticked against.
    private struct Draft: Equatable {
        let basis: TeamBotScopeBasis
        let choice: TeamBotScopeChoice
    }

    var body: some View {
        Group {
            if let context = TeamConversationContext.make(workspace: workspace, conversationID: conversationID) {
                form(context)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .navigationTitle("teams_bot_scope_title")
        .navigationBarTitleDisplayMode(.inline)
        .accessibilityIdentifier("teams.botscope")
    }

    private func form(_ context: TeamConversationContext) -> some View {
        let busy = workspace.messageBusy || context.model.offline
        return List {
            Section { TeamsNote(text: "teams_bot_scope_note", systemImage: "info.circle") }
            ForEach(context.limitableBots) { bot in
                let entry = context.conversation.botScopes?[bot.sharedBotId]
                let basis = TeamBotScopeBasis(published: bot.scope, entry: entry)
                // The ticks start from what the Chat allows. When that changes (a save, someone else, the maintainer), they start again from it.
                let draft = drafts[bot.sharedBotId]
                TeamBotScopeBotSection(
                    bot: bot, basis: basis, limited: TeamBotScopeRules.limits(entry),
                    choice: draft?.basis == basis ? draft?.choice ?? basis.allowed : basis.allowed,
                    busy: busy || (written[bot.sharedBotId] ?? 0) > basis.revision,
                    result: outcome?.sharedBotId == bot.sharedBotId ? outcome?.code : nil,
                    onChoice: { next in
                        drafts[bot.sharedBotId] = Draft(basis: basis, choice: next)
                        outcome = nil
                    },
                    onSend: { narrowing in send(bot, revision: basis.revision, narrowing, context) })
            }
        }
    }

    private func send(_ bot: TeamBotCard, revision: Int, _ narrowing: TeamBotScopeNarrowing?, _ context: TeamConversationContext) {
        outcome = nil
        let model = context.model, conversationId = context.conversation.id
        Task {
            var code = "", made = 0
            let sent = await workspace.run {
                do {
                    made = try await model.setConversationBotScope(conversationId, sharedBotId: bot.sharedBotId, expectedRevision: revision, scope: narrowing)
                } catch {
                    code = (error as? TeamMessageFailure)?.code ?? "UNKNOWN"
                    throw error
                }
            }
            if sent {
                written[bot.sharedBotId] = made
                outcome = Outcome(sharedBotId: bot.sharedBotId, code: "")
                return
            }
            // Nothing was tried while another action ran: there is nothing to say.
            guard !code.isEmpty else { return }
            // It is said here, under the Bot, and not again as a banner over the Chat.
            workspace.clearActionError()
            outcome = Outcome(sharedBotId: bot.sharedBotId, code: code)
            // What the Bot shares, or what the Chat holds, changed under this person: the Bots are read again, and the Chat's side arrives by its listener.
            if code == "SCOPE_NOT_PUBLISHED" || code == "BOT_SCOPE_REVISION_CONFLICT" { model.refreshBots() }
        }
    }
}

/// One Bot of the Chat. What is shown follows the Chat and the Bot as they are now (`basis`); `choice` is what is ticked.
private struct TeamBotScopeBotSection: View {
    let bot: TeamBotCard
    let basis: TeamBotScopeBasis
    /// Whether the Chat holds a narrowing of this Bot now: only then is there something to lift.
    let limited: Bool
    let choice: TeamBotScopeChoice
    let busy: Bool
    /// `nil` before anything was sent for it; empty once it was written; otherwise the code it failed with.
    let result: String?
    let onChoice: (TeamBotScopeChoice) -> Void
    let onSend: (TeamBotScopeNarrowing?) -> Void

    private func appBinding(_ app: TeamBotScopeApp) -> Binding<Bool> {
        Binding(get: { choice.localApps.contains(app.id) }, set: { on in
            var next = choice
            next.localApps = TeamBotScopeRules.toggled(choice.localApps, app.id, on: on)
            onChoice(next)
        })
    }

    private func datasetBinding(_ row: TeamBotScopeDataset) -> Binding<Bool> {
        Binding(get: { choice.datasets.contains(row.dataset) }, set: { on in
            var next = choice
            next.datasets = TeamBotScopeRules.toggled(choice.datasets, row.dataset, on: on)
            onChoice(next)
        })
    }

    var body: some View {
        Section {
            if !basis.published.localApps.isEmpty {
                Text("teams_bot_scope_apps").font(.footnote.weight(.semibold)).foregroundStyle(.secondary).accessibilityAddTraits(.isHeader)
                ForEach(basis.published.localApps) { app in
                    Toggle(app.name, isOn: appBinding(app))
                        .disabled(busy)
                        .accessibilityIdentifier("teams.botscope.\(bot.sharedBotId).app.\(app.id)")
                }
            }
            if !basis.published.datasets.isEmpty {
                Text("teams_bot_scope_datasets").font(.footnote.weight(.semibold)).foregroundStyle(.secondary).accessibilityAddTraits(.isHeader)
                ForEach(basis.published.datasets) { row in
                    Toggle(isOn: datasetBinding(row)) {
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(verbatim: row.dataset)
                            if row.write { TeamBotScopeTag(text: "teams_bot_scope_edit") }
                        }
                    }
                    .disabled(busy)
                    // The name and then what the Bot may do with it, as one label for the switch.
                    .accessibilityLabel(Text(verbatim: TeamsStreamLogic.spokenLine([row.dataset, row.write ? TeamsText.localized("teams_bot_scope_edit") : nil])))
                    .accessibilityIdentifier("teams.botscope.\(bot.sharedBotId).dataset.\(row.dataset)")
                }
            }
            if let result {
                if result.isEmpty {
                    Label("teams_bot_scope_saved", systemImage: "checkmark.circle")
                        .font(.footnote).foregroundStyle(.secondary)
                        .accessibilityIdentifier("teams.botscope.\(bot.sharedBotId).saved")
                } else {
                    Text(LocalizedStringKey(TeamsErrors.key(result)))
                        .font(.footnote).foregroundStyle(.red)
                        .accessibilityIdentifier("teams.botscope.\(bot.sharedBotId).error")
                }
            }
            Button("common_save") { onSend(TeamBotScopeRules.narrowing(of: choice, in: basis.published)) }
                .disabled(busy || TeamBotScopeRules.same(choice, basis.allowed))
                .accessibilityIdentifier("teams.botscope.\(bot.sharedBotId).save")
            Button("teams_bot_scope_reset") { onSend(nil) }
                .disabled(busy || !limited)
                .accessibilityIdentifier("teams.botscope.\(bot.sharedBotId).reset")
        } header: {
            Text("teams_bot_scope_limit_title \(bot.name)")
        }
    }
}

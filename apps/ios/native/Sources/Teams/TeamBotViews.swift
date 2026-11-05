import SwiftUI

// What a Bot's messages carry in a conversation (spec §6.3; Desktop `BotRunCards.tsx`): the run it started,
// the question it asked and, for its owner, the way to settle an interrupted request. Every word is a
// `Localizable.xcstrings` key. Pending answers and settlements are saved before they are sent
// (`TeamMessagingModel.botOutbox`), so a lost response resends the same identity.

/// Limits a text to `limit` UTF-16 units (the server's unit for answers), dropping whole characters.
func clampedToUTF16(_ text: String, limit: Int) -> String {
    guard text.utf16.count > limit else { return text }
    var result = text
    while result.utf16.count > limit { result.removeLast() }
    return result
}

private func localized(_ key: String) -> String { String(localized: String.LocalizationValue(key)) }

// MARK: - Run card

/// `"<Bot name> · <state>"` under the newest message of a trigger, the owner's Desktop line, and the
/// settlement form for the owner of an interrupted run.
struct TeamBotRunCard: View {
    @Environment(TeamsWorkspace.self) private var workspace
    let state: TeamBotState
    let triggerID: String
    let conversationID: String
    let context: TeamConversationContext
    let now: Double
    @State private var text = ""
    @State private var confirmed = false

    private var botName: String {
        let name = context.bots.first { $0.key == state.botKey }?.name ?? ""
        return name.isEmpty ? context.strings.bot : name
    }

    var body: some View {
        let pending = context.model.cache?.botOutbox?.first { $0.action == "recoverBot" && $0.conversationId == conversationID && $0.triggerId == triggerID }
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                AgentAvatar(agentID: context.avatarID(forBotKey: state.botKey), size: 18)
                Text(botName + " · " + localized(TeamsBotText.stateKey(state.state)))
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .combine)
            if context.uid == state.ownerUid, let key = TeamsBotText.hostKey(state.hostStatus) {
                Text(LocalizedStringKey(key)).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if let pending {
                pendingSettlement(pending, now: now)
            } else if context.model.canRecover(triggerID, now: now) {
                recoveryForm
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityIdentifier("teams.bot.run.\(triggerID)")
    }

    private func pendingSettlement(_ intent: TeamBotIntent, now: Double) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(LocalizedStringKey(intent.error != nil ? "teams_recover_rejected" : "teams_recover_saved")).font(.caption)
            if let result = intent.text { Text(result).font(.callout).textSelection(.enabled) }
            if intent.error != nil {
                Button("teams_recover_discard", role: .destructive) {
                    Task { await workspace.run { try await context.model.discardBotIntent(intent.id) } }
                }
                .font(.caption)
                .disabled(workspace.messageBusy || !intent.canDiscard(now: now))
            }
        }
    }

    private var recoveryForm: some View {
        let valid = !JSText.isBlank(text) && text.utf8.count <= TeamsComposeLogic.messageBytes
        return DisclosureGroup("teams_recover_summary") {
            VStack(alignment: .leading, spacing: 10) {
                Text("teams_recover_explain").font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                TextField("teams_recover_result", text: $text, axis: .vertical)
                    .lineLimit(2...6)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("teams.bot.recover.text")
                Toggle("teams_recover_confirm", isOn: $confirmed).font(.caption)
                Button("teams_recover_submit") {
                    let result = text
                    Task {
                        let done = await workspace.run { try await context.model.recover(trigger: triggerID, text: result) }
                        if done { text = ""; confirmed = false }
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .disabled(workspace.messageBusy || !confirmed || !valid)
            }
            .padding(.top, 6)
        }
        .font(.footnote.weight(.semibold))
    }
}

// MARK: - Question card

/// The question a Bot asked. Only the original requester, in the same membership generation, can answer,
/// and only while the run is still waiting for it; everyone else reads the questions.
struct TeamBotQuestionCard: View {
    @Environment(TeamsWorkspace.self) private var workspace
    let message: TeamMessage
    let context: TeamConversationContext
    let now: Double
    @State private var answers: [String: String] = [:]

    var body: some View {
        if let question = message.question {
            let trigger = message.triggerId
            let state = trigger.flatMap { context.model.botStates[$0] }
            let pending = context.model.cache?.botOutbox?.first {
                $0.action == "botAnswer" && $0.conversationId == context.conversation.id && $0.questionId == question.id
            }
            let answered = state?.questionId == question.id && state?.answered == true
            let recipient = message.origin == TeamBotOrigin(uid: context.uid, generation: context.team.generation)
            VStack(alignment: .leading, spacing: 8) {
                Text("teams_question_title").font(.subheadline.weight(.semibold))
                if answered {
                    Text("teams_question_received").font(.callout)
                } else if let pending {
                    pendingAnswer(pending, question: question)
                } else if recipient {
                    form(question, active: context.model.canAnswer(message, now: now), known: state != nil)
                } else {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(question.questions, id: \.self) { prompt in
                            Text("• " + prompt).font(.callout)
                        }
                    }
                    Text("teams_question_requester_only").font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("teams.bot.question.\(question.id)")
        }
    }

    private func pendingAnswer(_ intent: TeamBotIntent, question: TeamBotQuestion) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(LocalizedStringKey(intent.error != nil ? "teams_question_rejected" : "teams_question_pending")).font(.caption)
            ForEach(question.questions, id: \.self) { prompt in
                VStack(alignment: .leading, spacing: 1) {
                    Text(prompt).font(.callout.weight(.semibold))
                    Text(intent.answers?[prompt] ?? "").font(.callout)
                }
            }
            if intent.error != nil {
                Button("teams_question_discard", role: .destructive) {
                    Task { await workspace.run { try await context.model.discardBotIntent(intent.id) } }
                }
                .font(.caption)
                .disabled(workspace.messageBusy || !intent.canDiscard(now: now))
            }
        }
    }

    private func form(_ question: TeamBotQuestion, active: Bool, known: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(question.questions, id: \.self) { prompt in
                VStack(alignment: .leading, spacing: 4) {
                    Text(prompt).font(.callout.weight(.semibold))
                    TextField(text: Binding(
                        get: { answers[prompt] ?? "" },
                        set: { answers[prompt] = clampedToUTF16($0, limit: 2000) }
                    ), axis: .vertical) { Text(prompt) }
                    .lineLimit(1...5)
                    .textFieldStyle(.roundedBorder)
                    .disabled(workspace.messageBusy || !active)
                }
            }
            Button("teams_question_send") {
                let submitted = answers
                Task {
                    let done = await workspace.run { try await context.model.answer(message, answers: submitted) }
                    if done { answers = [:] }
                }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .disabled(workspace.messageBusy || !active || !TeamBotIntent.validAnswers(answers, for: question))
            if !active {
                Text(LocalizedStringKey(known ? "teams_question_inactive" : "teams_question_verifying")).font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

// MARK: - Bot card of the team details

/// A Bot shared with the team (spec §6.4): who owns it, whether it is reachable, who may use it, and the
/// actions the viewer has (message it; unbind it when it is theirs).
struct TeamBotCardView: View {
    let bot: TeamBotCard
    let ownerName: String
    let isOwner: Bool
    let busy: Bool
    let onMessage: () -> Void
    let onUnbind: () -> Void

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
            if bot.enabled {
                Label(LocalizedStringKey(bot.online ? "remote_online" : "teams_bot_offline_queued"), systemImage: bot.online ? "circle.fill" : "moon.zzz")
                    .font(.caption).foregroundStyle(bot.online ? Color.green : Color.secondary)
                Text(LocalizedStringKey(bot.use == "all" ? "teams_bot_use_all" : "teams_bot_use_listed")).font(.caption).foregroundStyle(.secondary)
                if !bot.permitted { Text("teams_bot_not_permitted").font(.caption).foregroundStyle(.orange) }
            } else {
                Text("teams_bot_disabled").font(.caption.weight(.semibold)).foregroundStyle(.orange)
                Text("teams_bot_not_shared").font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 10) {
                if bot.selectable {
                    Button("teams_bot_message", action: onMessage).buttonStyle(.borderedProminent)
                }
                if isOwner && bot.enabled {
                    Button("teams_bot_unbind", role: .destructive, action: onUnbind).buttonStyle(.bordered)
                }
            }
            .controlSize(.small)
            .disabled(busy)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.bot.\(bot.key)")
    }
}

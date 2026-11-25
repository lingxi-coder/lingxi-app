import SwiftUI

/// The Chat a log is limited to, and how the person knows it.
struct TeamTriageChat: Equatable, Sendable {
    let id: String
    let title: String
}

extension TeamConversationContext {
    /// The Bots of this Chat whose auto-reply log the viewer reads here: the ones they maintain that answer on their own in it. A friend chat has none.
    var triageBots: [TeamBotCard] { pair == nil ? TeamTriageRules.bots(conversation, cards: bots, viewer: viewer) : [] }
}

/// What a Bot decided about the messages nobody asked it about, for the person who maintains it (`packages/teams/TriageLogDialog.tsx`): newest first, a page at
/// a time, in every Chat of the Team or, with `conversation`, in one. Only the decision and the reason are kept, never what a model said. When the service says the
/// person does not maintain the Bot after all, `onDenied` takes its entry away and the screen closes.
struct TeamTriageLogView: View {
    let bot: TeamBotCard
    var conversation: TeamTriageChat? = nil
    let model: TeamMessagingModel
    let onDenied: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var list = TeamTriageList()
    @State private var reader: TeamTriageReader?

    private var key: String { bot.sharedBotId + "|" + (conversation?.id ?? "") }

    var body: some View {
        List {
            Section {
                TeamsNote(text: "teams_triage_intro", systemImage: "info.circle")
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: bot.name).font(.subheadline.weight(.semibold))
                    if let conversation { Text(verbatim: conversation.title).font(.footnote).foregroundStyle(.secondary) }
                }
                .accessibilityElement(children: .combine)
            }
            if list.status == .loading {
                Section { HStack(spacing: 10) { ProgressView(); Text("teams_triage_loading").foregroundStyle(.secondary) }.accessibilityIdentifier("teams.triage.loading") }
            }
            if list.status == .ready && list.items.isEmpty {
                Section { Text("teams_triage_empty").font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("teams.triage.empty") }
            }
            if !list.items.isEmpty {
                Section { ForEach(list.items) { item in row(item) } }
            }
            if list.status == .failed && !list.error.isEmpty {
                Section {
                    Text(LocalizedStringKey(TeamsErrors.key(list.error))).foregroundStyle(.red).accessibilityIdentifier("teams.triage.error")
                    if list.items.isEmpty {
                        Button("common_retry") { Task { await reader?.load() } }.accessibilityIdentifier("teams.triage.retry")
                    }
                }
            }
            if list.nextCursor != nil && list.status != .denied {
                Section {
                    Button { Task { await reader?.more() } } label: {
                        HStack(spacing: 10) {
                            if list.status == .more { ProgressView() }
                            Text(LocalizedStringKey(list.status == .more ? "teams_triage_loading" : "teams_triage_more"))
                        }
                        .frame(maxWidth: .infinity)
                    }
                    .disabled(list.busy)
                    .accessibilityIdentifier("teams.triage.more")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("teams_triage_title")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("teams_list_refresh", systemImage: "arrow.clockwise") { Task { await reader?.load() } }
                    .disabled(list.busy)
                    .accessibilityIdentifier("teams.triage.refresh")
            }
        }
        // A change of Bot or Chat starts a list over; leaving the screen drops what is still being read.
        .task(id: key) { @MainActor in
            reader?.close()
            let next = TeamTriageReader(read: { [model, bot, conversation] cursor, limit in
                try await model.listTriage(bot.sharedBotId, conversationId: conversation?.id, cursor: cursor, limit: limit)
            }, changed: { list = $0 })
            reader = next
            await next.load()
        }
        .onChange(of: list.status) { _, status in
            if status == .denied { onDenied(bot.sharedBotId); dismiss() }
        }
        .onDisappear { reader?.close() }
        .accessibilityIdentifier("teams.triage.\(bot.sharedBotId)")
    }

    private func row(_ item: TeamTriageItem) -> some View {
        let outcome = TeamTriageRules.outcomes[item.state] ?? .waiting
        let chat = item.conversationTitle ?? String(localized: "teams_triage_chat_untitled")
        let when = Date(timeIntervalSince1970: item.triage.at / 1000).formatted(date: .abbreviated, time: .shortened)
        return HStack(alignment: .top, spacing: 10) {
            Image(systemName: TeamTriageRules.symbol(item.triage.action))
                .foregroundStyle(item.triage.action == "run" ? Color.green : Color.secondary)
                .frame(width: 22)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    if let key = TeamTriageRules.actionKey(item.triage.action) { Text(LocalizedStringKey(key)).font(.subheadline.weight(.semibold)) }
                    if let probability = item.triage.probability {
                        Text("teams_triage_probability \(TeamTriageRules.percent(probability))")
                            .font(.caption2).foregroundStyle(.secondary)
                            .padding(.horizontal, 6).padding(.vertical, 1)
                            .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                    }
                }
                if let key = TeamTriageRules.reasonKey(item.triage.reason) { Text(LocalizedStringKey(key)).font(.footnote).foregroundStyle(.secondary) }
                Text(verbatim: TeamsStreamLogic.joinedLine([chat, when])).font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            Text(LocalizedStringKey(TeamTriageRules.outcomeKey(outcome)))
                .font(.caption2.weight(.semibold))
                .foregroundStyle(outcome == .failed ? Color.red : outcome == .replied ? Color.green : Color.secondary)
                .multilineTextAlignment(.trailing)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("teams.triage.row.\(item.taskId)")
    }
}

/// The log over the current screen, from a Bot's card: its own navigation bar with Close.
struct TeamTriageLogSheet: View {
    let bot: TeamBotCard
    var conversation: TeamTriageChat? = nil
    let model: TeamMessagingModel
    let onDenied: (String) -> Void
    let onClose: () -> Void

    var body: some View {
        NavigationStack {
            TeamTriageLogView(bot: bot, conversation: conversation, model: model, onDenied: onDenied)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("common_close", action: onClose) } }
        }
    }
}

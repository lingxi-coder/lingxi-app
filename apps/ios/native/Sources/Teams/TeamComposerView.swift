import SwiftUI

/// The message box at the bottom of an open conversation (spec §6.3; Desktop `Composer.tsx`). A growing field
/// (1 to 6 lines; return is a new line, sending is the button), the reply and edit bars, the Bots that will be
/// asked as removable chips, the `@` picker for groups and channels, and the footnotes. A lock replaces it
/// when the viewer cannot write here. The unsent state lives in the parent so reply, edit and the workspace's
/// per-conversation drafts can drive it.
struct TeamComposerView: View {
    let context: TeamConversationContext
    @Binding var draft: TeamComposerDraft
    let editing: TeamMessage?
    /// The message being answered and who wrote it.
    let replying: (author: String, text: String)?
    let busy: Bool
    var focus: FocusState<Bool>.Binding
    let onSend: () -> Void
    let onCancelEdit: () -> Void
    let onCancelReply: () -> Void

    @State private var selection: TextSelection?
    /// The `@` the viewer dismissed the picker for (UTF-16 offset); the picker stays closed until the `@` changes.
    @State private var dismissedMention = -1

    private var model: TeamMessagingModel { context.model }
    private var conversation: TeamMessageConversation { context.conversation }
    private var lock: ComposerLock {
        TeamsComposeLogic.composerLock(
            uid: context.uid, kind: conversation.kind, participants: conversation.participants, readers: conversation.readers,
            bots: context.bots, botsLoaded: model.botsLoaded, botsUnverified: model.botsUnverified)
    }
    private var asks: Bool { editing == nil && (conversation.kind == "group" || conversation.kind == "channel") }
    private var askable: Bool { asks && context.bots.contains { $0.selectable && conversation.participants.contains("shared_bot:\($0.sharedBotId)") } }
    private var bytes: Int { TeamsComposeLogic.byteLength(draft.text) }
    private var tooLong: Bool { bytes > TeamsComposeLogic.messageBytes }

    /// The caret as a UTF-16 offset: the end of the selection, or the end of the text.
    private var caret: Int {
        let text = draft.text
        guard let selection, case let .selection(range) = selection.indices, range.upperBound <= text.endIndex else { return text.utf16.count }
        return text.utf16.distance(from: text.startIndex, to: range.upperBound)
    }
    private var mention: MentionAt? { asks ? TeamsComposeLogic.mentionAt(draft.text, caret: caret) : nil }
    private var candidates: [TeamBotCard] {
        guard let mention, mention.start != dismissedMention, draft.targets.count < TeamsComposeLogic.maxTargets else { return [] }
        return TeamsComposeLogic.mentionCandidates(bots: context.bots, conversation: conversation, chosenKeys: draft.targets, query: mention.query)
    }

    var body: some View {
        VStack(spacing: 0) {
            Divider()
            if lock != .none {
                Text(LocalizedStringKey(Self.lockKey(lock)))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
                    .accessibilityIdentifier("teams.composer.lock")
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    contextBars
                    chips
                    picker
                    field
                    footer
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        }
        .background(.bar)
    }

    static func lockKey(_ lock: ComposerLock) -> String {
        switch lock {
        case .none: ""
        case .ownerReading: "teams_composer_lock_owner_reading"
        case .botHandedOver: "teams_composer_lock_bot_handed_over"
        case .botUnverified: "teams_composer_lock_bot_unverified"
        case .botChecking: "teams_composer_lock_bot_checking"
        case .botUnavailable: "teams_composer_lock_bot_unavailable"
        }
    }

    // MARK: Bars and chips

    @ViewBuilder
    private var contextBars: some View {
        if editing != nil {
            bar(systemImage: "pencil") {
                Text("teams_editing_message").font(.footnote.weight(.semibold))
            } trailing: {
                Button("teams_cancel", action: onCancelEdit).font(.footnote)
            }
        }
        if let replying {
            bar(systemImage: "arrowshape.turn.up.left") {
                Text("teams_replying_to \(replying.author)").font(.footnote.weight(.semibold))
                Text(replying.text).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            } trailing: {
                Button(action: onCancelReply) { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                    .accessibilityLabel(Text("teams_cancel_reply"))
            }
        }
    }

    private func bar<Content: View, Trailing: View>(systemImage: String, @ViewBuilder content: () -> Content, @ViewBuilder trailing: () -> Trailing) -> some View {
        HStack(spacing: 8) {
            Image(systemName: systemImage).font(.footnote).foregroundStyle(.secondary).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) { content() }
            Spacer(minLength: 8)
            trailing()
        }
    }

    @ViewBuilder
    private var chips: some View {
        if !draft.targets.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("teams_composer_bots_asked").font(.caption2).foregroundStyle(.secondary)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(draft.targets, id: \.self) { key in
                            let name = context.card(forKey: key)?.name ?? context.strings.bot
                            HStack(spacing: 4) {
                                AgentAvatar(agentID: context.avatarID(forSharedBotId: key), size: 20)
                                Text("@" + name).font(.footnote.weight(.semibold))
                                Button { draft.targets.removeAll { $0 == key } } label: {
                                    Image(systemName: "xmark.circle.fill").font(.footnote).foregroundStyle(.secondary)
                                }
                                .accessibilityLabel(Text("teams_composer_remove_bot \(name)"))
                            }
                            .padding(.leading, 6).padding(.trailing, 6).padding(.vertical, 3)
                            .background(Color(uiColor: .secondarySystemBackground), in: Capsule())
                        }
                    }
                }
            }
            .accessibilityIdentifier("teams.composer.chips")
        }
    }

    // MARK: Picker

    @ViewBuilder
    private var picker: some View {
        let options = candidates
        if !options.isEmpty, let mention {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("teams_composer_bots_can_ask").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button { dismissedMention = mention.start } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                        .accessibilityLabel(Text("common_close"))
                }
                .padding(.horizontal, 12).padding(.vertical, 6)
                ForEach(options) { bot in
                    Button { pick(bot, mention: mention) } label: {
                        HStack(spacing: 10) {
                            AgentAvatar(agentID: bot.avatarId, size: 24)
                            Text(bot.name).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                            Spacer()
                            Text(LocalizedStringKey(bot.ready ? "remote_online" : "teams_bot_offline_queued")).font(.caption).foregroundStyle(.secondary)
                        }
                        .padding(.horizontal, 12).padding(.vertical, 8)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("teams.composer.mention.\(bot.sharedBotId)")
                }
            }
            .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
    }

    private func pick(_ bot: TeamBotCard, mention: MentionAt) {
        let next = TeamsComposeLogic.withoutMention(draft.text, start: mention.start, caret: caret)
        draft.targets.append(bot.sharedBotId)
        draft.text = next
        selection = TextSelection(insertionPoint: String.Index(utf16Offset: min(mention.start, next.utf16.count), in: next))
        focus.wrappedValue = true
    }

    /// The "Ask a Bot (@)" button: types an `@` at the caret (after a space when the text before needs one).
    private func insertAt() {
        let text = draft.text
        let units = Array(text.utf16)
        let at = min(caret, units.count)
        let needsSpace = at > 0 && !(Unicode.Scalar(units[at - 1]).map(AvatarIdentity.isSpace) ?? false)
        let prefix = String(decoding: units[0..<at], as: UTF16.self) + (needsSpace ? " " : "") + "@"
        let next = prefix + String(decoding: units[at...], as: UTF16.self)
        draft.text = next
        selection = TextSelection(insertionPoint: String.Index(utf16Offset: prefix.utf16.count, in: next))
        focus.wrappedValue = true
    }

    // MARK: Field

    private var placeholder: String {
        if editing != nil { return String(localized: "teams_composer_edit_hint") }
        let name = (conversation.kind == "channel" ? "#" : "") + context.label
        return String(localized: "teams_composer_hint \(name)")
    }

    private var field: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if asks {
                Button(action: insertAt) {
                    Image(systemName: "at").font(.system(size: 18, weight: .medium)).frame(width: 32, height: 32)
                }
                .disabled(!askable || draft.targets.count >= TeamsComposeLogic.maxTargets)
                .accessibilityLabel(Text("teams_composer_ask_bot"))
                .accessibilityIdentifier("teams.composer.ask")
            }
            TextField(placeholder, text: $draft.text, selection: $selection, axis: .vertical)
                .lineLimit(1...6)
                .focused(focus)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                .accessibilityIdentifier("teams.composer.input")
                .onChange(of: draft.text) { _, _ in dismissedMention = -1 }
            Button(action: onSend) {
                Image(systemName: editing != nil ? "checkmark.circle.fill" : "arrow.up.circle.fill").font(.system(size: 30))
            }
            .disabled(busy || !TeamsComposeLogic.canSendText(draft.text))
            .accessibilityLabel(Text(LocalizedStringKey(editing != nil ? "teams_message_save" : "teams_message_send")))
            .accessibilityIdentifier("teams.composer.send")
        }
    }

    @ViewBuilder
    private var footer: some View {
        if tooLong {
            Text("teams_composer_too_long").font(.caption).foregroundStyle(.red)
        } else if bytes > Int(Double(TeamsComposeLogic.messageBytes) * 0.9) {
            Text(verbatim: "\(bytes) / \(TeamsComposeLogic.messageBytes)").font(.caption).monospacedDigit().foregroundStyle(.secondary)
        }
        if conversation.kind == "bot_dm" {
            Text("teams_composer_note_bot_dm").font(.caption2).foregroundStyle(.secondary)
        } else if askable && editing == nil {
            Text("teams_composer_note_group").font(.caption2).foregroundStyle(.secondary)
        }
        if model.offline {
            Text("teams_composer_note_offline").font(.caption2).foregroundStyle(.secondary)
        }
    }
}

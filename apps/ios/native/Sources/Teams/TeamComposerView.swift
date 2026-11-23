import SwiftUI

/// The message box at the bottom of an open conversation (spec §6.3; Desktop `Composer.tsx`). A growing field
/// (1 to 6 lines; return is a new line, sending is the button), the reply and edit bars, the Bots that will be
/// asked as removable chips, the `@` picker for groups and channels, and the footnotes. A lock replaces it
/// when the viewer cannot write here. The unsent state lives in the parent so reply, edit and the workspace's
/// per-conversation drafts can drive it.
struct TeamComposerView: View {
    @Environment(\.theme) private var theme
    let context: TeamConversationContext
    @Binding var draft: TeamComposerDraft
    let editing: TeamMessage?
    var answeringWait = false
    /// The message being answered and who wrote it.
    let replying: (author: String, text: String)?
    let busy: Bool
    /// The files of the unsent message and the chosen ones that were not taken. Empty while a message is edited or a
    /// Bot's question is answered: neither carries files, and the files wait in the draft.
    var files: [TeamDraftFile] = []
    var refusals: [TeamFileRefusal] = []
    /// `nil` where files cannot be added now.
    var onChooseFiles: (([TeamChosenFile]) -> Void)?
    var onRemoveFile: (String) -> Void = { _ in }
    var onDismissRefusals: () -> Void = {}
    /// Whether the microphone is taking words for this box now.
    var listening = false
    /// Starts taking spoken words for the box, or ends it. The words are put in the box and never sent.
    var onVoice: () -> Void = {}
    @Binding var focused: Bool
    let onSend: () -> Void
    let onCancelEdit: () -> Void
    let onCancelReply: () -> Void

    @State private var selection: NSRange?
    /// The `@` the viewer dismissed the picker for (UTF-16 offset); the picker stays closed until the `@` changes.
    @State private var dismissedMention = -1

    private var model: TeamMessagingModel { context.model }
    private var conversation: TeamMessageConversation { context.conversation }
    private var lock: ComposerLock {
        let value = TeamsComposeLogic.composerLock(
            uid: context.uid, kind: conversation.kind, participants: conversation.participants, readers: conversation.readers,
            bots: context.bots, botsLoaded: model.botsLoaded, botsUnverified: model.botsUnverified)
        if (editing != nil || answeringWait), [.botUnavailable, .botChecking, .botUnverified].contains(value) { return .none }
        return value
    }
    private var availabilityBlock: TeamBotAvailability? {
        TeamBotMemberRules.blockedAvailability(targets: TeamBotSelection.targets(in: conversation, selected: Set(draft.targets)), cards: context.bots,
            operation: editing == nil ? "create" : "edit", answeringWait: answeringWait)
    }
    private var canMention: Bool { !answeringWait && (conversation.kind == "group" || conversation.kind == "channel") }
    private var asks: Bool { editing == nil && canMention }
    private struct RosterSource: Equatable {
        let uid: String
        let participants: [String]
        let members: [TeamMember]
    }

    private struct Candidate: Identifiable {
        let member: TeamMember
        let key: String
        var id: String { member.uid }
    }

    @State private var roster: [Candidate] = []
    @State private var nameCounts: [String: Int] = [:]

    private var rosterSource: RosterSource {
        RosterSource(uid: context.uid, participants: conversation.participants, members: context.members)
    }

    private func indexRoster(_ source: RosterSource) {
        let participants = Set(source.participants)
        var people: [Candidate] = []
        var counts: [String: Int] = [:]
        for member in source.members {
            guard member.uid != source.uid, participants.contains("user:\(member.uid)"),
                  !member.displayName.contains(where: \.isNewline) else { continue }
            let key = TeamMentionEntity.searchKey(member.displayName)
            guard !key.isEmpty else { continue }
            people.append(Candidate(member: member, key: key))
            counts[key, default: 0] += 1
        }
        roster = people
        nameCounts = counts
    }

    private var askable: Bool { asks && context.bots.contains { $0.selectable && conversation.participants.contains("shared_bot:\($0.sharedBotId)") } }
    private var bytes: Int { TeamsComposeLogic.byteLength(draft.text) }
    private var tooLong: Bool { bytes > TeamsComposeLogic.messageBytes }

    private var caret: Int { min(selection.map(NSMaxRange) ?? draft.text.utf16.count, draft.text.utf16.count) }
    private var mention: MentionAt? { canMention ? TeamMentionEntity.at(draft.text, caret: caret, entities: draft.mentionEntities ?? []) : nil }
    private var memberCandidates: [Candidate] {
        guard let mention, mention.start != dismissedMention, (draft.mentionEntities?.count ?? 0) < TeamMentionEntity.limit else { return [] }
        let needle = TeamMentionEntity.searchKey(mention.query)
        return roster.filter { needle.isEmpty || $0.key.contains(needle) }
    }
    private var candidates: [TeamBotCard] {
        guard asks, let mention, mention.start != dismissedMention, draft.targets.count < TeamsComposeLogic.maxTargets else { return [] }
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
                    picker
                    ComposerInputSurface(focused: focused) {
                        contextBars
                        chips
                        TeamDraftFilesView(files: files, refusals: refusals, onRemove: onRemoveFile, onDismissRefusals: onDismissRefusals)
                        editor
                        toolbar
                    }
                    footer
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        }
        .background(.bar)
        .onChange(of: rosterSource, initial: true) { _, source in indexRoster(source) }
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
                Button(action: onCancelEdit) { Text("common_cancel").font(.footnote).minimumHitTarget(minWidth: 0) }
            }
        }
        if let replying {
            bar(systemImage: "arrowshape.turn.up.left") {
                Text("teams_replying_to \(replying.author)").font(.footnote.weight(.semibold))
                Text(replying.text).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            } trailing: {
                Button(action: onCancelReply) { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary).minimumHitTarget() }
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
        if !draft.targets.isEmpty && !answeringWait {
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
                                    // The chip stays small; the hit area of its remove button is 44 x 44 and overlaps the chip's padding.
                                    Image(systemName: "xmark.circle.fill").font(.footnote).foregroundStyle(.secondary)
                                        .minimumHitTarget()
                                        .padding(.horizontal, -14).padding(.vertical, -14)
                                }
                                .accessibilityLabel(Text("teams_composer_remove_bot \(name)"))
                            }
                            .padding(.leading, 6).padding(.trailing, 6).padding(.vertical, 3)
                            .background(Color(uiColor: .secondarySystemBackground), in: Capsule())
                        }
                    }
                    .frame(minHeight: 44)
                }
            }
            .accessibilityIdentifier("teams.composer.chips")
        }
    }

    // MARK: Picker

    @ViewBuilder
    private var picker: some View {
        let options = candidates
        let people = memberCandidates
        if !options.isEmpty || !people.isEmpty, let mention {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("teams_composer_mention_options").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button { dismissedMention = mention.start } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary).minimumHitTarget() }
                        .accessibilityLabel(Text("common_close"))
                }
                .padding(.horizontal, 12)
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(people) { candidate in
                            let member = candidate.member
                            Button { pick(member, mention: mention) } label: {
                                HStack(spacing: 10) {
                                    PersonAvatar(id: member.uid, name: member.displayName, size: 24)
                                    Text(member.displayName).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                                    Spacer()
                                    if nameCounts[candidate.key, default: 0] > 1 {
                                        Text(member.uid).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                }
                                .padding(.horizontal, 12)
                                .frame(minHeight: 44)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("teams.composer.mention.\(member.uid)")
                        }
                        ForEach(options) { bot in
                            Button { pick(bot, mention: mention) } label: {
                                HStack(spacing: 10) {
                                    AgentAvatar(agentID: bot.avatarId, size: 24)
                                    Text(bot.name).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                                    Spacer()
                                    Text(LocalizedStringKey(TeamBotMemberText.availabilityKey(bot.memberAvailability))).font(.caption).foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 12)
                                .frame(minHeight: 44)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("teams.composer.mention.\(bot.sharedBotId)")
                        }
                    }
                }
                .frame(height: CGFloat(min(4, people.count + options.count)) * 44)
            }
            .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
    }

    private func pick(_ bot: TeamBotCard, mention: MentionAt) {
        let before = draft.text
        let end = TeamMentionEntity.replacementEnd(before, start: mention.start, caret: caret, name: bot.name)
        let next = TeamsComposeLogic.withoutMention(before, start: mention.start, caret: end)
        draft.targets.append(bot.sharedBotId)
        let removed = before.utf16.count - next.utf16.count
        applyReplacement(next, range: mention.start..<(mention.start + removed), insertedLength: 0)
        selection = NSRange(location: min(mention.start, next.utf16.count), length: 0)
        focused = true
    }

    private func pick(_ member: TeamMember, mention: MentionAt) {
        let units = Array(draft.text.utf16)
        let label = "@" + JSText.trim(member.displayName)
        let end = TeamMentionEntity.replacementEnd(draft.text, start: mention.start, caret: caret, name: member.displayName)
        let rest = String(decoding: units[end...], as: UTF16.self)
        let separates = rest.first.map { $0.isPunctuation || $0.unicodeScalars.allSatisfy(AvatarIdentity.isSpace) } ?? false
        let separator = separates ? "" : " "
        let next = String(decoding: units[..<mention.start], as: UTF16.self) + label + separator + rest
        var entities = TeamMentionEntity.replacing(draft.mentionEntities ?? [], start: mention.start, end: end,
                                                   insertedLength: label.utf16.count + separator.utf16.count)
        entities.append(.init(userId: member.uid, offset: mention.start, length: label.utf16.count))
        draft.mentionEntities = entities.sorted { $0.offset < $1.offset }
        draft.text = next
        let nextCaret = mention.start + label.utf16.count + (!separator.isEmpty || rest.hasPrefix(" ") ? 1 : 0)
        selection = NSRange(location: nextCaret, length: 0)
        focused = true
    }

    private func updateText(_ text: String, edited: NSRange?, caretAfter: Int) {
        let range = edited.map { $0.location..<NSMaxRange($0) }
        draft.mentionEntities = TeamMentionEntity.shifted(draft.mentionEntities ?? [], from: draft.text, to: text,
                                                         selection: range, caretAfter: range == nil ? caretAfter : nil)
        draft.text = text
    }

    private func applyReplacement(_ text: String, range: Range<Int>, insertedLength: Int) {
        draft.mentionEntities = TeamMentionEntity.replacing(draft.mentionEntities ?? [], start: range.lowerBound,
                                                           end: range.upperBound, insertedLength: insertedLength)
        draft.text = text
    }

    /// The mention button: types an `@` at the caret (after a space when the text before needs one).
    private func insertAt() {
        let text = draft.text
        let units = Array(text.utf16)
        let start = min(selection?.location ?? units.count, units.count)
        let selected = start..<caret
        let at = selected.lowerBound
        let needsSpace = at > 0 && !(Unicode.Scalar(units[at - 1]).map(AvatarIdentity.isSpace) ?? false)
        let token = needsSpace ? " @" : "@"
        let prefix = String(decoding: units[..<at], as: UTF16.self) + token
        let next = prefix + String(decoding: units[selected.upperBound...], as: UTF16.self)
        applyReplacement(next, range: selected, insertedLength: token.utf16.count)
        dismissedMention = -1
        selection = NSRange(location: prefix.utf16.count, length: 0)
        focused = true
    }

    // MARK: Field

    private var placeholder: String {
        if editing != nil { return String(localized: "teams_composer_edit_hint") }
        let name = (conversation.kind == "channel" ? "#" : "") + context.label
        return String(localized: "teams_composer_hint \(name)")
    }

    private var editor: some View {
        ComposerTextInput(text: $draft.text, isFocused: $focused, placeholder: placeholder,
                          textColor: theme.text, hintColor: theme.text4, argumentHint: { _, _ in nil },
                          onSubmit: onSend, onAcceptSuggestion: nil, selection: $selection, onEdit: updateText,
                          submitsOnReturn: false, maxLines: 6, accessibilityIdentifier: "teams.composer.input")
            .padding(.horizontal, 6).padding(.vertical, 2)
            .onChange(of: draft.text) { _, _ in dismissedMention = -1 }
    }

    private var toolbar: some View {
        HStack(spacing: 0) {
            if let onChooseFiles {
                TeamAttachButton(disabled: files.count >= TeamAttachments.messageFiles, onChosen: onChooseFiles)
            }
            if canMention {
                Button(action: insertAt) {
                    Image(systemName: "at").font(.system(size: 18)).foregroundStyle(theme.text2)
                }
                .buttonStyle(ComposerActionButtonStyle())
                .disabled((roster.isEmpty || (draft.mentionEntities?.count ?? 0) >= TeamMentionEntity.limit)
                          && (!askable || draft.targets.count >= TeamsComposeLogic.maxTargets))
                .accessibilityLabel(Text("teams_composer_mention"))
                .accessibilityIdentifier("teams.composer.ask")
            }
            Spacer(minLength: 4)
            Button(action: onVoice) {
                Image(systemName: listening ? "stop.fill" : "mic").font(.system(size: 18)).foregroundStyle(theme.text2)
            }
            .buttonStyle(ComposerActionButtonStyle())
            .accessibilityLabel(Text(LocalizedStringKey(listening ? "composer_stop_recording" : "composer_record")))
            .accessibilityIdentifier("teams.composer.voice")
            let canSend = !busy && availabilityBlock == nil && TeamAttachments.canSend(text: draft.text, files: files)
            Button(action: onSend) {
                ComposerTurnActionIcon(systemName: editing != nil ? "checkmark" : "arrow.up", symbolSize: 15, background: theme.accent)
            }
            .buttonStyle(ComposerActionButtonStyle())
            .disabled(!canSend)
            .opacity(canSend ? 1 : 0.45)
            .accessibilityLabel(Text(LocalizedStringKey(editing != nil ? "teams_message_save" : "teams_message_send")))
            .accessibilityIdentifier("teams.composer.send")
        }
    }

    @ViewBuilder
    private var footer: some View {
        if let availabilityBlock {
            let status = model.botsUnverified || context.bots.contains { $0.newRequestBlock == availabilityBlock && $0.memberAvailability == .unverified } ? TeamBotAvailability.unverified : availabilityBlock
            Text(LocalizedStringKey(TeamBotMemberText.availabilityKey(status)))
                .font(.caption).foregroundStyle(.secondary).accessibilityIdentifier("teams.composer.availability")
        }
        if tooLong {
            Text("teams_composer_too_long").font(.caption).foregroundStyle(.red)
        } else if bytes > Int(Double(TeamsComposeLogic.messageBytes) * 0.9) {
            Text(verbatim: "\(bytes) / \(TeamsComposeLogic.messageBytes)").font(.caption).monospacedDigit().foregroundStyle(.secondary)
        }
        if files.contains(where: { $0.state == .failed }) {
            Text("teams_file_note_failed").font(.caption2).foregroundStyle(.secondary)
        } else if files.contains(where: { $0.state == .uploading }) {
            Text("teams_file_note_uploading").font(.caption2).foregroundStyle(.secondary)
        }
        if conversation.kind == "bot_dm" {
            Text("teams_composer_note_bot_dm").font(.caption2).foregroundStyle(.secondary)
        } else if askable && editing == nil {
            Text("teams_composer_note_group").font(.caption2).foregroundStyle(.secondary)
        }
    }
}

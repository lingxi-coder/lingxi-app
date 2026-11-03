import Foundation
import Observation
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Exact command boundary: ordinary prompts beginning with /sideways are untouched.
enum SideChatCommand {
    static func input(_ raw: String) -> String? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix("/side") else { return nil }
        let input = text.dropFirst(5)
        guard input.isEmpty || input.first?.isWhitespace == true else { return nil }
        return String(input).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func inputSourceCodeUnits(raw: String, sourceCodeUnits: [UInt16]?) -> [UInt16]? {
        guard let sourceCodeUnits,
              let expectedInput = input(raw),
              let trimmedUnits = PromptSuggestionDraftSource.trimmingWhitespace(
                sourceCodeUnits: sourceCodeUnits,
                displayText: raw
              ),
              trimmedUnits.count >= 5,
              String(decoding: Array(trimmedUnits.prefix(5)), as: UTF16.self) == "/side"
        else { return nil }

        let bodyUnits = Array(trimmedUnits.dropFirst(5))
        let bodyText = String(decoding: bodyUnits, as: UTF16.self)
        guard let inputUnits = PromptSuggestionDraftSource.trimmingWhitespace(
            sourceCodeUnits: bodyUnits,
            displayText: bodyText
        ),
              PromptSuggestionDraftSource.displayCodeUnits(for: inputUnits) == Array(expectedInput.utf16)
        else { return nil }
        return inputUnits
    }
}

#if canImport(harness_runtimeFFI)
struct SideChatUnsentPrompt: Identifiable, Equatable {
    let id = UUID()
    let text: String
    let sourceCodeUnits: [UInt16]?
    let images: [ImageRefDto]
}

/// Only value data survives an idle-expired side. In particular, this object
/// must never retain the side source or its shared native engine handle.
struct ExpiredSideChatInput: Identifiable {
    let id = UUID()
    let parentSessionID: String?
    let sideID: String?
    let sideOwnerEpoch: UInt64
    let text: String
    let sourceCodeUnits: [UInt16]?
    let images: [ImageRefDto]
    let visualizationContext: VisualizationContextChip?
}

struct ExpiredSideChatArchive: Identifiable {
    let id: String
    let parentSessionID: String?
    let parentOwnerEpoch: UInt64?
    let sideOwnerEpoch: UInt64
    let draft: String
    let draftSourceCodeUnits: [UInt16]?
    let attachment: ComposerAttachment?
    let visualizationContext: VisualizationContextChip?
    let unsent: [SideChatUnsentPrompt]
    let rejected: [ConversationUnsentPrompt]
    let outstanding: [ExpiredSideChatInput]

    private func matchesDraft(_ input: ExpiredSideChatInput) -> Bool {
        guard !draft.isEmpty || draftSourceCodeUnits != nil || attachment != nil || visualizationContext != nil else { return false }
        let draftImages = attachment.map { [ImageRefDto(mediaType: $0.mediaType, base64: $0.base64)] } ?? []
        return input.parentSessionID == parentSessionID && input.sideID == id && input.sideOwnerEpoch == sideOwnerEpoch &&
            input.text == draft && input.sourceCodeUnits == draftSourceCodeUnits &&
            input.images == draftImages && input.visualizationContext == visualizationContext
    }

    var draftIsUnconfirmed: Bool { outstanding.contains(where: matchesDraft) }
    var visibleOutstanding: [ExpiredSideChatInput] {
        outstanding.filter { $0.parentSessionID == parentSessionID && $0.sideID == id &&
            $0.sideOwnerEpoch == sideOwnerEpoch && !matchesDraft($0) }
    }

    func rebound(toParentEpoch epoch: UInt64) -> ExpiredSideChatArchive {
        ExpiredSideChatArchive(id: id, parentSessionID: parentSessionID, parentOwnerEpoch: epoch,
            sideOwnerEpoch: sideOwnerEpoch,
            draft: draft, draftSourceCodeUnits: draftSourceCodeUnits, attachment: attachment,
            visualizationContext: visualizationContext, unsent: unsent, rejected: rejected, outstanding: outstanding)
    }

    func appendingOutstanding(_ input: ExpiredSideChatInput) -> ExpiredSideChatArchive {
        ExpiredSideChatArchive(id: id, parentSessionID: parentSessionID, parentOwnerEpoch: parentOwnerEpoch,
            sideOwnerEpoch: sideOwnerEpoch,
            draft: draft, draftSourceCodeUnits: draftSourceCodeUnits, attachment: attachment,
            visualizationContext: visualizationContext, unsent: unsent, rejected: rejected,
            outstanding: outstanding + [input])
    }
}

@MainActor @Observable
final class SideChatEntry: Identifiable {
    let id: String
    let source: EngineConversationSource
    private(set) var draftRevision: UInt64 = 0
    var draft = "" {
        didSet { if Array(draft.utf16) != Array(oldValue.utf16) { draftRevision &+= 1 } }
    }
    var draftSourceCodeUnits: [UInt16]? {
        didSet { if draftSourceCodeUnits != oldValue { draftRevision &+= 1 } }
    }
    var attachment: ComposerAttachment? {
        didSet { if attachment != oldValue { draftRevision &+= 1 } }
    }
    var closing = false
    var ending = false
    private(set) var unsentPrompts: [SideChatUnsentPrompt] = []
    let voice = VoiceInteractionController(voiceCapture: VoiceCapture(), capability: VoiceCapabilityModel(),
        speechPlayer: SystemVoiceSpeechPlayer(), controlsRealtimeSession: false)
    init(source: EngineConversationSource, id: String) {
        self.source = source
        self.id = id
        source.onSideSubmissionFailure = { [weak self] text, sourceCodeUnits, images in
            guard let self, !closing else { return }
            unsentPrompts.append(SideChatUnsentPrompt(text: text, sourceCodeUnits: sourceCodeUnits, images: images))
        }
    }

    var canRestoreUnsent: Bool { draft.isEmpty && draftSourceCodeUnits == nil && attachment == nil }

    @discardableResult
    func restoreUnsent(_ prompt: SideChatUnsentPrompt) -> Bool {
        guard canRestoreUnsent, unsentPrompts.contains(prompt), prompt.images.count <= 1 else { return false }
        var restoredAttachment: ComposerAttachment?
        if let imageRef = prompt.images.first {
            guard let data = Data(base64Encoded: imageRef.base64), let image = UIImage(data: data) else { return false }
            restoredAttachment = ComposerAttachment(image: image, width: Int(image.size.width), height: Int(image.size.height),
                mediaType: imageRef.mediaType, base64: imageRef.base64)
        }
        draft = prompt.text
        draftSourceCodeUnits = prompt.sourceCodeUnits
        attachment = restoredAttachment
        unsentPrompts.removeAll { $0.id == prompt.id }
        return true
    }

    func discardUnsent(_ prompt: SideChatUnsentPrompt) {
        unsentPrompts.removeAll { $0.id == prompt.id }
    }
}

@MainActor @Observable
final class SideChatCollection {
    private weak var parent: EngineConversationSource?
    var entries: [SideChatEntry] = []
    private(set) var expiredArchives: [ExpiredSideChatArchive] = []
    var selectedID: String?
    var error: String?
    var creating = false
    private var creation: Task<Void, Never>?
    private var endedDuringCreation = Set<String>()
    private var backgroundCancellation: Task<Void, Never>?
    private var isBackgrounded = false
    private var foregroundGeneration: UInt64 = 0
    private let createEntry: ((String) async throws -> SideChatEntry)?
    init(parent: EngineConversationSource, createEntry: ((String) async throws -> SideChatEntry)? = nil) {
        self.parent = parent
        self.createEntry = createEntry
    }

    /// Side turns receive no background lease, durable pause, or Live Activity.
    /// Keep this owned by the collection when its sheet is hidden.
    func cancelCurrentTurns() {
        isBackgrounded = true
        foregroundGeneration &+= 1
        guard backgroundCancellation == nil else { return }
        let current = entries
        for entry in current { entry.voice.handleContextChange() }
        backgroundCancellation = Task {
            defer { backgroundCancellation = nil }
            let cancellations = current.map { entry in
                Task { @MainActor () -> String? in
                    do { try await entry.source.cancelAndWait(); return nil }
                    catch { return error.localizedDescription }
                }
            }
            for cancellation in cancellations {
                if let message = await cancellation.value { error = message }
            }
        }
    }

    func handleForeground() { isBackgrounded = false }

    func removeEnded(_ sideID: String) {
        guard let entry = entries.first(where: { $0.id == sideID }) else {
            if creating { endedDuringCreation.insert(sideID) }
            return
        }
        let draft = entry.draft
        let units = entry.draftSourceCodeUnits
        let attachment = entry.attachment
        let chip = entry.source.model.visualizationChip
        let unsent = entry.unsentPrompts
        let rejected = entry.source.model.unsentPrompts
        let (parentID, parentEpoch, sideEpoch, outstanding) = entry.source.detachExpiredSideChat()
        entry.voice.close()
        if !entry.closing && (!draft.isEmpty || units != nil || attachment != nil || chip != nil ||
                              !unsent.isEmpty || !rejected.isEmpty || !outstanding.isEmpty) {
            expiredArchives.append(ExpiredSideChatArchive(id: sideID, parentSessionID: parentID, parentOwnerEpoch: parentEpoch,
                sideOwnerEpoch: sideEpoch,
                draft: draft, draftSourceCodeUnits: units, attachment: attachment,
                visualizationContext: chip, unsent: unsent, rejected: rejected, outstanding: outstanding))
        }
        entry.draft = ""
        entry.draftSourceCodeUnits = nil
        entry.attachment = nil
        entries.removeAll { $0.id == sideID }
        error = nil
        if selectedID == sideID, !expiredArchives.contains(where: { $0.id == sideID }) {
            selectedID = entries.last?.id ?? expiredArchives.last?.id
        }
    }

    func discardExpired(_ sideID: String) {
        expiredArchives.removeAll { $0.id == sideID }
        if selectedID == sideID { selectedID = entries.last?.id ?? expiredArchives.last?.id }
    }

    /// SessionHost calls this only after validating the exact parent owner on
    /// both sides of a drained replacement.
    func takeExpiredArchives() -> [ExpiredSideChatArchive] {
        let archives = expiredArchives
        expiredArchives.removeAll()
        if let selectedID, archives.contains(where: { $0.id == selectedID }) { self.selectedID = entries.last?.id }
        return archives
    }

    func adoptExpiredArchives(_ archives: [ExpiredSideChatArchive], selectedID preferred: String? = nil) {
        expiredArchives.append(contentsOf: archives)
        if let preferred, archives.contains(where: { $0.id == preferred }) { selectedID = preferred }
        else if selectedID == nil { selectedID = expiredArchives.last?.id }
    }

    private func retainAdmittedInitialInput(_ entry: SideChatEntry, text: String, units: [UInt16]?) {
        let identity = entry.source.sideArchiveIdentity
        let input = ExpiredSideChatInput(parentSessionID: identity.parentID, sideID: entry.id,
            sideOwnerEpoch: identity.sideEpoch, text: text, sourceCodeUnits: units, images: [], visualizationContext: nil)
        if let index = expiredArchives.firstIndex(where: { $0.id == entry.id }) {
            let existing = expiredArchives[index]
            let alreadyKept = existing.outstanding.contains {
                $0.parentSessionID == input.parentSessionID && $0.sideID == input.sideID && $0.sideOwnerEpoch == input.sideOwnerEpoch &&
                    $0.text == input.text && $0.sourceCodeUnits == input.sourceCodeUnits && $0.images.isEmpty
            } || existing.rejected.contains {
                $0.sessionID == input.parentSessionID && $0.sideID == input.sideID && $0.ownerEpoch == input.sideOwnerEpoch &&
                    $0.text == input.text && $0.sourceCodeUnits == input.sourceCodeUnits && $0.images.isEmpty
            }
            if !alreadyKept { expiredArchives[index] = existing.appendingOutstanding(input) }
        } else {
            expiredArchives.append(ExpiredSideChatArchive(id: entry.id, parentSessionID: identity.parentID,
                parentOwnerEpoch: identity.parentEpoch, sideOwnerEpoch: identity.sideEpoch,
                draft: "", draftSourceCodeUnits: nil, attachment: nil,
                visualizationContext: nil, unsent: [], rejected: [], outstanding: [input]))
            if selectedID == nil { selectedID = entry.id }
        }
    }

    /// Opening an empty sheet must produce a real, editable child without sending a prompt.
    func prepareForPresentation() {
        guard entries.isEmpty, expiredArchives.isEmpty, !creating else { return }
        create(input: nil) { _ in }
    }

    /// Manager owns the operation, so dismissing the sheet cannot cancel admission.
    func create(
        input: String?,
        inputSourceCodeUnits: [UInt16]? = nil,
        completion: @escaping (Bool) -> Void
    ) {
        guard !creating, let parent else { return }
        creating = true
        error = nil
        let requestID = UUID().uuidString
        let foregroundGeneration = self.foregroundGeneration
        creation = Task {
            defer { creating = false; creation = nil; endedDuringCreation.removeAll() }
            do {
                // Creating a child never starts work. Only this MainActor owner
                // may admit the first prompt after checking every background
                // transition that occurred while creation/hydration suspended.
                let entry: SideChatEntry
                if let createEntry {
                    entry = try await createEntry(requestID)
                } else {
                    let child = try await parent.makeSideChat(requestID: requestID)
                    entry = SideChatEntry(source: child, id: child.sideID!)
                }
                let child = entry.source
                if endedDuringCreation.remove(entry.id) != nil {
                    _ = child.detachExpiredSideChat()
                    entry.voice.close()
                    completion(false)
                    return
                }
                entries.append(entry)
                selectedID = entry.id
                if let input, !input.isEmpty {
                    let initialRevision = entry.draftRevision
                    var admitted = false
                    if !isBackgrounded && self.foregroundGeneration == foregroundGeneration {
                        admitted = await child.send(
                            input,
                            sourceCodeUnits: inputSourceCodeUnits,
                            images: [],
                            selectedSlashCommand: nil
                        ) != nil
                        // Idle expiry can remove this child while admission is
                        // suspended. Its original /side input still belongs to
                        // the parent composer until a live entry can own it.
                        guard entries.contains(where: { $0 === entry }) else {
                            if admitted && !entry.closing { retainAdmittedInitialInput(entry, text: input, units: inputSourceCodeUnits) }
                            completion(admitted)
                            return
                        }
                        if admitted && (isBackgrounded || self.foregroundGeneration != foregroundGeneration) {
                            // Background cancellation may have run while send was
                            // suspended. Drain again after admission so it cannot
                            // miss a turn that appeared after the first cancel.
                            do { try await child.cancelAndWait() }
                            catch { self.error = error.localizedDescription }
                        }
                    }
                    guard entries.contains(where: { $0 === entry }) else {
                        if admitted && !entry.closing { retainAdmittedInitialInput(entry, text: input, units: inputSourceCodeUnits) }
                        completion(admitted)
                        return
                    }
                    if !admitted {
                        // A newly selected child can be edited while admission
                        // suspends. Leave the parent's original draft intact if
                        // the child no longer owns an untouched empty draft.
                        guard entry.draftRevision == initialRevision, entry.draft.isEmpty,
                              entry.draftSourceCodeUnits == nil, entry.attachment == nil else {
                            completion(false)
                            return
                        }
                        entry.draft = input
                        entry.draftSourceCodeUnits = inputSourceCodeUnits
                    }
                }
                completion(true)
            } catch {
                self.error = error.localizedDescription
                parent.model.error = ConversationError(kind: .host, message: error.localizedDescription)
                completion(false)
            }
        }
    }

    func end(_ entry: SideChatEntry) {
        guard !entry.ending else { return }
        entry.closing = true
        entry.ending = true
        entry.voice.handleContextChange()
        Task {
            do {
                try await entry.source.endSideChat()
                entry.draft = ""
                entry.draftSourceCodeUnits = nil
                entry.attachment = nil
                entries.removeAll { $0.id == entry.id }
                error = nil
                selectedID = entries.last?.id
            } catch {
                self.error = error.localizedDescription
                // Cleanup failure retains Closing and the RAM state for retry.
                entry.ending = false
            }
        }
    }
}

struct SideChatSheet: View {
    @Bindable var collection: SideChatCollection
    @Environment(\.dismiss) private var dismiss
    private var selected: SideChatEntry? { collection.entries.first { $0.id == collection.selectedID } }
    private var selectedArchive: ExpiredSideChatArchive? {
        collection.expiredArchives.first { $0.id == collection.selectedID }
    }

    private var selectedTitle: String {
        if let index = collection.entries.firstIndex(where: { $0.id == collection.selectedID }) {
            return "\(String(localized: "side_chat_title")) \(index + 1)"
        }
        if let index = collection.expiredArchives.firstIndex(where: { $0.id == collection.selectedID }) {
            return "\(String(localized: "side_chat_title")) \(collection.entries.count + index + 1)"
        }
        return String(localized: "side_chats_title")
    }

    var body: some View {
        NavigationStack {
            Group {
                if let selected {
                    SideChatConversation(entry: selected)
                        .id(selected.id)
                        .disabled(selected.closing)
                } else if let selectedArchive {
                    ExpiredSideChatView(archive: selectedArchive)
                        .id(selectedArchive.id)
                } else {
                    if collection.creating {
                        ProgressView(String(localized: "side_chat_creating"))
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else {
                        ContentUnavailableView {
                            Label(String(localized: "side_chats_title"), systemImage: "bubble.left.and.bubble.right")
                        } description: {
                            Text(String(localized: "side_chat_empty"))
                        } actions: {
                            Button(String(localized: "side_chat_new")) { collection.prepareForPresentation() }
                                .accessibilityIdentifier("side-chat.create")
                        }
                    }
                }
            }
            .navigationTitle(selectedTitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Menu {
                        ForEach(Array(collection.entries.enumerated()), id: \.element.id) { index, entry in
                            Button("\(String(localized: "side_chat_title")) \(index + 1)") {
                                selected?.voice.handleContextChange()
                                collection.selectedID = entry.id
                            }
                        }
                        ForEach(Array(collection.expiredArchives.enumerated()), id: \.element.id) { index, archive in
                            Button("\(String(localized: "side_chat_title")) \(collection.entries.count + index + 1)") {
                                selected?.voice.handleContextChange()
                                collection.selectedID = archive.id
                            }
                        }
                    } label: {
                        Label {
                            Text(String(localized: "side_chats_title"))
                        } icon: {
                            Image(systemName: "bubble.left.and.bubble.right")
                                .resizable()
                                .scaledToFit()
                                .frame(width: 20, height: 20)
                        }
                            .labelStyle(.iconOnly)
                            .font(.system(size: 17, weight: .medium))
                            .frame(width: 44, height: 44)
                    }
                    .disabled(collection.entries.isEmpty && collection.expiredArchives.isEmpty)
                    .accessibilityIdentifier("side-chat.select")
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button {
                        selected?.voice.handleContextChange()
                        collection.create(input: nil) { _ in }
                    } label: {
                        Label(String(localized: "side_chat_new"), systemImage: "plus")
                            .labelStyle(.iconOnly)
                            .font(.system(size: 17, weight: .medium))
                            .frame(width: 44, height: 44)
                    }
                    .disabled(collection.creating)
                    .accessibilityIdentifier("side-chat.create")
                    if let selected {
                        Menu {
                            Button(selected.ending ? String(localized: "side_chat_closing") : selected.closing ? String(localized: "side_chat_retry_end") : String(localized: "side_chat_end"), role: .destructive) { collection.end(selected) }
                                .disabled(selected.ending)
                        } label: {
                            Label(String(localized: "side_chat_actions"), systemImage: "ellipsis")
                                .labelStyle(.iconOnly)
                                .font(.system(size: 17, weight: .medium))
                                .frame(width: 44, height: 44)
                        }
                    } else if let selectedArchive {
                        Menu {
                            Button(String(localized: "side_chat_end"), role: .destructive) {
                                collection.discardExpired(selectedArchive.id)
                            }
                        } label: {
                            Label(String(localized: "side_chat_actions"), systemImage: "ellipsis")
                                .labelStyle(.iconOnly)
                                .font(.system(size: 17, weight: .medium))
                                .frame(width: 44, height: 44)
                        }
                    }
                    Button { selected?.voice.handleContextChange(); dismiss() } label: {
                        Label(String(localized: "side_chat_hide"), systemImage: "xmark")
                            .labelStyle(.iconOnly)
                            .font(.system(size: 17, weight: .medium))
                            .frame(width: 44, height: 44)
                    }
                    .accessibilityIdentifier("side-chat.hide")
                }
            }
            .safeAreaInset(edge: .top) {
                if let error = collection.error {
                    Text(error).font(.caption).foregroundStyle(.red).padding()
                }
            }
        }
        .task { collection.prepareForPresentation() }
        .accessibilityIdentifier("side-chat.sheet")
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
    }
}

/// Expired side data has no source or engine. It remains visible only in its
/// captured parent's side-chat collection and is never sent automatically.
private struct ExpiredSideChatView: View {
    let archive: ExpiredSideChatArchive

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("side_chat_expired_read_only").font(.callout)
                if !archive.draft.isEmpty || archive.draftSourceCodeUnits != nil ||
                    archive.attachment != nil || archive.visualizationContext != nil {
                    item(text: archive.draft, units: archive.draftSourceCodeUnits, images: [],
                         attachment: archive.attachment, chip: archive.visualizationContext,
                         unconfirmed: archive.draftIsUnconfirmed)
                }
                ForEach(archive.unsent) { prompt in
                    item(text: prompt.text, units: prompt.sourceCodeUnits, images: prompt.images,
                         attachment: nil, chip: nil, unconfirmed: false)
                }
                ForEach(archive.rejected.filter { $0.sessionID == archive.parentSessionID && $0.sideID == archive.id &&
                    $0.ownerEpoch == archive.sideOwnerEpoch }) { prompt in
                    item(text: prompt.text, units: prompt.sourceCodeUnits, images: prompt.images,
                         attachment: nil, chip: prompt.visualizationContext, unconfirmed: false)
                }
                ForEach(archive.visibleOutstanding) { prompt in
                    item(text: prompt.text, units: prompt.sourceCodeUnits, images: prompt.images,
                         attachment: nil, chip: prompt.visualizationContext, unconfirmed: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding()
        }
        .accessibilityIdentifier("side-chat.expired.\(archive.id)")
    }

    private func item(text: String, units: [UInt16]?, images: [ImageRefDto],
                      attachment: ComposerAttachment?, chip: VisualizationContextChip?,
                      unconfirmed: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if unconfirmed { Text("side_chat_delivery_unconfirmed").font(.caption) }
            if !text.isEmpty || units != nil {
                Text(text).textSelection(.enabled)
                Button("chat_message_copy") { copyText(text, units: units) }
                    .font(.caption)
            }
            if let attachment {
                image(attachment.image, original: Data(base64Encoded: attachment.base64), mediaType: attachment.mediaType)
            }
            ForEach(Array(images.enumerated()), id: \.offset) { item in
                let reference = item.element
                if let data = Data(base64Encoded: reference.base64) {
                    if let decoded = UIImage(data: data) {
                        image(decoded, original: data, mediaType: reference.mediaType)
                    } else {
                        Button("chat_message_copy") { copyImageData(data, mediaType: reference.mediaType) }
                    }
                }
            }
            if let chip { Label(chip.title, systemImage: "chart.bar") }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func image(_ value: UIImage, original: Data?, mediaType: String) -> some View {
        Image(uiImage: value).resizable().scaledToFit().frame(maxHeight: 180)
            .contextMenu {
                Button("chat_message_copy") {
                    if let original, !original.isEmpty { copyImageData(original, mediaType: mediaType) }
                    else { UIPasteboard.general.image = value }
                }
            }
    }

    private func copyImageData(_ data: Data, mediaType: String) {
        let type = UTType(mimeType: mediaType)?.identifier ?? UTType.data.identifier
        UIPasteboard.general.setData(data, forPasteboardType: type)
    }

    private func copyText(_ text: String, units: [UInt16]?) {
        guard let units else { UIPasteboard.general.string = text; return }
        var bytes = Data([0xff, 0xfe])
        for unit in units {
            bytes.append(UInt8(truncatingIfNeeded: unit))
            bytes.append(UInt8(truncatingIfNeeded: unit >> 8))
        }
        UIPasteboard.general.setData(bytes, forPasteboardType: "public.utf16-external-plain-text")
    }
}

private struct SideChatConversation: View {
    @Bindable var entry: SideChatEntry
    var body: some View {
        let owner = entry
        VStack(spacing: 0) {
            ChatView(session: SessionRef(id: owner.id, title: String(localized: "side_chat_title")),
                 draft: Binding(get: { owner.draft }, set: { owner.draft = $0 }),
                 voiceInteraction: owner.voice, source: owner.source,
                 draftRevision: { owner.draftRevision },
                 draftSourceCodeUnits: Binding(get: { owner.draftSourceCodeUnits }, set: { owner.draftSourceCodeUnits = $0 }),
                 attachmentBinding: Binding(get: { owner.attachment }, set: { owner.attachment = $0 }))
                .background {
                    EnginePermissionPromptHost(model: entry.source.model,
                        onApprove: entry.source.approvePermission, onDeny: entry.source.denyPermission)
                }
            ForEach(owner.unsentPrompts) { prompt in
                HStack {
                    if prompt.text.isEmpty { Image(systemName: "photo") }
                    else { Text(prompt.text).lineLimit(2) }
                    Spacer()
                    Button("remote_restore_unsent") { owner.restoreUnsent(prompt) }
                        .disabled(!owner.canRestoreUnsent)
                    Button("common_close") { owner.discardUnsent(prompt) }
                }.font(.caption).padding(10)
            }
        }
    }
}
#endif

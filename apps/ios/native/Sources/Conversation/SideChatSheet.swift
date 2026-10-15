import Foundation
import Observation
import SwiftUI
import UIKit

/// Exact command boundary: ordinary prompts beginning with /sideways are untouched.
enum SideChatCommand {
    static func input(_ raw: String) -> String? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix("/side") else { return nil }
        let input = text.dropFirst(5)
        guard input.isEmpty || input.first?.isWhitespace == true else { return nil }
        return String(input).trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

#if canImport(harness_runtimeFFI)
@MainActor @Observable
final class SideChatEntry: Identifiable {
    let id: String
    let source: EngineConversationSource
    var draft = ""
    var attachment: ComposerAttachment?
    var closing = false
    var ending = false
    let voice = VoiceInteractionController(voiceCapture: VoiceCapture(), capability: VoiceCapabilityModel(),
        speechPlayer: SystemVoiceSpeechPlayer(), controlsRealtimeSession: false)
    init(source: EngineConversationSource, id: String) {
        self.source = source
        self.id = id
        source.onSideSubmissionFailure = { [weak self] text, images in
            guard let self, !closing else { return }
            if draft.isEmpty { draft = text }
            if attachment == nil, let imageRef = images.first,
               let data = Data(base64Encoded: imageRef.base64), let image = UIImage(data: data) {
                attachment = ComposerAttachment(image: image, width: Int(image.size.width), height: Int(image.size.height),
                    mediaType: imageRef.mediaType, base64: imageRef.base64)
            }
        }
    }
}

@MainActor @Observable
final class SideChatCollection {
    private weak var parent: EngineConversationSource?
    var entries: [SideChatEntry] = []
    var selectedID: String?
    var error: String?
    var creating = false
    private var creation: Task<Void, Never>?
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
        guard let entry = entries.first(where: { $0.id == sideID }) else { return }
        entry.voice.handleContextChange()
        entry.source.clearEndedSideChat()
        entry.draft = ""
        entry.attachment = nil
        entries.removeAll { $0.id == sideID }
        error = nil
        if selectedID == sideID { selectedID = entries.last?.id }
    }

    /// Manager owns the operation, so dismissing the sheet cannot cancel admission.
    func create(input: String?, completion: @escaping (Bool) -> Void) {
        guard !creating, let parent else { return }
        creating = true
        error = nil
        let requestID = UUID().uuidString
        let foregroundGeneration = self.foregroundGeneration
        creation = Task {
            defer { creating = false; creation = nil }
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
                entries.append(entry)
                selectedID = entry.id
                if let input, !input.isEmpty {
                    if !isBackgrounded && self.foregroundGeneration == foregroundGeneration {
                        if child.send(input) == nil { entry.draft = input }
                    } else {
                        entry.draft = input
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

    var body: some View {
        NavigationStack {
            Group {
                if let selected {
                    SideChatConversation(entry: selected)
                        .id(selected.id)
                        .disabled(selected.closing)
                } else {
                    ContentUnavailableView(String(localized: "side_chats_title"), systemImage: "bubble.left.and.bubble.right", description: Text(String(localized: "side_chat_empty")))
                }
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Menu {
                        ForEach(Array(collection.entries.enumerated()), id: \.element.id) { index, entry in
                            Button("\(String(localized: "side_chat_title")) \(index + 1)") {
                                selected?.voice.handleContextChange()
                                collection.selectedID = entry.id
                            }
                        }
                    } label: { Label(String(localized: "side_chats_title"), systemImage: "bubble.left.and.bubble.right") }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(String(localized: "side_chat_hide")) { selected?.voice.handleContextChange(); dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    if let selected {
                        Button(selected.ending ? String(localized: "side_chat_closing") : selected.closing ? String(localized: "side_chat_retry_end") : String(localized: "side_chat_end"), role: .destructive) { collection.end(selected) }
                            .disabled(selected.ending)
                    }
                }
            }
            .safeAreaInset(edge: .top) {
                if let error = collection.error {
                    Text(error).font(.caption).foregroundStyle(.red).padding()
                }
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
    }
}

private struct SideChatConversation: View {
    @Bindable var entry: SideChatEntry
    var body: some View {
        ChatView(session: SessionRef(id: entry.id, title: String(localized: "side_chat_title")), draft: $entry.draft,
                 voiceInteraction: entry.voice, source: entry.source, attachmentBinding: $entry.attachment)
            .background {
                EnginePermissionPromptHost(model: entry.source.model,
                    onApprove: entry.source.approvePermission, onDeny: entry.source.denyPermission)
            }
    }
}
#endif

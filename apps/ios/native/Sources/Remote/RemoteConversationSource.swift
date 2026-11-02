import Foundation
import ImageIO
import Observation
import UIKit

#if canImport(harness_runtimeFFI)
struct RemoteUnsentPrompt: Identifiable, Equatable {
    let id: UUID
    let text: String
    let sourceCodeUnits: [UInt16]?
    let images: [ImageRefDto]
}

/// Owns the composer's current draft separately from a rejected submission.
struct RemoteConversationDraft {
    var text = ""
    var sourceCodeUnits: [UInt16]?
    var attachment: ComposerAttachment?

    mutating func restore(_ prompt: RemoteUnsentPrompt) -> Bool {
        guard text.isEmpty, attachment == nil, prompt.images.count <= 1 else { return false }
        var restoredAttachment: ComposerAttachment?
        if let imageRef = prompt.images.first {
            guard let data = Data(base64Encoded: imageRef.base64), let image = UIImage(data: data) else { return false }
            restoredAttachment = ComposerAttachment(image: image, width: Int(image.size.width), height: Int(image.size.height),
                                                     mediaType: imageRef.mediaType, base64: imageRef.base64)
        }
        text = prompt.text
        sourceCodeUnits = prompt.sourceCodeUnits
        attachment = restoredAttachment
        return true
    }
}

@MainActor @Observable
final class RemoteDraftOwner {
    var draft = RemoteConversationDraft()
    var unsentPrompts: [RemoteUnsentPrompt] = []
}

/// A remote session has no local engine handle, durable-turn checkpoint, or
/// side chat. Its transcript is projected by the same reducer as local chat.
@MainActor
@Observable
final class RemoteConversationSource: ConversationSource {
    let hostID: String
    let sessionID: String
    let uid: String
    var sessionMode: SessionMode { model.sessionMode }
    var isApprovalCapability: Bool { RemoteApprovalCapability.accepts(sessionID) }
    let isTemporaryConversation = true
    var model: ConversationModel { projection.model }
    private(set) var phase: RemoteSessionPhaseFfi = .loading
    private(set) var outgoing: [RemoteOutgoingPromptFfi] = []
    private(set) var hasOlder = false
    private(set) var loadingOlder = false
    private(set) var computerAccessPending = false
    private(set) var answeredElsewhere = false
    private(set) var hasOmittedContent = false
    private(set) var sending = false
    let draftOwner: RemoteDraftOwner
    var unsentPrompts: [RemoteUnsentPrompt] { draftOwner.unsentPrompts }
    @ObservationIgnored private let manager: RemoteControlManager
    @ObservationIgnored private let projection: EngineConversationSource
    @ObservationIgnored private let remotePromptSender: ((String, [RemotePromptImageFfi]) async throws -> Void)?
    @ObservationIgnored private let remotePermissionResponder: ((UInt64, Bool) async throws -> Void)?
    @ObservationIgnored private var modeAdmission = RemoteModeAdmission<RemoteSessionUpdateFfi>()
    @ObservationIgnored private var approvalFailed = false
    @ObservationIgnored private var history: [MessageDto] = []
    @ObservationIgnored private var records: [RemoteSeqRecordFfi] = []
    @ObservationIgnored private var lastSequence: UInt64?
    @ObservationIgnored private var imageTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var imageCache: [String: RemoteImageFfi] = [:]
    @ObservationIgnored private var imageEpoch: UInt64 = 0
    @ObservationIgnored private var promptBotNames = RemotePromptBotNames()
    @ObservationIgnored private var valid = true
    @ObservationIgnored private var nextSendID: UInt64 = 1

    init(
        hostID: String,
        sessionID: String,
        uid: String,
        manager: RemoteControlManager? = nil,
        mediaHostComposition: IOSNativeMediaHostComposition? = nil,
        mediaHostInitializationError: String? = nil,
        remotePromptSender: ((String, [RemotePromptImageFfi]) async throws -> Void)? = nil,
        remotePermissionResponder: ((UInt64, Bool) async throws -> Void)? = nil
    ) {
        self.hostID = hostID
        self.sessionID = sessionID
        self.uid = uid
        self.remotePromptSender = remotePromptSender
        self.remotePermissionResponder = remotePermissionResponder
        let manager = manager ?? RemoteControlManager(
            mediaHostComposition: mediaHostComposition,
            mediaHostInitializationError: mediaHostInitializationError
        )
        self.manager = manager
        self.draftOwner = manager.draftOwner(hostID: hostID, sessionID: sessionID, uid: uid)
        projection = EngineConversationSource(config: EngineConfig(
            apiBase: "", apiKey: "", model: "", appSandboxRoot: "", projectCwd: nil,
            sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        projection.replaceRemoteTranscript([], sessionID: sessionID)
        model.slashCommandsLoaded = true
    }

    func start() async {
        guard valid else { return }
        do {
            if manager.uid != uid { await manager.configure(uid: uid) }
            guard valid, !Task.isCancelled else { return }
            try await manager.follow(self)
        } catch is CancellationError { }
        catch { if valid { model.error = ConversationError(kind: .host, message: String(localized: "remote_err_generic")) } }
    }

    func stop() async { await manager.unfollow(self); phase = .offline }

    func invalidate() {
        valid = false
        imageEpoch &+= 1
        for task in imageTasks.values { task.cancel() }
        imageTasks.removeAll()
        imageCache.removeAll()
        promptBotNames.removeAll()
        modeAdmission.invalidate()
        history = []
        records = []
        outgoing = []
        projection.replaceRemoteTranscript([], sessionID: sessionID)
        phase = .closed
    }

    private func client() throws -> IosRemoteControlClient {
        guard valid, manager.connectionTicket(hostID: hostID, uid: uid) != nil else { throw RemoteFfiError.NotConnected }
        return try manager.activeClient()
    }

    func receive(_ update: RemoteSessionUpdateFfi) {
        guard valid, update.sessionId == sessionID else { return }
        if let code = update.error?.code,
           code == "SESSION_MODE_OVERFLOW" || code == "SESSION_MODE_MISMATCH" {
            // The native client may reject while withholding Loading records;
            // no later update can make that incomplete projection trustworthy.
            modeAdmission.invalidate()
            approvalFailed = true
            failModeAdmission()
            return
        }
        if isApprovalCapability {
            receiveApproval(update)
            return
        }
        var announcedModes = Set<SessionMode>()
        if let mode = update.sessionMode { announcedModes.insert(SessionMode(dto: mode)) }
        for entry in update.records {
            guard case let .event(event, _) = entry.record else { continue }
            switch event {
            case let .sessionStarted(id, mode), let .sessionResumed(id, mode, _):
                if canonicalSessionID(id) == canonicalSessionID(sessionID) {
                    announcedModes.insert(SessionMode(dto: mode))
                }
            default: break
            }
        }
        switch modeAdmission.receive(update, announcedModes: announcedModes,
                                     estimatedBytes: { RemoteUpdateMemoryEstimate.bytes(of: update) }) {
        case .waiting:
            // A LIVE partial update alone does not identify Chat or Code.
            phase = update.phase == .live ? .loading : update.phase
            if let error = update.error { model.error = ConversationError(kind: .host, message: error.message) }
        case let .replay(mode, updates):
            guard projection.confirmRemoteSessionMode(mode, sessionID: sessionID) else {
                modeAdmission.invalidate()
                failModeAdmission()
                return
            }
            for accepted in updates { applyUpdate(accepted) }
        case let .apply(accepted):
            applyUpdate(accepted)
        case .failed:
            failModeAdmission()
        }
    }

    private func receiveApproval(_ update: RemoteSessionUpdateFfi) {
        guard !approvalFailed, update.sessionMode == nil,
              update.messages.isEmpty, update.olderMessages.isEmpty, update.records.isEmpty,
              update.controls == nil, update.outgoing.isEmpty,
              update.pending.questions.isEmpty, update.pending.computerAccess.isEmpty,
              !update.turn.running else {
            approvalFailed = true
            failModeAdmission()
            return
        }
        phase = update.phase
        projection.reconcileRemoteState(update)
        if let error = update.error { model.error = ConversationError(kind: .host, message: error.message) }
        else if update.phase == .live { model.error = nil }
    }

    private func failModeAdmission() {
        phase = .error
        model.error = ConversationError(kind: .host, message: String(localized: "remote_err_generic"))
    }

    private func applyUpdate(_ update: RemoteSessionUpdateFfi) {
        let becameLive = phase != .live && update.phase == .live
        phase = update.phase
        hasOlder = update.hasOlder
        loadingOlder = update.loadingOlder
        outgoing = update.outgoing
        answeredElsewhere = update.answeredElsewhere
        computerAccessPending = !update.pending.computerAccess.isEmpty
        if update.replace {
            imageEpoch &+= 1
            hasOmittedContent = false
            history = update.messages
            records = update.records
            lastSequence = update.records.last?.seq
            for task in imageTasks.values { task.cancel() }
            imageTasks.removeAll()
            imageCache.removeAll()
            promptBotNames.removeAll()
            rebuild()
        } else {
            let appended = update.records.filter { lastSequence == nil || $0.seq > lastSequence! }
            records.append(contentsOf: appended)
            if let seq = appended.last?.seq { lastSequence = seq }
            if !update.olderMessages.isEmpty {
                history.insert(contentsOf: update.olderMessages, at: 0)
                rebuild()
            } else {
                for record in appended { apply(record) }
            }
        }
        projection.reconcileRemoteState(update)
        if let error = update.error { model.error = ConversationError(kind: .host, message: error.message) }
        hydrateImages()
        if becameLive {
            model.error = nil
            perform { client in
                let models = try await client.listModels()
                guard self.valid else { return }
                self.projection.applyRemoteEvent(.modelList(models: models.models, current: models.current,
                    currentModelSource: nil, details: models.details))
            }
        }
    }

    private func rebuild() {
        projection.replaceRemoteTranscript(history, sessionID: sessionID)
        for record in records { apply(record) }
    }

    private func apply(_ entry: RemoteSeqRecordFfi) {
        switch entry.record {
        case let .event(event, truncated):
            projection.applyRemoteEvent(event)
            if truncated { hasOmittedContent = true }
        case let .prompt(turnID, text, images, origin, deviceName):
            // A Bot's prompt reads as scheduled in its record; the client knows which Bot sent it.
            let label = RemotePromptOriginLabel.text(for: origin, deviceName: deviceName) {
                promptBotNames.name(forTurn: turnID) {
                    (try? client())?.promptOriginBotName(sessionId: sessionID, turnId: turnID)
                }
            }
            projection.appendRemotePrompt(turnID: turnID, text: text,
                images: images.map { MessageImage(mediaType: $0.mediaType, url: $0.imageRef) }, origin: label)
        case .permissionRequest: break // The complete pending set below is authoritative.
        case .computerAccessRequest: computerAccessPending = true
        case .omitted, .unsupported: hasOmittedContent = true
        }
    }

    private func hydrateImages() {
        let references = Set(model.messages.flatMap(\.images).map(\.url).filter { $0.hasPrefix(sessionID + ":") })
        for reference in references {
            if let image = imageCache[reference] {
                projection.hydrateRemoteImage(reference: reference, mediaType: image.mediaType, bytes: image.bytes)
            } else if imageTasks[reference] == nil {
                let epoch = imageEpoch
                imageTasks[reference] = Task { [weak self] in
                    guard let self else { return }
                    do {
                        guard let ticket = manager.connectionTicket(hostID: hostID, uid: uid) else { throw RemoteFfiError.NotConnected }
                        let image = try await manager.withSessionClient(hostID: hostID, uid: uid, ticket: ticket) {
                            try await $0.fetchImage(sessionId: sessionID, imageRef: reference)
                        }
                        guard valid, epoch == imageEpoch, !Task.isCancelled else { return }
                        imageCache[reference] = image
                        projection.hydrateRemoteImage(reference: reference, mediaType: image.mediaType, bytes: image.bytes)
                    } catch { }
                    if epoch == imageEpoch { imageTasks.removeValue(forKey: reference) }
                }
            }
        }
    }

    private var canControl: Bool { valid && model.sessionModeKnown && phase == .live }
    var hasReadyConnection: Bool { valid && manager.connectionTicket(hostID: hostID, uid: uid) != nil }
    var isAvailableForPrompts: Bool {
        canControl && !sending && (remotePromptSender != nil || hasReadyConnection)
    }

    private func perform(requiresLive: Bool = true, allowApproval: Bool = false,
                         _ operation: @escaping (IosRemoteControlClient) async throws -> Void) {
        let canApprove = isApprovalCapability && !approvalFailed && phase == .live
        guard valid, !requiresLive || canControl || (allowApproval && canApprove) else { return }
        guard let ticket = manager.connectionTicket(hostID: hostID, uid: uid),
              let client = try? manager.admitSessionRequest(hostID: hostID, uid: uid, ticket: ticket) else { return }
        Task { [self] in
            defer { manager.finishSessionRequest() }
            do { try await operation(client) }
            catch { if valid { model.error = ConversationError(kind: .host, message: String(localized: "remote_err_generic")) } }
        }
    }

    func startNewConversation() {}
    func openSession(_ session: SessionRef) {}
    func dismissError() { model.error = nil }
    func warmUp() {}
    func setFastMode(_ enabled: Bool) {}
    func confirmAndSetBypassPermissions(suppressWarning: Bool) {
        model.error = ConversationError(kind: .rejected, message: String(localized: "remote_bypass_desktop_only"))
    }

    @discardableResult
    func send(_ text: String) -> ConversationTurnToken? { send(text, images: []) }
    @discardableResult
    func send(_ text: String, images: [ImageRefDto]) -> ConversationTurnToken? {
        submit(text, sourceCodeUnits: nil, images: images)
    }

    private func submit(_ text: String, sourceCodeUnits: [UInt16]?, images: [ImageRefDto]) -> ConversationTurnToken? {
        guard canControl, !sending, (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty) else { return nil }
        guard !(model.streaming && !images.isEmpty) else {
            model.error = ConversationError(kind: .rejected, message: String(localized: "remote_images_only_when_idle")); return nil
        }
        let client: IosRemoteControlClient?
        if remotePromptSender != nil {
            client = nil
        } else if let ticket = manager.connectionTicket(hostID: hostID, uid: uid),
                  let admitted = try? manager.admitSessionRequest(hostID: hostID, uid: uid, ticket: ticket) {
            client = admitted
        } else {
            return nil
        }
        sending = true
        let token = ConversationTurnToken(clientTurnId: nextSendID, sessionEpoch: 0)
        nextSendID &+= 1
        let unsent = RemoteUnsentPrompt(id: UUID(), text: text, sourceCodeUnits: sourceCodeUnits, images: images)
        // A synchronous submission has been accepted by the composer. Keep
        // its owner alive until FFI admission succeeds or its draft is saved.
        Task { [self] in
            defer {
                sending = false
                if client != nil { manager.finishSessionRequest() }
            }
            do {
                // Image decoding/compression is CPU work; it owns immutable DTO
                // copies and runs off the UI actor. All UI state stays here.
                let prepared = try await Task.detached(priority: .userInitiated) {
                    try RemoteImageCompressor.prepare(images)
                }.value
                let submittedText = sourceCodeUnits.map { String(decoding: $0, as: UTF16.self) } ?? text
                if let remotePromptSender {
                    try await remotePromptSender(submittedText, prepared)
                } else if let client {
                    try await client.sendPrompt(sessionId: sessionID, text: submittedText, images: prepared)
                }
            } catch {
                if valid || manager.uid == uid { draftOwner.unsentPrompts.append(unsent) }
                if valid { model.error = ConversationError(kind: .host, message: String(localized: "remote_send_failed")) }
            }
        }
        return token
    }
    func send(_ text: String, sourceCodeUnits: [UInt16]?, images: [ImageRefDto], selectedSlashCommand: ConversationSlashCommand?) -> ConversationTurnToken? {
        submit(text, sourceCodeUnits: sourceCodeUnits, images: images)
    }
    func removeUnsentPrompt(_ id: UUID) { draftOwner.unsentPrompts.removeAll { $0.id == id } }
    func cancel() { perform { try await $0.cancelTurn(sessionId: self.sessionID) } }
    func cancelAndWait() async throws {
        guard canControl, let ticket = manager.connectionTicket(hostID: hostID, uid: uid) else { throw RemoteFfiError.NotConnected }
        try await manager.withSessionClient(hostID: hostID, uid: uid, ticket: ticket) {
            try await $0.cancelTurn(sessionId: sessionID)
        }
    }
    func setModel(_ id: String) { perform { try await $0.setModel(sessionId: self.sessionID, model: id) } }
    func setPermissionMode(_ mode: String) {
        let modes: [String: RemotePermissionModeFfi] = ["default": .default, "acceptEdits": .acceptEdits, "plan": .plan, "auto": .auto, "dontAsk": .dontAsk]
        guard let value = modes[mode] else { confirmAndSetBypassPermissions(suppressWarning: false); return }
        perform { try await $0.setPermissionMode(sessionId: self.sessionID, mode: value) }
    }
    func setReasoningSelection(_ selection: String) {
        let value: ReasoningSelectionDto
        switch selection {
        case "automatic": value = .automatic
        case "disabled": value = .disabled
        case "enabled": value = .enabled
        default:
            if selection.hasPrefix("budget:"), let tokens = UInt64(selection.dropFirst(7)) { value = .tokenBudget(tokens: tokens) }
            else { value = .level(id: selection) }
        }
        perform { try await $0.setReasoning(sessionId: self.sessionID, selection: value) }
    }
    func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto) {
        guard response == .allowOnce else { return }
        respondPermission(requestId, allow: true)
    }
    func denyPermission(_ requestId: UInt64) {
        respondPermission(requestId, allow: false)
    }
    private func respondPermission(_ requestId: UInt64, allow: Bool) {
        let canApprove = isApprovalCapability && !approvalFailed && phase == .live
        guard valid, canControl || canApprove else { return }
        if let remotePermissionResponder {
            Task { [weak self] in
                do { try await remotePermissionResponder(requestId, allow) }
                catch { if let self, valid { model.error = ConversationError(kind: .host, message: String(localized: "remote_err_generic")) } }
            }
        } else {
            perform(allowApproval: true) { try await $0.respondPermission(sessionId: self.sessionID, requestId: requestId, allow: allow) }
        }
    }
    func submitEngineCommand(_ command: ClientCommand) async throws {
        guard canControl, let ticket = manager.connectionTicket(hostID: hostID, uid: uid) else { throw RemoteFfiError.NotConnected }
        switch command {
        case let .answerAskUserQuestion(id, answers):
            var values: [String: String] = [:]
            for pair in answers.pairs { values[pair.question.displayText] = pair.answer.displayText }
            try await manager.withSessionClient(hostID: hostID, uid: uid, ticket: ticket) {
                try await $0.answerQuestion(sessionId: sessionID, requestId: id, answers: values)
            }
        case let .cancelAskUserQuestion(id):
            try await manager.withSessionClient(hostID: hostID, uid: uid, ticket: ticket) {
                try await $0.cancelQuestion(sessionId: sessionID, requestId: id)
            }
        default: throw RemoteFfiError.Rpc(code: "forbidden", detail: String(localized: "remote_err_forbidden"))
        }
    }
    func loadOlder() { perform { try await $0.loadOlderMessages(sessionId: self.sessionID) } }
    func retryPrompt(_ id: String) { perform { try await $0.retryPrompt(sessionId: self.sessionID, promptId: id) } }
    func dismissPrompt(_ id: String) { perform { try await $0.dismissPrompt(sessionId: self.sessionID, promptId: id) } }
    func handleBackground() { perform(requiresLive: false) { try await $0.setSessionForeground(sessionId: self.sessionID, foreground: false) } }
    func handleForeground() {
        Task { await manager.nudge() }
        perform(requiresLive: false) { try await $0.setSessionForeground(sessionId: self.sessionID, foreground: true) }
    }
}

enum RemoteImageCompressor {
    static func prepare(_ images: [ImageRefDto]) throws -> [RemotePromptImageFfi] {
        guard images.count <= 4 else { throw RemoteFfiError.InvalidArgument(message: "At most four images") }
        return try images.map { image in
            guard let data = Data(base64Encoded: image.base64),
                  let source = CGImageSourceCreateWithData(data as CFData, nil) else {
                throw RemoteFfiError.InvalidArgument(message: "Invalid image")
            }
            for side in [2048, 1536, 1024, 768] {
                let options: [CFString: Any] = [kCGImageSourceCreateThumbnailFromImageAlways: true,
                    kCGImageSourceCreateThumbnailWithTransform: true, kCGImageSourceThumbnailMaxPixelSize: side]
                guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { continue }
                if let jpeg = UIImage(cgImage: thumbnail).jpegData(compressionQuality: 0.82), jpeg.count <= 2 * 1024 * 1024 {
                    return RemotePromptImageFfi(mediaType: "image/jpeg", bytes: jpeg)
                }
            }
            throw RemoteFfiError.InvalidArgument(message: "Image too large")
        }
    }
}
#endif

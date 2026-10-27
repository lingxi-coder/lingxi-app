import Foundation
import ImageIO
import Observation
import UIKit

#if canImport(harness_runtimeFFI)
/// A remote session has no local engine handle, durable-turn checkpoint, or
/// side chat. Its transcript is projected by the same reducer as local chat.
@MainActor
@Observable
final class RemoteConversationSource: ConversationSource {
    let hostID: String
    let sessionID: String
    let uid: String
    let sessionMode: SessionMode = .code
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
    @ObservationIgnored private let manager: RemoteControlManager
    @ObservationIgnored private let projection: EngineConversationSource
    @ObservationIgnored private var history: [MessageDto] = []
    @ObservationIgnored private var records: [RemoteSeqRecordFfi] = []
    @ObservationIgnored private var lastSequence: UInt64?
    @ObservationIgnored private var imageTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var imageCache: [String: RemoteImageFfi] = [:]
    @ObservationIgnored private var imageEpoch: UInt64 = 0
    @ObservationIgnored private var valid = true
    @ObservationIgnored private var nextSendID: UInt64 = 1

    init(
        hostID: String,
        sessionID: String,
        uid: String,
        manager: RemoteControlManager? = nil,
        mediaHostComposition: IOSNativeMediaHostComposition? = nil,
        mediaHostInitializationError: String? = nil
    ) {
        self.hostID = hostID
        self.sessionID = sessionID
        self.uid = uid
        self.manager = manager ?? RemoteControlManager(
            mediaHostComposition: mediaHostComposition,
            mediaHostInitializationError: mediaHostInitializationError
        )
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
        history = []
        records = []
        outgoing = []
        projection.replaceRemoteTranscript([], sessionID: sessionID)
        phase = .closed
    }

    private func client() throws -> IosRemoteControlClient {
        guard valid, manager.uid == uid else { throw RemoteFfiError.NotConnected }
        return try manager.activeClient()
    }

    func receive(_ update: RemoteSessionUpdateFfi) {
        guard valid, update.sessionId == sessionID else { return }
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
            let label: String
            switch origin {
            case .desktop: label = String(localized: "remote_origin_desktop")
            case .scheduled: label = String(localized: "remote_origin_scheduled")
            case .device: label = deviceName ?? String(localized: "remote_origin_other_device")
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
                        let image = try await client().fetchImage(sessionId: sessionID, imageRef: reference)
                        guard valid, epoch == imageEpoch, !Task.isCancelled else { return }
                        imageCache[reference] = image
                        projection.hydrateRemoteImage(reference: reference, mediaType: image.mediaType, bytes: image.bytes)
                    } catch { }
                    if epoch == imageEpoch { imageTasks.removeValue(forKey: reference) }
                }
            }
        }
    }

    private func perform(_ operation: @escaping (IosRemoteControlClient) async throws -> Void) {
        Task { [weak self] in
            guard let self else { return }
            do { try await operation(client()) }
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
        guard valid, phase == .live, !sending, (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty) else { return nil }
        guard !(model.streaming && !images.isEmpty) else {
            model.error = ConversationError(kind: .rejected, message: String(localized: "remote_images_only_when_idle")); return nil
        }
        sending = true
        let token = ConversationTurnToken(clientTurnId: nextSendID, sessionEpoch: 0)
        nextSendID &+= 1
        Task { [weak self] in
            guard let self else { return }
            defer { sending = false }
            do {
                // Image decoding/compression is CPU work; it owns immutable DTO
                // copies and runs off the UI actor. All UI state stays here.
                let prepared = try await Task.detached(priority: .userInitiated) {
                    try RemoteImageCompressor.prepare(images)
                }.value
                try await client().sendPrompt(sessionId: sessionID, text: text, images: prepared)
            } catch {
                if valid { model.error = ConversationError(kind: .host, message: String(localized: "remote_send_failed")) }
            }
        }
        return token
    }
    func send(_ text: String, sourceCodeUnits: [UInt16]?, images: [ImageRefDto], selectedSlashCommand: ConversationSlashCommand?) -> ConversationTurnToken? {
        send(sourceCodeUnits.map { String(decoding: $0, as: UTF16.self) } ?? text, images: images)
    }
    func cancel() { perform { try await $0.cancelTurn(sessionId: self.sessionID) } }
    func cancelAndWait() async throws { try await client().cancelTurn(sessionId: sessionID) }
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
        perform { try await $0.respondPermission(sessionId: self.sessionID, requestId: requestId, allow: true) }
    }
    func denyPermission(_ requestId: UInt64) {
        perform { try await $0.respondPermission(sessionId: self.sessionID, requestId: requestId, allow: false) }
    }
    func submitEngineCommand(_ command: ClientCommand) async throws {
        switch command {
        case let .answerAskUserQuestion(id, answers):
            var values: [String: String] = [:]
            for pair in answers.pairs { values[pair.question.displayText] = pair.answer.displayText }
            try await client().answerQuestion(sessionId: sessionID, requestId: id, answers: values)
        case let .cancelAskUserQuestion(id): try await client().cancelQuestion(sessionId: sessionID, requestId: id)
        default: throw RemoteFfiError.Rpc(code: "forbidden", detail: String(localized: "remote_err_forbidden"))
        }
    }
    func loadOlder() { perform { try await $0.loadOlderMessages(sessionId: self.sessionID) } }
    func retryPrompt(_ id: String) { perform { try await $0.retryPrompt(sessionId: self.sessionID, promptId: id) } }
    func dismissPrompt(_ id: String) { perform { try await $0.dismissPrompt(sessionId: self.sessionID, promptId: id) } }
    func handleBackground() { perform { try await $0.setSessionForeground(sessionId: self.sessionID, foreground: false) } }
    func handleForeground() {
        Task { await manager.nudge() }
        perform { try await $0.setSessionForeground(sessionId: self.sessionID, foreground: true) }
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

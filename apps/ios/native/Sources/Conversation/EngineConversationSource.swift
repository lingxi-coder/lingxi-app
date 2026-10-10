import Combine
import Foundation
import OSLog
import SwiftUI

#if canImport(harness_runtimeFFI)
import AuthenticationServices
import UIKit
import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)
/// The real conversation source: an in-process engine reached over UniFFI.
///
/// Lifecycle: lazily build the `MobileEngineHandle` on first `send`; register
/// `EngineListener` (an `IosEventListener`) whose `onEvent(_:)` maps each
/// `ClientEvent` onto the `ConversationModel`. Turns are submitted via
/// `handle.submit(.sendPrompt(...))`; the engine streams `TextDelta` /
/// `ToolUse*` / `TurnEnded` back through the listener.
@MainActor
final class EngineConversationSource: ConversationSource {
    private static let turnLog = Logger(
        subsystem: "com.lingxi.code",
        category: "conversation-turn"
    )
    /// Project persistence replaces its cached index from SessionList, so
    /// iOS must override the host's five-row default with the full u32 range.
    private static let completeSessionListLimit = UInt32.max

    typealias HandleBuilder = (
        _ config: IosEngineLaunchConfigFfi,
        _ listener: IosEventListener,
        _ audio: IOSAudioServiceCallbackAdapter,
        _ permissions: IosPermissionSink
    ) throws -> MobileEngineHandle

    let model: ConversationModel
    let sessionMode: SessionMode

    private let config: EngineConfig
    private let handleBuilder: HandleBuilder
    private let permissionModeRepository: PermissionModeConfigurationRepository
    private var handle: MobileEngineHandle?
    /// One shared bootstrap attempt for every entry point that needs the
    /// engine. Access to this property remains main-actor-confined, which
    /// prevents interleaving callers from constructing competing handles;
    /// the task itself runs off-main because engine construction may install
    /// and boot the bundled Linux runtime.
    private var handleBuildTask: Task<MobileEngineHandle, Error>?
    private var handleBuildAttemptID: UInt64 = 0
    private var listener: EngineListener?
    /// The permission sink registered with the engine (SHIP-BLOCKER #3). Held so
    /// it outlives `ensureHandle`; Rust calls `onRequest` on it when a tool needs
    /// approval.
    private var permissionSink: EnginePermissionSink?
    private var externalEventHandler: ((ClientEvent) -> Void)?
    private static weak var settingsOwner: EngineConversationSource?
    private var settingsGeneration: UInt64 = 0
    private var providerCatalogEntries: [ProviderCatalogEntry] = []
    private var providerCatalogLoaded = false
    private var providerCatalogWaiters: [CheckedContinuation<[ProviderCatalogEntry], Error>] = []
    private var oauthSession: ASWebAuthenticationSession?
    private var oauthPresentationProvider: OAuthPresentationContextProvider?
    private var oauthFlowID: String?
    private var oauthCallbackHandler: ((URL) -> Void)?
    /// Index into `model.messages` of the assistant message currently being
    /// streamed (deltas append into it). `nil` between turns.
    private var streamingIndex: Int?
    /// The matching render-list slot for the in-flight assistant message.
    private var streamingItemIndex: Int?
    /// Narrative rows opened during the current assistant API response. A
    /// tool/reasoning event can split one response into multiple timeline
    /// rows; MessageComplete reconciles its structured payload back onto
    /// these stable identities instead of replacing only the last fragment.
    private var currentResponseMessageIDs: [UUID] = []
    private var currentResponseReasoningIDs: Set<String> = []
    private var pendingAssistantIdentity: String?
    private var assistantRowsByIdentity: [String: Set<UUID>] = [:]
    private var assistantReasoningByIdentity: [String: Set<String>] = [:]
    /// Process-unique monotonic correlator, also passed as the engine
    /// `turnId` so durable checkpoints cannot collide after a cold launch.
    /// `nil` between turns.
    private var currentTurnId: UInt64?
    private var nextTurnId: UInt64 = UInt64.random(in: 1..<UInt64.max)
    /// Session-scoped event guard. Increment whenever the visible session
    /// changes so late events from an abandoned session/turn are ignored.
    private var sessionEpoch: UInt64 = 1
    private var activeTurnEpoch: UInt64?
    /// The executor that owns the visible turn while it is live. A
    /// `WaitingForUser` snapshot can therefore mean either an ordinary
    /// in-foreground question (the executor is still attached) or an
    /// executor-less durable recovery after reattach. Keep those states
    /// distinct so only the latter gets the inactive Discard affordance.
    private var executorOwnedTurnID: UInt64?
    private var turnSpeechSequence: UInt64 = 0
    private var activeRunItemIndex: Int?
    /// Original user spelling for a slash command awaiting dispatch.
    private var pendingSlashRaw: String?
    private var testCommandSubmitter: ((ClientCommand) async throws -> Void)?
    private var testBypassPermissionsConfirmer: (() async throws -> Void)?
    private var testEmptySessionResumer: ((String, String) async throws -> Void)?
    private var cancellationOperation: CancellationOperation?
    private var nextCancellationOperationID: UInt64 = 1
    private var activeSessionTransitionOperationID: UInt64?
    private var nextSessionTransitionOperationID: UInt64 = 1
    private let durableTurns: DurableConversationTurnClientStore
    private var reattachedTurnIDs = Set<UInt64>()
    /// A non-terminal durable recovery owns the single turn slot even when
    /// the UI is no longer streaming. Keep this id until ResumeTurn reports
    /// Running or a terminal state so a new prompt cannot overwrite the
    /// sole durable checkpoint.
    private var unresolvedRecoveryTurnID: UInt64?
    /// Set only when a foreground attach is required for a paused turn.
    /// WaitingForUser can also be emitted for an ordinary in-foreground
    /// question and must not trigger an unsolicited attach/resume cycle.
    private var needsDurableForegroundRecoveryTurnID: UInt64?
    /// One AttachTurn/ResumeTurn chain may be in flight at a time. The
    /// source can receive duplicate foreground callbacks while SwiftUI
    /// rebuilds its scene; the task is the idempotency barrier for those
    /// callbacks.
    private var durableRecoveryTask: Task<Void, Never>?
    private var replayingTurnID: UInt64?
    /// AttachTurn and ResumeTurn both emit recovery snapshots. A Running
    /// snapshot from Attach is not a Resume acknowledgement: the source
    /// still has to submit ResumeTurn before it can expose an active
    /// executor to the composer.
    private enum DurableRecoveryPhase {
        case attaching(turnID: UInt64)
        case resuming(turnID: UInt64)
    }
    private var durableRecoveryPhase: DurableRecoveryPhase?
    /// The recovery state received for the currently attached durable turn.
    /// A terminal SessionResumed transcript is authoritative; retained
    /// envelopes are only a projection source while recovery is nonterminal.
    private var replayProjectionState: TurnRecoveryStateDto?
    private var pauseAcknowledgements: [ConversationTurnToken: [CheckedContinuation<Void, Error>]] = [:]
    /// Cancel's command acknowledgement only means the host accepted the
    /// request. Keep ownership until the FIFO listener observes a terminal
    /// recovery snapshot for the same session/turn.
    private var terminalRecoveryStates: [UInt64: TurnRecoveryStateDto] = [:]

    private enum PendingSessionTransition: Equatable {
        case new
        case resume(String)
    }

    private enum SessionTransitionSubmission {
        case command(ClientCommand)
        case resumeEmpty(sessionID: String, title: String)
    }

    private var pendingSessionTransition: PendingSessionTransition?
    /// True only after this source observed the engine's authoritative
    /// SessionStarted/SessionResumed confirmation. Unlike `model.isNew`, it
    /// distinguishes a confirmed empty session from a newly recreated source
    /// that still needs transcript replay.
    private var hasConfirmedSessionState = false

    private struct TurnPrompt: Equatable {
        let text: String
        let images: [ImageRefDto]
        let turnId: UInt64
        var visualizationContext: VisualizationContextChip? = nil
    }

    private struct CancellationOperation {
        let id: UInt64
        let turnId: UInt64
        let epoch: UInt64
        let task: Task<Void, Error>
    }

    init(
        config: EngineConfig,
        handleBuilder: @escaping HandleBuilder = EngineConversationSource.buildDefaultHandle,
        permissionModeRepository: PermissionModeConfigurationRepository? = nil
    ) {
        self.config = config
        sessionMode = config.sessionMode
        self.durableTurns = DurableConversationTurnClientStore(config: config)
        self.handleBuilder = handleBuilder
        self.permissionModeRepository = permissionModeRepository
            ?? PermissionModeConfigurationRepository()
        // Seed the chip from the mock catalog only as a placeholder until the
        // engine's `ModelList` lands (SHIP-BLOCKER #2). The REAL active model is
        // `activeModelId`, set below from the (possibly empty) configured id and
        // then authoritatively replaced by `ModelList.current` / `ModelChanged`.
        self.model = ConversationModel(
            messages: [],
            model: MockData.models.first(where: { $0.id == config.model })
                ?? MockData.models[0])
        // Out-of-band model state: the configured id (empty ⇒ engine default,
        // filled by the first `ModelList`). Never a branded mock id here.
        self.model.activeModelId = config.model
        self.model.bypassPermissionsWarningSuppressed = self.permissionModeRepository
            .bypassWarningSuppressed()
    }

    // MARK: ConversationSource

    func startNewConversation() {
        // A paused/waiting durable turn is still owned by the engine even
        // though `model.streaming` is false. Do not clear its checkpoint or
        // replace the session until recovery reaches Running or a terminal
        // state.
        guard unresolvedRecoveryTurnID == nil,
              pauseAcknowledgements.isEmpty
        else { return }
        durableTurns.clear()
        let turnIdToCancel = inFlightTurnForSessionSwitch()
        model.sessionRestoreRecovery = nil
        model.sessionTransitionFailure = nil
        let transitionOperationID = beginSessionTransition(.new)
        prepareSessionTransition(cancelling: turnIdToCancel, isNew: true)
        // Tell the engine to begin a fresh session (no cwd/model override —
        // the engine keeps its configured defaults). The new id arrives back
        // out-of-band via `SessionStarted`.
        submitSessionTransition(
            cancelling: turnIdToCancel,
            submission: .command(.newSession(cwd: nil, model: nil)),
            failurePrefix: String(localized: "chat_new_session_failed"),
            transitionOperationID: transitionOperationID,
            isNew: true
        )
    }

    /// Capture the old turn before a session transition. Its correlator stays
    /// live until Cancel succeeds so a delivery failure can restore the same
    /// retryable Stop state instead of discarding the old transcript.
    private func inFlightTurnForSessionSwitch() -> UInt64? {
        if let cancellationOperation { return cancellationOperation.turnId }
        guard model.streaming else { return nil }
        return currentTurnId
    }

    private func prepareSessionTransition(cancelling turnId: UInt64?, isNew: Bool) {
        guard let turnId else {
            resetTranscriptForSessionSwitch(isNew: isNew)
            return
        }
        _ = operationForCancelling(turnId: turnId)
        model.isCancelling = true
        model.statusLine = String(localized: "chat_stopping")
    }

    private func submitSessionTransition(
        cancelling turnId: UInt64?,
        submission: SessionTransitionSubmission,
        failurePrefix: String,
        transitionOperationID: UInt64,
        resumeTargetID: String? = nil,
        allowsMissingSessionReplacement: Bool = false,
        isNew: Bool
    ) {
        Task { [weak self] in
            guard let self else { return }
            if let turnId {
                do {
                    try await self.awaitCancellation(turnId)
                } catch {
                    guard self.activeSessionTransitionOperationID == transitionOperationID else {
                        return
                    }
                    // `finishCancellation` has already restored the original
                    // visible turn and retryable Stop state. Abandon only the
                    // requested session transition; treating this as a turn-
                    // terminal host error would incorrectly clear ownership.
                    if let resumeTargetID {
                        self.model.sessionTransitionFailure = SessionTransitionFailure(
                            requestedSessionID: resumeTargetID
                        )
                    }
                    self.setPendingSessionTransition(nil)
                    if self.model.error == nil {
                        self.model.error = ConversationError(
                            kind: .host,
                            message: String(localized: "chat_cancel_old_session_failed \(error)")
                        )
                    }
                    return
                }
                guard self.activeSessionTransitionOperationID == transitionOperationID else {
                    return
                }
                self.resetTranscriptForSessionSwitch(isNew: isNew)
            }
            guard self.activeSessionTransitionOperationID == transitionOperationID else {
                return
            }
            do {
                try await self.submitSessionTransition(submission)
            } catch {
                guard self.activeSessionTransitionOperationID == transitionOperationID else {
                    return
                }
                if let resumeTargetID {
                    // A slower failed restore must not clobber a newer drawer
                    // selection that has already replaced the pending target.
                    guard self.pendingSessionTransition == .resume(resumeTargetID) else {
                        return
                    }
                    if allowsMissingSessionReplacement,
                       Self.isMissingSessionResumeError(error) {
                        await self.replaceUnavailableSession(resumeTargetID)
                        return
                    }
                    self.model.sessionTransitionFailure = SessionTransitionFailure(
                        requestedSessionID: resumeTargetID
                    )
                }
                self.setPendingSessionTransition(nil)
                self.fail(.host, "\(failurePrefix)：\(error)")
            }
        }
    }

    private func submitSessionTransition(
        _ submission: SessionTransitionSubmission
    ) async throws {
        switch submission {
        case let .command(command):
            try await submitCommand(command)
        case let .resumeEmpty(sessionID, title):
            if let testEmptySessionResumer {
                try await testEmptySessionResumer(sessionID, title)
                return
            }
            let handle = try await ensureHandle()
            try await handle.resumeEmptySession(sessionId: sessionID, title: title)
        }
    }

    /// A missing on-disk session is recoverable startup state, not an engine
    /// outage. Keep the transition pending while a replacement is created so
    /// an older SessionList cannot erase the cached drawer index in between.
    private func replaceUnavailableSession(_ unavailableSessionID: String) async {
        model.sessionRestoreRecovery = SessionRestoreRecovery(
            unavailableSessionID: unavailableSessionID
        )
        setPendingSessionTransition(.new)
        resetTranscriptForSessionSwitch(isNew: true)
        do {
            try await submitCommand(.newSession(cwd: nil, model: nil))
            model.statusLine = String(localized: "chat_session_replaced")
        } catch {
            setPendingSessionTransition(nil)
            fail(.host, String(localized: "chat_session_replaced_failed \(error)"))
        }
    }

    /// Branch on the generated protocol error first. Older hosts may still
    /// report a missing resume target as Rejected, whose message is matched
    /// narrowly; unrelated transport/protocol failures stay visible.
    private static func isMissingSessionResumeError(_ error: Error) -> Bool {
        guard let clientError = error as? ClientError else { return false }
        switch clientError {
        case .NotFound:
            return true
        case let .Rejected(message):
            let normalized = message.lowercased()
            guard normalized.contains("not resumable") else { return false }
            return normalized.contains("was not found")
                || normalized.contains("session not found")
                || normalized.contains("sessionnotfound")
        case .Transport, .Protocol, .Internal:
            return false
        @unknown default:
            return false
        }
    }

    private func setPendingSessionTransition(_ transition: PendingSessionTransition?) {
        pendingSessionTransition = transition
        model.sessionTransitionPending = transition != nil
        if transition == nil {
            activeSessionTransitionOperationID = nil
        }
    }

    private func beginSessionTransition(
        _ transition: PendingSessionTransition
    ) -> UInt64 {
        let operationID = nextSessionTransitionOperationID
        nextSessionTransitionOperationID &+= 1
        activeSessionTransitionOperationID = operationID
        setPendingSessionTransition(transition)
        return operationID
    }

    private func submitSessionCancellation(_ turnId: UInt64?, isNew: Bool) {
        guard let turnId else {
            resetTranscriptForSessionSwitch(isNew: isNew)
            return
        }
        prepareSessionTransition(cancelling: turnId, isNew: isNew)
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.awaitCancellation(turnId)
                self.resetTranscriptForSessionSwitch(isNew: isNew)
            } catch {
                if self.model.error == nil {
                    self.model.error = ConversationError(
                        kind: .host,
                        message: String(localized: "chat_cancel_old_session_failed \(error)")
                    )
                }
            }
        }
    }

    /// Shared transition reset used by new/resume/open-session after any old
    /// turn has safely released its engine slot. Incrementing the epoch also
    /// makes late transport delivery harmless. A new session clears the
    /// committed transcript immediately; a resume keeps it visible until the
    /// authoritative `SessionResumed` replay replaces it.
    private func resetTranscriptForSessionSwitch(isNew: Bool) {
        invalidateTurnContext()
        replayProjectionState = nil
        hasConfirmedSessionState = false
        model.clearAgentState()
        if isNew {
            model.messages = []
            model.items = []
            model.messageDetails = [:]
        }
        model.streaming = false
        model.isCancelling = false
        model.slashCommandPending = false
        model.compactionStatus = nil
        model.turnCompletion = nil
        model.activeTurnToken = nil
        model.isNew = isNew
        model.statusLine = nil
        model.error = nil
        model.notice = nil
        // Same for a pending questionnaire: its request id is scoped to
        // the abandoned session/connection. If the target session still
        // has one pending, the broker replays it after the switch.
        model.pendingQuestions = []
        // The plan and the expanded-row set belong to the transcript we're
        // dropping; the next `PlanUpdated` re-establishes a plan.
        model.planTasks = []
        model.expandedToolCalls = []
        model.backgroundTasks = []
        model.workflowResumeState = .idle
    }

    private func invalidateTurnContext() {
        durableRecoveryTask?.cancel()
        durableRecoveryTask = nil
        durableRecoveryPhase = nil
        terminalRecoveryStates.removeAll()
        unresolvedRecoveryTurnID = nil
        needsDurableForegroundRecoveryTurnID = nil
        reattachedTurnIDs.removeAll()
        model.hasInactiveDurableRecovery = false
        model.hasUnresolvedTurnRecovery = false
        let pendingPauses = pauseAcknowledgements
        pauseAcknowledgements.removeAll()
        for continuations in pendingPauses.values {
            for continuation in continuations {
                continuation.resume(throwing: PauseAcknowledgementError.invalidated)
            }
        }
        sessionEpoch &+= 1
        streamingIndex = nil
        streamingItemIndex = nil
        currentResponseMessageIDs = []
        currentTurnId = nil
        executorOwnedTurnID = nil
        activeTurnEpoch = nil
        model.activeTurnToken = nil
        activeRunItemIndex = nil
        pendingSlashRaw = nil
        model.slashCommandPending = false
    }

    private func clearTurnPointers(keepEpoch: Bool = true) {
        streamingIndex = nil
        streamingItemIndex = nil
        currentResponseMessageIDs = []
        currentTurnId = nil
        executorOwnedTurnID = nil
        activeTurnEpoch = keepEpoch ? activeTurnEpoch : nil
        if !keepEpoch {
            model.activeTurnToken = nil
        }
        activeRunItemIndex = nil
        pendingSlashRaw = nil
        model.slashCommandPending = false
    }

    private var activeConversationTurnToken: ConversationTurnToken? {
        guard let currentTurnId,
              let activeTurnEpoch,
              activeTurnEpoch == sessionEpoch
        else { return nil }
        return ConversationTurnToken(
            clientTurnId: currentTurnId,
            sessionEpoch: activeTurnEpoch
        )
    }

    private func finalAssistantTextForActiveTurn() -> String {
        let message: Message?
        if let streamingIndex,
           model.messages.indices.contains(streamingIndex) {
            message = model.messages[streamingIndex]
        } else if let activeRunItemIndex,
                  model.items.indices.contains(activeRunItemIndex),
                  case let .run(run) = model.items[activeRunItemIndex],
                  let messageID = run.activities.reversed().compactMap({ activity -> UUID? in
                      guard case let .textBoundary(_, messageID) = activity else { return nil }
                      return messageID
                  }).first {
            message = model.messages.first(where: { $0.id == messageID })
        } else {
            message = nil
        }
        guard let message else { return "" }
        guard case .ai = message.role else { return "" }
        if let detail = model.messageDetails[message.id] {
            return detail.blocks.compactMap { block -> String? in
                guard case let .text(text) = block else { return nil }
                return text
            }
            .joined(separator: "\n\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return message.text
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func publishActiveTurnCompletion(
        _ outcome: ConversationTurnCompletion.Outcome
    ) {
        guard let token = activeConversationTurnToken else { return }
        model.turnCompletion = ConversationTurnCompletion(
            token: token,
            outcome: outcome,
            finalAssistantText: outcome == .completed
                ? finalAssistantTextForActiveTurn()
                : ""
        )
    }

    private func appendMessage(_ message: Message, detail: ConversationMessageDetail? = nil) {
        model.messages.append(message)
        model.items.append(.message(message))
        if let detail {
            model.messageDetails[message.id] = detail
        } else {
            model.messageDetails.removeValue(forKey: message.id)
        }
    }

    private func removeMessage(id: UUID) {
        model.messages.removeAll { $0.id == id }
        model.items.removeAll {
            if case let .message(message) = $0 { return message.id == id }
            return false
        }
        model.messageDetails.removeValue(forKey: id)
    }

    /// The chip attached in the composer, consumed by exactly one send.
    private func takeVisualizationChip() -> VisualizationContextChip? {
        defer { model.visualizationChip = nil }
        return model.visualizationChip
    }

    /// Order a live visualization slot. A reference line ends the narration
    /// it interrupts: the next text delta opens a new row after the widget.
    private func applyVisualizationBlock(status: VisualizationBlockStatusDto, reference: VisualizationRefDto?) {
        streamingIndex = nil
        streamingItemIndex = nil
        let pending = currentResponseMessageIDs.last { id in
            model.indexOfMessage(id: id).map { model.messages[$0].visualization?.status == .pending } ?? false
        }
        let slot: MessageVisualization
        switch status {
        case .pending:
            if pending != nil { return }
            slot = MessageVisualization(status: .pending)
        case .ready:
            slot = MessageVisualization(status: .ready, id: reference?.id, revision: reference?.revision)
        case .unavailable:
            slot = MessageVisualization(status: .unavailable)
        case .discarded:
            if let pending {
                removeMessage(id: pending)
                currentResponseMessageIDs.removeAll { $0 == pending }
            }
            return
        @unknown default:
            return
        }
        var message = Message(id: pending ?? UUID(), role: .ai, text: "")
        message.visualization = slot
        if let pending {
            replaceMessage(id: pending, with: message, detail: nil)
        } else {
            appendMessage(message)
            currentResponseMessageIDs.append(message.id)
            appendTextBoundaryActivity(messageID: message.id)
        }
    }

    private func replaceStreamingMessage(_ message: Message, detail: ConversationMessageDetail? = nil) {
        guard let streamingIndex,
              model.messages.indices.contains(streamingIndex)
        else {
            appendMessage(message, detail: detail)
            self.streamingIndex = model.messages.count - 1
            self.streamingItemIndex = model.items.count - 1
            return
        }
        let oldMessage = model.messages[streamingIndex]
        // A streamed assistant reply is one logical list row. Keep its
        // identity stable while text/tag/detail are replaced; otherwise
        // SwiftUI treats every token as a row deletion + insertion and
        // re-lays out the transcript from scratch.
        let stableMessage = Message(
            id: oldMessage.id,
            role: message.role,
            tag: message.tag,
            text: message.text
        )
        model.withIndexRebuildSuppressed {
            model.messages[streamingIndex] = stableMessage
            if let itemIndex = streamingItemIndex,
               model.items.indices.contains(itemIndex) {
                model.items[itemIndex] = .message(stableMessage)
            } else if let itemIndex = model.indexOfMessageItem(id: oldMessage.id) {
                model.items[itemIndex] = .message(stableMessage)
                streamingItemIndex = itemIndex
            }
        }
        model.messageDetails.removeValue(forKey: oldMessage.id)
        if let detail {
            model.messageDetails[stableMessage.id] = detail
        }
    }

    private func replaceMessage(
        id: UUID,
        with message: Message,
        detail: ConversationMessageDetail?
    ) {
        guard let messageIndex = model.indexOfMessage(id: id) else {
            appendMessage(message, detail: detail)
            return
        }
        var stableMessage = Message(id: id, role: message.role, tag: message.tag, text: message.text)
        stableMessage.visualization = message.visualization
        stableMessage.visualizationContext = message.visualizationContext
        model.withIndexRebuildSuppressed {
            model.messages[messageIndex] = stableMessage
            if let itemIndex = model.indexOfMessageItem(id: id) {
                model.items[itemIndex] = .message(stableMessage)
            }
        }
        model.messageDetails.removeValue(forKey: id)
        if let detail {
            model.messageDetails[id] = detail
        }
    }

    private func ensureActiveRun() -> ConversationExecutionRun {
        if let itemIndex = activeRunItemIndex,
           model.items.indices.contains(itemIndex),
           case let .run(run) = model.items[itemIndex] {
            return run
        }
        let run = ConversationExecutionRun(
            id: "session-\(sessionEpoch)-turn-\(currentTurnId ?? 0)",
            sessionId: model.activeSessionId,
            turnId: currentTurnId,
            status: .running
        )
        model.items.append(.run(run))
        activeRunItemIndex = model.items.count - 1
        return run
    }

    @discardableResult
    private func updateActiveRun(_ mutate: (inout ConversationExecutionRun) -> Void) -> ConversationExecutionRun {
        var run = ensureActiveRun()
        mutate(&run)
        if let itemIndex = activeRunItemIndex,
           model.items.indices.contains(itemIndex) {
            model.withIndexRebuildSuppressed {
                model.items[itemIndex] = .run(run)
            }
        }
        return run
    }

    private func appendActivity(_ activity: ConversationExecutionActivity) {
        updateActiveRun { run in
            run.activities.append(activity)
        }
    }

    private func appendReasoningActivity(_ text: String) {
        guard !text.isEmpty else { return }
        updateActiveRun { run in
            if let index = run.activities.lastIndex(where: {
                if case .reasoning = $0 { return true }
                return false
            }), index == run.activities.index(before: run.activities.endIndex),
               case let .reasoning(id, current) = run.activities[index],
               currentResponseReasoningIDs.contains(id) {
                run.activities[index] = .reasoning(id: id, text: current + text)
                currentResponseReasoningIDs.insert(id)
            } else {
                let id = "reasoning-\(UUID().uuidString)"
                run.activities.append(.reasoning(id: id, text: text))
                currentResponseReasoningIDs.insert(id)
            }
            run.reasoning += text
        }
    }

    private func appendTextBoundaryActivity(messageID: UUID) {
        updateActiveRun { run in
            guard !run.activities.contains(where: { activity in
                guard case let .textBoundary(_, existingMessageID) = activity else { return false }
                return existingMessageID == messageID
            }) else { return }
            run.activities.append(.textBoundary(
                id: "boundary-\(UUID().uuidString)",
                messageID: messageID
            ))
        }
    }

    private func appendToolActivity(id: String) {
        updateActiveRun { run in
            guard !run.activities.contains(where: {
                if case let .tool(existing) = $0 { return existing == id }
                return false
            }) else { return }
            run.activities.append(.tool(id: id))
        }
    }

    private func appendNoticeActivity(id: String) {
        appendActivity(.notice(id: id))
    }

    private func upsertTool(
        id: String,
        tool: String,
        fallbackSummary: String?,
        mutate: (inout ConversationToolTrace) -> Void
    ) {
        updateActiveRun { run in
            if let index = run.tools.firstIndex(where: { $0.id == id }) {
                var existing = run.tools[index]
                mutate(&existing)
                run.tools[index] = existing
            } else {
                var trace = ConversationToolTrace(
                    id: id,
                    tool: tool,
                    status: .running,
                    inputSummary: fallbackSummary,
                    outputSummary: nil,
                    elapsedMs: nil
                )
                mutate(&trace)
                run.tools.append(trace)
            }
        }
    }

    private func upsertShellCard(
        id: String,
        create: () -> ConversationShellCard,
        mutate: (inout ConversationShellCard) -> Void
    ) {
        updateActiveRun { run in
            if let index = run.shellCards.firstIndex(where: { $0.taskId == id }) {
                var existing = run.shellCards[index]
                mutate(&existing)
                run.shellCards[index] = existing
            } else {
                var card = create()
                mutate(&card)
                run.shellCards.append(card)
            }
        }
    }

    /// Close transient tool and shell rows owned by the active turn. A
    /// coordinator worker may legitimately outlive that turn, so its count
    /// remains live until the engine publishes a later idle status.
    private func finishActiveRun(_ status: ConversationExecutionStatus) {
        let toolStatus: ConversationToolStatus
        let shellStatus: ConversationShellStatus
        switch status {
        case .running:
            toolStatus = .running
            shellStatus = .running
        case .completed, .maxTurns, .restored:
            toolStatus = .completed
            shellStatus = .completed
        case .failed:
            toolStatus = .failed
            shellStatus = .failed
        case .cancelled:
            toolStatus = .cancelled
            shellStatus = .cancelled
        }
        updateActiveRun { run in
            run.status = status
            for index in run.tools.indices where run.tools[index].status == .running {
                run.tools[index].status = toolStatus
            }
            for index in run.shellCards.indices where run.shellCards[index].status == .running {
                run.shellCards[index].status = shellStatus
            }
        }
        // Keep the run at its original wire position. The timeline
        // projection removes the run card from durable message rows while
        // preserving the user → activity → assistant ordering.
    }

    /// Settle only the main run correlated with the durable recovery. A
    /// replay can contain tool/run envelopes from another workflow task;
    /// those rows and their background lease must remain untouched. The
    /// guard is also important because `finishActiveRun` would otherwise
    /// create a synthetic run while a recovery snapshot has no main run.
    private func settleCorrelatedMainRun() {
        guard let itemIndex = activeRunItemIndex,
              model.items.indices.contains(itemIndex),
              case var .run(run) = model.items[itemIndex]
        else { return }
        run.status = .restored
        for index in run.tools.indices where run.tools[index].status == .running {
            run.tools[index].status = .completed
        }
        for index in run.shellCards.indices where run.shellCards[index].status == .running {
            run.shellCards[index].status = .completed
        }
        model.withIndexRebuildSuppressed {
            model.items[itemIndex] = .run(run)
        }
    }

    private func updateCoordinatorStatus(activeWorkers: UInt32, team: String?) {
        if activeWorkers == 0 {
            // CoordinatorStatus is a global snapshot, not a turn-scoped
            // delta. A newer user turn may already own the active pointer,
            // so clear every older run that still carries a worker count.
            for itemIndex in model.items.indices {
                guard case var .run(run) = model.items[itemIndex],
                      run.activeWorkers > 0
                else { continue }
                run.activeWorkers = 0
                if let team { run.coordinatorTeam = team }
                model.withIndexRebuildSuppressed {
                    model.items[itemIndex] = .run(run)
                }
            }
            return
        }
        if let itemIndex = activeRunItemIndex,
           model.items.indices.contains(itemIndex),
           case var .run(run) = model.items[itemIndex] {
            run.activeWorkers = activeWorkers
            run.coordinatorTeam = team
            model.withIndexRebuildSuppressed {
                model.items[itemIndex] = .run(run)
            }
            return
        }

        // `TurnEnded` clears the active-turn pointers, but coordinator
        // workers are asynchronous and can report their final idle state
        // afterward. Update the latest run that still owns workers instead
        // of creating a detached synthetic run.
        guard let itemIndex = model.items.lastIndex(where: { item in
            guard case let .run(run) = item else { return false }
            return run.activeWorkers > 0
        }), case var .run(run) = model.items[itemIndex]
        else { return }
        run.activeWorkers = activeWorkers
        run.coordinatorTeam = team
        model.withIndexRebuildSuppressed {
            model.items[itemIndex] = .run(run)
        }
    }

    private func acceptTurnEvent(_ event: ClientEvent) -> Bool {
        switch event {
        case .askUserQuestion, .askUserQuestionResolved, .permissionRequestResolved,
             .taskStatusChanged, .taskRow, .workflowResumed, .planUpdated, .coordinatorStatus,
             .compactionStatus, .compactionCompleted:
            // Deliberately OUTSIDE the turn gate: the engine's broker
            // replays a still-pending AskUserQuestion (and resolves it)
            // after a foreground re-connect, a background task's status
            // change lands after its turn already ended, `TaskRow`
            // rows answer an out-of-band `TaskList`, and a `PlanUpdated`
            // is the authoritative full-list replace of a block that
            // outlives its turn — all of these arrive with no turn in
            // flight and must not be dropped as out-of-turn strays.
            return true
        default:
            break
        }
        guard let currentTurnId, let activeTurnEpoch, activeTurnEpoch == sessionEpoch else {
            return false
        }
        switch event {
        case let .turnStarted(turnId):
            return turnId == nil || turnId == currentTurnId
        default:
            return true
        }
    }

    private func submitCommand(_ command: ClientCommand) async throws {
        if let testCommandSubmitter {
            try await testCommandSubmitter(command)
            return
        }
        let handle = try await ensureHandle()
        do {
            try await handle.submit(command: command)
        } catch {
            let errorDescription = String(describing: error)
            Self.turnLog.error(
                "submit command failed command=\(String(describing: command), privacy: .public) error=\(errorDescription, privacy: .public)"
            )
            print("[LingxiCode] submitCommand failed command=\(command) error=\(errorDescription)")
            throw error
        }
    }

    private func confirmBypassPermissionsForCurrentSession() async throws {
        if let testBypassPermissionsConfirmer {
            try await testBypassPermissionsConfirmer()
            return
        }
        let handle = try await ensureHandle()
        try await handle.confirmBypassPermissions()
    }

    private func operationForCancelling(turnId: UInt64) -> CancellationOperation {
        if let operation = cancellationOperation, operation.turnId == turnId {
            return operation
        }
        let operationID = nextCancellationOperationID
        nextCancellationOperationID &+= 1
        let epoch = activeTurnEpoch ?? sessionEpoch
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            try await self.submitCommand(.cancel(turnId: turnId))
        }
        let operation = CancellationOperation(
            id: operationID,
            turnId: turnId,
            epoch: epoch,
            task: task
        )
        cancellationOperation = operation
        return operation
    }

    private func finishCancellation(
        _ operation: CancellationOperation,
        error: Error?
    ) {
        guard cancellationOperation?.id == operation.id else { return }
        cancellationOperation = nil
        terminalRecoveryStates.removeValue(forKey: operation.turnId)

        let stillOwnsVisibleTurn = currentTurnId == operation.turnId
            && activeTurnEpoch == operation.epoch
            && operation.epoch == sessionEpoch
        model.isCancelling = false

        if let error {
            if stillOwnsVisibleTurn {
                // Delivery failed before the engine confirmed release. Keep
                // the original turn live so Stop can be retried safely.
                if !model.hasInactiveDurableRecovery {
                    model.streaming = true
                }
                model.statusLine = nil
                model.error = ConversationError(kind: .host, message: String(localized: "chat_cancel_failed \(error)"))
            } else if model.statusLine == String(localized: "chat_stopping") {
                model.statusLine = nil
            }
            Self.turnLog.error(
                "cancel failed turn=\(operation.turnId, privacy: .public) error=\(String(describing: error), privacy: .private(mask: .hash))"
            )
            return
        }

        if stillOwnsVisibleTurn {
            durableTurns.clear(turnID: operation.turnId)
            if unresolvedRecoveryTurnID == operation.turnId {
                unresolvedRecoveryTurnID = nil
                needsDurableForegroundRecoveryTurnID = nil
                reattachedTurnIDs.remove(operation.turnId)
                model.hasInactiveDurableRecovery = false
            }
            // The host only returns after all event producers have joined and
            // the single-turn slot is released. This is also a fallback for a
            // transport that failed to deliver its terminal event.
            model.streaming = false
            model.statusLine = nil
            model.notice = .cancelled
            finishActiveRun(.cancelled)
            publishActiveTurnCompletion(.cancelled)
            clearTurnPointers(keepEpoch: false)
        } else if model.statusLine == String(localized: "chat_stopping") {
            model.statusLine = nil
        }
        requestSessionCatalogRefreshAfterSettledTurn()
        Self.turnLog.debug(
            "cancel returned turn=\(operation.turnId, privacy: .public)"
        )
    }

    private func awaitCancellation(_ turnId: UInt64) async throws {
        let operation = operationForCancelling(turnId: turnId)
        do {
            try await operation.task.value
            // Command Ok is only submission acknowledgement. First drain
            // the listener FIFO, then wait for the correlated terminal
            // TurnRecoveryState before releasing any visible ownership.
            await listener?.waitUntilIdle()
            try await waitForCancellationAcknowledgement(turnID: operation.turnId)
            finishCancellation(operation, error: nil)
        } catch {
            finishCancellation(operation, error: error)
            throw error
        }
    }

    private func startPrompt(_ prompt: TurnPrompt) -> ConversationTurnToken {
        model.notice = nil
        model.streaming = true
        model.hasInactiveDurableRecovery = false
        model.hasUnresolvedTurnRecovery = false
        model.isCancelling = false
        model.slashCommandPending = false
        model.compactionStatus = nil
        model.turnCompletion = nil
        turnSpeechSequence = 0
        model.statusLine = nil
        streamingIndex = nil
        streamingItemIndex = nil
        currentResponseMessageIDs = []
        replayProjectionState = nil
        currentTurnId = prompt.turnId
        executorOwnedTurnID = prompt.turnId
        activeTurnEpoch = sessionEpoch
        let token = ConversationTurnToken(
            clientTurnId: prompt.turnId,
            sessionEpoch: sessionEpoch
        )
        model.activeTurnToken = token
        durableTurns.begin(sessionID: model.activeSessionId, turnID: prompt.turnId)
        Self.turnLog.debug(
            "prompt submit turn=\(prompt.turnId, privacy: .public) epoch=\(self.sessionEpoch, privacy: .public)"
        )

        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.submitCommand(.sendPrompt(
                    text: prompt.text,
                    promptMode: nil,
                    images: prompt.images,
                    turnId: prompt.turnId,
                    visualizationContext: prompt.visualizationContext.map {
                        VisualizationRefDto(id: $0.id, revision: $0.revision)
                    }))
            } catch {
                guard self.activeConversationTurnToken == token else { return }
                self.fail(.host, "\(error)")
            }
        }
        return token
    }

    private static func isCompactSlash(_ raw: String) -> Bool {
        raw.split(whereSeparator: { $0.isWhitespace }).first?
            .lowercased() == "/compact"
    }

    private func startSlashCommand(raw: String, turnId: UInt64) -> ConversationTurnToken {
        model.notice = nil
        model.streaming = false
        model.isCancelling = false
        model.slashCommandPending = true
        model.compactionStatus = Self.isCompactSlash(raw)
            ? .queued
            : nil
        model.turnCompletion = nil
        turnSpeechSequence = 0
        model.statusLine = nil
        streamingIndex = nil
        streamingItemIndex = nil
        currentResponseMessageIDs = []
        replayProjectionState = nil
        currentTurnId = turnId
        activeTurnEpoch = sessionEpoch
        pendingSlashRaw = raw
        let token = ConversationTurnToken(clientTurnId: turnId, sessionEpoch: sessionEpoch)
        model.activeTurnToken = token

        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.submitCommand(.runSlashCommand(raw: raw, turnId: turnId))
            } catch {
                guard self.activeConversationTurnToken == token else { return }
                self.fail(.host, "\(error)")
            }
        }
        return token
    }

    private func requestSessionCatalogRefreshAfterSettledTurn() {
        guard
            !model.streaming,
            !model.isCancelling,
            !model.isNew,
            !model.activeSessionId.isEmpty,
            !model.sessionTransitionPending
        else { return }
        model.sessionRefreshRevision &+= 1
    }

    @discardableResult
    func send(_ text: String) -> ConversationTurnToken? {
        send(text, images: [])
    }

    @discardableResult
    func send(_ text: String, images: [ImageRefDto]) -> ConversationTurnToken? {
        guard
            !model.isCancelling,
            !model.slashCommandPending,
            !model.sessionTransitionPending,
            pauseAcknowledgements.isEmpty,
            unresolvedRecoveryTurnID == nil,
            !model.isSelectedAgentReadOnly
        else { return nil }

        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        // The engine command catalog is authoritative. Until it arrives we
        // cannot distinguish a valid local slash command from prompt text,
        // so keep the draft untouched instead of accidentally sending it
        // to the model through non-Composer call sites.
        guard model.slashCommandsLoaded || !trimmed.hasPrefix("/") else { return nil }

        let exactSlash = model.slashCommandsLoaded
            && SlashCommandMatcher.exactCommand(
                in: trimmed,
                catalog: model.slashCommands
            ) != nil
        if model.streaming {
            guard !exactSlash, let token = activeConversationTurnToken else { return nil }
            model.isNew = false
            model.notice = nil
            let chip = takeVisualizationChip()
            var queued = Message(role: .user, text: text, images: uiImages(from: images))
            queued.visualizationContext = chip
            appendMessage(queued)
            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.sendPrompt(
                        text: text,
                        promptMode: nil,
                        images: images,
                        turnId: nil,
                        visualizationContext: chip.map { VisualizationRefDto(id: $0.id, revision: $0.revision) }))
                } catch {
                    guard self.activeConversationTurnToken == token else { return }
                    self.model.error = ConversationError(kind: .host, message: "\(error)")
                }
            }
            return token
        }

        model.isNew = false
        model.notice = nil
        let chip = exactSlash ? nil : takeVisualizationChip()
        var prompt = Message(role: .user, text: text, images: uiImages(from: images))
        prompt.visualizationContext = chip
        appendMessage(prompt)

        let turnId = nextTurnId
        nextTurnId = nextTurnId == UInt64.max ? 1 : nextTurnId + 1
        if exactSlash {
            return startSlashCommand(raw: trimmed, turnId: turnId)
        }
        return startPrompt(TurnPrompt(text: text, images: images, turnId: turnId, visualizationContext: chip))
    }

    func cancelAndWait() async throws {
        let operation: CancellationOperation
        if let currentOperation = cancellationOperation {
            operation = currentOperation
        } else {
            // A WaitingForUser recovery is inactive in the UI (`streaming`
            // is false), but its durable checkpoint still owns the host's
            // single-turn slot. Stop/Discard must be able to terminalize
            // that exact correlated turn; unrelated idle sources remain a
            // no-op.
            guard let turnId = currentTurnId,
                  model.streaming || unresolvedRecoveryTurnID == turnId
            else { return }
            operation = operationForCancelling(turnId: turnId)
        }
        Self.turnLog.debug(
            "cancel requested turn=\(operation.turnId, privacy: .public) epoch=\(self.sessionEpoch, privacy: .public)"
        )
        model.isCancelling = true
        model.statusLine = String(localized: "chat_stopping")
        do {
            try await operation.task.value
            // Do not treat a successful Cancel command as terminal. The
            // host may return Ok for a stale/missing turn; only the
            // listener's matching terminal recovery snapshot proves that
            // this source may clear its durable checkpoint and gates.
            await listener?.waitUntilIdle()
            try await waitForCancellationAcknowledgement(turnID: operation.turnId)
            finishCancellation(operation, error: nil)
        } catch {
            finishCancellation(operation, error: error)
            throw error
        }
    }

    func cancel() {
        Task { [weak self] in
            guard let self else { return }
            try? await self.cancelAndWait()
        }
    }

    func dismissError() { model.error = nil }

    func submitEngineCommand(_ command: ClientCommand) async throws {
        try await submitCommand(command)
    }

    func markActiveTurnPausedRecoverable(_ token: ConversationTurnToken?) async throws {
        guard let token, activeConversationTurnToken == token else { return }
        guard model.streaming || pauseAcknowledgements[token] != nil else { return }

        let shouldSubmit = pauseAcknowledgements[token] == nil
        try await withCheckedThrowingContinuation { continuation in
            pauseAcknowledgements[token, default: []].append(continuation)
            guard shouldSubmit else { return }
            armPauseAcknowledgementDeadline(token: token)
            Task { [weak self] in
                guard let self else { return }
                do {
                    try await self.submitCommand(.pauseTurn(
                        turnId: token.clientTurnId,
                        reason: "background_time_expired"
                    ))
                } catch {
                    if self.activeConversationTurnToken == token {
                        // A failed pause leaves ownership unresolved. Keep
                        // the turn non-sendable and expose the host failure
                        // rather than converting an unknown state into
                        // Cancelled.
                        self.model.error = ConversationError(
                            kind: .host,
                            message: String(describing: error)
                        )
                    }
                    self.resolvePauseAcknowledgements(
                        token: token,
                        result: .failure(error)
                    )
                }
            }
        }
    }

    func resumeWorkflow(_ taskID: String) {
        guard !taskID.isEmpty, !model.sessionTransitionPending,
              model.backgroundTasks.contains(where: { $0.id == taskID && $0.canResumeWorkflow })
        else { return }
        if case let .resuming(activeTaskID) = model.workflowResumeState,
           activeTaskID == taskID { return }
        let originSessionID = model.activeSessionId
        model.workflowResumeState = .resuming(taskID: taskID)
        Task { [weak self] in
            guard let self else { return }
            guard !self.model.sessionTransitionPending,
                  self.model.activeSessionId == originSessionID,
                  self.model.backgroundTasks.contains(where: { $0.id == taskID && $0.canResumeWorkflow })
            else {
                if self.model.workflowResumeState == .resuming(taskID: taskID) {
                    self.model.workflowResumeState = .idle
                }
                return
            }
            do {
                try await self.submitCommand(.resumeWorkflow(taskId: taskID))
            } catch {
                self.model.workflowResumeState = .failed(
                    taskID: taskID,
                    message: String(describing: error)
                )
            }
        }
    }

    func handleForeground() {
        refreshBackgroundTasks()
        listSessionAgents()
        // A warm source keeps the durable checkpoint and listener alive,
        // but the engine may have parked the turn when the background lease
        // expired. Reattach from the latest in-memory client cursor before
        // asking the engine to resume it. Cold sources enter this same path
        // from SessionResumed below.
        guard let recoveryTurnID = needsDurableForegroundRecoveryTurnID,
              unresolvedRecoveryTurnID == recoveryTurnID,
              let record = durableTurns.load(),
              record.turnID == recoveryTurnID,
              record.sessionID == model.activeSessionId
        else { return }
        reattachDurableTurnIfNeeded(sessionID: record.sessionID)
    }

    func testProviderConnection(
        profile: ProviderLaunchProfile,
        credentialOverride: String?
    ) async throws -> ProviderConnectionTestResult {
        let handle = try await ensureHandle()
        let trimmed = credentialOverride?.trimmingCharacters(in: .whitespacesAndNewlines)
        let result = await handle.testProviderConnection(
            providerId: profile.id,
            providerPreset: profile.presetID,
            apiBase: profile.baseURL,
            model: profile.modelID,
            credentialOverride: trimmed.flatMap { value in
                value.isEmpty ? nil : ProviderCredentialSecretDto(value: value)
            }
        )
        return result.connected
            ? .success(
                message: "\(result.message) · \(result.latencyMs)ms",
                usedStoredCredential: result.usedStoredCredential
            )
            : .failure(message: result.message)
    }

    func providerCatalog() async throws -> [ProviderCatalogEntry] {
        if providerCatalogLoaded {
            return providerCatalogEntries
        }
        _ = try await ensureHandle()
        return try await withCheckedThrowingContinuation { continuation in
            providerCatalogWaiters.append(continuation)
            Task { @MainActor in
                do {
                    try await self.submitCommand(.listModels)
                } catch {
                    let waiters = self.providerCatalogWaiters
                    self.providerCatalogWaiters.removeAll()
                    for waiter in waiters {
                        waiter.resume(throwing: error)
                    }
                }
            }
        }
    }

    func loginOAuth(provider: String) async throws -> ProviderOAuthState {
        let handle = try await ensureHandle()
        let session = try await handle.beginOAuth(
            provider: provider,
            redirectUri: "lingxi://oauth/callback"
        )
        guard let authorizationURL = URL(string: session.authorizationUrl) else {
            throw NSError(domain: "ConversationSource", code: 2, userInfo: [
                NSLocalizedDescriptionKey: "OAuth 授权地址无效",
            ])
        }

        return try await withCheckedThrowingContinuation { continuation in
            oauthFlowID = session.flowId
            let flowID = session.flowId
            let callbackScheme = session.callbackUrlScheme
            let presentationProvider = OAuthPresentationContextProvider()
            oauthPresentationProvider = presentationProvider
            oauthCallbackHandler = { [weak self] url in
                guard let self else { return }
                guard self.oauthFlowID == flowID else { return }
                self.oauthCallbackHandler = nil
                self.oauthFlowID = nil
                Task { @MainActor in
                    do {
                        let state = try await handle.completeOAuth(
                            flowId: flowID,
                            callbackUrl: url.absoluteString
                        )
                        self.oauthSession = nil
                        self.oauthPresentationProvider = nil
                        continuation.resume(returning: Self.lowerOAuthState(state))
                    } catch {
                        self.oauthSession = nil
                        self.oauthPresentationProvider = nil
                        // The callback has become terminal even when the
                        // token exchange or secure-store write fails. Rust
                        // normally consumes the flow before exchange, but
                        // cancel is an idempotent defence for malformed
                        // callbacks and older engine binaries.
                        await handle.cancelOAuth(flowId: flowID)
                        continuation.resume(throwing: error)
                    }
                }
            }

            let webSession = ASWebAuthenticationSession(
                url: authorizationURL,
                callbackURLScheme: callbackScheme
            ) { [weak self] url, error in
                guard let self else { return }
                guard self.oauthFlowID == flowID else { return }
                if let error {
                    self.oauthCallbackHandler = nil
                    self.oauthSession = nil
                    self.oauthPresentationProvider = nil
                    self.oauthFlowID = nil
                    Task { await handle.cancelOAuth(flowId: flowID) }
                    continuation.resume(throwing: error)
                    return
                }
                if let url {
                    self.handleOAuthCallback(url)
                }
            }
            webSession.presentationContextProvider = presentationProvider
            webSession.prefersEphemeralWebBrowserSession = false
            oauthSession = webSession
            if !webSession.start() {
                oauthCallbackHandler = nil
                oauthSession = nil
                oauthPresentationProvider = nil
                oauthFlowID = nil
                Task { await handle.cancelOAuth(flowId: flowID) }
                continuation.resume(throwing: NSError(
                    domain: "ConversationSource",
                    code: 3,
                    userInfo: [NSLocalizedDescriptionKey: "无法启动系统浏览器 OAuth 会话"]
                ))
            }
        }
    }

    func logoutOAuth(provider: String) async throws {
        let handle = try await ensureHandle()
        try await handle.logoutOAuth(provider: provider)
    }

    func authState(provider: String) async throws -> ProviderOAuthState {
        let handle = try await ensureHandle()
        return Self.lowerOAuthState(try await handle.authState(provider: provider))
    }

    func testOAuthConnection(
        provider: String,
        profile: ProviderLaunchProfile
    ) async throws -> ProviderConnectionTestResult {
        let handle = try await ensureHandle()
        let result = await handle.testOAuthConnection(
            provider: provider,
            apiBase: profile.baseURL,
            model: profile.modelID
        )
        return result.connected
            ? .success(
                message: "\(result.message) · \(result.latencyMs)ms",
                usedStoredCredential: result.usedStoredCredential
            )
            : .failure(message: result.message)
    }

    func handleOAuthCallback(_ url: URL) {
        oauthCallbackHandler?(url)
    }

    private static func lowerOAuthState(_ state: MobileOAuthStateDto) -> ProviderOAuthState {
        ProviderOAuthState(
            provider: state.provider,
            signedIn: state.signedIn,
            accountLabel: state.accountLabel,
            accountID: state.accountId,
            organizationID: state.organizationId,
            fedramp: state.fedramp
        )
    }

    func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?) {
        externalEventHandler = handler
    }

    func setSettingsActive(_ active: Bool) {
        if active {
            guard Self.settingsOwner !== self else { return }
            Self.settingsOwner?.settingsGeneration &+= 1
            Self.settingsOwner = self
            settingsGeneration &+= 1
            let generation = settingsGeneration
            DesktopSettingsRepository.shared.configure(submitter: nil)
            Task { [weak self] in
                guard let self else { return }
                do {
                    let handle = try await self.ensureHandle()
                    guard Self.settingsOwner === self, generation == self.settingsGeneration else { return }
                    DesktopSettingsRepository.shared.configure(submitter: { command in
                        try await handle.submit(command: command)
                    })
                } catch {
                    guard Self.settingsOwner === self, generation == self.settingsGeneration else { return }
                    DesktopSettingsRepository.shared.configure(submitter: nil)
                }
            }
        } else if Self.settingsOwner === self {
            settingsGeneration &+= 1
            Self.settingsOwner = nil
            DesktopSettingsRepository.shared.configure(submitter: nil)
        }
    }

    // MARK: handle construction

    /// Build the engine handle once (lazily). Registers the listener so the
    /// adapter can stream events the moment the first turn runs.
    private func ensureHandle() async throws -> MobileEngineHandle {
        if let handle { return handle }
        if let handleBuildTask {
            return try await handleBuildTask.value
        }

        let listener = EngineListener(source: self)
        // SHIP-BLOCKER #3: register a real permission sink so a tool that needs
        // approval surfaces a prompt instead of hanging the turn forever.
        let permissionSink = EnginePermissionSink(listener: listener)
        let providerConfig = config.providerProfilesJson.map {
            IosProviderConfigFfi(
                providerProfilesJson: $0,
                routingJson: config.providerRoutingJson
            )
        }
        let launchConfig = IosEngineLaunchConfigFfi(
            apiBase: config.apiBase,
            apiKey: config.apiKey,
            model: config.model,
            sessionMode: config.sessionMode.dto,
            visionDelegationEnabled: config.visionDelegationEnabled,
            appSandboxRoot: config.appSandboxRoot,
            projectCwd: config.projectCwd,
            providerConfig: providerConfig,
            mobileLinux: config.mobileLinux.map {
                makeIosMobileLinuxConfig($0, appSandboxRoot: config.appSandboxRoot)
            },
            physicalMemoryBytes: ProcessInfo.processInfo.physicalMemory,
            hostEnvironment: makeIosHostEnvironment(launchMode: .interactive)
        )
        let handleBuilder = self.handleBuilder
        handleBuildAttemptID &+= 1
        let attemptID = handleBuildAttemptID
        // `buildIosEngineWithConfig` is synchronous and can enter the iSH
        // bridge, verify the bundled archive, and extract the rootfs on a
        // first launch. A child `Task` would inherit MainActor here and make
        // the entire setup UI unresponsive until that work completed.
        let audio = IOSAudioServiceCallbackAdapter.shared
        let buildTask = Task.detached(priority: .userInitiated) {
            [handleBuilder, launchConfig, listener, audio, permissionSink] in
            let handle = try handleBuilder(launchConfig, listener, audio, permissionSink)
            // Bootstrap listings are part of construction: never publish a
            // handle that failed halfway through initialization.
            try await handle.submit(command: .listModels)
            try await handle.submit(command: .getConversationControls)
            try await handle.submit(
                command: .listSessions(limit: EngineConversationSource.completeSessionListLimit)
            )
            // The composer has no static fallback: populate its command
            // palette from the same dynamic registry used for execution.
            try await handle.submit(command: .refreshListings(which: [.slashCommands]))
            // Agent roster is session-scoped and follows the same
            // out-of-band bootstrap path as models/sessions so the picker
            // is populated before the first turn.
            try? await handle.submit(command: .listSessionAgents)
            // Seed the pinned tasks panel: a re-opened scope may already
            // have a Workflow build running; its rows arrive as `TaskRow`
            // events (zero rows ⇒ zero events). Best-effort — unlike the
            // model/session listings above, the panel is not worth
            // failing the whole handle over (it self-heals on pushes).
            try? await handle.submit(command: .taskList(statusFilter: nil, requestId: nil))
            return handle
        }
        handleBuildTask = buildTask

        do {
            let builtHandle = try await buildTask.value
            if handleBuildAttemptID == attemptID {
                handle = builtHandle
                VisualizationWebHost.shared.attach(builtHandle.visualizationHost(origin: VisualizationWebHost.origin))
                self.listener = listener
                self.permissionSink = permissionSink
                handleBuildTask = nil
            }
            return handle ?? builtHandle
        } catch {
            if handleBuildAttemptID == attemptID {
                handleBuildTask = nil
            }
            throw error
        }
    }

    private nonisolated static func buildDefaultHandle(
        config: IosEngineLaunchConfigFfi,
        listener: IosEventListener,
        audio: IOSAudioServiceCallbackAdapter,
        permissions: IosPermissionSink
    ) throws -> MobileEngineHandle {
        try buildIosEngineWithConfig(
            config: config,
            listener: listener,
            audio: audio,
            camera: CameraImpl(),
            share: ShareImpl(),
            notifications: NotificationImpl(),
            clipboard: ClipboardImpl(),
            permissions: permissions,
            // Native Keychain secure store enables OAuth token persistence.
            secureStorage: SecureStorageImpl(),
            // One-shot device location. The cron
            // bridge passes nothing here (the FFI defaults it to nil):
            // a background wake has no user present to answer an
            // authorization sheet.
            deviceControl: DeviceControlImpl(),
            location: LocationImpl()
        )
    }

    /// Build the handle eagerly (independent of the first turn) so the model
    /// catalog populates as soon as the source is shown — the picker shouldn't
    /// have to wait for a sent message to learn the real ids. A build failure is
    /// surfaced as a host error banner; a later `send` will retry via the same
    /// `ensureHandle`.
    func warmUp() {
        guard handle == nil else { return }
        Task { [weak self] in
            guard let self else { return }
            do { _ = try await self.ensureHandle() }
            catch { self.fail(.host, "\(error)") }
        }
    }

    func prepare() async throws {
        _ = try await ensureHandle()
    }

    /// Refresh the real session catalog (drawer-open). Builds the handle if
    /// needed — `ensureHandle` already submits `listSessions` on first build,
    /// so the extra submit on a fresh build is a harmless idempotent refresh;
    /// on an existing handle it re-pulls so the drawer reflects sessions
    /// created since the handle was built. A build/submit failure surfaces as
    /// a host error banner.
    func listSessions() {
        Task { [weak self] in
            guard let self else { return }
            do {
                let handle = try await self.ensureHandle()
                try await handle.submit(
                    command: .listSessions(limit: Self.completeSessionListLimit)
                )
            } catch {
                self.fail(.host, "\(error)")
            }
        }
    }

    /// Pull the background-task rows (out-of-band, like `listSessions`).
    /// Each row lands on `apply` as a `TaskRow` event → the pinned panel.
    /// Uses the live handle in production, while allowing the injected
    /// command submitter to keep this path covered by hermetic tests.
    /// Neither route triggers an engine build.
    func refreshBackgroundTasks() {
        guard handle != nil || testCommandSubmitter != nil else { return }
        Task { [weak self] in
            guard let self else { return }
            // Best-effort: the panel self-heals on the next status push.
            try? await self.submitCommand(.taskList(statusFilter: nil, requestId: nil))
        }
    }

    /// Upsert one background-task row. `description: nil` keeps whatever
    /// text is already known (a status push carries no description).
    private func upsertBackgroundTask(
        id: String,
        description: String?,
        status: BackgroundTaskSnapshot.Status,
        canResume: Bool? = nil,
        startedAtMs: UInt64? = nil,
        errorText: String? = nil,
        taskType: String? = nil
    ) {
        let reason = errorText.flatMap { $0.isEmpty ? nil : $0 }
        if let index = model.backgroundTasks.firstIndex(where: { $0.id == id }) {
            if let taskType { model.backgroundTasks[index].taskType = taskType }
            let previousStatus = model.backgroundTasks[index].status
            if !(model.backgroundTasks[index].status.isTerminal && !status.isTerminal) {
                model.backgroundTasks[index].status = status
            }
            if let description, !description.isEmpty {
                model.backgroundTasks[index].descriptionText = description
            }
            // A capability belongs to its snapshot's status. A paused
            // workflow's old capability cannot authorize a later failure;
            // only the Host can grant Create recovery on a failed row.
            if previousStatus != model.backgroundTasks[index].status {
                model.backgroundTasks[index].canResume = false
            }
            if let canResume, status == model.backgroundTasks[index].status {
                model.backgroundTasks[index].canResume = canResume
            }
            model.backgroundTasks[index].startedAtMs = startedAtMs ?? model.backgroundTasks[index].startedAtMs
            // Never clear a reason already learned from the other source
            // (`TaskRow` backfill vs. the `TaskStatusChanged` push) — only
            // ever fill it in.
            if let reason {
                model.backgroundTasks[index].errorText = reason
            }
        } else {
            model.backgroundTasks.append(BackgroundTaskSnapshot(
                id: id,
                descriptionText: description ?? "",
                status: status,
                canResume: canResume ?? false,
                startedAtMs: startedAtMs,
                errorText: reason,
                workflow: nil,
                taskType: taskType
            ))
        }
    }

    private func upsertWorkflowProgress(
        taskId: String,
        runId: String,
        progress: ConversationWorkflowProgressPayload
    ) {
        let known = model.backgroundTasks.contains { $0.id == taskId }
        let nowMs = Self.currentWallClockMs()

        if let index = model.backgroundTasks.firstIndex(where: { $0.id == taskId }) {
            var task = model.backgroundTasks[index]
            if task.status.isTerminal,
               progress.kind == .workflowAgent,
               !(progress.state?.isTerminal ?? false) {
                return
            }
            if !task.status.isTerminal {
                task.status = .running
            }
            task.workflow = Self.updatedWorkflowRun(
                taskId: taskId,
                existing: task.workflow,
                runId: runId,
                progress: progress,
                nowMs: nowMs
            )
            model.backgroundTasks[index] = task
        } else {
            model.backgroundTasks.append(BackgroundTaskSnapshot(
                id: taskId,
                descriptionText: "",
                status: .running,
                workflow: Self.updatedWorkflowRun(
                    taskId: taskId,
                    existing: nil,
                    runId: runId,
                    progress: progress,
                    nowMs: nowMs
                )
            ))
        }

        if !known {
            refreshBackgroundTasks()
        }
    }

    private static func currentWallClockMs() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1_000)
    }

    private static func updatedWorkflowRun(
        taskId: String,
        existing: ConversationWorkflowRunSnapshot?,
        runId: String,
        progress: ConversationWorkflowProgressPayload,
        nowMs: UInt64
    ) -> ConversationWorkflowRunSnapshot {
        var run = existing?.runId == runId
            ? (existing ?? ConversationWorkflowRunSnapshot(taskId: taskId, runId: runId))
            : ConversationWorkflowRunSnapshot(taskId: taskId, runId: runId)
        run.lastUpdatedAtMs = max(run.lastUpdatedAtMs ?? 0, workflowEventMoment(progress, nowMs: nowMs))

        switch progress.kind {
        case .workflowPhase:
            upsertWorkflowPhase(&run, progress: progress, nowMs: nowMs)
        case .workflowLog:
            appendWorkflowLog(&run, progress: progress, nowMs: nowMs)
        case .workflowAgent:
            upsertWorkflowAgent(&run, progress: progress, nowMs: nowMs)
        }
        return run
    }

    private static func upsertWorkflowPhase(
        _ run: inout ConversationWorkflowRunSnapshot,
        progress: ConversationWorkflowProgressPayload,
        nowMs: UInt64
    ) {
        let title = progress.phaseTitle ?? progress.title ?? progress.label ?? progress.message ?? "Phase"
        let id = workflowPhaseID(
            index: progress.phaseIndex,
            title: title
        )
        let updatedAtMs = workflowEventMoment(progress, nowMs: nowMs)
        if let index = run.phases.firstIndex(where: { $0.id == id }) {
            run.phases[index].index = run.phases[index].index ?? progress.phaseIndex
            if !title.isEmpty { run.phases[index].title = title }
            if let label = progress.label, !label.isEmpty { run.phases[index].label = label }
            if let message = progress.message, !message.isEmpty { run.phases[index].message = message }
            run.phases[index].updatedAtMs = max(run.phases[index].updatedAtMs ?? 0, updatedAtMs)
        } else {
            run.phases.append(ConversationWorkflowPhaseSnapshot(
                id: id,
                index: progress.phaseIndex,
                title: title,
                label: progress.label,
                message: progress.message,
                updatedAtMs: updatedAtMs
            ))
        }
    }

    private static func appendWorkflowLog(
        _ run: inout ConversationWorkflowRunSnapshot,
        progress: ConversationWorkflowProgressPayload,
        nowMs: UInt64
    ) {
        let updatedAtMs = workflowEventMoment(progress, nowMs: nowMs)
        let id = workflowLogID(progress: progress, nowMs: nowMs)
        if run.logs.contains(where: { $0.id == id }) {
            return
        }
        run.logs.append(ConversationWorkflowLogSnapshot(
            id: id,
            phaseIndex: progress.phaseIndex,
            phaseTitle: progress.phaseTitle,
            label: progress.label,
            message: progress.message ?? progress.title,
            updatedAtMs: updatedAtMs
        ))
        if run.logs.count > 24 {
            run.logs.removeFirst(run.logs.count - 24)
        }
        if progress.phaseIndex != nil || progress.phaseTitle != nil {
            upsertWorkflowPhase(&run, progress: progress, nowMs: nowMs)
        }
    }

    private static func upsertWorkflowAgent(
        _ run: inout ConversationWorkflowRunSnapshot,
        progress: ConversationWorkflowProgressPayload,
        nowMs: UInt64
    ) {
        let resolvedIndex = progress.index ?? UInt64(run.agents.count)
        let incoming = ConversationWorkflowAgentSnapshot(
            id: "workflow-agent-\(resolvedIndex)",
            index: resolvedIndex,
            title: progress.title,
            message: progress.message,
            label: progress.label,
            phaseIndex: progress.phaseIndex,
            phaseTitle: progress.phaseTitle,
            agentId: progress.agentId,
            agentType: progress.agentType,
            model: progress.model,
            fallbackModel: progress.fallbackModel,
            state: progress.state ?? .progress,
            error: progress.error,
            toolUseId: progress.toolUseId,
            startedAtMs: progress.startedAtMs,
            queuedAtMs: progress.queuedAtMs,
            lastProgressAtMs: progress.lastProgressAtMs ?? workflowEventMoment(progress, nowMs: nowMs),
            attempt: progress.attempt,
            lastAttemptReason: progress.lastAttemptReason,
            tokens: progress.tokens,
            toolCalls: progress.toolCalls,
            lastToolName: progress.lastToolName,
            lastToolSummary: progress.lastToolSummary,
            promptPreview: progress.promptPreview
        )

        if let index = run.agents.firstIndex(where: { $0.index == resolvedIndex }) {
            run.agents[index] = mergeWorkflowAgent(
                current: run.agents[index],
                incoming: incoming,
                nowMs: nowMs
            )
        } else {
            run.agents.append(incoming)
        }

        if progress.phaseIndex != nil || progress.phaseTitle != nil {
            upsertWorkflowPhase(&run, progress: progress, nowMs: nowMs)
        }
    }

    private static func mergeWorkflowAgent(
        current: ConversationWorkflowAgentSnapshot,
        incoming: ConversationWorkflowAgentSnapshot,
        nowMs: UInt64
    ) -> ConversationWorkflowAgentSnapshot {
        // Once a child is terminal, no later start/progress payload may
        // mutate either its state or its final metadata. This also closes
        // the window where an out-of-order beacon could inflate counters
        // while the row still displayed as done/error.
        if current.state.isTerminal && !incoming.state.isTerminal {
            return current
        }

        var merged = current
        let currentMoment = workflowAgentMoment(current, nowMs: nowMs)
        let incomingMoment = workflowAgentMoment(incoming, nowMs: nowMs)
        let shouldAdvanceState: Bool
        if current.state.isTerminal {
            // Terminal is sticky. A later progress/start beacon must never
            // reopen a finished child; a newer terminal may still refine it.
            shouldAdvanceState = incoming.state.isTerminal && incomingMoment >= currentMoment
        } else {
            // A terminal always wins. Otherwise use event time to reject an
            // out-of-order start/progress update.
            shouldAdvanceState = incoming.state.isTerminal || incomingMoment >= currentMoment
        }

        if shouldAdvanceState {
            merged.state = incoming.state
        }
        merged.lastProgressAtMs = max(current.lastProgressAtMs ?? 0, incoming.lastProgressAtMs ?? 0)
        if shouldAdvanceState {
            merged.error = incoming.error ?? merged.error
        }

        merged.title = incoming.title ?? merged.title
        merged.message = shouldAdvanceState ? (incoming.message ?? merged.message) : merged.message
        merged.label = incoming.label ?? merged.label
        merged.phaseIndex = merged.phaseIndex ?? incoming.phaseIndex
        if let phaseIndex = incoming.phaseIndex, phaseIndex >= (merged.phaseIndex ?? 0) {
            merged.phaseIndex = phaseIndex
        }
        merged.phaseTitle = incoming.phaseTitle ?? merged.phaseTitle
        merged.agentId = incoming.agentId ?? merged.agentId
        merged.agentType = incoming.agentType ?? merged.agentType
        merged.model = incoming.model ?? merged.model
        merged.fallbackModel = incoming.fallbackModel ?? merged.fallbackModel
        merged.toolUseId = incoming.toolUseId ?? merged.toolUseId
        merged.startedAtMs = merged.startedAtMs ?? incoming.startedAtMs
        merged.queuedAtMs = merged.queuedAtMs ?? incoming.queuedAtMs
        if let attempt = incoming.attempt, attempt >= (merged.attempt ?? 0) {
            merged.attempt = attempt
            merged.lastAttemptReason = incoming.lastAttemptReason ?? merged.lastAttemptReason
        }
        if let tokens = incoming.tokens, tokens >= (merged.tokens ?? 0) {
            merged.tokens = tokens
        }
        if let toolCalls = incoming.toolCalls, toolCalls >= (merged.toolCalls ?? 0) {
            merged.toolCalls = toolCalls
        }
        merged.lastToolName = incoming.lastToolName ?? merged.lastToolName
        merged.lastToolSummary = incoming.lastToolSummary ?? merged.lastToolSummary
        merged.promptPreview = incoming.promptPreview ?? merged.promptPreview
        if shouldAdvanceState, let error = incoming.error, !error.isEmpty {
            merged.error = error
        }
        return merged
    }

    private static func workflowPhaseID(index: UInt32?, title: String?) -> String {
        if let index {
            return "phase-\(index)"
        }
        return "phase-\((title ?? "phase").lowercased())"
    }

    private static func workflowLogID(
        progress: ConversationWorkflowProgressPayload,
        nowMs: UInt64
    ) -> String {
        let moment = progress.lastProgressAtMs ?? nowMs
        return [
            progress.phaseIndex.map(String.init) ?? "phase",
            progress.label ?? "",
            progress.message ?? progress.title ?? "",
            String(moment),
        ].joined(separator: "|")
    }

    private static func workflowEventMoment(
        _ progress: ConversationWorkflowProgressPayload,
        nowMs: UInt64
    ) -> UInt64 {
        progress.lastProgressAtMs ?? progress.startedAtMs ?? progress.queuedAtMs ?? nowMs
    }

    private static func workflowAgentMoment(
        _ agent: ConversationWorkflowAgentSnapshot,
        nowMs: UInt64
    ) -> UInt64 {
        agent.lastProgressAtMs ?? agent.startedAtMs ?? agent.queuedAtMs ?? nowMs
    }

    private static func backgroundTaskStatus(
        _ status: TaskStatusDto
    ) -> BackgroundTaskSnapshot.Status? {
        switch status {
        case .pending: return .pending
        case .running: return .running
        case .paused: return .paused
        case .completed: return .completed
        case .failed: return .failed
        case .cancelled: return .cancelled
        @unknown default: return nil
        }
    }

    /// Pull the engine's real MCP server listing (out-of-band, like
    /// `listSessions`). The `McpServers` reply lands on `apply` → `model.mcpServers`.
    func refreshMcpServers() {
        Task { [weak self] in
            guard let self else { return }
            do {
                let handle = try await self.ensureHandle()
                try await handle.submit(command: .refreshListings(which: [.mcp]))
            } catch {
                self.fail(.host, "\(error)")
            }
        }
    }

    // MARK: session agents

    /// Pull the roster for the currently active session. Agent state is
    /// deliberately separate from the main turn stream so opening the
    /// picker never starts or interrupts a turn.
    func listSessionAgents() {
        guard !model.activeSessionId.isEmpty else { return }
        let requestSessionID = model.activeSessionId
        Task { [weak self] in
            guard let self else { return }
            guard self.model.activeSessionId == requestSessionID,
                  !self.model.sessionTransitionPending else { return }
            do {
                let handle = try await self.ensureHandle()
                guard self.model.activeSessionId == requestSessionID,
                      !self.model.sessionTransitionPending else { return }
                try await handle.submit(command: .listSessionAgents)
            } catch {
                // Listing failures must not overwrite a transcript load
                // error (or a newer request). The next picker open retries.
            }
        }
    }

    /// Select a root/child agent. Child selection is read-only; the
    /// transcript is loaded lazily the first time it is requested.
    func selectAgent(_ id: String) {
        guard id == ConversationModel.mainAgentID
            || model.agentSummaries.contains(where: { $0.id == id })
        else { return }
        model.markAgentTranscriptLoading(id, sessionID: model.activeSessionId)
        guard id != ConversationModel.mainAgentID else { return }
        if model.agentTranscripts[id]?.loaded == true { return }
        loadSessionAgentTranscript(id)
    }

    func loadSessionAgentTranscript(_ id: String) {
        guard id != ConversationModel.mainAgentID, !id.isEmpty,
              !model.activeSessionId.isEmpty
        else { return }
        let requestSessionID = model.activeSessionId
        let requestKey = model.markAgentTranscriptLoading(id, sessionID: requestSessionID)
        Task { [weak self] in
            guard let self else { return }
            guard self.model.activeSessionId == requestSessionID,
                  self.model.selectedAgentID == id else { return }
            do {
                let handle = try await self.ensureHandle()
                guard self.model.activeSessionId == requestSessionID,
                      self.model.selectedAgentID == id else { return }
                try await handle.submit(command: .loadSessionAgentTranscript(agentId: id))
            } catch {
                self.model.failAgentTranscript(
                    id,
                    sessionID: requestSessionID,
                    message: String(describing: error),
                    requestKey: requestKey
                )
            }
        }
    }

    // MARK: inbound-event application (called on the main actor)

    private static func agentSummary(from dto: SessionAgentSummaryDto) -> ConversationAgentSummary {
        let summary = ConversationAgentSummary(
            id: dto.agentId,
            name: dto.agentId == ConversationModel.mainAgentID
                ? ConversationAgentSummary.main.name
                : dto.name,
            agentType: dto.agentType,
            model: dto.model,
            modelProfile: dto.modelProfile,
            status: dto.status,
            latestActivity: dto.agentId == ConversationModel.mainAgentID
                ? nil
                : dto.latestActivity,
            updatedAtMs: dto.updatedAtMs ?? 0
        )
        return ConversationModel.normalizedAgentSummaryForSource(summary)
    }

    private func applyAgentTranscript(
        sessionID: String,
        agentId: String,
        rows: [SessionAgentMessageRowDto],
        nextMessageIndex: UInt64? = nil,
        revision: UInt64 = 0
    ) {
        guard agentId != ConversationModel.mainAgentID else { return }
        guard sessionID == model.activeSessionId else { return }
        // Deleted rows leave gaps in the host's numbering, so the position in
        // the array is not the wire index.
        let messages = rows.map(\.message)
        let watermark = nextMessageIndex ?? UInt64(messages.count)
        if let current = model.agentTranscripts[agentId],
           current.loaded,
           revision <= current.revision || watermark < current.nextMessageIndex {
            // A reply from an older load request arrived after a newer
            // snapshot. The engine revision counts raw transcript records,
            // including hidden/lifecycle entries, so it remains monotonic
            // even when two snapshots expose the same visible message
            // count. The visible watermark is a second independent fence:
            // live indexed rows advance it without advancing the raw
            // revision, so a delayed full reply must never move it back.
            // Equality remains allowed for a higher-revision compaction
            // update whose renderable details changed in place.
            return
        }
        let restored = Self.restoredTranscript(
            from: messages,
            sessionID: sessionID,
            identityPrefix: "\(sessionID)|agent|\(agentId)",
            occurrenceOffsets: [:],
            wireIndices: rows.map { Optional($0.messageIndex) }
        )
        var transcript = ConversationAgentTranscript(
            messages: restored.messages,
            items: restored.items,
            details: restored.details,
            wireSignatures: restored.wireSignatures,
            wireIndices: restored.wireIndices,
            nextMessageIndex: watermark,
            revision: revision,
            loaded: true
        )
        let pending = model.takePendingAgentMessages(agentID: agentId, sessionID: sessionID)
            .sorted { lhs, rhs in
                switch (lhs.index, rhs.index) {
                case let (left?, right?): return left < right
                case (.some, .none): return true
                default: return false
                }
            }
        var seenIndices = Set<UInt64>()
        for pendingMessage in pending {
            if let index = pendingMessage.index {
                guard index >= watermark, seenIndices.insert(index).inserted else { continue }
            }
            let one = Self.restoredTranscript(
                from: [pendingMessage.message],
                sessionID: sessionID,
                identityPrefix: "\(sessionID)|agent|\(agentId)",
                occurrenceOffsets: Self.signatureCounts(transcript.wireSignatures),
                wireIndices: [pendingMessage.index],
                finalizeOrphans: false
            )
            transcript = appendTranscript(one, to: transcript)
            if let index = pendingMessage.index {
                transcript.nextMessageIndex = max(transcript.nextMessageIndex, index + 1)
            }
        }
        model.setAgentTranscript(
            agentId,
            transcript: transcript,
            sessionID: sessionID
        )
    }

    /// A transcript reply and the live tail reader share one event stream,
    /// but a reply can still overtake a message that was emitted just before
    /// it. Merge the two projections instead of replacing the live cache.
    /// Message ids are deterministic hashes of the canonical wire message
    /// plus its occurrence, so repeated identical messages remain distinct;
    /// render item ids and details then retain structured tool rows without
    /// duplicating them.
    private func mergeAgentTranscript(
        _ restored: EngineConversationSource.RestoredTranscript,
        with current: ConversationAgentTranscript?,
        loaded: Bool
    ) -> ConversationAgentTranscript {
        guard let current, !current.messages.isEmpty || !current.items.isEmpty else {
            return ConversationAgentTranscript(
                messages: restored.messages,
                items: restored.items,
                details: restored.details,
                wireSignatures: restored.wireSignatures,
                wireIndices: restored.wireIndices,
                nextMessageIndex: restored.wireIndices.compactMap { $0 }.max().map { $0 + 1 }
                    ?? UInt64(restored.wireSignatures.count),
                revision: 0,
                loaded: loaded
            )
        }

        var restoredIDs = Set(restored.messages.map(\.id))
        var messages = restored.messages
        for message in current.messages {
            if restoredIDs.insert(message.id).inserted {
                messages.append(message)
            }
        }

        var itemIDs = Set(restored.items.map(\.id))
        var messageItemIDs = Set(
            restored.items.compactMap { item -> UUID? in
                guard case let .message(message) = item else { return nil }
                return message.id
            }
        )
        var items = restored.items
        for item in current.items {
            if case let .message(message) = item {
                guard messageItemIDs.insert(message.id).inserted else { continue }
            } else if case let .run(currentRun) = item,
                      let restoredIndex = items.firstIndex(where: { item in
                          guard case let .run(restoredRun) = item else { return false }
                          return restoredRun.tools.contains { restoredTool in
                              currentRun.tools.contains { $0.id == restoredTool.id }
                          }
                      }),
                      case let .run(restoredRun) = items[restoredIndex] {
                var merged = restoredRun
                merged.tools = mergeToolTraces(restoredRun.tools, currentRun.tools)
                merged.reasoning = currentRun.reasoning.isEmpty ? restoredRun.reasoning : currentRun.reasoning
                merged.activities = mergeActivities(restoredRun.activities, currentRun.activities)
                merged.status = merged.tools.contains(where: { $0.status == .running })
                    ? currentRun.status
                    : .restored
                items[restoredIndex] = .run(merged)
                continue
            } else {
                guard itemIDs.insert(item.id).inserted else { continue }
            }
            items.append(item)
        }
        var details = restored.details
        details.merge(current.details) { _, new in new }
        return ConversationAgentTranscript(
            messages: messages,
            items: items,
            details: details,
            wireSignatures: mergeWireSignatures(
                restored.wireSignatures,
                current.wireSignatures
            ),
            wireIndices: restored.wireIndices + current.wireIndices,
            nextMessageIndex: max(
                restored.wireIndices.compactMap { $0 }.max().map { $0 + 1 }
                    ?? UInt64(restored.wireSignatures.count),
                current.nextMessageIndex
            ),
            revision: current.revision,
            loaded: loaded || current.loaded
        )
    }

    private func mergeToolTraces(
        _ base: [ConversationToolTrace],
        _ update: [ConversationToolTrace]
    ) -> [ConversationToolTrace] {
        var result = base
        for trace in update {
            if let index = result.firstIndex(where: { $0.id == trace.id }) {
                var merged = result[index]
                merged.tool = trace.tool.isEmpty ? merged.tool : trace.tool
                merged.status = trace.status
                merged.inputSummary = trace.inputSummary ?? merged.inputSummary
                merged.outputSummary = trace.outputSummary ?? merged.outputSummary
                merged.elapsedMs = trace.elapsedMs ?? merged.elapsedMs
                merged.header = trace.header ?? merged.header
                merged.display = trace.display ?? merged.display
                result[index] = merged
            } else {
                result.append(trace)
            }
        }
        return result
    }

    private func mergeActivities(
        _ base: [ConversationExecutionActivity],
        _ update: [ConversationExecutionActivity]
    ) -> [ConversationExecutionActivity] {
        var result = base
        var ids = Set(base.map(\.id))
        for activity in update where ids.insert(activity.id).inserted {
            result.append(activity)
        }
        return result
    }

    private func appendTranscript(
        _ incoming: EngineConversationSource.RestoredTranscript,
        to current: ConversationAgentTranscript
    ) -> ConversationAgentTranscript {
        var messages = current.messages
        var messageIDs = Set(messages.map(\.id))
        for message in incoming.messages where messageIDs.insert(message.id).inserted {
            messages.append(message)
        }

        var items = current.items
        var itemIDs = Set(items.map(\.id))
        for item in incoming.items {
            if case let .run(incomingRun) = item,
               let index = items.firstIndex(where: { item in
                   guard case let .run(existingRun) = item else { return false }
                   return existingRun.tools.contains { existingTool in
                       incomingRun.tools.contains { $0.id == existingTool.id }
                   }
               }),
               case let .run(existingRun) = items[index] {
                var merged = existingRun
                merged.tools = mergeToolTraces(existingRun.tools, incomingRun.tools)
                merged.reasoning = incomingRun.reasoning.isEmpty ? existingRun.reasoning : incomingRun.reasoning
                merged.activities = mergeActivities(existingRun.activities, incomingRun.activities)
                merged.status = incomingRun.status
                items[index] = .run(merged)
            } else if itemIDs.insert(item.id).inserted {
                items.append(item)
            }
        }
        var details = current.details
        details.merge(incoming.details) { _, new in new }
        return ConversationAgentTranscript(
            messages: messages,
            items: items,
            details: details,
            wireSignatures: mergeWireSignatures(
                current.wireSignatures,
                incoming.wireSignatures
            ),
            wireIndices: current.wireIndices + incoming.wireIndices,
            nextMessageIndex: max(
                current.nextMessageIndex,
                incoming.wireIndices.compactMap { $0 }.max().map { $0 + 1 }
                    ?? current.nextMessageIndex
            ),
            revision: current.revision,
            loaded: current.loaded
        )
    }

    private func mergeWireSignatures(
        _ first: [String],
        _ second: [String]
    ) -> [String] {
        let firstCounts = Self.signatureCounts(first)
        let secondCounts = Self.signatureCounts(second)
        var result = first
        var consumed = firstCounts
        for signature in second {
            let target = max(
                firstCounts[signature, default: 0],
                secondCounts[signature, default: 0]
            )
            guard consumed[signature, default: 0] < target else { continue }
            result.append(signature)
            consumed[signature, default: 0] += 1
        }
        return result
    }

    private func appendAgentMessage(
        agentId: String,
        message: MessageDto,
        messageIndex: UInt64? = nil
    ) {
        guard agentId != ConversationModel.mainAgentID else { return }
        let sessionID = model.activeSessionId
        guard !sessionID.isEmpty else { return }
        guard model.shouldAcceptAgentMessage(
            agentID: agentId,
            sessionID: sessionID,
            index: messageIndex
        ) else { return }
        let previous = model.agentTranscripts[agentId]
        _ = model.enqueuePendingAgentMessage(
            message,
            index: messageIndex,
            agentID: agentId,
            sessionID: sessionID
        )
        let one = Self.restoredTranscript(
            from: [message],
            sessionID: sessionID,
            identityPrefix: "\(sessionID)|agent|\(agentId)",
            occurrenceOffsets: Self.signatureCounts(previous?.wireSignatures ?? []),
            wireIndices: [messageIndex],
            finalizeOrphans: false
        )
        var current = previous ?? ConversationAgentTranscript()
        // MessageDto has no wire id. Keep every occurrence here; collapsing
        // by role/text would lose two legitimate adjacent messages that say
        // the same thing. Full transcript replies are merged by deterministic
        // occurrence IDs, while a live event is necessarily treated as one
        // new occurrence.
        current = appendTranscript(one, to: current)
        if let messageIndex {
            current.nextMessageIndex = max(current.nextMessageIndex, messageIndex + 1)
        }
        model.setAgentTranscript(agentId, transcript: current, sessionID: sessionID)
        let activity = one.messages.last?.text
            .split(whereSeparator: { $0 == "\n" || $0 == "\r" })
            .first
            .map(String.init)
        let summary = model.agentSummaries.first(where: { $0.id == agentId })
            ?? ConversationAgentSummary(
                id: agentId,
                name: agentId,
                agentType: "unknown",
                status: "working"
            )
        // Transcript delivery has no engine lifecycle timestamp. Preserve
        // the authoritative status and clock: a delayed message must not
        // outrank a terminal update or resurrect an already finished agent.
        model.upsertAgentSummary(ConversationAgentSummary(
            id: summary.id,
            name: summary.name,
            agentType: summary.agentType,
            model: summary.model,
            modelProfile: summary.modelProfile,
            status: summary.status,
            latestActivity: activity ?? summary.latestActivity,
            updatedAtMs: summary.updatedAtMs
        ))
    }

    private func refreshSessionAgentsAfterTransition() {
        guard let handle else { return }
        Task { [handle] in
            try? await handle.submit(command: .listSessionAgents)
        }
    }

    private static func workflowProgressPayload(
        from progress: WorkflowProgressDto
    ) -> ConversationWorkflowProgressPayload? {
        guard let kind = ConversationWorkflowProgressKind(rawValue: progress.kind) else {
            return nil
        }
        return ConversationWorkflowProgressPayload(
            kind: kind,
            index: progress.index,
            title: progress.title,
            message: progress.message,
            label: progress.label,
            phaseIndex: progress.phaseIndex,
            phaseTitle: progress.phaseTitle,
            agentId: progress.agentId,
            agentType: progress.agentType,
            model: progress.model,
            fallbackModel: progress.fallbackModel,
            state: progress.state.flatMap(ConversationWorkflowAgentState.init(rawValue:)),
            error: progress.error,
            toolUseId: progress.toolUseId,
            startedAtMs: progress.startedAtMs,
            queuedAtMs: progress.queuedAtMs,
            lastProgressAtMs: progress.lastProgressAtMs,
            attempt: progress.attempt,
            lastAttemptReason: progress.lastAttemptReason,
            tokens: progress.tokens,
            toolCalls: progress.toolCalls,
            lastToolName: progress.lastToolName,
            lastToolSummary: progress.lastToolSummary,
            promptPreview: progress.promptPreview
        )
    }

    func applyWorkflowProgress(
        originSessionId: String,
        taskId: String,
        runId: String,
        progress: WorkflowProgressDto
    ) {
        guard !originSessionId.isEmpty,
              originSessionId == model.activeSessionId,
              !model.sessionTransitionPending
        else { return }
        guard let payload = Self.workflowProgressPayload(from: progress) else { return }
        upsertWorkflowProgress(taskId: taskId, runId: runId, progress: payload)
    }

    private static func isTerminalRecoveryState(_ state: TurnRecoveryStateDto) -> Bool {
        switch state {
        case .completed, .failed, .cancelled:
            return true
        case .running, .waitingForUser, .pausedRecoverable:
            return false
        @unknown default:
            return false
        }
    }

    private enum PauseAcknowledgementError: LocalizedError {
        case invalidated
        case recoveryState(TurnRecoveryStateDto)
        case timedOut(turnID: UInt64)

        var errorDescription: String? {
            switch self {
            case .invalidated:
                return "PauseTurn was invalidated by a session transition"
            case let .recoveryState(state):
                return "PauseTurn was not acknowledged: \(String(describing: state))"
            case let .timedOut(turnID):
                return "PauseTurn was not acknowledged for turn \(turnID)"
            }
        }
    }

    /// A pause acknowledgement is a REMOTE event, not a command result:
    /// `submitCommand` returning proves only that the host accepted the
    /// command. `MobileHost::pause_active_turn` answers a request whose
    /// turn id is no longer the active one with `Ok(())` and emits NOTHING,
    /// so the continuation parked in `pauseAcknowledgements` would never be
    /// resumed and the submit `catch` never runs. Because
    /// `pauseAcknowledgements.isEmpty` gates `startNewConversation`,
    /// `send`, `openSession` and `resumeSession` — and the only drain
    /// (`invalidateTurnContext`) is reachable only through those same
    /// blocked entry points — one unacknowledged pause bricked the app for
    /// the rest of the session. Bound the wait exactly the way
    /// `waitForCancellationAcknowledgement` bounds cancel.
    private static let pauseAcknowledgementTimeout: TimeInterval = 5

    /// Fail every continuation still parked on `token` once the deadline
    /// passes. A late `PausedRecoverable` snapshot is then a no-op:
    /// `resolvePauseAcknowledgements` finds no entry and returns.
    private func armPauseAcknowledgementDeadline(token: ConversationTurnToken) {
        Task { @MainActor [weak self] in
            try? await Task.sleep(
                nanoseconds: UInt64(Self.pauseAcknowledgementTimeout * 1_000_000_000)
            )
            guard let self, self.pauseAcknowledgements[token] != nil else { return }
            let error = PauseAcknowledgementError.timedOut(turnID: token.clientTurnId)
            if self.activeConversationTurnToken == token {
                // Same posture as a rejected submit: the turn stays owned
                // and the host failure is visible, rather than an unknown
                // state being laundered into Cancelled.
                self.model.error = ConversationError(
                    kind: .host,
                    message: error.localizedDescription
                )
            }
            self.resolvePauseAcknowledgements(token: token, result: .failure(error))
        }
    }

    private enum CancellationAcknowledgementError: LocalizedError {
        case timedOut(turnID: UInt64)

        var errorDescription: String? {
            switch self {
            case let .timedOut(turnID):
                return "Cancel was not terminally acknowledged for turn \(turnID)"
            }
        }
    }

    private func resolvePauseAcknowledgements(
        token: ConversationTurnToken,
        result: Result<Void, Error>
    ) {
        guard let continuations = pauseAcknowledgements.removeValue(forKey: token) else {
            return
        }
        for continuation in continuations {
            continuation.resume(with: result)
        }
    }

    private func waitForCancellationAcknowledgement(turnID: UInt64) async throws {
        if terminalRecoveryStates[turnID].map(Self.isTerminalRecoveryState) == true {
            return
        }
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            try await Task.sleep(nanoseconds: 10_000_000)
            if terminalRecoveryStates[turnID].map(Self.isTerminalRecoveryState) == true {
                return
            }
        }
        throw CancellationAcknowledgementError.timedOut(turnID: turnID)
    }

    private static func reasoningID(_ selection: ReasoningSelectionDto) -> String {
        switch selection {
        case .automatic: return "automatic"
        case .disabled: return "disabled"
        case .enabled: return "enabled"
        case let .level(id): return id
        case let .tokenBudget(tokens): return "budget:\(tokens)"
        }
    }

    private static func lowerModelRuntimeDetails(_ dto: ModelDetailsDto) -> ModelRuntimeDetails {
        ModelRuntimeDetails.from(dto)
    }

    private static func lowerProviderCatalogEntry(
        _ dto: ProviderModelCatalogEntryDto
    ) -> ProviderCatalogEntry {
        let presetID = dto.providerId == "gemini" ? "google" : dto.providerId
        let preset = Presets.llm.first(where: { $0.id == presetID })
        let loweredModels = dto.models.map(Self.lowerModelRuntimeDetails)
        return ProviderCatalogEntry(
            id: presetID,
            displayName: dto.providerLabel,
            baseURL: preset?.defaultUrl ?? "",
            protocolName: preset?.sub ?? "",
            authName: "",
            credentialEnv: nil,
            models: loweredModels.map(\.modelId),
            modelDetails: Dictionary(uniqueKeysWithValues: loweredModels.map { ($0.reference, $0) })
        )
    }

    private static func controlsState(from dto: ConversationControlsDto) -> ConversationControlsState {
        let reasoningOptions = dto.reasoning.spec.options.map { option in
            let id = reasoningID(option.selection)
            return ConversationReasoningOption(
                id: id,
                title: id.hasPrefix("budget:") ? "Token budget" : (id == "automatic" ? "Auto" : id.capitalized),
                isBudget: id.hasPrefix("budget:"),
                persistable: option.persistable
            )
        }
        let budget = dto.reasoning.spec.budgetRange.map {
            ClosedRange(uncheckedBounds: (lower: $0.minTokens, upper: $0.maxTokens))
        }
        return ConversationControlsState(
            qualifiedModel: dto.qualifiedModel,
            requestedPermission: dto.permission.requested,
            effectivePermission: dto.permission.effective,
            permissionOptions: dto.permission.options.map {
                ConversationPermissionOption(
                    id: $0.mode,
                    available: $0.available,
                    disabledReason: $0.disabledReason?.code
                )
            },
            requestedReasoning: reasoningID(dto.reasoning.requested),
            effectiveReasoning: reasoningID(dto.reasoning.effective),
            reasoningOptions: reasoningOptions,
            budgetRange: budget,
            providerDefault: reasoningID(dto.reasoning.spec.providerDefault),
            forcedReasoning: dto.reasoning.spec.forcedReasoning,
            editable: dto.reasoning.spec.editable,
            disabledReason: dto.reasoning.spec.disabledReason?.code
        )
    }

    private func applyControlsSnapshot(_ dto: ConversationControlsDto) {
        guard model.activeModelId.isEmpty || model.activeModelId == dto.qualifiedModel else {
            return
        }
        let snapshot = Self.controlsState(from: dto)
        model.controls = snapshot
        model.requestedPermissionMode = snapshot.requestedPermission
        model.effectivePermissionMode = snapshot.effectivePermission
        model.reasoningSelection = snapshot.effectiveReasoning
        model.reasoningOptions = snapshot.reasoningOptions.map(\.id)
        model.reasoningOptionDetails = snapshot.reasoningOptions
        model.reasoningDisabledReason = snapshot.disabledReason
        model.permissionOptions = snapshot.permissionOptions
        model.controlsPending = false
        model.controlsError = nil
    }

    /// Map one inbound `ClientEvent` onto the published state.
    func apply(_ event: ClientEvent) {
        if Self.settingsOwner === self {
            DesktopSettingsRepository.shared.consume(event)
        }
        externalEventHandler?(event)
        switch event {
        case .turnStarted:
            guard acceptTurnEvent(event) else { return }
            assistantRowsByIdentity = [:]
            assistantReasoningByIdentity = [:]
            currentResponseReasoningIDs = []
            pendingAssistantIdentity = nil
            if let currentTurnId {
                executorOwnedTurnID = currentTurnId
            }
            model.slashCommandPending = false
            pendingSlashRaw = nil
            model.streaming = true
            model.updateMainAgent(status: "working", latestActivity: String(localized: "chat_working"))
            model.notice = nil
            streamingIndex = nil
            streamingItemIndex = nil
            currentResponseMessageIDs = []
            updateActiveRun {
                $0.status = .running
                $0.retry = nil
            }

        case let .slashCommandResult(turnId, display, isError):
            guard let token = activeConversationTurnToken,
                  turnId == nil || turnId == token.clientTurnId
            else { return }
            let raw = pendingSlashRaw ?? "/"
            if Self.isCompactSlash(raw) {
                if case .completed = model.compactionStatus {
                    // The authoritative CompactionCompleted metrics arrived first.
                } else if !isError && display.hasPrefix("Compacted") {
                    model.compactionStatus = .completed(
                        messagesBefore: nil,
                        messagesAfter: nil,
                        bytesSaved: nil
                    )
                } else {
                    model.compactionStatus = .failed(detail: display)
                }
            }
            model.items.append(.commandOutput(ConversationCommandOutput(
                id: "slash-\(token.sessionEpoch)-\(token.clientTurnId)",
                command: raw,
                text: display,
                isError: isError
            )))
            model.streaming = false
            model.slashCommandPending = false
            model.statusLine = nil
            model.updateMainAgent(
                status: isError ? "failed" : "idle",
                latestActivity: isError ? display : String(localized: "chat_completed")
            )
            // A CLI slash turn ends HERE — there is no TurnEnded for it — so
            // it must settle its run the way `.turnEnded` does. Without this
            // the run card stays `.running` forever and, because
            // `clearTurnPointers` drops the active-run pointer on the next
            // line, nothing can ever settle it again: the conversation keeps
            // claiming `requiresBackgroundExecution` for the rest of the
            // session. `/compact` made it visible (the compaction card
            // finishes while the lease does not), but every CLI slash
            // command leaked the same way.
            //
            // Guarded on an EXISTING run: `finishActiveRun` goes through
            // `updateActiveRun` -> `ensureActiveRun`, which CREATES a run
            // when there is none. A locally-handled slash command has no
            // run, and settling one into existence appended a spurious run
            // card after the command output.
            if activeRunItemIndex.map({ model.items.indices.contains($0) }) == true {
                finishActiveRun(isError ? .failed : .completed)
            }
            publishActiveTurnCompletion(isError ? .failed : .completed)
            clearTurnPointers(keepEpoch: false)
            requestSessionCatalogRefreshAfterSettledTurn()

        case let .textDelta(text):
            guard acceptTurnEvent(event) else { return }
            if let messageID = appendDelta(text) {
                appendTextBoundaryActivity(messageID: messageID)
            }
            let activity = text
                .split(whereSeparator: { $0 == "\n" || $0 == "\r" })
                .first
                .map(String.init)
            model.updateMainAgent(status: "working", latestActivity: activity)
            publishTurnSpeechDelta(text)

        case let .thinkingDelta(thinking, signature):
            guard acceptTurnEvent(event) else { return }
            appendReasoningActivity(thinking)
            if signature != nil {
                var addedSignature = false
                updateActiveRun { run in
                    if run.notices.contains(where: { $0.id == "thinking-signature" }) == false {
                        run.notices.append(.init(id: "thinking-signature", kind: .info, text: String(localized: "chat_thinking_signature")))
                        addedSignature = true
                    }
                }
                if addedSignature { appendNoticeActivity(id: "thinking-signature") }
            }

        case let .scheduledTaskFire(message):
            // This session notice precedes turnStarted. Keep the ordinary
            // turn-event gate intact for SystemNotice and stale turn data.
            appendMessage(Message(role: .ai, text: message))

        case let .loopWakeup(message, companion, streak, _):
            // A scheduled fire precedes turn_started, so it must bypass
            // the active-turn gate just like other session-level events.
            let hidden = Set(model.messages.flatMap { $0.loopFoldedItemIDs })
            let rows = model.items.filter { !hidden.contains($0.id) }
            let start = rows.lastIndex { row in
                if case let .message(message) = row { return message.loopWakeupStreak != nil }
                return false
            }
            var wakeup = Message(role: .ai, text: message)
            wakeup.loopWakeupStreak = streak
            if streak > 0, let start {
                wakeup.loopFoldedItemIDs = Set(rows[start...].map(\.id))
            }
            appendMessage(wakeup)
            if let companion { appendMessage(Message(role: .ai, text: companion)) }

        case let .systemNotice(message, isError):
            guard acceptTurnEvent(event) else { return }
            let notice = ConversationExecutionNotice(
                id: "notice-\(UUID().uuidString)",
                kind: isError ? .error : .info,
                text: message
            )
            updateActiveRun { $0.notices.append(notice) }
            appendNoticeActivity(id: notice.id)
            model.statusLine = message

        case let .askUserQuestion(request):
            // Passes the turn gate via the allowlist in `acceptTurnEvent`:
            // a broker replay after a foreground re-connect arrives
            // OUTSIDE a turn. Dedupe by request id — a replay of a
            // still-pending request must not stack a second card.
            guard acceptTurnEvent(event) else { return }
            let question = Self.pendingQuestion(from: request)
            guard !model.pendingQuestions.contains(where: { $0.requestId == question.requestId }) else {
                return
            }
            // A broker replay can arrive after the visible source has
            // reconnected, but a live question is still owned by this
            // executor. The WaitingForUser recovery projection below uses
            // this bit to keep the normal active Stop affordance.
            if let currentTurnId {
                executorOwnedTurnID = currentTurnId
            }
            model.pendingQuestions.append(question)

        case let .askUserQuestionResolved(requestId):
            // Allowlisted like `.askUserQuestion` above — resolution can
            // also be replayed outside a turn. Drop the card whether the
            // request was answered here, elsewhere, or auto-continued.
            guard acceptTurnEvent(event) else { return }
            model.pendingQuestions.removeAll { $0.requestId == requestId }

        case let .permissionRequestResolved(requestId, _):
            // The engine is authoritative: cancellation and expiry can
            // resolve a request without a local button tap. Idempotently
            // remove the correlated prompt so a stale modal cannot survive.
            guard acceptTurnEvent(event) else { return }
            model.pendingPermissions.removeAll { $0.requestId == requestId }

        case let .taskRow(task):
            // A `TaskList` reply row (out-of-band, allowlisted). Rows
            // both seed the panel at bootstrap and backfill the
            // description for ids first seen via a status push.
            guard acceptTurnEvent(event) else { return }
            if let mapped = Self.backgroundTaskStatus(task.status) {
                upsertBackgroundTask(
                    id: task.taskId,
                    description: task.description,
                    status: mapped,
                    canResume: task.canResume,
                    startedAtMs: task.startedAtMs,
                    errorText: task.error,
                    taskType: task.taskType
                )
            }

        case let .workflowResumed(previousTaskId, task, runId, originSessionId):
            guard acceptTurnEvent(event) else { return }
            if let originSessionId {
                guard originSessionId == model.activeSessionId,
                      !model.sessionTransitionPending else { return }
            }
            model.backgroundTasks.removeAll { $0.id == previousTaskId }
            if let mapped = Self.backgroundTaskStatus(task.status) {
                upsertBackgroundTask(
                    id: task.taskId,
                    description: task.description,
                    status: mapped,
                    canResume: task.canResume,
                    startedAtMs: task.startedAtMs,
                    taskType: task.taskType
                )
            }
            model.workflowResumeState = .succeeded(taskID: task.taskId)
            model.statusLine = String(localized: "chat_workflow_resumed \(runId)")

        case let .planUpdated(tasks):
            // The model's working plan. FULL-LIST replace — an empty list
            // is the engine clearing it, never "no news". Allowlisted
            // through the turn gate like `taskRow` above: the plan block
            // outlives the turn that wrote it, and a dropped update would
            // strand a stale checklist above the composer.
            guard acceptTurnEvent(event) else { return }
            model.planTasks = tasks.map(Self.planTask(from:))

        case let .taskStatusChanged(taskId, status, originSessionId, taskError):
            // Allowlisted through the turn gate: a background task
            // normally finishes after its spawning turn already ended.
            guard acceptTurnEvent(event) else { return }
            if let originSessionId {
                guard !originSessionId.isEmpty,
                      originSessionId == model.activeSessionId,
                      !model.sessionTransitionPending
                else { return }
            }
            if let mapped = Self.backgroundTaskStatus(status) {
                let known = model.backgroundTasks.contains { $0.id == taskId }
                let becameFailed = mapped == .failed
                    && model.backgroundTasks.first(where: { $0.id == taskId })?.status != .failed
                upsertBackgroundTask(
                    id: taskId,
                    description: nil,
                    status: mapped,
                    errorText: taskError
                )
                if !known || becameFailed {
                    // First sighting via a push — pull the row list so
                    // the panel can show the human description. A failed
                    // Create run also needs the Host's recovery capability,
                    // which is carried only by full TaskRow snapshots.
                    refreshBackgroundTasks()
                }
            }
            // Name the task by its human description when one is already
            // known (a `TaskRow` reply, or an earlier push that triggered
            // the refresh above); fall back to the bare id only when it is
            // genuinely all we have.
            let label = model.backgroundTasks
                .first { $0.id == taskId }
                .map(\.descriptionText)
                .flatMap { $0.isEmpty ? nil : $0 }
                ?? taskId
            let reason = taskError.flatMap { $0.isEmpty ? nil : $0 }
            let text: String?
            switch status {
            case .completed: text = String(localized: "chat_task_completed \(label)")
            case .failed:
                text = reason.map { String(localized: "chat_task_failed_reason \(label) \($0)") }
                    ?? String(localized: "chat_task_failed \(label)")
            case .cancelled: text = String(localized: "chat_task_cancelled \(label)")
            case .pending, .running, .paused: text = nil
            @unknown default: text = nil
            }
            if let text {
                model.items.append(.notice(ConversationExecutionNotice(
                    id: "task-\(taskId)-\(UUID().uuidString)",
                    kind: status == .failed ? .error : .info,
                    text: text
                )))
            }

        case let .toolUseStarted(id, tool, inputJson, header):
            let accepted = acceptTurnEvent(event)
            Self.turnLog.debug(
                "tool started id=\(id, privacy: .public) name=\(tool, privacy: .public) turn=\(self.currentTurnId ?? 0, privacy: .public) accepted=\(accepted, privacy: .public)"
            )
            guard accepted else { return }
            // The engine derived the header once; we render THAT. The old
            // client-side summarizer below survives only as the fallback for
            // an engine too old to send one.
            let derivedHeader = header.map(Self.toolHeader(from:))
            // A tool starts a new timeline phase. The next text delta must
            // open a distinct message row rather than append across the tool.
            streamingIndex = nil
            streamingItemIndex = nil
            appendToolActivity(id: id)
            if ConversationExecutionParsing.isShellTool(tool) {
                let started = ConversationExecutionParsing.shellStarted(id: id, inputJson: inputJson)
                upsertShellCard(id: id, create: {
                    ConversationShellCard(
                        sessionId: model.activeSessionId,
                        turnId: currentTurnId,
                        taskId: started.taskId,
                        command: started.command,
                        cwd: started.cwd
                    )
                }, mutate: { card in
                    card.command = started.command
                    card.cwd = started.cwd
                    card.status = .running
                })
                upsertTool(id: id, tool: "Shell", fallbackSummary: started.command) { trace in
                    trace.tool = "Shell"
                    trace.status = .running
                    trace.inputSummary = started.command
                    trace.header = derivedHeader
                }
                model.statusLine = String(localized: "chat_shell_running")
                model.updateMainAgent(status: "working", latestActivity: model.statusLine)
            } else {
                let summary = ConversationExecutionParsing.summarizeToolInput(inputJson)
                upsertTool(id: id, tool: tool, fallbackSummary: summary) { trace in
                    trace.tool = tool
                    trace.status = .running
                    trace.inputSummary = summary
                    trace.planDocument = PlanDocument.tool(tool, json: inputJson)
                    trace.header = derivedHeader
                }
                model.statusLine = String(localized: "chat_tool_calling \(tool)")
                model.updateMainAgent(status: "working", latestActivity: model.statusLine)
            }

        case let .toolHeartbeat(id, tool, elapsedMs):
            let accepted = acceptTurnEvent(event)
            Self.turnLog.debug(
                "tool heartbeat id=\(id, privacy: .public) name=\(tool, privacy: .public) elapsed_ms=\(elapsedMs, privacy: .public) turn=\(self.currentTurnId ?? 0, privacy: .public) accepted=\(accepted, privacy: .public) cancelling=\(self.model.isCancelling, privacy: .public)"
            )
            guard accepted else { return }
            if ConversationExecutionParsing.isShellTool(tool) {
                upsertShellCard(id: id, create: {
                    ConversationShellCard(
                        sessionId: model.activeSessionId,
                        turnId: currentTurnId,
                        taskId: id,
                        command: tool
                    )
                }, mutate: { card in
                    card.durationMs = elapsedMs
                })
                upsertTool(id: id, tool: "Shell", fallbackSummary: nil) { trace in
                    trace.tool = "Shell"
                    trace.status = .running
                    trace.elapsedMs = elapsedMs
                }
                model.statusLine = String(localized: "chat_shell_running")
                model.updateMainAgent(status: "working", latestActivity: model.statusLine)
            } else {
                upsertTool(id: id, tool: tool, fallbackSummary: nil) { trace in
                    trace.tool = tool
                    trace.status = .running
                    trace.elapsedMs = elapsedMs
                }
                model.statusLine = String(localized: "chat_tool_running \(tool)")
                model.updateMainAgent(status: "working", latestActivity: model.statusLine)
            }

        case let .toolUseResult(id, tool, resultJson, isError, display):
            let accepted = acceptTurnEvent(event)
            Self.turnLog.debug(
                "tool result id=\(id, privacy: .public) name=\(tool, privacy: .public) is_error=\(isError, privacy: .public) turn=\(self.currentTurnId ?? 0, privacy: .public) accepted=\(accepted, privacy: .public)"
            )
            guard accepted else { return }
            // The pre-derived `⎿` block (headline / diff / clamped body).
            // `summarizeToolResult` below stays only as the older-engine path.
            let derivedDisplay = display.map(Self.resultDisplay(from:))
            let wasCancelled = ConversationExecutionParsing.isCancellationResult(resultJson)
            if ConversationExecutionParsing.isShellTool(tool) {
                let finished = ConversationExecutionParsing.shellFinished(id: id, resultJson: resultJson, isError: isError)
                let resolvedShellStatus: ConversationShellStatus = wasCancelled
                    ? .cancelled
                    : (finished?.status ?? (isError ? .failed : .completed))
                upsertShellCard(id: id, create: {
                    ConversationShellCard(
                        sessionId: model.activeSessionId,
                        turnId: currentTurnId,
                        taskId: id,
                        command: tool
                    )
                }, mutate: { card in
                    if let finished {
                        card.stdout = finished.stdout
                        card.stderr = finished.stderr
                        card.exitCode = finished.exitCode
                        card.durationMs = finished.durationMs ?? card.durationMs
                        card.status = resolvedShellStatus
                        card.truncated = finished.truncated
                    } else {
                        card.status = resolvedShellStatus
                    }
                })
                upsertTool(id: id, tool: "Shell", fallbackSummary: nil) { trace in
                    trace.tool = "Shell"
                    trace.status = resolvedShellStatus.asToolStatus
                    trace.display = derivedDisplay
                    trace.questionAnswers = isError ? nil : ConversationAnsweredQuestion.parse(tool: tool, json: resultJson)
                    trace.spawnedAgentID = ConversationExecutionParsing.spawnedAgentID(tool: tool, resultJson: resultJson)
                    trace.outputSummary = ConversationExecutionParsing.summarizeToolResult(resultJson, isError: isError, tool: tool)
                    trace.elapsedMs = finished?.durationMs ?? trace.elapsedMs
                }
                model.statusLine = ConversationExecutionParsing.shellStatusLabel(resolvedShellStatus)
                model.updateMainAgent(status: "working", latestActivity: model.statusLine)
            } else {
                upsertTool(id: id, tool: tool, fallbackSummary: nil) { trace in
                    trace.tool = tool
                    trace.status = wasCancelled ? .cancelled : (isError ? .failed : .completed)
                    trace.display = derivedDisplay
                    trace.questionAnswers = isError ? nil : ConversationAnsweredQuestion.parse(tool: tool, json: resultJson)
                    trace.spawnedAgentID = ConversationExecutionParsing.spawnedAgentID(tool: tool, resultJson: resultJson)
                    trace.outputSummary = ConversationExecutionParsing.summarizeToolResult(
                        resultJson,
                        isError: isError,
                        tool: tool
                    )
                }
                model.statusLine = wasCancelled
                    ? String(localized: "chat_tool_cancelled \(tool)")
                    : (isError ? String(localized: "chat_tool_failed \(tool)") : String(localized: "chat_tool_completed \(tool)"))
                model.updateMainAgent(status: "working", latestActivity: model.statusLine)
            }
            if !isError,
               tool.trimmingCharacters(in: .whitespacesAndNewlines)
                .caseInsensitiveCompare("Workflow") == .orderedSame {
                // Workflow returns as soon as the background task is
                // launched. Pull its registry row now; later terminal
                // transitions arrive through TaskStatusChanged.
                refreshBackgroundTasks()
            }

        case let .usageUpdate(_, inputTokens, outputTokens, cacheReadTokens, cacheCreationTokens):
            guard acceptTurnEvent(event) else { return }
            updateActiveRun {
                $0.usage = ConversationUsageSnapshot(
                    inputTokens: inputTokens,
                    outputTokens: outputTokens,
                    cacheReadTokens: cacheReadTokens,
                    cacheCreationTokens: cacheCreationTokens
                )
            }

        case let .apiRetry(message, attempt, maxRetries, delayMs):
            guard acceptTurnEvent(event) else { return }
            updateActiveRun {
                $0.retry = ConversationRetrySnapshot(
                    message: message,
                    attempt: attempt,
                    maxRetries: maxRetries,
                    delayMs: delayMs
                )
            }
            model.statusLine = String(localized: "chat_retrying \(attempt) \(maxRetries)")

        case let .costUpdate(_, _, _, _, _, formatted):
            guard acceptTurnEvent(event) else { return }
            updateActiveRun { $0.costFormatted = formatted }

        case let .compactionStatus(phase, error):
            guard acceptTurnEvent(event) else { return }
            model.compactionStatus = .reducing(model.compactionStatus, phase: phase, error: error)

        case let .compactionCompleted(messagesBefore, messagesAfter, bytesSaved, _):
            guard acceptTurnEvent(event) else { return }
            model.compactionStatus = .completed(
                messagesBefore: messagesBefore,
                messagesAfter: messagesAfter,
                bytesSaved: bytesSaved
            )
            updateActiveRun {
                $0.compactions.append(
                    ConversationCompactionSnapshot(
                        messagesBefore: messagesBefore,
                        messagesAfter: messagesAfter,
                        bytesSaved: bytesSaved
                    )
                )
            }

        case let .coordinatorStatus(activeWorkers, team):
            guard acceptTurnEvent(event) else { return }
            updateCoordinatorStatus(activeWorkers: activeWorkers, team: team)

        case let .coordinatorWorker(worker):
            guard acceptTurnEvent(event) else { return }
            // Older engines expose coordinator workers before the
            // session-agent listing exists. Keep those rows selectable as
            // read-only agents so the picker degrades gracefully.
            model.upsertAgentSummary(ConversationAgentSummary(
                id: worker.agentId,
                name: worker.name,
                agentType: worker.agentType,
                status: worker.status,
                latestActivity: model.statusLine
            ))
            updateActiveRun { run in
                let viewModel = ConversationCoordinatorWorker(
                    id: worker.agentId,
                    name: worker.name,
                    agentType: worker.agentType,
                    status: worker.status
                )
                if let index = run.workers.firstIndex(where: { $0.id == viewModel.id }) {
                    run.workers[index] = viewModel
                } else {
                    run.workers.append(viewModel)
                }
            }

        case let .messageIdentity(messageId):
            guard acceptTurnEvent(event) else { return }
            pendingAssistantIdentity = messageId

        case let .messageRetracted(messageId):
            guard acceptTurnEvent(event) else { return }
            let rows = assistantRowsByIdentity.removeValue(forKey: messageId) ?? []
            let reasoning = assistantReasoningByIdentity.removeValue(forKey: messageId) ?? []
            guard !rows.isEmpty || !reasoning.isEmpty else { return }
            let liveID = streamingIndex.flatMap { model.messages.indices.contains($0) ? model.messages[$0].id : nil }
            model.messages.removeAll { rows.contains($0.id) && $0.role == .ai }
            model.items.removeAll {
                if case let .message(message) = $0 { return rows.contains(message.id) && message.role == .ai }
                return false
            }
            for id in rows { model.messageDetails.removeValue(forKey: id) }
            for index in model.items.indices {
                guard case var .run(run) = model.items[index] else { continue }
                run.activities.removeAll {
                    switch $0 {
                    case let .textBoundary(_, messageID): return rows.contains(messageID)
                    case let .reasoning(id, _): return reasoning.contains(id)
                    default: return false
                    }
                }
                run.reasoning = run.activities.compactMap {
                    if case let .reasoning(_, text) = $0 { return text }
                    return nil
                }.joined()
                model.items[index] = .run(run)
            }
            streamingIndex = liveID.flatMap { model.indexOfMessage(id: $0) }
            streamingItemIndex = liveID.flatMap { model.indexOfMessageItem(id: $0) }
            activeRunItemIndex = model.items.lastIndex { if case .run = $0 { return true }; return false }

        case let .visualizationBlock(status, reference):
            guard acceptTurnEvent(event) else { return }
            applyVisualizationBlock(status: status, reference: reference)

        case let .messageComplete(_, message):
            guard acceptTurnEvent(event) else { return }
            // A placeholder whose reference line never completed is gone.
            for id in currentResponseMessageIDs {
                if let index = model.indexOfMessage(id: id),
                   model.messages[index].visualization?.status == .pending {
                    removeMessage(id: id)
                }
            }
            currentResponseMessageIDs.removeAll { model.indexOfMessage(id: $0) == nil }
            if let message {
                let segments = Self.narrativeSegments(from: message)
                // If an out-of-band notice split streamed text but does not
                // appear in MessageDto, the final payload can contain fewer
                // narrative segments than the ledger. In that case retain
                // the already-correct streamed rows instead of merging and
                // duplicating their suffix.
                if segments.count >= currentResponseMessageIDs.count {
                    for (index, segment) in segments.enumerated() {
                        if currentResponseMessageIDs.indices.contains(index) {
                            replaceMessage(
                                id: currentResponseMessageIDs[index],
                                with: segment.message,
                                detail: segment.detail
                            )
                        } else {
                            appendMessage(segment.message, detail: segment.detail)
                            currentResponseMessageIDs.append(segment.message.id)
                            appendTextBoundaryActivity(messageID: segment.message.id)
                        }
                    }
                }
            }
            // One MessageComplete closes one assistant API response. Tool
            // execution may continue the same user turn, but the next text
            // belongs to a new narrative row after those tool activities.
            if let identity = pendingAssistantIdentity {
                assistantRowsByIdentity[identity] = Set(currentResponseMessageIDs)
                assistantReasoningByIdentity[identity] = currentResponseReasoningIDs
            }
            pendingAssistantIdentity = nil
            currentResponseReasoningIDs = []
            streamingIndex = nil
            streamingItemIndex = nil
            currentResponseMessageIDs = []

        case let .turnEnded(outcome, _, _):
            let accepted = acceptTurnEvent(event)
            Self.turnLog.debug(
                "turn ended turn=\(self.currentTurnId ?? 0, privacy: .public) outcome=\(String(describing: outcome), privacy: .public) accepted=\(accepted, privacy: .public)"
            )
            guard accepted else { return }
            let awaitingCancellationTerminal = cancellationOperation.map {
                $0.turnId == currentTurnId
            } ?? false
            if let currentTurnId {
                if !awaitingCancellationTerminal {
                    durableTurns.clear(turnID: currentTurnId)
                    if unresolvedRecoveryTurnID == currentTurnId {
                        unresolvedRecoveryTurnID = nil
                        needsDurableForegroundRecoveryTurnID = nil
                        model.hasUnresolvedTurnRecovery = false
                    }
                    reattachedTurnIDs.remove(currentTurnId)
                    model.hasInactiveDurableRecovery = false
                }
            }
            // PR-4 item 3: don't treat every outcome as a clean end. A normal
            // `endTurn` just stops streaming; `maxTurns` / `cancelled` surface
            // a distinct notice so the user knows the turn was interrupted.
            model.streaming = false
            if !model.isCancelling { model.statusLine = nil }
            if awaitingCancellationTerminal {
                // TurnEnded is not the cancellation proof. Settle the
                // correlated run for rendering, but retain its durable
                // owner, pointers, and cancellation gate until the FIFO
                // observes TurnRecoveryState.cancelled.
                finishActiveRun(.cancelled)
                model.updateMainAgent(status: "cancelling", latestActivity: String(localized: "chat_stopping"))
                return
            }
            switch outcome {
            case .endTurn:
                model.notice = nil
                finishActiveRun(.completed)
                publishActiveTurnCompletion(.completed)
            case .maxTurns:
                model.notice = .maxTurns
                finishActiveRun(.maxTurns)
                publishActiveTurnCompletion(.maxTurns)
            case .cancelled:
                model.notice = .cancelled
                finishActiveRun(.cancelled)
                publishActiveTurnCompletion(.cancelled)
            @unknown default:
                // `#[non_exhaustive]` — a future outcome falls back to a clean
                // visual end rather than crashing, but remains non-speakable
                // until the client explicitly understands its semantics.
                model.notice = nil
                finishActiveRun(.completed)
                publishActiveTurnCompletion(.failed)
            }
            model.updateMainAgent(
                status: outcome == .cancelled ? "cancelled" : "idle",
                latestActivity: model.notice?.text ?? String(localized: "chat_completed")
            )
            clearTurnPointers(keepEpoch: false)
            requestSessionCatalogRefreshAfterSettledTurn()

        case let .turnRecoveryState(snapshot):
            guard snapshot.sessionId == model.activeSessionId else { return }
            let isCorrelated = snapshot.turnId == currentTurnId
                || snapshot.turnId == unresolvedRecoveryTurnID
                || snapshot.turnId == replayingTurnID
            if isCorrelated {
                replayProjectionState = snapshot.state
                if Self.isTerminalRecoveryState(snapshot.state) {
                    model.hasUnresolvedTurnRecovery = false
                }
            }
            switch snapshot.state {
            case .running:
                // ResumeTurn's acknowledgement is the point at which the
                // durable slot is live again. Clear the recovery gate only
                // for the correlated turn; a delayed state from an older
                // session must not unlock the composer.
                guard isCorrelated else { return }
                let isAttachSnapshot: Bool = {
                    guard case let .attaching(turnID) = durableRecoveryPhase else {
                        return false
                    }
                    return turnID == snapshot.turnId
                }()
                if isAttachSnapshot {
                    // AttachTurn can report Running while the executor is
                    // merely reconnected. ResumeTurn is still required;
                    // exposing this as active would let a send overwrite
                    // the sole durable checkpoint if ResumeTurn fails.
                    unresolvedRecoveryTurnID = snapshot.turnId
                    model.hasUnresolvedTurnRecovery = true
                    model.hasInactiveDurableRecovery = true
                    executorOwnedTurnID = nil
                    model.streaming = false
                    return
                }
                if case let .resuming(turnID) = durableRecoveryPhase,
                   turnID == snapshot.turnId {
                    durableRecoveryPhase = nil
                }
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    unresolvedRecoveryTurnID = nil
                    needsDurableForegroundRecoveryTurnID = nil
                    model.hasInactiveDurableRecovery = false
                    model.hasUnresolvedTurnRecovery = false
                }
                executorOwnedTurnID = snapshot.turnId
                currentTurnId = snapshot.turnId
                activeTurnEpoch = sessionEpoch
                model.activeTurnToken = ConversationTurnToken(
                    clientTurnId: snapshot.turnId,
                    sessionEpoch: sessionEpoch
                )
                model.streaming = true
            case .waitingForUser:
                // WaitingForUser is still a live durable checkpoint. Keep
                // it correlated and non-sendable until the engine reports
                // Running after the user resolves its question/permission.
                // A live executor-backed question remains an ordinary
                // active turn (Stop, not Discard). Only a post-reattach
                // executor-less checkpoint is inactive in the UI.
                guard isCorrelated else { return }
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    unresolvedRecoveryTurnID = snapshot.turnId
                    model.hasUnresolvedTurnRecovery = true
                    let hasLiveExecutor = executorOwnedTurnID == snapshot.turnId
                        && replayingTurnID != snapshot.turnId
                        && durableRecoveryTask == nil
                    model.hasInactiveDurableRecovery = !hasLiveExecutor
                    if durableRecoveryTask != nil {
                        // The attach chain has already attempted ResumeTurn;
                        // a returned WaitingForUser now needs explicit user
                        // input and must not be retried on every foreground.
                        needsDurableForegroundRecoveryTurnID = nil
                    }
                    if let token = activeConversationTurnToken {
                        resolvePauseAcknowledgements(
                            token: token,
                            result: .failure(PauseAcknowledgementError.recoveryState(snapshot.state))
                        )
                    }
                    if !hasLiveExecutor {
                        settleCorrelatedMainRun()
                    }
                    model.streaming = hasLiveExecutor
                }
                model.statusLine = String(localized: "chat_background_waiting_text")
            case .pausedRecoverable:
                guard isCorrelated else { return }
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    unresolvedRecoveryTurnID = snapshot.turnId
                    model.hasUnresolvedTurnRecovery = true
                    executorOwnedTurnID = nil
                    model.hasInactiveDurableRecovery = true
                    // If this is the warm-source pause acknowledgement there
                    // is no recovery chain yet, so allow the next foreground
                    // callback to issue one AttachTurn. An already-running
                    // attach keeps its id in the set until its ResumeTurn
                    // acknowledgement, preventing duplicate submissions.
                    if durableRecoveryTask == nil {
                        needsDurableForegroundRecoveryTurnID = snapshot.turnId
                        reattachedTurnIDs.remove(snapshot.turnId)
                    }
                    settleCorrelatedMainRun()
                }
                model.streaming = false
                model.statusLine = String(localized: "chat_background_paused_text")
                if let token = activeConversationTurnToken,
                   token.clientTurnId == snapshot.turnId {
                    resolvePauseAcknowledgements(token: token, result: .success(()))
                }
            case .completed:
                guard isCorrelated else { return }
                if cancellationOperation?.turnId == snapshot.turnId {
                    terminalRecoveryStates[snapshot.turnId] = snapshot.state
                }
                if snapshot.turnId == unresolvedRecoveryTurnID
                    || snapshot.turnId == currentTurnId
                    || snapshot.turnId == replayingTurnID {
                    model.hasInactiveDurableRecovery = false
                }
                executorOwnedTurnID = nil
                if let token = activeConversationTurnToken,
                   token.clientTurnId == snapshot.turnId {
                    resolvePauseAcknowledgements(
                        token: token,
                        result: .failure(PauseAcknowledgementError.recoveryState(snapshot.state))
                    )
                }
                if snapshot.turnId == unresolvedRecoveryTurnID {
                    unresolvedRecoveryTurnID = nil
                    needsDurableForegroundRecoveryTurnID = nil
                    model.hasUnresolvedTurnRecovery = false
                }
                model.streaming = false
                durableTurns.clear(turnID: snapshot.turnId)
                reattachedTurnIDs.remove(snapshot.turnId)
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    clearTurnPointers(keepEpoch: false)
                }
            case .failed:
                guard isCorrelated else { return }
                if cancellationOperation?.turnId == snapshot.turnId {
                    terminalRecoveryStates[snapshot.turnId] = snapshot.state
                }
                if snapshot.turnId == unresolvedRecoveryTurnID
                    || snapshot.turnId == currentTurnId
                    || snapshot.turnId == replayingTurnID {
                    model.hasInactiveDurableRecovery = false
                }
                executorOwnedTurnID = nil
                if let token = activeConversationTurnToken,
                   token.clientTurnId == snapshot.turnId {
                    resolvePauseAcknowledgements(
                        token: token,
                        result: .failure(PauseAcknowledgementError.recoveryState(snapshot.state))
                    )
                }
                if snapshot.turnId == unresolvedRecoveryTurnID {
                    unresolvedRecoveryTurnID = nil
                    needsDurableForegroundRecoveryTurnID = nil
                    model.hasUnresolvedTurnRecovery = false
                }
                model.streaming = false
                durableTurns.clear(turnID: snapshot.turnId)
                reattachedTurnIDs.remove(snapshot.turnId)
                model.error = ConversationError(
                    kind: .host,
                    message: snapshot.reason ?? String(localized: "chat_background_failed_text")
                )
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    clearTurnPointers(keepEpoch: false)
                }
            case .cancelled:
                guard isCorrelated else { return }
                if cancellationOperation?.turnId == snapshot.turnId {
                    terminalRecoveryStates[snapshot.turnId] = snapshot.state
                }
                if snapshot.turnId == unresolvedRecoveryTurnID
                    || snapshot.turnId == currentTurnId
                    || snapshot.turnId == replayingTurnID {
                    model.hasInactiveDurableRecovery = false
                }
                executorOwnedTurnID = nil
                if let token = activeConversationTurnToken,
                   token.clientTurnId == snapshot.turnId {
                    resolvePauseAcknowledgements(
                        token: token,
                        result: .failure(PauseAcknowledgementError.recoveryState(snapshot.state))
                    )
                }
                if snapshot.turnId == unresolvedRecoveryTurnID {
                    unresolvedRecoveryTurnID = nil
                    needsDurableForegroundRecoveryTurnID = nil
                    model.hasUnresolvedTurnRecovery = false
                }
                model.streaming = false
                model.notice = .cancelled
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    finishActiveRun(.cancelled)
                    publishActiveTurnCompletion(.cancelled)
                }
                durableTurns.clear(turnID: snapshot.turnId)
                reattachedTurnIDs.remove(snapshot.turnId)
                if snapshot.turnId == currentTurnId || snapshot.turnId == replayingTurnID {
                    clearTurnPointers(keepEpoch: false)
                }
            @unknown default:
                break
            }

        case let .turnEventReplay(sessionId, turnId, sequence, eventJson):
            guard sessionId == model.activeSessionId else { return }
            // SessionResumed carries the authoritative terminal transcript.
            // Attach still replays from cursor zero after a cold launch, but
            // terminal envelopes must not project that same assistant/tool
            // content a second time. Nonterminal recovery keeps the retained
            // suffix so partial work remains visible.
            if replayingTurnID == turnId,
               replayProjectionState.map({ !Self.isTerminalRecoveryState($0) }) ?? true {
                applyRetainedTurnEvent(eventJson, turnID: turnId)
            }
            durableTurns.updateSequence(turnID: turnId, sequence: sequence)

        case let .error(kind, message):
            let accepted = acceptTurnEvent(event)
            Self.turnLog.error(
                "turn error turn=\(self.currentTurnId ?? 0, privacy: .public) kind=\(String(describing: kind), privacy: .public) accepted=\(accepted, privacy: .public) message=\(message, privacy: .private(mask: .hash))"
            )
            Self.turnLog.error(
                "turn error details kind=\(String(describing: kind), privacy: .public) message=\(message, privacy: .public)"
            )
            print("[LingxiCode] turn error kind=\(kind) accepted=\(accepted) message=\(message)")
            guard accepted else { return }
            if model.compactionStatus?.isActive == true,
               message.lowercased().hasPrefix("force_compact failed:") {
                let detail = message
                    .replacingOccurrences(
                        of: "^force_compact failed:\\s*", with: "",
                        options: [.regularExpression, .caseInsensitive]
                    )
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .replacingOccurrences(
                        of: "^handle action failed:\\s*",
                        with: "",
                        options: [.regularExpression, .caseInsensitive]
                    )
                model.compactionStatus = .failed(detail: detail.isEmpty ? message : detail)
            }
            if case let .resuming(taskID) = model.workflowResumeState {
                model.workflowResumeState = .failed(taskID: taskID, message: message)
            }
            // PR-4 item 4: a terminal error is a persistent, kind-aware banner.
            finishActiveRun(.failed)
            model.updateMainAgent(status: "failed", latestActivity: message)
            fail(Self.kind(from: kind), message)

        case let .modelList(models, current, details):
            // Out-of-band model catalog (SHIP-BLOCKER #2). Drive the picker off
            // these REAL engine ids and adopt the engine's reported active model
            // — not a branded mock default.
            model.availableModels = models
            model.availableModelDetails = Dictionary(
                uniqueKeysWithValues: details.map { ($0.reference, Self.lowerModelRuntimeDetails($0)) }
            )
            applyActiveModel(current)

        case let .providerModelCatalog(providers):
            providerCatalogEntries = providers.map(Self.lowerProviderCatalogEntry)
            providerCatalogLoaded = true
            let waiters = providerCatalogWaiters
            providerCatalogWaiters.removeAll()
            for waiter in waiters {
                waiter.resume(returning: providerCatalogEntries)
            }

        case let .modelChanged(model: newModel):
            // Reflect the authoritative model, including slash-command changes.
            applyActiveModel(newModel)

        case let .fastModeChanged(enabled):
            model.fastMode = enabled
            model.fastModePending = false
            model.fastModeError = nil

        case let .conversationControlsChanged(controls):
            applyControlsSnapshot(controls)

        case let .permissionModeChanged(mode: mode):
            // The engine is authoritative: auto may be downgraded by the
            // active model/provider/killswitch gate, and a rejected live
            // change must never leave the settings page claiming success.
            model.effectivePermissionMode = mode
            // The following controls snapshot carries the requested value;
            // do not infer it from this legacy effective-only event.

        case let .typescriptLspModeChanged(requested: requested, effective: effective, available: available):
            model.requestedTypescriptLspMode = requested
            model.effectiveTypescriptLspMode = effective
            model.typescriptLspAvailable = available

        case let .sessionList(sessions):
            // Out-of-band session catalog: map each lowered `SessionRowDto`
            // to the UI model (uuid/title/count + RFC 3339 → relative time).
            // Drives the drawer off REAL history; an empty list lets the
            // drawer fall back to the mock lists.
            model.engineSessions = sessions.map {
                EngineSession(id: $0.uuid,
                              title: $0.title,
                              mode: SessionMode(dto: $0.mode),
                              messageCount: Int($0.messageCount),
                              modifiedAt: RelativeTime.parse($0.modifiedRfc3339),
                              relativeTime: RelativeTime.format($0.modifiedRfc3339))
            }
            // Publish the authoritative rows before flipping the loaded
            // bit. `ConversationProjectBridge` persists on the loaded
            // transition; reversing these assignments creates a crash
            // window where it can durably replace a valid index with an
            // intermediate empty value.
            model.engineSessionsLoaded = true

        case let .sessionAgentList(sessionId, agents):
            // Ignore a delayed listing from a previous session. The agent
            // picker is strictly scoped to the active session.
            guard sessionId == model.activeSessionId,
                  !model.sessionTransitionPending else { return }
            model.replaceAgentSummaries(agents.map { Self.agentSummary(from: $0) })

        case let .sessionAgentUpdated(sessionId, agent):
            guard sessionId == model.activeSessionId,
                  !model.sessionTransitionPending else { return }
            model.upsertAgentSummary(Self.agentSummary(from: agent))

        case let .sessionAgentTranscript(sessionId, agentId, messages, nextMessageIndex, revision):
            guard sessionId == model.activeSessionId,
                  !model.sessionTransitionPending else { return }
            applyAgentTranscript(
                sessionID: sessionId,
                agentId: agentId,
                rows: messages,
                nextMessageIndex: nextMessageIndex,
                revision: revision
            )

        case let .sessionAgentMessage(sessionId, agentId, row):
            guard sessionId == model.activeSessionId,
                  !model.sessionTransitionPending else { return }
            appendAgentMessage(
                agentId: agentId,
                message: row.message,
                messageIndex: row.messageIndex
            )

        case let .sessionStarted(sessionId, mode):
            guard SessionMode(dto: mode) == sessionMode else { return }
            // A fresh session began on the connection (1:1 with a successful
            // `NewSession`). Adopt it as active. Reset the transcript ONLY
            // when this is genuinely a new id AND no turn is streaming — so an
            // unexpected `SessionStarted` (e.g. one emitted for the initial
            // session at connect time) can never wipe a live conversation.
            // `startNewConversation` already reset locally for the user-driven
            // case; this confirms + adopts the engine-assigned id.
            let isSwitch = !sessionId.isEmpty && sessionId != model.activeSessionId
            let confirmedNewSession = pendingSessionTransition == .new
            let hadVisibleTranscript = !model.items.isEmpty || !model.messages.isEmpty
            // Resume is confirmed only by SessionResumed, because that event
            // atomically carries the replacement transcript. A bootstrap
            // SessionStarted must neither rebind the visible rows nor unblock
            // the composer while that replay is pending.
            if case .resume = pendingSessionTransition {
                return
            }
            // With no explicit NewSession transition, a different id is a
            // delayed/bootstrap event. Keep both the visible transcript and
            // its owning session id together instead of publishing mismatched
            // state (rows from A while commands target B).
            if isSwitch && hadVisibleTranscript && !confirmedNewSession {
                return
            }
            if isSwitch && !model.streaming
                && (confirmedNewSession || !hadVisibleTranscript) {
                resetTranscriptForSessionSwitch(isNew: true)
            }
            model.activeSessionId = sessionId
            hasConfirmedSessionState = true
            setPendingSessionTransition(nil)
            if confirmedNewSession {
                model.sessionRefreshRevision &+= 1
            }
            refreshSessionAgentsAfterTransition()

        case let .sessionResumed(sessionId, mode, messages):
            guard SessionMode(dto: mode) == sessionMode else { return }
            // A prior session was resumed (1:1 with a successful
            // `ResumeSession`). Adopt it as active AND surface the restored
            // transcript the engine just hot-loaded into the running
            // orchestrator, so the scrollback shows the prior conversation
            // and the user can see exactly the context the next turn will
            // continue from. `messages` is OLDEST-FIRST and always present
            // (may be empty for a zero-message session). We clear the
            // placeholder transcript `resumeSession` left in place and append
            // each restored message as a completed bubble — the out-of-band
            // session-state sibling of `SessionList` / `SessionStarted`.
            guard pendingSessionTransition == .resume(sessionId) else { return }
            invalidateTurnContext()
            replayProjectionState = nil
            model.clearAgentState()
            model.activeSessionId = sessionId
            hasConfirmedSessionState = true
            setPendingSessionTransition(nil)
            // BUG FIX: a restored message is NOT one render row. In the
            // Anthropic protocol a `tool_result` block lives in the USER
            // turn, so mapping each `MessageDto` to a single `.message`
            // rendered every restored tool result as a right-aligned user
            // bubble — and `MessageBubble`'s user branch draws only
            // `message.text`, never `detail`, so the payload was invisible
            // too. `restoredTranscript` interleaves instead: narrative
            // blocks stay a `.message`, each tool call becomes its own
            // `.toolCall` row with its result merged onto it.
            let restored = Self.restoredTranscript(
                from: messages,
                sessionID: sessionId
            )
            model.messages = restored.messages
            model.items = restored.items
            model.messageDetails = restored.details
            model.restoreTranscriptDisclosures()
            model.backgroundTasks = []
            model.planTasks = []
            model.workflowResumeState = .idle
            model.isNew = messages.isEmpty
            model.streaming = false
            model.turnCompletion = nil
            model.statusLine = nil
            model.notice = nil
            // A migration-safe empty resume may have just created its JSONL
            // anchor. Refresh so the full catalog and persisted project index
            // immediately include that preserved UUID.
            model.sessionRefreshRevision &+= 1
            refreshSessionAgentsAfterTransition()
            refreshBackgroundTasks()
            reattachDurableTurnIfNeeded(sessionID: sessionId)

        case .sessionEnded:
            // The current session ended (e.g. cleared). Drop the active id;
            // the next `SessionStarted`/`SessionResumed` re-establishes one.
            model.activeSessionId = ""
            hasConfirmedSessionState = false
            model.clearAgentState()
            model.backgroundTasks = []
            model.workflowResumeState = .idle
            setPendingSessionTransition(nil)
            invalidateTurnContext()
            model.turnCompletion = nil
            // A pending questionnaire belongs to the ended session; its
            // request id can never be answered now.
            model.pendingQuestions = []
            // The plan belonged to that session too.
            model.planTasks = []

        case let .mcpServers(servers):
            // Out-of-band MCP listing → the UI `MCPServer` model. The wire
            // intentionally carries health/name/transport only; endpoint
            // and command details are merged from the real config file by
            // the settings host, never fabricated here.
            model.mcpServers = servers.map { dto in
                let status: ConnStatus
                switch dto.status {
                case .connected: status = .connected
                case .disconnected: status = .idle
                case .error: status = .error
                @unknown default: status = .idle
                }
                return MCPServer(id: dto.name, name: dto.name, url: nil, command: "",
                                 args: [], env: [:], headers: [:], tools: nil,
                                 status: status, enabled: status == .connected,
                                 transport: MCPServer.normalizedTransport(dto.transport))
            }
            model.mcpServersLoaded = true

        case let .slashCommandCatalog(commands), let .commandsChanged(commands):
            applySlashCommandCatalog(commands)

        default:
            // Cost / message-boundary / other listing events are not rendered
            // in this surface; ignored without breaking the stream.
            break
        }
    }

    /// Test seam (the iOS analog of Android's `reduceSessionEvent`): drive one
    /// inbound `ClientEvent` through the same `apply` reducer the live listener
    /// uses, so a unit test can assert the out-of-band session-state mapping
    /// (notably `SessionResumed` → restored transcript) without standing up the
    /// engine. `internal` so `@testable import LingxiCode` reaches it; the live
    /// path still goes through `apply` directly.
    func applyForTesting(_ event: ClientEvent) {
        apply(event)
    }

    /// Construct the real callback adapter for ordering tests without
    /// building a MobileEngineHandle.
    func makeEventListenerForTesting() -> EngineListener {
        EngineListener(source: self)
    }

    /// Establish the same correlation guard as a submitted ResumeSession
    /// without constructing an engine handle. Restored-transcript tests use
    /// this before injecting the authoritative SessionResumed reply.
    func expectSessionResumeForTesting(_ sessionID: String) {
        _ = beginSessionTransition(.resume(sessionID))
    }

    func applyWorkflowProgressForTesting(
        taskId: String,
        runId: String,
        progress: ConversationWorkflowProgressPayload
    ) {
        upsertWorkflowProgress(taskId: taskId, runId: runId, progress: progress)
    }

    func applyWorkflowProgressForTesting(
        originSessionId: String,
        taskId: String,
        runId: String,
        progress: ConversationWorkflowProgressPayload
    ) {
        guard !originSessionId.isEmpty,
              originSessionId == model.activeSessionId,
              !model.sessionTransitionPending
        else { return }
        upsertWorkflowProgress(taskId: taskId, runId: runId, progress: progress)
    }

    private func applySlashCommandCatalog(_ commands: [SlashCommandDto]) {
        model.slashCommands = commands.map { command in
            ConversationSlashCommand(
                name: command.name,
                description: command.description,
                aliases: command.aliases,
                argumentHint: command.argumentHint,
                menuDescription: command.menuDescription,
                source: command.source,
                hidden: command.hidden
            )
        }
        model.slashCommandsLoaded = true

        // Settings derives its skill rows from the same catalog rather
        // than maintaining another command list.
        model.skills = commands.map { command in
            let source = command.source
            let author: String
            switch source {
            case "builtin", "bundled": author = String(localized: "skills_author_official")
            case "user": author = String(localized: "skills_author_mine")
            default: author = command.source
            }
            return Skill(
                id: "skill:\(command.name)",
                name: command.name,
                author: author,
                desc: command.description,
                triggers: ["/\(command.name)"] + command.aliases.map { "/\($0)" },
                enabled: true,
                builtin: source == "builtin" || source == "bundled" || source == "managed",
                source: source
            )
        }
        model.skillsLoaded = true
    }

    func beginTurnForTesting(turnId: UInt64 = 1, sessionId: String = "test-session") {
        model.activeSessionId = sessionId
        model.streaming = true
        model.turnCompletion = nil
        model.activeTurnToken = ConversationTurnToken(
            clientTurnId: turnId,
            sessionEpoch: sessionEpoch
        )
        turnSpeechSequence = 0
        currentTurnId = turnId
        executorOwnedTurnID = turnId
        activeTurnEpoch = sessionEpoch
        nextTurnId = turnId == UInt64.max ? 1 : turnId + 1
    }

    func beginDurableTurnForTesting(turnId: UInt64, sessionId: String) {
        durableTurns.begin(sessionID: sessionId, turnID: turnId)
    }

    func recordDurableSequenceForTesting(turnId: UInt64, sequence: UInt64) {
        durableTurns.updateSequence(turnID: turnId, sequence: sequence)
    }

    func durableAttachCursorForTesting() -> UInt64? {
        durableTurns.load()?.lastSequence
    }

    func cancelForTesting() {
        guard model.streaming, currentTurnId != nil else { return }
        model.isCancelling = true
        model.statusLine = String(localized: "chat_stopping")
    }

    func setCommandSubmitterForTesting(_ submitter: ((ClientCommand) async throws -> Void)?) {
        testCommandSubmitter = submitter
    }

    func setBypassPermissionsConfirmerForTesting(
        _ confirmer: (() async throws -> Void)?
    ) {
        testBypassPermissionsConfirmer = confirmer
    }

    func setEmptySessionResumerForTesting(
        _ resumer: ((String, String) async throws -> Void)?
    ) {
        testEmptySessionResumer = resumer
    }

    /// Append streamed text into the in-flight assistant message, creating it
    /// on the first delta of a turn.
    ///
    /// PR-4 item 1 guard: only append when a turn is actually in flight. After
    /// a cancel/turn-end we drop `streaming`/`streamingIndex`, so a late delta
    /// arriving from the engine must NOT resurrect or corrupt a message.
    private func appendDelta(_ delta: String) -> UUID? {
        guard model.streaming else { return nil }
        if let i = streamingIndex,
           model.messages.indices.contains(i),
           activeRunEndsWithBoundary(for: model.messages[i].id) {
            let updated = Message(role: .ai,
                                  tag: model.messages[i].tag,
                                  text: model.messages[i].text + delta)
            replaceStreamingMessage(updated)
            return model.messages[i].id
        } else {
            let opened = Message(role: .ai, text: delta)
            appendMessage(opened)
            streamingIndex = model.messages.count - 1
            streamingItemIndex = model.items.count - 1
            currentResponseMessageIDs.append(opened.id)
            return opened.id
        }
    }

    private func activeRunEndsWithBoundary(for messageID: UUID) -> Bool {
        guard let itemIndex = activeRunItemIndex,
              model.items.indices.contains(itemIndex),
              case let .run(run) = model.items[itemIndex],
              case let .textBoundary(_, boundaryMessageID) = run.activities.last
        else { return false }
        return boundaryMessageID == messageID
    }

    private func publishTurnSpeechDelta(_ delta: String) {
        guard !delta.isEmpty, let token = activeConversationTurnToken else { return }
        turnSpeechSequence &+= 1
        model.turnSpeechUpdates.send(ConversationTurnSpeechUpdate(
            token: token,
            sequence: turnSpeechSequence,
            delta: delta
        ))
    }

    // MARK: restored-transcript lowering (live ResumeSession)

    /// Map one restored `MessageDto` (the engine's lowered transcript line,
    /// `SessionResumed.messages`) onto the UI `Message` the scrollback renders.
    ///
    /// The engine roles are `"user" | "assistant" | "system"`; the iOS
    /// `Message.role` is the binary user/AI split, so non-user roles render on
    /// the AI side. `Message.text` remains a compatibility summary, while the
    /// complete typed block list is retained in `ConversationMessageDetail`
    /// so restored reasoning, tool and compaction blocks keep their identity.
    fileprivate struct RenderedMessage {
        let message: Message
        let detail: ConversationMessageDetail?
    }

    // MARK: engine-derived tool presentation (lowering)
    //
    // Straight field-for-field lowering of the additive wire DTOs onto the
    // FFI-independent mirrors the views render. NOTHING here re-parses
    // `input_json` / `result_json`: the engine already derived all of it,
    // and a second derivation on this side is exactly the drift the wire
    // fields exist to delete.

    fileprivate static func toolHeader(from dto: ToolHeaderDto) -> ConversationToolHeader {
        ConversationToolHeader(
            verb: toolVerb(from: dto.verb),
            icon: dto.icon.map(toolIcon(from:)),
            label: dto.label,
            primary: dto.primary,
            qualifier: dto.qualifier,
            count: dto.count,
            subLine: dto.subLine.map {
                ConversationToolSubLine(prefix: $0.prefix, text: $0.text)
            },
            title: dto.title
        )
    }

    private static func toolVerb(from dto: ToolVerbDto) -> ConversationToolVerb {
        switch dto {
        case .update: return .update
        case .create: return .create
        case .read: return .read
        case .search: return .search
        case .shell: return .shell
        case .output: return .output
        case .kill: return .kill
        case .fetch: return .fetch
        case .task: return .task
        case .todo: return .todo
        case .skill: return .skill
        case .generic: return .generic
        @unknown default:
            // `#[non_exhaustive]`: a verb this build does not know still
            // renders — as the engine's English label, via `.generic`.
            return .generic
        }
    }

    private static func toolIcon(from dto: ToolIconDto) -> ConversationToolIcon {
        switch dto {
        case .read: return .read
        case .search: return .search
        case .list: return .list
        case .edit: return .edit
        case .terminal: return .terminal
        case .globe: return .globe
        case .workflow: return .workflow
        case .listChecks: return .listChecks
        case .sparkles: return .sparkles
        case .plug: return .plug
        case .output: return .output
        case .stop: return .stop
        case .wrench: return .wrench
        @unknown default: return .wrench
        }
    }

    fileprivate static func resultDisplay(
        from dto: ToolResultDisplayDto
    ) -> ConversationToolResultDisplay {
        ConversationToolResultDisplay(
            headline: dto.headline,
            headlineKind: dto.headlineKind.map(headlineKind(from:)),
            headlineArgs: dto.headlineArgs,
            diff: dto.diff.map(structuredDiff(from:)),
            body: dto.body,
            bodyLines: dto.bodyLines,
            bodyTruncated: dto.bodyTruncated,
            collapsed: dto.collapsed
        )
    }

    private static func headlineKind(
        from dto: HeadlineKindDto
    ) -> ConversationResultHeadlineKind {
        switch dto {
        case .added: return .added
        case .removed: return .removed
        case .addedRemoved: return .addedRemoved
        case .linesRead: return .linesRead
        case .linesReadPartial: return .linesReadPartial
        case .filesFound: return .filesFound
        case .filesFoundTruncated: return .filesFoundTruncated
        case .linesFound: return .linesFound
        case .matchesFound: return .matchesFound
        case .interrupted: return .interrupted
        case .noContent: return .noContent
        case .failed: return .failed
        case .plain: return .plain
        @unknown default:
            // An unknown kind falls back to the engine's own English
            // `headline` string, which `.plain` renders verbatim.
            return .plain
        }
    }

    private static func structuredDiff(
        from dto: StructuredDiffDto
    ) -> ConversationStructuredDiff {
        ConversationStructuredDiff(
            filePath: dto.filePath,
            language: dto.language,
            gutterWidth: dto.gutterWidth,
            additions: dto.additions,
            removals: dto.removals,
            truncatedRows: dto.truncatedRows,
            rows: dto.rows.map(diffRow(from:))
        )
    }

    private static func diffRow(from dto: DiffRowDto) -> ConversationDiffRow {
        ConversationDiffRow(
            kind: diffLineKind(from: dto.kind),
            lineNo: dto.lineNo,
            hunk: dto.hunk,
            wordDiffed: dto.wordDiffed,
            segments: dto.segments.map(codeSegment(from:))
        )
    }

    private static func diffLineKind(
        from dto: DiffLineKindDto
    ) -> ConversationDiffLineKind {
        switch dto {
        case .add: return .add
        case .remove: return .remove
        case .context: return .context
        @unknown default: return .context
        }
    }

    private static func codeSegment(from dto: CodeSegmentDto) -> ConversationCodeSegment {
        ConversationCodeSegment(
            text: dto.text,
            // `class` is a Swift keyword; the generated property is backticked.
            syntax: syntaxClass(from: dto.`class`),
            rgb: dto.rgb,
            bold: dto.bold,
            italic: dto.italic,
            underline: dto.underline,
            emph: dto.emph
        )
    }

    private static func syntaxClass(from dto: SyntaxClassDto) -> ConversationSyntaxClass {
        switch dto {
        case .plain: return .plain
        case .keyword: return .keyword
        case .typeName: return .typeName
        case .function: return .function
        case .stringLit: return .stringLit
        case .number: return .number
        case .comment: return .comment
        case .punctuation: return .punctuation
        // `operator` is a Swift keyword; the generated case is backticked.
        case .`operator`: return .op
        case .variable: return .variable
        case .constant: return .constant
        case .attribute: return .attribute
        @unknown default: return .plain
        }
    }

    fileprivate static func planTask(from dto: PlanTaskDto) -> ConversationPlanTask {
        ConversationPlanTask(
            taskId: dto.id,
            subject: dto.subject,
            activeForm: dto.activeForm,
            state: planTaskState(from: dto.state)
        )
    }

    private static func planTaskState(
        from dto: PlanTaskStateDto
    ) -> ConversationPlanTaskState {
        switch dto {
        case .pending: return .pending
        case .inProgress: return .inProgress
        case .completed: return .completed
        @unknown default: return .pending
        }
    }

    /// Lower one wire `AskUserQuestionRequestDto` onto the FFI-independent
    /// model the chat surface renders.
    fileprivate static func pendingQuestion(
        from dto: AskUserQuestionRequestDto
    ) -> ConversationPendingQuestion {
        ConversationPendingQuestion(
            requestId: dto.requestId,
            questions: dto.questions.map { question in
                ConversationAskQuestion(
                    question: question.question,
                    header: question.header,
                    options: question.options.map { option in
                        ConversationAskOption(
                            label: option.label,
                            description: option.description,
                            preview: option.preview
                        )
                    },
                    multiSelect: question.multiSelect
                )
            },
            timeoutSecs: dto.timeoutSecs
        )
    }

    /// Split an assistant response at tool/reasoning boundaries. The run
    /// ledger owns those activities; message bubbles retain only narrative
    /// text and compact-boundary content.
    private static func narrativeSegments(from dto: MessageDto) -> [RenderedMessage] {
        let role: Role = (dto.role == "user") ? .user : .ai
        var segments: [RenderedMessage] = []
        var narrative: [ConversationMessageBlock] = []
        var attachedImages = false

        func flush() {
            guard !narrative.isEmpty else { return }
            let detail = ConversationMessageDetail(blocks: narrative)
            var message = Message(
                role: role,
                text: text(from: narrative),
                images: attachedImages ? [] : uiMessageImages(from: dto.images)
            )
            if role == .user, !attachedImages { message.visualizationContext = visualizationChip(from: dto) }
            segments.append(RenderedMessage(message: message, detail: detail))
            attachedImages = true
            narrative = []
        }

        for block in dto.blocks {
            switch block {
            case .text, .compactBoundary:
                if let lowered = messageBlock(from: block) {
                    narrative.append(lowered)
                }
            case let .visualization(reference):
                flush()
                segments.append(RenderedMessage(message: visualizationMessage(reference: reference), detail: nil))
            case .thinking, .redactedThinking, .toolUse, .toolResult:
                flush()
            }
        }
        flush()
        if !dto.images.isEmpty && !attachedImages {
            segments.append(RenderedMessage(
                message: Message(role: role, text: "", images: uiMessageImages(from: dto.images)),
                detail: nil
            ))
        }
        return segments
    }

    /// A restored widget row: `ready` with a reference, otherwise the
    /// revision is gone and the card says so.
    fileprivate static func visualizationMessage(reference: VisualizationRefDto?, id: UUID = UUID()) -> Message {
        var message = Message(id: id, role: .ai, text: "")
        message.visualization = reference.map {
            MessageVisualization(status: .ready, id: $0.id, revision: $0.revision)
        } ?? MessageVisualization(status: .unavailable)
        return message
    }

    fileprivate static func visualizationChip(from dto: MessageDto) -> VisualizationContextChip? {
        guard dto.role == "user", let context = dto.visualizationContext else { return nil }
        return VisualizationContextChip(id: context.id, revision: context.revision, title: context.title)
    }

    /// Lower ONE wire block. `nil` for a block with nothing to show (blank
    /// text) or one this build does not understand.
    private static func messageBlock(
        from block: MessageBlockDto
    ) -> ConversationMessageBlock? {
        switch block {
        case let .text(text):
            return text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : .text(text)
        case let .thinking(thinking, signature):
            return thinking.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil :
                .thinking(text: thinking, signature: signature)
        case .redactedThinking:
            return .redactedThinking
        case .visualization:
            // Widgets are their own rows, never part of a bubble.
            return nil
        case let .compactBoundary(messagesBefore, messagesAfter, summary):
            return .compactBoundary(
                messagesBefore: Int(messagesBefore),
                messagesAfter: Int(messagesAfter),
                summary: summary
            )
        case let .toolUse(id, tool, inputJson, header):
            return .toolUse(
                id: id,
                tool: tool,
                // Legacy fallback only — the header is what renders when present.
                inputSummary: ConversationExecutionParsing.summarizeToolInput(inputJson) ?? inputJson,
                inputJson: inputJson,
                header: header.map(toolHeader(from:))
            )
        case let .toolResult(id, tool, resultJson, isError, oldString, newString, filePath, display):
            return .toolResult(
                id: id,
                tool: tool,
                isError: isError,
                summary: ConversationExecutionParsing.summarizeToolResult(
                    resultJson,
                    isError: isError,
                    tool: tool
                ),
                resultJson: resultJson,
                oldString: oldString,
                newString: newString,
                filePath: filePath,
                display: display.map(resultDisplay(from:))
            )
        @unknown default:
            return nil
        }
    }

    // MARK: restored-transcript rebuild (BUG FIX)

    /// The interleaved render list a resumed session produces.
    fileprivate struct RestoredTranscript {
        var messages: [Message] = []
        var items: [ConversationRenderItem] = []
        var details: [UUID: ConversationMessageDetail] = [:]
        var wireSignatures: [String] = []
        var wireIndices: [UInt64?] = []
    }

    /// Canonical identity for one wire message. The protocol currently has
    /// no message id, so all fields that can distinguish a replayed message
    /// are length-prefixed into a collision-resistant key.
    fileprivate static func messageSignature(_ dto: MessageDto) -> String {
        func field(_ value: String) -> String { "\(value.utf8.count):\(value)" }
        let blocks = dto.blocks.map { block -> String in
            switch block {
            case let .text(text): return "text|\(field(text))"
            case let .thinking(thinking, signature):
                return "thinking|\(field(thinking))|\(field(signature ?? ""))"
            case let .redactedThinking(data): return "redacted|\(field(data))"
            case let .compactBoundary(before, after, summary):
                return "compact|\(before)|\(after)|\(field(summary))"
            case let .toolUse(id, tool, inputJson, _):
                return "tool-use|\(field(id))|\(field(tool))|\(field(inputJson))"
            case let .toolResult(id, tool, resultJson, isError, oldString, newString, filePath, _):
                return "tool-result|\(field(id))|\(field(tool))|\(field(resultJson))|\(isError)|\(field(oldString ?? ""))|\(field(newString ?? ""))|\(field(filePath ?? ""))"
            case let .visualization(reference):
                return "visualization|\(field(reference?.id ?? ""))|\(reference?.revision ?? 0)"
            @unknown default: return "unknown"
            }
        }.joined(separator: "|")
        let images = dto.images.map { "image|\(field($0.mediaType))|\(field($0.url))" }.joined(separator: "|")
        let wakeup = dto.loopWakeup.map {
            "|loop|\(field($0.message))|\(field($0.companion ?? ""))|\($0.streak)|\($0.sinceMs)"
        } ?? ""
        return "role|\(field(dto.role))|blocks|\(field(blocks))|images|\(field(images))" + wakeup
    }

    fileprivate static func signatureCounts(_ signatures: [String]) -> [String: Int] {
        var counts: [String: Int] = [:]
        for signature in signatures {
            counts[signature, default: 0] += 1
        }
        return counts
    }

    /// Stable UUID for a message occurrence. FNV-1a with two independent
    /// lanes is sufficient here: the result is only a UI identity, not a
    /// security token, and remains deterministic across process launches.
    private static func stableUUID(_ key: String) -> UUID {
        var first: UInt64 = 14_695_981_039_346_656_037
        var second: UInt64 = 10_995_116_282_111
        for byte in key.utf8 {
            first ^= UInt64(byte)
            first &*= 1_099_511_628_211
            second ^= UInt64(byte) &+ 0x9d
            second &*= 1_099_511_628_211
        }
        let bytes: uuid_t = (
            UInt8(truncatingIfNeeded: first >> 56),
            UInt8(truncatingIfNeeded: first >> 48),
            UInt8(truncatingIfNeeded: first >> 40),
            UInt8(truncatingIfNeeded: first >> 32),
            UInt8(truncatingIfNeeded: first >> 24),
            UInt8(truncatingIfNeeded: first >> 16),
            UInt8(truncatingIfNeeded: first >> 8),
            UInt8(truncatingIfNeeded: first),
            UInt8(truncatingIfNeeded: second >> 56),
            UInt8(truncatingIfNeeded: second >> 48),
            UInt8(truncatingIfNeeded: second >> 40),
            UInt8(truncatingIfNeeded: second >> 32),
            UInt8(truncatingIfNeeded: second >> 24),
            UInt8(truncatingIfNeeded: second >> 16),
            UInt8(truncatingIfNeeded: second >> 8),
            UInt8(truncatingIfNeeded: second)
        )
        return UUID(uuid: bytes)
    }

    /// Rebuild the scrollback from `SessionResumed.messages`.
    ///
    /// One `MessageDto` is NOT one row. In the Anthropic protocol a
    /// `tool_result` block lives in the USER turn, so a 1:1 mapping put every
    /// restored tool result inside a right-aligned user bubble whose renderer
    /// only ever draws `message.text` — the result payload was both misplaced
    /// and invisible. Here, narrative blocks (text / thinking / compaction)
    /// accumulate into a `.message`, and a whole user-initiated turn becomes
    /// one terminal `.run` row anchored after its final assistant message.
    ///
    /// The pair spans two turns, so a `toolResult` MERGES onto the row its
    /// `toolUse` opened (matched by tool-use id) rather than adding a second
    /// run. An orphan result — the assistant turn was compacted away — still
    /// gets a terminal run of its own.
    fileprivate static func restoredTranscript(
        from dtos: [MessageDto],
        sessionID: String,
        identityPrefix: String? = nil,
        occurrenceOffsets: [String: Int] = [:],
        wireIndices: [UInt64?] = [],
        finalizeOrphans: Bool = true
    ) -> RestoredTranscript {
        var out = RestoredTranscript()
        let identityPrefix = identityPrefix ?? sessionID
        var occurrenceCounts = occurrenceOffsets
        var pendingRun: ConversationExecutionRun?
        var pendingRunAnchorMessageID: UUID?
        var restoredRunSequence = 0
        var pendingRunIDHint: String?

        func ensurePendingRun() {
            guard pendingRun == nil else { return }
            restoredRunSequence += 1
            let runIdentity = pendingRunIDHint
                ?? "\(identityPrefix)|run|\(restoredRunSequence)"
            pendingRun = ConversationExecutionRun(
                id: "restored-\(stableUUID(runIdentity).uuidString)",
                sessionId: sessionID,
                turnId: nil,
                status: .restored
            )
        }

        func appendActivity(_ activity: ConversationExecutionActivity) {
            pendingRun?.activities.append(activity)
        }

        func finishPendingRun() {
            guard var run = pendingRun else { return }
            // A resumed session has no live turn owner. A call without a
            // persisted result is terminally incomplete, never live work in
            // this process.
            if finalizeOrphans && run.tools.contains(where: { $0.status == .running }) {
                for index in run.tools.indices where run.tools[index].status == .running {
                    run.tools[index].status = .unknown
                }
            }
            // MessageDto does not carry the turn outcome. Do not promote a
            // failed/cancelled tool into the whole Agent result: an Agent can
            // recover and finish after either. The run is only known to be a
            // settled history row; individual tool colors retain the facts
            // that are actually present on the wire.
            run.status = finalizeOrphans
                ? .restored
                : (run.tools.contains(where: { $0.status == .running }) ? .running : .completed)

            if let pendingRunAnchorMessageID,
               let anchorIndex = out.items.firstIndex(where: { item in
                   guard case let .message(message) = item else { return false }
                   return message.id == pendingRunAnchorMessageID
               }) {
                // The run starts when the assistant turn starts, so keep
                // the activity before its narrative message: user → run
                // activity → assistant. This avoids moving a completed run
                // to the tail and preserves wire order after resume.
                out.items.insert(.run(run), at: anchorIndex)
            } else {
                out.items.append(.run(run))
            }
            pendingRun = nil
            pendingRunAnchorMessageID = nil
        }

        for (dtoIndex, dto) in dtos.enumerated() {
            let wireSignature = messageSignature(dto)
            let occurrence = occurrenceCounts[wireSignature, default: 0]
            occurrenceCounts[wireSignature] = occurrence + 1
            out.wireSignatures.append(wireSignature)
            let wireIndex = wireIndices.indices.contains(dtoIndex) ? wireIndices[dtoIndex] : nil
            out.wireIndices.append(wireIndex)
            let messageIdentity = wireIndex.map {
                "\(identityPrefix)|message-index|\($0)"
            } ?? "\(identityPrefix)|message|\(wireSignature)|occurrence|\(occurrence)"
            if let fire = dto.loopWakeup {
                finishPendingRun()
                var wakeup = Message(id: stableUUID(messageIdentity), role: .ai, text: fire.message)
                wakeup.loopWakeupStreak = fire.streak
                if fire.streak > 0, let start = out.items.lastIndex(where: { item in
                    if case let .message(message) = item { return message.loopWakeupStreak != nil }
                    return false
                }) {
                    wakeup.loopFoldedItemIDs = Set(out.items[start...].map(\.id))
                }
                out.messages.append(wakeup)
                out.items.append(.message(wakeup))
                if let companion = fire.companion {
                    let row = Message(id: stableUUID(messageIdentity + "|companion"), role: .ai, text: companion)
                    out.messages.append(row)
                    out.items.append(.message(row))
                }
                continue
            }
            pendingRunIDHint = messageIdentity
            let role: Role = (dto.role == "user") ? .user : .ai
            let isAssistant = dto.role == "assistant"
            let containsToolResult = dto.blocks.contains { block in
                if case .toolResult = block { return true }
                return false
            }
            let startsNewUserTurn = dto.role == "user"
                && !containsToolResult
                && dto.blocks.contains { messageBlock(from: $0) != nil }
            if startsNewUserTurn {
                finishPendingRun()
            }
            if isAssistant {
                ensurePendingRun()
            }
            var narrative: [ConversationMessageBlock] = []
            var narrativeSequence = 0
            var reasoningSequence = 0
            let dtoImages = uiMessageImages(from: dto.images)
            var attachedImages = false

            func flushNarrative() {
                guard !narrative.isEmpty else { return }
                let detail = ConversationMessageDetail(blocks: narrative)
                var message = Message(
                    id: stableUUID("\(messageIdentity)|narrative|\(narrativeSequence)"),
                    role: role,
                    text: text(from: narrative),
                    images: attachedImages ? [] : dtoImages
                )
                if role == .user, !attachedImages { message.visualizationContext = visualizationChip(from: dto) }
                attachedImages = true
                narrativeSequence += 1
                out.messages.append(message)
                out.items.append(.message(message))
                out.details[message.id] = detail
                if isAssistant {
                    pendingRunAnchorMessageID = message.id
                    appendActivity(.textBoundary(
                        id: "boundary-\(messageIdentity)-\(narrativeSequence)",
                        messageID: message.id
                    ))
                }
                narrative = []
            }

            for block in dto.blocks {
                switch block {
                case let .visualization(reference):
                    flushNarrative()
                    let slot = visualizationMessage(
                        reference: reference,
                        id: stableUUID("\(messageIdentity)|visualization|\(narrativeSequence)")
                    )
                    narrativeSequence += 1
                    out.messages.append(slot)
                    out.items.append(.message(slot))
                    if isAssistant {
                        pendingRunAnchorMessageID = slot.id
                    }
                case let .toolUse(id, tool, inputJson, header):
                    flushNarrative()
                    ensurePendingRun()
                    appendActivity(.tool(id: id))
                    let lowered = header.map(toolHeader(from:))
                    let trace = ConversationToolTrace(
                        planDocument: PlanDocument.tool(tool, json: inputJson),
                        id: id,
                        tool: tool,
                        // Temporarily running until the matching result is
                        // seen; finalization marks an orphan as failed.
                        status: .running,
                        inputSummary: lowered == nil
                            ? (ConversationExecutionParsing.summarizeToolInput(inputJson) ?? inputJson)
                            : nil,
                        outputSummary: nil,
                        elapsedMs: nil,
                        header: lowered,
                        display: nil
                    )
                    if let index = pendingRun?.tools.firstIndex(where: { $0.id == id }) {
                        pendingRun?.tools[index] = trace
                    } else {
                        pendingRun?.tools.append(trace)
                    }

                case let .toolResult(id, tool, resultJson, isError, _, _, _, display):
                    flushNarrative()
                    ensurePendingRun()
                    if pendingRun?.activities.contains(where: {
                        if case let .tool(existing) = $0 { return existing == id }
                        return false
                    }) != true {
                        appendActivity(.tool(id: id))
                    }
                    let lowered = display.map(resultDisplay(from:))
                    let status: ConversationToolStatus =
                        ConversationExecutionParsing.isCancellationResult(resultJson)
                            ? .cancelled
                            : (isError ? .failed : .completed)
                    let fallback = lowered == nil
                        ? ConversationExecutionParsing.summarizeToolResult(
                            resultJson, isError: isError, tool: tool)
                        : nil
                    if let index = pendingRun?.tools.firstIndex(where: { $0.id == id }),
                       let existing = pendingRun?.tools[index] {
                        var merged = existing
                        merged.status = status
                        merged.display = lowered
                        merged.questionAnswers = isError ? nil : ConversationAnsweredQuestion.parse(tool: tool, json: resultJson)
                        merged.spawnedAgentID = ConversationExecutionParsing.spawnedAgentID(tool: tool, resultJson: resultJson)
                        merged.outputSummary = fallback
                        pendingRun?.tools[index] = merged
                    } else {
                        let trace = ConversationToolTrace(
                            id: id,
                            tool: tool,
                            status: status,
                            inputSummary: nil,
                            outputSummary: fallback,
                            elapsedMs: nil,
                            header: nil,
                            display: lowered,
                            spawnedAgentID: ConversationExecutionParsing.spawnedAgentID(tool: tool, resultJson: resultJson),
                            questionAnswers: isError ? nil : ConversationAnsweredQuestion.parse(tool: tool, json: resultJson)
                        )
                        pendingRun?.tools.append(trace)
                    }

                default:
                    if let lowered = messageBlock(from: block) {
                        if isAssistant,
                           case let .thinking(text, _) = lowered {
                            flushNarrative()
                            if let index = pendingRun?.activities.indices.last,
                               case let .reasoning(id, current) = pendingRun?.activities[index] {
                                pendingRun?.activities[index] = .reasoning(id: id, text: current + text)
                            } else {
                                reasoningSequence += 1
                                appendActivity(.reasoning(
                                    id: "reasoning-\(messageIdentity)-\(reasoningSequence)",
                                    text: text
                                ))
                            }
                            pendingRun?.reasoning += text
                        } else if isAssistant, case .redactedThinking = lowered {
                            flushNarrative()
                            let text = String(localized: "chat_redacted_thinking")
                            reasoningSequence += 1
                            appendActivity(.reasoning(
                                id: "reasoning-\(messageIdentity)-\(reasoningSequence)-redacted",
                                text: text
                            ))
                            pendingRun?.reasoning += text
                        } else {
                            narrative.append(lowered)
                        }
                    }
                }
            }
            flushNarrative()
            if !dtoImages.isEmpty && !attachedImages {
                let message = Message(
                    id: stableUUID("\(messageIdentity)|images"),
                    role: role,
                    text: "",
                    images: dtoImages
                )
                out.messages.append(message)
                out.items.append(.message(message))
            }
        }
        finishPendingRun()
        return out
    }

    /// Flatten structured blocks into the plain transcript text the rest of
    /// the app still consumes. The chat renderer itself uses the structured
    /// blocks for display.
    private static func text(from blocks: [ConversationMessageBlock]) -> String {
        blocks.compactMap { block -> String? in
            switch block {
            case let .text(text):
                return text
            case let .thinking(text, _):
                return text
            case .redactedThinking:
                return String(localized: "chat_redacted_thinking")
            case .compactBoundary:
                return String(localized: "chat_compacted_label")
            case let .toolUse(_, tool, _, _, header):
                // Prefer the engine's derived title so the compatibility
                // transcript (voice / setup surfaces) reads the same as the UI.
                return header.map(ToolDisplayText.title)
                    ?? String(localized: "chat_tool_calling \(tool)")
            case let .toolResult(_, _, isError, summary, _, _, _, _, display):
                let body = display.flatMap(ToolDisplayText.headline) ?? summary
                return isError ? body : String(localized: "chat_tool_result_summary \(body)")
            }
        }
        .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        .joined(separator: "\n\n")
    }

    /// Map the lowered `ErrorKindDto` onto the UI-facing `ConversationError.Kind`.
    private static func kind(from dto: ErrorKindDto) -> ConversationError.Kind {
        switch dto {
        case .transport: return .transport
        case .protocol: return .protocol
        case .server: return .server
        case .maxTurns: return .maxTurns
        case .rejected: return .rejected
        case .internal: return .internal
        @unknown default: return .internal
        }
    }

    private func fail(_ kind: ConversationError.Kind, _ message: String) {
        if let cancellationOperation,
           cancellationOperation.turnId == currentTurnId {
            // An engine error racing Cancel is not terminal ownership
            // proof. Keep the durable record and correlator so the caller
            // can retry Stop (or receive the later matching recovery
            // snapshot) instead of silently opening a second turn.
            model.error = ConversationError(kind: kind, message: message)
            if unresolvedRecoveryTurnID != cancellationOperation.turnId {
                model.streaming = true
            }
            model.statusLine = nil
            return
        }
        let settledTurn = currentTurnId != nil
        if let currentTurnId {
            durableTurns.clear(turnID: currentTurnId)
            if unresolvedRecoveryTurnID == currentTurnId {
                unresolvedRecoveryTurnID = nil
                needsDurableForegroundRecoveryTurnID = nil
            }
            reattachedTurnIDs.remove(currentTurnId)
            model.hasInactiveDurableRecovery = false
            model.hasUnresolvedTurnRecovery = false
            executorOwnedTurnID = nil
        }
        model.error = ConversationError(kind: kind, message: message)
        model.streaming = false
        model.statusLine = nil
        publishActiveTurnCompletion(.failed)
        // Permission requests are engine-scoped, not owned by this main turn.
        // A background workflow can still be parked after the visible turn
        // fails, so only an explicit approve/deny (or engine teardown) may
        // remove its request from the queue.
        clearTurnPointers(keepEpoch: false)
        if settledTurn {
            requestSessionCatalogRefreshAfterSettledTurn()
        }
    }

    // MARK: model selection (SHIP-BLOCKER #2)

    /// Reflect the engine's active model without saving a new preference.
    /// Boot, listings, and resumed sessions can also report a model; only a
    /// successful explicit switch in the shared engine owns persistence.
    private func applyActiveModel(_ id: String) {
        guard !id.isEmpty else { return }
        model.activeModelId = id
        if let opt = MockData.models.first(where: { $0.id == id || $0.name == id }) {
            model.model = opt
        }
    }

    /// Keep the model chip on the confirmed selection until ModelChanged
    /// arrives. A failed control command must not terminate the active reply.
    func setModel(_ id: String) {
        guard !id.isEmpty, id != model.activeModelId else { return }
        model.controlsError = nil
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.submitCommand(.setModel(model: id))
            } catch {
                self.model.controlsError = String(localized: "chat_switch_model_failed \(error)")
            }
        }
    }

    func setPermissionMode(_ mode: String) {
        guard mode != "bypassPermissions" else {
            model.controlsError = "Confirm Full Access from the conversation controls."
            return
        }
        let previous = model.requestedPermissionMode
        model.requestedPermissionMode = mode
        model.controlsPending = true
        model.controlsError = nil
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.submitCommand(.setPermissionMode(mode: mode))
                self.model.controlsPending = false
            } catch {
                self.model.requestedPermissionMode = previous
                self.model.controlsPending = false
                self.model.controlsError = error.localizedDescription
            }
        }
    }

    func confirmAndSetBypassPermissions(suppressWarning: Bool) {
        let previous = model.requestedPermissionMode
        model.requestedPermissionMode = "bypassPermissions"
        model.controlsPending = true
        model.controlsError = nil
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.confirmBypassPermissionsForCurrentSession()
                try await self.submitCommand(.setPermissionMode(mode: "bypassPermissions"))
                if suppressWarning {
                    self.permissionModeRepository.setBypassWarningSuppressed(true)
                    self.model.bypassPermissionsWarningSuppressed = true
                }
                self.model.controlsPending = false
            } catch {
                self.model.requestedPermissionMode = previous
                self.model.controlsPending = false
                self.model.controlsError = error.localizedDescription
            }
        }
    }

    func setFastMode(_ enabled: Bool) {
        guard !model.fastModePending, enabled != model.fastMode else { return }
        model.fastModePending = true
        model.fastModeError = nil
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.submitCommand(.setFastMode(enabled: enabled))
            } catch {
                self.model.fastModePending = false
                self.model.fastModeError = error.localizedDescription
            }
        }
    }

    func setReasoningSelection(_ selection: String) {
        guard !model.controlsPending else { return }
        let previous = model.controls
        model.controlsPending = true
        Task { [weak self] in
            guard let self else { return }
            do {
                let handle = try await self.ensureHandle()
                try await handle.submit(command: .setReasoningSelection(
                    selection: Self.reasoningSelectionDto(selection)
                ))
            } catch {
                self.model.controls = previous
                self.model.controlsPending = false
                self.model.controlsError = error.localizedDescription
            }
        }
    }

    private static func reasoningSelectionDto(_ value: String) -> ReasoningSelectionDto {
        switch value {
        case "automatic": return .automatic
        case "disabled": return .disabled
        case "enabled": return .enabled
        case let budget where budget.hasPrefix("budget:"):
            let tokens = UInt64(budget.dropFirst("budget:".count)) ?? 0
            return .tokenBudget(tokens: tokens)
        default: return .level(id: value)
        }
    }

    // MARK: session + lifecycle

    /// Switch the active conversation to another session (iOS analog of
    /// Android `ChatViewModel.openSession`). Cancels the in-flight turn first,
    /// retaining the old transcript and correlator until the host confirms
    /// release. Only then is the transcript reset for the selected session.
    func openSession(_ session: SessionRef) {
        guard unresolvedRecoveryTurnID == nil,
              pauseAcknowledgements.isEmpty
        else { return }
        let turnIdToCancel = inFlightTurnForSessionSwitch()
        submitSessionCancellation(turnIdToCancel, isNew: false)
    }

    func forkSession(_ sessionID: String, targetMode: SessionMode) async throws {
        guard !sessionID.isEmpty, targetMode != sessionMode else { return }
        try await submitCommand(
            .forkSession(sessionId: sessionID, targetMode: targetMode.dto)
        )
    }

    /// Resume a prior engine session by UUID (the drawer-tap path for a REAL
    /// history row). Cancels any in-flight turn first, resets the transcript
    /// only after the safe owner slot is released, then submits
    /// `ResumeSession`. The drawer owns the
    /// optimistic selection; the model changes only when the engine confirms
    /// the transition, preventing a stale SessionList from winning the race. The
    /// engine hot-restores the prior transcript into the running orchestrator
    /// and confirms with `SessionResumed{session_id, messages}`, which
    /// re-adopts the id AND replaces the placeholder with the real restored
    /// conversation (oldest-first) so the next turn continues with full prior
    /// context visible. An already-presented active session is a no-op (for
    /// example when SwiftUI restarts a lifecycle task after dismissing an
    /// overlay). Process restoration still re-requests the active id when the
    /// model has no committed transcript to display.
    func resumeSession(_ uuid: String, emptySessionTitle: String?) {
        guard !uuid.isEmpty,
              unresolvedRecoveryTurnID == nil,
              pauseAcknowledgements.isEmpty
        else { return }
        if let record = durableTurns.load(), record.sessionID != uuid {
            durableTurns.clear()
        }
        if uuid == model.activeSessionId,
           pendingSessionTransition == nil,
           (!model.items.isEmpty || !model.messages.isEmpty || hasConfirmedSessionState) {
            return
        }
        let turnIdToCancel = inFlightTurnForSessionSwitch()
        model.sessionRestoreRecovery = nil
        model.sessionTransitionFailure = nil
        let transitionOperationID = beginSessionTransition(.resume(uuid))
        prepareSessionTransition(cancelling: turnIdToCancel, isNew: false)
        let submission: SessionTransitionSubmission
        if let emptySessionTitle {
            submission = .resumeEmpty(sessionID: uuid, title: emptySessionTitle)
        } else {
            submission = .command(.resumeSession(sessionId: uuid, cwd: nil))
        }
        submitSessionTransition(
            cancelling: turnIdToCancel,
            submission: submission,
            failurePrefix: String(localized: "chat_resume_session_failed"),
            transitionOperationID: transitionOperationID,
            resumeTargetID: uuid,
            allowsMissingSessionReplacement: emptySessionTitle == nil,
            isNew: false
        )
    }

    private func reattachDurableTurnIfNeeded(sessionID: String) {
        guard
            durableRecoveryTask == nil,
            let record = durableTurns.load(),
            record.sessionID == sessionID,
            reattachedTurnIDs.insert(record.turnID).inserted
        else { return }
        currentTurnId = record.turnID
        activeTurnEpoch = sessionEpoch
        unresolvedRecoveryTurnID = record.turnID
        model.hasUnresolvedTurnRecovery = true
        model.hasInactiveDurableRecovery = true
        executorOwnedTurnID = nil
        let token = ConversationTurnToken(
            clientTurnId: record.turnID,
            sessionEpoch: sessionEpoch
        )
        model.activeTurnToken = token
        model.streaming = false
        let recoveryTask = Task { @MainActor [weak self] in
            defer {
                self?.replayingTurnID = nil
                self?.durableRecoveryPhase = nil
                self?.durableRecoveryTask = nil
            }
            guard let self else { return }
            do {
                self.replayingTurnID = record.turnID
                self.durableRecoveryPhase = .attaching(turnID: record.turnID)
                self.replayProjectionState = nil
                try await self.submitCommand(.attachTurn(
                    turnId: record.turnID,
                    afterSequence: record.lastSequence
                ))
                // `onEvent` must return before the engine constructor or
                // command submission can continue, so the callback only
                // enqueues. Wait until the single listener pump has applied
                // the recovery snapshot and every retained envelope before
                // allowing live ResumeTurn events to be projected.
                await self.listener?.waitUntilIdle()
                self.durableRecoveryPhase = .resuming(turnID: record.turnID)
                try await self.submitCommand(.resumeTurn(turnId: record.turnID))
                // ResumeTurn emits its own recovery state before any new
                // live envelopes. Drain that FIFO as well so a foreground
                // call cannot race the first resumed delta.
                await self.listener?.waitUntilIdle()
            } catch {
                self.restoreFailedDurableRecovery(record: record, error: error)
            }
        }
        durableRecoveryTask = recoveryTask
    }

    private func restoreFailedDurableRecovery(
        record: DurableConversationTurnRecord,
        error: Error
    ) {
        durableRecoveryPhase = nil
        reattachedTurnIDs.remove(record.turnID)
        guard model.activeSessionId == record.sessionID else { return }
        currentTurnId = record.turnID
        activeTurnEpoch = sessionEpoch
        unresolvedRecoveryTurnID = record.turnID
        needsDurableForegroundRecoveryTurnID = record.turnID
        model.activeTurnToken = ConversationTurnToken(
            clientTurnId: record.turnID,
            sessionEpoch: sessionEpoch
        )
        model.hasUnresolvedTurnRecovery = true
        model.hasInactiveDurableRecovery = true
        executorOwnedTurnID = nil
        settleCorrelatedMainRun()
        model.streaming = false
        model.statusLine = nil
        model.error = ConversationError(
            kind: .host,
            message: String(describing: error)
        )
    }

    private func applyRetainedTurnEvent(_ eventJSON: String, turnID: UInt64) {
        guard
            currentTurnId == turnID,
            let data = eventJSON.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let type = object["type"] as? String
        else { return }
        switch type {
        case "text_delta":
            if let text = object["text"] as? String {
                apply(.textDelta(text: text))
            }
        case "tool_use_started":
            if let id = object["id"] as? String,
               let tool = object["tool"] as? String,
               let inputJSON = object["input_json"] as? String {
                apply(.toolUseStarted(
                    id: id,
                    tool: tool,
                    inputJson: inputJSON,
                    header: nil
                ))
            }
        case "tool_heartbeat":
            if let id = object["id"] as? String,
               let tool = object["tool"] as? String,
               let elapsed = object["elapsed_ms"] as? NSNumber {
                apply(.toolHeartbeat(
                    id: id,
                    tool: tool,
                    elapsedMs: elapsed.uint64Value
                ))
            }
        case "tool_use_result":
            if let id = object["id"] as? String,
               let tool = object["tool"] as? String,
               let resultJSON = object["result_json"] as? String,
               let isError = object["is_error"] as? Bool {
                apply(.toolUseResult(
                    id: id,
                    tool: tool,
                    resultJson: resultJSON,
                    isError: isError,
                    display: nil
                ))
            }
        // The engine journals every forwarded live-turn event verbatim
        // (`serde_json::to_string(&event)` in `TurnLifecycleListener`), and
        // `is_live_turn_payload` explicitly includes `SystemNotice` and
        // `ThinkingDelta`. Dropping them here meant a turn that reasoned or
        // emitted notices while the app was backgrounded came back with no
        // reasoning trace and no notices at all, while Android's
        // `retainedTurnEventToReply` projected both.
        case "thinking_delta":
            if let thinking = object["thinking"] as? String {
                apply(.thinkingDelta(
                    thinking: thinking,
                    signature: object["signature"] as? String
                ))
            }
        case "system_notice":
            if let message = object["message"] as? String {
                apply(.systemNotice(
                    message: message,
                    isError: object["is_error"] as? Bool ?? false
                ))
            }
        // `turn_started`, `turn_ended` and `error` are deliberately NOT
        // projected. Unlike Android, whose `ReplyEvent` cases only append to
        // a transcript, the iOS handlers for these three mutate
        // durable-recovery OWNERSHIP, and projecting one mid-replay destroys
        // the checkpoint the replay is reading from:
        //   - `.turnStarted` sets `executorOwnedTurnID` and
        //     `model.streaming = true`, which shows Stop for a parked turn.
        //   - `.turnEnded` calls `durableTurns.clear(turnID:)` plus
        //     `clearTurnPointers`.
        //   - `.error` reaches `fail(...)`, which likewise calls
        //     `durableTurns.clear(turnID:)`, drops `hasUnresolvedTurnRecovery`
        //     and nils `currentTurnId` via `clearTurnPointers(keepEpoch:false)`.
        // Once the checkpoint is gone, `applyRetainedTurnEvent`'s
        // `currentTurnId == turnID` guard silently rejects every REMAINING
        // event in the same replay suffix — strictly worse than the silence
        // it would replace.
        //
        // Do NOT restate the old rationale that the engine "transitions the
        // checkpoint to a terminal state alongside journalling them", which
        // would make this rule look redundant. It is false for the case that
        // matters: `host.rs`'s `TurnOutcomeDto::Cancelled => return None`
        // (and the `_ => return None` beside it) journal the event with NO
        // transition at all, so a non-terminal snapshot really can be
        // followed by these events in the replayed suffix. Surfacing a
        // replayed failure needs a path that does not clear the checkpoint,
        // not an entry in this switch.
        default:
            break
        }
    }

    // MARK: permission gating (SHIP-BLOCKER #3)

    /// Enqueue one outbound permission request (called on the main actor by the
    /// sink). De-dupes by `requestId` so a re-delivered request can't stack two
    /// prompts. Requests are engine-scoped: a background workflow worker can
    /// ask after the main conversation turn has ended, so main-turn streaming
    /// and cancellation state must not decide whether the request is retained.
    func enqueuePermission(_ request: PermissionRequest) {
        guard !model.pendingPermissions.contains(where: { $0.requestId == request.requestId })
        else { return }
        model.pendingPermissions.append(PendingPermission(request: request))
    }

    /// Resolve the head request by approving it (allow-once / allow-always):
    /// submit `ApprovePermission` and pop it so the next request surfaces.
    func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto) {
        resolve(requestId, command: .approvePermission(requestId: requestId, response: response))
    }

    /// Resolve the head request by denying it: submit `DenyPermission` and pop it.
    func denyPermission(_ requestId: UInt64) {
        resolve(requestId, command: .denyPermission(requestId: requestId))
    }

    /// Shared resolution path: optimistically pop the prompt (the gate's oneshot
    /// fires from the submitted command) and submit the resolving command on the
    /// engine runtime. If delivery fails, restore the exact request at its prior
    /// queue position so the still-parked engine gate remains actionable.
    private func resolve(_ requestId: UInt64, command: ClientCommand) {
        let removedIndex = model.pendingPermissions.firstIndex { $0.requestId == requestId }
        let removedPermission = removedIndex.map { model.pendingPermissions.remove(at: $0) }
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.submitCommand(command)
            } catch {
                if let removedPermission,
                   !self.model.pendingPermissions.contains(where: {
                       $0.requestId == removedPermission.requestId
                   })
                {
                    let index = min(removedIndex ?? 0, self.model.pendingPermissions.count)
                    self.model.pendingPermissions.insert(removedPermission, at: index)
                }
                self.fail(.host, String(localized: "chat_permission_response_failed \(error)"))
            }
        }
    }
}
#endif

import Combine
import Foundation
import Observation
import SwiftUI

struct SessionHandoffReference: Codable, Equatable {
    let sourceScopeKey: String
    let sourceSessionID: String
    let sourceMode: SessionMode
    let targetMode: SessionMode
    let cutoff: String
    let requestID: String
}

/// The engine's summary is UTF-16 data. A Swift String is only its display
/// projection and cannot represent unpaired code units losslessly.
struct SessionHandoffDraft: Identifiable, Equatable {
    let id = UUID()
    let mode: SessionMode
    let sourceCodeUnits: [UInt16]
    let reference: SessionHandoffReference

    var displayText: String { String(decoding: sourceCodeUnits, as: UTF16.self) }
}

enum SessionHandoffPresentationPolicy {
    static func mayOpenAutomatically(requestRevision: UUID, currentRevision: UUID,
                                     ownerMatches: Bool, conversationVisible: Bool,
                                     switching: Bool) -> Bool {
        requestRevision == currentRevision && ownerMatches && conversationVisible && !switching
    }
}

/// Owns every live conversation independently of the visible SwiftUI projection.
/// Switching tabs never tears down a source or changes its session owner.
@MainActor
@Observable
final class SessionHost {
    enum OwnershipError: LocalizedError {
        case conflictingOwner
        case duplicateSource
        case sourceIdentityMismatch

        var errorDescription: String? {
            switch self {
            case .conflictingOwner: return "This session is already open in another scope or mode."
            case .duplicateSource: return "This session already has a live owner."
            case .sourceIdentityMismatch: return "The conversation source has a different session identity."
            }
        }
    }

    @MainActor
    @Observable
    final class Entry {
        let id: UUID
        let source: any ConversationSource
        let scope: ConversationScope
        let mode: SessionMode
        let initialSessionID: String
        fileprivate var claimedSessionID: String
        let workspace: String
        var draft: String
        var attachment: ComposerAttachment?
        var draftSourceCodeUnits: [UInt16]?
        var handoffReference: SessionHandoffReference?
        let voice: VoiceInteractionController
        fileprivate let execution = ConversationBackgroundExecutionController.live()
        fileprivate let alerts: ConversationBackgroundAlertController
        fileprivate var observer: AnyCancellable?
        fileprivate var speechObserver: AnyCancellable?
        fileprivate var completionObserver: AnyCancellable?
        var recordedFirstMessageID: UUID?
        fileprivate var syncScheduled = false

        init(source: any ConversationSource, scope: ConversationScope, mode: SessionMode,
             sessionID: String, workspace: String, draft: String, voice: VoiceInteractionController) {
            let id = UUID()
            self.id = id
            self.alerts = ConversationBackgroundAlertController.live(ownerID: id.uuidString)
            self.source = source
            self.scope = scope
            self.mode = mode
            self.initialSessionID = sessionID
            self.claimedSessionID = sessionID.isEmpty ? source.model.activeSessionId : sessionID
            self.workspace = workspace
            self.draft = draft
            self.voice = voice
        }

        var sessionID: String {
            claimedSessionID.isEmpty ? initialSessionID : claimedSessionID
        }
    }

    private(set) var entries: [Entry] = []
    private(set) var liveStatuses: [String: RecentStatus] = [:]
    var onActivity: ((Entry) -> Void)?
    var admitsSessionIdentity: ((String, ConversationScope, SessionMode) -> Bool)?
    var admitsUnclaimedOwner: (() -> Bool)?
    private var scenePhase: ScenePhase = .active
    private var preferences = NotificationPreferencesStore.load()
    private let voiceFactory: @MainActor () -> VoiceInteractionController

    init(voiceFactory: @escaping @MainActor () -> VoiceInteractionController = { VoiceInteractionController() }) {
        self.voiceFactory = voiceFactory
    }

    /// A cold restore can lose admission between the catalog read and source
    /// retention. Keep a blank owner in that case and leave the rejected
    /// session's persisted draft and ownership untouched.
    func retainInitial(
        source: any ConversationSource, scope: ConversationScope, mode: SessionMode,
        sessionID: String, workspace: String, draft: String,
        makeFallback: () -> (source: any ConversationSource, workspace: String, draft: String)
    ) -> (entry: Entry, rejection: Error?) {
        do {
            return (try retain(source: source, scope: scope, mode: mode, sessionID: sessionID,
                               workspace: workspace, draft: draft), nil)
        } catch {
            let fallback = makeFallback()
            // A newly constructed source has no claimed owner. UI-test
            // fixtures may seed a synthetic ID, which must not be adopted here.
            fallback.source.model.activeSessionId = ""
            // The host is empty and this source has no ID, so the ordinary
            // identity checks cannot reject the placeholder.
            let entry = try! retain(source: fallback.source, scope: scope, mode: mode,
                                    sessionID: "", workspace: fallback.workspace, draft: fallback.draft)
            return (entry, error)
        }
    }

    @discardableResult
    func retain(source: any ConversationSource, scope: ConversationScope, mode: SessionMode,
                sessionID: String, workspace: String, draft: String,
                voice: VoiceInteractionController? = nil) throws -> Entry {
        let requestedID = canonicalSessionID(sessionID)
        let activeID = canonicalSessionID(source.model.activeSessionId)
        if !source.model.sessionTransitionPending, !requestedID.isEmpty, !activeID.isEmpty,
           requestedID != activeID {
            throw OwnershipError.sourceIdentityMismatch
        }
        if let existing = entry(for: source) {
            guard existing.scope == scope, existing.mode == mode,
                  requestedID.isEmpty || canonicalSessionID(existing.sessionID) == requestedID else {
                throw OwnershipError.conflictingOwner
            }
            return existing
        }
        let identity = requestedID.isEmpty ? activeID : requestedID
        if !identity.isEmpty, !(admitsSessionIdentity?(identity, scope, mode) ?? true) {
            throw OwnershipError.conflictingOwner
        }
        if try entryForClaim(sessionID: identity, scope: scope, mode: mode) != nil {
            // The caller must reuse the existing entry before starting another
            // source. A late collision must never publish a second owner.
            throw OwnershipError.duplicateSource
        }
        let entry = Entry(source: source, scope: scope, mode: mode, sessionID: sessionID,
                          workspace: workspace, draft: draft, voice: voice ?? voiceFactory())
        entries.append(entry)
        protectIdentity(of: source, scope: scope, mode: mode, sessionID: identity)
        entry.alerts.setPreferences(preferences)
        entry.alerts.setScenePhase(scenePhase)
        entry.observer = source.model.objectWillChange.sink { [weak self, weak entry] _ in
            Task { @MainActor [weak self, weak entry] in
                guard let self, let entry, !entry.syncScheduled else { return }
                entry.syncScheduled = true
                await Task.yield()
                entry.syncScheduled = false
                self.sync(entry)
            }
        }
        // These subscriptions live with the owner, even while its view is off
        // screen. An admitted Flow turn can therefore finish its audio response.
        entry.speechObserver = source.model.turnSpeechUpdates.sink { [weak entry] update in
            Task { @MainActor [weak entry] in entry?.voice.handleTurnSpeechUpdate(update) }
        }
        entry.completionObserver = source.model.$turnCompletion.dropFirst().sink { [weak entry] completion in
            guard let completion else { return }
            Task { @MainActor [weak entry] in entry?.voice.handleTurnCompletion(completion) }
        }
        sync(entry)
        return entry
    }

    func releaseIdleSource(_ source: any ConversationSource) {
        guard let index = entries.firstIndex(where: { $0.source === source }) else { return }
        guard !entries[index].voice.hasOwnedAudio, !source.hasLiveSideChats,
              ConversationSourceReplacementPolicy.allowsReplacement(
                of: source.model, hasActiveRealtimeAudio: source.hasActiveRealtimeAudio) else { return }
        let entry = entries.remove(at: index)
        entry.observer?.cancel()
        entry.speechObserver?.cancel()
        entry.completionObserver?.cancel()
        entry.execution.setTurnActive(false)
        entry.alerts.clear()
    }

    func status(sessionID: String) -> RecentStatus {
        liveStatuses[canonicalSessionID(sessionID)] ?? .idle
    }

    func entry(for source: any ConversationSource) -> Entry? {
        entries.first { $0.source === source }
    }

    func entry(sessionID: String, scope: ConversationScope, mode: SessionMode) -> Entry? {
        guard !sessionID.isEmpty else { return nil }
        // cwd is mutable and the on-disk session index can lag it. The scope,
        // mode and UUID identify the retained owner independently of cwd.
        return entries.first {
            canonicalSessionID($0.sessionID) == canonicalSessionID(sessionID)
                && $0.scope == scope && $0.mode == mode
        }
    }

    /// Check the UUID before constructing or preparing another native source.
    /// UUID owns its original scope and mode even if its cwd has changed.
    func entryForClaim(sessionID: String, scope: ConversationScope, mode: SessionMode) throws -> Entry? {
        let identity = canonicalSessionID(sessionID)
        guard !identity.isEmpty else { return nil }
        guard let existing = entries.first(where: { canonicalSessionID($0.sessionID) == identity }) else {
            return nil
        }
        guard existing.scope == scope, existing.mode == mode else { throw OwnershipError.conflictingOwner }
        return existing
    }

    /// Install before `prepare()`: startup and first-turn workspace callbacks
    /// can discover the UUID after the source was constructed.
    func protectIdentity(of source: any ConversationSource, scope: ConversationScope, mode: SessionMode,
                         sessionID: String = "") {
        #if canImport(harness_runtimeFFI)
        guard let engine = source as? EngineConversationSource else { return }
        engine.setSessionStartupAdmission { [weak self, weak source] in
            guard let self, let source else { return false }
            return self.allowsStartup(of: source, sessionID: sessionID, scope: scope, mode: mode)
        }
        engine.setSessionOwnerAdmission { [weak self, weak source] sessionID in
            guard let self, let source else { return false }
            return self.claimIdentity(sessionID, for: source, scope: scope, mode: mode)
        }
        #endif
    }

    func allowsStartup(of source: any ConversationSource) -> Bool {
        // Once a source owns a UUID, later catalog failures must not disrupt
        // that owner or its already accepted work.
        if let entry = entry(for: source), !entry.sessionID.isEmpty { return true }
        return admitsUnclaimedOwner?() ?? true
    }

    func allowsStartup(of source: any ConversationSource, sessionID: String,
                       scope: ConversationScope, mode: SessionMode) -> Bool {
        if !canonicalSessionID(sessionID).isEmpty {
            return claimIdentity(sessionID, for: source, scope: scope, mode: mode)
        }
        return allowsStartup(of: source)
    }

    @discardableResult
    func claimIdentity(_ sessionID: String, for source: any ConversationSource,
                       scope: ConversationScope, mode: SessionMode) -> Bool {
        let identity = canonicalSessionID(sessionID)
        guard !identity.isEmpty else { return false }
        // Admission is stable for a retained owner. A later storage error must
        // not interrupt its already accepted turn or its background lease.
        if let own = entry(for: source), !own.claimedSessionID.isEmpty,
           own.scope == scope, own.mode == mode,
           canonicalSessionID(own.claimedSessionID) == identity { return true }
        guard admitsSessionIdentity?(identity, scope, mode) ?? true else { return false }
        do {
            if let existing = try entryForClaim(sessionID: identity, scope: scope, mode: mode),
               existing.source !== source { return false }
            if let own = entry(for: source) {
                guard own.scope == scope, own.mode == mode,
                      own.claimedSessionID.isEmpty || canonicalSessionID(own.claimedSessionID) == identity else { return false }
                if own.claimedSessionID.isEmpty { own.claimedSessionID = sessionID }
            }
            return true
        } catch { return false }
    }

    func setPreferences(_ value: NotifConfig) {
        preferences = value
        for entry in entries { entry.alerts.setPreferences(value) }
    }

    func setScenePhase(_ phase: ScenePhase) {
        scenePhase = phase
        for entry in entries {
            entry.alerts.setScenePhase(phase)
            switch phase {
            case .background:
                entry.voice.handleBackground()
                #if canImport(harness_runtimeFFI)
                (entry.source as? EngineConversationSource)?.sideChats.cancelCurrentTurns()
                #endif
                entry.source.handleBackground()
            case .active:
                #if canImport(harness_runtimeFFI)
                (entry.source as? EngineConversationSource)?.sideChats.handleForeground()
                #endif
                entry.source.handleForeground()
            default: break
            }
            sync(entry)
        }
    }

    func continuedProcessingLease(attached: Bool, identifier: String?, turnID: UInt64?) {
        guard let identifier, let turnID else { return }
        for entry in entries where entry.source.model.activeTurnToken?.clientTurnId == turnID {
            guard identifier == ConversationContinuedProcessingIdentifier.make(sessionID: entry.sessionID, turnID: turnID) else { continue }
            entry.execution.setContinuedProcessingLeaseAttached(attached, turnID: turnID)
        }
    }

    func continuedProcessingExpired(identifier: String, turnID: UInt64?) {
        guard let turnID else { return }
        for entry in entries where entry.source.model.activeTurnToken?.clientTurnId == turnID {
            guard identifier == ConversationContinuedProcessingIdentifier.make(sessionID: entry.sessionID, turnID: turnID) else { continue }
            entry.execution.setContinuedProcessingLeaseAttached(false, turnID: turnID)
            pause(entry, token: entry.source.model.activeTurnToken)
        }
    }

    func sync(_ entry: Entry) {
        let activeID = entry.source.model.activeSessionId
        if !activeID.isEmpty, !entry.source.model.sessionTransitionPending,
           !claimIdentity(activeID, for: entry.source, scope: entry.scope, mode: entry.mode) {
            if entry.source.model.error == nil {
                entry.source.model.error = ConversationError(kind: .host, message: OwnershipError.conflictingOwner.localizedDescription)
            }
            return
        }
        onActivity?(entry)
        let model = entry.source.model
        let token = model.activeTurnToken
        var awaiting = !model.pendingQuestions.isEmpty
        #if canImport(harness_runtimeFFI)
        awaiting = awaiting || !model.pendingPermissions.isEmpty
        #endif
        let status: RecentStatus = awaiting ? .awaitingConfirmation : (model.streaming ? .running : .idle)
        let sessionID = canonicalSessionID(entry.sessionID)
        if !sessionID.isEmpty, liveStatuses[sessionID] != status { liveStatuses[sessionID] = status }
        entry.execution.setExpirationObserver { [weak self, weak entry] in
            guard let self, let entry else { return }
            self.pause(entry, token: token)
        }
        entry.execution.setTurnActive(scenePhase == .background && model.requiresBackgroundExecution,
                                      turnID: token?.clientTurnId)
        #if canImport(harness_runtimeFFI)
        let permissions = model.pendingPermissions.map { pending -> ConversationPendingPermission in
            var toolName = ""
            if case let .toolUseConfirm(name, _, _) = pending.kind { toolName = name }
            return ConversationPendingPermission(requestId: pending.requestId, toolName: toolName)
        }
        #else
        let permissions: [ConversationPendingPermission] = []
        #endif
        entry.alerts.sync(ConversationBackgroundSnapshot(
            sessionID: entry.sessionID,
            turnToken: token,
            turnCompletion: model.turnCompletion,
            pendingQuestions: model.pendingQuestions,
            pendingPermissions: permissions,
            backgroundTasks: model.backgroundTasks,
            requiresExecutionLease: model.requiresBackgroundExecution,
            workspaceKey: entry.scope.workspaceKey,
            sessionMode: entry.mode
        ))
    }

    private func pause(_ entry: Entry, token: ConversationTurnToken?) {
        guard let token, entry.source.model.activeTurnToken == token else { return }
        let owner = entry.source
        Task { @MainActor [weak self, weak entry, weak owner] in
            guard let self, let entry, let owner, owner.model.activeTurnToken == token else { return }
            do {
                try await owner.markActiveTurnPausedRecoverable(token)
                guard owner.model.activeTurnToken == token, !owner.model.streaming,
                      owner.model.statusLine == String(localized: "chat_background_paused_text") else { return }
                entry.alerts.markRecoverablePause(sessionID: entry.sessionID, turnToken: token,
                                                    workspaceKey: entry.scope.workspaceKey, sessionMode: entry.mode)
                self.sync(entry)
            } catch { self.sync(entry) }
        }
    }
}

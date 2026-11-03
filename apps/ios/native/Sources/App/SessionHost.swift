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

@MainActor
enum SessionOwnerDrainPolicy {
    static func confirmed(waitForMedia: () async -> Bool, waitForSession: () async -> Bool) async -> Bool {
        guard await waitForMedia() else { return false }
        return await waitForSession()
    }
}

/// A queued workspace commit must still belong to the same live source and
/// session revision when the project writer actually reaches it. The lock is
/// never held while waiting for that queue or a native async operation.
final class WorkspaceWriteAuthority: @unchecked Sendable {
    enum Failure: Error { case stale }
    private let lock = NSLock()
    private var revision: UInt64 = 0
    private var live = true
    private var selected = false

    func currentRevision() -> UInt64 { lock.withLock { revision } }
    func invalidate() { lock.withLock { revision &+= 1 } }
    func retire() { lock.withLock { live = false; revision &+= 1 } }
    func setSelected(_ value: Bool) { lock.withLock { selected = value } }

    func commit<T>(revision expected: UInt64, _ write: (Bool) throws -> T) throws -> T {
        try lock.withLock {
            guard live, revision == expected else { throw Failure.stale }
            return try write(selected)
        }
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
        private(set) var draftRevision: UInt64 = 0
        var draft: String {
            didSet { if Array(draft.utf16) != Array(oldValue.utf16) { draftRevision &+= 1 } }
        }
        var attachment: ComposerAttachment? {
            didSet { if attachment != oldValue { draftRevision &+= 1 } }
        }
        var draftSourceCodeUnits: [UInt16]? {
            didSet { if draftSourceCodeUnits != oldValue { draftRevision &+= 1 } }
        }
        var handoffReference: SessionHandoffReference?
        fileprivate var replacement: Entry?
        let voice: VoiceInteractionController
        fileprivate let execution = ConversationBackgroundExecutionController.live()
        fileprivate let alerts: ConversationBackgroundAlertController
        fileprivate var observer: AnyCancellable?
        fileprivate var speechObserver: AnyCancellable?
        fileprivate var completionObserver: AnyCancellable?
        fileprivate var workspaceInvalidationObservers: [AnyCancellable] = []
        let workspaceWriteAuthority = WorkspaceWriteAuthority()
        var recordedFirstMessageID: UUID?
        var isRecordingSession = false
        fileprivate var syncScheduled = false
        fileprivate var observedWorkspaceState: SessionWorkspaceState?
        fileprivate var observedWorkspaceSessionID = ""

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
    private final class PreparingClaim {
        weak var source: (any ConversationSource)?
        let scope: ConversationScope
        let mode: SessionMode
        init(source: any ConversationSource, scope: ConversationScope, mode: SessionMode) {
            self.source = source
            self.scope = scope
            self.mode = mode
        }
    }
    private var preparingClaims: [String: PreparingClaim] = [:]
    private struct FailedPreparation {
        let source: any ConversationSource
        let scope: ConversationScope
        let mode: SessionMode
        let expectedSessionID: String?
        var sessionID: String?
        var drainID: UUID?
        var drainTask: Task<Void, Error>?
    }
    // A prepared native handle can outlive its Swift call site. Retain it
    // until its exact native UUID has drained, even if publication failed.
    private var failedPreparations: [String: FailedPreparation] = [:]
    #if DEBUG
    @ObservationIgnored var onFailedPreparationDrainJoinForTesting: (() -> Void)?
    #endif
    private(set) var liveStatuses: [String: RecentStatus] = [:]
    var onActivity: ((Entry) -> Void)?
    var onWorkspaceChanged: ((Entry) -> Void)?
    private(set) var selectedEntryID: UUID?
    var admitsSessionIdentity: ((String, ConversationScope, SessionMode) -> Bool)?
    var admitsUnclaimedOwner: (() -> Bool)?
    var startupAdmissionReady: (() -> Bool)?
    var beginSourcePreparation: ((any ConversationSource) -> Bool)?
    var finishSourcePreparation: ((any ConversationSource) -> Void)?
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
                voice: VoiceInteractionController? = nil, replacing: Entry? = nil) throws -> Entry {
        if let replacing {
            guard entries.contains(where: { $0 === replacing }), replacing.source !== source,
                  entry(for: source) == nil,
                  replacing.scope == scope, replacing.mode == mode,
                  !replacing.voice.hasOwnedAudio, !replacing.source.hasLiveSideChats,
                  ConversationSourceReplacementPolicy.allowsReplacement(
                    of: replacing.source.model, hasActiveRealtimeAudio: replacing.source.hasActiveRealtimeAudio) else {
                throw OwnershipError.conflictingOwner
            }
        }
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
        let sourceClaims = preparingClaims.filter { $0.value.source === source }
        guard sourceClaims.count <= 1 else { throw OwnershipError.conflictingOwner }
        let prepared = sourceClaims.first
        if let prepared {
            guard prepared.value.scope == scope, prepared.value.mode == mode,
                  requestedID.isEmpty || requestedID == prepared.key,
                  source.model.sessionTransitionPending || activeID.isEmpty || activeID == prepared.key else {
                throw OwnershipError.sourceIdentityMismatch
            }
        }
        let identity = requestedID.isEmpty ? (prepared?.key ?? activeID) : requestedID
        if let replacing {
            let oldIdentity = canonicalSessionID(replacing.sessionID)
            guard oldIdentity.isEmpty || oldIdentity == identity else { throw OwnershipError.sourceIdentityMismatch }
        }
        if !identity.isEmpty, !(admitsSessionIdentity?(identity, scope, mode) ?? true) {
            throw OwnershipError.conflictingOwner
        }
        if failedPreparations.values.contains(where: {
            $0.source !== source && ($0.sessionID == nil || $0.sessionID == identity)
        }) { throw OwnershipError.conflictingOwner }
        if let other = preparingClaims[identity]?.source, other !== source {
            throw OwnershipError.duplicateSource
        }
        if let existing = try entryForClaim(sessionID: identity, scope: scope, mode: mode), existing !== replacing {
            // The caller must reuse the existing entry before starting another
            // source. A late collision must never publish a second owner.
            throw OwnershipError.duplicateSource
        }
        let entry = Entry(source: source, scope: scope, mode: mode, sessionID: sessionID,
                          workspace: workspace, draft: draft, voice: voice ?? voiceFactory())
        // The native workspace admission can finish before SessionStarted is
        // delivered. Transfer its authenticated temporary claim atomically to
        // the retained owner before the preparation lease is released.
        entry.claimedSessionID = identity
        if let replacing { removeEntry(replacing) }
        entries.append(entry)
        if preparingClaims[identity]?.source === source { preparingClaims.removeValue(forKey: identity) }
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
        // Published values notify before mutation. Invalidate a queued write
        // immediately, even if the main-actor sync task has not yet run.
        let authority = entry.workspaceWriteAuthority
        entry.workspaceInvalidationObservers = [
            source.model.$workspaceState.dropFirst().sink { _ in authority.invalidate() },
            source.model.$activeSessionId.dropFirst().sink { _ in authority.invalidate() },
            source.model.$sessionTransitionPending.dropFirst().sink { _ in authority.invalidate() },
            source.model.$sessionMode.dropFirst().sink { _ in authority.invalidate() },
            source.model.$activeTurnToken.dropFirst().sink { _ in authority.invalidate() }
        ]
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
        guard let entry = entry(for: source) else { return }
        #if canImport(harness_runtimeFFI)
        // An unshown rejected input is still owned by this source. It can be
        // moved only by the same-identity replacement transaction below.
        guard source.model.unsentPrompts.isEmpty else { return }
        #endif
        guard !entry.voice.hasOwnedAudio, !source.hasLiveSideChats,
              ConversationSourceReplacementPolicy.allowsReplacement(
                of: source.model, hasActiveRealtimeAudio: source.hasActiveRealtimeAudio) else { return }
        removeEntry(entry)
    }

    /// An unselected cold authoring source has never admitted a turn. It can
    /// be removed after the native handle was retired and its claim drained.
    func releaseRetiredUnconfirmedAuthoringSource(_ source: any ConversationSource) {
        #if canImport(harness_runtimeFFI)
        guard let engine = source as? EngineConversationSource, engine.isRetiredForReconnect,
              engine.pendingRecoveredAuthoringOwner != nil, let entry = entry(for: source),
              selectedEntryID != entry.id, !entry.voice.hasOwnedAudio, !source.hasLiveSideChats,
              ConversationSourceReplacementPolicy.allowsReplacement(of: source.model,
                hasActiveRealtimeAudio: source.hasActiveRealtimeAudio, allowUnconfirmedAuthoring: true) else { return }
        removeEntry(entry)
        #endif
    }

    #if canImport(harness_runtimeFFI)
    /// A retired source still owns its UUID until native acknowledges drain.
    /// Recheck the captured entry after the wait before removing its fence.
    func releaseRetiredAuthoringSourceAfterDrain(_ source: EngineConversationSource, entry: Entry,
                                                stillCurrent: () -> Bool,
                                                waitForRelease: () async -> Bool) async -> Bool {
        guard source.isRetiredForReconnect, self.entry(for: source) === entry,
              selectedEntryID != entry.id, stillCurrent() else { return false }
        guard await waitForRelease(), stillCurrent(), self.entry(for: source) === entry,
              selectedEntryID != entry.id else { return false }
        if source.pendingRecoveredAuthoringOwner != nil {
            releaseRetiredUnconfirmedAuthoringSource(source)
        } else {
            releaseIdleSource(source)
        }
        return self.entry(for: source) == nil
    }
    #endif

    /// Validate the new source while the old entry remains registered. Once
    /// validation succeeds, the main actor swaps entries without suspension.
    func replaceIdleSource(_ old: any ConversationSource, with replacement: any ConversationSource,
                           scope: ConversationScope, mode: SessionMode, sessionID: String,
                           workspace: String, draft: String,
                           validateUnsentOwner: (() throws -> Void)? = nil) throws -> Entry {
        guard let previous = entry(for: old) else { throw OwnershipError.conflictingOwner }
        #if canImport(harness_runtimeFFI)
        let unsent = old.model.unsentPrompts
        let ownerID = canonicalSessionID(previous.sessionID)
        let replacementEngine = replacement as? EngineConversationSource
        if !unsent.isEmpty {
            guard let oldEngine = old as? EngineConversationSource,
                  replacementEngine != nil,
                  !ownerID.isEmpty, previous.scope == scope, previous.mode == mode,
                  previous.workspace == workspace, replacement.model.unsentPrompts.isEmpty,
                  unsent.allSatisfy({ $0.sideID == nil && $0.sessionID == ownerID && oldEngine.ownsUnsentPrompt($0) }),
                  let oldState = old.model.workspaceState, let newState = replacement.model.workspaceState,
                  canonicalSessionID(oldState.sessionId) == ownerID,
                  canonicalSessionID(newState.sessionId) == ownerID,
                  !oldState.transcriptPath.isEmpty, (oldState.transcriptPath as NSString).isAbsolutePath,
                  oldState.transcriptPath == newState.transcriptPath,
                  oldState.cwd == newState.cwd, oldState.projectRoot == newState.projectRoot,
                  oldState.additionalDirectories == newState.additionalDirectories else {
                throw OwnershipError.sourceIdentityMismatch
            }
            guard let validateUnsentOwner else { throw OwnershipError.sourceIdentityMismatch }
            try validateUnsentOwner()
        }
        #endif
        let next = try retain(source: replacement, scope: scope, mode: mode, sessionID: sessionID,
                              workspace: workspace, draft: draft, voice: previous.voice, replacing: previous)
        #if canImport(harness_runtimeFFI)
        if !unsent.isEmpty {
            replacementEngine?.adoptUnsentPromptsFromVerifiedOwner(unsent)
            old.model.unsentPrompts.removeAll()
        }
        #endif
        next.attachment = previous.attachment
        next.draftSourceCodeUnits = previous.draftSourceCodeUnits
        next.handoffReference = previous.handoffReference
        previous.replacement = next
        return next
    }

    /// Long-running host actions report to their captured owner. A settings
    /// reconnect may replace that owner's idle source while the action runs.
    func deliverOperationFailure(_ error: ConversationError, to owner: Entry) {
        var target = owner
        while let replacement = target.replacement { target = replacement }
        if entry(for: target.source) === target {
            target.source.model.error = error
        } else if !target.sessionID.isEmpty,
                  let current = try? entryForClaim(sessionID: target.sessionID, scope: target.scope, mode: target.mode) {
            current.source.model.error = error
        } else {
            // The captured source remains the owner; never deliver to a new
            // foreground conversation merely because it is now selected.
            target.source.model.error = error
        }
    }

    private func removeEntry(_ entry: Entry) {
        guard let index = entries.firstIndex(where: { $0 === entry }) else { return }
        entries.remove(at: index)
        entry.workspaceWriteAuthority.retire()
        if selectedEntryID == entry.id { selectedEntryID = nil }
        entry.observer?.cancel()
        entry.speechObserver?.cancel()
        entry.completionObserver?.cancel()
        entry.workspaceInvalidationObservers.forEach { $0.cancel() }
        entry.execution.setTurnActive(false)
        entry.alerts.clear()
    }

    func status(sessionID: String) -> RecentStatus {
        liveStatuses[canonicalSessionID(sessionID)] ?? .idle
    }

    func entry(for source: any ConversationSource) -> Entry? {
        entries.first { $0.source === source }
    }

    func select(_ entry: Entry) {
        guard entries.contains(where: { $0 === entry }) else { return }
        if let previous = entries.first(where: { $0.id == selectedEntryID }), previous !== entry {
            previous.workspaceWriteAuthority.setSelected(false)
        }
        entry.workspaceWriteAuthority.setSelected(true)
        selectedEntryID = entry.id
    }

    func refreshWorkspace(_ entry: Entry) {
        guard entries.contains(where: { $0 === entry }) else { return }
        onWorkspaceChanged?(entry)
    }

    /// Bind the returned workspace to the revision that preceded its native
    /// read. A workspace event arriving while the read is suspended forces a
    /// new read; callers must carry this revision through the disk CAS.
    func authenticatedWorkspaceSnapshot(for entry: Entry) async -> (state: SessionWorkspaceState, revision: UInt64)? {
        let source = entry.source
        let id = canonicalSessionID(entry.sessionID)
        guard !id.isEmpty else { return nil }
        for _ in 0..<2 {
            guard self.entry(for: source) === entry,
                  canonicalSessionID(entry.sessionID) == id,
                  canonicalSessionID(source.model.activeSessionId) == id,
                  !source.model.sessionTransitionPending else { return nil }
            let revision = entry.workspaceWriteAuthority.currentRevision()
            guard let state = await source.authenticatedWorkspaceState(sessionID: id, mode: entry.mode),
                  self.entry(for: source) === entry,
                  canonicalSessionID(entry.sessionID) == id,
                  canonicalSessionID(source.model.activeSessionId) == id,
                  canonicalSessionID(state.sessionId) == id,
                  !source.model.sessionTransitionPending else { return nil }
            if entry.workspaceWriteAuthority.currentRevision() != revision { continue }
            return (state, revision)
        }
        return nil
    }

    /// A catalog may discover a session while its owner is already live but
    /// its first row is still being written. Only that owner's native handle
    /// can confirm that the listed UUID resolves to the same transcript.
    func validatesCatalogSource(sessionID: String, scope: ConversationScope,
                                mode: SessionMode, transcriptPath: String?) async -> Bool {
        guard !hasPreparingClaim(sessionID: sessionID) else { return false }
        do {
            guard let entry = try entryForClaim(sessionID: sessionID, scope: scope, mode: mode) else { return true }
            guard let path = transcriptPath, !path.isEmpty, (path as NSString).isAbsolutePath else { return false }
            let source = entry.source
            guard let state = await source.authenticatedWorkspaceState(sessionID: sessionID, mode: mode),
                  self.entry(for: source) === entry,
                  canonicalSessionID(entry.sessionID) == canonicalSessionID(sessionID),
                  canonicalSessionID(state.sessionId) == canonicalSessionID(sessionID) else { return false }
            return state.transcriptPath == path
        } catch { return false }
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

    func hasPreparingClaim(sessionID: String) -> Bool {
        let id = canonicalSessionID(sessionID)
        if failedPreparations.values.contains(where: { $0.sessionID == nil || $0.sessionID == id }) { return true }
        guard let claim = preparingClaims[id] else { return false }
        if claim.source == nil { preparingClaims.removeValue(forKey: id); return false }
        return true
    }

    func beginReplacement(of old: any ConversationSource, with replacement: any ConversationSource,
                          sessionID: String, scope: ConversationScope, mode: SessionMode) -> Bool {
        let id = canonicalSessionID(sessionID)
        guard !id.isEmpty else { return beginSourcePreparation?(replacement) ?? true }
        guard admitsSessionIdentity?(id, scope, mode) ?? true,
              let existing = try? entryForClaim(sessionID: id, scope: scope, mode: mode),
              existing.source === old, !hasPreparingClaim(sessionID: id),
              beginSourcePreparation?(replacement) ?? true else { return false }
        existing.workspaceWriteAuthority.invalidate()
        preparingClaims[id] = PreparingClaim(source: replacement, scope: scope, mode: mode)
        return true
    }

    func endPreparingClaim(of source: any ConversationSource) {
        guard !failedPreparations.values.contains(where: { $0.source === source }) else { return }
        preparingClaims = preparingClaims.filter { $0.value.source !== source && $0.value.source != nil }
        finishSourcePreparation?(source)
    }

    func quarantineFailedPreparation(_ source: any ConversationSource, requestID: String,
                                     scope: ConversationScope, mode: SessionMode, sessionID: String?) throws {
        let request = canonicalSessionID(requestID)
        let identity = sessionID.map(canonicalSessionID).flatMap { $0.isEmpty ? nil : $0 }
        guard UUID(uuidString: request) != nil, identity.map({ UUID(uuidString: $0) != nil }) ?? true,
              entry(for: source) == nil else { throw OwnershipError.sourceIdentityMismatch }
        if let existing = failedPreparations[request] {
            guard existing.source === source, existing.scope == scope, existing.mode == mode,
                  existing.expectedSessionID == identity else { throw OwnershipError.conflictingOwner }
            return
        }
        // Until the native handle is inspected, block all new identities. A
        // corrupt result must not be mistaken for the requested UUID.
        failedPreparations[request] = FailedPreparation(source: source, scope: scope, mode: mode,
                                                         expectedSessionID: identity, sessionID: nil,
                                                         drainID: nil, drainTask: nil)
    }

    func failedPreparation(requestID: String) -> (source: any ConversationSource, scope: ConversationScope,
                                                  mode: SessionMode, expectedSessionID: String?, sessionID: String?)? {
        guard let item = failedPreparations[canonicalSessionID(requestID)] else { return nil }
        return (item.source, item.scope, item.mode, item.expectedSessionID, item.sessionID)
    }

    /// Only an explicit retry of the same logical destination may drain its
    /// abandoned preparation. An unidentified preparation remains global
    /// admission evidence until its actual native UUID is authenticated.
    func failedPreparationIDs(scope: ConversationScope, mode: SessionMode, sessionID: String) -> [String] {
        let identity = canonicalSessionID(sessionID)
        return failedPreparations.compactMap { request, item in
            guard item.scope == scope, item.mode == mode else { return nil }
            if identity.isEmpty { return item.expectedSessionID == nil ? request : nil }
            return item.expectedSessionID == identity || item.sessionID == identity ? request : nil
        }.sorted()
    }

    /// Join the whole retirement, identity lookup and drain, not just the
    /// native wait. Otherwise a second retry can reach identity lookup after
    /// the first retry has already released this exact preparation.
    func drainFailedPreparation(requestID: String,
                                perform: @escaping @MainActor (any ConversationSource) async throws -> Void) async throws {
        let request = canonicalSessionID(requestID)
        guard var item = failedPreparations[request] else { return }
        if let task = item.drainTask {
            #if DEBUG
            onFailedPreparationDrainJoinForTesting?()
            #endif
            return try await task.value
        }
        let drainID = UUID(), source = item.source
        let task = Task { @MainActor in try await perform(source) }
        item.drainID = drainID
        item.drainTask = task
        failedPreparations[request] = item
        do {
            try await task.value
            clearFailedPreparationDrain(request: request, source: source, drainID: drainID)
        } catch {
            clearFailedPreparationDrain(request: request, source: source, drainID: drainID)
            throw error
        }
    }

    private func clearFailedPreparationDrain(request: String, source: any ConversationSource, drainID: UUID) {
        guard var current = failedPreparations[request], current.source === source,
              current.drainID == drainID else { return }
        current.drainID = nil
        current.drainTask = nil
        failedPreparations[request] = current
    }

    func identifyFailedPreparation(requestID: String, source: any ConversationSource, sessionID: String) throws {
        let request = canonicalSessionID(requestID), identity = canonicalSessionID(sessionID)
        guard UUID(uuidString: identity) != nil, var item = failedPreparations[request], item.source === source,
              item.expectedSessionID == nil || item.expectedSessionID == identity,
              item.sessionID == nil || item.sessionID == identity else { throw OwnershipError.sourceIdentityMismatch }
        item.sessionID = identity
        failedPreparations[request] = item
    }

    func releaseFailedPreparationAfterDrain(requestID: String, source: any ConversationSource,
                                            sessionID: String, waitForRelease: () async -> Bool) async -> Bool {
        let request = canonicalSessionID(requestID), identity = canonicalSessionID(sessionID)
        guard let item = failedPreparations[request], item.source === source, item.sessionID == identity,
              await waitForRelease() else { return false }
        // Another waiter may have confirmed this same captured source while
        // this one was suspended. Absence can only result from that confirmed
        // release; a different source under the request ID still fails closed.
        guard let current = failedPreparations[request] else { return true }
        guard current.source === source, current.sessionID == identity else { return false }
        failedPreparations.removeValue(forKey: request)
        endPreparingClaim(of: source)
        return true
    }

    /// Install before `prepare()`: startup and first-turn workspace callbacks
    /// can discover the UUID after the source was constructed.
    func protectIdentity(of source: any ConversationSource, scope: ConversationScope, mode: SessionMode,
                         sessionID: String = "") {
        #if canImport(harness_runtimeFFI)
        guard let engine = source as? EngineConversationSource else { return }
        engine.setSessionStartupAdmission { [weak self, weak source, weak engine] in
            guard let self, let source, let engine else { return false }
            if engine.hasPreparedNativeHandle, let entry = self.entry(for: source), !entry.sessionID.isEmpty {
                self.endPreparingClaim(of: source)
                return true
            }
            guard self.startupAdmissionReady?() ?? true else { return false }
            // An initial UI placeholder may have been retained before the
            // scheduler's durable receipts were loaded; recheck it now.
            let identity = canonicalSessionID(sessionID)
            if !identity.isEmpty, !(self.admitsSessionIdentity?(identity, scope, mode) ?? true) { return false }
            guard self.allowsStartup(of: source, sessionID: sessionID, scope: scope, mode: mode) else { return false }
            return true
        }
        engine.setSessionStartupLease(begin: { [weak self, weak source] in
            guard let self, let source else { return false }
            guard self.beginSourcePreparation?(source) ?? true else {
                self.endPreparingClaim(of: source)
                return false
            }
            let id = canonicalSessionID(sessionID)
            if !id.isEmpty { self.preparingClaims[id] = PreparingClaim(source: source, scope: scope, mode: mode) }
            return true
        }, finish: { [weak self, weak source] in
            guard let self, let source else { return }
            // A new source stays fenced until RootView retains or discards it.
            if self.entry(for: source) != nil { self.finishSourcePreparation?(source) }
        })
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
        if failedPreparations.values.contains(where: { $0.source !== source && $0.sessionID == nil }) { return false }
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
        if failedPreparations.values.contains(where: {
            $0.source !== source && ($0.sessionID == nil || $0.sessionID == identity)
        }) { return false }
        if preparingClaims.contains(where: { key, claim in
            claim.source === source && (key != identity || claim.scope != scope || claim.mode != mode)
        }) { return false }
        guard admitsSessionIdentity?(identity, scope, mode) ?? true else { return false }
        do {
            if let existing = try entryForClaim(sessionID: identity, scope: scope, mode: mode),
               existing.source !== source { return false }
            if let other = preparingClaims[identity]?.source, other !== source { return false }
            if let own = entry(for: source) {
                guard own.scope == scope, own.mode == mode,
                      own.claimedSessionID.isEmpty || canonicalSessionID(own.claimedSessionID) == identity else { return false }
                if own.claimedSessionID.isEmpty { own.claimedSessionID = sessionID }
                preparingClaims.removeValue(forKey: identity)
            } else {
                // Preparation may suspend before the source is retained. The
                // scheduler must see this claim before reserving its UUID.
                preparingClaims[identity] = PreparingClaim(source: source, scope: scope, mode: mode)
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
        let model = entry.source.model
        if model.sessionTransitionPending {
            entry.observedWorkspaceState = nil
            entry.observedWorkspaceSessionID = ""
        }
        if !model.sessionTransitionPending, let workspace = model.workspaceState,
           canonicalSessionID(workspace.sessionId) == canonicalSessionID(entry.sessionID), !entry.sessionID.isEmpty,
           (workspace != entry.observedWorkspaceState || entry.observedWorkspaceSessionID != entry.sessionID) {
            entry.observedWorkspaceState = workspace
            entry.observedWorkspaceSessionID = entry.sessionID
            onWorkspaceChanged?(entry)
        }
        onActivity?(entry)
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

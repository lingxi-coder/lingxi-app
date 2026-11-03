import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class SessionHostOwnershipTests: XCTestCase {
    func testBackgroundWorkspaceNotificationStaysWithItsRetainedOwner() throws {
        let host = SessionHost()
        let a = MockConversationSource(sessionMode: .chat)
        let b = MockConversationSource(sessionMode: .code)
        let aID = "11111111-1111-4111-8111-111111111111"
        let bID = "22222222-2222-4222-8222-222222222222"
        a.model.activeSessionId = aID
        b.model.activeSessionId = bID
        let first = try host.retain(source: a, scope: .global, mode: .chat, sessionID: aID,
                                    workspace: "/home/a", draft: "")
        let second = try host.retain(source: b, scope: .project("p"), mode: .code, sessionID: bID,
                                     workspace: "/home/b", draft: "")
        host.select(second)
        var notified: [UUID] = []
        host.onWorkspaceChanged = { notified.append($0.id) }
        a.model.workspaceState = SessionWorkspaceState(sessionId: aID, cwd: "/home/a/new",
            projectRoot: "/home/a", additionalDirectories: [], transcriptPath: "/catalog/a.jsonl")
        host.sync(first)
        XCTAssertEqual(notified, [first.id])
        XCTAssertEqual(host.selectedEntryID, second.id)
        host.sync(first)
        XCTAssertEqual(notified, [first.id], "unrelated model updates must not refetch the workspace")
    }

    @MainActor
    func testExplicitAudioReservesAgainstQueuedAutomaticPlayback() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject(), c = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        lane.enqueueAutomatic(c)
        XCTAssertTrue(lane.reserveExplicit(b) === a)
        XCTAssertTrue(lane.release(a))
        XCTAssertNil(lane.nextAutomatic())
        XCTAssertFalse(lane.acquire(c, kind: .automatic))
        XCTAssertTrue(lane.finishReservation(b, canAcquire: true))
        XCTAssertTrue(lane.owner === b)
        XCTAssertTrue(lane.release(b))
        XCTAssertTrue(lane.nextAutomatic() === c)
        XCTAssertTrue(lane.acquire(c, kind: .automatic))
    }

    @MainActor
    func testExplicitAudioNeverPreemptsAnotherExplicitOwner() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .explicit))
        XCTAssertNil(lane.reserveExplicit(b))
        XCTAssertFalse(lane.acquire(b, kind: .explicit))
        XCTAssertFalse(lane.acquire(a, kind: .automatic))
        XCTAssertTrue(lane.owner === a)
    }

    @MainActor
    func testFailedAudioStopKeepsAutomaticOwnerAndQueuedWork() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject(), c = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        lane.enqueueAutomatic(c)
        XCTAssertTrue(lane.reserveExplicit(b) === a)
        XCTAssertFalse(lane.finishReservation(b, canAcquire: false))
        XCTAssertTrue(lane.owner === a)
        XCTAssertNil(lane.nextAutomatic())
        XCTAssertTrue(lane.release(a))
        XCTAssertTrue(lane.nextAutomatic() === c)
    }

    @MainActor
    func testOwnerCanReserveItsOwnAutomaticPlaybackForExplicitCapture() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        XCTAssertTrue(lane.reserveExplicit(a) === a)
        XCTAssertTrue(lane.release(a))
        XCTAssertTrue(lane.finishReservation(a, canAcquire: true))
        XCTAssertTrue(lane.owner === a)
        XCTAssertEqual(lane.kind, .explicit)
    }

    @MainActor
    func testCancelledExplicitReservationLetsQueuedAutomaticOwnerProceed() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject(), c = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        lane.enqueueAutomatic(c)
        XCTAssertTrue(lane.reserveExplicit(b) === a)
        XCTAssertTrue(lane.release(a))
        XCTAssertFalse(lane.finishReservation(b, canAcquire: false))
        XCTAssertTrue(lane.nextAutomatic() === c)
        XCTAssertTrue(lane.acquire(c, kind: .automatic))
    }

    @MainActor
    func testAutomaticPlaybackDeadlineRequiresActualTaskCompletion() async {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        let operation = Task { @MainActor in for await _ in release { break } }
        let timedOut = await VoiceAudioStopDeadline.completed(operation, within: .milliseconds(10))
        XCTAssertFalse(timedOut)
        continuation.yield(())
        continuation.finish()
        let completed = await VoiceAudioStopDeadline.completed(operation, within: .seconds(1))
        XCTAssertTrue(completed)
    }

    @MainActor
    func testBlockedAutomaticOwnerDoesNotStarveTheNextQueuedOwner() {
        let lane = VoiceAudioOwnership<NSObject>()
        let active = NSObject(), blocked = NSObject(), ready = NSObject()
        XCTAssertTrue(lane.acquire(active, kind: .explicit))
        lane.enqueueAutomatic(blocked)
        lane.enqueueAutomatic(ready)
        XCTAssertTrue(lane.release(active))
        XCTAssertEqual(lane.waitingCount, 2)
        XCTAssertTrue(lane.nextAutomatic() === blocked)
        lane.enqueueAutomatic(blocked)
        XCTAssertTrue(lane.nextAutomatic() === ready)
        XCTAssertTrue(lane.acquire(ready, kind: .automatic))
        XCTAssertNil(lane.nextAutomatic())
    }

    @MainActor
    func testLatestNavigationKeepsCompleteDestinationWhileEarlierPreparationIsDelayed() async {
        struct Destination: Equatable {
            let scope: ConversationScope
            let mode: SessionMode
            let sessionID: String
        }
        let a = Destination(scope: .project("a"), mode: .code, sessionID: "session-a")
        let b = Destination(scope: .global, mode: .chat, sessionID: "session-b")
        let c = Destination(scope: .project("c"), mode: .code, sessionID: "session-c")
        let (release, continuation) = AsyncStream<Void>.makeStream()
        var requests = ConversationNavigationIntents<Destination>()
        var selected: Destination?
        XCTAssertTrue(requests.submit(a, whileBusy: false))
        let generation = requests.generation
        let preparingA = Task { @MainActor in
            for await _ in release { break }
            if requests.isCurrent(generation) { selected = a }
        }
        await Task.yield()
        XCTAssertFalse(requests.submit(b, whileBusy: true))
        XCTAssertFalse(requests.submit(c, whileBusy: true))
        continuation.yield(())
        continuation.finish()
        await preparingA.value
        XCTAssertNil(selected)
        XCTAssertEqual(requests.takePending(), c)
        XCTAssertNil(requests.takePending())
        XCTAssertTrue(requests.isCurrent(requests.generation))
        selected = c
        XCTAssertEqual(selected, c)
    }

    func testHandoffNeverNavigatesAfterSelectionChanges() {
        let revision = UUID()
        XCTAssertTrue(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: true,
            conversationVisible: true, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: UUID(), ownerMatches: true,
            conversationVisible: true, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: false,
            conversationVisible: true, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: true,
            conversationVisible: false, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: true,
            conversationVisible: true, switching: true))
    }

    func testHandoffDraftRetainsUnpairedUTF16Units() {
        let units: [UInt16] = [0x61, 0xD800, 0x62]
        let draft = SessionHandoffDraft(mode: .chat, sourceCodeUnits: units,
            reference: SessionHandoffReference(sourceScopeKey: "project.p", sourceSessionID: "source",
                sourceMode: .code, targetMode: .chat, cutoff: "fixed", requestID: "request"))
        XCTAssertEqual(draft.displayText, "a\u{FFFD}b")
        XCTAssertEqual(draft.sourceCodeUnits, units)
        XCTAssertEqual(draft.reference.cutoff, "fixed")
    }

    func testSeparateOwnersRetainTurnsAndDraftsAcrossNavigation() async throws {
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        let second = MockConversationSource(sessionMode: .code)
        let firstID = "11111111-1111-4111-8111-111111111111"
        let secondID = "22222222-2222-4222-8222-222222222222"
        first.model.activeSessionId = firstID
        second.model.activeSessionId = secondID
        let firstEntry = try host.retain(source: first, scope: .global, mode: .chat,
                                     sessionID: firstID, workspace: "/home/global", draft: "first draft")
        let secondEntry = try host.retain(source: second, scope: .project("p"), mode: .code,
                                      sessionID: secondID, workspace: "/home/p", draft: "second draft")
        let firstToken = await first.send("work continues")
        XCTAssertNotNil(firstToken)
        let secondToken = await second.send("other work")
        XCTAssertNotNil(secondToken)
        XCTAssertTrue(first.model.streaming)
        XCTAssertTrue(second.model.streaming)
        XCTAssertTrue(host.entry(sessionID: firstID, scope: .global, mode: .chat) === firstEntry)
        XCTAssertTrue(host.entry(sessionID: secondID, scope: .project("p"), mode: .code) === secondEntry)
        XCTAssertNil(host.entry(sessionID: firstID, scope: .global, mode: .code))
        XCTAssertNil(host.entry(sessionID: firstID, scope: .project("p"), mode: .chat))
        second.model.workspaceState = SessionWorkspaceState(sessionId: secondID, cwd: "/home/current",
            projectRoot: "/home/p", additionalDirectories: [], transcriptPath: "")
        // Even if a previously indexed cwd differs from both the initial and
        // live cwd, navigation must reuse the same admitted owner.
        XCTAssertTrue(host.entry(sessionID: secondID, scope: .project("p"), mode: .code) === secondEntry)
        XCTAssertEqual(firstEntry.draft, "first draft")
        XCTAssertEqual(secondEntry.draft, "second draft")
        XCTAssertTrue(first.model.streaming, "navigation must not cancel the hidden owner")
        XCTAssertTrue(second.model.streaming)
    }

    func testDuplicateUUIDIsReusedOnlyForTheSameScopeAndMode() throws {
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        let sessionID = "11111111-1111-4111-8111-111111111111"
        first.model.activeSessionId = sessionID
        let retained = try host.retain(source: first, scope: .global, mode: .chat,
                                       sessionID: sessionID, workspace: "/global", draft: "keep")
        XCTAssertTrue(try host.entryForClaim(sessionID: "sess:" + sessionID.uppercased(), scope: .global, mode: .chat) === retained)
        XCTAssertThrowsError(try host.entryForClaim(sessionID: sessionID, scope: .project("p"), mode: .chat))
        XCTAssertThrowsError(try host.entryForClaim(sessionID: sessionID, scope: .global, mode: .code))
        let duplicate = MockConversationSource(sessionMode: .chat)
        duplicate.model.activeSessionId = sessionID
        XCTAssertThrowsError(try host.retain(source: duplicate, scope: .global, mode: .chat,
                                             sessionID: sessionID, workspace: "/global", draft: "wrong"))
        XCTAssertEqual(host.entries.count, 1)
        XCTAssertEqual(retained.draft, "keep")
    }

    func testLateIdentityClaimsAreAtomicAndCannotRebindAnOwner() throws {
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        let second = MockConversationSource(sessionMode: .code)
        let a = try host.retain(source: first, scope: .global, mode: .chat,
                                sessionID: "", workspace: "/global", draft: "a")
        let b = try host.retain(source: second, scope: .project("p"), mode: .code,
                                sessionID: "", workspace: "/p", draft: "b")
        let id = "22222222-2222-4222-8222-222222222222"
        XCTAssertTrue(host.claimIdentity(id, for: first, scope: .global, mode: .chat))
        XCTAssertFalse(host.claimIdentity("sess:" + id.uppercased(), for: second, scope: .project("p"), mode: .code))
        XCTAssertFalse(host.claimIdentity("33333333-3333-4333-8333-333333333333", for: first, scope: .global, mode: .chat))
        XCTAssertEqual(a.sessionID, id)
        XCTAssertEqual(b.sessionID, "")
        XCTAssertEqual(a.draft, "a")
        XCTAssertEqual(b.draft, "b")
    }

    func testPreparedIdentityTransfersBeforeListenerCallbackAndBlocksScheduler() throws {
        let host = SessionHost()
        let gate = CronOwnerAdmissionGate()
        gate.sessionHost = host
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        let otherID = "22222222-2222-4222-8222-222222222222"
        // Native prepare has authenticated the UUID, but its listener has not
        // yet delivered SessionStarted to the projected model.
        XCTAssertTrue(host.claimIdentity(id, for: source, scope: .global, mode: .chat))
        XCTAssertFalse(host.claimIdentity(otherID, for: source, scope: .global, mode: .chat))
        XCTAssertTrue(host.hasPreparingClaim(sessionID: id))
        let entry = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: "", workspace: "/global", draft: "")
        host.endPreparingClaim(of: source)
        XCTAssertEqual(entry.sessionID, id)
        XCTAssertFalse(host.hasPreparingClaim(sessionID: id))
        XCTAssertFalse(gate.allowsUnclaimedReservation(sessionID: id))
        XCTAssertTrue(try host.entryForClaim(sessionID: id, scope: .global, mode: .chat) === entry)
        source.model.activeSessionId = id
        host.sync(entry)
        XCTAssertEqual(entry.sessionID, id)
    }

    func testFailedReconnectAdmissionKeepsOldOwnerAndDraftForRetry() throws {
        for initiallyUnclaimed in [false, true] {
            let host = SessionHost()
            let id = "11111111-1111-4111-8111-111111111111"
            var catalogAvailable = true
            host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
            let old = MockConversationSource(sessionMode: .chat)
            old.model.activeSessionId = initiallyUnclaimed ? "" : id
            let previous = try host.retain(source: old, scope: .global, mode: .chat,
                sessionID: initiallyUnclaimed ? "" : id, workspace: "/global", draft: "keep")
            previous.draftSourceCodeUnits = [0xD800]
            host.select(previous)
            let replacement = MockConversationSource(sessionMode: .chat)
            replacement.model.activeSessionId = id
            XCTAssertTrue(host.beginReplacement(of: old, with: replacement,
                sessionID: previous.sessionID, scope: .global, mode: .chat))
            if initiallyUnclaimed {
                XCTAssertTrue(host.claimIdentity(id, for: replacement, scope: .global, mode: .chat))
            }
            catalogAvailable = false
            XCTAssertThrowsError(try host.replaceIdleSource(old, with: replacement, scope: .global,
                mode: .chat, sessionID: previous.sessionID, workspace: "/global", draft: "keep"))
            XCTAssertTrue(host.entry(for: old) === previous)
            XCTAssertEqual(host.selectedEntryID, previous.id)
            XCTAssertEqual(previous.draft, "keep")
            XCTAssertEqual(previous.draftSourceCodeUnits, [0xD800])
            host.endPreparingClaim(of: replacement)

            catalogAvailable = true
            let retry = MockConversationSource(sessionMode: .chat)
            retry.model.activeSessionId = id
            XCTAssertTrue(host.beginReplacement(of: old, with: retry,
                sessionID: previous.sessionID, scope: .global, mode: .chat))
            if initiallyUnclaimed { XCTAssertTrue(host.claimIdentity(id, for: retry, scope: .global, mode: .chat)) }
            let next = try host.replaceIdleSource(old, with: retry, scope: .global, mode: .chat,
                sessionID: previous.sessionID, workspace: "/global", draft: "keep")
            XCTAssertNil(host.entry(for: old))
            XCTAssertTrue(host.entry(for: retry) === next)
            XCTAssertTrue(next.voice === previous.voice)
            XCTAssertEqual(next.draft, "keep")
            XCTAssertEqual(next.draftSourceCodeUnits, [0xD800])
        }
    }

    func testReconnectCannotAdoptAnotherRetainedBlankOwner() throws {
        let host = SessionHost()
        let old = MockConversationSource(sessionMode: .chat)
        let other = MockConversationSource(sessionMode: .chat)
        let previous = try host.retain(source: old, scope: .global, mode: .chat,
                                       sessionID: "", workspace: "/global", draft: "first")
        let otherEntry = try host.retain(source: other, scope: .global, mode: .chat,
                                         sessionID: "", workspace: "/global", draft: "second")
        host.select(previous)
        XCTAssertThrowsError(try host.replaceIdleSource(old, with: other, scope: .global,
            mode: .chat, sessionID: "", workspace: "/global", draft: "first"))
        XCTAssertTrue(host.entry(for: old) === previous)
        XCTAssertTrue(host.entry(for: other) === otherEntry)
        XCTAssertEqual(host.selectedEntryID, previous.id)
        XCTAssertEqual(previous.draft, "first")
        XCTAssertEqual(otherEntry.draft, "second")
    }

    func testPersistedOwnerAdmissionRejectsLateIdentityBeforeItIsClaimed() throws {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let entry = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: "", workspace: "/global", draft: "keep")
        let id = "11111111-1111-4111-8111-111111111111"
        host.admitsSessionIdentity = { candidate, scope, mode in
            candidate != id || (scope == .project("owner") && mode == .chat)
        }
        XCTAssertFalse(host.claimIdentity(id, for: source, scope: .global, mode: .chat))
        XCTAssertTrue(entry.sessionID.isEmpty)
        XCTAssertEqual(entry.draft, "keep")
    }

    func testColdRestoreRetainsBlankOwnerWhenCatalogBecomesUnavailable() {
        let host = SessionHost()
        let original = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        original.model.activeSessionId = id
        var catalogAvailable = true
        host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
        host.admitsUnclaimedOwner = { catalogAvailable }
        XCTAssertTrue(host.admitsSessionIdentity?(id, .global, .chat) == true)
        catalogAvailable = false

        let placeholder = MockConversationSource(sessionMode: .chat)
        placeholder.model.activeSessionId = "synthetic-fixture-id"
        let result = host.retainInitial(source: original, scope: .global, mode: .chat,
                                        sessionID: id, workspace: "/old", draft: "saved draft") {
            (placeholder, "/global", "blank draft")
        }
        XCTAssertNotNil(result.rejection)
        XCTAssertEqual(host.entries.count, 1)
        XCTAssertTrue(result.entry.source === placeholder)
        XCTAssertEqual(result.entry.sessionID, "")
        XCTAssertEqual(result.entry.draft, "blank draft")
        XCTAssertNil(host.entry(for: original))
        XCTAssertFalse(host.allowsStartup(of: placeholder))
        XCTAssertFalse(host.claimIdentity(id, for: placeholder, scope: .global, mode: .chat))
        XCTAssertEqual(original.model.activeSessionId, id)
        catalogAvailable = true
        XCTAssertTrue(host.allowsStartup(of: placeholder))
    }

    func testExistingIdentityIsRecheckedAfterItsIndexedOwnerChanges() {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        var indexedScope = ConversationScope.global
        host.admitsUnclaimedOwner = { true }
        host.admitsSessionIdentity = { _, scope, _ in scope == indexedScope }
        XCTAssertTrue(host.allowsStartup(of: source, sessionID: id, scope: .global, mode: .chat))
        indexedScope = .project("other")
        XCTAssertFalse(host.allowsStartup(of: source, sessionID: id, scope: .global, mode: .chat))
        XCTAssertTrue(host.entries.isEmpty)
    }

    func testAdmittedOwnerKeepsItsTurnIfCatalogLaterBecomesUnavailable() async throws {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        source.model.activeSessionId = id
        var catalogAvailable = true
        host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
        host.admitsUnclaimedOwner = { catalogAvailable }
        let owner = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: id, workspace: "/global", draft: "keep")
        let sentToken = await source.send("continue")
        XCTAssertNotNil(sentToken)
        catalogAvailable = false
        XCTAssertTrue(host.allowsStartup(of: source))
        var activityObserved = false
        host.onActivity = { _ in activityObserved = true }
        host.sync(owner)
        XCTAssertTrue(activityObserved)
        XCTAssertTrue(source.model.streaming)
        XCTAssertNil(source.model.error)
        let other = MockConversationSource(sessionMode: .chat)
        other.model.activeSessionId = "22222222-2222-4222-8222-222222222222"
        XCTAssertThrowsError(try host.retain(source: other, scope: .global, mode: .chat,
                                             sessionID: other.model.activeSessionId,
                                             workspace: "/global", draft: ""))
    }
}

#if canImport(harness_runtimeFFI)
@MainActor
final class DurableConversationTurnCollectionTests: XCTestCase {
    func testLateNativeStartupCannotAdoptAnotherRetainedOwner() throws {
        let host = SessionHost()
        let sessionID = "44444444-4444-4444-8444-444444444444"
        let first = MockConversationSource(sessionMode: .chat)
        first.model.activeSessionId = sessionID
        _ = try host.retain(source: first, scope: .global, mode: .chat,
                            sessionID: sessionID, workspace: "/global", draft: "keep")
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                                  projectCwd: "/p", sessionMode: .code, visionDelegationEnabled: false)
        let second = EngineConversationSource(config: config)
        _ = try host.retain(source: second, scope: .project("p"), mode: .code,
                            sessionID: "", workspace: "/p", draft: "other")
        second.applyForTesting(.sessionStarted(sessionId: sessionID, mode: .code))
        XCTAssertEqual(second.model.activeSessionId, "")
        XCTAssertFalse(second.isAvailableForPrompts)
        XCTAssertNotNil(second.model.error)
        XCTAssertTrue(try host.entryForClaim(sessionID: sessionID, scope: .global, mode: .chat)?.source === first)
    }

    func testUnresolvedCheckpointBlocksNextTurnForCanonicalOwner() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let firstID = UUID().uuidString
        let otherID = UUID().uuidString
        func config(_ session: String) -> EngineConfig {
            EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                         projectCwd: "/home/global", sessionMode: .code, visionDelegationEnabled: false,
                         initialSessionId: session)
        }
        let first = DurableConversationTurnClientStore(config: config(firstID.lowercased()))
        let other = DurableConversationTurnClientStore(config: config(otherID))
        try await first.begin(sessionID: "sess:\(firstID)", turnID: 1)
        try await other.begin(sessionID: otherID, turnID: 7)
        do {
            try await first.begin(sessionID: firstID.lowercased(), turnID: 2)
            XCTFail("A turn cannot replace an unresolved checkpoint")
        } catch DurableConversationRecoveryError.unresolvedOwner {
            // Expected: the old owner must be cleared before admitting turn 2.
        }
        let unresolved = try await first.load()
        let otherBefore = try await other.load()
        XCTAssertEqual(unresolved?.turnID, 1)
        XCTAssertEqual(otherBefore?.turnID, 7)

        try await first.clear(turnID: 1)
        try await first.begin(sessionID: firstID, turnID: 2)
        do {
            try await first.clear(turnID: 1)
            XCTFail("A delayed clear cannot retire a different turn")
        } catch DurableConversationRecoveryError.invalidStore {
            // The mismatched clear is fenced; the actual checkpoint remains.
        }
        let current = try await first.load()
        let otherAfter = try await other.load()
        XCTAssertEqual(current?.turnID, 2)
        XCTAssertEqual(otherAfter?.turnID, 7)
        try await first.clear(turnID: 2)
        let cleared = try await first.load()
        XCTAssertNil(cleared, "Clearing the current turn must not revive an older checkpoint")
    }

    func testExactOwnerAndTurnClearPreservesOtherOwners() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        func config(_ session: String, cwd: String = "/home/global") -> EngineConfig {
            EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                         projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: false,
                         initialSessionId: session)
        }
        let first = DurableConversationTurnClientStore(config: config("first"))
        let second = DurableConversationTurnClientStore(config: config("second"))
        try await first.begin(sessionID: "first", turnID: 7)
        try await second.begin(sessionID: "second", turnID: 7)
        try await first.clear(turnID: 7)
        try await first.begin(sessionID: "first", turnID: 8)
        let firstRecord = try await first.load()
        let secondRecord = try await second.load()
        let staleCatalog = try await DurableConversationTurnClientStore(
            config: config("second", cwd: "/home/old-index")).load()
        XCTAssertEqual(firstRecord?.turnID, 8)
        XCTAssertEqual(secondRecord?.turnID, 7)
        XCTAssertEqual(staleCatalog?.turnID, 7,
                       "A stale catalog cwd must not hide the UUID owner's checkpoint")
        try await first.clear(turnID: 8)
        let clearedFirst = try await first.load()
        let reopenedSecond = try await DurableConversationTurnClientStore(config: config("second")).load()
        XCTAssertNil(clearedFirst)
        XCTAssertEqual(reopenedSecond?.turnID, 7)
        try await second.clear(turnID: 7)
        let clearedSecond = try await second.load()
        XCTAssertNil(clearedSecond)
    }
}
#endif

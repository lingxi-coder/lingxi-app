import Darwin
import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI
#endif

@MainActor
final class CronRepositoryTests: XCTestCase {
    func testReceiptStagingDoesNotBlockOtherOwnersOrReleaseItsReservation() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronReceiptAsync.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let staged = expectation(description: "receipt staged off main")
        let release = DispatchSemaphore(value: 0)
        defer { release.signal() }
        let writer = DefaultProjectAtomicWriter(onGuardedStaged: { _ in staged.fulfill(); release.wait() })
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path,
            preferences: ProjectScopedPreferences(defaults: defaults), writer: writer)
        XCTAssertTrue(try index.claims().isEmpty)
        let first = "11111111-1111-4111-8111-111111111111"
        let second = "22222222-2222-4222-8222-222222222222"
        let gate = CronOwnerAdmissionGate()
        let token = UUID()
        XCTAssertTrue(gate.begin(sessionID: first, token: token))
        let row = CronGeneratedSession(id: first, projectID: nil, title: "Scheduled", updatedAtMs: 1,
            sessionTranscriptPath: "/first.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        let pending = Task { try await index.mergeReceipts([row]) }
        await fulfillment(of: [staged], timeout: 2)
        XCTAssertFalse(gate.allowsNewOwner(sessionID: first))
        XCTAssertTrue(gate.allowsNewOwner(sessionID: second), "another owner remains navigable while staging is held")
        release.signal()
        try await pending.value
        gate.finish(token)
        XCTAssertTrue(gate.allowsNewOwner(sessionID: first))
        XCTAssertEqual(try index.claims()[first]?.transcriptPath, "/first.jsonl")
    }

    #if canImport(harness_runtimeFFI)
    func testNativeCronCallbackRetainsReservationWhileConfirmationSuspends() async throws {
        let gate = CronOwnerAdmissionGate()
        let scope = CronScope.global(appSandboxRoot: NSTemporaryDirectory())
        var task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily")
        var automation = CronAutomation()
        automation.sessionMode = .chat
        automation.sessionScopeKey = "global"
        task.automation = automation
        let first = "11111111-1111-4111-8111-111111111111"
        let second = "22222222-2222-4222-8222-222222222222"
        let waiting = expectation(description: "receipt confirmation suspended")
        var resume: CheckedContinuation<Void, Never>?
        let callback = FfiCronOwnerAdmission(gate: gate, scope: scope, task: task, prepare: { _, _, _ in true },
            confirm: { _, _, _ in await withCheckedContinuation { continuation in resume = continuation; waiting.fulfill() } },
            blockAdmission: { _ in XCTFail("valid receipt should be admitted") })
        let candidate = CronOwnerCandidateDto(sessionId: first, sessionTranscriptPath: nil, sessionMode: "chat", sessionScopeKey: "global")
        let began = await callback.beginOwner(candidate: candidate)
        XCTAssertTrue(began)
        let receipt = CronOwnerReservationDto(sessionId: first, sessionTranscriptPath: "/first.jsonl", sessionMode: "chat", sessionScopeKey: "global")
        let pending = Task { await callback.confirmOwner(reservation: receipt) }
        await fulfillment(of: [waiting], timeout: 2)
        XCTAssertFalse(gate.allowsNewOwner(sessionID: first))
        XCTAssertTrue(gate.allowsNewOwner(sessionID: second))
        resume?.resume()
        let accepted = await pending.value
        XCTAssertTrue(accepted)
        XCTAssertTrue(gate.allowsNewOwner(sessionID: first))
    }

    func testNativeCronCallbackMirrorsBeforeReleasingAdmissionGate() async {
        let gate = CronOwnerAdmissionGate()
        let scope = CronScope.global(appSandboxRoot: NSTemporaryDirectory())
        var task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily")
        var automation = CronAutomation()
        automation.sessionMode = .chat
        automation.sessionScopeKey = "global"
        task.automation = automation
        var captured: CronOwnerReservation?
        var blocked = false
        let id = "11111111-1111-4111-8111-111111111111"
        let callback = FfiCronOwnerAdmission(gate: gate, scope: scope, task: task, prepare: { _, _, _ in true }, confirm: { owner, _, _ in
            XCTAssertFalse(gate.allowsNewOwner(sessionID: id))
            captured = owner
        }, blockAdmission: { _ in blocked = true })
        XCTAssertTrue(gate.allowsNewOwner(sessionID: id))
        let candidate = CronOwnerCandidateDto(sessionId: id, sessionTranscriptPath: nil, sessionMode: "chat", sessionScopeKey: "global")
        let began = await callback.beginOwner(candidate: candidate)
        XCTAssertTrue(began)
        let beganAgain = await callback.beginOwner(candidate: candidate)
        XCTAssertFalse(beganAgain)
        XCTAssertFalse(gate.allowsNewOwner(sessionID: id))
        XCTAssertTrue(gate.allowsNewOwner(sessionID: "22222222-2222-4222-8222-222222222222"))
        let dto = CronOwnerReservationDto(sessionId: id,
            sessionTranscriptPath: "/home/lingxi/source.jsonl", sessionMode: "chat", sessionScopeKey: "global")
        let accepted = await callback.confirmOwner(reservation: dto)
        XCTAssertTrue(accepted)
        XCTAssertEqual(captured?.sessionTranscriptPath, "/home/lingxi/source.jsonl")
        XCTAssertEqual(captured?.sessionMode, .chat)
        XCTAssertTrue(gate.allowsNewOwner(sessionID: id))
        XCTAssertFalse(blocked)
    }

    func testNativeCronCallbackRejectsConflictingOwnerAndReleasesGate() async {
        let gate = CronOwnerAdmissionGate()
        let scope = CronScope.global(appSandboxRoot: NSTemporaryDirectory())
        var task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily")
        var automation = CronAutomation()
        automation.sessionMode = .chat
        automation.sessionScopeKey = "global"
        task.automation = automation
        var blocked = false
        let callback = FfiCronOwnerAdmission(gate: gate, scope: scope, task: task,
            prepare: { _, _, _ in true },
            confirm: { _, _, _ in throw ProjectRepositoryError.conflictingSessionScope },
            blockAdmission: { _ in blocked = true })
        let id = "11111111-1111-4111-8111-111111111111"
        let candidate = CronOwnerCandidateDto(sessionId: id, sessionTranscriptPath: nil, sessionMode: "chat", sessionScopeKey: "global")
        let began = await callback.beginOwner(candidate: candidate)
        XCTAssertTrue(began)
        let dto = CronOwnerReservationDto(sessionId: id,
            sessionTranscriptPath: "/home/lingxi/source.jsonl", sessionMode: "chat", sessionScopeKey: "global")
        let accepted = await callback.confirmOwner(reservation: dto)
        XCTAssertFalse(accepted)
        XCTAssertTrue(blocked)
        XCTAssertTrue(gate.allowsNewOwner(sessionID: id))
    }

    func testNativeCronCallbackRejectsReceiptForDifferentReservation() async {
        let gate = CronOwnerAdmissionGate()
        let scope = CronScope.global(appSandboxRoot: NSTemporaryDirectory())
        var automation = CronAutomation()
        automation.sessionMode = .chat
        automation.sessionScopeKey = "global"
        let task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily", automation: automation)
        var blocked = false
        let callback = FfiCronOwnerAdmission(gate: gate, scope: scope, task: task,
            prepare: { _, _, _ in true }, confirm: { _, _, _ in XCTFail("Mismatched owner must not be mirrored") },
            blockAdmission: { _ in blocked = true })
        let id = "11111111-1111-4111-8111-111111111111"
        let candidate = CronOwnerCandidateDto(sessionId: id, sessionTranscriptPath: nil, sessionMode: "chat", sessionScopeKey: "global")
        let began = await callback.beginOwner(candidate: candidate)
        XCTAssertTrue(began)
        let receipt = CronOwnerReservationDto(sessionId: "22222222-2222-4222-8222-222222222222",
            sessionTranscriptPath: "/home/lingxi/other.jsonl", sessionMode: "chat", sessionScopeKey: "global")
        let accepted = await callback.confirmOwner(reservation: receipt)
        XCTAssertFalse(accepted)
        XCTAssertTrue(blocked)
        XCTAssertTrue(gate.allowsNewOwner(sessionID: id))
    }
    #endif
    func testScheduledAdmissionGateOnlyHoldsDuringReservation() {
        let gate = CronOwnerAdmissionGate()
        let first = UUID()
        let second = UUID()
        let a = "11111111-1111-4111-8111-111111111111"
        let b = "22222222-2222-4222-8222-222222222222"
        XCTAssertTrue(gate.begin(sessionID: a, token: first))
        XCTAssertFalse(gate.begin(sessionID: a, token: second))
        XCTAssertFalse(gate.allowsNewOwner(sessionID: a))
        XCTAssertTrue(gate.allowsNewOwner(sessionID: b))
        XCTAssertTrue(gate.begin(sessionID: b, token: second))
        gate.finish(first)
        gate.finish(first) // Completion and cancellation can race.
        XCTAssertTrue(gate.allowsNewOwner(sessionID: a))
        XCTAssertFalse(gate.allowsNewOwner(sessionID: b))
        gate.finish(second)
        XCTAssertTrue(gate.allowsNewOwner(sessionID: b))
    }

    func testOnlyTheReservedSessionWaitsAndCancelledWaitersDoNotInterfere() async throws {
        let gate = CronOwnerAdmissionGate()
        let a = "11111111-1111-4111-8111-111111111111"
        let b = "22222222-2222-4222-8222-222222222222"
        let token = UUID()
        XCTAssertTrue(gate.begin(sessionID: a, token: token))
        try await gate.waitUntilAvailable(sessionID: b)
        let cancelled = Task { try await gate.waitUntilAvailable(sessionID: a) }
        let waiting = Task { try await gate.waitUntilAvailable(sessionID: a) }
        await Task.yield()
        cancelled.cancel()
        do { try await cancelled.value; XCTFail("The cancelled waiter must not be admitted") }
        catch is CancellationError {}
        XCTAssertFalse(gate.allowsNewOwner(sessionID: a))
        gate.finish(token)
        try await waiting.value
        XCTAssertTrue(gate.allowsNewOwner(sessionID: a))
    }

    func testScopeMutationWaitsForPreparingSourceAndBlocksOnlyNewClaims() async throws {
        let gate = CronOwnerAdmissionGate()
        let existing = MockConversationSource(sessionMode: .chat)
        let existingToken = await existing.send("continue")
        XCTAssertNotNil(existingToken)
        XCTAssertTrue(gate.beginSourcePreparation(existing))
        let pending = Task { try await gate.beginScopeMutation() }
        for _ in 0..<100 where !gate.scopeMutationPending { await Task.yield() }
        XCTAssertTrue(gate.scopeMutationPending)
        let other = MockConversationSource(sessionMode: .code)
        XCTAssertFalse(gate.beginSourcePreparation(other))
        XCTAssertFalse(gate.begin(sessionID: "11111111-1111-4111-8111-111111111111", token: UUID()))
        XCTAssertTrue(existing.model.streaming)
        gate.endSourcePreparation(existing)
        let token = try await pending.value
        XCTAssertTrue(gate.scopeMutationPending)
        XCTAssertTrue(existing.model.streaming)
        gate.endScopeMutation(token)
        XCTAssertTrue(gate.beginSourcePreparation(other))
        gate.endSourcePreparation(other)
    }

    func testScopeMutationWaitsForAnEmptyCatalogWrite() async throws {
        let gate = CronOwnerAdmissionGate()
        let catalog = try await gate.beginCatalogUpdate(sessionIDs: [])
        let mutation = Task { try await gate.beginScopeMutation() }
        for _ in 0..<100 where !gate.scopeMutationPending { await Task.yield() }
        XCTAssertTrue(gate.scopeMutationPending)
        gate.finish(catalog)
        let token = try await mutation.value
        XCTAssertTrue(gate.scopeMutationPending)
        gate.endScopeMutation(token)
        XCTAssertFalse(gate.scopeMutationPending)
    }

    func testScheduledAdmissionChecksLiveOwnerWithoutStoppingItsTurn() async throws {
        let host = SessionHost()
        let gate = CronOwnerAdmissionGate()
        gate.sessionHost = host
        let id = "11111111-1111-4111-8111-111111111111"
        let source = MockConversationSource(sessionMode: .chat)
        source.model.activeSessionId = id
        source.model.workspaceState = SessionWorkspaceState(sessionId: id, cwd: "/workspace", projectRoot: "/workspace",
            additionalDirectories: [], transcriptPath: "/catalog/original.jsonl")
        _ = try host.retain(source: source, scope: .global, mode: .chat, sessionID: id, workspace: "/workspace", draft: "keep")
        let sourceToken = await source.send("continue")
        XCTAssertNotNil(sourceToken)
        let token = UUID()
        XCTAssertTrue(gate.begin(sessionID: id, token: token))
        XCTAssertTrue(gate.allowsReservation(sessionID: id,
            owner: SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: "/catalog/original.jsonl"), previouslyKnown: true))
        XCTAssertFalse(gate.allowsReservation(sessionID: id,
            owner: SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: "/catalog/copy.jsonl"), previouslyKnown: true))
        XCTAssertFalse(gate.allowsReservation(sessionID: id,
            owner: SessionOwnerEvidence(scope: .scheduled, mode: .chat, transcriptPath: "/catalog/original.jsonl"), previouslyKnown: false))
        XCTAssertTrue(source.model.streaming)
        gate.finish(token)
        XCTAssertTrue(source.model.streaming)
    }

    func testScheduledReservationCompetesWithPreparingOwnerAndReconnect() throws {
        let host = SessionHost()
        let gate = CronOwnerAdmissionGate()
        gate.sessionHost = host
        host.admitsSessionIdentity = { id, _, _ in gate.allowsNewOwner(sessionID: id) }
        let id = "11111111-1111-4111-8111-111111111111"
        let preparing = MockConversationSource(sessionMode: .chat)
        XCTAssertTrue(host.claimIdentity(id, for: preparing, scope: .global, mode: .chat))
        XCTAssertFalse(gate.allowsUnclaimedReservation(sessionID: id))
        host.endPreparingClaim(of: preparing)
        XCTAssertTrue(gate.allowsUnclaimedReservation(sessionID: id))

        preparing.model.activeSessionId = id
        _ = try host.retain(source: preparing, scope: .global, mode: .chat, sessionID: id, workspace: "/", draft: "keep")
        let replacement = MockConversationSource(sessionMode: .chat)
        XCTAssertTrue(host.beginReplacement(of: preparing, with: replacement, sessionID: id, scope: .global, mode: .chat))
        let owner = SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: "/catalog/one.jsonl")
        XCTAssertFalse(gate.allowsReservation(sessionID: id, owner: owner, previouslyKnown: true))
        host.endPreparingClaim(of: replacement)
        XCTAssertTrue(gate.allowsReservation(sessionID: id, owner: owner, previouslyKnown: true))
        let token = UUID()
        XCTAssertTrue(gate.begin(sessionID: id, token: token))
        XCTAssertFalse(host.beginReplacement(of: preparing, with: replacement, sessionID: id, scope: .global, mode: .chat))
        XCTAssertTrue(host.claimIdentity(id, for: preparing, scope: .global, mode: .chat), "An admitted owner continues")
        gate.finish(token)
    }

    func testScheduledCandidateAndReceiptMatchTheCapturedTaskOwner() {
        let id = "11111111-1111-4111-8111-111111111111"
        let path = "/home/lingxi/original.jsonl"
        var task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
                                  lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily")
        var config = CronAutomation()
        config.runMode = .selectedSession
        config.targetSessionId = id
        config.targetSessionMode = .chat
        config.targetSessionScopeKey = "global"
        config.targetSessionTranscriptPath = path
        task.automation = config
        let selected = CronOwnerCandidate(sessionId: id, sessionMode: .chat, sessionScopeKey: "global", sessionTranscriptPath: path)
        XCTAssertTrue(cronCandidateMatchesTask(selected, task: task))
        XCTAssertFalse(cronCandidateMatchesTask(CronOwnerCandidate(sessionId: id, sessionMode: .chat,
            sessionScopeKey: "global", sessionTranscriptPath: "/home/lingxi/copy.jsonl"), task: task))
        XCTAssertTrue(cronReservationMatchesTask(CronOwnerReservation(sessionId: id, sessionTranscriptPath: path,
            sessionMode: .chat, sessionScopeKey: "global"), task: task))
        XCTAssertFalse(cronReservationMatchesTask(CronOwnerReservation(sessionId: id, sessionTranscriptPath: path,
            sessionMode: .code, sessionScopeKey: "global"), task: task))
        config.runMode = .newSession
        config.sessionMode = .code
        config.sessionScopeKey = "scheduled"
        task.automation = config
        XCTAssertTrue(cronCandidateMatchesTask(CronOwnerCandidate(sessionId: id, sessionMode: .code,
            sessionScopeKey: "scheduled", sessionTranscriptPath: nil), task: task))
        XCTAssertFalse(cronCandidateMatchesTask(CronOwnerCandidate(sessionId: id, sessionMode: .code,
            sessionScopeKey: "scheduled", sessionTranscriptPath: path), task: task))
    }

    func testOwnedTaskSessionRoundTripsTheCompleteSourceAndCopyClearsIt() throws {
        var automation = CronAutomation()
        automation.runMode = .taskSession
        automation.sessionMode = .chat
        automation.sessionScopeKey = "scheduled"
        automation.ownedSessionId = "11111111-1111-4111-8111-111111111111"
        automation.ownedSessionTranscriptPath = "/home/lingxi/original.jsonl"
        automation.ownedSessionMode = .chat
        automation.ownedSessionScopeKey = "scheduled"
        let decoded = try JSONDecoder().decode(CronAutomation.self, from: JSONEncoder().encode(automation))
        XCTAssertEqual(decoded, automation)
        let task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily", automation: decoded)
        let candidate = CronOwnerCandidate(sessionId: automation.ownedSessionId!, sessionMode: .chat,
            sessionScopeKey: "scheduled", sessionTranscriptPath: automation.ownedSessionTranscriptPath)
        XCTAssertTrue(cronCandidateMatchesTask(candidate, task: task))
        XCTAssertFalse(cronCandidateMatchesTask(CronOwnerCandidate(sessionId: candidate.sessionId, sessionMode: .chat,
            sessionScopeKey: "scheduled", sessionTranscriptPath: "/home/lingxi/copy.jsonl"), task: task))
        let repository = CronRepository(appSandboxRoot: NSTemporaryDirectory())
        repository.draft.automation = decoded
        repository.beginCopy()
        XCTAssertNil(repository.draft.automation.ownedSessionId)
        XCTAssertNil(repository.draft.automation.ownedSessionTranscriptPath)
        XCTAssertNil(repository.draft.automation.ownedSessionMode)
        XCTAssertNil(repository.draft.automation.ownedSessionScopeKey)
    }

    func testScheduledRunPreservesItsNativeClaimOwner() throws {
        let json = #"{"id":"run","taskId":"task","ownerPid":42,"claimGeneration":7,"scheduledAt":1,"status":"running","model":"provider/model"}"#
        let run = try JSONDecoder().decode(CronAutomationRun.self, from: Data(json.utf8))
        XCTAssertEqual(run.ownerPid, 42)
        XCTAssertEqual(run.claimGeneration, 7)
        let reopened = try JSONDecoder().decode(CronAutomationRun.self, from: JSONEncoder().encode(run))
        XCTAssertEqual(reopened.ownerPid, 42)
        XCTAssertEqual(reopened.claimGeneration, 7)
    }

    func testChangingTaskSessionToNewSessionClearsTheEntireOwnedSource() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: root.path)
        var original = CronTaskDraft.create()
        original.prompt = "Owned task"
        original.automation.model = "provider/model"
        original.automation.runMode = .taskSession
        original.automation.sessionMode = .chat
        original.automation.sessionScopeKey = "scheduled"
        original.automation.ownedSessionId = "11111111-1111-4111-8111-111111111111"
        original.automation.ownedSessionTranscriptPath = "/home/lingxi/original.jsonl"
        original.automation.ownedSessionMode = .chat
        original.automation.ownedSessionScopeKey = "scheduled"
        let task = try await store.saveConfigured(draft: original)
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider)
        repository.editorMode = .chat
        await repository.refresh()
        repository.beginEdit(scopeID: scope.scopeID, taskID: task.id)
        repository.draft.automation.runMode = .newSession
        await repository.saveDraft()
        let savedTasks = try await store.list()
        let saved = try XCTUnwrap(savedTasks.first)
        XCTAssertEqual(saved.configuration.runMode, .newSession)
        XCTAssertNil(saved.configuration.ownedSessionId)
        XCTAssertNil(saved.configuration.ownedSessionTranscriptPath)
        XCTAssertNil(saved.configuration.ownedSessionMode)
        XCTAssertNil(saved.configuration.ownedSessionScopeKey)
    }
    func testGeneratedOwnerIndexJoinsCatalogAndPreservesConflicts() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: preferences)
        let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("cron/history.json"))
        let scope = CronScope.global(appSandboxRoot: root.path)
        let queued = try await history.claim(scope: scope, taskID: "queued", prompt: "No receipt yet", scheduledAtMs: 1)
        XCTAssertTrue(try index.claims().isEmpty, "A queued run without a session receipt is a valid first-run state")
        let file = root.appendingPathComponent("cron/sessions.json")
        let id = "11111111-1111-4111-8111-111111111111"
        _ = try await history.markTerminal(runID: XCTUnwrap(queued?.runID), status: .succeeded, sessionID: id,
            sessionTranscriptPath: "/home/lingxi/scheduled.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        XCTAssertEqual(try index.claims()[id], SessionOwnerEvidence(scope: .scheduled, mode: .chat,
            transcriptPath: "/home/lingxi/scheduled.jsonl"), "A history receipt owns its UUID before index refresh")
        let row = CronGeneratedSession(id: id, projectID: nil, title: "Scheduled", updatedAtMs: 1,
            sessionTranscriptPath: "/home/lingxi/scheduled.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        try JSONEncoder().encode([row]).write(to: file)
        let repository = ProjectRepository(projectsRoot: root.appendingPathComponent("projects"))
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        store.sessionScopeClaims = { preferences.sessionScopeClaims }
        store.additionalSessionEvidence = { try index.claims() }
        XCTAssertTrue(store.admitsSessionIdentity(id, in: .scheduled, mode: .chat))
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .scheduled, mode: .code))
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .global, mode: .chat))

        let validIndex = try Data(contentsOf: file)
        let corruptIndex = Data("{".utf8)
        try corruptIndex.write(to: file)
        XCTAssertFalse(store.admitsUnclaimedOwner())
        XCTAssertTrue(store.errorMessage?.contains("scheduled") == true)
        XCTAssertEqual(try Data(contentsOf: file), corruptIndex)
        try validIndex.write(to: file)
        var wrongMode = row
        wrongMode.sessionMode = .code
        try JSONEncoder().encode([wrongMode]).write(to: file)
        XCTAssertFalse(store.admitsUnclaimedOwner(), "Index and history cannot disagree about fixed mode")
        try validIndex.write(to: file)

        let global = ProjectSessionSummary(sessionId: id, title: "Conflicting", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/other/copy.jsonl")
        _ = try repository.updateSessions(projectId: nil, sessions: [global])
        let preserved = try Data(contentsOf: file)
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .scheduled, mode: .chat))
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .global, mode: .chat))
        XCTAssertEqual(try Data(contentsOf: file), preserved)

        try FileManager.default.removeItem(at: file)
        let reopened = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: preferences)
        XCTAssertThrowsError(try reopened.claims(), "A previously observed missing index is never recreated")
        XCTAssertFalse(FileManager.default.fileExists(atPath: file.path))
    }

    func testMissingGeneratedIndexWithSessionReceiptIsNotInitialized() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("cron/history.json"))
        let claimed = try await history.claim(scope: .global(appSandboxRoot: root.path), taskID: "task", prompt: "Done", scheduledAtMs: 1)
        _ = try await history.markTerminal(runID: XCTUnwrap(claimed?.runID), status: .succeeded,
                                          sessionID: "11111111-1111-4111-8111-111111111111")
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertThrowsError(try index.claims())
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("cron/sessions.json").path))
    }

    func testGeneratedIndexSymlinkIsRejectedAndPreserved() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let cron = root.appendingPathComponent("cron")
        try FileManager.default.createDirectory(at: cron, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let external = root.appendingPathComponent("other.json")
        let original = Data("[]".utf8)
        try original.write(to: external)
        let file = cron.appendingPathComponent("sessions.json")
        try FileManager.default.createSymbolicLink(at: file, withDestinationURL: external)
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertThrowsError(try index.claims())
        XCTAssertEqual(try Data(contentsOf: external), original)
        XCTAssertTrue(try file.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink == true)
    }

    func testGeneratedIndexDuplicateOwnerKeysAreRejectedAndPreserved() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let file = root.appendingPathComponent("cron/sessions.json")
        let corrupt = Data(#"[{"id":"11111111-1111-4111-8111-111111111111","projectID":null,"title":"x","updatedAtMs":1,"sessionTranscriptPath":"/a","sessionMode":"chat","sessionScopeKey":"global","sessionScopeKey":"scheduled"}]"#.utf8)
        try corrupt.write(to: file)
        XCTAssertThrowsError(try index.claims())
        XCTAssertEqual(try Data(contentsOf: file), corrupt)
    }

    func testStartupScansNativeReceiptsBeforeAllowingOwnerAdmission() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let id = "11111111-1111-4111-8111-111111111111"
        let reservation = CronOwnerReservation(sessionId: id, sessionTranscriptPath: "/home/lingxi/actual.jsonl",
                                               sessionMode: .chat, sessionScopeKey: "scheduled")
        let scope = CronScope.global(appSandboxRoot: root.path)
        // The task and its bounded run history were deleted; the ledger still owns the UUID.
        let provider = FakeStoreProvider(stores: [scope.scopeID: FakeCronStore(tasks: [], reservations: [reservation])])
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider)
        repository.startupOwnerIndex = index
        XCTAssertFalse(repository.ownerAdmissionReady)
        try await repository.prepareOwnerAdmission()
        XCTAssertTrue(repository.ownerAdmissionReady)
        XCTAssertEqual(try index.claims()[id], SessionOwnerEvidence(scope: .scheduled, mode: .chat,
            transcriptPath: "/home/lingxi/actual.jsonl"))
        XCTAssertEqual(try CronRepository.scheduledTranscriptPath(appSandboxRoot: root.path, sessionID: id, mode: .chat),
                       "/home/lingxi/actual.jsonl")
    }

    func testInvalidNativeReceiptFreezesStartupWithoutOverwritingIndex() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let file = root.appendingPathComponent("cron/sessions.json")
        let original = try Data(contentsOf: file)
        let invalid = CronOwnerReservation(sessionId: "11111111-1111-4111-8111-111111111111",
            sessionTranscriptPath: "", sessionMode: .chat, sessionScopeKey: "scheduled")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let repository = CronRepository(appSandboxRoot: root.path,
            storeProvider: FakeStoreProvider(stores: [scope.scopeID: FakeCronStore(tasks: [], reservations: [invalid])]))
        repository.startupOwnerIndex = index
        do { try await repository.prepareOwnerAdmission(); XCTFail("Incomplete receipt must prevent startup") } catch {}
        XCTAssertFalse(repository.ownerAdmissionReady)
        XCTAssertEqual(try Data(contentsOf: file), original)
    }

    func testEarlyCronEntryPointsDoNotRunWhenOwnerPreflightFails() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        let executor = FakeExecutor()
        let repository = CronRepository(appSandboxRoot: root.path, executor: executor)
        repository.startupOwnerIndex = index
        repository.startupOwnerCatalogPreflight = {
            throw ProjectRepositoryError.invalidSessionOwnership("project.bad: unreadable index")
        }
        await repository.reconcile(reason: "foreground")
        await repository.runNow(scopeID: "global", taskID: "task")
        let due = await executor.dueCalls
        let manual = await executor.manualCalls
        XCTAssertTrue(due.isEmpty)
        XCTAssertTrue(manual.isEmpty)
        XCTAssertFalse(repository.ownerAdmissionReady)
    }

    func testRefreshPreservesOwnerIndexWhenHistoryContradictsIt() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let id = "11111111-1111-4111-8111-111111111111"
        let row = CronGeneratedSession(id: id, projectID: nil, title: "Existing", updatedAtMs: 1,
            sessionTranscriptPath: "/home/lingxi/original.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        let file = root.appendingPathComponent("cron/sessions.json")
        let original = try JSONEncoder().encode([row])
        try original.write(to: file)
        let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("cron/history.json"))
        let run = try await history.claim(scope: .global(appSandboxRoot: root.path), taskID: "task", prompt: "Conflict", scheduledAtMs: 1)
        _ = try await history.markTerminal(runID: XCTUnwrap(run?.runID), status: .succeeded, sessionID: id,
            sessionTranscriptPath: "/home/lingxi/other.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        let repository = CronRepository(appSandboxRoot: root.path, historyStore: history)
        repository.startupOwnerIndex = index
        await repository.refresh()
        XCTAssertEqual(try Data(contentsOf: file), original)
        XCTAssertNotNil(repository.state.errorMessage)
        XCTAssertThrowsError(try index.claims())
    }

    func testCorruptReservationLedgerFreezesStartupAndPreservesFiles() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let directory = root.appendingPathComponent("cron/global")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let ledger = directory.appendingPathComponent("owner-reservations.json")
        let corrupt = Data("{".utf8)
        try corrupt.write(to: ledger)
        let indexFile = root.appendingPathComponent("cron/sessions.json")
        let original = try Data(contentsOf: indexFile)
        let repository = CronRepository(appSandboxRoot: root.path)
        repository.startupOwnerIndex = index
        do { try await repository.prepareOwnerAdmission(); XCTFail("Unreadable ledger must prevent startup") } catch {}
        XCTAssertFalse(repository.ownerAdmissionReady)
        XCTAssertTrue(repository.state.errorMessage?.contains("scheduled") == true)
        XCTAssertEqual(try Data(contentsOf: ledger), corrupt)
        XCTAssertEqual(try Data(contentsOf: indexFile), original)
    }

    func testStartupDoesNotAdmitWhenSchedulerScopesChangeDuringScan() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let provider = PausingCronStoreProvider()
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider)
        repository.startupOwnerIndex = index
        var scopes = [CronScope.global(appSandboxRoot: root.path)]
        repository.startupOwnerScopes = { scopes }
        await provider.pauseNextStore()
        let scan = Task { try await repository.prepareOwnerAdmission() }
        await provider.waitUntilPaused()
        scopes.append(CronScope(scopeID: "project", projectID: "project", projectName: "Project",
                                projectCwd: "/workspace", guestWorkspacePath: nil))
        await provider.releaseStore()
        do { try await scan.value; XCTFail("A newly discovered scheduler scope must be scanned") } catch {}
        XCTAssertFalse(repository.ownerAdmissionReady)
        XCTAssertTrue(try index.claims().isEmpty)
    }

    func testNewProjectScopeIsScannedBeforeAnotherOwnerIsAdmitted() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let global = CronScope.global(appSandboxRoot: root.path)
        let project = CronScope(scopeID: "new-project", projectID: "new-project", projectName: "New",
                                projectCwd: "/project", guestWorkspacePath: nil)
        let id = "11111111-1111-4111-8111-111111111111"
        let owner = CronOwnerReservation(sessionId: id, sessionTranscriptPath: "/home/lingxi/project/session.jsonl",
                                         sessionMode: .chat, sessionScopeKey: "project.new-project")
        let provider = FakeStoreProvider(stores: [global.scopeID: FakeCronStore(tasks: []),
                                                  project.scopeID: FakeCronStore(tasks: [], reservations: [owner])])
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider)
        repository.startupOwnerIndex = index
        var scopes = [global]
        repository.startupOwnerScopes = { scopes }
        try await repository.prepareOwnerAdmission()
        XCTAssertTrue(repository.ownerAdmissionIsCurrent)
        XCTAssertNil(try index.claims()[id])
        scopes.append(project)
        XCTAssertFalse(repository.ownerAdmissionIsCurrent)
        try await repository.prepareOwnerAdmission()
        XCTAssertTrue(repository.ownerAdmissionIsCurrent)
        XCTAssertEqual(try index.claims()[id], SessionOwnerEvidence(scope: .project("new-project"), mode: .chat,
                                                                    transcriptPath: owner.sessionTranscriptPath))
    }

    func testConflictingNativeReceiptDoesNotReplaceExistingOwnerIndex() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "CronOwnerIndex.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let index = CronSessionOwnershipIndex(appSandboxRoot: root.path, preferences: ProjectScopedPreferences(defaults: defaults))
        XCTAssertTrue(try index.claims().isEmpty)
        let file = root.appendingPathComponent("cron/sessions.json")
        let original = try Data(contentsOf: file)
        let id = "11111111-1111-4111-8111-111111111111"
        let projects = ProjectRepository(projectsRoot: root.appendingPathComponent("projects"))
        let row = ProjectSessionSummary(sessionId: id, title: "Global", messageCount: 1, relativeTime: "Now",
            updatedAt: Date(), mode: .chat, transcriptPath: "/home/lingxi/original.jsonl")
        _ = try projects.updateSessions(projectId: nil, sessions: [row])
        let store = ProjectStore(repository: projects, synchronizer: ProjectWorkspaceSynchronizer(repository: projects))
        store.additionalSessionEvidence = { try index.claims() }
        let conflicting = CronOwnerReservation(sessionId: id, sessionTranscriptPath: "/home/lingxi/copy.jsonl",
                                             sessionMode: .chat, sessionScopeKey: "global")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let repository = CronRepository(appSandboxRoot: root.path,
            storeProvider: FakeStoreProvider(stores: [scope.scopeID: FakeCronStore(tasks: [], reservations: [conflicting])]))
        repository.startupOwnerIndex = index
        repository.startupReceiptValidator = { receipt in
            guard let path = receipt.sessionTranscriptPath,
                  store.admitsSessionEvidence(receipt.id, owner: SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: path)) else {
                throw ProjectRepositoryError.conflictingSessionScope
            }
        }
        do { try await repository.prepareOwnerAdmission(); XCTFail("Conflicting receipt must prevent startup") } catch {}
        XCTAssertFalse(repository.ownerAdmissionReady)
        XCTAssertEqual(try Data(contentsOf: file), original)
        XCTAssertEqual(try store.transcriptPathForSession(id, in: .global, mode: .chat), "/home/lingxi/original.jsonl")
    }

    func testCancelledHistoryWaiterDoesNotCommitAfterLockRelease() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let lockURL = file.appendingPathExtension("lock")
        let fd = lockURL.path.withCString { open($0, O_WRONLY | O_CREAT, 0o600) }
        XCTAssertGreaterThanOrEqual(fd, 0)
        guard fd >= 0 else { return }
        XCTAssertEqual(flock(fd, LOCK_EX), 0)
        let store = CronRunHistoryStore(fileURL: file)
        let task = Task {
            try await store.claim(scope: .global(appSandboxRoot: root.path), taskID: "cancelled", prompt: "Keep", scheduledAtMs: 1)
        }
        try await Task.sleep(for: .milliseconds(40))
        task.cancel()
        flock(fd, LOCK_UN)
        close(fd)
        do {
            _ = try await task.value
            XCTFail("A cancelled waiter must not commit")
        } catch is CancellationError {}
        let records = try await store.records()
        XCTAssertTrue(records.isEmpty)
    }
    func testSelectedTargetIsRevalidatedAfterStorePreparationSuspends() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let provider = PausingCronStoreProvider()
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider)
        await repository.refresh()
        repository.editorMode = .chat
        let id = "11111111-1111-4111-8111-111111111111"
        var path = "/home/lingxi/.lingxi/projects/original.jsonl"
        repository.sessionChoiceProvider = { mode in
            guard mode == .chat else { return [] }
            return [CronSessionChoice(id: id, title: "Global", scopeID: globalCronScopeID,
                owner: .global, mode: .chat, transcriptPath: path)]
        }
        repository.beginCreate()
        repository.draft.prompt = "Preserve the original source"
        repository.draft.automation.model = "provider/model"
        repository.draft.automation.runMode = .selectedSession
        repository.selectTarget(id)
        await provider.pauseNextStore()
        let saving = Task { await repository.saveDraft() }
        await provider.waitUntilPaused()
        path = "/home/lingxi/.lingxi/projects/copy.jsonl"
        await provider.releaseStore()
        await saving.value
        XCTAssertNotNil(repository.state.errorMessage)
        XCTAssertEqual(repository.draft.automation.targetSessionTranscriptPath, "/home/lingxi/.lingxi/projects/original.jsonl")
        XCTAssertEqual(repository.draft.prompt, "Preserve the original source")
        let store = try await provider.store(for: .global(appSandboxRoot: root.path), appSandboxRoot: root.path)
        let before = try await store.list()
        XCTAssertTrue(before.isEmpty)

        path = "/home/lingxi/.lingxi/projects/original.jsonl"
        repository.editorMode = .code
        await repository.saveDraft()
        XCTAssertNotNil(repository.state.errorMessage)
        repository.editorMode = .chat
        await repository.saveDraft()
        let tasks = try await store.list()
        let saved = try XCTUnwrap(tasks.first?.configuration)
        XCTAssertEqual(saved.targetSessionId, id)
        XCTAssertEqual(saved.targetSessionTranscriptPath, path)
        XCTAssertEqual(saved.targetSessionMode, .chat)
        XCTAssertEqual(saved.targetSessionScopeKey, ConversationScope.global.workspaceKey)
    }

    func testNewCronCapturesModeAndScheduledOwner() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let provider = JsonCronStoreProvider()
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider)
        await repository.refresh()
        repository.editorMode = .chat
        repository.beginCreate()
        repository.draft.prompt = "Run in Chat"
        repository.draft.automation.model = "provider/model"
        await repository.saveDraft()
        let store = try await provider.store(for: .global(appSandboxRoot: root.path), appSandboxRoot: root.path)
        let tasks = try await store.list()
        let saved = try XCTUnwrap(tasks.first?.configuration)
        XCTAssertEqual(saved.sessionMode, .chat)
        XCTAssertEqual(saved.sessionScopeKey, ConversationScope.scheduled.workspaceKey)
    }

    func testCorruptGeneratedIndexIsPreservedAndFreezesAdmission() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = CronRepository(appSandboxRoot: root.path)
        await repository.refresh()
        XCTAssertNotNil(repository.generatedSessionsForAdmission())
        let index = root.appendingPathComponent("cron/sessions.json")
        let corrupt = Data("{".utf8)
        try corrupt.write(to: index)
        XCTAssertNil(repository.generatedSessionsForAdmission())
        await repository.refresh()
        XCTAssertNil(repository.generatedSessionsForAdmission())
        XCTAssertFalse(repository.state.scopes.isEmpty, "Unrelated task access should remain available")
        XCTAssertTrue(repository.state.errorMessage?.contains("scheduled") == true)
        XCTAssertEqual(try Data(contentsOf: index), corrupt)
    }

    func testScheduledRestoreRequiresOneExactSource() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let directory = root.appendingPathComponent("cron")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let id = "11111111-1111-4111-8111-111111111111"
        let row = CronGeneratedSession(id: id, projectID: nil, title: "Result", updatedAtMs: 1,
            sessionTranscriptPath: "/home/lingxi/original.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        let index = directory.appendingPathComponent("sessions.json")
        try JSONEncoder().encode([row]).write(to: index)
        XCTAssertEqual(try CronRepository.scheduledTranscriptPath(appSandboxRoot: root.path, sessionID: id, mode: .chat), row.sessionTranscriptPath)
        XCTAssertThrowsError(try CronRepository.scheduledTranscriptPath(appSandboxRoot: root.path, sessionID: id, mode: .code))
        var conflicting = row
        conflicting.sessionTranscriptPath = "/home/lingxi/copy.jsonl"
        try JSONEncoder().encode([row, conflicting]).write(to: index)
        XCTAssertThrowsError(try CronRepository.scheduledTranscriptPath(appSandboxRoot: root.path, sessionID: id, mode: .chat))
    }
    func testConfiguredTaskPausesPersistsAndResumesInFuture() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        let file = directory.appendingPathComponent("tasks.json")
        let store = JsonCronStoreClient(fileURL: file, now: { 1_700_000_000_000 }, makeID: { "configured" }, calendar: calendar)
        var draft = CronTaskDraft.create()
        draft.prompt = "Summarize changes"
        draft.automation.model = "provider/model"
        draft.automation.reasoning = CronReasoning(selection: "high")
        draft.automation.runMode = .taskSession
        draft.automation.notificationPolicy = .failed
        let created = try await store.saveConfigured(draft: draft)
        var config = created.configuration
        config.status = .paused
        _ = try await store.updateAutomation(taskID: created.id, automation: config)
        let reopened = JsonCronStoreClient(fileURL: file, now: { 1_700_100_000_000 }, makeID: { "other" }, calendar: calendar)
        let paused = try await reopened.list()
        XCTAssertEqual(paused.first?.configuration, config)
        let due = try await reopened.dueOccurrences(nowMs: 1_700_100_000_000)
        XCTAssertTrue(due.isEmpty)
        config.status = .active
        let resumed = try await reopened.updateAutomation(taskID: created.id, automation: config)
        XCTAssertGreaterThan(try XCTUnwrap(resumed.nextFireMs), 1_700_100_000_000)
        XCTAssertEqual(resumed.configuration.model, "provider/model")
        XCTAssertEqual(resumed.configuration.reasoning.selection, "high")
    }

    func testConfiguredOneShotRetainsCompletedRecord() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = JsonCronStoreClient(fileURL: directory.appendingPathComponent("tasks.json"), now: { 1_700_000_000_000 }, makeID: { "once" }, calendar: Calendar(identifier: .gregorian))
        var draft = CronTaskDraft.create()
        draft.prompt = "Once"
        draft.recurring = false
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        _ = try await store.acknowledgeOccurrence(taskID: task.id, scheduledAtMs: XCTUnwrap(task.nextFireMs))
        let completed = try await store.list()
        XCTAssertEqual(completed.count, 1)
        XCTAssertEqual(completed.first?.status, .completed)
        XCTAssertNil(completed.first?.nextFireMs)
    }

    func testNotificationPolicyAndReasoningRoundTrip() throws {
        var config = CronAutomation()
        config.notificationPolicy = .failed
        config.reasoning = CronReasoning(selection: "budget:8192")
        let decoded = try JSONDecoder().decode(CronAutomation.self, from: JSONEncoder().encode(config))
        XCTAssertEqual(decoded.reasoning.selection, "budget:8192")
        XCTAssertFalse(decoded.shouldNotify(.succeeded))
        XCTAssertFalse(decoded.shouldNotify(.running))
        XCTAssertTrue(decoded.shouldNotify(.failed))
        XCTAssertTrue(decoded.shouldNotify(.interrupted))
        config.notificationPolicy = .none
        XCTAssertFalse(config.shouldNotify(.failed))
    }

    func testSaveValidationKeepsDraft() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let repository = CronRepository(appSandboxRoot: directory.path)
        await repository.refresh()
        repository.beginCreate()
        repository.draft.prompt = "Keep this prompt"
        await repository.saveDraft()
        XCTAssertEqual(repository.draft.prompt, "Keep this prompt")
        XCTAssertNotNil(repository.state.errorMessage)
        XCTAssertFalse(repository.routePath.isEmpty)
    }

    func testArchivingTargetPausesTaskAndCancelsOnlyQueuedRun() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let scope = CronScope.global(appSandboxRoot: directory.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: directory.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Follow this chat"
        draft.automation.model = "provider/model"
        draft.automation.runMode = .selectedSession
        draft.automation.targetSessionId = "selected-chat"
        let task = try await store.saveConfigured(draft: draft)
        let history = CronRunHistoryStore(fileURL: directory.appendingPathComponent("history.json"))
        let queued = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 100)
        let running = try await history.claim(scope: scope, taskID: "already-running", prompt: "Running", scheduledAtMs: 100)
        _ = try await history.markRunning(runID: XCTUnwrap(running?.runID), attempt: 1)
        let repository = CronRepository(appSandboxRoot: directory.path, storeProvider: provider, historyStore: history)
        await repository.refresh()
        try await repository.pauseTasksForArchivedSession(projectID: nil, sessionID: "selected-chat")
        let paused = try await store.list()
        let run = try await history.record(runID: XCTUnwrap(queued?.runID))
        XCTAssertEqual(paused.first?.status, .paused)
        XCTAssertTrue(paused.first?.configuration.statusReason?.contains("archived") == true)
        XCTAssertEqual(run?.status, .cancelled)
        let ongoing = try await history.record(runID: XCTUnwrap(running?.runID))
        XCTAssertEqual(ongoing?.status, .running)
    }

    func testScheduledScopeHasDedicatedPreferenceAndWorkspaceIdentity() {
        XCTAssertEqual(ConversationScope(workspaceKey: "scheduled"), .scheduled)
        XCTAssertNotEqual(ConversationScope.scheduled.preferenceScope, ConversationScope.global.preferenceScope)
        XCTAssertEqual(CronScope.global(appSandboxRoot: "/app").guestWorkspacePath, LXISHGuestPaths.workspace("scheduled"))
    }

    func testProjectCronScopeKeepsItsStorageRootWhenSessionDirectoryChanges() {
        let id = "11111111-1111-4111-8111-111111111111"
        var record = ProjectRecord(id: id, name: "Project", storageKind: .internal, createdAt: Date(), updatedAt: Date())
        let root = ProjectWorkspace(projectId: id, hostURL: URL(fileURLWithPath: "/managed/project/workspace"),
                                    guestPath: "/home/lingxi/projects/project/workspace")
        var project = ProjectSnapshot(record: record, workspace: root, sourceWorkspace: root, sessions: [])
        let before = CronScope.project(project)
        record.activeDirectory = "/home/lingxi/other"
        project.record = record
        project.workspace = ProjectWorkspace(projectId: id, hostURL: URL(fileURLWithPath: "/managed/other"),
                                             guestPath: record.activeDirectory)
        let after = CronScope.project(project)
        XCTAssertEqual(before, after)
        XCTAssertEqual(after.projectCwd, "/managed/project/workspace")
        XCTAssertEqual(after.guestWorkspacePath, "/home/lingxi/projects/project/workspace")
    }

    func testRecoveryUsesNativeTerminalResultBeforeMarkingInterrupted() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let scope = CronScope.global(appSandboxRoot: directory.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: directory.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Completed before host stopped"
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        var configuration = task.configuration
        configuration.status = .completed
        configuration.runs = [CronAutomationRun(id: "native-run", taskId: task.id, scheduledAt: 200,
            status: .succeeded, model: "provider/model", sessionId: "result-chat", summary: "Done")]
        _ = try await store.updateAutomation(taskID: task.id, automation: configuration)
        let history = CronRunHistoryStore(fileURL: directory.appendingPathComponent("history.json"))
        let pending = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 200)
        _ = try await history.markRunning(runID: XCTUnwrap(pending?.runID), attempt: 1)
        let notifier = FakeNotifier()
        let repository = CronRepository(appSandboxRoot: directory.path, storeProvider: provider, historyStore: history, notifier: notifier)
        await repository.refresh() // Establish the empty index before recovery records a result.
        await repository.reconcile(reason: "restart")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertEqual(repository.state.history.first?.sessionID, "result-chat")
        XCTAssertEqual(repository.state.generatedSessions.first?.id, "result-chat")
        await repository.reconcile(reason: "again")
        let notifications = await notifier.payloads
        XCTAssertEqual(notifications.count, 1)
    }

    #if canImport(harness_runtimeFFI)
    func testNativeConfiguredStoreRoundTripsAndHonorsPause() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let handle = try buildIosCronStore(appSandboxRoot: directory.path, managedRoot: directory.appendingPathComponent("runtime", isDirectory: true).path, projectCwd: nil)
        var configuration = CronAutomation()
        configuration.model = "provider/model"
        configuration.reasoning = CronReasoning(selection: "high")
        configuration.runMode = .taskSession
        configuration.sessionMode = .code
        configuration.sessionScopeKey = ConversationScope.scheduled.workspaceKey
        let created = try await handle.createConfigured(cronExpr: "0 9 * * *", prompt: "Native round trip", recurring: true,
            automationJson: String(decoding: JSONEncoder().encode(configuration), as: UTF8.self))
        let decoded = try JSONDecoder().decode(CronAutomation.self, from: Data(XCTUnwrap(created.automationJson).utf8))
        XCTAssertEqual(decoded.model, configuration.model)
        XCTAssertEqual(decoded.reasoning, configuration.reasoning)
        XCTAssertEqual(decoded.runMode, .taskSession)
        configuration.status = .paused
        _ = try await handle.updateAutomation(id: created.id, automationJson: String(decoding: JSONEncoder().encode(configuration), as: UTF8.self))
        let due = await handle.dueOccurrences(nowMs: UInt64(Date().timeIntervalSince1970 * 1000) + 86_400_000)
        XCTAssertTrue(due.isEmpty)
        let listed = await handle.list()
        XCTAssertEqual(listed.count, 1)
        let deleted = await handle.delete(id: created.id)
        XCTAssertTrue(deleted)
    }
    #endif

    func testRefreshLoadsGlobalAndProjectScopes() async throws {
        let global = CronScope(
            scopeID: globalCronScopeID,
            projectID: nil,
            projectName: "全局",
            projectCwd: nil,
            guestWorkspacePath: "/home/lingxi/lingxi/global"
        )
        let project = CronScope(
            scopeID: "project-a",
            projectID: "project-a",
            projectName: "项目 A",
            projectCwd: "/tmp/project-a",
            guestWorkspacePath: "/home/lingxi/lingxi/project-a"
        )
        let globalTask = CronTaskRecord(
            id: "global-task",
            cron: "0 9 * * *",
            prompt: "全局晨报",
            createdAtMs: 1,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 9_000,
            human: "每天 09:00"
        )
        let projectTask = CronTaskRecord(
            id: "project-task",
            cron: "0 10 * * *",
            prompt: "项目日报",
            createdAtMs: 2,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 10_000,
            human: "每天 10:00"
        )

        let provider = FakeStoreProvider(
            stores: [
                global.scopeID: FakeCronStore(tasks: [globalTask]),
                project.scopeID: FakeCronStore(tasks: [projectTask]),
            ]
        )
        let repository = makeRepository(
            scopes: [global, project],
            activeScopeID: project.scopeID,
            storeProvider: provider
        )

        await repository.refresh()

        XCTAssertEqual(repository.state.scopes.map(\.scopeID), [global.scopeID, project.scopeID])
        XCTAssertEqual(repository.state.activeScopeID, project.scopeID)
        XCTAssertEqual(repository.state.tasks.count, 2)
        XCTAssertEqual(Set(repository.state.tasks.map(\.scope.projectName)), ["全局", "项目 A"])

        repository.beginCreate()
        XCTAssertEqual(repository.draft.scopeID, global.scopeID, "New tasks must not inherit the selected project")
        repository.beginCreate(scopeID: project.scopeID)
        XCTAssertEqual(repository.draft.scopeID, project.scopeID)
    }

    func testSchedulingPolicyUsesBestEffortBackgroundAndEarliestTask() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let store = FakeCronStore(tasks: [
            CronTaskRecord(
                id: "late",
                cron: "0 11 * * *",
                prompt: "晚一点",
                createdAtMs: 1,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 11_000,
                human: "每天 11:00"
            ),
            CronTaskRecord(
                id: "early",
                cron: "0 8 * * *",
                prompt: "早一点",
                createdAtMs: 2,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 8_000,
                human: "每天 08:00"
            ),
        ])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            scheduler: scheduler
        )

        await repository.refresh()

        XCTAssertEqual(repository.state.scheduling.mode, CronSchedulingMode.bestEffortBackground)
        XCTAssertEqual(repository.state.scheduling.nextEarliestAtMs, 8_000)
        let scheduledEarliestTimes = await scheduler.scheduledEarliestTimes
        XCTAssertEqual(scheduledEarliestTimes, [8_000])
    }

    func testUnsupportedScheduleNeverArmsTheBackgroundWake() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let store = FakeCronStore(tasks: [
            CronTaskRecord(
                id: "too-frequent",
                cron: "*/5 * * * *",
                prompt: "每 5 分钟",
                createdAtMs: 1,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 5_000,
                human: "每 5 分钟",
                unsupportedReason: "recurring tasks must be at least 15 minutes apart"
            ),
            CronTaskRecord(
                id: "supported",
                cron: "0 8 * * *",
                prompt: "早一点",
                createdAtMs: 2,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 8_000,
                human: "每天 08:00"
            ),
        ])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            scheduler: scheduler
        )

        await repository.refresh()

        // The engine's `dueOccurrences` filters unsupported tasks out, so waking
        // the app for them would run nothing; the earliest wake must skip them.
        XCTAssertEqual(repository.state.scheduling.nextEarliestAtMs, 8_000)
        let scheduledEarliestTimes = await scheduler.scheduledEarliestTimes
        XCTAssertEqual(scheduledEarliestTimes, [8_000])
        XCTAssertEqual(repository.state.tasks.count, 2, "unsupported tasks stay listed so the user can fix them")
        XCTAssertFalse(repository.state.tasks.first { $0.task.id == "too-frequent" }!.task.mobileSupported)
    }

    func testBackgroundTransitionReArmsWakeWithoutExecuting() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let store = FakeCronStore(tasks: [
            CronTaskRecord(
                id: "due-now",
                cron: "* * * * *",
                prompt: "已到期",
                createdAtMs: 0,
                lastFiredAtMs: nil,
                recurring: true,
                nextFireMs: 1,
                human: "每分钟"
            ),
        ])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let executor = FakeExecutor(dueResult: .failure(SimpleError("must not execute while backgrounding")))
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            scheduler: scheduler
        )

        await repository.handleSceneDidEnterBackground()

        let scheduledEarliestTimes = await scheduler.scheduledEarliestTimes
        XCTAssertEqual(scheduledEarliestTimes, [1])
        let dueCalls = await executor.dueCalls
        XCTAssertEqual(dueCalls.count, 0, "backgrounding only hands the next fire to the OS")
    }

    func testReconcileDeduplicatesRepeatedDeliveryAndRecordsHistory() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let scheduler = FakeScheduler(mode: .bestEffortBackground, note: "系统调度不是精确闹钟")
        let notifier = FakeNotifier()
        let executor = FakeExecutor(
            dueResult: .success(
                CronExecutionOutcome(
                    status: .succeeded,
                    resultText: "完成",
                    errorMessage: nil,
                    errorKind: nil
                )
            )
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            scheduler: scheduler,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "test-pass-1")
        await repository.reconcile(reason: "test-pass-2")

        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.succeeded)
        let acknowledgedCount = await store.acknowledgedOccurrences.count
        let notificationCount = await notifier.payloads.count
        let dueRunCount = await executor.dueCalls.count
        XCTAssertEqual(acknowledgedCount, 1)
        XCTAssertEqual(notificationCount, 1)
        XCTAssertEqual(dueRunCount, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    /// A transient failure at wake time must NOT consume the occurrence: the run
    /// is parked with its attempt count and the next wake retries it, the way
    /// Android's worker returns `Result.retry()`.
    func testTransientFailureParksTheOccurrenceAndTheNextWakeResumesIt() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-transient",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let notifier = FakeNotifier()
        let offline = CronExecutionError(kind: .network, message: "offline", statusOverride: nil)
        let executor = SequencedExecutor(
            dueResults: Array(repeating: .failure(offline), count: 3),
            fallback: .success(
                CronExecutionOutcome(status: .succeeded, resultText: "完成", errorMessage: nil, errorKind: nil)
            )
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "wake-1")

        // Parked, not finished: no acknowledgement, no notification, and the run
        // keeps the attempts this wake spent.
        var acknowledged = await store.acknowledgedOccurrences.count
        var notified = await notifier.payloads.count
        var dueCalls = await executor.dueCallCount
        XCTAssertEqual(acknowledged, 0, "a retryable failure must not consume the occurrence")
        XCTAssertEqual(notified, 0)
        XCTAssertEqual(dueCalls, 3, "one wake spends its in-process attempt budget")
        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.queued)
        XCTAssertEqual(repository.state.history.first?.attempt, 3)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 1_000, "still due")

        await repository.reconcile(reason: "wake-2")

        acknowledged = await store.acknowledgedOccurrences.count
        notified = await notifier.payloads.count
        dueCalls = await executor.dueCallCount
        XCTAssertEqual(dueCalls, 4, "the next wake resumes the same run")
        XCTAssertEqual(repository.state.history.count, 1, "resumed, not re-claimed")
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.succeeded)
        XCTAssertEqual(repository.state.history.first?.attempt, 4)
        XCTAssertEqual(acknowledged, 1)
        XCTAssertEqual(notified, 1)
    }

    /// Once the cross-wake attempt budget is spent the failure becomes terminal
    /// and the occurrence is acknowledged, so a permanently offline device does
    /// not retry the same fire forever (Android `MAX_EXECUTION_ATTEMPTS`).
    func testExhaustedRetriesAcknowledgeAndReportTheFailure() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-exhausted",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let notifier = FakeNotifier()
        let executor = SequencedExecutor(
            dueResults: [],
            fallback: .failure(CronExecutionError(kind: .network, message: "offline", statusOverride: nil))
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "wake-1")
        var acknowledged = await store.acknowledgedOccurrences.count
        XCTAssertEqual(acknowledged, 0)

        await repository.reconcile(reason: "wake-2")

        acknowledged = await store.acknowledgedOccurrences.count
        let notified = await notifier.payloads.count
        let dueCalls = await executor.dueCallCount
        XCTAssertEqual(dueCalls, 5, "five attempts across two wakes, then stop")
        XCTAssertEqual(repository.state.history.first?.status, CronRunStatus.failed)
        XCTAssertEqual(repository.state.history.first?.attempt, 5)
        XCTAssertEqual(acknowledged, 1, "the exhausted occurrence is consumed")
        XCTAssertEqual(notified, 1)

        await repository.reconcile(reason: "wake-3")
        let afterTerminal = await executor.dueCallCount
        XCTAssertEqual(afterTerminal, 5, "a terminal run is never retried")
    }

    func testConcurrentReconcileDoesNotRecoverActiveOccurrenceAsTimedOut() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-concurrent",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let executor = GatedCronExecutor()
        let notifier = FakeNotifier()
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        let first = Task { await repository.reconcile(reason: "concurrent-first") }
        await executor.waitUntilDueCallStarts()
        await repository.reconcile(reason: "concurrent-second")
        await executor.releaseDueCall()
        await first.value

        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertNil(repository.state.history.first?.errorKind)
        let dueCallCount = await executor.dueCallCount
        let acknowledgedCount = await store.acknowledgedOccurrences.count
        let notificationCount = await notifier.payloads.count
        XCTAssertEqual(dueCallCount, 1)
        XCTAssertEqual(acknowledgedCount, 1)
        XCTAssertEqual(notificationCount, 1)
    }

    func testCancellingReconcileDoesNotCommitOrAcknowledgeOccurrence() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-cancelled",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let executor = GatedCronExecutor()
        let notifier = FakeNotifier()
        let historyRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-cancellation-\(UUID().uuidString)", isDirectory: true)
        let historyStore = CronRunHistoryStore(
            fileURL: historyRoot.appendingPathComponent("history.json", isDirectory: false),
            now: { 2_000 },
            newID: { "run-cancelled" }
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            historyStore: historyStore,
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        let reconciliation = Task { await repository.reconcile(reason: "background-task") }
        await executor.waitUntilDueCallStarts()
        reconciliation.cancel()
        await executor.releaseDueCall()
        await reconciliation.value

        var dueCallCount = await executor.dueCallCount
        var acknowledgedCount = await store.acknowledgedOccurrences.count
        var notificationCount = await notifier.payloads.count
        XCTAssertEqual(dueCallCount, 1)
        XCTAssertEqual(acknowledgedCount, 0)
        XCTAssertEqual(notificationCount, 0)
        var persistedHistory = try await historyStore.records()
        XCTAssertEqual(persistedHistory.first?.status, .running)
        XCTAssertNil(persistedHistory.first?.errorKind)

        await repository.reconcile(reason: "recover-after-expiration")

        dueCallCount = await executor.dueCallCount
        acknowledgedCount = await store.acknowledgedOccurrences.count
        notificationCount = await notifier.payloads.count
        XCTAssertEqual(dueCallCount, 1)
        XCTAssertEqual(acknowledgedCount, 1)
        XCTAssertEqual(notificationCount, 0)
        XCTAssertEqual(repository.state.history.first?.status, .interrupted)
        XCTAssertEqual(repository.state.history.first?.errorKind, .system)
        persistedHistory = try await historyStore.records()
        XCTAssertEqual(persistedHistory.first?.status, .interrupted)
        XCTAssertEqual(persistedHistory.first?.errorKind, .system)
    }

    func testReconcileSelfHealsTerminalOccurrenceWithoutRerunOrRenotify() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let notifier = FakeNotifier()
        let executor = FakeExecutor()
        let historyRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-history-\(UUID().uuidString)", isDirectory: true)
        let historyStore = CronRunHistoryStore(
            fileURL: historyRoot.appendingPathComponent("history.json", isDirectory: false),
            now: { 2_000 },
            newID: { "run-terminal" }
        )
        _ = try await historyStore.claim(
            scope: global,
            taskID: task.id,
            prompt: task.prompt,
            scheduledAtMs: 1_000,
            triggeredAtMs: 1_500,
            manual: false
        )
        _ = try await historyStore.markTerminal(
            runID: "run-terminal",
            status: .succeeded,
            resultText: "done",
            errorMessage: nil,
            errorKind: nil
        )

        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            historyStore: historyStore,
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "recover-terminal")

        let dueCalls = await executor.dueCalls.count
        let notifications = await notifier.payloads.count
        let acknowledged = await store.acknowledgedOccurrences.count
        XCTAssertEqual(dueCalls, 0)
        XCTAssertEqual(notifications, 0)
        XCTAssertEqual(acknowledged, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testReconcileAckFailureRecoversMissingNotificationOnceWithoutRerun() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task], ackFailureCount: 1)
        let notifier = FakeNotifier()
        let executor = FakeExecutor(
            dueResult: .success(
                CronExecutionOutcome(
                    status: .succeeded,
                    resultText: "完成",
                    errorMessage: nil,
                    errorKind: nil
                )
            )
        )
        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "ack-fails-once")
        let firstPassDueCalls = await executor.dueCalls.count
        let firstPassNotifications = await notifier.payloads.count
        XCTAssertEqual(firstPassDueCalls, 1)
        XCTAssertEqual(firstPassNotifications, 0)
        XCTAssertNotNil(repository.state.errorMessage)

        await repository.reconcile(reason: "ack-recovers")

        let recoveredDueCalls = await executor.dueCalls.count
        let recoveredNotifications = await notifier.payloads.count
        let acknowledgedOccurrences = await store.acknowledgedOccurrences.count
        XCTAssertEqual(recoveredDueCalls, 1)
        XCTAssertEqual(recoveredNotifications, 1)
        XCTAssertEqual(acknowledgedOccurrences, 1)
        XCTAssertEqual(repository.state.history.count, 1)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
        await repository.reconcile(reason: "again")
        let afterThirdPass = await notifier.payloads.count
        XCTAssertEqual(afterThirdPass, 1)
    }

    func testUnknownScheduledOutcomeIsTerminalAndNeverAutomaticallyReplayed() async {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(id: "unknown", cron: "*/15 * * * *", prompt: "Run", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: 1_000, human: "Every 15 minutes")
        let store = FakeCronStore(tasks: [task])
        let outcome = CronExecutionOutcome(status: .interrupted, resultText: nil,
            errorMessage: "The final outcome could not be saved", errorKind: .system,
            sessionID: "11111111-1111-4111-8111-111111111111", sessionTranscriptPath: "/catalog/exact.jsonl",
            sessionMode: .chat, sessionScopeKey: "scheduled")
        let executor = FakeExecutor(dueResult: .success(outcome))
        let repository = makeRepository(scopes: [global], activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]), executor: executor, now: { 2_000 })
        await repository.reconcile(reason: "first")
        await repository.reconcile(reason: "again")
        let calls = await executor.dueCalls
        XCTAssertEqual(calls.count, 1)
        XCTAssertEqual(repository.state.history.first?.status, .interrupted)
        XCTAssertEqual(repository.state.history.first?.sessionTranscriptPath, "/catalog/exact.jsonl")
    }

    func testReconcileRecoversStartedOccurrenceWithoutRerunOrRenotify() async throws {
        let global = CronScope.global(appSandboxRoot: "/tmp")
        let task = CronTaskRecord(
            id: "cron-1",
            cron: "*/15 * * * *",
            prompt: "发送摘要",
            createdAtMs: 0,
            lastFiredAtMs: nil,
            recurring: true,
            nextFireMs: 1_000,
            human: "每 15 分钟"
        )
        let store = FakeCronStore(tasks: [task])
        let notifier = FakeNotifier()
        let executor = FakeExecutor()
        let historyRoot = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-stale-\(UUID().uuidString)", isDirectory: true)
        let historyStore = CronRunHistoryStore(
            fileURL: historyRoot.appendingPathComponent("history.json", isDirectory: false),
            now: { 2_000 },
            newID: { "run-stale" }
        )
        _ = try await historyStore.claim(
            scope: global,
            taskID: task.id,
            prompt: task.prompt,
            scheduledAtMs: 1_000,
            triggeredAtMs: 1_500,
            manual: false
        )

        _ = try await historyStore.markRunning(runID: "run-stale", attempt: 1)

        let repository = makeRepository(
            scopes: [global],
            activeScopeID: global.scopeID,
            storeProvider: FakeStoreProvider(stores: [global.scopeID: store]),
            historyStore: historyStore,
            executor: executor,
            notifier: notifier,
            now: { 2_000 }
        )

        await repository.reconcile(reason: "recover-stale")

        let dueCalls = await executor.dueCalls.count
        let notifications = await notifier.payloads.count
        let acknowledged = await store.acknowledgedOccurrences.count
        XCTAssertEqual(dueCalls, 0)
        XCTAssertEqual(notifications, 0)
        XCTAssertEqual(acknowledged, 1)
        XCTAssertEqual(repository.state.history.first?.status, .interrupted)
        XCTAssertEqual(repository.state.history.first?.errorKind, .system)
        XCTAssertEqual(repository.state.tasks.first?.task.nextFireMs, 901_000)
    }

    func testRestartRetriesUnstartedClaimOnceForRecurringAndOneShotTasks() async throws {
        for recurring in [true, false] {
            let scope = CronScope.global(appSandboxRoot: "/tmp")
            var task = CronTaskRecord(id: "pending", cron: "*/15 * * * *", prompt: "Run once",
                createdAtMs: 0, lastFiredAtMs: nil, recurring: recurring, nextFireMs: 1_000, human: "Every 15 minutes")
            var configuration = CronAutomation()
            configuration.model = "provider/model"
            task.automation = configuration
            let store = FakeCronStore(tasks: [task])
            let executor = FakeExecutor()
            let notifier = FakeNotifier()
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let file = root.appendingPathComponent("history.json")
            let beforeRestart = CronRunHistoryStore(fileURL: file, now: { 1_500 }, newID: { "persisted-claim" })
            _ = try await beforeRestart.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000)
            let reopened = CronRunHistoryStore(fileURL: file, now: { 2_000 })
            let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(stores: [scope.scopeID: store]),
                historyStore: reopened, executor: executor, notifier: notifier, now: { 2_000 })

            await repository.reconcile(reason: "restart")
            await repository.reconcile(reason: "again")

            let calls = await executor.dueCalls.count
            let acknowledgements = await store.acknowledgedOccurrences.count
            let notifications = await notifier.payloads.count
            XCTAssertEqual(calls, 1)
            XCTAssertEqual(acknowledgements, 1)
            XCTAssertEqual(notifications, 1)
            XCTAssertEqual(repository.state.history.count, 1)
            XCTAssertEqual(repository.state.history.first?.runID, "persisted-claim")
            XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        }
    }

    func testRestartRetriesUnstartedManualClaimOnce() async throws {
        let scope = CronScope.global(appSandboxRoot: "/tmp")
        var task = CronTaskRecord(id: "manual", cron: "*/15 * * * *", prompt: "Manual",
            createdAtMs: 0, lastFiredAtMs: nil, recurring: true, nextFireMs: 900_000, human: "Every 15 minutes")
        var configuration = CronAutomation()
        configuration.model = "provider/model"
        task.automation = configuration
        let store = FakeCronStore(tasks: [task])
        let executor = FakeExecutor()
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let history = CronRunHistoryStore(fileURL: file, now: { 1_000 }, newID: { "manual-claim" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
        let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(stores: [scope.scopeID: store]),
            historyStore: CronRunHistoryStore(fileURL: file), executor: executor)
        await repository.reconcile(reason: "restart")
        await repository.reconcile(reason: "again")
        let calls = await executor.manualCalls.count
        let acknowledgements = await store.acknowledgedOccurrences.count
        XCTAssertEqual(calls, 1)
        XCTAssertEqual(acknowledgements, 0)
        XCTAssertEqual(repository.state.history.first?.runID, "manual-claim")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
    }

    func testBusyScheduledAndManualRunsRemainQueuedThenRetryAfterRestart() async throws {
        for manual in [false, true] {
            let scope = CronScope.global(appSandboxRoot: "/tmp")
            var task = CronTaskRecord(id: "busy", cron: "*/15 * * * *", prompt: "Wait for chat",
                createdAtMs: 0, lastFiredAtMs: nil, recurring: true,
                nextFireMs: manual ? 900_000 : 1_000, human: "Every 15 minutes")
            var configuration = CronAutomation()
            configuration.model = "provider/model"
            task.automation = configuration
            let store = FakeCronStore(tasks: [task])
            let provider = FakeStoreProvider(stores: [scope.scopeID: store])
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let file = root.appendingPathComponent("history.json")
            let history = CronRunHistoryStore(fileURL: file, now: { 1_000 }, newID: { "busy-claim" })
            _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: manual)
            let busy = CronExecutionOutcome(status: .queued, resultText: nil, errorMessage: "Waiting for chat", errorKind: nil)
            let repository = makeRepository(scopes: [scope], storeProvider: provider, historyStore: history,
                executor: FakeExecutor(manualResult: .success(busy), dueResult: .success(busy)))
            await repository.reconcile(reason: "busy")
            let beforeRetry = await store.acknowledgedOccurrences.count
            XCTAssertEqual(beforeRetry, 0)
            XCTAssertEqual(repository.state.history.first?.status, .queued)
            XCTAssertEqual(repository.state.history.first?.attempt, 1)
            let executor = FakeExecutor()
            let restarted = makeRepository(scopes: [scope], storeProvider: provider,
                historyStore: CronRunHistoryStore(fileURL: file), executor: executor)
            await restarted.reconcile(reason: "restart")
            await restarted.reconcile(reason: "again")
            let manualCalls = await executor.manualCalls.count
            let timestamps = await executor.manualTimestamps
            if manual { XCTAssertEqual(timestamps, [1_000]) }
            let dueCalls = await executor.dueCalls.count
            let acknowledgements = await store.acknowledgedOccurrences.count
            XCTAssertEqual(manualCalls + dueCalls, 1)
            XCTAssertEqual(acknowledgements, manual ? 0 : 1)
            XCTAssertEqual(restarted.state.history.count, 1)
            XCTAssertEqual(restarted.state.history.first?.runID, "busy-claim")
            XCTAssertEqual(restarted.state.history.first?.status, .succeeded)
        }
    }

    func testInitialManualExecutionAndRestartRetryReceiveSameIdentity() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        var config = CronAutomation()
        config.model = "provider/model"
        let task = CronTaskRecord(id: "manual", cron: "0 9 * * *", prompt: "Manual", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: 900_000, human: "Daily", automation: config)
        let provider = FakeStoreProvider(stores: [scope.scopeID: FakeCronStore(tasks: [task])])
        let historyFile = root.appendingPathComponent("history.json")
        let history = CronRunHistoryStore(fileURL: historyFile)
        let busy = FakeExecutor(manualResult: .success(CronExecutionOutcome(
            status: .queued, resultText: nil, errorMessage: "Waiting for chat", errorKind: nil)))
        let repository = makeRepository(scopes: [scope], storeProvider: provider, historyStore: history,
            executor: busy, now: { 1_000 })
        await repository.refresh()
        await repository.runNow(scopeID: scope.scopeID, taskID: task.id)
        let initial = await busy.manualTimestamps
        XCTAssertEqual(initial, [1_000])
        let success = FakeExecutor()
        let restarted = makeRepository(scopes: [scope], storeProvider: provider,
            historyStore: CronRunHistoryStore(fileURL: historyFile), executor: success, now: { 20_000 })
        await restarted.reconcile(reason: "restart")
        let retried = await success.manualTimestamps
        XCTAssertEqual(retried, initial)
        XCTAssertEqual(restarted.state.history.first?.status, .succeeded)
    }

    func testObservedHistoryMissingAfterClaimFailsClosedAcrossReopen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let store = CronRunHistoryStore(fileURL: file, newID: { "claimed" })
        let claim = try await store.claim(scope: scope, taskID: "task", prompt: "Run", scheduledAtMs: 1)
        XCTAssertNotNil(claim)
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.appendingPathExtension("observed").path))
        try FileManager.default.removeItem(at: file)
        do { _ = try await store.markRunning(runID: "claimed", attempt: 1); XCTFail("Missing observed history must reject dispatch") }
        catch {}
        let reopened = CronRunHistoryStore(fileURL: file)
        do { _ = try await reopened.claim(scope: scope, taskID: "other", prompt: "Run", scheduledAtMs: 2); XCTFail("Cold reopen must not recreate history") }
        catch {}
        XCTAssertThrowsError(try CronRunHistoryStore.storedSessionReceipts(fileURL: file))
        XCTAssertFalse(FileManager.default.fileExists(atPath: file.path))
    }

    func testInterruptedHistoryInitializationIsNotTreatedAsFresh() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let file = root.appendingPathComponent("history.json")
        try Data("lingxi-cron-history-observed-v1\n".utf8).write(to: file.appendingPathExtension("observed"))
        let store = CronRunHistoryStore(fileURL: file)
        do { _ = try await store.records(); XCTFail("A witness without history is interrupted initialization") }
        catch {}
        XCTAssertFalse(FileManager.default.fileExists(atPath: file.path))
    }

    func testHistoryUpdateRejectsReplacementDuringStaging() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let first = CronRunHistoryStore(fileURL: file, newID: { "claimed" })
        _ = try await first.claim(scope: scope, taskID: "task", prompt: "Run", scheduledAtMs: 1)
        let external = Data("unrelated replacement".utf8)
        let writer = DefaultProjectAtomicWriter(onGuardedStaged: { _ in try? external.write(to: file, options: .atomic) })
        let store = CronRunHistoryStore(fileURL: file, writer: writer)
        do { _ = try await store.markRunning(runID: "claimed", attempt: 1); XCTFail("Changed source must not be published over") }
        catch {}
        XCTAssertEqual(try Data(contentsOf: file), external)
        do { _ = try await CronRunHistoryStore(fileURL: file).records(); XCTFail("Corrupt evidence must stay closed on reopen") }
        catch {}
        XCTAssertEqual(try Data(contentsOf: file), external)
    }

    func testNotificationReservationIsExclusiveAcrossHistoryStoreInstances() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let first = CronRunHistoryStore(fileURL: file, newID: { "terminal" })
        _ = try await first.claim(scope: .global(appSandboxRoot: root.path), taskID: "task", prompt: "Done", scheduledAtMs: 1_000)
        _ = try await first.markTerminal(runID: "terminal", status: .succeeded)
        let second = CronRunHistoryStore(fileURL: file)
        async let claimA = first.claimTerminalNotification(runID: "terminal")
        async let claimB = second.claimTerminalNotification(runID: "terminal")
        let claims = try await [claimA, claimB]
        XCTAssertEqual(claims.compactMap { $0 }.count, 1)
        let reopened = CronRunHistoryStore(fileURL: file)
        let duplicate = try await reopened.claimTerminalNotification(runID: "terminal")
        XCTAssertNil(duplicate)
    }

    /// 🚨 The claim is written BEFORE delivery, so a refusal that keeps it makes
    /// the run permanently un-notifiable: turning the scheduled-run toggle on or
    /// granting permission later re-enters recovery, which hits EEXIST. Both the
    /// Android and desktop mirrors give the key back; this pins that iOS does.
    func testARefusedDeliveryReturnsTheNotificationClaimSoRecoveryCanRetry() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let store = CronRunHistoryStore(fileURL: file, newID: { "terminal" })
        _ = try await store.claim(scope: .global(appSandboxRoot: root.path), taskID: "task", prompt: "Done", scheduledAtMs: 1_000)
        _ = try await store.markTerminal(runID: "terminal", status: .succeeded)

        // Each claim is hoisted out of the assertion: XCTAssert* take a
        // non-async autoclosure, and every call here has a side effect, so the
        // order must stay exactly as written.
        let firstClaim = try await store.claimTerminalNotification(runID: "terminal")
        XCTAssertNotNil(firstClaim)
        let secondClaim = try await store.claimTerminalNotification(runID: "terminal")
        XCTAssertNil(secondClaim, "the claim is exclusive while it is held")
        try await store.releaseTerminalNotification(runID: "terminal")
        let reclaimed = try await store.claimTerminalNotification(runID: "terminal")
        XCTAssertNotNil(reclaimed, "a released claim must be reclaimable")
    }

    func testNotificationReceiptsArePrunedWithHistoryAndStaleStoresCannotReclaim() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let stale = CronRunHistoryStore(fileURL: file)
        var firstID = ""
        for index in 0..<25 {
            let history = CronRunHistoryStore(fileURL: file)
            let run = try await history.claim(scope: scope, taskID: "task", prompt: "Done",
                scheduledAtMs: UInt64(index), triggeredAtMs: UInt64(index))
            let id = try XCTUnwrap(run?.runID)
            if index == 0 { firstID = id }
            _ = try await history.markTerminal(runID: id, status: .succeeded)
            let receipt = try await history.claimTerminalNotification(runID: id)
            XCTAssertNotNil(receipt)
        }
        let markers = file.appendingPathExtension("notification-claims")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: markers.path).count, maxCronRunsPerTask)
        // An instance created before pruning must neither resurrect history nor
        // reacquire the deleted receipt when recovery still holds an old ID.
        let prunedID = firstID
        async let staleWrite = stale.markTerminal(runID: prunedID, status: .succeeded)
        let reopened = CronRunHistoryStore(fileURL: file)
        async let staleClaim = reopened.claimTerminalNotification(runID: prunedID)
        let (write, claim) = try await (staleWrite, staleClaim)
        XCTAssertNil(write)
        XCTAssertNil(claim)
        let retained = try await reopened.records()
        XCTAssertEqual(retained.count, maxCronRunsPerTask)
        for record in retained {
            let duplicate = try await reopened.claimTerminalNotification(runID: record.runID)
            XCTAssertNil(duplicate)
        }
        // Legacy orphan receipts are reclaimed even without a new history write.
        let orphan = markers.appendingPathComponent(String(repeating: "0", count: 64))
        try Data().write(to: orphan)
        _ = try await reopened.claimTerminalNotification(runID: "missing")
        XCTAssertFalse(FileManager.default.fileExists(atPath: orphan.path))
    }

    func testConcurrentHistoryWritersPreserveAllNotificationReceipts() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        try await withThrowingTaskGroup(of: Void.self) { group in
            for index in 0..<20 {
                group.addTask {
                    let store = CronRunHistoryStore(fileURL: file)
                    guard let run = try await store.claim(scope: scope, taskID: "task-\(index)",
                        prompt: "Done", scheduledAtMs: 1_000) else {
                        XCTFail("Independent task claim was lost")
                        return
                    }
                    _ = try await store.markTerminal(runID: run.runID, status: .succeeded)
                    let receipt = try await store.claimTerminalNotification(runID: run.runID)
                    XCTAssertNotNil(receipt)
                }
            }
            try await group.waitForAll()
        }
        let reopened = CronRunHistoryStore(fileURL: file)
        let retained = try await reopened.records()
        XCTAssertEqual(retained.count, 20)
        for run in retained {
            XCTAssertEqual(run.status, .succeeded)
            let duplicate = try await reopened.claimTerminalNotification(runID: run.runID)
            XCTAssertNil(duplicate)
        }
    }

    func testManualRecoveryUsesExactHostOccurrenceMarker() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        let provider = JsonCronStoreProvider()
        let store = try await provider.store(for: scope, appSandboxRoot: root.path)
        var draft = CronTaskDraft.create()
        draft.prompt = "Manual recovery"
        draft.automation.model = "provider/model"
        let task = try await store.saveConfigured(draft: draft)
        var config = task.configuration
        config.runs = [CronAutomationRun(id: "native-manual", taskId: task.id, scheduledAt: 1_010,
            startedAt: 1_011, finishedAt: 1_020, status: .succeeded, model: "provider/model",
            sessionId: "actual-chat", summary: "Actual success", manualOccurrenceAt: 1_000)]
        _ = try await store.updateAutomation(taskID: task.id, automation: config)
        let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("history.json"), newID: { "host-manual" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
        _ = try await history.markRunning(runID: "host-manual", attempt: 1)
        let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider, historyStore: history)
        await repository.reconcile(reason: "restart")
        XCTAssertEqual(repository.state.history.first?.status, .succeeded)
        XCTAssertEqual(repository.state.history.first?.sessionID, "actual-chat")
    }

    func testManualRecoveryDoesNotInferAnOccurrenceFromTimestampOrOtherRuns() async throws {
        for (scheduledAt, ambiguous) in [(UInt64(1_000), false), (1_010, false), (1_010, true)] {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let scope = CronScope.global(appSandboxRoot: root.path)
            let provider = JsonCronStoreProvider()
            let store = try await provider.store(for: scope, appSandboxRoot: root.path)
            var draft = CronTaskDraft.create()
            draft.prompt = "Legacy manual recovery"
            draft.automation.model = "provider/model"
            let task = try await store.saveConfigured(draft: draft)
            var config = task.configuration
            config.runs = [CronAutomationRun(id: "\(task.id)-manual-1010-0", taskId: task.id,
                scheduledAt: scheduledAt, startedAt: 1_011, finishedAt: 1_020, status: .succeeded,
                model: "provider/model", sessionId: "actual-chat", summary: "Actual success")]
            if ambiguous {
                config.runs.append(CronAutomationRun(id: "\(task.id)-manual-1020-1", taskId: task.id,
                    scheduledAt: 1_020, status: .succeeded, model: "provider/model", sessionId: "other-chat"))
            }
            _ = try await store.updateAutomation(taskID: task.id, automation: config)
            let history = CronRunHistoryStore(fileURL: root.appendingPathComponent("history.json"),
                now: { 1_000 }, newID: { "host-manual" })
            _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000, manual: true)
            _ = try await history.markRunning(runID: "host-manual", attempt: 1)
            let executor = FakeExecutor()
            let repository = CronRepository(appSandboxRoot: root.path, storeProvider: provider,
                historyStore: history, executor: executor)
            await repository.reconcile(reason: "upgrade")
            await repository.reconcile(reason: "again")
            XCTAssertEqual(repository.state.history.first?.status, .interrupted)
            XCTAssertNil(repository.state.history.first?.sessionID)
            XCTAssertNil(repository.state.history.first?.resultText)
            let calls = await executor.manualCalls.count
            XCTAssertEqual(calls, 0, "Recovery must never replay a started run")
        }
    }

    func testConcurrentRecoveryClaimsNotificationOnceAndPersistsReservation() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        var config = CronAutomation()
        config.model = "provider/model"
        config.status = .completed
        config.runs = [CronAutomationRun(id: "native", taskId: "task", scheduledAt: 1_000,
            status: .succeeded, model: "provider/model", sessionId: "actual-chat", summary: "Done")]
        let task = CronTaskRecord(id: "task", cron: "0 9 * * *", prompt: "Recovery", createdAtMs: 0,
            lastFiredAtMs: 1_000, recurring: false, nextFireMs: nil, human: "Daily", automation: config)
        let historyFile = root.appendingPathComponent("history.json")
        let history = CronRunHistoryStore(fileURL: historyFile, newID: { "same-host-run" })
        _ = try await history.claim(scope: scope, taskID: task.id, prompt: task.prompt, scheduledAtMs: 1_000)
        _ = try await history.markRunning(runID: "same-host-run", attempt: 1)
        let notifier = FakeNotifier()
        let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(
            stores: [scope.scopeID: FakeCronStore(tasks: [task], recoveryBarrier: true)]),
            historyStore: history, notifier: notifier)
        async let first: Void = repository.reconcile(reason: "foreground")
        async let second: Void = repository.reconcile(reason: "chat-idle")
        _ = await (first, second)
        let notifications = await notifier.payloads
        XCTAssertEqual(notifications.count, 1)
        let reopened = CronRunHistoryStore(fileURL: historyFile)
        let duplicate = try await reopened.claimTerminalNotification(runID: "same-host-run")
        XCTAssertNil(duplicate)
        let saved = try await reopened.record(runID: "same-host-run")
        XCTAssertEqual(saved?.status, .succeeded)
    }

    func testLegacyPolicyMigrationCapturesUnfinishedOnlyAndNeverOverwritesSnapshot() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("history.json")
        let scope = CronScope.global(appSandboxRoot: root.path)
        let history = CronRunHistoryStore(fileURL: file, newID: { "legacy-pending" })
        _ = try await history.claim(scope: scope, taskID: "pending", prompt: "Pending", scheduledAtMs: 1_000)
        try await history.snapshotNotificationPolicy(runID: "legacy-pending", policy: .none)
        let reopened = CronRunHistoryStore(fileURL: file, newID: { "legacy-terminal" })
        try await reopened.snapshotNotificationPolicy(runID: "legacy-pending", policy: .all)
        _ = try await reopened.markTerminal(runID: "legacy-pending", status: .failed)
        let migrated = try await reopened.record(runID: "legacy-pending")
        XCTAssertEqual(migrated?.notificationPolicy, CronNotificationPolicy.none)
        _ = try await reopened.claim(scope: scope, taskID: "terminal", prompt: "Already notified", scheduledAtMs: 1_000)
        _ = try await reopened.markTerminal(runID: "legacy-terminal", status: .succeeded)
        try await reopened.snapshotNotificationPolicy(runID: "legacy-terminal", policy: .all)
        let historical = try await reopened.record(runID: "legacy-terminal")
        XCTAssertNil(historical?.notificationPolicy)
    }

    func testCompletedRunNotificationRecoveryUsesPersistedPolicyWithoutDueOccurrence() async throws {
        let cases: [(CronNotificationPolicy, CronRunStatus, Int)] = [
            (.all, .succeeded, 1), (.failed, .failed, 1), (.failed, .succeeded, 0), (.none, .failed, 0),
        ]
        for (savedPolicy, status, expected) in cases {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let scope = CronScope.global(appSandboxRoot: root.path)
            var config = CronAutomation()
            config.model = "provider/model"
            config.status = .completed
            config.notificationPolicy = savedPolicy == .all ? .none : .all
            let task = CronTaskRecord(id: "one-shot", cron: "0 9 * * *", prompt: "Completed",
                createdAtMs: 0, lastFiredAtMs: 1_000, recurring: false, nextFireMs: nil, human: "Once", automation: config)
            let file = root.appendingPathComponent("history.json")
            let oldHistory = CronRunHistoryStore(fileURL: file, newID: { "run" })
            _ = try await oldHistory.claim(scope: scope, taskID: task.id, prompt: task.prompt,
                scheduledAtMs: 1_000, notificationPolicy: savedPolicy)
            _ = try await oldHistory.markTerminal(runID: "run", status: status)
            let notifier = FakeNotifier()
            let executor = FakeExecutor()
            let repository = makeRepository(scopes: [scope], storeProvider: FakeStoreProvider(
                stores: [scope.scopeID: FakeCronStore(tasks: [task])]),
                historyStore: CronRunHistoryStore(fileURL: file), executor: executor, notifier: notifier)
            await repository.reconcile(reason: "restart")
            await repository.reconcile(reason: "again")
            let count = await notifier.payloads.count
            let executions = await executor.dueCalls.count
            XCTAssertEqual(count, expected)
            XCTAssertEqual(executions, 0)
            XCTAssertEqual(repository.state.history.first?.notificationPolicy, savedPolicy)
        }
    }

    func testCancellationAfterTerminalWriteRecoversNotificationOnRestart() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let scope = CronScope.global(appSandboxRoot: root.path)
        var config = CronAutomation()
        config.model = "provider/model"
        let task = CronTaskRecord(id: "one-shot", cron: "0 9 * * *", prompt: "Run once",
            createdAtMs: 0, lastFiredAtMs: nil, recurring: false, nextFireMs: 1_000, human: "Once", automation: config)
        let store = FakeCronStore(tasks: [task], cancelAfterAcknowledgement: true)
        let provider = FakeStoreProvider(stores: [scope.scopeID: store])
        let file = root.appendingPathComponent("history.json")
        let notifier = FakeNotifier()
        let executor = FakeExecutor()
        let repository = makeRepository(scopes: [scope], storeProvider: provider,
            historyStore: CronRunHistoryStore(fileURL: file), executor: executor, notifier: notifier, now: { 2_000 })
        let firstWake = Task { await repository.reconcile(reason: "background") }
        await firstWake.value
        let beforeRecovery = await notifier.payloads.count
        XCTAssertEqual(beforeRecovery, 0)
        let reopened = CronRunHistoryStore(fileURL: file)
        let saved = try await reopened.records()
        XCTAssertEqual(saved.first?.status, .succeeded)
        let restarted = makeRepository(scopes: [scope], storeProvider: provider, historyStore: reopened,
            executor: executor, notifier: notifier, now: { 3_000 })
        await restarted.reconcile(reason: "restart")
        await restarted.reconcile(reason: "again")
        let recovered = await notifier.payloads.count
        let executions = await executor.dueCalls.count
        XCTAssertEqual(recovered, 1)
        XCTAssertEqual(executions, 1)
    }

    func testDiagnosticsSnapshotSummarizesIdentifierScopeAndCounts() {
        let scope = CronScope(
            scopeID: "project-a",
            projectID: "project-a",
            projectName: "项目 A",
            projectCwd: "/tmp/project-a",
            guestWorkspacePath: "/home/lingxi/lingxi/project-a"
        )
        let run = makeRun(
            runID: "success-1",
            taskID: "task-a",
            triggeredAtMs: 10_000,
            status: .succeeded,
            errorKind: nil
        )
        let active = CronRunRecord(
            runID: "running-1",
            taskID: "task-b",
            scopeID: scope.scopeID,
            projectID: scope.projectID,
            projectName: scope.projectName,
            prompt: "正在运行",
            scheduledAtMs: 11_000,
            triggeredAtMs: 11_000,
            startedAtMs: 11_000,
            finishedAtMs: nil,
            status: .running,
            attempt: 1,
            resultText: nil,
            errorMessage: nil,
            errorKind: nil,
            manual: false
        )
        let state = CronRepositoryState(
            loading: false,
            tasks: [
                CronScopedTask(
                    scope: scope,
                    task: CronTaskRecord(
                        id: "task-a",
                        cron: "0 9 * * *",
                        prompt: "日报",
                        createdAtMs: 1,
                        lastFiredAtMs: nil,
                        recurring: true,
                        nextFireMs: 12_000,
                        human: "每天 09:00"
                    ),
                    activeRun: nil,
                    lastRun: run
                )
            ],
            scopes: [scope],
            activeScopeID: scope.scopeID,
            history: [active, run],
            scheduling: CronSchedulingSnapshot(
                mode: .bestEffortBackground,
                nextEarliestAtMs: 12_000,
                lastReconciledAtMs: 13_000,
                note: "系统尽力调度",
                backgroundTaskIdentifier: cronBackgroundTaskIdentifier
            ),
            errorMessage: nil,
            lastActionMessage: nil
        )

        let diagnostics = state.diagnostics
        XCTAssertEqual(diagnostics.backgroundTaskIdentifier, cronBackgroundTaskIdentifier)
        XCTAssertEqual(diagnostics.schedulingModeTitle, CronSchedulingMode.bestEffortBackground.title)
        XCTAssertEqual(diagnostics.activeScopeName, "项目 A")
        XCTAssertEqual(diagnostics.activeScopeID, "project-a")
        XCTAssertEqual(diagnostics.taskCount, 1)
        XCTAssertEqual(diagnostics.historyCount, 2)
        XCTAssertEqual(diagnostics.activeRunCount, 1)
        XCTAssertEqual(diagnostics.activeRunSummary, "当前 1 个活动运行")
    }

    func testResultCategorySummaryGroupsLatestRunsByType() {
        let history = [
            makeRun(
                runID: "network-new",
                taskID: "task-1",
                triggeredAtMs: 20_000,
                status: .failed,
                errorKind: .network
            ),
            makeRun(
                runID: "network-old",
                taskID: "task-2",
                triggeredAtMs: 10_000,
                status: .failed,
                errorKind: .network
            ),
            makeRun(
                runID: "success-1",
                taskID: "task-3",
                triggeredAtMs: 15_000,
                status: .succeeded,
                errorKind: nil
            ),
            makeRun(
                runID: "credential-1",
                taskID: "task-4",
                triggeredAtMs: 12_000,
                status: .failed,
                errorKind: .missingCredentials
            ),
        ]
        let state = CronRepositoryState(
            loading: false,
            tasks: [],
            scopes: [],
            activeScopeID: globalCronScopeID,
            history: history,
            scheduling: .initial(),
            errorMessage: nil,
            lastActionMessage: nil
        )

        let summaries = state.resultCategorySummaries
        XCTAssertEqual(summaries.map(\.category), [.success, .missingCredentials, .network])
        XCTAssertEqual(summaries.first(where: { $0.category == .network })?.count, 2)
        XCTAssertEqual(summaries.first(where: { $0.category == .network })?.latestRun.runID, "network-new")
        XCTAssertEqual(summaries.first(where: { $0.category == .success })?.title, "成功")
    }

    func testRunResultCategoryMapsExistingErrorTypes() {
        XCTAssertEqual(makeRun(runID: "a", taskID: "1", triggeredAtMs: 1, status: .succeeded, errorKind: nil).resultCategory, .success)
        XCTAssertEqual(makeRun(runID: "b", taskID: "1", triggeredAtMs: 1, status: .failed, errorKind: .missingCredentials).resultCategory, .missingCredentials)
        XCTAssertEqual(makeRun(runID: "c", taskID: "1", triggeredAtMs: 1, status: .failed, errorKind: .network).resultCategory, .network)
        XCTAssertEqual(makeRun(runID: "d", taskID: "1", triggeredAtMs: 1, status: .timedOut, errorKind: .timedOut).resultCategory, .timedOut)
        XCTAssertEqual(makeRun(runID: "e", taskID: "1", triggeredAtMs: 1, status: .cancelled, errorKind: .cancelled).resultCategory, .cancelled)
        XCTAssertEqual(makeRun(runID: "f", taskID: "1", triggeredAtMs: 1, status: .skipped, errorKind: .system).resultCategory, .skipped)
    }

    func testRetainCronRunHistoryHonorsPerTaskAndTotalLimits() {
        let active = CronRunRecord(
            runID: "active",
            taskID: "task-a",
            scopeID: "global",
            projectID: nil,
            projectName: "全局",
            prompt: "running",
            scheduledAtMs: 10,
            triggeredAtMs: 10,
            startedAtMs: 10,
            finishedAtMs: nil,
            status: .running,
            attempt: 1,
            resultText: nil,
            errorMessage: nil,
            errorKind: nil,
            manual: false
        )
        let terminalSameTask = makeRuns(
            prefix: "same",
            count: 30,
            taskID: "task-a",
            status: .succeeded,
            triggeredStart: 1_000,
            errorKind: nil
        )
        let manyOthers = (0..<600).map { index in
            makeRun(
                runID: "other-\(index)",
                taskID: "task-\(index)",
                triggeredAtMs: UInt64(10_000 - index),
                status: .failed,
                errorKind: .system
            )
        }

        let retained = retainCronRunHistory([active] + terminalSameTask + manyOthers)

        XCTAssertLessThanOrEqual(retained.count, maxCronRunsTotal)
        XCTAssertEqual(retained.filter { $0.taskID == "task-a" }.count, maxCronRunsPerTask)
        XCTAssertTrue(retained.contains(where: { $0.runID == "active" }))
    }

    func testErrorClassificationSeparatesCredentialNetworkAndTimeout() {
        let repository = makeRepository()

        XCTAssertEqual(
            repository.classifyExecutionError(SimpleError("missing api key")).kind,
            CronRunErrorKind.missingCredentials
        )
        XCTAssertEqual(
            repository.classifyExecutionError(SimpleError("network connection lost")).kind,
            CronRunErrorKind.network
        )
        let timeout = repository.classifyExecutionError(SimpleError("request timed out"))
        XCTAssertEqual(timeout.kind, CronRunErrorKind.timedOut)
        XCTAssertEqual(timeout.statusOverride, CronRunStatus.timedOut)
    }

    private func makeRepository(
        scopes: [CronScope] = [CronScope.global(appSandboxRoot: NSTemporaryDirectory())],
        activeScopeID: String? = nil,
        storeProvider: any CronStoreProviding = FakeStoreProvider(stores: [:]),
        historyStore: CronRunHistoryStore? = nil,
        executor: any CronTaskExecuting = UnavailableCronExecutor(),
        notifier: any CronNotificationDelivering = NoopCronNotifier(),
        scheduler: any CronBackgroundScheduling = ForegroundOnlyCronScheduler(),
        backgroundTaskBridge: CronBackgroundTaskBridge = .shared,
        now: @escaping @Sendable () -> UInt64 = { 1_000 }
    ) -> CronRepository {
        let root = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("cron-tests-\(UUID().uuidString)", isDirectory: true)
        return CronRepository(
            appSandboxRoot: root.path,
            scopeProvider: FakeScopeProvider(scopes: scopes, activeScopeID: activeScopeID ?? scopes.first?.scopeID ?? globalCronScopeID),
            storeProvider: storeProvider,
            historyStore: historyStore ?? CronRunHistoryStore(
                fileURL: root.appendingPathComponent("history.json", isDirectory: false),
                now: now,
                newID: {
                    struct Counter {
                        static var value = 0
                    }
                    Counter.value += 1
                    return "run-\(Counter.value)"
                }
            ),
            executor: executor,
            notifier: notifier,
            scheduler: scheduler,
            backgroundTaskBridge: backgroundTaskBridge,
            now: now
        )
    }
}

private func makeRuns(
    prefix: String,
    count: Int,
    taskID: String,
    status: CronRunStatus,
    triggeredStart: UInt64,
    errorKind: CronRunErrorKind?
) -> [CronRunRecord] {
    (0..<count).map { index in
        makeRun(
            runID: "\(prefix)-\(index)",
            taskID: taskID,
            triggeredAtMs: triggeredStart - UInt64(index),
            status: status,
            errorKind: errorKind
        )
    }
}

private func makeRun(
    runID: String,
    taskID: String,
    triggeredAtMs: UInt64,
    status: CronRunStatus,
    errorKind: CronRunErrorKind?
) -> CronRunRecord {
    CronRunRecord(
        runID: runID,
        taskID: taskID,
        scopeID: "global",
        projectID: nil,
        projectName: "全局",
        prompt: "sample",
        scheduledAtMs: triggeredAtMs,
        triggeredAtMs: triggeredAtMs,
        startedAtMs: nil,
        finishedAtMs: triggeredAtMs,
        status: status,
        attempt: 1,
        resultText: status == .succeeded ? "ok" : nil,
        errorMessage: status == .failed ? "nope" : nil,
        errorKind: errorKind,
        manual: false
    )
}

private struct FakeScopeProvider: CronScopeProviding {
    let scopes: [CronScope]
    let activeScopeID: String

    func scopes(appSandboxRoot: String) async -> [CronScope] {
        scopes
    }

    func activeScopeID(for scopes: [CronScope]) async -> String {
        activeScopeID
    }
}

private actor FakeCronStore: CronStoreClient {
    private(set) var tasks: [CronTaskRecord]
    private(set) var acknowledgedOccurrences: [CronOccurrence] = []
    private var ackFailureCount: Int
    private let recoveryBarrier: Bool
    private let cancelAfterAcknowledgement: Bool
    private var listCalls = 0
    private var waitingList: CheckedContinuation<Void, Never>?
    private let reservations: [CronOwnerReservation]

    init(tasks: [CronTaskRecord], ackFailureCount: Int = 0, recoveryBarrier: Bool = false,
         cancelAfterAcknowledgement: Bool = false, reservations: [CronOwnerReservation] = []) {
        self.tasks = tasks
        self.ackFailureCount = ackFailureCount
        self.recoveryBarrier = recoveryBarrier
        self.cancelAfterAcknowledgement = cancelAfterAcknowledgement
        self.reservations = reservations
    }

    func listOwnerReservations() async throws -> [CronOwnerReservation] { reservations }

    func list() async throws -> [CronTaskRecord] {
        listCalls += 1
        if recoveryBarrier && listCalls == 1 {
            await withCheckedContinuation { waitingList = $0 }
        } else if recoveryBarrier && listCalls == 2 {
            waitingList?.resume()
            waitingList = nil
        }
        return tasks.sorted { lhs, rhs in
            (lhs.nextFireMs ?? .max) < (rhs.nextFireMs ?? .max)
        }
    }

    func delete(taskID: String) async throws -> Bool {
        let original = tasks.count
        tasks.removeAll { $0.id == taskID }
        return tasks.count != original
    }

    func nextFireTime() async throws -> UInt64? {
        tasks.compactMap(\.nextFireMs).min()
    }

    func dueOccurrences(nowMs: UInt64) async throws -> [CronOccurrence] {
        tasks.compactMap { task in
            guard let next = task.nextFireMs, next <= nowMs else { return nil }
            return CronOccurrence(taskID: task.id, scheduledAtMs: next)
        }
    }

    func acknowledgeOccurrence(taskID: String, scheduledAtMs: UInt64) async throws -> Bool {
        guard let index = tasks.firstIndex(where: { $0.id == taskID }) else { return false }
        if ackFailureCount > 0 {
            ackFailureCount -= 1
            throw SimpleError("ack write failed")
        }
        acknowledgedOccurrences.append(CronOccurrence(taskID: taskID, scheduledAtMs: scheduledAtMs))
        tasks[index].lastFiredAtMs = scheduledAtMs
        if tasks[index].recurring {
            tasks[index].nextFireMs = scheduledAtMs + 900_000
        } else {
            tasks[index].nextFireMs = nil
        }
        if cancelAfterAcknowledgement { withUnsafeCurrentTask { $0?.cancel() } }
        return true
    }
}

private actor PausingCronStoreProvider: CronStoreProviding {
    private let inner = JsonCronStoreProvider()
    private var shouldPause = false
    private var pausedWaiter: CheckedContinuation<Void, Never>?
    private var release: CheckedContinuation<Void, Never>?

    func pauseNextStore() { shouldPause = true }
    func waitUntilPaused() async {
        if release != nil { return }
        await withCheckedContinuation { pausedWaiter = $0 }
    }
    func releaseStore() { release?.resume(); release = nil }
    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        if shouldPause {
            shouldPause = false
            await withCheckedContinuation { continuation in
                release = continuation
                pausedWaiter?.resume()
                pausedWaiter = nil
            }
        }
        return try await inner.store(for: scope, appSandboxRoot: appSandboxRoot)
    }
}

private actor FakeStoreProvider: CronStoreProviding {
    private let stores: [String: FakeCronStore]

    init(stores: [String: FakeCronStore]) {
        self.stores = stores
    }

    func store(for scope: CronScope, appSandboxRoot: String) async throws -> any CronStoreClient {
        stores[scope.scopeID] ?? FakeCronStore(tasks: [])
    }
}

/// An executor whose scheduled-run results are scripted per call, so a test can
/// fail transiently a few times and then succeed.
private actor SequencedExecutor: CronTaskExecuting {
    private var dueResults: [Result<CronExecutionOutcome?, Error>]
    private let fallback: Result<CronExecutionOutcome?, Error>
    private(set) var dueCallCount = 0

    init(
        dueResults: [Result<CronExecutionOutcome?, Error>],
        fallback: Result<CronExecutionOutcome?, Error>
    ) {
        self.dueResults = dueResults
        self.fallback = fallback
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        throw SimpleError("manual run not scripted")
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        dueCallCount += 1
        let next = dueResults.isEmpty ? fallback : dueResults.removeFirst()
        return try next.get()
    }
}

private actor FakeExecutor: CronTaskExecuting {
    private let manualResult: Result<CronExecutionOutcome, Error>
    private let dueResult: Result<CronExecutionOutcome?, Error>
    private(set) var dueCalls: [(String, UInt64)] = []
    private(set) var manualCalls: [String] = []
    private(set) var manualTimestamps: [UInt64] = []

    init(
        manualResult: Result<CronExecutionOutcome, Error> = .success(
            CronExecutionOutcome(status: .succeeded, resultText: "ok", errorMessage: nil, errorKind: nil)
        ),
        dueResult: Result<CronExecutionOutcome?, Error> = .success(
            CronExecutionOutcome(status: .succeeded, resultText: "ok", errorMessage: nil, errorKind: nil)
        )
    ) {
        self.manualResult = manualResult
        self.dueResult = dueResult
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        manualCalls.append(task.id)
        return try manualResult.get()
    }

    func runTaskNow(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome {
        manualTimestamps.append(scheduledAtMs)
        return try await runTaskNow(scope: scope, task: task)
    }

    func runTaskIfDue(scope: CronScope, task: CronTaskRecord, scheduledAtMs: UInt64) async throws -> CronExecutionOutcome? {
        dueCalls.append((task.id, scheduledAtMs))
        return try dueResult.get()
    }
}

private actor GatedCronExecutor: CronTaskExecuting {
    private var callStarted = false
    private var startWaiters: [CheckedContinuation<Void, Never>] = []
    private var released = false
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []
    private(set) var dueCallCount = 0

    func runTaskNow(scope: CronScope, task: CronTaskRecord) async throws -> CronExecutionOutcome {
        CronExecutionOutcome(status: .succeeded, resultText: "ok", errorMessage: nil, errorKind: nil)
    }

    func runTaskIfDue(
        scope: CronScope,
        task: CronTaskRecord,
        scheduledAtMs: UInt64
    ) async throws -> CronExecutionOutcome? {
        dueCallCount += 1
        callStarted = true
        let currentStartWaiters = startWaiters
        startWaiters.removeAll()
        currentStartWaiters.forEach { $0.resume() }
        if !released {
            await withCheckedContinuation { continuation in
                releaseWaiters.append(continuation)
            }
        }
        return CronExecutionOutcome(
            status: .succeeded,
            resultText: "完成",
            errorMessage: nil,
            errorKind: nil
        )
    }

    func waitUntilDueCallStarts() async {
        guard !callStarted else { return }
        await withCheckedContinuation { continuation in
            startWaiters.append(continuation)
        }
    }

    func releaseDueCall() {
        released = true
        let currentReleaseWaiters = releaseWaiters
        releaseWaiters.removeAll()
        currentReleaseWaiters.forEach { $0.resume() }
    }
}

private actor FakeNotifier: CronNotificationDelivering {
    private(set) var payloads: [CronNotificationPayload] = []
    /// Set to model the real notifier's silent refusals (preference off,
    /// authorization denied), which must give the delivery claim back.
    var refuses = false

    func setRefuses(_ value: Bool) { refuses = value }

    func deliver(_ payload: CronNotificationPayload) async -> Bool {
        payloads.append(payload)
        return !refuses
    }
}

private actor FakeScheduler: CronBackgroundScheduling {
    let mode: CronSchedulingMode
    let note: String
    private(set) var scheduledEarliestTimes: [UInt64?] = []

    init(mode: CronSchedulingMode, note: String) {
        self.mode = mode
        self.note = note
    }

    func schedule(taskIdentifier: String, earliestAtMs: UInt64?) async throws {
        scheduledEarliestTimes.append(earliestAtMs)
    }

    func cancel(taskIdentifier: String) async {}
}

private struct SimpleError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

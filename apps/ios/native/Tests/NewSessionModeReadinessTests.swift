import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class SessionHostOwnershipTests: XCTestCase {
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

    func testSeparateOwnersRetainTurnsAndDraftsAcrossNavigation() throws {
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
        XCTAssertNotNil(first.send("work continues"))
        XCTAssertNotNil(second.send("other work"))
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

    func testAdmittedOwnerKeepsItsTurnIfCatalogLaterBecomesUnavailable() throws {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        source.model.activeSessionId = id
        var catalogAvailable = true
        host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
        host.admitsUnclaimedOwner = { catalogAvailable }
        let owner = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: id, workspace: "/global", draft: "keep")
        XCTAssertNotNil(source.send("continue"))
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
        let config = EngineConfig(apiBase: "", apiKey: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
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

    func testNewTurnReplacesPriorCheckpointForCanonicalOwner() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let firstID = UUID().uuidString
        let otherID = UUID().uuidString
        func config(_ session: String) -> EngineConfig {
            EngineConfig(apiBase: "", apiKey: "", model: "", appSandboxRoot: root.path,
                         projectCwd: "/home/global", sessionMode: .code, visionDelegationEnabled: false,
                         initialSessionId: session)
        }
        let first = DurableConversationTurnClientStore(config: config(firstID.lowercased()))
        let other = DurableConversationTurnClientStore(config: config(otherID))
        first.begin(sessionID: "sess:\(firstID)", turnID: 1)
        other.begin(sessionID: otherID, turnID: 7)
        first.begin(sessionID: firstID.lowercased(), turnID: 2)
        first.clear(turnID: 2)
        XCTAssertNil(first.load(), "Clearing the current turn must not revive an older checkpoint")
        XCTAssertEqual(other.load()?.turnID, 7)

        first.begin(sessionID: firstID, turnID: 3)
        first.clear(turnID: 1)
        XCTAssertEqual(first.load()?.turnID, 3, "A delayed old clear must preserve the new turn")
        XCTAssertEqual(other.load()?.turnID, 7)
    }

    func testExactOwnerAndTurnClearPreservesOtherOwners() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        func config(_ session: String, cwd: String = "/home/global") -> EngineConfig {
            EngineConfig(apiBase: "", apiKey: "", model: "", appSandboxRoot: root.path,
                         projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: false,
                         initialSessionId: session)
        }
        let first = DurableConversationTurnClientStore(config: config("first"))
        let second = DurableConversationTurnClientStore(config: config("second"))
        first.begin(sessionID: "first", turnID: 7)
        second.begin(sessionID: "second", turnID: 7)
        first.begin(sessionID: "first", turnID: 8)
        first.clear(turnID: 7)
        XCTAssertEqual(first.load()?.turnID, 8)
        XCTAssertEqual(second.load()?.turnID, 7)
        XCTAssertEqual(DurableConversationTurnClientStore(config: config("second", cwd: "/home/old-index")).load()?.turnID, 7,
                       "A stale catalog cwd must not hide the UUID owner's checkpoint")
        first.clear(turnID: 8)
        XCTAssertNil(first.load())
        XCTAssertEqual(DurableConversationTurnClientStore(config: config("second")).load()?.turnID, 7)
        second.clear(turnID: 7)
        XCTAssertNil(second.load())
    }
}
#endif

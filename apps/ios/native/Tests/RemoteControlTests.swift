import XCTest
import UIKit
import FirebaseCore
@testable import LingxiCode

final class RemoteControlTests: XCTestCase {
    private let host = "Hk3xQ9vR2mT7wL1pZ8cN4a"
    private let session = "11111111-2222-4333-8444-555555555555"

    @MainActor
    func testIsolatedSimulatorDoesNotInitializeFirebase() throws {
        try XCTSkipUnless(IsolatedSimulatorTesting.enabled, "Requires the isolated simulator fixture")
        XCTAssertNil(FirebaseApp.app())
        XCTAssertNil(FirebaseAccount.shared.user)
    }

    func testRemoteLinksPreserveCaseAndDoNotRebindLocalWorkspace() throws {
        let route = "remote/\(host)/\(session)"
        let link = try XCTUnwrap(CanonicalDeepLink.parse("lingxi://" + route))
        XCTAssertEqual(link.route, route)
        XCTAssertFalse(link.needsContextRebind(workspace: "project.other", mode: "chat"))
        XCTAssertEqual(CanonicalDeepLink.parse("https://lingxios.net/open/" + route), link)
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://" + route + "?workspace=project.other"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://remote/host/../session"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://" + route + "/extra"))
    }

    func testPushOnlyRoutesCurrentUnexpiredAccountToRemoteControl() {
        let route = "remote/\(host)/\(session)"
        let now = Date(timeIntervalSince1970: 1)
        var payload: [AnyHashable: Any] = ["recipientUid": "A", "expiresAt": "2000", "route": route]
        XCTAssertNotNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "B", enabled: true, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: false, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: Date(timeIntervalSince1970: 2)))
        for invalid in ["settings/account", "new", "remote/\(host)/bad", route + "?workspace=global", route + "\n"] {
            payload["route"] = invalid
            XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: now), invalid)
        }
    }

    #if canImport(harness_runtimeFFI)
    @MainActor
    func testNativeModeRejectionStaysFailedAfterLaterModeUpdate() {
        for code in ["SESSION_MODE_OVERFLOW", "SESSION_MODE_MISMATCH"] {
            let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A")
            func update(error: RemoteSessionErrorFfi?, mode: SessionModeDto?, phase: RemoteSessionPhaseFfi) -> RemoteSessionUpdateFfi {
                RemoteSessionUpdateFfi(
                    sessionId: session, sessionMode: mode, phase: phase, error: error, replace: true,
                    messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                    pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                    turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                    hasOlder: false, loadingOlder: false, answeredElsewhere: false)
            }
            remote.receive(update(error: RemoteSessionErrorFfi(code: code, message: "rejected"), mode: nil, phase: .error))
            XCTAssertEqual(remote.phase, .error)
            remote.receive(update(error: nil, mode: .chat, phase: .live))
            XCTAssertEqual(remote.phase, .error)
            XCTAssertFalse(remote.model.sessionModeKnown)
            XCTAssertFalse(remote.isAvailableForPrompts)
        }
    }

    @MainActor
    func testTeamApprovalCapabilityShowsOnlyPermissionAndNeverInventsMode() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        var decisions: [Bool] = []
        let id = "team-approval:" + String(repeating: "a", count: 32)
        let remote = RemoteConversationSource(hostID: host, sessionID: id, uid: "A",
            remotePermissionResponder: { _, allow in
                decisions.append(allow)
                for await _ in release { break }
            })
        let permission = PermissionRequest(requestId: 1,
            kind: .toolUseConfirm(toolName: "tool", toolInputJson: "{}", defaultAllow: false),
            worker: nil, owner: nil, suppressAlwaysAllowRule: true, autoModePrompt: nil)
        let update = RemoteSessionUpdateFfi(
            sessionId: id, sessionMode: nil, phase: .live, error: nil, replace: true,
            messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
            pending: RemotePendingFfi(permissions: [permission], questions: [], computerAccess: []),
            turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
            hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        remote.receive(update)
        XCTAssertTrue(remote.isApprovalCapability)
        XCTAssertEqual(remote.phase, .live)
        XCTAssertFalse(remote.model.sessionModeKnown)
        XCTAssertEqual(remote.model.pendingPermissions.count, 1)
        XCTAssertFalse(remote.isAvailableForPrompts)
        let rejectedToken = await remote.send("not a chat")
        XCTAssertNil(rejectedToken)
        remote.approvePermission(99, .allowOnce)
        remote.approvePermission(1, .allowOnce)
        remote.denyPermission(1)
        for _ in 0..<100 where decisions.isEmpty { await Task.yield() }
        XCTAssertEqual(decisions, [true])
        continuation.yield(())
        continuation.finish()
        for _ in 0..<100 where !remote.model.pendingPermissions.isEmpty { await Task.yield() }
        XCTAssertTrue(remote.model.pendingPermissions.isEmpty)
        remote.receive(update) // A repeated snapshot cannot resurrect a spent capability.
        remote.denyPermission(1)
        await Task.yield()
        XCTAssertEqual(decisions, [true])
        XCTAssertTrue(remote.model.pendingPermissions.isEmpty)
    }

    @MainActor
    func testTeamApprovalDeliveryFailureCanRetryTheStillPendingRequest() async throws {
        var attempts: [Bool] = []
        let id = "team-approval:" + String(repeating: "b", count: 32)
        let remote = RemoteConversationSource(hostID: host, sessionID: id, uid: "A",
            remotePermissionResponder: { _, allow in
                attempts.append(allow)
                if attempts.count == 1 { throw RemoteFfiError.NotConnected }
            })
        let permission = PermissionRequest(requestId: 7,
            kind: .toolUseConfirm(toolName: "tool", toolInputJson: "{}", defaultAllow: false),
            worker: nil, owner: nil, suppressAlwaysAllowRule: true, autoModePrompt: nil)
        remote.receive(RemoteSessionUpdateFfi(
            sessionId: id, sessionMode: nil, phase: .live, error: nil, replace: true,
            messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
            pending: RemotePendingFfi(permissions: [permission], questions: [], computerAccess: []),
            turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
            hasOlder: false, loadingOlder: false, answeredElsewhere: false))
        remote.approvePermission(7, .allowOnce)
        for _ in 0..<100 where remote.model.error == nil { await Task.yield() }
        XCTAssertEqual(attempts, [true])
        XCTAssertEqual(remote.model.pendingPermissions.map(\.requestId), [7])
        remote.denyPermission(7)
        for _ in 0..<100 where !remote.model.pendingPermissions.isEmpty { await Task.yield() }
        XCTAssertEqual(attempts, [true, false])
        XCTAssertTrue(remote.model.pendingPermissions.isEmpty)
    }

    @MainActor
    func testTeamApprovalNotPendingConsumesTheCapability() async {
        var calls = 0
        let id = "team-approval:" + String(repeating: "d", count: 32)
        let remote = RemoteConversationSource(hostID: host, sessionID: id, uid: "A",
            remotePermissionResponder: { _, _ in
                calls += 1
                throw RemoteFfiError.Rpc(code: "not_pending", detail: "already answered")
            })
        let permission = PermissionRequest(requestId: 8,
            kind: .toolUseConfirm(toolName: "tool", toolInputJson: "{}", defaultAllow: false),
            worker: nil, owner: nil, suppressAlwaysAllowRule: true, autoModePrompt: nil)
        let update = RemoteSessionUpdateFfi(
            sessionId: id, sessionMode: nil, phase: .live, error: nil, replace: true,
            messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
            pending: RemotePendingFfi(permissions: [permission], questions: [], computerAccess: []),
            turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
            hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        remote.receive(update)
        remote.approvePermission(8, .allowOnce)
        for _ in 0..<100 where !remote.answeredElsewhere { await Task.yield() }
        XCTAssertTrue(remote.answeredElsewhere)
        XCTAssertTrue(remote.model.pendingPermissions.isEmpty)
        remote.receive(update)
        remote.denyPermission(8)
        await Task.yield()
        XCTAssertEqual(calls, 1)
        XCTAssertTrue(remote.model.pendingPermissions.isEmpty)
    }

    @MainActor
    func testTeamApprovalUncertainDeliveryCannotBeAnsweredAgain() async {
        for code in ["timeout", "closed", "internal", "unknown_wire_code"] {
            var calls = 0
            let id = "team-approval:" + String(repeating: "e", count: 32)
            let remote = RemoteConversationSource(hostID: host, sessionID: id, uid: "A",
                remotePermissionResponder: { _, _ in
                    calls += 1
                    throw RemoteFfiError.Rpc(code: code, detail: "delivery unknown")
                })
            let permission = PermissionRequest(requestId: 8,
                kind: .toolUseConfirm(toolName: "tool", toolInputJson: "{}", defaultAllow: false),
                worker: nil, owner: nil, suppressAlwaysAllowRule: true, autoModePrompt: nil)
            let update = RemoteSessionUpdateFfi(
                sessionId: id, sessionMode: nil, phase: .live, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [permission], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
            remote.receive(update)
            remote.approvePermission(8, .allowOnce)
            for _ in 0..<100 where remote.model.error == nil { await Task.yield() }
            XCTAssertNotNil(remote.model.error)
            remote.receive(update)
            remote.denyPermission(8)
            await Task.yield()
            XCTAssertEqual(calls, 1, code)
            XCTAssertTrue(remote.model.pendingPermissions.isEmpty, code)
        }
    }

    @MainActor
    func testTeamApprovalRejectsConversationTurnAndHistoryMetadata() {
        let id = "team-approval:" + String(repeating: "c", count: 32)
        let permission = PermissionRequest(requestId: 1,
            kind: .toolUseConfirm(toolName: "tool", toolInputJson: "{}", defaultAllow: false),
            worker: nil, owner: nil, suppressAlwaysAllowRule: true, autoModePrompt: nil)
        func update(mode: SessionModeDto? = nil, running: Bool = false,
                    turnId: UInt64? = nil, first: UInt64 = 0, hasOlder: Bool = false,
                    loadingOlder: Bool = false) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: id, sessionMode: mode, phase: .live, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: first, records: [],
                pending: RemotePendingFfi(permissions: [permission], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: running, turnId: turnId), controls: nil, outgoing: [],
                hasOlder: hasOlder, loadingOlder: loadingOlder, answeredElsewhere: false)
        }
        for malformed in [update(mode: .code), update(running: true), update(turnId: 2),
                          update(first: 1), update(hasOlder: true), update(loadingOlder: true)] {
            let remote = RemoteConversationSource(hostID: host, sessionID: id, uid: "A")
            remote.receive(update())
            XCTAssertEqual(remote.model.pendingPermissions.count, 1)
            remote.receive(malformed)
            XCTAssertEqual(remote.phase, .error)
            XCTAssertTrue(remote.model.pendingPermissions.isEmpty)
            remote.receive(update())
            XCTAssertEqual(remote.phase, .error)
            XCTAssertFalse(remote.model.sessionModeKnown)
        }
    }

    @MainActor
    func testProviderAccountsRequireConfirmedLiveConversationAndRejectTeamCapability() async throws {
        var calls = 0
        let sender: (String, String?) async throws -> String = { _, _ in
            calls += 1
            return "{}"
        }
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A",
                                              remoteProviderAccountSender: sender)
        func update(_ id: String, mode: SessionModeDto?) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: id, sessionMode: mode, phase: .live, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update(session, mode: nil))
        do { _ = try await remote.providerAccountsSnapshot(); XCTFail("Unknown mode must not access accounts") }
        catch {}
        XCTAssertEqual(calls, 0)

        remote.receive(update(session, mode: .chat))
        let snapshot = try await remote.providerAccountsSnapshot()
        XCTAssertEqual(snapshot, "{}")
        XCTAssertEqual(calls, 1)

        remote.receive(update(session, mode: .code))
        XCTAssertEqual(remote.phase, .error)
        do { _ = try await remote.providerAccountCommand(requestJSON: "{}", credential: nil); XCTFail("Conflicting mode must not access accounts") }
        catch {}
        XCTAssertEqual(calls, 1)

        let teamID = "team-approval:" + String(repeating: "a", count: 32)
        let team = RemoteConversationSource(hostID: host, sessionID: teamID, uid: "A",
                                            remoteProviderAccountSender: sender)
        team.receive(update(teamID, mode: nil))
        do { _ = try await team.providerAccountsSnapshot(); XCTFail("Team approval has no account capability") }
        catch {}
        XCTAssertEqual(calls, 1)
    }

    @MainActor
    func testProviderAccountReplyIsDiscardedIfModeConflictsWhileRequestRuns() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        var started = false
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A",
            remoteProviderAccountSender: { _, _ in
                started = true
                for await _ in release { break }
                return "{}"
            })
        func update(_ mode: SessionModeDto) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: mode, phase: .live, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update(.chat))
        let request = Task { try await remote.providerAccountsSnapshot() }
        for _ in 0..<100 where !started { await Task.yield() }
        XCTAssertTrue(started)
        remote.receive(update(.code))
        continuation.yield(())
        continuation.finish()
        do { _ = try await request.value; XCTFail("A stale reply must be discarded") }
        catch is CancellationError {}
        catch { XCTFail("Unexpected error: \(error)") }
    }

    @MainActor
    func testProviderAccountReplyIsDiscardedAfterOfflineAndReconnect() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        var started = false
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A",
            remoteProviderAccountSender: { _, _ in
                started = true
                for await _ in release { break }
                return "{}"
            })
        func update(_ phase: RemoteSessionPhaseFfi) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: .chat, phase: phase, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update(.live))
        let request = Task { try await remote.providerAccountsSnapshot() }
        for _ in 0..<100 where !started { await Task.yield() }
        XCTAssertTrue(started)
        remote.receive(update(.offline))
        remote.receive(update(.live))
        continuation.yield(())
        continuation.finish()
        do { _ = try await request.value; XCTFail("A reply from before reconnect must be discarded") }
        catch is CancellationError {}
        catch { XCTFail("Unexpected error: \(error)") }
    }

    @MainActor
    func testProviderAccountReplyIsFencedAtTheBeginningOfStop() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        var started = false
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A",
            remoteProviderAccountSender: { _, _ in
                started = true
                for await _ in release { break }
                return "{}"
            })
        let live = RemoteSessionUpdateFfi(
            sessionId: session, sessionMode: .chat, phase: .live, error: nil, replace: true,
            messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
            pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
            turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
            hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        remote.receive(live)
        let request = Task { try await remote.providerAccountsSnapshot() }
        for _ in 0..<100 where !started { await Task.yield() }
        XCTAssertTrue(started)
        await remote.stop()
        remote.receive(live) // Even a late LIVE callback cannot validate the old reply.
        continuation.yield(())
        continuation.finish()
        do { _ = try await request.value; XCTFail("A reply admitted before stop must be discarded") }
        catch is CancellationError {}
        catch { XCTFail("Unexpected error: \(error)") }
    }

    @MainActor
    func testAttachmentAdmissionRacePreservesUnsentOriginalImage() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A",
            remotePromptSender: { _, _ in
                for await _ in release { break }
                throw RemoteFfiError.Rpc(code: "forbidden", detail: "turn already running")
            })
        func update(running: Bool) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: .chat, phase: .live, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: running, turnId: running ? 9 : nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update(running: false))
        let png = UIGraphicsImageRenderer(size: CGSize(width: 2, height: 2)).image { _ in UIColor.red.setFill(); UIRectFill(CGRect(x: 0, y: 0, width: 2, height: 2)) }
        let original = try XCTUnwrap(png.pngData()).base64EncodedString()
        let acceptedToken = await remote.send("attachment", images: [ImageRefDto(mediaType: "image/png", base64: original)])
        XCTAssertNotNil(acceptedToken)
        remote.receive(update(running: true))
        continuation.yield(())
        for _ in 0..<100 {
            if !remote.unsentPrompts.isEmpty { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        let unsent = try XCTUnwrap(remote.unsentPrompts.first)
        XCTAssertEqual(unsent.text, "attachment")
        XCTAssertEqual(unsent.images.first?.base64, original)
        XCTAssertNotNil(remote.model.error)
    }

    @MainActor
    func testRejectedRemoteSendRestoresOriginalDraftWithoutOverwritingNewEdits() async throws {
        func update(_ phase: RemoteSessionPhaseFfi, mode: SessionModeDto?) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: mode, phase: phase, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        for conflict in [false, true] {
            let (release, continuation) = AsyncStream<Void>.makeStream()
            let manager = RemoteControlManager()
            let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A", manager: manager,
                remotePromptSender: { _, _ in
                    for await _ in release { break }
                    throw RemoteFfiError.NotConnected
                })
            remote.receive(update(.live, mode: .chat))
            let units: [UInt16] = [0xD800, 0x0061]
            let acceptedToken = await remote.send("original", sourceCodeUnits: units, images: [], selectedSlashCommand: nil)
            XCTAssertNotNil(acceptedToken)
            remote.receive(conflict ? update(.live, mode: .code) : update(.loading, mode: nil))
            XCTAssertEqual(remote.phase, conflict ? .error : .loading)
            await remote.stop()
            let reopened = RemoteConversationSource(hostID: host, sessionID: "sess:" + session, uid: "A", manager: manager)
            XCTAssertTrue(reopened.draftOwner === remote.draftOwner)
            XCTAssertFalse(RemoteConversationSource(hostID: "other", sessionID: session, uid: "A", manager: manager).draftOwner === remote.draftOwner)
            XCTAssertFalse(RemoteConversationSource(hostID: host, sessionID: session, uid: "B", manager: manager).draftOwner === remote.draftOwner)
            continuation.yield(())
            for _ in 0..<100 {
                if !reopened.unsentPrompts.isEmpty { break }
                try await Task.sleep(for: .milliseconds(10))
            }
            let unsent = try XCTUnwrap(reopened.unsentPrompts.first)
            XCTAssertEqual(unsent.text, "original")
            XCTAssertEqual(unsent.sourceCodeUnits, units)
            var draft = RemoteConversationDraft()
            draft.text = "new edits"
            XCTAssertFalse(draft.restore(unsent))
            XCTAssertEqual(draft.text, "new edits")
            draft.text = ""
            XCTAssertTrue(draft.restore(unsent))
            XCTAssertEqual(draft.text, "original")
            XCTAssertEqual(draft.sourceCodeUnits, units)
            await manager.configure(uid: nil)
            let afterLogout = RemoteConversationSource(hostID: host, sessionID: session, uid: "A", manager: manager)
            XCTAssertFalse(afterLogout.draftOwner === remote.draftOwner)
        }
    }

    @MainActor
    func testAdmittedSendKeepsItsOwnerAliveUntilRejectionIsRecorded() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        let manager = RemoteControlManager()
        var remote: RemoteConversationSource? = RemoteConversationSource(hostID: host, sessionID: session, uid: "A", manager: manager,
            remotePromptSender: { _, _ in
                for await _ in release { break }
                throw RemoteFfiError.NotConnected
            })
        remote?.receive(RemoteSessionUpdateFfi(
            sessionId: session, sessionMode: .chat, phase: .live, error: nil, replace: true,
            messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
            pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
            turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
            hasOlder: false, loadingOlder: false, answeredElsewhere: false))
        let owner = try XCTUnwrap(remote?.draftOwner)
        weak var admittedOwner = remote
        let acceptedToken = await remote?.send("retain this")
        XCTAssertNotNil(acceptedToken)
        remote = nil
        XCTAssertNotNil(admittedOwner, "the accepted task must own the source before its first suspension")
        continuation.yield(())
        for _ in 0..<100 {
            if !owner.unsentPrompts.isEmpty { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(owner.unsentPrompts.first?.text, "retain this")
    }

    @MainActor
    func testRejectedRemoteSendStaysQueuedAfterEditAndClear() async throws {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        let manager = RemoteControlManager()
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A", manager: manager,
            remotePromptSender: { _, _ in
                for await _ in release { break }
                throw RemoteFfiError.NotConnected
            })
        remote.receive(RemoteSessionUpdateFfi(
            sessionId: session, sessionMode: .chat, phase: .live, error: nil, replace: true,
            messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
            pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
            turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
            hasOlder: false, loadingOlder: false, answeredElsewhere: false))
        let token = await remote.send("old draft")
        XCTAssertNotNil(token)
        let owner = remote.draftOwner
        owner.draft.text = "new draft"
        owner.draft.text = ""
        continuation.yield(())
        for _ in 0..<100 {
            if !owner.unsentPrompts.isEmpty { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        let pending = try XCTUnwrap(owner.unsentPrompts.first)
        XCTAssertTrue(owner.draft.text.isEmpty, "failure must not undo a later edit and clear")
        XCTAssertTrue(owner.draft.restore(pending), "the queued draft remains available for explicit restore")
        XCTAssertEqual(owner.draft.text, "old draft")
    }

    @MainActor
    func testRemoteDraftRecoveryPreservesAttachment() throws {
        let image = UIGraphicsImageRenderer(size: CGSize(width: 2, height: 2)).image { context in
            UIColor.red.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 2, height: 2))
        }
        let base64 = try XCTUnwrap(image.pngData()).base64EncodedString()
        let unsent = RemoteUnsentPrompt(id: UUID(), text: "photo", sourceCodeUnits: nil,
            images: [ImageRefDto(mediaType: "image/png", base64: base64)])
        var draft = RemoteConversationDraft()
        XCTAssertTrue(draft.restore(unsent))
        XCTAssertEqual(draft.text, "photo")
        XCTAssertEqual(draft.attachment?.base64, base64)
    }

    @MainActor
    func testRemoteProjectionAcceptsDesktopTurnsAndRetainsStablePromptIdentity() {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        source.replaceRemoteTranscript([], sessionID: session)
        source.appendRemotePrompt(turnID: 10, text: "from desktop", images: [], origin: "Desktop")
        let id = source.model.messages.first?.id
        source.applyRemoteEvent(.turnStarted(turnId: 10))
        source.applyRemoteEvent(.textDelta(text: "first answer".clientTextDto))
        XCTAssertTrue(source.model.messages.contains { $0.text.contains("first answer") })
        source.applyRemoteEvent(.slashCommandResult(turnId: 99, display: "model changed".clientTextDto, isError: false))
        XCTAssertTrue(source.model.streaming, "out-of-band command output must not end the active desktop turn")
        XCTAssertTrue(source.model.items.contains { if case let .commandOutput(output) = $0 { return output.text == "model changed" }; return false })
        source.appendRemotePrompt(turnID: 10, text: "from desktop", images: [], origin: "Desktop")
        XCTAssertEqual(source.model.messages.filter { $0.role == .user }.count, 1)
        source.replaceRemoteTranscript([], sessionID: session)
        source.appendRemotePrompt(turnID: 10, text: "from desktop", images: [], origin: "Desktop")
        XCTAssertEqual(source.model.messages.first?.id, id)
        source.applyRemoteEvent(.turnStarted(turnId: 11))
        source.applyRemoteEvent(.textDelta(text: "second answer".clientTextDto))
        XCTAssertTrue(source.model.messages.contains { $0.text.contains("second answer") })
    }

    @MainActor
    func testRemoteWaitsForModeAndReplaysSnapshotAndEventsInOrder() async {
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A")
        func update(mode: SessionModeDto? = nil, replace: Bool = false, records: [RemoteSeqRecordFfi] = [],
                    running: Bool = false) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: mode, phase: .live, error: nil, replace: replace,
                messages: replace ? [MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "history".clientTextDto)])] : [],
                olderMessages: [], firstMessageIndex: 0, records: records,
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: running, turnId: running ? 10 : nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update(replace: true))
        remote.receive(update(records: [
            RemoteSeqRecordFfi(seq: 1, record: .prompt(turnId: 10, text: "prompt", images: [], origin: .desktop, deviceName: nil)),
            RemoteSeqRecordFfi(seq: 2, record: .event(event: .turnStarted(turnId: 10), truncated: false)),
            RemoteSeqRecordFfi(seq: 3, record: .event(event: .textDelta(text: "answer".clientTextDto), truncated: false))
        ], running: true))
        XCTAssertEqual(remote.phase, .loading)
        XCTAssertFalse(remote.model.sessionModeKnown)
        XCTAssertTrue(remote.model.messages.isEmpty)
        let earlyToken = await remote.send("not yet")
        XCTAssertNil(earlyToken)

        remote.receive(update(mode: .chat, running: true))
        XCTAssertEqual(remote.phase, .live)
        XCTAssertTrue(remote.model.sessionModeKnown)
        XCTAssertEqual(remote.sessionMode, .chat)
        XCTAssertEqual(remote.model.messages.map(\.text), ["history", "prompt", "answer"])

        remote.receive(update(mode: .code, running: true))
        XCTAssertEqual(remote.phase, .error)
        XCTAssertEqual(remote.sessionMode, .chat)
        let conflictingToken = await remote.send("conflicting mode")
        XCTAssertNil(conflictingToken)
        remote.receive(update(running: true))
        XCTAssertEqual(remote.phase, .error)
    }

    @MainActor
    func testRemoteModeLifecycleAfterPromptDoesNotResetReplayedHistory() {
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A")
        func update(replace: Bool = false, records: [RemoteSeqRecordFfi] = []) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: nil, phase: .live, error: nil, replace: replace,
                messages: replace ? [MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "history".clientTextDto)])] : [],
                olderMessages: [], firstMessageIndex: 0, records: records,
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update(replace: true))
        remote.receive(update(records: [
            RemoteSeqRecordFfi(seq: 1, record: .prompt(turnId: 9, text: "prompt", images: [], origin: .desktop, deviceName: nil)),
            RemoteSeqRecordFfi(seq: 2, record: .event(event: .sessionStarted(sessionId: "foreign", mode: .code), truncated: false)),
        ]))
        remote.receive(update(records: [
            RemoteSeqRecordFfi(seq: 3, record: .event(event: .sessionResumed(sessionId: session, mode: .chat, messages: []), truncated: false)),
        ]))
        XCTAssertEqual(remote.phase, .live)
        XCTAssertEqual(remote.sessionMode, .chat)
        XCTAssertEqual(remote.model.messages.map(\.text), ["history", "prompt"])
    }

    @MainActor
    func testRemoteLifecycleModeMustBelongToCurrentSession() {
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A")
        func update(_ id: String) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: nil, phase: .live, error: nil, replace: false,
                messages: [], olderMessages: [], firstMessageIndex: 0,
                records: [RemoteSeqRecordFfi(seq: 1, record: .event(event: .sessionStarted(sessionId: id, mode: .chat), truncated: false))],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        remote.receive(update("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"))
        XCTAssertEqual(remote.phase, .loading)
        XCTAssertFalse(remote.model.sessionModeKnown)
        remote.receive(update(session))
        XCTAssertEqual(remote.phase, .live)
        XCTAssertEqual(remote.sessionMode, .chat)
    }

    @MainActor
    func testRemoteSnapshotAndLiveModeDoNotReplaceTheTranscript() {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        source.replaceRemoteTranscript([MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "hello".clientTextDto)])], sessionID: session)
        func update(_ mode: SessionModeDto?) -> RemoteSessionUpdateFfi {
            RemoteSessionUpdateFfi(
                sessionId: session, sessionMode: mode, phase: .live, error: nil, replace: true,
                messages: [], olderMessages: [], firstMessageIndex: 0, records: [],
                pending: RemotePendingFfi(permissions: [], questions: [], computerAccess: []),
                turn: RemoteTurnFfi(running: false, turnId: nil), controls: nil, outgoing: [],
                hasOlder: false, loadingOlder: false, answeredElsewhere: false)
        }
        source.reconcileRemoteState(update(nil))
        XCTAssertFalse(source.model.sessionModeKnown, "A partial update must not invent a mode")
        source.reconcileRemoteState(update(.chat))
        XCTAssertTrue(source.model.sessionModeKnown)
        XCTAssertEqual(source.model.sessionMode, .chat)
        source.reconcileRemoteState(update(nil))
        XCTAssertTrue(source.model.sessionModeKnown)
        XCTAssertEqual(source.model.sessionMode, .chat)
        let rows = source.model.messages.map(\.text)
        source.applyRemoteEvent(.sessionStarted(sessionId: session, mode: .code))
        XCTAssertEqual(source.model.sessionMode, .chat)
        XCTAssertEqual(source.model.messages.map(\.text), rows)
        XCTAssertEqual(source.model.activeSessionId, session)
    }

    @MainActor
    func testRemoteHistoryImageHydrationPreservesRowsAndAccountInvalidationClearsData() async {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        let ref = session + ":m:0:0"
        source.replaceRemoteTranscript([MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "photo".clientTextDto)],
            images: [MessageImageDto(mediaType: "image/png", url: ref)])], sessionID: session)
        let ids = source.model.messages.map(\.id)
        source.hydrateRemoteImage(reference: ref, mediaType: "image/png", bytes: Data([1, 2, 3]))
        XCTAssertEqual(source.model.messages.map(\.id), ids)
        XCTAssertEqual(source.model.messages.flatMap(\.images).first?.url, "data:image/png;base64,AQID")
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A")
        remote.model.messages = source.model.messages
        remote.invalidate()
        XCTAssertTrue(remote.model.messages.isEmpty)
        XCTAssertEqual(remote.phase, .closed)
        let rejectedToken = await remote.send("must not send")
        XCTAssertNil(rejectedToken)
    }

    @MainActor
    func testManagerFilesBotReportsUnderTheirComputerAndDropsThoseOfAReplacedClient() {
        let manager = RemoteControlManager()
        let bot = RemoteBotSummaryFfi(botId: "bot-1", name: "Scribe", description: "", status: .running,
            waitingThreads: 1, lastActivityAtMs: nil)
        let chats = RemoteBotThreadsStateFfi(threads: [], error: nil)
        // A new manager is on its first client, whose reports carry generation 0.
        manager.receive(bots: .bots(bots: [bot]), hostID: host, generation: 0)
        manager.receive(botThreads: chats, hostID: host, botID: "bot-1", generation: 0)
        XCTAssertEqual(manager.botDirectory.bots(host: host), .bots(bots: [bot]))
        XCTAssertEqual(manager.botDirectory.chats(host: host, bot: "bot-1"), chats)
        XCTAssertEqual(manager.botDirectory.bots(host: "another-computer"), .unknown)

        manager.receive(bots: .unsupported, hostID: host, generation: 1)
        manager.receive(botThreads: RemoteBotThreadsStateFfi(threads: nil, error: nil), hostID: host, botID: "bot-1", generation: 1)
        XCTAssertEqual(manager.botDirectory.bots(host: host), .bots(bots: [bot]))
        XCTAssertEqual(manager.botDirectory.chats(host: host, bot: "bot-1"), chats)
    }

    @MainActor
    func testManagerWithoutAClientShowsNoBotsAndRefusesToStartAChat() async {
        let manager = RemoteControlManager()
        manager.hideBot(RemoteBotKey(hostID: host, botID: "bot-1"))
        XCTAssertNil(manager.botOfThread(session))
        XCTAssertEqual(manager.botDirectory.bots(host: host), .unknown)
        do {
            _ = try await manager.createBotChat(RemoteBotKey(hostID: host, botID: "bot-1"))
            XCTFail("a chat was started with no connection")
        } catch { XCTAssertEqual(error as? RemoteFfiError, .NotConnected) }
    }

    @MainActor
    func testRemoteProjectionCannotBootstrapLocalEngine() async {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        do { try await source.prepare(); XCTFail("remote projection built a local engine") }
        catch { XCTAssertTrue(error is RemoteFfiError) }
    }
    #endif
}

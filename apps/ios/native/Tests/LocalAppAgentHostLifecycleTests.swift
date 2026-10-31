import Foundation
import Security
import XCTest
@testable import LingxiCode

final class LocalAppAgentHostLifecycleTests: XCTestCase {
    func testStaleSameRunPreparationPreservesNewOwnerUnlessAppAuthorityChanged() {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let other = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-b")
        let old = UUID(), newer = UUID()
        func shouldCancel(_ current: LocalAppAgentWakeRequest?, _ token: UUID?, _ revision: UInt64?) -> Bool {
            LocalAppAgentHandoffCancellationPolicy.shouldCancelPreparation(
                wake: wake, token: old, currentWake: current, currentToken: token, currentRevision: revision
            )
        }
        XCTAssertFalse(shouldCancel(wake, newer, 1), "An old preparation must leave the requeued same-run context alone.")
        XCTAssertTrue(shouldCancel(wake, old, 1), "A genuine failure of the current owner must clean up.")
        XCTAssertTrue(shouldCancel(other, newer, 1), "A different run still replaces the old request.")
        XCTAssertTrue(shouldCancel(nil, nil, 1), "Explicit cancellation still cleans up.")
        XCTAssertTrue(shouldCancel(wake, newer, 2), "A release change takes priority over requeue.")
        XCTAssertTrue(shouldCancel(wake, newer, nil), "Removing the App takes priority over requeue.")
    }

    func testOldBackgroundIdentifierCannotActOnNewContext() {
        XCTAssertTrue(LocalAppAgentContextAdmission.isCurrent(
            expectedIdentifier: "request-a", actualIdentifier: "request-a", finished: false
        ))
        XCTAssertFalse(LocalAppAgentContextAdmission.isCurrent(
            expectedIdentifier: "request-a", actualIdentifier: "request-b", finished: false
        ))
        XCTAssertFalse(LocalAppAgentContextAdmission.isCurrent(
            expectedIdentifier: "request-a", actualIdentifier: "request-a", finished: true
        ))
        XCTAssertFalse(LocalAppAgentContextAdmission.isCurrent(
            expectedIdentifier: "request-a", actualIdentifier: nil, finished: false
        ))
    }

    func testReservationCannotBeReplacedUntilFinishedContextIsRemoved() {
        let now = Date(timeIntervalSince1970: 1_000)
        let future = now.addingTimeInterval(120)
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: false, finished: false, taskAttached: false, reservationExpiry: nil, now: now
        ), .create)
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: false, finished: false, taskAttached: false, reservationExpiry: nil,
            reservationRequestInFlight: true, now: now
        ), .wait, "A reserve response and its cleanup must finish before a retry can reserve.")
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: true, finished: true, taskAttached: true, reservationExpiry: future, now: now
        ), .wait, "A finished context remains owned until its asynchronous cleanup removes it.")
        XCTAssertTrue(LocalAppAgentContextAdmission.cleanupAlreadyRunning(contextMatches: true, finished: true),
                      "A second cancellation must not start another late reservation clear.")
        XCTAssertFalse(LocalAppAgentContextAdmission.cleanupAlreadyRunning(contextMatches: false, finished: true))
        XCTAssertFalse(LocalAppAgentContextAdmission.cleanupAlreadyRunning(contextMatches: true, finished: false))
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: true, finished: false, taskAttached: false, reservationExpiry: future, now: now
        ), .wait)
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: true, finished: false, taskAttached: true, reservationExpiry: now, now: now
        ), .wait)
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: true, finished: false, taskAttached: true, reservationExpiry: future, now: now
        ), .reuse)
        XCTAssertEqual(LocalAppAgentContextAdmission.decide(
            contextExists: false, finished: false, taskAttached: false, reservationExpiry: nil, now: now
        ), .create, "A retry becomes available after cleanup removes the old context.")
    }

    @MainActor
    func testCleanupRequiresTheExactNewReservationEvenAfterFailureOrExpiration() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let reservationID = UUID().uuidString
        func receipt(created: Bool, appID: String = "app-a", identifier: String? = nil) -> LocalAppWakeReservation {
            LocalAppWakeReservation(appId: appID, revision: 1, runId: "run-a", expiresAtMs: 100,
                                    created: created, reservationId: identifier ?? reservationID)
        }
        let created = receipt(created: true)
        XCTAssertEqual(LocalAppAgentWakeReservationDecision.decide(created, for: wake, nowMillis: 50, contextReady: true),
                       .accept(reservationID))
        XCTAssertEqual(LocalAppAgentWakeReservationDecision.decide(created, for: wake, nowMillis: 50, contextReady: false),
                       .clear(reservationID), "A late response after iOS removed the context clears its own generation.")
        XCTAssertEqual(LocalAppAgentWakeReservationDecision.decide(created, for: wake, nowMillis: 100, contextReady: true),
                       .clear(reservationID), "An expired receipt still identifies its generation to clear.")
        XCTAssertEqual(LocalAppAgentWakeReservationDecision.decide(receipt(created: false), for: wake, nowMillis: 50, contextReady: false),
                       .existing, "A reused reservation never transfers cleanup ownership.")
        XCTAssertEqual(LocalAppAgentWakeReservationDecision.decide(receipt(created: true, appID: "other"), for: wake, nowMillis: 50, contextReady: true), .invalid)
        XCTAssertEqual(LocalAppAgentWakeReservationDecision.decide(receipt(created: true, identifier: "bad"), for: wake, nowMillis: 50, contextReady: true), .invalid)

        var cleared: [String] = []
        try await LocalAppAgentWakeCleanup.perform(ownedReservationID: nil) { cleared.append($0) }
        XCTAssertTrue(cleared.isEmpty, "An unconfirmed or reused reservation must never cancel an existing run.")
        // Both browser-open failure and system expiration use this owned cleanup.
        try await LocalAppAgentWakeCleanup.perform(ownedReservationID: reservationID) { cleared.append($0) }
        try await LocalAppAgentWakeCleanup.perform(ownedReservationID: reservationID) { cleared.append($0) }
        XCTAssertEqual(cleared, [reservationID, reservationID])
    }

    @MainActor
    func testOnlySystemExpirationSendsItsCauseForAnOwnedReservation() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, runID: "run-a")
        let reservationID = UUID().uuidString
        var sent: [[String: LocalAppJSON]] = []
        try await LocalAppAgentWakeCleanup.perform(ownedReservationID: nil) { id in
            sent.append(LocalAppAgentWakeCleanup.request(wake: wake, reservationID: id, reason: .systemExpiration))
        }
        XCTAssertTrue(sent.isEmpty, "An unowned reservation must not be cleared even on expiration.")
        try await LocalAppAgentWakeCleanup.perform(ownedReservationID: reservationID) { id in
            sent.append(LocalAppAgentWakeCleanup.request(wake: wake, reservationID: id, reason: .systemExpiration))
        }
        let expired = try XCTUnwrap(sent.first)
        XCTAssertEqual(expired["op"], .string("wake.clear"))
        XCTAssertEqual(expired["appId"], .string("app-a"))
        XCTAssertEqual(expired["revision"], .number(7))
        XCTAssertEqual(expired["runId"], .string("run-a"))
        XCTAssertEqual(expired["reservationId"], .string(reservationID))
        XCTAssertEqual(expired["reason"], .string("system_expiration"))

        for reason: LocalAppAgentCancellationReason in [
            .explicit, .requestReplaced, .releaseChanged, .monitorDeadline,
            .acquireFailure, .intentCancellation, .intentDeadline
        ] {
            let request = LocalAppAgentWakeCleanup.request(wake: wake, reservationID: reservationID, reason: reason)
            XCTAssertNil(request["reason"], "Only the system expiration callback may assert its cause.")
            XCTAssertEqual(request["reservationId"], .string(reservationID))
        }
        XCTAssertNil(LocalAppAgentWakeCleanup.request(wake: wake, reservationID: reservationID)["reason"])
    }

    func testTaskStatusDistinguishesExpirationCancelAndUnknownRestart() {
        let expired = LocalAppAgentTaskStatusText.status(state: "interrupted", errorCode: "ios_background_expired")
        let restarted = LocalAppAgentTaskStatusText.status(state: "interrupted", errorCode: "host_restarted")
        let cancelled = LocalAppAgentTaskStatusText.status(state: "cancelled", errorCode: "cancelled")
        XCTAssertTrue(expired.contains("后台执行"))
        XCTAssertTrue(expired.contains("已中断"))
        XCTAssertTrue(expired.contains("检查已完成操作"))
        XCTAssertTrue(restarted.contains("原因未知"))
        XCTAssertFalse(restarted.contains("后台执行"))
        XCTAssertEqual(cancelled, "已取消")
        XCTAssertEqual(LocalAppAgentTaskStatusText.status(state: "running", errorCode: "ios_background_expired"), "running")
    }

    @MainActor
    func testConfigurationCleanupUsesTheHostResultAndCapturedGeneration() async {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let old = UUID(), newer = UUID()
        let snapshot = LocalAppAgentConfigurationScopeSnapshot(wake: wake, token: old, contexts: [])
        XCTAssertTrue(snapshot.matches(currentWake: wake, currentToken: old))
        XCTAssertFalse(snapshot.matches(currentWake: wake, currentToken: newer),
                       "A late settings response cannot detach a new same-run handoff.")
        XCTAssertFalse(snapshot.matches(currentWake: nil, currentToken: nil))
        XCTAssertFalse(LocalAppAgentContextAdmission.isCurrent(
            expectedIdentifier: "old", actualIdentifier: "new", finished: false
        ), "A captured old context cannot cancel its replacement.")

        var cancellations = 0
        await LocalAppAgentConfigurationCleanup.perform(changed: false) { cancellations += 1 }
        XCTAssertEqual(cancellations, 0, "Saving unchanged settings must not cancel a running task.")
        await LocalAppAgentConfigurationCleanup.perform(changed: true) { cancellations += 1 }
        XCTAssertEqual(cancellations, 1, "A changed or revoked grant cleans the captured scope.")
    }

    @MainActor
    func testDuplicatePreparationCannotEnterOrClearTheFirstReservation() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let token = UUID()
        let preparations = LocalAppAgentHandoffPreparations()
        let gate = HandoffPreparationTestGate()
        var reserved = false
        var duplicateEntered = false

        let first = Task { @MainActor in
            try await preparations.prepare(wake, token: token) {
                await gate.suspend()
                reserved = true
                return "ready"
            }
        }
        await gate.waitUntilSuspended()
        do {
            _ = try await preparations.prepare(wake, token: UUID()) {
                duplicateEntered = true
                reserved = false
                return "duplicate"
            }
            XCTFail("A second caller must not enter while the first is preparing.")
        } catch let failure as LocalAppServiceFailure {
            XCTAssertEqual(failure.code, "AGENT_WAKE_IN_PROGRESS")
        }
        gate.resume()
        let firstResult = try await first.value
        XCTAssertEqual(firstResult, "ready")
        XCTAssertTrue(reserved)
        XCTAssertFalse(duplicateEntered)

        do {
            _ = try await preparations.prepare(wake, token: UUID()) {
                duplicateEntered = true
                reserved = false
                return "duplicate"
            }
            XCTFail("The first caller owns the reservation through the browser-open callback.")
        } catch let failure as LocalAppServiceFailure {
            XCTAssertEqual(failure.code, "AGENT_WAKE_IN_PROGRESS")
        }
        XCTAssertTrue(reserved)
        XCTAssertFalse(duplicateEntered)
        preparations.finish(wake, token: token)
    }

    @MainActor
    func testCancelledPreparationKeepsItsSlotUntilItUnwinds() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let replacement = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-b")
        let token = UUID()
        let replacementToken = UUID()
        let preparations = LocalAppAgentHandoffPreparations()
        let gate = HandoffPreparationTestGate()
        let first = Task { @MainActor in
            try await preparations.prepare(wake, token: token) { () async throws -> String in
                await gate.suspend()
                throw CancellationError()
            }
        }
        await gate.waitUntilSuspended()
        preparations.finish(wake, token: token) // cancellation must not release an in-flight owner
        do {
            _ = try await preparations.prepare(wake, token: UUID()) { "duplicate" }
            XCTFail("The canceled owner has not unwound yet.")
        } catch let failure as LocalAppServiceFailure {
            XCTAssertEqual(failure.code, "AGENT_WAKE_IN_PROGRESS")
        }

        let replacementResult = try await preparations.prepare(replacement, token: replacementToken) { "replacement" }
        XCTAssertEqual(replacementResult, "replacement")
        preparations.finish(replacement, token: replacementToken)
        gate.resume()
        do {
            _ = try await first.value
            XCTFail("The canceled preparation must fail.")
        } catch is CancellationError {}

        let retryToken = UUID()
        let retry = try await preparations.prepare(wake, token: retryToken) { "retry" }
        XCTAssertEqual(retry, "retry")
        preparations.finish(wake, token: retryToken)
    }

    @MainActor
    func testFailedPreparationAndBrowserCallbackReleaseTheirOwnership() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let preparations = LocalAppAgentHandoffPreparations()
        let firstToken = UUID()
        do {
            _ = try await preparations.prepare(wake, token: firstToken) { () async throws -> String in
                throw LocalAppServiceFailure("APP_BUSY", "fixture")
            }
            XCTFail("The first preparation must fail.")
        } catch let failure as LocalAppServiceFailure {
            XCTAssertEqual(failure.code, "APP_BUSY")
        }
        let ready = try await preparations.prepare(wake, token: firstToken) { "ready" }
        XCTAssertEqual(ready, "ready")
        preparations.finish(wake, token: firstToken) // accepted or rejected callback
        let retryToken = UUID()
        let retry = try await preparations.prepare(wake, token: retryToken) { "retry" }
        XCTAssertEqual(retry, "retry")
        XCTAssertFalse(preparations.finish(wake, token: firstToken), "A late callback must not release the new owner.")
        XCTAssertTrue(preparations.isReady(wake, token: retryToken))
        preparations.finish(wake, token: retryToken)
    }

    func testStaleHandoffReasonDistinguishesReplacementAndRevisionChange() {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-a")
        let replacement = LocalAppAgentWakeRequest(appID: "app-a", revision: 1, runID: "run-b")
        XCTAssertEqual(LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake, currentWake: replacement, currentRevision: 1
        ), .requestReplaced)
        XCTAssertEqual(LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake, currentWake: wake, currentRevision: 2
        ), .releaseChanged)
        XCTAssertEqual(LocalAppAgentHandoffCancellationPolicy.staleReason(
            wake: wake, currentWake: nil, currentRevision: nil
        ), .acquireFailure)
    }

    func testExistingTaskOperationsReuseTheAvailableHost() {
        for operation in [
            "tasks.list", "eventsNative", "tasks.cancel", "wake.clear",
            "interactions", "answer", "settings"
        ] {
            let request = #"{"op":"\#(operation)"}"#
            XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: request), .available, operation)
        }

        for operation in ["wake.reserve", "startNative", "catalog", "connectMcp", "configure"] {
            let request = #"{"op":"\#(operation)"}"#
            XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: request), .currentConfiguration, operation)
        }
        XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: #"{"op":"configure","settings":null}"#), .available)
        XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: #"{"op":"registerAuthoring"}"#), .none)
        XCTAssertEqual(LocalAppAgentHostPolicy.requirement(for: "invalid JSON"), .currentConfiguration)
    }

    func testModelHostReusesOnlyTheCurrentConfiguration() {
        XCTAssertEqual(LocalAppAgentModelHostConfiguration.decide(
            hasHost: false, currentDigest: nil, requestedDigest: "new"
        ), .build)
        XCTAssertEqual(LocalAppAgentModelHostConfiguration.decide(
            hasHost: true, currentDigest: "same", requestedDigest: "same"
        ), .reuse)
        XCTAssertEqual(LocalAppAgentModelHostConfiguration.decide(
            hasHost: true, currentDigest: "old", requestedDigest: "new"
        ), .restartRequired)
        XCTAssertEqual(LocalAppAgentModelHostConfiguration.decide(
            hasHost: true, currentDigest: nil, requestedDigest: "new"
        ), .restartRequired)
    }

    func testKeychainReadFailureIsNotADeletedValue() throws {
        XCTAssertNil(try Keychain.decodeRead(status: errSecItemNotFound, data: nil))
        XCTAssertNil(try Keychain.decodeRead(status: errSecSuccess, data: Data()))
        XCTAssertEqual(try Keychain.decodeRead(status: errSecSuccess, data: Data("fixture".utf8)), "fixture")
        XCTAssertThrowsError(try Keychain.decodeRead(status: errSecInteractionNotAllowed, data: nil)) { error in
            XCTAssertEqual(error as? Keychain.ReadError, .status(errSecInteractionNotAllowed))
        }
        XCTAssertThrowsError(try Keychain.decodeRead(status: errSecSuccess, data: nil)) { error in
            XCTAssertEqual(error as? Keychain.ReadError, .invalidData)
        }
        XCTAssertThrowsError(try Keychain.decodeRead(status: errSecSuccess, data: Data([0xff]))) { error in
            XCTAssertEqual(error as? Keychain.ReadError, .invalidData)
        }
    }
}

@MainActor
private final class HandoffPreparationTestGate {
    private var suspended: CheckedContinuation<Void, Never>?
    private var started: CheckedContinuation<Void, Never>?

    func suspend() async {
        await withCheckedContinuation { continuation in
            suspended = continuation
            started?.resume()
            started = nil
        }
    }

    func waitUntilSuspended() async {
        if suspended != nil { return }
        await withCheckedContinuation { started = $0 }
    }

    func resume() {
        suspended?.resume()
        suspended = nil
    }
}

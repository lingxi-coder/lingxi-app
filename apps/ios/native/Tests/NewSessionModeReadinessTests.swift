import XCTest
@testable import LingxiCode

final class NewSessionModeReadinessTests: XCTestCase {
    func testRepeatedNewestChoiceNeedsItsOwnEventAndSubmission() {
        var readiness = NewSessionModeReadiness()
        let first = readiness.request(.chat, sessionID: "session-a")
        XCTAssertTrue(readiness.beginSubmission(first, epoch: 1))
        let skipped = readiness.request(.code, sessionID: "session-a")
        let newest = readiness.request(.chat, sessionID: "session-a")
        XCTAssertFalse(readiness.beginSubmission(skipped, epoch: 1))
        XCTAssertTrue(readiness.beginSubmission(newest, epoch: 1))
        readiness.observe(sessionID: "session-a", mode: .chat, pending: nil, epoch: 1)
        readiness.submitted(newest)
        XCTAssertFalse(readiness.takeReady(sessionID: "session-a", mode: .chat, pending: nil))
        readiness.observe(sessionID: "session-a", mode: .chat, pending: nil, epoch: 1)
        XCTAssertTrue(readiness.takeReady(sessionID: "session-a", mode: .chat, pending: nil))
    }

    func testPendingEventAndFailureStayBlockedUntilRetryApplies() {
        var readiness = NewSessionModeReadiness()
        let failed = readiness.request(.chat, sessionID: "session-a")
        XCTAssertTrue(readiness.beginSubmission(failed, epoch: 1))
        XCTAssertTrue(readiness.failed(failed))
        XCTAssertFalse(readiness.takeReady(sessionID: "session-a", mode: .chat, pending: nil))
        let retry = readiness.request(.chat, sessionID: "session-a")
        XCTAssertTrue(readiness.beginSubmission(retry, epoch: 1))
        readiness.submitted(retry)
        readiness.observe(sessionID: "session-a", mode: .code, pending: .chat, epoch: 1)
        XCTAssertFalse(readiness.takeReady(sessionID: "session-a", mode: .code, pending: .chat))
        readiness.observe(sessionID: "session-a", mode: .chat, pending: nil, epoch: 1)
        XCTAssertTrue(readiness.takeReady(sessionID: "session-a", mode: .chat, pending: nil))
    }

    func testOwnerAndEpochChangesRejectOldEvents() {
        var readiness = NewSessionModeReadiness()
        let old = readiness.request(.chat, sessionID: "old")
        XCTAssertTrue(readiness.beginSubmission(old, epoch: 1))
        readiness.reset()
        let current = readiness.request(.chat, sessionID: "current")
        XCTAssertTrue(readiness.beginSubmission(current, epoch: 2))
        readiness.submitted(current)
        readiness.observe(sessionID: "old", mode: .chat, pending: nil, epoch: 1)
        readiness.observe(sessionID: "old", mode: .chat, pending: nil, epoch: 2)
        XCTAssertFalse(readiness.takeReady(sessionID: "current", mode: .chat, pending: nil))
        readiness.observe(sessionID: "current", mode: .chat, pending: nil, epoch: 2)
        XCTAssertTrue(readiness.takeReady(sessionID: "current", mode: .chat, pending: nil))
    }

    func testAppliedNewestChoiceDiscardsSupersededPendingObservation() {
        var readiness = NewSessionModeReadiness()
        let old = readiness.request(.chat, sessionID: "session-a")
        XCTAssertTrue(readiness.beginSubmission(old, epoch: 1))
        readiness.submitted(old)
        readiness.observe(sessionID: "session-a", mode: .code, pending: .chat, epoch: 1)
        let newest = readiness.request(.code, sessionID: "session-a")
        XCTAssertTrue(readiness.beginSubmission(newest, epoch: 1))
        readiness.submitted(newest)
        readiness.observe(sessionID: "session-a", mode: .code, pending: nil, epoch: 1)
        XCTAssertTrue(readiness.takeReady(sessionID: "session-a", mode: .code, pending: nil))
        let next = readiness.request(.chat, sessionID: "session-a")
        XCTAssertTrue(readiness.beginSubmission(next, epoch: 1))
        readiness.submitted(next)
        readiness.observe(sessionID: "session-a", mode: .chat, pending: nil, epoch: 1)
        XCTAssertTrue(readiness.takeReady(sessionID: "session-a", mode: .chat, pending: nil))
    }

    func testEventBeforeCommandReturnDoesNotUnlockEarly() {
        var readiness = NewSessionModeReadiness()
        let request = readiness.request(.chat, sessionID: "")
        XCTAssertTrue(readiness.beginSubmission(request, epoch: 1))
        readiness.observe(sessionID: "bootstrap", mode: .chat, pending: nil, epoch: 1)
        XCTAssertFalse(readiness.takeReady(sessionID: "bootstrap", mode: .chat, pending: nil))
        readiness.submitted(request)
        XCTAssertTrue(readiness.takeReady(sessionID: "bootstrap", mode: .chat, pending: nil))
    }
}

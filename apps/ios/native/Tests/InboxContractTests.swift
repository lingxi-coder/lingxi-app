import Foundation
import XCTest
@testable import LingxiCode

final class InboxContractTests: XCTestCase {
    private let ref = InboxRef(kind: .team, sourceId: "pair", conversationId: "friend_chat", generation: 3)

    private func friendRow() -> InboxRow {
        InboxRow(key: ref.key, ref: ref, category: .friend, title: "Friend",
                 sourceCursor: 18, readThrough: 11, unreadCount: 4, pendingActionCount: 1,
                 manualUnread: true, manualUnreadRevision: 5)
    }

    func testCanonicalFriendIdentityAndGeneration() throws {
        let row = friendRow()
        XCTAssertTrue(row.valid)
        XCTAssertEqual(row.id, "team:pair:friend_chat")
        XCTAssertEqual(InboxRef(key: row.key, generation: 3), ref)
        XCTAssertNotEqual(InboxRef(key: row.key, generation: 4), ref)
        XCTAssertNil(InboxRef(key: "friend:pair:friend_chat", generation: 3))
        XCTAssertNil(InboxRef(key: "team:pair:../friend_chat", generation: 3))
        XCTAssertNil(InboxRef(key: row.key, generation: 0))
        let wire = try JSONEncoder().encode(row)
        XCTAssertEqual(try JSONDecoder().decode(InboxRow.self, from: wire), row)
    }

    func testFrozenReadAndNewManualReminderRemainIndependent() {
        let row = friendRow()
        let frozen = row.frozenRead()
        XCTAssertEqual(frozen.through, 18)
        XCTAssertEqual(frozen.manualUnreadRevision, 5)
        var state = InboxAccountState()
        state.applyLocal(.init(action: .markUnread, ref: ref), row: row)
        state.applyLocal(.init(frozen), row: row)
        XCTAssertEqual(state.personal[row.key]?.readThrough, 18)
        XCTAssertTrue(state.personal[row.key]?.manualUnread == true)
        XCTAssertEqual(state.personal[row.key]?.manualUnreadRevision, 6)
        XCTAssertTrue(row.needsAttention)
    }

    func testOutboxCoalescesReadsAndKeepsOrderAcrossReminderAndInFlight() throws {
        let row = friendRow()
        let first = InboxOperation(row.frozenRead(through: 14))
        var state = InboxAccountState()
        try state.enqueue([first, .init(row.frozenRead(through: 12))])
        XCTAssertEqual(state.queue.map(\.through), [14])
        let wire = state.queue[0]
        let reminder = InboxOperation(action: .markUnread, ref: ref)
        try state.enqueue([.init(row.frozenRead(through: 16)), reminder, .init(row.frozenRead(through: 18))], inFlight: wire.id)
        XCTAssertEqual(state.queue.map(\.action), [.markRead, .markRead, .markUnread, .markRead])
        XCTAssertEqual(state.queue.map(\.through), [14, 16, nil, 18])
    }

    func testAdmissionBackpressureLeavesDurableQueueUnchanged() throws {
        var state = InboxAccountState()
        try state.enqueue([.init(friendRow().frozenRead())])
        let saved = state
        XCTAssertThrowsError(try state.enqueue([.init(action: .markUnread, ref: ref)], maxEntries: 1))
        XCTAssertThrowsError(try state.enqueue([.init(action: .markUnread, ref: ref)], maxBytes: 1))
        XCTAssertEqual(saved, state)
        XCTAssertEqual(try JSONDecoder().decode(InboxAccountState.self, from: JSONEncoder().encode(state)), saved)
    }

    func testUnpinSurvivesRestartAndMergesWithPreferenceChanges() throws {
        var state = InboxAccountState()
        try state.enqueue([.init(action: .preferences, ref: ref, preferences: .init(pinOrder: 30, notificationMode: .all)),
                           .init(action: .preferences, ref: ref, preferences: .init(clearPin: true, archived: true))])
        let persisted = try JSONEncoder().encode(state)
        state = try JSONDecoder().decode(InboxAccountState.self, from: persisted)
        let patch = try XCTUnwrap(state.queue.first?.preferences)
        XCTAssertTrue(patch.clearPin == true)
        XCTAssertNil(patch.pinOrder)
        XCTAssertEqual(patch.notificationMode, .all)
        XCTAssertTrue(patch.archived == true)
        let wire = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(patch)) as? [String: Any])
        XCTAssertTrue(wire["pinOrder"] is NSNull)
    }

    func testVisibilityNeverJumpsOverUnseenMiddle() {
        var visibility = InboxReadVisibility()
        XCTAssertEqual(visibility.observe([17, 18], after: 11), 11)
        XCTAssertEqual(visibility.observe([12, 13], after: 11), 13)
        XCTAssertEqual(visibility.observe([15, 16], after: 13), 13)
        XCTAssertEqual(visibility.observe([14], after: 13), 18)
        XCTAssertEqual(visibility.observe([3], after: 18), 18)
    }

    func testAssistantOnlyCountsCommittedActivityAboveBaseline() {
        let assistant = InboxRef(kind: .assistant, sourceId: "device", conversationId: "session", generation: 1)
        let row = InboxRow(key: assistant.key, ref: assistant, category: .assistant, title: "Assistant")
        let activity = InboxAssistantActivity(sourceId: assistant.sourceId, cursor: 9, baselineCursor: 7,
                                              lastMeaningfulAt: 600, previewKind: "reply", pendingActionCount: 1,
                                              failed: false, sourceRevision: 20)
        let ready = InboxAssistantSummary(activity: activity).updating(row)
        XCTAssertEqual(ready.readThrough, 7)
        XCTAssertEqual(ready.unreadCount, 2)
        XCTAssertTrue(ready.needsAttention)
        XCTAssertEqual(ready.sourceRevision, 20)
    }

    func testPushAccountRouteExpiryAndPersistentDedupe() throws {
        let payload: [String: String] = [
            "kind": "inbox", "key": ref.key, "generation": "3", "through": "18", "eventKind": "dm",
            "recipientUid": "account", "expiresAt": "3000", "route": "inbox/team%3Apair%3Afriend_chat",
            "notificationId": "event"
        ]
        XCTAssertNil(InboxPush.parse(payload, currentUID: "different", enabled: true, now: 2000))
        XCTAssertNil(InboxPush.parse(payload, currentUID: "account", enabled: true, now: 3000))
        XCTAssertNil(InboxPush.parse(payload.merging(["route": "inbox/team:wrong:friend_chat"]) { _, new in new },
                                     currentUID: "account", enabled: true, now: 2000))
        let push = try XCTUnwrap(InboxPush.parse(payload, currentUID: "account", enabled: true, now: 2000))
        var state = InboxAccountState()
        XCTAssertTrue(state.admitDelivery(push.eventID, now: 2000, expiresAt: push.expiresAt))
        state = try JSONDecoder().decode(InboxAccountState.self, from: JSONEncoder().encode(state))
        XCTAssertFalse(state.admitDelivery(push.eventID, now: 2001, expiresAt: push.expiresAt))
        XCTAssertEqual(state.delivered[push.eventID], 2000 + InboxAccountState.deliveryLifetime)
    }

    func testAssistantTwoCommittedResultsSeenOutOfOrderAdvanceOnlyContiguously() {
        let first = UUID()
        let second = UUID()
        let earlierBlockOfFirstResult = UUID()
        let page = [
            InboxAssistantReadEntry(cursor: 12, resultUuids: [earlierBlockOfFirstResult.uuidString, first.uuidString]),
            InboxAssistantReadEntry(cursor: 13, resultUuids: [second.uuidString])
        ]
        var exposure = InboxAssistantReadWindow()
        exposure.confirm([second, earlierBlockOfFirstResult])
        XCTAssertEqual(exposure.advance(page, after: 11), 11)
        exposure.confirm([first])
        XCTAssertEqual(exposure.advance(page, after: 11), 13)
        XCTAssertEqual(exposure.advance(page, after: 11), 13)

        var differentPresentation = InboxAssistantReadWindow()
        differentPresentation.confirm([first])
        XCTAssertEqual(differentPresentation.advance(page, after: 11), 12)
    }

    func testAssistantApprovalHasIndependentRevisionAndTimestamp() {
        let assistant = InboxRef(kind: .assistant, sourceId: "device", conversationId: "session", generation: 1)
        let row = InboxRow(key: assistant.key, ref: assistant, category: .assistant, title: "Assistant")
        let activity = InboxAssistantActivity(sourceId: assistant.sourceId, cursor: 0, baselineCursor: 0,
                                              lastMeaningfulAt: 0, previewKind: nil, pendingActionCount: 1,
                                              failed: false, sourceRevision: 3, pendingActionKind: .approval,
                                              pendingActionAt: 6_000, pendingActionRevision: 2)
        let summary = InboxAssistantSummary(activity: activity).updating(row)
        XCTAssertEqual(summary.state, "waiting_for_approval")
        XCTAssertEqual(summary.lastActivityAt, 6_000)
        XCTAssertEqual(summary.frozenRead().observedPendingActionRevision, 2)
        XCTAssertEqual(summary.frozenRead().through, 0)
        XCTAssertTrue(summary.needsAttention)
        XCTAssertFalse(summary.isUnread)
    }

    func testNestedStaleMembershipPrunesOnlyRejectedSourceGeneration() throws {
        let failure = InboxAuthorityFailure(status: 409,
            data: Data(#"{"error":{"code":"STALE_MEMBERSHIP"}}"#.utf8))
        XCTAssertEqual(failure.code, "STALE_MEMBERSHIP")
        XCTAssertTrue(failure.authorityDenied)
        let anotherThread = InboxRef(kind: .team, sourceId: ref.sourceId, conversationId: "channel", generation: ref.generation)
        let otherSource = InboxRef(kind: .team, sourceId: "elsewhere", conversationId: "channel", generation: ref.generation)
        let newGeneration = InboxRef(kind: .team, sourceId: ref.sourceId, conversationId: "channel", generation: ref.generation + 1)
        let refused = InboxOperation(action: .markUnread, ref: ref)
        let independent = InboxOperation(action: .markUnread, ref: otherSource)
        let renewal = InboxOperation(action: .markUnread, ref: newGeneration)
        var state = InboxAccountState()
        try state.enqueue([refused, .init(action: .markUnread, ref: anotherThread), independent, renewal])
        state.discardRejected(refused, rejected: ref, failure: failure)
        XCTAssertEqual(state.queue, [independent, renewal])
    }

    func testCachedCloudRowExpiresAndDenialRevokesBeforeLease() throws {
        let row = friendRow()
        let record = InboxCachedRow(row: row, verifiedAt: 5_000)
        let restored = try JSONDecoder().decode(InboxCachedRow.self, from: JSONEncoder().encode(record))
        XCTAssertTrue(restored.isAuthorized(at: 5_000 + InboxCachedRow.lease - 1))
        XCTAssertFalse(restored.isAuthorized(at: 5_000 + InboxCachedRow.lease))
        XCTAssertFalse(restored.isAuthorized(at: 4_999))
        let denied = InboxAuthorityFailure(status: 403, data: Data(#"{"error":{"code":"SOURCE_ACCESS_REQUIRED"}}"#.utf8))
        XCTAssertTrue(denied.revokes(row.ref, rejected: row.ref))
        let other = InboxRef(kind: .team, sourceId: row.ref.sourceId, conversationId: "different", generation: row.ref.generation)
        XCTAssertFalse(denied.revokes(other, rejected: row.ref))
    }

    func testIndexedTitleSearchUsesGlobalQueryAndReplaysServerCursor() throws {
        let prefix = InboxTitleSearch.prefix("  ＦＩｌｅ  ")
        XCTAssertEqual(prefix, "file")
        XCTAssertNil(InboxTitleSearch.prefix("  "))
        XCTAssertEqual(InboxTitleSearch.prefix(String(repeating: "x", count: 119) + "🐦")?.utf16.count, 119)

        let first = InboxListRequest(filter: .unread, category: .assistant, archived: false, searchPrefix: prefix, after: nil)
        XCTAssertEqual(first.requestValues["action"] as? String, "list")
        XCTAssertEqual(first.requestValues["searchPrefix"] as? String, "file")
        XCTAssertEqual(first.requestValues["filter"] as? String, "unread")
        XCTAssertEqual(first.requestValues["category"] as? String, "assistant")
        XCTAssertNil(first.requestValues["after"])

        let cursorJSON = Data(#"{"lastActivityAt":27,"key":"assistant:source:thread","titleSort":"file plan"}"#.utf8)
        let cursor = try JSONDecoder().decode(InboxCursor.self, from: cursorJSON)
        XCTAssertEqual(try JSONDecoder().decode(InboxCursor.self, from: JSONEncoder().encode(cursor)), cursor)
        let next = InboxListRequest(filter: .unread, category: .assistant, archived: false, searchPrefix: prefix, after: cursor)
        let after = try XCTUnwrap(next.requestValues["after"] as? [String: Any])
        XCTAssertEqual(after["titleSort"] as? String, "file plan")
        XCTAssertEqual(after["key"] as? String, cursor.key)
        XCTAssertNil(after["pinOrder"])
        XCTAssertTrue(InboxCursor(lastActivityAt: 27, key: cursor.key).requestValues["pinOrder"] is NSNull)
    }
}

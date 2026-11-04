import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class WorkspaceSessionCatalogTests: XCTestCase {
    func testRecentProjectionRetainsTheDisplayedPhysicalSource() {
        let id = "11111111-2222-4333-8444-555555555555"
        var active = live(id)
        active.transcriptPath = "/sessions/original/\(id).jsonl"
        let cachedCopy = ProjectSessionSummary(sessionId: id, title: "A different source", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat,
            transcriptPath: "/sessions/copy/\(id).jsonl")
        let liveRows = WorkspaceGroupBuilder.mergedSessionRows(live: [active], cached: [cachedCopy])
        let cachedRows = WorkspaceGroupBuilder.mergedSessionRows(live: [], cached: [cachedCopy])
        for (rows, expected) in [(liveRows, active.transcriptPath), (cachedRows, cachedCopy.transcriptPath)] {
            let seed = WorkspaceGroupSeed(scope: .global, kind: .global, title: "Global", subtitle: nil,
                                          updatedAt: nil, sessions: rows)
            let recent = RecentSessionsBuilder.groups(seeds: [seed], filter: .chat, query: "", pinnedAt: [:])
                .flatMap(\.sessions)
            XCTAssertEqual(recent.count, 1)
            XCTAssertEqual(recent.first?.transcriptPath, expected)
            XCTAssertEqual(recent.first?.scope, .global)
        }
    }

    func testPendingSessionIsVisibleWhileLiveCatalogIsStale() {
        let pending = cached("pending", pending: true)
        let rows = WorkspaceGroupBuilder.mergedSessionRows(live: [], cached: [pending])
        XCTAssertEqual(rows.map(\.id), ["pending"])
        XCTAssertEqual(rows.first?.title, pending.title)
        XCTAssertEqual(rows.first?.mode, .chat)
    }

    func testLiveConfirmationReplacesPendingRowWithoutDuplicate() {
        let rows = WorkspaceGroupBuilder.mergedSessionRows(
            live: [live("pending", title: "Confirmed title")],
            cached: [cached("pending", pending: true)]
        )
        XCTAssertEqual(rows.map(\.id), ["pending"])
        XCTAssertEqual(rows.first?.title, "Confirmed title")
        XCTAssertEqual(rows.first?.messageCount, 2)
    }

    func testArchivedCacheSuppressesLiveAndCachedRows() {
        let rows = WorkspaceGroupBuilder.mergedSessionRows(
            live: [live("archived"), live("visible")],
            cached: [cached("archived", archived: true), cached("offline-archived", archived: true)]
        )
        XCTAssertEqual(rows.map(\.id), ["visible"])
    }

    func testRestoredPendingRowAppearsBeforeCatalogCatchesUp() {
        let rows = WorkspaceGroupBuilder.mergedSessionRows(
            live: [live("existing")],
            cached: [cached("restored", pending: true), cached("existing")]
        )
        XCTAssertEqual(Set(rows.map(\.id)), ["restored", "existing"])
        XCTAssertEqual(rows.count, 2)
    }

    func testPendingSubmissionWinsAgainstExistingEmptyLiveRow() {
        let pending = cached("first", pending: true)
        let stale = EngineSession(id: "first", title: "New conversation", mode: .chat, messageCount: 0,
                                  modifiedAt: Date(timeIntervalSince1970: 999), relativeTime: "Before")
        let rows = WorkspaceGroupBuilder.mergedSessionRows(live: [stale], cached: [pending])
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows.first?.title, pending.title)
        XCTAssertEqual(rows.first?.messageCount, 1)
        let confirmed = WorkspaceGroupBuilder.mergedSessionRows(live: [live("first", title: "Confirmed")], cached: [pending])
        XCTAssertEqual(confirmed.first?.title, "Confirmed")
    }

    private func cached(_ id: String, archived: Bool = false, pending: Bool = false) -> ProjectSessionSummary {
        ProjectSessionSummary(sessionId: id, title: "First prompt", messageCount: 1, relativeTime: "Now",
                              updatedAt: Date(timeIntervalSince1970: 1000), mode: .chat,
                              isArchived: archived, pendingCatalogConfirmation: pending)
    }

    private func live(_ id: String, title: String = "Live") -> EngineSession {
        EngineSession(id: id, title: title, mode: .chat, messageCount: 2,
                      modifiedAt: Date(timeIntervalSince1970: 1001), relativeTime: "Now")
    }
}

import XCTest
@testable import LingxiCode

final class RecentSessionsTests: XCTestCase {
    @MainActor
    func testConversationTitleUsesLiveCatalogEvenWhenTranscriptIsNew() {
        let model = ConversationModel(persistDisclosures: false)
        model.isNew = true
        model.engineSessions = [EngineSession(
            id: "named", title: "用户指定的 Session 名称", mode: .chat,
            messageCount: 0, modifiedAt: nil, relativeTime: ""
        )]
        XCTAssertEqual(model.sessionReference(for: "named", cachedTitle: "旧名称").title,
                       "用户指定的 Session 名称")
        model.engineSessions = [EngineSession(
            id: "named", title: "更新后的名称", mode: .chat,
            messageCount: 0, modifiedAt: nil, relativeTime: ""
        )]
        XCTAssertEqual(model.sessionReference(for: "named", cachedTitle: "旧名称").title,
                       "更新后的名称")
        // A delayed catalog for a different session never supplies this title.
        XCTAssertEqual(model.sessionReference(for: "other", cachedTitle: "全局缓存名称").title,
                       "全局缓存名称")
        XCTAssertEqual(model.sessionReference(for: "other").title,
                       String(localized: "chat_new_conversation"))
    }

    private var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        return calendar
    }

    /// 2026-01-15 12:00 UTC.
    private let now = Date(timeIntervalSince1970: 1_768_478_400)

    private func row(
        _ id: String,
        mode: SessionMode = .code,
        title: String? = nil,
        daysAgo: Double? = 0
    ) -> WorkspaceSessionRow {
        WorkspaceSessionRow(
            id: id,
            title: title ?? id,
            mode: mode,
            modifiedAt: daysAgo.map { now.addingTimeInterval(-$0 * 86_400) },
            relativeTime: "",
            messageCount: 1
        )
    }

    private func seed(
        _ scope: ConversationScope,
        _ title: String,
        _ rows: [WorkspaceSessionRow]
    ) -> WorkspaceGroupSeed {
        WorkspaceGroupSeed(
            scope: scope,
            kind: scope == .global ? .global : .project,
            title: title,
            subtitle: nil,
            updatedAt: nil,
            sessions: rows
        )
    }

    private func groups(
        _ seeds: [WorkspaceGroupSeed],
        filter: RecentsFilter = .all,
        query: String = "",
        pinnedAt: [String: Date] = [:],
        statusFor: (String) -> RecentStatus = { _ in .idle }
    ) -> [RecentGroup] {
        RecentSessionsBuilder.groups(
            seeds: seeds,
            filter: filter,
            query: query,
            pinnedAt: pinnedAt,
            statusFor: statusFor,
            now: now,
            calendar: calendar
        )
    }

    func testBucketsFollowLocalCalendarDays() {
        let result = groups([
            seed(.global, "Global", [
                row("today", daysAgo: 0),
                row("yesterday", daysAgo: 1),
                row("two-days", daysAgo: 2),
                row("six-days", daysAgo: 6),
                row("seven-days", daysAgo: 7),
                row("unknown", daysAgo: nil),
            ])
        ])

        XCTAssertEqual(result.map(\.bucket), [.today, .yesterday, .lastWeek, .earlier])
        XCTAssertEqual(result[0].sessions.map(\.id), ["today"])
        XCTAssertEqual(result[1].sessions.map(\.id), ["yesterday"])
        XCTAssertEqual(result[2].sessions.map(\.id), ["two-days", "six-days"])
        XCTAssertEqual(result[3].sessions.map(\.id), ["seven-days", "unknown"])
    }

    func testPinnedSessionsLeadRegardlessOfAgeAndLatestPinIsFirst() {
        let result = groups(
            [
                seed(.global, "Global", [
                    row("recent", daysAgo: 0),
                    row("old-pin", daysAgo: 30),
                    row("new-pin", daysAgo: 20),
                ])
            ],
            pinnedAt: [
                "old-pin": now.addingTimeInterval(-500),
                "new-pin": now.addingTimeInterval(-10),
            ]
        )

        XCTAssertEqual(result.map(\.bucket), [.pinned, .today])
        XCTAssertEqual(result[0].sessions.map(\.id), ["new-pin", "old-pin"])
    }

    func testFilterNarrowsByModeAcrossWorkspaces() {
        let seeds = [
            seed(.global, "Global", [row("g-chat", mode: .chat), row("g-code", mode: .code)]),
            seed(.project("p"), "Alpha", [row("p-chat", mode: .chat, daysAgo: 1)]),
        ]

        XCTAssertEqual(groups(seeds, filter: .all).flatMap(\.sessions).map(\.id), ["g-chat", "g-code", "p-chat"])
        XCTAssertEqual(groups(seeds, filter: .chat).flatMap(\.sessions).map(\.id), ["g-chat", "p-chat"])
        XCTAssertEqual(groups(seeds, filter: .code).flatMap(\.sessions).map(\.id), ["g-code"])
    }

    func testRowsCarryTheirWorkspaceAndMode() {
        let result = groups([
            seed(.project("p"), "Alpha", [row("s", mode: .chat)])
        ])
        let session = try! XCTUnwrap(result.first?.sessions.first)

        XCTAssertEqual(session.scope, .project("p"))
        XCTAssertEqual(session.workspaceKey, "project.p")
        XCTAssertEqual(session.workspaceName, "Alpha")
        XCTAssertEqual(session.mode, .chat)
    }

    func testSearchMatchesTitleAndWorkspaceName() {
        let seeds = [
            seed(.global, "Global", [row("a", title: "Build spec"), row("b", title: "Notes")]),
            seed(.project("alpha"), "Alpha", [row("c", title: "Unrelated")]),
        ]

        XCTAssertEqual(groups(seeds, query: "spec").flatMap(\.sessions).map(\.id), ["a"])
        XCTAssertEqual(groups(seeds, query: "alpha").flatMap(\.sessions).map(\.id), ["c"])
        XCTAssertTrue(groups(seeds, query: "nothing-matches").isEmpty)
    }

    func testDuplicateSessionIDsAreListedOnce() {
        let result = groups([
            seed(.global, "Global", [row("dup")]),
            seed(.project("p"), "Alpha", [row("dup")]),
        ])

        XCTAssertEqual(result.flatMap(\.sessions).map(\.id), ["dup"])
        XCTAssertEqual(result.first?.sessions.first?.scope, .global)
    }

    func testStatusComesFromTheLiveSessionOnly() {
        let result = groups(
            [seed(.global, "Global", [row("live"), row("idle", daysAgo: 1)])],
            statusFor: { $0 == "live" ? .awaitingConfirmation : .idle }
        )

        XCTAssertEqual(result.flatMap(\.sessions).first { $0.id == "live" }?.status, .awaitingConfirmation)
        XCTAssertEqual(result.flatMap(\.sessions).first { $0.id == "idle" }?.status, .idle)
    }

    func testFutureTimestampsReadAsToday() {
        let result = groups([seed(.global, "Global", [row("skewed", daysAgo: -3)])])

        XCTAssertEqual(result.map(\.bucket), [.today])
    }

    func testProjectSessionsFilterAndOrderNewestFirst() {
        let rows = [
            row("old-code", mode: .code, daysAgo: 9),
            row("chat", mode: .chat, daysAgo: 5),
            row("new-code", mode: .code, daysAgo: 1),
        ]

        XCTAssertEqual(
            RecentSessionsBuilder.sessions(in: rows, filter: .all).map(\.id),
            ["new-code", "chat", "old-code"]
        )
        XCTAssertEqual(RecentSessionsBuilder.sessions(in: rows, filter: .chat).map(\.id), ["chat"])
        XCTAssertEqual(
            RecentSessionsBuilder.sessions(in: rows, filter: .code).map(\.id),
            ["new-code", "old-code"]
        )
    }

    private func project(
        _ id: String,
        name: String? = nil,
        updatedDaysAgo: Double = 100,
        sessions: [ProjectSessionSummary] = []
    ) -> ProjectSnapshot {
        ProjectSnapshot(
            record: ProjectRecord(
                id: id,
                name: name ?? id,
                storageKind: .internal,
                createdAt: now,
                updatedAt: now.addingTimeInterval(-updatedDaysAgo * 86_400)
            ),
            workspace: ProjectWorkspace(projectId: id, hostURL: URL(fileURLWithPath: "/tmp/\(id)")),
            sessions: sessions
        )
    }

    func testPinnedProjectsLeadAndUnpinnedOrderByActivityThenName() {
        let busy = ProjectSessionSummary(
            sessionId: "s",
            title: "s",
            messageCount: 1,
            relativeTime: "",
            updatedAt: now.addingTimeInterval(-86_400)
        )
        let projects = [
            project("b", name: "Bravo"),
            project("a", name: "Alpha"),
            project("busy", sessions: [busy]),
            project("pinned", updatedDaysAgo: 400),
        ]

        let sorted = RecentSessionsBuilder.sortedProjects(
            projects,
            pinnedAt: ["project.pinned": now]
        )

        XCTAssertEqual(sorted.map(\.id), ["pinned", "busy", "a", "b"])
    }
}

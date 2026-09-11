import XCTest

@testable import LingxiCode

/// The transcript's row identity must not change while a turn is streaming.
/// A tool group grows from one tool to two mid-turn; if that flips the row's
/// `ForEach` id, SwiftUI tears the row down and rebuilds it, which moves the
/// scroll position under the reader.
final class ConversationTimelineSegmentsTests: XCTestCase {
    private func trace(_ id: String) -> ConversationToolTrace {
        ConversationToolTrace(id: id, tool: "Bash", status: .completed)
    }

    private func toolGroup(_ traces: [ConversationToolTrace]) -> ConversationTimelineGroup {
        ConversationTimelineGroup(
            id: "run:R1:tools:\(traces[0].id)",
            runID: "R1",
            rows: traces.map { .tool(runID: "R1", trace: $0) },
            status: .running
        )
    }

    func testToolGroupKeepsItsRowIdentityWhenASecondToolArrives() {
        let oneTool = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t1")]))
        let twoTools = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t1"), trace("t2")]))

        XCTAssertEqual(oneTool.count, 1)
        XCTAssertEqual(twoTools.count, 1)
        XCTAssertEqual(
            oneTool,
            twoTools,
            "A tool group must keep one stable ForEach id as it grows from one tool to two"
        )
    }

    func testToolDisclosureIdentitySurvivesLiveToRestoredRunReplacement() {
        let live = toolGroup([trace("stable-tool")])
        let restored = ConversationTimelineGroup(id: "restored-run:new-projection", runID: "restored-run", rows: live.rows, status: .restored)
        XCTAssertEqual(ConversationTimelineView.segmentIDs(in: live), ConversationTimelineView.segmentIDs(in: restored))
    }

    func testTwoToolGroupsInTheSameRunDoNotCollide() {
        let first = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t1"), trace("t2")]))
        let second = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t3"), trace("t4")]))

        XCTAssertNotEqual(first, second)
    }

    func testDesktopProjectionGroupsToolsAcrossHistoricalReasoning() {
        let groups = [
            toolGroup([trace("t1")]),
            ConversationTimelineGroup(id: "thought", runID: "R1", rows: [.reasoning(runID: "R1", activityID: "r", text: "private reasoning")], status: .completed),
            toolGroup([trace("t2")])
        ]
        let projected = ConversationDesktopTimeline.groups(groups)
        XCTAssertEqual(projected.count, 1)
        XCTAssertEqual(projected[0].rows.count, 2)
        XCTAssertEqual(projected[0].id, groups[0].id)
    }

    func testDesktopProjectionKeepsOnlyTrailingLiveThinking() {
        let thought = ConversationTimelineGroup(id: "thought", runID: "R1", rows: [.reasoning(runID: "R1", activityID: "r", text: "thinking")], status: .running)
        XCTAssertEqual(ConversationDesktopTimeline.groups([thought]).count, 1)
        XCTAssertEqual(ConversationDesktopTimeline.groups([thought, toolGroup([trace("t1")])]).count, 1)
        let completed = ConversationTimelineGroup(id: "done", runID: "R1", rows: thought.rows, status: .completed)
        XCTAssertTrue(ConversationDesktopTimeline.groups([completed]).isEmpty)
    }

    func testDesktopToolSummaryUsesLastToolAndShowsOnlyActiveWork() {
        let tools = [trace("done"), ConversationToolTrace(id: "live", tool: "Read", status: .running)]
        XCTAssertEqual(ConversationDesktopTimeline.activeTools(tools).map(\.id), ["live"])
        XCTAssertEqual(ConversationDesktopTimeline.summary(tools), "Read")
    }

    #if canImport(engine_mobileFFI)
    @MainActor
    func testOnlySelectedSourceCanPublishOrRevokeSettings() {
        struct Offline: Error {}
        func source() -> EngineConversationSource {
            EngineConversationSource(config: EngineConfig(
                apiBase: "https://invalid.example", apiKey: "", model: "",
                appSandboxRoot: NSTemporaryDirectory(), projectCwd: nil,
                sessionMode: .code, visionDelegationEnabled: false),
                handleBuilder: { _, _, _ in throw Offline() })
        }
        func snapshot(_ value: String) -> ClientEvent {
            .settingsSnapshot(effectiveJson: "{\"owner\":\"\(value)\"}", provenanceJson: "{}",
                              filesJson: nil, activeJson: nil, locked: [], layersJson: "{}", mergedKeys: [])
        }
        let first = source()
        let second = source()
        let repository = DesktopSettingsRepository.shared
        defer { second.setSettingsActive(false) }
        first.setSettingsActive(true)
        first.applyForTesting(snapshot("first"))
        XCTAssertEqual(repository.effective["owner"] as? String, "first")
        second.setSettingsActive(true)
        second.applyForTesting(snapshot("second"))
        first.applyForTesting(snapshot("stale"))
        first.setSettingsActive(false)
        XCTAssertEqual(repository.effective["owner"] as? String, "second")
        XCTAssertTrue(repository.loaded)
        second.setSettingsActive(false)
        second.applyForTesting(snapshot("after revoke"))
        XCTAssertFalse(repository.loaded)
        XCTAssertTrue(repository.effective.isEmpty)
    }
    #endif

    func testUnownedRunningRowsRenderUnknownWithoutMutatingLiveTraces() {
        let running = ConversationToolTrace(id: "old", tool: "Read", status: .running)
        let active = ConversationToolTrace(id: "live", tool: "Bash", status: .running)
        let input = toolGroup([running, active])
        let display = ConversationDesktopTimeline.groups([input], liveToolIDs: ["live"])
        let statuses = display.flatMap(\.rows).compactMap { row -> ConversationToolStatus? in
            if case let .tool(_, trace) = row { return trace.status }; return nil
        }
        XCTAssertEqual(statuses, [.unknown, .running])
        guard case let .tool(_, raw) = input.rows[0] else { return XCTFail("Expected original tool") }
        XCTAssertEqual(raw.status, .running)
        let settled = ConversationDesktopTimeline.groups([input], liveToolIDs: [], hasLiveOwner: false)
        XCTAssertTrue(settled.flatMap(\.rows).allSatisfy { row in
            if case let .tool(_, trace) = row { return trace.status == .unknown }; return false
        })
    }

    @MainActor
    func testLiveToolOwnershipRequiresCurrentRecoveryOrActiveWorkers() {
        let model = ConversationModel()
        let run = ConversationExecutionRun(id: "run", sessionId: "session", turnId: 1, status: .running,
                                          tools: [ConversationToolTrace(id: "tool", tool: "Read", status: .running)])
        model.items = [.run(run)]
        XCTAssertTrue(model.liveTranscriptToolIDs.isEmpty)
        model.streaming = true
        XCTAssertEqual(model.liveTranscriptToolIDs, ["tool"])
        model.streaming = false
        model.hasUnresolvedTurnRecovery = true
        XCTAssertEqual(model.liveTranscriptToolIDs, ["tool"])
        model.hasUnresolvedTurnRecovery = false
        XCTAssertTrue(model.liveTranscriptToolIDs.isEmpty)
    }

    func testTimelineChevronKeepsATouchVisibleRestingAffordance() {
        XCTAssertGreaterThan(ConversationTimelineChevronPresentation.opacity(isHighlighted: false), 0)
        XCTAssertGreaterThan(ConversationTimelineChevronPresentation.scale(isHighlighted: false), 0)
    }

    func testTimelineChevronHighlightOnlyStrengthensTheAffordance() {
        XCTAssertGreaterThan(
            ConversationTimelineChevronPresentation.opacity(isHighlighted: true),
            ConversationTimelineChevronPresentation.opacity(isHighlighted: false)
        )
        XCTAssertGreaterThan(
            ConversationTimelineChevronPresentation.scale(isHighlighted: true),
            ConversationTimelineChevronPresentation.scale(isHighlighted: false)
        )
    }

    func testRuntimeFooterMotionPolicyHasAnExactTruthTable() {
        XCTAssertEqual(
            RuntimeFooterMotionPolicy.indicatorPresentation(isAnimated: true, hasIcon: false, reduceMotion: false),
            .hidden
        )
        XCTAssertTrue(
            RuntimeFooterMotionPolicy.textSweepIsActive(
                isAnimated: true,
                reduceMotion: false
            )
        )

        XCTAssertEqual(
            RuntimeFooterMotionPolicy.indicatorPresentation(isAnimated: true, hasIcon: false, reduceMotion: true),
            .activeIndicator
        )
        XCTAssertFalse(
            RuntimeFooterMotionPolicy.textSweepIsActive(
                isAnimated: true,
                reduceMotion: true
            )
        )

        for reduceMotion in [false, true] {
            XCTAssertEqual(
                RuntimeFooterMotionPolicy.indicatorPresentation(
                    isAnimated: false,
                    hasIcon: false,
                    reduceMotion: reduceMotion
                ),
                .staticIndicator
            )
            XCTAssertFalse(
                RuntimeFooterMotionPolicy.textSweepIsActive(
                    isAnimated: false,
                    reduceMotion: reduceMotion
                )
            )
            XCTAssertEqual(
                RuntimeFooterMotionPolicy.indicatorPresentation(
                    isAnimated: true,
                    hasIcon: true,
                    reduceMotion: reduceMotion
                ),
                .hidden
            )
            XCTAssertEqual(
                RuntimeFooterMotionPolicy.textSweepIsActive(
                    isAnimated: true,
                    reduceMotion: reduceMotion
                ),
                !reduceMotion
            )
        }
    }
}

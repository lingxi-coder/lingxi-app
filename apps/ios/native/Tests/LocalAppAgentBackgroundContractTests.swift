import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class LocalAppAgentBackgroundContractTests: XCTestCase {
    func testWakeContextKeysIsolateSameRequestAcrossAppsAndRevisions() {
        let appA = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, runID: "run-1")
        let appB = LocalAppAgentWakeRequest(appID: "app-b", revision: 7, runID: "run-1")
        let nextRelease = LocalAppAgentWakeRequest(appID: "app-a", revision: 8, runID: "run-1")
        let retry = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, runID: "run-1")

        XCTAssertNotEqual(appA.contextKey, appB.contextKey)
        XCTAssertNotEqual(appA.contextKey, nextRelease.contextKey)
        XCTAssertEqual(appA.contextKey, retry.contextKey)

        var activeContexts = [appA.contextKey: "task-a", appB.contextKey: "task-b"]
        activeContexts.removeValue(forKey: appA.contextKey) // expiry/clear of app-a
        XCTAssertNil(activeContexts[appA.contextKey])
        XCTAssertEqual(activeContexts[appB.contextKey], "task-b")
    }

    func testHandoffResolvesFreshAppRunAndKeepsThePublicAgentRunID() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, runID: "public-agent-run")
        let current = launch(appID: "app-a", revision: 7, appRunID: "current-app-run")
        var resolutionCount = 0

        let resolved = try await LocalAppAgentHandoffLaunchResolver.resolve(wake) {
            resolutionCount += 1
            return current
        }
        let readyURL = try LocalAppAgentWakeURL.ready(resolved, agentRunID: wake.runID)
        let query = URLComponents(url: readyURL, resolvingAgainstBaseURL: false)?.queryItems

        XCTAssertEqual(resolutionCount, 1)
        XCTAssertTrue(readyURL.path.hasPrefix("/__run/current-app-run/"))
        XCTAssertEqual(query?.first(where: { $0.name == "__lingxi_ready" })?.value, wake.runID)
    }

    func testHandoffRejectsRunURLsAndAppRevisionsThatDoNotMatchTheWake() async throws {
        let wake = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, runID: "public-agent-run")
        let staleURL = launch(
            appID: "app-a",
            revision: 7,
            appRunID: "current-app-run",
            urlRunID: "old-app-run"
        )
        XCTAssertThrowsError(try LocalAppAgentWakeURL.ready(staleURL, agentRunID: wake.runID)) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "STALE_APP_RUN_URL")
        }

        do {
            _ = try await LocalAppAgentHandoffLaunchResolver.resolve(wake) {
                self.launch(appID: "app-a", revision: 8, appRunID: "new-revision-run")
            }
            XCTFail("A changed app revision must not consume the queued Agent wake.")
        } catch let failure as LocalAppServiceFailure {
            XCTAssertEqual(failure.code, "STALE_AGENT_WAKE")
        }
    }

    func testHandoffRequiresTheExactGatewayRunPrefixAndSafeRunID() {
        let wakeRunID = "public-agent-run"
        let malformedPaths = [
            "/__run//current-app-run/index.html",
            "//__run/current-app-run/index.html",
            "/__run/current-app-run%2Fsub/index.html"
        ]

        for path in malformedPaths {
            let malformed = launch(appID: "app-a", revision: 7, appRunID: "current-app-run", urlPath: path)
            XCTAssertThrowsError(try LocalAppAgentWakeURL.ready(malformed, agentRunID: wakeRunID)) { error in
                XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "STALE_APP_RUN_URL")
            }
        }

        let encodedRunID = launch(appID: "app-a", revision: 7, appRunID: "current%2Frun")
        XCTAssertThrowsError(try LocalAppAgentWakeURL.ready(encodedRunID, agentRunID: wakeRunID)) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "STALE_APP_RUN_URL")
        }
    }

    private func launch(
        appID: String,
        revision: UInt64,
        appRunID: String,
        urlRunID: String? = nil,
        urlPath: String? = nil
    ) -> LocalAppLaunch {
        let origin = "http://127.0.0.1:33936"
        let run = UnifiedAppRun(
            appId: appID,
            runId: appRunID,
            revision: revision,
            releaseDigest: "test-digest",
            phase: "running",
            services: [],
            error: nil,
            startedAtMs: 1,
            updatedAtMs: 1
        )
        let pathRunID = urlRunID ?? appRunID
        return LocalAppLaunch(
            run: run,
            url: URL(string: "\(origin)\(urlPath ?? "/__run/\(pathRunID)/index.html")")!,
            origin: origin
        )
    }

    func testPermanentReleaseErrorsEndTheLeaseButStorageErrorsRemainRetryable() {
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("release_changed", "release changed")
        ))
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("app_not_found", "app removed")
        ))
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("INVALID_AGENT_EVENT", "invalid terminal event")
        ))
        XCTAssertFalse(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("agent_storage_unavailable", "try again")
        ))
    }

    func testObservedTurnCountDoesNotBecomeACompletionPercentage() {
        let progress = Progress(totalUnitCount: 0)
        LocalAppAgentProgressReporting.update(
            progress,
            outputBytes: 0,
            completedModelTurns: 3,
            turnLimit: 20
        )

        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.totalUnitCount, -1)
        XCTAssertEqual(progress.completedUnitCount, 0)
        XCTAssertEqual(progress.fractionCompleted, 0)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Completed 3 model turns; limit 20")

        LocalAppAgentProgressReporting.finish(progress, success: true)
        XCTAssertEqual(progress.totalUnitCount, 1)
        XCTAssertEqual(progress.completedUnitCount, 1)
        XCTAssertEqual(progress.fractionCompleted, 1)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Completed 3 model turns; limit 20")
    }

    func testOutputProgressCountsUTF8TextAndToolArgumentsOnlyOnce() throws {
        var output = LocalAppAgentOutputProgress()
        let batch = LocalAppAgentNativeEvents(events: [
            .object(["type": .string("RUN_STARTED"), "runId": .string("run-1"), "threadId": .string("thread-1")]),
            textEvent("A汉🙂"),
            .object(["type": .string("TOOL_CALL_ARGS"), "toolCallId": .string("tool-1"), "delta": .string("{\"n\":1}")]),
            textEvent(""),
            .object(["type": .string("CUSTOM"), "delta": .string("not output")])
        ], sequence: 5)

        XCTAssertTrue(try output.consume(batch, runID: "run-1", threadID: "thread-1"))
        XCTAssertEqual(output.sequence, 5)
        XCTAssertEqual(output.outputBytes, Int64("A汉🙂".utf8.count + "{\"n\":1}".utf8.count))
        XCTAssertFalse(try output.consume(batch, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(try output.consume(.init(events: [], sequence: 5), runID: "run-1"))
        XCTAssertFalse(try output.consume(.init(events: [textEvent("")], sequence: 6), runID: "run-1"))
        XCTAssertEqual(output.sequence, 6)
        XCTAssertEqual(output.outputBytes, 15)
    }

    func testInvalidOrRegressingPagesAreAtomicAndNeverInflateOutput() throws {
        var output = LocalAppAgentOutputProgress()
        XCTAssertTrue(try output.consume(.init(events: [textEvent("ok")], sequence: 1), runID: "run-1"))
        let invalidEvents: [LocalAppJSON] = [
            .object(["type": .string("TEXT_MESSAGE_CONTENT"), "messageId": .string("m"), "delta": .number(1)]),
            .object(["type": .string("TOOL_CALL_ARGS"), "delta": .string("missing id")]),
            .object(["type": .string("RUN_ERROR"), "message": .null]),
            .object(["type": .string("RUN_FINISHED"), "runId": .string("other"), "threadId": .string("thread-1")]),
            .object(["type": .string("RUN_STARTED"), "runId": .string("run-1"), "threadId": .string("other")]),
            .object(["type": .number(1)])
        ]
        for invalid in invalidEvents {
            XCTAssertThrowsError(try output.consume(
                .init(events: [textEvent("new"), invalid], sequence: 3),
                runID: "run-1", threadID: "thread-1"
            ))
            XCTAssertEqual(output.sequence, 1)
            XCTAssertEqual(output.outputBytes, 2)
        }
        XCTAssertThrowsError(try output.consume(.init(events: [], sequence: 0), runID: "run-1"))
        XCTAssertThrowsError(try output.consume(.init(events: [textEvent("gap")], sequence: 3), runID: "run-1"))
        XCTAssertEqual(output.sequence, 1)
        XCTAssertEqual(output.outputBytes, 2)
        XCTAssertEqual(LocalAppAgentOutputProgress.saturatingAdd(Int64.max - 1, 8), Int64.max)
    }

    func testObservedBytesAreNotResetByTurnsAndOnlySuccessCompletesProgress() {
        let progress = Progress(totalUnitCount: 0)
        LocalAppAgentProgressReporting.update(progress, outputBytes: 11)
        LocalAppAgentProgressReporting.update(progress, outputBytes: 11, completedModelTurns: 20, turnLimit: 20)

        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.totalUnitCount, -1)
        XCTAssertEqual(progress.completedUnitCount, 11)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Received 11 output bytes; total unknown")
        LocalAppAgentProgressReporting.finish(progress, success: false)
        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.totalUnitCount, -1)
        XCTAssertEqual(progress.completedUnitCount, 11)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Received 11 output bytes; total unknown; task ended")

        LocalAppAgentProgressReporting.update(progress, outputBytes: 11)
        LocalAppAgentProgressReporting.finish(progress, success: true)
        XCTAssertEqual(progress.totalUnitCount, 11)
        XCTAssertEqual(progress.completedUnitCount, 11)
        XCTAssertEqual(progress.fractionCompleted, 1)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Received 11 output bytes; complete")
    }

    func testCancelledTerminalDoesNotFinishProgressAsSuccess() throws {
        var output = LocalAppAgentOutputProgress()
        let cancelled: LocalAppJSON = .object([
            "type": .string("RUN_FINISHED"), "runId": .string("run-1"), "threadId": .string("thread-1"),
            "outcome": .object(["type": .string("cancelled")])
        ])
        XCTAssertTrue(try output.consume(.init(events: [textEvent("partial"), cancelled], sequence: 2),
                                         runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessfulTerminal(cancelled, runID: "run-1", threadID: "thread-1"))
        let progress = Progress(totalUnitCount: 0)
        LocalAppAgentProgressReporting.update(progress, outputBytes: output.outputBytes)
        LocalAppAgentProgressReporting.finish(progress, success: false)
        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.completedUnitCount, 7)
    }

    private func textEvent(_ delta: String) -> LocalAppJSON {
        .object(["type": .string("TEXT_MESSAGE_CONTENT"), "messageId": .string("m"), "delta": .string(delta)])
    }
}

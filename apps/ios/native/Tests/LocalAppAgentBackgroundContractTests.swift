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
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("AGENT_TERMINAL_EVENT_MISSING", "missing terminal event")
        ))
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("AGENT_TERMINAL_EVENTS_UNAVAILABLE", "terminal event storage unavailable")
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

    func testNativeAndEventOutputAreAtomicMonotonicAndNotAddedTogether() throws {
        var empty = LocalAppAgentOutputProgress()
        XCTAssertTrue(try empty.consume(.init(events: [], sequence: 0, nativeOutputBytes: 4), runID: "run-1"))
        XCTAssertEqual(empty.outputBytes, 4)
        var output = LocalAppAgentOutputProgress()
        let first = LocalAppAgentNativeEvents(events: [textEvent("abc")], sequence: 1, nativeOutputBytes: 5)
        XCTAssertTrue(try output.consume(first, runID: "run-1"))
        XCTAssertEqual(output.eventOutputBytes, 3)
        XCTAssertEqual(output.nativeOutputBytes, 5)
        XCTAssertEqual(output.outputBytes, 5)

        XCTAssertTrue(try output.consume(.init(events: [], sequence: 1, nativeOutputBytes: 8), runID: "run-1"))
        XCTAssertFalse(try output.consume(.init(events: [textEvent("abc")], sequence: 1, nativeOutputBytes: 8), runID: "run-1"))
        XCTAssertEqual(output.eventOutputBytes, 3)
        XCTAssertEqual(output.outputBytes, 8)

        XCTAssertTrue(try output.consume(.init(events: [textEvent("abcdef")], sequence: 2, nativeOutputBytes: 8), runID: "run-1"))
        XCTAssertEqual(output.eventOutputBytes, 9)
        XCTAssertEqual(output.outputBytes, 9)
        XCTAssertFalse(try output.consume(.init(events: [], sequence: 2, nativeOutputBytes: 1), runID: "run-1"))
        XCTAssertEqual(output.nativeOutputBytes, 8)

        let invalid: LocalAppJSON = .object(["type": .string("TEXT_MESSAGE_CONTENT"), "delta": .string("bad")])
        for sequence: UInt64 in [2, 3] {
            XCTAssertThrowsError(try output.consume(
                .init(events: [invalid], sequence: sequence, nativeOutputBytes: 100), runID: "run-1"
            ))
            XCTAssertEqual(output.sequence, 2)
            XCTAssertEqual(output.eventOutputBytes, 9)
            XCTAssertEqual(output.nativeOutputBytes, 8)
        }
        XCTAssertThrowsError(try output.consume(
            .init(events: [textEvent("gap")], sequence: 5, nativeOutputBytes: 100), runID: "run-1"
        ))
        XCTAssertEqual(output.sequence, 2)
        XCTAssertEqual(output.nativeOutputBytes, 8)
        XCTAssertTrue(try output.consume(.init(events: [textEvent("xy")], sequence: 3), runID: "run-1"))
        XCTAssertEqual(output.outputBytes, 11)
        XCTAssertThrowsError(try output.consume(.init(events: [], sequence: 3, nativeOutputBytes: UInt64.max), runID: "run-1"))
        XCTAssertEqual(output.outputBytes, 11)
        XCTAssertTrue(try output.consume(.init(events: [], sequence: 3, nativeOutputBytes: LocalAppAgentNativeEvents.maximumSafeNativeOutputBytes), runID: "run-1"))
        XCTAssertEqual(output.outputBytes, Int64(LocalAppAgentNativeEvents.maximumSafeNativeOutputBytes))
    }

    func testNativeOutputCounterDecodesWhenPresentOrAbsent() throws {
        let present = try JSONDecoder().decode(LocalAppAgentNativeEvents.self, from: Data(#"{"events":[],"sequence":0,"nativeOutputBytes":42}"#.utf8))
        let absent = try JSONDecoder().decode(LocalAppAgentNativeEvents.self, from: Data(#"{"events":[],"sequence":0}"#.utf8))
        XCTAssertEqual(present.nativeOutputBytes, 42)
        XCTAssertNil(absent.nativeOutputBytes)
    }

    func testNativeCounterSurvivesLocalAppJSONRoundTripAndRejectsUnsafeNumbers() throws {
        func decode(_ json: String) throws -> LocalAppAgentNativeEvents {
            let intermediate = try JSONDecoder().decode(LocalAppJSON.self, from: Data(json.utf8))
            return try JSONDecoder().decode(LocalAppAgentNativeEvents.self, from: JSONEncoder().encode(intermediate))
        }
        let maximum = try decode(#"{"events":[],"sequence":0,"nativeOutputBytes":9007199254740991}"#)
        XCTAssertEqual(maximum.nativeOutputBytes, LocalAppAgentNativeEvents.maximumSafeNativeOutputBytes)

        var output = LocalAppAgentOutputProgress()
        _ = try output.consume(.init(events: [textEvent("ok")], sequence: 1, nativeOutputBytes: 5), runID: "run-1")
        for bad in ["9007199254740992", "9007199254740993", "-1", "1.5", "\"not-a-number\""] {
            let json = "{\"events\":[{\"type\":\"TEXT_MESSAGE_CONTENT\",\"messageId\":\"m\",\"delta\":\"new\"}],\"sequence\":2,\"nativeOutputBytes\":\(bad)}"
            do {
                let page = try decode(json)
                _ = try output.consume(page, runID: "run-1")
                XCTFail("An unsafe native output counter must be rejected.")
            } catch let failure as LocalAppServiceFailure {
                XCTAssertEqual(failure.code, "INVALID_AGENT_EVENT")
            }
            XCTAssertEqual(output.sequence, 1)
            XCTAssertEqual(output.eventOutputBytes, 2)
            XCTAssertEqual(output.nativeOutputBytes, 5)
            XCTAssertEqual(output.outputBytes, 5)
        }
    }

    func testObservedBytesAreNotResetByTurnsAndOnlySuccessCompletesProgress() {
        let progress = Progress(totalUnitCount: 0)
        LocalAppAgentProgressReporting.update(progress, outputBytes: 11)
        LocalAppAgentProgressReporting.update(progress, outputBytes: 11, completedModelTurns: 20, turnLimit: 20)

        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.totalUnitCount, -1)
        XCTAssertEqual(progress.completedUnitCount, 11)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Observed 11 stream bytes; total unknown")
        LocalAppAgentProgressReporting.finish(progress, success: false)
        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.totalUnitCount, -1)
        XCTAssertEqual(progress.completedUnitCount, 11)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Observed 11 stream bytes; total unknown; task ended")

        LocalAppAgentProgressReporting.update(progress, outputBytes: 11)
        LocalAppAgentProgressReporting.finish(progress, success: true)
        XCTAssertEqual(progress.totalUnitCount, 11)
        XCTAssertEqual(progress.completedUnitCount, 11)
        XCTAssertEqual(progress.fractionCompleted, 1)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Observed 11 stream bytes; complete")
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

    func testTerminalReplayDrainsBoundedPagesForSuccessCancelAndError() async throws {
        let terminals: [LocalAppJSON] = [
            .object(["type": .string("RUN_FINISHED"), "runId": .string("run-1"), "threadId": .string("thread-1"),
                     "outcome": .object(["type": .string("success")])]),
            .object(["type": .string("RUN_FINISHED"), "runId": .string("run-1"), "threadId": .string("thread-1"),
                     "outcome": .object(["type": .string("cancelled")])]),
            .object(["type": .string("RUN_ERROR"), "message": .string("fixture failure")])
        ]
        for (index, terminal) in terminals.enumerated() {
            let all: [LocalAppJSON] = [.object(["type": .string("RUN_STARTED"), "runId": .string("run-1"),
                                               "threadId": .string("thread-1")])]
                + Array(repeating: textEvent("x"), count: 257) + [terminal]
            var progress = LocalAppAgentOutputProgress()
            var cursors: [UInt64] = []
            var observed: [Int64] = []
            let found = try await LocalAppAgentEventPageReader.readAvailable(
                progress: &progress, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1", threadID: "thread-1",
                fetch: { cursor in
                    cursors.append(cursor)
                    let events = Array(all.dropFirst(Int(cursor)).prefix(128))
                    return LocalAppAgentNativeEvents(events: events, sequence: cursor + UInt64(events.count))
                },
                onOutput: { observed.append($0) }
            )
            XCTAssertEqual(found, terminal)
            XCTAssertEqual(cursors, [0, 128, 256])
            XCTAssertEqual(progress.sequence, 259)
            XCTAssertEqual(progress.outputBytes, 257)
            XCTAssertEqual(observed, [127, 255, 257])
            XCTAssertEqual(LocalAppAgentRunEvent.isSuccessfulTerminal(found, runID: "run-1", threadID: "thread-1"),
                           index == 0)
        }
    }

    func testRunningPollReadsOnlyOnePageAndTerminalWithoutEventFails() async throws {
        var progress = LocalAppAgentOutputProgress()
        var calls = 0
        let running = try await LocalAppAgentEventPageReader.readAvailable(
            progress: &progress, taskIsTerminal: false, deadline: Date().addingTimeInterval(5), runID: "run-1",
            fetch: { cursor in
                calls += 1
                return LocalAppAgentNativeEvents(events: [self.textEvent("x")], sequence: cursor + 1)
            }, onOutput: { _ in }
        )
        XCTAssertNil(running)
        XCTAssertEqual(calls, 1)

        do {
            _ = try await LocalAppAgentEventPageReader.readAvailable(
                progress: &progress, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1",
                fetch: { cursor in LocalAppAgentNativeEvents(events: [], sequence: cursor) },
                onOutput: { _ in }
            )
            XCTFail("A terminal task requires its terminal event.")
        } catch let failure as LocalAppServiceFailure {
            XCTAssertEqual(failure.code, "AGENT_TERMINAL_EVENT_MISSING")
        }
        XCTAssertEqual(progress.outputBytes, 1)
    }

    func testTerminalReplayRejectsBadCursorAndPropagatesFetchErrors() async throws {
        var progress = LocalAppAgentOutputProgress()
        for failFetch in [false, true] {
            do {
                _ = try await LocalAppAgentEventPageReader.readAvailable(
                    progress: &progress, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1",
                    fetch: { cursor in
                        if failFetch { throw LocalAppServiceFailure("other_failure", "fixture") }
                        return LocalAppAgentNativeEvents(events: [self.textEvent("x")], sequence: cursor + 2)
                    }, onOutput: { _ in XCTFail("An invalid page must not be reported.") }
                )
                XCTFail("A bad page or failed fetch must not be accepted.")
            } catch let failure as LocalAppServiceFailure {
                XCTAssertEqual(failure.code, failFetch ? "other_failure" : "INVALID_AGENT_EVENT")
            }
            XCTAssertEqual(progress.sequence, 0)
            XCTAssertEqual(progress.outputBytes, 0)
        }
        _ = try progress.consume(.init(events: [textEvent("x")], sequence: 1), runID: "run-1")
        for sequence: UInt64 in [0, 10_001] {
            do {
                _ = try await LocalAppAgentEventPageReader.readAvailable(
                    progress: &progress, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1",
                    fetch: { _ in LocalAppAgentNativeEvents(events: [self.textEvent("bad")], sequence: sequence) },
                    onOutput: { _ in XCTFail("An invalid page must not be reported.") }
                )
                XCTFail("A regressing or oversized cursor must not be accepted.")
            } catch let failure as LocalAppServiceFailure {
                XCTAssertEqual(failure.code, "INVALID_AGENT_EVENT")
            }
            XCTAssertEqual(progress.sequence, 1)
            XCTAssertEqual(progress.outputBytes, 1)
        }
    }

    func testTerminalReplayStopsWhenCancelledBetweenPages() async throws {
        let task = Task { () throws -> Int in
            var progress = LocalAppAgentOutputProgress()
            var calls = 0
            do {
                _ = try await LocalAppAgentEventPageReader.readAvailable(
                    progress: &progress, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1",
                    fetch: { cursor in
                        calls += 1
                        return LocalAppAgentNativeEvents(events: [self.textEvent("x")], sequence: cursor + 1)
                    },
                    onOutput: { _ in withUnsafeCurrentTask { $0?.cancel() } }
                )
                XCTFail("Cancelled replay must stop before fetching another page.")
            } catch is CancellationError {
                return calls
            }
            return calls
        }
        let calls = try await task.value
        XCTAssertEqual(calls, 1)
    }

    func testTerminalPublicationRaceRetriesOnlyTheKnownStorageError() async throws {
        var progress = LocalAppAgentOutputProgress()
        var calls = 0
        let terminal: LocalAppJSON = .object([
            "type": .string("RUN_FINISHED"), "runId": .string("run-1"), "threadId": .string("thread-1"),
            "outcome": .object(["type": .string("success")])
        ])
        let found = try await LocalAppAgentEventPageReader.readAvailable(
            progress: &progress, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1",
            fetch: { cursor in
                calls += 1
                if calls == 1 { throw LocalAppServiceFailure("agent_events_unavailable", "fixture") }
                return .init(events: [terminal], sequence: cursor + 1)
            }, onOutput: { _ in }
        )
        XCTAssertEqual(found, terminal)
        XCTAssertEqual(calls, 2)
        XCTAssertEqual(progress.sequence, 1)

        for (code, expectedCode, terminalState, remainingTime, expectedCalls) in [
            ("agent_events_unavailable", "AGENT_TERMINAL_EVENTS_UNAVAILABLE", true, 5.0, 3),
            ("other_failure", "other_failure", true, 5.0, 1),
            ("agent_events_unavailable", "agent_events_unavailable", false, 5.0, 1)
        ] {
            var output = LocalAppAgentOutputProgress()
            var attempts = 0
            do {
                _ = try await LocalAppAgentEventPageReader.readAvailable(
                    progress: &output, taskIsTerminal: terminalState,
                    deadline: Date().addingTimeInterval(remainingTime), runID: "run-1",
                    fetch: { _ in attempts += 1; throw LocalAppServiceFailure(code, "fixture") },
                    onOutput: { _ in XCTFail("A failed fetch has no output.") }
                )
                XCTFail("A failed fetch must not be accepted.")
            } catch let failure as LocalAppServiceFailure {
                XCTAssertEqual(failure.code, expectedCode)
            }
            XCTAssertEqual(attempts, expectedCalls)
            XCTAssertEqual(output.sequence, 0)
        }
    }

    func testCancellationWinsOverRetryableTerminalFetchFailure() async throws {
        let task = Task { () -> Int in
            var output = LocalAppAgentOutputProgress()
            var calls = 0
            do {
                _ = try await LocalAppAgentEventPageReader.readAvailable(
                    progress: &output, taskIsTerminal: true, deadline: Date().addingTimeInterval(5), runID: "run-1",
                    fetch: { _ in
                        calls += 1
                        withUnsafeCurrentTask { $0?.cancel() }
                        throw LocalAppServiceFailure("agent_events_unavailable", "fixture")
                    }, onOutput: { _ in XCTFail("A cancelled fetch has no output.") }
                )
                XCTFail("Cancellation must stop the retry.")
            } catch is CancellationError {
                return calls
            } catch {
                XCTFail("Expected cancellation rather than a fetch failure.")
            }
            return calls
        }
        let calls = await task.value
        XCTAssertEqual(calls, 1)
    }

    func testDeadlinePreventsFetchAndStopsBetweenSuccessfulPages() async throws {
        let start = Date(timeIntervalSince1970: 1_000)
        for terminal in [false, true] {
            var output = LocalAppAgentOutputProgress()
            var calls = 0
            do {
                _ = try await LocalAppAgentEventPageReader.readAvailable(
                    progress: &output, taskIsTerminal: terminal, deadline: start, runID: "run-1",
                    fetch: { cursor in
                        calls += 1
                        return .init(events: [self.textEvent("x")], sequence: cursor + 1)
                    }, onOutput: { _ in }, now: { start }
                )
                XCTFail("An expired reader must not fetch.")
            } catch LocalAppAgentEventPageError.deadline {}
            XCTAssertEqual(calls, 0)
            XCTAssertEqual(output.sequence, 0)
        }

        let deadline = start.addingTimeInterval(1)
        var now = start
        var output = LocalAppAgentOutputProgress()
        var calls = 0
        do {
            _ = try await LocalAppAgentEventPageReader.readAvailable(
                progress: &output, taskIsTerminal: true, deadline: deadline, runID: "run-1",
                fetch: { cursor in
                    calls += 1
                    return .init(events: [self.textEvent("x")], sequence: cursor + 1)
                }, onOutput: { _ in now = deadline }, now: { now }
            )
            XCTFail("Replay must stop when its deadline passes between pages.")
        } catch LocalAppAgentEventPageError.deadline {}
        XCTAssertEqual(calls, 1)
        XCTAssertEqual(output.sequence, 1)
        XCTAssertEqual(output.outputBytes, 1)
    }

    private func textEvent(_ delta: String) -> LocalAppJSON {
        .object(["type": .string("TEXT_MESSAGE_CONTENT"), "messageId": .string("m"), "delta": .string(delta)])
    }
}

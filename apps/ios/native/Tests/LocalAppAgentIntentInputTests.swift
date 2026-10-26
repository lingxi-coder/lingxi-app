import Foundation
import XCTest
@testable import LingxiCode

final class LocalAppAgentIntentInputTests: XCTestCase {
    func testIntentAcceptsCompleteRunAgentInputWithoutRewritingItsMessages() throws {
        let text = #"""
        {
          "threadId": "thread-1",
          "runId": "run-1",
          "messages": [
            {
              "id": "message-1",
              "role": "user",
              "content": [
                {"type": "text", "text": "Describe this image."},
                {"type": "image", "source": {"type": "data", "value": "aGVsbG8=", "mimeType": "image/png"}}
              ]
            }
          ],
          "forwardedProps": {"com.lingxi.localApp": {"version": 1, "tools": []}}
        }
        """#

        let input = try LocalAppAgentIntentInput.runAgentInput(text)

        XCTAssertEqual(input.threadID, "thread-1")
        XCTAssertEqual(input.runID, "run-1")
        XCTAssertEqual(input.value, try JSONDecoder().decode(LocalAppJSON.self, from: Data(text.utf8)))
    }

    func testIntentRejectsNonObjectOrMissingRunCorrelationFields() {
        for text in [
            #"[]"#,
            #"{"runId":"run-1","messages":[]}"#,
            #"{"threadId":"thread-1","messages":[]}"#
        ] {
            XCTAssertThrowsError(try LocalAppAgentIntentInput.runAgentInput(text), text)
        }
    }

    func testIntentBoundsInputBeforeDecoding() {
        XCTAssertTrue(LocalAppAgentIntentInput.isWithinInputLimit(LocalAppAgentIntentInput.maximumInputBytes))
        XCTAssertFalse(LocalAppAgentIntentInput.isWithinInputLimit(LocalAppAgentIntentInput.maximumInputBytes + 1))
    }

    func testOnlySuccessfulRunFinishedEventMarksIntentProgressSuccessful() {
        let successWithoutOutcome: LocalAppJSON = .object([
            "type": .string("RUN_FINISHED"),
            "threadId": .string("thread-1"),
            "runId": .string("run-1")
        ])
        let successWithOutcome: LocalAppJSON = .object([
            "type": .string("RUN_FINISHED"),
            "threadId": .string("thread-1"),
            "runId": .string("run-1"),
            "outcome": .object(["type": .string("success")])
        ])
        let cancelled: LocalAppJSON = .object([
            "type": .string("RUN_FINISHED"),
            "threadId": .string("thread-1"),
            "runId": .string("run-1"),
            "outcome": .object(["type": .string("cancelled")])
        ])
        let interrupted: LocalAppJSON = .object([
            "type": .string("RUN_FINISHED"),
            "threadId": .string("thread-1"),
            "runId": .string("run-1"),
            "outcome": .object(["type": .string("interrupt"), "interrupts": .array([.object(["id": .string("i1")])])])
        ])
        let failed: LocalAppJSON = .object(["type": .string("RUN_ERROR"), "message": .string("failed")])
        let started: LocalAppJSON = .object([
            "type": .string("RUN_STARTED"),
            "threadId": .string("thread-1"),
            "runId": .string("run-1")
        ])
        let nonTerminal: LocalAppJSON = .object(["type": .string("TEXT_MESSAGE_CONTENT"), "delta": .string("partial")])

        XCTAssertTrue(LocalAppAgentRunEvent.isSuccessful(successWithoutOutcome))
        XCTAssertTrue(LocalAppAgentRunEvent.isSuccessful(successWithOutcome))
        XCTAssertTrue(LocalAppAgentRunEvent.isSuccessfulTerminal(successWithOutcome, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessful(cancelled))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessful(interrupted))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessful(failed))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessfulTerminal(nil, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessfulTerminal(cancelled, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessfulTerminal(interrupted, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessfulTerminal(failed, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.isTerminal(nonTerminal))
        XCTAssertFalse(LocalAppAgentRunEvent.isSuccessful(nonTerminal))
        XCTAssertNil(LocalAppAgentRunEvent.terminalEvent(in: []))
        XCTAssertNil(LocalAppAgentRunEvent.terminalEvent(in: [started, nonTerminal]))
        XCTAssertEqual(LocalAppAgentRunEvent.terminalEvent(in: [started, cancelled]), cancelled)
        XCTAssertTrue(LocalAppAgentRunEvent.matches(successWithOutcome, runID: "run-1", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.matches(successWithOutcome, runID: "other-run", threadID: "thread-1"))
        XCTAssertFalse(LocalAppAgentRunEvent.matches(successWithOutcome, runID: "run-1", threadID: "other-thread"))
    }
}

private actor LocalAppAgentHostRequestRecorder {
    private var recorded: [[String: LocalAppJSON]] = []

    func append(_ request: [String: LocalAppJSON]) {
        recorded.append(request)
    }

    func requests() -> [[String: LocalAppJSON]] {
        recorded
    }
}

final class LocalAppAgentIntentCancellationTests: XCTestCase {
    func testIntentCancellationSendsThePublicRequestIdSelector() async throws {
        let recorder = LocalAppAgentHostRequestRecorder()
        let publicRequestID = "public-request-23"
        let internalTaskID = "internal-task-uuid-91"
        let cancellation = LocalAppAgentIntentCancellation(
            appID: "app-7",
            revision: 19,
            sendHostRequest: { await recorder.append($0) }
        )

        try await cancellation.didStart(publicRequestID)
        await cancellation.cancel()

        let requests = await recorder.requests()
        XCTAssertNotEqual(publicRequestID, internalTaskID)
        XCTAssertEqual(requests, [[
            "op": .string("tasks.cancel"),
            "appId": .string("app-7"),
            "revision": .number(19),
            "requestId": .string(publicRequestID)
        ]])
        XCTAssertNil(requests.first?["runId"])
    }

    func testCancellationBeforeStartUsesTheSamePublicRequestIdSelector() async {
        let recorder = LocalAppAgentHostRequestRecorder()
        let publicRequestID = "public-request-23"
        let cancellation = LocalAppAgentIntentCancellation(
            appID: "app-7",
            revision: 19,
            sendHostRequest: { await recorder.append($0) }
        )

        await cancellation.cancel()
        do {
            try await cancellation.didStart(publicRequestID)
            XCTFail("A start arriving after cancellation must remain cancelled.")
        } catch is CancellationError {
        } catch {
            XCTFail("Unexpected error: \(error)")
        }

        let requests = await recorder.requests()
        XCTAssertEqual(requests, [[
            "op": .string("tasks.cancel"),
            "appId": .string("app-7"),
            "revision": .number(19),
            "requestId": .string(publicRequestID)
        ]])
        XCTAssertNil(requests.first?["runId"])
    }
}

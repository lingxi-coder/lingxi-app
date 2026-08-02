// SessionResumeTests.swift — live ResumeSession (iOS).
//
// Unit coverage for the inbound `SessionResumed` mapping the engine now carries
// the restored transcript on (`SessionResumed { session_id, messages }`). The
// iOS analog of Android's `SessionStateTest` mapper test: it drives a synthetic
// `ClientEvent.sessionResumed` through `EngineConversationSource.apply` (via the
// `applyForTesting` seam) and asserts the out-of-band session-state path both
// adopts the session id AND surfaces the restored conversation — so the chat
// scrollback shows the prior turns the next message will continue from.
//
// No engine, no key, no network: this constructs the lowered DTOs directly and
// exercises only the Swift mapping, so it is hermetic and fast.

import Combine
import XCTest

@testable import LingxiCode

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

#if canImport(engine_mobileFFI)

    @MainActor
    final class SessionResumeTests: XCTestCase {

        /// A minimal `EngineConfig` for an `EngineConversationSource` under test —
        /// keyless and rooted at a throwaway temp dir. The source is NEVER asked to
        /// build a handle here; we only drive `applyForTesting`, so no engine is
        /// spun up and no network/key is touched.
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory())
            return EngineConversationSource(config: config)
        }

        private func uuid() -> String { "22222222-2222-4222-8222-222222222222" }

        /// An empty array before the first `SessionList` means "not loaded";
        /// the same array after the event is an authoritative empty catalog.
        /// Persisting code relies on this distinction to avoid erasing a cached
        /// project index when engine startup or listing has not completed yet.
        func testSessionListMarksAuthoritativeEmptyCatalogAsLoaded() {
            let source = makeSource()
            XCTAssertTrue(source.model.engineSessions.isEmpty)
            XCTAssertFalse(source.model.engineSessionsLoaded)

            source.applyForTesting(.sessionList(sessions: []))

            XCTAssertTrue(source.model.engineSessions.isEmpty)
            XCTAssertTrue(source.model.engineSessionsLoaded)
        }

        func testSessionIndexPreservesPendingRestoreAndUnlistedActiveSession() {
            XCTAssertFalse(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: true,
                    activeSessionID: "old-session",
                    listedSessionIDs: []
                )
            )
            XCTAssertFalse(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: false,
                    activeSessionID: "new-session",
                    listedSessionIDs: ["old-session"]
                )
            )
            XCTAssertFalse(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: false,
                    restorePending: true,
                    activeSessionID: "saved-session",
                    listedSessionIDs: []
                )
            )
            XCTAssertTrue(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: false,
                    activeSessionID: "saved-session",
                    listedSessionIDs: ["saved-session"]
                )
            )
        }

        func testPendingRestoreRejectsTransientStartupSessionUntilTargetArrives() {
            XCTAssertFalse(
                ConversationSessionRestorePolicy.shouldAdopt(
                    candidateSessionID: "startup-session",
                    pendingRestoreID: "saved-session"
                )
            )
            XCTAssertTrue(
                ConversationSessionRestorePolicy.shouldAdopt(
                    candidateSessionID: "saved-session",
                    pendingRestoreID: "saved-session"
                )
            )
            XCTAssertTrue(
                ConversationSessionRestorePolicy.shouldAdopt(
                    candidateSessionID: "fresh-session",
                    pendingRestoreID: nil
                )
            )
            XCTAssertTrue(
                ConversationSessionRestorePolicy.shouldClearUnavailableSession(
                    unavailableSessionID: "saved-session",
                    pendingRestoreID: "saved-session",
                    activeSessionID: "saved-session"
                )
            )
            XCTAssertFalse(
                ConversationSessionRestorePolicy.shouldClearUnavailableSession(
                    unavailableSessionID: "old-session",
                    pendingRestoreID: "new-session",
                    activeSessionID: "new-session"
                )
            )
            XCTAssertEqual(
                ConversationSessionRestorePolicy.rollbackSelection(
                    failedSessionID: "new-session",
                    pendingRestoreID: "new-session",
                    activeSessionID: "new-session",
                    confirmedSessionID: "previous-session"
                ),
                "previous-session"
            )
            XCTAssertNil(
                ConversationSessionRestorePolicy.rollbackSelection(
                    failedSessionID: "stale-failure",
                    pendingRestoreID: "new-session",
                    activeSessionID: "new-session",
                    confirmedSessionID: "previous-session"
                )
            )
        }

        func testMissingResumeCreatesReplacementWithoutShowingEngineError() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .missing)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("missing-session")
            let receivedCommands = await recorder.waitForCommandCount(2)
            XCTAssertTrue(receivedCommands)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume, .new])
            XCTAssertEqual(
                source.model.sessionRestoreRecovery?.unavailableSessionID,
                "missing-session"
            )
            XCTAssertTrue(source.model.sessionTransitionPending)
            XCTAssertTrue(source.model.isNew)
            XCTAssertNil(source.model.error)
            XCTAssertEqual(source.model.statusLine, "原会话已不存在，已创建新对话")

            source.applyForTesting(.sessionStarted(sessionId: "replacement-session"))
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.activeSessionId, "replacement-session")
            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
        }

        func testNonMissingResumeFailureRemainsVisible() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .generic)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("saved-session")
            let receivedCommand = await recorder.waitForCommandCount(1)
            XCTAssertTrue(receivedCommand)
            let transitionSettled = await waitForSessionTransitionToSettle(source)
            XCTAssertTrue(transitionSettled)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume])
            XCTAssertNil(source.model.sessionRestoreRecovery)
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertNotNil(source.model.error)
            XCTAssertEqual(
                source.model.sessionTransitionFailure?.requestedSessionID,
                "saved-session"
            )
        }

        func testRejectedMissingResumeUsesTypedFallback() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .missingRejected)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("missing-session")
            let receivedCommands = await recorder.waitForCommandCount(2)
            XCTAssertTrue(receivedCommands)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume, .new])
            XCTAssertNil(source.model.error)
        }

        func testConfirmedEmptyResumePreservesSessionIDThroughDedicatedEntryPoint() async {
            let source = makeSource()
            let recorder = EmptySessionResumeRecorder()
            source.setCommandSubmitterForTesting { _ in
                XCTFail("a confirmed empty session must not use ResumeSession")
            }
            source.setEmptySessionResumerForTesting { sessionID, title in
                await recorder.resume(sessionID: sessionID, title: title)
            }

            source.resumeSession("empty-session", emptySessionTitle: "空会话")
            let didResume = await recorder.waitForResume()
            XCTAssertTrue(didResume)

            let request = await recorder.snapshot()
            XCTAssertEqual(
                request,
                .init(sessionID: "empty-session", title: "空会话")
            )
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionResumed(sessionId: "empty-session", messages: []))
            XCTAssertEqual(source.model.activeSessionId, "empty-session")
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
        }

        func testBootstrapSessionStartedDoesNotClearPendingResume() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }

            source.resumeSession("saved-session")
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionStarted(sessionId: "startup-session"))
            XCTAssertTrue(
                source.model.sessionTransitionPending,
                "a delayed bootstrap SessionStarted is not ResumeSession confirmation"
            )

            source.applyForTesting(.sessionResumed(sessionId: "saved-session", messages: []))
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.activeSessionId, "saved-session")
        }

        /// The loaded transition is the persistence trigger. It must never be
        /// observable before the rows carried by the same engine event, or a
        /// crash between the two publications can durably erase the old index.
        func testSessionListPublishesRowsBeforeLoadedTransition() {
            let source = makeSource()
            var sessionIDsObservedWhenLoaded: [String] = []
            let observation = source.model.$engineSessionsLoaded
                .dropFirst()
                .sink { loaded in
                    if loaded {
                        sessionIDsObservedWhenLoaded = source.model.engineSessions.map(\.id)
                    }
                }
            defer { observation.cancel() }

            source.applyForTesting(.sessionList(sessions: [
                SessionRowDto(
                    uuid: uuid(),
                    title: "保留的会话",
                    modifiedRfc3339: "2026-08-02T08:00:00Z",
                    messageCount: 3,
                    path: "/tmp/retained.jsonl"
                ),
            ]))

            XCTAssertEqual(sessionIDsObservedWhenLoaded, [uuid()])
        }

        /// SessionResumed with a 2-message transcript (a user text turn + an
        /// assistant text turn, oldest-first) must: adopt the session id, replace
        /// the transcript with exactly those messages in order, map roles to the
        /// UI user/AI split, and leave the turn idle (not streaming). This proves
        /// the restored scrollback the next turn continues from is visible.
        func testSessionResumedSurfacesRestoredTranscriptOldestFirst() {
            let source = makeSource()
            // Seed a stale placeholder transcript (what `resumeSession` leaves in
            // place before the engine confirms) so we can prove it is replaced.
            source.model.messages = [Message(role: .ai, text: "placeholder")]
            source.model.streaming = true

            let messages: [MessageDto] = [
                MessageDto(role: "user", blocks: [.text(text: "第一条用户消息")]),
                MessageDto(role: "assistant", blocks: [.text(text: "助手的回复")]),
            ]
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: messages))

            XCTAssertEqual(source.model.activeSessionId, uuid(),
                           "SessionResumed must adopt the resumed session id")
            XCTAssertEqual(source.model.messages.count, 2,
                           "the restored transcript must replace the placeholder, one bubble per restored message")
            // Oldest-first order + role split preserved.
            XCTAssertEqual(source.model.messages[0].role, .user)
            XCTAssertEqual(source.model.messages[0].text, "第一条用户消息")
            XCTAssertEqual(source.model.messages[1].role, .ai)
            XCTAssertEqual(source.model.messages[1].text, "助手的回复")
            XCTAssertFalse(source.model.streaming,
                           "a resumed session is idle until the user sends the next turn")
            XCTAssertFalse(source.model.isNew,
                           "a non-empty restored transcript is not the empty-state")
        }

        /// An assistant message carrying multiple block kinds (text + thinking +
        /// tool_use + tool_result) must flatten into the single display string the
        /// iOS `Message` model carries — text/thinking bodies plus a labeled line
        /// for the tool blocks (the conversation surface has no tool cards yet), so
        /// no restored content silently vanishes.
        func testSessionResumedFlattensRichAssistantBlocks() {
            let source = makeSource()
            let assistant = MessageDto(role: "assistant", blocks: [
                .text(text: "正文"),
                .thinking(thinking: "推理", signature: nil),
                .compactBoundary(messagesBefore: 8, messagesAfter: 2,
                                 summary: "hidden compact summary"),
                .toolUse(id: "t1", tool: "Read", inputJson: "{\"path\":\"a\"}"),
                .toolResult(id: "t1", tool: "", resultJson: "\"ok\"",
                            isError: false, oldString: nil, newString: nil, filePath: nil),
            ])
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: [assistant]))

            XCTAssertEqual(source.model.messages.count, 1)
            let text = source.model.messages[0].text
            XCTAssertTrue(text.contains("正文"), "text block body must be present")
            XCTAssertTrue(text.contains("推理"), "thinking block body must be present")
            XCTAssertTrue(text.contains("对话已压缩"),
                          "compact boundary must remain visible after resume")
            XCTAssertFalse(text.contains("hidden compact summary"),
                           "internal compact summary must not be rendered as user text")
            XCTAssertTrue(text.contains("调用工具 Read"),
                          "a tool_use block must surface a labeled line, not vanish")
            XCTAssertEqual(source.model.messages[0].role, .ai,
                           "assistant role maps to the AI side")
        }

        /// A zero-message resume (a session with no transcript) must still adopt
        /// the id, clear the placeholder, and fall into the empty-state — never
        /// leave a stale placeholder bubble behind.
        func testSessionResumedEmptyTranscriptClearsToEmptyState() {
            let source = makeSource()
            source.model.messages = [Message(role: .ai, text: "placeholder")]

            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: []))

            XCTAssertEqual(source.model.activeSessionId, uuid())
            XCTAssertTrue(source.model.messages.isEmpty,
                          "an empty restored transcript clears the placeholder")
            XCTAssertTrue(source.model.isNew,
                          "a zero-message resume lands in the empty-state")
        }

        /// A `system` role (the engine emits it for system messages) renders on
        /// the AI side (the iOS `Message.role` is the binary user/AI split), never
        /// dropped.
        func testSessionResumedSystemRoleRendersAsAi() {
            let source = makeSource()
            let system = MessageDto(role: "system", blocks: [.text(text: "系统提示")])
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: [system]))

            XCTAssertEqual(source.model.messages.count, 1)
            XCTAssertEqual(source.model.messages[0].role, .ai)
            XCTAssertEqual(source.model.messages[0].text, "系统提示")
        }

        func testConcurrentEngineEntryPointsShareOneHandleBuild() async throws {
            let gate = EngineSubmitGate()
            let handle = TestMobileEngineHandle { command in
                try await gate.submit(command)
            }
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _ in
                buildCount += 1
                return handle
            })

            source.warmUp()
            source.listSessions()
            source.send("并发消息")
            let prepare = Task { try await source.prepare() }

            await gate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 1)
            await gate.releaseFirstSubmit()
            try await prepare.value
            let receivedCommands = await gate.waitForSubmitCount(4)
            XCTAssertTrue(receivedCommands)

            XCTAssertEqual(buildCount, 1,
                           "warm-up, prepare, list and send must await the same engine build")
            let commands = await gate.snapshot()
            var sessionListCommandCount = 0
            for command in commands {
                guard case let .listSessions(limit) = command else { continue }
                sessionListCommandCount += 1
                XCTAssertEqual(limit, UInt32.max,
                               "bootstrap and drawer refresh must both request the full catalog")
            }
            XCTAssertGreaterThan(sessionListCommandCount, 0)
        }

        func testBootstrapRequestsTheCompleteSessionCatalog() async throws {
            let gate = EngineSubmitGate()
            let handle = TestMobileEngineHandle { command in
                try await gate.submit(command)
            }
            let source = makeSource(handleBuilder: { _, _, _ in handle })

            let prepare = Task { try await source.prepare() }
            await gate.waitForFirstSubmit()
            await gate.releaseFirstSubmit()
            try await prepare.value
            let receivedCommands = await gate.waitForSubmitCount(2)
            XCTAssertTrue(receivedCommands)

            let commands = await gate.snapshot()
            guard commands.count >= 2 else { return }
            guard case let .listSessions(limit) = commands[1] else {
                return XCTFail("engine bootstrap must request the session catalog")
            }
            XCTAssertEqual(limit, UInt32.max,
                           "project persistence requires an uncapped catalog")
        }

        func testFailedSharedEngineBuildCanRetryWithoutCachingPartialHandle() async throws {
            let failingGate = EngineSubmitGate(firstSubmitError: EngineBuildTestError.bootstrapFailed)
            let succeedingGate = EngineSubmitGate()
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _ in
                buildCount += 1
                if buildCount == 1 {
                    return TestMobileEngineHandle { command in
                        try await failingGate.submit(command)
                    }
                }
                return TestMobileEngineHandle { command in
                    try await succeedingGate.submit(command)
                }
            })

            let first = Task { try await source.prepare() }
            let concurrent = Task { try await source.prepare() }
            await failingGate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 1, "concurrent prepare calls must share the failing attempt")
            await failingGate.releaseFirstSubmit()

            await XCTAssertThrowsAsyncError(try await first.value)
            await XCTAssertThrowsAsyncError(try await concurrent.value)

            let retry = Task { try await source.prepare() }
            await succeedingGate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 2, "a failed bootstrap must not leave a cached handle")
            await succeedingGate.releaseFirstSubmit()
            try await retry.value
            XCTAssertEqual(buildCount, 2)
        }

        private func makeSource(
            handleBuilder: @escaping EngineConversationSource.HandleBuilder
        ) -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory())
            return EngineConversationSource(config: config, handleBuilder: handleBuilder)
        }

        private func XCTAssertThrowsAsyncError<T>(
            _ expression: @autoclosure () async throws -> T,
            file: StaticString = #filePath,
            line: UInt = #line
        ) async {
            do {
                _ = try await expression()
                XCTFail("expected async operation to throw", file: file, line: line)
            } catch {
                // Expected.
            }
        }

        private func waitForSessionTransitionToSettle(
            _ source: EngineConversationSource
        ) async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while source.model.sessionTransitionPending, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return !source.model.sessionTransitionPending
        }
    }

    private enum EngineBuildTestError: Error {
        case bootstrapFailed
    }

    private enum SessionTransitionTestError: Error {
        case missing
        case missingRejected
        case generic
    }

    private actor SessionTransitionRecorder {
        enum Command: Equatable {
            case resume
            case new
            case other
        }

        private let failure: SessionTransitionTestError
        private var commands: [Command] = []

        init(failure: SessionTransitionTestError) {
            self.failure = failure
        }

        func submit(_ command: ClientCommand) throws {
            switch command {
            case .resumeSession:
                commands.append(.resume)
                switch failure {
                case .missing:
                    throw ClientError.NotFound(message: "Session missing-session was not found.")
                case .missingRejected:
                    throw ClientError.Rejected(
                        message: "resume: session missing-session not resumable: Session missing-session was not found."
                    )
                case .generic:
                    throw ClientError.Transport(message: "resume transport rejected")
                }
            case .newSession:
                commands.append(.new)
            default:
                commands.append(.other)
            }
        }

        func waitForCommandCount(_ count: Int) async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while commands.count < count, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return commands.count >= count
        }

        func snapshot() -> [Command] {
            commands
        }
    }

    private actor EmptySessionResumeRecorder {
        struct Request: Equatable {
            let sessionID: String
            let title: String
        }

        private var request: Request?

        func resume(sessionID: String, title: String) {
            request = Request(sessionID: sessionID, title: title)
        }

        func waitForResume() async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while request == nil, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return request != nil
        }

        func snapshot() -> Request? {
            request
        }
    }

    private final class TestMobileEngineHandle: MobileEngineHandle {
        private let submitHandler: (ClientCommand) async throws -> Void

        required init(unsafeFromRawPointer pointer: UnsafeMutableRawPointer) {
            submitHandler = { _ in }
            super.init(unsafeFromRawPointer: pointer)
        }

        init(submitHandler: @escaping (ClientCommand) async throws -> Void) {
            self.submitHandler = submitHandler
            super.init(noPointer: .init())
        }

        override func submit(command: ClientCommand) async throws {
            try await submitHandler(command)
        }
    }

    private actor EngineSubmitGate {
        private let firstSubmitError: Error?
        private var commands: [ClientCommand] = []
        private var firstSubmitContinuation: CheckedContinuation<Void, Never>?

        init(firstSubmitError: Error? = nil) {
            self.firstSubmitError = firstSubmitError
        }

        func submit(_ command: ClientCommand) async throws {
            commands.append(command)
            if commands.count == 1 {
                await withCheckedContinuation { continuation in
                    firstSubmitContinuation = continuation
                }
                if let firstSubmitError {
                    throw firstSubmitError
                }
            }
        }

        func waitForFirstSubmit() async {
            _ = await waitForSubmitCount(1)
        }

        func waitForSubmitCount(_ expectedCount: Int) async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while commands.count < expectedCount, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return commands.count >= expectedCount
        }

        func releaseFirstSubmit() {
            firstSubmitContinuation?.resume()
            firstSubmitContinuation = nil
        }

        func snapshot() -> [ClientCommand] {
            commands
        }
    }

#endif

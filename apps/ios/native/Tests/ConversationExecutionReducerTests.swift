import XCTest
import SwiftUI

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)

    @MainActor
    final class ConversationExecutionReducerTests: XCTestCase {
        private let zeroCost = CostDto(
            totalUsd: 0,
            inputTokens: 0,
            outputTokens: 0,
            apiCalls: 0,
            sessionDurationSecs: 0,
            formatted: "$0.00"
        )

        private func makeSource(
            permissionModeRepository: PermissionModeConfigurationRepository? = nil,
            appSandboxRoot: String = NSTemporaryDirectory()
        ) -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: appSandboxRoot,
                projectCwd: nil,
                sessionMode: .code,
                visionDelegationEnabled: true)
            return EngineConversationSource(
                config: config,
                permissionModeRepository: permissionModeRepository
            )
        }

        private func makeSandboxRoot() throws -> String {
            let root = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
                .appendingPathComponent(UUID().uuidString, isDirectory: true)
            try FileManager.default.createDirectory(
                at: root,
                withIntermediateDirectories: true
            )
            return root.path
        }

        func testRestoredLoopWakeupFoldsQuietTranscript() {
            let source = makeSource()
            source.expectSessionResumeForTesting("loop-restored")
            source.applyForTesting(.sessionResumed(sessionId: "loop-restored", mode: .code, messages: [
                MessageDto(role: "system", blocks: [], loopWakeup: LoopWakeupDto(message: "first", companion: nil, streak: 0, sinceMs: 0)),
                MessageDto(role: "assistant", blocks: [.text(text: "quiet")]),
                MessageDto(role: "system", blocks: [], loopWakeup: LoopWakeupDto(message: "second", companion: "healthy", streak: 1, sinceMs: 1)),
            ]))
            let visible = source.model.selectedAgentItems.compactMap { item -> String? in
                if case let .message(message) = item { return message.text }
                return nil
            }
            XCTAssertEqual(visible, ["second", "healthy"])
            XCTAssertTrue(source.model.messages.contains { $0.text == "quiet" })
        }

        func testFixedScheduleNoticeArrivesBeforeTurnWithoutCreatingLoopFoldMarker() {
            let source = makeSource()
            source.applyForTesting(.scheduledTaskFire(message: "Fixed task is ready"))
            XCTAssertEqual(source.model.messages.last?.text, "Fixed task is ready")
            XCTAssertNil(source.model.messages.last?.loopWakeupStreak)
            XCTAssertTrue(source.model.messages.last?.loopFoldedItemIDs.isEmpty ?? false)
            XCTAssertFalse(source.model.streaming)
            source.applyForTesting(.systemNotice(message: "Scheduled task text cannot bypass turn gating", isError: false))
            XCTAssertEqual(source.model.messages.count, 1)
            XCTAssertEqual(source.model.messages.last?.text, "Fixed task is ready")
        }

        func testLoopWakeupFoldsQuietTranscriptBeforeNextTurnStarts() {
            let source = makeSource()
            source.applyForTesting(.loopWakeup(message: "first", companion: nil, streak: 0, sinceMs: 0))
            XCTAssertEqual(source.model.messages.last?.text, "first")
            source.applyForTesting(.loopWakeup(message: "second", companion: "healthy", streak: 1, sinceMs: 1))
            XCTAssertEqual(source.model.selectedAgentItems.count, 2)
            source.applyForTesting(.loopWakeup(message: "third", companion: "still healthy", streak: 2, sinceMs: 1))
            XCTAssertEqual(source.model.selectedAgentItems.count, 2)
            XCTAssertEqual(source.model.messages.count, 5, "Folding preserves transcript data")
            source.applyForTesting(.loopWakeup(message: "actionable", companion: nil, streak: 0, sinceMs: 0))
            XCTAssertEqual(source.model.selectedAgentItems.count, 3)
        }

        func testRetractionUsesExactIdentityAndPreservesLaterResponseAndNotice() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 901, sessionId: "retraction")
            source.applyForTesting(.turnStarted(turnId: 901))
            source.applyForTesting(.thinkingDelta(thinking: "failed reasoning", signature: nil))
            source.applyForTesting(.textDelta(text: "failed"))
            source.applyForTesting(.messageIdentity(messageId: "failed-id"))
            source.applyForTesting(.messageComplete(stopReason: "max_tokens", message: MessageDto(
                role: "assistant", blocks: [.text(text: "failed")]
            )))
            source.applyForTesting(.systemNotice(message: "unrelated notice", isError: false))
            source.applyForTesting(.thinkingDelta(thinking: "retained reasoning", signature: nil))
            source.applyForTesting(.textDelta(text: "retained"))
            source.applyForTesting(.messageIdentity(messageId: "retained-id"))
            source.applyForTesting(.messageComplete(stopReason: "end_turn", message: MessageDto(
                role: "assistant", blocks: [.text(text: "retained")]
            )))
            source.applyForTesting(.messageRetracted(messageId: "unknown"))
            XCTAssertTrue(source.model.messages.contains { $0.text == "failed" })
            source.applyForTesting(.messageRetracted(messageId: "failed-id"))
            XCTAssertFalse(source.model.messages.contains { $0.text == "failed" })
            XCTAssertTrue(source.model.messages.contains { $0.text == "retained" })
            let runs = source.model.items.compactMap { item -> ConversationExecutionRun? in
                if case let .run(run) = item { return run }
                return nil
            }
            XCTAssertEqual(runs.map(\.reasoning).joined(), "retained reasoning")
            XCTAssertTrue(runs.flatMap(\.notices).contains { $0.text == "unrelated notice" })
            source.applyForTesting(.messageRetracted(messageId: "failed-id"))
            XCTAssertTrue(source.model.messages.contains { $0.text == "retained" })
        }

        func testBypassPermissionsIsSelectableBeforeRiskConfirmation() {
            let source = makeSource()
            let bypass = source.model.permissionOptions.first { $0.id == "bypassPermissions" }

            XCTAssertEqual(bypass?.available, true)
            XCTAssertNil(bypass?.disabledReason)
        }

        func testSuppressedPermissionRequestPropagatesToPendingModel() {
            let request = PermissionRequest(
                requestId: 123,
                kind: .toolUseConfirm(
                    toolName: "Bash",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: nil,
                owner: nil,
                suppressAlwaysAllowRule: true,
                autoModePrompt: nil
            )

            let pending = PendingPermission(request: request)

            XCTAssertTrue(pending.suppressAlwaysAllowRule)
        }

        func testAutoModePromptPropagatesToPendingModel() {
            let request = PermissionRequest(
                requestId: 124,
                kind: .exitPlanMode(plan: "step 1"),
                worker: nil,
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: .exitPlanMode
            )

            let pending = PendingPermission(request: request)

            XCTAssertEqual(pending.autoModePrompt, .exitPlanMode)
        }

        func testBypassWarningIsSkippedOnlyAfterUserSuppressesIt() {
            XCTAssertTrue(ConversationControlsSheet.requiresRiskConfirmation(
                for: "bypassPermissions",
                bypassWarningSuppressed: false
            ))
            XCTAssertFalse(ConversationControlsSheet.requiresRiskConfirmation(
                for: "bypassPermissions",
                bypassWarningSuppressed: true
            ))
            XCTAssertTrue(ConversationControlsSheet.requiresRiskConfirmation(
                for: "dontAsk",
                bypassWarningSuppressed: true
            ))
        }

        func testBypassWarningPreferencePersists() throws {
            let suiteName = "PermissionModeRepositoryTests.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer {
                defaults.removePersistentDomain(forName: suiteName)
            }
            let repository = PermissionModeConfigurationRepository(defaults: defaults)
            repository.setBypassWarningSuppressed(true)

            XCTAssertTrue(PermissionModeConfigurationRepository(defaults: defaults)
                .bypassWarningSuppressed())
        }

        func testConfirmingBypassSendsTheSessionModeCommand() async throws {
            let suiteName = "BypassSessionTests.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer {
                defaults.removePersistentDomain(forName: suiteName)
            }
            let repository = PermissionModeConfigurationRepository(defaults: defaults)
            let source = makeSource(permissionModeRepository: repository)
            var confirmations = 0
            var submitted: [ClientCommand] = []
            source.setBypassPermissionsConfirmerForTesting { confirmations += 1 }
            source.setCommandSubmitterForTesting { command in submitted.append(command) }

            source.confirmAndSetBypassPermissions(suppressWarning: true)
            await waitForSubmittedCommands(1, commands: submitted)

            XCTAssertEqual(confirmations, 1)
            guard case .setPermissionMode(mode: "bypassPermissions")? = submitted.first else {
                return XCTFail("expected a bypass permission-mode command")
            }
            XCTAssertTrue(repository.bypassWarningSuppressed())
            XCTAssertFalse(source.model.controlsPending)
        }

        func testReportedModelsDoNotOverwriteLegacyLaunchFallback() {
            let previous = Keychain.get(.model)
            defer {
                if let previous { Keychain.set(.model, previous) }
                else { Keychain.clear(.model) }
            }
            XCTAssertTrue(Keychain.set(.model, "anthropic/legacy-model"))
            let source = makeSource()

            source.applyForTesting(.modelList(
                models: ["openai/current-model"], current: "openai/current-model", details: []))
            XCTAssertEqual(source.model.activeModelId, "openai/current-model")
            XCTAssertEqual(Keychain.get(.model), "anthropic/legacy-model")

            source.applyForTesting(.modelChanged(model: "other-profile/confirmed-model"))
            XCTAssertEqual(source.model.activeModelId, "other-profile/confirmed-model")
            XCTAssertEqual(Keychain.get(.model), "anthropic/legacy-model")
        }

        func testModelSwitchWaitsForConfirmationWhileStreaming() async {
            let source = makeSource()
            source.model.activeModelId = "provider/old"
            source.model.streaming = true
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }

            source.setModel("provider/new")
            await waitForSubmittedCommands(1, commands: submitted)
            XCTAssertEqual(source.model.activeModelId, "provider/old")
            XCTAssertTrue(source.model.streaming)
            guard case .setModel(model: "provider/new")? = submitted.first else {
                return XCTFail("expected the qualified model switch command")
            }
            source.applyForTesting(.modelChanged(model: "provider/new"))
            XCTAssertEqual(source.model.activeModelId, "provider/new")
            XCTAssertTrue(source.model.streaming)
        }

        func testRejectedModelSwitchDoesNotStopStreaming() async {
            let source = makeSource()
            source.model.activeModelId = "provider/old"
            source.model.streaming = true
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting {
                submitted.append($0)
                throw NSError(domain: "ModelSwitch", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "model unavailable"])
            }

            source.setModel("provider/new")
            await waitForSubmittedCommands(1, commands: submitted)
            XCTAssertEqual(source.model.activeModelId, "provider/old")
            XCTAssertTrue(source.model.streaming)
            XCTAssertNotNil(source.model.controlsError)
            XCTAssertNil(source.model.error)
        }

        func testFastModeWaitsForEngineConfirmationAndRejectsDuplicateChanges() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }

            source.setFastMode(true)
            source.setFastMode(true)
            XCTAssertTrue(source.model.fastModePending)
            XCTAssertFalse(source.model.fastMode)
            await waitForSubmittedCommands(1, commands: submitted)
            XCTAssertEqual(submitted.count, 1)
            guard case .setFastMode(enabled: true)? = submitted.first else {
                return XCTFail("expected the engine speed command")
            }
            XCTAssertTrue(source.model.fastModePending)
            source.applyForTesting(.fastModeChanged(enabled: true))
            XCTAssertTrue(source.model.fastMode)
            XCTAssertFalse(source.model.fastModePending)
            XCTAssertNil(source.model.fastModeError)

            source.applyForTesting(.fastModeChanged(enabled: false))
            XCTAssertFalse(source.model.fastMode)
        }

        func testRejectedFastModeChangePreservesConfirmedStateAndReply() async {
            let source = makeSource()
            source.model.fastMode = true
            source.model.streaming = true
            source.setCommandSubmitterForTesting { _ in
                throw NSError(domain: "FastMode", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "speed unavailable"])
            }

            source.setFastMode(false)
            for _ in 0..<50 where source.model.fastModePending { await Task.yield() }
            XCTAssertTrue(source.model.fastMode)
            XCTAssertFalse(source.model.fastModePending)
            XCTAssertEqual(source.model.fastModeError, "speed unavailable")
            XCTAssertTrue(source.model.streaming)
            XCTAssertNil(source.model.error)
        }

        func testNormalPermissionModeSendsTheSessionModeCommand() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in submitted.append(command) }

            source.setPermissionMode("plan")
            await waitForSubmittedCommands(1, commands: submitted)

            guard case .setPermissionMode(mode: "plan")? = submitted.first else {
                return XCTFail("expected a plan permission-mode command")
            }
            XCTAssertFalse(source.model.controlsPending)
        }

        func testSettingsPermissionSelectionUsesTheSameEnginePersistenceCommand() async throws {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }

            // SettingsHost calls this path, whereas the composer calls
            // setPermissionMode. Both must reach the engine's persisted setter.
            try await source.submitEngineCommand(.setPermissionMode(mode: "acceptEdits"))

            XCTAssertEqual(submitted.count, 1)
            guard case .setPermissionMode(mode: "acceptEdits")? = submitted.first else {
                return XCTFail("expected the shared engine permission-mode setter")
            }
        }

        func testRejectedPermissionSelectionKeepsThePreviousMode() async {
            let source = makeSource()
            source.model.requestedPermissionMode = "acceptEdits"
            source.model.effectivePermissionMode = "acceptEdits"
            source.model.streaming = true
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting {
                submitted.append($0)
                throw NSError(domain: "PermissionSelection", code: 1)
            }

            source.setPermissionMode("plan")
            await waitForSubmittedCommands(1, commands: submitted)

            XCTAssertEqual(source.model.requestedPermissionMode, "acceptEdits")
            XCTAssertEqual(source.model.effectivePermissionMode, "acceptEdits")
            XCTAssertFalse(source.model.controlsPending)
            XCTAssertNotNil(source.model.controlsError)
            XCTAssertTrue(source.model.streaming)
        }

        func testRejectedBypassConfirmationDoesNotSubmitOrSuppressTheWarning() async throws {
            let suiteName = "RejectedBypassTests.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer { defaults.removePersistentDomain(forName: suiteName) }
            let repository = PermissionModeConfigurationRepository(defaults: defaults)
            let source = makeSource(permissionModeRepository: repository)
            source.model.requestedPermissionMode = "acceptEdits"
            var submitted: [ClientCommand] = []
            var confirmations = 0
            source.setBypassPermissionsConfirmerForTesting {
                confirmations += 1
                throw NSError(domain: "BypassConfirmation", code: 1)
            }
            source.setCommandSubmitterForTesting { submitted.append($0) }

            source.confirmAndSetBypassPermissions(suppressWarning: true)
            for _ in 0..<50 where source.model.controlsPending { await Task.yield() }

            XCTAssertEqual(confirmations, 1)
            XCTAssertTrue(submitted.isEmpty)
            XCTAssertEqual(source.model.requestedPermissionMode, "acceptEdits")
            XCTAssertFalse(repository.bypassWarningSuppressed())
            XCTAssertFalse(source.model.controlsPending)
            XCTAssertNotNil(source.model.controlsError)
        }

        private func flushTasks(_ count: Int = 4) async {
            for _ in 0..<count { await Task.yield() }
        }

        private func waitForSubmittedCommands(
            _ expectedCount: Int,
            commands: @autoclosure () -> [ClientCommand]
        ) async {
            for _ in 0..<50 where commands().count < expectedCount {
                await Task.yield()
            }
        }

        func testShellLifecycleBuildsStructuredRunCard() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 7, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 7))
            source.applyForTesting(
                .toolUseStarted(
                    id: "task-1",
                    tool: "bash",
                    inputJson: #"{"command":"echo ok","cwd":"/workspace"}"#,
                    header: nil
                ))
            source.applyForTesting(.toolHeartbeat(id: "task-1", tool: "bash", elapsedMs: 320))
            source.applyForTesting(
                .toolUseResult(
                    id: "task-1",
                    tool: "bash",
                    resultJson: #"{"data":{"stdout":"ok\n","stderr":"","exit_code":0,"duration_ms":480,"truncated":false}}"#,
                    isError: false,
                    display: nil
                ))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }

            XCTAssertEqual(run.status, .running, "tool events do not end the turn")
            XCTAssertEqual(run.shellCards.count, 1)
            XCTAssertEqual(run.shellCards[0].command, "echo ok")
            XCTAssertEqual(run.shellCards[0].cwd, "/workspace")
            XCTAssertEqual(run.shellCards[0].stdout, "ok\n")
            XCTAssertEqual(run.shellCards[0].durationMs, 480)
            XCTAssertEqual(run.shellCards[0].status, .completed)
            XCTAssertEqual(run.tools.first?.tool, "Shell")
            XCTAssertEqual(run.tools.first?.status, .completed)
            XCTAssertEqual(source.model.statusLine, "Shell 完成")
        }

        func testLiveActivityLedgerPreservesWireOrderAndTextBoundaries() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 8, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 8))
            source.applyForTesting(.thinkingDelta(thinking: "inspect ", signature: nil))
            source.applyForTesting(.thinkingDelta(thinking: "workspace", signature: nil))
            source.applyForTesting(.toolUseStarted(
                id: "tool-a", tool: "Read", inputJson: #"{"path":"a"}"#, header: nil))
            source.applyForTesting(.textDelta(text: "result text"))
            source.applyForTesting(.systemNotice(message: "waiting", isError: false))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else { return XCTFail("expected active run") }
            XCTAssertEqual(run.activities.count, 4)
            guard case let .reasoning(_, reasoning) = run.activities[0],
                  case .tool(let toolID) = run.activities[1],
                  case .textBoundary = run.activities[2],
                  case .notice(let noticeID) = run.activities[3]
            else { return XCTFail("activity order must follow event order") }
            XCTAssertEqual(reasoning, "inspect workspace")
            XCTAssertEqual(toolID, "tool-a")
            XCTAssertTrue(noticeID.hasPrefix("notice-"))

            let groups = source.model.visibleTimelineGroups
            XCTAssertEqual(groups.filter(\.isToolGroup).count, 1)
            XCTAssertEqual(groups.map(\.status), [.running, .running, nil, .running])
        }

        func testLiveTimelineInterleavesTextToolAndLaterText() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 81, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 81))
            source.applyForTesting(.textDelta(text: "A"))
            source.applyForTesting(.toolUseStarted(
                id: "tool-a",
                tool: "Read",
                inputJson: #"{"path":"a"}"#,
                header: nil
            ))
            source.applyForTesting(.textDelta(text: "B"))
            source.applyForTesting(.messageComplete(
                stopReason: "tool_use",
                message: MessageDto(role: "assistant", blocks: [
                    .text(text: "A"),
                    .toolUse(id: "tool-a", tool: "Read", inputJson: #"{"path":"a"}"#, header: nil),
                    .text(text: "B"),
                ])
            ))

            let order = source.model.visibleTimelineGroups.flatMap(\.rows).map { row -> String in
                switch row {
                case let .message(message): return "message:\(message.text)"
                case let .tool(_, trace): return "tool:\(trace.id)"
                case .reasoning: return "reasoning"
                case .notice: return "notice"
                case .commandOutput: return "command-output"
                }
            }
            XCTAssertEqual(order, ["message:A", "tool:tool-a", "message:B"])

            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost)
            )
            XCTAssertEqual(
                source.model.turnCompletion?.finalAssistantText,
                "B",
                "closing MessageComplete must not erase the final text used by turn completion"
            )
        }

        func testSettledDurableTurnRequestsAuthoritativeSessionRefresh() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 31, sessionId: "session-a")

            XCTAssertEqual(source.model.sessionRefreshRevision, 0)
            source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
            XCTAssertFalse(source.model.streaming)
        }

        func testTerminalRunStaysAnchoredBeforeLaterOutOfBandNotice() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 33, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 33))
            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [.text(text: "完成")])
            ))
            source.applyForTesting(.taskStatusChanged(
                taskId: "late-task", status: .completed, originSessionId: nil, error: nil))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            let kinds = source.model.items.map { item -> String in
                switch item {
                case .message: return "message"
                case .commandOutput: return "command-output"
                case .run: return "run"
                case .notice: return "notice"
                case .toolCall: return "tool"
                }
            }
            XCTAssertEqual(
                kinds,
                ["run", "message", "notice"],
                "the terminal result belongs to its assistant message, not the list tail")
        }

        func testCoordinatorWorkersKeepBackgroundExecutionAfterTurnEndsUntilIdlePush() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 34, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 34))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 1, team: "review"))
            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [.text(text: "Delegated")])
            ))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertTrue(
                source.model.requiresBackgroundExecution,
                "a worker can outlive the assistant turn that started it")

            source.applyForTesting(.coordinatorStatus(activeWorkers: 0, team: "review"))
            XCTAssertFalse(source.model.requiresBackgroundExecution)
        }

        func testCoordinatorIdlePushClearsWorkersFromAnEarlierTurn() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 35, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 35))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 1, team: "review"))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            source.beginTurnForTesting(turnId: 36, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 36))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 0, team: "review"))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            let workerCounts = source.model.items.compactMap { item -> UInt32? in
                guard case let .run(run) = item else { return nil }
                return run.activeWorkers
            }
            XCTAssertTrue(workerCounts.allSatisfy { $0 == 0 })
            XCTAssertFalse(source.model.requiresBackgroundExecution)
        }

        func testPendingSessionTransitionDoesNotRequestPrematureCatalogRefresh() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 32, sessionId: "session-a")
            source.model.sessionTransitionPending = true

            source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertEqual(source.model.sessionRefreshRevision, 0)
        }

        func testTelemetryAndCoordinatorEventsAccumulateOnActiveRun() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 3, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 3))
            source.applyForTesting(.thinkingDelta(thinking: "reasoning", signature: nil))
            source.applyForTesting(.systemNotice(message: "still working", isError: false))
            source.applyForTesting(
                .usageUpdate(isSnapshot: nil, inputTokens: 11, outputTokens: 22, cacheReadTokens: 3, cacheCreationTokens: 4))
            source.applyForTesting(.apiRetry(message: "429", attempt: 2, maxRetries: 5, delayMs: 1200))
            source.applyForTesting(.costUpdate(
                totalUsd: 0.12, inputTokens: 11, outputTokens: 22, apiCalls: 1, sessionDurationSecs: 9, formatted: "$0.12"))
            source.applyForTesting(.compactionCompleted(messagesBefore: 20, messagesAfter: 7, bytesSaved: 4096, summary: "kept context"))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 2, team: "triage"))
            source.applyForTesting(
                .coordinatorWorker(
                    worker: CoordinatorWorkerDto(agentId: "w1", name: "worker-1", agentType: "executor", status: "working")))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }

            XCTAssertEqual(run.reasoning, "reasoning")
            XCTAssertEqual(run.notices.last?.text, "still working")
            XCTAssertEqual(run.usage, ConversationUsageSnapshot(inputTokens: 11, outputTokens: 22, cacheReadTokens: 3, cacheCreationTokens: 4))
            XCTAssertEqual(run.retry, ConversationRetrySnapshot(message: "429", attempt: 2, maxRetries: 5, delayMs: 1200))
            XCTAssertEqual(run.costFormatted, "$0.12")
            XCTAssertEqual(run.compactions.first?.messagesAfter, 7)
            XCTAssertEqual(run.activeWorkers, 2)
            XCTAssertEqual(run.coordinatorTeam, "triage")
            XCTAssertEqual(run.workers.first?.name, "worker-1")
        }

        func testMessageCompleteKeepsThoughtAndToolsOutOfMessageBubble() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 9, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 9))
            source.applyForTesting(.thinkingDelta(thinking: "推理", signature: nil))
            source.applyForTesting(.toolUseStarted(
                id: "t1",
                tool: "Read",
                inputJson: #"{"path":"/tmp/a"}"#,
                header: nil
            ))
            source.applyForTesting(.textDelta(text: "正文"))
            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [
                    .thinking(thinking: "推理", signature: nil),
                    .toolUse(id: "t1", tool: "Read", inputJson: #"{"path":"/tmp/a"}"#, header: nil),
                    .text(text: "正文"),
                    .toolResult(id: "t1", tool: "Read", resultJson: #"{"result":"ok"}"#, isError: false, oldString: nil, newString: nil, filePath: nil, display: nil),
                ])))

            XCTAssertEqual(source.model.messages.count, 1)
            let message = source.model.messages[0]
            let detail = source.model.messageDetails[message.id]

            XCTAssertNotNil(detail)
            XCTAssertEqual(detail?.blocks, [.text("正文")])
            XCTAssertEqual(message.text, "正文")

            let rows = source.model.visibleTimelineGroups.flatMap(\.rows)
            XCTAssertEqual(rows.filter { if case .reasoning = $0 { return true }; return false }.count, 1)
            XCTAssertEqual(rows.filter { if case .tool = $0 { return true }; return false }.count, 1)
            XCTAssertEqual(rows.filter { if case .message = $0 { return true }; return false }.count, 1)
        }

        func testUnloadedSlashCatalogNeverRoutesSlashTextAsPrompt() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in submitted.append(command) }

            XCTAssertFalse(source.model.slashCommandsLoaded)
            XCTAssertNil(source.send("  /help  "))
            await flushTasks()

            XCTAssertTrue(submitted.isEmpty)
            XCTAssertTrue(source.model.messages.isEmpty)
            XCTAssertFalse(source.model.streaming)
        }

        func testStreamingMessageKeepsStableIdentityAcrossUpdates() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 10, sessionId: "session-a")

            source.applyForTesting(.textDelta(text: "Hel"))
            let initialMessageID = source.model.messages[0].id
            let initialRenderID = source.model.items[0].id

            source.applyForTesting(.textDelta(text: "lo"))

            XCTAssertEqual(source.model.messages[0].id, initialMessageID)
            XCTAssertEqual(source.model.items[0].id, initialRenderID)
            XCTAssertEqual(source.model.messages[0].text, "Hello")

            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [.text(text: "Hello!")])
            ))

            XCTAssertEqual(source.model.messages[0].id, initialMessageID)
            XCTAssertEqual(source.model.items[0].id, initialRenderID)
            XCTAssertEqual(source.model.messages[0].text, "Hello!")
        }

        func testLateEventsDroppedAfterSessionResume() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 4, sessionId: "old-session")

            source.applyForTesting(.turnStarted(turnId: 4))
            source.applyForTesting(
                .toolUseStarted(
                    id: "task-2",
                    tool: "bash",
                    inputJson: #"{"command":"pwd"}"#,
                    header: nil
                ))
            source.expectSessionResumeForTesting("new-session")
            source.applyForTesting(.sessionResumed(sessionId: "new-session", mode: .code, messages: []))
            source.applyForTesting(
                .toolUseResult(
                    id: "task-2",
                    tool: "bash",
                    resultJson: #"{"data":{"stdout":"stale","stderr":"","exit_code":0}}"#,
                    isError: false,
                    display: nil
                ))

            XCTAssertTrue(source.model.messages.isEmpty)
            XCTAssertFalse(source.model.items.contains {
                if case let .run(run) = $0 {
                    return run.shellCards.contains(where: { $0.stdout == "stale" })
                }
                return false
            })
        }

        func testCancellationKeepsTurnOwnedAndDraftSendBlockedUntilHostReturns() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                await MainActor.run {
                    submittedCommands.append(command)
                }
                if case .cancel = command {
                    await withCheckedContinuation { continuation in
                        releaseCancellation = continuation
                    }
                }
            }

            source.beginTurnForTesting(turnId: 1, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 1))
            source.applyForTesting(.toolUseStarted(
                id: "tool-a",
                tool: "WebSearch",
                inputJson: #"{"query":"weather"}"#,
                header: nil
            ))
            source.cancel()
            await flushTasks()

            source.send("B prompt")
            await flushTasks()

            XCTAssertTrue(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming, "the current owner remains live while Cancel awaits Block tools")
            XCTAssertEqual(source.model.statusLine, "正在停止…")
            XCTAssertEqual(submittedCommands.count, 1, "the next prompt must stay local while Cancel is pending")
            if case let .cancel(turnId)? = submittedCommands.first {
                XCTAssertEqual(turnId, 1)
            } else {
                XCTFail("expected the matching cancel submission")
            }
            XCTAssertTrue(source.model.messages.isEmpty, "a blocked send must not append or consume the draft")

            source.applyForTesting(.toolHeartbeat(id: "tool-a", tool: "WebSearch", elapsedMs: 2_000))
            guard case let .run(stoppingRun)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                releaseCancellation?.resume()
                return XCTFail("expected the active run")
            }
            XCTAssertEqual(stoppingRun.tools.first?.status, .running)
            XCTAssertEqual(stoppingRun.tools.first?.elapsedMs, 2_000, "heartbeat must keep advancing during safe cancellation")

            source.applyForTesting(.turnEnded(outcome: .cancelled, stopReason: nil, cost: zeroCost))
            await flushTasks()
            XCTAssertFalse(source.model.streaming, "terminal may settle the visible run")
            XCTAssertTrue(source.model.isCancelling, "terminal must not release send before Cancel FFI returns")

            releaseCancellation?.resume()
            releaseCancellation = nil
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "session-a",
                turnId: 1,
                state: .cancelled,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                reason: "user_cancelled"
            )))
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            source.send("B prompt")
            await flushTasks()

            XCTAssertEqual(submittedCommands.count, 2, "B should submit exactly once after cancellation returns")
            if case let .sendPrompt(text, _, _, turnId)? = submittedCommands.last {
                XCTAssertEqual(text, "B prompt")
                XCTAssertEqual(turnId, 2)
            } else {
                XCTFail("expected B sendPrompt after cancellation completion")
            }
            XCTAssertEqual(source.model.messages.last?.text, "B prompt")
            XCTAssertTrue(source.model.streaming)
        }

        func testCancellingTurnKeepsRowsLiveUntilTerminalThenClosesThem() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 8, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 8))
            source.applyForTesting(.toolUseStarted(
                id: "search-1",
                tool: "WebSearch",
                inputJson: #"{"query":"Wuhan weather today"}"#,
                header: nil
            ))
            source.applyForTesting(.toolUseStarted(
                id: "shell-1",
                tool: "Bash",
                inputJson: #"{"command":"sleep 30"}"#,
                header: nil
            ))

            source.cancelForTesting()
            source.applyForTesting(.toolHeartbeat(id: "search-1", tool: "WebSearch", elapsedMs: 3_000))

            guard case let .run(stoppingRun)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            XCTAssertEqual(stoppingRun.status, .running)
            XCTAssertEqual(stoppingRun.tools.map(\.status), [.running, .running])
            XCTAssertEqual(stoppingRun.tools.first?.elapsedMs, 3_000)
            XCTAssertEqual(stoppingRun.shellCards.first?.status, .running)

            source.applyForTesting(.turnEnded(outcome: .cancelled, stopReason: "cancelled", cost: zeroCost))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected the settled run")
            }
            XCTAssertEqual(run.status, .cancelled)
            XCTAssertEqual(run.tools.map(\.status), [.cancelled, .cancelled])
            XCTAssertEqual(run.shellCards.first?.status, .cancelled)
            XCTAssertFalse(run.tools.contains(where: { $0.status == .running }))
            XCTAssertFalse(run.shellCards.contains(where: { $0.status == .running }))
        }

        func testCancellationFailureRestoresRetryableStopState() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            var cancelAttempts = 0
            source.setCommandSubmitterForTesting { command in
                if case .cancel = command {
                    await MainActor.run { cancelAttempts += 1 }
                    throw CancelFailure.rejected
                }
            }
            source.beginTurnForTesting(turnId: 44, sessionId: "session-a")

            source.cancel()
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming, "failed delivery must preserve the original turn owner")
            XCTAssertEqual(source.model.error?.kind, .host)
            XCTAssertEqual(cancelAttempts, 1)

            source.cancel()
            await flushTasks(8)
            XCTAssertEqual(cancelAttempts, 2, "Stop must become retryable after a Cancel submission failure")
        }

        func testCancellationFailureKeepsQueuedPermission() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 91,
                kind: .toolUseConfirm(
                    toolName: "Bash",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: nil,
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            source.setCommandSubmitterForTesting { command in
                if case .cancel = command { throw CancelFailure.rejected }
            }
            source.beginTurnForTesting(turnId: 46, sessionId: "session-a")
            source.model.pendingPermissions = [permission]

            source.cancel()
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.pendingPermissions, [permission])
        }

        func testCancellationFailureDoesNotRestoreResolvedPermission() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 97,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "design", color: "design", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                guard case .cancel = command else { return }
                await withCheckedContinuation { continuation in
                    releaseCancellation = continuation
                }
                throw CancelFailure.rejected
            }
            source.beginTurnForTesting(turnId: 50, sessionId: "session-a")
            source.model.pendingPermissions = [permission]

            source.cancel()
            await flushTasks()
            source.model.pendingPermissions = []
            releaseCancellation?.resume()
            releaseCancellation = nil
            await flushTasks(8)

            XCTAssertTrue(source.model.pendingPermissions.isEmpty)
        }

        func testCancelForTestingKeepsBackgroundPermissionQueued() {
            let source = makeSource()
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 94,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"ls -la"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "review", color: "review", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            source.beginTurnForTesting(turnId: 48, sessionId: "session-a")
            source.model.pendingPermissions = [permission]

            source.cancelForTesting()

            XCTAssertTrue(source.model.isCancelling)
            XCTAssertEqual(source.model.statusLine, String(localized: "chat_stopping"))
            XCTAssertEqual(source.model.pendingPermissions, [permission])
        }

        func testResumeSessionKeepsVisibleTranscriptAndBackgroundPermissionUntilReplay() async {
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 95,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "design", color: "design", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))

            let resumedSource = makeSource()
            var resumedCommands: [ClientCommand] = []
            resumedSource.setCommandSubmitterForTesting { command in
                await MainActor.run { resumedCommands.append(command) }
            }
            let resumedMessage = Message(role: .user, text: "keep transcript behavior")
            resumedSource.model.messages = [resumedMessage]
            resumedSource.model.items = [.message(resumedMessage)]
            resumedSource.model.pendingPermissions = [permission]

            resumedSource.resumeSession("session-b")
            await waitForSubmittedCommands(1, commands: resumedCommands)

            XCTAssertEqual(resumedSource.model.messages, [resumedMessage])
            XCTAssertEqual(resumedSource.model.items, [.message(resumedMessage)])
            XCTAssertEqual(resumedSource.model.pendingPermissions, [permission])
            XCTAssertFalse(resumedSource.model.isNew)
            resumedSource.applyForTesting(.sessionResumed(sessionId: "session-b", mode: .code, messages: []))
            XCTAssertTrue(resumedSource.model.messages.isEmpty)
            XCTAssertTrue(resumedSource.model.items.isEmpty)
            XCTAssertEqual(resumedSource.model.pendingPermissions, [permission])
            XCTAssertEqual(resumedSource.model.activeSessionId, "session-b")
        }

        func testVisibleActiveSessionResumeIsNoOpAcrossLifecycleReentry() async {
            let source = makeSource()
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { commands.append(command) }
            }
            let visibleMessage = Message(role: .ai, text: "still visible")
            source.model.activeSessionId = "session-a"
            source.model.messages = [visibleMessage]
            source.model.items = [.message(visibleMessage)]

            source.resumeSession("session-a")
            await flushTasks(8)

            XCTAssertTrue(commands.isEmpty)
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.messages, [visibleMessage])
            XCTAssertEqual(source.model.items, [.message(visibleMessage)])
        }

        func testDurableTurnAttachCursorStaysInMemoryWithinOneSource() throws {
            let sandboxRoot = try makeSandboxRoot()
            let source = makeSource(appSandboxRoot: sandboxRoot)

            source.beginDurableTurnForTesting(turnId: 700, sessionId: "session-a")
            source.recordDurableSequenceForTesting(turnId: 700, sequence: 9)

            XCTAssertEqual(source.durableAttachCursorForTesting(), 9)
        }

        func testDurableTurnColdAttachResetsPersistedCursorToReplayFromRetainedEvents() throws {
            let sandboxRoot = try makeSandboxRoot()
            let source = makeSource(appSandboxRoot: sandboxRoot)

            source.beginDurableTurnForTesting(turnId: 701, sessionId: "session-a")
            source.recordDurableSequenceForTesting(turnId: 701, sequence: 11)

            let resumedSource = makeSource(appSandboxRoot: sandboxRoot)
            XCTAssertEqual(
                resumedSource.durableAttachCursorForTesting(),
                0,
                "a fresh source must not trust a cursor whose UI projection was never durably persisted"
            )
        }

        func testColdRestoreKeepsAuthoritativeTerminalTranscriptWhenRetainedEventsReplay() async throws {
            let sandboxRoot = try makeSandboxRoot()
            let firstSource = makeSource(appSandboxRoot: sandboxRoot)
            firstSource.beginDurableTurnForTesting(turnId: 702, sessionId: "session-a")

            let source = makeSource(appSandboxRoot: sandboxRoot)
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                submitted.append(command)
                switch command {
                case .attachTurn:
                    // A terminal checkpoint is delivered before its retained
                    // envelopes. The transcript from SessionResumed is the
                    // authoritative rendering in this crash window.
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "session-a",
                        turnId: 702,
                        state: .completed,
                        firstSequence: 1,
                        lastSequence: 3,
                        safeToResume: false,
                        reason: nil
                    )))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "session-a",
                        turnId: 702,
                        sequence: 1,
                        eventJson: #"{"type":"text_delta","text":"terminal answer"}"#
                    ))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "session-a",
                        turnId: 702,
                        sequence: 2,
                        eventJson: #"{"type":"tool_use_started","id":"tool-702","tool":"Read","input_json":"{}"}"#
                    ))
                case .resumeTurn:
                    break
                default:
                    break
                }
            }

            source.expectSessionResumeForTesting("session-a")
            source.applyForTesting(.sessionResumed(
                sessionId: "session-a",
                mode: .code,
                messages: [
                    MessageDto(role: "user", blocks: [.text(text: "run")]),
                    MessageDto(role: "assistant", blocks: [
                        .text(text: "terminal answer"),
                        .toolUse(id: "tool-702", tool: "Read", inputJson: "{}", header: nil),
                    ]),
                    MessageDto(role: "user", blocks: [
                        .toolResult(
                            id: "tool-702",
                            tool: "Read",
                            resultJson: #"{"result":"ok"}"#,
                            isError: false,
                            oldString: nil,
                            newString: nil,
                            filePath: nil,
                            display: nil
                        ),
                    ]),
                ]
            ))
            await flushTasks(12)

            XCTAssertEqual(submitted.compactMap { command -> String? in
                switch command {
                case .attachTurn: return "attach"
                case .resumeTurn: return "resume"
                default: return nil
                }
            }, ["attach", "resume"])
            XCTAssertEqual(source.model.messages.map(\.text), ["run", "terminal answer"])
            XCTAssertEqual(
                source.model.items.filter {
                    if case .run = $0 { return true }
                    return false
                }.count,
                1,
                "retained terminal tool events must not duplicate the restored run"
            )
            XCTAssertFalse(source.model.streaming)
        }

        func testEmptyUnconfirmedActiveSessionStillRequestsProcessRestoreReplay() async {
            let source = makeSource()
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { commands.append(command) }
            }
            source.model.activeSessionId = "session-a"
            source.model.isNew = true

            source.resumeSession("session-a")
            await waitForSubmittedCommands(1, commands: commands)

            XCTAssertEqual(commands.count, 1)
            guard let command = commands.first else { return }
            guard case let .resumeSession(sessionId, cwd) = command else {
                return XCTFail("process restoration must still request transcript replay")
            }
            XCTAssertEqual(sessionId, "session-a")
            XCTAssertNil(cwd)
            XCTAssertTrue(source.model.sessionTransitionPending)
        }

        func testPausedTaskDoesNotHoldBackgroundExecutionLease() {
            let source = makeSource()
            source.model.backgroundTasks = [BackgroundTaskSnapshot(
                id: "workflow-paused",
                descriptionText: "Paused workflow",
                status: .paused
            )]

            XCTAssertFalse(source.model.requiresBackgroundExecution)

            source.model.backgroundTasks[0].status = .pending
            XCTAssertTrue(source.model.requiresBackgroundExecution)
            source.model.backgroundTasks[0].status = .running
            XCTAssertTrue(source.model.requiresBackgroundExecution)
        }

        func testPermissionPresentationRequiresForegroundAttachedPresenter() {
            XCTAssertFalse(PermissionPromptPresentationPolicy.canPresent(
                sceneActivationState: .background,
                presenterIsAttached: true
            ))
            XCTAssertFalse(PermissionPromptPresentationPolicy.canPresent(
                sceneActivationState: .foregroundActive,
                presenterIsAttached: false
            ))
            XCTAssertTrue(PermissionPromptPresentationPolicy.canPresent(
                sceneActivationState: .foregroundActive,
                presenterIsAttached: true
            ))
        }

        func testBackgroundWorkerPermissionSurfacesAfterMainTurnEnds() async {
            let source = makeSource()
            let request = PermissionRequest(
                requestId: 92,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"ls -la"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "design", color: "design", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            )

            await EnginePermissionSink(source: source).onRequest(request: request)

            XCTAssertFalse(source.model.streaming)
            XCTAssertEqual(source.model.pendingPermissions, [PendingPermission(request: request)])
        }

        func testAuthoritativePermissionResolutionRemovesOnlyMatchingPrompt() {
            let source = makeSource()
            let first = PendingPermission(request: PermissionRequest(
                requestId: 201,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: nil,
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            let second = PendingPermission(request: PermissionRequest(
                requestId: 202,
                kind: .toolUseConfirm(
                    toolName: "Write",
                    toolInputJson: #"{"path":"notes.md"}"#,
                    defaultAllow: false
                ),
                worker: nil,
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            source.model.pendingPermissions = [first, second]

            source.applyForTesting(.permissionRequestResolved(
                requestId: first.requestId,
                resolution: .expired
            ))

            XCTAssertEqual(source.model.pendingPermissions, [second])
        }

        func testFailedPermissionResponseRestoresQueuedRequest() async {
            enum SubmitFailure: Error { case rejected }

            let source = makeSource()
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 98,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "design", color: "design", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            source.setCommandSubmitterForTesting { _ in
                throw SubmitFailure.rejected
            }
            source.model.pendingPermissions = [permission]

            source.approvePermission(permission.requestId, .allowOnce)
            XCTAssertTrue(source.model.pendingPermissions.isEmpty)
            await flushTasks(8)

            XCTAssertEqual(source.model.pendingPermissions, [permission])
            XCTAssertEqual(source.model.error?.kind, .host)
        }

        func testAllowAutoPermissionResponseSubmitsApproveCommand() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submitted.append(command) }
            }

            source.approvePermission(55, .allowAuto)
            await waitForSubmittedCommands(1, commands: submitted)

            guard case let .approvePermission(requestId, response)? = submitted.first else {
                return XCTFail("expected an approve permission command")
            }
            XCTAssertEqual(requestId, 55)
            XCTAssertEqual(response, .allowAuto)
        }

        func testTerminalTurnErrorKeepsBackgroundPermissionQueued() {
            let source = makeSource()
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 96,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"whoami"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "review", color: "review", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
            source.beginTurnForTesting(turnId: 49, sessionId: "session-a")
            source.model.pendingPermissions = [permission]

            source.applyForTesting(.error(kind: .internal, message: "boom"))

            XCTAssertEqual(source.model.pendingPermissions, [permission])
            XCTAssertEqual(source.model.error?.kind, .internal)
        }

        func testPermissionPromptPresentsAboveCurrentModal() async throws {
            guard let scene = UIApplication.shared.connectedScenes
                .compactMap({ $0 as? UIWindowScene })
                .first(where: { $0.activationState == .foregroundActive })
            else {
                throw XCTSkip("No foreground window scene")
            }

            let previousKeyWindow = scene.windows.first(where: \.isKeyWindow)
            let source = makeSource()
            let host = UIHostingController(rootView: EnginePermissionPromptHost(
                model: source.model,
                onApprove: { _, _ in },
                onDeny: { _ in }
            ))
            let window = UIWindow(windowScene: scene)
            window.rootViewController = host
            window.makeKeyAndVisible()
            let animationsWereEnabled = UIView.areAnimationsEnabled
            UIView.setAnimationsEnabled(false)
            defer {
                host.dismiss(animated: false)
                window.isHidden = true
                previousKeyWindow?.makeKey()
                UIView.setAnimationsEnabled(animationsWereEnabled)
            }

            let existingModal = UIViewController()
            existingModal.modalPresentationStyle = .fullScreen
            host.present(existingModal, animated: false)

            source.model.pendingPermissions = [PendingPermission(request: PermissionRequest(
                requestId: 93,
                kind: .toolUseConfirm(
                    toolName: "Shell",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: WorkerInfoDto(name: "design", color: "design", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))]

            // `DispatchQueue.main.async` retries need an actual run-loop turn;
            // repeated task yields are not guaranteed to drain GCD while the
            // full test target is running under load.
            for _ in 0..<50 where existingModal.presentedViewController == nil {
                try await Task.sleep(for: .milliseconds(10))
            }

            XCTAssertNotNil(existingModal.presentedViewController)
            XCTAssertEqual(
                existingModal.presentedViewController?.modalPresentationStyle,
                .pageSheet
            )
        }

        func testTerminalBeforeFailedCancellationClearsStoppingStatus() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                guard case .cancel = command else { return }
                await withCheckedContinuation { continuation in
                    releaseCancellation = continuation
                }
                throw CancelFailure.rejected
            }
            source.beginTurnForTesting(turnId: 47, sessionId: "session-a")

            source.cancel()
            await flushTasks()
            source.applyForTesting(.turnEnded(
                outcome: .cancelled,
                stopReason: "cancelled",
                cost: zeroCost
            ))
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "session-a",
                turnId: 47,
                state: .cancelled,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                reason: "user_cancelled"
            )))
            releaseCancellation?.resume()
            releaseCancellation = nil
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            XCTAssertFalse(source.model.streaming)
            XCTAssertNil(source.model.statusLine)
        }

        func testSessionSwitchCancellationFailureKeepsOriginalTurnRetryable() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { commands.append(command) }
                if case .cancel = command { throw CancelFailure.rejected }
            }
            source.beginTurnForTesting(turnId: 45, sessionId: "session-a")
            source.applyForTesting(.textDelta(text: "still owned"))
            let originalItems = source.model.items

            source.resumeSession("session-b")
            await flushTasks(12)

            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertFalse(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.items, originalItems)
            XCTAssertEqual(commands.count, 1)
            guard let first = commands.first else { return }
            guard case let .cancel(turnId) = first else {
                return XCTFail("a failed cancellation must not submit ResumeSession")
            }
            XCTAssertEqual(turnId, 45)

            source.cancel()
            await flushTasks(8)
            XCTAssertEqual(commands.count, 2, "the same visible turn must expose a retryable Stop")
        }

        func testInterruptionMetadataMarksOnlyInterruptedToolCancelled() {
            XCTAssertTrue(ConversationExecutionParsing.isCancellationResult(
                #"{"error":"interrupted","tool_denial_kind":"interrupted"}"#
            ))
            XCTAssertFalse(ConversationExecutionParsing.isCancellationResult(
                #"{"error":"network unavailable"}"#
            ))
        }

        func testResumeSerializesCancelBeforeSessionTransition() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
                if case let .cancel(turnId?) = command {
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "session-a",
                        turnId: turnId,
                        state: .cancelled,
                        firstSequence: 0,
                        lastSequence: 0,
                        safeToResume: false,
                        reason: "user_cancelled"
                    )))
                }
            }
            source.beginTurnForTesting(turnId: 17, sessionId: "session-a")

            source.resumeSession("session-b")
            await waitForSubmittedCommands(2, commands: submittedCommands)

            XCTAssertEqual(submittedCommands.count, 2)
            guard submittedCommands.count == 2 else { return }
            if case let .cancel(turnId) = submittedCommands[0] {
                XCTAssertEqual(turnId, 17)
            } else {
                XCTFail("the old turn must be cancelled first")
            }
            if case let .resumeSession(sessionId, cwd) = submittedCommands[1] {
                XCTAssertEqual(sessionId, "session-b")
                XCTAssertNil(cwd)
            } else {
                XCTFail("resume must follow the awaited cancel")
            }
        }

        func testRapidResumeSubmitsOnlyLatestSessionAfterSharedCancellation() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
                if case .cancel = command {
                    await withCheckedContinuation { continuation in
                        releaseCancellation = continuation
                    }
                    if case let .cancel(turnId?) = command {
                        source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                            sessionId: "session-a",
                            turnId: turnId,
                            state: .cancelled,
                            firstSequence: 0,
                            lastSequence: 0,
                            safeToResume: false,
                            reason: "user_cancelled"
                        )))
                    }
                }
            }
            source.beginTurnForTesting(turnId: 18, sessionId: "session-a")

            source.resumeSession("session-b")
            await waitForSubmittedCommands(1, commands: submittedCommands)
            source.resumeSession("session-c")
            await flushTasks()
            releaseCancellation?.resume()
            releaseCancellation = nil
            await waitForSubmittedCommands(2, commands: submittedCommands)
            await flushTasks(8)

            XCTAssertEqual(submittedCommands.count, 2)
            guard submittedCommands.count == 2 else { return }
            if case let .cancel(turnId) = submittedCommands[0] {
                XCTAssertEqual(turnId, 18)
            } else {
                XCTFail("the shared cancellation must be submitted first")
            }
            if case let .resumeSession(sessionId, cwd) = submittedCommands[1] {
                XCTAssertEqual(sessionId, "session-c")
                XCTAssertNil(cwd)
            } else {
                XCTFail("only the latest resume target may be submitted")
            }
        }

        func testNewSessionSerializesCancelBeforeSessionTransition() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
                if case let .cancel(turnId?) = command {
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "session-a",
                        turnId: turnId,
                        state: .cancelled,
                        firstSequence: 0,
                        lastSequence: 0,
                        safeToResume: false,
                        reason: "user_cancelled"
                    )))
                }
            }
            source.beginTurnForTesting(turnId: 23, sessionId: "session-a")

            source.startNewConversation()
            await waitForSubmittedCommands(2, commands: submittedCommands)

            XCTAssertEqual(submittedCommands.count, 2)
            guard submittedCommands.count == 2 else { return }
            if case let .cancel(turnId) = submittedCommands[0] {
                XCTAssertEqual(turnId, 23)
            } else {
                XCTFail("the old turn must be cancelled first")
            }
            if case let .newSession(cwd, model) = submittedCommands[1] {
                XCTAssertNil(cwd)
                XCTAssertNil(model)
            } else {
                XCTFail("newSession must follow the awaited cancel")
            }
        }

        // MARK: - Engine-derived tool presentation
        //
        // The engine derives the header / result block ONCE and ships them on
        // additive wire fields. These assert the reducer CARRIES them instead of
        // re-deriving anything from `input_json` / `result_json`.

        private func sampleHeader() -> ToolHeaderDto {
            ToolHeaderDto(
                verb: .update,
                icon: .edit,
                label: "Update",
                primary: "src/host.rs",
                qualifier: " (3 edits)",
                count: nil,
                subLine: nil,
                title: "Update(src/host.rs) (3 edits)"
            )
        }

        private func sampleDiff() -> StructuredDiffDto {
            StructuredDiffDto(
                filePath: "src/host.rs",
                language: "rust",
                gutterWidth: 3,
                additions: 1,
                removals: 1,
                truncatedRows: 2,
                rows: [
                    DiffRowDto(
                        kind: .remove, lineNo: 41, hunk: 0, wordDiffed: true,
                        segments: [
                            CodeSegmentDto(text: "let ", class: .keyword, rgb: nil,
                                           bold: false, italic: false, underline: false, emph: false),
                            CodeSegmentDto(text: "old", class: .variable, rgb: 0x00FF_0000,
                                           bold: false, italic: false, underline: false, emph: true),
                        ]
                    ),
                    DiffRowDto(
                        kind: .add, lineNo: 41, hunk: 1, wordDiffed: false,
                        segments: [
                            CodeSegmentDto(text: "let new", class: .plain, rgb: nil,
                                           bold: true, italic: false, underline: false, emph: false),
                        ]
                    ),
                ]
            )
        }

        func testToolUseStartedCarriesTheEngineDerivedHeader() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 60, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 60))

            source.applyForTesting(.toolUseStarted(
                id: "edit-1",
                tool: "MultiEdit",
                inputJson: #"{"file_path":"src/host.rs"}"#,
                header: sampleHeader()
            ))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            guard let header = run.tools.first?.header else {
                return XCTFail("the derived header must survive onto the trace")
            }
            XCTAssertEqual(header.verb, .update)
            XCTAssertEqual(header.primary, "src/host.rs")
            XCTAssertEqual(header.qualifier, " (3 edits)")
            XCTAssertEqual(header.title, "Update(src/host.rs) (3 edits)")
            XCTAssertEqual(header.icon, .some(.edit))
            // The verb is what a localizing client renders — never the English
            // `label` that rides along for non-localizing surfaces.
            XCTAssertEqual(
                ToolDisplayText.verbLabel(header),
                String(localized: "chat_tool_verb_update"))
        }

        func testToolUseResultCarriesTheEngineDerivedDisplay() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 61, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 61))
            source.applyForTesting(.toolUseStarted(
                id: "edit-1", tool: "Edit",
                inputJson: #"{"file_path":"src/host.rs"}"#, header: sampleHeader()))
            source.applyForTesting(.toolUseResult(
                id: "edit-1",
                tool: "Edit",
                resultJson: #"{"result":"ok"}"#,
                isError: false,
                display: ToolResultDisplayDto(
                    headline: "Added 18 lines, removed 4 lines",
                    headlineKind: .addedRemoved,
                    headlineArgs: [18, 4],
                    diff: sampleDiff(),
                    body: "line a\nline b",
                    bodyLines: 42,
                    bodyTruncated: true,
                    collapsed: true
                )
            ))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            guard let display = run.tools.first?.display else {
                return XCTFail("the result display must survive onto the trace")
            }
            XCTAssertEqual(display.headlineKind, .addedRemoved)
            XCTAssertEqual(display.headlineArgs, [18, 4])
            XCTAssertEqual(display.bodyLines, 42)
            XCTAssertTrue(display.bodyTruncated)
            XCTAssertTrue(display.collapsed, "an over-budget body renders collapsed")

            // Localized from the KIND + args, not from the English headline.
            XCTAssertEqual(
                ToolDisplayText.headline(display),
                String(localized: "chat_result_added_removed \(18) \(4)"))

            // The structured diff arrives PRE-SPLIT: concatenating a row's
            // segments reproduces the line exactly, with no offsets on the wire.
            guard let diff = display.diff else { return XCTFail("expected a diff") }
            XCTAssertEqual(diff.gutterWidth, 3)
            XCTAssertEqual(diff.truncatedRows, 2)
            XCTAssertEqual(diff.rows.count, 2)
            XCTAssertEqual(diff.rows[0].segments.map(\.text).joined(), "let old")
            XCTAssertEqual(diff.rows[0].kind, .remove)
            XCTAssertEqual(diff.rows[0].segments[0].syntax, .keyword)
            XCTAssertTrue(diff.rows[0].segments[1].emph,
                          "a word-diffed run keeps its emphasis flag")
            XCTAssertEqual(diff.rows[0].segments[1].rgb, 0x00FF_0000)
            // A hunk index CHANGE between consecutive rows is where the `⋯`
            // separator belongs; there is no separator row kind.
            XCTAssertNotEqual(diff.rows[0].hunk, diff.rows[1].hunk)
            XCTAssertTrue(diff.rows[1].segments[0].bold)
        }

        func testEngineOverriddenLabelSurvivesLocalization() {
            // `REPL` and a named subagent share a verb with a canonical English
            // label but override it with a proper noun. That override must be
            // rendered verbatim — the localized verb would erase it.
            let repl = ConversationToolHeader(
                verb: .shell, label: "REPL", primary: nil, qualifier: nil,
                count: nil, subLine: nil, title: "REPL")
            XCTAssertEqual(ToolDisplayText.verbLabel(repl), "REPL")

            let subagent = ConversationToolHeader(
                verb: .task, label: "code-reviewer", primary: "review the diff",
                qualifier: nil, count: nil, subLine: nil,
                title: "code-reviewer(review the diff)")
            XCTAssertEqual(ToolDisplayText.verbLabel(subagent), "code-reviewer")

            // A shell call that DID count is localized with its count.
            let bash = ConversationToolHeader(
                verb: .shell, label: "Running 1 shell command…", primary: nil,
                qualifier: nil, count: 1,
                subLine: ConversationToolSubLine(prefix: "$", text: "cargo test"),
                title: "Running 1 shell command…")
            XCTAssertEqual(
                ToolDisplayText.verbLabel(bash),
                String(localized: "chat_tool_verb_shell \(1)"))
            XCTAssertNotEqual(ToolDisplayText.verbLabel(bash), bash.label)
            XCTAssertEqual(bash.icon(for: "bash"), .terminal)
            let legacy = ConversationToolHeader(
                verb: .generic, label: "custom", primary: nil, qualifier: nil,
                count: nil, subLine: nil, title: "custom")
            XCTAssertEqual(legacy.icon(for: "WebSearch"), .globe)
        }

        func testPlanUpdatedReplacesTheWholeListAndAnEmptyListClearsIt() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 62, sessionId: "session-a")

            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: "1", subject: "写代码", activeForm: "正在写代码", state: .inProgress),
                PlanTaskDto(id: nil, subject: "跑测试", activeForm: nil, state: .pending),
            ]))
            XCTAssertEqual(source.model.planTasks.count, 2)
            XCTAssertEqual(source.model.planTasks[0].state, .inProgress)
            XCTAssertEqual(source.model.planTasks[0].taskId, "1")
            XCTAssertEqual(source.model.planTasks[1].id, "跑测试",
                           "a TodoWrite V1 item has no id and falls back to its subject")

            // FULL-LIST replace, not a merge.
            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: "1", subject: "写代码", activeForm: nil, state: .completed),
            ]))
            XCTAssertEqual(source.model.planTasks.count, 1)
            XCTAssertEqual(source.model.planTasks[0].state, .completed)

            // An empty list CLEARS the plan — it is never "no news".
            source.applyForTesting(.planUpdated(tasks: []))
            XCTAssertTrue(source.model.planTasks.isEmpty)
        }

        func testPlanUpdatedIsNotDroppedOutsideATurn() {
            // The plan block outlives the turn that wrote it; the turn gate must
            // allowlist it exactly like TaskRow / AskUserQuestion.
            let source = makeSource()
            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: nil, subject: "落库", activeForm: nil, state: .pending),
            ]))
            XCTAssertEqual(source.model.planTasks.count, 1)
        }

        func testSessionResumeClearsThePlanAndTheExpandedRows() {
            let source = makeSource()
            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: nil, subject: "旧计划", activeForm: nil, state: .pending),
            ]))
            source.model.expandedToolCalls = ["edit-1"]

            source.expectSessionResumeForTesting("session-z")
            source.applyForTesting(.sessionResumed(sessionId: "session-z", mode: .code, messages: []))

            XCTAssertTrue(source.model.planTasks.isEmpty,
                          "the plan belonged to the transcript we just replaced")
            XCTAssertTrue(source.model.expandedToolCalls.isEmpty)
        }

        func testOlderEngineWithoutHeaderOrDisplayKeepsTheLegacySummaries() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 63, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 63))
            source.applyForTesting(.toolUseStarted(
                id: "old-1", tool: "Read",
                inputJson: #"{"file_path":"/tmp/a"}"#, header: nil))
            source.applyForTesting(.toolUseResult(
                id: "old-1", tool: "Read",
                resultJson: #"{"message":"read 3 lines"}"#, isError: false, display: nil))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            XCTAssertNil(run.tools.first?.header)
            XCTAssertNil(run.tools.first?.display)
            XCTAssertEqual(run.tools.first?.inputSummary, "/tmp/a")
            XCTAssertEqual(run.tools.first?.outputSummary, "read 3 lines")
        }
    }

#endif

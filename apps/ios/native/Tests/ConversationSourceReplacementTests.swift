import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    import harness_runtimeFFI
#endif

@MainActor
final class ConversationSourceReplacementTests: XCTestCase {
    func testIdleRunningAndPendingTasksKeepOriginalSource() {
        let model = ConversationModel()
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        for status in [BackgroundTaskSnapshot.Status.pending, .running] {
            model.backgroundTasks = [BackgroundTaskSnapshot(id: "job", descriptionText: "build", status: status)]
            XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        }
        for status in [BackgroundTaskSnapshot.Status.paused, .completed, .failed, .cancelled] {
            model.backgroundTasks[0].status = status
            XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        }
    }

    func testLiveChildAgentsBlockReplacementWhileMainCanBeCancelled() {
        let model = ConversationModel()
        model.streaming = true
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        for status in ["running", "working", "pending", "queued", "initializing"] {
            model.replaceAgentSummaries([.main, ConversationAgentSummary(
                id: "child-\(status)", name: "worker", agentType: "worker", status: status
            )])
            XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        }
        model.replaceAgentSummaries([.main, ConversationAgentSummary(
            id: "finished", name: "worker", agentType: "worker", status: "completed"
        )])
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
    }

    func testWorkflowWorkersOutliveTerminalMainRun() {
        let model = ConversationModel()
        var run = ConversationExecutionRun(id: "run", sessionId: "session", turnId: 1, status: .completed)
        run.activeWorkers = 1
        model.items = [.run(run)]
        XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        run.activeWorkers = 0
        model.items = [.run(run)]
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
    }

    func testBackgroundArrivalDuringAwaitRequiresFreshDecision() async {
        let model = ConversationModel()
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        // Model the same actor suspension used by cancellation/persistence/
        // replacement preparation. The original source's callback can publish
        // newly admitted background work before the operation resumes.
        await Task { @MainActor in
            model.backgroundTasks = [BackgroundTaskSnapshot(id: "late-job", descriptionText: "build", status: .running)]
        }.value
        XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
    }

    func testRollbackOnlyRestoresTheSelectionOwnedByFailedSwitch() {
        XCTAssertTrue(ConversationSourceReplacementPolicy.shouldRollbackSelection(
            previousSourceIsCurrent: true, currentProjectID: "requested", requestedProjectID: "requested"
        ))
        XCTAssertFalse(ConversationSourceReplacementPolicy.shouldRollbackSelection(
            previousSourceIsCurrent: false, currentProjectID: "requested", requestedProjectID: "requested"
        ))
        XCTAssertFalse(ConversationSourceReplacementPolicy.shouldRollbackSelection(
            previousSourceIsCurrent: true, currentProjectID: "newer-selection", requestedProjectID: "requested"
        ))
    }

    func testScopeSwitchKeepsTheOriginallyPreservedRecoveryAndNewlyAdmittedRecovery() {
        let policy = ConversationSourceReplacementPolicy.self
        XCTAssertEqual(policy.scopeSwitchRecoveryDisposition(
            allowRecoveryRouting: false, hasInactiveDurableRecovery: false
        ), .retire)
        XCTAssertEqual(policy.scopeSwitchRecoveryDisposition(
            allowRecoveryRouting: true, hasInactiveDurableRecovery: true
        ), .preserve)
        // The recovery was already kept alive before an await. It must not
        // be retired even if its model changes while replacement prepares.
        XCTAssertEqual(policy.scopeSwitchRecoveryDisposition(
            allowRecoveryRouting: true, hasInactiveDurableRecovery: false, previouslyPreserved: true
        ), .preserve)
        // A recovery can also arrive while the old cancellation / new prepare
        // suspends, so a previously retiring switch must upgrade to preserve.
        XCTAssertEqual(policy.scopeSwitchRecoveryDisposition(
            allowRecoveryRouting: true, hasInactiveDurableRecovery: true, previouslyPreserved: false
        ), .preserve)
    }

    func testScopeSwitchRechecksRecoveryBeforeCommittingWithoutRouting() {
        let model = ConversationModel()
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsScopeSwitchCommit(
            of: model, allowRecoveryRouting: false
        ))
        model.hasInactiveDurableRecovery = true
        model.hasUnresolvedTurnRecovery = true
        XCTAssertFalse(ConversationSourceReplacementPolicy.allowsScopeSwitchCommit(
            of: model, allowRecoveryRouting: false
        ))
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsScopeSwitchCommit(
            of: model, allowRecoveryRouting: true
        ))
    }

    func testRetirementRequiresTheOldTurnAndSlashCommandToHaveSettled() {
        let model = ConversationModel()
        XCTAssertTrue(ConversationSourceReplacementPolicy.allowsRetiringRecoveryIngress(of: model))
        model.streaming = true
        XCTAssertFalse(ConversationSourceReplacementPolicy.allowsRetiringRecoveryIngress(of: model))
        model.streaming = false
        model.slashCommandPending = true
        XCTAssertFalse(ConversationSourceReplacementPolicy.allowsRetiringRecoveryIngress(of: model))
        model.slashCommandPending = false
        model.hasUnresolvedTurnRecovery = true
        XCTAssertFalse(ConversationSourceReplacementPolicy.allowsRetiringRecoveryIngress(of: model))
    }

    #if canImport(harness_runtimeFFI)
        private func makeRecoverySource(root: String, session: String) -> EngineConversationSource {
            let source = EngineConversationSource(
                config: EngineConfig(
                    apiBase: "https://api.anthropic.com",
                    apiKey: "",
                    model: "",
                    appSandboxRoot: root,
                    projectCwd: nil,
                    sessionMode: .code,
                    visionDelegationEnabled: true
                ),
                memoryOnly: true,
                recoveryStoreRootForTesting: root
            )
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            return source
        }

        private func recoveryState(session: String, revision: UInt64) -> ClientEvent {
            .turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: session,
                turnId: 71,
                revision: revision,
                state: .running,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                projectionUncertain: false,
                reason: nil
            ))
        }

        func testCommittedReplacementRetiresOldIngressBeforeInstallingNewSource() throws {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
            let session = "replacement-\(UUID().uuidString)"
            let previous = makeRecoverySource(root: root, session: session)
            let replacement = makeRecoverySource(root: root, session: session)
            // The cancellation barrier completed; retain the turn identity to
            // simulate a callback already queued on the old listener.
            previous.model.streaming = false
            defer { replacement.setSettingsActive(false) }
            var selected: any ConversationSource = previous
            var previousEvents = 0
            var replacementEvents = 0
            previous.setExternalEventHandler { _ in previousEvents += 1 }
            replacement.setExternalEventHandler { _ in replacementEvents += 1 }

            try ConversationSourceReplacementPolicy.commitReplacement(
                previous: previous,
                replacement: replacement,
                recoveryDisposition: .retire,
                validate: { XCTAssertTrue(selected === previous) },
                install: {
                    selected = replacement
                    // A reentrant callback cannot publish the old high-water
                    // revision, even before the installation returns.
                    previous.applyForTesting(self.recoveryState(session: session, revision: 9))
                }
            )
            XCTAssertTrue(selected === replacement)
            XCTAssertEqual(previousEvents, 0)
            replacement.applyForTesting(recoveryState(session: session, revision: 1))
            XCTAssertEqual(replacementEvents, 1, "the old source must not poison the shared revision")
        }

        func testPreservedBackgroundRecoveryStillReceivesItsEvents() throws {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
            let session = "background-\(UUID().uuidString)"
            let previous = makeRecoverySource(root: root, session: session)
            let replacement = MockConversationSource()
            previous.model.hasInactiveDurableRecovery = true
            let initial = ConversationSourceReplacementPolicy.scopeSwitchRecoveryDisposition(
                allowRecoveryRouting: true,
                hasInactiveDurableRecovery: previous.model.hasInactiveDurableRecovery
            )
            // Model the asynchronous preparation window: the initial
            // background owner must survive even if its visible flag changes.
            previous.model.hasInactiveDurableRecovery = false
            let committed = ConversationSourceReplacementPolicy.scopeSwitchRecoveryDisposition(
                allowRecoveryRouting: true,
                hasInactiveDurableRecovery: previous.model.hasInactiveDurableRecovery,
                previouslyPreserved: initial == .preserve
            )
            var previousEvents = 0
            previous.setExternalEventHandler { _ in previousEvents += 1 }
            var selected: any ConversationSource = previous
            try ConversationSourceReplacementPolicy.commitReplacement(
                previous: previous,
                replacement: replacement,
                recoveryDisposition: committed,
                validate: {},
                install: { selected = replacement }
            )
            XCTAssertTrue(selected === replacement)
            previous.applyForTesting(recoveryState(session: session, revision: 1))
            XCTAssertEqual(previousEvents, 1, "the headless owner keeps its independent recovery interest")
        }

        func testNewTurnDuringPreparationPreventsRetirement() {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
            let session = "new-turn-\(UUID().uuidString)"
            // A turn began on the previous source while replacement prepared.
            let previous = makeRecoverySource(root: root, session: session)
            let replacement = MockConversationSource()
            var selected: any ConversationSource = previous
            var previousEvents = 0
            previous.setExternalEventHandler { _ in previousEvents += 1 }

            XCTAssertThrowsError(try ConversationSourceReplacementPolicy.commitReplacement(
                previous: previous,
                replacement: replacement,
                recoveryDisposition: .retire,
                validate: {},
                install: { selected = replacement }
            ))
            XCTAssertTrue(selected === previous)
            previous.applyForTesting(recoveryState(session: session, revision: 1))
            XCTAssertEqual(previousEvents, 1)
        }

        func testRejectedReplacementLeavesOldRecoveryIngressActive() {
            enum ReplacementFailure: Error { case recoveryPending }
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
            let session = "failed-replacement-\(UUID().uuidString)"
            let previous = makeRecoverySource(root: root, session: session)
            let replacement = MockConversationSource()
            previous.model.hasInactiveDurableRecovery = true
            previous.model.hasUnresolvedTurnRecovery = true
            var previousEvents = 0
            previous.setExternalEventHandler { _ in previousEvents += 1 }
            var selected: any ConversationSource = previous

            XCTAssertThrowsError(try ConversationSourceReplacementPolicy.commitReplacement(
                previous: previous,
                replacement: replacement,
                recoveryDisposition: .retire,
                validate: {
                    guard ConversationSourceReplacementPolicy.allowsScopeSwitchCommit(
                        of: previous.model, allowRecoveryRouting: false
                    ) else { throw ReplacementFailure.recoveryPending }
                },
                install: { selected = replacement }
            ))
            XCTAssertTrue(selected === previous)
            previous.applyForTesting(recoveryState(session: session, revision: 1))
            XCTAssertEqual(previousEvents, 1)
        }

        func testBackgroundPermissionsKeepOwnerAndForegroundCancelRemainsPossible() {
            let model = ConversationModel()
            model.streaming = true
            model.pendingPermissions = [permission(worker: nil)]
            XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
            model.pendingPermissions = [permission(worker: WorkerInfoDto(name: "background", color: "background", team: nil))]
            XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
            model.streaming = false
            model.pendingPermissions = [permission(worker: nil)]
            XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model))
        }

        func testInactiveRecoveryRoutingExceptionDoesNotBypassBackgroundWork() {
            let model = ConversationModel()
            model.hasInactiveDurableRecovery = true
            model.hasUnresolvedTurnRecovery = true
            model.pendingPermissions = [permission(worker: nil)]
            XCTAssertTrue(ConversationSourceReplacementPolicy.allowsReplacement(of: model, allowInactiveRecovery: true))
            model.backgroundTasks = [BackgroundTaskSnapshot(id: "live", descriptionText: "build", status: .running)]
            XCTAssertFalse(ConversationSourceReplacementPolicy.allowsReplacement(of: model, allowInactiveRecovery: true))
        }

        private func permission(worker: WorkerInfoDto?) -> PendingPermission {
            PendingPermission(request: PermissionRequest(
                requestId: 1,
                kind: .toolUseConfirm(toolName: "Shell", toolInputJson: "{}", defaultAllow: false),
                worker: worker,
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            ))
        }
    #endif
}

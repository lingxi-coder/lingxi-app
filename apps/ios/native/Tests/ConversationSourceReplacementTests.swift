import XCTest

@testable import LingxiCode

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

    #if canImport(harness_runtimeFFI)
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

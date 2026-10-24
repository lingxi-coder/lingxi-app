import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class LocalAppAgentBackgroundContractTests: XCTestCase {
    func testWakeContextKeysIsolateSameRequestAcrossAppsAndRevisions() {
        let appA = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, requestID: "request-1")
        let appB = LocalAppAgentWakeRequest(appID: "app-b", revision: 7, requestID: "request-1")
        let nextRelease = LocalAppAgentWakeRequest(appID: "app-a", revision: 8, requestID: "request-1")
        let retry = LocalAppAgentWakeRequest(appID: "app-a", revision: 7, requestID: "request-1")

        XCTAssertNotEqual(appA.contextKey, appB.contextKey)
        XCTAssertNotEqual(appA.contextKey, nextRelease.contextKey)
        XCTAssertEqual(appA.contextKey, retry.contextKey)

        var activeContexts = [appA.contextKey: "task-a", appB.contextKey: "task-b"]
        activeContexts.removeValue(forKey: appA.contextKey) // expiry/clear of app-a
        XCTAssertNil(activeContexts[appA.contextKey])
        XCTAssertEqual(activeContexts[appB.contextKey], "task-b")
    }

    func testPermanentReleaseErrorsEndTheLeaseButStorageErrorsRemainRetryable() {
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("release_changed", "release changed")
        ))
        XCTAssertTrue(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("app_not_found", "app removed")
        ))
        XCTAssertFalse(LocalAppAgentBackgroundController.isPermanentMonitorFailure(
            LocalAppServiceFailure("agent_storage_unavailable", "try again")
        ))
    }

    func testObservedTurnCountDoesNotBecomeACompletionPercentage() {
        let progress = Progress(totalUnitCount: 0)
        LocalAppAgentProgressReporting.update(
            progress,
            completedModelTurns: 3,
            turnLimit: 20
        )

        XCTAssertTrue(progress.isIndeterminate)
        XCTAssertEqual(progress.totalUnitCount, -1)
        XCTAssertEqual(progress.completedUnitCount, 3)
        XCTAssertEqual(progress.fractionCompleted, 0)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Completed 3 model turns; limit 20")

        LocalAppAgentProgressReporting.finish(progress, success: true)
        XCTAssertEqual(progress.totalUnitCount, 3)
        XCTAssertEqual(progress.completedUnitCount, 3)
        XCTAssertEqual(progress.fractionCompleted, 1)
        XCTAssertEqual(progress.localizedAdditionalDescription, "Completed 3 model turns; limit 20")
    }
}

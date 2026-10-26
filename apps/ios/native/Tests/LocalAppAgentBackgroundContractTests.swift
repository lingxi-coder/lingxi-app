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

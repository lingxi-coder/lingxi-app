import XCTest

@testable import LingxiCode

final class AppIntegrationTests: XCTestCase {
    func testPendingActionsSurviveStoreRecreationAndDrainOnce() async {
        let suiteName = "AppIntegrationTests.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            return XCTFail("expected isolated defaults")
        }
        defer { defaults.removePersistentDomain(forName: suiteName) }

        let writer = LingxiAppActionStore(defaults: defaults)
        await writer.enqueue(.newConversation)
        await writer.enqueue(.ask("检查项目"))

        let reader = LingxiAppActionStore(defaults: defaults)
        let restored = await reader.drain()
        let drainedAgain = await reader.drain()

        XCTAssertEqual(restored, [.newConversation, .ask("检查项目")])
        XCTAssertTrue(drainedAgain.isEmpty, "an App Intent must be handled exactly once")
    }
}

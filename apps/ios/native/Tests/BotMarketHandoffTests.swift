import XCTest
@testable import LingxiCode
final class BotMarketHandoffTests: XCTestCase {
    func testBotHandoffHasDedicatedPinnedRouteWithoutPrivateConfiguration() throws {
        let intent = BotMarketHandoff(listingID: "travel-helper", versionID: "release1", stage: "awaiting_desktop")
        let url = try XCTUnwrap(intent.desktopURL)
        let target = try XCTUnwrap(CanonicalDeepLink.parse(url))
        XCTAssertEqual(target.route, "market/bot/travel-helper")
        XCTAssertEqual(target.params, ["versionId": "release1"])
        XCTAssertFalse(target.needsContextRebind(workspace: "project.private", mode: "code"))
        for raw in ["lingxi://market/bot/helper?token=secret", "lingxi://market/bot/helper?workspace=private", "lingxi://market/app1?versionId=release1"] { XCTAssertNil(CanonicalDeepLink.parse(raw), raw) }
        XCTAssertNil(BotMarketHandoff(listingID: "../private", versionID: nil, stage: "awaiting_desktop").desktopURL)
    }
}

import XCTest
@testable import LingxiCode
final class BotMarketHandoffTests: XCTestCase {
    func testBundledBotRootIsTheCanonicalDirectEndpoint() throws {
        let url = try XCTUnwrap(Bundle.main.url(forResource: "product-config", withExtension: "json"))
        let config = try JSONDecoder().decode(ProductMarketConfig.self, from: Data(contentsOf: url))
        XCTAssertEqual(config.botMarketURL?.absoluteString, "https://marketgateway-pacws5zsoa-uc.a.run.app/api/v3/")
    }

    func testBotAPIConfigUsesExplicitRootAndPreservesAppRoot() throws {
        let config = try JSONDecoder().decode(ProductMarketConfig.self, from: Data(#"{"localAppMarketApiRoot":"https://website.test/api/v2","botMarketApiRoot":"https://direct.test/api/v3/"}"#.utf8))
        XCTAssertEqual(config.localAppMarketApiRoot, "https://website.test/api/v2")
        XCTAssertEqual(config.botMarketURL?.absoluteString, "https://direct.test/api/v3/")
        XCTAssertThrowsError(try JSONDecoder().decode(ProductMarketConfig.self, from: Data(#"{"localAppMarketApiRoot":"https://website.test/api/v2"}"#.utf8)))
    }

    func testBotAPIRootRejectsAmbiguousOrCredentialedEndpoints() {
        XCTAssertNotNil(BotMarketHandoff.validatedAPIRoot("https://fixture.test/api/v3/"))
        for raw in ["http://fixture.test/api/v3/", "https://user:secret@fixture.test/api/v3/", "https://fixture.test/api/v2/", "https://fixture.test/api/v3", "https://fixture.test/api/v3/extra", "https://fixture.test/api/v3/?x=1", "https://fixture.test/api/v3/#part", "https://fixture.test/api/%763/"] {
            XCTAssertNil(BotMarketHandoff.validatedAPIRoot(raw), raw)
        }
    }

    func testBotRedirectPolicyRejectsSameOriginAndCrossOriginWhileAppDefaultRemains() throws {
        let root = try XCTUnwrap(URL(string: "https://fixture.test/api/v3/"))
        let bot = try MarketAPI(baseURL: root, followsRedirects: false)
        let app = try MarketAPI(baseURL: root)
        let task = URLSession.shared.dataTask(with: URLRequest(url: root))
        let response = try XCTUnwrap(HTTPURLResponse(url: root, statusCode: 302, httpVersion: nil, headerFields: nil))
        for destination in ["https://fixture.test/api/v3/elsewhere", "https://other.test/api/v3/"] {
            bot.urlSession(.shared, task: task, willPerformHTTPRedirection: response, newRequest: URLRequest(url: URL(string: destination)!)) { XCTAssertNil($0) }
        }
        app.urlSession(.shared, task: task, willPerformHTTPRedirection: response, newRequest: URLRequest(url: root)) { XCTAssertEqual($0?.url, root) }
    }

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

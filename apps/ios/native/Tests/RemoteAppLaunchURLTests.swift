import XCTest
@testable import LingxiCode

final class RemoteAppLaunchURLTests: XCTestCase {
    func testPresentedBrowserSurvivesConnectionLossAndStaleLaunchDoesNotReplaceIt() {
        let original = URL(string: "https://first.example/#ticket")!
        let stale = URL(string: "https://second.example/#ticket")!
        var browser = RemoteAppBrowserLifetime()
        browser.present(original, generation: browser.generation)
        let originalID = browser.ticket?.id
        let pending = browser.generation

        // A catalog disappears on disconnect, but its presented browser remains.
        browser.invalidatePending()
        browser.present(stale, generation: pending)
        XCTAssertEqual(browser.ticket?.id, originalID)
        XCTAssertEqual(browser.ticket?.url, original)

        browser.dismiss()
        XCTAssertNil(browser.ticket)
        browser.present(stale, generation: browser.generation)
        XCTAssertEqual(browser.ticket?.url, stale)
    }

    func testOnlyUnexpiredHTTPSLaunchURLsAreAccepted() {
        let now = Date(timeIntervalSince1970: 1_000)
        XCTAssertNotNil(RemoteAppLaunchURL.validate("https://a0123456789abcdef0123456789abcdef-0123456789abcdef0123.desktop.apps.example.com/__lingxi_remote_bootstrap#ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq", expiresAt: 1_001_000, now: now))
        for raw in ["http://example.com/app", "https://user:pass@example.com", "https://example.com/", "https:///missing-host", "javascript:alert(1)"] {
            XCTAssertNil(RemoteAppLaunchURL.validate(raw, expiresAt: 1_001_000, now: now))
        }
        XCTAssertNil(RemoteAppLaunchURL.validate("https://example.com/#ticket", expiresAt: 1_000_000, now: now))
    }
}

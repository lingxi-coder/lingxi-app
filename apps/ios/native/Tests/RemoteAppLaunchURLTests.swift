import XCTest
@testable import LingxiCode

final class RemoteAppLaunchURLTests: XCTestCase {
    func testOnlyUnexpiredHTTPSLaunchURLsAreAccepted() {
        let now = Date(timeIntervalSince1970: 1_000)
        XCTAssertNotNil(RemoteAppLaunchURL.validate("https://a0123456789abcdef0123456789abcdef-0123456789abcdef0123.desktop.apps.example.com/__lingxi_remote_bootstrap#ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq", expiresAt: 1_001_000, now: now))
        for raw in ["http://example.com/app", "https://user:pass@example.com", "https://example.com/", "https:///missing-host", "javascript:alert(1)"] {
            XCTAssertNil(RemoteAppLaunchURL.validate(raw, expiresAt: 1_001_000, now: now))
        }
        XCTAssertNil(RemoteAppLaunchURL.validate("https://example.com/#ticket", expiresAt: 1_000_000, now: now))
    }
}

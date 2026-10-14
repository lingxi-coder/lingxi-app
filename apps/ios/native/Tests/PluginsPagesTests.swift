@testable import LingxiCode
import XCTest

final class PluginsPagesTests: XCTestCase {
    func testTheLocalAppBuilderHasItsOwnIcon() {
        XCTAssertEqual(pluginIcon("local-app-builder@lingxi"), .appBuilder)
        XCTAssertEqual(pluginIcon("local-app-builder"), .appBuilder)
        XCTAssertEqual(pluginIcon("notes@acme"), .plug)
    }

    func testRowsShowVendorAndVersion() {
        XCTAssertEqual(pluginSubtitle(vendor: "LingXi", version: "2.1.0"), "LingXi · 2.1.0")
        XCTAssertEqual(pluginSubtitle(vendor: nil, version: "2.1.0"), "2.1.0")
        XCTAssertEqual(pluginSubtitle(vendor: "LingXi", version: ""), "LingXi")
        XCTAssertEqual(pluginSubtitle(vendor: " ", version: ""), "")
    }
}

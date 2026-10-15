import XCTest
import SwiftUI
import UIKit
@testable import LingxiCode

@MainActor
final class NativeAppearanceTests: XCTestCase {
    func testSystemAppearanceTracksTheEffectiveScheme() {
        let name = "NativeAppearanceTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defer { defaults.removePersistentDomain(forName: name) }
        let app = AppState(defaults: defaults)
        XCTAssertEqual(app.themeRaw, "system")
        XCTAssertNil(app.colorScheme)
        app.systemIsDark = false
        XCTAssertFalse(app.palette.isDark)
        app.systemIsDark = true
        XCTAssertTrue(app.palette.isDark)
    }

    func testExplicitAppearancePersistsAndOverridesTheSystem() {
        let name = "NativeAppearanceTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defer { defaults.removePersistentDomain(forName: name) }
        let app = AppState(defaults: defaults)
        app.setTheme("light")
        app.systemIsDark = true
        XCTAssertFalse(app.isDark)
        XCTAssertEqual(app.colorScheme, .light)
        XCTAssertEqual(AppState(defaults: defaults).themeRaw, "light")
        app.setTheme("dark")
        app.systemIsDark = false
        XCTAssertTrue(app.isDark)
        XCTAssertEqual(app.colorScheme, .dark)
        app.setTheme("system")
        XCTAssertNil(app.colorScheme)
        XCTAssertFalse(app.isDark)
    }

    func testEverySharedIconHasAnAvailableSystemSymbol() {
        for icon in LXIconName.allCases {
            XCTAssertNotNil(UIImage(systemName: icon.systemName), icon.rawValue)
        }
    }
}

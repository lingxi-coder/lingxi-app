import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class LocalizationManagerTests: XCTestCase {
    func testNativeStringsSwitchImmediatelyAndRestoreSystemLanguage() {
        withManager(systemLanguages: ["en-GB"]) { manager, defaults, suite in
            XCTAssertEqual(String(localized: "common_cancel"), "Cancel")
            XCTAssertEqual(LocalizationManager.supported.first?.label, "Follow System")

            manager.language = "ja-JP"
            XCTAssertEqual(String(localized: "common_cancel"), "キャンセル")
            XCTAssertEqual(String(localized: "chat_switch_model_failed \("test")"), "モデルの切り替えに失敗しました：test")
            XCTAssertEqual(LocalizationManager.supported.first?.label, "システムに従う")
            XCTAssertEqual(defaults.array(forKey: "AppleLanguages") as? [String], ["ja"])

            manager.language = ""
            XCTAssertEqual(String(localized: "common_cancel"), "Cancel")
            XCTAssertNil(defaults.persistentDomain(forName: suite)?["AppleLanguages"])
            XCTAssertNil(defaults.persistentDomain(forName: suite)?["appLanguage"])
        }
    }

    func testNativeInterpolationPreservesPercentSignsStringsAndLargeIntegers() {
        withManager(systemLanguages: ["en"]) { _, _, _ in
            let percent = 42
            let seconds = 123
            XCTAssertEqual(
                String(localized: "chat_compaction_progress \(percent) \(seconds)"),
                "Organizing context: 42% (123 s)"
            )
            let error = "Disk 100% full"
            XCTAssertEqual(
                String(localized: "session_index_save_new_failed \(error)"),
                "Failed to save new session index: Disk 100% full"
            )
            let count = Int(Int32.max) + 1
            XCTAssertEqual(
                String(localized: "project_corrupt_projects_message \(count)"),
                "2,147,483,648 project(s) had corrupted data or an invalid path and were quarantined; other projects remain usable."
            )
        }
    }

    func testSystemLanguageMatchingUsesScriptAndNextSupportedPreference() {
        withManager(systemLanguages: ["zh-HK"]) { _, _, _ in
            XCTAssertEqual(Bundle._lxCurrentLanguageID.withLock { $0 }, "zh-Hant")
        }
        withManager(systemLanguages: ["fr-FR", "ko-KR"]) { _, _, _ in
            XCTAssertEqual(Bundle._lxCurrentLanguageID.withLock { $0 }, "ko")
        }
    }

    func testLanguageOverrideDoesNotRedirectOtherBundlesOrTables() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let resources = directory.appendingPathComponent("Vendor.bundle/en.lproj")
        try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        try "\"common_cancel\" = \"Vendor Cancel\";".write(
            to: resources.appendingPathComponent("Localizable.strings"), atomically: true, encoding: .utf8
        )
        let vendor = try XCTUnwrap(Bundle(path: resources.deletingLastPathComponent().path))
        withManager(systemLanguages: ["en"]) { manager, _, _ in
            manager.language = "ja-JP"
            XCTAssertEqual(vendor.localizedString(forKey: "common_cancel", value: nil, table: nil), "Vendor Cancel")
            XCTAssertEqual(
                Bundle.main.localizedString(forKey: "common_cancel", value: "Other table fallback", table: "MissingTable"),
                "Other table fallback"
            )
        }
    }

    private func withManager(
        systemLanguages: [String],
        _ body: (LocalizationManager, UserDefaults, String) -> Void
    ) {
        let suite = "LocalizationManagerTests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        let previousLanguage = Bundle._lxCurrentLanguageID.withLock { $0 }
        defer {
            Bundle._lxCurrentLanguageID.withLock { $0 = previousLanguage }
            defaults.removePersistentDomain(forName: suite)
        }
        body(LocalizationManager(defaults: defaults, systemLanguages: { systemLanguages }), defaults, suite)
    }
}

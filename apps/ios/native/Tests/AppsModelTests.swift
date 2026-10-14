import XCTest

@testable import LingxiCode

final class AppsModelTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_700_000_000)

    func testMyAppsPutsTheLastOpenedFirstThenSortsByName() {
        let info: [String: InstalledAppInfo] = [
            "a": InstalledAppInfo(appId: "a", name: "Beta", lastOpenedAt: t0),
            "b": InstalledAppInfo(appId: "b", name: "alpha"),
            "c": InstalledAppInfo(appId: "c", name: "Gamma", lastOpenedAt: t0.addingTimeInterval(10)),
        ]
        let apps = AppsModel.myApps(installedIDs: ["a", "b", "c", "d"], info: info, updatable: [], operations: [:])
        XCTAssertEqual(apps.map(\.appId), ["c", "a", "b", "d"])
        // Nothing known about "d" yet: it is shown by its id.
        XCTAssertEqual(apps.last?.name, "d")
    }

    func testMyAppsMarksUpdatesAndOnlyRunningOperations() {
        let apps = AppsModel.myApps(
            installedIDs: ["a", "b", "c"],
            info: [:],
            updatable: ["a"],
            operations: ["b": .downloading(0.5), "c": .failed("HTTP_503")]
        )
        let byID = Dictionary(uniqueKeysWithValues: apps.map { ($0.appId, $0) })
        XCTAssertEqual(byID["a"]?.updateAvailable, true)
        XCTAssertEqual(byID["b"]?.activeOperation, .downloading(0.5))
        XCTAssertNil(byID["c"]?.activeOperation)
    }

    func testRecentAppsAreTheLastThreeOpened() {
        func app(_ id: String, _ offset: TimeInterval?) -> MyApp {
            MyApp(appId: id, name: id, category: "", lastOpenedAt: offset.map(t0.addingTimeInterval),
                  updateAvailable: false, activeOperation: nil)
        }
        let apps = [app("a", 1), app("b", nil), app("c", 3), app("d", 4), app("e", 2)]
        XCTAssertEqual(AppsModel.recentApps(apps).map(\.appId), ["d", "c", "e"])
    }

    func testMatchingAppsAndInitials() {
        let apps = AppsModel.myApps(
            installedIDs: ["a", "b"],
            info: ["a": InstalledAppInfo(appId: "a", name: "Water Tracker"), "b": InstalledAppInfo(appId: "b", name: "记账本")],
            updatable: [],
            operations: [:]
        )
        XCTAssertTrue(AppsModel.matchingApps(apps, query: " ").isEmpty)
        XCTAssertEqual(AppsModel.matchingApps(apps, query: "water").map(\.appId), ["a"])
        XCTAssertEqual(AppsModel.matchingApps(apps, query: "账").map(\.appId), ["b"])
        XCTAssertEqual(AppsModel.initial("记账本"), "记")
        XCTAssertEqual(AppsModel.initial("  calculator"), "C")
        XCTAssertEqual(AppsModel.initial(" "), "?")
    }

    func testActionFollowsInstallStateVersionsAndOperations() {
        func action(_ current: (String, Int)?, installed: Int?, update: String? = nil, op: AppOperation? = nil) -> AppAction {
            AppsModel.action(currentVersionID: current?.0, currentSequence: current?.1,
                             installedSequence: installed, updateVersionID: update, operation: op)
        }
        XCTAssertEqual(action(("v1", 1), installed: nil), .get)
        XCTAssertEqual(action(nil, installed: nil), .unavailable)
        XCTAssertEqual(action(("v1", 1), installed: 1), .open)
        XCTAssertEqual(action(("v2", 2), installed: 1), .update(versionID: "v2"))
        XCTAssertEqual(action(("v2", 2), installed: 1, update: "v3"), .update(versionID: "v3"))
        XCTAssertEqual(action(("v1", 1), installed: nil, op: .downloading(0.25)), .downloading(0.25))
        XCTAssertEqual(action(("v1", 1), installed: nil, op: .installing), .installing)
        XCTAssertEqual(action(("v1", 1), installed: 1, op: .removing), .removing)
        // A failed operation leaves the button as it was.
        XCTAssertEqual(action(("v1", 1), installed: nil, op: .failed("HASH_MISMATCH")), .get)
    }
}

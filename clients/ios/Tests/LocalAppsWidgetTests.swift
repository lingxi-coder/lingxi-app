import XCTest

@testable import LingxiCode

final class LocalAppsWidgetTests: XCTestCase {
    func testWidgetSnapshotRejectsInvalidAppIdentifier() {
        XCTAssertTrue(LocalAppWidgetSnapshotStore.isValidAppID("tracker-1"))
        XCTAssertFalse(LocalAppWidgetSnapshotStore.isValidAppID("Tracker"))
        XCTAssertFalse(LocalAppWidgetSnapshotStore.isValidAppID("../tracker"))
    }

    func testWidgetURLUsesUnifiedOpenLocalAppRoute() {
        let url = LocalAppWidgetSnapshotStore.makeOpenURL(appID: "tracker-1")

        XCTAssertEqual(
            url?.absoluteString,
            "lingxi://open_local_app?appId=tracker-1&destination=preview&autostart=1&source=widget"
        )
    }

    func testSnapshotLoadRejectsUnsupportedVersion() throws {
        let fileURL = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("json")
        let payload = #"{"version":2,"apps":[]}"#.data(using: .utf8)!
        try payload.write(to: fileURL)

        XCTAssertEqual(LocalAppWidgetSnapshotStore.load(from: fileURL), .empty)
    }

    func testSnapshotLoadDropsInvalidAndDuplicateAppIdentifiers() throws {
        let fileURL = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("json")
        let payload = #"""
        {"version":1,"apps":[
            {"id":"tracker-1","name":"First","brief":"","workflow":"ready","runtimeState":"stopped","updatedAtMs":1},
            {"id":"tracker-1","name":"Duplicate","brief":"","workflow":"ready","runtimeState":"stopped","updatedAtMs":2},
            {"id":"../unsafe","name":"Unsafe","brief":"","workflow":"ready","runtimeState":"stopped","updatedAtMs":3}
        ]}
        """#.data(using: .utf8)!
        try payload.write(to: fileURL)

        let snapshot = LocalAppWidgetSnapshotStore.load(from: fileURL)

        XCTAssertEqual(snapshot.apps.map(\.id), ["tracker-1"])
        XCTAssertEqual(snapshot.apps.first?.name, "First")
    }

    func testSnapshotWriteFailsClosedWhenAppGroupIsMissing() {
        XCTAssertThrowsError(try LocalAppWidgetSnapshotStore.write(.empty, to: nil)) { error in
            XCTAssertEqual(
                error as? LocalAppWidgetSnapshotStore.SnapshotError,
                .containerUnavailable
            )
        }
    }
}

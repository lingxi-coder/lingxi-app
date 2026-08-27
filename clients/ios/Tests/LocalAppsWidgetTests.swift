import Foundation
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

    // ── The permanent entry ───────────────────────────────────────────────

    /// The widget request's PERMANENT entry lives on the app detail screen,
    /// and is offered only for a FORMED app.
    ///
    /// Asserted against the view's source because this suite renders no
    /// SwiftUI: the entry is a `Button` inside a `Menu`, and nothing here can
    /// observe which items a detail screen offers. What it can pin is the
    /// wiring — and the wiring is the half that actually broke. The widget
    /// request used to exist ONLY as a toggle inside the create form, so
    /// deleting that form retired the feature outright while every
    /// store-side test stayed green: `requestWidgetSetup` was still correct,
    /// still tested, and no longer reachable from anywhere in the product.
    /// `LocalAppsStoreTests.testRequestWidgetSetupArmsAFormedAppAndRefusesAShell`
    /// covers what happens when the entry is tapped; this covers that there
    /// IS one.
    ///
    /// The `scaffolded` gate is asserted separately from the store's own
    /// refusal, and is not redundant with it: the store refusing a shell is
    /// what keeps a placeholder-named, un-openable icon off the home screen,
    /// while the gate here is what keeps the user from being offered a menu
    /// item that silently does nothing when tapped.
    func testTheDetailScreenOffersTheWidgetEntryOnlyForAFormedApp() throws {
        let source = try clientSource("Sources/LocalApps/LocalAppDetailView.swift")

        // Prove the needle-bearing file was actually read before trusting any
        // assertion made against its contents.
        XCTAssertTrue(
            source.contains("struct LocalAppDetailView"),
            "read the wrong file — every assertion below would be vacuous")

        XCTAssertTrue(
            source.contains("store.requestWidgetSetup("),
            "the detail screen no longer asks for widget setup: the home-screen "
                + "widget has no entry left in the product")
        XCTAssertTrue(
            source.contains("\"local-apps.detail.add-widget\""),
            "the entry lost the identifier the UI suite reaches it by")

        XCTAssertTrue(
            isEnclosed(
                needle: "store.requestWidgetSetup(",
                byBlockOpenedBy: "if app.scaffolded {",
                in: source),
            "the widget entry must be offered only for a formed app — a shell "
                + "would get a menu item the store refuses to act on")
    }

    // ── Source-level helpers ──────────────────────────────────────────────

    /// A file from the iOS client tree, located relative to THIS test file so
    /// the lookup cannot drift with the working directory the runner happens
    /// to use. Throws (fails the test) if the file is gone — an unreadable
    /// source must never read as "the assertion holds".
    private func clientSource(_ relativePath: String) throws -> String {
        let clientRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // clients/ios
        return try String(
            contentsOf: clientRoot.appendingPathComponent(relativePath),
            encoding: .utf8)
    }

    /// Whether `needle` appears inside the braced block that some occurrence of
    /// `opener` starts — `opener` ending at that block's `{`.
    ///
    /// Brace-matched rather than line-counted, so moving the entry around
    /// inside its gate keeps passing while removing the gate, or moving the
    /// entry out from under it, fails.
    private func isEnclosed(
        needle: String,
        byBlockOpenedBy opener: String,
        in source: String
    ) -> Bool {
        var searchFrom = source.startIndex
        while let opened = source.range(of: opener, range: searchFrom..<source.endIndex) {
            var depth = 0
            var cursor = source.index(before: opened.upperBound) // the `{`
            var blockEnd: String.Index?
            while cursor < source.endIndex {
                switch source[cursor] {
                case "{": depth += 1
                case "}":
                    depth -= 1
                    if depth == 0 { blockEnd = cursor }
                default: break
                }
                if blockEnd != nil { break }
                cursor = source.index(after: cursor)
            }
            if let blockEnd,
                source.range(of: needle, range: opened.upperBound..<blockEnd) != nil
            {
                return true
            }
            searchFrom = opened.upperBound
        }
        return false
    }
}

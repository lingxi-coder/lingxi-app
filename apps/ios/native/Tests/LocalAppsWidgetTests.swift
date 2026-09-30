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
            {"id":"tracker-1","name":"First","brief":"","workflow":"published_verified","runtimeState":"stopped","updatedAtMs":1},
            {"id":"tracker-1","name":"Duplicate","brief":"","workflow":"published_verified","runtimeState":"stopped","updatedAtMs":2},
            {"id":"../unsafe","name":"Unsafe","brief":"","workflow":"published_unverified","runtimeState":"stopped","updatedAtMs":3}
        ]}
        """#.data(using: .utf8)!
        try payload.write(to: fileURL)

        let snapshot = LocalAppWidgetSnapshotStore.load(from: fileURL)

        XCTAssertEqual(snapshot.apps.map(\.id), ["tracker-1"])
        XCTAssertEqual(snapshot.apps.first?.name, "First")
    }

    func testWidgetUsesPublicationStatesInsteadOfLegacyReady() throws {
        let source = try clientSource("Sources/LocalAppsWidget/LocalAppWidget.swift")

        XCTAssertFalse(source.contains("workflow == \"ready\""))
        XCTAssertTrue(source.contains("publishedUnverified"))
        XCTAssertTrue(source.contains("publishedVerified"))
        XCTAssertTrue(source.contains("state.isPublished"))
    }

    func testRootViewHostsPhase8ApprovalSheetsAtTheRootLevel() throws {
        let source = try clientSource("Sources/App/RootView.swift")

        XCTAssertTrue(source.contains(".sheet(item: localAppMcpProposalApprovalItem)"))
        XCTAssertTrue(source.contains("LocalAppMcpProposalApprovalSheet(store: localAppsStore, prompt: prompt)"))
    }

    /// The drawer/library `armLibraryFallback` split
    /// (`LocalAppsStoreTests.swift` pins the STORE's behavior for each
    /// literal bool) is otherwise asserted only against literals the test
    /// itself supplies — nothing pins which literal each real call site
    /// actually passes, so swapping the drawer's `false` for `true` at
    /// `RootView.swift` would leave every one of those tests green. Assert
    /// the two real call sites directly.
    func testArmLibraryFallbackCallSitesMatchTheirDocumentedSplit() throws {
        let rootView = try clientSource("Sources/App/RootView.swift")
        XCTAssertTrue(
            rootView.contains("createShellApp(armLibraryFallback: false)"),
            "the DRAWER's create must not arm the library fallback — it hands "
                + "the conversation over directly")

        let libraryView = try clientSource("Sources/LocalApps/LocalAppsLibraryView.swift")
        XCTAssertTrue(
            libraryView.contains("createShellApp(armLibraryFallback: true)"),
            "the LIBRARY's create must arm the fallback so a failure leaves "
                + "the user looking at the library rather than a chat that "
                + "never got its app")
    }

    func testApprovalSheetsStayScrollableAndDetentedForLargeTextAndKeyboard() throws {
        let sheet = try clientSource("Sources/LocalApps/LocalAppApprovalSheets.swift")

        XCTAssertTrue(sheet.contains("struct LocalAppMcpProposalApprovalSheet"))
        XCTAssertTrue(sheet.contains("NavigationStack"), "mcp-proposal sheet lost NavigationStack")
        XCTAssertTrue(sheet.contains("ScrollView"), "mcp-proposal sheet lost ScrollView")
        XCTAssertTrue(
            sheet.contains(".presentationDetents([.medium, .large])"),
            "mcp-proposal sheet lost its detents")
        XCTAssertTrue(
            sheet.contains(".interactiveDismissDisabled()"),
            "mcp-proposal sheet lost .interactiveDismissDisabled()")
        XCTAssertTrue(sheet.contains("accessibilityIdentifier(\"local-apps.mcp-proposal."))
    }

    func testMcpPagesKeepsGenericEditorAlongsideManagedLocalAppRestrictions() throws {
        let source = try clientSource("Sources/Settings/MCPPages.swift")

        let active = try clientSource("Sources/Settings/DesktopAdminPages.swift")
        XCTAssertTrue(active.contains("struct DesktopMCPAdminPage"))
        XCTAssertTrue(active.contains("managedMcpInventory(serverName:"))
        XCTAssertTrue(source.contains("struct MCPEditPage"))
        XCTAssertTrue(source.contains("struct ManagedLocalAppMCPEditPage"))
        XCTAssertTrue(source.contains("allowsMcpConfigurationEditing"))
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

    /// The widget request's permanent entry lives in the compact management
    /// sheet, and is offered only for a formed app.
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
    func testManagementSheetOffersTheWidgetEntryOnlyForAFormedApp() throws {
        let source = try clientSource("Sources/LocalApps/LocalAppDetailView.swift")

        // Prove the needle-bearing file was actually read before trusting any
        // assertion made against its contents.
        XCTAssertTrue(
            source.contains("struct LocalAppManagementSheet"),
            "read the wrong file — every assertion below would be vacuous")

        XCTAssertTrue(
            source.contains("store.requestWidgetSetup("),
            "the detail screen no longer asks for widget setup: the home-screen "
                + "widget has no entry left in the product")
        XCTAssertTrue(
            source.contains("\"local-apps.management.add-widget\""),
            "the entry lost the identifier the UI suite reaches it by")

        XCTAssertTrue(
            isEnclosed(
                needle: "store.requestWidgetSetup(",
                byBlockOpenedBy: "if app.scaffolded {",
                in: source),
            "the widget entry must be offered only for a formed app — a shell "
                + "would get a menu item the store refuses to act on")
    }

    func testLocalAppWorkspaceIsTheFocusedRepairSurface() throws {
        let root = try clientSource("Sources/App/RootView.swift")
        let workspace = try clientSource("Sources/LocalApps/LocalAppWorkspaceView.swift")

        XCTAssertTrue(root.contains("LocalAppWorkspaceView("))
        XCTAssertTrue(root.contains("navigation.focusDetail()"))
        XCTAssertTrue(root.contains("localAppConversationSheetBinding"))
        XCTAssertTrue(root.contains(".presentationDetents([.medium, .large])"))
        XCTAssertTrue(workspace.contains("LocalAppWebView("))
        XCTAssertTrue(workspace.contains("LocalAppManagementSheet("))
        XCTAssertTrue(workspace.contains("local-apps.workspace.sessions"))
    }

    func testTheDetailScreenHostsManagedMcpControlsInsideTheLocalAppFlow() throws {
        let source = try clientSource("Sources/LocalApps/LocalAppDetailView.swift")

        XCTAssertTrue(source.contains("case mcp"))
        XCTAssertTrue(source.contains("LocalAppMcpControlsCard("))
        XCTAssertTrue(source.contains("Toggle(\"\", isOn: serviceEnabled)"))
        XCTAssertTrue(source.contains("TextEditor(text: $goal)"))
        XCTAssertTrue(source.contains("store.setManagedMcpEnabled(appID: appID, enabled: enabled)"))
        XCTAssertTrue(source.contains("store.setManagedMcpToolEnabled("))
        XCTAssertTrue(source.contains("store.startManagedMcpAuthoring(appID: appID, userGoal: goal)"))
        XCTAssertFalse(source.contains("MCPConfigurationRepository"))
    }

    /// `.preview` is only ever pushed from `LocalAppDetailView` for the SAME
    /// app it is already showing (`LocalAppsLibraryView.swift`'s
    /// `onOpenPreview: { path.append(.preview(appID)) }`), so every route on
    /// a `LocalAppsRootView`'s stack at once shares one id — which is what
    /// the collapse-on-disappear `.onChange` below relies on.
    func testLocalAppsRouteAppIDMatchesEitherCase() {
        XCTAssertEqual(LocalAppsRoute.details("tracker").appID, "tracker")
        XCTAssertEqual(LocalAppsRoute.preview("tracker").appID, "tracker")
    }

    /// Mirrors Android's collapse in `LocalAppsViewModel.kt`
    /// (`appIdOnScreen()` against `liveIds`): without it, deleting the app
    /// currently open in `.details`/`.preview` — the delete confirmation is
    /// anchored on the LIST screen, not on those two routes, so a delete
    /// never pops the stack on its own — left the stack pushed on a route
    /// for an app that no longer exists.
    func testLocalAppsRootViewCollapsesItsPathWhenTheOnScreenAppDisappears() throws {
        let source = try clientSource("Sources/LocalApps/LocalAppsLibraryView.swift")

        guard let start = source.range(of: "struct LocalAppsRootView"),
              let end = source.range(of: "private func destination(", range: start.upperBound..<source.endIndex)
        else {
            return XCTFail("read the wrong file: LocalAppsRootView not found")
        }
        let body = source[start.lowerBound..<end.lowerBound]

        XCTAssertTrue(
            body.contains(".onChange(of: store.apps.map(\\.id))"),
            "the stack must watch the live catalog, not just react to a delete "
                + "action that happens to be anchored elsewhere")
        XCTAssertTrue(body.contains("path.removeAll()"))
    }

    // ── Source-level helpers ──────────────────────────────────────────────

    /// A file from the iOS client tree, located relative to THIS test file so
    /// the lookup cannot drift with the working directory the runner happens
    /// to use. Throws (fails the test) if the file is gone — an unreadable
    /// source must never read as "the assertion holds".
    private func clientSource(_ relativePath: String) throws -> String {
        let clientRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // apps/ios/native
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

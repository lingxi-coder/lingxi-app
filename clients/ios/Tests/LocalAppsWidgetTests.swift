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

        XCTAssertTrue(source.contains(".sheet(item: localAppCreateConfirmationItem)"))
        XCTAssertTrue(source.contains("LocalAppCreateConfirmationSheet(store: localAppsStore, prompt: prompt)"))
        XCTAssertTrue(source.contains(".sheet(item: localAppMcpProposalApprovalItem)"))
        XCTAssertTrue(source.contains("LocalAppMcpProposalApprovalSheet(store: localAppsStore, prompt: prompt)"))

        // Sliced, not whole-file: `pendingCreateConfirmation == nil` also
        // appears in the UNRELATED `localAppApprovalPresenterIsFree` gate
        // above this property, so a whole-file `contains` stays green even
        // after BOTH gates below are deleted. The property has two —
        // getter and setter — so the MCP sheet cannot outlive (or write
        // through while) a create confirmation is pending; a bare
        // `.contains` is satisfied by either one alone, or even by the
        // unrelated gate, so it cannot catch either gate going missing.
        guard let start = source.range(of: "private var localAppMcpProposalApprovalItem"),
              let end = source.range(
                  of: "private var ",
                  range: start.upperBound..<source.endIndex)
        else {
            return XCTFail("read the wrong file: localAppMcpProposalApprovalItem not found")
        }
        let body = source[start.lowerBound..<end.lowerBound]
        let gateCount = body.components(separatedBy: "localAppsStore.pendingCreateConfirmation == nil").count - 1
        XCTAssertEqual(
            gateCount, 2,
            "localAppMcpProposalApprovalItem must gate BOTH its getter and its "
                + "setter on no create confirmation being pending — deleting either "
                + "gate must turn this test red")
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
        let source = try clientSource("Sources/LocalApps/LocalAppApprovalSheets.swift")

        // This file hosts TWO sheets. A whole-file `.contains` is satisfied
        // by either one alone, so dropping a modifier from just ONE sheet —
        // say `.interactiveDismissDisabled()` off the create sheet — leaves
        // every assertion here green as long as the OTHER sheet still has
        // it. Split at the second sheet's declaration and check both halves
        // independently so either sheet regressing turns this red.
        guard let split = source.range(of: "struct LocalAppMcpProposalApprovalSheet") else {
            return XCTFail("read the wrong file: LocalAppMcpProposalApprovalSheet not found")
        }
        let createSheet = source[source.startIndex..<split.lowerBound]
        let mcpSheet = source[split.lowerBound...]

        for (name, sheet) in [("create", createSheet), ("mcp-proposal", mcpSheet)] {
            XCTAssertTrue(sheet.contains("NavigationStack"), "\(name) sheet lost NavigationStack")
            XCTAssertTrue(sheet.contains("ScrollView"), "\(name) sheet lost ScrollView")
            XCTAssertTrue(
                sheet.contains(".presentationDetents([.medium, .large])"),
                "\(name) sheet lost its detents")
            XCTAssertTrue(
                sheet.contains(".interactiveDismissDisabled()"),
                "\(name) sheet lost .interactiveDismissDisabled()")
        }
        XCTAssertTrue(createSheet.contains("accessibilityIdentifier(\"local-apps.create-confirm."))
        XCTAssertTrue(mcpSheet.contains("accessibilityIdentifier(\"local-apps.mcp-proposal."))
    }

    /// `LocalAppRejectedCandidate` carries no surface of its own — the
    /// rejected-candidates row must not render `prompt.selectedTemplate`'s
    /// surface as if it belonged to the rejected candidate.
    func testRejectedCandidateRowDoesNotBorrowTheSelectedTemplatesSurface() throws {
        let source = try clientSource("Sources/LocalApps/LocalAppApprovalSheets.swift")

        guard let start = source.range(of: "local_apps_create_confirm_rejected_candidates"),
              let end = source.range(of: "SettingsSection(", range: start.upperBound..<source.endIndex)
        else {
            return XCTFail("read the wrong file: rejected-candidates section not found")
        }
        // Comment lines stripped before asserting: this fix's own comment
        // names the removed code so a maintainer does not reintroduce it,
        // which would otherwise defeat a bare `contains` check below.
        let body = source[start.lowerBound..<end.lowerBound]
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")

        XCTAssertTrue(body.contains("rejected.templateID"), "the row must still show the candidate")
        XCTAssertTrue(body.contains("rejected.reason"), "the row must still show why it lost")
        XCTAssertFalse(
            body.contains("prompt.selectedTemplate.surface"),
            "the WINNING template's surface must not be rendered as if it were "
                + "this REJECTED candidate's own surface")
    }

    func testMcpPagesKeepsGenericEditorAlongsideManagedLocalAppRestrictions() throws {
        let source = try clientSource("Sources/Settings/MCPPages.swift")

        XCTAssertTrue(source.contains("struct MCPListPage"))
        XCTAssertTrue(source.contains("struct MCPEditPage"))
        XCTAssertTrue(source.contains("struct ManagedLocalAppMCPEditPage"))
        XCTAssertTrue(source.contains("allowsMcpConfigurationEditing"))
        XCTAssertTrue(source.contains("managedServerRow(server:"))
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

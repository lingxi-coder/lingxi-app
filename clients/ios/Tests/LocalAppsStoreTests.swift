import XCTest
import WebKit
@testable import LingxiCode

@MainActor
final class LocalAppsStoreTests: XCTestCase {
    func testQaEnvelopePreservesOpaqueActionValueAndAddsRuntimeIdentity() throws {
        let original = #"{"rect":{"x":10.5,"y":2,"width":30,"height":20}}"#
        let valueData = try JSONSerialization.data(withJSONObject: [
            "lingxi_qa": [
                "version": 1,
                "expected_runtime_url": "http://127.0.0.1:43123/?lingxi_runtime=42",
                "action_value": original,
            ],
        ])
        let value = try XCTUnwrap(String(data: valueData, encoding: .utf8))
        let envelope = try XCTUnwrap(try LocalAppQaEnvelope.decode(value, requestID: "qa-ui-1").get())
        XCTAssertEqual(envelope.expectedRuntimeURL.absoluteString, "http://127.0.0.1:43123/?lingxi_runtime=42")
        XCTAssertEqual(envelope.actionValue, original)
        XCTAssertNil(try LocalAppQaEnvelope.decode(value, requestID: "app-ui-1").get())
        XCTAssertNil(try LocalAppQaEnvelope.decode(original, requestID: "app-ui-2").get())
    }

    func testQaEnvelopeRejectsMissingMarkerAndNonStringActionValue() throws {
        let missingMarker = #"{"lingxi_qa":{"version":1,"expected_runtime_url":"http://127.0.0.1:43123","action_value":null}}"#
        XCTAssertThrowsError(try LocalAppQaEnvelope.decode(missingMarker, requestID: "qa-ui-1").get())
        let nonString = #"{"lingxi_qa":{"version":1,"expected_runtime_url":"http://127.0.0.1:43123/?lingxi_runtime=42","action_value":{"x":1}}}"#
        XCTAssertThrowsError(try LocalAppQaEnvelope.decode(nonString, requestID: "qa-ui-2").get())
        let booleanVersion = #"{"lingxi_qa":{"version":true,"expected_runtime_url":"http://127.0.0.1:43123/?lingxi_runtime=42","action_value":null}}"#
        XCTAssertThrowsError(try LocalAppQaEnvelope.decode(booleanVersion, requestID: "qa-ui-3").get())
        let routedExpected = #"{"lingxi_qa":{"version":1,"expected_runtime_url":"http://127.0.0.1:43123/route?lingxi_runtime=42","action_value":null}}"#
        XCTAssertThrowsError(try LocalAppQaEnvelope.decode(routedExpected, requestID: "qa-ui-4").get())
    }

    func testRuntimeIdentityIgnoresRoutePathButRequiresGenerationMarker() throws {
        let expected = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let route = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/settings?lingxi_runtime=42"))
        let stale = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/settings?lingxi_runtime=41"))
        XCTAssertTrue(LocalAppWebViewController.sameRuntimeIdentity(expected, route))
        XCTAssertFalse(LocalAppWebViewController.sameRuntimeIdentity(expected, stale))
    }

    func testQaControllerRejectsStaleSamePortFinishAndCannotReviveAfterClose() throws {
        let runtimeA = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=41"))
        let runtimeB = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let routeB = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/settings?lingxi_runtime=42"))
        let controller = LocalAppWebViewController(
            appID: "tracker",
            broker: LocalAppBridgeBroker(appID: "tracker")
        )
        controller.webView = WKWebView()

        let generationA = controller.beginNavigation(expectedURL: runtimeA)
        controller.markReady(committedURL: runtimeA, navigationGeneration: generationA)
        XCTAssertEqual(controller.qaDocument(expectedURL: runtimeA)?.navigationGeneration, generationA)

        let generationB = controller.beginNavigation(expectedURL: runtimeB)
        controller.markReady(committedURL: runtimeA, navigationGeneration: generationA)
        XCTAssertNil(controller.qaDocument(expectedURL: runtimeB), "late generation A callback must be ignored")
        controller.markReady(committedURL: runtimeA, navigationGeneration: generationB)
        XCTAssertNil(controller.qaDocument(expectedURL: runtimeB), "same port does not make runtime A equal B")
        controller.markReady(committedURL: routeB, navigationGeneration: generationB)
        XCTAssertEqual(controller.qaDocument(expectedURL: runtimeB)?.loadedURL, routeB)

        controller.close()
        controller.markReady(committedURL: routeB, navigationGeneration: generationB)
        XCTAssertNil(controller.qaDocument(expectedURL: runtimeB), "a detached controller cannot be revived")
    }

    func testSameDocumentCommitAdvancesTheRouteWithoutADocumentLoad() throws {
        let runtime = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let first = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/first?tab=all&lingxi_runtime=42"))
        let second = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/second?lingxi_runtime=42"))
        let controller = LocalAppWebViewController(
            appID: "tracker",
            broker: LocalAppBridgeBroker(appID: "tracker")
        )
        controller.webView = WKWebView()

        let load = controller.beginNavigation(expectedURL: runtime)
        controller.beginDocumentLoad()
        controller.markReady(committedURL: runtime, navigationGeneration: load)
        let loaded = try XCTUnwrap(controller.qaDocument(expectedURL: runtime))
        XCTAssertEqual(loaded.documentLoadGeneration, 1)

        // pushState: a new route, a new generation, the SAME document load.
        XCTAssertEqual(controller.commitSameDocumentNavigation(first), load &+ 1)
        let routed = try XCTUnwrap(controller.qaDocument(expectedURL: runtime))
        XCTAssertEqual(routed.loadedURL, first)
        XCTAssertEqual(
            routed.documentLoadGeneration,
            loaded.documentLoadGeneration,
            "a same-document history commit is not a document load"
        )
        XCTAssertNil(
            controller.commitSameDocumentNavigation(first),
            "a repeat of the loaded URL is a no-op, not a step"
        )
        XCTAssertEqual(controller.commitSameDocumentNavigation(second), load &+ 2)

        // A full load raises the counter and nothing else does.
        let reload = controller.beginNavigation(expectedURL: runtime)
        controller.beginDocumentLoad()
        controller.markReady(committedURL: runtime, navigationGeneration: reload)
        XCTAssertEqual(try XCTUnwrap(controller.qaDocument(expectedURL: runtime)).documentLoadGeneration, 2)
    }

    func testSameDocumentCommitAuthenticatesOnlyTheRequestedHistoryDestination() throws {
        let runtime = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let first = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/first?lingxi_runtime=42"))
        let second = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/second?lingxi_runtime=42"))
        let foreign = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/first?lingxi_runtime=41"))
        let controller = LocalAppWebViewController(
            appID: "tracker",
            broker: LocalAppBridgeBroker(appID: "tracker")
        )
        controller.webView = WKWebView()

        let load = controller.beginNavigation(expectedURL: runtime)
        controller.beginDocumentLoad()
        controller.markReady(committedURL: runtime, navigationGeneration: load)
        controller.commitSameDocumentNavigation(first)
        controller.commitSameDocumentNavigation(second)

        // Host Back: only the destination taken from the back/forward list can
        // complete it. Before this existed, a same-document step produced no
        // navigation callback and the document never became ready again.
        let back = controller.beginHistoryNavigation(to: first)
        XCTAssertNil(controller.qaDocument(expectedURL: runtime), "the step is not complete yet")
        XCTAssertNil(
            controller.commitSameDocumentNavigation(second),
            "a delayed callback for the current entry cannot complete Back"
        )
        XCTAssertEqual(controller.commitSameDocumentNavigation(first), back)
        XCTAssertEqual(try XCTUnwrap(controller.qaDocument(expectedURL: runtime)).loadedURL, first)

        // A URL outside the Host runtime identity invalidates the document
        // rather than leaving it attestable.
        XCTAssertNil(controller.commitSameDocumentNavigation(foreign))
        XCTAssertNil(
            controller.qaDocument(expectedURL: runtime),
            "old state must not attest after a foreign runtime URL"
        )
    }

    func testEventActionMayAttestAnSpaRouteButNotADocumentAFullLoadReplaced() throws {
        let runtime = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let route = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/settings?lingxi_runtime=42"))
        let other = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/other?lingxi_runtime=42"))
        let before = LocalAppWebViewController.QaDocument(
            loadedURL: runtime,
            navigationGeneration: 7,
            documentLoadGeneration: 2
        )
        let routed = LocalAppWebViewController.QaDocument(
            loadedURL: route,
            navigationGeneration: 8,
            documentLoadGeneration: 2
        )
        // Same generation arithmetic; only the document load differs.
        let reloaded = LocalAppWebViewController.QaDocument(
            loadedURL: other,
            navigationGeneration: 8,
            documentLoadGeneration: 3
        )
        let ok = LocalAppUIExecutionResult(resultJSON: "{\"ok\":true}", error: nil)
        let failed = LocalAppUIExecutionResult.failure("target was not found")

        XCTAssertTrue(LocalAppWebViewController.qaActionMayAdvanceDocument(action: .click, result: ok))
        XCTAssertFalse(LocalAppWebViewController.qaActionMayAdvanceDocument(action: .click, result: failed))
        XCTAssertFalse(LocalAppWebViewController.qaActionMayAdvanceDocument(action: .captureView, result: ok))
        XCTAssertFalse(LocalAppWebViewController.qaActionMayAdvanceDocument(action: .inspect, result: ok))

        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: routed,
                intentionalNavigation: false,
                allowInteractiveNavigation: true
            )
        )
        XCTAssertFalse(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: reloaded,
                intentionalNavigation: false,
                allowInteractiveNavigation: true
            ),
            "a cross-document load during an event action cannot be attested"
        )
        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: before,
                intentionalNavigation: false,
                allowInteractiveNavigation: true
            ),
            "the pre-action document is still the one the action ran in"
        )
        XCTAssertFalse(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: routed,
                intentionalNavigation: false
            ),
            "an action that may not advance still needs the exact document"
        )
        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: reloaded,
                intentionalNavigation: true
            ),
            "Navigate/Back/Reload ARE the load and stay judged by generation"
        )
    }

    func testQaNonNavigationRequiresExactPrePostDocumentAndIntentionalNavigationAdvancesGeneration() throws {
        let firstURL = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let routeURL = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/settings?lingxi_runtime=42"))
        let before = LocalAppWebViewController.QaDocument(loadedURL: firstURL, navigationGeneration: 7, documentLoadGeneration: 2)
        let routeAfter = LocalAppWebViewController.QaDocument(loadedURL: routeURL, navigationGeneration: 8, documentLoadGeneration: 2)
        let sameDocument = LocalAppWebViewController.QaDocument(loadedURL: firstURL, navigationGeneration: 7, documentLoadGeneration: 2)

        XCTAssertFalse(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: routeAfter,
                intentionalNavigation: false
            ),
            "inspect/capture must not attest a later same-runtime route"
        )
        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: routeAfter,
                intentionalNavigation: true
            )
        )
        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: sameDocument,
                intentionalNavigation: false
            )
        )
    }

    func testQaFailedNavigationPreservesErrorWithoutAdvancingGeneration() throws {
        let runtimeURL = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let routeURL = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/settings?lingxi_runtime=42"))
        let before = LocalAppWebViewController.QaDocument(loadedURL: runtimeURL, navigationGeneration: 7, documentLoadGeneration: 2)
        let routeAfter = LocalAppWebViewController.QaDocument(loadedURL: routeURL, navigationGeneration: 8, documentLoadGeneration: 2)
        let controller = LocalAppWebViewController(
            appID: "tracker",
            broker: LocalAppBridgeBroker(appID: "tracker")
        )

        let failedBack = LocalAppUIExecutionResult.failure("no history")
        let failedNavigate = LocalAppUIExecutionResult.failure("Only trusted loopback navigation is allowed")
        let acceptedNavigate = LocalAppUIExecutionResult(resultJSON: "{\"ok\":true}", error: nil)
        XCTAssertFalse(LocalAppWebViewController.qaNavigationWasAccepted(action: .back, result: failedBack))
        XCTAssertFalse(LocalAppWebViewController.qaNavigationWasAccepted(action: .navigate, result: failedNavigate))
        XCTAssertTrue(LocalAppWebViewController.qaNavigationWasAccepted(action: .navigate, result: acceptedNavigate))

        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: before,
                intentionalNavigation: false
            )
        )
        XCTAssertFalse(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: routeAfter,
                intentionalNavigation: false
            ),
            "a failed navigation cannot certify a different document"
        )
        XCTAssertTrue(
            LocalAppWebViewController.qaDocumentMatches(
                before: before,
                after: routeAfter,
                intentionalNavigation: true
            )
        )

        for (failure, message) in [(failedBack, "no history"), (failedNavigate, "Only trusted loopback navigation is allowed")] {
            let wrapped = controller.wrapQaResult(failure, requestedURL: runtimeURL, document: before)
            let data = try XCTUnwrap(wrapped.resultJSON?.data(using: .utf8))
            let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            let result = try XCTUnwrap(object["result"] as? [String: Any])
            XCTAssertEqual(result["ok"] as? Bool, false)
            XCTAssertEqual(result["error"] as? String, message)
            XCTAssertNil(wrapped.error)
        }
    }

    func testQaResultWrapPreservesNativeOperationErrorInsideAuthenticatedEnvelope() throws {
        let requested = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
        let controller = LocalAppWebViewController(
            appID: "tracker",
            broker: LocalAppBridgeBroker(appID: "tracker")
        )
        let document = LocalAppWebViewController.QaDocument(
            loadedURL: requested,
            navigationGeneration: 3,
            documentLoadGeneration: 1
        )
        let wrapped = controller.wrapQaResult(
            .failure("target was not found"),
            requestedURL: requested,
            document: document
        )

        XCTAssertNil(wrapped.error)
        let data = try XCTUnwrap(wrapped.resultJSON?.data(using: .utf8))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let result = try XCTUnwrap(object["result"] as? [String: Any])
        XCTAssertEqual(result["ok"] as? Bool, false)
        XCTAssertEqual(result["error"] as? String, "target was not found")
    }

    func testFilteringUsesLocalizedSearch() {
        let store = LocalAppsStore()
        #if canImport(engine_mobileFFI)
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "tracker", name: "订单跟踪", brief: "跟踪订单状态"),
                appRecord(id: "metrics", name: "Metrics", brief: "查看运营指标"),
            ]))
        #endif

        store.searchQuery = "订单"

        XCTAssertEqual(store.filteredApps.map(\.id), ["tracker"])
    }

    func testWebsiteDataCleanupIsDurableAndWaitsForAppAbsence() async throws {
        let suiteName = "LocalAppsStoreTests.website-cleanup.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defer { defaults.removePersistentDomain(forName: suiteName) }

        var removedIdentifiers: [UUID] = []
        let registry = LocalAppWebsiteDataStoreRegistry(
            defaults: defaults,
            removeDataStore: { identifier in
                removedIdentifiers.append(identifier)
                return true
            }
        )
        registry.prepareForDeletion(appID: "tracker")
        let identifier = try XCTUnwrap(registry.storedIdentifier(appID: "tracker"))

        // A new registry reads the persisted journal, matching a process relaunch.
        let relaunched = LocalAppWebsiteDataStoreRegistry(
            defaults: defaults,
            removeDataStore: { identifier in
                removedIdentifiers.append(identifier)
                return true
            }
        )
        XCTAssertEqual(
            relaunched.pendingCleanups,
            [.init(appID: "tracker", dataStoreIdentifier: identifier)]
        )

        await relaunched.removeDataForDeletedApps(activeAppIDs: ["tracker"])
        XCTAssertTrue(removedIdentifiers.isEmpty, "an existing app is not proof that deletion completed")
        XCTAssertEqual(relaunched.pendingCleanups.count, 1)

        await relaunched.removeDataForDeletedApps(activeAppIDs: [])
        XCTAssertEqual(removedIdentifiers, [identifier])
        XCTAssertTrue(relaunched.pendingCleanups.isEmpty)
        XCTAssertNil(relaunched.storedIdentifier(appID: "tracker"))
    }

    func testPublishedWorkflowBadgesStayTextualAndActionable() {
        let unverified = LocalAppWorkflow.publishedUnverified.statusBadge
        XCTAssertEqual(unverified.systemImageName, "exclamationmark.triangle.fill")
        XCTAssertEqual(
            unverified.accessibilityLabel,
            String(localized: "local_apps_verification_published_unverified"))
        XCTAssertNotEqual(
            unverified.accessibilityLabel, "local_apps_verification_published_unverified",
            "the key fell through as itself — it is missing from the catalog")
        XCTAssertEqual(LocalAppWorkflow.publishedUnverified.isPublished, true)

        let verified = LocalAppWorkflow.publishedVerified.statusBadge
        XCTAssertEqual(verified.systemImageName, "checkmark.seal.fill")
        XCTAssertEqual(
            verified.accessibilityLabel,
            String(localized: "local_apps_verification_published_verified"))
        XCTAssertNotEqual(
            verified.accessibilityLabel, "local_apps_verification_published_verified",
            "the key fell through as itself — it is missing from the catalog")
        XCTAssertEqual(LocalAppWorkflow.publishedVerified.isPublished, true)

        let draft = LocalAppWorkflow.draft.statusBadge
        XCTAssertEqual(draft.systemImageName, "pencil.circle.fill")
        XCTAssertEqual(LocalAppWorkflow.draft.isPublished, false)
    }

    func testPluginStatusEventUpdatesBuiltinPluginState() {
        let store = LocalAppsStore()
        store.handle(event: .appEvent(event: .pluginStatusChanged(status: PluginStatusDto(
            pluginId: "lingxi-local-app",
            state: .disabled,
            manifestDefaultEnabled: true
        ))))

        XCTAssertEqual(store.builtinPluginStatus?.state, .disabled)
        XCTAssertEqual(store.builtinPluginStatus?.manifestDefaultEnabled, true)
        XCTAssertEqual(store.builtinPluginEffectiveEnabled, false)
    }

    func testPluginInventoryEventUpdatesBuiltinPluginMetadata() {
        let store = LocalAppsStore()
        store.handle(event: .appEvent(event: .pluginInventoryChanged(inventory: LocalAppPluginInventoryDto(
            pluginId: "lingxi-local-app",
            displayName: "LingXi Local App",
            source: "builtin",
            version: "2.0.1",
            bundleSha256: String(repeating: "d", count: 64),
            state: .disabled,
            manifestDefaultEnabled: false,
            counts: LocalAppPluginComponentCountsDto(skills: 9, agents: 4, workflows: 2, templates: 3),
            validationError: "manifest mismatch"
        ))))

        XCTAssertEqual(store.builtinPluginDescriptor.version, "2.0.1")
        XCTAssertEqual(store.builtinPluginDescriptor.archiveDigest, String(repeating: "d", count: 64))
        XCTAssertEqual(store.builtinPluginDescriptor.skillCount, 9)
        XCTAssertEqual(store.builtinPluginDescriptor.agentCount, 4)
        XCTAssertEqual(store.builtinPluginStatus?.state, .disabled)
        XCTAssertEqual(store.builtinPluginStatus?.validationError, "manifest mismatch")
        XCTAssertFalse(store.builtinPluginEffectiveEnabled)
    }

    func testManagedInventoryReaderLoadsPublishedCatalogFromDisk() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let appID = "tracker"
        let catalogDigest = String(repeating: "a", count: 64)
        let toolDigest = String(repeating: "b", count: 64)
        let verificationDigest = String(repeating: "c", count: 64)
        let manifestURL = root
            .appendingPathComponent("apps/\(appID)/workspace/.lingxi/manifest.json")
        try FileManager.default.createDirectory(
            at: manifestURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let manifest = """
        {
          "activeMcpCatalog": {
            "buildId": "build-42",
            "catalogSha256": "\(catalogDigest)",
            "toolSurfaceSha256": "\(toolDigest)",
            "mcpVerificationSha256": "\(verificationDigest)",
            "authoringRevision": 7
          }
        }
        """
        try manifest.write(to: manifestURL, atomically: true, encoding: .utf8)

        let catalogURL = root
            .appendingPathComponent("apps/\(appID)/mcp/catalogs/\(catalogDigest).json")
        try FileManager.default.createDirectory(
            at: catalogURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let catalog = """
        {
          "tools": [
            {
              "definition": {
                "name": "read_value",
                "title": "Read Value",
                "description": "Reads a value",
                "inputSchema": {"type":"object","properties":{"id":{"type":"string"}}},
                "outputSchema": {"type":"object"},
                "annotations": {"readOnlyHint": true},
                "_meta": {"visible": true}
              },
              "flow": {"steps":[{"type":"read"}]},
              "ceiling": {"type":"allow"}
            }
          ]
        }
        """
        try catalog.write(to: catalogURL, atomically: true, encoding: .utf8)

        let settingsURL = root
            .appendingPathComponent("apps/\(appID)/mcp/settings.json")
        let settings = """
        {
          "enabled": true,
          "revision": 7,
          "enabledTools": ["read_value"]
        }
        """
        try settings.write(to: settingsURL, atomically: true, encoding: .utf8)

        let reader = LocalAppManagedMcpInventoryReader(appSandboxRoot: root.path)
        let inventory = reader.read(
            serverName: "local_app_tracker",
            apps: [
                LocalAppSummary(
                    id: appID,
                    name: "Tracker",
                    brief: "Reads values",
                    updatedAt: .now,
                    workflow: .publishedVerified,
                    workspaceRelativePath: "apps/\(appID)/workspace"
                )
            ]
        )

        XCTAssertEqual(inventory?.appID, appID)
        XCTAssertEqual(inventory?.buildID, "build-42")
        XCTAssertEqual(inventory?.toolCount, 1)
        XCTAssertEqual(inventory?.authoringRevision, 7)
        XCTAssertEqual(inventory?.enabled, true)
        XCTAssertEqual(inventory?.status, .enabled)
        XCTAssertEqual(inventory?.settingsRevision, 7)
        XCTAssertEqual(inventory?.publicationState, .publishedVerified)
        XCTAssertEqual(inventory?.uiVerification.status, .passed)
        XCTAssertEqual(inventory?.mcpVerification.status, .passed)
        XCTAssertEqual(inventory?.mcpVerification.code, verificationDigest)
        XCTAssertEqual(inventory?.enabledTools, Set(["read_value"]))
        XCTAssertEqual(inventory?.tools.first?.name, "read_value")
    }

    func testManagedMcpInventoryFallsBackToDefaultOffNeedsSetupState() {
        let store = LocalAppsStore()
        #if canImport(engine_mobileFFI)
            store.handle(event: .appsChanged(apps: [
                appRecord(
                    id: "tracker",
                    name: "Tracker",
                    brief: "Summarize local state",
                    workflowState: .publishedVerified
                )
            ]))
        #endif

        let inventory = store.managedMcpInventory(appID: "tracker", appSandboxRoot: "/nonexistent")

        XCTAssertEqual(inventory.serverName, "local_app_tracker")
        XCTAssertFalse(inventory.enabled)
        XCTAssertEqual(inventory.status, .needsSetup)
        XCTAssertEqual(inventory.settingsRevision, 0)
        XCTAssertTrue(inventory.enabledTools.isEmpty)
        XCTAssertTrue(inventory.tools.isEmpty)
        XCTAssertEqual(inventory.publicationState, .publishedVerified)
    }

    func testManagedInventoryRestrictionsStayReadOnly() {
        let inventory = LocalAppManagedMcpInventory(
            serverName: "local_app_tracker",
            appID: "tracker",
            appName: "Tracker",
            buildID: "build-42",
            catalogDigest: String(repeating: "a", count: 64),
            toolSurfaceDigest: String(repeating: "b", count: 64),
            authoringRevision: 7,
            enabled: true,
            status: .enabled,
            settingsRevision: 9,
            pinnedToCurrentConversation: false,
            publicationState: .publishedVerified,
            mcpVerification: LocalAppVerificationSummary(
                status: .passed,
                summary: "MCP verification passed",
                code: String(repeating: "c", count: 64)
            ),
            uiVerification: LocalAppVerificationSummary(
                status: .passed,
                summary: "UI verification passed",
                code: nil
            ),
            enabledTools: [],
            widget: nil,
            tools: []
        )

        XCTAssertFalse(allowsMcpConfigurationEditing(inventory))
        XCTAssertTrue(allowsMcpConfigurationEditing(nil))
    }

    func testFailedWebsiteDataCleanupKeepsJournalAndMappingForRetry() async throws {
        let suiteName = "LocalAppsStoreTests.website-cleanup-failure.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defer { defaults.removePersistentDomain(forName: suiteName) }

        var removalSucceeds = false
        let registry = LocalAppWebsiteDataStoreRegistry(
            defaults: defaults,
            removeDataStore: { _ in removalSucceeds }
        )
        registry.prepareForDeletion(appID: "tracker")
        let identifier = try XCTUnwrap(registry.storedIdentifier(appID: "tracker"))

        await registry.removeDataForDeletedApps(activeAppIDs: [])
        XCTAssertEqual(registry.pendingCleanups.count, 1)
        XCTAssertEqual(registry.pendingCleanups.first?.absenceConfirmed, true)
        XCTAssertEqual(registry.storedIdentifier(appID: "tracker"), identifier)

        registry.cancelDeletion(appID: "tracker")
        XCTAssertEqual(
            registry.pendingCleanups.count, 1,
            "a later failed same-id request must not cancel already-confirmed cleanup"
        )

        removalSucceeds = true
        await registry.removeDataForDeletedApps(activeAppIDs: [])
        XCTAssertTrue(registry.pendingCleanups.isEmpty)
        XCTAssertNil(registry.storedIdentifier(appID: "tracker"))
    }

    func testClosingAnAppDetachesItsWebViewAndBridge() async throws {
        let broker = LocalAppBridgeBroker(appID: "tracker")
        var forwarded = 0
        broker.onRequest = { _ in forwarded += 1 }
        let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
        let webView = try await makeBridgeCaptureWebView()
        controller.webView = webView
        broker.webView = webView
        LocalAppWebViewRegistry.shared.register(controller, appID: "tracker")

        broker.receive(
            body: ["requestId": "before-close", "operation": "query", "payload": [:]],
            namespace: "data"
        )

        LocalAppWebViewRegistry.shared.close(appID: "tracker")

        XCTAssertNil(controller.webView)
        XCTAssertNil(broker.webView)
        let envelope = try await bridgeEnvelope(containing: "bridge_detached", in: webView)
        XCTAssertTrue(envelope?.contains("bridge_detached") == true)
        broker.receive(
            body: ["requestId": "after-close", "operation": "query", "payload": [:]],
            namespace: "data"
        )
        XCTAssertEqual(forwarded, 1)
        XCTAssertEqual(broker.inFlightCount, 0)
    }

    func testBridgeRejectsEmptyIdentifiersAndOperations() async throws {
        let broker = LocalAppBridgeBroker(appID: "tracker")
        var forwarded = 0
        broker.onRequest = { _ in forwarded += 1 }
        let webView = try await makeBridgeCaptureWebView()
        broker.webView = webView

        broker.receive(
            body: ["requestId": "", "operation": "query", "payload": [:]],
            namespace: "data"
        )
        broker.receive(
            body: ["requestId": "usable-id", "operation": "", "payload": [:]],
            namespace: "data"
        )

        XCTAssertEqual(forwarded, 0)
        XCTAssertEqual(broker.inFlightCount, 0)
        XCTAssertEqual(LocalAppBridgeBroker.requestIDInvalidCode, "request_id_invalid")
        XCTAssertEqual(LocalAppBridgeBroker.operationInvalidCode, "operation_invalid")
        let envelope = try await bridgeEnvelope(containing: "operation_invalid", in: webView)
        XCTAssertTrue(envelope?.contains("operation_invalid") == true)
    }

    func testBridgeCapsTheCompleteRequestEvenWhenPayloadAloneFits() async throws {
        let payload: [String: Any] = [
            "blob": String(repeating: "x", count: LocalAppBridgeBroker.maxControlBytes - 32),
        ]
        let payloadData = try JSONSerialization.data(withJSONObject: payload)
        XCTAssertLessThanOrEqual(payloadData.count, LocalAppBridgeBroker.maxControlBytes)

        let body: [String: Any] = [
            "requestId": "request-with-envelope-overhead",
            "operation": "mutate",
            "payload": payload,
        ]
        let byteCount = try XCTUnwrap(LocalAppBridgeBroker.oversizedRequestByteCount(
            body,
            limit: LocalAppBridgeBroker.maxControlBytes
        ))
        XCTAssertGreaterThan(byteCount, LocalAppBridgeBroker.maxControlBytes)

        let broker = LocalAppBridgeBroker(appID: "tracker")
        let webView = try await makeBridgeCaptureWebView()
        broker.webView = webView
        var forwarded = 0
        broker.onRequest = { _ in forwarded += 1 }
        broker.receive(body: body, namespace: "data")
        XCTAssertEqual(forwarded, 0)
        XCTAssertEqual(broker.inFlightCount, 0)
        let envelope = try await bridgeEnvelope(containing: "request_too_large", in: webView)
        XCTAssertTrue(envelope?.contains("request_too_large") == true)
    }

    func testBridgeGivesLLMChatABoundedLargeContextLane() {
        XCTAssertEqual(LocalAppBridgeBroker.byteLimit(namespace: "data", operation: "query"), 64 * 1_024)
        XCTAssertEqual(
            LocalAppBridgeBroker.byteLimit(namespace: "llm", operation: "chat"),
            8 * 1_024 * 1_024
        )

        let broker = LocalAppBridgeBroker(appID: "tracker")
        var forwarded: [LocalAppBridgeRequest] = []
        broker.onRequest = { forwarded.append($0) }
        broker.receive(
            body: [
                "requestId": "large-context",
                "operation": "chat",
                "payload": [
                    "messages": [[
                        "role": "user",
                        "content": String(repeating: "x", count: LocalAppBridgeBroker.maxControlBytes),
                    ]],
                ],
            ],
            namespace: "llm"
        )

        XCTAssertEqual(forwarded.map(\.id), ["large-context"])
    }

    func testBridgeRejectsNonObjectPayloadInsteadOfCoercingItToEmptyObject() async throws {
        let broker = LocalAppBridgeBroker(appID: "tracker")
        let webView = try await makeBridgeCaptureWebView()
        broker.webView = webView
        var forwarded = 0
        broker.onRequest = { _ in forwarded += 1 }

        broker.receive(
            body: ["requestId": "bad-payload", "operation": "mutate", "payload": []],
            namespace: "data"
        )

        XCTAssertEqual(forwarded, 0)
        XCTAssertEqual(broker.inFlightCount, 0)
        let envelope = try await bridgeEnvelope(containing: "payload_invalid", in: webView)
        XCTAssertTrue(envelope?.contains("payload_invalid") == true)
    }

    func testBridgeRejectsDuplicateIDsWithoutDroppingTheOriginalRequest() async throws {
        let broker = LocalAppBridgeBroker(appID: "tracker")
        var forwarded: [String] = []
        broker.onRequest = { forwarded.append($0.id) }
        let webView = try await makeBridgeCaptureWebView()
        broker.webView = webView
        let body: [String: Any] = ["requestId": "same-id", "operation": "query", "payload": [:]]

        broker.receive(body: body, namespace: "data")
        broker.receive(body: body, namespace: "data")

        XCTAssertEqual(forwarded, ["same-id"])
        XCTAssertEqual(broker.inFlightCount, 1, "the duplicate rejection must not remove the original")
        XCTAssertEqual(LocalAppBridgeBroker.duplicateRequestIDCode, "duplicate_request_id")
        let envelope = try await bridgeEnvelope(containing: "duplicate_request_id", in: webView)
        XCTAssertTrue(envelope?.contains("duplicate_request_id") == true)
    }

    func testBridgeCapsOutstandingRequestsAt128() async throws {
        let broker = LocalAppBridgeBroker(appID: "tracker")
        var forwarded = 0
        broker.onRequest = { _ in forwarded += 1 }
        let webView = try await makeBridgeCaptureWebView()
        broker.webView = webView
        for index in 0 ... LocalAppBridgeBroker.maxInFlightRequests {
            broker.receive(
                body: ["requestId": "request-\(index)", "operation": "query", "payload": [:]],
                namespace: "data"
            )
        }

        XCTAssertEqual(forwarded, LocalAppBridgeBroker.maxInFlightRequests)
        XCTAssertEqual(broker.inFlightCount, LocalAppBridgeBroker.maxInFlightRequests)
        XCTAssertEqual(LocalAppBridgeBroker.tooManyInFlightCode, "too_many_requests")
        let envelope = try await bridgeEnvelope(containing: "too_many_requests", in: webView)
        XCTAssertTrue(envelope?.contains("too_many_requests") == true)
    }

    func testWebViewNavigationRequiresTheExactRuntimeOrigin() {
        let origin = URL(string: "http://127.0.0.1:43123")!
        XCTAssertTrue(LocalAppWebViewRepresentable.Coordinator.isAllowed(
            URL(string: "http://127.0.0.1:43123/detail")!,
            origin: origin
        ))
        XCTAssertFalse(LocalAppWebViewRepresentable.Coordinator.isAllowed(
            URL(string: "https://127.0.0.1:43123/detail")!,
            origin: origin
        ))
        XCTAssertFalse(LocalAppWebViewRepresentable.Coordinator.isAllowed(
            URL(string: "http://localhost:43123/detail")!,
            origin: origin
        ))
        XCTAssertFalse(LocalAppWebViewRepresentable.Coordinator.isAllowed(
            URL(string: "http://127.0.0.1:43124/detail")!,
            origin: origin
        ))
    }

    /// Asserted as one whole string, not by `contains` on the directives that
    /// happen to be interesting: a CSP is only as strong as its most permissive
    /// directive, and this injected meta INTERSECTS with the host's header, so
    /// a drift here silently overrides the engine.
    ///
    /// What this CANNOT do is compare against the engine: `LOCAL_APP_CONTENT_
    /// SECURITY_POLICY` lives in Rust and is not shipped to this target, so the
    /// literal below is a hand-kept copy — and it is deliberately not
    /// byte-identical (the engine orders `img-src; font-src; connect-src;
    /// media-src`, this orders `img-src; media-src; font-src; connect-src`;
    /// directive ORDER is not meaningful to a CSP parser). Locking the two
    /// together needs the policy to travel over the protocol; until then, an
    /// engine-only edit is caught by review, not by this test.
    func testInjectedCSPPinsTheWholePolicy() {
        let source = LocalAppWebViewRepresentable.bridgeSource(formFactor: "iphone")
        XCTAssertTrue(source.contains(
            "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
        ))
        // The one placeholder this template really has. Asserting a
        // `__LINGXI_CSP__` token that exists nowhere in the repo could never
        // fail; this one goes red if the substitution is dropped or the no-arg
        // accessor is used, which would boot every app with the literal string
        // as its `formFactor`.
        XCTAssertFalse(source.contains("__LINGXI_NATIVE_FORM_FACTOR__"))
        XCTAssertTrue(source.contains("formFactor: 'iphone'"))
    }

    /// Ionic keeps a component's interactive internals in a SHADOW ROOT, which
    /// `document.querySelectorAll` does not cross. Before this walk an app built
    /// from `ion-*` components reported `elements: []` — indistinguishable from
    /// a blank screen and from a crash, which is the very ambiguity
    /// `canvasCount` exists to resolve for a drawn surface.
    ///
    /// The Android twin is `LocalAppWebViewTest.ui inspection crosses shadow
    /// roots and resolves the native control`; the two scripts are near-copies,
    /// so both are pinned or neither is.
    func testUIInspectionCrossesShadowRootsAndResolvesTheNativeControl() {
        let source = LocalAppWebViewController.executionSource(requestJSON: "{}")
        for token in [
            "const deepQuery = (selector, limit)",
            "if (host.shadowRoot) visit(host.shadowRoot, depth + 1)",
            // Both bounds, or a nested/looping page hangs the tool call.
            "depth > 8 || found.length >= limit",
            "const candidates = () => deepQuery(SELECTOR, 400)",
            // A shadow root is its own id scope; `getElementById` cannot see in.
            "|| deepQuery('[id=\"' +",
            // `ion-input` holds the real <input> inside its shadow root.
            "const nativeControl = element =>",
            "element.shadowRoot.querySelector('input,textarea,select')",
        ] {
            XCTAssertTrue(source.contains(token), "missing shadow-DOM contract: \(token)")
        }
        XCTAssertFalse(
            source.contains("Array.from(document.querySelectorAll('button,a[href]"),
            "the light-DOM-only walk must be gone, not merely supplemented"
        )
    }

    /// Phase 1a — a user-drawn rectangle can only be mapped onto elements if the
    /// snapshot carries geometry. `getBoundingClientRect()` was already being
    /// computed and then discarded down to a `visible` boolean.
    ///
    /// The Android twin is `LocalAppWebViewTest.ui inspection reports element and
    /// canvas geometry`; the two scripts are near-copies, so both are pinned or
    /// neither is.
    func testUIInspectionReportsElementAndCanvasGeometry() {
        let source = LocalAppWebViewController.executionSource(requestJSON: "{}")
        for token in [
            // Element rect, integer CSS pixels.
            "rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)]",
            // Canvas rects, bounded at 16 — the host gate compares only these pixels.
            "canvases: deepQuery('canvas', 16).map",
            // The legacy count stays for compatibility.
            "canvasCount:",
            // Readiness and the coordinate frame the rect is expressed in.
            "documentState: document.readyState",
            "offsetLeft: Math.round(vv.offsetLeft)",
            "scale: vv.scale",
        ] {
            XCTAssertTrue(source.contains(token), "missing geometry contract: \(token)")
        }
        // Geometry is ADDED to the element shape, not substituted for `visible`:
        // a reader that only asks "is it on screen" must keep working.
        XCTAssertTrue(
            source.contains("visible: rect.width > 0 && rect.height > 0,"),
            "`visible` must survive alongside the new rect, not be replaced by it"
        )
    }

    /// Phase 1a — criterion 6 is structurally blind without `console.error`.
    /// Both scaffolds wrap the tree in a React ErrorBoundary whose only hook is
    /// `getDerivedStateFromError`; React 19 routes a caught render error to
    /// `console.error` and never to `window.onerror`. A crashed app then renders
    /// its fallback and passes "has elements", "frame is not uniform" and "no
    /// uncaught exception" all at once.
    func testDocumentStartInstallsABoundedRuntimeErrorLedger() {
        let source = LocalAppWebViewRepresentable.bridgeSourceTemplate
        for token in [
            "addEventListener('error'",
            "addEventListener('unhandledrejection'",
            // The one that actually catches a React ErrorBoundary.
            "console.error = function",
            "kind: 'console'",
            // Bounded on both axes, and cleared per document.
            // The bound itself, spelled the way the code spells it.
            "const cap = 8",
            "__lingxiRuntimeErrors.length >= cap",
            "__lingxiRuntimeErrorsDropped",
        ] {
            XCTAssertTrue(source.contains(token), "missing runtime-error ledger: \(token)")
        }
        let snapshotSource = LocalAppWebViewController.executionSource(requestJSON: "{}")
        XCTAssertTrue(
            snapshotSource.contains("runtimeErrors:"),
            "the ledger must surface in the inspect snapshot, not only in the page"
        )
    }

    /// Phase 1a — `result_json` over 256 KiB is a hard failure, not a truncation
    /// (`LocalAppWebView.swift:407-411`). The ledger must not be able to silence
    /// the criterion it exists to feed.
    ///
    /// Fix round 1: code review found the budget measured `.length` — UTF-16
    /// CODE UNITS — while the guard above measures `resultJSON.utf8.count` —
    /// real UTF-8 BYTES. A CJK character is 1 code unit but 3 bytes, so a
    /// length-only check could call a payload "safe" at roughly a third of
    /// its true size, and this product's default content is Chinese. The
    /// `TextEncoder` token below is pinned so a future edit back to `.length`
    /// fails here instead of silently reintroducing the gap.
    func testSnapshotDegradesInAFixedOrderAndSaysSo() {
        let source = LocalAppWebViewController.executionSource(requestJSON: "{}")
        for token in [
            "const BUDGET = 200 * 1024",
            "for (const seg of ['elements', 'canvases', 'runtimeErrors'])",
            "truncated.push(seg)",
            "truncated: []",
            // Still TextEncoder (not `.length`), but through the reference the
            // bootstrap captured before page code could shadow it.
            "new (window.__lingxiTextEncoder || TextEncoder)().encode(JSON.stringify(out)).length",
        ] {
            XCTAssertTrue(source.contains(token), "missing payload budget: \(token)")
        }
    }

    /// Final review, finding 5 — `size()` calls `new TextEncoder()` at snapshot
    /// time, and `window.TextEncoder` is PAGE-CONTROLLABLE. An app that shadows
    /// it made `snapshot()` throw, so `inspect_ui` returned an error for the
    /// whole app instead of a snapshot — a strictly worse failure than the
    /// `.length` measurement it replaced, which could not throw at all.
    ///
    /// The ledger above already solved this class by binding `console.error`
    /// at document-start; this pins the same treatment for `TextEncoder`, on
    /// both halves: the capture in the bootstrap and the USE in the snapshot.
    func testDocumentStartCapturesTextEncoderBeforeThePageCanShadowIt() {
        let bootstrap = LocalAppWebViewRepresentable.bridgeSourceTemplate
        for token in [
            "Object.defineProperty(window, '__lingxiTextEncoder', { value: window.TextEncoder });",
            "if (!window.__lingxiTextEncoder) {",
        ] {
            XCTAssertTrue(bootstrap.contains(token), "missing captured TextEncoder: \(token)")
        }
        XCTAssertFalse(
            LocalAppWebViewController.executionSource(requestJSON: "{}")
                .contains("new TextEncoder().encode(JSON.stringify(out))"),
            "the snapshot budget must not reach for the page-controllable global"
        )
    }

    /// The conversation-scope cwd (`RootView.makeSource(scope: .localApp(id))`)
    /// and the code browser resolve the SAME validated workspace directory —
    /// one derivation, `LocalAppWorkspacePath`, no second copy to diverge.
    func testWorkspacePathDerivationIsSharedAndValidated() throws {
        let root = try LocalAppWorkspacePath.validatedRoot(appID: "tracker")
        let viaRelative = try LocalAppWorkspacePath.validatedRoot(
            relativePath: LocalAppWorkspacePath.relativePath(appID: "tracker")
        )
        XCTAssertEqual(root, viaRelative)
        XCTAssertTrue(root.path.hasSuffix("/apps/tracker/workspace"))
        XCTAssertTrue(root.path.hasPrefix(ConversationSourceFactory.appSandboxRoot()))

        XCTAssertThrowsError(try LocalAppWorkspacePath.validatedRoot(appID: "../escape"))
        XCTAssertThrowsError(try LocalAppWorkspacePath.validatedRoot(appID: "UPPER"))
        XCTAssertThrowsError(try LocalAppWorkspacePath.validatedRoot(relativePath: "/apps/tracker/workspace"))
        XCTAssertThrowsError(try LocalAppWorkspacePath.validatedRoot(relativePath: "apps/tracker/elsewhere"))
    }

    func testLocalAppWorkspaceBuildsDedicatedMobileLinuxRuntimeConfig() throws {
        let config = try XCTUnwrap(
            LocalAppWorkspacePath.mobileLinuxRuntimeConfig(appID: "tracker")
        )

        XCTAssertEqual(config.mode, .mobileLinux)
        XCTAssertEqual(
            config.workspaceHostPath,
            ConversationSourceFactory.appSandboxRoot()
                + "/" + LocalAppWorkspacePath.relativePath(appID: "tracker")
        )
        XCTAssertEqual(config.stableWorkspaceId, "local-app-tracker")
        XCTAssertFalse(config.managedRoot.isEmpty)
        XCTAssertFalse(config.rootfsVersion.isEmpty)
    }

    private func makeBridgeCaptureWebView() async throws -> WKWebView {
        let webView = WKWebView()
        webView.loadHTMLString(#"<div id="ready">ready</div>"#, baseURL: nil)
        var loaded = false
        for _ in 0 ..< 100 {
            if (try? await webView.evaluateJavaScript("document.getElementById('ready') !== null") as? Bool) == true {
                loaded = true
                break
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(loaded, "bridge capture WebView did not finish loading")
        _ = try await webView.evaluateJavaScript(
            "window.__bridgeEnvelope = null; window.lingxi = { __resolve: value => window.__bridgeEnvelope = value }; true"
        )
        return webView
    }

    private func bridgeEnvelope(containing code: String, in webView: WKWebView) async throws -> String? {
        for _ in 0 ..< 100 {
            let value = try await webView.evaluateJavaScript("JSON.stringify(window.__bridgeEnvelope)") as? String
            if value?.contains(code) == true { return value }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("Timed out waiting for bridge error code \(code)")
        return nil
    }

    #if canImport(engine_mobileFFI)
        func testRejectedDeleteCancelsOnlyItsUnconfirmedCleanupJournal() async throws {
            enum ExpectedFailure: Error { case rejected }
            let suiteName = "LocalAppsStoreTests.delete-rejected.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer { defaults.removePersistentDomain(forName: suiteName) }

            var removedIdentifiers: [UUID] = []
            let registry = LocalAppWebsiteDataStoreRegistry(
                defaults: defaults,
                removeDataStore: { identifier in
                    removedIdentifiers.append(identifier)
                    return true
                }
            )
            let originalIdentifier = try XCTUnwrap(registry.dataStore(appID: "tracker").identifier)
            let store = LocalAppsStore(websiteDataStoreRegistry: registry)
            store.configure { _ in throw ExpectedFailure.rejected }

            let accepted = await store.delete(appID: "tracker")
            XCTAssertFalse(accepted)
            XCTAssertTrue(registry.pendingCleanups.isEmpty)
            XCTAssertEqual(registry.storedIdentifier(appID: "tracker"), originalIdentifier)

            await registry.removeDataForDeletedApps(activeAppIDs: [])
            XCTAssertTrue(removedIdentifiers.isEmpty)
        }

        func testDeleteJournalsBeforeCommandAndCleansOnlyAfterSnapshotDropsApp() async throws {
            let suiteName = "LocalAppsStoreTests.delete-cleanup.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer { defaults.removePersistentDomain(forName: suiteName) }

            var removedIdentifiers: [UUID] = []
            let registry = LocalAppWebsiteDataStoreRegistry(
                defaults: defaults,
                removeDataStore: { identifier in
                    removedIdentifiers.append(identifier)
                    return true
                }
            )
            let store = LocalAppsStore(websiteDataStoreRegistry: registry)
            var pendingAtSubmission: [LocalAppWebsiteDataStoreRegistry.PendingCleanup] = []
            store.configure { _ in pendingAtSubmission = registry.pendingCleanups }
            store.handle(event: .appsChanged(apps: [appRecord(id: "tracker", name: "Tracker")]))

            let submitted = await store.delete(appID: "tracker")
            XCTAssertTrue(submitted)
            XCTAssertEqual(pendingAtSubmission.count, 1, "the cleanup journal must precede the delete command")

            store.handle(event: .appsChanged(apps: [appRecord(id: "tracker", name: "Tracker")]))
            try await Task.sleep(for: .milliseconds(20))
            XCTAssertTrue(removedIdentifiers.isEmpty)

            store.handle(event: .appsChanged(apps: []))
            try await waitUntil("the confirmed deletion cleanup") { removedIdentifiers.count == 1 }
            XCTAssertTrue(registry.pendingCleanups.isEmpty)
        }

        func testRecreatedAppGetsANewDataStoreBeforeOldRemovalCompletes() async throws {
            let suiteName = "LocalAppsStoreTests.website-cleanup-race.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer { defaults.removePersistentDomain(forName: suiteName) }

            var removalContinuation: CheckedContinuation<Bool, Never>?
            let registry = LocalAppWebsiteDataStoreRegistry(
                defaults: defaults,
                removeDataStore: { _ in
                    await withCheckedContinuation { continuation in
                        removalContinuation = continuation
                    }
                }
            )
            registry.prepareForDeletion(appID: "tracker")
            let oldIdentifier = try XCTUnwrap(registry.storedIdentifier(appID: "tracker"))

            let cleanup = Task { @MainActor in
                await registry.removeDataForDeletedApps(activeAppIDs: [])
            }
            try await waitUntil("the old store removal to start") { removalContinuation != nil }
            XCTAssertEqual(registry.pendingCleanups.first?.absenceConfirmed, true)

            let recreatedIdentifier = try XCTUnwrap(registry.dataStore(appID: "tracker").identifier)
            XCTAssertNotEqual(recreatedIdentifier, oldIdentifier)
            XCTAssertEqual(registry.storedIdentifier(appID: "tracker"), recreatedIdentifier)

            let continuation = removalContinuation
            removalContinuation = nil
            continuation?.resume(returning: true)
            await cleanup.value

            XCTAssertTrue(registry.pendingCleanups.isEmpty)
            XCTAssertEqual(
                registry.storedIdentifier(appID: "tracker"),
                recreatedIdentifier,
                "finishing old cleanup must not delete the recreated app's mapping"
            )
        }

        func testDeletedWebsiteDataStoreDoesNotRestoreLocalStorageWhenAppIDIsReused() async throws {
            let suiteName = "LocalAppsStoreTests.website-cleanup-real.\(UUID().uuidString)"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
            defer { defaults.removePersistentDomain(forName: suiteName) }
            let registry = LocalAppWebsiteDataStoreRegistry(defaults: defaults)
            let origin = URL(string: "http://127.0.0.1:43199")!

            func seedStore() async throws -> UUID {
                let configuration = WKWebViewConfiguration()
                configuration.websiteDataStore = registry.dataStore(appID: "tracker")
                let webView = WKWebView(frame: .zero, configuration: configuration)
                webView.loadHTMLString(#"<div id="ready">ready</div>"#, baseURL: origin)
                try await waitForElement("ready", in: webView)
                _ = try await webView.evaluateJavaScript("localStorage.setItem('lingxi-test', 'old-value')")
                return try XCTUnwrap(configuration.websiteDataStore.identifier)
            }

            func readRecreatedStore() async throws -> (UUID, Bool) {
                let configuration = WKWebViewConfiguration()
                configuration.websiteDataStore = registry.dataStore(appID: "tracker")
                let webView = WKWebView(frame: .zero, configuration: configuration)
                webView.loadHTMLString(#"<div id="ready">ready</div>"#, baseURL: origin)
                try await waitForElement("ready", in: webView)
                let valueIsMissing = try await webView.evaluateJavaScript(
                    "localStorage.getItem('lingxi-test') === null"
                ) as? Bool
                return (
                    try XCTUnwrap(configuration.websiteDataStore.identifier),
                    try XCTUnwrap(valueIsMissing)
                )
            }

            let deletedIdentifier = try await seedStore()
            registry.prepareForDeletion(appID: "tracker")
            for _ in 0 ..< 20 where !registry.pendingCleanups.isEmpty {
                await registry.removeDataForDeletedApps(activeAppIDs: [])
                if !registry.pendingCleanups.isEmpty {
                    try await Task.sleep(for: .milliseconds(50))
                }
            }
            XCTAssertTrue(registry.pendingCleanups.isEmpty, "WebKit must release and remove the identified store")

            let (recreatedIdentifier, staleValueIsMissing) = try await readRecreatedStore()
            XCTAssertNotEqual(recreatedIdentifier, deletedIdentifier)
            XCTAssertTrue(
                staleValueIsMissing,
                "a recreated app id must not recover the deleted app's local storage"
            )
        }

        // MARK: - v3 session catalog (ListAppSessions → AppSessionsChanged)

        /// The reduce pins the init row first and keeps the rest in the wire's
        /// modified-descending order — checked against a payload whose init
        /// row is deliberately NOT first, so the pinning cannot pass vacuously.
        func testAppSessionsReducePinsTheInitRowFirst() async {
            let store = LocalAppsStore()
            store.configure { _ in }

            await store.listSessions(appID: "tracker")
            store.handle(event: .appSessionsChanged(
                appId: "tracker",
                sessions: [
                    appSessionRow(uuid: "s-newest", title: "最近会话", modified: "2026-08-09T10:00:00Z", messageCount: 4),
                    appSessionRow(uuid: "s-init", title: "初始化会话", modified: "2026-08-01T09:00:00Z", messageCount: 12, kind: .`init`),
                    appSessionRow(uuid: "s-older", title: "旧会话", modified: "2026-08-02T10:00:00Z", messageCount: 2),
                ],
                nextOffset: nil
            ))

            let page = store.sessionPages["tracker"]
            XCTAssertEqual(page?.rows.map(\.uuid), ["s-init", "s-newest", "s-older"])
            XCTAssertEqual(page?.rows.first?.isInit, true)
            XCTAssertEqual(page?.rows.first?.messageCount, 12)
            XCTAssertNil(page?.nextOffset)
        }

        func testAppSessionsPreserveChatModeForNavigation() async {
            let store = LocalAppsStore()
            store.configure { _ in }

            store.handle(event: .appSessionsChanged(
                appId: "tracker",
                sessions: [appSessionRow(uuid: "s-chat", mode: .chat)],
                nextOffset: nil
            ))

            XCTAssertEqual(store.sessionPages["tracker"]?.rows.first?.mode, .chat)
        }

        func testAppsSnapshotEagerlyLoadsEveryMissingSessionCatalog() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            store.handle(event: .appsChanged(apps: [
                appRecord(id: "tracker", name: "Tracker"),
                appRecord(id: "weather", name: "Weather"),
            ]))

            try await waitUntil("all app session catalogs to be requested") {
                Set(submitted.compactMap { command -> String? in
                    guard case let .listAppSessions(appID, _, _) = command else { return nil }
                    return appID
                }) == Set(["tracker", "weather"])
            }
        }

        /// Paging: `loadMoreSessions` requests the reply's `nextOffset`, the
        /// later page APPENDS (deduplicating a row the pages share), and a
        /// fresh offset-0 refresh REPLACES the accumulated rows.
        func testAppSessionsPagingAppendsDeduplicatesAndRefreshReplaces() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            await store.listSessions(appID: "tracker")
            guard case let .listAppSessions(_, firstOffset, _) = submitted.last else {
                return XCTFail("Expected a ListAppSessions command")
            }
            XCTAssertNil(firstOffset)
            store.handle(event: .appSessionsChanged(
                appId: "tracker",
                sessions: [
                    appSessionRow(uuid: "s-init", kind: .`init`),
                    appSessionRow(uuid: "s-1"),
                ],
                nextOffset: 2
            ))
            XCTAssertEqual(store.sessionPages["tracker"]?.nextOffset, 2)

            await store.loadMoreSessions(appID: "tracker")
            guard case let .listAppSessions(appId, offset, _) = submitted.last else {
                return XCTFail("Expected the next-page ListAppSessions command")
            }
            XCTAssertEqual(appId, "tracker")
            XCTAssertEqual(offset, 2)
            store.handle(event: .appSessionsChanged(
                appId: "tracker",
                sessions: [
                    appSessionRow(uuid: "s-1"), // shared with page 1 — must not duplicate
                    appSessionRow(uuid: "s-2"),
                ],
                nextOffset: nil
            ))
            XCTAssertEqual(
                store.sessionPages["tracker"]?.rows.map(\.uuid),
                ["s-init", "s-1", "s-2"]
            )
            XCTAssertNil(store.sessionPages["tracker"]?.nextOffset, "the last page ends paging")

            // A fresh refresh replaces the accumulated catalog.
            await store.listSessions(appID: "tracker")
            store.handle(event: .appSessionsChanged(
                appId: "tracker",
                sessions: [appSessionRow(uuid: "s-9")],
                nextOffset: nil
            ))
            XCTAssertEqual(store.sessionPages["tracker"]?.rows.map(\.uuid), ["s-9"])
        }

        /// `AppRecordDto.initSessionId` must survive the summary mapping —
        /// it is the pinned first row's identity in the session catalog.
        func testAppRecordMapsInitSessionId() {
            let summary = LocalAppsProtocolAdapter.app(
                appRecord(id: "tracker", name: "Tracker", initSessionId: "11111111-1111-4111-8111-111111111111")
            )
            XCTAssertEqual(summary.initSessionId, "11111111-1111-4111-8111-111111111111")
            XCTAssertEqual(summary.workflow, .publishedUnverified)

            let draft = LocalAppsProtocolAdapter.app(
                appRecord(id: "draft-app", name: "Draft", workflowState: .draft)
            )
            XCTAssertEqual(draft.workflow, .draft)
            XCTAssertNil(draft.initSessionId)

            let verified = LocalAppsProtocolAdapter.app(
                appRecord(id: "verified-app", name: "Verified", workflowState: .publishedVerified)
            )
            XCTAssertEqual(verified.workflow, .publishedVerified)
        }

        /// The details snapshot's manifest is the only structured data-model
        /// description now — its collections must reach the store.
        func testAppDetailsMirrorsManifestCollections() {
            let store = LocalAppsStore()
            store.handle(event: .appEvent(event: .appDetailsChanged(details: AppDetailsDto(
                app: appRecord(id: "tracker", name: "Tracker"),
                manifest: AppManifestDto(
                    schemaVersion: 2,
                    runtimeApiVersion: 2,
                    appId: "tracker",
                    name: "Tracker",
                    designRevision: 3,
                    collections: [AppDataCollectionDto(
                        id: "notes",
                        label: "Notes",
                        fields: [AppDataFieldDto(id: "title", label: "Title", fieldType: .text, required: true, options: [])],
                        enabledByDefault: true
                    )],
                    allowedDomains: [],
                    capabilities: [],
                    deviceContext: nil,
                    surface: nil,
                    runtimeProfile: nil,
                    dependencySnapshot: nil
                ),
                runtimeProfileStatus: .verified,
                runtime: AppRuntimeDetailsDto(
                    state: .stopped,
                    mode: .nextProduction,
                    loopbackUrl: nil,
                    suspensionReason: nil,
                    recoveryState: .recovered,
                    lastError: nil
                ),
                checkpoints: []
            ))))

            XCTAssertEqual(store.collections["tracker"]?.count, 1)
            XCTAssertEqual(store.collections["tracker"]?.first?.fields.first?.id, "title")
            XCTAssertEqual(store.apps.map(\.id), ["tracker"])
            XCTAssertEqual(store.app(id: "tracker")?.runtimeProfileStatus, .verified)
        }

        func testRuntimeProfileStatusMapsAllWireValuesAndSurvivesListRefresh() {
            let mappings: [(AppRuntimeProfileStatusDto, LocalAppRuntimeProfileStatus)] = [
                (.verified, .verified),
                (.dependenciesDirty, .dependenciesDirty),
                (.coreDependencyDrift, .coreDependencyDrift),
                (.rebuildRequired, .rebuildRequired),
                (.migrationAvailable, .migrationAvailable),
                (.runtimeBundleMissing, .runtimeBundleMissing),
                (.runtimeContractCorrupt, .runtimeContractCorrupt),
            ]
            for (wire, local) in mappings {
                XCTAssertEqual(LocalAppsProtocolAdapter.runtimeProfileStatus(wire), local)
            }

            let store = LocalAppsStore()
            store.handle(event: .appEvent(event: .appDetailsChanged(details: AppDetailsDto(
                app: appRecord(id: "tracker", name: "Tracker"),
                manifest: nil,
                runtimeProfileStatus: .runtimeContractCorrupt,
                runtime: AppRuntimeDetailsDto(
                    state: .stopped,
                    mode: .nextProduction,
                    loopbackUrl: nil,
                    suspensionReason: nil,
                    recoveryState: .recovered,
                    lastError: nil
                ),
                checkpoints: []
            ))))
            XCTAssertEqual(store.app(id: "tracker")?.runtimeProfileStatus, .runtimeContractCorrupt)

            // The list endpoint does not carry details; refreshing it must
            // retain the last known host snapshot so a card does not flicker
            // back to an unknown state.
            store.handle(event: .appsChanged(apps: [appRecord(id: "tracker", name: "Tracker")]))
            XCTAssertEqual(store.app(id: "tracker")?.runtimeProfileStatus, .runtimeContractCorrupt)
        }

        func testRuntimeDetailsUseTheLoopbackURL() {
            let store = LocalAppsStore()
            store.handle(event: .appRuntimeChanged(
                appId: "tracker",
                state: .running,
                details: AppRuntimeDetailsDto(
                    state: .running,
                    mode: .nextProduction,
                    loopbackUrl: "http://127.0.0.1:43123",
                    suspensionReason: nil,
                    recoveryState: .recovered,
                    lastError: nil
                ),
                lastError: nil
            ))
            XCTAssertEqual(store.runtimes["tracker"]?.url?.absoluteString, "http://127.0.0.1:43123")
        }

        func testInspectRefreshesRuntimeDetailsBeforeReadingThePreview() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            broker.webView = webView
            controller.markNotReady()
            defer { LocalAppWebViewRegistry.shared.unregister(controller, appID: "tracker") }

            store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                requestId: "inspect-1",
                appId: "tracker",
                action: .inspect,
                target: nil,
                value: nil
            ))))

            try await waitUntil("the preview details request to be submitted") {
                submitted.contains {
                    if case let .getAppDetails(appId) = $0 { appId == "tracker" } else { false }
                }
            }
            try await Task.sleep(for: .milliseconds(100))
            XCTAssertFalse(submitted.contains {
                if case let .resolveAppUiRequest(requestId, _, _, _) = $0 {
                    requestId == "inspect-1"
                } else {
                    false
                }
            }, "Inspect must wait for the preview document, not only its controller")

            // The runtime-details event mounts the preview after the request
            // has already started waiting. Registration alone is not enough;
            // the document must also finish loading before the request runs.
            LocalAppWebViewRegistry.shared.register(controller, appID: "tracker")
            webView.loadHTMLString(
                #"<main id="preview-ready">Ready</main>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("preview-ready", in: webView)
            controller.markReady()

            try await waitUntil("the preview inspection to resolve") {
                submitted.contains {
                    if case let .resolveAppUiRequest(requestId, _, _, _) = $0 {
                        requestId == "inspect-1"
                    } else {
                        false
                    }
                }
            }

            let detailsIndex = try XCTUnwrap(submitted.firstIndex {
                if case let .getAppDetails(appId) = $0 { appId == "tracker" } else { false }
            })
            let resolutionIndex = try XCTUnwrap(submitted.firstIndex {
                if case let .resolveAppUiRequest(requestId, _, _, _) = $0 {
                    requestId == "inspect-1"
                } else {
                    false
                }
            })
            XCTAssertLessThan(
                detailsIndex,
                resolutionIndex,
                "Inspect must hydrate the runtime URL before it asks the preview WebView to render"
            )
        }

        func testCapabilityPromptResolvesWithSessionAuthorization() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appEvent(event: .appCapabilityRequested(request: AppCapabilityRequestDto(
                requestId: "permission-1",
                appId: "tracker",
                capability: .networkDomain,
                domain: "api.example.com",
                reason: "读取公开数据"
            ))))

            XCTAssertEqual(store.pendingPermission?.domain, "api.example.com")
            await store.resolvePendingPermission(.session)

            guard case let .resolveAppCapabilityRequest(requestId, decision) = submitted.last else {
                return XCTFail("Expected capability resolution command")
            }
            XCTAssertEqual(requestId, "permission-1")
            XCTAssertEqual(decision, .allowSession)
            XCTAssertNil(store.pendingPermission)
        }

        func testDependencyChangePromptOnlyAllowsOneShotAuthorization() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appEvent(event: .appCapabilityRequested(request: AppCapabilityRequestDto(
                requestId: "permission-runtime-1",
                appId: "tracker",
                capability: .dependencyChange,
                domain: nil,
                reason: "Review dependency change"
            ))))

            XCTAssertEqual(store.pendingPermission?.allowsPersistentGrant, false)
            await store.resolvePendingPermission(.always)

            guard case let .resolveAppCapabilityRequest(requestId, decision) = submitted.last else {
                return XCTFail("Expected capability resolution command")
            }
            XCTAssertEqual(requestId, "permission-runtime-1")
            XCTAssertEqual(decision, .allowOnce)
            XCTAssertNil(store.pendingPermission)
        }

        func testDependencyChangeConfirmationShowsPolicyAndReturnsApproval() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appEvent(event: .appDependencyChangeConfirmationRequested(
                request: AppDependencyChangeConfirmationRequestDto(
                    requestId: "dependency-confirm-1",
                    appId: "tracker",
                    reason: "pre_resolution_no_network",
                    changes: [
                        AppDependencyChangeDto(
                            kind: .add,
                            package: "dayjs",
                            version: "1.11.13",
                            cacheStatus: "unknown_until_resolution",
                            downloadStatus: "may_be_required"
                        ),
                    ],
                    licenseRisk: "unknown_until_resolution",
                    sbomRisk: "unknown_until_resolution",
                    lifecycleScriptsBlocked: true,
                    nativeAddonsBlocked: true,
                    rollbackPolicy: "rollback_on_validation_failure"
                )
            )))

            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.id, "dependency-confirm-1")
            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.changes.first?.package, "dayjs")
            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.licenseRisk, "unknown_until_resolution")
            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.lifecycleScriptsBlocked, true)
            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.nativeAddonsBlocked, true)

            await store.resolvePendingDependencyChangeConfirmation(true)

            guard case let .resolveAppDependencyChangeConfirmation(requestId, approved) = submitted.last else {
                return XCTFail("Expected dependency confirmation resolution command")
            }
            XCTAssertEqual(requestId, "dependency-confirm-1")
            XCTAssertTrue(approved)
            XCTAssertNil(store.pendingDependencyChangeConfirmation)
        }

        func testDependencyChangeConfirmationQueuesAndCancelIsOneShot() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            let request = { (id: String) in
                AppDependencyChangeConfirmationRequestDto(
                    requestId: id,
                    appId: "tracker",
                    reason: "pre_resolution_no_network",
                    changes: [
                        AppDependencyChangeDto(
                            kind: .update,
                            package: "zod",
                            version: "4.4.3",
                            cacheStatus: "unknown_until_resolution",
                            downloadStatus: "may_be_required"
                        ),
                    ],
                    licenseRisk: "unknown_until_resolution",
                    sbomRisk: "unknown_until_resolution",
                    lifecycleScriptsBlocked: true,
                    nativeAddonsBlocked: true,
                    rollbackPolicy: "rollback_on_validation_failure"
                )
            }
            store.handle(event: .appEvent(event: .appDependencyChangeConfirmationRequested(request: request("dependency-confirm-a"))))
            store.handle(event: .appEvent(event: .appDependencyChangeConfirmationRequested(request: request("dependency-confirm-b"))))
            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.id, "dependency-confirm-a")

            await store.resolvePendingDependencyChangeConfirmation(false)

            guard case let .resolveAppDependencyChangeConfirmation(requestId, approved) = submitted.first else {
                return XCTFail("Expected dependency confirmation cancellation command")
            }
            XCTAssertEqual(requestId, "dependency-confirm-a")
            XCTAssertFalse(approved)
            XCTAssertEqual(store.pendingDependencyChangeConfirmation?.id, "dependency-confirm-b")
        }

        /// Every operation the injected page bridge advertises must reach a
        /// wire operation — and the table is DERIVED from the injected
        /// JavaScript, not hand-copied beside it.
        func testEveryInjectedBridgeOperationMapsToAWireOperation() async {
            // `request('Device', 'capturePhoto', …)` in the injected source.
            let pattern = try! NSRegularExpression(pattern: #"request\('(\w+)',\s*'(\w+)'"#)
            let source = LocalAppWebViewRepresentable.bridgeSource
            let matches = pattern.matches(
                in: source,
                range: NSRange(source.startIndex..., in: source))
            let advertised: [(namespace: String, operation: String)] = matches.compactMap {
                guard let ns = Range($0.range(at: 1), in: source),
                      let op = Range($0.range(at: 2), in: source)
                else { return nil }
                // The injected JS names the handler suffix (`Device`); the
                // store keys on the lowercased namespace (`device`).
                return (String(source[ns]).lowercased(), String(source[op]))
            }
            XCTAssertGreaterThanOrEqual(
                advertised.count, 13,
                "the bridge should advertise at least the data/network/runtime/device/llm/agent surface")

            for (namespace, operation) in advertised {
                let store = LocalAppsStore()
                var submitted: [ClientCommand] = []
                store.configure { command in submitted.append(command) }
                await store.executeBridge(LocalAppBridgeRequest(
                    id: "req-\(namespace)-\(operation)",
                    appID: "tracker",
                    namespace: namespace,
                    operation: operation,
                    payloadJSON: "{}"
                ))
                guard case .executeAppBridgeRequest = submitted.last else {
                    return XCTFail(
                        "\(namespace).\(operation) is advertised by the injected bridge but "
                            + "reaches no wire operation")
                }
            }
        }

        func testTheBridgePayloadCapMatchesTheDocumentedContract() {
            XCTAssertEqual(
                LocalAppBridgeBroker.maxControlBytes, 64 * 1024,
                "control operations must retain a small bounded request surface")
            XCTAssertEqual(
                LocalAppBridgeBroker.maxLLMBytes, 8 * 1024 * 1024,
                "long model input needs a separate bounded lane; media still travels by mediaId")
        }

        func testEveryInjectedNamespaceHasARegisteredMessageHandler() {
            let pattern = try! NSRegularExpression(pattern: #"request\('(\w+)'"#)
            let source = LocalAppWebViewRepresentable.bridgeSource
            let namespaces = Set(pattern.matches(
                in: source,
                range: NSRange(source.startIndex..., in: source)
            ).compactMap { match -> String? in
                guard let range = Range(match.range(at: 1), in: source) else { return nil }
                return "lingxi\(source[range])"
            })
            XCTAssertFalse(namespaces.isEmpty)
            for handler in namespaces {
                XCTAssertTrue(
                    LocalAppWebViewRepresentable.messageHandlerNames.contains(handler),
                    "\(handler) is called by the injected bridge but never registered")
            }
        }

        func testInjectedBridgeAttachesTheErrorCodeToRejections() {
            let source = LocalAppWebViewRepresentable.bridgeSource
            XCTAssertTrue(
                source.contains("error.code = envelope.code"),
                "a failure envelope's `code` must reach page code as `error.code`")
            XCTAssertTrue(source.contains("const channel = pending.get(frame.requestId)?.channel"))
            XCTAssertTrue(source.contains("streamListeners.set(listener, 'llm')"))
            XCTAssertTrue(source.contains("streamListeners.set(listener, 'agent')"))
        }

        func testLlmActivityAndAgentEventsUpdateTheStore() {
            let store = LocalAppsStore()
            store.configure { _ in }

            store.handle(event: .appEvent(event: .appLlmActivityChanged(appId: "tracker", active: true)))
            XCTAssertTrue(store.llmActiveAppIDs.contains("tracker"))
            store.handle(event: .appEvent(event: .appLlmActivityChanged(appId: "tracker", active: false)))
            XCTAssertFalse(store.llmActiveAppIDs.contains("tracker"))

            store.handle(event: .appEvent(event: .appAgentEventPosted(
                appId: "tracker", seq: 1, topic: "timer.done", createdAtMs: 1
            )))
            store.handle(event: .appEvent(event: .appAgentEventPosted(
                appId: "tracker", seq: 2, topic: "note.added", createdAtMs: 2
            )))
            XCTAssertEqual(store.unreadAgentEvents["tracker"], 2)
        }

        func testResetPermissionsSubmitsAppScopedRevocation() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let accepted = await store.resetPermissions(appID: "tracker")
            XCTAssertTrue(accepted)
            guard case let .resetAppPermissions(appId) = submitted.last else {
                return XCTFail("Expected reset app permissions command")
            }
            XCTAssertEqual(appId, "tracker")
        }

        func testForegroundRefreshesSnapshotsAndRestartsPreviouslyActiveRuntime() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "tracker", name: "Tracker", brief: "跟踪任务"),
            ]))
            store.handle(event: .appRuntimeChanged(
                appId: "tracker",
                state: .starting,
                details: nil,
                lastError: nil
            ))

            store.sceneDidEnterBackground()
            await store.sceneWillEnterForeground()

            XCTAssertTrue(submitted.contains { if case .listApps = $0 { true } else { false } })
            XCTAssertTrue(submitted.contains {
                if case let .getAppDetails(appId) = $0 { return appId == "tracker" }
                return false
            })
            XCTAssertTrue(submitted.contains {
                if case let .startApp(appId) = $0 { return appId == "tracker" }
                return false
            })
            UserDefaults.standard.removeObject(forKey: "local-apps.running-before-suspension")
        }

        func testUiCapabilityGrantExecutesNextStructuredRequestAndReturnsResult() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appEvent(event: .appCapabilityRequested(request: AppCapabilityRequestDto(
                requestId: "capability-1",
                appId: "tracker",
                capability: .uiControl,
                domain: nil,
                reason: "填写标题"
            ))))
            await store.resolvePendingPermission(.once)

            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            broker.webView = webView
            LocalAppWebViewRegistry.shared.register(controller, appID: "tracker")
            defer { LocalAppWebViewRegistry.shared.unregister(controller, appID: "tracker") }
            webView.loadHTMLString(#"<input id="title">"#, baseURL: URL(string: "http://127.0.0.1:43123"))
            try await waitForElement("title", in: webView)

            store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                requestId: "ui-1",
                appId: "tracker",
                action: .fill,
                target: AppUiTargetDto(elementId: "title", role: nil, name: nil),
                value: "Orders"
            ))))
            for _ in 0 ..< 50 {
                if submitted.contains(where: {
                    if case let .resolveAppUiRequest(requestId, _, _, _) = $0 { return requestId == "ui-1" }
                    return false
                }) { break }
                try await Task.sleep(for: .milliseconds(20))
            }

            XCTAssertNil(store.pendingPermission, "Capability approval should prevent a duplicate UI prompt")
            guard let command = submitted.first(where: {
                if case let .resolveAppUiRequest(requestId, _, _, _) = $0 { return requestId == "ui-1" }
                return false
            }), case let .resolveAppUiRequest(_, decision, resultJSON, error) = command else {
                return XCTFail("Expected UI action result command")
            }
            XCTAssertEqual(decision, .allowOnce)
            XCTAssertNotNil(resultJSON)
            XCTAssertNil(error)
            let value = try await webView.evaluateJavaScript("document.getElementById('title').value") as? String
            XCTAssertEqual(value, "Orders")

            _ = try await webView.evaluateJavaScript(
                "window.__bridgeEnvelope = null; window.lingxi = { __resolve: value => window.__bridgeEnvelope = value }; true"
            )
            LocalAppWebViewRegistry.shared.resolveBridge(
                appID: "tracker",
                requestID: "bridge-1",
                resultJSON: #"{"records":[1]}"#,
                error: nil
            )
            try await Task.sleep(for: .milliseconds(20))
            let envelope = try await webView.evaluateJavaScript("JSON.stringify(window.__bridgeEnvelope)") as? String
            XCTAssertTrue(envelope?.contains("bridge-1") == true)
            XCTAssertTrue(envelope?.contains("records") == true)
        }

        func testStructuredWebViewActionsInspectAndFillWithoutArbitraryScriptInput() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            webView.loadHTMLString(
                #"<label for="title">Title</label><input id="title"><button id="save">Save</button>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("title", in: webView)

            let inspect = await controller.execute(request: AppUiRequestDto(
                requestId: "inspect-1",
                appId: "tracker",
                action: .inspect,
                target: nil,
                value: nil
            ))
            XCTAssertNil(inspect.error)
            XCTAssertTrue(inspect.resultJSON?.contains("title") == true)

            let fill = await controller.execute(request: AppUiRequestDto(
                requestId: "fill-1",
                appId: "tracker",
                action: .fill,
                target: AppUiTargetDto(elementId: "title", role: nil, name: nil),
                value: "Orders"
            ))
            XCTAssertNil(fill.error)

            let value = try await webView.evaluateJavaScript("document.getElementById('title').value") as? String
            XCTAssertEqual(value, "Orders")
        }

        func testQaExecutionPreservesExplicitNullActionValueAndAttestsTheFinishedDocument() async throws {
            let runtimeURL = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView(frame: CGRect(x: 0, y: 0, width: 393, height: 852))
            controller.webView = webView
            let generation = controller.beginNavigation(expectedURL: runtimeURL)
            webView.loadHTMLString(#"<input id="title" value="Orders">"#, baseURL: runtimeURL)
            try await waitForElement("title", in: webView)
            let committedURL = try XCTUnwrap(webView.url)
            XCTAssertTrue(LocalAppWebViewController.sameRuntimeIdentity(runtimeURL, committedURL))
            controller.markReady(committedURL: committedURL, navigationGeneration: generation)

            let wrapperData = try JSONSerialization.data(withJSONObject: [
                "lingxi_qa": [
                    "version": 1,
                    "expected_runtime_url": runtimeURL.absoluteString,
                    "action_value": NSNull(),
                ],
            ])
            let wrapper = try XCTUnwrap(String(data: wrapperData, encoding: .utf8))
            let response = await controller.execute(request: AppUiRequestDto(
                requestId: "qa-ui-fill",
                appId: "tracker",
                action: .fill,
                target: AppUiTargetDto(elementId: "title", role: nil, name: nil),
                value: wrapper
            ))

            XCTAssertNil(response.error)
            let fieldValue = try await webView.evaluateJavaScript(
                "document.getElementById('title').value"
            ) as? String
            XCTAssertEqual(
                fieldValue,
                "",
                "action_value:null must remain null, not become the QA wrapper text"
            )
            let responseData = try XCTUnwrap(response.resultJSON?.data(using: .utf8))
            let object = try XCTUnwrap(
                JSONSerialization.jsonObject(with: responseData) as? [String: Any]
            )
            let attestation = try XCTUnwrap(object["lingxi_qa"] as? [String: Any])
            XCTAssertEqual(attestation["requested_runtime_url"] as? String, runtimeURL.absoluteString)
            XCTAssertEqual(attestation["loaded_runtime_url"] as? String, committedURL.absoluteString)
            XCTAssertEqual(attestation["navigation_generation"] as? Int, Int(generation))
            XCTAssertNotNil(object["result"] as? [String: Any])

            let ordinary = await controller.execute(request: AppUiRequestDto(
                requestId: "app-ui-literal",
                appId: "tracker",
                action: .fill,
                target: AppUiTargetDto(elementId: "title", role: nil, name: nil),
                value: wrapper
            ))
            XCTAssertNil(ordinary.error)
            let ordinaryFieldValue = try await webView.evaluateJavaScript(
                "document.getElementById('title').value"
            ) as? String
            XCTAssertEqual(
                ordinaryFieldValue,
                wrapper,
                "an ordinary request must treat reserved-looking JSON as literal text"
            )
        }

        func testQaRegistryWaitsForTheCurrentRuntimeControllerInsteadOfUsingStaleSamePortController() async throws {
            let runtimeA = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=41"))
            let runtimeB = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
            let stale = LocalAppWebViewController(
                appID: "tracker",
                broker: LocalAppBridgeBroker(appID: "tracker")
            )
            stale.webView = WKWebView()
            let staleGeneration = stale.beginNavigation(expectedURL: runtimeA)
            stale.markReady(committedURL: runtimeA, navigationGeneration: staleGeneration)
            LocalAppWebViewRegistry.shared.register(stale, appID: "tracker")

            let wrapperData = try JSONSerialization.data(withJSONObject: [
                "lingxi_qa": [
                    "version": 1,
                    "expected_runtime_url": runtimeB.absoluteString,
                    "action_value": NSNull(),
                ],
            ])
            let wrapper = try XCTUnwrap(String(data: wrapperData, encoding: .utf8))
            let request = AppUiRequestDto(
                requestId: "qa-ui-current-controller",
                appId: "tracker",
                action: .inspect,
                target: nil,
                value: wrapper
            )
            async let pendingResult = LocalAppWebViewRegistry.shared.execute(request: request)
            try await Task.sleep(for: .milliseconds(100))

            let current = LocalAppWebViewController(
                appID: "tracker",
                broker: LocalAppBridgeBroker(appID: "tracker")
            )
            let currentWebView = WKWebView()
            current.webView = currentWebView
            let currentGeneration = current.beginNavigation(expectedURL: runtimeB)
            LocalAppWebViewRegistry.shared.register(current, appID: "tracker")
            defer { LocalAppWebViewRegistry.shared.unregister(current, appID: "tracker") }
            currentWebView.loadHTMLString(#"<main id="runtime-b">Runtime B</main>"#, baseURL: runtimeB)
            try await waitForElement("runtime-b", in: currentWebView)
            let committedURL = try XCTUnwrap(currentWebView.url)
            current.markReady(committedURL: committedURL, navigationGeneration: currentGeneration)

            let result = await pendingResult
            XCTAssertNil(result.error)
            XCTAssertTrue(stale.webView == nil, "replacement must detach the stale controller")
            let data = try XCTUnwrap(result.resultJSON?.data(using: .utf8))
            let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            let attestation = try XCTUnwrap(object["lingxi_qa"] as? [String: Any])
            XCTAssertEqual(attestation["requested_runtime_url"] as? String, runtimeB.absoluteString)
            let loadedString = try XCTUnwrap(attestation["loaded_runtime_url"] as? String)
            let loadedURL = try XCTUnwrap(URL(string: loadedString))
            XCTAssertTrue(LocalAppWebViewController.sameRuntimeIdentity(runtimeB, loadedURL))
            XCTAssertEqual(attestation["navigation_generation"] as? Int, Int(currentGeneration))
        }

        /// Fix round 1 — code review required PROOF, not an assertion, that
        /// `TextEncoder` (which the budget ladder in `snapshot()` now calls
        /// to measure real UTF-8 bytes) is not silently blocked by the CSP
        /// every local app page gets. This installs the REAL production CSP
        /// via the REAL bootstrap entry point (`LocalAppWebViewRepresentable
        /// .bridgeSource`, not a hand-copied duplicate that could drift), on
        /// a REAL WKWebView, then probes `TextEncoder` and runs an actual
        /// `.inspect` through `LocalAppWebViewController.execute` -- the
        /// same host-injected `evaluateJavaScript` channel `evaluate(_:in:)`
        /// uses in production (verified by reading it: it wraps
        /// `webView.evaluateJavaScript(source) { ... }` directly). If the
        /// CSP blocked `TextEncoder`, `new TextEncoder()` would throw inside
        /// `executionSource`'s `try` block and `inspect.error` would be
        /// non-nil here -- this is a live result, not a read of the spec.
        func testTextEncoderSurvivesTheInstalledCSPDuringARealInspect() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            webView.loadHTMLString(
                #"<button id="go">中文按钮文字标签</button>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("go", in: webView)

            _ = try await webView.evaluateJavaScript(LocalAppWebViewRepresentable.bridgeSource(formFactor: "iphone"))
            let cspInstalled = try await webView.evaluateJavaScript(
                "document.head.querySelector('meta[http-equiv=\"Content-Security-Policy\"]') !== null"
            ) as? Bool
            XCTAssertEqual(cspInstalled, true, "installCsp() must actually install the meta tag or the probe below proves nothing")

            let probe = try await webView.evaluateJavaScript(
                "JSON.stringify({has: typeof TextEncoder !== 'undefined', bytes: new TextEncoder().encode('中文').length, units: '中文'.length})"
            ) as? String
            XCTAssertEqual(
                probe, #"{"has":true,"bytes":6,"units":2}"#,
                "TextEncoder must survive the installed CSP and report real UTF-8 bytes (3/char for CJK), not `.length`'s UTF-16 code units (1/char)"
            )

            let inspect = await controller.execute(request: AppUiRequestDto(
                requestId: "inspect-1",
                appId: "tracker",
                action: .inspect,
                target: nil,
                value: nil
            ))
            XCTAssertNil(inspect.error)
            XCTAssertTrue(inspect.resultJSON?.contains("中文按钮文字标签") == true)
        }

        func testFailedRuntimeLabelCarriesTheEngineReason() {
            let store = LocalAppsStore()
            store.handle(event: .appRuntimeChanged(
                appId: "tracker",
                state: .failed,
                details: nil,
                lastError: "Next 服务已退出"
            ))

            XCTAssertEqual(store.runtimes["tracker"]?.label, "运行失败 · Next 服务已退出")
        }

        /// A page issuing two `fetch()` calls to two unauthorized domains in one
        /// tick raises two capability requests. Each has its own 5-minute
        /// approval timeout, so the second must not replace the first.
        func testASecondCapabilityRequestQueuesInsteadOfReplacingTheFirst() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appEvent(event: .appCapabilityRequested(request: AppCapabilityRequestDto(
                requestId: "permission-1",
                appId: "tracker",
                capability: .networkDomain,
                domain: "api.example.com",
                reason: "读取公开数据"
            ))))
            store.handle(event: .appEvent(event: .appCapabilityRequested(request: AppCapabilityRequestDto(
                requestId: "permission-2",
                appId: "tracker",
                capability: .networkDomain,
                domain: "cdn.example.com",
                reason: "加载图片"
            ))))

            XCTAssertEqual(store.pendingPermission?.id, "permission-1", "FIFO: the first request keeps the sheet")

            await store.resolvePendingPermission(.once)
            XCTAssertEqual(store.pendingPermission?.id, "permission-2", "The queued request must present next")
            XCTAssertEqual(store.pendingPermission?.domain, "cdn.example.com")

            await store.resolvePendingPermission(.deny)
            XCTAssertNil(store.pendingPermission)

            let resolutions = submitted.compactMap { command -> (String, AppAuthorizationDecisionDto)? in
                guard case let .resolveAppCapabilityRequest(requestId, decision) = command else { return nil }
                return (requestId, decision)
            }
            XCTAssertEqual(resolutions.map(\.0), ["permission-1", "permission-2"])
            XCTAssertEqual(resolutions.map(\.1), [.allowOnce, .deny])
        }

        /// The pushed preview route must tell a FAILED runtime apart from one
        /// that is merely still starting, and must surface the failure reason —
        /// otherwise every non-running state renders the same bare hourglass
        /// with no reason and no way to retry.
        func testPreviewPlaceholderDistinguishesRuntimeStates() {
            let url = URL(string: "http://127.0.0.1:8080")!
            XCTAssertNil(
                LocalAppPreviewPlaceholder.forStatus(.running(url)),
                "a live URL renders the web view, not a placeholder"
            )
            XCTAssertEqual(LocalAppPreviewPlaceholder.forStatus(.starting), .transient)
            XCTAssertEqual(LocalAppPreviewPlaceholder.forStatus(.stopping), .transient)
            XCTAssertEqual(
                LocalAppPreviewPlaceholder.forStatus(.running(nil)),
                .transient,
                "running without a URL yet is still coming up"
            )
            XCTAssertEqual(
                LocalAppPreviewPlaceholder.forStatus(.failed("no servable output")),
                .failed("no servable output")
            )
            XCTAssertEqual(
                LocalAppPreviewPlaceholder.forStatus(.suspended("memory pressure")),
                .suspended("memory pressure")
            )
            XCTAssertEqual(LocalAppPreviewPlaceholder.forStatus(.stopped), .idle)
            XCTAssertEqual(LocalAppPreviewPlaceholder.forStatus(nil), .idle)
        }

        /// `activeUIRequestAppID` was a SINGLE slot serving a multi-app queue:
        /// app B's request overwrote app A's, and resolving either one cleared
        /// it for both. A's request stays queued but `hasPendingUIRequest`
        /// reports false, so opening A never routes to the preview and its
        /// stalled call never gets its UI shown.
        func testPendingUiRequestsAreTrackedPerApp() async {
            let store = LocalAppsStore()
            store.configure { _ in }

            for (index, appID) in ["app-a", "app-b"].enumerated() {
                store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                    requestId: "req-\(index)",
                    appId: appID,
                    action: .reload,
                    target: nil,
                    value: nil
                ))))
            }
            XCTAssertTrue(store.hasPendingUIRequest(appID: "app-a"))
            XCTAssertTrue(
                store.hasPendingUIRequest(appID: "app-b"),
                "a second app's request must not erase the first"
            )

            // Resolve whichever prompt is on screen; the other app is still pending.
            await store.resolvePendingPermission(.deny)
            let stillPending = store.hasPendingUIRequest(appID: "app-a")
                || store.hasPendingUIRequest(appID: "app-b")
            XCTAssertTrue(
                stillPending,
                "resolving one app's request must not clear the other's"
            )
        }

        /// A UI request that is DROPPED on permission-queue overflow must not
        /// leave a `pendingUIRequestAppIDs` entry pointing at its app.
        ///
        /// `.appUiRequest` records the request before enqueuing, but
        /// `enqueuePermission` returns early once the queue is full — and the
        /// entry is removed only in `resolveUIRequest`, which a dropped request
        /// never reaches. The entry then sticks forever, and
        /// `hasPendingUIRequest` force-routes every later open of that app to
        /// the preview tab, which renders a bare "preview isn't ready yet".
        func testDroppedUiRequestDoesNotLeaveAStickyPendingFlag() async {
            let store = LocalAppsStore()
            store.configure { _ in }

            // One request becomes the presented prompt; the next eight fill the
            // queue to `maxQueuedPermissions`.
            for index in 0...8 {
                store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                    requestId: "filler-\(index)",
                    appId: "filler-app",
                    action: .reload,
                    target: nil,
                    value: nil
                ))))
            }
            XCTAssertNotNil(store.pendingPermission, "the first request should be presented")

            // This one overflows and is dropped. It belongs to a DIFFERENT app,
            // so nothing is pending for `victim-app` afterwards.
            store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                requestId: "overflowed",
                appId: "victim-app",
                action: .reload,
                target: nil,
                value: nil
            ))))
            XCTAssertNotNil(store.errorMessage, "overflow should surface an error")

            XCTAssertFalse(
                store.hasPendingUIRequest(appID: "victim-app"),
                "a dropped UI request must not leave a sticky pending flag"
            )
        }

        func testUiActionGrantIsRememberedForSubsequentRequests() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            broker.webView = webView
            LocalAppWebViewRegistry.shared.register(controller, appID: "tracker")
            defer { LocalAppWebViewRegistry.shared.unregister(controller, appID: "tracker") }

            store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                requestId: "ui-1",
                appId: "tracker",
                action: .reload,
                target: nil,
                value: nil
            ))))
            XCTAssertNotNil(store.pendingPermission)
            await store.resolvePendingPermission(.always)
            XCTAssertTrue(submitted.contains {
                if case let .resolveAppUiRequest(requestId, _, _, _) = $0 { requestId == "ui-1" } else { false }
            })
            // This fixture owns a bare controller rather than the SwiftUI
            // representable, so no WKNavigationDelegate will deliver the
            // didFinish callback that normally re-arms a reload.
            controller.markReady()

            store.handle(event: .appEvent(event: .appUiRequest(request: AppUiRequestDto(
                requestId: "ui-2",
                appId: "tracker",
                action: .reload,
                target: nil,
                value: nil
            ))))
            XCTAssertNil(store.pendingPermission, "始终允许 must not re-prompt on the next UI action")
            // Let the granted action finish before `defer` unregisters the controller.
            try await waitUntil("the granted UI action to resolve") {
                submitted.contains {
                    if case let .resolveAppUiRequest(requestId, _, _, _) = $0 { requestId == "ui-2" } else { false }
                }
            }
        }

        func testForegroundRestoreIsConsumedOncePerBackgroundCycle() async {
            UserDefaults.standard.removeObject(forKey: "local-apps.running-before-suspension")
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "tracker", name: "Tracker", brief: "跟踪任务"),
            ]))
            store.handle(event: .appRuntimeChanged(
                appId: "tracker",
                state: .starting,
                details: nil,
                lastError: nil
            ))

            store.sceneDidEnterBackground()
            await store.sceneWillEnterForeground()
            await store.sceneWillEnterForeground()

            let starts = submitted.filter { if case .startApp = $0 { true } else { false } }.count
            XCTAssertEqual(starts, 1, "An .inactive bounce must not relaunch an app the user stopped")
            UserDefaults.standard.removeObject(forKey: "local-apps.running-before-suspension")
        }

        func testToggleHonoursTheRequestedStateInsteadOfFlipping() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            webView.loadHTMLString(
                #"<input id="flag" type="checkbox">"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("flag", in: webView)

            let alreadyOff = await controller.execute(request: AppUiRequestDto(
                requestId: "toggle-1",
                appId: "tracker",
                action: .toggle,
                target: AppUiTargetDto(elementId: "flag", role: nil, name: nil),
                value: "false"
            ))
            XCTAssertNil(alreadyOff.error)
            var checked = try await webView.evaluateJavaScript("document.getElementById('flag').checked") as? Bool
            XCTAssertEqual(checked, false, "A toggle to the state the element already holds must not flip it")

            let turnOn = await controller.execute(request: AppUiRequestDto(
                requestId: "toggle-2",
                appId: "tracker",
                action: .toggle,
                target: AppUiTargetDto(elementId: "flag", role: nil, name: nil),
                value: "true"
            ))
            XCTAssertNil(turnOn.error)
            checked = try await webView.evaluateJavaScript("document.getElementById('flag').checked") as? Bool
            XCTAssertEqual(checked, true)
        }

        func testFailedUiActionCarriesTheScriptDiagnostic() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            webView.loadHTMLString(
                #"<button id="save">Save</button>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("save", in: webView)

            let result = await controller.execute(request: AppUiRequestDto(
                requestId: "click-1",
                appId: "tracker",
                action: .click,
                target: AppUiTargetDto(elementId: "missing", role: nil, name: "缺失按钮"),
                value: nil
            ))
            XCTAssertTrue(
                result.error?.contains("UI target was not found") == true,
                "Expected the script diagnostic, got: \(result.error ?? "nil")"
            )
        }

        /// Phase 1a — reusing the whole-view ladder for a crop UPSCALES it. A tall
        /// narrow crop must be capped on its own long edge and never enlarged.
        func testCropIsCappedOnItsOwnLongEdgeAndNeverUpscaled() {
            let cap: CGFloat = 1_024 / 3   // 1024 px cap on a 3x screen, in points
            // Tall crop: long edge is the height, so the cap applies to height.
            let tall = LocalAppWebViewController.snapshotWidthPoints(
                rect: CGRect(x: 0, y: 0, width: 120, height: 800), capPoints: cap)
            XCTAssertEqual(tall, 120 * (cap / 800), accuracy: 0.01)
            // Small crop: already under the cap, so it must be left alone.
            let small = LocalAppWebViewController.snapshotWidthPoints(
                rect: CGRect(x: 0, y: 0, width: 118, height: 74), capPoints: cap)
            XCTAssertEqual(small, 118, accuracy: 0.01, "a crop under the cap must not be enlarged")
        }

        /// No existing test exercised `captureFrame`'s whole-view (no-crop)
        /// path before this task, so this pins the dispatch brief's own
        /// worked example (393x852pt view, 3x screen -> 157.4pt) as a
        /// regression anchor: the formula is only ALGEBRAICALLY the same as
        /// the pre-existing one (`bounds.width * min(longEdge, cap) /
        /// longEdge` vs. this helper's `rect.width * min(1, cap /
        /// longEdge)`), now that it is factored through the shared helper.
        func testSnapshotWidthPointsMatchesTheWholeViewWorkedExample() {
            let capPoints: CGFloat = 1_024 / 3
            let wholeView = LocalAppWebViewController.snapshotWidthPoints(
                rect: CGRect(x: 0, y: 0, width: 393, height: 852), capPoints: capPoints)
            XCTAssertEqual(wholeView, 157.4, accuracy: 0.1)
        }

        /// Task 7's `capture_ui_value` (Rust) deliberately preserves the
        /// caller's original numeric form, so `x`/`y`/`width`/`height` can each
        /// land as a JSON integer OR a JSON float. A parser written against
        /// only the integer shape would break on a routine fractional CSS
        /// pixel -- this pins both, plus the mixed case.
        func testParseRequestedRectAcceptsBothIntegerAndFractionalJSONNumbers() {
            let fromInts = LocalAppWebViewController.parseRequestedRect(
                fromValueJSON: #"{"rect":{"x":10,"y":20,"width":120,"height":80}}"#
            )
            XCTAssertEqual(fromInts, CGRect(x: 10, y: 20, width: 120, height: 80))

            let fromFloats = LocalAppWebViewController.parseRequestedRect(
                fromValueJSON: #"{"rect":{"x":10.5,"y":0,"width":118.25,"height":74.0}}"#
            )
            XCTAssertEqual(fromFloats, CGRect(x: 10.5, y: 0, width: 118.25, height: 74))

            let mixed = LocalAppWebViewController.parseRequestedRect(
                fromValueJSON: #"{"rect":{"x":0,"y":0.0,"width":118,"height":74.5}}"#
            )
            XCTAssertEqual(mixed, CGRect(x: 0, y: 0, width: 118, height: 74.5))
        }

        /// An absent, shapeless, or non-numeric rect must fall back to "no
        /// rect" (whole-view capture) rather than crash or produce garbage
        /// geometry -- the Rust side already rejects a genuinely invalid rect
        /// before this ever ships, so this is the client's defensive fallback.
        func testParseRequestedRectReturnsNilForAbsentOrMalformedValue() {
            XCTAssertNil(LocalAppWebViewController.parseRequestedRect(fromValueJSON: nil))
            XCTAssertNil(LocalAppWebViewController.parseRequestedRect(fromValueJSON: #"{"app_id":"demo"}"#))
            XCTAssertNil(LocalAppWebViewController.parseRequestedRect(fromValueJSON: #"{"rect":{"x":10}}"#))
            XCTAssertNil(LocalAppWebViewController.parseRequestedRect(fromValueJSON: #"{"rect":{"x":"10","y":0,"width":10,"height":10}}"#))
            XCTAssertNil(LocalAppWebViewController.parseRequestedRect(fromValueJSON: "not json"))
        }

        /// Live proof (not just a read of the code) that a crop entirely
        /// outside the viewport is refused rather than silently widened back
        /// to the whole frame. `captureFrame`'s clamp-and-guard runs BEFORE
        /// `takeSnapshot` is ever called, so — unlike a positive capture —
        /// this does not depend on the webview being on-screen and composited.
        func testCaptureViewRejectsARectEntirelyOutsideTheViewportInsteadOfWideningIt() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView(frame: CGRect(x: 0, y: 0, width: 393, height: 852))
            controller.webView = webView
            webView.loadHTMLString(#"<div id="ready">hi</div>"#, baseURL: URL(string: "http://127.0.0.1:43123"))
            try await waitForElement("ready", in: webView)

            let result = await controller.execute(request: AppUiRequestDto(
                requestId: "capture-outside",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: #"{"rect":{"x":1000,"y":1000,"width":50,"height":50}}"#
            ))
            XCTAssertNil(result.resultJSON, "an out-of-viewport crop must not fall back to the whole frame")
            XCTAssertEqual(result.error, String(localized: "local_apps_error_ui_capture_unavailable"))
        }

        /// Same as above with a FRACTIONAL rect, proving the float path also
        /// reaches the clamp guard end-to-end and not just the pure parser.
        func testCaptureViewRejectsAFractionalRectEntirelyOutsideTheViewport() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView(frame: CGRect(x: 0, y: 0, width: 393, height: 852))
            controller.webView = webView
            webView.loadHTMLString(#"<div id="ready">hi</div>"#, baseURL: URL(string: "http://127.0.0.1:43123"))
            try await waitForElement("ready", in: webView)

            let result = await controller.execute(request: AppUiRequestDto(
                requestId: "capture-outside-float",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: #"{"rect":{"x":500.5,"y":900.25,"width":50.5,"height":50.5}}"#
            ))
            XCTAssertNil(result.resultJSON)
            XCTAssertEqual(result.error, String(localized: "local_apps_error_ui_capture_unavailable"))
        }

        /// Live proof, on a real composited window (an offscreen `WKWebView`
        /// can have `takeSnapshot` call back with neither image nor error, so
        /// only an on-screen view proves anything about the RESULT), that an
        /// in-bounds crop comes back sized from its OWN long edge and not
        /// stretched up to the view-sized width -- the enlargement bug this
        /// task fixes. A whole-view capture on this 393x852pt webview would
        /// report a width near the 1024px cap; a 118pt-wide crop (well under
        /// the ~341pt per-edge cap at 3x) must instead come back near
        /// `118 * displayScale` px.
        func testCaptureViewHonoursAnInBoundsCropWithoutUpscalingIt() async throws {
            let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 393, height: 852))
            let webView = WKWebView(frame: window.bounds)
            window.rootViewController = UIViewController()
            window.rootViewController?.view.addSubview(webView)
            window.makeKeyAndVisible()
            defer { window.isHidden = true }

            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            controller.webView = webView
            webView.loadHTMLString(
                #"<body style="margin:0;background:#3366ff"><div id="ready">hi</div></body>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("ready", in: webView)

            let request = AppUiRequestDto(
                requestId: "capture-crop",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: #"{"rect":{"x":0,"y":0,"width":118,"height":74}}"#
            )
            var result = await controller.execute(request: request)
            // `takeSnapshot` can report "not yet composited" on the very first
            // runloop turns after `makeKeyAndVisible()`; retrying (production
            // does not) is what makes THIS TEST deterministic, not a change to
            // the guard being verified -- that guard already returned (or
            // didn't) synchronously before any of this.
            for _ in 0 ..< 40 where result.error != nil {
                try await Task.sleep(for: .milliseconds(50))
                result = await controller.execute(request: request)
            }
            XCTAssertNil(result.error, "capture never succeeded: \(result.error ?? "?")")
            guard let resultJSON = result.resultJSON,
                  let data = resultJSON.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let image = object["image"] as? [String: Any],
                  let width = image["width"] as? Int
            else { return XCTFail("expected an image in the result, got \(result.resultJSON ?? "nil")") }
            let scale = Double(webView.traitCollection.displayScale)
            XCTAssertEqual(
                Double(width), 118 * scale, accuracy: 2,
                "a crop under the cap must come back at its own size, not the view's"
            )
            // Compare against what a WHOLE-VIEW capture on this same webview
            // would report, via the identical production formula -- scale-
            // agnostic proof that the crop was not stretched up to view size,
            // rather than a hardcoded pixel constant tied to one scale factor.
            let capPoints = 1_024 / CGFloat(scale)
            let wholeViewWidthPixels = Double(
                LocalAppWebViewController.snapshotWidthPoints(rect: webView.bounds, capPoints: capPoints)
            ) * scale
            XCTAssertLessThan(
                Double(width), wholeViewWidthPixels,
                "a 118pt crop must not be reported as wide as a whole-view capture (\(wholeViewWidthPixels)px)"
            )
        }

        /// Fix round 1 — sets up a window-attached, composited webview the way
        /// `testCaptureViewHonoursAnInBoundsCropWithoutUpscalingIt` does, so the
        /// two `capture_rect` tests below don't each hand-roll the same setup.
        private func makeComposedCaptureController() -> (controller: LocalAppWebViewController, webView: WKWebView, window: UIWindow) {
            let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 393, height: 852))
            let webView = WKWebView(frame: window.bounds)
            window.rootViewController = UIViewController()
            window.rootViewController?.view.addSubview(webView)
            window.makeKeyAndVisible()
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            controller.webView = webView
            return (controller, webView, window)
        }

        /// `takeSnapshot` can report "not yet composited" on the first runloop
        /// turns after `makeKeyAndVisible()`; retrying here (production does
        /// not retry) is what makes a WINDOWED capture test deterministic
        /// without changing what is being verified — `captureFrame`'s
        /// clamp/guard already ran (or didn't) synchronously before any of
        /// this, same rationale as the in-bounds-crop test above.
        private func captureUntilComposited(
            _ controller: LocalAppWebViewController, request: AppUiRequestDto
        ) async -> LocalAppUIExecutionResult {
            var result = await controller.execute(request: request)
            for _ in 0 ..< 40 where result.error != nil {
                try? await Task.sleep(for: .milliseconds(50))
                result = await controller.execute(request: request)
            }
            return result
        }

        func testQaCaptureAttestationPreservesTheOwnedImageObject() async throws {
            let runtimeURL = try XCTUnwrap(URL(string: "http://127.0.0.1:43123/?lingxi_runtime=42"))
            let (controller, webView, window) = makeComposedCaptureController()
            defer { window.isHidden = true }
            let generation = controller.beginNavigation(expectedURL: runtimeURL)
            webView.loadHTMLString(
                #"<body style="margin:0;background:#3366ff"><div id="ready">hi</div></body>"#,
                baseURL: runtimeURL
            )
            try await waitForElement("ready", in: webView)
            let committedURL = try XCTUnwrap(webView.url)
            controller.markReady(committedURL: committedURL, navigationGeneration: generation)
            let cropValue = #"{"rect":{"x":0,"y":0,"width":118,"height":74}}"#
            let wrapperData = try JSONSerialization.data(withJSONObject: [
                "lingxi_qa": [
                    "version": 1,
                    "expected_runtime_url": runtimeURL.absoluteString,
                    "action_value": cropValue,
                ],
            ])
            let wrapper = try XCTUnwrap(String(data: wrapperData, encoding: .utf8))

            let result = await captureUntilComposited(controller, request: AppUiRequestDto(
                requestId: "qa-ui-capture",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: wrapper
            ))
            XCTAssertNil(result.error, "QA capture never succeeded: \(result.error ?? "?")")
            let data = try XCTUnwrap(result.resultJSON?.data(using: .utf8))
            let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            let original = try XCTUnwrap(object["result"] as? [String: Any])
            let image = try XCTUnwrap(original["image"] as? [String: Any])
            XCTAssertFalse((image["data"] as? String)?.isEmpty ?? true)
            XCTAssertEqual((original["capture_rect"] as? [String: Any])?["width"] as? Double, 118)
            XCTAssertNotNil(object["lingxi_qa"] as? [String: Any])
        }

        /// Fix round 1, ruling point 1 — `capture_rect` must be OMITTED
        /// entirely for a whole-view capture (no `rect` in the request), not
        /// present-and-null, so that path's JSON stays exactly as it was
        /// before crops existed.
        func testCaptureViewOmitsCaptureRectForAWholeViewCapture() async throws {
            let (controller, webView, window) = makeComposedCaptureController()
            defer { window.isHidden = true }
            webView.loadHTMLString(
                #"<body style="margin:0;background:#3366ff"><div id="ready">hi</div></body>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("ready", in: webView)

            let result = await captureUntilComposited(controller, request: AppUiRequestDto(
                requestId: "capture-whole",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: nil
            ))
            XCTAssertNil(result.error, "capture never succeeded: \(result.error ?? "?")")
            guard let resultJSON = result.resultJSON,
                  let data = resultJSON.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            else { return XCTFail("expected a result, got \(result.resultJSON ?? "nil")") }
            XCTAssertNil(object["capture_rect"], "a whole-view capture must not carry capture_rect at all")
            XCTAssertNotNil(object["viewport"], "viewport must still be present")
        }

        /// Fix round 1, ruling point 4 (CRITICAL fix) — `capture_rect` must be
        /// the CLAMPED region actually handed to `takeSnapshot`, not the
        /// caller's original request. That distinction is the whole reason
        /// this field has to be computed from `region` after `intersection`,
        /// not echoed back from the parsed request. The requested rect below
        /// extends past BOTH the right and bottom edges of a 393x852 view, so
        /// a clamp bug on just one axis would not be caught by a single-edge
        /// case (matches the `intersection` behaviour independently verified
        /// against a standalone CoreGraphics script in the task-8 report:
        /// `(300,800,200,200)` ∩ `(0,0,393,852)` = `(300,800,93,52)`).
        func testCaptureViewReportsCaptureRectAsTheClampedRegionNotTheRequestedOne() async throws {
            let (controller, webView, window) = makeComposedCaptureController()
            defer { window.isHidden = true }
            webView.loadHTMLString(
                #"<body style="margin:0;background:#3366ff"><div id="ready">hi</div></body>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("ready", in: webView)

            let result = await captureUntilComposited(controller, request: AppUiRequestDto(
                requestId: "capture-partial",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: #"{"rect":{"x":300,"y":800,"width":200,"height":200}}"#
            ))
            XCTAssertNil(result.error, "capture never succeeded: \(result.error ?? "?")")
            guard let resultJSON = result.resultJSON,
                  let data = resultJSON.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let captureRect = object["capture_rect"] as? [String: Any]
            else { return XCTFail("expected capture_rect in the result, got \(result.resultJSON ?? "nil")") }
            XCTAssertEqual(captureRect["x"] as? Double, 300, "origin inside the viewport is unchanged by clamping")
            XCTAssertEqual(captureRect["y"] as? Double, 800)
            XCTAssertEqual(captureRect["width"] as? Double, 93, "393 - 300: the CLAMPED width, not the requested 200")
            XCTAssertEqual(captureRect["height"] as? Double, 52, "852 - 800: the CLAMPED height, not the requested 200")
        }

        /// Final review, finding 2 — a NEGATIVE origin is the single most
        /// common crop an agent will compute, because `inspect_ui`'s
        /// `elements[].rect` reports `getBoundingClientRect().top`, which is
        /// negative for anything scrolled above the fold. The host used to
        /// refuse it outright (`x < 0 || y < 0`), which made this clamp
        /// unreachable; with that gone, the client must do what it was always
        /// built to do — clamp to the viewport and REPORT the clamped region,
        /// never refuse and never silently widen back to the whole frame.
        ///
        /// `(-50, -40, 200, 150)` ∩ `(0, 0, 393, 852)` = `(0, 0, 150, 110)`,
        /// verified by running `CGRect.intersection` directly rather than
        /// assuming it. Both axes are negative in the same request, so a
        /// one-axis bug cannot hide behind the other being right.
        func testCaptureViewClampsANegativeOriginInsteadOfRefusingIt() async throws {
            let (controller, webView, window) = makeComposedCaptureController()
            defer { window.isHidden = true }
            webView.loadHTMLString(
                #"<body style="margin:0;background:#3366ff"><div id="ready">hi</div></body>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("ready", in: webView)

            let result = await captureUntilComposited(controller, request: AppUiRequestDto(
                requestId: "capture-negative-origin",
                appId: "tracker",
                action: .captureView,
                target: nil,
                value: #"{"rect":{"x":-50,"y":-40,"width":200,"height":150}}"#
            ))
            XCTAssertNil(
                result.error,
                "a scrolled-above-the-fold element rect must be clamped, not refused: \(result.error ?? "?")"
            )
            guard let resultJSON = result.resultJSON,
                  let data = resultJSON.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let captureRect = object["capture_rect"] as? [String: Any]
            else { return XCTFail("expected capture_rect in the result, got \(result.resultJSON ?? "nil")") }
            XCTAssertEqual(captureRect["x"] as? Double, 0, "a negative x must be clamped to the viewport edge")
            XCTAssertEqual(captureRect["y"] as? Double, 0, "a negative y must be clamped to the viewport edge")
            XCTAssertEqual(captureRect["width"] as? Double, 150, "-50 + 200: the visible part, not the requested 200")
            XCTAssertEqual(captureRect["height"] as? Double, 110, "-40 + 150: the visible part, not the requested 150")
        }

        /// Final review, finding 5 — LIVE proof (not a token match) that a page
        /// shadowing `window.TextEncoder` no longer takes `inspect_ui` down
        /// with it.
        ///
        /// The bootstrap is evaluated FIRST (as `WKUserScript(injectionTime:
        /// .atDocumentStart)` does in production — verified by reading the
        /// `WKUserScript` construction in `LocalAppWebView.swift`), then the
        /// "page" replaces the global with a constructor that throws. Before
        /// the fix, `snapshot()`'s `new TextEncoder()` would run that throwing
        /// constructor inside `executionSource`'s `try` and the whole inspect
        /// would come back an error.
        func testInspectSurvivesAPageThatShadowsTextEncoder() async throws {
            let broker = LocalAppBridgeBroker(appID: "tracker")
            let controller = LocalAppWebViewController(appID: "tracker", broker: broker)
            let webView = WKWebView()
            controller.webView = webView
            webView.loadHTMLString(
                #"<button id="go">中文按钮文字标签</button>"#,
                baseURL: URL(string: "http://127.0.0.1:43123")
            )
            try await waitForElement("go", in: webView)

            _ = try await webView.evaluateJavaScript(LocalAppWebViewRepresentable.bridgeSource(formFactor: "iphone"))
            let captured = try await webView.evaluateJavaScript(
                "typeof window.__lingxiTextEncoder === 'function'"
            ) as? Bool
            XCTAssertEqual(captured, true, "the bootstrap must capture TextEncoder or the probe below proves nothing")

            // The page takes the global away, exactly as an app bundling its own
            // polyfill (or a hostile one) can.
            _ = try await webView.evaluateJavaScript(
                "window.TextEncoder = function () { throw new Error('page shadowed TextEncoder'); }; true"
            )
            let shadowed = try await webView.evaluateJavaScript(
                "(() => { try { new TextEncoder(); return 'alive'; } catch (e) { return e.message; } })()"
            ) as? String
            XCTAssertEqual(
                shadowed, "page shadowed TextEncoder",
                "the shadow must actually take effect, or this test cannot fail"
            )

            let inspect = await controller.execute(request: AppUiRequestDto(
                requestId: "inspect-shadowed",
                appId: "tracker",
                action: .inspect,
                target: nil,
                value: nil
            ))
            XCTAssertNil(inspect.error, "a page-shadowed TextEncoder must not kill inspect_ui: \(inspect.error ?? "?")")
            XCTAssertTrue(
                inspect.resultJSON?.contains("中文按钮文字标签") == true,
                "got \(inspect.resultJSON ?? "nil")"
            )
        }

        private func waitUntil(_ description: String, _ condition: () -> Bool) async throws {
            for _ in 0 ..< 200 {
                if condition() { return }
                try await Task.sleep(for: .milliseconds(10))
            }
            XCTFail("Timed out waiting for \(description)")
        }

        private func waitForElement(_ elementID: String, in webView: WKWebView) async throws {
            for _ in 0 ..< 100 {
                let source = "document.getElementById('\(elementID)') !== null"
                if (try? await webView.evaluateJavaScript(source) as? Bool) == true { return }
                try await Task.sleep(for: .milliseconds(20))
            }
            XCTFail("Timed out waiting for WebView element \(elementID)")
        }
    #endif

    #if canImport(engine_mobileFFI)
        // ── The conversational create flow ────────────────────────────────
        //
        // The whole block these replaced was built around a create FORM: a
        // brief, a name, a surface, a model, and a claim that matched the
        // record by its brief. All of it is gone. "+" now commits an empty
        // SHELL and the interview happens inside the app's own conversation,
        // so the only thing tying an `AppCreated` back to the tap that caused
        // it is the client-generated `request_id`.
        //
        // `createShellApp` takes NO default for `armLibraryFallback`, so every
        // call below names it. The tests in this run of the section are the
        // LIBRARY's "+" — the caller that mounts the cover — and pass `true`.
        // That is not bookkeeping: three of them assert
        // `consumeCreatedAppID()` is nil for an event that is not theirs
        // (`testOnlyTheAppCreatedCarryingOurRequestIdIsClaimed`,
        // `testAnAppCreatedWithoutARequestIdIsNeverClaimed`,
        // `testAReconnectClearsPendingAndRefusesToClaimALaterEvent`), and
        // under `false` the store never arms `createdAppID` for ANY event, so
        // those three would pass without exercising the correlation they
        // exist to pin. The drawer's `false` is covered by the two tests at
        // the end of the section.

        /// "+" sends exactly one `CreateApp`, in shell mode, with a client
        /// correlation key and nothing the user was never asked for.
        ///
        /// Every field is asserted, including the ones that must be EMPTY: a
        /// name or a brief invented here would be a value the user never
        /// chose being written into a record they cannot rename, and a
        /// non-nil `surface` would fix the app's shape before anyone had
        /// discussed what it is.
        func testPlusSendsAShellCreateWithAClientCorrelationKey() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let started = await store.createShellApp(armLibraryFallback: true)

            XCTAssertTrue(started)
            XCTAssertEqual(submitted.count, 1)
            guard case let .createApp(name, origin, brief, gitEnabled, workflowModel,
                                      conversationId, surface, mode, requestId) =
                try XCTUnwrap(submitted.first)
            else { return XCTFail("expected CreateApp, got \(submitted)") }
            XCTAssertEqual(name, "", "a shell has no name yet — the agent proposes one")
            XCTAssertEqual(origin, .library)
            XCTAssertEqual(brief, "", "a shell has no brief — that is what the interview is for")
            XCTAssertTrue(gitEnabled, "the default, not a form toggle")
            XCTAssertNil(workflowModel, "the model picker went with the form")
            XCTAssertNil(
                conversationId,
                "a .library-origin create binds no conversation: "
                    + "AppCreateOrigin::conversation_binding returns None for Library "
                    + "whatever is sent, so a non-nil value would be discarded")
            XCTAssertNil(
                surface, "the shape is decided when the scaffold lands, not now")
            XCTAssertEqual(mode, .shell, "no scaffold may be laid down by this command")
            let key = try XCTUnwrap(requestId, "without a correlation key nothing can be claimed")
            XCTAssertNotNil(
                UUID(uuidString: key), "the key must be a client-generated UUID, got \(key)")
        }

        /// Only the `AppCreated` carrying OUR request id is claimed.
        ///
        /// The engine emits `AppCreated` for BOTH creation paths, so an agent
        /// creating an app in another conversation lands one on this client
        /// mid-flight. Written so a store that claims the first event it sees
        /// FAILS: the foreign event is delivered first, and it is the one
        /// carrying a real (but different) request id — the case a bare
        /// "is this event correlated at all?" check would still get wrong.
        func testOnlyTheAppCreatedCarryingOurRequestIdIsClaimed() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))

            let theirs = shellRecord(id: "theirs", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(
                record: theirs, requestId: "somebody-elses")))
            store.handle(event: .appEvent(event: .appRecordChanged(record: theirs)))

            XCTAssertNil(store.createdAppLanding, "not this client's create")
            XCTAssertNil(store.consumeCreatedAppID())
            XCTAssertTrue(
                store.apps.contains { $0.id == "theirs" },
                "an unclaimed app still belongs in the catalog — it is just not OURS")

            let mine = shellRecord(id: "mine", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: mine, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "mine", name: "untitled", initSessionId: "session-9"))))

            XCTAssertEqual(store.createdAppLanding?.appID, "mine")
        }

        /// An `AppCreated` with NO request id is an agent-tool create or a
        /// backfill. It must never be claimed, however convenient it looks.
        func testAnAppCreatedWithoutARequestIdIsNeverClaimed() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createShellApp(armLibraryFallback: true)
            let agents = shellRecord(id: "agents", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: agents, requestId: nil)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: agents)))

            XCTAssertNil(store.createdAppLanding)
            XCTAssertNil(store.consumeCreatedAppID())
        }

        /// The hand-off arms on `AppCreated` and is completed by the pin that
        /// arrives afterwards on `AppRecordChanged`.
        ///
        /// `AppCreated` is emitted INSIDE the create transaction and the init
        /// session is minted after it, so the record on the create event never
        /// carries one. Publishing the landing there hands `RootView` a
        /// `nil` pin, which it answers by starting a FRESH conversation —
        /// orphaning the session the engine is about to mint.
        func testTheLandingArmsOnAppCreatedAndTakesThePinFromTheRecordUpdate() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            let created = shellRecord(id: "notes", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: key)))

            XCTAssertNil(
                store.createdAppLanding,
                "`AppCreated` never carries the pin, so publishing the landing "
                    + "here would make RootView start a FRESH conversation and "
                    + "orphan the session the engine is about to mint")

            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "notes", name: "untitled", initSessionId: "session-9"))))

            let landing = try XCTUnwrap(store.createdAppLanding)
            XCTAssertEqual(landing.appID, "notes")
            XCTAssertEqual(landing.initSessionID, "session-9")
            XCTAssertEqual(store.consumeCreatedAppLanding()?.appID, "notes")
            XCTAssertNil(store.consumeCreatedAppLanding(), "the landing is one-shot")
        }

        /// A create whose best-effort init-session mint FAILED must still
        /// land: the SCOPE, not the session, is what roots the agent in the
        /// app workspace.
        func testTheLandingStillFiresWhenNoInitSessionWasPinned() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            let created = shellRecord(id: "notes", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: created)))

            let landing = try XCTUnwrap(
                store.createdAppLanding, "a failed pin must not strand the hand-off")
            XCTAssertEqual(landing.appID, "notes")
            XCTAssertNil(landing.initSessionID)
        }

        /// Two landings, one slot: the FIRST must still be delivered.
        ///
        /// `RootView.landCreatedAppIfReady` refuses to consume while a turn is
        /// streaming, while `pendingWidgetSetup` is armed, and while Settings
        /// is open. A second create finishing inside any of those windows used
        /// to OVERWRITE the held landing (`createdAppLanding = landing`), and
        /// the single one-shot `consumeCreatedAppLanding()` then drained only
        /// the newest — the first app was created, listed in the library, and
        /// never handed off at all. Android buffers a `Channel` for exactly
        /// this (`LocalAppsViewModel.createdAppLandingChannel`).
        ///
        /// Order is asserted, not just membership: the older landing is the one
        /// the user has been waiting on longest.
        func testASecondLandingQueuesBehindTheFirstInsteadOfReplacingIt() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let firstKey = try XCTUnwrap(sentCreateRequestID(submitted))
            store.handle(event: .appEvent(event: .appCreated(
                record: shellRecord(id: "first", name: "untitled"), requestId: firstKey)))
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "first", name: "untitled", initSessionId: "s-1"))))

            // Vacuity guard: without this the queue assertions below could pass
            // over a slot that was empty the whole time.
            XCTAssertEqual(
                store.createdAppLanding?.appID, "first",
                "the first landing must be published before the second one arrives")

            submitted.removeAll()
            _ = await store.createShellApp(armLibraryFallback: true)
            let secondKey = try XCTUnwrap(sentCreateRequestID(submitted))
            store.handle(event: .appEvent(event: .appCreated(
                record: shellRecord(id: "second", name: "untitled"), requestId: secondKey)))
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "second", name: "untitled", initSessionId: "s-2"))))

            XCTAssertEqual(
                store.createdAppLanding?.appID, "first",
                "the newcomer must queue BEHIND the held landing, not replace it")

            let head = try XCTUnwrap(store.consumeCreatedAppLanding())
            XCTAssertEqual(head.appID, "first")
            XCTAssertEqual(head.initSessionID, "s-1")
            XCTAssertEqual(
                store.createdAppLanding?.appID, "second",
                "draining the head must PROMOTE the backlog, or the observed "
                    + "property never changes again and RootView's onChange sink "
                    + "is never re-driven for the queued landing")

            let next = try XCTUnwrap(store.consumeCreatedAppLanding())
            XCTAssertEqual(next.appID, "second")
            XCTAssertEqual(next.initSessionID, "s-2")
            XCTAssertNil(store.consumeCreatedAppLanding(), "the queue is drained")
        }

        /// Republishing for the SAME app supersedes in place.
        ///
        /// The two producers for one app are the pin arriving on
        /// `AppRecordChanged` and the pin-wait stop-loss giving up; queuing
        /// both would take the user into the same app twice.
        func testARepublishedLandingForTheSameAppSupersedesRatherThanQueues() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            store.handle(event: .appEvent(event: .appCreated(
                record: shellRecord(id: "notes", name: "untitled"), requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "notes", name: "untitled", initSessionId: "s-1"))))
            XCTAssertEqual(store.createdAppLanding?.appID, "notes", "vacuity guard")

            store.restoreCreatedAppLanding(
                LocalAppsStore.CreatedAppLanding(appID: "notes", initSessionID: "s-1"))

            XCTAssertEqual(store.consumeCreatedAppLanding()?.appID, "notes")
            XCTAssertNil(
                store.consumeCreatedAppLanding(),
                "one app must not be handed off twice")
        }

        /// The production stop-loss is THIRTY seconds, and a fresh store uses
        /// it.
        ///
        /// The timer tests below shorten `createResultTimeout` so they can
        /// exercise the real code path in milliseconds; this is what stops
        /// that override from quietly becoming the shipped value.
        func testTheCreateResultStopLossIsThirtySeconds() {
            XCTAssertEqual(LocalAppsStore.defaultCreateResultTimeout, .seconds(30))
            XCTAssertEqual(
                LocalAppsStore().createResultTimeout,
                LocalAppsStore.defaultCreateResultTimeout,
                "a store built the production way must use the production timeout")
        }

        /// On timeout the pending create is dropped, the user is told the
        /// result is unknown and pointed at the library — and a matching
        /// `AppCreated` that shows up LATER must no longer be claimed.
        ///
        /// The second half is the part that matters: a timeout that only
        /// showed a message while leaving the claim armed would still hijack
        /// the user minutes later.
        func testTheCreateResultTimeoutClearsPendingAndStopsClaiming() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.createResultTimeout = .milliseconds(30)
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            try await Task.sleep(for: .milliseconds(300))

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_creation_result_unknown"))
            XCTAssertNotEqual(
                store.errorMessage, "local_apps_creation_result_unknown",
                "the key must resolve in the catalog, not fall through as itself")

            let late = shellRecord(id: "late", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: late, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: late)))
            XCTAssertNil(
                store.createdAppLanding,
                "a create that already timed out must not hijack the user later")

            // …and the store is usable again rather than latched shut.
            let again = await store.createShellApp(armLibraryFallback: true)
            XCTAssertTrue(again, "the timeout is a stop-loss, not a permanent latch")
        }

        /// `RootView`'s created-app landing retries `switchScope` up to 40
        /// times (250ms apart) before giving up; the give-up path calls this
        /// method directly rather than routing back through the create-result
        /// timeout. This pins only that store-side half of the hand-off: that
        /// calling it surfaces the same "result unknown" copy the timeout
        /// path uses, resolved out of the catalog rather than falling through
        /// as the raw key. It does NOT exercise `RootView`'s retry loop, the
        /// kickoff-send-once-landed guard, or the 40-attempt bound itself —
        /// that wiring lives in `RootView.swift`, outside this file.
        func testReportCreatedAppLandingExhaustedSurfacesTheUnknownResultCopy() {
            let store = LocalAppsStore()
            XCTAssertNil(store.errorMessage, "a fresh store must start with no error surfaced")

            store.reportCreatedAppLandingExhausted()

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_creation_result_unknown"))
            XCTAssertNotEqual(
                store.errorMessage, "local_apps_creation_result_unknown",
                "the key must resolve in the catalog, not fall through as itself")
        }

        /// A reconnect clears the pending create, says so, and refuses to
        /// claim anything that arrives afterwards.
        ///
        /// `AppCreated` is one-shot on the connection that was just torn down,
        /// so the answer to this create can never arrive on the new one.
        func testAReconnectClearsPendingAndRefusesToClaimALaterEvent() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))

            await store.refreshAfterEngineRebind()

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_creation_result_unknown"))

            let stray = shellRecord(id: "stray", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: stray, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: stray)))
            XCTAssertNil(store.createdAppLanding)
            XCTAssertNil(store.consumeCreatedAppID())
        }

        /// `landingAwaitingPin` is a SECOND latch — armed by `AppCreated`,
        /// cleared only by a matching `AppRecordChanged` — separate from
        /// `pendingCreateRequestID`. A rebind must drop both: dropping only
        /// the request-id latch (what `refreshAfterEngineRebind` did before
        /// this test) leaves a record update for the same app id, arriving
        /// arbitrarily later for any unrelated reason, free to publish a
        /// landing and pull the user into a session they never asked to
        /// enter — mirroring `LocalAppsViewModel.kt:308` on Android, which
        /// already clears both on rebind.
        func testEngineRebindDropsALandingStillAwaitingItsPin() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            let created = shellRecord(id: "notes", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: key)))
            XCTAssertNil(
                store.createdAppLanding,
                "AppCreated never carries the pin — landingAwaitingPin is armed, "
                    + "not createdAppLanding")

            await store.refreshAfterEngineRebind()

            // `AppCreated` already cleared `pendingCreateRequestID` before the
            // pin was the only thing left outstanding, so this drop must
            // surface its OWN error rather than rely on the request-id
            // branch's — a rebind that drops only the pin latch previously
            // set nothing here and was silent.
            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_creation_result_unknown"),
                "dropping the pin latch on rebind must not be silent")

            // A record update for the SAME app id, arriving long after the
            // rebind, must not resurrect a landing for a session the user
            // never asked to enter.
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "notes", name: "untitled", initSessionId: "session-9"))))

            XCTAssertNil(
                store.createdAppLanding,
                "a rebind must drop the pin latch, not just the request-id latch")
        }

        /// The pin the engine mints right after `AppCreated` is a
        /// best-effort mint that can fail to arrive at all. Left unbounded,
        /// `landingAwaitingPin` would wait forever with no route into the
        /// app the user just watched get created.
        func testPinWaitTimeoutPublishesALandingWithNoSessionID() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.pinWaitTimeout = .milliseconds(30)
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            let created = shellRecord(id: "notes", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: key)))
            XCTAssertNil(
                store.createdAppLanding,
                "AppCreated alone must not publish the landing before the pin arrives")

            try await Task.sleep(for: .milliseconds(300))

            let landing = try XCTUnwrap(
                store.createdAppLanding,
                "the pin-wait stop-loss must publish the landing rather than wait forever")
            XCTAssertEqual(landing.appID, "notes")
            XCTAssertNil(
                landing.initSessionID,
                "no pin ever arrived, so the hand-off must open a fresh conversation")

            // A pin arriving late, after the stop-loss already fired and the
            // landing was consumed, must not resurrect a second landing.
            _ = store.consumeCreatedAppLanding()
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "notes", name: "untitled", initSessionId: "session-9"))))
            XCTAssertNil(
                store.createdAppLanding,
                "a pin arriving after the stop-loss fired must not resurrect a spent landing")
        }

        /// A pending (or queued) approval sheet is a promise to resolve ITS
        /// request against the source that raised it. That source is torn down
        /// by a rebind, so answering a SURVIVING sheet would submit a stale
        /// request id into the new engine.
        /// Mirrors `LocalAppsViewModel.kt`'s `abandonedSheet` capture.
        func testEngineRebindClearsPendingApprovalSheetsAndTheirQueuesAndReportsUnknownResult() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(
                    requestId: "proposal-1", appId: "alpha", diffs: [],
                    requiredFlowChanges: ["Add audit step"]))))
            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(
                    requestId: "proposal-2", appId: "beta", diffs: [],
                    requiredFlowChanges: ["Add audit step"]))))
            XCTAssertEqual(store.pendingMcpProposalApproval?.requestID, "proposal-1")

            store.handle(event: .appEvent(event: .appProfileProposal(
                proposal: AppAgentProfileProposalDto(
                    appId: "tracker",
                    approvalToken: "token-1",
                    baseRevision: 1,
                    currentRevision: 2,
                    instructions: "Add a filter",
                    reason: "User asked for a filter"
                )
            )))
            XCTAssertEqual(store.pendingProfileProposal?.approvalToken, "token-1")

            await store.refreshAfterEngineRebind()

            XCTAssertNil(
                store.pendingMcpProposalApproval,
                "an approval sheet answered into a torn-down source must not survive a rebind")
            XCTAssertNil(store.pendingProfileProposal)
            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_creation_result_unknown"))

            // The queued "beta" proposal must be gone too, not just the
            // visible sheet: if it survived, a third proposal for the
            // SAME app would supersede it in place (and reject its stale
            // request id) instead of becoming the new pending sheet directly.
            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(
                    requestId: "proposal-3", appId: "beta", diffs: [],
                    requiredFlowChanges: ["Add audit step"]))))
            XCTAssertEqual(
                store.pendingMcpProposalApproval?.requestID, "proposal-3",
                "the queue must have been emptied by the rebind, not just the visible sheet")
            XCTAssertFalse(
                submitted.contains { command in
                    guard case let .pluginCommand(command: .resolveMcpProposalApproval(requestId, approved)) = command
                    else { return false }
                    return requestId == "proposal-2" && approved == false
                },
                "a queued proposal that survived the rebind must not be superseded/rejected later")
        }

        /// The app's very first bootstrap has no prior engine session to
        /// disown a create FROM: a create started while that bootstrap's
        /// catalog/`prepare()` round-trip is still in flight (the UI is
        /// already interactive) is not stale the way one surviving an ACTUAL
        /// rebind is, and must still be claimable when its `AppCreated` /
        /// `AppRecordChanged` pair arrives.
        func testInitialBootstrapDoesNotDisownACreateStartedDuringItsAsyncGap() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))

            await store.refreshAfterEngineRebind(isInitialBootstrap: true)

            XCTAssertNil(
                store.errorMessage,
                "the first bootstrap must not report the create's result as unknown")

            let created = shellRecord(id: "notes", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(
                record: shellRecord(id: "notes", name: "untitled", initSessionId: "session-9"))))

            XCTAssertEqual(
                store.createdAppLanding?.appID, "notes",
                "a create started during the first bootstrap's async gap must "
                    + "still be claimed, not disowned as if it predated a rebind")
        }

        /// A subsequent (non-initial) rebind still disowns a create in
        /// flight — `isInitialBootstrap` must not blanket-disable the
        /// stop-loss, only exempt the one bootstrap that has no prior
        /// session to rebind from.
        func testANonInitialRebindStillDisownsAPendingCreate() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            XCTAssertNotNil(sentCreateRequestID(submitted))

            await store.refreshAfterEngineRebind(isInitialBootstrap: false)

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_creation_result_unknown"))
        }

        /// A failure carrying OUR request id disarms the create and surfaces
        /// its message; a failure carrying somebody else's does neither.
        ///
        /// The old rule was `app_id == nil`, which is not a correlation key at
        /// all: an agent create that fails before it has a record also reports
        /// no app id, and letting that disarm this create leaves the app the
        /// user asked for sitting in the library with nobody waiting for it.
        func testOnlyAFailureCarryingOurRequestIdDisarmsTheCreate() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))

            store.handle(event: .appOperationFailed(
                appId: nil, code: .io, message: "别人的创建失败了",
                requestId: "somebody-elses"))
            let mine = shellRecord(id: "mine", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: mine, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: mine)))
            XCTAssertEqual(
                store.createdAppLanding?.appID, "mine",
                "another request's failure must not disarm this create")
            _ = store.consumeCreatedAppLanding()

            submitted.removeAll()
            _ = await store.createShellApp(armLibraryFallback: true)
            let second = try XCTUnwrap(sentCreateRequestID(submitted))
            store.handle(event: .appOperationFailed(
                appId: nil, code: .io, message: "创建失败", requestId: second))
            XCTAssertEqual(
                store.errorMessage, String(localized: "local_apps_error_operation_io"))

            let later = shellRecord(id: "later", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: later, requestId: second)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: later)))
            XCTAssertNil(
                store.createdAppLanding,
                "a create that already failed must not claim a later app")
        }

        /// `isCreateInFlight` must stay true for the WHOLE create — not just
        /// the round-trip that arms it — since it is the "+" button's only
        /// observed signal (`pendingCreateRequestID` itself is
        /// `@ObservationIgnored`). A view keying `.disabled` on a local flag
        /// scoped to the `await store.createShellApp(...)` call re-enables
        /// the button the instant that call returns, long before `AppCreated`
        /// / `AppOperationFailed` actually resolves the create.
        func testIsCreateInFlightStaysTrueUntilTheCreateResolves() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            XCTAssertFalse(store.isCreateInFlight, "nothing has started yet")

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            XCTAssertTrue(
                store.isCreateInFlight,
                "the round-trip to the engine finished, but the create itself "
                    + "has not resolved")

            store.handle(event: .appOperationFailed(
                appId: nil, code: .io, message: "disk full", requestId: key))
            XCTAssertFalse(
                store.isCreateInFlight, "the failure resolves the create")

            _ = await store.createShellApp(armLibraryFallback: true)
            let secondKey = try XCTUnwrap(sentCreateRequestID(Array(submitted.dropFirst())))
            let created = shellRecord(id: "arrived", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: secondKey)))
            XCTAssertFalse(
                store.isCreateInFlight, "AppCreated resolves the create too")
        }

        /// A double tap on "+" creates one app, not two.
        func testASecondCreateWhileOneIsInFlightIsRefused() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let first = await store.createShellApp(armLibraryFallback: true)
            let second = await store.createShellApp(armLibraryFallback: true)

            XCTAssertTrue(first)
            XCTAssertFalse(second, "only one create may be in flight at a time")
            XCTAssertEqual(
                submitted.count, 1, "the refused create must not reach the engine")
            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_error_create_in_progress"))
        }

        /// The drawer's create runs the SAME store path as the library's
        /// "+", and a create that fails leaves its message on the store for a
        /// presenter to read.
        ///
        /// This is the store half of the drawer create: the drawer creates
        /// with NO cover mounted, so `LocalAppsRootView`'s alert — the only
        /// presenter of `errorMessage` before this change — is not on screen
        /// when the engine reports the failure. Written so a store that
        /// swallowed the failure, or that disarmed the create without saying
        /// anything, FAILS: the message is compared against the engine's own
        /// text rather than merely checked for non-nil, the failure is
        /// correlated by the key the store actually put on the wire, and the
        /// channel is re-exercised after `clearError()` because dismissing
        /// the alert is what calls it.
        func testADrawerCreateSendsOneCreateAndLeavesTheFailureForAPresenter() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            // The DRAWER's own argument. There is no default to fall back to
            // any more — `createShellApp` refuses to guess — so this line is
            // what makes the test a DRAWER create rather than a library one
            // wearing a drawer's name.
            let started = await store.createShellApp(armLibraryFallback: false)

            XCTAssertTrue(
                started,
                "the drawer calls the same store method the library's + calls")
            XCTAssertEqual(
                submitted.count, 1,
                "one tap, one CreateApp — the drawer opens no library on the way")
            let key = try XCTUnwrap(
                sentCreateRequestID(submitted),
                "without a correlation key a failure cannot be matched back to this tap")
            XCTAssertNil(store.errorMessage, "nothing has failed yet")

            store.handle(event: .appOperationFailed(
                appId: nil, code: .io, message: "创建失败：磁盘写入被拒绝", requestId: key))

            XCTAssertEqual(
                store.errorMessage, String(localized: "local_apps_error_operation_io"),
                "the raw engine diagnostic is never localized for the client's "
                    + "locale, so the store must map `code` to a client-owned, "
                    + "localized message rather than surface it verbatim — see "
                    + "`LocalAppsStore.localizedOperationFailureMessage`")
            XCTAssertNil(
                store.createdAppLanding,
                "a create that failed must not land a session on the conversation")
            XCTAssertNil(
                store.createdAppID,
                "nor may a FAILED create arm any landing — `createdAppID` is "
                    + "written on AppCreated, and no AppCreated arrived")
            // NOTE: the line above pins the FAILURE path and nothing else. It
            // says nothing about `armLibraryFallback`: `createdAppID` is
            // assigned in exactly one place — the `.appCreated` arm's
            // `if armsLibraryFallback { createdAppID = summary.id }` — which
            // this test never reaches, so it is nil here for `true` and
            // `false` alike. The parameter is pinned, in both directions and
            // on the SUCCESS path, by
            // `testOnlyALibraryCreateArmsTheLibrarysFallbackLanding` below.

            // Dismissing the alert clears the channel — and does not wedge it.
            store.clearError()
            XCTAssertNil(store.errorMessage)

            submitted.removeAll()
            let again = await store.createShellApp(armLibraryFallback: false)
            XCTAssertTrue(
                again, "the failed create was disarmed, so the drawer may create again")
            let second = try XCTUnwrap(sentCreateRequestID(submitted))
            XCTAssertNotEqual(second, key, "each tap mints its own correlation key")
            store.handle(event: .appOperationFailed(
                appId: nil, code: .io, message: "创建失败：引擎已断开", requestId: second))
            XCTAssertEqual(
                store.errorMessage, String(localized: "local_apps_error_operation_io"),
                "the second failure must reach the presenter too")
        }

        /// `pendingProfileProposal` is a single overwritable slot with no
        /// queue: a second proposal arriving before the first is answered
        /// must not just silently replace it, stranding the engine holding
        /// the first one's approval token with nothing that will ever answer
        /// it — mirroring the same-app supersession already proven for
        /// create confirmations and MCP proposals below.
        func testASecondProfileProposalDeclinesTheSupersededOne() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            store.handle(event: .appEvent(event: .appProfileProposal(
                proposal: AppAgentProfileProposalDto(
                    appId: "tracker",
                    approvalToken: "token-1",
                    baseRevision: 1,
                    currentRevision: 2,
                    instructions: "Add a filter",
                    reason: "User asked for a filter"
                )
            )))
            XCTAssertEqual(store.pendingProfileProposal?.approvalToken, "token-1")

            store.handle(event: .appEvent(event: .appProfileProposal(
                proposal: AppAgentProfileProposalDto(
                    appId: "tracker",
                    approvalToken: "token-2",
                    baseRevision: 2,
                    currentRevision: 3,
                    instructions: "Add sorting",
                    reason: "User asked for sorting"
                )
            )))

            XCTAssertEqual(
                store.pendingProfileProposal?.approvalToken, "token-2",
                "the newer proposal must be the one shown")
            try await waitUntil {
                submitted.contains { command in
                    guard case let .resolveAppProfileProposal(appId, approvalToken, approved) = command
                    else { return false }
                    return appId == "tracker" && approvalToken == "token-1" && approved == false
                }
            }
        }

        func testMcpProposalShowsAllDiffKindsRejectsExplicitlyAndFailsClosedWhenUnchanged() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(
                    requestId: "proposal-1",
                    appId: "tracker",
                    diffs: [
                        .init(
                            kind: .added,
                            name: "added_tool",
                            before: nil,
                            after: toolSurface(name: "added_tool", title: "Added"),
                            changedFields: []
                        ),
                        .init(
                            kind: .removed,
                            name: "removed_tool",
                            before: toolSurface(name: "removed_tool", title: "Removed"),
                            after: nil,
                            changedFields: []
                        ),
                        .init(
                            kind: .changed,
                            name: "changed_tool",
                            before: toolSurface(name: "changed_tool", title: "Before"),
                            after: toolSurface(name: "changed_tool", title: "After"),
                            changedFields: [.name, .title, .description, .inputSchema, .outputSchema, .annotations, .execution, .visibleMeta, .semanticFlow, .permissionCeiling]
                        ),
                    ],
                    requiredFlowChanges: ["Add audit step"],
                    excludedCapabilities: ["delete_server"],
                    pendingGates: [gateStatus(id: "runner", status: .pending, available: false)]
                )
            )))

            let prompt = try XCTUnwrap(store.pendingMcpProposalApproval)
            XCTAssertEqual(prompt.toolDiffs.map(\.kind), [.added, .removed, .changed])
            XCTAssertEqual(prompt.toolDiffs.last?.changedFields.count, 10)
            XCTAssertEqual(prompt.pendingGates.first?.status, .pending)

            await store.resolvePendingMcpProposalApproval(false)

            XCTAssertTrue(submitted.contains { command in
                guard case let .pluginCommand(command: .resolveMcpProposalApproval(requestId, approved)) = command
                else { return false }
                return requestId == "proposal-1" && approved == false
            })
            XCTAssertNil(store.pendingMcpProposalApproval)

            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(requestId: "proposal-2", appId: "tracker", diffs: [])
            )))

            XCTAssertNil(store.pendingMcpProposalApproval)
            try await waitUntil {
                submitted.contains { command in
                    guard case let .pluginCommand(command: .resolveMcpProposalApproval(requestId, approved)) = command
                    else { return false }
                    return requestId == "proposal-2" && approved == false
                }
            }
        }

        /// The library's fallback landing is armed by a LIBRARY create and
        /// refused to a DRAWER one.
        ///
        /// `createdAppID` has exactly ONE consumer,
        /// `LocalAppsLibraryView.openCreatedAppIfNeeded`, driven by that
        /// screen's `onAppear` / `onChange(of: store.createdAppID)`. A drawer
        /// create runs with the cover down, so an id armed there is never
        /// drained where it was meant to be: it survives for the process
        /// lifetime and the user's next unrelated "View all" consumes it and
        /// drops them on that stale app's details page instead of the list.
        ///
        /// Both directions are asserted deliberately, because either half
        /// alone passes for a store that IGNORES `armLibraryFallback`: one
        /// that always arms satisfies the library half, one that never arms
        /// satisfies the drawer half. Only the pair pins the parameter. And
        /// the PRIMARY landing is asserted armed in both halves, so the
        /// drawer's "not armed" cannot be satisfied by a create that quietly
        /// did nothing at all.
        func testOnlyALibraryCreateArmsTheLibrarysFallbackLanding() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            // The drawer — `RootView.createLocalAppFromDrawer` passes false.
            let drawerStarted = await store.createShellApp(armLibraryFallback: false)
            XCTAssertTrue(drawerStarted, "the drawer's create must still reach the engine")
            let drawerKey = try XCTUnwrap(sentCreateRequestID(submitted))
            let drawerApp = shellRecord(id: "from-drawer", name: "untitled")
            store.handle(event: .appEvent(
                event: .appCreated(record: drawerApp, requestId: drawerKey)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: drawerApp)))

            XCTAssertNil(
                store.createdAppID,
                "a drawer create mounts no library cover, so an armed createdAppID "
                    + "is never consumed where it was armed: the next unrelated "
                    + "View all drains it and hijacks the user onto this app")
            XCTAssertEqual(
                store.createdAppLanding?.appID, "from-drawer",
                "only the FALLBACK is refused — the drawer create's own landing "
                    + "into the app's conversation must still arm")
            _ = store.consumeCreatedAppLanding()

            // The library's "+" — `LocalAppsLibraryView.createShellApp` passes true.
            submitted.removeAll()
            let libraryStarted = await store.createShellApp(armLibraryFallback: true)
            XCTAssertTrue(
                libraryStarted, "the resolved drawer create must not wedge the next one")
            let libraryKey = try XCTUnwrap(sentCreateRequestID(submitted))
            XCTAssertNotEqual(
                libraryKey, drawerKey, "each create mints its own correlation key")
            let libraryApp = shellRecord(id: "from-library", name: "untitled")
            store.handle(event: .appEvent(
                event: .appCreated(record: libraryApp, requestId: libraryKey)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: libraryApp)))

            XCTAssertEqual(
                store.createdAppID, "from-library",
                "the library's + is the fallback's only consumer: unarmed, its "
                    + "onAppear/onChange landing has nothing to fire on")
            XCTAssertEqual(
                store.consumeCreatedAppID(), "from-library",
                "and the cover must be able to drain exactly that id")
            XCTAssertNil(store.createdAppID, "the fallback landing is one-shot")
            XCTAssertEqual(
                store.createdAppLanding?.appID, "from-library",
                "the primary landing arms for a library create too")
        }

        /// The comments above name the two call sites; this pins them. A
        /// hand-passed literal in a test proves the STORE honours
        /// `armLibraryFallback` — it says nothing about which value each
        /// PRODUCT call site actually passes, and the drawer/library split
        /// silently inverting at either call site would leave every test
        /// above green.
        func testDrawerAndLibraryCreateCallSitesPassTheDocumentedFallbackFlag() throws {
            let rootView = try clientSource("Sources/App/RootView.swift")
            let libraryView = try clientSource("Sources/LocalApps/LocalAppsLibraryView.swift")

            XCTAssertTrue(
                rootView.contains("createShellApp(armLibraryFallback: false)"),
                "RootView.createLocalAppFromDrawer must pass false: the drawer "
                    + "mounts no library cover to consume the fallback")
            XCTAssertTrue(
                libraryView.contains("createShellApp(armLibraryFallback: true)"),
                "LocalAppsLibraryView.createShellApp must pass true: this screen "
                    + "IS the fallback's only consumer")
        }

        /// Dismissing the library cover between the '+' tap and `AppCreated`
        /// must not leave `createdAppID` armed for the process lifetime: the
        /// cover — the fallback's only consumer — is gone, so nothing will
        /// ever drain it, and a much later, unrelated visit to the library
        /// would otherwise be hijacked onto that stale app's details page.
        ///
        /// The PRIMARY landing must survive this: it is a separate latch
        /// (`createdAppLanding`), consumed by `RootView` independently of
        /// whether this cover is on screen, so dropping only the fallback
        /// claim cannot lose the create.
        func testClearingTheLibraryFallbackArmDropsTheStaleClaimButNotThePrimaryLanding() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            _ = await store.createShellApp(armLibraryFallback: true)
            let key = try XCTUnwrap(sentCreateRequestID(submitted))
            let created = shellRecord(id: "from-library", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: created, requestId: key)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: created)))

            XCTAssertEqual(store.createdAppID, "from-library")

            store.clearLibraryFallbackArm()

            XCTAssertNil(
                store.createdAppID,
                "the library cover disappeared before it could consume this — "
                    + "leaving it armed would hijack a later, unrelated visit")
            XCTAssertEqual(
                store.createdAppLanding?.appID, "from-library",
                "clearing the FALLBACK claim must not touch the PRIMARY landing "
                    + "RootView consumes independently of this cover")
        }

        /// The kickoff message resolves to real copy rather than falling
        /// through as its own key, in the test host's current locale.
        ///
        /// This is the ONLY thing that can catch a mistyped localization key:
        /// `String(localized:)` returns the key itself when it is absent, so a
        /// wrong key compiles, runs, and sends `local_apps_kickoff_message` to
        /// the model as the user's opening line — which is exactly what this
        /// branch did until the key was corrected. Asserted on
        /// `LocalAppKickoff.message`, the same value `RootView`'s
        /// `openCreatedAppSession` sends.
        func testTheKickoffMessageResolvesToRealCopy() {
            let message = LocalAppKickoff.message
            XCTAssertFalse(message.isEmpty)
            // Compared against the key IN USE, not a hardcoded spelling: an
            // assertion against a literal `"local_apps_kickoff"` passes for
            // any OTHER mistyped key, which is precisely the defect being
            // guarded. Verified by breaking the key on purpose and watching
            // this line fail.
            XCTAssertNotEqual(
                message, LocalAppKickoff.key,
                "the key fell through as itself — it is missing from the catalog")
            XCTAssertFalse(
                message.contains("%@"),
                "a shell has no brief to interpolate: the copy must be placeholder-free")
        }

        // ── Draft-state rendering ─────────────────────────────────────────

        /// A shell renders the localized draft copy and shows NEITHER its
        /// stored name nor its brief.
        ///
        /// The stored name is an engine placeholder the user never chose and
        /// the brief is empty, so both are values the UI must refuse to put on
        /// screen. Asserted against the localized catalog rather than a
        /// hardcoded string, and asserted to be DIFFERENT from the raw key, so
        /// deleting the key later fails this test instead of silently
        /// rendering `local_apps_draft_card_title` to the user.
        func testAShellRendersTheDraftCopyAndHidesItsNameAndBrief() {
            // Both stamps are "now": this test is about the draft COPY, and a
            // shell has to be genuinely fresh to render the non-stalled line.
            // `createdAtMs` is what the staleness window measures — it used to
            // measure `updatedAtMs`, and the fixture set only that.
            let nowMs = UInt64(Date().timeIntervalSince1970 * 1_000)
            let shell = LocalAppsProtocolAdapter.app(
                shellRecord(
                    id: "shell", name: "untitled",
                    createdAtMs: nowMs, updatedAtMs: nowMs))

            XCTAssertFalse(shell.scaffolded, "the flag must come off the wire")
            XCTAssertTrue(shell.isDraftShell)
            XCTAssertEqual(shell.displayName, String(localized: "local_apps_draft_card_title"))
            XCTAssertNotEqual(shell.displayName, "untitled", "the placeholder must not reach the UI")
            XCTAssertNotEqual(
                shell.displayName, "local_apps_draft_card_title",
                "the key must resolve in the catalog, not fall through as itself")
            XCTAssertEqual(
                shell.draftStatusLine, String(localized: "local_apps_draft_card_subtitle"))
            XCTAssertNotEqual(shell.draftStatusLine, "local_apps_draft_card_subtitle")
            XCTAssertNil(shell.displayBrief, "a shell's brief is empty and must not be rendered")

            let formed = LocalAppsProtocolAdapter.app(
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"))
            XCTAssertTrue(formed.scaffolded)
            XCTAssertEqual(formed.displayName, "记事本")
            XCTAssertNil(
                formed.draftStatusLine,
                "a formed app keeps its own status line — the draft line is the exception")
            XCTAssertEqual(formed.displayBrief, "一个记事本")
        }

        /// Every terminal create failure used to leave a card that read
        /// "Creating…" forever: `draftStatusLine` was a pure function of
        /// `isDraftShell` alone, with no notion of how long it had been
        /// stuck. A shell CREATED far enough in the past has not scaffolded in
        /// any bounded window the create flow itself waits on, so its card
        /// must say so instead of still claiming to be in progress.
        func testAStalledShellRendersAFailedCopyInsteadOfCreatingForever() {
            let now = Date().timeIntervalSince1970
            let freshShell = LocalAppsProtocolAdapter.app(
                shellRecord(
                    id: "fresh", name: "untitled",
                    createdAtMs: UInt64(now * 1_000),
                    updatedAtMs: UInt64(now * 1_000)))
            XCTAssertFalse(freshShell.isDraftStalled, "just created — still within the create flow's own window")
            XCTAssertEqual(
                freshShell.draftStatusLine, String(localized: "local_apps_draft_card_subtitle"))

            // The healthy path this window must not misread. The engine writes
            // `updated_at_ms` in `set_init_session` and then not again until
            // `commit_scaffold` — the entire interview with the user happens
            // between those two writes with the timestamp frozen — so a shell
            // half an hour old is routinely a create that is still going.
            // Both stamps move together here: this is a shell that really was
            // created 30 minutes ago and has been interviewed since.
            let midInterview = LocalAppsProtocolAdapter.app(
                shellRecord(
                    id: "interviewing", name: "untitled",
                    createdAtMs: UInt64((now - 30 * 60) * 1_000),
                    updatedAtMs: UInt64((now - 30 * 60) * 1_000)))
            XCTAssertFalse(
                midInterview.isDraftStalled,
                "a 30-minute-old shell is a normal in-progress interview, not a stall")
            XCTAssertEqual(
                midInterview.draftStatusLine, String(localized: "local_apps_draft_card_subtitle"))

            // `shellRecord`'s default `createdAtMs` (1ms past epoch) is, by
            // construction, always past the staleness window.
            let stalledShell = LocalAppsProtocolAdapter.app(
                shellRecord(id: "stuck", name: "untitled"))
            XCTAssertTrue(stalledShell.isDraftStalled)
            XCTAssertEqual(
                stalledShell.draftStatusLine,
                String(localized: "local_apps_draft_card_subtitle_stalled"))
            XCTAssertNotEqual(
                stalledShell.draftStatusLine, "local_apps_draft_card_subtitle_stalled",
                "the key fell through as itself — it is missing from the catalog")
            XCTAssertNotEqual(
                stalledShell.draftStatusLine, String(localized: "local_apps_draft_card_subtitle"),
                "a stalled shell must stop claiming to still be in progress")
            // Nothing here observed a failure, so it must not say one happened.
            XCTAssertNotEqual(
                stalledShell.draftStatusLine,
                String(localized: "local_apps_workflow_generation_failed"),
                "an unfinished setup is not an observed generation failure")
        }

        /// Search matches what the card SHOWS — and a draft shell shows
        /// nothing worth matching at all.
        ///
        /// A query typed against the placeholder name must not surface a card
        /// whose visible title has nothing to do with it. Every draft shell
        /// shares the same displayed title and the same `workflow.label`, so
        /// even the LOCALIZED placeholder title must not surface one:
        /// matching Android (`LocalAppsContract.kt`'s `app.scaffolded`
        /// term), a non-empty query excludes every unscaffolded app outright
        /// rather than matching it through borrowed, shared text.
        func testSearchExcludesDraftShellsAndMatchesAFormedAppsOwnName() {
            let store = LocalAppsStore()
            store.handle(event: .appsChanged(apps: [
                shellRecord(id: "shell", name: "untitled"),
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"),
            ]))

            store.searchQuery = "untitled"
            XCTAssertTrue(
                store.filteredApps.isEmpty,
                "the placeholder name is invisible to the user and must be invisible to search")

            store.searchQuery = String(localized: "local_apps_draft_card_title")
            XCTAssertTrue(
                store.filteredApps.isEmpty,
                "a draft shell must not surface through its own placeholder "
                    + "title, which every other shell shares too")

            store.searchQuery = "记事本"
            XCTAssertEqual(store.filteredApps.map(\.id), ["notes"])
        }

        // ── The widget ────────────────────────────────────────────────────

        /// A shell is EXCLUDED from the home-screen snapshot, not relabelled.
        ///
        /// The widget extension renders `name` and `brief` straight out of the
        /// snapshot — it never sees this client's draft branches — so a shell
        /// left in it appears on the user's home screen as a placeholder-named
        /// icon that cannot be opened.
        func testTheWidgetSnapshotExcludesUnscaffoldedShells() throws {
            let store = LocalAppsStore()
            var published: [LocalAppWidgetSnapshot] = []
            store.widgetSnapshotPublisher = { snapshot in
                published.append(snapshot)
                return nil
            }
            store.handle(event: .appsChanged(apps: [
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"),
            ]))
            store.requestWidgetSetup(appID: "notes")

            let snapshot = try XCTUnwrap(published.last)
            XCTAssertEqual(snapshot.apps.map(\.id), ["notes"])

            published.removeAll()
            store.handle(event: .appsChanged(apps: [
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"),
                shellRecord(id: "shell", name: "untitled"),
            ]))
            store.requestWidgetSetup(appID: "notes")
            let second = try XCTUnwrap(published.last)
            XCTAssertEqual(
                second.apps.map(\.id), ["notes"],
                "the shell must not reach the home screen")
        }

        /// The widget entry's new home arms the setup guide for a formed app
        /// and refuses a shell.
        func testRequestWidgetSetupArmsAFormedAppAndRefusesAShell() {
            let store = LocalAppsStore()
            store.widgetSnapshotPublisher = { _ in nil }
            store.handle(event: .appsChanged(apps: [
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"),
                shellRecord(id: "shell", name: "untitled"),
            ]))

            store.requestWidgetSetup(appID: "shell")
            XCTAssertNil(
                store.pendingWidgetSetup,
                "a shell is not in the snapshot, so there is nothing to pin")

            store.requestWidgetSetup(appID: "notes")
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "notes")
            XCTAssertEqual(store.pendingWidgetSetup?.appName, "记事本")

            store.completeWidgetSetup()
            XCTAssertNil(store.pendingWidgetSetup)
        }

        /// A missing App Group is a CONFIGURATION fact, not a failure: the
        /// guide still arms so the user can be walked through it.
        func testWidgetSetupStillArmsWhenTheAppGroupIsMissing() {
            let store = LocalAppsStore()
            store.widgetSnapshotPublisher = { _ in
                LocalAppWidgetSnapshotStore.SnapshotError.containerUnavailable
            }
            store.handle(event: .appsChanged(apps: [
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"),
            ]))

            store.requestWidgetSetup(appID: "notes")

            XCTAssertNil(store.errorMessage)
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "notes")
        }

        /// A REAL write failure is different from a missing container and must
        /// still reach the user.
        func testWidgetSetupSurfacesARealSnapshotWriteFailure() {
            struct DiskFull: Error {}
            let store = LocalAppsStore()
            store.widgetSnapshotPublisher = { _ in DiskFull() }
            store.handle(event: .appsChanged(apps: [
                formedRecord(id: "notes", name: "记事本", brief: "一个记事本"),
            ]))

            store.requestWidgetSetup(appID: "notes")

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_error_widget_snapshot"))
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "notes")
        }

        // ── r3-never-wired-09: the engine's verification `code` ───────────

        /// Every value `LocalAppVerificationSummaryDto.code` can carry maps to
        /// the client's own catalog, INCLUDING `nil`.
        ///
        /// Existing MCP summaries use `needs_setup`, `needs_revalidation`,
        /// `verification_unavailable`, or no code for a clean MCP pass. Host
        /// UI verification uses three explicit `ui_verification_*` codes, so a
        /// UI pass can never reuse the nil-code MCP sentence.
        ///
        /// Each key is asserted to differ BOTH from the raw key (so deleting
        /// the key from the catalog fails here instead of putting
        /// `local_apps_verification_summary_passed` on screen) and from the
        /// engine sentence it replaces (so a mapping that quietly falls
        /// through to `summary` cannot pass).
        func testVerificationSummaryLocalizesEveryEngineCodeIncludingTheNilPass() {
            let cases: [(code: String?, status: LocalAppVerificationStatus, key: String)] = [
                ("needs_setup", .unverified, "local_apps_verification_summary_needs_setup"),
                (
                    "needs_revalidation", .unverified,
                    "local_apps_verification_summary_needs_revalidation"
                ),
                (
                    "verification_unavailable", .unavailable,
                    "local_apps_verification_summary_verification_unavailable"
                ),
                (
                    "ui_verification_required", .unverified,
                    "local_apps_verification_summary_ui_verification_required"
                ),
                (
                    "ui_verification_passed", .passed,
                    "local_apps_verification_summary_ui_verification_passed"
                ),
                (
                    "ui_verification_corrupt", .failed,
                    "local_apps_verification_summary_ui_verification_corrupt"
                ),
                (nil, .passed, "local_apps_verification_summary_passed"),
            ]
            for (code, status, key) in cases {
                let engineEnglish = "ENGINE SENTENCE for \(code ?? "nil")"
                let summary = LocalAppVerificationSummary(
                    status: status, summary: engineEnglish, code: code)
                let localized = String(localized: String.LocalizationValue(stringLiteral: key))
                XCTAssertEqual(
                    summary.localizedSummary, localized,
                    "code \(code ?? "nil") must render the client catalog's sentence")
                XCTAssertNotEqual(
                    summary.localizedSummary, key,
                    "\(key) must resolve in the catalog, not fall through as its own name")
                XCTAssertNotEqual(
                    summary.localizedSummary, engineEnglish,
                    "code \(code ?? "nil") must NOT fall through to the engine's English")
            }

            let uiPass = LocalAppVerificationSummary(
                status: .passed,
                summary: "Host UI/data verification evidence passed.",
                code: "ui_verification_passed")
            XCTAssertNotEqual(
                uiPass.localizedSummary,
                String(localized: "local_apps_verification_summary_passed"),
                "a UI QA pass must never render the nil-code MCP verification sentence")
        }

        /// An unrecognized future code keeps the engine's sentence rather than
        /// showing nothing — the same rule `localizedGateLabel` follows for
        /// gate ids. `active_state_corrupt` is a real one: the engine emits it
        /// (`local_apps_host.rs`, the catalog-identity mismatch) and no key
        /// exists for it yet.
        func testAnUnknownVerificationCodeFallsBackToTheEngineSentence() {
            let summary = LocalAppVerificationSummary(
                status: .failed,
                summary: "The approved MCP catalog does not match this app and build.",
                code: "active_state_corrupt")

            XCTAssertEqual(
                summary.localizedSummary,
                "The approved MCP catalog does not match this app and build.")
        }

        /// A summary this CLIENT built has no wire code, so "no code" must not
        /// be read as the engine's one no-code state.
        ///
        /// `LocalAppManagedMcpInventoryReader` builds a `passed` UI
        /// verification off the on-disk manifest with no code; without the
        /// `isHostSourced` guard its "Published UI verification passed."
        /// rendered as the MCP pass sentence.
        func testAClientBuiltPassedSummaryKeepsItsOwnSentence() {
            let clientBuilt = LocalAppVerificationSummary(
                status: .passed,
                summary: "Published UI verification passed.",
                code: nil,
                isHostSourced: false)

            XCTAssertEqual(clientBuilt.localizedSummary, "Published UI verification passed.")
            XCTAssertNotEqual(
                clientBuilt.localizedSummary,
                String(localized: "local_apps_verification_summary_passed"))
        }

        // ── r4-never-wired-07: `AppRecordDto.created_at_ms` ────────────────

        /// The shell staleness window runs from CREATION, not from the last
        /// mutation.
        ///
        /// `set_init_session` is set-once (only its `None` arm writes
        /// `updated_at_ms`; a pinned record is refused) and the boot sweep
        /// skips already-pinned records, so the re-stamp below is not a
        /// once-per-launch event. It is the one case that survives both
        /// guards: a shell whose create died BEFORE it could pin. The next
        /// launch's backfill pins it and `updated_at_ms` jumps to now — so a
        /// window anchored on `updated_at_ms` restarts on a create that has
        /// been dead for days, and the card claims it is still in flight.
        /// `created_at_ms` is immune, which is what this pins.
        func testAShellStalenessIsMeasuredFromCreationNotLastUpdate() {
            let nowMs = UInt64(Date().timeIntervalSince1970 * 1_000)
            let threeDaysAgoMs = nowMs - UInt64(3 * 24 * 60 * 60 * 1_000)

            let backfilled = LocalAppsProtocolAdapter.app(
                shellRecord(
                    id: "abandoned", name: "untitled",
                    createdAtMs: threeDaysAgoMs, updatedAtMs: nowMs))

            XCTAssertEqual(
                backfilled.createdAt,
                Date(timeIntervalSince1970: TimeInterval(threeDaysAgoMs) / 1_000),
                "created_at_ms must be decoded off the wire, not dropped")
            XCTAssertTrue(
                backfilled.isDraftStalled,
                "a shell created three days ago is stalled however recently the "
                    + "boot backfill touched its record")
            XCTAssertEqual(
                backfilled.draftStatusLine,
                String(localized: "local_apps_draft_card_subtitle_stalled"))

            let fresh = LocalAppsProtocolAdapter.app(
                shellRecord(
                    id: "creating", name: "untitled",
                    createdAtMs: nowMs, updatedAtMs: nowMs))
            XCTAssertFalse(fresh.isDraftStalled, "a shell created seconds ago is still creating")
        }

        // ── the expired rejection ──────────────────────────────────────────

        /// The engine's "unknown or expired" rejection must not reach the user
        /// as raw English.
        ///
        /// `ClientError` is flat by design, so there is no code to switch on;
        /// what makes this localizable is the CALL SITE — `host.rs`'s
        /// `ResolveMcpProposalApproval` arm has exactly one `Err` path.
        func testAnExpiredMcpProposalRejectionIsLocalized() async throws {
            let store = LocalAppsStore()
            let engineEnglish = "unknown or expired Local App MCP proposal approval"
            store.configure { _ in throw ClientError.Rejected(message: engineEnglish) }

            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(
                    requestId: "mcp-1", appId: "tracker",
                    diffs: [
                        .init(
                            kind: .added,
                            name: "added_tool",
                            before: nil,
                            after: toolSurface(name: "added_tool", title: "Added"),
                            changedFields: []
                        )
                    ])
            )))
            await store.resolvePendingMcpProposalApproval(true)

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_error_operation_interaction_invalid"))
            XCTAssertNotEqual(
                store.errorMessage, engineEnglish,
                "the engine's English must not reach the alert")
            XCTAssertFalse(
                store.errorMessage?.contains("ClientError") ?? false,
                "UniFFI's errorDescription is String(reflecting:) — the Swift debug "
                    + "dump must not reach the alert either")
        }

        /// The MCP proposal answer takes the same path, and a NON-rejection
        /// failure keeps its own description rather than being relabelled
        /// "no longer pending".
        func testANonRejectionApprovalFailureIsNotRelabelled() async throws {
            struct Offline: LocalizedError {
                var errorDescription: String? { "the socket dropped" }
            }
            let store = LocalAppsStore()
            store.configure { _ in throw Offline() }

            // A NON-empty diff: an empty-diff proposal is auto-declined by the
            // store, so the sheet would never be pending for this test to answer.
            store.handle(event: .appEvent(event: .mcpProposalApprovalRequested(
                request: mcpProposalRequest(
                    requestId: "mcp-1", appId: "tracker",
                    diffs: [
                        .init(
                            kind: .added,
                            name: "added_tool",
                            before: nil,
                            after: toolSurface(name: "added_tool", title: "Added"),
                            changedFields: []
                        )
                    ])
            )))
            XCTAssertEqual(store.pendingMcpProposalApproval?.requestID, "mcp-1")
            await store.resolvePendingMcpProposalApproval(true)

            XCTAssertEqual(store.errorMessage, "the socket dropped")
            XCTAssertNotEqual(
                store.errorMessage,
                String(localized: "local_apps_error_operation_interaction_invalid"),
                "only a Rejected means 'no longer pending'")
        }

        // ── Fixtures ──────────────────────────────────────────────────────

        /// The `request_id` the store actually put on the wire.
        ///
        /// Read off the submitted command rather than fabricated, so a store
        /// that never correlates at all cannot pass these tests by having a
        /// test echo a key it invented.
        private func sentCreateRequestID(_ submitted: [ClientCommand]) -> String? {
            for command in submitted {
                if case let .createApp(_, _, _, _, _, _, _, _, requestId) = command {
                    return requestId
                }
            }
            return nil
        }

        /// A wire record, spelled out here rather than taken from the shared
        /// `appRecord` fixture, so `scaffolded` is stated explicitly at every
        /// call site instead of inherited from a default that would make the
        /// draft tests pass for the wrong reason.
        private func wireRecord(
            id: String,
            name: String,
            brief: String,
            scaffolded: Bool,
            initSessionId: String? = nil,
            // `1`/`2`ms-past-epoch by default, the same fixed placeholders
            // every one of these fixtures used before
            // `LocalAppSummary.isDraftStalled` existed. A caller exercising
            // that staleness window passes explicit values instead —
            // everyone else keeps the old constants, so these defaults change
            // NOTHING for the tests that predate them.
            createdAtMs: UInt64 = 1,
            updatedAtMs: UInt64 = 2
        ) -> AppRecordDto {
            AppRecordDto(
                id: id,
                name: name,
                brief: brief,
                gitEnabled: true,
                createdAtMs: createdAtMs,
                updatedAtMs: updatedAtMs,
                workflowState: .draft,
                conversationId: nil,
                initSessionId: initSessionId,
                workspaceRel: "apps/\(id)/workspace",
                scaffolded: scaffolded
            )
        }

        /// An unscaffolded shell: an engine placeholder for a name, an EMPTY
        /// brief — exactly what `mode: .shell` commits.
        private func shellRecord(
            id: String,
            name: String,
            initSessionId: String? = nil,
            createdAtMs: UInt64 = 1,
            updatedAtMs: UInt64 = 2
        ) -> AppRecordDto {
            wireRecord(
                id: id, name: name, brief: "", scaffolded: false,
                initSessionId: initSessionId,
                createdAtMs: createdAtMs, updatedAtMs: updatedAtMs)
        }

        /// An app whose scaffold has landed.
        private func formedRecord(
            id: String,
            name: String,
            brief: String,
            initSessionId: String? = nil
        ) -> AppRecordDto {
            wireRecord(
                id: id, name: name, brief: brief, scaffolded: true,
                initSessionId: initSessionId)
        }

        private func waitUntil(
            timeout: Duration = .seconds(1),
            condition: @escaping @MainActor () -> Bool
        ) async throws {
            let deadline = ContinuousClock.now + timeout
            while ContinuousClock.now < deadline {
                if condition() { return }
                try await Task.sleep(for: .milliseconds(20))
            }
            XCTAssertTrue(condition(), "condition not satisfied before timeout")
        }

        private func mcpProposalRequest(
            requestId: String,
            appId: String,
            diffs: [LocalAppMcpToolDiffDto],
            requiredFlowChanges: [String] = [],
            excludedCapabilities: [String] = [],
            pendingGates: [LocalAppGateStatusDto] = []
        ) -> LocalAppMcpProposalApprovalRequestDto {
            LocalAppMcpProposalApprovalRequestDto(
                requestId: requestId,
                appId: appId,
                workflowRunId: "wf-\(requestId)",
                summary: "Proposal summary",
                proposalSha256: String(repeating: "p", count: 64),
                approvalContractSha256: String(repeating: "a", count: 64),
                toolSurfaceSha256: String(repeating: "t", count: 64),
                toolDiffs: diffs,
                requiredFlowChanges: requiredFlowChanges,
                excludedCapabilities: excludedCapabilities,
                pendingGates: pendingGates
            )
        }

        private func gateStatus(
            id: String,
            status: LocalAppVerificationStatusDto,
            available: Bool
        ) -> LocalAppGateStatusDto {
            LocalAppGateStatusDto(
                gateId: id,
                label: "Runner availability",
                status: status,
                available: available,
                detail: available ? "ready" : "runner unavailable"
            )
        }

        private func toolSurface(name: String, title: String?) -> LocalAppMcpToolSurfaceDto {
            LocalAppMcpToolSurfaceDto(
                name: name,
                title: title,
                description: "Reads local state",
                inputSchemaJson: #"{"type":"object"}"#,
                outputSchemaJson: #"{"type":"object"}"#,
                annotationsJson: #"{"readOnlyHint":true}"#,
                executionJson: #"{"transport":"stdio"}"#,
                visibleMetaJson: #"{"visible":true}"#,
                semanticFlowJson: #"{"steps":[{"type":"read"}]}"#,
                permissionCeiling: "read-only"
            )
        }
    #endif

    // MARK: - Smoke-gate spike (master order step 3b) — MEASURED, not designed

    // The verification design's candidate mount point for the smoke gate is a
    // store-held OFFSCREEN `WKWebView`. It refuses to assume the three cheap
    // ways of hiding a view are usable:
    //
    //   "不得用零尺寸、isHidden=true 或 alpha=0 伪装离屏,
    //    这三者都可能让 WebKit 不布局/不绘制"
    //
    // "可能" is the whole problem. Four paper proposals died to four different
    // causes — timing, page-load semantics, the permission table, result
    // observability — and none was findable by re-reading harder. `:21` of
    // LocalAppWebView.swift already warns that `takeSnapshot` can call back
    // with NEITHER an image NOR an error for "an offscreen or not-yet-
    // composited view", which is precisely the failure this spike must rule in
    // or out on real hardware.
    //
    // S1 and S2 are a PAIR. S1 alone proves nothing: a snapshot could come back
    // red for reasons unrelated to the offscreen container. S2 is the control —
    // the same page in a zero-sized view must NOT come back painted. If S2 ever
    // starts passing the way S1 does, S1 has stopped measuring anything.

    private func spikeKeyWindow() throws -> UIWindow {
        let window = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
            .first { $0.isKeyWindow }
        return try XCTUnwrap(window, "the unit-test host app must expose a key window")
    }

    /// Polls the DOM rather than trusting `didFinish`: whether navigation
    /// callbacks even fire for a view parked outside the visible bounds is one
    /// of the things under measurement, so it cannot also be the instrument.
    private func spikeWaitForBody(_ webView: WKWebView) async throws -> Bool {
        for _ in 0 ..< 200 {
            if let ready = try? await webView.evaluateJavaScript(
                "document.getElementById('ready') !== null"
            ) as? Bool, ready { return true }
            try await Task.sleep(for: .milliseconds(25))
        }
        return false
    }

    private func spikeSnapshot(_ webView: WKWebView) async throws -> UIImage {
        let configuration = WKSnapshotConfiguration()
        configuration.rect = webView.bounds
        return try await withCheckedThrowingContinuation { continuation in
            webView.takeSnapshot(with: configuration) { snapshot, error in
                if let snapshot {
                    continuation.resume(returning: snapshot)
                } else {
                    continuation.resume(throwing: error ?? LocalAppSnapshotError.unavailable)
                }
            }
        }
    }

    /// The centre pixel, read straight out of the bitmap. A solid fill colour is
    /// the only assertion here that a blank-but-correctly-sized image cannot pass.
    private func spikeCentrePixel(_ image: UIImage) throws -> (r: Int, g: Int, b: Int) {
        let cgImage = try XCTUnwrap(image.cgImage, "snapshot carried no CGImage")
        let centre = CGRect(x: cgImage.width / 2, y: cgImage.height / 2, width: 1, height: 1)
        let cropped = try XCTUnwrap(cgImage.cropping(to: centre), "could not crop the centre pixel")
        var pixel = [UInt8](repeating: 0, count: 4)
        let context = try XCTUnwrap(CGContext(
            data: &pixel,
            width: 1, height: 1,
            bitsPerComponent: 8, bytesPerRow: 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.draw(cropped, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        return (Int(pixel[0]), Int(pixel[1]), Int(pixel[2]))
    }

    private static let spikeRedPage = """
    <body style="margin:0"><div id="ready" \
    style="width:100vw;height:100vh;background:#FF0000"></div></body>
    """

    /// A non-throwing centre-pixel read. The negative-path tests need to
    /// distinguish "no image" from "black image" WITHOUT recording a failure —
    /// `XCTUnwrap` registers one even when the throw is swallowed by `try?`,
    /// which is exactly how the first run of this spike produced a red herring.
    private func spikeCentrePixelIfAny(_ image: UIImage?) -> (r: Int, g: Int, b: Int)? {
        guard let cgImage = image?.cgImage else { return nil }
        let centre = CGRect(x: cgImage.width / 2, y: cgImage.height / 2, width: 1, height: 1)
        guard let cropped = cgImage.cropping(to: centre) else { return nil }
        var pixel = [UInt8](repeating: 0, count: 4)
        guard let context = CGContext(
            data: &pixel,
            width: 1, height: 1,
            bitsPerComponent: 8, bytesPerRow: 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { return nil }
        context.draw(cropped, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        return (Int(pixel[0]), Int(pixel[1]), Int(pixel[2]))
    }

    private func spikeIsRed(_ pixel: (r: Int, g: Int, b: Int)?) -> Bool {
        guard let pixel else { return false }
        return pixel.r > 200 && pixel.g < 60 && pixel.b < 60
    }

    /// Runs the red page in one placement strategy and reports its centre pixel.
    /// `nil` means the snapshot produced no bitmap at all.
    private func spikeMeasure(
        place: (UIView) -> Void,
        cleanup: () -> Void
    ) async throws -> (webKit: (r: Int, g: Int, b: Int)?, uiKit: (r: Int, g: Int, b: Int)?) {
        let host = UIView(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
        place(host)
        defer { cleanup(); host.removeFromSuperview() }

        let webView = WKWebView(frame: host.bounds, configuration: WKWebViewConfiguration())
        host.addSubview(webView)
        webView.loadHTMLString(Self.spikeRedPage, baseURL: nil)
        guard try await spikeWaitForBody(webView) else { return (nil, nil) }

        // Force a layout + let the compositor turn at least once. Omitting this
        // was the leading alternative explanation for a black snapshot, so it is
        // done for EVERY strategy — otherwise a negative result is unattributable.
        host.setNeedsLayout()
        host.layoutIfNeeded()
        try await Task.sleep(for: .milliseconds(250))

        // TWO independent instruments. `takeSnapshot` is WebKit's own path and
        // is documented to return nothing for a non-composited view; UIKit's
        // `drawHierarchy` renders the layer tree instead. If both come back
        // black the negative is instrument-independent, which is the only way a
        // "pixels are impossible offscreen" claim is worth acting on.
        let byWebKit = spikeCentrePixelIfAny(try? await spikeSnapshot(webView))
        let byUIKit = spikeCentrePixelIfAny(
            UIGraphicsImageRenderer(bounds: host.bounds).image { _ in
                host.drawHierarchy(in: host.bounds, afterScreenUpdates: true)
            }
        )
        return (webKit: byWebKit, uiKit: byUIKit)
    }

    /// S1 — the candidate's core mechanism, measured across every placement that
    /// keeps the page off the user's screen. The design named exactly one
    /// ("non-zero fixed viewport … container moved outside the visible bounds")
    /// and forbade three others without measuring them. A single strategy
    /// failing does not condemn the approach; what the gate needs is whether
    /// ANY offscreen placement composites on real hardware.
    func testSpikeWhichOffscreenPlacementsActuallyPaint() async throws {
        let window = try spikeKeyWindow()
        var extraWindow: UIWindow?
        var report: [(String, String)] = []

        let strategies: [(String, (UIView) -> Void, () -> Void)] = [
            ("outside-bounds x=-10000", { host in
                host.frame.origin = CGPoint(x: -10_000, y: 0)
                window.addSubview(host)
            }, {}),
            ("below-the-fold y=height", { host in
                host.frame.origin = CGPoint(x: 0, y: window.bounds.height)
                window.addSubview(host)
            }, {}),
            ("behind content, sent to back", { host in
                window.addSubview(host)
                window.sendSubviewToBack(host)
            }, {}),
            ("alpha 0.01 at origin", { host in
                host.alpha = 0.01
                window.addSubview(host)
            }, {}),
            ("own window below normal level", { host in
                let scene = window.windowScene
                let carrier = scene.map(UIWindow.init(windowScene:)) ?? UIWindow(frame: window.bounds)
                carrier.frame = window.bounds
                carrier.windowLevel = .normal - 1
                carrier.isHidden = false
                carrier.addSubview(host)
                extraWindow = carrier
            }, { extraWindow?.isHidden = true; extraWindow = nil }),
        ]

        var anyPainted = false
        for (name, place, cleanup) in strategies {
            let measured = try await spikeMeasure(place: place, cleanup: cleanup)
            let painted = spikeIsRed(measured.webKit) || spikeIsRed(measured.uiKit)
            anyPainted = anyPainted || painted
            func describe(_ pixel: (r: Int, g: Int, b: Int)?) -> String {
                guard let pixel else { return "no bitmap" }
                let tag = spikeIsRed(pixel) ? "PAINTED" : "blank"
                return "\(tag) rgb(\(pixel.r),\(pixel.g),\(pixel.b))"
            }
            report.append((name, "takeSnapshot=\(describe(measured.webKit))  drawHierarchy=\(describe(measured.uiKit))"))
        }

        let table = report.map { "  \($0.0.padding(toLength: 32, withPad: " ", startingAt: 0)) \($0.1)" }
            .joined(separator: "\n")
        print("SPIKE offscreen placement matrix (iOS \(UIDevice.current.systemVersion)):\n\(table)")

        XCTAssertTrue(
            anyPainted,
            "NO offscreen placement composites on this device. The smoke gate cannot "
                + "observe a page without taking the screen. Matrix:\n\(table)"
        )
    }

    /// S2 — the control. A zero-sized view is the cheap way to "hide" one and
    /// the design forbids it. Asserting the NEGATIVE is what makes a positive
    /// result in the matrix above attributable to layout rather than to WebKit
    /// painting regardless of geometry.
    func testSpikeAZeroSizedWebViewCannotStandInForAnOffscreenOne() async throws {
        let window = try spikeKeyWindow()
        let host = UIView(frame: .zero)
        window.addSubview(host)
        defer { host.removeFromSuperview() }

        let webView = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        host.addSubview(webView)
        webView.loadHTMLString(Self.spikeRedPage, baseURL: nil)
        _ = try await spikeWaitForBody(webView)

        let pixel = spikeCentrePixelIfAny(try? await spikeSnapshot(webView))
        XCTAssertFalse(
            spikeIsRed(pixel),
            "a zero-sized view painted the page — the matrix above stops measuring layout"
        )
    }

    /// S4 — the open question the placement matrix raises but cannot answer:
    /// `drawHierarchy` renders the UIKit layer tree, and GPU-composited content
    /// is exactly what it is known to miss. create-flow is landing `canvas-2d`
    /// and `threejs` runtime profiles whose ENTIRE surface is a composited
    /// canvas, so "pixels work offscreen" proven on a solid-colour <div> does
    /// not transfer to them.
    ///
    /// Two sub-probes, because two different things can go wrong:
    ///   - the canvas backing store may not be captured offscreen at all;
    ///   - `requestAnimationFrame` may be throttled or parked for a view the
    ///     system considers non-visible, in which case a three.js app renders
    ///     NOTHING offscreen regardless of how it is captured.
    /// Each page therefore paints ONCE synchronously and then keeps painting in
    /// rAF, so a blank result from the synchronous draw and a blank result from
    /// the rAF draw are distinguishable.
    func testSpikeWhetherOffscreenCanvasAndWebGLActuallyRender() async throws {
        let window = try spikeKeyWindow()

        func page(context: String) -> String {
            """
            <body style="margin:0">
            <canvas id="surface" style="width:100vw;height:100vh;display:block"></canvas>
            <div id="ready"></div>
            <script>
            var c = document.getElementById('surface');
            c.width = window.innerWidth; c.height = window.innerHeight;
            var kind = '\(context)';
            var painted = false;
            if (kind === '2d') {
              var ctx = c.getContext('2d');
              var paint = function () {
                ctx.fillStyle = '#FF0000';
                ctx.fillRect(0, 0, c.width, c.height);
                painted = true;
              };
              paint();
              var loop2d = function () { paint(); requestAnimationFrame(loop2d); };
              requestAnimationFrame(loop2d);
              window.__spikeContextOk = !!ctx;
            } else {
              var gl = c.getContext('webgl') || c.getContext('experimental-webgl');
              if (gl) {
                var paintGl = function () {
                  gl.clearColor(1, 0, 0, 1);
                  gl.clear(gl.COLOR_BUFFER_BIT);
                  painted = true;
                };
                paintGl();
                var loopGl = function () { paintGl(); requestAnimationFrame(loopGl); };
                requestAnimationFrame(loopGl);
              }
              window.__spikeContextOk = !!gl;
            }
            window.__spikeFrames = 0;
            requestAnimationFrame(function tick() {
              window.__spikeFrames++;
              requestAnimationFrame(tick);
            });
            </script>
            </body>
            """
        }

        var report: [String] = []
        for kind in ["2d", "webgl"] {
            let host = UIView(frame: CGRect(x: -10_000, y: 0, width: 390, height: 844))
            window.addSubview(host)
            defer { host.removeFromSuperview() }

            let webView = WKWebView(frame: host.bounds, configuration: WKWebViewConfiguration())
            host.addSubview(webView)
            webView.loadHTMLString(page(context: kind), baseURL: nil)
            let loaded = try await spikeWaitForBody(webView)
            host.setNeedsLayout()
            host.layoutIfNeeded()
            try await Task.sleep(for: .milliseconds(400))

            let contextOk = (try? await webView.evaluateJavaScript("window.__spikeContextOk === true")) as? Bool
            // Does rAF even run for a view the system does not consider visible?
            let frames = (try? await webView.evaluateJavaScript("window.__spikeFrames || 0")) as? Int

            let byWebKit = spikeCentrePixelIfAny(try? await spikeSnapshot(webView))
            let byUIKit = spikeCentrePixelIfAny(
                UIGraphicsImageRenderer(bounds: host.bounds).image { _ in
                    host.drawHierarchy(in: host.bounds, afterScreenUpdates: true)
                }
            )
            func describe(_ pixel: (r: Int, g: Int, b: Int)?) -> String {
                guard let pixel else { return "no bitmap" }
                return "\(spikeIsRed(pixel) ? "PAINTED" : "blank") rgb(\(pixel.r),\(pixel.g),\(pixel.b))"
            }
            report.append(
                "  \(kind.padding(toLength: 6, withPad: " ", startingAt: 0)) "
                    + "loaded=\(loaded) context=\(contextOk.map(String.init) ?? "nil") "
                    + "rAFframes=\(frames.map(String.init) ?? "nil")  "
                    + "takeSnapshot=\(describe(byWebKit))  drawHierarchy=\(describe(byUIKit))"
            )
        }

        print("SPIKE offscreen canvas/WebGL (iOS \(UIDevice.current.systemVersion)):\n"
            + report.joined(separator: "\n"))
        // Reporting probe: it records what the device does. Phase 2 reads the
        // table; there is no single pass/fail worth asserting until the design
        // decides which profiles must carry pixel evidence.
    }

    /// S3 — criterion 2, "fresh document". `updateUIView` returns early when the
    /// url is unchanged (`LocalAppWebView.swift:1104`) and a rebuild keeps the
    /// port stable, which is why a repaired app keeps showing the old page. This
    /// measures that an EXPLICIT reload of the identical url really does tear the
    /// document down, so the gate has a real mechanism to build on.
    func testSpikeReloadingTheIdenticalURLReplacesTheDocument() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let page = directory.appendingPathComponent("index.html")
        try #"<body><div id="ready">v1</div></body>"#.write(to: page, atomically: true, encoding: .utf8)

        let webView = WKWebView(frame: CGRect(x: 0, y: 0, width: 320, height: 480))
        webView.loadFileURL(page, allowingReadAccessTo: directory)
        let firstLoad = try await spikeWaitForBody(webView)
        XCTAssertTrue(firstLoad, "first load never produced a DOM")

        // Mark THIS document. Only a genuinely new document loses the mark.
        _ = try await webView.evaluateJavaScript("window.__spikeSameDocument = true; true")
        let markBefore = try await webView.evaluateJavaScript(
            "window.__spikeSameDocument === true"
        ) as? Bool
        XCTAssertEqual(markBefore, true, "the mark did not take, so its absence later proves nothing")

        webView.loadFileURL(page, allowingReadAccessTo: directory)
        let secondLoad = try await spikeWaitForBody(webView)
        XCTAssertTrue(secondLoad, "second load never produced a DOM")

        var markAfter: Bool? = true
        for _ in 0 ..< 200 {
            markAfter = try await webView.evaluateJavaScript(
                "window.__spikeSameDocument === true"
            ) as? Bool
            if markAfter == false { break }
            try await Task.sleep(for: .milliseconds(25))
        }
        XCTAssertEqual(
            markAfter, false,
            "the identical url was served the SAME document — a rebuilt app would keep showing stale bytes"
        )
    }

    // ── The created-app landing hand-off, read out of RootView.swift ──────
    //
    // Android pins the same chain in
    // `RootLocalAppPresenterSourceTest.kt`. The iOS half lives in
    // `RootView.openCreatedAppSession`, which is a private method on a
    // SwiftUI `View` with no seam this suite can drive, so the gate is a
    // source-level read of that one function — the same technique
    // `LocalAppsWidgetTests` already uses on this file.

    /// Reads a file from the iOS client tree relative to THIS test file, so
    /// the lookup cannot drift with whatever working directory the runner
    /// uses. Throws (failing the test) when the file is gone: an unreadable
    /// source must never read as "the assertion holds".
    private func clientSource(_ relativePath: String) throws -> String {
        let clientRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // clients/ios
        return try String(
            contentsOf: clientRoot.appendingPathComponent(relativePath),
            encoding: .utf8)
    }

    /// The local-app Code pin must cover MINTING only, never a resume.
    ///
    /// `switchScope` is the one seam every caller funnels through, so a pin
    /// written here with no `startNew` term also rewrites the mode of an
    /// explicit RESUME. `apps/engine-mobile/src/host.rs` rejects a cross-mode
    /// resume outright ("session ... belongs to chat mode, but this source runs
    /// code mode") and the app's session catalog lists rows of BOTH modes, so
    /// an unconditional pin makes every pre-existing Chat-mode app session
    /// permanently un-openable. It also turns the drawer's Chat tab into a
    /// silent no-op inside an app scope, since `switchMode` funnels through
    /// here too.
    func testTheLocalAppCodePinCoversMintingOnlyAndNotAResume() throws {
        let source = try clientSource("Sources/App/RootView.swift")

        guard let start = source.range(of: "private func switchScope("),
              let end = source.range(
                  of: "voiceInteraction.handleContextChange()",
                  range: start.upperBound ..< source.endIndex)
        else {
            return XCTFail("read the wrong file: switchScope's mode resolution not found")
        }
        let pin = try XCTUnwrap(
            String(source[start.lowerBound ..< end.lowerBound])
                .split(separator: "\n", omittingEmptySubsequences: false)
                .first(where: { $0.contains("let targetMode") }),
            "switchScope must still resolve a targetMode")

        XCTAssertTrue(
            pin.contains("scope.isLocalApp"),
            "a conversation MINTED in an app scope must still be pinned to Code")
        XCTAssertTrue(
            pin.contains("startNew"),
            """
            the pin must be gated on startNew: applied to a resume it discards the \
            session's own recorded mode, and host.rs rejects a cross-mode resume, so \
            every pre-existing Chat-mode app session becomes un-openable
            """)
    }

    /// Everything below is source-text containment INSIDE the sliced body of
    /// `openCreatedAppSession`. It proves the named code is written in that
    /// function; it does not run the function and proves nothing about the
    /// runtime behaviour of the retry.
    ///
    /// The slice matters: `switchScope(` and the sleep have other homes in
    /// this 3000-line view, so a whole-file check for them could not fail on
    /// the defect its own message names. Comment lines are stripped before
    /// asserting, because the function's own comment quotes the very
    /// `0..<40 where projectSwitching` shape the regression guard forbids.
    func testCreatedAppLandingRetriesTheScopeSwitchOnABoundedLoop() throws {
        let source = try clientSource("Sources/App/RootView.swift")

        guard let start = source.range(of: "private func openCreatedAppSession("),
              let end = source.range(
                  of: "private func startNewAppSession(",
                  range: start.upperBound ..< source.endIndex)
        else {
            return XCTFail("read the wrong file: openCreatedAppSession/startNewAppSession not found")
        }
        let body = String(source[start.lowerBound ..< end.lowerBound])
        let code = body
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")

        XCTAssertTrue(
            code.contains("switchScope("),
            "vacuity guard: the sliced openCreatedAppSession body must still call switchScope")

        XCTAssertTrue(
            code.contains("for _ in 0..<40 {"),
            """
            the retry must be a bounded LOOP over 40 attempts; a tail call back into \
            openCreatedAppSession is unbounded recursion on the MainActor
            """)
        XCTAssertFalse(
            code.contains("0..<40 where"),
            """
            `for _ in 0..<40 where <cond>` is a FILTER, not a wait: under the \
            mutation-policy refusal the condition is false, the body never runs and \
            nothing sleeps
            """)
        XCTAssertTrue(
            code.contains("milliseconds(250)"),
            "every retry attempt must sleep, otherwise 40 attempts are spent in one runloop tick")
        // A containment check cannot tell the first attempt from the retry --
        // both call `switchScope(` inside this slice -- so count instead: EVERY
        // attempt has to carry the kickoff, or the attempt that finally lands
        // opens the app's first conversation with no prompt in it.
        let switchCalls = code.components(separatedBy: "switchScope(").count - 1
        let kickoffCalls = code.components(separatedBy: "initialPrompt: kickoff").count - 1
        XCTAssertEqual(switchCalls, 2, "expected exactly two switch attempts: the first and the retry")
        XCTAssertEqual(
            kickoffCalls, switchCalls,
            """
            the kickoff must ride on the switch itself, so it is sent by whichever \
            attempt succeeded rather than fired independently of where the session landed
            """)
        XCTAssertTrue(
            code.contains("localAppsStore.reportCreatedAppLandingExhausted()"),
            """
            bounding the retry adds a give-up path the recursive version never had; it \
            must name the app rather than dropping a spent one-shot landing silently
            """)
        // The give-up path used to just report and drop the landing for good.
        // It must now RESTORE it (before reporting, order does not matter for
        // correctness but both calls must be present) so a later clearing of
        // whatever refused the switch for 10s can pick the hand-off back up
        // instead of losing it permanently.
        XCTAssertTrue(
            code.contains("localAppsStore.restoreCreatedAppLanding("),
            """
            the give-up path must re-arm the landing via restoreCreatedAppLanding, \
            not just report an error, or a switch that clears after the retry window \
            has nothing left to resume the hand-off with
            """)
        // `openCreatedAppSession` only has `appID`/`sessionID` in scope here, not
        // a `CreatedAppLanding` value — the re-arm must rebuild one from them.
        XCTAssertTrue(
            code.contains("LocalAppsStore.CreatedAppLanding(appID: appID, initSessionID: sessionID)"),
            "the restored landing must carry this attempt's own appID/sessionID, not a stale value")
        // …and re-arm AT MOST ONCE. `landCreatedAppIfReady`'s guard does not
        // test `projectSwitching`, so an unguarded restore is re-drained by
        // the `createdAppLanding` sink immediately, starts another 10s window
        // under the same unchanged refusal, dismisses the user's presented
        // route again, and repeats forever — the unbounded retry this bound
        // was added to remove, merely paced.
        XCTAssertTrue(
            code.contains("if reArmedCreatedAppLandingID != appID {"),
            """
            the re-arm must be latched per app, or restoring the landing turns the \
            bounded retry back into an endless one
            """)
        XCTAssertTrue(
            code.contains("reArmedCreatedAppLandingID = appID"),
            "the latch must be set, or the guard above never becomes false")
    }

    /// One declaration's source, comment lines stripped, sliced by brace depth.
    ///
    /// Comments are removed BEFORE counting so a `{` quoted in prose cannot
    /// desynchronise the depth — and so no assertion below can be satisfied by
    /// a sentence that merely describes the code.
    private func declarationSource(_ declaration: String, in source: String) throws -> String {
        let start = try XCTUnwrap(
            source.range(of: declaration), "\(declaration) not found — read the wrong file?")
        let tail = source[start.lowerBound...]
        let stripped = tail
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")
        var depth = 0
        var opened = false
        var index = stripped.startIndex
        while index < stripped.endIndex {
            switch stripped[index] {
            case "{":
                depth += 1
                opened = true
            case "}":
                depth -= 1
                if opened, depth == 0 { return String(stripped[..<stripped.index(after: index)]) }
            default:
                break
            }
            index = stripped.index(after: index)
        }
        XCTFail("unbalanced braces slicing \(declaration)")
        return stripped
    }

    /// A REFUSED scope switch must not cost the user the cover as well.
    ///
    /// Every local-app re-entry path used to be
    /// `navigation.closePresentedRoute()` followed by a `switchScope(...)`
    /// whose `Bool` was discarded. `switchScope` refuses SYNCHRONOUSLY for
    /// either of two reasons (`projectSwitching`, or
    /// `ConversationSessionMutationPolicy` holding the session), so a refused
    /// tap dismissed the library, did nothing, and said nothing — the user was
    /// left on the chat with no way back to the row they had just tapped.
    /// Android's draft-landing collector states the same contract in
    /// `RootScreen.kt`: "Close it only on success."
    func testLocalAppReEntryPathsCloseTheCoverOnlyAfterAnAcceptedSwitch() throws {
        let source = try clientSource("Sources/App/RootView.swift")
        let entryPoints = [
            "private func openAppSession(",
            "private func startNewAppSession(",
            "private func startDraftAppInterview(",
        ]
        // A gate that silently scans nothing reports "all clear", so say what
        // it actually covered.
        print("local-app re-entry paths audited: \(entryPoints.count)")
        for declaration in entryPoints {
            let body = try declarationSource(declaration, in: source)

            // Vacuity guards: this really is a slice of a re-entry path.
            XCTAssertTrue(
                body.contains("switchScope("),
                "\(declaration) must still be the site that requests the switch")
            let close = try XCTUnwrap(
                body.range(of: "navigation.closePresentedRoute()"),
                "\(declaration) must still dismiss the cover on the success path")

            let refusal = try XCTUnwrap(
                body.range(of: "guard switchScope("),
                "\(declaration) must BRANCH on switchScope's result; discarding it is what made a refused tap silent")
            XCTAssertTrue(
                body.contains("localAppsStore.reportScopeSwitchRefused("),
                "\(declaration)'s refusal path must report — routed through the store because while the cover is up LocalAppsRootView's alert is the only presenter that can show it")
            XCTAssertTrue(
                refusal.lowerBound < close.lowerBound,
                "\(declaration) must close the cover AFTER the switch is accepted; closing first strands the user outside the library on a refusal")
        }
    }

    /// Tapping a PIN-LESS draft card must re-arm the interview.
    ///
    /// A draft shell whose best-effort init-session mint failed has no session
    /// to resume, and the pin-less branch used to call `onNewAppSession` —
    /// `switchScope(..., startNew: true)` with no `initialPrompt`. That mints a
    /// silent anchor on EVERY tap and never re-arms the create interview, which
    /// is the entire point of the card. It has to send the same kickoff the
    /// create landing sends.
    func testAPinlessDraftCardTapCarriesTheCreateKickoff() throws {
        let rootView = try clientSource("Sources/App/RootView.swift")
        let interview = try declarationSource("private func startDraftAppInterview(", in: rootView)
        XCTAssertTrue(
            interview.contains("switchScope("),
            "vacuity guard: the sliced body must still request the switch")
        XCTAssertTrue(
            interview.contains("initialPrompt: LocalAppKickoff.message"),
            "the draft tap must ride the kickoff on the switch itself, so it is sent by the attempt that actually landed the session")
        XCTAssertTrue(
            interview.contains("startNew: true"),
            "there is no pin to resume, so the tap has to mint the session")

        // …and the library must actually ROUTE the pin-less branch here. The
        // fix is worthless if `open(_:)` still calls `onNewAppSession`.
        let library = try clientSource("Sources/LocalApps/LocalAppsLibraryView.swift")
        let open = try declarationSource("private func open(_ app: LocalAppSummary)", in: library)
        XCTAssertTrue(
            open.contains("onOpenAppSession(app.id, sessionID, .code)"),
            "vacuity guard: the pinned branch must still resume the pin")
        XCTAssertTrue(
            open.contains("onStartDraftInterview(app.id)"),
            "the pin-less branch must route to the kickoff-carrying path, not to onNewAppSession, which is 「新会话」 and deliberately carries no prompt")
        XCTAssertFalse(
            open.contains("onNewAppSession("),
            "onNewAppSession is the session catalog's 「新会话」; using it for a draft card is the defect this test pins")
    }

    /// Every `drawer.*` identifier the UI tests drive must have a producer.
    ///
    /// The drawer's Apps tab exposes creation and the focused app-session
    /// return action. The whole family of drawer identifiers had rotted
    /// out from under it: `drawer.tab.chats`/`drawer.tab.projects`/
    /// `drawer.tab.crons` (the rawValues are `chat`/`code`/`cron` and there is
    /// no projects tab), `drawer.scope.global`/`drawer.scope.project.<name>`
    /// (scopes are `drawer.workspace.<workspaceKey>` cards now),
    /// `drawer.apps.view-all`, `drawer.apps.row.<id>` and
    /// `drawer.shortcut.cron`. An id with no producer never resolves: a tap on
    /// it fails loudly, but an `XCTAssertFalse(...exists)` on it is VACUOUSLY
    /// true and pins nothing at all — two of them were sitting green.
    ///
    /// Textual on purpose. XCUITest cannot run in this bundle, so the check
    /// that survives is "the identifier the test names is one `Sources/`
    /// actually emits".
    ///
    /// That is necessary and NOT sufficient: a producer in `Sources/` still
    /// resolves to nothing at runtime if a container above it carries its own
    /// identifier. `testAccessibilityContainersCarryingAnIdentifierDeclareChildrenContain`
    /// below asserts that structural precondition; do not read this test as
    /// proof that an identifier is addressable.
    func testEveryDrawerIdentifierTheUITestsDriveHasASourceProducer() throws {
        let drawer = try clientSource("Sources/Drawer/Drawer.swift")
        let uiTests = try clientSource("UITests/LingxiCodeUITests.swift")

        // Tab ids are interpolated over `DrawerSection`, so enumerate the cases
        // from the type itself rather than trusting a prefix — a prefix would
        // readmit `drawer.tab.projects`, a tab that does not exist.
        var exact = Set(DrawerSection.allCases.map { "drawer.tab.\($0.rawValue)" })
        var prefixes: Set<String> = []
        let producers = try NSRegularExpression(
            pattern: #"\.accessibilityIdentifier\("(drawer[^"]*)"\)"#)
        let drawerRange = NSRange(drawer.startIndex ..< drawer.endIndex, in: drawer)
        for match in producers.matches(in: drawer, range: drawerRange) {
            guard let range = Range(match.range(at: 1), in: drawer) else { continue }
            let literal = String(drawer[range])
            if let interpolation = literal.range(of: #"\("#) {
                prefixes.insert(String(literal[..<interpolation.lowerBound]))
            } else {
                exact.insert(literal)
            }
        }
        prefixes.remove("drawer.tab.")

        XCTAssertTrue(
            exact.contains("drawer.apps.create") && exact.contains("drawer.apps.all"),
            """
            vacuity guard: the producer scan must have found the drawer's local-app \
            workspace entry points, or it is reading the wrong file
            """)
        XCTAssertTrue(
            prefixes.contains("drawer.workspace."),
            "vacuity guard: the interpolated workspace-card producer must have been found")

        let consumers = try NSRegularExpression(pattern: #""(drawer\.[A-Za-z0-9_.\-]*)"#)
        let code = uiTests
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")
        let codeRange = NSRange(code.startIndex ..< code.endIndex, in: code)
        var used: Set<String> = []
        for match in consumers.matches(in: code, range: codeRange) {
            guard let range = Range(match.range(at: 1), in: code) else { continue }
            used.insert(String(code[range]))
        }
        // A gate that silently scans nothing reports "all clear".
        print("drawer identifiers: \(exact.count) exact producers, \(prefixes.count) interpolated, \(used.count) driven by the UI tests")
        XCTAssertGreaterThanOrEqual(
            used.count, 8,
            "vacuity guard: the UI tests drive far more than a handful of drawer ids")

        let orphans = used
            .filter { id in !exact.contains(id) && !prefixes.contains(where: id.hasPrefix) }
            .sorted()
        XCTAssertEqual(
            orphans, [],
            """
            these identifiers are driven by LingxiCodeUITests and produced by nothing \
            in Sources/Drawer/Drawer.swift, so the assertions naming them can only \
            fail (a tap) or pass for free (an exists check)
            """)
    }

    private struct A11yContainer {
        let line: Int
        let identifier: String
        let encloses: [String]
        let declaresContain: Bool
    }

    /// Textual model of "is this `.accessibilityIdentifier` sitting on a
    /// container that encloses other identifiers, and does it declare
    /// `children: .contain`?".
    ///
    /// For each identifier line it walks back over the modifier chain (and the
    /// comments in it) to the view the chain is attached to. If that view ends
    /// in `}`, the matching `{` bounds the element; every identifier lexically
    /// inside is a descendant, and so is every identifier in a view member the
    /// span references by name (the drawer's `sessionRow` is reached that way,
    /// exactly as `AskUserQuestionCard`'s `actions` is).
    private func accessibilityContainers(in source: String) -> [A11yContainer] {
        let lines = source.components(separatedBy: "\n")
        // Blank out string literals so `{`/`}` inside copy cannot skew nesting.
        let braceLines: [String] = lines.map { line in
            var out = ""
            var inString = false
            var escaped = false
            for ch in line {
                if escaped { escaped = false; continue }
                if inString, ch == "\\" { escaped = true; continue }
                if ch == "\"" { inString.toggle(); continue }
                if !inString { out.append(ch) }
            }
            return out
        }
        let idLines = lines.indices.filter { lines[$0].contains(".accessibilityIdentifier(") }
        let idSet = Set(idLines)

        func identifierLiteral(_ line: String) -> String {
            guard let open = line.range(of: ".accessibilityIdentifier(\"") else { return "" }
            let rest = line[open.upperBound...]
            guard let close = rest.firstIndex(of: "\"") else { return String(rest) }
            return String(rest[..<close])
        }
        // Forward brace match: the last line of the block opened on `start`.
        func blockEnd(from start: Int) -> Int? {
            var depth = 0
            for n in start ..< braceLines.count {
                for ch in braceLines[n] {
                    if ch == "{" { depth += 1 } else if ch == "}" {
                        depth -= 1
                        if depth == 0 { return n }
                    }
                }
            }
            return nil
        }
        // Backward brace match: the line carrying the `{` that `end`'s `}` closes.
        func blockStart(from end: Int) -> Int? {
            var depth = 0
            for n in stride(from: end, through: 0, by: -1) {
                for ch in braceLines[n].reversed() {
                    if ch == "}" { depth += 1 } else if ch == "{" {
                        depth -= 1
                        if depth == 0 { return n }
                    }
                }
            }
            return nil
        }

        // name -> body span, for `func`/`var` members the containers reference.
        let memberPattern = try? NSRegularExpression(
            pattern: #"^\s*(?:private\s+|fileprivate\s+|public\s+)?(?:func|var)\s+(\w+)\b"#)
        var members: [(name: String, start: Int, end: Int)] = []
        for (i, brace) in braceLines.enumerated() where brace.contains("{") {
            let range = NSRange(brace.startIndex ..< brace.endIndex, in: brace)
            guard let match = memberPattern?.firstMatch(in: brace, range: range),
                  let nameRange = Range(match.range(at: 1), in: brace),
                  let end = blockEnd(from: i)
            else { continue }
            members.append((String(brace[nameRange]), i, end))
        }

        func descendants(from start: Int, to end: Int) -> [String] {
            var seen: Set<String> = []
            var found: Set<Int> = []
            var work = [(start, end)]
            while let (a, b) = work.popLast() {
                guard seen.insert("\(a)-\(b)").inserted else { continue }
                found.formUnion(idSet.filter { $0 > a && $0 < b })
                // A `} label: {` receiver matches to its own line: an empty span,
                // not a crash.
                let body = a + 1 < b ? braceLines[(a + 1) ..< b].joined(separator: "\n") : ""
                let bodyRange = NSRange(body.startIndex ..< body.endIndex, in: body)
                for member in members where !(member.start >= a && member.end <= b) {
                    guard let reference = try? NSRegularExpression(
                        pattern: #"(?<![\w.])"# + NSRegularExpression.escapedPattern(for: member.name) + #"(?![\w])"#)
                    else { continue }
                    if reference.firstMatch(in: body, range: bodyRange) != nil {
                        work.append((member.start, member.end))
                    }
                }
            }
            return found.sorted().map { identifierLiteral(lines[$0]) }
        }

        var containers: [A11yContainer] = []
        for i in idLines {
            var j = i - 1
            while j >= 0 {
                let trimmed = lines[j].trimmingCharacters(in: .whitespaces)
                if trimmed.isEmpty || trimmed.hasPrefix(".") || trimmed.hasPrefix("//") { j -= 1 } else { break }
            }
            guard j >= 0, lines[j].trimmingCharacters(in: .whitespaces).hasPrefix("}"),
                  let start = blockStart(from: j)
            else { continue }
            let enclosed = descendants(from: start, to: j)
            guard !enclosed.isEmpty else { continue }
            let chain = lines[(j + 1) ..< i].joined(separator: "\n")
            containers.append(A11yContainer(
                line: i + 1,
                identifier: identifierLiteral(lines[i]),
                encloses: enclosed,
                declaresContain: chain.contains(".accessibilityElement(children: .contain)")))
        }
        return containers
    }

    /// A producer in `Sources/` is NOT an element XCUITest can address.
    ///
    /// `.accessibilityIdentifier` applied to a SwiftUI **container** propagates
    /// down and REPLACES every descendant's identifier unless the container
    /// also declares `.accessibilityElement(children: .contain)`. Found
    /// empirically 2026-08-23 in `AskUserQuestionCard.swift`, where one card id
    /// swallowed all of `chat.ask.cancel`/`prev`/`next`/`submit`/`other.N`.
    ///
    /// The producer test above is textual by construction and can only prove
    /// the string EXISTS in `Sources/` — the wrong thing to prove for a
    /// never-wired hook. `drawer.workspace.new.*`, `drawer.workspace.pin.*`,
    /// `drawer.workspace.collapse.*` and every `drawer.session.*` row sit
    /// inside `conversationGroupCard`'s outer `VStack`, which carries
    /// `drawer.workspace.<key>`; with no `children: .contain` on that VStack
    /// `app.buttons["drawer.workspace.new.global"].tap()` resolved to nothing
    /// and the UI test failed on the tap. So assert the STRUCTURAL
    /// precondition: a container carrying an identifier while enclosing other
    /// identifiers must declare `children: .contain`.
    func testAccessibilityContainersCarryingAnIdentifierDeclareChildrenContain() throws {
        // `AskUserQuestionCard` is the recorded reference case and is already
        // remediated: scanning it proves the detector FINDS containers rather
        // than reporting a vacuous zero from a file it failed to parse.
        let subjects: [(path: String, mustEnclose: [String])] = [
            ("Sources/Drawer/Drawer.swift", [
                "drawer.workspace.new.\\(group.key)",
                "drawer.workspace.pin.\\(group.key)",
                "drawer.workspace.collapse.\\(group.key)",
                "drawer.session.\\(scope.workspaceKey).\\(row.id)",
            ]),
            ("Sources/Conversation/AskUserQuestionCard.swift", ["chat.ask.submit", "chat.ask.cancel"]),
        ]
        var totalContainers = 0
        for subject in subjects {
            let containers = accessibilityContainers(in: try clientSource(subject.path))
            totalContainers += containers.count
            // A gate that silently scans nothing reports "all clear".
            print("\(subject.path): \(containers.count) identifier-carrying container(s) — "
                + containers.map { "line \($0.line) \($0.identifier) encloses \($0.encloses)" }.joined(separator: "; "))
            XCTAssertFalse(
                containers.isEmpty,
                "vacuity guard: \(subject.path) has a known identifier-carrying container; finding none means the scan is reading nothing")
            let enclosedEverywhere = Set(containers.flatMap(\.encloses))
            for expected in subject.mustEnclose {
                XCTAssertTrue(
                    enclosedEverywhere.contains(expected),
                    "vacuity guard: \(expected) is known to sit inside an identifier-carrying container in \(subject.path)")
            }
            let clobbering = containers.filter { !$0.declaresContain }
            XCTAssertEqual(
                clobbering.map { "line \($0.line): \($0.identifier) clobbers \($0.encloses)" }, [],
                """
                a SwiftUI container's .accessibilityIdentifier OVERWRITES every \
                descendant's identifier; these containers enclose other identifiers \
                without declaring .accessibilityElement(children: .contain), so the \
                ids inside them exist in Sources/ but no XCUITest query can resolve them
                """)
        }
        XCTAssertGreaterThanOrEqual(totalContainers, 3, "vacuity guard: three container cases are known across these two files")
    }

    /// A drawer tap that both entered an app scope AND changed section mode
    /// used to fire `onModeChanged` (a `switchScope`) and `onSelectAppSession`
    /// (a SECOND `switchScope`) back to back. The first call sets
    /// `projectSwitching = true` synchronously, so the second was refused —
    /// silently, since the drawer discards `onSelectAppSession`'s return
    /// value. The fix folds both into ONE `switchScope` call.
    func testDrawerAppSessionTapCarriesModeInOneCallInsteadOfTwoRacingSwitches() throws {
        let drawerSource = try clientSource("Sources/Drawer/Drawer.swift")
        XCTAssertTrue(
            drawerSource.contains("let onSelectAppSession: (String, String, WorkspaceSessionMode?) -> Void"),
            "onSelectAppSession must carry the section's mode so both fold into one switch")

        guard let start = drawerSource.range(of: "private func selectSession("),
              let end = drawerSource.range(
                  of: "private func startNewConversation(",
                  range: start.upperBound ..< drawerSource.endIndex)
        else {
            return XCTFail("read the wrong file: selectSession not found")
        }
        let body = String(drawerSource[start.lowerBound ..< end.lowerBound])
        XCTAssertTrue(
            body.contains("onSelectAppSession(id, sessionID, section.sessionMode)"),
            "the local-app branch must pass the mode straight through, not call onModeChanged separately")
        // The local-app case must NOT be covered by the shared `onModeChanged`
        // call any more — it has its own mode parameter now.
        let localAppCaseRange = body.range(of: "case let .localApp(id):")
        XCTAssertNotNil(localAppCaseRange)
        if let localAppCaseRange {
            let localAppCase = String(body[localAppCaseRange.lowerBound...])
            XCTAssertFalse(
                localAppCase.contains("onModeChanged?("),
                "the local-app branch must not ALSO fire a separate mode-change switch")
        }

        let rootSource = try clientSource("Sources/App/RootView.swift")
        guard let bindingStart = rootSource.range(of: "onSelectAppSession: { appID, sessionID, mode in"),
              let bindingEnd = rootSource.range(
                  of: "onNewAppChat: { appID in",
                  range: bindingStart.upperBound ..< rootSource.endIndex)
        else {
            return XCTFail("read the wrong file: RootView's onSelectAppSession binding not found")
        }
        let binding = String(rootSource[bindingStart.lowerBound ..< bindingEnd.lowerBound])
        XCTAssertTrue(
            binding.contains("switchScope(to: .localApp(appID), mode: mode ?? activeMode, resumeSessionID: sessionID)"),
            "the binding must fold the drawer's mode into the SAME switchScope call, not switchMode then switchScope")
    }

    /// `pendingInitKickoff` used to have NO expiry at all: if none of
    /// `adoptEngineSession` / `clearUnavailableSession` / `rollbackFailedSession`
    /// ever fired for the scope it was armed for, the create-flow brief
    /// waited forever with nothing to expire it and no error ever shown —
    /// the Android twin of this is `RootScreen.kt`'s
    /// `SESSION_READY_TIMEOUT_MS`, which at least reports the drop.
    func testPendingInitKickoffHasAStopLossTimerThatSurfacesTheDroppedKickoff() throws {
        let source = try clientSource("Sources/App/RootView.swift")

        guard let armStart = source.range(of: "private func armPendingInitKickoffTimeout()"),
              let armEnd = source.range(
                  of: "private func clearPendingInitKickoffTimeout()",
                  range: armStart.upperBound ..< source.endIndex)
        else {
            return XCTFail("read the wrong file: armPendingInitKickoffTimeout not found")
        }
        let armBody = String(source[armStart.lowerBound ..< armEnd.lowerBound])
        XCTAssertTrue(armBody.contains("Task.sleep"), "the stop-loss must actually wait, not fire immediately")
        XCTAssertTrue(
            armBody.contains("localAppsStore.reportCreatedAppLandingExhausted()"),
            "a timed-out kickoff must surface the same unknown-result copy every other give-up path uses")

        // Every site that resolves the latch (fired, superseded, or dropped)
        // must also disarm the timer, or it fires later against whatever the
        // latch holds next.
        let pendingInitKickoffNilCount = source.components(separatedBy: "pendingInitKickoff = nil").count - 1
        let clearTimeoutCount = source.components(separatedBy: "clearPendingInitKickoffTimeout()").count - 1
        XCTAssertEqual(
            clearTimeoutCount, pendingInitKickoffNilCount,
            "every site that nils pendingInitKickoff must also disarm its stop-loss timer")
    }

    /// `beginAppIntegratedConversation` (the `lingxi://` new-conversation/ask
    /// deep-link action) used to mint with whatever `activeMode` happened to
    /// be, with no pin — unlike `switchScope`'s own Code pin for a scope
    /// switch. A Local App's own `LINGXI.md` contract needs the
    /// create-local-app skill and the `Workflow`/`LocalApp*`/`Write` tools,
    /// all stripped in Chat mode, so an app scope left in Chat (`switchMode`
    /// deliberately allows this) made this action mint an unsatisfiable
    /// conversation.
    func testBeginAppIntegratedConversationPinsALocalAppScopeOutOfChatModeBeforeMinting() throws {
        let source = try clientSource("Sources/App/RootView.swift")
        guard let start = source.range(of: "private func beginAppIntegratedConversation("),
              let end = source.range(
                  of: "private func openLocalAppFromDeepLink(",
                  range: start.upperBound ..< source.endIndex)
        else {
            return XCTFail("read the wrong file: beginAppIntegratedConversation not found")
        }
        // COMMENT LINES ARE STRIPPED. A plain `contains` over the raw slice
        // passes when the very line it pins has been commented OUT — measured:
        // commenting the refusal-clear below left this gate green.
        let body = String(source[start.lowerBound ..< end.lowerBound])
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")
        XCTAssertTrue(
            body.contains("activeScope.isLocalApp") && body.contains("activeMode != .code"),
            "must detect a Local App scope sitting outside Code mode before minting")
        XCTAssertTrue(
            body.contains("switchScope(to: activeScope, mode: .code, startNew: true)"),
            "the fix must route through switchScope's own mode pin, not mint directly in Chat")
        // `switchScope` refuses SYNCHRONOUSLY (`projectSwitching`, or the
        // mutation policy) before either latch-draining site runs, so a
        // refusal that leaves `pendingBeginActionDraft` armed lets the NEXT
        // unrelated `startNew` switch overwrite the composer it restores.
        XCTAssertTrue(
            body.contains("if !switched { pendingBeginActionDraft = nil }"),
            "a refused mode-pinning switch must not leave the draft armed for a later startNew")
    }

    /// `rollbackFailedSession` used to drop `pendingInitKickoff` on a failed
    /// init-session transition with nothing shown for it: the app record
    /// exists, but the interview brief that was supposed to fire into its
    /// session is gone and the screen says nothing about it.
    func testRollbackFailedSessionSurfacesTheDroppedKickoff() throws {
        let source = try clientSource("Sources/App/RootView.swift")
        guard let start = source.range(of: "private func rollbackFailedSession("),
              let end = source.range(
                  of: "private func toggleWorkspacePinned(",
                  range: start.upperBound ..< source.endIndex)
        else {
            return XCTFail("read the wrong file: rollbackFailedSession not found")
        }
        let body = String(source[start.lowerBound ..< end.lowerBound])
        XCTAssertTrue(
            body.contains("pendingInitKickoff = nil"),
            "the stale latch must still be dropped on a failed transition")
        // Through the store's own mutator, NOT `localAppsStore.errorMessage = …`:
        // the property is `private(set)` (LocalAppsStore.swift), so assigning it
        // from RootView.swift does not compile at all.
        XCTAssertTrue(
            body.contains("localAppsStore.reportCreatedAppLandingExhausted()"),
            """
            dropping the create kickoff on a failed session transition must not be \
            silent: the same copy the bounded-retry give-up path uses belongs here too
            """)
        XCTAssertFalse(
            body.contains("localAppsStore.errorMessage ="),
            "errorMessage is private(set); assigning it from this file does not compile")
    }

    /// A restored landing (see the assertion above) that is NOT drained on the
    /// spot — `landCreatedAppIfReady`'s guard also refuses while the turn is
    /// streaming or the widget-setup sheet is up — has to be re-checked by
    /// something later, and a switch refusal clearing is exactly that moment.
    ///
    /// Note what this sink is not: it is not what stops the re-arm looping.
    /// The guard does not test `projectSwitching` at all, so the
    /// `createdAppLanding` sink normally re-drains the restore immediately;
    /// the per-app latch asserted above is what makes that terminate.
    func testProjectSwitchingClearingRetriesARestoredCreatedAppLanding() throws {
        let source = try clientSource("Sources/App/RootView.swift")
        guard let range = source.range(of: ".onChange(of: projectSwitching) { _, switching in") else {
            return XCTFail("read the wrong file: the projectSwitching onChange sink was not found")
        }
        guard let closeBrace = source.range(of: "\n            }", range: range.upperBound ..< source.endIndex)
        else {
            return XCTFail("could not find the end of the projectSwitching onChange sink")
        }
        let body = String(source[range.upperBound ..< closeBrace.lowerBound])
        XCTAssertTrue(
            body.contains("landCreatedAppIfReady()"),
            """
            the projectSwitching sink must retry landCreatedAppIfReady() once switching \
            clears, or a landing restored by openCreatedAppSession's give-up path is \
            never picked back up
            """)
    }
}

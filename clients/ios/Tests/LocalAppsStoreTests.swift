import XCTest
import WebKit
@testable import LingxiCode

@MainActor
final class LocalAppsStoreTests: XCTestCase {
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
        XCTAssertEqual(unverified.accessibilityLabel, "local_apps_verification_published_unverified")
        XCTAssertEqual(LocalAppWorkflow.publishedUnverified.isPublished, true)

        let verified = LocalAppWorkflow.publishedVerified.statusBadge
        XCTAssertEqual(verified.systemImageName, "checkmark.seal.fill")
        XCTAssertEqual(verified.accessibilityLabel, "local_apps_verification_published_verified")
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
            XCTAssertEqual(store.errorMessage, "创建失败")

            let later = shellRecord(id: "later", name: "untitled")
            store.handle(event: .appEvent(event: .appCreated(record: later, requestId: second)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: later)))
            XCTAssertNil(
                store.createdAppLanding,
                "a create that already failed must not claim a later app")
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
                store.errorMessage, "创建失败：磁盘写入被拒绝",
                "with no cover mounted the message must survive on the store, "
                    + "which is the only thing a root-level presenter can read")
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
                store.errorMessage, "创建失败：引擎已断开",
                "the second failure must reach the presenter too")
        }

        func testCreateConfirmationSupersedesSameAppAndRejectsOldToken() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            store.handle(event: .appEvent(event: .createConfirmationRequested(
                request: createConfirmationRequest(requestId: "create-1", appId: "tracker", name: "Tracker")
            )))
            XCTAssertEqual(store.pendingCreateConfirmation?.requestID, "create-1")

            store.handle(event: .appEvent(event: .createConfirmationRequested(
                request: createConfirmationRequest(requestId: "create-2", appId: "tracker", name: "Tracker v2")
            )))

            XCTAssertEqual(store.pendingCreateConfirmation?.requestID, "create-2")
            try await waitUntil {
                submitted.contains { command in
                    guard case let .pluginCommand(command: .resolveCreateConfirmation(requestId, approved)) = command
                    else { return false }
                    return requestId == "create-1" && approved == false
                }
            }
        }

        func testResolvingCreateConfirmationApprovesCurrentAndAdvancesQueue() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            store.handle(event: .appEvent(event: .createConfirmationRequested(
                request: createConfirmationRequest(requestId: "create-1", appId: "tracker", name: "Tracker")
            )))
            store.handle(event: .appEvent(event: .createConfirmationRequested(
                request: createConfirmationRequest(requestId: "create-2", appId: "notes", name: "Notes")
            )))

            await store.resolvePendingCreateConfirmation(true)

            XCTAssertEqual(store.pendingCreateConfirmation?.requestID, "create-2")
            XCTAssertTrue(submitted.contains { command in
                guard case let .pluginCommand(command: .resolveCreateConfirmation(requestId, approved)) = command
                else { return false }
                return requestId == "create-1" && approved
            })
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

        /// The kickoff message resolves to real copy, in every locale the
        /// catalog carries.
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
            let shell = LocalAppsProtocolAdapter.app(
                shellRecord(id: "shell", name: "untitled"))

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

        /// Search matches what the card SHOWS.
        ///
        /// A query typed against the placeholder name must not surface a card
        /// whose visible title has nothing to do with it.
        func testSearchMatchesTheDraftTitleAndNotThePlaceholderName() {
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
            XCTAssertEqual(store.filteredApps.map(\.id), ["shell"])

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
            initSessionId: String? = nil
        ) -> AppRecordDto {
            AppRecordDto(
                id: id,
                name: name,
                brief: brief,
                gitEnabled: true,
                createdAtMs: 1,
                updatedAtMs: 2,
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
            initSessionId: String? = nil
        ) -> AppRecordDto {
            wireRecord(
                id: id, name: name, brief: "", scaffolded: false,
                initSessionId: initSessionId)
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

        private func createConfirmationRequest(
            requestId: String,
            appId: String,
            name: String
        ) -> LocalAppCreateConfirmationRequestDto {
            LocalAppCreateConfirmationRequestDto(
                requestId: requestId,
                appId: appId,
                name: name,
                brief: "Summarize local data",
                selectedTemplate: .init(
                    templateId: "template.dom",
                    surface: .dom,
                    summary: "DOM template"
                ),
                runtimeProfile: runtimeProfileOption(),
                reason: "Best match for the requested workflow",
                rejected: [.init(templateId: "template.canvas", reason: "Needs multiple panes")],
                initialTools: [toolSurface(name: "read_value", title: "Read Value")],
                requiredGates: [gateStatus(id: "runner", status: .pending, available: false)],
                receipt: receiptStatus(appId: appId)
            )
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
                pendingGates: pendingGates,
                receipt: receiptStatus(appId: appId)
            )
        }

        private func runtimeProfileOption() -> AppRuntimeProfileOptionDto {
            AppRuntimeProfileOptionDto(
                family: .reactDom,
                revision: 3,
                contractSha256: String(repeating: "r", count: 64),
                surface: .dom,
                corePackages: [.init(name: "next", version: "15.0.0")],
                cacheStatus: "cached",
                downloadStatus: "ready",
                available: true,
                reason: nil
            )
        }

        private func receiptStatus(appId: String) -> LocalAppReceiptStatusDto {
            LocalAppReceiptStatusDto(
                receiptId: "receipt-\(appId)",
                appId: appId,
                workflowRunId: "workflow-\(appId)",
                approvalContractSha256: String(repeating: "c", count: 64),
                candidateDigest: String(repeating: "d", count: 64),
                issuedAtMs: 1,
                expiresAtMs: 2,
                consumed: false,
                superseded: false
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
}

import XCTest
import WebKit
@testable import LingxiCode

@MainActor
final class LocalAppsStoreTests: XCTestCase {
    func testFilteringUsesLocalizedSearch() {
        let store = LocalAppsStore()
        #if canImport(engine_mobileFFI)
            store.handle(event: .appsChanged(apps: [
                app(id: "tracker", name: "订单跟踪", brief: "跟踪订单状态"),
                app(id: "metrics", name: "Metrics", brief: "查看运营指标"),
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
            "new TextEncoder().encode(JSON.stringify(out)).length",
        ] {
            XCTAssertTrue(source.contains(token), "missing payload budget: \(token)")
        }
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
            "window.__bridgeEnvelope = null; window.lingxi = { __resolve: value => window.__bridgeEnvelope = value };"
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
            store.handle(event: .appsChanged(apps: [app(id: "tracker", name: "Tracker")]))

            let submitted = await store.delete(appID: "tracker")
            XCTAssertTrue(submitted)
            XCTAssertEqual(pendingAtSubmission.count, 1, "the cleanup journal must precede the delete command")

            store.handle(event: .appsChanged(apps: [app(id: "tracker", name: "Tracker")]))
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
            XCTAssertEqual(summary.workflow, .ready)

            let draft = LocalAppsProtocolAdapter.app(
                appRecord(id: "draft-app", name: "Draft", workflowState: .draft)
            )
            XCTAssertEqual(draft.workflow, .draft)
            XCTAssertNil(draft.initSessionId)
        }

        /// The details snapshot's manifest is the only structured data-model
        /// description now — its collections must reach the store.
        func testAppDetailsMirrorsManifestCollections() {
            let store = LocalAppsStore()
            store.handle(event: .appEvent(event: .appDetailsChanged(details: AppDetailsDto(
                app: appRecord(id: "tracker", name: "Tracker"),
                manifest: AppManifestDto(
                    schemaVersion: 1,
                    runtimeApiVersion: nil,
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
                    deviceContext: nil
                ),
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
                app(id: "tracker", name: "Tracker", brief: "跟踪任务"),
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
                "window.__bridgeEnvelope = null; window.lingxi = { __resolve: value => window.__bridgeEnvelope = value };"
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
                app(id: "tracker", name: "Tracker", brief: "跟踪任务"),
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

    /// `LocalAppCreateView` refuses a whitespace-only brief.
    /// Drives the predicate directly rather than poking `view.brief`: that
    /// property is `@State`, so assigning to it on a bare struct outside a
    /// view hierarchy silently does nothing.
    func testTheCreateViewRefusesAWhitespaceOnlyBrief() {
        XCTAssertFalse(LocalAppCreateView.canSubmit(brief: "   \n  "))
        XCTAssertFalse(LocalAppCreateView.canSubmit(brief: ""))
    }

    func testTheCreateViewAcceptsANonEmptyBrief() {
        XCTAssertTrue(LocalAppCreateView.canSubmit(brief: "a shared grocery list for my household"))
        XCTAssertTrue(LocalAppCreateView.canSubmit(brief: "  记事本  "))
    }

    #if canImport(engine_mobileFFI)
        /// The create sheet creates the app OUTRIGHT, carrying the name and the
        /// surface the user confirmed.
        ///
        /// Both are fixed at creation — a surface is immutable once scaffolded
        /// and apps have no rename — so neither may be left to a value the user
        /// never saw, and neither may travel through the model.
        func testCreateSendsCreateAppWithTheConfirmedNameAndSurface() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let created = await store.createApp(
                brief: "一个打砖块游戏",
                name: "打砖块",
                surface: .canvas,
                gitEnabled: false,
                modelOverride: "anthropic/claude-sonnet-4-5")

            XCTAssertTrue(created)
            guard case let .createApp(name, origin, brief, gitEnabled, workflowModel,
                                      conversationId, surface) =
                try XCTUnwrap(submitted.first)
            else { return XCTFail("expected CreateApp, got \(submitted)") }
            XCTAssertEqual(name, "打砖块")
            XCTAssertEqual(origin, .library)
            XCTAssertEqual(brief, "一个打砖块游戏")
            XCTAssertFalse(gitEnabled)
            XCTAssertEqual(workflowModel, "anthropic/claude-sonnet-4-5")
            XCTAssertNil(
                conversationId,
                "a library create binds no conversation — none exists yet")
            XCTAssertEqual(
                surface, .canvas,
                "the user's confirmed surface, not the routed default")
        }

        /// `ProposeAppIdentity` is a REAL command, and its answer is routed back
        /// to the ONE sheet that asked.
        func testProposeIdentityAsksTheHostAndAdoptsTheAnswer() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            async let proposal = store.proposeIdentity(brief: "一个打砖块游戏")
            // Reply on the request id the store actually sent. Echoing a
            // fabricated one would let this pass against a store that never
            // correlates at all.
            var requestID: String?
            for _ in 0..<200 where requestID == nil {
                if case let .proposeAppIdentity(id, _) = submitted.first { requestID = id }
                try await Task.sleep(for: .milliseconds(5))
            }
            let id = try XCTUnwrap(requestID, "the store must send ProposeAppIdentity")
            guard case let .proposeAppIdentity(_, brief) = try XCTUnwrap(submitted.first)
            else { return XCTFail("expected ProposeAppIdentity, got \(submitted)") }
            XCTAssertEqual(brief, "一个打砖块游戏")
            store.handle(event: .appIdentityProposed(
                requestId: id, name: "打砖块", surface: .canvas))

            let answer = await proposal
            XCTAssertEqual(answer.name, "打砖块")
            XCTAssertEqual(answer.surface, .canvas)
        }

        /// An answer for a DIFFERENT request must not resolve this one.
        ///
        /// A sheet that was retyped and re-submitted has a stale proposal in
        /// flight; adopting it would name the app after the abandoned brief.
        /// Written so a store that ignored `request_id` entirely FAILS: the
        /// wrong-id answer is delivered first, and only the matching one may
        /// resolve the call.
        func testProposeIdentityIgnoresAnAnswerForAnotherRequest() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let resolved = Resolved()
            async let proposal: LocalAppsStore.AppIdentityProposal = {
                let answer = await store.proposeIdentity(brief: "一个记事本")
                await resolved.mark()
                return answer
            }()

            var requestID: String?
            for _ in 0..<200 where requestID == nil {
                if case let .proposeAppIdentity(id, _) = submitted.first { requestID = id }
                try await Task.sleep(for: .milliseconds(5))
            }
            let id = try XCTUnwrap(requestID)

            store.handle(event: .appIdentityProposed(
                requestId: "somebody-elses", name: "别的应用", surface: .canvas))
            try await Task.sleep(for: .milliseconds(50))
            let resolvedEarly = await resolved.value
            XCTAssertFalse(
                resolvedEarly,
                "an answer for another request must not resolve this call")

            store.handle(event: .appIdentityProposed(
                requestId: id, name: "记事本", surface: .dom))
            let answer = await proposal
            XCTAssertEqual(answer.name, "记事本")
            XCTAssertEqual(answer.surface, .dom)
        }

        /// Tiny actor so the test can observe "has it resolved yet" without a
        /// data race on a plain `var`.
        private actor Resolved {
            private(set) var value = false
            func mark() { value = true }
        }

        /// An engine that never answers must not wedge the sheet: the fields
        /// are editable, so the derived default is a usable starting point.
        func testProposeIdentityFallsBackWhenTheEngineIsNotConnected() async throws {
            let store = LocalAppsStore()
            // Deliberately NOT configured: `send` fails closed.

            let answer = await store.proposeIdentity(brief: "  一个记事本  ")

            XCTAssertEqual(
                answer.name, "一个记事本",
                "the same first-24-characters derivation AppService applies")
            XCTAssertEqual(answer.surface, .dom)
        }

        /// The fallback truncates by CHARACTERS. A byte cut would split a CJK
        /// codepoint and produce invalid UTF-8.
        func testFallbackNameTruncatesByCharacters() {
            let long = String(repeating: "记", count: 40)
            XCTAssertEqual(
                LocalAppsStore.fallbackName(brief: long).count, 24)
        }

        /// `AppCreated` names the record the engine just committed.
        ///
        /// The previous version inferred it by diffing the catalog against a
        /// pre-create snapshot — an inference the deferred flow invalidates,
        /// because minutes pass and the user can create something else.
        func testAppCreatedLandsTheNewAppAndArmsTheWidget() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createApp(brief: "一个带桌面入口的记事本", name: "", surface: .dom, addWidget: true)
            let record = app(id: "notes", name: "记事本", brief: "一个带桌面入口的记事本")
            store.handle(event: .appsChanged(apps: [record]))
            store.handle(event: .appEvent(event: .appCreated(record: record)))

            XCTAssertEqual(store.consumeCreatedAppID(), "notes")
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "notes")
        }

        /// The hand-off into the app's own conversation arms on `AppCreated`
        /// and is completed by the pin that arrives afterwards.
        ///
        /// `AppCreated` is emitted inside the create transaction and the init
        /// session is minted AFTER it, so the record on the create event never
        /// carries one. Gating the landing on it made the hand-off unreachable
        /// — the agent stayed in a conversation rooted outside the app and
        /// every build failed on the workspace.
        func testTheLandingArmsOnAppCreatedAndTakesThePinFromTheRecordUpdate()
            async throws
        {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createApp(
                brief: "一个记事本", name: "记事本", surface: .dom,
                modelOverride: "anthropic/claude-sonnet-4-5")
            let created = app(id: "notes", name: "记事本", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [created]))
            store.handle(event: .appEvent(event: .appCreated(record: created)))

            XCTAssertNil(
                store.createdAppLanding,
                "`AppCreated` never carries the pin, so publishing the landing "
                    + "here would make RootView start a FRESH conversation and "
                    + "orphan the session the engine is about to mint")

            store.handle(event: .appEvent(event: .appRecordChanged(
                record: appRecord(
                    id: "notes", name: "记事本", brief: "简介",
                    initSessionId: "session-9"))))

            let landing = try XCTUnwrap(store.createdAppLanding)
            XCTAssertEqual(landing.appID, "notes")
            XCTAssertEqual(landing.initSessionID, "session-9")
            XCTAssertEqual(
                landing.modelOverride, "anthropic/claude-sonnet-4-5",
                "the sheet's model choice rides the landing — the record does "
                    + "not echo it back")
            XCTAssertEqual(store.consumeCreatedAppLanding()?.appID, "notes")
            XCTAssertNil(store.consumeCreatedAppLanding(), "the landing is one-shot")
        }

        /// A create whose best-effort init-session mint FAILED must still land.
        ///
        /// The engine announces the record either way. Landing on a fresh
        /// conversation is still correct: the SCOPE, not the session, is what
        /// roots the agent in the app workspace.
        func testTheLandingStillFiresWhenNoInitSessionWasPinned() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createApp(brief: "一个记事本", name: "记事本", surface: .dom)
            let created = app(id: "notes", name: "记事本", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [created]))
            store.handle(event: .appEvent(event: .appCreated(record: created)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: created)))

            let landing = try XCTUnwrap(
                store.createdAppLanding,
                "a failed pin must not strand the hand-off")
            XCTAssertEqual(landing.appID, "notes")
            XCTAssertNil(landing.initSessionID)
        }

        /// A create committed by an AGENT in another conversation must not be
        /// claimed by this sheet.
        ///
        /// `AppCreated` carries no correlator back to the client that asked, so
        /// an unkeyed claim opens whichever app committed first — the user
        /// lands in someone else's app and their own create never lands at all.
        func testAnotherConversationsCreateIsNotClaimedBySheet() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createApp(
                brief: "我的记事本", name: "记事本", surface: .dom, addWidget: true)

            // The agent's app commits first.
            let theirs = app(id: "theirs", name: "别人的", brief: "代理自己的应用")
            store.handle(event: .appsChanged(apps: [theirs]))
            store.handle(event: .appEvent(event: .appCreated(record: theirs)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: theirs)))

            XCTAssertNil(store.createdAppLanding, "not this sheet's app")
            XCTAssertNil(store.consumeCreatedAppID())
            XCTAssertNil(store.pendingWidgetSetup, "the widget belongs to MY create")

            // Mine commits afterwards and is still claimable.
            let mine = app(id: "mine", name: "记事本", brief: "我的记事本")
            store.handle(event: .appsChanged(apps: [theirs, mine]))
            store.handle(event: .appEvent(event: .appCreated(record: mine)))
            store.handle(event: .appEvent(event: .appRecordChanged(record: mine)))

            XCTAssertEqual(store.createdAppLanding?.appID, "mine")
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "mine")
        }

        /// An app that simply APPEARS is not the one this session asked for.
        func testANewAppWithoutAppCreatedIsNotClaimed() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createApp(brief: "一个记事本", name: "", surface: .dom, addWidget: true)
            store.handle(event: .appsChanged(apps: [app(id: "someone-elses", name: "别人的")]))

            XCTAssertNil(
                store.consumeCreatedAppID(),
                "a catalog row alone must not be claimed")
            XCTAssertNil(store.pendingWidgetSetup)
        }

        /// A second create is refused while one is in flight, so a double tap
        /// cannot create two apps.
        func testASecondCreateWhileOneIsInFlightIsRefused() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let first = await store.createApp(brief: "第一个", name: "", surface: .dom)
            let second = await store.createApp(brief: "第二个", name: "", surface: .dom)

            XCTAssertTrue(first)
            XCTAssertFalse(second, "only one create may be in flight at a time")
            XCTAssertEqual(
                submitted.count, 1,
                "the refused create must not reach the engine")
        }

        /// A GLOBAL failure disarms the claim; a per-app failure belongs to some
        /// other app and must leave it alone.
        func testOnlyAGlobalFailureDisarmsTheArmedCreate() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            _ = await store.createApp(brief: "一个记事本", name: "", surface: .dom, addWidget: true)
            store.handle(event: .appOperationFailed(
                appId: "another-app", code: .workflowStateInvalid, message: "别的应用失败了"))
            let mine = app(id: "mine", name: "我的", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [mine]))
            store.handle(event: .appEvent(event: .appCreated(record: mine)))
            XCTAssertEqual(store.consumeCreatedAppID(), "mine")

            _ = await store.createApp(brief: "第二个", name: "", surface: .dom, addWidget: true)
            store.handle(event: .appOperationFailed(
                appId: nil, code: .workflowStateInvalid, message: "创建失败"))
            let later = app(id: "later", name: "后来的", brief: "第二个")
            store.handle(event: .appsChanged(apps: [later]))
            store.handle(event: .appEvent(event: .appCreated(record: later)))
            XCTAssertNil(
                store.consumeCreatedAppID(),
                "a disarmed create must not claim a later app")
        }

        /// Restored with the new trigger: the widget must not arm for a create
        /// that never asked for one.
        func testCreateWithoutWidgetDoesNotArmSetupGuide() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            let armed = await store.createApp(brief: "一个记事本", name: "", surface: .dom)
            XCTAssertTrue(armed)
            let record = app(id: "notes", name: "记事本", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [record]))
            store.handle(event: .appEvent(event: .appCreated(record: record)))

            XCTAssertNil(store.pendingWidgetSetup)
        }

        /// A missing App Group is a CONFIGURATION fact, not a create failure:
        /// the setup guide still arms so the user can be walked through it.
        func testAddWidgetStillArmsSetupWhenTheAppGroupIsMissing() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }
            store.widgetSnapshotPublisher = { _ in
                LocalAppWidgetSnapshotStore.SnapshotError.containerUnavailable
            }

            let armed = await store.createApp(brief: "一个记事本", name: "", surface: .dom, addWidget: true)
            XCTAssertTrue(armed)
            let record = app(id: "notes", name: "记事本", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [record]))
            store.handle(event: .appEvent(event: .appCreated(record: record)))

            XCTAssertNil(store.errorMessage)
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "notes")
        }

        /// A REAL write failure is different from a missing container and must
        /// still reach the user.
        func testAddWidgetSurfacesARealSnapshotWriteFailure() async throws {
            struct DiskFull: Error {}
            let store = LocalAppsStore()
            store.configure { _ in }
            store.widgetSnapshotPublisher = { _ in DiskFull() }

            let armed = await store.createApp(brief: "一个记事本", name: "", surface: .dom, addWidget: true)
            XCTAssertTrue(armed)
            let record = app(id: "notes", name: "记事本", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [record]))
            store.handle(event: .appEvent(event: .appCreated(record: record)))

            XCTAssertEqual(
                store.errorMessage,
                String(localized: "local_apps_error_widget_snapshot")
            )
            XCTAssertEqual(store.pendingWidgetSetup?.appID, "notes")
        }

        /// A missing App Group must not be reported as a create error.
        func testCreateDoesNotSurfaceAMissingAppGroupAsAnError() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }
            store.widgetSnapshotPublisher = { _ in
                LocalAppWidgetSnapshotStore.SnapshotError.containerUnavailable
            }

            let armed = await store.createApp(brief: "一个记事本", name: "", surface: .dom)
            XCTAssertTrue(armed)
            let record = app(id: "notes", name: "记事本", brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [record]))
            store.handle(event: .appEvent(event: .appCreated(record: record)))

            XCTAssertNil(store.errorMessage)
            XCTAssertEqual(store.consumeCreatedAppID(), "notes")
        }

        private func app(
            id: String,
            name: String,
            brief: String = "简介"
        ) -> AppRecordDto {
            appRecord(id: id, name: name, brief: brief)
        }
    #endif
}

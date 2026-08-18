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

    func testInjectedCSPDisablesWorkersWithoutBlockingLocalMedia() {
        let source = LocalAppWebViewRepresentable.bridgeSource
        XCTAssertTrue(source.contains("worker-src 'none'"))
        XCTAssertTrue(source.contains("media-src 'self' data: blob:"))
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
        func testCreateAppSubmitsSelectedWorkflowModelAsStructuredData() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            let didSubmit = await store.createApp(
                brief: "一个记事本",
                modelOverride: "  deepseek/deepseek-v4-flash  "
            )
            XCTAssertTrue(didSubmit)

            let command = try XCTUnwrap(submitted.last)
            guard case let .createApp(
                name,
                origin,
                brief,
                gitEnabled,
                workflowModel,
                conversationId
            ) = command else {
                return XCTFail("expected CreateApp, got \(command)")
            }
            XCTAssertEqual(name, "")
            XCTAssertEqual(origin, .library)
            XCTAssertEqual(brief, "一个记事本")
            XCTAssertTrue(gitEnabled)
            XCTAssertEqual(workflowModel, "deepseek/deepseek-v4-flash")
            XCTAssertNil(conversationId)
        }

        func testCreateAppWithWidgetArmsThePostCreationSetupGuide() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            let submitted = await store.createApp(brief: "一个记事本", addWidget: true)
            XCTAssertTrue(submitted)
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-uuid-1")
            ]))

            let setup = try XCTUnwrap(store.pendingWidgetSetup)
            XCTAssertEqual(setup.appID, "notes")
            XCTAssertEqual(setup.appName, "记事本")

            store.completeWidgetSetup()
            XCTAssertNil(store.pendingWidgetSetup)
        }

        func testCreateAppWithoutWidgetDoesNotArmSetupGuide() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            let submitted = await store.createApp(brief: "一个记事本")
            XCTAssertTrue(submitted)
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-uuid-1")
            ]))

            XCTAssertNil(store.pendingWidgetSetup)
        }

        /// v3: creation should land in the INIT CHAT, and the engine
        /// announces twice — first the bare record, then the
        /// `init_session_id` pin. The claim must wait for the pin (second
        /// announce) and fire `createdAppSession` exactly once; the details
        /// fallback (`createdAppID`) must stay quiet on this path.
        func testCreateClaimWaitsForTheInitSessionPin() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }
            _ = await store.createApp(brief: "一个记事本")
            // Announce #1: record only, no pin yet.
            store.handle(event: .appsChanged(apps: [app(id: "notes", name: "记事本")]))
            XCTAssertNil(store.createdAppSession, "must wait for the pin")
            XCTAssertNil(store.createdAppID)
            // Announce #2: the pin arrives.
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-uuid-1")
            ]))
            let created = try XCTUnwrap(store.consumeCreatedAppSession())
            XCTAssertEqual(created.appID, "notes")
            XCTAssertEqual(created.initSessionID, "init-uuid-1")
            XCTAssertEqual(created.brief, "简介", "the kickoff carries the user's brief")
            XCTAssertNil(store.consumeCreatedAppSession(), "consume-once")
            XCTAssertNil(store.consumeCreatedAppID(), "no details fallback when the pin arrived")
        }

        /// The fast path: a single announce already carrying the pin fires
        /// the init-chat signal immediately.
        func testCreateClaimFiresImmediatelyWhenThePinIsInTheFirstAnnounce() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }
            _ = await store.createApp(brief: "一个记事本")
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-uuid-1")
            ]))
            let created = try XCTUnwrap(store.consumeCreatedAppSession())
            XCTAssertEqual(created.initSessionID, "init-uuid-1")
        }

        /// A create that fails ENGINE-SIDE must disarm the claim.
        ///
        /// `createApp`'s own `if !succeeded { pendingCreation = nil }` cannot
        /// cover this: `send(_:)` reports success the moment the FFI submit does
        /// not throw, so an engine-side rejection arrives later and
        /// asynchronously, as `AppOperationFailed`. The claim matches "an app id
        /// absent from the pre-create snapshot", so leaving it armed makes the
        /// NEXT app to appear — including one the assistant creates through the
        /// MCP tool minutes later — look like the user's pending creation and
        /// yank them out of their conversation into its init chat.
        func testAnEngineSideCreateFailureDisarmsTheCreateClaim() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            // The submit itself succeeds — the engine rejects afterwards.
            do { let submitted = await store.createApp(brief: "一个记事本"); XCTAssertTrue(submitted) }
            store.handle(event: .appOperationFailed(
                appId: nil, code: .workflowStateInvalid, message: "创建失败"))
            XCTAssertEqual(store.errorMessage, "创建失败")

            // Minutes later the assistant creates an unrelated app through MCP.
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "assistant-app", name: "助手的应用", initSessionId: "init-uuid-9")
            ]))

            XCTAssertNil(
                store.consumeCreatedAppSession(),
                "a failed create must not claim the next app that appears")
            XCTAssertNil(
                store.consumeCreatedAppID(),
                "nor may the details-page fallback claim it")
        }

        func testAnEngineSideCreateFailureDoesNotArmWidgetSetup() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            let submitted = await store.createApp(brief: "一个记事本", addWidget: true)
            XCTAssertTrue(submitted)
            store.handle(event: .appOperationFailed(
                appId: nil, code: .workflowStateInvalid, message: "创建失败"))

            store.handle(event: .appsChanged(apps: [
                appRecord(id: "assistant-app", name: "助手的应用", initSessionId: "init-uuid-9")
            ]))

            XCTAssertNil(store.pendingWidgetSetup)
            XCTAssertNil(store.consumeCreatedAppSession())
            XCTAssertNil(store.consumeCreatedAppID())
        }

        func testAPerAppOperationFailureDoesNotDropTheInitPinWait() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            let submitted = await store.createApp(brief: "一个记事本")
            XCTAssertTrue(submitted)
            store.handle(event: .appsChanged(apps: [appRecord(id: "notes", name: "记事本")]))
            XCTAssertNil(store.createdAppSession)

            store.handle(event: .appOperationFailed(
                appId: "notes", code: .notFound, message: "详情加载失败"))

            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-uuid-1")
            ]))

            let created = try XCTUnwrap(
                store.consumeCreatedAppSession(),
                "a later per-app failure must not cancel an already-announced create")
            XCTAssertEqual(created.appID, "notes")
            XCTAssertEqual(created.initSessionID, "init-uuid-1")
        }

        /// The pin wait is a SET, not one slot. Two creates inside the 3s
        /// fallback window each arm their own landing; a single slot would let
        /// the second overwrite the first, stranding it with no landing at all
        /// — neither the init chat nor the details fallback.
        func testTwoCreatesInsideTheFallbackWindowBothLand() async throws {
            let store = LocalAppsStore()
            store.configure { _ in }

            // Create #1 announces its record with no pin yet → armed.
            do { let submitted = await store.createApp(brief: "记事本"); XCTAssertTrue(submitted) }
            store.handle(event: .appsChanged(apps: [appRecord(id: "notes", name: "记事本")]))
            XCTAssertNil(store.createdAppSession)

            // Create #2 lands inside #1's window and announces its own record.
            do { let submitted = await store.createApp(brief: "清单"); XCTAssertTrue(submitted) }
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本"),
                appRecord(id: "list", name: "清单"),
            ]))
            XCTAssertNil(store.createdAppSession)

            // #1's pin arrives first — it must still be armed.
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-notes"),
                appRecord(id: "list", name: "清单"),
            ]))
            let first = try XCTUnwrap(
                store.consumeCreatedAppSession(),
                "the FIRST create must not be stranded by the second")
            XCTAssertEqual(first.appID, "notes")
            XCTAssertEqual(first.initSessionID, "init-notes")

            // …and #2 still lands on its own announce.
            store.handle(event: .appsChanged(apps: [
                appRecord(id: "notes", name: "记事本", initSessionId: "init-notes"),
                appRecord(id: "list", name: "清单", initSessionId: "init-list"),
            ]))
            let second = try XCTUnwrap(store.consumeCreatedAppSession())
            XCTAssertEqual(second.appID, "list")
            XCTAssertEqual(second.initSessionID, "init-list")
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

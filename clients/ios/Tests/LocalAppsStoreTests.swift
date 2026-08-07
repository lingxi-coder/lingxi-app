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

    #if canImport(engine_mobileFFI)
        func testQuestionnaireEventReplacesTheStoredSteps() {
            let store = LocalAppsStore()
            store.handle(event: .appEvent(event: .appQuestionnaireChanged(
                appId: "a",
                revision: 1,
                steps: [oneStepDTO()]
            )))

            XCTAssertEqual(store.questionnaires["a"]?.count, 1)
            XCTAssertEqual(store.questionnaires["a"]?.first?.fields.first?.allowsDefer, true)
        }

        func testPlanEventStoresAndClears() {
            let store = LocalAppsStore()
            store.handle(event: .appEvent(event: .appPlanChanged(appId: "a", revision: 2, plan: onePlanDTO())))
            XCTAssertEqual(store.plans["a"]?.summary, "记事本")

            store.handle(event: .appEvent(event: .appPlanChanged(appId: "a", revision: 3, plan: nil)))
            XCTAssertNil(store.plans["a"], "an answer edit voids the plan on the client too")
        }

        /// `.deferred` ("let the model decide") must survive the trip through
        /// the REAL edit path — `edit()` -> `flushNextEdit` ->
        /// `LocalAppsProtocolAdapter.designValue(_:fieldType:)` — for a field
        /// type other than `.shortText`. `designValue(_:fieldType:)` checks
        /// `.deferred` before ever consulting `fieldType`, so a `.shortText`
        /// field could not have distinguished "fieldType is ignored" from
        /// "fieldType happens to be right"; `.multipleChoice` here rules that
        /// out and additionally exercises the non-debounced edit path (see
        /// `edit(field:value:appID:)`: only `.shortText`/`.longText` debounce).
        func testDeferredAnswerRoundTripsThroughTheRealEditPathForANonShortTextField() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 5))
            store.edit(
                field: designField(id: "tone", type: .multipleChoice),
                value: .deferred,
                appID: "tracker"
            )
            try await waitUntil("the deferred draft patch") { draftPatches(submitted).count == 1 }

            guard case let .set(fieldId, value) = draftPatches(submitted).first?.patch.ops.first else {
                return XCTFail("Expected a set op")
            }
            XCTAssertEqual(fieldId, "tone")
            XCTAssertEqual(value, .deferred)
        }
    #endif

    #if canImport(engine_mobileFFI)
        func testDesignerEventsPreserveQueuedStateAndSuggestionDiff() {
            let store = LocalAppsStore()
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 3))
            store.handle(event: .appDesignDraftChanged(
                appId: "tracker",
                revision: 4,
                fields: ["name": .shortText(value: "Orders")]
            ))
            store.handle(event: .appDesignSuggestionAvailable(
                appId: "tracker",
                suggestionId: "suggestion-1",
                basedOnRevision: 4,
                patch: AppDesignPatchDto(
                    ops: [.set(fieldId: "name", value: .shortText(value: "Sales Orders"))],
                    note: "Make the entity clearer"
                )
            ))

            XCTAssertEqual(store.designers["tracker"]?.revision, 4)
            XCTAssertEqual(store.designers["tracker"]?.interactionID, "gate-1")
            XCTAssertEqual(store.suggestions["tracker"]?.changes.first?.oldValue, .text("Orders"))
            XCTAssertEqual(store.suggestions["tracker"]?.changes.first?.newValue, .text("Sales Orders"))
        }

        /// Replaces the deleted `testAppEventInstallsDynamicTemplateContract`
        /// (local-apps#questionnaire, Task 13: the static template catalog it
        /// exercised no longer exists) with equivalent coverage of the LLM-
        /// authored questionnaire event: a `dataFieldList` default value and
        /// the new `allowsCustom`/`allowsDefer` flags must all survive the
        /// DTO → `LocalAppDesignStep` mapping.
        func testQuestionnaireEventMapsFieldsIncludingDataFieldListDefaults() {
            let store = LocalAppsStore()
            let fields = [
                AppDesignFieldDto(
                    id: "entity_fields",
                    label: "字段",
                    description: "由 Rust 下发",
                    fieldType: .dataFieldList,
                    required: true,
                    allowsCustom: false,
                    allowsDefer: true,
                    defaultValue: .dataFieldList(value: [
                        AppDataFieldDto(
                            id: "title",
                            label: "标题",
                            fieldType: .text,
                            required: true,
                            options: []
                        ),
                    ]),
                    options: []
                ),
            ]
            let steps = (0 ..< 5).map { index in
                AppDesignStepDto(
                    id: "step-\(index)",
                    order: UInt32(index),
                    title: "Step \(index)",
                    description: nil,
                    fields: index == 2 ? fields : []
                )
            }
            store.handle(event: .appEvent(event: .appQuestionnaireChanged(
                appId: "tracker",
                revision: 3,
                steps: steps
            )))

            XCTAssertEqual(store.questionnaires["tracker"]?.count, 5)
            XCTAssertEqual(store.questionnaires["tracker"]?[2].fields.first?.type, .dataFieldList)
            XCTAssertEqual(store.questionnaires["tracker"]?[2].fields.first?.allowsDefer, true)
            XCTAssertEqual(store.questionnaires["tracker"]?[2].fields.first?.allowsCustom, false)
            XCTAssertEqual(
                store.questionnaires["tracker"]?[2].fields.first?.defaultValue,
                .dataFields([
                    LocalAppDataField(
                        id: "title",
                        label: "标题",
                        fieldType: .text,
                        required: true,
                        options: []
                    ),
                ])
            )
        }

        /// `suspension_reason` has no producer: `lower_runtime_details`
        /// (local_apps_bridge.rs) hardcodes `None` because the core
        /// `AppRuntimeRecord` carries no non-user-initiated stop reason. The
        /// suspended-runtime label is therefore unreachable in this phase and
        /// asserting it only asserted the hand-built DTO. The loopback URL is
        /// real, so that is what this test pins.
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

        func testPreviewReadyKeepsTheLiveLoopbackURL() {
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
            store.handle(event: .appPreviewReady(
                appId: "tracker",
                interactionId: "gate-2",
                revision: 7,
                url: nil
            ))

            XCTAssertEqual(store.runtimes["tracker"]?.url?.absoluteString, "http://127.0.0.1:43123")
            XCTAssertEqual(store.previews["tracker"]?.url?.absoluteString, "http://127.0.0.1:43123")
        }

        // review NEW-1: `AppService::resync_pending_gates` re-announces the
        // pending gate of EVERY app at engine bootstrap (service.rs), not
        // just a gate that just opened this session. `PreviewReady` must not
        // unconditionally arm cross-screen navigation, or a relaunch with a
        // stale `awaiting_preview_confirmation` app hijacks the screen into
        // the local-apps cover for an app the user never touched.
        func testPreviewReadyDoesNotHijackNavigationOnALoadTimeReannouncement() {
            let store = LocalAppsStore()
            // No `appGenerationProgress`/`appGenerationJobChanged` preceded
            // this — exactly what a bootstrap resync looks like: the gate
            // announcement arrives cold, with no live job in this process.
            store.handle(event: .appPreviewReady(
                appId: "tracker",
                interactionId: "gate-2",
                revision: 7,
                url: nil
            ))

            XCTAssertNil(store.requestedPresentationAppID)
            XCTAssertFalse(store.consumePendingPreviewRouteAppID(appID: "tracker"))
        }

        // The F2 fix this must not regress: a generation that actually ran
        // this session still routes the user to the gate it just opened.
        func testPreviewReadyStillArmsNavigationForALiveSessionGeneration() {
            let store = LocalAppsStore()
            store.handle(event: .appGenerationProgress(
                appId: "tracker",
                stage: "scaffold",
                percent: 60,
                detail: nil
            ))
            store.handle(event: .appPreviewReady(
                appId: "tracker",
                interactionId: "gate-2",
                revision: 7,
                url: nil
            ))

            XCTAssertEqual(store.requestedPresentationAppID, "tracker")
            // Consumed exactly once — a cold cover's `.task` reads this to
            // land directly on `.preview` instead of `.details`; a second
            // read (e.g. a re-render) must not resurrect it.
            XCTAssertTrue(store.consumePendingPreviewRouteAppID(appID: "tracker"))
            XCTAssertFalse(store.consumePendingPreviewRouteAppID(appID: "tracker"))
        }

        // review NEW-1, round 2: gating on `generationProgress` (round 1's
        // fix) was still wrong — `appDetailsChanged` also populates that map
        // whenever it mirrors a snapshot's `generationJob`, which happens
        // just from opening an app's DETAIL screen (`LocalAppDetailView.task`
        // -> `getDetails`), no live run required. An app parked at
        // `awaiting_preview_confirmation` keeps its durable job forever
        // (`AppService::load_jobs`), so `handle_get_app_details` attaches it
        // on every fetch. This pins that merely viewing the details of a
        // long-parked app must not, on a LATER gate re-announcement, make it
        // look like a live generation.
        func testAPersistedJobSeenOnlyViaAppDetailsDoesNotArmNavigation() {
            let store = LocalAppsStore()
            store.handle(event: .appEvent(event: .appDetailsChanged(details: AppDetailsDto(
                app: app(id: "tracker", name: "Tracker"),
                designRevision: 1,
                designFields: [],
                questionnaire: [],
                plan: nil,
                manifest: nil,
                runtime: AppRuntimeDetailsDto(
                    state: .stopped,
                    mode: .nextProduction,
                    loopbackUrl: nil,
                    suspensionReason: nil,
                    recoveryState: .recovered,
                    lastError: nil
                ),
                generationJob: AppGenerationJobDto(
                    id: "job-1",
                    appId: "tracker",
                    revision: 1,
                    continuationSeq: 1,
                    state: .awaitingApproval,
                    percent: nil,
                    detail: nil,
                    logRel: nil,
                    updatedAtMs: 1
                ),
                checkpoints: []
            ))))
            // Sanity: the details snapshot really did mirror the job into
            // the map the round-1 fix (wrongly) gated on — otherwise this
            // test would pass for the wrong reason.
            XCTAssertNotNil(
                store.generationProgress["tracker"],
                "sanity: appDetailsChanged mirrors a persisted job into generationProgress"
            )

            store.handle(event: .appPreviewReady(
                appId: "tracker",
                interactionId: "gate-1",
                revision: 1,
                url: nil
            ))

            XCTAssertNil(store.requestedPresentationAppID)
            XCTAssertFalse(store.consumePendingPreviewRouteAppID(appID: "tracker"))
        }

        // review NEW-1, round 2: the within-session variant. A generation
        // that really did run this session correctly arms navigation once
        // (regression guard above) — but if the SAME still-pending gate is
        // re-announced a second time later in the same process (a project or
        // provider switch re-wires the engine source and re-runs
        // `resync_pending_gates` without recreating this store — `RootView`'s
        // `@State private var localAppsStore` is not reset by either), the
        // second announcement must not re-arm just because the app WAS live
        // earlier this session.
        func testASecondReannouncementInTheSameSessionDoesNotReArmAfterTheFirstConsumedIt() {
            let store = LocalAppsStore()
            store.handle(event: .appGenerationProgress(
                appId: "tracker",
                stage: "scaffold",
                percent: 60,
                detail: nil
            ))
            store.handle(event: .appPreviewReady(
                appId: "tracker",
                interactionId: "gate-1",
                revision: 1,
                url: nil
            ))
            XCTAssertEqual(store.requestedPresentationAppID, "tracker", "the first, genuinely live announcement arms")
            _ = store.consumePendingPreviewRouteAppID(appID: "tracker")
            // Mirrors `RootView`'s `onChange` consuming this synchronously in
            // production — without this, the field would trivially still
            // read "tracker" from the first event regardless of whether the
            // second event re-armed it, making the assertion below vacuous.
            _ = store.consumeRequestedPresentationAppID()

            // A second resync re-announces the SAME still-unconfirmed gate —
            // no new `appGenerationProgress` precedes it, because nothing is
            // running; it is a replay, exactly like the cold-bootstrap case.
            store.handle(event: .appPreviewReady(
                appId: "tracker",
                interactionId: "gate-1",
                revision: 1,
                url: nil
            ))

            XCTAssertNil(store.requestedPresentationAppID, "a replay of an already-consumed gate must not re-arm")
            XCTAssertFalse(store.consumePendingPreviewRouteAppID(appID: "tracker"))
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

        func testDraftSnapshotKeepsTextTypedDuringTheDebounceWindow() {
            let store = LocalAppsStore()
            store.configure { _ in }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 4))
            store.edit(
                field: designField(id: "summary", type: .longText),
                value: .text("最终说明"),
                appID: "tracker"
            )
            store.handle(event: .appDesignDraftChanged(
                appId: "tracker",
                revision: 5,
                fields: ["name": .shortText(value: "Orders")]
            ))

            XCTAssertEqual(store.designers["tracker"]?.fields["summary"], .text("最终说明"))
            XCTAssertEqual(store.designers["tracker"]?.fields["name"], .text("Orders"))
        }

        func testConfirmDesignFlushesTheDebouncedEditBeforeConfirming() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in
                submitted.append(command)
                guard case .updateAppDesignDraft = command else { return }
                store.handle(event: .appDesignDraftChanged(
                    appId: "tracker",
                    revision: 5,
                    fields: ["final_summary": .longText(value: "最终说明")]
                ))
            }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 4))
            store.edit(
                field: designField(id: "final_summary", type: .longText),
                value: .text("最终说明"),
                appID: "tracker"
            )

            let confirmed = await store.confirmDesign(appID: "tracker")
            XCTAssertTrue(confirmed)

            let draftIndex = submitted.firstIndex { if case .updateAppDesignDraft = $0 { true } else { false } }
            let confirmIndex = submitted.firstIndex { if case .confirmAppDesign = $0 { true } else { false } }
            XCTAssertNotNil(draftIndex, "The debounced answer must be sent before the confirm")
            XCTAssertNotNil(confirmIndex)
            guard let draftIndex, let confirmIndex else { return }
            XCTAssertLessThan(draftIndex, confirmIndex)
            guard case let .confirmAppDesign(_, revision, interactionId) = submitted[confirmIndex] else {
                return XCTFail("Expected a confirm design command")
            }
            XCTAssertEqual(revision, 5, "The confirm must carry the revision the engine acked")
            XCTAssertEqual(interactionId, "gate-1")
        }

        /// The drain has a 5 s escape hatch. Falling through it submits the
        /// confirm BEHIND a patch that has not landed, so the engine sees the
        /// patch first and the confirm can only lose the revision race — while
        /// the designer navigates away as if generation had started.
        func testConfirmIsRefusedWhenTheDraftEditNeverAcks() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            // No `.appDesignDraftChanged` echo, so the patch never acks.
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 4))
            store.edit(
                field: designField(id: "final_summary", type: .longText),
                value: .text("最终说明"),
                appID: "tracker"
            )

            let confirmed = await store.confirmDesign(appID: "tracker")

            XCTAssertFalse(confirmed, "A confirm that cannot succeed must not report success")
            XCTAssertFalse(
                submitted.contains { if case .confirmAppDesign = $0 { true } else { false } },
                "The confirm must not be sent behind an unacked draft patch"
            )
            XCTAssertEqual(store.errorMessage, "设计尚未保存完成，请重试。")
            XCTAssertTrue(
                draftPatches(submitted).count == 1,
                "The answer stays queued for the retry rather than being dropped"
            )
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

        /// A rejected patch is provably dead, so it must free the single-slot
        /// queue and let the next edit through.
        func testRejectedDraftPatchReleasesTheQueue() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 5))
            store.edit(
                field: designField(id: "pages", type: .screenList),
                value: .strings(["Home"]),
                appID: "tracker"
            )
            try await waitUntil("the first draft patch") { draftPatches(submitted).count == 1 }
            store.edit(
                field: designField(id: "features", type: .featureList),
                value: .strings(["Reminders"]),
                appID: "tracker"
            )
            try await Task.sleep(for: .milliseconds(50))
            XCTAssertEqual(draftPatches(submitted).count, 1, "The slot is held while the first patch is live")

            store.handle(event: .appOperationFailed(
                appId: "tracker",
                code: .invalidRequest,
                message: "invalid design value"
            ))

            try await waitUntil("the queued edit after the rejection") { draftPatches(submitted).count == 2 }
        }

        /// `AppOperationFailed` carries an app id for runtime, checkpoint and
        /// suggestion commands too; none of those says anything about a draft
        /// patch that is still outstanding.
        func testUnrelatedAppFailureKeepsTheLiveDraftPatch() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 5))
            store.edit(
                field: designField(id: "pages", type: .screenList),
                value: .strings(["Home"]),
                appID: "tracker"
            )
            try await waitUntil("the first draft patch") { draftPatches(submitted).count == 1 }

            store.handle(event: .appOperationFailed(
                appId: "tracker",
                code: .io,
                message: "local app runtime start failed"
            ))
            store.edit(
                field: designField(id: "features", type: .featureList),
                value: .strings(["Reminders"]),
                appID: "tracker"
            )
            try await Task.sleep(for: .milliseconds(100))

            XCTAssertEqual(
                draftPatches(submitted).count,
                1,
                "A runtime failure must not free the slot a live patch still holds"
            )
            XCTAssertEqual(store.errorMessage, "local app runtime start failed")
        }

        /// `update_draft` runs inside `AppService::with_app`, so its own failures also
        /// arrive as Io / NotFound / StorageCorrupt — codes indistinguishable from an
        /// unrelated command's, because `AppOperationFailed` carries no correlation id.
        /// Neither arm of `appOperationFailed` may release the slot for those, so the
        /// watchdog is the only thing standing between a lost patch and a designer
        /// that is wedged for the rest of the session.
        func testAStrandedDraftPatchStopsBlockingLaterEdits() async throws {
            let store = LocalAppsStore()
            store.inFlightEditBudget = .zero
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 5))
            store.edit(
                field: designField(id: "pages", type: .screenList),
                value: .strings(["Home"]),
                appID: "tracker"
            )
            try await waitUntil("the first draft patch") { draftPatches(submitted).count == 1 }

            // The persist behind THIS patch failed. Same shape on the wire as the
            // unrelated-runtime-failure case above, so the slot is still held here.
            store.handle(event: .appOperationFailed(
                appId: "tracker",
                code: .io,
                message: "persist draft: disk full"
            ))
            try await Task.sleep(for: .milliseconds(50))
            XCTAssertEqual(
                draftPatches(submitted).count,
                1,
                "The failure itself must not release the slot — it is indistinguishable from an unrelated command's"
            )

            // Typing again is what un-wedges it: the stale slot is aged out on the
            // next flush rather than by a timer.
            store.edit(
                field: designField(id: "features", type: .featureList),
                value: .strings(["Reminders"]),
                appID: "tracker"
            )
            try await waitUntil("the queued edit once the stale slot is aged out") {
                draftPatches(submitted).count == 2
            }

            // The stranded patch must be RE-QUEUED, not dropped: dropping it loses the
            // user's answer while the optimistic copy keeps the confirm gate satisfied,
            // so a confirm would ship a spec the engine never received. Ack the edit
            // that took the freed slot and the original must go back out.
            store.handle(event: .appDesignDraftChanged(
                appId: "tracker",
                revision: 6,
                fields: ["features": .featureList(value: ["Reminders"])]
            ))
            try await waitUntil("the stranded patch resent after the slot frees again") {
                draftPatches(submitted).count == 3
            }
            guard case let .set(fieldId, value) = draftPatches(submitted).last?.patch.ops.first else {
                return XCTFail("Expected a set op on the resent patch")
            }
            XCTAssertEqual(fieldId, "pages", "The resent patch must carry the stranded field, not a new one")
            XCTAssertEqual(value, .screenList(value: ["Home"]))
        }

        func testInFlightDraftEditSurvivesAnUnrelatedDraftSnapshot() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 4))
            store.edit(
                field: designField(id: "pages", type: .screenList),
                value: .strings(["Home"]),
                appID: "tracker"
            )
            try await waitUntil("the first draft patch") { draftPatches(submitted).count == 1 }

            store.handle(event: .appDesignDraftChanged(
                appId: "tracker",
                revision: 5,
                fields: ["name": .shortText(value: "Orders")]
            ))
            XCTAssertEqual(store.designers["tracker"]?.fields["pages"], .strings(["Home"]))

            store.edit(
                field: designField(id: "features", type: .featureList),
                value: .strings(["Reminders"]),
                appID: "tracker"
            )
            try await Task.sleep(for: .milliseconds(50))
            XCTAssertEqual(
                draftPatches(submitted).count,
                1,
                "A snapshot that does not carry our value must not free the single-slot queue"
            )
        }

        func testDraftConflictKeepsTheNewerQueuedEdit() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 5))
            let pages = designField(id: "pages", type: .screenList)
            store.edit(field: pages, value: .strings(["Home"]), appID: "tracker")
            try await waitUntil("the first draft patch") { draftPatches(submitted).count == 1 }

            store.edit(field: pages, value: .strings(["Home", "Detail"]), appID: "tracker")
            store.handle(event: .appDesignConflict(
                appId: "tracker",
                expectedRevision: 5,
                actualRevision: 6
            ))
            try await waitUntil("the rebased retry") { draftPatches(submitted).count == 2 }

            guard let retry = draftPatches(submitted).last,
                  let operation = retry.patch.ops.first,
                  case let .set(fieldId, value) = operation,
                  case let .screenList(values) = value
            else { return XCTFail("Expected a screen-list retry patch") }
            XCTAssertEqual(retry.revision, 6)
            XCTAssertEqual(fieldId, "pages")
            XCTAssertEqual(values, ["Home", "Detail"], "The rejected value must not overwrite the newer one")
        }

        func testRevisionConflictFailureKeepsTheLocalizedRecoveryCopy() async throws {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }
            store.handle(event: .appDesignerRequested(appId: "tracker", interactionId: "gate-1", revision: 5))
            store.edit(
                field: designField(id: "pages", type: .screenList),
                value: .strings(["Home"]),
                appID: "tracker"
            )
            try await waitUntil("the first draft patch") { draftPatches(submitted).count == 1 }

            store.handle(event: .appDesignConflict(
                appId: "tracker",
                expectedRevision: 5,
                actualRevision: 6
            ))
            store.handle(event: .appOperationFailed(
                appId: "tracker",
                code: .revisionConflict,
                message: "revision conflict: expected 5, actual 6"
            ))
            XCTAssertEqual(store.errorMessage, "设计已在其他位置更新，正在基于最新版本重试。")

            store.handle(event: .appOperationFailed(appId: "tracker", code: .io, message: "boom"))
            XCTAssertEqual(store.errorMessage, "boom", "A genuine failure must still surface")
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

        private func draftPatches(
            _ commands: [ClientCommand]
        ) -> [(revision: UInt64, patch: AppDesignPatchDto)] {
            commands.compactMap { command -> (revision: UInt64, patch: AppDesignPatchDto)? in
                guard case let .updateAppDesignDraft(_, expectedRevision, patch) = command else { return nil }
                return (expectedRevision, patch)
            }
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

    /// The engine's draft gate enforces `^[a-z][a-z0-9_]{0,63}$` on every data
    /// field id, so the designer's own mint has to satisfy it — the UUID this
    /// button used to mint stranded the app in `generation_failed`.
    func testMintedDataFieldIDMatchesTheManifestIdentifierContract() throws {
        let contract = try NSRegularExpression(pattern: "^[a-z][a-z0-9_]{0,63}$")
        func isLegal(_ value: String) -> Bool {
            contract.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) != nil
        }

        // `field_4` is already taken, so the mint has to skip past it.
        var existing = [
            dataField(id: "label"),
            dataField(id: "field_3"),
            dataField(id: "field_4"),
        ]
        for _ in 0 ..< 3 {
            let minted = LocalAppDataFieldIDPolicy.nextID(existing: existing)
            XCTAssertTrue(isLegal(minted), "\(minted) is rejected by the manifest contract")
            XCTAssertFalse(existing.contains { $0.id == minted }, "\(minted) collides")
            existing.append(dataField(id: minted))
        }

        XCTAssertFalse(isLegal(UUID().uuidString.lowercased()), "the pre-fix mint must fail")
    }

    /// The step pills are ungated forward jumps and `confirm_design` has no
    /// required-field check of its own, so the terminal gate is every step.
    func testConfirmGateRequiresEveryStepNotOnlyTheVisibleOne() {
        let step1 = LocalAppDesignStep(
            id: "basics",
            order: 0,
            title: "基础",
            description: "",
            fields: [designField(id: "name", type: .shortText)]
        )
        let step5 = LocalAppDesignStep(
            id: "permissions",
            order: 4,
            title: "确认",
            description: "",
            fields: [designField(id: "final_summary", type: .longText)]
        )
        let values: [String: LocalAppDesignValue] = ["final_summary": .text("done")]

        // The pre-fix gate was `isSatisfied(currentStep, …)`, i.e. the last step
        // alone — which is satisfied here. The real gate must still refuse.
        XCTAssertTrue(LocalAppDesignerGate.canConfirm([step5], values: values))
        XCTAssertFalse(LocalAppDesignerGate.canConfirm([step1, step5], values: values))
    }

    /// `.deferred` ("let the model decide") must satisfy a required field
    /// the same way a real answer does — the questionnaire's "let the LLM
    /// decide" affordance must not silently block the confirm gate. Checked
    /// against a genuinely missing answer on the same field/step so this
    /// cannot pass merely because everything happens to satisfy the gate.
    func testDeferredSatisfiesARequiredFieldGate() {
        let step = LocalAppDesignStep(
            id: "style",
            order: 0,
            title: "风格",
            description: "",
            fields: [designField(id: "tone", type: .singleChoice)]
        )

        XCTAssertTrue(
            LocalAppDesignerGate.isSatisfied(step, values: ["tone": .deferred]),
            ".deferred must satisfy a required field, not read as missing"
        )
        XCTAssertFalse(
            LocalAppDesignerGate.isSatisfied(step, values: [:]),
            "a genuinely missing answer must still block the gate"
        )
    }

    // MARK: - DesignerFieldChips / LocalAppDesignerView.isEditable
    //
    // Adapted from the brief's Swift-Testing (`@Test`/`#expect`) pseudocode to
    // this file's established XCTest conventions, same as Task 13's designer
    // tests above. `DesignerFieldChips`'s testable surface
    // (`chipValues`/`showsCustomInput`/`select(_:)`) is deliberately
    // state-independent so it is exercisable here without a live view host,
    // the same reason `LocalAppDesignerGate` lives outside the
    // `#if canImport(engine_mobileFFI)` block above.

    func testAFieldThatAllowsDeferOffersTheDeferChip() {
        let field = designField(allowsDefer: true)
        XCTAssertTrue(DesignerFieldChips(field: field).chipValues.contains(.deferred))

        // Not vacuously true: a field that does NOT allow defer must not
        // offer the chip either.
        let withoutDefer = designField(allowsDefer: false)
        XCTAssertFalse(DesignerFieldChips(field: withoutDefer).chipValues.contains(.deferred))
    }

    func testAFieldThatAllowsCustomOffersTheOtherBox() {
        let field = designField(allowsCustom: true)
        XCTAssertTrue(DesignerFieldChips(field: field).showsCustomInput)

        let withoutCustom = designField(allowsCustom: false)
        XCTAssertFalse(DesignerFieldChips(field: withoutCustom).showsCustomInput)
    }

    /// `.deferred` is an answer, not an absence — selecting 「由你决定」
    /// must send `LocalAppDesignValue.deferred`, never clear the field
    /// (local-apps#questionnaire, Task 1/13/14).
    func testChoosingDeferStoresTheDeferredValueRatherThanClearingTheField() {
        var recorded: LocalAppDesignValue?
        let chips = DesignerFieldChips(field: designField(allowsDefer: true)) { recorded = $0 }
        chips.select(.deferred)
        XCTAssertEqual(recorded, .deferred, "defer is an answer, not an absence")
    }

    func testSelectingAnOptionChipSetsASingleChoiceFieldsTextValue() {
        let field = LocalAppDesignField(
            id: "tone",
            label: "语气",
            description: "",
            type: .singleChoice,
            required: true,
            allowsCustom: false,
            allowsDefer: false,
            defaultValue: nil,
            options: [LocalAppDesignOption(value: "playful", label: "俏皮")]
        )
        var recorded: LocalAppDesignValue?
        let chips = DesignerFieldChips(field: field) { recorded = $0 }
        chips.select(.option("playful"))
        XCTAssertEqual(recorded, .text("playful"))
    }

    /// A second tap on an already-selected chip removes it — the chips ARE
    /// the multi-select editor now, not an additive-only list.
    func testSelectingAnOptionChipTwiceTogglesAMultipleChoiceFieldsMembership() {
        let field = LocalAppDesignField(
            id: "features",
            label: "需要哪些功能",
            description: "",
            type: .multipleChoice,
            required: true,
            allowsCustom: false,
            allowsDefer: false,
            defaultValue: nil,
            options: [LocalAppDesignOption(value: "list", label: "笔记列表")]
        )
        var recorded: LocalAppDesignValue?
        let chips = DesignerFieldChips(field: field, value: .strings(["list"])) { recorded = $0 }
        chips.select(.option("list"))
        XCTAssertEqual(recorded, .strings([]))
    }

    /// The designer's answering form is only interactive at `collectingSpec`
    /// — while an LLM round trip owns the draft (`authoringQuestionnaire`/
    /// `planning`) a concurrent edit would race it.
    func testTheDesignerIsReadOnlyWhileTheModelIsWorking() {
        for workflow: LocalAppWorkflow in [.authoringQuestionnaire, .planning] {
            XCTAssertFalse(LocalAppDesignerView.isEditable(workflow), "\(workflow) must be read-only")
        }
        XCTAssertTrue(LocalAppDesignerView.isEditable(.collectingSpec))
    }

    /// The two failure states must NOT be silently treated as editable —
    /// each renders its own retry UI instead (`unavailableView(for:)`).
    func testTheTwoFailureStatesAreAlsoReadOnly() {
        for workflow: LocalAppWorkflow in [.questionnaireFailed, .planFailed] {
            XCTAssertFalse(LocalAppDesignerView.isEditable(workflow), "\(workflow) must be read-only")
        }
    }

    /// A pasted URL or a typed capital used to reach the engine verbatim and
    /// come back as a raw English `invalid_request`.
    func testDomainNormalizationMirrorsTheManifestContract() {
        XCTAssertEqual(LocalAppDomainPolicy.normalize("https://api.example.com/v1?x=1"), "api.example.com")
        XCTAssertEqual(LocalAppDomainPolicy.normalize("API.Example.com"), "api.example.com")
        XCTAssertEqual(LocalAppDomainPolicy.normalize("user@api.example.com:8443"), "api.example.com")
        XCTAssertNil(LocalAppDomainPolicy.normalize("https://"))
        XCTAssertNil(LocalAppDomainPolicy.normalize("api..example.com"))
        XCTAssertNil(LocalAppDomainPolicy.normalize("-api.example.com"))
        XCTAssertNil(LocalAppDomainPolicy.normalize("api_example.com"))
    }

    // MARK: - LocalAppPlanConfirmView
    //
    // Adapted from the brief's Swift-Testing (`@Test`/`#expect`) pseudocode to
    // this file's established XCTest conventions, same as the
    // `DesignerFieldChips`/`isEditable` tests above. `LocalAppPlanConfirmView`
    // never touches `LocalAppsStore`/FFI directly — it takes plain closures —
    // so it lives here outside the `#if canImport(engine_mobileFFI)` block too.

    func testThePlanSheetListsEveryCollectionAndField() {
        let view = LocalAppPlanConfirmView(plan: notesPlan(), onConfirm: {}, onBack: {})
        XCTAssertTrue(view.summaryLines.contains { $0.contains("notes") && $0.contains("title") })
    }

    func testThePlanSheetNamesTheDomainsItWillAllow() {
        let view = LocalAppPlanConfirmView(plan: planWithDomain("api.example.com"), onConfirm: {}, onBack: {})
        XCTAssertTrue(view.summaryLines.contains { $0.contains("api.example.com") })
    }

    func testThePlanSheetSaysSoWhenNoNetworkAccessIsRequested() {
        let view = LocalAppPlanConfirmView(plan: notesPlan(), onConfirm: {}, onBack: {})
        XCTAssertTrue(
            view.summaryLines.contains { $0.contains("不访问网络") },
            "silence about network access reads as an omission, not as a guarantee"
        )
    }

    func testTheSheetHasExactlyTwoExits() {
        let view = LocalAppPlanConfirmView(plan: notesPlan(), onConfirm: {}, onBack: {})
        XCTAssertEqual(view.actionTitles, ["返回修改", "确认并生成"])
    }

    /// The sheet's capability section must also say something when the plan
    /// requests none, mirroring the domain section's "silence reads as an
    /// omission" reasoning — not asserted by the brief, but the same logic
    /// applies and the copy exists (`local_apps_plan_confirm_no_capabilities`).
    func testThePlanSheetSaysSoWhenNoCapabilitiesAreRequested() {
        let plan = notesPlan()
        XCTAssertTrue(plan.capabilities.isEmpty, "fixture sanity check")
        let view = LocalAppPlanConfirmView(plan: plan, onConfirm: {}, onBack: {})
        XCTAssertTrue(view.summaryLines.contains { $0.contains("无需额外权限") })
    }

    // MARK: - Task 16: create entry point + persistent revision input

    /// `LocalAppCreateView` (Task 13's replacement for the deleted
    /// `LocalAppCreateSheet`) refuses a whitespace-only brief — the same
    /// empty-input-refused acceptance criterion the brief's now-stale
    /// `theCreateSheetRefusesAnEmptyDescription` targeted, adapted to the
    /// view that actually exists.
    /// Drives the predicate directly rather than poking `view.brief`: that
    /// property is `@State`, so assigning to it on a bare struct outside a
    /// view hierarchy silently does nothing. The previous form left this
    /// test passing vacuously (asserting `false` on a `brief` that was still
    /// `""`) while its non-empty sibling failed — caught by the first actual
    /// run of the iOS suite, which had never been executed before.
    func testTheCreateViewRefusesAWhitespaceOnlyBrief() {
        XCTAssertFalse(LocalAppCreateView.canSubmit(brief: "   \n  "))
        XCTAssertFalse(LocalAppCreateView.canSubmit(brief: ""))
    }

    func testTheCreateViewAcceptsANonEmptyBrief() {
        XCTAssertTrue(LocalAppCreateView.canSubmit(brief: "a shared grocery list for my household"))
        XCTAssertTrue(LocalAppCreateView.canSubmit(brief: "  记事本  "))
    }

    /// `showsRevisionInput` must mirror `AppState::request_revision`'s
    /// accepted source states exactly (`lingxi-code/local-apps/src/state.rs`
    /// `ensure_workflow("request_revision", &[AwaitingPreviewConfirmation,
    /// Ready])`) — enumerating every `LocalAppWorkflow` case, not just the
    /// four the original brief named, so a future workflow addition can't
    /// silently widen or narrow the gate without this test noticing.
    func testShowsRevisionInputMatchesExactlyTheStatesTheEngineAccepts() {
        let accepting: Set<LocalAppWorkflow> = [.ready, .awaitingPreviewConfirmation]
        for workflow in LocalAppWorkflow.allCases {
            XCTAssertEqual(
                LocalAppDetailView.showsRevisionInput(for: workflow),
                accepting.contains(workflow),
                "workflow \(workflow) diverged from AppState::request_revision's accepted states"
            )
        }
    }

    func testAReadyAppShowsAPersistentRevisionInput() {
        XCTAssertTrue(LocalAppDetailView.showsRevisionInput(for: .ready))
        XCTAssertTrue(LocalAppDetailView.showsRevisionInput(for: .awaitingPreviewConfirmation))
    }

    func testAnAppStillGeneratingDoesNotShowTheRevisionInput() {
        XCTAssertFalse(LocalAppDetailView.showsRevisionInput(for: .generating))
        XCTAssertFalse(LocalAppDetailView.showsRevisionInput(for: .authoringQuestionnaire))
    }

    #if canImport(engine_mobileFFI)
        /// Creation has two independent callers of `openDesigner` — the
        /// `appsChanged` handler and the designer view's `prepare()` — and the
        /// engine rejects the second with
        /// `open_designer is not allowed while app … is in workflow state
        /// awaiting_spec_confirmation`, because the first already made that
        /// transition. Only one request may leave the client until the gate
        /// comes back.
        func testASecondDesignerOpenIsWithheldUntilTheGateArrives() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            await store.openDesigner(appID: "tracker")
            await store.openDesigner(appID: "tracker")

            XCTAssertEqual(
                Self.opens(in: submitted),
                1,
                "the second open would be rejected by the engine"
            )
            // The turned-away caller still gets its refresh, so it is not left
            // staring at an empty designer.
            XCTAssertEqual(Self.detailFetches(in: submitted), 2)
        }

        /// A reopen after `generation_failed` is legal, so arrival of the gate
        /// must release the marker rather than wedge the app forever.
        func testTheGateArrivingReleasesTheHoldOnFurtherOpens() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            await store.openDesigner(appID: "tracker")
            store.handle(event: .appDesignerRequested(
                appId: "tracker",
                interactionId: "gate-1",
                revision: 1
            ))
            await store.openDesigner(appID: "tracker")

            XCTAssertEqual(Self.opens(in: submitted), 2)
        }

        /// A failed open produces no gate, so nothing else would ever clear the
        /// marker — the designer would be unreachable for the rest of the run.
        func testAFailureReleasesTheHoldEvenThoughItCarriesNoCorrelationID() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            await store.openDesigner(appID: "tracker")
            store.handle(event: .appOperationFailed(
                appId: "tracker",
                code: .workflowStateInvalid,
                message: "boom"
            ))
            await store.openDesigner(appID: "tracker")

            XCTAssertEqual(Self.opens(in: submitted), 2)
        }

        /// The hold is per app, not global.
        func testTheHoldDoesNotBlockADifferentApp() async {
            let store = LocalAppsStore()
            var submitted: [ClientCommand] = []
            store.configure { command in submitted.append(command) }

            await store.openDesigner(appID: "tracker")
            await store.openDesigner(appID: "metrics")

            XCTAssertEqual(Self.opens(in: submitted), 2)
        }

        /// Counted by pattern-matching the case rather than by string, so a
        /// renamed or reshaped command is a compile error instead of a count
        /// that quietly drops to zero and makes the "withheld" assertion pass
        /// for the wrong reason.
        private static func opens(in submitted: [ClientCommand]) -> Int {
            submitted.filter { if case .openAppDesigner = $0 { true } else { false } }.count
        }

        private static func detailFetches(in submitted: [ClientCommand]) -> Int {
            submitted.filter { if case .getAppDetails = $0 { true } else { false } }.count
        }
    #endif

    #if canImport(engine_mobileFFI)
        private func app(
            id: String,
            name: String,
            brief: String = "简介"
        ) -> AppRecordDto {
            AppRecordDto(
                id: id,
                name: name,
                brief: brief,
                createdAtMs: 1,
                updatedAtMs: 2,
                workflowState: .collectingSpec,
                conversationId: nil,
                workspaceRel: "apps/\(id)/workspace"
            )
        }
    #endif
}

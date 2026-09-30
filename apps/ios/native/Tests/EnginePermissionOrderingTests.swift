import Foundation
import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    @MainActor
    final class EnginePermissionOrderingTests: XCTestCase {
        func testExistingPumpCannotResurrectCancelledPermission() async {
            await assertExistingPumpRemovesPermission(resolution: .cancelled)
        }

        func testExistingPumpCannotResurrectExpiredPermission() async {
            await assertExistingPumpRemovesPermission(resolution: .expired)
        }

        func testIdleBackgroundPermissionSurvivesUnrelatedResolution() async {
            let source = makeSource()
            let listener = EngineListener(source: source)
            let sink = EnginePermissionSink(listener: listener)
            let request = makeRequest(id: 202)

            await sink.onRequest(request: request)
            await listener.onEvent(event: .permissionRequestResolved(requestId: 201, resolution: .cancelled))
            await listener.waitUntilIdle()

            XCTAssertFalse(source.model.streaming)
            XCTAssertEqual(source.model.pendingPermissions, [PendingPermission(request: request)])
        }

        func testResolutionBeforeNotifyDoesNotReviveTerminalRequest() async {
            let source = makeSource()
            let listener = EngineListener(source: source)
            let sink = EnginePermissionSink(listener: listener)
            await listener.onEvent(event: .permissionRequestResolved(requestId: 201, resolution: .cancelled))
            await listener.waitUntilIdle()

            await sink.onRequest(request: makeRequest(id: 201))
            await sink.onRequest(request: makeRequest(id: 202))
            await listener.waitUntilIdle()

            XCTAssertEqual(source.model.pendingPermissions.map(\.requestId), [202])
        }

        func testNewListenerGenerationCanReuseRequestID() async {
            let source = makeSource()
            let oldListener = EngineListener(source: source)
            await oldListener.onEvent(event: .permissionRequestResolved(requestId: 201, resolution: .cancelled))
            await oldListener.waitUntilIdle()
            let newListener = EngineListener(source: source)
            await EnginePermissionSink(listener: newListener).onRequest(request: makeRequest(id: 201))
            await newListener.waitUntilIdle()

            XCTAssertEqual(source.model.pendingPermissions.map(\.requestId), [201])
        }

        func testSameEngineSessionSwitchPreservesTerminalRequestIDs() async {
            let source = makeSource()
            let listener = EngineListener(source: source)
            let sink = EnginePermissionSink(listener: listener)
            await listener.onEvent(event: .permissionRequestResolved(requestId: 201, resolution: .cancelled))
            await listener.onEvent(event: .sessionStarted(sessionId: "next-session", mode: .code))
            await listener.waitUntilIdle()
            await sink.onRequest(request: makeRequest(id: 201))
            await sink.onRequest(request: makeRequest(id: 202))
            await listener.waitUntilIdle()

            XCTAssertEqual(source.model.pendingPermissions.map(\.requestId), [202])
        }

        private func assertExistingPumpRemovesPermission(resolution: PermissionResolutionDto) async {
            let source = makeSource()
            let listener = EngineListener(source: source)
            let sink = EnginePermissionSink(listener: listener)
            let request = makeRequest(id: 201)
            let delivered = DispatchSemaphore(value: 0)
            // This synchronous test segment keeps the existing pump pending.
            // Both real native callbacks must return without awaiting the actor.
            listener.enqueueForTesting(.permissionRequestResolved(requestId: 999, resolution: .cancelled))
            Task.detached {
                await sink.onRequest(request: request)
                await listener.onEvent(event: .permissionRequestResolved(requestId: 201, resolution: resolution))
                delivered.signal()
            }
            XCTAssertEqual(delivered.wait(timeout: .now() + 2), .success)
            await listener.waitUntilIdle()

            XCTAssertFalse(source.model.streaming)
            XCTAssertTrue(source.model.pendingPermissions.isEmpty)
        }

        private func makeSource() -> EngineConversationSource {
            EngineConversationSource(config: EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                projectCwd: nil,
                sessionMode: .code,
                visionDelegationEnabled: true
            ))
        }

        private func makeRequest(id: UInt64) -> PermissionRequest {
            PermissionRequest(
                requestId: id,
                kind: .toolUseConfirm(toolName: "Shell", toolInputJson: "{}", defaultAllow: false),
                worker: WorkerInfoDto(name: "background", color: "background", team: nil),
                owner: nil,
                suppressAlwaysAllowRule: false,
                autoModePrompt: nil
            )
        }
    }
#endif

import Foundation
import XCTest
@testable import LingxiCode

#if canImport(engine_mobileFFI)
import engine_mobileFFI

private actor AdminRoundtripListener: IosEventListener {
    private var events: [ClientEvent] = []

    func onEvent(event: ClientEvent) async { events.append(event) }
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
    func onWorkflowProgress(taskId: String, runId: String, progress: WorkflowProgressDto) async {}
    func clear() { events.removeAll(keepingCapacity: true) }
    func pop() -> ClientEvent? { events.isEmpty ? nil : events.removeFirst() }
}

/// Real Swift ABI -> native admin handlers -> typed callbacks. No provider,
/// credential store, user configuration, or successful no-op ACK can satisfy it.
final class NativeAdminEngineRoundtripTests: XCTestCase {
    func testKeylessAdminCatalogsAndCorrelatedHookValidationReturnThroughNativeCallbacks() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("native-admin-\(UUID().uuidString)", isDirectory: true)
        let workspace = root.appendingPathComponent("Projects/\(UUID().uuidString.lowercased())/workspace", isDirectory: true)
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try await exercise(root: root, workspace: workspace)
    }

    // Keeping the handle in this frame releases it before the caller removes its sandbox.
    private func exercise(root: URL, workspace: URL) async throws {
        let listener = AdminRoundtripListener()
        let audio = await MainActor.run { IOSAudioServiceCallbackAdapter.shared }
        let handle = try buildIosEngineWithConfig(config: IosEngineLaunchConfigFfi(
            apiBase: "https://invalid.example", apiKey: "", model: "", sessionMode: .code,
            visionDelegationEnabled: false, appSandboxRoot: root.path, projectCwd: workspace.path,
            providerConfig: IosProviderConfigFfi(providerProfilesJson: "{}", routingJson: #"{"mobileEnabledProfiles":[]}"#),
            mobileLinux: nil, localAppsFullRuntime: false, localAppsRuntimeRoot: nil,
            physicalMemoryBytes: 0, hostEnvironment: nil),
            listener: listener, audio: audio, camera: CameraImpl(), share: ShareImpl(),
            notifications: NotificationImpl(), clipboard: ClipboardImpl(),
            permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)

        func response(_ command: ClientCommand, matching predicate: (ClientEvent) -> Bool) async throws -> ClientEvent {
            await listener.clear()
            try await handle.submit(command: command)
            let deadline = Date().addingTimeInterval(20)
            while Date() < deadline {
                if let event = await listener.pop() {
                    if case let .error(_, message) = event {
                        throw NSError(domain: "Native admin command failed", code: 1,
                                      userInfo: [NSLocalizedDescriptionKey: message])
                    }
                    if predicate(event) { return event }
                }
                try await Task.sleep(for: .milliseconds(20))
            }
            throw NSError(domain: "Timed out waiting for typed native admin reply", code: 2)
        }

        let mcp = try await response(.mcpAdmin(command: McpAdminCommandDto(
            action: "get_snapshot", operationId: nil, target: nil, scope: nil, revision: nil, payloadJson: nil))) {
            if case .mcpConfigurationSnapshot = $0 { return true }; return false
        }
        guard case let .mcpConfigurationSnapshot(mcpJSON) = mcp else { return XCTFail("Expected typed MCP snapshot") }
        let scopes = try XCTUnwrap(try object(mcpJSON)["scopes"] as? [[String: Any]])
        XCTAssertFalse(scopes.isEmpty)
        XCTAssertEqual((scopes.first?["revision_sha256"] as? String)?.count, 64)

        let skills = try await response(.skillAdmin(command: SkillAdminCommandDto(
            action: "get_catalog", operationId: nil, target: nil, scope: nil, revision: nil, payloadJson: nil))) {
            if case .skillCatalog = $0 { return true }; return false
        }
        guard case let .skillCatalog(skillsJSON) = skills else { return XCTFail("Expected typed skill catalog") }
        XCTAssertNotNil(try object(skillsJSON)["entries"] as? [Any])

        let plugins = try await response(.pluginAdmin(command: PluginAdminCommandDto(
            action: "get_catalog", operationId: nil, target: nil, scope: nil, revision: nil, payloadJson: nil))) {
            if case .pluginCatalog = $0 { return true }; return false
        }
        guard case let .pluginCatalog(pluginsJSON) = plugins else { return XCTFail("Expected typed plugin catalog") }
        XCTAssertNotNil(try object(pluginsJSON)["installed"] as? [Any])
        XCTAssertNotNil(try object(pluginsJSON)["revisions"] as? [String: Any])

        let hooks = try await response(.hookAdmin(command: HookAdminCommandDto(
            action: "get_document", operationId: nil, target: nil, scope: "user", revision: nil, payloadJson: nil))) {
            if case let .configurationOperation(domain, _, _, _, _, details) = $0 {
                return domain == .hook && details != nil
            }
            return false
        }
        guard case let .configurationOperation(_, _, hookStatus, _, _, hookDetails) = hooks else {
            return XCTFail("Expected typed hook document operation")
        }
        XCTAssertEqual(hookStatus, .succeeded)
        let document = try object(XCTUnwrap(hookDetails))
        XCTAssertEqual(document["scope"] as? String, "user")
        XCTAssertEqual((document["revision_sha256"] as? String)?.count, 64)
        _ = try object(XCTUnwrap(document["own_json"] as? String))

        let operationID: UInt64 = 917
        let validation = try await response(.hookAdmin(command: HookAdminCommandDto(
            action: "validate_document", operationId: operationID, target: nil, scope: "user", revision: nil,
            payloadJson: #"{"scope":"user","hooks":{}}"#))) {
            if case let .configurationOperation(domain, id, status, _, _, _) = $0 {
                return domain == .hook && id == operationID && (status == .succeeded || status == .failed)
            }
            return false
        }
        guard case let .configurationOperation(_, returnedID, status, effect, _, _) = validation else {
            return XCTFail("Expected correlated hook validation result")
        }
        XCTAssertEqual(returnedID, operationID)
        XCTAssertEqual(status, .succeeded)
        XCTAssertEqual(effect, .notApplicable)
    }

    private func object(_ json: String) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
    }
}
#else
final class NativeAdminEngineRoundtripTests: XCTestCase {
    func testAdminRequiresRealNativeBindings() { XCTFail("Native admin roundtrip requires linked engine_mobileFFI") }
}
#endif

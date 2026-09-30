import Foundation
import XCTest
@testable import LingxiCode
#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

private struct SettingsRoundtripSnapshot: Sendable {
    let effective: String
    let layers: String?
}

private actor SettingsRoundtripListener: IosEventListener {
    enum Reply: Sendable { case snapshot(SettingsRoundtripSnapshot), failure(String) }
    private var replies: [Reply] = []
    func onEvent(event: ClientEvent) async {
        switch event {
        case let .settingsSnapshot(effective, _, _, _, _, layers, _):
            replies.append(.snapshot(SettingsRoundtripSnapshot(effective: effective, layers: layers)))
        case let .error(_, message): replies.append(.failure(message))
        default: break
        }
    }
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
    func onWorkflowProgress(taskId: String, runId: String, progress: WorkflowProgressDto) async {}
    func pop() -> Reply? { replies.isEmpty ? nil : replies.removeFirst() }
}

/// Real Swift ABI -> engine -> distinct user/project/local files, with no keychain or provider.
final class NativeSettingsEngineRoundtripTests: XCTestCase {
    func testKeylessLayerWritesReadBackAndSurviveEngineRebuild() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("native-settings-\(UUID().uuidString)", isDirectory: true)
        let workspace = root.appendingPathComponent("Projects/\(UUID().uuidString.lowercased())/workspace", isDirectory: true)
        let userFile = root.appendingPathComponent(".lingxi/settings.json")
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: userFile.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(#"{"viewMode":"focus"}"#.utf8).write(to: userFile)
        defer { try? FileManager.default.removeItem(at: root) }
        let layers: [(WritableScopeDto, String, URL, String)] = [
            (.user, "user", userFile, "terse"),
            (.project, "project", workspace.appendingPathComponent(".lingxi/settings.json"), "verbose"),
            (.local, "local", workspace.appendingPathComponent(".lingxi/settings.local.json"), "default")
        ]
        // Each call builds a new real handle, which is released on return.
        try await exercise(root: root, workspace: workspace, layers: layers, write: true)
        try await exercise(root: root, workspace: workspace, layers: layers, write: false)
    }

    private func exercise(root: URL, workspace: URL,
                          layers: [(WritableScopeDto, String, URL, String)], write: Bool) async throws {
        let listener = SettingsRoundtripListener()
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
        try await handle.submit(command: .refreshListings(which: [.settings]))
        let initial = try await snapshot(listener)
        XCTAssertEqual(try object(initial.effective)["viewMode"] as? String, "focus")
        if write {
            for (destination, layer, file, value) in layers {
                let patch = String(decoding: try JSONSerialization.data(withJSONObject: ["outputStyle": value]), as: UTF8.self)
                try await handle.submit(command: .updateSettings(destination: destination, patchJson: patch))
                let ack = try await snapshot(listener, layer: layer, value: value)
                XCTAssertEqual(try object(ack.effective)["outputStyle"] as? String, value)
                XCTAssertEqual(try fileObject(file)["outputStyle"] as? String, value)
                try await handle.submit(command: .refreshListings(which: [.settings]))
                _ = try await snapshot(listener, layer: layer, value: value)
            }
        }
        try await handle.submit(command: .refreshListings(which: [.settings]))
        let final = try await snapshot(listener, layer: "local", value: "default")
        let maps = try object(XCTUnwrap(final.layers))
        for (_, layer, file, value) in layers {
            XCTAssertEqual((maps[layer] as? [String: Any])?["outputStyle"] as? String, value)
            XCTAssertEqual(try fileObject(file)["outputStyle"] as? String, value)
        }
        XCTAssertEqual(try fileObject(layers[0].2)["viewMode"] as? String, "focus")
    }

    private func snapshot(_ listener: SettingsRoundtripListener, layer: String? = nil, value: String? = nil) async throws -> SettingsRoundtripSnapshot {
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline {
            if let reply = await listener.pop() {
                switch reply {
                case let .failure(message): throw NSError(domain: "Real engine settings command failed", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
                case let .snapshot(snapshot):
                    if let layer {
                        let maps = try object(snapshot.layers ?? "{}")
                        if (maps[layer] as? [String: Any])?["outputStyle"] as? String == value { return snapshot }
                    } else { return snapshot }
                }
            }
            try await Task.sleep(for: .milliseconds(20))
        }
        throw NSError(domain: "Timed out waiting for real engine SettingsSnapshot", code: 2)
    }
    private func object(_ json: String) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
    }
    private func fileObject(_ file: URL) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
    }
}
#else
final class NativeSettingsEngineRoundtripTests: XCTestCase {
    func testSettingsRequiresRealNativeBindings() { XCTFail("Native settings roundtrip requires linked harness_runtimeFFI") }
}
#endif

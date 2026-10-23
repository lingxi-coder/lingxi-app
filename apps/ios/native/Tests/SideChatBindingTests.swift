import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

@MainActor
final class SideChatBindingTests: XCTestCase {
    /// Execute the regenerated Swift -> native library contract without a model
    /// request, credentials, or changes to the user's persistent workspace.
    func testSideChatCreatesDispatchesAndEndsThroughNativeBindings() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("side-chat-binding-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let engine = try buildIosEngine(
            apiBase: "https://api.anthropic.com", apiKey: "", model: "",
            appSandboxRoot: root.path,
            listener: NoopEventListener(), audio: IOSAudioServiceCallbackAdapter.shared,
            camera: CameraImpl(), share: ShareImpl(), notifications: NotificationImpl(),
            clipboard: ClipboardImpl(), permissions: NoopPermissionSink(),
            mobileLinux: try mobileWorkspaceTestConfiguration(sandboxRoot: root), secureStorage: nil, deviceControl: nil)
        let info = try await createIosSideChat(
            engine: engine, requestId: UUID().uuidString, initialInput: nil,
            listener: NoopEventListener(), permissions: NoopPermissionSink())
        XCTAssertFalse(info.sideId.isEmpty)
        let created = await engine.listSideChats()
        XCTAssertTrue(created.contains { $0.sideId == info.sideId })
        try await engine.submitSideChat(sideId: info.sideId, command: .getConversationControls)
        try await engine.endSideChat(sideId: info.sideId)
        let remaining = await engine.listSideChats()
        XCTAssertFalse(remaining.contains { $0.sideId == info.sideId })
    }
}
#endif

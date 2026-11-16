import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

@MainActor
final class SideChatBindingTests: XCTestCase {
    /// Execute the regenerated Swift -> native library contract without a model
    /// request, credentials, or changes to the user's persistent workspace.
    func testSideChatCreatesDispatchesAndEndsThroughNativeBindings() async throws {
        let fixture = try MobileWorkspaceNativeTestFixture(testCase: self)
        let mediaHostComposition = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)
        )
        let engine = try buildIosEngineWithConfig(config: try fixture.launchConfiguration(apiBase: "https://api.anthropic.com", model: ""), listener: NoopEventListener(), mediaHost: mediaHostComposition.nativeHost,
            share: ShareImpl(), notifications: NotificationImpl(),
            clipboard: ClipboardImpl(), permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)
        fixture.track(engine)
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

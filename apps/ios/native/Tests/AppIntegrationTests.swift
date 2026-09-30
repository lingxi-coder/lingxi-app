import XCTest

@testable import LingxiCode

final class AppIntegrationTests: XCTestCase {
    func testPendingActionsSurviveStoreRecreationAndDrainOnce() async {
        let suiteName = "AppIntegrationTests.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            return XCTFail("expected isolated defaults")
        }
        defer { defaults.removePersistentDomain(forName: suiteName) }

        let writer = LingxiAppActionStore(defaults: defaults)
        await writer.enqueue(.newConversation)
        await writer.enqueue(.ask("检查项目"))

        let reader = LingxiAppActionStore(defaults: defaults)
        let restored = await reader.drain()
        let drainedAgain = await reader.drain()

        XCTAssertEqual(restored, [.newConversation, .ask("检查项目")])
        XCTAssertTrue(drainedAgain.isEmpty, "an App Intent must be handled exactly once")
    }

    func testTerminalDeepLinkMatchesAndroidRouteAndDecodesCommand() {
        let url = URL(string: "lingxi://open_terminal?sessionId=shell-7&initCommand=git%20status")!

        XCTAssertEqual(
            LingxiDeepLink.action(from: url),
            .openTerminal(sessionID: "shell-7", initialCommand: "git status")
        )
    }

    func testTerminalDeepLinkDefaultsSessionAndRejectsUnknownRoutes() {
        XCTAssertEqual(
            LingxiDeepLink.action(from: URL(string: "lingxi://open_terminal")!),
            .openTerminal(sessionID: "interactive", initialCommand: nil)
        )
        XCTAssertNil(LingxiDeepLink.action(from: URL(string: "https://open_terminal")!))
        XCTAssertNil(LingxiDeepLink.action(from: URL(string: "lingxi://unknown")!))
    }

    func testConversationDeepLinkParsesSessionAndOptionalTurn() {
        XCTAssertEqual(
            LingxiDeepLink.action(
                from: URL(string: "lingxi://open_conversation?sessionId=session-a&turnId=42")!
            ),
            .openConversation(
                sessionID: "session-a",
                turnID: 42,
                workspaceKey: nil,
                mode: .code
            )
        )
        XCTAssertEqual(
            LingxiDeepLink.action(
                from: URL(string: "lingxi://open_conversation?sessionId=session-a")!
            ),
            .openConversation(
                sessionID: "session-a",
                turnID: nil,
                workspaceKey: nil,
                mode: .code
            )
        )
        XCTAssertEqual(
            LingxiDeepLink.conversationURL(sessionID: "session-a", turnID: 42)?.absoluteString,
            "lingxi://open_conversation?sessionId=session-a&turnId=42&sessionMode=code"
        )
        XCTAssertEqual(
            LingxiDeepLink.action(from: URL(
                string: "lingxi://open_conversation?sessionId=chat-a&workspaceKey=app.weather&sessionMode=chat"
            )!),
            .openConversation(
                sessionID: "chat-a",
                turnID: nil,
                workspaceKey: "app.weather",
                mode: .chat
            )
        )
    }

    func testLocalAppDeepLinkParsesValidatedUnifiedRoute() {
        let url = URL(
            string: "lingxi://open_local_app?appId=tracker-1&destination=preview&autostart=1&source=widget"
        )!

        XCTAssertEqual(
            LingxiDeepLink.action(from: url),
            .openLocalApp(
                appID: "tracker-1",
                destination: "preview",
                autostart: true,
                source: "widget"
            )
        )
    }

    func testDeviceControlCalendarQueryDecodesHostSnakeCase() throws {
        let query = try DeviceControlWireJSON.decodeCalendarQuery(
            #"{"start_ms":1000,"end_ms":2000,"limit":8}"#
        )
        XCTAssertEqual(query.startMs, 1000)
        XCTAssertEqual(query.endMs, 2000)
        XCTAssertEqual(query.limit, 8)
    }

    func testDeviceControlCalendarQueryRejectsCamelCaseHostDrift() {
        XCTAssertThrowsError(
            try DeviceControlWireJSON.decodeCalendarQuery(
                #"{"startMs":1000,"endMs":2000,"limit":8}"#
            )
        )
    }

    func testLocalAppDeepLinkRejectsInvalidIDsAndUnknownQueries() {
        XCTAssertNil(
            LingxiDeepLink.action(
                from: URL(string: "lingxi://open_local_app?appId=Tracker&destination=preview&autostart=1")!
            )
        )
        XCTAssertNil(
            LingxiDeepLink.action(
                from: URL(string: "lingxi://open_local_app?appId=tracker&destination=preview&autostart=1&extra=1")!
            )
        )
    }

    func testMarkdownTerminalLinkKeepsAndroidCompatibleURL() {
        let expected = URL(string: "lingxi://open_terminal?sessionId=shell-8&initCommand=pwd")!
        let rendered = AIText.parseInline(
            "在 [终端](\(expected.absoluteString)) 中继续",
            size: 15.5
        )

        XCTAssertEqual(rendered.runs.compactMap(\.link), [expected])
        XCTAssertEqual(String(rendered.characters), "在 终端 中继续")
    }
}

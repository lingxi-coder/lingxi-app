import XCTest

@testable import LingxiCode

final class AppIntegrationTests: XCTestCase {
    func testBrowserAgentReturnIsLimitedToAppReview() {
        XCTAssertEqual(CanonicalDeepLink.parse("lingxi://apps/a123?review=agent")?.params["review"], "agent")
        XCTAssertEqual(CanonicalDeepLink.parse("https://lingxios.net/open/apps/a123?review=agent")?.route, "apps/a123")
        for raw in ["lingxi://new?review=agent", "lingxi://session/a123?review=agent",
                    "lingxi://apps/a123?review=approve", "lingxi://apps/a123?review=agent&review=agent"] {
            XCTAssertNil(CanonicalDeepLink.parse(raw), raw)
        }
    }

    func testBrowserAgentWakePreservesRunIDExactly() throws {
        let runID = "opaque run +/&-灵犀"
        var parts = URLComponents(string: "lingxi://apps/a123")!
        parts.queryItems = [
            URLQueryItem(name: "review", value: "agent"),
            URLQueryItem(name: "runId", value: runID),
            URLQueryItem(name: "revision", value: "7")
        ]
        let route = try XCTUnwrap(parts.url.flatMap(CanonicalDeepLink.parse))
        XCTAssertEqual(route.params["runId"], runID)
        XCTAssertEqual(route.params["revision"], "7")
        for query in [
            "review=agent&runId=run&revision=0",
            "review=agent&runId=run&revision=07",
            "review=agent&runId=run",
            "review=agent&revision=1",
            "review=agent&runId=\(String(repeating: "a", count: 129))&revision=1"
        ] {
            XCTAssertNil(CanonicalDeepLink.parse("lingxi://apps/a123?\(query)"), query)
        }
    }

    func testRuntimeContextRebindHonorsModeAndWorkspaceWithoutRepeatingSameContext() throws {
        let modeOnly = try XCTUnwrap(CanonicalDeepLink.parse("lingxi://runtime/agent/Agent-A?mode=chat"))
        XCTAssertTrue(modeOnly.needsContextRebind(workspace: "global", mode: "code"))
        XCTAssertFalse(modeOnly.needsContextRebind(workspace: "global", mode: "chat"))
        let both = try XCTUnwrap(CanonicalDeepLink.parse("lingxi://runtime/agent/Agent-A?workspace=global&mode=chat"))
        XCTAssertTrue(both.needsContextRebind(workspace: "project.A", mode: "chat"))
        XCTAssertTrue(both.needsContextRebind(workspace: "global", mode: "code"))
        XCTAssertFalse(both.needsContextRebind(workspace: "global", mode: "chat"))
        let session = try XCTUnwrap(CanonicalDeepLink.parse("lingxi://session/Session-A?mode=chat"))
        XCTAssertFalse(session.needsContextRebind(workspace: "global", mode: "code"))
        let new = try XCTUnwrap(CanonicalDeepLink.parse("lingxi://new?workspace=global&mode=chat"))
        XCTAssertFalse(new.needsContextRebind(workspace: "project.A", mode: "code"))
    }
    func testCanonicalHTTPSPreservesDraftAndRejectsRepeatedParameters() {
        let raw = "https://lingxios.net/open/new#payload=%7B%22prompt%22%3A%22hello+world%26next%22%7D"
        guard case let .canonical(target)? = LingxiDeepLink.action(from: URL(string: raw)!) else { return XCTFail("expected canonical draft") }
        XCTAssertEqual(target.payload["prompt"] as? String, "hello world&next")
        XCTAssertNil(LingxiDeepLink.action(from: URL(string: "lingxi://session/a?mode=code&mode=chat")!))
        XCTAssertNil(LingxiDeepLink.action(from: URL(string: "https://lingxios.net.evil/open/new")!))
    }

    func testCanonicalRoutesAndPersistedMessageIdentity() {
        let uuid = "4A16A316-E6FE-4D5A-9A5D-A42E390990DA"
        for route in ["provider/custom", "extensions/mcp/server", "extensions/skill/skill", "extensions/plugin/plugin", "cron/job/job", "cron/run/run", "apps/app", "runtime/agent/a", "runtime/task/t", "runtime/plan/p", "runtime/review/r", "runtime/activity/a"] {
            XCTAssertEqual(CanonicalDeepLink.parse("lingxi://" + route)?.route, route)
        }
        let link = ConversationDeepLink.makeURL(sessionID: "session-A", turnID: nil, workspaceKey: "project.a", sessionMode: "chat", messageUUID: uuid)
        let target = link.flatMap(CanonicalDeepLink.parse)
        XCTAssertEqual(target?.params["message"], uuid.lowercased())
        XCTAssertEqual(target?.params["workspace"], "project.a")
        XCTAssertEqual(target?.parts[1], "session-A")
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://session/a?message=i1"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://session/%2fsecret"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://session/%ZZ"))
    }

    func testImportsRejectNestedCredentialsAndURLQueries() throws {
        for payload in [#"{"name":"x","config":{"env":{"TOKEN":"secret"}}}"#,
                        #"{"plugin":"x","source":"https://example.com/repo?token=x"}"#,
                        #"{"providers":{"x":{"apiKey":"secret"}}}"#] {
            var components = URLComponents(string: "lingxi://import/mcp")!
            components.percentEncodedFragment = "payload=" + payload.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
            XCTAssertNil(CanonicalDeepLink.parse(components.url!))
        }
        XCTAssertNotNil(CanonicalDeepLink.parse("lingxi://import/skill#payload=%7B%22name%22%3A%22review%22%2C%22markdown%22%3A%22%23%20Review%22%7D"))
    }

    func testCanonicalNativeAdapterMatchesSharedFixtureCorpus() throws {
        let file = try XCTUnwrap(Bundle(for: AppIntegrationTests.self).url(forResource: "fixtures", withExtension: "json"))
        let data = try Data(contentsOf: file)
        let corpus = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        for entry in try XCTUnwrap(corpus["valid"] as? [[String: Any]]) {
            let target = CanonicalDeepLink.parse(try XCTUnwrap(entry["url"] as? String))
            XCTAssertEqual(target?.route, entry["route"] as? String)
            if let params = entry["params"] as? [String: String] { XCTAssertEqual(target?.params, params) }
        }
        for raw in try XCTUnwrap(corpus["invalid"] as? [String]) { XCTAssertNil(CanonicalDeepLink.parse(raw), raw) }
    }

    func testMCPDetailSelectionRetainsScopeAndRejectsAmbiguousNames() {
        let scopes: [[String: Any]] = ["user", "project"].map {
            ["scope": $0, "raw_json": #"{"mcpServers":{"same":{"command":"test"}}}"#]
        }
        let snapshot: [String: Any] = ["scopes": scopes]
        let target = CanonicalDeepLink.parse("lingxi://extensions/mcp/same?scope=project")!
        XCTAssertEqual(MCPDeepLinkSelection.resolve(name: target.parts[2], requestedScope: target.params["scope"], snapshot: snapshot), .selected(scope: "project"))
        XCTAssertEqual(MCPDeepLinkSelection.resolve(name: "same", requestedScope: nil, snapshot: snapshot), .ambiguous)
        XCTAssertEqual(MCPDeepLinkSelection.resolve(name: "missing", requestedScope: "user", snapshot: snapshot), .missing)
        XCTAssertEqual(MCPDeepLinkSelection.resolve(name: "same", requestedScope: "managed", snapshot: snapshot), .invalidScope)
    }

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

    func testPendingQueueIsBoundedAndDeduplicatesRepeatedOSDelivery() async {
        let suite = "AppIntegrationQueue.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = LingxiAppActionStore(defaults: defaults)
        for index in 0..<10 { await store.enqueue(.ask(String(index))) }
        await store.enqueue(.ask("8"))
        let actions = await store.drain()
        XCTAssertEqual(actions, ["2", "3", "4", "5", "6", "7", "9", "8"].map(LingxiAppAction.ask))
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
            "https://lingxios.net/open/session/session-a?mode=code"
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

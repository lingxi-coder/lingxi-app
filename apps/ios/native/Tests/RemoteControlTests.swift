import XCTest
@testable import LingxiCode

final class RemoteControlTests: XCTestCase {
    private let host = "Hk3xQ9vR2mT7wL1pZ8cN4a"
    private let session = "11111111-2222-4333-8444-555555555555"

    func testRemoteLinksPreserveCaseAndDoNotRebindLocalWorkspace() throws {
        let route = "remote/\(host)/\(session)"
        let link = try XCTUnwrap(CanonicalDeepLink.parse("lingxi://" + route))
        XCTAssertEqual(link.route, route)
        XCTAssertFalse(link.needsContextRebind(workspace: "project.other", mode: "chat"))
        XCTAssertEqual(CanonicalDeepLink.parse("https://lingxios.net/open/" + route), link)
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://" + route + "?workspace=project.other"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://remote/host/../session"))
        XCTAssertNil(CanonicalDeepLink.parse("lingxi://" + route + "/extra"))
    }

    func testPushOnlyRoutesCurrentUnexpiredAccountToRemoteControl() {
        let route = "remote/\(host)/\(session)"
        let now = Date(timeIntervalSince1970: 1)
        var payload: [AnyHashable: Any] = ["recipientUid": "A", "expiresAt": "2000", "route": route]
        XCTAssertNotNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "B", enabled: true, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: false, now: now))
        XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: Date(timeIntervalSince1970: 2)))
        for invalid in ["settings/account", "new", "remote/\(host)/bad", route + "?workspace=global", route + "\n"] {
            payload["route"] = invalid
            XCTAssertNil(PushMessagePolicy.remoteControlAction(payload, uid: "A", enabled: true, now: now), invalid)
        }
    }

    #if canImport(harness_runtimeFFI)
    @MainActor
    func testRemoteProjectionAcceptsDesktopTurnsAndRetainsStablePromptIdentity() {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", apiKey: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        source.replaceRemoteTranscript([], sessionID: session)
        source.appendRemotePrompt(turnID: 10, text: "from desktop", images: [], origin: "Desktop")
        let id = source.model.messages.first?.id
        source.applyRemoteEvent(.turnStarted(turnId: 10))
        source.applyRemoteEvent(.textDelta(text: "first answer".clientTextDto))
        XCTAssertTrue(source.model.messages.contains { $0.text.contains("first answer") })
        source.applyRemoteEvent(.slashCommandResult(turnId: 99, display: "model changed".clientTextDto, isError: false))
        XCTAssertTrue(source.model.streaming, "out-of-band command output must not end the active desktop turn")
        XCTAssertTrue(source.model.items.contains { if case let .commandOutput(output) = $0 { return output.text == "model changed" }; return false })
        source.appendRemotePrompt(turnID: 10, text: "from desktop", images: [], origin: "Desktop")
        XCTAssertEqual(source.model.messages.filter { $0.role == .user }.count, 1)
        source.replaceRemoteTranscript([], sessionID: session)
        source.appendRemotePrompt(turnID: 10, text: "from desktop", images: [], origin: "Desktop")
        XCTAssertEqual(source.model.messages.first?.id, id)
        source.applyRemoteEvent(.turnStarted(turnId: 11))
        source.applyRemoteEvent(.textDelta(text: "second answer".clientTextDto))
        XCTAssertTrue(source.model.messages.contains { $0.text.contains("second answer") })
    }

    @MainActor
    func testRemoteHistoryImageHydrationPreservesRowsAndAccountInvalidationClearsData() {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", apiKey: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        let ref = session + ":m:0:0"
        source.replaceRemoteTranscript([MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "photo".clientTextDto)],
            images: [MessageImageDto(mediaType: "image/png", url: ref)])], sessionID: session)
        let ids = source.model.messages.map(\.id)
        source.hydrateRemoteImage(reference: ref, mediaType: "image/png", bytes: Data([1, 2, 3]))
        XCTAssertEqual(source.model.messages.map(\.id), ids)
        XCTAssertEqual(source.model.messages.flatMap(\.images).first?.url, "data:image/png;base64,AQID")
        let remote = RemoteConversationSource(hostID: host, sessionID: session, uid: "A")
        remote.model.messages = source.model.messages
        remote.invalidate()
        XCTAssertTrue(remote.model.messages.isEmpty)
        XCTAssertEqual(remote.phase, .closed)
        XCTAssertNil(remote.send("must not send"))
    }

    @MainActor
    func testManagerFilesBotReportsUnderTheirComputerAndDropsThoseOfAReplacedClient() {
        let manager = RemoteControlManager()
        let bot = RemoteBotSummaryFfi(botId: "bot-1", name: "Scribe", description: "", status: .running,
            waitingThreads: 1, lastActivityAtMs: nil)
        let chats = RemoteBotThreadsStateFfi(threads: [], error: nil)
        // A new manager is on its first client, whose reports carry generation 0.
        manager.receive(bots: .bots(bots: [bot]), hostID: host, generation: 0)
        manager.receive(botThreads: chats, hostID: host, botID: "bot-1", generation: 0)
        XCTAssertEqual(manager.botDirectory.bots(host: host), .bots(bots: [bot]))
        XCTAssertEqual(manager.botDirectory.chats(host: host, bot: "bot-1"), chats)
        XCTAssertEqual(manager.botDirectory.bots(host: "another-computer"), .unknown)

        manager.receive(bots: .unsupported, hostID: host, generation: 1)
        manager.receive(botThreads: RemoteBotThreadsStateFfi(threads: nil, error: nil), hostID: host, botID: "bot-1", generation: 1)
        XCTAssertEqual(manager.botDirectory.bots(host: host), .bots(bots: [bot]))
        XCTAssertEqual(manager.botDirectory.chats(host: host, bot: "bot-1"), chats)
    }

    @MainActor
    func testManagerWithoutAClientShowsNoBotsAndRefusesToStartAChat() async {
        let manager = RemoteControlManager()
        manager.hideBot(RemoteBotKey(hostID: host, botID: "bot-1"))
        XCTAssertNil(manager.botOfThread(session))
        XCTAssertEqual(manager.botDirectory.bots(host: host), .unknown)
        do {
            _ = try await manager.createBotChat(RemoteBotKey(hostID: host, botID: "bot-1"))
            XCTFail("a chat was started with no connection")
        } catch { XCTAssertEqual(error as? RemoteFfiError, .NotConnected) }
    }

    @MainActor
    func testRemoteProjectionCannotBootstrapLocalEngine() async {
        let source = EngineConversationSource(config: EngineConfig(apiBase: "", apiKey: "", model: "",
            appSandboxRoot: "", projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false), remoteProjection: true)
        do { try await source.prepare(); XCTFail("remote projection built a local engine") }
        catch { XCTAssertTrue(error is RemoteFfiError) }
    }
    #endif
}

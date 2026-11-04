import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
final class RemoteBotsTests: XCTestCase {
    private let host = "Hk3xQ9vR2mT7wL1pZ8cN4a"
    private let otherHost = "Zr8cN4aHk3xQ9vR2mT7wL1"
    private let bot = RemoteBotSummaryFfi(botId: "bot-1", name: "Scribe", description: "Keeps the notes", status: .idle,
        waitingThreads: 0, lastActivityAtMs: nil)

    private func chat(_ id: String, status: RemoteSessionStatusFfi = .idle) -> RemoteSessionSummaryFfi {
        RemoteSessionSummaryFfi(sessionId: id, title: "Chat " + id, projectLabel: "", status: status,
            pendingAction: nil, model: nil, viewers: 0)
    }

    private func screen(_ bots: RemoteBotsStateFfi, _ chats: RemoteBotThreadsStateFfi = RemoteBotThreadsStateFfi(threads: nil, error: nil)) -> RemoteBotScreen {
        RemoteBotScreen(bot: bot.botId, bots: bots, chats: chats)
    }

    // MARK: Bots and their chats, per computer

    func testDirectoryKeepsEachComputersBotsAndEachBotsChatsApart() {
        var directory = RemoteBotDirectory()
        XCTAssertEqual(directory.bots(host: host), .unknown)
        XCTAssertEqual(directory.chats(host: host, bot: bot.botId), RemoteBotThreadsStateFfi(threads: nil, error: nil))

        directory.set(.bots(bots: [bot]), host: host)
        directory.set(.unsupported, host: otherHost)
        directory.set(RemoteBotThreadsStateFfi(threads: [chat("a")], error: nil), host: host, bot: bot.botId)
        directory.set(RemoteBotThreadsStateFfi(threads: [], error: nil), host: otherHost, bot: bot.botId)

        XCTAssertEqual(directory.bots(host: host), .bots(bots: [bot]))
        XCTAssertEqual(directory.bots(host: otherHost), .unsupported)
        XCTAssertEqual(directory.chats(host: host, bot: bot.botId).threads?.map(\.sessionId), ["a"])
        XCTAssertEqual(directory.chats(host: otherHost, bot: bot.botId).threads?.count, 0)
        XCTAssertEqual(directory.chats(host: host, bot: "bot-2"), RemoteBotThreadsStateFfi(threads: nil, error: nil))
    }

    func testDirectoryForgetsEverythingWhenTheComputerChanges() {
        var directory = RemoteBotDirectory()
        directory.set(.bots(bots: [bot]), host: host)
        directory.set(RemoteBotThreadsStateFfi(threads: [chat("a")], error: nil), host: host, bot: bot.botId)
        directory.removeAll()
        XCTAssertEqual(directory, RemoteBotDirectory())
        XCTAssertEqual(directory.bots(host: host), .unknown)
    }

    // MARK: The Bot's screen

    func testScreenLoadsUntilTheComputerHasSaidWhichBotsItHas() {
        XCTAssertEqual(screen(.unknown).phase, .loading)
    }

    func testScreenIsGoneWhereTheBotCannotBeReached() {
        // A computer from before Bots, and one whose list lacks this Bot (its switch is off, or it was deleted).
        XCTAssertEqual(screen(.unsupported).phase, .gone)
        XCTAssertEqual(screen(.bots(bots: [])).phase, .gone)
        let other = RemoteBotSummaryFfi(botId: "bot-2", name: "Other", description: "", status: .running, waitingThreads: 2, lastActivityAtMs: 5)
        XCTAssertEqual(screen(.bots(bots: [other])).phase, .gone)
        // Listing the chats can say so before the list of Bots does.
        for code in ["unknown_session", "not_remote_enabled"] {
            let said = RemoteBotThreadsStateFfi(threads: [chat("a")], error: RemoteSessionErrorFfi(code: code, message: "no"))
            XCTAssertEqual(screen(.bots(bots: [bot]), said).phase, .gone, code)
        }
    }

    func testScreenIsReadyForABotTheComputerListsAndKeepsItsAnswerAfterAnotherKindOfFailure() {
        XCTAssertEqual(screen(.bots(bots: [bot])).phase, .ready(bot))
        let failed = RemoteBotThreadsStateFfi(threads: [chat("a")], error: RemoteSessionErrorFfi(code: "internal", message: "oops"))
        let kept = screen(.bots(bots: [bot]), failed)
        XCTAssertEqual(kept.phase, .ready(bot))
        XCTAssertTrue(kept.listFailed)
        XCTAssertEqual(kept.chats.map(\.sessionId), ["a"])
    }

    func testScreenTellsLoadingEmptyAndListedChatsApart() {
        let loading = screen(.bots(bots: [bot]))
        XCTAssertTrue(loading.loadingChats)
        XCTAssertFalse(loading.noChats)
        XCTAssertFalse(loading.listFailed)

        let empty = screen(.bots(bots: [bot]), RemoteBotThreadsStateFfi(threads: [], error: nil))
        XCTAssertFalse(empty.loadingChats)
        XCTAssertTrue(empty.noChats)

        // Newest first is the order the computer listed them in; the screen does not reorder.
        let listed = screen(.bots(bots: [bot]), RemoteBotThreadsStateFfi(threads: [chat("new"), chat("old")], error: nil))
        XCTAssertEqual(listed.chats.map(\.sessionId), ["new", "old"])
        XCTAssertFalse(listed.loadingChats)
        XCTAssertFalse(listed.noChats)
        XCTAssertFalse(listed.listFailed)
    }

    func testScreenOffersARetryWhenNothingCouldBeListed() {
        let failed = screen(.bots(bots: [bot]), RemoteBotThreadsStateFfi(threads: nil, error: RemoteSessionErrorFfi(code: "timeout", message: "slow")))
        XCTAssertTrue(failed.listFailed)
        XCTAssertFalse(failed.loadingChats, "a failed listing is not a pending one")
        XCTAssertFalse(failed.noChats, "nothing was listed, so nothing can be said to be empty")
    }

    // MARK: Where back goes

    func testChatReturnsToItsBotWhenTheBotIsKnown() {
        let title = "Untitled"
        XCTAssertEqual(RemoteRoute.stack(host: host, session: nil, title: title, bot: nil), [.host(host)])
        XCTAssertEqual(RemoteRoute.stack(host: host, session: "s1", title: title, bot: nil),
            [.host(host), .session(host: host, session: "s1", title: title)])
        XCTAssertEqual(RemoteRoute.stack(host: host, session: "s1", title: title, bot: bot.botId),
            [.host(host), .bot(host: host, bot: bot.botId), .session(host: host, session: "s1", title: title)])
        // A Bot with no chat to open is not pushed.
        XCTAssertEqual(RemoteRoute.stack(host: host, session: nil, title: title, bot: bot.botId), [.host(host)])
    }

    // MARK: Starting a chat

    func testCreateFailureTellsTheBotGoingAwayFromOtherRefusals() {
        XCTAssertEqual(RemoteBotCreateFailure(RemoteFfiError.Rpc(code: "unknown_session", detail: "")), .gone)
        XCTAssertEqual(RemoteBotCreateFailure(RemoteFfiError.Rpc(code: "not_remote_enabled", detail: "")), .gone)
        XCTAssertEqual(RemoteBotCreateFailure(RemoteFfiError.Rpc(code: "capacity", detail: "")), .capacity)
        XCTAssertEqual(RemoteBotCreateFailure(RemoteFfiError.Rpc(code: "forbidden", detail: "")), .forbidden)
        XCTAssertEqual(RemoteBotCreateFailure(RemoteFfiError.Rpc(code: "closed", detail: "")), .other)
        XCTAssertEqual(RemoteBotCreateFailure(RemoteFfiError.NotConnected), .other)
        XCTAssertEqual(RemoteBotCreateFailure(CancellationError()), .other)
        XCTAssertEqual(RemoteBotCreateFailure.gone.message, String(localized: "remote_bot_gone"))
    }

    // MARK: Who sent a prompt

    func testScheduledPromptNamesTheBotThatHandedItOver() {
        let named = RemotePromptOriginLabel.text(for: .scheduled, deviceName: nil) { "Scribe" }
        XCTAssertTrue(named.contains("Scribe"))
        XCTAssertNotEqual(named, String(localized: "remote_origin_scheduled"))
    }

    func testScheduledPromptWithoutABotNameKeepsTodaysLabel() {
        XCTAssertEqual(RemotePromptOriginLabel.text(for: .scheduled, deviceName: nil) { nil },
            String(localized: "remote_origin_scheduled"))
    }

    func testOtherOriginsKeepTodaysLabelsAndNeverAskForABot() {
        var asked = 0
        let desktop = RemotePromptOriginLabel.text(for: .desktop, deviceName: "ignored") { asked += 1; return "Scribe" }
        XCTAssertEqual(desktop, String(localized: "remote_origin_desktop"))
        XCTAssertEqual(RemotePromptOriginLabel.text(for: .device, deviceName: "Pixel") { asked += 1; return "Scribe" }, "Pixel")
        XCTAssertEqual(RemotePromptOriginLabel.text(for: .device, deviceName: nil) { asked += 1; return "Scribe" },
            String(localized: "remote_origin_other_device"))
        XCTAssertEqual(asked, 0)
    }

    func testBotNameIsAskedOncePerPromptWhateverTheAnswer() {
        var names = RemotePromptBotNames()
        var asked: [UInt64] = []
        func lookup(_ turn: UInt64, _ answer: String?) -> String? {
            names.name(forTurn: turn) { asked.append(turn); return answer }
        }
        XCTAssertEqual(lookup(1, "Scribe"), "Scribe")
        XCTAssertEqual(lookup(1, "changed"), "Scribe", "the first answer stands")
        XCTAssertNil(lookup(2, nil))
        XCTAssertNil(lookup(2, "late"), "no name is an answer too")
        XCTAssertEqual(asked, [1, 2])

        // A replaced transcript may carry other records: it is asked again.
        names.removeAll()
        XCTAssertEqual(lookup(2, "late"), "late")
        XCTAssertEqual(asked, [1, 2, 2])
    }
}
#endif

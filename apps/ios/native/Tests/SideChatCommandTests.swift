import XCTest
@testable import LingxiCode

final class SideChatCommandTests: XCTestCase {
    func testEmptySideCommandCreatesWithoutInput() {
        XCTAssertEqual(SideChatCommand.input(" /side \n"), "")
    }
    func testSideInputPreservesInternalWhitespace() {
        XCTAssertEqual(SideChatCommand.input("/side  explain\nthis"), "explain\nthis")
        XCTAssertEqual(SideChatCommand.input("/side\tquestion"), "question")
        XCTAssertEqual(SideChatCommand.input("/side\rquestion"), "question")
        XCTAssertEqual(SideChatCommand.input("/side\u{3000}question"), "question")
    }
    func testOnlyExactCommandBoundaryMatches() {
        XCTAssertNil(SideChatCommand.input("/sideways"))
        XCTAssertNil(SideChatCommand.input("ask /side question"))
        XCTAssertNil(SideChatCommand.input("/Side question"))
    }

    @MainActor
    func testTemporaryTranscriptDisclosuresStayInMemory() {
        let suite = "side-chat-disclosures-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = ConversationModel(disclosureDefaults: defaults, persistDisclosures: false)
        model.activeSessionId = "temporary-side"
        model.toggleTranscriptDisclosure("side-tool-detail")
        XCTAssertTrue(model.expandedToolCalls.contains("side-tool-detail"))
        XCTAssertNil(defaults.data(forKey: "conversation.disclosures.v1"))
    }
}

#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

@MainActor
final class SideChatConversationSourceTests: XCTestCase {
    private func makeSource(root: URL) -> EngineConversationSource {
        EngineConversationSource(config: EngineConfig(apiBase: "https://api.anthropic.com", apiKey: "", model: "",
            appSandboxRoot: root.path, projectCwd: nil, sessionMode: .code, visionDelegationEnabled: true), memoryOnly: true)
    }

    func testInitialPromptWaitsForForegroundCreationAdmission() async {
        await assertInitialPromptAdmission(background: false, resume: false)
    }

    func testCreationCompletingInBackgroundRetainsInitialPromptWithoutStartingWork() async {
        await assertInitialPromptAdmission(background: true, resume: false)
    }

    func testBackgroundThenForegroundDuringCreationStillRetainsInitialPrompt() async {
        await assertInitialPromptAdmission(background: true, resume: true)
    }

    private func assertInitialPromptAdmission(background: Bool, resume: Bool) async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let parent = makeSource(root: root)
        let child = makeSource(root: root)
        var commands: [ClientCommand] = []
        child.setCommandSubmitterForTesting { commands.append($0) }
        let started = expectation(description: "creation is suspended before returning its child")
        let finished = expectation(description: "creation completes")
        var admission: CheckedContinuation<SideChatEntry, Never>?
        let collection = SideChatCollection(parent: parent, createEntry: { _ in
            await withCheckedContinuation { continuation in
                admission = continuation
                started.fulfill()
            }
        })
        collection.create(input: "first side question") { success in
            XCTAssertTrue(success)
            finished.fulfill()
        }
        await fulfillment(of: [started], timeout: 2)
        if background { collection.cancelCurrentTurns() }
        if resume { collection.handleForeground() }
        XCTAssertTrue(commands.isEmpty)
        admission?.resume(returning: SideChatEntry(source: child, id: "delayed-side"))
        await fulfillment(of: [finished], timeout: 2)
        for _ in 0..<20 { await Task.yield() }
        let prompts = commands.filter { if case .sendPrompt = $0 { return true }; return false }
        XCTAssertEqual(prompts.count, background ? 0 : 1)
        XCTAssertEqual(collection.entries.first?.draft, background ? "first side question" : "")
        XCTAssertEqual(child.model.streaming, !background)
        XCTAssertFalse(collection.creating)
        child.clearEndedSideChat()
    }

    func testNestedSideRejectsWithoutAppendingOrSubmitting() {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        source.setCommandSubmitterForTesting { _ in XCTFail("must reject before native submission") }
        XCTAssertNil(source.send("/side nested"))
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertEqual(source.model.error?.kind, .rejected)
    }

    func testPendingCreationBlocksHostReplacement() {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        XCTAssertFalse(source.hasLiveSideChats)
        source.sideChats.creating = true
        XCTAssertTrue(source.hasLiveSideChats)
        source.sideChats.creating = false
        XCTAssertFalse(source.hasLiveSideChats)
    }

    func testBackgroundCancelsHiddenSideTurnWithoutEndingOrClearingDraft() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let parent = makeSource(root: root)
        let child = makeSource(root: root)
        var commands: [ClientCommand] = []
        child.setCommandSubmitterForTesting { commands.append($0) }
        child.applyForTesting(.turnStarted(turnId: 77))
        let entry = SideChatEntry(source: child, id: "hidden-side")
        entry.draft = "keep this unsent draft"
        parent.sideChats.entries = [entry]
        parent.sideChats.selectedID = nil
        parent.sideChats.cancelCurrentTurns()
        for _ in 0..<20 { await Task.yield() }
        XCTAssertTrue(commands.contains { command in
            if case let .cancel(turnID) = command { return turnID == 77 }
            return false
        })
        XCTAssertEqual(parent.sideChats.entries.count, 1)
        XCTAssertEqual(entry.draft, "keep this unsent draft")
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("durable-conversation-turn.json").path))
    }

    func testFailedSubmissionRestoresDraftWhileSideViewIsHidden() {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let source = makeSource(root: root)
        source.setCommandSubmitterForTesting { _ in }
        let entry = SideChatEntry(source: source, id: "hidden-side")
        XCTAssertNotNil(source.send("retry this prompt"))
        source.applyForTesting(.error(kind: .rejected, message: "not admitted"))
        XCTAssertEqual(entry.draft, "retry this prompt")
        XCTAssertFalse(source.model.streaming)
    }

    func testEndClearsPendingDraftRecoveryAndRejectsLateCallbacks() {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        source.setCommandSubmitterForTesting { _ in }
        let entry = SideChatEntry(source: source, id: "ended-side")
        XCTAssertNotNil(source.send("do not restore after End"))
        source.clearEndedSideChat()
        source.applyForTesting(.error(kind: .rejected, message: "late failure"))
        XCTAssertTrue(entry.draft.isEmpty)
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertTrue(source.model.messageDetails.isEmpty)
        XCTAssertNil(source.model.error)
        XCTAssertNil(source.send("late send"))
    }

    func testSideVoiceCannotEnterFlowOrRealtimeMode() {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        let entry = SideChatEntry(source: source, id: "dictation-only-side")
        entry.voice.startFlow(source: source)
        XCTAssertNil(entry.voice.mode)
        XCTAssertFalse(entry.voice.isPresented)
    }

    func testSubmissionRecoveryDoesNotRetainRemovedEntry() {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        weak var released: SideChatEntry?
        do {
            let entry = SideChatEntry(source: source, id: "short-lived-side")
            released = entry
        }
        XCTAssertNil(released)
    }

    func testInitialTurnAdoptsIndependentCorrelatorAndDoesNotWriteCheckpoint() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let source = makeSource(root: root)
        source.setCommandSubmitterForTesting { _ in }
        source.applyForTesting(.turnStarted(turnId: 41))
        source.applyForTesting(.textDelta(text: "side response"))
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 41)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("durable-conversation-turn.json").path))
        source.startNewConversation()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 41)
    }
}
#endif

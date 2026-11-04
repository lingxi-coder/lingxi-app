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

    func testSideInputSourceUnitsPreserveRawSurrogateAfterCommandTrim() {
        let sourceUnits = Array("  /side \t".utf16) + [0xD800] + Array(" x \n".utf16)
        let display = String(decoding: sourceUnits, as: UTF16.self)

        XCTAssertEqual(SideChatCommand.input(display), "\u{FFFD} x")
        XCTAssertEqual(
            SideChatCommand.inputSourceCodeUnits(raw: display, sourceCodeUnits: sourceUnits) ?? [],
            [UInt16(0xD800), 0x20, 0x78]
        )
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
import UIKit

@MainActor
final class SideChatConversationSourceTests: XCTestCase {
    private func makeSource(root: URL) -> EngineConversationSource {
        EngineConversationSource(config: EngineConfig(apiBase: "https://api.anthropic.com", model: "",
            appSandboxRoot: root.path, projectCwd: nil, sessionMode: .code, visionDelegationEnabled: true), memoryOnly: true)
    }

    private func installHelpCommand(on source: EngineConversationSource) {
        source.applyForTesting(.slashCommandCatalog(commands: [SlashCommandDto(
            name: "help".clientTextDto, description: "Help".clientTextDto, source: "builtin", aliases: [],
            argumentHint: nil, argumentNames: [], menuDescription: nil, hidden: false)]))
    }

    func testOpeningEmptySheetCreatesOneEditableChildAndReopeningPreservesIt() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let parent = makeSource(root: root)
        let child = makeSource(root: root)
        var creations = 0
        let created = expectation(description: "empty sheet creates a child")
        let collection = SideChatCollection(parent: parent, createEntry: { _ in
            creations += 1
            created.fulfill()
            return SideChatEntry(source: child, id: "empty-sheet-child")
        })
        collection.prepareForPresentation()
        collection.prepareForPresentation()
        await fulfillment(of: [created], timeout: 2)
        for _ in 0..<20 { await Task.yield() }
        XCTAssertEqual(creations, 1)
        XCTAssertEqual(collection.selectedID, "empty-sheet-child")
        collection.entries.first?.draft = "keep the child draft"
        collection.prepareForPresentation()
        XCTAssertEqual(creations, 1)
        XCTAssertEqual(collection.entries.first?.draft, "keep the child draft")
        XCTAssertFalse(child.model.streaming)
    }

    func testInitialPromptWaitsForForegroundCreationAdmission() async {
        await assertInitialPromptAdmission(background: false, resume: false)
    }

    func testInitialSidePromptPreservesRawSourceUnits() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let parent = makeSource(root: root)
        let child = makeSource(root: root)
        var commands: [ClientCommand] = []
        child.setCommandSubmitterForTesting { commands.append($0) }
        let sourceUnits: [UInt16] = [0xD800, 0x78]
        let created = expectation(description: "side child admits exact source prompt")
        let collection = SideChatCollection(parent: parent, createEntry: { _ in
            SideChatEntry(source: child, id: "source-preserving-child")
        })

        collection.create(input: "\u{FFFD}x", inputSourceCodeUnits: sourceUnits) { success in
            XCTAssertTrue(success)
            created.fulfill()
        }
        await fulfillment(of: [created], timeout: 2)
        for _ in 0..<20 { await Task.yield() }

        guard case let .sendPrompt(text, _, _, _, _) = commands.first else {
            return XCTFail("side child must submit its initial prompt")
        }
        XCTAssertEqual(text.codeUnits, sourceUnits)
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

    func testNestedSideRejectsWithoutAppendingOrSubmitting() async {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        source.setCommandSubmitterForTesting { _ in XCTFail("must reject before native submission") }
        let rejectedToken = await source.send("/side nested")
        XCTAssertNil(rejectedToken)
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

    func testFailedSlashSubmissionRemainsAvailableWhileSideViewIsHidden() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let source = makeSource(root: root)
        installHelpCommand(on: source)
        source.setCommandSubmitterForTesting { _ in }
        let entry = SideChatEntry(source: source, id: "hidden-side")
        let sourceUnits = Array("/help ".utf16) + [0xD800, 0x78]
        let acceptedToken = await source.send(
            "/help \u{FFFD}x",
            sourceCodeUnits: sourceUnits,
            images: [],
            selectedSlashCommand: nil
        )
        let token = try XCTUnwrap(acceptedToken)
        source.applyForTesting(.slashCommandResult(turnId: token.clientTurnId, display: "failed".clientTextDto, isError: true))
        let pending = try XCTUnwrap(entry.unsentPrompts.first)
        XCTAssertTrue(entry.restoreUnsent(pending))
        XCTAssertEqual(entry.draft, "/help \u{FFFD}x")
        XCTAssertEqual(entry.draftSourceCodeUnits, sourceUnits)
        XCTAssertFalse(source.model.streaming)
    }

    func testFailedSlashInputNeverMixesWithAnEditedDraftOrAttachment() async throws {
        let image = UIGraphicsImageRenderer(size: CGSize(width: 2, height: 2)).image { context in
            UIColor.red.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 2, height: 2))
        }
        let encoded = try XCTUnwrap(image.pngData()).base64EncodedString()
        let imageRef = ImageRefDto(mediaType: "image/png", base64: encoded)
        for laterImage in [false, true] {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            let source = makeSource(root: root)
            installHelpCommand(on: source)
            source.setCommandSubmitterForTesting { _ in }
            let entry = SideChatEntry(source: source, id: "hidden-side")
            let units = Array("/help ".utf16) + [0xD800, 0x61]
            let admitted = await source.send("/help \u{FFFD}a", sourceCodeUnits: units, images: [imageRef], selectedSlashCommand: nil)
            let token = try XCTUnwrap(admitted)
            if laterImage {
                entry.attachment = ComposerAttachment(image: image, width: 2, height: 2, mediaType: "image/png", base64: "new")
            } else {
                entry.draft = "new text"
            }
            source.applyForTesting(.slashCommandResult(turnId: token.clientTurnId, display: "failed".clientTextDto, isError: true))
            let pending = try XCTUnwrap(entry.unsentPrompts.first)
            XCTAssertFalse(entry.restoreUnsent(pending))
            XCTAssertEqual(entry.draft, laterImage ? "" : "new text")
            XCTAssertEqual(entry.attachment?.base64, laterImage ? "new" : nil)
            entry.draft = ""
            entry.attachment = nil
            XCTAssertTrue(entry.restoreUnsent(pending))
            XCTAssertEqual(entry.draft, "/help \u{FFFD}a")
            XCTAssertEqual(entry.draftSourceCodeUnits, units)
            XCTAssertEqual(entry.attachment?.base64, encoded)
        }
    }

    func testEndClearsPendingDraftRecoveryAndRejectsLateCallbacks() async {
        let source = makeSource(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        source.setCommandSubmitterForTesting { _ in }
        let entry = SideChatEntry(source: source, id: "ended-side")
        let acceptedToken = await source.send("do not restore after End")
        XCTAssertNotNil(acceptedToken)
        source.clearEndedSideChat()
        source.applyForTesting(.error(kind: .rejected, message: "late failure".clientTextDto))
        XCTAssertTrue(entry.draft.isEmpty)
        XCTAssertTrue(source.model.messages.isEmpty)
        XCTAssertTrue(source.model.messageDetails.isEmpty)
        XCTAssertNil(source.model.error)
        let rejectedToken = await source.send("late send")
        XCTAssertNil(rejectedToken)
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
        source.applyForTesting(.textDelta(text: "side response".clientTextDto))
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 41)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("durable-conversation-turn.json").path))
        source.startNewConversation()
        XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, 41)
    }
}
#endif

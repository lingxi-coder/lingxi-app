import XCTest
@testable import LingxiCode

@MainActor
final class ConversationDisclosureStoreTests: XCTestCase {
    func testRecreatedModelRestoresSessionDisclosuresAndKeepsSessionsSeparate() throws {
        let name = "disclosure-test-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let first = ConversationModel(disclosureDefaults: defaults)
        first.activeSessionId = "session-a"
        first.toggleTranscriptDisclosure("timeline-tools:run:first-tool")
        first.toggleTranscriptDisclosure("assistant:message:stable-id")
        let recreated = ConversationModel(disclosureDefaults: defaults)
        recreated.activeSessionId = "session-a"
        XCTAssertEqual(recreated.expandedToolCalls, first.expandedToolCalls)
        recreated.activeSessionId = "session-b"
        XCTAssertTrue(recreated.expandedToolCalls.isEmpty)
        recreated.toggleTranscriptDisclosure("other-session-tool")
        recreated.activeSessionId = "session-a"
        XCTAssertEqual(recreated.expandedToolCalls, first.expandedToolCalls)
        recreated.toggleTranscriptDisclosure("assistant:message:stable-id")
        let third = ConversationModel(disclosureDefaults: defaults)
        third.activeSessionId = "session-a"
        XCTAssertEqual(third.expandedToolCalls, ["timeline-tools:run:first-tool"])
    }

    func testPersistenceIsBoundedAndDoesNotSaveUnnamedSession() throws {
        let name = "disclosure-bounds-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let store = ConversationDisclosureStore(defaults: defaults)
        store.save(["unsaved"], sessionID: "")
        XCTAssertTrue(store.load(sessionID: "").isEmpty)
        for index in 0...ConversationDisclosureStore.maximumSessions {
            store.save(["tool"], sessionID: "session-\(index)")
        }
        XCTAssertTrue(store.load(sessionID: "session-0").isEmpty)
        store.save(Set((0..<1000).map { "tool-\($0)" }), sessionID: "latest")
        XCTAssertEqual(store.load(sessionID: "latest").count, ConversationDisclosureStore.maximumExpandedRows)
    }
}

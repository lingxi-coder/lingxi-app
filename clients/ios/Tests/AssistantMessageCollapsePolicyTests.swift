import XCTest
@testable import LingxiCode

final class AssistantMessageCollapsePolicyTests: XCTestCase {
    func testLLMActivityStateDistinguishesHiddenRunningStoppingAndPaused() {
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: false,
                streaming: false,
                isCancelling: false
            ),
            .hidden
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: true,
                streaming: true,
                isCancelling: false
            ),
            .running
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: false,
                streaming: true,
                isCancelling: false
            ),
            .running
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: true,
                streaming: true,
                isCancelling: true
            ),
            .stopping
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: true,
                streaming: false,
                isCancelling: false
            ),
            .paused
        )
    }

    func testShortReplyStaysExpanded() {
        XCTAssertFalse(AssistantMessageCollapsePolicy.shouldCollapse("A concise reply."))
    }

    func testLongUnbrokenReplyCanCollapse() {
        XCTAssertTrue(
            AssistantMessageCollapsePolicy.shouldCollapse(String(repeating: "长", count: 700))
        )
    }

    func testManyShortLinesCanCollapse() {
        let reply = Array(repeating: "step", count: 22).joined(separator: "\n")
        XCTAssertTrue(AssistantMessageCollapsePolicy.shouldCollapse(reply))
    }

    func testToolIconsUseDifferentActionsForDifferentVerbs() {
        let read = ConversationToolHeader(
            verb: .read,
            label: "Read",
            primary: "README.md",
            qualifier: nil,
            count: nil,
            subLine: nil,
            title: "Read(README.md)"
        )
        let shell = ConversationToolHeader(
            verb: .shell,
            label: "Shell",
            primary: nil,
            qualifier: nil,
            count: nil,
            subLine: nil,
            title: "Shell"
        )

        XCTAssertEqual(ToolDisplayText.icon(header: read, tool: "Read"), .search)
        XCTAssertEqual(ToolDisplayText.icon(header: shell, tool: "Shell"), .terminal)
        XCTAssertNotEqual(
            ToolDisplayText.icon(header: read, tool: "Read"),
            ToolDisplayText.icon(header: shell, tool: "Shell")
        )
    }

    func testLegacyToolNamesStillGetSpecificIcons() {
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "WebSearch"), .globe)
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "bash"), .terminal)
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "Write"), .edit)
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "Search documentation"), .book)
    }

    func testStructuredToolExpansionKeysAreScopedToTheirMessage() {
        let firstMessage = UUID()
        let secondMessage = UUID()
        let firstKey = ConversationToolExpansionKey.structured(
            messageID: firstMessage,
            toolID: "read-1"
        )
        let secondKey = ConversationToolExpansionKey.structured(
            messageID: secondMessage,
            toolID: "read-1"
        )

        XCTAssertNotEqual(firstKey, secondKey)
        XCTAssertEqual(
            ConversationToolExpansionKey.structuredToolIDs(
                in: [firstKey, secondKey, "standalone-tool"],
                messageID: firstMessage
            ),
            ["read-1"]
        )
    }
}

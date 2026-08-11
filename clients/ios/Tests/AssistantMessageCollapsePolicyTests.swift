import XCTest
@testable import LingxiCode

final class AssistantMessageCollapsePolicyTests: XCTestCase {
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
}

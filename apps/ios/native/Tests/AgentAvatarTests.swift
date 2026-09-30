import XCTest
@testable import LingxiCode

final class AgentAvatarTests: XCTestCase {
    func testDesktopUTF16IdentityVectors() {
        let vectors: [(String, Int)] = [("", 0), ("main", 13), ("agent-review-42", 13), ("搜索-agent", 20), ("🧠-review", 23), (String(repeating: "a", count: 128), 9)]
        for (id, expected) in vectors { XCTAssertEqual(agentAvatarIndex(id), expected, id) }
        XCTAssertEqual(Set((0..<300).map { agentAvatarIndex("agent-\($0)") }).count, 28)
    }
}

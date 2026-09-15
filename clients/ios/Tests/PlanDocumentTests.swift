import XCTest
@testable import LingxiCode

final class PlanDocumentTests: XCTestCase {
    func testDocumentPreservesSurroundingText() {
        XCTAssertEqual(PlanDocument.segments("Before\n<proposed_plan>\n# Title\nBody\n</proposed_plan>\nAfter"), [
            .text("Before\n"), .plan(.init(markdown: "# Title\nBody", isWriting: false)), .text("After")
        ])
    }
    func testStreamingAndCodeExample() {
        XCTAssertEqual(PlanDocument.segments("<proposed_plan>\n# Partial"), [.plan(.init(markdown: "# Partial", isWriting: true))])
        let example = "```xml\n<proposed_plan>\nExample\n</proposed_plan>\n```"
        XCTAssertEqual(PlanDocument.segments(example), [.text(example)])
    }
    func testFenceRequiresMatchingMarkerLengthAndEmptySuffix() {
        for (opener, falseCloser) in [("````markdown", "```"), ("```markdown", "~~~"), ("~~~markdown", "```"), ("```", "```still-code")] {
            let marker = opener.first!
            let closer = String(repeating: String(marker), count: 5)
            let example = "\(opener)\n\(falseCloser)\n<proposed_plan>\nExample\n</proposed_plan>\n\(closer)"
            XCTAssertEqual(PlanDocument.segments(example), [.text(example)])
            XCTAssertEqual(PlanDocument.segments(example + "\n<proposed_plan>\n# Actual\n</proposed_plan>"), [
                .text(example + "\n"), .plan(.init(markdown: "# Actual", isWriting: false))
            ])
        }
    }
    func testPlanMayContainFencedClosingTags() {
        let markdown = "# Actual\n````xml\n```\n</proposed_plan>\n````"
        XCTAssertEqual(PlanDocument.segments("<proposed_plan>\n" + markdown + "\n</proposed_plan>"), [.plan(.init(markdown: markdown, isWriting: false))])
    }
    func testExitPlanModeUsesOnlyRealPlan() {
        XCTAssertEqual(PlanDocument.tool("ExitPlanMode", json: "{\"plan\":\"# Real plan\"}")?.markdown, "# Real plan")
        XCTAssertNil(PlanDocument.tool("TodoWrite", json: "{\"plan\":\"Not a document\"}"))
        XCTAssertNil(PlanDocument.tool("ExitPlanMode", json: "{}"))
    }
}

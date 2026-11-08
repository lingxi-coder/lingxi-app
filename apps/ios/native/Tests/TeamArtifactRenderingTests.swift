import Foundation
import XCTest
@testable import LingxiCode

final class TeamArtifactRenderingTests: XCTestCase {
    func testLinkPolicyAllowsOnlyWebLinks() {
        for ok in ["https://example.com/a", "HTTP://example.com", "http://example.com:8080/x?y=1"] {
            XCTAssertTrue(TeamArtifactLinkPolicy.allows(URL(string: ok)!), ok)
        }
        for bad in ["javascript:alert(1)", "tel:123", "mailto:a@b.c", "file:///etc/passwd", "lingxi://open", "sms:1", "https:///nohost", "data:text/html,hi"] {
            if let url = URL(string: bad) { XCTAssertFalse(TeamArtifactLinkPolicy.allows(url), bad) }
        }
    }

    func testArtifactMarkdownParsesIntoBlocks() {
        let body = """
        # Plan

        Some **bold** text with a [link](https://example.com).

        - [ ] open item
        - [x] done item
        - plain bullet
        1. first

        ```swift
        let x = 1
        ```
        """
        let blocks = AIText.parseBlocks(body)
        XCTAssertEqual(blocks.count, 2)
        guard case let .text(lines) = blocks[0] else { return XCTFail("expected text block") }
        XCTAssertEqual(lines.first, .heading(level: 1, "Plan"))
        XCTAssertTrue(lines.contains(.task(checked: false, "open item")))
        XCTAssertTrue(lines.contains(.task(checked: true, "done item")))
        XCTAssertTrue(lines.contains(.bullet("plain bullet")))
        XCTAssertTrue(lines.contains(.numbered(marker: "1.", "first")))
        XCTAssertEqual(blocks[1], .code("let x = 1", lang: "swift"))
    }

    func testInlineMarkdownRemovesMarkersAndKeepsImagesInert() {
        let rendered = AIText.parseInline("**bold** ![pic](https://tracker.example/p.png)", size: 17)
        let plain = String(rendered.characters)
        XCTAssertFalse(plain.contains("**"))
        XCTAssertFalse(plain.contains("tracker.example"))
    }
}

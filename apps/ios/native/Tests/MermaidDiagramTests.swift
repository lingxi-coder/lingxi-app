import XCTest
@testable import LingxiCode

@MainActor
final class MermaidDiagramTests: XCTestCase {
    func testCompletedMermaidFenceRendersAndPreservesSource() {
        let source = "flowchart TB\n    A[\"中文<br/>第二行\"] <--> B\n"
        XCTAssertEqual(AIText.parseBlocks("```mermaid\n" + source + "```"), [.mermaid(String(source.dropLast()))])
        XCTAssertEqual(AIText.parseBlocks("```MERMAID\nA --> B\n```"), [.mermaid("A --> B")])
    }

    func testIncompleteStreamingFenceRemainsSourceCode() {
        XCTAssertEqual(AIText.parseBlocks("```mermaid\nflowchart TB\nA -->"), [.code("flowchart TB\nA -->", lang: "mermaid")])
        XCTAssertEqual(AIText.parseBlocks("```text\nflowchart TB\n```"), [.code("flowchart TB", lang: "text")])
        XCTAssertEqual(AIText.parseBlocks("````mermaid\nA --> B\n```"), [.code("A --> B\n```", lang: "mermaid")])
        XCTAssertEqual(AIText.parseBlocks("```mermaid\nA --> B\n```suffix"), [.code("A --> B\n```suffix", lang: "mermaid")])
        XCTAssertEqual(AIText.parseBlocks("~~~mermaid\r\nA --> B\r\n~~~"), [.mermaid("A --> B")])
    }

    func testOfflineRendererIsInTheApplicationBundle() throws {
        let file = try XCTUnwrap(Bundle.main.url(forResource: "mermaid", withExtension: "html"))
        let html = try String(contentsOf: file, encoding: .utf8)
        XCTAssertTrue(html.contains("connect-src 'none'"))
        XCTAssertFalse(html.contains("<script src="))
    }
}

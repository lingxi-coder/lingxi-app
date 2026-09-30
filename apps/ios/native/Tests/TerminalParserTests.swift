import XCTest
@testable import LingxiCode

@MainActor
final class TerminalParserTests: XCTestCase {
    func testAnsiParserAppliesForegroundAndReset() {
        var buffer = TerminalScreenBuffer(maxScrollback: 16)
        var parser = TerminalANSIParser()

        parser.consume(text: "plain \u{001B}[31mred\u{001B}[0m done", buffer: &buffer)

        XCTAssertEqual(buffer.lines.count, 1)
        XCTAssertEqual(buffer.lines[0].text, "plain red done")
        let cells = buffer.lines[0].cells
        XCTAssertEqual(cells[6].style.foreground, .named(.red))
        XCTAssertNil(cells.last?.style.foreground)
    }

    func testAnsiParserHandlesCursorMovementAndClearLine() {
        var buffer = TerminalScreenBuffer(maxScrollback: 16)
        var parser = TerminalANSIParser()

        parser.consume(text: "hello\r\u{001B}[2Kbye", buffer: &buffer)

        XCTAssertEqual(buffer.lines[0].text, "bye")
    }

    func testFragmentedUtf8DecodesAcrossChunks() {
        var decoder = TerminalUTF8Decoder()

        let first = decoder.append(Data([0xE4, 0xBD]))
        let second = decoder.append(Data([0xA0, 0xE5, 0xA5, 0xBD]))

        XCTAssertEqual(first, "")
        XCTAssertEqual(second, "你好")
    }
}

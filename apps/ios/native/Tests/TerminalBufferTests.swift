import XCTest
@testable import LingxiCode

@MainActor
final class TerminalBufferTests: XCTestCase {
    func testBufferTrimsScrollback() {
        var buffer = TerminalScreenBuffer(maxScrollback: 3)
        let style = TerminalTextStyle()

        for value in ["1", "2", "3", "4"] {
            buffer.put(Character(value), style: style)
            buffer.newline()
        }

        XCTAssertEqual(buffer.lines.count, 3)
        XCTAssertEqual(buffer.lines.map(\.text), ["3", "4", ""])
    }

    func testBufferSupportsBackspaceAndOverwrite() {
        var buffer = TerminalScreenBuffer(maxScrollback: 8)
        let style = TerminalTextStyle()

        buffer.put("a", style: style)
        buffer.put("b", style: style)
        buffer.backspace()
        buffer.put("c", style: style)

        XCTAssertEqual(buffer.lines[0].text, "ac")
    }

    func testHistoryNavigationReturnsPreviousAndNextCommand() {
        let descriptor = TerminalRuntimeDescriptor(
            config: nil,
            workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/demo", displayName: "Demo")
        )
        let model = TerminalSessionModel(descriptor: descriptor, client: FakeTerminalRuntimeClient())
        model.history = ["ls", "pwd", "git status"]

        model.previousHistory()
        XCTAssertEqual(model.inputText, "git status")
        model.previousHistory()
        XCTAssertEqual(model.inputText, "pwd")
        model.nextHistory()
        XCTAssertEqual(model.inputText, "git status")
        model.nextHistory()
        XCTAssertEqual(model.inputText, "")
    }
}

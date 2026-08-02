import XCTest

@testable import LingxiCode

@MainActor
final class ConversationNavigationTests: XCTestCase {
    func testTerminalRouteCwdParsesGuestAbsoluteAndWorkspaceRelativePaths() {
        XCTAssertEqual(TerminalRouteCwd("/workspace/project-a/src"), .guestPath("/workspace/project-a/src"))
        XCTAssertEqual(TerminalRouteCwd("src/features"), .workspaceRelative("src/features"))
        XCTAssertNil(TerminalRouteCwd("   "))
        XCTAssertNil(TerminalRouteCwd(nil))
    }

    func testOpenTerminalFromShellRequestPreservesTypedCwd() {
        let navigation = AppNavigationModel()
        let request = ConversationShellLaunchRequest(
            taskId: "task-1",
            command: "npm test",
            cwd: .guestPath("/workspace/project-a/app")
        )

        navigation.openTerminal(shellRequest: request, projectID: "project-a", sessionID: "shell-task-1")

        guard case let .terminal(sessionID, initialCommand, projectID, requestedCwd)? = navigation.path.last else {
            return XCTFail("expected terminal route")
        }
        XCTAssertEqual(sessionID, "shell-task-1")
        XCTAssertEqual(initialCommand, "npm test")
        XCTAssertEqual(projectID, "project-a")
        XCTAssertEqual(requestedCwd, .guestPath("/workspace/project-a/app"))
    }

    func testShellCardLaunchRequestUsesTypedCwdForTerminalButton() {
        let card = ConversationShellCard(
            sessionId: "session-a",
            turnId: 3,
            taskId: "task-7",
            command: "ls",
            cwd: "docs"
        )

        let request = ConversationShellLaunchRequest(
            taskId: card.taskId,
            command: card.command,
            cwd: card.requestedCwd
        )

        XCTAssertEqual(request.cwd, .workspaceRelative("docs"))
    }
}

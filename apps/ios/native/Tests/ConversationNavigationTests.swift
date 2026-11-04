import XCTest

@testable import LingxiCode

@MainActor
final class ConversationNavigationTests: XCTestCase {
    func testTerminalRouteCwdParsesGuestAbsoluteAndWorkspaceRelativePaths() {
        XCTAssertEqual(TerminalRouteCwd("/home/lingxi/lingxi/project-a/src"), .guestPath("/home/lingxi/lingxi/project-a/src"))
        XCTAssertEqual(TerminalRouteCwd("src/features"), .workspaceRelative("src/features"))
        XCTAssertNil(TerminalRouteCwd("   "))
        XCTAssertNil(TerminalRouteCwd(nil))
    }

    func testOpenTerminalFromShellRequestPreservesTypedCwd() {
        let navigation = AppNavigationModel()
        let request = ConversationShellLaunchRequest(
            taskId: "task-1",
            command: "npm test",
            cwd: .guestPath("/home/lingxi/lingxi/project-a/app")
        )

        navigation.openTerminal(shellRequest: request, projectID: "project-a", sessionID: "shell-task-1")

        guard case let .terminal(sessionID, initialCommand, projectID, requestedCwd)? = navigation.path.last else {
            return XCTFail("expected terminal route")
        }
        XCTAssertEqual(sessionID, "shell-task-1")
        XCTAssertEqual(initialCommand, "npm test")
        XCTAssertEqual(projectID, "project-a")
        XCTAssertEqual(requestedCwd, .guestPath("/home/lingxi/lingxi/project-a/app"))
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

    func testSessionDetailsRouteUsesTheCurrentSessionID() {
        let navigation = AppNavigationModel()

        navigation.openSessionDetails(sessionID: "session-details-1")

        XCTAssertEqual(navigation.path.last, .sessionDetails(sessionID: "session-details-1"))
        XCTAssertEqual(navigation.path.last?.id, "session-details:session-details-1")
    }

    func testReopeningProjectsFromAProjectPageGoesBackToTheList() {
        let navigation = AppNavigationModel()

        navigation.openProjects()
        navigation.openProject("project-a")
        navigation.openProjects()
        XCTAssertEqual(navigation.path, [.projects])

        navigation.openProject("project-a")
        navigation.openProject("project-b")
        XCTAssertEqual(navigation.path, [.projects, .project(id: "project-b")])
    }

    func testAdmittedImportFinishesWithoutReplacingNewerConversationOrPage() async {
        let navigation = AppNavigationModel()
        navigation.openApps()
        var selections = ConversationNavigationIntents<SessionMode>()
        let revision = navigation.presentationRevision
        let generation = selections.generation
        let started = expectation(description: "import is suspended")
        var release: CheckedContinuation<Void, Never>?
        var imported = false
        let task = Task { @MainActor in
            await withCheckedContinuation { continuation in
                release = continuation
                started.fulfill()
            }
            imported = true
            if navigation.mayPresentImportedApp(capturedRevision: revision), selections.isCurrent(generation) {
                navigation.openLocalApp("imported-app")
            }
        }
        await fulfillment(of: [started], timeout: 1)
        _ = selections.submit(.chat, whileBusy: false)
        navigation.path.removeAll()
        navigation.openApps() // Returning to the same page must not revive the old presentation.
        release?.resume()
        await task.value
        XCTAssertTrue(imported)
        XCTAssertEqual(navigation.path, [.apps])
        XCTAssertFalse(selections.isCurrent(generation))
        XCTAssertFalse(navigation.mayPresentImportedApp(capturedRevision: revision))
    }

    func testFailedImportReportsCapturedOwnerAfterNavigationAndReconnect() async throws {
        let navigation = AppNavigationModel()
        navigation.openApps()
        let revision = navigation.presentationRevision
        let host = SessionHost()
        let sessionID = UUID().uuidString.lowercased()
        let original = MockConversationSource(sessionMode: .code)
        original.model.activeSessionId = sessionID
        let owner = try host.retain(source: original, scope: .global, mode: .code,
                                    sessionID: sessionID, workspace: "/code", draft: "")
        host.select(owner)
        let presentation = LocalAppActionPresentation(
            mayPresent: { navigation.mayPresentImportedApp(capturedRevision: revision) },
            reportOwnerFailure: { host.deliverOperationFailure(ConversationError(kind: .host, message: $0), to: owner) })
        let started = expectation(description: "import is suspended")
        var release: CheckedContinuation<Void, Never>?
        var pageError: String?
        let task = Task { @MainActor in
            await withCheckedContinuation { continuation in
                release = continuation
                started.fulfill()
            }
            presentation.deliverFailure("import failed") { pageError = $0 }
        }
        await fulfillment(of: [started], timeout: 1)
        navigation.path.removeAll()
        let chat = MockConversationSource(sessionMode: .chat)
        let chatEntry = try host.retain(source: chat, scope: .global, mode: .chat,
                                        sessionID: "", workspace: "/chat", draft: "")
        host.select(chatEntry)
        let reconnected = MockConversationSource(sessionMode: .code)
        reconnected.model.activeSessionId = sessionID
        _ = try host.replaceIdleSource(original, with: reconnected, scope: .global, mode: .code,
                                       sessionID: sessionID, workspace: "/code", draft: "")
        release?.resume()
        await task.value
        XCTAssertNil(pageError)
        XCTAssertEqual(reconnected.model.error?.message, "import failed")
        XCTAssertNil(chat.model.error)
        XCTAssertEqual(host.selectedEntryID, chatEntry.id)
    }
}

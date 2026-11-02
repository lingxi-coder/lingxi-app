import Foundation
import XCTest

@testable import LingxiCode

final class ProjectRepositoryTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("ios-project-repo-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        if root != nil {
            try? FileManager.default.removeItem(at: root)
        }
    }

    func testModeMembershipSurvivesAnEmptyCatalog() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Code")
        XCTAssertEqual(try repository.project(projectId: project.id).record.visibleModes, [.code])
        let row = ProjectSessionSummary(sessionId: "chat", title: "Chat", messageCount: 1,
                                        relativeTime: "Now", updatedAt: Date(), mode: .chat)
        _ = try repository.updateSessions(projectId: project.id, sessions: [row])
        _ = try repository.updateSessions(projectId: project.id, sessions: [])
        XCTAssertEqual(try repository.project(projectId: project.id).record.visibleModes, [.chat, .code])
        let newChat = try repository.createInternal(name: "Chat only", mode: .chat)
        XCTAssertEqual(try repository.project(projectId: newChat.id).record.visibleModes, [.chat])
    }

    func testProjectWithoutExplicitModeMembershipIsRejected() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Code")
        let data = try JSONEncoder().encode(project.record)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        object.removeValue(forKey: "modeMembership")
        let oldRecord = try JSONSerialization.data(withJSONObject: object)
        XCTAssertThrowsError(try JSONDecoder().decode(ProjectRecord.self, from: oldRecord))
        let manifest = root.appendingPathComponent(project.id).appendingPathComponent(projectManifestFileName)
        let index = root.appendingPathComponent(projectIndexFileName)
        try oldRecord.write(to: manifest)
        let originalIndex = try Data(contentsOf: index)
        XCTAssertTrue(repository.load().projects.isEmpty)
        XCTAssertEqual(try Data(contentsOf: manifest), oldRecord)
        XCTAssertEqual(try Data(contentsOf: index), originalIndex)
        object["modeMembership"] = NSNull()
        let nullRecord = try JSONSerialization.data(withJSONObject: object)
        XCTAssertThrowsError(try JSONDecoder().decode(ProjectRecord.self, from: nullRecord))
    }

    func testOldSessionIndexIsIgnoredAndNeverOverwritten() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let oldURL = root.appendingPathComponent("global-session-index.json")
        let oldData = Data(#"{"version":1,"sessions":[]}"#.utf8)
        try oldData.write(to: oldURL)
        XCTAssertTrue(repository.load().globalSessions.isEmpty)
        _ = try repository.updateSessions(projectId: nil, sessions: [session("current", at: Date())])
        XCTAssertEqual(try Data(contentsOf: oldURL), oldData)
        XCTAssertEqual(repository.load().globalSessions.map(\.sessionId), ["current"])

        let currentURL = root.appendingPathComponent(globalSessionIndexFileName)
        try oldData.write(to: currentURL)
        XCTAssertTrue(repository.load().errorMessage?.contains("global") == true)
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: []))
        XCTAssertEqual(try Data(contentsOf: currentURL), oldData)
    }

    func testCorruptProjectIndexIsReportedAndNeverOverwrittenOrDeleted() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Keep")
        let index = root.appendingPathComponent(projectIndexFileName)
        for bytes in [Data("{".utf8),
                      Data(#"{"version":1,"version":1,"projectIds":[]}"#.utf8),
                      Data(#"{"version":1,"projectIds":[],"\u0070rojectIds":[]}"#.utf8)] {
            try bytes.write(to: index)
            XCTAssertTrue(repository.load().errorMessage?.contains("projects-index") == true)
            XCTAssertThrowsError(try repository.createInternal(name: "Must fail"))
            XCTAssertThrowsError(try repository.setActiveProject(project.id))
            XCTAssertThrowsError(try repository.deleteProject(projectId: project.id))
            XCTAssertEqual(try Data(contentsOf: index), bytes)
            XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent(project.id).path))
        }
    }

    func testMissingOrMalformedProjectSessionsAreNotRecreated() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Keep")
        let projectIndex = root.appendingPathComponent(projectIndexFileName)
        let originalIndex = try Data(contentsOf: projectIndex)
        let sessions = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        try FileManager.default.removeItem(at: sessions)
        XCTAssertTrue(repository.load().errorMessage?.contains("project.\(project.id)") == true)
        XCTAssertThrowsError(try repository.project(projectId: project.id))
        XCTAssertThrowsError(try repository.updateSessions(projectId: project.id, sessions: []))
        XCTAssertFalse(FileManager.default.fileExists(atPath: sessions.path))
        XCTAssertEqual(try Data(contentsOf: projectIndex), originalIndex)

        let malformed = Data(#"{"version":2,"sessions":[{"sessionId":"x"}]}"#.utf8)
        try malformed.write(to: sessions)
        XCTAssertNotNil(repository.load().errorMessage)
        XCTAssertThrowsError(try repository.updateSessions(projectId: project.id, sessions: []))
        XCTAssertEqual(try Data(contentsOf: sessions), malformed)
        XCTAssertEqual(try Data(contentsOf: projectIndex), originalIndex)
    }

    func testUUIDNamedProjectEntryThatCannotBeScannedFreezesNewClaims() throws {
        let repository = ProjectRepository(projectsRoot: root)
        _ = try repository.createInternal(name: "Existing")
        let id = UUID().uuidString.lowercased()
        let invalid = root.appendingPathComponent(id)
        let bytes = Data("keep original".utf8)
        try bytes.write(to: invalid)
        XCTAssertTrue(repository.load().errorMessage?.contains("project.\(id)") == true)
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: nil, sessionId: "new", title: "New"))
        XCTAssertEqual(try Data(contentsOf: invalid), bytes)
    }

    func testMissingProjectIndexAndDuplicateSessionRowsFailClosed() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Keep")
        let projectIndex = root.appendingPathComponent(projectIndexFileName)
        let savedIndex = try Data(contentsOf: projectIndex)
        try FileManager.default.removeItem(at: projectIndex)
        XCTAssertNotNil(repository.load().errorMessage)
        XCTAssertThrowsError(try repository.createInternal(name: "Must fail"))
        XCTAssertFalse(FileManager.default.fileExists(atPath: projectIndex.path))
        try savedIndex.write(to: projectIndex)

        _ = try repository.updateSessions(projectId: project.id, sessions: [session("a", at: Date())])
        let sessions = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: sessions)) as? [String: Any])
        let rows = try XCTUnwrap(object["sessions"] as? [[String: Any]])
        object["sessions"] = rows + rows
        let duplicateRows = try JSONSerialization.data(withJSONObject: object)
        try duplicateRows.write(to: sessions)
        XCTAssertNotNil(repository.load().errorMessage)
        XCTAssertThrowsError(try repository.updateSessions(projectId: project.id, sessions: []))
        XCTAssertEqual(try Data(contentsOf: sessions), duplicateRows)
    }

    func testConflictingSessionModeNeverRewritesExistingIndex() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let original = ProjectSessionSummary(sessionId: "same", title: "A", messageCount: 1,
                                             relativeTime: "Now", updatedAt: Date(), mode: .chat)
        _ = try repository.updateSessions(projectId: nil, sessions: [original])
        let index = root.appendingPathComponent(globalSessionIndexFileName)
        let bytes = try Data(contentsOf: index)
        var conflicting = original
        conflicting.mode = .code
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: [conflicting]))
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: nil, sessionId: "same", title: "A", mode: .code))
        XCTAssertEqual(try Data(contentsOf: index), bytes)
    }

    func testCrossScopeSessionWritesAreRejectedBeforeEitherIndexChanges() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let id = "11111111-1111-4111-8111-111111111111"
        let row = session(id, at: Date())
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        let globalURL = root.appendingPathComponent(globalSessionIndexFileName)
        let projectURL = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let global = try Data(contentsOf: globalURL)
        let projectIndex = try Data(contentsOf: projectURL)
        var alias = row
        alias.sessionId = "sess:" + id.uppercased()
        XCTAssertThrowsError(try repository.updateSessions(projectId: project.id, sessions: [alias]))
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: project.id, sessionId: alias.sessionId, title: "wrong"))
        XCTAssertEqual(try Data(contentsOf: globalURL), global)
        XCTAssertEqual(try Data(contentsOf: projectURL), projectIndex)
    }

    func testColdConflictingCatalogDoesNotSelectAnOwnerOrOverwriteFiles() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let id = "11111111-1111-4111-8111-111111111111"
        _ = try repository.updateSessions(projectId: nil, sessions: [session(id, at: Date())])
        let globalURL = root.appendingPathComponent(globalSessionIndexFileName)
        let projectURL = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let duplicate = try Data(contentsOf: globalURL)
        try duplicate.write(to: projectURL)
        let state = ProjectRepository(projectsRoot: root).load()
        XCTAssertNotNil(state.errorMessage)
        XCTAssertTrue(state.errorMessage?.contains("global") == true)
        XCTAssertTrue(state.errorMessage?.contains("project.\(project.id)") == true)
        for scope: ConversationScope in [.global, .project(project.id)] {
            XCTAssertFalse(SessionCatalogOwnership.allows(sessionID: id, scope: scope, mode: .code,
                projects: state.projects, globalSessions: state.globalSessions, claimedScope: scope))
        }
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: []))
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: project.id, sessionId: id, title: "wrong"))
        XCTAssertEqual(try Data(contentsOf: globalURL), duplicate)
        XCTAssertEqual(try Data(contentsOf: projectURL), duplicate)
    }

    func testPersistedClaimCannotOverrideAnIndexedScopeOrMode() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let id = "11111111-1111-4111-8111-111111111111"
        let row = ProjectSessionSummary(sessionId: id, title: "Chat", messageCount: 1,
                                        relativeTime: "Now", updatedAt: Date(), mode: .chat)
        let state = try repository.updateSessions(projectId: nil, sessions: [row])
        XCTAssertFalse(SessionCatalogOwnership.allows(sessionID: id, scope: .project("p"), mode: .chat,
            projects: state.projects, globalSessions: state.globalSessions, claimedScope: .project("p")))
        XCTAssertFalse(SessionCatalogOwnership.allows(sessionID: id, scope: .global, mode: .code,
            projects: state.projects, globalSessions: state.globalSessions, claimedScope: .global))
        XCTAssertTrue(SessionCatalogOwnership.allows(sessionID: id, scope: .global, mode: .chat,
            projects: state.projects, globalSessions: state.globalSessions, claimedScope: nil))
    }

    func testCatalogPreservesConfirmedSessionsFromOtherWorkspaces() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Linked directories")
        let linked = "/home/lingxi/lingxi/projects/linked"
        try FileManager.default.createDirectory(at: root.appendingPathComponent("linked"), withIntermediateDirectories: true)
        let original = ProjectSessionSummary(sessionId: "a", title: "A", messageCount: 2,
            relativeTime: "Now", updatedAt: Date(), workspaceDirectory: project.record.directory, transcriptPath: "/catalog/a.jsonl")
        _ = try repository.updateSessions(projectId: project.id, sessions: [original])
        _ = try repository.recordWorkspace(projectId: project.id, state: SessionWorkspaceState(sessionId: "draft",
            cwd: linked, projectRoot: linked, additionalDirectories: [linked], transcriptPath: ""))
        let incoming = ProjectSessionSummary(sessionId: "b", title: "B", messageCount: 2,
            relativeTime: "Now", updatedAt: Date(), workspaceDirectory: linked, transcriptPath: "/catalog/b.jsonl")
        _ = try repository.updateSessions(projectId: project.id, sessions: [incoming])
        let restored = try ProjectRepository(projectsRoot: root).project(projectId: project.id)
        XCTAssertEqual(Set(restored.sessions.map(\.sessionId)), Set(["a", "b"]))
        XCTAssertEqual(restored.sessions.first { $0.sessionId == "a" }?.workspaceDirectory, project.record.directory)
        XCTAssertEqual(restored.sessions.first { $0.sessionId == "b" }?.workspaceDirectory, linked)
        _ = try repository.updateSessions(projectId: project.id, sessions: [])
        XCTAssertEqual(try repository.project(projectId: project.id).sessions.map(\.sessionId), ["a"])
    }

    func testMissingLinkedDirectoryDoesNotBlockLiveWorkspacePersistence() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Missing linked folder")
        let missing = "/home/lingxi/lingxi/projects/deleted"
        _ = try repository.recordWorkspace(projectId: project.id, state: SessionWorkspaceState(sessionId: "draft",
            cwd: project.record.directory, projectRoot: project.record.directory, additionalDirectories: [missing], transcriptPath: ""))
        XCTAssertEqual(try repository.project(projectId: project.id).record.additionalDirectories, [missing])
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("deleted").path))
    }

    func testSyncStateUpdatesPreserveModeMembership() throws {
        let project = try ProjectRepository(projectsRoot: root).createInternal(name: "Session owner", mode: .chat)
        XCTAssertEqual(project.record.with(syncState: .authorizationLost).modeMembership, [.chat])
    }

    func testUserHomeCanBeSelectedAsWorkspaceWithoutChangingSource() throws {
        let repository = ProjectRepository(projectsRoot: root.appendingPathComponent("projects"), userHome: root)
        let project = try repository.createInternal(name: "User home")
        _ = try repository.recordWorkspace(projectId: project.id, state: SessionWorkspaceState(sessionId: "draft",
            cwd: "/home/lingxi", projectRoot: project.record.directory, additionalDirectories: ["/home/lingxi"], transcriptPath: ""))
        let restored = try repository.project(projectId: project.id)
        XCTAssertEqual(restored.workspace.hostURL.path, root.resolvingSymlinksInPath().path)
        XCTAssertEqual(restored.sourceWorkspace, project.sourceWorkspace)
    }

    func testCreateAndReloadUsesStableUUIDWorkspaceLayout() throws {
        let repository = ProjectRepository(projectsRoot: root)

        let created = try repository.createInternal(name: "代码项目")
        let projectDirectory = root.appendingPathComponent(created.record.id, isDirectory: true)

        XCTAssertTrue(isLowercaseUUID(created.record.id))
        XCTAssertEqual(
            projectDirectory.appendingPathComponent(projectWorkspaceDirectoryName, isDirectory: true).resolvingSymlinksInPath(),
            created.workspace.hostURL
        )
        XCTAssertEqual("/home/lingxi/lingxi/projects/\(created.record.id)/workspace", created.workspace.guestPath)
        XCTAssertTrue(FileManager.default.fileExists(atPath: projectDirectory.appendingPathComponent(projectManifestFileName).path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: projectDirectory.appendingPathComponent(projectSessionIndexFileName).path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: projectDirectory.appendingPathComponent(projectSyncBaselineFileName).path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: created.workspace.hostURL.appendingPathComponent(projectManifestFileName).path))

        let restored = ProjectRepository(projectsRoot: root).load()
        XCTAssertEqual([created.record.id], restored.projects.map(\.record.id))
    }

    func testProjectAndGlobalSessionIndexesStayIsolated() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let first = try repository.createInternal(name: "一")
        let second = try repository.createInternal(name: "二")
        let now = Date()

        _ = try repository.updateSessions(projectId: first.record.id, sessions: [
            session("session-a", at: now),
            session("session-b", at: now.addingTimeInterval(-1)),
        ])
        _ = try repository.updateSessions(projectId: second.record.id, sessions: [
            session("session-c", at: now)
        ])
        _ = try repository.updateSessions(projectId: nil, sessions: [
            session("legacy-global", at: now)
        ])

        let restored = repository.load()
        XCTAssertEqual(
            Set(["session-a", "session-b"]),
            Set(restored.projects.first(where: { $0.record.id == first.record.id })?.sessions.map(\.sessionId) ?? [])
        )
        XCTAssertEqual(
            ["session-c"],
            restored.projects.first(where: { $0.record.id == second.record.id })?.sessions.map(\.sessionId)
        )
        XCTAssertEqual(["legacy-global"], restored.globalSessions.map(\.sessionId))
        XCTAssertNotEqual(first.workspace.hostURL, second.workspace.hostURL)
    }

    func testFailedAtomicManifestWriteRollsBackNewProjectDirectory() {
        let id = "20000000-0000-4000-8000-000000000001"
        let repository = ProjectRepository(
            projectsRoot: root,
            newId: { id },
            writer: FailingWriter(failingFileName: projectManifestFileName)
        )

        XCTAssertThrowsError(try repository.createInternal(name: "失败"))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(id).path))
    }

    func testWorkspaceSymlinkIsQuarantinedAndNestedLinksAreUnsafe() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "链接安全")
        let workspace = project.workspace.hostURL
        let outside = root.appendingPathComponent("outside-workspace", isDirectory: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
        let secret = outside.appendingPathComponent("secret.txt")
        try Data("secret".utf8).write(to: secret)
        let nestedLink = workspace.appendingPathComponent("secret-link")
        try FileManager.default.createSymbolicLink(at: nestedLink, withDestinationURL: secret)

        XCTAssertFalse(isSafeProjectWorkspacePath(root: workspace, candidate: nestedLink))
        let regular = workspace.appendingPathComponent("regular.txt")
        try Data("ok".utf8).write(to: regular)
        XCTAssertTrue(isSafeProjectWorkspacePath(root: workspace, candidate: regular))

        try FileManager.default.removeItem(at: workspace)
        try FileManager.default.createSymbolicLink(at: workspace, withDestinationURL: outside)
        let reloaded = repository.load()
        XCTAssertTrue(reloaded.projects.isEmpty)
        XCTAssertFalse(reloaded.errorMessage?.isEmpty ?? true)
    }


    func testPendingSessionSurvivesStaleCatalogUntilAcknowledged() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        _ = try repository.recordStartedSession(projectId: project.record.id, sessionId: "sess:20000000-0000-4000-8000-0000000000AA", title: "First prompt", mode: .chat)
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [])
        let restored = try ProjectRepository(projectsRoot: root).project(projectId: project.record.id)
        XCTAssertEqual(restored.sessions.map(\.sessionId), ["20000000-0000-4000-8000-0000000000aa"])
        XCTAssertTrue(restored.sessions[0].pendingCatalogConfirmation)
        var confirmed = restored.sessions[0]
        confirmed.messageCount = 2
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [confirmed, confirmed])
        XCTAssertFalse(try repository.project(projectId: project.record.id).sessions[0].pendingCatalogConfirmation)
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [])
        XCTAssertTrue(try repository.project(projectId: project.record.id).sessions.isEmpty)
    }

    func testArchiveSurvivesCatalogAndReloadAndRestoresData() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let row = session("session-a", at: Date())
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        _ = try repository.setSessionArchived(projectId: nil, sessionId: row.sessionId, archived: true)
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        XCTAssertTrue(ProjectRepository(projectsRoot: root).load().globalSessions[0].isArchived)
        _ = try repository.updateSessions(projectId: nil, sessions: [])
        XCTAssertEqual(repository.load().globalSessions.count, 1)
        _ = try repository.setSessionArchived(projectId: nil, sessionId: row.sessionId, archived: false)
        _ = try repository.updateSessions(projectId: nil, sessions: [])
        XCTAssertFalse(ProjectRepository(projectsRoot: root).load().globalSessions[0].isArchived)
        XCTAssertEqual(repository.load().globalSessions[0].messageCount, row.messageCount)
    }

    func testArchiveIsIsolatedByProjectAndPreservedWhenSessionStartsAgain() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let row = session("project-id", at: Date())
        _ = try repository.updateSessions(projectId: nil, sessions: [session("global-id", at: Date())])
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [row])
        _ = try repository.setSessionArchived(projectId: project.record.id, sessionId: row.sessionId, archived: true)
        XCTAssertFalse(repository.load().globalSessions[0].isArchived)
        _ = try repository.recordStartedSession(projectId: project.record.id, sessionId: row.sessionId, title: row.title)
        XCTAssertTrue(try repository.project(projectId: project.record.id).sessions[0].isArchived)
    }

    func testFirstSubmissionCountDoesNotRegressOrUnarchiveExistingSession() throws {
        let repository = ProjectRepository(projectsRoot: root)
        _ = try repository.recordStartedSession(projectId: nil, sessionId: "first", title: "Prompt", initialMessageCount: 1)
        XCTAssertEqual(repository.load().globalSessions[0].messageCount, 1)
        _ = try repository.setSessionArchived(projectId: nil, sessionId: "first", archived: true)
        _ = try repository.recordStartedSession(projectId: nil, sessionId: "first", title: "Prompt", initialMessageCount: 0)
        XCTAssertEqual(repository.load().globalSessions[0].messageCount, 1)
        XCTAssertTrue(repository.load().globalSessions[0].isArchived)
    }

    func testStaleZeroMessageCatalogDoesNotAcknowledgeFirstSubmission() throws {
        let repository = ProjectRepository(projectsRoot: root)
        var incoming = ProjectSessionSummary(sessionId: "first", title: "New conversation", messageCount: 0,
                                             relativeTime: "Before", updatedAt: Date(timeIntervalSince1970: 900))
        _ = try repository.updateSessions(projectId: nil, sessions: [incoming])
        _ = try repository.recordStartedSession(projectId: nil, sessionId: "first", title: "First prompt", initialMessageCount: 1)
        _ = try repository.updateSessions(projectId: nil, sessions: [incoming])
        let pending = ProjectRepository(projectsRoot: root).load().globalSessions[0]
        XCTAssertEqual(pending.title, "First prompt")
        XCTAssertEqual(pending.messageCount, 1)
        XCTAssertTrue(pending.pendingCatalogConfirmation)
        incoming.title = "Confirmed"
        incoming.messageCount = 1
        _ = try repository.updateSessions(projectId: nil, sessions: [incoming])
        let confirmed = repository.load().globalSessions[0]
        XCTAssertEqual(confirmed.title, "Confirmed")
        XCTAssertFalse(confirmed.pendingCatalogConfirmation)
    }


    func testOneProjectMovesAndColdRestoresOneSessionWithoutMovingItsSourceMirror() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Workspace project")
        let linked = root.appendingPathComponent("linked", isDirectory: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: true)
        let guest = "/home/lingxi/lingxi/projects/linked"
        _ = try repository.recordStartedSession(projectId: project.id, sessionId: "session-one", title: "Shared", mode: .code)
        _ = try repository.recordWorkspace(projectId: project.id, state: SessionWorkspaceState(sessionId: "session-one", cwd: guest,
            projectRoot: project.record.directory, additionalDirectories: [guest], transcriptPath: "/home/lingxi/.lingxi/projects/b/session-one.jsonl"))
        let restored = try ProjectRepository(projectsRoot: root).project(projectId: project.id)
        XCTAssertEqual(restored.workspace.hostURL, linked.resolvingSymlinksInPath())
        XCTAssertEqual(restored.sourceWorkspace, project.sourceWorkspace)
        XCTAssertEqual(restored.sessions.map(\.sessionId), ["session-one"])
        XCTAssertEqual(restored.sessions.first?.workspaceDirectory, guest)
        XCTAssertEqual(restored.sessions.first?.originalProjectDirectory, project.record.directory)
        try FileManager.default.removeItem(at: linked)
        XCTAssertEqual(ProjectRepository(projectsRoot: root).load().projects.map(\.id), [project.id])
        XCTAssertFalse(FileManager.default.fileExists(atPath: linked.path))
    }

    func testDirectoryNavigationDoesNotCreateAnEmptySessionOrEscapeHome() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Empty draft")
        let guest = project.record.directory
        _ = try repository.recordWorkspace(projectId: project.id, state: SessionWorkspaceState(sessionId: "draft", cwd: guest,
            projectRoot: guest, additionalDirectories: [], transcriptPath: ""))
        XCTAssertTrue(try repository.project(projectId: project.id).sessions.isEmpty)
        XCTAssertThrowsError(try repository.recordWorkspace(projectId: project.id, state: SessionWorkspaceState(sessionId: "draft", cwd: "/root",
            projectRoot: guest, additionalDirectories: [], transcriptPath: "")))
        XCTAssertEqual(try repository.project(projectId: project.id).record.activeDirectory, guest)
    }

    private func session(_ id: String, at date: Date) -> ProjectSessionSummary {
        ProjectSessionSummary(
            sessionId: id,
            title: id,
            messageCount: 1,
            relativeTime: "刚刚",
            updatedAt: date
        )
    }
}

private struct FailingWriter: ProjectAtomicWriting {
    let failingFileName: String

    func write<T>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws where T: Decodable, T: Encodable {
        if url.lastPathComponent == failingFileName {
            throw CocoaError(.fileWriteUnknown)
        }
        try DefaultProjectAtomicWriter().write(value, to: url, encoder: encoder, decoder: decoder)
    }

    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        if url.lastPathComponent == failingFileName {
            throw CocoaError(.fileWriteUnknown)
        }
        try DefaultProjectAtomicWriter().writeData(data, to: url, validate: validate)
    }
}

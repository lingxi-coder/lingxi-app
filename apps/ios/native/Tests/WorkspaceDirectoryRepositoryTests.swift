import XCTest
@testable import LingxiCode

final class WorkspaceDirectoryRepositoryTests: XCTestCase {
    private var root: URL!
    private var paths: WorkspaceDirectoryPaths!
    private var repository: WorkspaceDirectoryRepository!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let directories = MobileUserStorage.directories()
        let project = directories.projectsGuestPath + "/11111111-1111-4111-8111-111111111111/workspace"
        let ownedScheduled = directories.configGuestPath + "/workspaces/2026-11-02/owner"
        paths = WorkspaceDirectoryPaths(home: root, guestHome: directories.guestHome,
            workspace: directories.chatGuestCwd, chatWorkspace: directories.chatGuestCwd,
            codeWorkspace: directories.codeGuestCwd, projects: directories.projectsGuestPath,
            scheduled: directories.scheduledGuestCwd,
            settings: URL(fileURLWithPath: directories.configHome).lastPathComponent,
            projectWorkspaces: [project], ownedScheduledWorkspace: ownedScheduled)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        repository = WorkspaceDirectoryRepository(paths: paths)
    }

    override func tearDownWithError() throws { try FileManager.default.removeItem(at: root) }

    func testCreateBrowseAndPersistSavedLocationWithoutBootingLinux() async throws {
        let path = try await repository.createDirectory(in: paths.guestHome, name: "Demo 项目")
        try Data("hello".utf8).write(to: root.appendingPathComponent("note.txt"))
        let entries = try await repository.entries(at: paths.guestHome, includingHidden: false)
        XCTAssertEqual(entries.map(\.name), ["Demo 项目", "note.txt"])
        XCTAssertTrue(entries[0].isDirectory)
        let saved = try await repository.saveLocation(path: path, name: "我的项目")
        let reloaded = try await WorkspaceDirectoryRepository(paths: paths).locations()
        XCTAssertEqual(saved, reloaded)
        XCTAssertEqual(paths.display(path), "~/Demo 项目")
    }

    func testRemoveOnlyUnlinksLocationAndEditingPreservesIdentity() async throws {
        let path = try await repository.createDirectory(in: paths.guestHome, name: "keep")
        let saved = try await repository.saveLocation(path: path, name: "Old label")
        let edited = try await repository.saveLocation(path: path, name: "New label", id: saved[0].id)
        XCTAssertEqual(edited[0].id, saved[0].id)
        XCTAssertEqual(edited[0].name, "New label")
        let remaining = try await repository.removeLocation(id: saved[0].id)
        XCTAssertTrue(remaining.isEmpty)
        try await repository.validateDirectory(path)
    }

    func testRenameUpdatesDescendantLocationsAndDeleteRemovesThem() async throws {
        let parent = try await repository.createDirectory(in: paths.guestHome, name: "before")
        let child = try await repository.createDirectory(in: parent, name: "child")
        _ = try await repository.saveLocation(path: child, name: "Child")
        let renamed = try await repository.renameDirectory(parent, name: "after", protectedPaths: [])
        let rows = try await repository.locations()
        XCTAssertEqual(rows[0].path, renamed + "/child")
        try await repository.deleteDirectory(renamed, protectedPaths: [])
        let afterDelete = try await repository.locations()
        XCTAssertTrue(afterDelete.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("after").path))
    }

    func testCannotMutateAnAncestorOfAnActiveWorkspaceOrHome() async throws {
        let project = try await repository.createDirectory(in: paths.guestHome, name: "project")
        let current = try await repository.createDirectory(in: project, name: "src")
        for path in [paths.guestHome, project] {
            do {
                try await repository.deleteDirectory(path, protectedPaths: [current])
                XCTFail("Must preserve \(path)")
            } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        }
        try await repository.validateDirectory(current)
    }

    func testRejectsTraversalAndEscapingSymlinks() async throws {
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("escape"), withDestinationURL: root.deletingLastPathComponent())
        for path in ["/etc", paths.guestHome + "/../outside", paths.guestHome + "/escape"] {
            do {
                _ = try await repository.entries(at: path, includingHidden: false)
                XCTFail("Must reject \(path)")
            } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        }
        do {
            _ = try await repository.createDirectory(in: paths.guestHome, name: "../bad")
            XCTFail("Must reject a path used as a name")
        } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
    }

    func testInternalSymlinksCannotBypassActiveDirectoryProtection() async throws {
        let current = try await repository.createDirectory(in: paths.guestHome, name: "active")
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("alias"), withDestinationURL: root.appendingPathComponent("active"))
        do {
            try await repository.deleteDirectory(paths.guestHome + "/alias", protectedPaths: [current])
            XCTFail("Must preserve the active target")
        } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        try await repository.validateDirectory(current)
    }

    func testManagedWorkspacesAllowContentsButProtectRootsAndConfiguration() async throws {
        let config = root.appendingPathComponent(paths.settings, isDirectory: true)
        for workspace in paths.managedWorkspaces {
            let suffix = String(workspace.dropFirst(paths.guestHome.count + 1))
            try FileManager.default.createDirectory(at: root.appendingPathComponent(suffix), withIntermediateDirectories: true)
            let child = try await repository.createDirectory(in: workspace, name: "owned")
            try await repository.deleteDirectory(child, protectedPaths: [])
            do {
                try await repository.deleteDirectory(workspace, protectedPaths: [])
                XCTFail("The managed root must remain in place: \(workspace)")
            } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        }
        let secret = config.appendingPathComponent("secrets", isDirectory: true)
        try FileManager.default.createDirectory(at: secret, withIntermediateDirectories: true)
        let otherProject = paths.projects + "/22222222-2222-4222-8222-222222222222/workspace"
        try FileManager.default.createDirectory(at: root.appendingPathComponent(String(otherProject.dropFirst(paths.guestHome.count + 1))), withIntermediateDirectories: true)
        let otherScheduled = paths.settingsPath + "/workspaces/2026-11-02/other-owner"
        try FileManager.default.createDirectory(at: root.appendingPathComponent(String(otherScheduled.dropFirst(paths.guestHome.count + 1))), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: root.appendingPathComponent(String(paths.scheduled.dropFirst(paths.guestHome.count + 1))), withIntermediateDirectories: true)
        let projectMetadata = (paths.projectWorkspaces[0] as NSString).deletingLastPathComponent
        for protected in [paths.settingsPath, paths.settingsPath + "/secrets", paths.settingsPath + "/.apps",
                          paths.projects, projectMetadata, otherProject, paths.scheduled,
                          paths.settingsPath + "/workspaces", otherScheduled] {
            if protected.hasSuffix("/.apps") {
                try FileManager.default.createDirectory(at: config.appendingPathComponent(".apps"), withIntermediateDirectories: true)
            }
            do {
                _ = try await repository.createDirectory(in: protected, name: "forbidden")
                XCTFail("Must protect config data: \(protected)")
            } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        }
        let chat = root.appendingPathComponent(String(paths.chatWorkspace.dropFirst(paths.guestHome.count + 1)))
        try FileManager.default.createSymbolicLink(at: chat.appendingPathComponent("secrets-alias"), withDestinationURL: secret)
        do {
            _ = try await repository.createDirectory(in: paths.chatWorkspace + "/secrets-alias", name: "forbidden")
            XCTFail("An alias inside a workspace must not expose config writes")
        } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        let code = root.appendingPathComponent(String(paths.codeWorkspace.dropFirst(paths.guestHome.count + 1)))
        try FileManager.default.createSymbolicLink(at: chat.appendingPathComponent("code-alias"), withDestinationURL: code)
        do {
            _ = try await repository.createDirectory(in: paths.chatWorkspace + "/code-alias", name: "forbidden")
            XCTFail("A workspace alias must not grant access to another root")
        } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }

        let unowned = WorkspaceDirectoryPaths(home: root, guestHome: paths.guestHome, workspace: paths.workspace,
            chatWorkspace: paths.chatWorkspace, codeWorkspace: paths.codeWorkspace, projects: paths.projects,
            scheduled: paths.scheduled, settings: paths.settings)
        let unownedRepository = WorkspaceDirectoryRepository(paths: unowned)
        for path in [paths.projectWorkspaces[0], try XCTUnwrap(paths.ownedScheduledWorkspace)] {
            do {
                _ = try await unownedRepository.createDirectory(in: path, name: "forbidden")
                XCTFail("An unverified owner must not grant content access: \(path)")
            } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        }
    }

    func testCurrentDefaultWorkspaceTracksTheFixedSessionMode() {
        let directories = MobileUserStorage.directories()
        XCTAssertEqual(WorkspaceDirectoryPaths.current(mode: .chat).workspace, directories.chatGuestCwd)
        XCTAssertEqual(WorkspaceDirectoryPaths.current(mode: .code).workspace, directories.codeGuestCwd)
        XCTAssertNotEqual(directories.chatGuestCwd, directories.codeGuestCwd)
    }

    func testOnlyExactProjectSourceWorkspaceEntersTheWriteAllowlist() {
        let directories = MobileUserStorage.directories()
        let id = UUID().uuidString.lowercased()
        let record = ProjectRecord(id: id, name: "Test", storageKind: .internal,
            createdAt: .distantPast, updatedAt: .distantPast)
        let host = URL(fileURLWithPath: directories.projects, isDirectory: true)
            .appendingPathComponent(id, isDirectory: true).appendingPathComponent("workspace", isDirectory: true)
        let guest = directories.projectsGuestPath + "/" + id + "/workspace"
        let source = ProjectWorkspace(projectId: id, hostURL: host, guestPath: guest)
        let valid = ProjectSnapshot(record: record, workspace: source, sessions: [])
        let forgedGuest = ProjectWorkspace(projectId: id, hostURL: host, guestPath: directories.projectsGuestPath)
        let forgedHost = ProjectWorkspace(projectId: id, hostURL: host.deletingLastPathComponent(), guestPath: guest)
        let forgedID = ProjectWorkspace(projectId: UUID().uuidString.lowercased(), hostURL: host, guestPath: guest)
        let invalid = [forgedGuest, forgedHost, forgedID].map {
            ProjectSnapshot(record: record, workspace: source, sourceWorkspace: $0, sessions: [])
        }
        XCTAssertEqual(WorkspaceDirectoryPaths.current(mode: .chat, projects: [valid]).projectWorkspaces, [guest])
        XCTAssertTrue(WorkspaceDirectoryPaths.current(mode: .chat, projects: invalid).projectWorkspaces.isEmpty)
    }

    func testConfigAliasCannotBypassWriteProtection() async throws {
        let folder = try await repository.createDirectory(in: paths.guestHome, name: "normal")
        _ = try await repository.saveLocation(path: folder, name: "Normal")
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("settings-alias"),
            withDestinationURL: root.appendingPathComponent(paths.settings))
        do {
            _ = try await repository.createDirectory(in: paths.guestHome + "/settings-alias", name: "forbidden")
            XCTFail("Must not modify runtime settings through an alias")
        } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
    }

    func testLinkedLocationStoreDoesNotReadOrOverwriteAnotherFile() async throws {
        let settings = root.appendingPathComponent(paths.settings)
        try FileManager.default.createDirectory(at: settings, withIntermediateDirectories: true)
        let other = root.appendingPathComponent("other.json")
        let original = Data("[]".utf8)
        try original.write(to: other)
        try FileManager.default.createSymbolicLink(at: paths.savedLocations, withDestinationURL: other)
        do {
            _ = try await repository.locations()
            XCTFail("Must reject a linked location store")
        } catch { XCTAssertTrue(error is WorkspaceDirectoryError) }
        XCTAssertEqual(try Data(contentsOf: other), original)
    }

    func testSavedLocationUsesTheSameCanonicalPathAsTheRuntime() async throws {
        let target = try await repository.createDirectory(in: paths.guestHome, name: "real-folder")
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("shortcut"),
            withDestinationURL: root.appendingPathComponent("real-folder"))
        let rows = try await repository.saveLocation(path: paths.guestHome + "/shortcut", name: "Shortcut")
        XCTAssertEqual(rows[0].path, target)
        let again = try await repository.saveLocation(path: target, name: "Keep existing label")
        XCTAssertEqual(again.count, 1)
        XCTAssertEqual(again[0].name, "Shortcut")
    }
}

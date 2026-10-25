import XCTest
@testable import LingxiCode

final class WorkspaceDirectoryRepositoryTests: XCTestCase {
    private var root: URL!
    private var paths: WorkspaceDirectoryPaths!
    private var repository: WorkspaceDirectoryRepository!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        paths = WorkspaceDirectoryPaths(home: root, guestHome: LXISHGuestPaths.home,
            workspace: LXISHGuestPaths.workspaceRoot, projects: LXISHGuestPaths.workspace("projects"), settings: ".lingxi")
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

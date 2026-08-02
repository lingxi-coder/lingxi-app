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

    func testCreateAndReloadUsesStableUUIDWorkspaceLayout() throws {
        let repository = ProjectRepository(projectsRoot: root)

        let created = try repository.createInternal(name: "代码项目")
        let projectDirectory = root.appendingPathComponent(created.record.id, isDirectory: true)

        XCTAssertTrue(isLowercaseUUID(created.record.id))
        XCTAssertEqual(
            projectDirectory.appendingPathComponent(projectWorkspaceDirectoryName, isDirectory: true).resolvingSymlinksInPath(),
            created.workspace.hostURL
        )
        XCTAssertEqual("/workspace/\(created.record.id)", created.workspace.guestPath)
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
        XCTAssertTrue(reloaded.errorMessage?.contains("已隔离") == true)
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

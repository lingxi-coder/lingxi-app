import XCTest
@testable import LingxiCode

final class WorkspaceFilesTests: XCTestCase {
    func testSystemPickerCompletionHandlesEitherCallbackOrderAndCancellation() {
        let state = WorkspaceSystemPickerCompletion()
        let url = URL(fileURLWithPath: "/tmp/test-document.txt")
        state.receive(urls: [url])
        XCTAssertFalse(state.consumeIfReady())
        state.dismissed = true
        XCTAssertTrue(state.consumeIfReady())
        XCTAssertEqual(state.urls, [url])
        XCTAssertFalse(state.consumeIfReady())
        state.reset(exporting: true)
        state.dismissed = true
        XCTAssertFalse(state.consumeIfReady())
        state.receive(destination: url)
        XCTAssertTrue(state.consumeIfReady())
        XCTAssertEqual(state.destination, url)
        state.reset(exporting: false)
        state.receive(urls: [])
        state.dismissed = true
        XCTAssertTrue(state.consumeIfReady())
        XCTAssertTrue(state.urls.isEmpty)
    }

    private var root: URL!
    private var paths: WorkspaceDirectoryPaths!
    private var repo: WorkspaceDirectoryRepository!
    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("files-test-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        paths = .init(home: root, guestHome: "/home/brand", workspace: "/home/brand/work", projects: "/home/brand/work/projects", settings: ".config")
        repo = .init(paths: paths)
    }
    override func tearDownWithError() throws { try FileManager.default.removeItem(at: root) }
    private func write(_ name: String, _ text: String) throws { try Data(text.utf8).write(to: root.appendingPathComponent(name)) }
    func testMetadataNaturalSortRecursiveSearchAndHiddenFiles() async throws {
        let sub = try await repo.createDirectory(in: paths.guestHome, name: "sub")
        try write("file10.txt", "ten"); try write("file2.txt", "two"); try write(".hidden", "secret"); try write("sub/needle.txt", "match")
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("loop"), withDestinationURL: root)
        let search = try await repo.files(at: paths.guestHome, hidden: false, query: "needle", recursive: true)
        XCTAssertEqual(search.map(\.path), [sub + "/needle.txt"])
        let entries = try await repo.files(at: paths.guestHome, hidden: false, query: "", recursive: false)
        let names = WorkspaceDirectoryEntry.sorted(entries, by: .name).filter { !$0.isDirectory && !$0.isSymbolicLink }.map(\.name)
        XCTAssertEqual(names, ["file2.txt", "file10.txt"])
        XCTAssertFalse(entries.contains { $0.name == ".hidden" })
        XCTAssertEqual(entries.first { $0.name == "file2.txt" }?.byteCount, 3)
    }
    func testFileRenameMoveAndSavedDirectoryIdentity() async throws {
        let folder = try await repo.createDirectory(in: paths.guestHome, name: "source")
        let target = try await repo.createDirectory(in: paths.guestHome, name: "target")
        try write("source/a.txt", "hello")
        _ = try await repo.saveLocation(path: folder, name: "Custom")
        _ = try await repo.renameDirectory(folder + "/a.txt", name: "b.txt", protectedPaths: [folder])
        let failures = try await repo.transfer([folder], to: target, move: true, conflict: .keepBoth, protectedPaths: [], progress: { _ in })
        XCTAssertTrue(failures.isEmpty)
        let locations = try await repo.locations()
        XCTAssertEqual(locations.first?.path, target + "/source")
        XCTAssertEqual(locations.first?.name, "Custom")
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("target/source/b.txt").path))
    }
    func testCopyConflictSkipSelfDestinationAndPartialFailure() async throws {
        let target = try await repo.createDirectory(in: paths.guestHome, name: "target")
        try write("note.txt", "source"); try write("target/note.txt", "keep")
        let failures = try await repo.transfer([paths.guestHome + "/missing", paths.guestHome + "/note.txt"], to: target, move: false, conflict: .keepBoth, protectedPaths: [], progress: { _ in })
        XCTAssertEqual(failures.count, 1)
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("target/note.txt"), encoding: .utf8), "keep")
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("target/note (2).txt"), encoding: .utf8), "source")
        _ = try await repo.transfer([paths.guestHome + "/note.txt"], to: target, move: false, conflict: .skip, protectedPaths: [], progress: { _ in })
        let bad = try await repo.transfer([target], to: target, move: true, conflict: .keepBoth, protectedPaths: [], progress: { _ in })
        XCTAssertEqual(bad.count, 1)
    }
    func testSnapshotIsIndependentTextLimitAndCleanup() async throws {
        try write("note.txt", "original")
        let snapshot = try await repo.snapshot(paths.guestHome + "/note.txt")
        try write("note.txt", "changed")
        let text = try await repo.textPreview(snapshot)
        XCTAssertEqual(text, "original")
        snapshot.remove(); XCTAssertFalse(FileManager.default.fileExists(atPath: snapshot.directory.path))
        try Data(repeating: 65, count: 2 * 1024 * 1024 + 1).write(to: root.appendingPathComponent("large.txt"))
        let large = try await repo.snapshot(paths.guestHome + "/large.txt")
        defer { large.remove() }
        do { _ = try await repo.textPreview(large); XCTFail("Expected explicit size limit") } catch { XCTAssertTrue(error.localizedDescription.contains("2 MiB")) }
    }
    func testCancellationAndLinkCopyLeaveNoTemporaryFiles() async throws {
        let target = try await repo.createDirectory(in: paths.guestHome, name: "target")
        let source = try await repo.createDirectory(in: paths.guestHome, name: "source")
        try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("source/link"), withDestinationURL: root)
        let failures = try await repo.transfer([source], to: target, move: false, conflict: .keepBoth, protectedPaths: [], progress: { _ in })
        XCTAssertEqual(failures.count, 1)
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: root.appendingPathComponent("target").path).isEmpty)
        let task = Task { try Task.checkCancellation(); return try await repo.files(at: paths.guestHome, hidden: true, query: "", recursive: true) }
        task.cancel()
        do { _ = try await task.value; XCTFail("Expected cancellation") } catch is CancellationError {} catch { XCTFail("\(error)") }
    }
    func testImportExportFoldersPreserveConflictsAndCleanStaging() async throws {
        let external = FileManager.default.temporaryDirectory.appendingPathComponent("external-files-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: external.appendingPathComponent("incoming"), withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: external) }
        try Data("imported".utf8).write(to: external.appendingPathComponent("incoming/note.txt"))
        let incoming = external.appendingPathComponent("incoming")
        let failures = try await repo.importItems([incoming], to: paths.guestHome, conflict: .keepBoth, progress: { _ in })
        XCTAssertTrue(failures.isEmpty)
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("incoming/note.txt"), encoding: .utf8), "imported")
        let exported = try await repo.exportItems([paths.guestHome + "/incoming"], to: external, conflict: .keepBoth, progress: { _ in })
        XCTAssertTrue(exported.isEmpty)
        XCTAssertEqual(try String(contentsOf: external.appendingPathComponent("incoming (2)/note.txt"), encoding: .utf8), "imported")
        XCTAssertFalse(try FileManager.default.contentsOfDirectory(atPath: external.path).contains { $0.hasPrefix(".lingxi-copy-") })
    }
    func testBatchCancellationPreservesCompletedFiles() async throws {
        let target = try await repo.createDirectory(in: paths.guestHome, name: "target")
        try write("first.txt", "one"); try write("second.txt", "two")
        let source = repo!; let guest = paths.guestHome
        let operation = Task {
            try await source.transfer([guest + "/first.txt", guest + "/second.txt"], to: target, move: false,
                conflict: .keepBoth, protectedPaths: [], progress: { progress in
                    if progress.completed == 1 { withUnsafeCurrentTask { $0?.cancel() } }
                })
        }
        do { _ = try await operation.value; XCTFail("Expected cancellation") } catch is CancellationError {} catch { XCTFail("\(error)") }
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("target/first.txt").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("target/second.txt").path))
    }
    func testDeletingFileKeepsActiveParentDirectory() async throws {
        try write("note.txt", "hello")
        let failures = try await repo.deleteFiles([paths.guestHome + "/note.txt"], protectedPaths: [paths.guestHome], progress: { _ in })
        XCTAssertTrue(failures.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("note.txt").path))
    }
}

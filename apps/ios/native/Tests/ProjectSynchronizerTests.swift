import Foundation
import XCTest

@testable import LingxiCode

final class ProjectSynchronizerTests: XCTestCase {
    private var root: URL!
    private var externalRoot: URL!

    override func setUpWithError() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("ios-project-sync-\(UUID().uuidString)", isDirectory: true)
        root = base.appendingPathComponent("managed", isDirectory: true)
        externalRoot = base.appendingPathComponent("external", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: externalRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        let base = root?.deletingLastPathComponent()
        if let base {
            try? FileManager.default.removeItem(at: base)
        }
    }

    func testInitialImportCopiesExternalFilesAndWritesBaseline() throws {
        try writeExternal("src/main.swift", "print(\"hi\")")
        let repository = ProjectRepository(projectsRoot: root)
        let bookmarkResolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try bookmarkResolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: bookmarkResolver,
            coordinator: PassthroughCoordinator()
        )

        let result = try synchronizer.importInitial(projectId: project.record.id)

        let imported = try String(
            contentsOf: project.workspace.hostURL.appendingPathComponent("src/main.swift"),
            encoding: .utf8
        )
        XCTAssertEqual("print(\"hi\")", imported)
        XCTAssertTrue(result.conflicts.isEmpty)
        XCTAssertEqual(.synced, result.project.record.syncState)
        let baseline = try repository.readBaseline(projectId: project.record.id)
        XCTAssertEqual(["src/main.swift"], baseline.files.keys.sorted())
    }

    func testSyncKeepsLocalAndExternalSchedulerOwnershipFilesSeparate() throws {
        let configDir = MobileUserStorage.directories().configDirName
        let ledger = "\(configDir)/scheduled_owner_reservations.jsonl"
        let lock = "\(configDir)/scheduled_tasks.lock"
        let config = "\(configDir)/config.toml"
        try writeExternal(ledger, "external ledger")
        try writeExternal(lock, "external lock")
        try writeExternal(config, "user configuration")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(repository: repository, bookmarkResolver: resolver,
                                                         coordinator: PassthroughCoordinator())
        _ = try synchronizer.importInitial(projectId: project.record.id)
        let localLedger = project.sourceWorkspace.hostURL.appendingPathComponent(ledger)
        let localLock = project.sourceWorkspace.hostURL.appendingPathComponent(lock)
        XCTAssertFalse(FileManager.default.fileExists(atPath: localLedger.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: localLock.path))
        let localConfig = project.sourceWorkspace.hostURL.appendingPathComponent(config)
        XCTAssertEqual(try String(contentsOf: localConfig, encoding: .utf8), "user configuration")
        try Data("local ledger".utf8).write(to: localLedger)
        try Data("local lock".utf8).write(to: localLock)
        // Even an older baseline containing these paths cannot turn an absent
        // scanned file into a deletion on either side.
        var baseline = try repository.readBaseline(projectId: project.record.id)
        for path in [ledger, lock] {
            baseline.files[path] = ProjectSyncFile(relativePath: path, sha256: "previous", sizeBytes: 1)
        }
        try repository.writeBaseline(projectId: project.record.id, baseline: baseline)
        let backup = ledger + ".__lingxi_backup__" + UUID().uuidString
        try writeExternal(backup, "external backup")
        _ = try synchronizer.reimport(projectId: project.record.id)
        _ = try synchronizer.export(projectId: project.record.id)
        _ = try synchronizer.resolve(projectId: project.record.id, resolution: .keepExternal)
        _ = try synchronizer.resolve(projectId: project.record.id, resolution: .keepInternal)
        XCTAssertEqual(try Data(contentsOf: localLedger), Data("local ledger".utf8))
        XCTAssertEqual(try Data(contentsOf: localLock), Data("local lock".utf8))
        XCTAssertEqual(try Data(contentsOf: externalRoot.appendingPathComponent(ledger)), Data("external ledger".utf8))
        XCTAssertEqual(try Data(contentsOf: externalRoot.appendingPathComponent(lock)), Data("external lock".utf8))
        XCTAssertEqual(try Data(contentsOf: externalRoot.appendingPathComponent(backup)), Data("external backup".utf8))
        try Data("updated configuration".utf8).write(to: localConfig)
        _ = try synchronizer.export(projectId: project.record.id)
        XCTAssertEqual(try String(contentsOf: externalRoot.appendingPathComponent(config), encoding: .utf8),
                       "updated configuration")
    }

    func testSyncExcludesOwnershipFilesInTheRuntimeBrandDirectory() throws {
        let ledger = ".atlas/scheduled_owner_reservations.jsonl"
        let lock = ".atlas/scheduled_tasks.lock"
        let ordinary = ".orion/scheduled_tasks.lock"
        let ownerBackup = ledger + ".__lingxi_backup__" + UUID().uuidString
        let ownerTemp = lock + ".__lingxi_temp__" + UUID().uuidString
        try writeExternal(ledger, "external ledger")
        try writeExternal(lock, "external lock")
        try writeExternal(ordinary, "ordinary file")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = ProjectWorkspaceSynchronizer(repository: repository, configDirName: ".atlas",
            bookmarkResolver: resolver, coordinator: PassthroughCoordinator(), availableCapacity: { _ in Int64.max })

        _ = try synchronizer.importInitial(projectId: project.record.id)
        let localLedger = project.sourceWorkspace.hostURL.appendingPathComponent(ledger)
        let localLock = project.sourceWorkspace.hostURL.appendingPathComponent(lock)
        let localOrdinary = project.sourceWorkspace.hostURL.appendingPathComponent(ordinary)
        XCTAssertFalse(FileManager.default.fileExists(atPath: localLedger.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: localLock.path))
        XCTAssertEqual(try String(contentsOf: localOrdinary, encoding: .utf8), "ordinary file")

        try FileManager.default.createDirectory(at: localLedger.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("local ledger".utf8).write(to: localLedger)
        try Data("local lock".utf8).write(to: localLock)
        try Data("updated file".utf8).write(to: localOrdinary)
        try writeExternal(ownerBackup, "protected backup")
        try writeExternal(ownerTemp, "protected temporary file")
        _ = try synchronizer.export(projectId: project.record.id)
        XCTAssertEqual(try String(contentsOf: externalRoot.appendingPathComponent(ledger), encoding: .utf8), "external ledger")
        XCTAssertEqual(try String(contentsOf: externalRoot.appendingPathComponent(lock), encoding: .utf8), "external lock")
        XCTAssertEqual(try String(contentsOf: externalRoot.appendingPathComponent(ordinary), encoding: .utf8), "updated file")
        XCTAssertEqual(try String(contentsOf: externalRoot.appendingPathComponent(ownerBackup), encoding: .utf8), "protected backup")
        XCTAssertEqual(try String(contentsOf: externalRoot.appendingPathComponent(ownerTemp), encoding: .utf8), "protected temporary file")
    }

    func testInsufficientCapacityLeavesTheExternalSourceUntouched() throws {
        try writeExternal("notes.txt", "original")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(repository: repository, bookmarkResolver: resolver,
                                            coordinator: PassthroughCoordinator(), availableCapacity: { _ in 0 })
        XCTAssertThrowsError(try synchronizer.importInitial(projectId: project.record.id))
        XCTAssertEqual(try Data(contentsOf: externalRoot.appendingPathComponent("notes.txt")), Data("original".utf8))
        XCTAssertFalse(FileManager.default.fileExists(atPath: project.sourceWorkspace.hostURL.appendingPathComponent("notes.txt").path))
    }

    func testReimportDetectsConflictWithoutOverwritingEitherSide() throws {
        try writeExternal("notes.txt", "external-v1")
        let repository = ProjectRepository(projectsRoot: root)
        let bookmarkResolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try bookmarkResolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: bookmarkResolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try Data("internal-v2".utf8).write(to: project.workspace.hostURL.appendingPathComponent("notes.txt"))
        try writeExternal("notes.txt", "external-v2")

        let result = try synchronizer.reimport(projectId: project.record.id)

        XCTAssertEqual(1, result.conflicts.count)
        XCTAssertEqual("notes.txt", result.conflicts.first?.relativePath)
        XCTAssertEqual(
            "internal-v2",
            try String(contentsOf: project.workspace.hostURL.appendingPathComponent("notes.txt"), encoding: .utf8)
        )
        XCTAssertEqual(
            "external-v2",
            try String(contentsOf: externalRoot.appendingPathComponent("notes.txt"), encoding: .utf8)
        )
        XCTAssertEqual(.conflict, result.project.record.syncState)
    }

    func testFailedExternalPromotionRestoresOriginalFileAndCleansArtifacts() throws {
        try writeExternal("notes.txt", "original")
        let repository = ProjectRepository(projectsRoot: root)
        let bookmarkResolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try bookmarkResolver.makeBookmark(for: externalRoot))
        let baselineSynchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: bookmarkResolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try baselineSynchronizer.importInitial(projectId: project.record.id)
        try Data("replacement".utf8).write(to: project.workspace.hostURL.appendingPathComponent("notes.txt"))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: bookmarkResolver,
            coordinator: PassthroughCoordinator(),
            hooks: ProjectExternalTransactionHooks(beforePromoteTemp: {
                throw CocoaError(.fileWriteUnknown)
            })
        )

        XCTAssertThrowsError(try synchronizer.export(projectId: project.record.id))
        XCTAssertEqual(
            "original",
            try String(contentsOf: externalRoot.appendingPathComponent("notes.txt"), encoding: .utf8)
        )
        let remaining = try FileManager.default.contentsOfDirectory(atPath: externalRoot.path)
        XCTAssertFalse(remaining.contains(where: isLingxiSyncArtifact))
    }

    func testExportRescansInsideWriteCoordinationBeforeConflictDecision() throws {
        try writeExternal("notes.txt", "base")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(
            name: "Repo",
            sourceBookmark: try resolver.makeBookmark(for: externalRoot)
        )
        let initial = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try initial.importInitial(projectId: project.record.id)
        try Data("internal-edit".utf8).write(
            to: project.workspace.hostURL.appendingPathComponent("notes.txt")
        )

        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: MutatingWriteCoordinator(
                relativePath: "notes.txt",
                contents: "external-edit-during-coordination"
            )
        )

        let result = try synchronizer.export(projectId: project.record.id)

        XCTAssertEqual(["notes.txt"], result.conflicts.map(\.relativePath))
        XCTAssertEqual(
            "external-edit-during-coordination",
            try String(
                contentsOf: externalRoot.appendingPathComponent("notes.txt"),
                encoding: .utf8
            )
        )
        XCTAssertEqual(.conflict, result.project.record.syncState)
    }

    func testAuthorizationLossPreservesInternalCopyAndMarksProject() throws {
        try writeExternal("notes.txt", "external")
        let repository = ProjectRepository(projectsRoot: root)
        let workingResolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try workingResolver.makeBookmark(for: externalRoot))
        let initial = makeSynchronizer(
            repository: repository,
            bookmarkResolver: workingResolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try initial.importInitial(projectId: project.record.id)
        try Data("device copy".utf8).write(to: project.workspace.hostURL.appendingPathComponent("notes.txt"))

        let brokenResolver = TestBookmarkResolver(shouldFailResolve: true)
        let broken = makeSynchronizer(
            repository: repository,
            bookmarkResolver: brokenResolver,
            coordinator: PassthroughCoordinator()
        )

        let result = try broken.reimport(projectId: project.record.id)

        XCTAssertEqual(.authorizationLost, result.project.record.syncState)
        XCTAssertEqual(
            "device copy",
            try String(contentsOf: project.workspace.hostURL.appendingPathComponent("notes.txt"), encoding: .utf8)
        )
    }

    func testExternalDeletionRemovesInternalFileAndBaseline() throws {
        try writeExternal("nested/notes.txt", "external")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try FileManager.default.removeItem(at: externalRoot.appendingPathComponent("nested/notes.txt"))
        let result = try synchronizer.reimport(projectId: project.record.id)

        XCTAssertFalse(FileManager.default.fileExists(atPath: project.workspace.hostURL.appendingPathComponent("nested/notes.txt").path))
        XCTAssertTrue((try repository.readBaseline(projectId: project.record.id)).files.isEmpty)
        XCTAssertEqual(.synced, result.project.record.syncState)
    }

    func testInternalDeletionRemovesExternalFileAndBaseline() throws {
        try writeExternal("nested/notes.txt", "external")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try FileManager.default.removeItem(at: project.workspace.hostURL.appendingPathComponent("nested/notes.txt"))
        let result = try synchronizer.export(projectId: project.record.id)

        XCTAssertFalse(FileManager.default.fileExists(atPath: externalRoot.appendingPathComponent("nested/notes.txt").path))
        XCTAssertTrue((try repository.readBaseline(projectId: project.record.id)).files.isEmpty)
        XCTAssertEqual(.synced, result.project.record.syncState)
    }

    func testInternalOnlyAdditionWithoutBaselineStillCopiesToExternal() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        let internalURL = project.workspace.hostURL.appendingPathComponent("fresh.swift")
        try Data("print(1)".utf8).write(to: internalURL)

        let result = try synchronizer.export(projectId: project.record.id)

        XCTAssertEqual("print(1)", try String(contentsOf: externalRoot.appendingPathComponent("fresh.swift"), encoding: .utf8))
        XCTAssertEqual(["fresh.swift"], (try repository.readBaseline(projectId: project.record.id)).files.keys.sorted())
        XCTAssertEqual(.synced, result.project.record.syncState)
    }

    func testExternalDeletionVsInternalEditIsConflict() throws {
        try writeExternal("notes.txt", "base")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try FileManager.default.removeItem(at: externalRoot.appendingPathComponent("notes.txt"))
        try Data("internal-edit".utf8).write(to: project.workspace.hostURL.appendingPathComponent("notes.txt"))

        let result = try synchronizer.reimport(projectId: project.record.id)

        XCTAssertEqual(["notes.txt"], result.conflicts.map(\.relativePath))
        XCTAssertTrue(FileManager.default.fileExists(atPath: project.workspace.hostURL.appendingPathComponent("notes.txt").path))
        XCTAssertEqual(.conflict, result.project.record.syncState)
    }

    func testResolveKeepExternalForDeleteConflictDeletesInternalAndClearsBaseline() throws {
        try writeExternal("notes.txt", "base")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try FileManager.default.removeItem(at: externalRoot.appendingPathComponent("notes.txt"))
        try Data("internal-edit".utf8).write(to: project.workspace.hostURL.appendingPathComponent("notes.txt"))

        let result = try synchronizer.resolve(projectId: project.record.id, resolution: .keepExternal)

        XCTAssertFalse(FileManager.default.fileExists(atPath: project.workspace.hostURL.appendingPathComponent("notes.txt").path))
        XCTAssertTrue((try repository.readBaseline(projectId: project.record.id)).files.isEmpty)
        XCTAssertEqual(.synced, result.project.record.syncState)
    }

    func testInternalDeletionVsExternalEditIsConflict() throws {
        try writeExternal("notes.txt", "base")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try FileManager.default.removeItem(at: project.workspace.hostURL.appendingPathComponent("notes.txt"))
        try writeExternal("notes.txt", "external-edit")

        let result = try synchronizer.export(projectId: project.record.id)

        XCTAssertEqual(["notes.txt"], result.conflicts.map(\.relativePath))
        XCTAssertTrue(FileManager.default.fileExists(atPath: externalRoot.appendingPathComponent("notes.txt").path))
        XCTAssertEqual(.conflict, result.project.record.syncState)
    }

    func testResolveKeepInternalForDeleteConflictDeletesExternalAndClearsBaseline() throws {
        try writeExternal("notes.txt", "base")
        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        _ = try synchronizer.importInitial(projectId: project.record.id)

        try FileManager.default.removeItem(at: project.workspace.hostURL.appendingPathComponent("notes.txt"))
        try writeExternal("notes.txt", "external-edit")

        let result = try synchronizer.resolve(projectId: project.record.id, resolution: .keepInternal)

        XCTAssertFalse(FileManager.default.fileExists(atPath: externalRoot.appendingPathComponent("notes.txt").path))
        XCTAssertTrue((try repository.readBaseline(projectId: project.record.id)).files.isEmpty)
        XCTAssertEqual(.synced, result.project.record.syncState)
    }

    func testExternalDeleteRejectsParentSymlinkEscapeAndLeavesOutsideFile() throws {
        let outsideRoot = root.deletingLastPathComponent().appendingPathComponent("outside", isDirectory: true)
        try FileManager.default.createDirectory(at: outsideRoot, withIntermediateDirectories: true)
        let outsideFile = outsideRoot.appendingPathComponent("escaped.txt")
        try Data("outside".utf8).write(to: outsideFile)

        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(name: "Repo", sourceBookmark: try resolver.makeBookmark(for: externalRoot))
        try repository.writeBaseline(
            projectId: project.record.id,
            baseline: ProjectSyncBaseline(files: [
                "linked/escaped.txt": ProjectSyncFile(relativePath: "linked/escaped.txt", sha256: "base", sizeBytes: 7)
            ])
        )
        let linked = externalRoot.appendingPathComponent("linked", isDirectory: true)
        try FileManager.default.createSymbolicLink(at: linked, withDestinationURL: outsideRoot)

        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )

        XCTAssertThrowsError(try synchronizer.export(projectId: project.record.id))
        XCTAssertEqual("outside", try String(contentsOf: outsideFile, encoding: .utf8))
        XCTAssertEqual(
            ["linked/escaped.txt"],
            (try repository.readBaseline(projectId: project.record.id)).files.keys.sorted()
        )
        XCTAssertEqual(.error, try repository.project(projectId: project.record.id).record.syncState)
    }

    func testExportRejectsParentSymlinkEscapeAndDoesNotWriteOutsideExternalRoot() throws {
        let outsideRoot = root.deletingLastPathComponent().appendingPathComponent("outside-export", isDirectory: true)
        try FileManager.default.createDirectory(at: outsideRoot, withIntermediateDirectories: true)

        let repository = ProjectRepository(projectsRoot: root)
        let resolver = TestBookmarkResolver()
        let project = try repository.createImported(
            name: "Repo",
            sourceBookmark: try resolver.makeBookmark(for: externalRoot)
        )
        let internalFile = project.workspace.hostURL.appendingPathComponent("linked/escaped.txt")
        try FileManager.default.createDirectory(
            at: internalFile.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data("device-data".utf8).write(to: internalFile)
        try FileManager.default.createSymbolicLink(
            at: externalRoot.appendingPathComponent("linked", isDirectory: true),
            withDestinationURL: outsideRoot
        )

        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )

        XCTAssertThrowsError(try synchronizer.export(projectId: project.record.id))
        XCTAssertFalse(FileManager.default.fileExists(atPath: outsideRoot.appendingPathComponent("escaped.txt").path))
        XCTAssertTrue((try repository.readBaseline(projectId: project.record.id)).files.isEmpty)
        XCTAssertEqual(.error, try repository.project(projectId: project.record.id).record.syncState)
    }

    func testStaleBookmarkIsRefreshedAndPersisted() throws {
        try writeExternal("notes.txt", "external")
        let repository = ProjectRepository(projectsRoot: root)
        let bookmarkResolver = TestBookmarkResolver(staleOnResolve: true, refreshedBookmarkData: Data("fresh-bookmark".utf8))
        let project = try repository.createImported(name: "Repo", sourceBookmark: try bookmarkResolver.makeBookmark(for: externalRoot))
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: bookmarkResolver,
            coordinator: PassthroughCoordinator()
        )

        let result = try synchronizer.importInitial(projectId: project.record.id)
        let refreshed = try repository.project(projectId: project.record.id)

        XCTAssertEqual(.synced, result.project.record.syncState)
        XCTAssertEqual(Data("fresh-bookmark".utf8), refreshed.record.sourceBookmark?.data)
        XCTAssertEqual(false, refreshed.record.sourceBookmark?.isStale)
    }

    func testStaleBookmarkRemainsRefreshedWhenSyncFailsAfterResolution() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let bookmarkResolver = TestBookmarkResolver(
            staleOnResolve: true,
            refreshedBookmarkData: Data("fresh-after-failure".utf8)
        )
        let project = try repository.createImported(
            name: "Repo",
            sourceBookmark: try bookmarkResolver.makeBookmark(for: externalRoot)
        )
        let synchronizer = makeSynchronizer(
            repository: repository,
            bookmarkResolver: bookmarkResolver,
            coordinator: FailingReadCoordinator()
        )

        XCTAssertThrowsError(try synchronizer.reimport(projectId: project.record.id))

        let persisted = try repository.project(projectId: project.record.id).record
        XCTAssertEqual(.error, persisted.syncState)
        XCTAssertEqual(Data("fresh-after-failure".utf8), persisted.sourceBookmark?.data)
        XCTAssertEqual(false, persisted.sourceBookmark?.isStale)
    }

    func testConflictResolutionOnlyForActualConflictPaths() {
        XCTAssertEqual(
            .copyExternal,
            decideProjectSyncAction(
                direction: .externalToInternal,
                resolution: .keepExternal,
                baselineSha256: "base",
                externalSha256: "external",
                internalSha256: "internal"
            )
        )
        XCTAssertEqual(
            .skip,
            decideProjectSyncAction(
                direction: .externalToInternal,
                resolution: .keepExternal,
                baselineSha256: "base",
                externalSha256: "base",
                internalSha256: "local-change"
            )
        )
        XCTAssertEqual(
            .skip,
            decideProjectSyncAction(
                direction: .internalToExternal,
                resolution: .keepInternal,
                baselineSha256: "base",
                externalSha256: "external-change",
                internalSha256: "base"
            )
        )
        XCTAssertEqual(
            .deleteInternal,
            decideProjectSyncAction(
                direction: .externalToInternal,
                resolution: nil,
                baselineSha256: "base",
                externalSha256: nil,
                internalSha256: "base"
            )
        )
        XCTAssertEqual(
            .deleteExternal,
            decideProjectSyncAction(
                direction: .internalToExternal,
                resolution: nil,
                baselineSha256: "base",
                externalSha256: "base",
                internalSha256: nil
            )
        )
        XCTAssertEqual(
            .deleteInternal,
            decideProjectSyncAction(
                direction: .externalToInternal,
                resolution: .keepExternal,
                baselineSha256: "base",
                externalSha256: nil,
                internalSha256: "internal-edit"
            )
        )
        XCTAssertEqual(
            .deleteExternal,
            decideProjectSyncAction(
                direction: .internalToExternal,
                resolution: .keepInternal,
                baselineSha256: "base",
                externalSha256: "external-edit",
                internalSha256: nil
            )
        )
    }

    private func makeSynchronizer(
        repository: ProjectRepository, bookmarkResolver: any ProjectBookmarkResolving,
        coordinator: any ProjectPathCoordinating, hooks: ProjectExternalTransactionHooks = .init(),
        availableCapacity: @escaping @Sendable (URL) throws -> Int64 = { _ in Int64.max }
    ) -> ProjectWorkspaceSynchronizer {
        ProjectWorkspaceSynchronizer(repository: repository, bookmarkResolver: bookmarkResolver,
                                     coordinator: coordinator, hooks: hooks, availableCapacity: availableCapacity)
    }

    private func writeExternal(_ relativePath: String, _ contents: String) throws {
        let url = externalRoot.appendingPathComponent(relativePath)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(contents.utf8).write(to: url)
    }
}

private struct PassthroughCoordinator: ProjectPathCoordinating {
    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T { try block(url) }
    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T { try block(url) }
}

private struct FailingReadCoordinator: ProjectPathCoordinating {
    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        throw CocoaError(.fileReadUnknown)
    }

    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        try block(url)
    }
}

private struct MutatingWriteCoordinator: ProjectPathCoordinating {
    let relativePath: String
    let contents: String

    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        try block(url)
    }

    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        try Data(contents.utf8).write(to: url.appendingPathComponent(relativePath))
        return try block(url)
    }
}

private struct TestBookmarkResolver: ProjectBookmarkResolving {
    var shouldFailResolve = false
    var staleOnResolve = false
    var refreshedBookmarkData: Data? = nil

    func withInitialAccess<T>(to directoryURL: URL, _ body: (URL) throws -> T) throws -> T {
        try body(directoryURL)
    }

    func makeBookmark(for directoryURL: URL) throws -> ProjectExternalBookmark {
        ProjectExternalBookmark(
            data: Data(directoryURL.path.utf8),
            displayName: directoryURL.lastPathComponent,
            pathHint: directoryURL.path,
            isStale: false
        )
    }

    func refreshBookmark(for resolved: ResolvedProjectBookmark) throws -> ProjectExternalBookmark {
        ProjectExternalBookmark(
            data: refreshedBookmarkData ?? Data(resolved.url.path.utf8),
            displayName: resolved.url.lastPathComponent,
            pathHint: resolved.url.path,
            isStale: false
        )
    }

    func resolve(_ bookmark: ProjectExternalBookmark) throws -> ResolvedProjectBookmark {
        if shouldFailResolve {
            throw ProjectBookmarkError.authorizationDenied
        }
        let path = String(decoding: bookmark.data, as: UTF8.self)
        return ResolvedProjectBookmark(url: URL(fileURLWithPath: path), isStale: staleOnResolve || bookmark.isStale)
    }
}

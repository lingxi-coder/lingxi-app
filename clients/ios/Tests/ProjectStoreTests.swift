import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class ProjectStoreTests: XCTestCase {
    private var managedRoot: URL!
    private var externalRoot: URL!

    override func setUpWithError() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("ios-project-store-\(UUID().uuidString)", isDirectory: true)
        managedRoot = base.appendingPathComponent("managed", isDirectory: true)
        externalRoot = base.appendingPathComponent("external", isDirectory: true)
        try FileManager.default.createDirectory(at: managedRoot, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: externalRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        if let base = managedRoot?.deletingLastPathComponent() {
            try? FileManager.default.removeItem(at: base)
        }
    }

    func testImportUsesInitialScopeAcrossBookmarkAndResolveAndStopsAfterward() async throws {
        try writeExternal("notes.txt", "external")
        let repository = ProjectRepository(projectsRoot: managedRoot)
        let trackingResolver = TrackingBookmarkResolver()
        let synchronizer = ProjectWorkspaceSynchronizer(
            repository: repository,
            bookmarkResolver: trackingResolver,
            coordinator: PassthroughCoordinator()
        )
        let store = ProjectStore(repository: repository, synchronizer: synchronizer, bookmarkResolver: trackingResolver)

        let project = try await store.importExternal(name: "Repo", directoryURL: externalRoot)

        XCTAssertEqual(project.id, store.projects.single?.id)
        XCTAssertEqual(1, trackingResolver.initialStartCount)
        XCTAssertEqual(1, trackingResolver.initialStopCount)
        XCTAssertEqual([1], trackingResolver.makeBookmarkDepths)
        XCTAssertEqual([1], trackingResolver.resolveDepths)
    }

    func testImportFailureRollsBackNewProject() async {
        let repository = ProjectRepository(projectsRoot: managedRoot)
        let resolver = TrackingBookmarkResolver()
        let synchronizer = ProjectWorkspaceSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: FailingCoordinator(failRead: true)
        )
        let store = ProjectStore(repository: repository, synchronizer: synchronizer, bookmarkResolver: resolver)

        await XCTAssertThrowsErrorAsync {
            _ = try await store.importExternal(name: "Repo", directoryURL: self.externalRoot)
        }

        XCTAssertTrue(store.projects.isEmpty)
        XCTAssertTrue(repository.load().projects.isEmpty)
    }

    func testReauthorizeFailureRestoresOriginalBookmarkAndState() async throws {
        try writeExternal("notes.txt", "external")
        let repository = ProjectRepository(projectsRoot: managedRoot)
        let initialResolver = TrackingBookmarkResolver()
        let initialSync = ProjectWorkspaceSynchronizer(
            repository: repository,
            bookmarkResolver: initialResolver,
            coordinator: PassthroughCoordinator()
        )
        let store = ProjectStore(repository: repository, synchronizer: initialSync, bookmarkResolver: initialResolver)
        let project = try await store.importExternal(name: "Repo", directoryURL: externalRoot)
        let originalRecord = try repository.project(projectId: project.id).record

        let replacementRoot = managedRoot.deletingLastPathComponent().appendingPathComponent("replacement", isDirectory: true)
        try FileManager.default.createDirectory(at: replacementRoot, withIntermediateDirectories: true)
        let failingResolver = TrackingBookmarkResolver(bookmarkDataForMake: Data(replacementRoot.path.utf8))
        let failingSync = ProjectWorkspaceSynchronizer(
            repository: repository,
            bookmarkResolver: failingResolver,
            coordinator: FailingCoordinator(failRead: true)
        )
        let failingStore = ProjectStore(repository: repository, synchronizer: failingSync, bookmarkResolver: failingResolver)
        failingStore.publishActive(repository.load())

        await failingStore.reauthorizeAsync(projectId: project.id, directoryURL: replacementRoot)

        let restored = try repository.project(projectId: project.id).record
        XCTAssertEqual(originalRecord.sourceBookmark?.data, restored.sourceBookmark?.data)
        XCTAssertEqual(originalRecord.syncState, restored.syncState)
        XCTAssertEqual(1, failingResolver.initialStartCount)
        XCTAssertEqual(1, failingResolver.initialStopCount)
    }

    func testAsyncImportSetsOperationBeforeBackgroundWorkCompletes() async throws {
        let gate = DispatchSemaphore(value: 0)
        let repository = ProjectRepository(projectsRoot: managedRoot)
        let resolver = TrackingBookmarkResolver(blockingSemaphore: gate)
        let synchronizer = ProjectWorkspaceSynchronizer(
            repository: repository,
            bookmarkResolver: resolver,
            coordinator: PassthroughCoordinator()
        )
        let store = ProjectStore(repository: repository, synchronizer: synchronizer, bookmarkResolver: resolver)

        let task = Task {
            try await store.importExternal(name: "Repo", directoryURL: self.externalRoot)
        }

        for _ in 0..<20 where store.operation == nil {
            await Task.yield()
        }
        XCTAssertEqual(ProjectOperationKind.import, store.operation?.kind)
        gate.signal()
        _ = try await task.value
        XCTAssertNil(store.operation)
    }

    private func writeExternal(_ relativePath: String, _ contents: String) throws {
        let url = externalRoot.appendingPathComponent(relativePath)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(contents.utf8).write(to: url)
    }
}

private final class TrackingBookmarkResolver: ProjectBookmarkResolving, @unchecked Sendable {
    private let lock = NSLock()
    private var initialDepth = 0
    private(set) var initialStartCount = 0
    private(set) var initialStopCount = 0
    private(set) var makeBookmarkDepths: [Int] = []
    private(set) var resolveDepths: [Int] = []
    private let bookmarkDataForMake: Data?
    private let blockingSemaphore: DispatchSemaphore?

    init(bookmarkDataForMake: Data? = nil, blockingSemaphore: DispatchSemaphore? = nil) {
        self.bookmarkDataForMake = bookmarkDataForMake
        self.blockingSemaphore = blockingSemaphore
    }

    func withInitialAccess<T>(to directoryURL: URL, _ body: (URL) throws -> T) throws -> T {
        blockingSemaphore?.wait()
        lock.lock()
        initialDepth += 1
        initialStartCount += 1
        lock.unlock()
        defer {
            lock.lock()
            initialDepth -= 1
            initialStopCount += 1
            lock.unlock()
        }
        return try body(directoryURL)
    }

    func makeBookmark(for directoryURL: URL) throws -> ProjectExternalBookmark {
        lock.lock()
        makeBookmarkDepths.append(initialDepth)
        lock.unlock()
        return ProjectExternalBookmark(
            data: bookmarkDataForMake ?? Data(directoryURL.path.utf8),
            displayName: directoryURL.lastPathComponent,
            pathHint: directoryURL.path,
            isStale: false
        )
    }

    func refreshBookmark(for resolved: ResolvedProjectBookmark) throws -> ProjectExternalBookmark {
        try makeBookmark(for: resolved.url)
    }

    func resolve(_ bookmark: ProjectExternalBookmark) throws -> ResolvedProjectBookmark {
        lock.lock()
        resolveDepths.append(initialDepth)
        lock.unlock()
        return ResolvedProjectBookmark(
            url: URL(fileURLWithPath: String(decoding: bookmark.data, as: UTF8.self)),
            isStale: false
        )
    }
}

private struct FailingCoordinator: ProjectPathCoordinating {
    var failRead: Bool = false

    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        if failRead {
            throw CocoaError(.fileReadUnknown)
        }
        return try block(url)
    }

    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        try block(url)
    }
}

private struct PassthroughCoordinator: ProjectPathCoordinating {
    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T { try block(url) }
    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T { try block(url) }
}

private extension Array {
    var single: Element? { count == 1 ? first : nil }
}

private func XCTAssertThrowsErrorAsync(
    _ expression: @escaping () async throws -> some Any,
    file: StaticString = #filePath,
    line: UInt = #line
) async {
    do {
        _ = try await expression()
        XCTFail("expected error", file: file, line: line)
    } catch {
    }
}

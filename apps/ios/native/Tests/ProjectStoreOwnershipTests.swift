import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class ProjectStoreOwnershipTests: XCTestCase {
    func testMissingIndexesWithPersistedOwnerClaimsDoNotBecomeFreshCatalogs() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let id = "11111111-1111-4111-8111-111111111111"
        let indexURL = root.appendingPathComponent(globalSessionIndexFileName)
        store.sessionScopeClaims = { [id: .global] }
        XCTAssertTrue(store.sessionOwnershipError?.contains("global") == true)
        XCTAssertFalse(store.allowsSession(id, in: .global, mode: .chat))
        do {
            try await store.recordStartedSession(projectId: nil, sessionId: id, title: "Must not replace", mode: .chat)
            XCTFail("A missing global index with an existing owner must not be recreated")
        } catch {}
        XCTAssertFalse(FileManager.default.fileExists(atPath: indexURL.path))

        store.sessionScopeClaims = { [id: .project("p")] }
        XCTAssertTrue(store.sessionOwnershipError?.contains("projects-index") == true)
        XCTAssertFalse(store.allowsSession(id, in: .project("p"), mode: .chat))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(projectIndexFileName).path))
    }

    func testFirstGlobalSessionWritesItsIndexBeforePersistingOwnerClaim() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let id = "11111111-1111-4111-8111-111111111111"
        store.sessionScopeClaims = { [:] }
        XCTAssertTrue(store.allowsSession(id, in: .global, mode: .chat))
        XCTAssertFalse(store.isSessionIndexed(id, in: .global))
        try await store.recordStartedSession(projectId: nil, sessionId: id, title: "First", mode: .chat)
        XCTAssertTrue(store.isSessionIndexed(id, in: .global))
        store.sessionScopeClaims = { [id: .global] }
        XCTAssertNil(store.sessionOwnershipError)
        XCTAssertTrue(store.allowsSession(id, in: .global, mode: .chat))
    }

    func testIndexRemovedAfterLoadingIsNotRecreatedOrUsedForNewClaims() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let id = "11111111-1111-4111-8111-111111111111"
        _ = try repository.recordStartedSession(projectId: nil, sessionId: id, title: "Existing", mode: .chat)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        store.sessionScopeClaims = { [id: .global] }
        let index = root.appendingPathComponent(globalSessionIndexFileName)
        try FileManager.default.removeItem(at: index)
        XCTAssertTrue(store.sessionOwnershipError?.contains("global") == true)
        XCTAssertFalse(store.allowsSession("22222222-2222-4222-8222-222222222222", in: .global, mode: .chat))
        XCTAssertNotNil(repository.load().errorMessage)
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: []))
        XCTAssertFalse(FileManager.default.fileExists(atPath: index.path))
    }

    func testRegistryRemovedAfterLoadingFreezesNewOwners() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let suite = "ProjectStoreOwnershipTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = ProjectScopedPreferences(defaults: defaults)
        let repository = ProjectRepository(projectsRoot: root)
        let id = "11111111-1111-4111-8111-111111111111"
        _ = try repository.recordStartedSession(projectId: nil, sessionId: id, title: "Existing", mode: .chat)
        XCTAssertTrue(preferences.claimSessionScope(sessionID: id, scope: .global))
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        store.sessionScopeClaims = { preferences.sessionScopeClaims }
        XCTAssertTrue(store.allowsSession(id, in: .global, mode: .chat))
        defaults.removeObject(forKey: "conversation.spaces.session-scopes")
        XCTAssertTrue(store.sessionOwnershipError?.contains("session-scopes") == true)
        XCTAssertFalse(store.allowsSession("22222222-2222-4222-8222-222222222222", in: .global, mode: .chat))
        XCTAssertNil(defaults.object(forKey: "conversation.spaces.session-scopes"))
    }

    func testFreshAdmissionRejectsAnIndexCorruptedAfterTheUIWasLoaded() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Existing")
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let id = "11111111-1111-4111-8111-111111111111"
        XCTAssertTrue(store.admitsSessionIdentity(id, in: .global, mode: .chat))
        XCTAssertTrue(store.admitsUnclaimedOwner())
        let index = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let malformed = Data("{".utf8)
        try malformed.write(to: index)
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .global, mode: .chat))
        XCTAssertFalse(store.admitsUnclaimedOwner())
        XCTAssertTrue(store.errorMessage?.contains("project.\(project.id)") == true)
        XCTAssertEqual(try Data(contentsOf: index), malformed)
    }

    func testConflictingColdCatalogRefusesEveryOwnerEvenAfterErrorDismissal() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let id = "11111111-1111-4111-8111-111111111111"
        let row = ProjectSessionSummary(sessionId: id, title: "Chat", messageCount: 1,
                                        relativeTime: "Now", updatedAt: Date(), mode: .chat)
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        let globalURL = root.appendingPathComponent(globalSessionIndexFileName)
        let projectURL = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let original = try Data(contentsOf: globalURL)
        try original.write(to: projectURL)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        XCTAssertNotNil(store.errorMessage)
        store.sessionScopeClaims = { [id: .global] }
        XCTAssertFalse(store.allowsSession(id, in: .global, mode: .chat))
        XCTAssertFalse(store.allowsSession(id, in: .project(project.id), mode: .chat))
        store.clearError()
        XCTAssertFalse(store.allowsSession(id, in: .global, mode: .chat))
        XCTAssertEqual(try Data(contentsOf: globalURL), original)
        XCTAssertEqual(try Data(contentsOf: projectURL), original)
    }

    func testClaimAndRegistryCannotOverrideAnIndexedOwner() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let id = "11111111-1111-4111-8111-111111111111"
        let row = ProjectSessionSummary(sessionId: id, title: "Chat", messageCount: 1,
                                        relativeTime: "Now", updatedAt: Date(), mode: .chat)
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let indexURL = root.appendingPathComponent(globalSessionIndexFileName)
        let bytes = try Data(contentsOf: indexURL)
        XCTAssertTrue(store.allowsSession(id, in: .global, mode: .chat))
        XCTAssertFalse(store.allowsSession(id, in: .global, mode: .code))
        store.sessionScopeClaims = { [id: .project("p")] }
        XCTAssertTrue(store.sessionOwnershipError?.contains("global") == true)
        XCTAssertTrue(store.sessionOwnershipError?.contains("project.p") == true)
        XCTAssertFalse(store.allowsSession(id, in: .project("p"), mode: .chat))
        XCTAssertFalse(store.allowsSession(id, in: .global, mode: .chat))
        XCTAssertFalse(store.allowsSession("22222222-2222-4222-8222-222222222222", in: .global, mode: .chat))
        do {
            try await store.syncEngineSessions(projectId: nil, rows: [])
            XCTFail("A conflicting claim must not erase the existing catalog")
        } catch {}
        XCTAssertEqual(try Data(contentsOf: indexURL), bytes)
        do {
            _ = try await store.createInternal(name: "Must not write")
            XCTFail("An owner conflict must prevent new project index writes")
        } catch {}
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(projectIndexFileName).path))
        store.sessionScopeClaims = { nil }
        XCTAssertTrue(store.sessionOwnershipError?.contains("session-scopes") == true)
        XCTAssertFalse(store.allowsSession(id, in: .global, mode: .chat))
        do {
            try await store.syncEngineSessions(projectId: nil, rows: [])
            XCTFail("An invalid owner registry must not erase the existing catalog")
        } catch {}
        XCTAssertEqual(try Data(contentsOf: indexURL), bytes)
    }
}

import Foundation
import Darwin
import XCTest
@testable import LingxiCode

@MainActor
final class ProjectStoreOwnershipTests: XCTestCase {
    func testWorkspaceCommitDoesNotRecreateOrReplaceAnIndexChangedDuringStaging() async throws {
        let cases = [false, true].flatMap { manifest in
            ["delete", "replace", "edit", "fifo"].flatMap { change in
                [false, true].map { beforeCommit in (manifest, change, beforeCommit) }
            }
        }
        for (manifest, change, beforeCommit) in cases {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: root) }
            let staged = expectation(description: "workspace file staged")
            let release = DispatchSemaphore(value: 0)
            let fallback = DispatchWorkItem { release.signal() }
            defer { fallback.cancel(); release.signal() }
            let writer = BlockingWorkspaceWriter(staged: staged, release: release, beforeCommit: beforeCommit)
            let repository = ProjectRepository(projectsRoot: root, writer: writer)
            let project = try repository.createInternal(name: "Project")
            let id = "11111111-1111-4111-8111-111111111111"
            let path = "/catalog/original.jsonl"
            if !manifest {
                let row = ProjectSessionSummary(sessionId: id, title: "Owner", messageCount: 1,
                    relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: path)
                _ = try repository.updateSessions(projectId: project.id, sessions: [row])
            }
            let target = root.appendingPathComponent(project.id)
                .appendingPathComponent(manifest ? projectManifestFileName : projectSessionIndexFileName)
            let original = try Data(contentsOf: target)
            let originalFileID = try FileManager.default.attributesOfItem(atPath: target.path)[.systemFileNumber] as? NSNumber
            let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
            let authority = WorkspaceWriteAuthority()
            authority.setSelected(true)
            let state = SessionWorkspaceState(sessionId: id, cwd: project.record.directory,
                projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: path)
            writer.arm(target: target)
            DispatchQueue.global().asyncAfter(deadline: .now() + 3, execute: fallback)
            let pending = Task {
                try await store.recordAuthenticatedWorkspace(state, in: .project(project.id), mode: .chat,
                    authority: authority, revision: authority.currentRevision())
            }
            await fulfillment(of: [staged], timeout: 2)
            let saved = root.appendingPathComponent("saved-original")
            if change == "replace" {
                try FileManager.default.moveItem(at: target, to: saved)
                try Data("replacement bytes".utf8).write(to: target)
            } else if change == "edit" {
                let file = try FileHandle(forWritingTo: target)
                try file.truncate(atOffset: 0)
                try file.write(contentsOf: Data("replacement bytes".utf8))
                try file.close()
                XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: target.path)[.systemFileNumber] as? NSNumber,
                               originalFileID, "the edit must preserve the file identity")
            } else if change == "fifo" {
                try FileManager.default.removeItem(at: target)
                guard target.path.withCString({ mkfifo($0, mode_t(0o600)) }) == 0 else {
                    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                }
            } else {
                try FileManager.default.removeItem(at: target)
            }
            release.signal()
            do {
                try await pending.value
                XCTFail("A changed workspace file must not be replaced")
            } catch {}
            if change == "replace" {
                XCTAssertEqual(try Data(contentsOf: target), Data("replacement bytes".utf8))
                XCTAssertEqual(try Data(contentsOf: saved), original)
            } else if change == "edit" {
                XCTAssertEqual(try Data(contentsOf: target), Data("replacement bytes".utf8))
            } else if change == "fifo" {
                var information = stat()
                XCTAssertEqual(target.path.withCString { lstat($0, &information) }, 0)
                XCTAssertEqual(information.st_mode & mode_t(S_IFMT), mode_t(S_IFIFO))
            } else {
                XCTAssertFalse(FileManager.default.fileExists(atPath: target.path))
            }
        }
    }

    func testWorkspaceStagingDoesNotBlockNavigationOrCommitAStaleRevision() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let staged = expectation(description: "workspace index staged before rename")
        let release = DispatchSemaphore(value: 0)
        let fallback = DispatchWorkItem { release.signal() }
        defer { fallback.cancel(); release.signal() }
        let writer = BlockingWorkspaceWriter(staged: staged, release: release)
        let repository = ProjectRepository(projectsRoot: root, writer: writer)
        let project = try repository.createInternal(name: "Project")
        let linked = root.appendingPathComponent("linked", isDirectory: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: true)
        let id = "11111111-1111-4111-8111-111111111111"
        let path = "/catalog/original.jsonl"
        let row = ProjectSessionSummary(sessionId: id, title: "Owner", messageCount: 1, relativeTime: "Now",
            updatedAt: Date(), mode: .chat, workspaceDirectory: project.record.directory, transcriptPath: path)
        _ = try repository.updateSessions(projectId: project.id, sessions: [row])
        let index = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let bytes = try Data(contentsOf: index)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        first.model.activeSessionId = id
        let initial = SessionWorkspaceState(sessionId: id, cwd: project.record.directory,
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: path)
        first.model.workspaceState = initial
        let firstEntry = try host.retain(source: first, scope: .project(project.id), mode: .chat,
                                         sessionID: id, workspace: project.record.directory, draft: "")
        let second = MockConversationSource(sessionMode: .code)
        second.model.activeSessionId = "22222222-2222-4222-8222-222222222222"
        let secondEntry = try host.retain(source: second, scope: .global, mode: .code,
            sessionID: second.model.activeSessionId, workspace: "/home/other", draft: "")
        host.select(firstEntry)
        let state = SessionWorkspaceState(sessionId: id, cwd: LXISHGuestPaths.workspace("projects") + "/linked",
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: path)
        let authority = firstEntry.workspaceWriteAuthority
        let revision = authority.currentRevision()
        writer.arm(target: index)
        DispatchQueue.global().asyncAfter(deadline: .now() + 3, execute: fallback)
        let pending = Task {
            try await store.recordAuthenticatedWorkspace(state, in: .project(project.id), mode: .chat,
                                                         authority: authority, revision: revision)
        }
        await fulfillment(of: [staged], timeout: 2)
        let started = Date()
        host.select(secondEntry)
        first.model.workspaceState = state
        XCTAssertLessThan(Date().timeIntervalSince(started), 2, "staging I/O must not hold the UI authority lock")
        release.signal()
        do {
            try await pending.value
            XCTFail("A staged workspace with a stale revision must not replace the index")
        } catch WorkspaceWriteAuthority.Failure.stale {} catch { XCTFail("Unexpected failure: \(error)") }
        XCTAssertEqual(try Data(contentsOf: index), bytes)
        let files = try FileManager.default.contentsOfDirectory(atPath: index.deletingLastPathComponent().path)
        XCTAssertFalse(files.contains(where: { $0.hasPrefix(".\(projectSessionIndexFileName).") && $0.hasSuffix(".tmp") }))
    }

    func testQueuedWorkspaceWriteDoesNotBlockNavigationOrOverwriteANewerOwnerEpoch() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let linked = root.appendingPathComponent("linked", isDirectory: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: true)
        let domain = ProjectStoreExecutionDomain()
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository),
                                 executionDomain: domain)
        let gate = CronOwnerAdmissionGate()
        store.beginCatalogUpdate = { try await gate.beginCatalogUpdate(sessionIDs: $0) }
        store.finishCatalogUpdate = { gate.finish($0) }
        let id = "11111111-1111-4111-8111-111111111111"
        let otherID = "22222222-2222-4222-8222-222222222222"
        let path = "/catalog/original.jsonl"
        let row = ProjectSessionSummary(sessionId: id, title: "Owner", messageCount: 1, relativeTime: "Now",
            updatedAt: Date(), mode: .chat, workspaceDirectory: project.record.directory, transcriptPath: path)
        _ = try repository.updateSessions(projectId: project.id, sessions: [row])
        await store.reloadAsync()
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        first.model.activeSessionId = id
        first.model.workspaceState = SessionWorkspaceState(sessionId: id, cwd: project.record.directory,
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: path)
        let firstEntry = try host.retain(source: first, scope: .project(project.id), mode: .chat,
                                         sessionID: id, workspace: project.record.directory, draft: "")
        let second = MockConversationSource(sessionMode: .code)
        second.model.activeSessionId = otherID
        let secondEntry = try host.retain(source: second, scope: .global, mode: .code,
                                          sessionID: otherID, workspace: "/home/other", draft: "")
        host.select(firstEntry)
        let authority = firstEntry.workspaceWriteAuthority
        let revision = authority.currentRevision()
        let stale = SessionWorkspaceState(sessionId: id, cwd: LXISHGuestPaths.workspace("projects") + "/linked",
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: path)
        let entered = expectation(description: "long project operation holds its queue")
        let release = DispatchSemaphore(value: 0)
        let fallback = DispatchWorkItem { release.signal() }
        DispatchQueue.global().asyncAfter(deadline: .now() + 3, execute: fallback)
        defer { fallback.cancel(); release.signal() }
        let hold = Task { try await domain.async { entered.fulfill(); release.wait() } }
        await fulfillment(of: [entered], timeout: 2)
        let pending = Task {
            try await store.recordAuthenticatedWorkspace(stale, in: .project(project.id), mode: .chat,
                                                         authority: authority, revision: revision)
        }
        for _ in 0..<200 where gate.allowsNewOwner(sessionID: id) { await Task.yield() }
        XCTAssertFalse(gate.allowsNewOwner(sessionID: id))
        let started = Date()
        host.select(secondEntry)
        first.model.workspaceState = SessionWorkspaceState(sessionId: id, cwd: "/home/lingxi/newer",
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: path)
        XCTAssertLessThan(Date().timeIntervalSince(started), 2, "navigation must not wait for the project queue")
        XCTAssertEqual(host.selectedEntryID, secondEntry.id)
        let otherToken = UUID()
        XCTAssertTrue(gate.begin(sessionID: otherID, token: otherToken))
        gate.finish(otherToken)
        release.signal()
        try await hold.value
        do {
            try await pending.value
            XCTFail("A queued workspace from the previous revision must not commit")
        } catch WorkspaceWriteAuthority.Failure.stale {} catch { XCTFail("Unexpected failure: \(error)") }
        let restored = try repository.project(projectId: project.id)
        XCTAssertEqual(restored.sessions.first?.workspaceDirectory, project.record.directory)
        XCTAssertEqual(restored.record.activeDirectory, project.record.directory)
        XCTAssertEqual(restored.sessions.first?.transcriptPath, path)

        // A navigation alone does not discard the old owner's workspace,
        // but it must not replace the newly selected project's default cwd.
        first.model.workspaceState = stale
        host.select(firstEntry)
        let currentRevision = authority.currentRevision()
        let enteredAgain = expectation(description: "project queue held again")
        let releaseAgain = DispatchSemaphore(value: 0)
        let fallbackAgain = DispatchWorkItem { releaseAgain.signal() }
        DispatchQueue.global().asyncAfter(deadline: .now() + 3, execute: fallbackAgain)
        defer { fallbackAgain.cancel(); releaseAgain.signal() }
        let holdAgain = Task { try await domain.async { enteredAgain.fulfill(); releaseAgain.wait() } }
        await fulfillment(of: [enteredAgain], timeout: 2)
        let backgroundUpdate = Task {
            try await store.recordAuthenticatedWorkspace(stale, in: .project(project.id), mode: .chat,
                                                         authority: authority, revision: currentRevision)
        }
        for _ in 0..<200 where gate.allowsNewOwner(sessionID: id) { await Task.yield() }
        XCTAssertFalse(gate.allowsNewOwner(sessionID: id))
        host.select(secondEntry)
        releaseAgain.signal()
        try await holdAgain.value
        try await backgroundUpdate.value
        let background = try repository.project(projectId: project.id)
        XCTAssertEqual(background.sessions.first?.workspaceDirectory, stale.cwd)
        XCTAssertEqual(background.record.activeDirectory, project.record.directory)
    }

    #if !canImport(harness_runtimeFFI)
    func testWorkspaceQueryRetriesWhenANewerRevisionArrivesWhileSuspended() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let id = "11111111-1111-4111-8111-111111111111"
        let path = "/catalog/original.jsonl"
        let row = ProjectSessionSummary(sessionId: id, title: "Owner", messageCount: 1, relativeTime: "Now",
            updatedAt: Date(), mode: .chat, workspaceDirectory: "/home/old", transcriptPath: path)
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let old = SessionWorkspaceState(sessionId: id, cwd: "/home/old", projectRoot: "/home",
                                        additionalDirectories: [], transcriptPath: path)
        let new = SessionWorkspaceState(sessionId: id, cwd: "/home/new", projectRoot: "/home",
                                        additionalDirectories: [], transcriptPath: path)
        let started = expectation(description: "old native read is suspended")
        let reader = SuspendedWorkspaceReader(initial: old, started: started)
        let source = MockConversationSource(sessionMode: .chat)
        source.model.activeSessionId = id
        source.model.workspaceState = old
        source.workspaceRead = { _, _ in await reader.read() }
        let host = SessionHost()
        let entry = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: id, workspace: old.cwd, draft: "")
        host.select(entry)
        let read = Task { await host.authenticatedWorkspaceSnapshot(for: entry) }
        await fulfillment(of: [started], timeout: 2)
        source.model.workspaceState = new
        await reader.setCurrent(new)
        await reader.release()
        let result = await read.value
        let snapshot = try XCTUnwrap(result)
        XCTAssertEqual(snapshot.state.cwd, new.cwd)
        try await store.recordAuthenticatedWorkspace(snapshot.state, in: .global, mode: .chat,
            authority: entry.workspaceWriteAuthority, revision: snapshot.revision)
        XCTAssertEqual(store.globalSessions.first?.workspaceDirectory, new.cwd)
    }

    func testCatalogCannotClaimAnotherCopyOfALiveUnindexedOwner() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let gate = CronOwnerAdmissionGate()
        let host = SessionHost()
        store.beginCatalogUpdate = { try await gate.beginCatalogUpdate(sessionIDs: $0) }
        store.finishCatalogUpdate = { gate.finish($0) }
        store.validateCatalogRows = { rows, scope in
            for row in rows {
                guard await host.validatesCatalogSource(sessionID: row.sessionId, scope: scope,
                    mode: row.mode, transcriptPath: row.transcriptPath) else {
                    throw ProjectRepositoryError.conflictingSessionScope
                }
            }
        }
        let id = "11111111-1111-4111-8111-111111111111"
        let source = MockConversationSource(sessionMode: .chat)
        source.model.activeSessionId = id
        source.model.workspaceState = SessionWorkspaceState(sessionId: id, cwd: "/home/owner",
            projectRoot: "/home/owner", additionalDirectories: [], transcriptPath: "/catalog/a.jsonl")
        _ = try host.retain(source: source, scope: .global, mode: .chat, sessionID: id, workspace: "/home/owner", draft: "")
        var row = ProjectSessionSummary(sessionId: id, title: "Other copy", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/catalog/b.jsonl")
        do {
            try await store.syncEngineSessions(projectId: nil, rows: [row])
            XCTFail("A live owner must reject another source before its first row exists")
        } catch {}
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(globalSessionIndexFileName).path))
        row.transcriptPath = "/catalog/a.jsonl"
        try await store.syncEngineSessions(projectId: nil, rows: [row])
        XCTAssertEqual(store.globalSessions.first?.transcriptPath, "/catalog/a.jsonl")
    }
    #endif

    func testCatalogWriteReservesOnlyItsUUIDUntilTheIndexIsPublished() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let domain = ProjectStoreExecutionDomain()
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository),
                                 executionDomain: domain)
        let gate = CronOwnerAdmissionGate()
        store.beginCatalogUpdate = { try await gate.beginCatalogUpdate(sessionIDs: $0) }
        store.finishCatalogUpdate = { gate.finish($0) }
        let id = "11111111-1111-4111-8111-111111111111"
        let other = "22222222-2222-4222-8222-222222222222"
        let row = ProjectSessionSummary(sessionId: id, title: "Listed", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/catalog/a.jsonl")
        let entered = expectation(description: "index queue held")
        let release = DispatchSemaphore(value: 0)
        defer { release.signal() }
        let hold = Task {
            try await domain.async { entered.fulfill(); release.wait() }
        }
        await fulfillment(of: [entered], timeout: 2)
        let update = Task { try await store.syncEngineSessions(projectId: nil, rows: [row]) }
        for _ in 0..<200 where gate.allowsNewOwner(sessionID: id) { await Task.yield() }
        XCTAssertFalse(gate.allowsNewOwner(sessionID: id))
        XCTAssertFalse(gate.begin(sessionID: id, token: UUID()))
        let otherToken = UUID()
        XCTAssertTrue(gate.begin(sessionID: other, token: otherToken), "an unrelated owner must remain available")
        gate.finish(otherToken)
        release.signal()
        try await hold.value
        try await update.value
        XCTAssertEqual(store.globalSessions.first?.transcriptPath, "/catalog/a.jsonl")
        XCTAssertTrue(gate.allowsNewOwner(sessionID: id))
    }

    func testCatalogRevalidatesDurableOwnerAfterAnEarlierSchedulerReservation() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        _ = try repository.updateSessions(projectId: nil, sessions: [])
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let gate = CronOwnerAdmissionGate()
        store.beginCatalogUpdate = { try await gate.beginCatalogUpdate(sessionIDs: $0) }
        store.finishCatalogUpdate = { gate.finish($0) }
        let id = "11111111-1111-4111-8111-111111111111"
        let row = ProjectSessionSummary(sessionId: id, title: "Other copy", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/catalog/a.jsonl")
        let index = root.appendingPathComponent(globalSessionIndexFileName)
        let original = try Data(contentsOf: index)
        let reservation = UUID()
        XCTAssertTrue(gate.begin(sessionID: id, token: reservation))
        let update = Task { try await store.syncEngineSessions(projectId: nil, rows: [row]) }
        await Task.yield()
        store.additionalSessionEvidence = { [id: SessionOwnerEvidence(scope: .global, mode: .chat,
                                                                        transcriptPath: "/catalog/b.jsonl")] }
        gate.finish(reservation)
        do {
            try await update.value
            XCTFail("The catalog must revalidate the scheduler's committed source")
        } catch {}
        XCTAssertEqual(try Data(contentsOf: index), original)
        XCTAssertEqual(try store.transcriptPathForSession(id, in: .global, mode: .chat), "/catalog/b.jsonl")
    }

    func testFirstHandoffAndAuthoringRowsRequireTheirExactSource() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Authoring")
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let cases: [(String?, String, SessionMode, String)] = [
            (nil, "11111111-1111-4111-8111-111111111111", .chat, "/catalog/handoff.jsonl"),
            (project.id, "22222222-2222-4222-8222-222222222222", .code, "/catalog/authoring.jsonl")
        ]
        for (projectID, id, mode, path) in cases {
            do {
                try await store.recordStartedSession(projectId: projectID, sessionId: id, title: "New", mode: mode,
                                                     transcriptPath: "")
                XCTFail("An owner row without a source must never be written")
            } catch {}
            let scope = projectID.map { ConversationScope.project($0) } ?? .global
            XCTAssertFalse(store.isSessionIndexed(id, in: scope))
            try await store.recordStartedSession(projectId: projectID, sessionId: id, title: "New", mode: mode,
                                                 transcriptPath: path)
            let reloaded = ProjectStore(repository: ProjectRepository(projectsRoot: root),
                                        synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
            XCTAssertEqual(try reloaded.transcriptPathForSession(id, in: scope, mode: mode), path)
        }
    }

    func testAuthenticatedWorkspaceKeepsExactSourceForGlobalAndProjectOwners() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let linked = root.appendingPathComponent("linked", isDirectory: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: true)
        let projectCwd = LXISHGuestPaths.workspace("projects") + "/linked"
        let globalID = "11111111-1111-4111-8111-111111111111"
        let projectID = "22222222-2222-4222-8222-222222222222"
        let source = "/home/lingxi/.lingxi/projects/original.jsonl"
        func row(_ id: String) -> ProjectSessionSummary {
            ProjectSessionSummary(sessionId: id, title: id, messageCount: 1, relativeTime: "Now",
                                  updatedAt: Date(), mode: .chat, transcriptPath: source)
        }
        _ = try repository.updateSessions(projectId: nil, sessions: [row(globalID)])
        _ = try repository.updateSessions(projectId: project.id, sessions: [row(projectID)])
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let global = SessionWorkspaceState(sessionId: globalID, cwd: "/home/lingxi/global-next",
            projectRoot: "/home/lingxi", additionalDirectories: [], transcriptPath: source)
        let moved = SessionWorkspaceState(sessionId: projectID, cwd: projectCwd,
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: source)
        let globalAuthority = WorkspaceWriteAuthority()
        globalAuthority.setSelected(true)
        let projectAuthority = WorkspaceWriteAuthority()
        try await store.recordAuthenticatedWorkspace(global, in: .global, mode: .chat,
            authority: globalAuthority, revision: globalAuthority.currentRevision())
        try await store.recordAuthenticatedWorkspace(moved, in: .project(project.id), mode: .chat,
            authority: projectAuthority, revision: projectAuthority.currentRevision())
        var restored = ProjectRepository(projectsRoot: root).load()
        XCTAssertEqual(restored.globalSessions.first?.workspaceDirectory, global.cwd)
        XCTAssertEqual(restored.globalSessions.first?.transcriptPath, source)
        XCTAssertEqual(restored.projects.first?.sessions.first?.workspaceDirectory, projectCwd)
        XCTAssertEqual(restored.projects.first?.sessions.first?.transcriptPath, source)
        XCTAssertEqual(restored.projects.first?.record.activeDirectory, project.record.directory,
                       "a background owner must not change the selected project's cwd")
        projectAuthority.setSelected(true)
        try await store.recordAuthenticatedWorkspace(moved, in: .project(project.id), mode: .chat,
            authority: projectAuthority, revision: projectAuthority.currentRevision())
        restored = ProjectRepository(projectsRoot: root).load()
        XCTAssertEqual(restored.projects.first?.record.activeDirectory, projectCwd)
        let index = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let bytes = try Data(contentsOf: index)
        let differentSource = SessionWorkspaceState(sessionId: projectID, cwd: projectCwd,
            projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: "/other/copy.jsonl")
        do {
            try await store.recordAuthenticatedWorkspace(differentSource, in: .project(project.id), mode: .chat,
                authority: projectAuthority, revision: projectAuthority.currentRevision())
            XCTFail("A different source must not replace the owner")
        } catch {}
        do {
            try await store.recordAuthenticatedWorkspace(moved, in: .project(project.id), mode: .code,
                authority: projectAuthority, revision: projectAuthority.currentRevision())
            XCTFail("A different mode must not replace the owner")
        } catch {}
        XCTAssertEqual(try Data(contentsOf: index), bytes)
        XCTAssertEqual(try store.transcriptPathForSession(projectID, in: .project(project.id), mode: .chat), source)
    }

    func testProjectCreationWaitsForSourcePreparationAndRescansBeforeRelease() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let gate = CronOwnerAdmissionGate()
        let source = MockConversationSource(sessionMode: .chat)
        XCTAssertTrue(gate.beginSourcePreparation(source))
        var refreshed = false
        store.beginOwnerScopeMutation = { try await gate.beginScopeMutation() }
        store.finishOwnerScopeMutation = { gate.endScopeMutation($0) }
        store.refreshOwnerScopes = {
            XCTAssertTrue(gate.scopeMutationPending)
            refreshed = true
        }
        let create = Task { try await store.createInternal(name: "New") }
        for _ in 0..<100 where !gate.scopeMutationPending { await Task.yield() }
        XCTAssertTrue(gate.scopeMutationPending)
        XCTAssertTrue(store.projects.isEmpty)
        gate.endSourcePreparation(source)
        let project = try await create.value
        XCTAssertTrue(refreshed)
        XCTAssertFalse(gate.scopeMutationPending)
        XCTAssertEqual(store.projects.first?.id, project.id)
    }
    func testProjectCronReceiptOpensItsExactSourceBeforeSessionListRefresh() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let id = "11111111-1111-4111-8111-111111111111"
        let scope = ConversationScope.project(project.id)
        let path = "/home/lingxi/.lingxi/projects/exact/source.jsonl"
        store.additionalSessionEvidence = { [id: SessionOwnerEvidence(scope: scope, mode: .chat, transcriptPath: path)] }
        XCTAssertTrue(store.projects.first?.sessions.isEmpty == true)
        XCTAssertTrue(store.admitsSessionIdentity(id, in: scope, mode: .chat))
        XCTAssertEqual(try store.transcriptPathForSession(id, in: scope, mode: .chat), path)
        let receipt = CronGeneratedSession(id: id, projectID: project.id, title: "Scheduled result", updatedAtMs: 1,
            sessionTranscriptPath: path, sessionMode: .chat, sessionScopeKey: scope.workspaceKey)
        let choices = store.cronSessionChoices(mode: .chat, generated: [receipt])
        XCTAssertEqual(choices.map(\.id), [id])
        XCTAssertEqual(choices.first?.owner, scope)
        XCTAssertEqual(choices.first?.transcriptPath, path)
        XCTAssertTrue(store.cronSessionChoices(mode: .code, generated: [receipt]).isEmpty)
        XCTAssertThrowsError(try store.transcriptPathForSession(id, in: scope, mode: .code))
        XCTAssertThrowsError(try store.transcriptPathForSession(id, in: .global, mode: .chat))
        XCTAssertThrowsError(try store.transcriptPathForSession("22222222-2222-4222-8222-222222222222", in: scope, mode: .chat))
        let index = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let original = try Data(contentsOf: index)
        let wrongRow = ProjectSessionSummary(sessionId: id, title: "wrong copy", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/elsewhere/copy.jsonl")
        do {
            try await store.syncEngineSessions(projectId: project.id, rows: [wrongRow])
            XCTFail("A catalog copy must not displace the durable owner")
        } catch {}
        XCTAssertEqual(try Data(contentsOf: index), original)
        try await store.recordStartedSession(projectId: project.id, sessionId: id, title: "Continue", mode: .chat,
                                             initialMessageCount: 1, transcriptPath: path)
        XCTAssertEqual(store.projects.first?.sessions.first?.transcriptPath, path)
        XCTAssertNil(store.sessionOwnershipError)
        var missingPath = wrongRow
        missingPath.transcriptPath = nil
        try await store.syncEngineSessions(projectId: project.id, rows: [missingPath])
        XCTAssertEqual(try store.transcriptPathForSession(id, in: scope, mode: .chat), path)
    }

    func testGlobalCronReceiptSeedsExactSourceWhenFirstCatalogRowOmitsIt() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        _ = try repository.updateSessions(projectId: nil, sessions: [])
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let id = "11111111-1111-4111-8111-111111111111"
        let path = "/catalog/original.jsonl"
        store.additionalSessionEvidence = { [id: SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: path)] }
        let row = ProjectSessionSummary(sessionId: id, title: "Scheduled", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat)
        try await store.syncEngineSessions(projectId: nil, rows: [row])
        XCTAssertEqual(store.globalSessions.first?.transcriptPath, path)
        XCTAssertNil(store.sessionOwnershipError)
        try await store.recordStartedSession(projectId: nil, sessionId: id, title: "Continue", mode: .chat,
                                             initialMessageCount: 2, transcriptPath: path)
        XCTAssertEqual(try store.transcriptPathForSession(id, in: .global, mode: .chat), path)
    }
    func testSymlinkedGlobalIndexCannotAuthorizeOrOverwriteAnOwner() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let id = "11111111-1111-4111-8111-111111111111"
        let row = ProjectSessionSummary(sessionId: id, title: "Original", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/catalog/original.jsonl")
        _ = try repository.updateSessions(projectId: nil, sessions: [row])
        let index = root.appendingPathComponent(globalSessionIndexFileName)
        let foreign = root.appendingPathComponent("foreign.json")
        try FileManager.default.moveItem(at: index, to: foreign)
        try FileManager.default.createSymbolicLink(at: index, withDestinationURL: foreign)
        let original = try Data(contentsOf: foreign)
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .global, mode: .chat))
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: []))
        XCTAssertEqual(try Data(contentsOf: foreign), original)
        XCTAssertEqual(try FileManager.default.destinationOfSymbolicLink(atPath: index.path), foreign.path)
        try FileManager.default.removeItem(at: foreign)
        XCTAssertFalse(store.admitsSessionIdentity(id, in: .global, mode: .chat))
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: []))
        XCTAssertEqual(try FileManager.default.destinationOfSymbolicLink(atPath: index.path), foreign.path)
    }
    func testCronChoicesAndColdRestoreUseExactOwnerModeAndPath() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        func row(_ id: String, _ mode: SessionMode, _ path: String?) -> ProjectSessionSummary {
            ProjectSessionSummary(sessionId: id, title: id, messageCount: 1, relativeTime: "Now",
                                  updatedAt: Date(), mode: mode, transcriptPath: path)
        }
        let globalChat = "11111111-1111-4111-8111-111111111111"
        let globalCode = "22222222-2222-4222-8222-222222222222"
        let projectChat = "33333333-3333-4333-8333-333333333333"
        let scheduledCode = "44444444-4444-4444-8444-444444444444"
        let guestPath = "/home/lingxi/.lingxi/projects/global/first.jsonl"
        _ = try repository.updateSessions(projectId: nil, sessions: [
            row(globalChat, .chat, guestPath), row(globalCode, .code, "/catalog/code.jsonl")
        ])
        _ = try repository.updateSessions(projectId: project.id, sessions: [row(projectChat, .chat, "/catalog/project.jsonl")])
        let store = ProjectStore(repository: repository, synchronizer: ProjectWorkspaceSynchronizer(repository: repository))
        let scheduled = CronGeneratedSession(id: scheduledCode, projectID: nil, title: "Scheduled", updatedAtMs: 1,
            sessionTranscriptPath: "/catalog/scheduled.jsonl", sessionMode: .code, sessionScopeKey: "scheduled")
        store.additionalSessionEvidence = { [scheduledCode: SessionOwnerEvidence(scope: .scheduled, mode: .code,
                                                                                  transcriptPath: "/catalog/scheduled.jsonl")] }

        let chats = store.cronSessionChoices(mode: .chat, generated: [scheduled])
        XCTAssertEqual(Set(chats.map(\.id)), Set([globalChat, projectChat]))
        XCTAssertEqual(chats.first(where: { $0.id == globalChat })?.owner, .global)
        XCTAssertEqual(chats.first(where: { $0.id == projectChat })?.owner, .project(project.id))
        let codes = store.cronSessionChoices(mode: .code, generated: [scheduled])
        XCTAssertEqual(Set(codes.map(\.id)), Set([globalCode, scheduledCode]))
        XCTAssertEqual(codes.first(where: { $0.id == scheduledCode })?.owner, .scheduled)
        XCTAssertEqual(try store.transcriptPathForSession(globalChat, in: .global, mode: .chat), guestPath)
        XCTAssertTrue(store.admitsSessionEvidence(globalChat, owner: SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: guestPath)))
        XCTAssertFalse(store.admitsSessionEvidence(globalChat, owner: SessionOwnerEvidence(scope: .global, mode: .chat, transcriptPath: "/other/copy.jsonl")))
        XCTAssertThrowsError(try store.transcriptPathForSession(globalChat, in: .global, mode: .code))
        XCTAssertThrowsError(try store.transcriptPathForSession(globalChat, in: .project(project.id), mode: .chat))

        let conflict = CronGeneratedSession(id: globalChat, projectID: nil, title: "Wrong owner", updatedAtMs: 2,
            sessionTranscriptPath: "/elsewhere/copy.jsonl", sessionMode: .chat, sessionScopeKey: "scheduled")
        XCTAssertFalse(store.cronSessionChoices(mode: .chat, generated: [conflict]).contains { $0.id == globalChat })
        let index = root.appendingPathComponent(project.id).appendingPathComponent(projectSessionIndexFileName)
        let corrupt = Data("{".utf8)
        try corrupt.write(to: index)
        XCTAssertTrue(store.cronSessionChoices(mode: .chat, generated: [scheduled]).isEmpty)
        XCTAssertThrowsError(try store.transcriptPathForSession(globalChat, in: .global, mode: .chat))
        XCTAssertEqual(try Data(contentsOf: index), corrupt)
    }
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
            try await store.recordStartedSession(projectId: nil, sessionId: id, title: "Must not replace", mode: .chat,
                                                 transcriptPath: "/catalog/session.jsonl")
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
        try await store.recordStartedSession(projectId: nil, sessionId: id, title: "First", mode: .chat,
                                             transcriptPath: "/catalog/session.jsonl")
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
        _ = try repository.recordStartedSession(projectId: nil, sessionId: id, title: "Existing", mode: .chat,
                                                authoritativeTranscriptPath: "/catalog/session.jsonl")
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
        _ = try repository.recordStartedSession(projectId: nil, sessionId: id, title: "Existing", mode: .chat,
                                                authoritativeTranscriptPath: "/catalog/session.jsonl")
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

private final class BlockingWorkspaceWriter: ProjectAtomicWriting, @unchecked Sendable {
    private let staged: XCTestExpectation
    private let release: DispatchSemaphore
    private let lock = NSLock()
    private var hasPaused = false
    private var armedPath: String?
    private let beforeCommit: Bool
    init(staged: XCTestExpectation, release: DispatchSemaphore, beforeCommit: Bool = false) {
        self.staged = staged
        self.release = release
        self.beforeCommit = beforeCommit
    }
    func arm(target: URL) { lock.withLock { armedPath = target.standardizedFileURL.path } }
    func write<T: Codable>(_ value: T, to url: URL, encoder: JSONEncoder, decoder: JSONDecoder) throws {
        try DefaultProjectAtomicWriter().write(value, to: url, encoder: encoder, decoder: decoder)
    }
    func writeData(_ data: Data, to url: URL, validate: (Data) throws -> Void) throws {
        try DefaultProjectAtomicWriter().writeData(data, to: url, validate: validate)
    }
    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation, validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        let pause: @Sendable () -> Void = { [self] in
            let shouldPause = lock.withLock {
                if hasPaused || armedPath != url.standardizedFileURL.path { return false }
                hasPaused = true
                return true
            }
            if shouldPause { staged.fulfill(); release.wait() }
        }
        let stagedCallback: (@Sendable (URL) -> Void)?
        let verifiedCallback: (@Sendable () -> Void)?
        if beforeCommit {
            stagedCallback = nil
            verifiedCallback = pause
        } else {
            stagedCallback = { _ in pause() }
            verifiedCallback = nil
        }
        let writer = DefaultProjectAtomicWriter(onGuardedStaged: stagedCallback, onGuardedVerified: verifiedCallback)
        try writer.writeData(data, to: url, expected: expected, validate: validate, commit: commit)
    }
}

#if !canImport(harness_runtimeFFI)
private actor SuspendedWorkspaceReader {
    private var current: SessionWorkspaceState
    private var first = true
    private var released = false
    private let started: XCTestExpectation
    private var waiter: CheckedContinuation<Void, Never>?
    init(initial: SessionWorkspaceState, started: XCTestExpectation) { current = initial; self.started = started }
    func setCurrent(_ state: SessionWorkspaceState) { current = state }
    func release() { released = true; waiter?.resume(); waiter = nil }
    func read() async -> SessionWorkspaceState {
        let captured = current
        if first {
            first = false
            started.fulfill()
            if !released { await withCheckedContinuation { waiter = $0 } }
        }
        return captured
    }
}
#endif

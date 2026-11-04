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

    func testWorkspacePickerShowsOnlyProjectsInTheCapturedMode() {
        func project(_ name: String, modes: Set<SessionMode>) -> ProjectSnapshot {
            let id = UUID().uuidString.lowercased()
            let record = ProjectRecord(id: id, name: name, storageKind: .internal,
                createdAt: .distantPast, updatedAt: .distantPast, modeMembership: modes)
            let workspace = ProjectWorkspace(projectId: id, hostURL: root.appendingPathComponent(id),
                                             guestPath: "/home/lingxi/\(id)")
            return ProjectSnapshot(record: record, workspace: workspace, sessions: [])
        }
        let chat = project("chat", modes: [.chat])
        let code = project("code", modes: [.code])
        let shared = project("shared", modes: [.chat, .code])
        let all = [chat, code, shared]
        XCTAssertEqual(Set(ProjectSnapshot.visible(all, in: .chat).map(\.id)), Set([chat.id, shared.id]))
        XCTAssertEqual(Set(ProjectSnapshot.visible(all, in: .code).map(\.id)), Set([code.id, shared.id]))
    }

    func testGuardedPublicationRejectsChangedTargetAndChangedStagingFile() throws {
        let target = root.appendingPathComponent("guarded.json")
        let original = Data("original".utf8)
        let replacement = Data("replacement".utf8)
        for attack in 0..<4 {
            try? FileManager.default.removeItem(at: target)
            if attack != 3 { try original.write(to: target) }
            let expected = try ownerCatalogExpectation(target)
            let writer = DefaultProjectAtomicWriter(onGuardedStaged: { staged in
                switch attack {
                case 0: try? FileManager.default.removeItem(at: target)
                case 1: try? replacement.write(to: target, options: .atomic)
                case 2: try? replacement.write(to: staged, options: .atomic)
                default: try? replacement.write(to: target)
                }
            })
            XCTAssertThrowsError(try writer.writeData(Data("new".utf8), to: target, expected: expected,
                                                       validate: { _ in }, commit: { try $0() }))
            if attack == 0 { XCTAssertFalse(FileManager.default.fileExists(atPath: target.path)) }
            else if attack == 2 { XCTAssertEqual(try Data(contentsOf: target), original) }
            else { XCTAssertEqual(try Data(contentsOf: target), replacement) }
        }
    }

    func testCatalogUpdateRejectsAnIndexReplacedDuringStaging() throws {
        let initial = ProjectRepository(projectsRoot: root)
        _ = try initial.updateSessions(projectId: nil, sessions: [session("first", at: Date())])
        let file = root.appendingPathComponent(globalSessionIndexFileName)
        let replacement = Data("external owner evidence".utf8)
        let writer = DefaultProjectAtomicWriter(onGuardedStaged: { staged in
            guard staged.lastPathComponent.contains(globalSessionIndexFileName) else { return }
            try? replacement.write(to: file, options: .atomic)
        })
        let repository = ProjectRepository(projectsRoot: root, writer: writer)
        XCTAssertThrowsError(try repository.updateSessions(projectId: nil, sessions: [session("second", at: Date())]))
        XCTAssertEqual(try Data(contentsOf: file), replacement)
    }

    func testCreateIntentSurvivesBeforeProjectAndStagedFailure() throws {
        let projects = root.appendingPathComponent("projects", isDirectory: true)
        let repository = ProjectRepository(projectsRoot: projects, userHome: root)
        let request = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: .code, name: "Notes", prompt: "keep this request")
        try repository.storeLocalAppSourceCreateIntent(request)
        XCTAssertEqual(try ProjectRepository(projectsRoot: projects, userHome: root).pendingLocalAppSourceCreations(), [request])
        let source = MobileUserStorage.directories().appsGuestPath + "/notes"
        let sourceID = UUID().uuidString.lowercased()
        let failing = ProjectRepository(projectsRoot: projects,
            writer: FailingWriter(failingFileName: ".local-app-selection.json"), userHome: root)
        XCTAssertThrowsError(try failing.createLocalAppAuthoringProject(name: request.name, sourceDirectory: source,
            sourceID: sourceID, sourceRequestID: request.requestId, appID: nil,
            projectID: request.projectId, intentID: request.requestId))
        let reopened = ProjectRepository(projectsRoot: projects, userHome: root)
        XCTAssertNil(reopened.load().errorMessage)
        XCTAssertTrue(reopened.load().projects.isEmpty)
        XCTAssertEqual(try reopened.pendingLocalAppSourceCreations(), [request])

        let (project, intent) = try reopened.createLocalAppAuthoringProject(name: request.name, sourceDirectory: source,
            sourceID: sourceID, sourceRequestID: request.requestId, appID: nil,
            projectID: request.projectId, intentID: request.requestId)
        XCTAssertEqual(project.id, request.projectId)
        XCTAssertEqual(try reopened.selectedLocalAppAuthoringIntent(projectId: project.id), intent)
        XCTAssertEqual(try reopened.pendingLocalAppSourceCreations(), [request],
                       "The recovery offer remains visible until an owner receipt is confirmed")
        let retry = try reopened.createLocalAppAuthoringProject(name: request.name, sourceDirectory: source,
            sourceID: sourceID, sourceRequestID: request.requestId, appID: nil,
            projectID: request.projectId, intentID: request.requestId)
        XCTAssertEqual(retry.0.id, project.id)
        XCTAssertEqual(retry.1, intent)
        XCTAssertThrowsError(try reopened.createLocalAppAuthoringProject(name: request.name, sourceDirectory: source,
            sourceID: UUID().uuidString.lowercased(), sourceRequestID: request.requestId, appID: nil,
            projectID: request.projectId, intentID: request.requestId))
        var owner = intent
        owner.authoringSessionId = UUID().uuidString.lowercased()
        owner.transcriptPath = "/transcripts/owner.jsonl"
        try reopened.storeLocalAppAuthoringIntent(owner, replacing: intent)
        try reopened.completeLocalAppSourceCreateIntent(requestID: request.requestId, owner: owner)
        XCTAssertTrue(try reopened.pendingLocalAppSourceCreations().isEmpty)
    }

    func testMarketRemixPersistsExactPrivateInputAndRejectsChangedArchive() throws {
        let projects = root.appendingPathComponent("projects", isDirectory: true)
        let privateRoot = root.appendingPathComponent("private", isDirectory: true)
        try FileManager.default.createDirectory(at: privateRoot, withIntermediateDirectories: true)
        let repository = ProjectRepository(projectsRoot: projects, userHome: root)
        let requestID = UUID().uuidString.lowercased()
        let manifestData = Data(#"{"schema":1,"name":"Notes","summary":null,"entry":{"type":"web","path":"index.html"},"targets":[],"remix":{"allowed":true}}"#.utf8)
        let manifest = try JSONDecoder().decode(UnifiedAppManifest.self, from: manifestData)
        let signedManifest = try JSONDecoder().decode(LocalAppJSON.self, from: manifestData)
        let file = LocalAppMarketRemixInput.File(path: "lingxi-app.json", sizeBytes: 1,
            sha256: String(repeating: "a", count: 64), executable: false)
        let input = LocalAppMarketRemixInput(requestId: requestID,
            archivePath: privateRoot.appendingPathComponent("local-app-imports").appendingPathComponent("market-" + requestID).appendingPathComponent("source.zip").path,
            archiveSha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            archiveBytes: 3, manifest: manifest, files: [file], dependencies: [],
            signedManifest: signedManifest, signedFiles: try .encode([file]), signedDependencies: .array([]),
            marketOrigin: .init(appId: "market-app", releaseId: "release-1", digest: "release-digest", remixedFrom: "market-app"))
        let digest = try ProjectRepository.localAppRemixInputDigest(input)
        let intent = LocalAppSourceCreateIntent(requestId: requestID, projectId: UUID().uuidString.lowercased(),
            mode: .code, name: "Notes", prompt: "", operation: .marketRemix, remixInputDigest: digest)
        let chatIntent = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: .chat, name: "Notes", prompt: "",
            operation: .marketRemix, remixInputDigest: digest)
        XCTAssertThrowsError(try repository.storeLocalAppRemixRequest(chatIntent, input: input, configHome: privateRoot))
        XCTAssertFalse(FileManager.default.fileExists(atPath: privateRoot.appendingPathComponent("local-app-imports").path))
        try repository.storeLocalAppRemixRequest(intent, input: input, configHome: privateRoot)
        let reopened = ProjectRepository(projectsRoot: projects, userHome: root)
        XCTAssertEqual(try reopened.pendingLocalAppSourceCreations(), [intent])
        let (saved, archive) = try reopened.localAppRemixInput(intent, configHome: privateRoot)
        XCTAssertEqual(saved, input)
        guard case let .object(savedManifest) = saved.signedManifest else { return XCTFail("Expected the signed manifest") }
        XCTAssertEqual(savedManifest["summary"], .null, "Recovery must preserve explicit null in the signed input")
        XCTAssertFalse(FileManager.default.fileExists(atPath: archive.path),
                       "A committed native request can be replayed without its old ZIP")
        XCTAssertThrowsError(try reopened.storeLocalAppRemixArchive(Data("wrong".utf8), intent: intent, configHome: privateRoot))
        XCTAssertEqual(try reopened.storeLocalAppRemixArchive(Data("abc".utf8), intent: intent, configHome: privateRoot), archive)
        XCTAssertEqual(try Data(contentsOf: archive), Data("abc".utf8))
        XCTAssertEqual(try reopened.storeLocalAppRemixArchive(Data("abc".utf8), intent: intent, configHome: privateRoot), archive)
        // Model the native producer's immutable result; registration is a
        // separate step and cannot hide this recovery offer before readback.
        let appID = "new-market-remix", sourceID = UUID().uuidString.lowercased()
        let source = MobileUserStorage.directories().appsGuestPath + "/remix"
        let (project, owner) = try reopened.createLocalAppAuthoringProject(name: intent.name, sourceDirectory: source,
            sourceID: sourceID, sourceRequestID: requestID, appID: appID,
            projectID: intent.projectId, intentID: requestID)
        XCTAssertEqual(owner.appId, appID)
        XCTAssertEqual(try reopened.pendingLocalAppSourceCreations(), [intent])
        var authenticated = owner
        authenticated.authoringSessionId = UUID().uuidString.lowercased()
        authenticated.transcriptPath = "/catalog/owner.jsonl"
        try reopened.storeLocalAppAuthoringIntent(authenticated, replacing: owner)
        XCTAssertEqual(try reopened.pendingLocalAppSourceCreations(), [intent], "A pending or failed native register must remain retryable")
        let restoredOwner = try XCTUnwrap(reopened.selectedLocalAppAuthoringIntent(projectId: project.id))
        XCTAssertEqual(restoredOwner.appId, appID)
        XCTAssertEqual(restoredOwner.sourceId, sourceID)
        let session = try XCTUnwrap(restoredOwner.authoringSessionId)
        let response = "{\"sourceDirectory\":\"\(source)\",\"workspaceId\":\"\(project.id)\",\"projectRef\":{\"kind\":\"mobileProject\",\"projectId\":\"\(project.id)\"},\"authoringSessionId\":\"\(session)\",\"sourceId\":\"\(sourceID)\",\"sessionMode\":\"code\"}"
        let receipt = try JSONDecoder().decode(LocalAppAuthoringReceipt.self, from: Data(response.utf8))
        let expectedOwner = LocalAppAuthoringOwner(sourceDirectory: source, projectID: project.id, sessionID: session, appID: appID)
        XCTAssertTrue(expectedOwner.matches(receipt.authoring))
        XCTAssertTrue(receipt.matchesSourceID(restoredOwner.sourceId))
        XCTAssertFalse(receipt.matchesSourceID(UUID().uuidString.lowercased()))
        try reopened.completeLocalAppSourceCreateIntent(requestID: requestID, owner: restoredOwner)
        XCTAssertTrue(try reopened.pendingLocalAppSourceCreations().isEmpty)
        let moved = root.appendingPathComponent("moved-config", isDirectory: true)
        try FileManager.default.moveItem(at: privateRoot, to: moved)
        var completed = intent
        completed.completed = true
        let (movedInput, originalPath) = try reopened.localAppRemixInput(completed, configHome: moved)
        XCTAssertEqual(movedInput, input)
        XCTAssertEqual(originalPath.path, input.archivePath, "Replay must preserve the original request payload")
        XCTAssertThrowsError(try reopened.storeLocalAppRemixArchive(Data("abc".utf8), intent: completed, configHome: moved),
                             "A moved sandbox must not write into an old path")
    }

    func testMarketRemixStagesRealZipAndSignedHiddenFileInventoryWithoutExtraction() throws {
        // ZIP entries: lingxi-app.json, index.html, .gitignore and
        // .well-known/security.txt. Native import validates the ZIP and paths.
        let encoded = "UEsDBBQAAAAAAAAAIViFsZTibAAAAGwAAAAPAAAAbGluZ3hpLWFwcC5qc29ueyJzY2hlbWEiOjEsIm5hbWUiOiJOb3RlcyIsImVudHJ5Ijp7InR5cGUiOiJ3ZWIiLCJwYXRoIjoiaW5kZXguaHRtbCJ9LCJ0YXJnZXRzIjpbXSwicmVtaXgiOnsiYWxsb3dlZCI6dHJ1ZX19UEsDBBQAAAAAAAAAIVibWLMIIwAAACMAAAAKAAAAaW5kZXguaHRtbDwhZG9jdHlwZSBodG1sPjx0aXRsZT5Ob3RlczwvdGl0bGU+UEsDBBQAAAAAAAAAIViqpgOJDgAAAA4AAAAKAAAALmdpdGlnbm9yZW5vZGVfbW9kdWxlcy8KUEsDBBQAAAAAAAAAIVi7UZbFHwAAAB8AAAAYAAAALndlbGwta25vd24vc2VjdXJpdHkudHh0Q29udGFjdDogc2VjdXJpdHlAZXhhbXBsZS50ZXN0ClBLAQIUAxQAAAAAAAAAIViFsZTibAAAAGwAAAAPAAAAAAAAAAAAAACkgQAAAABsaW5neGktYXBwLmpzb25QSwECFAMUAAAAAAAAACFYm1izCCMAAAAjAAAACgAAAAAAAAAAAAAApIGZAAAAaW5kZXguaHRtbFBLAQIUAxQAAAAAAAAAIViqpgOJDgAAAA4AAAAKAAAAAAAAAAAAAACkgeQAAAAuZ2l0aWdub3JlUEsBAhQDFAAAAAAAAAAhWLtRlsUfAAAAHwAAABgAAAAAAAAAAAAAAKSBGgEAAC53ZWxsLWtub3duL3NlY3VyaXR5LnR4dFBLBQYAAAAABAAEAPMAAABvAQAAAAA="
        let bytes = try XCTUnwrap(Data(base64Encoded: encoded))
        let config = root.appendingPathComponent("config", isDirectory: true)
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        let repository = ProjectRepository(projectsRoot: root.appendingPathComponent("projects"), userHome: root)
        let requestID = UUID().uuidString.lowercased()
        let manifestData = Data(#"{"schema":1,"name":"Notes","entry":{"type":"web","path":"index.html"},"targets":[],"remix":{"allowed":true}}"#.utf8)
        let manifest = try JSONDecoder().decode(UnifiedAppManifest.self, from: manifestData)
        let files: [LocalAppMarketRemixInput.File] = [
            .init(path: "lingxi-app.json", sizeBytes: 108, sha256: "fc2c0ec1a63c84b6def425a46b2e95429fea41db8c9635eaff291a2b0d4af5cf", executable: false),
            .init(path: "index.html", sizeBytes: 35, sha256: "190656c049cf9a75ef41619dd0ea48ca4d644ae7725b4b10856a7bd0896d9bff", executable: false),
            .init(path: ".gitignore", sizeBytes: 14, sha256: "4d56952b0fb13bf8f9b6c13a6d4c34a075bac3af447636a1df4335d7576e2f97", executable: false),
            .init(path: ".well-known/security.txt", sizeBytes: 31, sha256: "1b49129282fc3f4ecc302c73c5cad0762dd0596532e9e411e14678b693286d7f", executable: false),
        ]
        let archivePath = config.appendingPathComponent("local-app-imports/market-\(requestID)/source.zip").path
        let input = LocalAppMarketRemixInput(requestId: requestID, archivePath: archivePath,
            archiveSha256: "2b33ccadd0cf52a1fb0d2dacbf9d472c9340cb78d50ea49292c080333fa3cd8f",
            archiveBytes: 632, manifest: manifest, files: files, dependencies: [],
            signedManifest: try JSONDecoder().decode(LocalAppJSON.self, from: manifestData),
            signedFiles: try .encode(files), signedDependencies: .array([]),
            marketOrigin: .init(appId: "market-app", releaseId: "release-1", digest: "digest", remixedFrom: "market-app"))
        let intent = LocalAppSourceCreateIntent(requestId: requestID, projectId: UUID().uuidString.lowercased(),
            mode: .code, name: "Notes", prompt: "", operation: .marketRemix,
            remixInputDigest: try ProjectRepository.localAppRemixInputDigest(input))
        try repository.storeLocalAppRemixRequest(intent, input: input, configHome: config)
        let staged = try repository.storeLocalAppRemixArchive(bytes, intent: intent, configHome: config)
        XCTAssertEqual(try Data(contentsOf: staged), bytes)
        XCTAssertEqual(try repository.localAppRemixInput(intent, configHome: config).0.files, files)
        XCTAssertFalse(FileManager.default.fileExists(atPath: staged.deletingLastPathComponent().appendingPathComponent(".gitignore").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: staged.deletingLastPathComponent().appendingPathComponent(".well-known").path))
    }

    func testMarketRemixFailureBeforeIntentAndSymlinkRemainUntrusted() throws {
        let projects = root.appendingPathComponent("projects", isDirectory: true)
        let privateRoot = root.appendingPathComponent("private", isDirectory: true)
        try FileManager.default.createDirectory(at: privateRoot, withIntermediateDirectories: true)
        let requestID = UUID().uuidString.lowercased()
        let manifest = try JSONDecoder().decode(UnifiedAppManifest.self, from: Data(
            #"{"schema":1,"name":"Notes","entry":{"type":"web","path":"index.html"},"targets":[],"remix":{"allowed":true}}"#.utf8))
        let file = LocalAppMarketRemixInput.File(path: "lingxi-app.json", sizeBytes: 1,
            sha256: String(repeating: "a", count: 64), executable: false)
        let input = LocalAppMarketRemixInput(requestId: requestID,
            archivePath: privateRoot.appendingPathComponent("local-app-imports").appendingPathComponent("market-" + requestID).appendingPathComponent("source.zip").path,
            archiveSha256: String(repeating: "b", count: 64),
            archiveBytes: 3, manifest: manifest, files: [file], dependencies: [],
            signedManifest: try .encode(manifest), signedFiles: try .encode([file]), signedDependencies: .array([]),
            marketOrigin: .init(appId: "market-app", releaseId: "release-1", digest: "digest", remixedFrom: "market-app"))
        let intent = LocalAppSourceCreateIntent(requestId: requestID, projectId: UUID().uuidString.lowercased(),
            mode: .code, name: "Notes", prompt: "", operation: .marketRemix,
            remixInputDigest: try ProjectRepository.localAppRemixInputDigest(input))
        let failing = ProjectRepository(projectsRoot: projects,
            writer: FailingWriter(failingFileName: ".local-app-create-\(requestID).json"), userHome: root)
        XCTAssertThrowsError(try failing.storeLocalAppRemixRequest(intent, input: input, configHome: privateRoot))
        let reopened = ProjectRepository(projectsRoot: projects, userHome: root)
        XCTAssertTrue(try reopened.pendingLocalAppSourceCreations().isEmpty)
        XCTAssertThrowsError(try reopened.localAppRemixInput(intent, configHome: privateRoot))
        try reopened.storeLocalAppRemixRequest(intent, input: input, configHome: privateRoot)
        let metadata = privateRoot.appendingPathComponent("local-app-imports").appendingPathComponent("market-" + requestID).appendingPathComponent("input.json")
        let original = try Data(contentsOf: metadata)
        try Data("{}".utf8).write(to: metadata)
        XCTAssertThrowsError(try reopened.localAppRemixInput(intent, configHome: privateRoot))
        try original.write(to: metadata)
        let staging = privateRoot.appendingPathComponent("local-app-imports", isDirectory: true)
        let target = privateRoot.appendingPathComponent("moved", isDirectory: true)
        try FileManager.default.moveItem(at: staging, to: target)
        try FileManager.default.createSymbolicLink(at: staging, withDestinationURL: target)
        XCTAssertThrowsError(try reopened.localAppRemixInput(intent, configHome: privateRoot))
    }

    func testPublishedAuthoringProjectRemainsRecoverableWhenIndexCommitFails() throws {
        let projects = root.appendingPathComponent("projects", isDirectory: true)
        let initial = ProjectRepository(projectsRoot: projects, userHome: root)
        _ = try initial.createInternal(name: "Existing")
        let request = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: .code, name: "Notes", prompt: "keep")
        try initial.storeLocalAppSourceCreateIntent(request)
        let failing = ProjectRepository(projectsRoot: projects,
            writer: FailingWriter(failingFileName: projectIndexFileName), userHome: root)
        XCTAssertThrowsError(try failing.createLocalAppAuthoringProject(name: request.name,
            sourceDirectory: MobileUserStorage.directories().appsGuestPath + "/notes",
            sourceID: UUID().uuidString.lowercased(), sourceRequestID: request.requestId, appID: nil,
            projectID: request.projectId, intentID: request.requestId))
        let reopened = ProjectRepository(projectsRoot: projects, userHome: root)
        let state = reopened.load()
        XCTAssertNil(state.errorMessage)
        XCTAssertTrue(state.projects.contains { $0.id == request.projectId })
        let selected = try XCTUnwrap(reopened.selectedLocalAppAuthoringIntent(projectId: request.projectId))
        XCTAssertEqual(selected.intentId, request.requestId)
        XCTAssertEqual(try reopened.pendingLocalAppSourceCreations(), [request])
    }

    func testCreateIntentRejectsChangedPayloadAndSymlink() throws {
        let repository = ProjectRepository(projectsRoot: root, userHome: root)
        let request = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: .code, name: "Notes", prompt: "first")
        try repository.storeLocalAppSourceCreateIntent(request)
        let changed = LocalAppSourceCreateIntent(requestId: request.requestId,
            projectId: request.projectId, mode: .code, name: request.name, prompt: "different")
        XCTAssertThrowsError(try repository.storeLocalAppSourceCreateIntent(changed))
        XCTAssertEqual(try repository.localAppSourceCreateIntent(requestID: request.requestId), request)
        let path = root.appendingPathComponent(".local-app-create-\(request.requestId).json")
        let target = root.appendingPathComponent("other.json")
        try FileManager.default.moveItem(at: path, to: target)
        try FileManager.default.createSymbolicLink(at: path, withDestinationURL: target)
        XCTAssertThrowsError(try repository.localAppSourceCreateIntent(requestID: request.requestId))
        XCTAssertThrowsError(try repository.pendingLocalAppSourceCreations())
    }

    func testCreateIntentNameUsesTheServiceUnicodeScalarLimit() throws {
        let repository = ProjectRepository(projectsRoot: root, userHome: root)
        func request(_ name: String) -> LocalAppSourceCreateIntent {
            LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
                projectId: UUID().uuidString.lowercased(), mode: .code, name: name, prompt: "")
        }
        XCTAssertNoThrow(try repository.storeLocalAppSourceCreateIntent(request(String(repeating: "a", count: 80))))
        XCTAssertThrowsError(try repository.storeLocalAppSourceCreateIntent(request(String(repeating: "a", count: 81))))
        XCTAssertNoThrow(try repository.storeLocalAppSourceCreateIntent(request(String(repeating: "e\u{301}", count: 40))))
        XCTAssertThrowsError(try repository.storeLocalAppSourceCreateIntent(request(String(repeating: "e\u{301}", count: 41))))
    }

    func testChatCreateIntentIsRejectedBeforeAnyFileIsPublished() throws {
        let repository = ProjectRepository(projectsRoot: root, userHome: root)
        let request = LocalAppSourceCreateIntent(requestId: UUID().uuidString.lowercased(),
            projectId: UUID().uuidString.lowercased(), mode: .chat, name: "Notes", prompt: "draft")
        XCTAssertThrowsError(try repository.storeLocalAppSourceCreateIntent(request))
        XCTAssertFalse(FileManager.default.fileExists(atPath:
            root.appendingPathComponent(".local-app-create-\(request.requestId).json").path))
        XCTAssertTrue(try repository.pendingLocalAppSourceCreations().isEmpty)
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
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: nil, sessionId: "new", title: "New", authoritativeTranscriptPath: "/catalog/new.jsonl"))
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
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: nil, sessionId: "same", title: "A", mode: .code, authoritativeTranscriptPath: "/catalog/same.jsonl"))
        XCTAssertEqual(try Data(contentsOf: index), bytes)
    }

    func testCatalogCannotReplaceOrForgetAnExactSourceInAnyScope() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        for projectID: String? in [nil, project.id] {
            let id = projectID == nil ? "global-source" : "project-source"
            let original = ProjectSessionSummary(sessionId: id, title: "Original", messageCount: 1,
                relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/catalog/original.jsonl")
            _ = try repository.updateSessions(projectId: projectID, sessions: [original])
            let index = projectID.map { root.appendingPathComponent($0).appendingPathComponent(projectSessionIndexFileName) }
                ?? root.appendingPathComponent(globalSessionIndexFileName)
            let bytes = try Data(contentsOf: index)
            var otherCopy = original
            otherCopy.transcriptPath = "/catalog/other.jsonl"
            XCTAssertThrowsError(try repository.updateSessions(projectId: projectID, sessions: [otherCopy]))
            XCTAssertEqual(try Data(contentsOf: index), bytes)

            // Missing source in a catalog row, or an omitted row, cannot
            // replace or remove an already confirmed owner.
            var noPath = original
            noPath.transcriptPath = ""
            _ = try repository.updateSessions(projectId: projectID, sessions: [noPath])
            _ = try repository.updateSessions(projectId: projectID, sessions: [])
            let rows = try projectID.map { try repository.project(projectId: $0).sessions }
                ?? repository.load().globalSessions
            XCTAssertEqual(rows.first(where: { $0.sessionId == id })?.transcriptPath, original.transcriptPath)
        }
    }

    func testWorkspaceNotificationCannotReplaceAnIndexedTranscript() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Project")
        let row = ProjectSessionSummary(sessionId: "session", title: "Original", messageCount: 1,
            relativeTime: "Now", updatedAt: Date(), mode: .chat, transcriptPath: "/catalog/original.jsonl")
        _ = try repository.updateSessions(projectId: project.id, sessions: [row])
        XCTAssertThrowsError(try recordWorkspace(repository, projectId: project.id,
            state: SessionWorkspaceState(sessionId: row.sessionId, cwd: project.record.directory,
                projectRoot: project.record.directory, additionalDirectories: [], transcriptPath: "/catalog/other.jsonl"),
            mode: .chat, updateProjectDirectory: true))
        XCTAssertEqual(try repository.project(projectId: project.id).sessions.first?.transcriptPath, row.transcriptPath)
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
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: project.id, sessionId: alias.sessionId, title: "wrong", authoritativeTranscriptPath: "/catalog/wrong.jsonl"))
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
        XCTAssertThrowsError(try repository.recordStartedSession(projectId: project.id, sessionId: id, title: "wrong", authoritativeTranscriptPath: "/catalog/wrong.jsonl"))
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
        let linked = LXISHGuestPaths.workspace("projects") + "/linked"
        try FileManager.default.createDirectory(at: root.appendingPathComponent("linked"), withIntermediateDirectories: true)
        let original = ProjectSessionSummary(sessionId: "a", title: "A", messageCount: 2,
            relativeTime: "Now", updatedAt: Date(), workspaceDirectory: project.record.directory, transcriptPath: "/catalog/a.jsonl")
        _ = try repository.updateSessions(projectId: project.id, sessions: [original])
        _ = try recordWorkspace(repository, projectId: project.id, state: SessionWorkspaceState(sessionId: "draft",
            cwd: linked, projectRoot: linked, additionalDirectories: [linked], transcriptPath: "/catalog/draft.jsonl"),
            mode: .code, updateProjectDirectory: true)
        let incoming = ProjectSessionSummary(sessionId: "b", title: "B", messageCount: 2,
            relativeTime: "Now", updatedAt: Date(), workspaceDirectory: linked, transcriptPath: "/catalog/b.jsonl")
        _ = try repository.updateSessions(projectId: project.id, sessions: [incoming])
        let restored = try ProjectRepository(projectsRoot: root).project(projectId: project.id)
        XCTAssertEqual(Set(restored.sessions.map(\.sessionId)), Set(["a", "b"]))
        XCTAssertEqual(restored.sessions.first { $0.sessionId == "a" }?.workspaceDirectory, project.record.directory)
        XCTAssertEqual(restored.sessions.first { $0.sessionId == "b" }?.workspaceDirectory, linked)
        _ = try repository.updateSessions(projectId: project.id, sessions: [])
        XCTAssertEqual(Set(try repository.project(projectId: project.id).sessions.map(\.sessionId)), Set(["a", "b"]))
    }

    func testMissingLinkedDirectoryDoesNotBlockLiveWorkspacePersistence() throws {
        let repository = ProjectRepository(projectsRoot: root)
        let project = try repository.createInternal(name: "Missing linked folder")
        let missing = LXISHGuestPaths.workspace("projects") + "/deleted"
        _ = try recordWorkspace(repository, projectId: project.id, state: SessionWorkspaceState(sessionId: "draft",
            cwd: project.record.directory, projectRoot: project.record.directory, additionalDirectories: [missing], transcriptPath: "/catalog/draft.jsonl"),
            mode: .code, updateProjectDirectory: true)
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
        _ = try recordWorkspace(repository, projectId: project.id, state: SessionWorkspaceState(sessionId: "draft",
            cwd: "/home/lingxi", projectRoot: project.record.directory, additionalDirectories: ["/home/lingxi"], transcriptPath: "/catalog/draft.jsonl"),
            mode: .code, updateProjectDirectory: true)
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
        XCTAssertEqual(LXISHGuestPaths.workspace("projects") + "/\(created.record.id)/workspace", created.workspace.guestPath)
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
        _ = try repository.recordStartedSession(projectId: project.record.id, sessionId: "sess:20000000-0000-4000-8000-0000000000AA", title: "First prompt", mode: .chat, authoritativeTranscriptPath: "/catalog/first.jsonl")
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [])
        let restored = try ProjectRepository(projectsRoot: root).project(projectId: project.record.id)
        XCTAssertEqual(restored.sessions.map(\.sessionId), ["20000000-0000-4000-8000-0000000000aa"])
        XCTAssertTrue(restored.sessions[0].pendingCatalogConfirmation)
        var confirmed = restored.sessions[0]
        confirmed.messageCount = 2
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [confirmed, confirmed])
        XCTAssertFalse(try repository.project(projectId: project.record.id).sessions[0].pendingCatalogConfirmation)
        _ = try repository.updateSessions(projectId: project.record.id, sessions: [])
        XCTAssertEqual(try repository.project(projectId: project.record.id).sessions.first?.transcriptPath, "/catalog/first.jsonl")
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
        _ = try repository.recordStartedSession(projectId: project.record.id, sessionId: row.sessionId, title: row.title, authoritativeTranscriptPath: "/catalog/project-id.jsonl")
        XCTAssertTrue(try repository.project(projectId: project.record.id).sessions[0].isArchived)
    }

    func testFirstSubmissionCountDoesNotRegressOrUnarchiveExistingSession() throws {
        let repository = ProjectRepository(projectsRoot: root)
        _ = try repository.recordStartedSession(projectId: nil, sessionId: "first", title: "Prompt", initialMessageCount: 1, authoritativeTranscriptPath: "/catalog/first.jsonl")
        XCTAssertEqual(repository.load().globalSessions[0].messageCount, 1)
        _ = try repository.setSessionArchived(projectId: nil, sessionId: "first", archived: true)
        _ = try repository.recordStartedSession(projectId: nil, sessionId: "first", title: "Prompt", initialMessageCount: 0, authoritativeTranscriptPath: "/catalog/first.jsonl")
        XCTAssertEqual(repository.load().globalSessions[0].messageCount, 1)
        XCTAssertTrue(repository.load().globalSessions[0].isArchived)
    }

    func testStaleZeroMessageCatalogDoesNotAcknowledgeFirstSubmission() throws {
        let repository = ProjectRepository(projectsRoot: root)
        var incoming = ProjectSessionSummary(sessionId: "first", title: "New conversation", messageCount: 0,
                                             relativeTime: "Before", updatedAt: Date(timeIntervalSince1970: 900))
        _ = try repository.updateSessions(projectId: nil, sessions: [incoming])
        _ = try repository.recordStartedSession(projectId: nil, sessionId: "first", title: "First prompt", initialMessageCount: 1, authoritativeTranscriptPath: "/catalog/first.jsonl")
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
        let guest = LXISHGuestPaths.workspace("projects") + "/linked"
        let transcript = LXISHGuestPaths.workspace("projects") + "/b/session-one.jsonl"
        _ = try repository.recordStartedSession(projectId: project.id, sessionId: "session-one", title: "Shared", mode: .code, authoritativeTranscriptPath: transcript)
        _ = try recordWorkspace(repository, projectId: project.id, state: SessionWorkspaceState(sessionId: "session-one", cwd: guest,
            projectRoot: project.record.directory, additionalDirectories: [guest], transcriptPath: transcript),
            mode: .code, updateProjectDirectory: true)
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
        _ = try recordWorkspace(repository, projectId: project.id, state: SessionWorkspaceState(sessionId: "draft", cwd: guest,
            projectRoot: guest, additionalDirectories: [], transcriptPath: "/catalog/draft.jsonl"), mode: .code, updateProjectDirectory: true)
        XCTAssertTrue(try repository.project(projectId: project.id).sessions.isEmpty)
        XCTAssertThrowsError(try recordWorkspace(repository, projectId: project.id, state: SessionWorkspaceState(sessionId: "draft", cwd: "/root",
            projectRoot: guest, additionalDirectories: [], transcriptPath: "/catalog/draft.jsonl"), mode: .code, updateProjectDirectory: true))
        XCTAssertEqual(try repository.project(projectId: project.id).record.activeDirectory, guest)
    }

    private func recordWorkspace(_ repository: ProjectRepository, projectId: String?, state: SessionWorkspaceState,
                                 mode: SessionMode, updateProjectDirectory: Bool) throws -> ProjectRepositoryState {
        let authority = WorkspaceWriteAuthority()
        authority.setSelected(updateProjectDirectory)
        return try repository.recordAuthenticatedWorkspace(projectId: projectId, state: state, mode: mode,
                                                            authority: authority, revision: authority.currentRevision())
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

    func writeData(_ data: Data, to url: URL, expected: OwnerCatalogExpectation, validate: (Data) throws -> Void,
                   commit: (_ replace: () throws -> Void) throws -> Void) throws {
        if url.lastPathComponent == failingFileName { throw CocoaError(.fileWriteUnknown) }
        try DefaultProjectAtomicWriter().writeData(data, to: url, expected: expected, validate: validate, commit: commit)
    }
}

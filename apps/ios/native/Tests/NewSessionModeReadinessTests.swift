import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class SessionHostOwnershipTests: XCTestCase {
    #if DEBUG
    func testUIOwnerProbeRestoresTheExactRequestedOwnerAndWorkspace() throws {
        let id = "11111111-1111-4111-8111-111111111111"
        let mock = MockConversationSource(sessionMode: .code)
        mock.model.activeSessionId = "different-fixture-owner"
        mock.prepareUIOwnerProbeWorkspace(sessionID: id, cwd: "/workspace/exact",
                                          transcriptPath: "/transcripts/exact.jsonl", enabled: true)
        let host = SessionHost()
        let owner = try host.retain(source: mock, scope: .global, mode: .code,
                                    sessionID: id, workspace: "/workspace/exact", draft: "")
        XCTAssertEqual(owner.sessionID, id)
        XCTAssertEqual(mock.model.workspaceState?.sessionId, id)
        XCTAssertEqual(mock.model.workspaceState?.cwd, "/workspace/exact")
        XCTAssertEqual(mock.model.workspaceState?.transcriptPath, "/transcripts/exact.jsonl")
    }
    #endif

    func testChatLocalAppIngressRejectsBeforeSDKAndFiles() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        var nativeCalls = 0
        func nativeStub() throws {
            nativeCalls += 1
            for name in ["source", "intent", "project"] {
                try Data("unexpected".utf8).write(to: root.appendingPathComponent(name))
            }
        }
        let hostOperations = ["createAuthoring", "prepareAuthoringSource", "bindAuthoringSource",
                              "registerAuthoring", "getAuthoringGrant", "restoreAuthoringSource",
                              "remixAuthoringSource", "importRemixSource"]
        for operation in hostOperations {
            let request = #"{"op":"\#(operation)"}"#
            for mode in [SessionMode?.some(.chat), nil] {
                XCTAssertThrowsError(try {
                    try LocalAppDevelopmentAdmission.requireAgentHostRequest(request, mode: mode)
                    try nativeStub()
                }()) { error in
                    XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "requiresCode")
                }
            }
            XCTAssertNoThrow(try LocalAppDevelopmentAdmission.requireAgentHostRequest(request, mode: .code))
        }
        for operation in ["import", "publish", "prepareAuthoring"] {
            let fields: [String: LocalAppJSON] = ["op": .string(operation)]
            for purpose in [LocalAppRequestPurpose.development(.chat), .ordinary] {
                XCTAssertThrowsError(try {
                    try LocalAppDevelopmentAdmission.requireServiceRequest(fields, purpose: purpose)
                    try nativeStub()
                }()) { error in
                    XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "requiresCode")
                }
            }
            XCTAssertNoThrow(try LocalAppDevelopmentAdmission.requireServiceRequest(fields, purpose: .development(.code)))
        }
        XCTAssertNoThrow(try LocalAppDevelopmentAdmission.requireServiceRequest(["op": .string("list")], purpose: .ordinary))
        XCTAssertNoThrow(try LocalAppDevelopmentAdmission.requireServiceRequest(
            ["op": .string("import"), "marketOrigin": .object([:])], purpose: .marketInstallation))
        XCTAssertEqual(nativeCalls, 0)
        for name in ["source", "intent", "project"] {
            XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(name).path))
        }
    }

    func testFailedLocalAppResultPresentationUsesStableKeyWithoutChangingRawText() {
        let raw = "local_app_development_requires_code: Local App development requires a Code session."
        for (headline, body) in [(raw, "details"), ("Failed", raw), ("Failed", "Error: " + raw)] {
            for fallback in [headline, body] {
                XCTAssertEqual(LocalAppToolFailurePresentation.visibleText(tool: "LocalApp", failed: true,
                    headline: headline, body: body, fallback: fallback),
                    String(localized: "local_app_development_requires_code"))
                XCTAssertEqual(LocalAppToolFailurePresentation.visibleText(tool: "Read", failed: true,
                    headline: headline, body: body, fallback: fallback), fallback)
                XCTAssertEqual(LocalAppToolFailurePresentation.visibleText(tool: "LocalApp", failed: false,
                    headline: headline, body: body, fallback: fallback), fallback)
            }
        }
        let blockedPath = "path blocked: /tmp/not_local_app_development_requires_code/file"
        XCTAssertEqual(LocalAppToolFailurePresentation.visibleText(tool: "LocalApp", failed: true,
            headline: blockedPath, body: blockedPath, fallback: blockedPath), blockedPath)
        let embedded = "error: local_app_development_requires_code: from a filename"
        XCTAssertEqual(LocalAppToolFailurePresentation.visibleText(tool: "LocalApp", failed: true,
            headline: embedded, body: nil, fallback: embedded), embedded)
        XCTAssertEqual(raw, "local_app_development_requires_code: Local App development requires a Code session.")
    }

    func testAuthoringReceiptRequiresAndMatchesItsExactSource() throws {
        let json = #"{"sourceDirectory":"/home/test/.apps/source","workspaceId":"11111111-1111-4111-8111-111111111111","projectRef":{"kind":"mobileProject","projectId":"11111111-1111-4111-8111-111111111111"},"authoringSessionId":"22222222-2222-4222-8222-222222222222","sessionMode":"code","sourceId":"33333333-3333-4333-8333-333333333333","ownerAppId":"app-id"}"#
        let receipt = try JSONDecoder().decode(LocalAppAuthoringReceipt.self, from: Data(json.utf8))
        XCTAssertEqual(receipt.sessionMode, .code)
        XCTAssertTrue(receipt.matchesSourceID("33333333-3333-4333-8333-333333333333"))
        XCTAssertFalse(receipt.matchesSourceID("44444444-4444-4444-8444-444444444444"))
        XCTAssertTrue(receipt.matchesSourceID(nil), "A valid native receipt remains usable when no client intent exists")
        XCTAssertEqual(receipt.ownerAppId, "app-id")
        let sourceField = ",\"sourceId\":\"33333333-3333-4333-8333-333333333333\""
        XCTAssertThrowsError(try JSONDecoder().decode(LocalAppAuthoringReceipt.self,
            from: Data(json.replacingOccurrences(of: sourceField, with: "").utf8)))
        XCTAssertThrowsError(try JSONDecoder().decode(LocalAppAuthoringReceipt.self,
            from: Data(json.replacingOccurrences(of: "33333333-3333-4333-8333-333333333333", with: "bad-id").utf8)))
    }

    func testColdAuthoringReceiptMustMatchAnExistingOwnerIntent() throws {
        let project = UUID().uuidString.lowercased(), session = UUID().uuidString.lowercased()
        let sourceA = UUID().uuidString.lowercased(), sourceB = UUID().uuidString.lowercased()
        let directory = "/home/test/.apps/source"
        let intent = LocalAppAuthoringIntent(intentId: UUID().uuidString.lowercased(), sourceId: sourceA,
            sourceRequestId: UUID().uuidString.lowercased(), sourceDirectory: directory, projectId: project,
            authoringSessionId: session, transcriptPath: "/catalog/session.jsonl")
        let selected = try XCTUnwrap(LocalAppAuthoringIntent.recoveryCandidate(in: [intent], projectID: project,
            sessionID: session, sourceDirectories: [directory]))
        let json = "{\"sourceDirectory\":\"\(directory)\",\"sessionMode\":\"code\",\"sourceId\":\"\(sourceB)\"}"
        let receipt = try JSONDecoder().decode(LocalAppAuthoringReceipt.self, from: Data(json.utf8))
        XCTAssertFalse(receipt.matchesSourceID(selected.sourceId), "The same owner tuple cannot substitute another source receipt")
        XCTAssertNil(try LocalAppAuthoringIntent.recoveryCandidate(in: [], projectID: project,
            sessionID: session, sourceDirectories: [directory]))
        XCTAssertThrowsError(try LocalAppAuthoringIntent.recoveryCandidate(in: [intent, intent], projectID: project,
            sessionID: session, sourceDirectories: [directory]))
        XCTAssertThrowsError(try LocalAppAuthoringIntent.recoveryCandidate(in: [intent], projectID: project,
            sessionID: session, sourceDirectories: [directory + "-changed"]))
    }

    func testActualNativeAuthoringHandlerFixtureDecodesOnIOS() async throws {
        struct Exchange<T: Decodable>: Decodable {
            let request: [String: LocalAppJSON]
            let response: LocalAppServiceEnvelope<T>
        }
        struct MarketRequest: Decodable {
            let requestId: String
            let archivePath: String
            let archiveSha256: String
            let archiveBytes: Int
            let manifest: UnifiedAppManifest
            let files: [LocalAppMarketRemixInput.File]
            let dependencies: [LocalAppMarketRemixInput.File]
            let marketOrigin: LocalAppMarketOrigin
        }
        struct Fixture: Decodable {
            let schema: Int
            let create: Exchange<LocalAppCreatedAuthoringSource>
            let sourceProof: Exchange<LocalAppVerifiedAuthoringSource>
            let register: Exchange<LocalAppAuthoringReceipt>
            let get: Exchange<LocalAppAuthoringReceipt>
            let prepare: Exchange<LocalAppAuthoringResolution>
            let sourceMissing: Exchange<LocalAppAuthoringResolution>
            let installedRemix: Exchange<LocalAppMarketRemixResult>
            let marketMissing: Exchange<LocalAppMarketRemixResult>
            let marketImport: Exchange<LocalAppMarketRemixResult>
            let marketReplay: Exchange<LocalAppMarketRemixResult>
        }
        #if SWIFT_PACKAGE
        let bundle = Bundle.module
        #else
        let bundle = Bundle(for: SessionHostOwnershipTests.self)
        #endif
        let url = try XCTUnwrap(bundle.url(forResource: "native-authoring-handler-candidate-r2", withExtension: "json", subdirectory: "Fixtures")
                                ?? bundle.url(forResource: "native-authoring-handler-candidate-r2", withExtension: "json"))
        let fixture = try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
        XCTAssertEqual(fixture.schema, 1)
        let source = try fixture.create.response.result()
        let proof = try fixture.sourceProof.response.result()
        let registered = try fixture.register.response.result()
        let fetched = try fixture.get.response.result()
        let prepared = try fixture.prepare.response.result()
        let missing = try fixture.sourceMissing.response.result()
        XCTAssertEqual(proof.sourceId, source.sourceId)
        XCTAssertEqual(proof.sourceDirectory, source.sourceDirectory)
        XCTAssertEqual(proof.sourcePath, source.sourcePath) // A recorded temporary path; no file access.
        XCTAssertTrue(registered.matchesSourceID(source.sourceId))
        XCTAssertTrue(fetched.matchesSourceID(source.sourceId))
        XCTAssertEqual(registered.sessionMode, .code)
        XCTAssertEqual(fetched.authoring, registered.authoring)
        XCTAssertNil(fetched.authoring.appId, "Publishing must not rewrite the earlier immutable owner receipt")
        let owner = try XCTUnwrap(fetched.authoring.authoringSessionId)
        guard case let .mobileProject(project)? = fetched.authoring.projectRef else {
            return XCTFail("The native fixture must contain a mobile project reference")
        }
        XCTAssertEqual(try LocalAppAuthoringHostRequest.getGrant(sourceDirectory: source.sourceDirectory,
            projectID: project, sessionID: owner, mode: .code), fixture.get.request)
        XCTAssertEqual(prepared.action, "resumeSession")
        XCTAssertEqual(prepared.sourceId, source.sourceId)
        XCTAssertNotNil(prepared.authoring?.appId, "The current published record can have newer app metadata")
        XCTAssertEqual(missing.action, "sourceMissing")
        XCTAssertNil(missing.sourceId)
        XCTAssertNil(missing.sourcePath)

        let installed = try fixture.installedRemix.response.result()
        XCTAssertEqual(installed.requestId, fixture.installedRemix.request["requestId"]?.text)
        XCTAssertEqual(installed.record.appId, installed.release.appId)
        XCTAssertNotEqual(installed.record.appId, fixture.installedRemix.request["appId"]?.text)
        XCTAssertNotNil(UUID(uuidString: installed.sourceId))

        XCTAssertEqual(fixture.marketMissing.request, fixture.marketImport.request)
        XCTAssertEqual(fixture.marketImport.request, fixture.marketReplay.request)
        let request = try JSONDecoder().decode(MarketRequest.self,
            from: JSONEncoder().encode(LocalAppJSON.object(fixture.marketImport.request)))
        let signedManifest = try XCTUnwrap(fixture.marketImport.request["manifest"])
        let signedFiles = try XCTUnwrap(fixture.marketImport.request["files"])
        let signedDependencies = try XCTUnwrap(fixture.marketImport.request["dependencies"])
        let origin = LocalAppMarketOrigin(appId: request.marketOrigin.appId, releaseId: request.marketOrigin.releaseId,
                                          digest: request.marketOrigin.digest, remixedFrom: request.marketOrigin.appId)
        let input = LocalAppMarketRemixInput(requestId: request.requestId, archivePath: request.archivePath,
            archiveSha256: request.archiveSha256, archiveBytes: request.archiveBytes, manifest: request.manifest,
            files: request.files, dependencies: request.dependencies, signedManifest: signedManifest,
            signedFiles: signedFiles, signedDependencies: signedDependencies, marketOrigin: origin)
        let actualRequest = try LocalAppAuthoringHostRequest.importRemix(input, archivePath: input.archivePath, mode: .code)
        XCTAssertEqual(actualRequest, fixture.marketImport.request, "Send exactly the native handler's accepted payload")
        guard case let .object(manifest) = signedManifest else { return XCTFail("Expected a signed manifest") }
        XCTAssertEqual(manifest["dataSchema"], .null, "Explicit null is part of the signed payload")
        XCTAssertTrue(request.files.contains(where: { $0.path == ".gitignore" }))
        XCTAssertTrue(request.files.contains(where: { $0.path == ".well-known/security.txt" }))

        var submissions = 0, downloads = 0
        let imported = try await LocalAppRemixSubmission.run(mode: .code, submit: {
            submissions += 1
            return try (submissions == 1 ? fixture.marketMissing.response : fixture.marketImport.response).result()
        }, restoreArchive: { downloads += 1 })
        XCTAssertEqual(submissions, 2)
        XCTAssertEqual(downloads, 1, "Only the native archiveMissing result requests the exact archive")
        XCTAssertEqual(imported.requestId, request.requestId)
        XCTAssertEqual(imported.record.appId, imported.release.appId)
        XCTAssertNotEqual(imported.record.appId, request.marketOrigin.appId)
        XCTAssertNotNil(UUID(uuidString: imported.sourceId))
        XCTAssertEqual(imported.record.marketOrigin?.appId, request.marketOrigin.appId)
        XCTAssertEqual(imported.record.marketOrigin?.remixedFrom, request.marketOrigin.appId)
        XCTAssertTrue(imported.release.files.contains(where: { $0.path == ".gitignore" }))
        XCTAssertTrue(imported.release.files.contains(where: { $0.path == ".well-known/security.txt" }))

        let replay = try await LocalAppRemixSubmission.run(mode: .code, submit: {
            try fixture.marketReplay.response.result()
        }, restoreArchive: { XCTFail("A committed receipt must replay without the deleted ZIP") })
        XCTAssertEqual(replay.requestId, imported.requestId)
        XCTAssertEqual(replay.sourceId, imported.sourceId)
        XCTAssertEqual(replay.sourceDirectory, imported.sourceDirectory)
        XCTAssertEqual(replay.sourcePath, imported.sourcePath)
        XCTAssertEqual(replay.record, imported.record)
        XCTAssertEqual(replay.release, imported.release)
    }

    func testMarketRemixReplaysBeforeArchiveRecoveryAndRejectsChat() async throws {
        var calls: [String] = []
        let replayed = try await LocalAppRemixSubmission.run(mode: .code, submit: {
            calls.append("submit"); return "existing source"
        }, restoreArchive: { calls.append("download") })
        XCTAssertEqual(replayed, "existing source")
        XCTAssertEqual(calls, ["submit"], "A committed receipt must not depend on the old ZIP")

        calls.removeAll()
        let restored = try await LocalAppRemixSubmission.run(mode: .code, submit: {
            calls.append("submit")
            if calls.count == 1 { throw LocalAppServiceFailure("archiveMissing", "missing") }
            return "new source"
        }, restoreArchive: { calls.append("download") })
        XCTAssertEqual(restored, "new source")
        XCTAssertEqual(calls, ["submit", "download", "submit"])

        calls.removeAll()
        do {
            let _: String = try await LocalAppRemixSubmission.run(mode: .code, submit: {
                calls.append("submit"); throw LocalAppServiceFailure("integrity", "changed")
            }, restoreArchive: { calls.append("download") })
            XCTFail("An uncertain or corrupt result must not create a new source")
        } catch { XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "integrity") }
        XCTAssertEqual(calls, ["submit"])

        calls.removeAll()
        do {
            let _: String = try await LocalAppRemixSubmission.run(mode: .chat, submit: {
                calls.append("submit"); return "unexpected"
            }, restoreArchive: { calls.append("download") })
            XCTFail("Chat must be rejected before either SDK or archive work")
        } catch { XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "requiresCode") }
        XCTAssertTrue(calls.isEmpty)
    }

    func testMarketRemixVerifiesRealZipWithoutFilteringHiddenFilesOrExtracting() throws {
        // A real ZIP containing lingxi-app.json, .gitignore, and
        // .well-known/security.txt; native import owns ZIP/path validation.
        let encoded = "UEsDBBQAAAAAAAAAIViMwfuJDAAAAAwAAAAPAAAAbGluZ3hpLWFwcC5qc29ueyJzY2hlbWEiOjF9UEsDBBQAAAAAAAAAIVisLGB1CAAAAAgAAAAKAAAALmdpdGlnbm9yZWZpeHR1cmUKUEsDBBQAAAAAAAAAIVisLGB1CAAAAAgAAAAYAAAALndlbGwta25vd24vc2VjdXJpdHkudHh0Zml4dHVyZQpQSwECFAMUAAAAAAAAACFYjMH7iQwAAAAMAAAADwAAAAAAAAAAAAAApIEAAAAAbGluZ3hpLWFwcC5qc29uUEsBAhQDFAAAAAAAAAAhWKwsYHUIAAAACAAAAAoAAAAAAAAAAAAAAKSBOQAAAC5naXRpZ25vcmVQSwECFAMUAAAAAAAAACFYrCxgdQgAAAAIAAAAGAAAAAAAAAAAAAAApIFpAAAALndlbGwta25vd24vc2VjdXJpdHkudHh0UEsFBgAAAAADAAMAuwAAAKcAAAAAAA=="
        let bytes = try XCTUnwrap(Data(base64Encoded: encoded))
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let archive = root.appendingPathComponent("source.zip")
        try bytes.write(to: archive)
        let sha256 = "3d77da91bf868252770c72d6986678f25a3b398eaf5ce33f4f9fab31e8cf7eb4"
        XCTAssertEqual(try MarketRemixArchive.readVerified(archive, size: 376, sha256: sha256), bytes)
        XCTAssertThrowsError(try MarketRemixArchive.readVerified(archive, size: 375, sha256: sha256))
        XCTAssertThrowsError(try MarketRemixArchive.readVerified(archive, size: 376, sha256: String(repeating: "0", count: 64)))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(".gitignore").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent(".well-known").path))
    }

    func testMarketRemixHostRequestUsesCapturedReleaseAndRequiresCode() throws {
        let manifestData = Data(#"{"schema":1,"name":"Notes","summary":null,"entry":{"type":"web","path":"index.html","process":null},"targets":[],"remix":{"allowed":true}}"#.utf8)
        let manifest = try JSONDecoder().decode(UnifiedAppManifest.self, from: manifestData)
        let signedManifest = try JSONDecoder().decode(LocalAppJSON.self, from: manifestData)
        let file = LocalAppMarketRemixInput.File(path: "lingxi-app.json", sizeBytes: 1,
            sha256: String(repeating: "a", count: 64), executable: false)
        let requestID = UUID().uuidString.lowercased()
        let path = "/config/local-app-imports/market-\(requestID)/source.zip"
        let input = LocalAppMarketRemixInput(requestId: requestID, archivePath: path,
            archiveSha256: String(repeating: "b", count: 64), archiveBytes: 12,
            manifest: manifest, files: [file], dependencies: [file],
            signedManifest: signedManifest, signedFiles: try .encode([file]), signedDependencies: try .encode([file]),
            marketOrigin: .init(appId: "market-app", releaseId: "release-1", digest: "release-digest", remixedFrom: "market-app"))
        XCTAssertThrowsError(try LocalAppAuthoringHostRequest.importRemix(input, archivePath: path, mode: .chat))
        XCTAssertThrowsError(try LocalAppAuthoringHostRequest.importRemix(input, archivePath: path + ".other", mode: .code))
        let request = try LocalAppAuthoringHostRequest.importRemix(input, archivePath: path, mode: .code)
        XCTAssertEqual(request["op"], .string("importRemixSource"))
        XCTAssertEqual(request["requestId"], .string(requestID))
        XCTAssertEqual(request["archivePath"], .string(path))
        XCTAssertEqual(request["manifest"], signedManifest, "Explicit null belongs to the signed release input")
        XCTAssertEqual(request["dependencies"], try .encode([file]))
        XCTAssertEqual(request["marketOrigin"], .object(["appId": .string("market-app"),
            "releaseId": .string("release-1"), "digest": .string("release-digest")]))
    }

    func testMarketDistributionRetainsSignedNullsAndInventory() throws {
        let json = #"{"schema":2,"appId":"market","versionId":"v1","sequence":1,"releaseDigest":"digest","manifest":{"schema":1,"name":"Notes","summary":null,"entry":{"type":"web","path":"index.html","process":null},"targets":[],"remix":{"allowed":true}},"files":[{"path":"lingxi-app.json","sizeBytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":false}],"dependencies":[],"artifact":{"format":"zip","filename":"source.zip","sizeBytes":3,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","downloadUrl":"https://example.test/source?signature=secret"},"executionConsentRequired":false}"#
        let distribution = try JSONDecoder().decode(MarketDistribution.self, from: Data(json.utf8))
        guard case let .object(manifest) = distribution.signedManifest,
              case let .object(entry)? = manifest["entry"] else { return XCTFail("Expected signed manifest object") }
        XCTAssertEqual(manifest["summary"], .null)
        XCTAssertEqual(entry["process"], .null)
        XCTAssertNotEqual(distribution.signedManifest, try LocalAppJSON.encode(distribution.manifest),
                          "Typed projection omits null and cannot replace the signed input")
        guard case let .array(files) = distribution.signedFiles else { return XCTFail("Expected signed file inventory") }
        XCTAssertEqual(files.count, 1)
        XCTAssertEqual(distribution.signedDependencies, .array([]))
        XCTAssertFalse(String(decoding: try JSONEncoder().encode(distribution.signedManifest), as: UTF8.self).contains("signature=secret"))

        var marketSelection = LocalAppRemixSelection<MarketDistribution>()
        marketSelection.capture(distribution)
        let refreshed = try JSONDecoder().decode(MarketDistribution.self,
            from: Data(json.replacingOccurrences(of: "\"versionId\":\"v1\"", with: "\"versionId\":\"v2\"").utf8))
        XCTAssertEqual(refreshed.versionId, "v2")
        XCTAssertEqual(marketSelection.take()?.versionId, "v1", "Confirm the release selected before refresh")
        XCTAssertNil(marketSelection.take(), "Confirmation is single-use")
    }

    func testInstalledAppRemixConfirmationKeepsItsSelectedOrigin() {
        func record(_ release: String) -> UnifiedAppRecord {
            UnifiedAppRecord(appId: "local", name: "Notes", summary: "", currentRevision: 1, latestRevision: 1,
                authoring: nil, marketOrigin: .init(appId: "market", releaseId: release, digest: "digest-" + release, remixedFrom: nil),
                originPort: 1, dataSchemaVersion: 1, executionGrants: [], createdAtMs: 0, updatedAtMs: 0, removed: false)
        }
        var selection = LocalAppRemixSelection<UnifiedAppRecord>()
        selection.capture(record("v1"))
        let refreshed = record("v2")
        XCTAssertEqual(refreshed.marketOrigin?.releaseId, "v2")
        XCTAssertEqual(selection.take()?.marketOrigin?.releaseId, "v1", "Confirm the app origin displayed before refresh")
        XCTAssertNil(selection.take())
    }

    func testBackgroundWorkspaceNotificationStaysWithItsRetainedOwner() throws {
        let host = SessionHost()
        let a = MockConversationSource(sessionMode: .chat)
        let b = MockConversationSource(sessionMode: .code)
        let aID = "11111111-1111-4111-8111-111111111111"
        let bID = "22222222-2222-4222-8222-222222222222"
        a.model.activeSessionId = aID
        b.model.activeSessionId = bID
        let first = try host.retain(source: a, scope: .global, mode: .chat, sessionID: aID,
                                    workspace: "/home/a", draft: "")
        let second = try host.retain(source: b, scope: .project("p"), mode: .code, sessionID: bID,
                                     workspace: "/home/b", draft: "")
        host.select(second)
        var notified: [UUID] = []
        host.onWorkspaceChanged = { notified.append($0.id) }
        a.model.workspaceState = SessionWorkspaceState(sessionId: aID, cwd: "/home/a/new",
            projectRoot: "/home/a", additionalDirectories: [], transcriptPath: "/catalog/a.jsonl")
        host.sync(first)
        XCTAssertEqual(notified, [first.id])
        XCTAssertEqual(host.selectedEntryID, second.id)
        host.sync(first)
        XCTAssertEqual(notified, [first.id], "unrelated model updates must not refetch the workspace")
    }

    @MainActor
    func testExplicitAudioReservesAgainstQueuedAutomaticPlayback() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject(), c = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        lane.enqueueAutomatic(c)
        XCTAssertTrue(lane.reserveExplicit(b) === a)
        XCTAssertTrue(lane.release(a))
        XCTAssertNil(lane.nextAutomatic())
        XCTAssertFalse(lane.acquire(c, kind: .automatic))
        XCTAssertTrue(lane.finishReservation(b, canAcquire: true))
        XCTAssertTrue(lane.owner === b)
        XCTAssertTrue(lane.release(b))
        XCTAssertTrue(lane.nextAutomatic() === c)
        XCTAssertTrue(lane.acquire(c, kind: .automatic))
    }

    @MainActor
    func testExplicitAudioNeverPreemptsAnotherExplicitOwner() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .explicit))
        XCTAssertNil(lane.reserveExplicit(b))
        XCTAssertFalse(lane.acquire(b, kind: .explicit))
        XCTAssertFalse(lane.acquire(a, kind: .automatic))
        XCTAssertTrue(lane.owner === a)
    }

    @MainActor
    func testFailedAudioStopKeepsAutomaticOwnerAndQueuedWork() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject(), c = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        lane.enqueueAutomatic(c)
        XCTAssertTrue(lane.reserveExplicit(b) === a)
        XCTAssertFalse(lane.finishReservation(b, canAcquire: false))
        XCTAssertTrue(lane.owner === a)
        XCTAssertNil(lane.nextAutomatic())
        XCTAssertTrue(lane.release(a))
        XCTAssertTrue(lane.nextAutomatic() === c)
    }

    @MainActor
    func testOwnerCanReserveItsOwnAutomaticPlaybackForExplicitCapture() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        XCTAssertTrue(lane.reserveExplicit(a) === a)
        XCTAssertTrue(lane.release(a))
        XCTAssertTrue(lane.finishReservation(a, canAcquire: true))
        XCTAssertTrue(lane.owner === a)
        XCTAssertEqual(lane.kind, .explicit)
    }

    @MainActor
    func testCancelledExplicitReservationLetsQueuedAutomaticOwnerProceed() {
        let lane = VoiceAudioOwnership<NSObject>()
        let a = NSObject(), b = NSObject(), c = NSObject()
        XCTAssertTrue(lane.acquire(a, kind: .automatic))
        lane.enqueueAutomatic(c)
        XCTAssertTrue(lane.reserveExplicit(b) === a)
        XCTAssertTrue(lane.release(a))
        XCTAssertFalse(lane.finishReservation(b, canAcquire: false))
        XCTAssertTrue(lane.nextAutomatic() === c)
        XCTAssertTrue(lane.acquire(c, kind: .automatic))
    }

    @MainActor
    func testAutomaticPlaybackDeadlineRequiresActualTaskCompletion() async {
        let (release, continuation) = AsyncStream<Void>.makeStream()
        let operation = Task { @MainActor in for await _ in release { break } }
        let timedOut = await VoiceAudioStopDeadline.completed(operation, within: .milliseconds(10))
        XCTAssertFalse(timedOut)
        continuation.yield(())
        continuation.finish()
        let completed = await VoiceAudioStopDeadline.completed(operation, within: .seconds(1))
        XCTAssertTrue(completed)
    }

    @MainActor
    func testBlockedAutomaticOwnerDoesNotStarveTheNextQueuedOwner() {
        let lane = VoiceAudioOwnership<NSObject>()
        let active = NSObject(), blocked = NSObject(), ready = NSObject()
        XCTAssertTrue(lane.acquire(active, kind: .explicit))
        lane.enqueueAutomatic(blocked)
        lane.enqueueAutomatic(ready)
        XCTAssertTrue(lane.release(active))
        XCTAssertEqual(lane.waitingCount, 2)
        XCTAssertTrue(lane.nextAutomatic() === blocked)
        lane.enqueueAutomatic(blocked)
        XCTAssertTrue(lane.nextAutomatic() === ready)
        XCTAssertTrue(lane.acquire(ready, kind: .automatic))
        XCTAssertNil(lane.nextAutomatic())
    }

    @MainActor
    func testLatestNavigationKeepsCompleteDestinationWhileEarlierPreparationIsDelayed() async {
        struct Destination: Equatable {
            let scope: ConversationScope
            let mode: SessionMode
            let sessionID: String
        }
        let a = Destination(scope: .project("a"), mode: .code, sessionID: "session-a")
        let b = Destination(scope: .global, mode: .chat, sessionID: "session-b")
        let c = Destination(scope: .project("c"), mode: .code, sessionID: "session-c")
        let (release, continuation) = AsyncStream<Void>.makeStream()
        var requests = ConversationNavigationIntents<Destination>()
        var selected: Destination?
        XCTAssertTrue(requests.submit(a, whileBusy: false))
        let generation = requests.generation
        let preparingA = Task { @MainActor in
            for await _ in release { break }
            if requests.isCurrent(generation) { selected = a }
        }
        await Task.yield()
        XCTAssertFalse(requests.submit(b, whileBusy: true))
        XCTAssertFalse(requests.submit(c, whileBusy: true))
        continuation.yield(())
        continuation.finish()
        await preparingA.value
        XCTAssertNil(selected)
        XCTAssertEqual(requests.takePending(), c)
        XCTAssertNil(requests.takePending())
        XCTAssertTrue(requests.isCurrent(requests.generation))
        selected = c
        XCTAssertEqual(selected, c)
    }

    func testHandoffNeverNavigatesAfterSelectionChanges() {
        let revision = UUID()
        XCTAssertTrue(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: true,
            conversationVisible: true, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: UUID(), ownerMatches: true,
            conversationVisible: true, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: false,
            conversationVisible: true, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: true,
            conversationVisible: false, switching: false))
        XCTAssertFalse(SessionHandoffPresentationPolicy.mayOpenAutomatically(
            requestRevision: revision, currentRevision: revision, ownerMatches: true,
            conversationVisible: true, switching: true))
    }

    func testHandoffDraftRetainsUnpairedUTF16Units() {
        let units: [UInt16] = [0x61, 0xD800, 0x62]
        let draft = SessionHandoffDraft(mode: .chat, sourceCodeUnits: units,
            reference: SessionHandoffReference(sourceScopeKey: "project.p", sourceSessionID: "source",
                sourceMode: .code, targetMode: .chat, cutoff: "fixed", requestID: "request"))
        XCTAssertEqual(draft.displayText, "a\u{FFFD}b")
        XCTAssertEqual(draft.sourceCodeUnits, units)
        XCTAssertEqual(draft.reference.cutoff, "fixed")
    }

    func testSeparateOwnersRetainTurnsAndDraftsAcrossNavigation() async throws {
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        let second = MockConversationSource(sessionMode: .code)
        let firstID = "11111111-1111-4111-8111-111111111111"
        let secondID = "22222222-2222-4222-8222-222222222222"
        first.model.activeSessionId = firstID
        second.model.activeSessionId = secondID
        let firstEntry = try host.retain(source: first, scope: .global, mode: .chat,
                                     sessionID: firstID, workspace: "/home/global", draft: "first draft")
        let secondEntry = try host.retain(source: second, scope: .project("p"), mode: .code,
                                      sessionID: secondID, workspace: "/home/p", draft: "second draft")
        let firstToken = await first.send("work continues")
        XCTAssertNotNil(firstToken)
        let secondToken = await second.send("other work")
        XCTAssertNotNil(secondToken)
        XCTAssertTrue(first.model.streaming)
        XCTAssertTrue(second.model.streaming)
        XCTAssertTrue(host.entry(sessionID: firstID, scope: .global, mode: .chat) === firstEntry)
        XCTAssertTrue(host.entry(sessionID: secondID, scope: .project("p"), mode: .code) === secondEntry)
        XCTAssertNil(host.entry(sessionID: firstID, scope: .global, mode: .code))
        XCTAssertNil(host.entry(sessionID: firstID, scope: .project("p"), mode: .chat))
        second.model.workspaceState = SessionWorkspaceState(sessionId: secondID, cwd: "/home/current",
            projectRoot: "/home/p", additionalDirectories: [], transcriptPath: "")
        // Even if a previously indexed cwd differs from both the initial and
        // live cwd, navigation must reuse the same admitted owner.
        XCTAssertTrue(host.entry(sessionID: secondID, scope: .project("p"), mode: .code) === secondEntry)
        XCTAssertEqual(firstEntry.draft, "first draft")
        XCTAssertEqual(secondEntry.draft, "second draft")
        XCTAssertTrue(first.model.streaming, "navigation must not cancel the hidden owner")
        XCTAssertTrue(second.model.streaming)
    }

    func testDuplicateUUIDIsReusedOnlyForTheSameScopeAndMode() throws {
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        let sessionID = "11111111-1111-4111-8111-111111111111"
        first.model.activeSessionId = sessionID
        let retained = try host.retain(source: first, scope: .global, mode: .chat,
                                       sessionID: sessionID, workspace: "/global", draft: "keep")
        XCTAssertTrue(try host.entryForClaim(sessionID: "sess:" + sessionID.uppercased(), scope: .global, mode: .chat) === retained)
        XCTAssertThrowsError(try host.entryForClaim(sessionID: sessionID, scope: .project("p"), mode: .chat))
        XCTAssertThrowsError(try host.entryForClaim(sessionID: sessionID, scope: .global, mode: .code))
        let duplicate = MockConversationSource(sessionMode: .chat)
        duplicate.model.activeSessionId = sessionID
        XCTAssertThrowsError(try host.retain(source: duplicate, scope: .global, mode: .chat,
                                             sessionID: sessionID, workspace: "/global", draft: "wrong"))
        XCTAssertEqual(host.entries.count, 1)
        XCTAssertEqual(retained.draft, "keep")
    }

    func testUnindexedAuthoringRecoveryReusesOnlyItsExactRetainedOwner() throws {
        let host = SessionHost()
        let projectID = UUID().uuidString.lowercased()
        let sessionID = UUID().uuidString.lowercased()
        let source = MobileUserStorage.directories().appsGuestPath + "/saved-source"
        let intent = LocalAppAuthoringIntent(intentId: UUID().uuidString.lowercased(),
            sourceId: UUID().uuidString.lowercased(), sourceRequestId: UUID().uuidString.lowercased(),
            sourceDirectory: source, projectId: projectID, authoringSessionId: sessionID,
            transcriptPath: "/transcripts/owner.jsonl")
        let owner = MockConversationSource(sessionMode: .code)
        owner.model.activeSessionId = sessionID
        let retained = try host.retain(source: owner, scope: .project(projectID), mode: .code,
                                       sessionID: sessionID, workspace: source, draft: "keep")
        let other = MockConversationSource(sessionMode: .chat)
        other.model.activeSessionId = UUID().uuidString.lowercased()
        let otherEntry = try host.retain(source: other, scope: .global, mode: .chat,
                                         sessionID: other.model.activeSessionId, workspace: "/global", draft: "other")
        host.select(otherEntry)
        let found = try XCTUnwrap(host.entryForClaim(sessionID: sessionID, scope: .project(projectID), mode: .code))
        XCTAssertTrue(found === retained)
        let workspace = SessionWorkspaceState(sessionId: sessionID, cwd: source, projectRoot: source,
            additionalDirectories: [], transcriptPath: "/transcripts/owner.jsonl")
        XCTAssertTrue(intent.matchesRetainedOwner(scope: found.scope, mode: found.mode,
                                                   sessionID: found.sessionID, workspace: workspace))
        var moved = workspace
        moved = SessionWorkspaceState(sessionId: sessionID, cwd: source + "/other", projectRoot: source,
            additionalDirectories: [], transcriptPath: workspace.transcriptPath)
        XCTAssertFalse(intent.matchesRetainedOwner(scope: found.scope, mode: found.mode,
                                                    sessionID: found.sessionID, workspace: moved))
        XCTAssertEqual(host.selectedEntryID, otherEntry.id)
        XCTAssertEqual(retained.draft, "keep")
        XCTAssertEqual(host.entries.count, 2)
    }

    func testDurableAuthoringIdentityUpdateMatchesOnlyTheSameInFlightRequest() {
        let base = LocalAppAuthoringIntent(intentId: UUID().uuidString.lowercased(),
            sourceId: UUID().uuidString.lowercased(), sourceRequestId: UUID().uuidString.lowercased(),
            sourceDirectory: "/source", projectId: UUID().uuidString.lowercased())
        var committed = base
        committed.authoringSessionId = UUID().uuidString.lowercased()
        committed.transcriptPath = "/transcripts/one.jsonl"
        XCTAssertTrue(committed.matchesInFlightAttempt(base))
        XCTAssertTrue(base.matchesInFlightAttempt(committed))
        var conflicting = committed
        conflicting.authoringSessionId = UUID().uuidString.lowercased()
        XCTAssertFalse(committed.matchesInFlightAttempt(conflicting))
        conflicting = committed
        conflicting.transcriptPath = "/transcripts/another.jsonl"
        XCTAssertFalse(committed.matchesInFlightAttempt(conflicting))
    }

    func testLateIdentityClaimsAreAtomicAndCannotRebindAnOwner() throws {
        let host = SessionHost()
        let first = MockConversationSource(sessionMode: .chat)
        let second = MockConversationSource(sessionMode: .code)
        let a = try host.retain(source: first, scope: .global, mode: .chat,
                                sessionID: "", workspace: "/global", draft: "a")
        let b = try host.retain(source: second, scope: .project("p"), mode: .code,
                                sessionID: "", workspace: "/p", draft: "b")
        let id = "22222222-2222-4222-8222-222222222222"
        XCTAssertTrue(host.claimIdentity(id, for: first, scope: .global, mode: .chat))
        XCTAssertFalse(host.claimIdentity("sess:" + id.uppercased(), for: second, scope: .project("p"), mode: .code))
        XCTAssertFalse(host.claimIdentity("33333333-3333-4333-8333-333333333333", for: first, scope: .global, mode: .chat))
        XCTAssertEqual(a.sessionID, id)
        XCTAssertEqual(b.sessionID, "")
        XCTAssertEqual(a.draft, "a")
        XCTAssertEqual(b.draft, "b")
    }

    func testPreparedIdentityTransfersBeforeListenerCallbackAndBlocksScheduler() throws {
        let host = SessionHost()
        let gate = CronOwnerAdmissionGate()
        gate.sessionHost = host
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        let otherID = "22222222-2222-4222-8222-222222222222"
        // Native prepare has authenticated the UUID, but its listener has not
        // yet delivered SessionStarted to the projected model.
        XCTAssertTrue(host.claimIdentity(id, for: source, scope: .global, mode: .chat))
        XCTAssertFalse(host.claimIdentity(otherID, for: source, scope: .global, mode: .chat))
        XCTAssertTrue(host.hasPreparingClaim(sessionID: id))
        let entry = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: "", workspace: "/global", draft: "")
        host.endPreparingClaim(of: source)
        XCTAssertEqual(entry.sessionID, id)
        XCTAssertFalse(host.hasPreparingClaim(sessionID: id))
        XCTAssertFalse(gate.allowsUnclaimedReservation(sessionID: id))
        XCTAssertTrue(try host.entryForClaim(sessionID: id, scope: .global, mode: .chat) === entry)
        source.model.activeSessionId = id
        host.sync(entry)
        XCTAssertEqual(entry.sessionID, id)
    }

    func testFailedPreparedOwnerRemainsFencedUntilExactNativeDrain() async throws {
        let host = SessionHost()
        let gate = CronOwnerAdmissionGate()
        gate.sessionHost = host
        let id = UUID().uuidString.lowercased()
        let request = UUID().uuidString.lowercased()
        let otherID = UUID().uuidString.lowercased()
        let project = UUID().uuidString.lowercased()
        let source = MockConversationSource(sessionMode: .code)
        var catalogReady = true
        host.admitsSessionIdentity = { _, _, _ in catalogReady }
        XCTAssertTrue(host.claimIdentity(id, for: source, scope: .project(project), mode: .code))
        // The catalog can fail after native prepare and before retention.
        catalogReady = false
        XCTAssertThrowsError(try host.retain(source: source, scope: .project(project), mode: .code,
            sessionID: id, workspace: "/source", draft: ""))
        try host.quarantineFailedPreparation(source, requestID: request, scope: .project(project),
                                             mode: .code, sessionID: id)
        host.endPreparingClaim(of: source)
        XCTAssertTrue(host.hasPreparingClaim(sessionID: otherID), "An unverified identity stays globally fenced")
        XCTAssertThrowsError(try host.identifyFailedPreparation(requestID: request, source: source, sessionID: otherID))
        XCTAssertTrue(host.hasPreparingClaim(sessionID: otherID), "A mismatched native UUID must not narrow the fence")
        try host.identifyFailedPreparation(requestID: request, source: source, sessionID: id)
        XCTAssertFalse(host.hasPreparingClaim(sessionID: otherID))
        XCTAssertFalse(gate.allowsUnclaimedReservation(sessionID: id))
        var sessionWaits = 0
        let mediaUnknown = await host.releaseFailedPreparationAfterDrain(requestID: request, source: source,
            sessionID: id, waitForRelease: {
                await SessionOwnerDrainPolicy.confirmed(waitForMedia: { false }, waitForSession: {
                    sessionWaits += 1
                    return true
                })
            })
        XCTAssertFalse(mediaUnknown)
        XCTAssertEqual(sessionWaits, 0)
        let sessionTimedOut = await host.releaseFailedPreparationAfterDrain(requestID: request, source: source,
            sessionID: id, waitForRelease: {
                await SessionOwnerDrainPolicy.confirmed(waitForMedia: { true }, waitForSession: { false })
            })
        XCTAssertFalse(sessionTimedOut)
        catalogReady = true
        let retry = MockConversationSource(sessionMode: .code)
        XCTAssertFalse(host.claimIdentity(id, for: retry, scope: .project(project), mode: .code))
        let released = await host.releaseFailedPreparationAfterDrain(requestID: request, source: source,
            sessionID: id, waitForRelease: {
                await SessionOwnerDrainPolicy.confirmed(waitForMedia: { true }, waitForSession: { true })
            })
        XCTAssertTrue(released)
        XCTAssertTrue(host.claimIdentity(id, for: retry, scope: .project(project), mode: .code))
    }

    func testFailedOrdinaryReplacementKeepsOldOwnerUntilRetryDrainsExactSource() async throws {
        let host = SessionHost()
        let session = UUID().uuidString.lowercased(), request = UUID().uuidString.lowercased()
        let old = MockConversationSource(sessionMode: .chat)
        old.model.activeSessionId = session
        var catalogReady = true
        host.admitsSessionIdentity = { _, _, _ in catalogReady }
        let entry = try host.retain(source: old, scope: .global, mode: .chat,
                                    sessionID: session, workspace: "/chat", draft: "keep")
        host.select(entry)
        let replacement = MockConversationSource(sessionMode: .chat)
        replacement.model.activeSessionId = session
        XCTAssertTrue(host.beginReplacement(of: old, with: replacement, sessionID: session, scope: .global, mode: .chat))
        catalogReady = false
        XCTAssertThrowsError(try host.replaceIdleSource(old, with: replacement, scope: .global, mode: .chat,
                                                        sessionID: session, workspace: "/chat", draft: "keep"))
        try host.quarantineFailedPreparation(replacement, requestID: request, scope: .global, mode: .chat, sessionID: session)
        host.endPreparingClaim(of: replacement)
        XCTAssertTrue(host.entry(for: old) === entry)
        XCTAssertEqual(host.selectedEntryID, entry.id)
        XCTAssertEqual(entry.draft, "keep")
        XCTAssertEqual(host.failedPreparationIDs(scope: .global, mode: .chat, sessionID: session), [request])
        XCTAssertTrue(host.failedPreparationIDs(scope: .global, mode: .code, sessionID: session).isEmpty)
        try host.identifyFailedPreparation(requestID: request, source: replacement, sessionID: session)
        let waiting = await host.releaseFailedPreparationAfterDrain(requestID: request, source: replacement,
            sessionID: session, waitForRelease: { false })
        XCTAssertFalse(waiting)
        XCTAssertTrue(host.hasPreparingClaim(sessionID: session))
        let released = await host.releaseFailedPreparationAfterDrain(requestID: request, source: replacement,
            sessionID: session, waitForRelease: { true })
        XCTAssertTrue(released)
        XCTAssertTrue(host.entry(for: old) === entry)
        catalogReady = true
        let retry = MockConversationSource(sessionMode: .chat)
        retry.model.activeSessionId = session
        XCTAssertTrue(host.beginReplacement(of: old, with: retry, sessionID: session, scope: .global, mode: .chat))
        host.endPreparingClaim(of: retry)

        let blankRequest = UUID().uuidString.lowercased()
        let blank = MockConversationSource(sessionMode: .code)
        try host.quarantineFailedPreparation(blank, requestID: blankRequest, scope: .global, mode: .code, sessionID: nil)
        XCTAssertEqual(host.failedPreparationIDs(scope: .global, mode: .code, sessionID: ""), [blankRequest])
        XCTAssertTrue(host.failedPreparationIDs(scope: .global, mode: .chat, sessionID: "").isEmpty)
        XCTAssertTrue(host.hasPreparingClaim(sessionID: UUID().uuidString), "Unknown native identity remains globally fenced")
    }

    func testConcurrentFailedPreparationWaitersJoinTheSameConfirmedRelease() async throws {
        let host = SessionHost(), source = MockConversationSource(sessionMode: .chat)
        let request = UUID().uuidString.lowercased(), session = UUID().uuidString.lowercased()
        try host.quarantineFailedPreparation(source, requestID: request, scope: .global, mode: .chat, sessionID: session)
        try host.identifyFailedPreparation(requestID: request, source: source, sessionID: session)
        let started = expectation(description: "both release waiters started")
        started.expectedFulfillmentCount = 2
        var firstWait: CheckedContinuation<Bool, Never>?
        var secondWait: CheckedContinuation<Bool, Never>?
        let first = Task { await host.releaseFailedPreparationAfterDrain(requestID: request, source: source,
            sessionID: session, waitForRelease: {
                await withCheckedContinuation { firstWait = $0; started.fulfill() }
            }) }
        let second = Task { await host.releaseFailedPreparationAfterDrain(requestID: request, source: source,
            sessionID: session, waitForRelease: {
                await withCheckedContinuation { secondWait = $0; started.fulfill() }
            }) }
        await fulfillment(of: [started], timeout: 1)
        guard let firstWait, let secondWait else { return XCTFail("Both release waiters must suspend") }
        firstWait.resume(returning: true)
        let firstResult = await first.value
        secondWait.resume(returning: true)
        let secondResult = await second.value
        XCTAssertTrue(firstResult)
        XCTAssertTrue(secondResult)
        XCTAssertNil(host.failedPreparation(requestID: request))
    }

    #if DEBUG
    func testConcurrentFailedPreparationRetriesJoinBeforeIdentityIsKnown() async throws {
        let host = SessionHost(), source = MockConversationSource(sessionMode: .chat)
        let request = UUID().uuidString.lowercased(), session = UUID().uuidString.lowercased()
        try host.quarantineFailedPreparation(source, requestID: request, scope: .global, mode: .chat, sessionID: session)
        let firstStarted = expectation(description: "first retirement started")
        let secondJoined = expectation(description: "second retry joined the existing retirement")
        host.onFailedPreparationDrainJoinForTesting = { secondJoined.fulfill() }
        var resumeRetirement: CheckedContinuation<Void, Never>?
        var retirements = 0
        let first = Task {
            try await host.drainFailedPreparation(requestID: request) { captured in
                XCTAssertTrue(captured === source)
                retirements += 1
                await withCheckedContinuation { resumeRetirement = $0; firstStarted.fulfill() }
                try host.identifyFailedPreparation(requestID: request, source: captured, sessionID: session)
                let released = await host.releaseFailedPreparationAfterDrain(requestID: request, source: captured,
                    sessionID: session, waitForRelease: { true })
                XCTAssertTrue(released)
            }
        }
        await fulfillment(of: [firstStarted], timeout: 1)
        let second = Task {
            try await host.drainFailedPreparation(requestID: request) { _ in
                XCTFail("Concurrent retries must join the captured preparation's retirement")
            }
        }
        await fulfillment(of: [secondJoined], timeout: 1)
        guard let resumeRetirement else { return XCTFail("Retirement must be suspended") }
        resumeRetirement.resume()
        try await first.value
        try await second.value
        XCTAssertEqual(retirements, 1)
        XCTAssertNil(host.failedPreparation(requestID: request))
    }
    #endif

    func testFailedPreparationDrainCanRetryAfterTimeout() async throws {
        let host = SessionHost(), source = MockConversationSource(sessionMode: .chat)
        let request = UUID().uuidString.lowercased(), session = UUID().uuidString.lowercased()
        try host.quarantineFailedPreparation(source, requestID: request, scope: .global, mode: .chat, sessionID: session)
        do {
            try await host.drainFailedPreparation(requestID: request) { captured in
                try host.identifyFailedPreparation(requestID: request, source: captured, sessionID: session)
                let released = await host.releaseFailedPreparationAfterDrain(requestID: request, source: captured,
                    sessionID: session, waitForRelease: { false })
                XCTAssertFalse(released)
                throw SessionHost.OwnershipError.conflictingOwner
            }
            XCTFail("A timed-out drain must remain fenced")
        } catch { XCTAssertTrue(host.hasPreparingClaim(sessionID: session)) }
        try await host.drainFailedPreparation(requestID: request) { captured in
            let released = await host.releaseFailedPreparationAfterDrain(requestID: request, source: captured,
                sessionID: session, waitForRelease: { true })
            XCTAssertTrue(released)
        }
        XCTAssertNil(host.failedPreparation(requestID: request))
    }

    func testFailedReconnectAdmissionKeepsOldOwnerAndDraftForRetry() throws {
        for initiallyUnclaimed in [false, true] {
            let host = SessionHost()
            let id = "11111111-1111-4111-8111-111111111111"
            var catalogAvailable = true
            host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
            let old = MockConversationSource(sessionMode: .chat)
            old.model.activeSessionId = initiallyUnclaimed ? "" : id
            let previous = try host.retain(source: old, scope: .global, mode: .chat,
                sessionID: initiallyUnclaimed ? "" : id, workspace: "/global", draft: "keep")
            previous.draftSourceCodeUnits = [0xD800]
            host.select(previous)
            let replacement = MockConversationSource(sessionMode: .chat)
            replacement.model.activeSessionId = id
            XCTAssertTrue(host.beginReplacement(of: old, with: replacement,
                sessionID: previous.sessionID, scope: .global, mode: .chat))
            if initiallyUnclaimed {
                XCTAssertTrue(host.claimIdentity(id, for: replacement, scope: .global, mode: .chat))
            }
            catalogAvailable = false
            XCTAssertThrowsError(try host.replaceIdleSource(old, with: replacement, scope: .global,
                mode: .chat, sessionID: previous.sessionID, workspace: "/global", draft: "keep"))
            XCTAssertTrue(host.entry(for: old) === previous)
            XCTAssertEqual(host.selectedEntryID, previous.id)
            XCTAssertEqual(previous.draft, "keep")
            XCTAssertEqual(previous.draftSourceCodeUnits, [0xD800])
            host.endPreparingClaim(of: replacement)

            catalogAvailable = true
            let retry = MockConversationSource(sessionMode: .chat)
            retry.model.activeSessionId = id
            XCTAssertTrue(host.beginReplacement(of: old, with: retry,
                sessionID: previous.sessionID, scope: .global, mode: .chat))
            if initiallyUnclaimed { XCTAssertTrue(host.claimIdentity(id, for: retry, scope: .global, mode: .chat)) }
            let next = try host.replaceIdleSource(old, with: retry, scope: .global, mode: .chat,
                sessionID: previous.sessionID, workspace: "/global", draft: "keep")
            XCTAssertNil(host.entry(for: old))
            XCTAssertTrue(host.entry(for: retry) === next)
            XCTAssertTrue(next.voice === previous.voice)
            XCTAssertEqual(next.draft, "keep")
            XCTAssertEqual(next.draftSourceCodeUnits, [0xD800])
        }
    }

    func testReconnectCannotAdoptAnotherRetainedBlankOwner() throws {
        let host = SessionHost()
        let old = MockConversationSource(sessionMode: .chat)
        let other = MockConversationSource(sessionMode: .chat)
        let previous = try host.retain(source: old, scope: .global, mode: .chat,
                                       sessionID: "", workspace: "/global", draft: "first")
        let otherEntry = try host.retain(source: other, scope: .global, mode: .chat,
                                         sessionID: "", workspace: "/global", draft: "second")
        host.select(previous)
        XCTAssertThrowsError(try host.replaceIdleSource(old, with: other, scope: .global,
            mode: .chat, sessionID: "", workspace: "/global", draft: "first"))
        XCTAssertTrue(host.entry(for: old) === previous)
        XCTAssertTrue(host.entry(for: other) === otherEntry)
        XCTAssertEqual(host.selectedEntryID, previous.id)
        XCTAssertEqual(previous.draft, "first")
        XCTAssertEqual(otherEntry.draft, "second")
    }

    func testPersistedOwnerAdmissionRejectsLateIdentityBeforeItIsClaimed() throws {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let entry = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: "", workspace: "/global", draft: "keep")
        let id = "11111111-1111-4111-8111-111111111111"
        host.admitsSessionIdentity = { candidate, scope, mode in
            candidate != id || (scope == .project("owner") && mode == .chat)
        }
        XCTAssertFalse(host.claimIdentity(id, for: source, scope: .global, mode: .chat))
        XCTAssertTrue(entry.sessionID.isEmpty)
        XCTAssertEqual(entry.draft, "keep")
    }

    func testColdRestoreRetainsBlankOwnerWhenCatalogBecomesUnavailable() {
        let host = SessionHost()
        let original = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        original.model.activeSessionId = id
        var catalogAvailable = true
        host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
        host.admitsUnclaimedOwner = { catalogAvailable }
        XCTAssertTrue(host.admitsSessionIdentity?(id, .global, .chat) == true)
        catalogAvailable = false

        let placeholder = MockConversationSource(sessionMode: .chat)
        placeholder.model.activeSessionId = "synthetic-fixture-id"
        let result = host.retainInitial(source: original, scope: .global, mode: .chat,
                                        sessionID: id, workspace: "/old", draft: "saved draft") {
            (placeholder, "/global", "blank draft")
        }
        XCTAssertNotNil(result.rejection)
        XCTAssertEqual(host.entries.count, 1)
        XCTAssertTrue(result.entry.source === placeholder)
        XCTAssertEqual(result.entry.sessionID, "")
        XCTAssertEqual(result.entry.draft, "blank draft")
        XCTAssertNil(host.entry(for: original))
        XCTAssertFalse(host.allowsStartup(of: placeholder))
        XCTAssertFalse(host.claimIdentity(id, for: placeholder, scope: .global, mode: .chat))
        XCTAssertEqual(original.model.activeSessionId, id)
        catalogAvailable = true
        XCTAssertTrue(host.allowsStartup(of: placeholder))
    }

    func testExistingIdentityIsRecheckedAfterItsIndexedOwnerChanges() {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        var indexedScope = ConversationScope.global
        host.admitsUnclaimedOwner = { true }
        host.admitsSessionIdentity = { _, scope, _ in scope == indexedScope }
        XCTAssertTrue(host.allowsStartup(of: source, sessionID: id, scope: .global, mode: .chat))
        indexedScope = .project("other")
        XCTAssertFalse(host.allowsStartup(of: source, sessionID: id, scope: .global, mode: .chat))
        XCTAssertTrue(host.entries.isEmpty)
    }

    func testAdmittedOwnerKeepsItsTurnIfCatalogLaterBecomesUnavailable() async throws {
        let host = SessionHost()
        let source = MockConversationSource(sessionMode: .chat)
        let id = "11111111-1111-4111-8111-111111111111"
        source.model.activeSessionId = id
        var catalogAvailable = true
        host.admitsSessionIdentity = { _, _, _ in catalogAvailable }
        host.admitsUnclaimedOwner = { catalogAvailable }
        let owner = try host.retain(source: source, scope: .global, mode: .chat,
                                    sessionID: id, workspace: "/global", draft: "keep")
        let sentToken = await source.send("continue")
        XCTAssertNotNil(sentToken)
        catalogAvailable = false
        XCTAssertTrue(host.allowsStartup(of: source))
        var activityObserved = false
        host.onActivity = { _ in activityObserved = true }
        host.sync(owner)
        XCTAssertTrue(activityObserved)
        XCTAssertTrue(source.model.streaming)
        XCTAssertNil(source.model.error)
        let other = MockConversationSource(sessionMode: .chat)
        other.model.activeSessionId = "22222222-2222-4222-8222-222222222222"
        XCTAssertThrowsError(try host.retain(source: other, scope: .global, mode: .chat,
                                             sessionID: other.model.activeSessionId,
                                             workspace: "/global", draft: ""))
    }
}

#if canImport(harness_runtimeFFI)
@MainActor
final class DurableConversationTurnCollectionTests: XCTestCase {
    func testLateNativeStartupCannotAdoptAnotherRetainedOwner() throws {
        let host = SessionHost()
        let sessionID = "44444444-4444-4444-8444-444444444444"
        let first = MockConversationSource(sessionMode: .chat)
        first.model.activeSessionId = sessionID
        _ = try host.retain(source: first, scope: .global, mode: .chat,
                            sessionID: sessionID, workspace: "/global", draft: "keep")
        let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                                  projectCwd: "/p", sessionMode: .code, visionDelegationEnabled: false)
        let second = EngineConversationSource(config: config)
        _ = try host.retain(source: second, scope: .project("p"), mode: .code,
                            sessionID: "", workspace: "/p", draft: "other")
        second.applyForTesting(.sessionStarted(sessionId: sessionID, mode: .code))
        XCTAssertEqual(second.model.activeSessionId, "")
        XCTAssertFalse(second.isAvailableForPrompts)
        XCTAssertNotNil(second.model.error)
        XCTAssertTrue(try host.entryForClaim(sessionID: sessionID, scope: .global, mode: .chat)?.source === first)
    }

    func testUnresolvedCheckpointBlocksNextTurnForCanonicalOwner() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let firstID = UUID().uuidString
        let otherID = UUID().uuidString
        func config(_ session: String) -> EngineConfig {
            EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                         projectCwd: "/home/global", sessionMode: .code, visionDelegationEnabled: false,
                         initialSessionId: session)
        }
        let first = DurableConversationTurnClientStore(config: config(firstID.lowercased()))
        let other = DurableConversationTurnClientStore(config: config(otherID))
        try await first.begin(sessionID: "sess:\(firstID)", turnID: 1)
        try await other.begin(sessionID: otherID, turnID: 7)
        do {
            try await first.begin(sessionID: firstID.lowercased(), turnID: 2)
            XCTFail("A turn cannot replace an unresolved checkpoint")
        } catch DurableConversationRecoveryError.unresolvedOwner {
            // Expected: the old owner must be cleared before admitting turn 2.
        }
        let unresolved = try await first.load()
        let otherBefore = try await other.load()
        XCTAssertEqual(unresolved?.turnID, 1)
        XCTAssertEqual(otherBefore?.turnID, 7)

        try await first.clear(turnID: 1)
        try await first.begin(sessionID: firstID, turnID: 2)
        do {
            try await first.clear(turnID: 1)
            XCTFail("A delayed clear cannot retire a different turn")
        } catch DurableConversationRecoveryError.invalidStore {
            // The mismatched clear is fenced; the actual checkpoint remains.
        }
        let current = try await first.load()
        let otherAfter = try await other.load()
        XCTAssertEqual(current?.turnID, 2)
        XCTAssertEqual(otherAfter?.turnID, 7)
        try await first.clear(turnID: 2)
        let cleared = try await first.load()
        XCTAssertNil(cleared, "Clearing the current turn must not revive an older checkpoint")
    }

    func testExactOwnerAndTurnClearPreservesOtherOwners() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        func config(_ session: String, cwd: String = "/home/global") -> EngineConfig {
            EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                         projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: false,
                         initialSessionId: session)
        }
        let first = DurableConversationTurnClientStore(config: config("first"))
        let second = DurableConversationTurnClientStore(config: config("second"))
        try await first.begin(sessionID: "first", turnID: 7)
        try await second.begin(sessionID: "second", turnID: 7)
        try await first.clear(turnID: 7)
        try await first.begin(sessionID: "first", turnID: 8)
        let firstRecord = try await first.load()
        let secondRecord = try await second.load()
        let staleCatalog = try await DurableConversationTurnClientStore(
            config: config("second", cwd: "/home/old-index")).load()
        XCTAssertEqual(firstRecord?.turnID, 8)
        XCTAssertEqual(secondRecord?.turnID, 7)
        XCTAssertEqual(staleCatalog?.turnID, 7,
                       "A stale catalog cwd must not hide the UUID owner's checkpoint")
        try await first.clear(turnID: 8)
        let clearedFirst = try await first.load()
        let reopenedSecond = try await DurableConversationTurnClientStore(config: config("second")).load()
        XCTAssertNil(clearedFirst)
        XCTAssertEqual(reopenedSecond?.turnID, 7)
        try await second.clear(turnID: 7)
        let clearedSecond = try await second.load()
        XCTAssertNil(clearedSecond)
    }
}
#endif

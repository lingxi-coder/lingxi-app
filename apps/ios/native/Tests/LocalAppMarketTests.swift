import XCTest
import ZIPFoundation
@testable import LingxiCode

final class LocalAppMarketTests: XCTestCase {
    func testServiceEnvelopePreservesNullSuccessAndFailure() throws {
        let decoder = JSONDecoder()
        let success = try decoder.decode(LocalAppServiceEnvelope<LocalAppJSON>.self,
            from: Data(#"{"ok":true,"value":null}"#.utf8))
        XCTAssertEqual(try success.result(), .null)
        let missing = try decoder.decode(LocalAppServiceEnvelope<LocalAppJSON>.self,
            from: Data(#"{"ok":true}"#.utf8))
        XCTAssertThrowsError(try missing.result()) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "INVALID_RESPONSE")
        }
        let failure = try decoder.decode(LocalAppServiceEnvelope<UnifiedAppRecord>.self,
            from: Data(#"{"ok":false,"value":null,"error":{"code":"DENIED","message":"Denied"}}"#.utf8))
        XCTAssertThrowsError(try failure.result()) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "DENIED")
        }
    }

    private var manifest: String { #"{"schema":2,"name":"Fixture","summary":"Fixture","entry":{"type":"static","path":"index.html"},"targets":[{"platform":"ios","environment":"web"}],"package":{"files":["lingxi-app.json","index.html"],"dependencyLocks":[]}}"# }
    func testExternalBrowserOriginAndExactReadinessMarker() throws {
        let origin = "http://127.0.0.1:33001"
        let url = URL(string: origin + "/__run/test/index.html?photo=kept")!
        XCTAssertTrue(LocalAppExternalBrowserURL.matches(url, origin: origin))
        let requestID = "request + exact/&unicode-灵犀"
        let ready = try LocalAppAgentWakeURL.ready(url, origin: origin, requestID: requestID)
        let query = URLComponents(url: ready, resolvingAgainstBaseURL: false)?.queryItems
        XCTAssertEqual(query?.first(where: { $0.name == "photo" })?.value, "kept")
        XCTAssertEqual(query?.first(where: { $0.name == "__lingxi_ready" })?.value, requestID)
        for value in ["http://127.0.0.1:33002/", "http://localhost:33001/", "https://127.0.0.1:33001/", "http://user@127.0.0.1:33001/"] {
            XCTAssertFalse(LocalAppExternalBrowserURL.matches(URL(string: value)!, origin: origin))
        }
        XCTAssertThrowsError(try LocalAppAgentWakeURL.ready(url, origin: origin, requestID: "bad\nrequest"))
    }
    @MainActor
    func testAgentHostRuntimeGateMatchesTheSelectedStoreOrFullTarget() {
        let hasMobileLinuxTarget = UnifiedAppMarket.targets.contains { $0.environment == "mobile-linux" }
        XCTAssertEqual(LocalAppAgentModelHost.usesMobileLinuxRuntime, hasMobileLinuxTarget)
    }
    func testMobileAuthoringReferenceAndTaskWireShape() throws {
        let authoring = LocalAppAuthoring(
            sourceDirectory: "/home/user/.lingxi/.apps/app-source",
            workspaceId: "12345678-1234-4abc-8def-1234567890ab",
            projectRef: .mobileProject("12345678-1234-4abc-8def-1234567890ab"),
            authoringSessionId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            appId: "a12345678901234567890123456789012"
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(authoring)) as? [String: Any])
        XCTAssertEqual(object["workspaceId"] as? String, "12345678-1234-4abc-8def-1234567890ab")
        let project = try XCTUnwrap(object["projectRef"] as? [String: Any])
        XCTAssertEqual(project["kind"] as? String, "mobileProject")
        XCTAssertEqual(project["projectId"] as? String, "12345678-1234-4abc-8def-1234567890ab")
        let incomplete = try JSONDecoder().decode(LocalAppAuthoring.self, from: Data(#"{"sourceDirectory":"/home/user/.lingxi/.apps/legacy"}"#.utf8))
        XCTAssertNil(incomplete.workspaceId)
        XCTAssertNil(incomplete.projectRef)

        let response = try JSONDecoder().decode(LocalAppAgentTaskListResponse.self, from: Data(#"{"appId":"a12345678901234567890123456789012","revision":7,"tasks":[{"runId":"run-1","requestId":"opaque request","state":"running","sequence":3,"createdAt":1000,"updatedAt":1200,"model":{"providerId":"p","modelId":"m"},"limits":{"maxTurns":20,"timeoutMs":600000},"progress":{"completedModelTurns":2,"turnLimit":20}}]}"#.utf8))
        XCTAssertEqual(response.revision, 7)
        XCTAssertEqual(response.tasks.first?.requestId, "opaque request")
        XCTAssertEqual(response.tasks.first?.progress?.completedModelTurns, 2)
        XCTAssertEqual(response.tasks.first?.limits.timeoutMs, 600_000)
    }
    func testNewAppAuthoringUsesTrustedPendingRegistrationWithCompleteMobileReference() throws {
        let sourceDirectory = MobileUserStorage.directories().appsGuestPath + "/notes-abcdef12"
        let authoring = LocalAppAuthoring(
            sourceDirectory: sourceDirectory,
            workspaceId: "12345678-1234-4abc-8def-1234567890ab",
            projectRef: .mobileProject("12345678-1234-4abc-8def-1234567890ab"),
            authoringSessionId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        )

        let request = try LocalAppAuthoringHostRequest.registerPending(authoring)
        XCTAssertEqual(request["op"], .string("registerAuthoring"))
        let requestBytes = try JSONEncoder().encode(request)
        XCTAssertFalse(LocalAppAgentHostPolicy.requiresModelHost(String(decoding: requestBytes, as: UTF8.self)))
        XCTAssertTrue(LocalAppAgentHostPolicy.requiresModelHost(#"{"op":"startNative"}"#))
        XCTAssertTrue(LocalAppAgentHostPolicy.requiresModelHost("invalid JSON"))
        guard case .object(let fields)? = request["authoring"] else {
            return XCTFail("The trusted host request must carry the complete AuthoringRef.")
        }
        XCTAssertEqual(fields["workspaceId"], .string("12345678-1234-4abc-8def-1234567890ab"))
        XCTAssertEqual(fields["sourceDirectory"], .string(sourceDirectory))
        XCTAssertEqual(fields["authoringSessionId"], .string("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"))
        XCTAssertNil(fields["appId"], "Pending first-publish registration has no App ID yet.")
        guard case .object(let project)? = fields["projectRef"] else {
            return XCTFail("The AuthoringRef must identify the dedicated mobile Project.")
        }
        XCTAssertEqual(project["kind"], .string("mobileProject"))
        XCTAssertEqual(project["projectId"], .string("12345678-1234-4abc-8def-1234567890ab"))

        XCTAssertThrowsError(try LocalAppAuthoringHostRequest.registerPending(LocalAppAuthoring(
            sourceDirectory: authoring.sourceDirectory,
            workspaceId: authoring.workspaceId,
            projectRef: authoring.projectRef,
            authoringSessionId: authoring.authoringSessionId,
            appId: "a12345678901234567890123456789012"
        )))
        XCTAssertThrowsError(try LocalAppAuthoringHostRequest.registerPending(LocalAppAuthoring(
            sourceDirectory: authoring.sourceDirectory,
            workspaceId: authoring.workspaceId,
            projectRef: .mobileProject("different-project"),
            authoringSessionId: authoring.authoringSessionId
        )))
    }
    @MainActor
    func testAuthoringDirectoryUsesLinuxHomeNotDefaultWorkspaceRoot() throws {
        let paths = MobileUserStorage.directories()
        let home = URL(fileURLWithPath: paths.home).resolvingSymlinksInPath()
        XCTAssertEqual(try UnifiedLocalApps.hostSourceDirectory(paths.appsGuestPath + "/test").path,
                       home.appendingPathComponent(String(paths.appsGuestPath.dropFirst(paths.guestHome.count + 1)) + "/test").path)
        XCTAssertThrowsError(try UnifiedLocalApps.hostSourceDirectory(paths.guestHome + "/../../outside"))
        XCTAssertThrowsError(try UnifiedLocalApps.hostSourceDirectory("/root/private"))
    }
    func testServiceErrorsRemainExplicit() throws {
        let bytes = Data(#"{"ok":false,"error":{"code":"executionConsentRequired","message":"Approve this digest"}}"#.utf8)
        let envelope = try JSONDecoder().decode(LocalAppServiceEnvelope<[UnifiedAppRecord]>.self,from:bytes)
        XCTAssertThrowsError(try envelope.result()) { XCTAssertEqual(($0 as? LocalAppServiceFailure)?.code,"executionConsentRequired") }
        let invalid = try JSONDecoder().decode(LocalAppServiceEnvelope<[UnifiedAppRecord]>.self,from:Data(#"{"ok":true}"#.utf8))
        XCTAssertThrowsError(try invalid.result())
    }
    func testStaticAndExecutableManifestDistinction() throws {
        let basic = try JSONDecoder().decode(UnifiedAppManifest.self,from:Data(manifest.utf8))
        XCTAssertFalse(basic.executesCode)
        var value = try JSONSerialization.jsonObject(with:Data(manifest.utf8)) as! [String:Any]
        value["install"] = [["command":"npm ci"]]
        XCTAssertTrue(try JSONDecoder().decode(UnifiedAppManifest.self,from:JSONSerialization.data(withJSONObject:value)).executesCode)
    }
    func testPackageRejectsTraversalAliasesAndMalformedZip() throws {
        for path in ["../index.html","/index.html","a/../b","a\\b","a//b","a/%2e",".hidden","a/last.","a/last ","C:a"] { XCTAssertThrowsError(try MarketPackageFiles.safePath(path),path) }
        XCTAssertEqual(try MarketPackageFiles.safePath("assets/main.js"),"assets/main.js")
        XCTAssertEqual(try MarketPackageFiles.safePath("e\u{301}.html"),"é.html")
        XCTAssertThrowsError(try MarketPackageFiles.validateZIP(Data()))
    }
    func testSourceArchiveRequiresManifestAndPreservesOldSource() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let source = root.appendingPathComponent("source"), extracted = root.appendingPathComponent("extracted")
        try FileManager.default.createDirectory(at:source,withIntermediateDirectories:true)
        try FileManager.default.createDirectory(at:extracted,withIntermediateDirectories:true)
        try Data(manifest.utf8).write(to:source.appendingPathComponent("lingxi-app.json"))
        try Data("<!doctype html><html>fixture</html>".utf8).write(to:source.appendingPathComponent("index.html"))
        let zip = root.appendingPathComponent("source.zip")
        try FileManager.default.zipItem(at:source,to:zip,shouldKeepParent:false)
        try MarketPackageFiles.extract(zip,to:extracted)
        XCTAssertEqual(try Data(contentsOf:source.appendingPathComponent("index.html")),try Data(contentsOf:extracted.appendingPathComponent("index.html")))
        XCTAssertTrue(FileManager.default.fileExists(atPath:source.appendingPathComponent("lingxi-app.json").path))
    }
    @MainActor
    func testPublicationIdentitySurvivesRecreationAndCorruptionStopsMutations() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let first = try LocalAppPublicationReceipts(directory: directory)
        let key = try first.mutationKey("release:createVersion")
        let recreated = try LocalAppPublicationReceipts(directory: directory)
        XCTAssertEqual(try recreated.mutationKey("release:createVersion"), key)
        XCTAssertEqual(try first.fixedValue("listing:createBody", proposed: "original name"), "original name")
        XCTAssertEqual(try recreated.fixedValue("listing:createBody", proposed: "renamed in another release"), "original name")
        try recreated.set("listing", "market-id")
        XCTAssertEqual(try first.get("listing"), "market-id")
        try Data("invalid receipt".utf8).write(to: directory.appendingPathComponent("receipts.json"))
        XCTAssertThrowsError(try recreated.mutationKey("release:publish"))
        XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("receipts.json")), Data("invalid receipt".utf8))
    }
    @MainActor
    func testUnsubmittedArchiveRecoveryAndSubmittedArchiveProtection() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let receipts = try LocalAppPublicationReceipts(directory: directory)
        let archive = directory.appendingPathComponent("source.zip")
        try Data("orphan from failed receipt write".utf8).write(to: archive)
        let original = Data("reviewed archive".utf8)
        XCTAssertEqual(try receipts.preparedArchive(archive, releaseKey: "release", proposed: original), original)
        XCTAssertEqual(try receipts.preparedArchive(archive, releaseKey: "release", proposed: Data("different ZIP timestamp".utf8)), original)
        _ = try receipts.mutationKey("release:createVersion")
        try FileManager.default.removeItem(at: archive)
        XCTAssertThrowsError(try receipts.preparedArchive(archive, releaseKey: "release", proposed: original))
        XCTAssertFalse(FileManager.default.fileExists(atPath: archive.path))
    }
    @MainActor
    func testUnwritablePublicationReceiptDoesNotIssueAnIdentity() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let receipts = try LocalAppPublicationReceipts(directory: directory)
        // Replacing the directory with a file models a failed local commit.
        try FileManager.default.removeItem(at: directory)
        try Data().write(to: directory)
        XCTAssertThrowsError(try receipts.mutationKey("release:publish"))
    }
    func testDistributionSelectsPostTargetAndKeepsCatalogAnonymous() async throws {
        let config = URLSessionConfiguration.ephemeral; config.protocolClasses = [LocalAppMarketProtocol.self]
        let api = try MarketAPI(baseURL:URL(string:"https://fixture.test/api/v2")!,configuration:config)
        LocalAppMarketProtocol.response = "{\"data\":{\"schema\":2,\"appId\":\"a\",\"versionId\":\"v1\",\"sequence\":1,\"releaseDigest\":\"digest\",\"manifest\":\(manifest),\"files\":[],\"dependencies\":[],\"artifact\":{\"format\":\"zip\",\"filename\":\"source.zip\",\"sizeBytes\":2,\"sha256\":\"digest\",\"downloadUrl\":\"https://fixture.test/source.zip\"},\"executionConsentRequired\":false}}"
        let result = try await api.distribution("a",target:.init(platform:"ios",environment:"web",architecture:"aarch64",channels:["full"],capabilities:[]))
        XCTAssertEqual(result.versionId,"v1")
        let request = try XCTUnwrap(LocalAppMarketProtocol.lastRequest)
        XCTAssertEqual(request.httpMethod,"POST")
        XCTAssertEqual(request.url?.path,"/api/v2/apps/a/distribution")
        XCTAssertNil(request.value(forHTTPHeaderField:"Authorization"))
        XCTAssertThrowsError(try MarketAPI(baseURL:URL(string:"http://fixture.test/api/v2")!))
    }
}
private final class LocalAppMarketProtocol: URLProtocol, @unchecked Sendable {
    static var response = "{}"
    static var lastRequest: URLRequest?
    override class func canInit(with request:URLRequest) -> Bool { true }
    override class func canonicalRequest(for request:URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lastRequest = request
        client?.urlProtocol(self,didReceive:HTTPURLResponse(url:request.url!,statusCode:200,httpVersion:nil,headerFields:["Content-Type":"application/json"])!,cacheStoragePolicy:.notAllowed)
        client?.urlProtocol(self,didLoad:Data(Self.response.utf8)); client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

import XCTest
import WebKit
@testable import LingxiCode

final class LocalAppMarketTests: XCTestCase {
    func testPathValidationMatchesMarketAndNormalizesUnicode() throws {
        for path in ["../index.html", "/index.html", "a/../b", "a\\b", "a//b", "a/%2e", "a?b", ".hidden", "a/last.", "a/last ", "a\u{007f}", "a/#b", "C:a"] {
            XCTAssertThrowsError(try MarketRepository.safePath(path), path)
        }
        XCTAssertEqual(try MarketRepository.safePath("assets/main.js"), "assets/main.js")
        XCTAssertEqual(try MarketRepository.safePath("assets/"), "assets")
        XCTAssertEqual(try MarketRepository.safePath("e\u{301}.html"), "é.html")
    }
    func testZIPExtractionCRCAndAdversarialPaths() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let fixtures: [String: String] = [
            "valid": "UEsDBBQAAAAIAMy8TF2GphA2BwAAAAUAAAAKAAAAaW5kZXguaHRtbMtIzcnJBwBQSwMEFAAAAAgAzLxMXX/pUVISAAAAEAAAAA4AAABhc3NldHMvbWFpbi5qcyvPzEvJL9crSS0usS0pKk0FAFBLAQIUAxQAAAAIAMy8TF2GphA2BwAAAAUAAAAKAAAAAAAAAAAAAACAAQAAAABpbmRleC5odG1sUEsBAhQDFAAAAAgAzLxMXX/pUVISAAAAEAAAAA4AAAAAAAAAAAAAAIABLwAAAGFzc2V0cy9tYWluLmpzUEsFBgAAAAACAAIAdAAAAG0AAAAAAA==",
            "traversal": "UEsDBBQAAAAIAMy8TF2GphA2BwAAAAUAAAAKAAAAaW5kZXguaHRtbMtIzcnJBwBQSwMEFAAAAAgAzLxMXfs5K4IFAAAAAwAAAAcAAAAuLi9ldmlsS0pMAQBQSwECFAMUAAAACADMvExdhqYQNgcAAAAFAAAACgAAAAAAAAAAAAAAgAEAAAAAaW5kZXguaHRtbFBLAQIUAxQAAAAIAMy8TF37OSuCBQAAAAMAAAAHAAAAAAAAAAAAAACAAS8AAAAuLi9ldmlsUEsFBgAAAAACAAIAbQAAAFkAAAAAAA==",
            "collision": "UEsDBBQAAAAIAMy8TF2GphA2BwAAAAUAAAAKAAAAaW5kZXguaHRtbMtIzcnJBwBQSwMEFAAAAAgAzLxMXRA2n4wGAAAABAAAAAEAAABhS8vMSQUAUEsDBBQAAAAIAMy8TF0pVLMiBwAAAAUAAAADAAAAYS9iS87IzEkBAFBLAQIUAxQAAAAIAMy8TF2GphA2BwAAAAUAAAAKAAAAAAAAAAAAAACAAQAAAABpbmRleC5odG1sUEsBAhQDFAAAAAgAzLxMXRA2n4wGAAAABAAAAAEAAAAAAAAAAAAAAIABLwAAAGFQSwECFAMUAAAACADMvExdKVSzIgcAAAAFAAAAAwAAAAAAAAAAAAAAgAFUAAAAYS9iUEsFBgAAAAADAAMAmAAAAHwAAAAAAA==",
            "noindex": "UEsDBBQAAAAIAMy8TF2sKpPYBAAAAAIAAAAJAAAAbWFpbi5odG1sy8gEAFBLAQIUAxQAAAAIAMy8TF2sKpPYBAAAAAIAAAAJAAAAAAAAAAAAAACAAQAAAABtYWluLmh0bWxQSwUGAAAAAAEAAQA3AAAAKwAAAAAA"
        ]
        for (name, encoded) in fixtures {
            let archive = root.appendingPathComponent(name + ".zip")
            let destination = root.appendingPathComponent(name)
            try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
            try Data(base64Encoded: encoded)!.write(to: archive)
            if name == "valid" {
                try MarketRepository.extract(archive, to: destination)
                XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("index.html")), "hello")
            } else { XCTAssertThrowsError(try MarketRepository.extract(archive, to: destination)) }
        }
        var corrupt = Data(base64Encoded: fixtures["valid"]!)!
        // First central-directory CRC is at signature + 16.
        let marker = Data([0x50,0x4b,0x01,0x02])
        let central = try XCTUnwrap(corrupt.range(of: marker)?.lowerBound)
        corrupt[central + 16] ^= 0xff
        let archive = root.appendingPathComponent("corrupt.zip")
        try corrupt.write(to: archive)
        XCTAssertThrowsError(try MarketRepository.extract(archive, to: root.appendingPathComponent("corrupt")))
        var encrypted = Data(base64Encoded: fixtures["valid"]!)!
        encrypted[6] |= 1
        encrypted[central + 8] |= 1
        XCTAssertThrowsError(try MarketRepository.validateZIP(encrypted))
    }
    @MainActor
    func testAPIPlatformPaginationAnonymousAndStaleSearch() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        MarketMockProtocol.resetRequests()
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [MarketMockProtocol.self]
        let market = try LocalAppMarket(baseURL: URL(string: "https://fixture.test/api/v1")!, root: root, sessionConfiguration: config)
        let first = Task { await market.search("slow") }
        try await Task.sleep(nanoseconds: 50_000_000)
        await market.search("fresh")
        await first.value
        XCTAssertEqual(market.apps.map(\.id), ["fresh"])
        XCTAssertTrue(market.hasNextPage)
        await market.loadNextPage()
        XCTAssertEqual(market.apps.map(\.id), ["fresh", "next"])
        XCTAssertFalse(market.hasNextPage)
        await market.search("error")
        XCTAssertEqual(market.error, "Service unavailable")
        let requests = MarketMockProtocol.requests()
        XCTAssertTrue(requests.allSatisfy { $0.value(forHTTPHeaderField: "Authorization") == nil })
        for request in requests {
            let parts = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!.queryItems!
            XCTAssertTrue(parts.contains(URLQueryItem(name: "platform", value: "ios")))
            XCTAssertTrue(parts.contains(URLQueryItem(name: "limit", value: "24")))
            XCTAssertTrue(parts.contains(URLQueryItem(name: "sort", value: "name")))
        }
    }
    func testDownloadIntegritySizeAndHTMLStaging() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = try MarketRepository(root: root)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [MarketMockProtocol.self]
        let api = try MarketAPI(baseURL: URL(string: "https://fixture.test/api/v1")!, configuration: config)
        let artifact = MarketDistribution.Artifact(format: "html", filename: "app.html", sizeBytes: 5, sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824", downloadUrl: URL(string: "https://fixture.test/download")!)
        let distribution = MarketDistribution(appId: "sample", versionId: "v1", sequence: 1, artifact: artifact)
        let staged = try await repository.stage(distribution, api: api, progress: { _ in })
        XCTAssertEqual(try String(contentsOf: staged.appendingPathComponent("content/index.html")), "hello")
        let installed = try await repository.commit(distribution, staged: staged, source: api.baseURL)
        XCTAssertEqual(installed.versionId, "v1")
        for bytes in [4, 6] {
            let mismatch = MarketDistribution.Artifact(format: "html", filename: "app.html", sizeBytes: bytes, sha256: artifact.sha256, downloadUrl: artifact.downloadUrl)
            do { try await api.download(mismatch, to: root.appendingPathComponent("mismatch"), progress: { _ in }); XCTFail("must enforce actual size") }
            catch { XCTAssertEqual((error as? MarketFailure)?.code, "SIZE_MISMATCH") }
        }
        let bad = MarketDistribution.Artifact(format: "html", filename: "app.html", sizeBytes: 5, sha256: String(repeating: "0", count: 64), downloadUrl: artifact.downloadUrl)
        do { _ = try await repository.stage(.init(appId: "sample", versionId: "v2", sequence: 2, artifact: bad), api: api, progress: { _ in }); XCTFail("must reject hash mismatch") }
        catch { XCTAssertEqual((error as? MarketFailure)?.code, "HASH_MISMATCH") }
        let old = try await repository.installation("sample")
        XCTAssertEqual(old.versionId, "v1")
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: root.appendingPathComponent("staging").path).isEmpty)
    }
    func testRangesAndModuleMIME() {
        XCTAssertEqual(MarketHTTPServer.range("bytes=2-5", size: 10), 2...5)
        XCTAssertEqual(MarketHTTPServer.range("bytes=-3", size: 10), 7...9)
        XCTAssertEqual(MarketHTTPServer.range("bytes=8-", size: 10), 8...9)
        XCTAssertEqual(MarketHTTPServer.range("bytes=8-100", size: 10), 8...9)
        XCTAssertNil(MarketHTTPServer.range("bytes=10-", size: 10))
        XCTAssertNil(MarketHTTPServer.range("bytes=2-1", size: 10))
        XCTAssertNil(MarketHTTPServer.range("bytes=1-2,5-6", size: 10))
        XCTAssertEqual(MarketHTTPServer.mime("mjs"), "text/javascript; charset=utf-8")
        XCTAssertTrue(MarketHTTPServer.csp.contains("connect-src 'self' https: wss:"))
        XCTAssertTrue(MarketHTTPServer.csp.contains("worker-src 'none'"))
    }
    func testRejectsInsecureMarketAndMalformedZIP() {
        XCTAssertThrowsError(try MarketAPI(baseURL: URL(string: "http://example.com/api/v1")!))
        XCTAssertThrowsError(try MarketRepository.validateZIP(Data()))
        XCTAssertThrowsError(try MarketRepository.validateZIP(Data(repeating: 0, count: 50)))
    }
    func testAtomicRegistryRecoveryAndRemovalRetry() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = try MarketRepository(root: root)
        let stage = root.appendingPathComponent("staging/prepared/content")
        try FileManager.default.createDirectory(at: stage, withIntermediateDirectories: true)
        try Data("hello".utf8).write(to: stage.appendingPathComponent("index.html"))
        let distribution = MarketDistribution(appId: "test", versionId: "v1", sequence: 1, artifact: .init(format: "html", filename: "app.html", sizeBytes: 5, sha256: "", downloadUrl: URL(string: "https://example.com/download")!))
        let record = try await repository.commit(distribution, staged: stage.deletingLastPathComponent(), source: URL(string: "https://example.com/api/v1")!)
        try await repository.savePort("test", installationID: record.installationID, port: 23456)
        let restored = try MarketRepository(root: root)
        let current = try await restored.installation("test")
        XCTAssertEqual(current.port, 23456)
        XCTAssertEqual(current.installationID, record.installationID)
        let tombstone = try await restored.beginRemoval("test")
        do { _ = try await restored.installation("test"); XCTFail("removing record cannot open") } catch {}
        let location = await restored.resources(tombstone)
        try FileManager.default.removeItem(at: location)
        try await restored.finishRemoval(tombstone)
        let remaining = await restored.installations()
        XCTAssertTrue(remaining.isEmpty)
    }
    func testLoopbackServerRangeHeadAndTraversal() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try Data("0123456789".utf8).write(to: root.appendingPathComponent("index.html"))
        let server = MarketHTTPServer(root: root)
        let port = try await server.start(port: nil)
        defer { Task { await server.close() } }
        let url = URL(string: "http://127.0.0.1:\(port)/index.html")!
        var request = URLRequest(url: url)
        request.setValue("bytes=2-5", forHTTPHeaderField: "Range")
        let (data, response) = try await URLSession.shared.data(for: request)
        XCTAssertEqual(String(decoding: data, as: UTF8.self), "2345")
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 206)
        request.httpMethod = "HEAD"
        let (empty, head) = try await URLSession.shared.data(for: request)
        XCTAssertTrue(empty.isEmpty)
        XCTAssertEqual((head as? HTTPURLResponse)?.value(forHTTPHeaderField: "Content-Length"), "4")
        let (_, blocked) = try await URLSession.shared.data(from: URL(string: "http://127.0.0.1:\(port)/%2e%2e/secret")!)
        XCTAssertEqual((blocked as? HTTPURLResponse)?.statusCode, 404)
        let conflict = MarketHTTPServer(root: root)
        do { _ = try await conflict.start(port: port); XCTFail("must reject occupied port") } catch {}
        await conflict.close()
    }
    @MainActor
    func testPersistentWebKitStorageIsolationAndReopen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = try MarketRepository(root: root)
        var records: [MarketInstallation] = []
        for id in ["first", "second"] {
            let content = root.appendingPathComponent("staging/" + id + "/content")
            try FileManager.default.createDirectory(at: content, withIntermediateDirectories: true)
            try Data("<html><script type='module' src='./main.mjs'></script></html>".utf8).write(to: content.appendingPathComponent("index.html"))
            try Data("window.moduleLoaded = true".utf8).write(to: content.appendingPathComponent("main.mjs"))
            let distribution = MarketDistribution(appId: id, versionId: "v1", sequence: 1, artifact: .init(format: "html", filename: "index.html", sizeBytes: 1, sha256: "", downloadUrl: URL(string: "https://example.com")!))
            records.append(try await repository.commit(distribution, staged: content.deletingLastPathComponent(), source: URL(string: "https://example.com/api/v1")!))
        }
        let runtime = MarketRuntime(repository: repository)
        do {
          do {

            var first: WKWebView? = try await runtime.open("first")
            try await waitForModule(first!)
            _ = try await first!.evaluateJavaScript("localStorage.setItem('marker', 'first')")
            let script = """
            const db = await new Promise((resolve,reject) => {
              const request = indexedDB.open('market-test',1);
              request.onupgradeneeded = () => request.result.createObjectStore('values');
              request.onsuccess = () => resolve(request.result);
              request.onerror = () => reject(request.error);
            });
            await new Promise((resolve,reject) => {
              const tx = db.transaction('values','readwrite');
              tx.objectStore('values').put('persisted','key');
              tx.oncomplete = resolve; tx.onerror = () => reject(tx.error);
            });
            db.close(); return true;
            """
            _ = try await first!.callAsyncJavaScript(script, arguments: [:], in: nil, contentWorld: .page)

            var second: WKWebView? = try await runtime.open("second")
            try await waitForModule(second!)
            let missing = try await second!.evaluateJavaScript("localStorage.getItem('marker')")
            XCTAssertTrue(missing is NSNull)
            XCTAssertNotEqual(first!.configuration.websiteDataStore.identifier, second!.configuration.websiteDataStore.identifier)
            await runtime.close("first")

            var reopened: WKWebView? = try await runtime.open("first")
            try await waitForModule(reopened!)
            let restored = try await reopened!.evaluateJavaScript("localStorage.getItem('marker')")
            XCTAssertEqual(restored as? String, "first")
            let read = """
            const db = await new Promise((resolve,reject) => {
              const r = indexedDB.open('market-test',1); r.onsuccess=()=>resolve(r.result); r.onerror=()=>reject(r.error);
            });
            const value = await new Promise((resolve,reject) => {
              const r = db.transaction('values').objectStore('values').get('key'); r.onsuccess=()=>resolve(r.result); r.onerror=()=>reject(r.error);
            }); db.close(); return value;
            """

            let persisted = try await reopened!.callAsyncJavaScript(read, arguments: [:], in: nil, contentWorld: .page)
            XCTAssertEqual(persisted as? String, "persisted")
            await runtime.close("first")
            await runtime.close("second")
            first = nil; second = nil; reopened = nil
          }
            for record in records { try await runtime.clear(record) }
        } catch {
            for record in records { try? await runtime.clear(record) }
            throw error
        }
    }
    @MainActor
    private func waitForModule(_ view: WKWebView) async throws {
        for _ in 0..<200 {
            if let ready = try? await view.evaluateJavaScript("window.moduleLoaded === true"), ready as? Bool == true { return }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw MarketFailure("MODULE_LOAD_TIMEOUT")
    }

}

private final class MarketMockProtocol: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    private static var captured: [URLRequest] = []
    static func resetRequests() { lock.lock(); captured = []; lock.unlock() }
    static func requests() -> [URLRequest] { lock.lock(); defer { lock.unlock() }; return captured }
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host == "fixture.test" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.lock(); Self.captured.append(request); Self.lock.unlock()
        let items = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!.queryItems ?? []
        let q = items.first(where: { $0.name == "q" })?.value ?? ""
        let paged = items.contains(where: { $0.name == "cursor" })
        let id = paged ? "next" : q
        let object: [String: Any]
        let status: Int
        if q == "error" { status = 503; object = ["error": ["code": "SERVICE_UNAVAILABLE", "message": "Service unavailable"]] }
        else {
            status = 200
            let app: [String: Any] = ["id": id, "name": id, "summary": "test", "category": "tools", "platforms": ["ios"], "currentVersion": NSNull(), "coverUrl": NSNull()]
            object = ["data": paged ? [app, app] : [app], "pagination": ["nextCursor": paged ? NSNull() as Any : "cursor" as Any]]
        }
        let payload = request.url?.lastPathComponent == "download" ? Data("hello".utf8) : (try! JSONSerialization.data(withJSONObject: object))
        DispatchQueue.global().asyncAfter(deadline: .now() + (q == "slow" ? 0.3 : 0)) { [self] in
            client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: payload)
            client?.urlProtocolDidFinishLoading(self)
        }
    }
    override func stopLoading() {}
}

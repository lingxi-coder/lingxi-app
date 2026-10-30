import Foundation

struct MarketVersion: Codable, Equatable, Identifiable, Sendable {
    let id: String; let appId: String; let sequence: Int
    let format: String; let filename: String; let sizeBytes: Int?; let sha256: String?; let notes: String
    let manifest: UnifiedAppManifest?; let releaseDigest: String?
}
struct MarketApp: Codable, Equatable, Identifiable, Sendable {
    let id: String; let name: String; let summary: String; let category: String
    let platforms: [String]; let coverUrl: String?; let currentVersion: MarketVersion?
}
struct MarketDistribution: Codable, Sendable {
    struct File: Codable, Sendable { let path: String; let sizeBytes: Int; let sha256: String; let executable: Bool }
    struct Artifact: Codable, Sendable { let format: String; let filename: String; let sizeBytes: Int; let sha256: String; let downloadUrl: URL }
    let schema: Int; let appId: String; let versionId: String; let sequence: Int; let releaseDigest: String
    let manifest: UnifiedAppManifest; let files: [File]; let dependencies: [File]; let artifact: Artifact
    let executionConsentRequired: Bool
}
struct MarketUpdate: Codable, Equatable, Sendable {
    let appId: String; let status: String; let currentVersionId: String?; let currentSequence: Int?
}
struct MarketFailure: Error, LocalizedError, Sendable {
    let code: String; let message: String
    var errorDescription: String? { message }
    init(_ code: String, _ message: String? = nil) { self.code = code; self.message = message ?? code }
}
private struct MarketEnvelope<T: Decodable>: Decodable { let data: T }
private struct MarketErrorEnvelope: Decodable { let error: Detail; struct Detail: Decodable { let code: String; let message: String } }

/// Anonymous catalog/distribution calls and explicitly authenticated publishing.
final class MarketAPI: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    let baseURL: URL
    private let configuration: URLSessionConfiguration
    private lazy var session: URLSession = {
        configuration.httpAdditionalHeaders = nil; configuration.httpCookieStorage = nil; configuration.urlCredentialStorage = nil
        return URLSession(configuration: configuration, delegate: self, delegateQueue: nil)
    }()
    init(baseURL: URL, configuration: URLSessionConfiguration = .ephemeral) throws {
        guard baseURL.scheme == "https", baseURL.host != nil, baseURL.user == nil, baseURL.password == nil else { throw MarketFailure("HTTPS_REQUIRED") }
        self.baseURL = baseURL; self.configuration = configuration
    }
    var browseURL: URL? {
        guard var parts = URLComponents(url: baseURL, resolvingAgainstBaseURL: false) else { return nil }
        parts.path = "/"; parts.queryItems = [URLQueryItem(name: "platform", value: "ios")]; parts.fragment = nil
        return parts.url
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        // Authenticated writes never forward credentials across an origin.
        let authenticated = task.originalRequest?.value(forHTTPHeaderField: "Authorization") != nil
        let sameOrigin = request.url?.host == baseURL.host && request.url?.port == baseURL.port
        completionHandler(request.url?.scheme == "https" && (!authenticated || sameOrigin) ? request : nil)
    }
    func capabilities() async throws {
        struct Value: Decodable { let apiVersion: String }
        let envelope: MarketEnvelope<Value> = try await request("capabilities")
        guard envelope.data.apiVersion == "v2" else { throw MarketFailure("UNSUPPORTED_MARKET_API") }
    }
    func detail(_ id: String) async throws -> MarketApp { let r: MarketEnvelope<MarketApp> = try await request("apps/\(try identifier(id))"); return r.data }
    func distribution(_ id: String, target: UnifiedAppManifest.Target, versionID: String? = nil) async throws -> MarketDistribution {
        var path = "apps/\(try identifier(id))"
        if let versionID { path += "/versions/\(try identifier(versionID))" }; path += "/distribution"
        let r: MarketEnvelope<MarketDistribution> = try await request(path, body: ["target": try .encode(target)])
        guard r.data.schema == 2, r.data.appId == id, versionID == nil || r.data.versionId == versionID else { throw MarketFailure("DISTRIBUTION_ID_MISMATCH") }
        guard r.data.manifest.targets.contains(where: { $0.platform == target.platform && $0.environment == target.environment &&
            ($0.channels?.isEmpty != false || !Set($0.channels ?? []).isDisjoint(with: target.channels ?? [])) }) else {
            throw MarketFailure("NO_COMPATIBLE_RELEASE", "This release does not support this device's platform and runtime.")
        }
        return r.data
    }
    func publish(_ path: String, body: [String: LocalAppJSON], bearer: String, idempotencyKey: String? = nil) async throws -> LocalAppJSON {
        let r: MarketEnvelope<LocalAppJSON> = try await request(path, body: body, bearer: bearer, idempotencyKey: idempotencyKey); return r.data
    }
    func ownedVersion(_ id: String, bearer: String) async throws -> LocalAppJSON {
        let value: MarketEnvelope<LocalAppJSON> = try await request("versions/\(try identifier(id))", bearer: bearer)
        return value.data
    }
    func upload(versionID: String, artifact: Data, bearer: String) async throws {
        var request = URLRequest(url: baseURL.appendingPathComponent("versions/\(try identifier(versionID))/artifact"))
        request.httpMethod = "PUT"; request.httpBody = artifact; request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        let (data,response) = try await session.data(for: request); try Self.check(response,data:data)
    }
    private func identifier(_ id: String) throws -> String {
        guard id.range(of: "^[A-Za-z0-9_-]{1,128}$", options: .regularExpression) != nil else { throw MarketFailure("INVALID_ID") }; return id
    }
    private func request<T: Decodable>(_ path: String, query: [URLQueryItem] = [], body: [String: LocalAppJSON]? = nil, bearer: String? = nil, idempotencyKey: String? = nil) async throws -> T {
        var parts = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
        parts.queryItems = query.isEmpty ? nil : query
        var request = URLRequest(url: parts.url!)
        if let body { request.setValue(idempotencyKey ?? UUID().uuidString, forHTTPHeaderField: "Idempotency-Key"); request.httpMethod = "POST"; request.httpBody = try JSONEncoder().encode(body); request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        if let bearer { request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization") }
        let (data,response) = try await session.data(for: request); try Self.check(response,data:data)
        return try JSONDecoder().decode(T.self, from:data)
    }
    static func check(_ response: URLResponse, data: Data = Data()) throws {
        guard let http = response as? HTTPURLResponse, response.url?.scheme == "https" else { throw MarketFailure("HTTPS_REQUIRED") }
        guard (200..<300).contains(http.statusCode) else {
            if let error = try? JSONDecoder().decode(MarketErrorEnvelope.self, from:data) { throw MarketFailure(error.error.code,error.error.message) }
            throw MarketFailure("HTTP_\(http.statusCode)")
        }
    }
    func download(_ artifact: MarketDistribution.Artifact, to destination: URL, progress: @escaping @Sendable (Double) async -> Void) async throws {
        guard artifact.downloadUrl.scheme == "https", artifact.sizeBytes > 0, artifact.sizeBytes <= 25 * 1024 * 1024 else { throw MarketFailure("INVALID_ARTIFACT") }
        let (bytes,response) = try await session.bytes(from: artifact.downloadUrl); try Self.check(response)
        if response.expectedContentLength > 25 * 1024 * 1024 { throw MarketFailure("SIZE_LIMIT") }
        FileManager.default.createFile(atPath: destination.path, contents:nil)
        let file = try FileHandle(forWritingTo:destination); defer { try? file.close() }
        var buffer = Data(); var total = 0
        for try await byte in bytes {
            try Task.checkCancellation(); buffer.append(byte); total += 1
            guard total <= artifact.sizeBytes else { throw MarketFailure("SIZE_MISMATCH") }
            if buffer.count >= 65536 { try file.write(contentsOf:buffer); buffer.removeAll(keepingCapacity:true); await progress(Double(total)/Double(artifact.sizeBytes)) }
        }
        try file.write(contentsOf:buffer)
        guard total == artifact.sizeBytes else { throw MarketFailure("SIZE_MISMATCH") }; await progress(1)
    }
}

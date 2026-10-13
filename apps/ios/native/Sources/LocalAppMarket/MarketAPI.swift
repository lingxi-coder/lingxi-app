import Foundation

struct MarketVersion: Codable, Equatable, Identifiable {
    let id: String
    let appId: String
    let sequence: Int
    let format: String
    let filename: String
    let sizeBytes: Int?
    let sha256: String?
    let notes: String
}
struct MarketApp: Codable, Equatable, Identifiable {
    let id: String
    let name: String
    let summary: String
    let category: String
    let platforms: [String]
    let coverUrl: String?
    let currentVersion: MarketVersion?
}
struct MarketPage<T: Decodable>: Decodable {
    let data: [T]
    let pagination: Pagination
    struct Pagination: Decodable { let nextCursor: String? }
}
struct MarketDistribution: Codable {
    let appId: String
    let versionId: String
    let sequence: Int
    let artifact: Artifact
    struct Artifact: Codable {
        let format: String
        let filename: String
        let sizeBytes: Int
        let sha256: String
        let downloadUrl: URL
    }
}
struct MarketUpdate: Codable, Equatable {
    let appId: String
    let status: Status
    let currentVersionId: String?
    let currentSequence: Int?
    enum Status: String, Codable { case unavailable, unknown_version, up_to_date, update_available }
}
struct MarketFailure: Error, LocalizedError {
    let code: String
    let message: String
    var errorDescription: String? { message }
    init(_ code: String, _ message: String? = nil) { self.code = code; self.message = message ?? code }
}
private struct MarketEnvelope<T: Decodable>: Decodable { let data: T }
private struct MarketErrorEnvelope: Decodable {
    let error: Detail
    struct Detail: Decodable { let code: String; let message: String }
}
/// All public market requests are anonymous. Inject an HTTPS /api/v1 base URL.
final class MarketAPI: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    let baseURL: URL
    private let configuration: URLSessionConfiguration
    private lazy var session: URLSession = {
        let config = configuration
        config.httpAdditionalHeaders = nil
        config.httpCookieStorage = nil
        config.urlCredentialStorage = nil
        return URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }()
    init(baseURL: URL, configuration: URLSessionConfiguration = .ephemeral) throws {
        guard baseURL.scheme == "https", baseURL.host != nil else { throw MarketFailure("HTTPS_REQUIRED") }
        self.baseURL = baseURL
        self.configuration = configuration
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(request.url?.scheme == "https" ? request : nil)
    }
    func catalog(query: String = "", cursor: String? = nil) async throws -> MarketPage<MarketApp> {
        var p = [URLQueryItem(name: "platform", value: "ios"), URLQueryItem(name: "limit", value: "24"), URLQueryItem(name: "sort", value: query.isEmpty ? "updated" : "name")]
        if !query.isEmpty { p.append(URLQueryItem(name: "q", value: query)) }
        if let cursor { p.append(URLQueryItem(name: "cursor", value: cursor)) }
        return try await request("apps", query: p)
    }
    func detail(_ appId: String) async throws -> MarketApp { try await value("apps/\(try identifier(appId))") }
    func versions(_ appId: String, cursor: String? = nil) async throws -> MarketPage<MarketVersion> {
        var p = [URLQueryItem(name: "limit", value: "24")]
        if let cursor { p.append(URLQueryItem(name: "cursor", value: cursor)) }
        return try await request("apps/\(try identifier(appId))/versions", query: p)
    }
    func distribution(_ appId: String, _ versionId: String) async throws -> MarketDistribution {
        let d: MarketDistribution = try await value("apps/\(try identifier(appId))/versions/\(try identifier(versionId))/distribution")
        guard d.appId == appId, d.versionId == versionId else { throw MarketFailure("DISTRIBUTION_ID_MISMATCH") }
        return d
    }
    func updates(_ installed: [MarketInstallation]) async throws -> [MarketUpdate] {
        var results: [MarketUpdate] = []
        for start in stride(from: 0, to: installed.count, by: 100) {
            let apps = installed[start..<min(start + 100, installed.count)].map { ["appId": $0.appId, "installedVersionId": $0.versionId] }
            let response: MarketEnvelope<[MarketUpdate]> = try await request("updates/check", body: JSONSerialization.data(withJSONObject: ["apps": apps]))
            results += response.data
        }
        return results
    }
    private func identifier(_ value: String) throws -> String {
        guard value.range(of: "^[A-Za-z0-9_-]{1,128}$", options: .regularExpression) != nil else { throw MarketFailure("INVALID_ID") }
        return value
    }
    private func value<T: Decodable>(_ path: String) async throws -> T {
        let response: MarketEnvelope<T> = try await request(path)
        return response.data
    }
    private func request<T: Decodable>(_ path: String, query: [URLQueryItem] = [], body: Data? = nil) async throws -> T {
        var components = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false)!
        components.queryItems = query.isEmpty ? nil : query
        var request = URLRequest(url: components.url!)
        if let body { request.httpMethod = "POST"; request.httpBody = body; request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        let (data, response) = try await session.data(for: request)
        try Self.check(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }
    static func check(_ response: URLResponse, data: Data = Data()) throws {
        guard let http = response as? HTTPURLResponse, response.url?.scheme == "https" else { throw MarketFailure("HTTPS_REQUIRED") }
        guard (200..<300).contains(http.statusCode) else {
            if let error = try? JSONDecoder().decode(MarketErrorEnvelope.self, from: data) { throw MarketFailure(error.error.code, error.error.message) }
            throw MarketFailure("HTTP_\(http.statusCode)")
        }
    }
    func download(_ artifact: MarketDistribution.Artifact, to destination: URL, progress: @escaping @Sendable (Double) async -> Void) async throws {
        guard artifact.downloadUrl.scheme == "https", artifact.sizeBytes > 0, artifact.sizeBytes <= 25 * 1024 * 1024 else { throw MarketFailure("INVALID_ARTIFACT") }
        let (bytes, response) = try await session.bytes(from: artifact.downloadUrl)
        try Self.check(response)
        if response.expectedContentLength > 25 * 1024 * 1024 { throw MarketFailure("SIZE_LIMIT") }
        FileManager.default.createFile(atPath: destination.path, contents: nil)
        let file = try FileHandle(forWritingTo: destination)
        defer { try? file.close() }
        var buffer = Data(); var total = 0
        for try await byte in bytes {
            try Task.checkCancellation()
            buffer.append(byte); total += 1
            if total > artifact.sizeBytes || total > 25 * 1024 * 1024 { throw MarketFailure("SIZE_MISMATCH") }
            if buffer.count >= 65536 { try file.write(contentsOf: buffer); buffer.removeAll(keepingCapacity: true); await progress(Double(total) / Double(artifact.sizeBytes)) }
        }
        try file.write(contentsOf: buffer)
        guard total == artifact.sizeBytes else { throw MarketFailure("SIZE_MISMATCH") }
        await progress(1)
    }
}

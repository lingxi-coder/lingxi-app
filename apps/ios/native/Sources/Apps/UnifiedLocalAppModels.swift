import Foundation

/// Values crossing the standalone service boundary. They never contain engine
/// handles, account credentials, or a currently selected conversation.
indirect enum LocalAppJSON: Codable, Equatable, Sendable {
    case object([String: LocalAppJSON]), array([LocalAppJSON]), string(String), number(Double), bool(Bool), null
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode([String: LocalAppJSON].self) { self = .object(v) }
        else { self = .array(try c.decode([LocalAppJSON].self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .object(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .bool(let v): try c.encode(v)
        case .null: try c.encodeNil()
        }
    }
    static func encode<T: Encodable>(_ value: T) throws -> Self {
        try JSONDecoder().decode(Self.self, from: JSONEncoder().encode(value))
    }
    var text: String? { if case .string(let value) = self { return value }; return nil }
}

struct LocalAppAuthoring: Codable, Equatable, Sendable {
    var sourceDirectory: String
    var sourceSessionId: String?
    var projectId: String?
}
struct LocalAppMarketOrigin: Codable, Equatable, Sendable {
    let appId: String
    let releaseId: String
    let digest: String
    var remixedFrom: String?
}
struct UnifiedAppRecord: Codable, Equatable, Identifiable, Sendable {
    var id: String { appId }
    let appId: String
    let name: String
    let summary: String
    let currentRevision: UInt64
    let latestRevision: UInt64
    let authoring: LocalAppAuthoring?
    let marketOrigin: LocalAppMarketOrigin?
    let originPort: UInt16
    let dataSchemaVersion: UInt32
    let executionGrants: [String]
    let createdAtMs: UInt64
    let updatedAtMs: UInt64
    let removed: Bool
}
struct UnifiedAppManifest: Codable, Equatable, Sendable {
    struct Entry: Codable, Equatable, Sendable { let type: String; var path: String?; var process: String? }
    struct Target: Codable, Equatable, Sendable {
        var platform: String
        var environment: String
        var architecture: String?
        var runtimeVersion: String?
        var runtimeAbi: String?
        var channels: [String]?
        var capabilities: [String]?
    }
    struct Remix: Codable, Equatable, Sendable { let allowed: Bool; let license: String? }
    let schema: Int
    let name: String
    let summary: String?
    let entry: Entry
    let install: [LocalAppJSON]?
    let build: [LocalAppJSON]?
    let processes: [LocalAppJSON]?
    let capabilities: [String]?
    let targets: [Target]
    let dataSchema: LocalAppJSON?
    let remix: Remix?
    let package: LocalAppJSON?
    let agent: LocalAppJSON?
    var executesCode: Bool { (capabilities ?? []).contains("process") || entry.type == "server" || !(install ?? []).isEmpty || !(build ?? []).isEmpty || !(processes ?? []).isEmpty }
}
struct UnifiedAppRelease: Codable, Equatable, Identifiable, Sendable {
    struct File: Codable, Equatable, Sendable { let path: String; let sha256: String; let bytes: UInt64; let executable: Bool }
    var id: UInt64 { revision }
    let appId: String
    let revision: UInt64
    let digest: String
    let manifest: UnifiedAppManifest
    let files: [File]
    let createdAtMs: UInt64
    let marketOrigin: LocalAppMarketOrigin?
}
struct UnifiedAppRun: Codable, Equatable, Identifiable, Sendable {
    struct Service: Codable, Equatable, Sendable { let name: String; let port: UInt16 }
    var id: String { runId }
    let appId: String
    let runId: String
    let revision: UInt64
    let releaseDigest: String
    let phase: String
    let services: [Service]
    let error: String?
    let startedAtMs: UInt64
    let updatedAtMs: UInt64
    var isLive: Bool { ["preparing", "starting", "running", "stopping"].contains(phase) }
}
struct LocalAppLaunch: Decodable, Sendable { let run: UnifiedAppRun; let url: URL; let origin: String }
struct LocalAppRuntimeStatus: Decodable, Sendable { let run: UnifiedAppRun?; let preparingRun: UnifiedAppRun? }
struct LocalAppPublishResult: Decodable, Sendable { let record: UnifiedAppRecord; let release: UnifiedAppRelease }
struct LocalAppAuthoringResolution: Decodable, Sendable { let action: String; let sourceDirectory: String; let sessionId: String? }
struct LocalAppLegacyArchive: Decodable, Identifiable, Sendable { var id: String { path }; let path: String; let kind: String; let status: String }
struct LocalAppLayout: Decodable, Sendable { let sourcePath: String; let dataPath: String; let releasesPath: String; let runsPath: String }
struct LocalAppServiceFailure: Error, LocalizedError, Codable, Sendable {
    let code: String
    let message: String
    var errorDescription: String? { "\(message) (\(code))" }
    init(_ code: String, _ message: String) { self.code = code; self.message = message }
}
struct LocalAppServiceEnvelope<T: Decodable>: Decodable {
    let ok: Bool
    let value: T?
    let error: LocalAppServiceFailure?
    func result() throws -> T {
        guard ok, let value else { throw error ?? LocalAppServiceFailure("INVALID_RESPONSE", "The app service returned an invalid response.") }
        return value
    }
}

struct LocalAppLegacyEntry: Decodable, Identifiable, Sendable { var id: String { legacyId }; let legacyId: String; let name: String; let summary: String; let revision: UInt64; let sourcePath: String; let status: String }

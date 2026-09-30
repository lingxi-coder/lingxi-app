import XCTest
@testable import LingxiCode

@MainActor
final class PluginSecretRepositoryTests: XCTestCase {
    /// Golden shape from protocol/src/secret.rs serde derives and
    /// SecretKind::PluginSecret::as_dto(): kind is itself a JSON STRING.
    func testEnvelopeMatchesRustSecureStorageDataGolden() throws {
        let data = try PluginSecretEnvelope.encode(secret: "sk-x", plugin: "weather@acme", key: "API_KEY",
                                                   createdAt: Date(timeIntervalSince1970: 1_700_000_000.125))
        let golden = #"{"bytes":[115,107,45,120],"metadata":{"created_at":{"secs_since_epoch":1700000000,"nanos_since_epoch":125000000},"last_accessed":null,"kind":"{\"PluginSecret\":{\"plugin\":\"weather@acme\",\"key\":\"API_KEY\"}}"}}"#
        var actual = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        var expected = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(golden.utf8)) as? [String: Any])
        var actualMetadata = try XCTUnwrap(actual["metadata"] as? [String: Any])
        var expectedMetadata = try XCTUnwrap(expected["metadata"] as? [String: Any])
        let actualKind = try XCTUnwrap(actualMetadata.removeValue(forKey: "kind") as? String)
        let expectedKind = try XCTUnwrap(expectedMetadata.removeValue(forKey: "kind") as? String)
        XCTAssertEqual(actualKind, expectedKind, "SecretKindDto is an opaque Rust string, including field order")
        actual["metadata"] = actualMetadata
        expected["metadata"] = expectedMetadata
        XCTAssertEqual(actual as NSDictionary, expected as NSDictionary)
        XCTAssertEqual(try PluginSecretEnvelope.account(plugin: "weather@acme", key: "API_KEY"), "plugin-secret-weather@acme/API_KEY")
    }

    func testNonASCIISecretUsesUTF8ByteArray() throws {
        let data = try PluginSecretEnvelope.encode(secret: "🔐", plugin: "p", key: "key", createdAt: Date(timeIntervalSince1970: 0))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["bytes"] as? [Int], [240, 159, 148, 144])
    }

    func testSaveDeleteAndStatusNeverRetrieveSecret() async {
        let storage = PluginSecretTestStorage()
        let repository = PluginSecretRepository(storage: storage)
        await repository.refresh()
        XCTAssertEqual(repository.isConfigured(plugin: "p", key: "token"), false)
        await repository.save(plugin: "p", key: "token", secret: "test-only")
        XCTAssertEqual(repository.isConfigured(plugin: "p", key: "token"), true)
        XCTAssertTrue(repository.needsReconnect)
        XCTAssertNil(repository.errorMessage)
        await repository.delete(plugin: "p", key: "token")
        XCTAssertEqual(repository.isConfigured(plugin: "p", key: "token"), false)
        let retrieves = await storage.retrieveCount
        XCTAssertEqual(retrieves, 0)
        let services = await storage.services
        XCTAssertEqual(services, ["lingxi"])
    }

    func testFailedWriteIsNotReportedAsSaved() async {
        let storage = PluginSecretTestStorage(failWrites: true)
        let repository = PluginSecretRepository(storage: storage)
        await repository.save(plugin: "p", key: "token", secret: "test-only")
        XCTAssertNotNil(repository.errorMessage)
        XCTAssertNil(repository.message)
        XCTAssertFalse(repository.needsReconnect)
        XCTAssertNil(repository.isConfigured(plugin: "p", key: "token"))
    }

    func testEnvelopeRejectsAmbiguousNamespaceWithoutWriting() throws {
        XCTAssertThrowsError(try PluginSecretEnvelope.account(plugin: "p/other", key: "key"))
        XCTAssertThrowsError(try PluginSecretEnvelope.account(plugin: "p", key: ""))
    }

    func testNativeKeychainPresenceRoundtrip() async throws {
        let storage = SecureStorageImpl()
        let plugin = "lingxi-settings-test-\(UUID().uuidString)"
        let repository = PluginSecretRepository(storage: storage)
        await repository.save(plugin: plugin, key: "TOKEN", secret: "test-only")
        XCTAssertNil(repository.errorMessage)
        XCTAssertEqual(repository.isConfigured(plugin: plugin, key: "TOKEN"), true)
        await repository.delete(plugin: plugin, key: "TOKEN")
        XCTAssertNil(repository.errorMessage)
        XCTAssertEqual(repository.isConfigured(plugin: plugin, key: "TOKEN"), false)
    }
}

private actor PluginSecretTestStorage: IosSecureStorage {
    private var entries: [String: Data] = [:]
    private let failWrites: Bool
    private(set) var retrieveCount = 0
    private(set) var services: Set<String> = []
    init(failWrites: Bool = false) { self.failWrites = failWrites }
    func store(service: String, account: String, blob: Data) async throws {
        services.insert(service)
        if failWrites { throw NSError(domain: "test", code: 1) }
        entries[account] = blob
    }
    func retrieve(service: String, account: String) async throws -> Data? {
        retrieveCount += 1
        return entries[account]
    }
    func delete(service: String, account: String) async throws { services.insert(service); entries.removeValue(forKey: account) }
    func list(service: String) async throws -> [String] { services.insert(service); return Array(entries.keys) }
}

import Foundation
import XCTest

@testable import LingxiCode

/// One provider reached several ways: the editing rules, the validation rules,
/// and what those turn into in the provider settings the engine is launched
/// with. The engine-side desugaring is covered by `connection_group_test.rs`;
/// what is proved here is that the iOS store can express it at all, and that a
/// provider carrying connections survives the emit path.
@MainActor
final class ProviderConnectionTests: XCTestCase {
    private var tempDirectory: URL!
    private var persistenceURL: URL!

    override func setUpWithError() throws {
        tempDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("provider-connection-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDirectory, withIntermediateDirectories: true)
        persistenceURL = tempDirectory.appendingPathComponent("provider-settings.json")
    }

    override func tearDownWithError() throws {
        if let tempDirectory {
            try? FileManager.default.removeItem(at: tempDirectory)
        }
    }

    private func profile(baseURL: String = "https://api.deepseek.com") -> ProviderStoredProfile {
        ProviderStoredProfile(
            id: "deepseek",
            presetID: "deepseek",
            name: "DeepSeek",
            baseURL: baseURL,
            modelID: "deepseek-flash",
            enabled: true,
            isDefault: false
        )
    }

    private func jsonObject(from string: String) throws -> [String: Any]? {
        let data = try XCTUnwrap(string.data(using: .utf8))
        return try JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    // MARK: - Persistence

    /// Every profile written before this feature existed has no `connections`
    /// key at all; decoding one must not fail and must not invent a connection.
    func testProfileWrittenBeforeConnectionsDecodesAsReachableOneWay() throws {
        let legacy = """
        {"id":"deepseek","presetID":"deepseek","name":"DeepSeek",
         "baseURL":"https://api.deepseek.com","modelID":"deepseek-flash",
         "enabled":true,"isDefault":true}
        """
        let decoded = try JSONDecoder().decode(
            ProviderStoredProfile.self,
            from: XCTUnwrap(legacy.data(using: .utf8))
        )
        XCTAssertTrue(decoded.connections.isEmpty)
        XCTAssertEqual(decoded.baseURL, "https://api.deepseek.com")
    }

    func testConnectionsSurviveAnEncodeDecodeRoundTrip() throws {
        var subject = profile()
        subject.addConnection()
        subject.connections[1].id = "cn"
        subject.connections[1].baseURL = "https://api.deepseek.cn/v1"
        subject.connections[1].modelIDs = ["deepseek-flash"]

        let data = try JSONEncoder().encode(subject)
        let decoded = try JSONDecoder().decode(ProviderStoredProfile.self, from: data)
        XCTAssertEqual(decoded, subject)
        XCTAssertEqual(decoded.connections.map(\.id), ["default", "cn"])
        XCTAssertEqual(decoded.connections[1].modelIDs, ["deepseek-flash"])
    }

    // MARK: - Editing rules

    /// The first add must not leave the configured endpoint behind: it becomes
    /// `default`, so the way the provider was already reachable is preserved.
    func testFirstAddMigratesTheFlatProfileIntoTwoConnections() {
        var subject = profile()
        subject.addConnection()

        XCTAssertEqual(subject.connections.count, 2)
        XCTAssertEqual(subject.connections[0].id, "default")
        XCTAssertEqual(subject.connections[0].baseURL, "https://api.deepseek.com")
        XCTAssertEqual(subject.connections[1].id, "")
        XCTAssertEqual(subject.connections[1].baseURL, "")
    }

    func testFurtherAddsAppendASingleEmptyConnection() {
        var subject = profile()
        subject.addConnection()
        subject.addConnection()

        XCTAssertEqual(subject.connections.count, 3)
        XCTAssertEqual(subject.connections[0].id, "default")
    }

    /// Dropping back to one connection collapses to a flat provider AND lifts
    /// the survivor's endpoint, so the provider stays reachable at the URL the
    /// surviving connection named rather than the one it replaced.
    func testRemovingBackToOneCollapsesAndKeepsTheSurvivingEndpoint() {
        var subject = profile()
        subject.addConnection()
        subject.connections[1].id = "cn"
        subject.connections[1].baseURL = "https://api.deepseek.cn/v1"

        subject.removeConnection(at: 0)

        XCTAssertTrue(subject.connections.isEmpty)
        XCTAssertEqual(subject.baseURL, "https://api.deepseek.cn/v1")
    }

    func testRemovingAnOutOfRangeIndexChangesNothing() {
        var subject = profile()
        subject.addConnection()
        let before = subject

        subject.removeConnection(at: 7)

        XCTAssertEqual(subject, before)
    }

    // MARK: - Validation

    func testAProviderWithoutConnectionsValidates() throws {
        XCTAssertNoThrow(try profile().validateConnections())
    }

    func testAnEmptyConnectionIDIsRejected() {
        var subject = profile()
        subject.addConnection()
        subject.connections[1].baseURL = "https://api.deepseek.cn/v1"

        XCTAssertThrowsError(try subject.validateConnections()) { error in
            XCTAssertEqual(error as? ProviderProfileValidationError, .connectionMissingID)
        }
    }

    /// `:` would produce `group:a:b`, which the engine splits at the wrong
    /// colon; `/` would break the `profile/model` qualified reference.
    func testConnectionIDsCannotCarryTheEnginesSeparators() {
        for bad in ["cn:1", "cn/1"] {
            var subject = profile()
            subject.addConnection()
            subject.connections[1].id = bad
            subject.connections[1].baseURL = "https://api.deepseek.cn/v1"

            XCTAssertThrowsError(try subject.validateConnections(), "\(bad) must be rejected") { error in
                XCTAssertEqual(error as? ProviderProfileValidationError, .connectionMissingID)
            }
        }
    }

    func testDuplicateConnectionIDsAreRejected() {
        var subject = profile()
        subject.addConnection()
        subject.connections[1].id = "default"
        subject.connections[1].baseURL = "https://api.deepseek.cn/v1"

        XCTAssertThrowsError(try subject.validateConnections()) { error in
            XCTAssertEqual(error as? ProviderProfileValidationError, .connectionDuplicateID("default"))
        }
    }

    /// A connection with no endpoint of its own would inherit the provider's —
    /// i.e. point at the endpoint the user is adding one BESIDE.
    func testAConnectionMustCarryItsOwnEndpoint() {
        var subject = profile()
        subject.addConnection()
        subject.connections[1].id = "cn"

        XCTAssertThrowsError(try subject.validateConnections()) { error in
            XCTAssertEqual(error as? ProviderProfileValidationError, .connectionInvalidBaseURL("cn"))
        }
    }

    func testANonHTTPConnectionEndpointIsRejected() {
        var subject = profile()
        subject.addConnection()
        subject.connections[1].id = "cn"
        subject.connections[1].baseURL = "ftp://api.deepseek.cn"

        XCTAssertThrowsError(try subject.validateConnections()) { error in
            XCTAssertEqual(error as? ProviderProfileValidationError, .connectionInvalidBaseURL("cn"))
        }
    }

    // MARK: - What the engine is launched with

    func testTwoConnectionsReachTheEmittedProviderSettings() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepseek = repository.addProfile(presetID: "deepseek")
        repository.updateProfile(deepseek) {
            $0.baseURL = "https://api.deepseek.com"
            $0.modelID = "deepseek-flash"
            $0.addConnection()
            $0.connections[1].id = "cn"
            $0.connections[1].baseURL = "https://api.deepseek.cn/v1"
            $0.connections[1].modelIDs = ["deepseek-flash"]
        }

        let providers = try XCTUnwrap(jsonObject(from: repository.makeLaunchSnapshot().providerProfilesJSON))
        let entry = try XCTUnwrap(providers[deepseek] as? [String: Any])
        let connections = try XCTUnwrap(entry["connections"] as? [[String: Any]])

        XCTAssertEqual(connections.count, 2)
        XCTAssertEqual(connections[0]["id"] as? String, "default")
        XCTAssertEqual(connections[0]["baseUrl"] as? String, "https://api.deepseek.com")
        XCTAssertNil(connections[0]["models"], "a connection serving every model must not pin a subset")
        XCTAssertEqual(connections[1]["id"] as? String, "cn")
        XCTAssertEqual(connections[1]["baseUrl"] as? String, "https://api.deepseek.cn/v1")
        XCTAssertEqual((connections[1]["models"] as? [[String: Any]])?.first?["id"] as? String, "deepseek-flash")
    }

    /// The regression this feature is one line away from: a provider sitting on
    /// the catalog's official endpoint is deliberately SKIPPED from the emitted
    /// map (`testLaunchSnapshotUsesBuiltInProfileForOfficialEndpoint` pins that
    /// it emits nothing at all). Adding connections to such a provider must
    /// take it out of that class — otherwise the connections are dropped with
    /// no error on any surface, and the picker still looks right.
    func testAnOfficialEndpointProviderWithConnectionsIsStillEmitted() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepseek = repository.addProfile(presetID: "deepseek")
        repository.updateProfile(deepseek) {
            $0.modelID = "deepseek-flash"
            $0.enabled = true
        }

        let before = try XCTUnwrap(jsonObject(from: repository.makeLaunchSnapshot().providerProfilesJSON))
        XCTAssertEqual(before.count, 0, "precondition: the official endpoint is emitted as a built-in")

        repository.updateProfile(deepseek) {
            $0.addConnection()
            $0.connections[1].id = "cn"
            $0.connections[1].baseURL = "https://api.deepseek.cn/v1"
        }

        let after = try XCTUnwrap(jsonObject(from: repository.makeLaunchSnapshot().providerProfilesJSON))
        XCTAssertEqual(after.count, 1)
        // Named exactly like the preset: `assemble` REPLACES the built-in of the
        // same name (provider-config/tests/preset_name_collision_test.rs), which
        // is also what keeps the stored credential resolving. A different name
        // here would strand the key the user already saved.
        XCTAssertEqual(Array(after.keys), [deepseek])
        let entry = try XCTUnwrap(after[deepseek] as? [String: Any])
        XCTAssertEqual((entry["connections"] as? [[String: Any]])?.count, 2)
    }

    /// The model reference keeps naming the GROUP as connections come and go —
    /// `resolve_in` matches a profile name or a group, and the mobile clients
    /// derive their model label from this id.
    func testTheModelReferenceStillNamesTheGroup() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepseek = repository.addProfile(presetID: "deepseek")
        repository.updateProfile(deepseek) {
            $0.modelID = "deepseek-flash"
            $0.enabled = true
            $0.addConnection()
            $0.connections[1].id = "cn"
            $0.connections[1].baseURL = "https://api.deepseek.cn/v1"
        }
        repository.setDefaultProfile(deepseek)

        let snapshot = repository.makeLaunchSnapshot()
        XCTAssertEqual(snapshot.defaultModelID, "deepseek/deepseek-flash")
        XCTAssertFalse(
            snapshot.defaultModelID?.contains(":") ?? false,
            "a connection id must not leak into the model reference"
        )
    }
}

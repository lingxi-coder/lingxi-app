import XCTest
@testable import LingxiCode

final class ProviderBulkImportTests: XCTestCase {

    /// A provider reachable several ways must SURVIVE import. `connections` was
    /// absent from the carried-key set, so a multi-connection config used to be
    /// dropped with only a warning — silently yielding a single-connection
    /// provider, which is the worst possible outcome for a routing config.
    func testMultiConnectionProviderSurvivesImport() throws {
        let json = #"{"providers":{"deepseek":{"type":"openai","models":[{"id":"deepseek-flash"}],"connections":[{"id":"intl","baseUrl":"https://api.deepseek.test"},{"id":"cn","baseUrl":"https://cn.deepseek.test/v1"}],"fallback":{"on":["rate_limit","auth"]}}}}"#
        let document = ProviderBulkImport.parse(json, existing: [:])
        let entry = try XCTUnwrap(document.entries.first)
        XCTAssertNil(ProviderBulkImport.validate(entry, credentialConfigured: true))
        let connections = try XCTUnwrap(entry.draft["connections"] as? [[String: Any]])
        XCTAssertEqual(connections.count, 2)
        XCTAssertEqual(connections[0]["id"] as? String, "intl")
        XCTAssertEqual(connections[1]["baseUrl"] as? String, "https://cn.deepseek.test/v1")
        XCTAssertNotNil(entry.draft["fallback"])
    }

    /// The provider row only supplies defaults, so it need not carry `baseUrl`
    /// itself when every connection does.
    func testProviderLevelBaseUrlIsNotRequiredWhenConnectionsSupplyIt() throws {
        let json = #"{"providers":{"p":{"type":"openai","models":[{"id":"m"}],"connections":[{"id":"a","baseUrl":"https://a.test/v1"}]}}}"#
        let entry = try XCTUnwrap(ProviderBulkImport.parse(json, existing: [:]).entries.first)
        XCTAssertNil(ProviderBulkImport.validate(entry, credentialConfigured: true))
    }

    /// Every connection is validated as the flat provider it desugars to, and
    /// the failure must survive rather than one good endpoint masking a bad one.
    func testABrokenConnectionFailsValidationEvenWhenSiblingsAreValid() throws {
        let json = #"{"providers":{"p":{"type":"openai","models":[{"id":"m"}],"connections":[{"id":"good","baseUrl":"https://good.test/v1"},{"id":"bad","baseUrl":"not-a-url"}]}}}"#
        let entry = try XCTUnwrap(ProviderBulkImport.parse(json, existing: [:]).entries.first)
        XCTAssertNotNil(ProviderBulkImport.validate(entry, credentialConfigured: true))
    }

    /// A connection id becomes part of a qualified model reference, so a
    /// separator in it would produce a reference that cannot be routed.
    func testConnectionIdsRejectReferenceSeparators() throws {
        for bad in ["a/b", "a:b", "a#b"] {
            let json = "{\"providers\":{\"p\":{\"type\":\"openai\",\"baseUrl\":\"https://x.test/v1\",\"models\":[{\"id\":\"m\"}],\"connections\":[{\"id\":\"\(bad)\"}]}}}"
            let entry = try XCTUnwrap(ProviderBulkImport.parse(json, existing: [:]).entries.first)
            XCTAssertNotNil(ProviderBulkImport.validate(entry, credentialConfigured: true), "id \(bad) must be rejected")
        }
    }

    /// A connection written with the shorthand `models: ["m"]` must be
    /// normalized like a provider-level list, not passed through unchanged.
    func testConnectionModelsAreNormalizedLikeProviderModels() throws {
        let json = #"{"providers":{"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"connections":[{"id":"a","models":["shorthand"]}]}}}"#
        let entry = try XCTUnwrap(ProviderBulkImport.parse(json, existing: [:]).entries.first)
        let connections = try XCTUnwrap(entry.draft["connections"] as? [[String: Any]])
        let models = try XCTUnwrap(connections[0]["models"] as? [[String: Any]])
        XCTAssertEqual(models.first?["id"] as? String, "shorthand")
        XCTAssertNil(ProviderBulkImport.validate(entry, credentialConfigured: true))
    }

    /// Duplicate connection ids would desugar to two profiles with one name.
    func testDuplicateConnectionIdsAreRejected() throws {
        let json = #"{"providers":{"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"connections":[{"id":"a"},{"id":"a"}]}}}"#
        let entry = try XCTUnwrap(ProviderBulkImport.parse(json, existing: [:]).entries.first)
        XCTAssertNotNil(ProviderBulkImport.validate(entry, credentialConfigured: true))
    }

    /// `apiKeys` names stored credentials; it must be distinct non-empty ids.
    func testApiKeysMustBeDistinctNonEmptyStrings() throws {
        let ok = #"{"providers":{"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"apiKeys":["k1","k2"]}}}"#
        XCTAssertNil(ProviderBulkImport.validate(try XCTUnwrap(ProviderBulkImport.parse(ok, existing: [:]).entries.first), credentialConfigured: true))
        let dupe = #"{"providers":{"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"apiKeys":["k1","k1"]}}}"#
        XCTAssertNotNil(ProviderBulkImport.validate(try XCTUnwrap(ProviderBulkImport.parse(dupe, existing: [:]).entries.first), credentialConfigured: true))
    }

    /// An unknown trigger silently disables failover, which is invisible until
    /// the day it is needed — so it must be rejected by name.
    func testFallbackTriggersAreCheckedAgainstWhatTheEngineImplements() throws {
        let bad = #"{"providers":{"p":{"type":"openai","baseUrl":"https://x.test/v1","models":[{"id":"m"}],"connections":[{"id":"a"}],"fallback":{"on":["rate_limits"]}}}}"#
        XCTAssertNotNil(ProviderBulkImport.validate(try XCTUnwrap(ProviderBulkImport.parse(bad, existing: [:]).entries.first), credentialConfigured: true))
    }
    func testNativeWrappedImportExtractsSecretAndRetainsMetadata() throws {
        let document = ProviderBulkImport.parse(#"{"providers":{"custom":{"type":"openai","baseUrl":"https://example.test/v1","apiKey":"fake-test-key","models":[{"id":"m","metadata":{"contextWindowTokens":1000,"pricing":{"inputPerMillion":1.2}}}]}}}"#, existing: [:])
        let entry = try XCTUnwrap(document.entries.first)
        XCTAssertEqual(entry.credential, "fake-test-key")
        XCTAssertNil(entry.draft["apiKey"])
        XCTAssertNil(ProviderBulkImport.validate(entry, credentialConfigured: false))
        let merged = try ProviderBulkImport.merge(document.entries, into: ["untouched": ["type": "anthropic"]], configured: [])
        XCTAssertNotNil(merged["untouched"])
        let data = try JSONSerialization.data(withJSONObject: merged)
        XCTAssertFalse(String(decoding: data, as: UTF8.self).contains("fake-test-key"))
        let provider = try XCTUnwrap(merged["custom"] as? [String: Any])
        let model = try XCTUnwrap((provider["models"] as? [[String: Any]])?.first)
        XCTAssertEqual((model["metadata"] as? [String: Any])?["contextWindowTokens"] as? Int, 1000)
    }

    func testOpenCodeMappingPreservesModelAliasAndEnvironmentReference() throws {
        let document = ProviderBulkImport.parse(#"{"provider":{"custom":{"npm":"@ai-sdk/openai","options":{"baseURL":"https://example.test","apiKey":"{env:API_TOKEN}"},"models":{"friendly":{"id":"real-model"}}}},"tools":{"ignored":true}}"#, existing: [:])
        let entry = try XCTUnwrap(document.entries.first)
        XCTAssertEqual(entry.draft["type"] as? String, "openai-responses")
        XCTAssertEqual(entry.draft["apiKeyEnv"] as? String, "API_TOKEN")
        XCTAssertNil(entry.credential)
        XCTAssertEqual((entry.draft["models"] as? [[String: Any]])?.first?["aliases"] as? [String], ["friendly"])
        XCTAssertFalse(document.warnings.isEmpty)
        XCTAssertNil(ProviderBulkImport.validate(entry, credentialConfigured: false))
    }

    func testExistingProfilesStartUnselectedAndMustBeExplicitlyChosen() throws {
        var document = ProviderBulkImport.parse(validInput, existing: ["custom": ["type": "anthropic"]])
        XCTAssertFalse(try XCTUnwrap(document.entries.first).selected)
        XCTAssertThrowsError(try ProviderBulkImport.merge(document.entries, into: [:], configured: []))
        document.entries[0].selected = true
        XCTAssertNoThrow(try ProviderBulkImport.merge(document.entries, into: [:], configured: []))
    }

    func testNestedCredentialsAreRejectedWithoutDiagnosticsLeakingValues() throws {
        let document = ProviderBulkImport.parse(#"{"custom":{"type":"openai","baseUrl":"https://example.test","apiKeyEnv":"API_KEY","models":[{"id":"m","metadata":{"password":"do-not-print"}}]}}"#, existing: [:])
        let entry = try XCTUnwrap(document.entries.first)
        XCTAssertFalse(entry.errors.isEmpty)
        XCTAssertFalse(entry.errors.joined().contains("do-not-print"))
        XCTAssertFalse(try String(data: JSONSerialization.data(withJSONObject: entry.draft), encoding: .utf8)!.contains("do-not-print"))
        XCTAssertThrowsError(try ProviderBulkImport.merge(document.entries, into: [:], configured: []))
    }

    func testUnsafeURLDuplicateModelsAndUnknownSDKAreRejected() throws {
        var entry = try XCTUnwrap(ProviderBulkImport.parse(validInput, existing: [:]).entries.first)
        entry.draft["baseUrl"] = "https://user:password@example.test"
        XCTAssertNotNil(ProviderBulkImport.validate(entry, credentialConfigured: false))
        entry.draft["baseUrl"] = "https://example.test"
        entry.draft["models"] = [["id": "duplicate"], ["id": "duplicate"]]
        XCTAssertNotNil(ProviderBulkImport.validate(entry, credentialConfigured: false))
        let unknown = ProviderBulkImport.parse(#"{"provider":{"custom":{"npm":"unknown","options":{},"models":{"m":{}}}}}"#, existing: [:])
        XCTAssertFalse(try XCTUnwrap(unknown.entries.first).errors.isEmpty)
    }

    func testCredentialReferenceAndAmbiguousFormatsFailClosed() throws {
        XCTAssertFalse(ProviderBulkImport.parse(#"{"provider":{},"providers":{}}"#, existing: [:]).errors.isEmpty)
        let document = ProviderBulkImport.parse(#"{"custom":{"type":"openai","baseUrl":"https://example.test","apiKey":"{file:secret.txt}","models":["m"]}}"#, existing: [:])
        XCTAssertFalse(try XCTUnwrap(document.entries.first).errors.isEmpty)
        XCTAssertNil(document.entries.first?.credential)
    }

    func testBooleanPricingDoesNotPassAsNumber() throws {
        var entry = try XCTUnwrap(ProviderBulkImport.parse(validInput, existing: [:]).entries.first)
        entry.draft["pricing"] = ["m": ["inputPerMtok": true, "outputPerMtok": 1]]
        XCTAssertNotNil(ProviderBulkImport.validate(entry, credentialConfigured: false))
    }

    private var validInput: String { #"{"custom":{"type":"openai","baseUrl":"https://example.test","apiKeyEnv":"API_KEY","models":["m"]}}"# }
}

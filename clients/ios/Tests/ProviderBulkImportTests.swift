import XCTest
@testable import LingxiCode

final class ProviderBulkImportTests: XCTestCase {
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

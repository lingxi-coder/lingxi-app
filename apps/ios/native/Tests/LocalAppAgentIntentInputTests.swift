import AppIntents
import Foundation
import UniformTypeIdentifiers
import XCTest
@testable import LingxiCode

final class LocalAppAgentIntentInputTests: XCTestCase {
    func testFileLargerThanLimitIsRejectedFromFileMetadata() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("local-app-agent-\(UUID().uuidString).bin")
        defer { try? FileManager.default.removeItem(at: url) }
        XCTAssertTrue(FileManager.default.createFile(atPath: url.path, contents: Data()))

        let handle = try FileHandle(forWritingTo: url)
        try handle.truncate(atOffset: UInt64(LocalAppAgentIntentInput.maximumFileBytes + 1))
        try handle.close()

        let file = IntentFile(fileURL: url, filename: "large.bin", type: .data)
        XCTAssertThrowsError(try LocalAppAgentIntentInput.attachments([file])) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "INVALID_AGENT_ATTACHMENTS")
        }
    }

    func testUnsafeFilenameIsRejectedBeforeReadingFile() {
        let file = IntentFile(data: Data("safe payload".utf8), filename: "../secret.txt", type: .plainText)
        XCTAssertThrowsError(try LocalAppAgentIntentInput.attachments([file])) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "INVALID_AGENT_ATTACHMENTS")
        }
    }

    func testOutputSchemaSizeIsBoundedBeforeJSONDecode() {
        let oversizedValidJSON = String(repeating: " ", count: LocalAppAgentIntentInput.maximumSchemaBytes) + "{}"
        XCTAssertThrowsError(try LocalAppAgentIntentInput.outputSchema(oversizedValidJSON)) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "INVALID_AGENT_INPUT")
        }
    }

    func testOutputSchemaRequiresJSONObject() throws {
        XCTAssertEqual(try LocalAppAgentIntentInput.outputSchema(#"{"type":"object"}"#), .object(["type": .string("object")]))
        XCTAssertThrowsError(try LocalAppAgentIntentInput.outputSchema(#"["not","an","object"]"#)) { error in
            XCTAssertEqual((error as? LocalAppServiceFailure)?.code, "INVALID_AGENT_INPUT")
        }
    }
}

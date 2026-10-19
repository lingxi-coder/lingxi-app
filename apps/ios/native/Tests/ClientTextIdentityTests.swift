import XCTest
@testable import LingxiCode

#if canImport(harness_runtimeFFI)
@MainActor
final class ClientTextIdentityTests: XCTestCase {
    func testReplaySignatureDistinguishesSourceUnitsWithTheSameDisplay() {
        let texts = [
            ClientTextDto(codeUnits: [0xD800]),
            ClientTextDto(codeUnits: [0xD801]),
            ClientTextDto(codeUnits: [0xFFFD]),
        ]
        XCTAssertEqual(Set(texts.map(\.displayText)).count, 1)
        let signatures = texts.map {
            EngineConversationSource.messageSignature(MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: $0)]))
        }
        XCTAssertEqual(Set(signatures).count, texts.count)
        XCTAssertEqual(
            signatures[0],
            EngineConversationSource.messageSignature(MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: texts[0])]))
        )
    }

    func testReplaySignatureKeepsCodeUnitAndBlockBoundaries() {
        func signature(_ blocks: [[UInt16]]) -> String {
            EngineConversationSource.messageSignature(MessageDto(transcriptUuid: nil,
                role: "assistant",
                blocks: blocks.map { .text(text: ClientTextDto(codeUnits: $0)) }
            ))
        }
        XCTAssertNotEqual(signature([[0x1, 0x23]]), signature([[0x12, 0x3]]))
        XCTAssertNotEqual(signature([[0x61], [0x62]]), signature([[0x61, 0x62]]))
        XCTAssertNotEqual(signature([[0xE9]]), signature([[0x65, 0x301]]))
    }
}
#endif

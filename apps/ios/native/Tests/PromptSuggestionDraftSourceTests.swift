import XCTest
@testable import LingxiCode

final class PromptSuggestionDraftSourceTests: XCTestCase {
    func testAppendingAfterDisplayedIsolatedSurrogatePreservesItsSourceUnit() throws {
        let source = [UInt16(0xD800), 0x61]
        let displayed = PromptSuggestionDraftSource.displayCodeUnits(for: source)
        let edited = displayed + [UInt16(0x78)]

        XCTAssertEqual(displayed, [0xFFFD, 0x61])
        let updated = try XCTUnwrap(
            PromptSuggestionDraftSource.applyingEdit(
                sourceCodeUnits: source,
                previousDisplayCodeUnits: displayed,
                newDisplayCodeUnits: edited
            )
        )
        XCTAssertEqual(updated, [UInt16(0xD800), 0x61, 0x78])
    }

    func testEditingDisplayedReplacementReplacesOnlyItsSourceUnit() throws {
        let suffix = Array("😃 tail".utf16)
        let source = [UInt16(0xD800)] + suffix
        let displayed = PromptSuggestionDraftSource.displayCodeUnits(for: source)
        let edited = [UInt16(0x78)] + Array("😃 tail".utf16)

        let updated = try XCTUnwrap(
            PromptSuggestionDraftSource.applyingEdit(
                sourceCodeUnits: source,
                previousDisplayCodeUnits: displayed,
                newDisplayCodeUnits: edited
            )
        )
        XCTAssertEqual(updated, [UInt16(0x78)] + suffix)
    }

    func testDeletingDisplayedReplacementRemovesItsSourceUnit() throws {
        let suffix = Array("tail".utf16)
        let source = [UInt16(0xD800)] + suffix
        let displayed = PromptSuggestionDraftSource.displayCodeUnits(for: source)

        let updated = try XCTUnwrap(
            PromptSuggestionDraftSource.applyingEdit(
                sourceCodeUnits: source,
                previousDisplayCodeUnits: displayed,
                newDisplayCodeUnits: suffix
            )
        )
        XCTAssertEqual(updated, suffix)
    }

    func testCanonicalEquivalentEditUpdatesUtf16SourceUnits() throws {
        let composed = Array("é".utf16)
        let decomposed = Array("e\u{301}".utf16)
        let source = [UInt16(0xD800)] + composed
        let previousDisplay = PromptSuggestionDraftSource.displayCodeUnits(for: source)
        let newDisplay = [UInt16(0xFFFD)] + decomposed

        let updated = try XCTUnwrap(
            PromptSuggestionDraftSource.applyingEdit(
                sourceCodeUnits: source,
                previousDisplayCodeUnits: previousDisplay,
                newDisplayCodeUnits: newDisplay
            )
        )
        XCTAssertEqual(updated, [UInt16(0xD800)] + decomposed)
    }

    func testUtf16EditDoesNotCollapseCanonicalEquivalentStringChanges() throws {
        let source = Array("é".utf16)
        let previousDisplay = source
        let decomposed = Array("e\u{301}".utf16)
        XCTAssertEqual(
            String(decoding: source, as: UTF16.self),
            String(decoding: decomposed, as: UTF16.self)
        )

        let updated = try XCTUnwrap(
            PromptSuggestionDraftSource.applyingEdit(
                sourceCodeUnits: source,
                previousDisplayCodeUnits: previousDisplay,
                newDisplayCodeUnits: decomposed
            )
        )
        XCTAssertEqual(updated, decomposed)
        XCTAssertNotEqual(previousDisplay, updated)
    }

    func testWhitespaceTrimPreservesRawSuffixUnits() throws {
        let source: [UInt16] = [0x20, 0x2F, 0x68, 0x65, 0x6C, 0x70, 0x20, 0xD800, 0x20]
        let display = String(decoding: source, as: UTF16.self)

        let trimmed = try XCTUnwrap(
            PromptSuggestionDraftSource.trimmingWhitespace(
                sourceCodeUnits: source,
                displayText: display
            )
        )
        XCTAssertEqual(trimmed, [0x2F, 0x68, 0x65, 0x6C, 0x70, 0x20, 0xD800])
    }
}

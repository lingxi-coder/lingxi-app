import XCTest

@testable import LingxiCode

final class StreamingSpeechSegmenterTests: XCTestCase {
    func testStrongPunctuationEmitsBeforeTurnCompletion() {
        var segmenter = StreamingSpeechSegmenter()

        XCTAssertEqual(segmenter.append("你好，这是第一句。后面"), ["你好，这是第一句。"])
        XCTAssertEqual(segmenter.finish(finalText: "你好，这是第一句。后面结束"), ["后面结束"])
    }

    func testWeakAndHardBoundariesKeepLongStreamsMoving() {
        var weak = StreamingSpeechSegmenter(
            minimumSegmentLength: 3,
            softBoundaryThreshold: 8,
            hardBoundaryThreshold: 16
        )
        XCTAssertEqual(weak.append("这是一个较长短句，仍在继续"), ["这是一个较长短句，"])

        var hard = StreamingSpeechSegmenter(
            minimumSegmentLength: 3,
            softBoundaryThreshold: 8,
            hardBoundaryThreshold: 12
        )
        XCTAssertEqual(hard.append("abcdefghijklmnop"), ["abcdefghijkl"])

        var latePunctuation = StreamingSpeechSegmenter(
            minimumSegmentLength: 3,
            softBoundaryThreshold: 8,
            hardBoundaryThreshold: 12
        )
        XCTAssertEqual(
            latePunctuation.append("abcdefghijklmnop,tail"),
            ["abcdefghijkl", "mnop,"]
        )
    }

    func testMarkdownFenceCanSpanTransportDeltas() {
        var segmenter = StreamingSpeechSegmenter(minimumSegmentLength: 2)

        XCTAssertTrue(segmenter.append("答案。``").contains("答案。"))
        XCTAssertTrue(segmenter.append("`swift\nprint(1)\n``").isEmpty)
        XCTAssertEqual(segmenter.append("`继续。"), ["继续。"])
    }

    func testTerminalFallbackSpeaksProviderWithoutDeltasOnce() {
        var segmenter = StreamingSpeechSegmenter()

        XCTAssertEqual(segmenter.finish(finalText: "完整回复"), ["完整回复"])
        XCTAssertTrue(segmenter.finish(finalText: "完整回复").isEmpty)
    }

    func testTerminalPrefixOnlyAddsMissingSuffix() {
        var segmenter = StreamingSpeechSegmenter()

        XCTAssertEqual(segmenter.append("这是第一句话。后半"), ["这是第一句话。"])
        XCTAssertEqual(segmenter.finish(finalText: "这是第一句话。后半部分"), ["后半部分"])
    }

    func testTerminalRewriteDoesNotReplayCommittedSpeech() {
        var segmenter = StreamingSpeechSegmenter()

        XCTAssertEqual(segmenter.append("这段已经播报。旧尾部"), ["这段已经播报。"])
        XCTAssertEqual(segmenter.finish(finalText: "完全改写后的结果"), ["旧尾部"])
        XCTAssertTrue(segmenter.detectedTerminalRewrite)
    }
}

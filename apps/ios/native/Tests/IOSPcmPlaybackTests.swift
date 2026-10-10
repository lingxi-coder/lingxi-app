import Foundation
import XCTest
@testable import LingxiCode

@MainActor
final class IOSPcmPlaybackTests: XCTestCase {
    func testPcmWavePreservesActualRateAndPayload() throws {
        let pcm = Data([1, 0, 255, 127])
        let wave = try IOSPcmPlayback.wave(pcm: pcm, sampleRateHz: 24_000, maximumBytes: 4)
        XCTAssertEqual(String(decoding: wave.prefix(4), as: UTF8.self), "RIFF")
        XCTAssertEqual(wave.suffix(4), pcm)
        XCTAssertEqual(Array(wave[24 ..< 28]), [192, 93, 0, 0])
        XCTAssertEqual(wave.count, 48)
    }

    func testPcmWaveRejectsOddSamplesAndExcessivePayload() {
        XCTAssertThrowsError(try IOSPcmPlayback.wave(pcm: Data([0]), sampleRateHz: 24_000, maximumBytes: 10))
        XCTAssertThrowsError(try IOSPcmPlayback.wave(pcm: Data([0, 0]), sampleRateHz: 24_000, maximumBytes: 1))
        XCTAssertThrowsError(try IOSPcmPlayback.wave(pcm: Data([0, 0]), sampleRateHz: 0, maximumBytes: 10))
    }

}

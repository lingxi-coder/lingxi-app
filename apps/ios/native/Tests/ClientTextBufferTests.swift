import XCTest
@testable import LingxiCode

final class ClientTextBufferTests: XCTestCase {
    func testEveryFragmentPartitionMatchesCompleteNativeDecode() {
        let units: [UInt16] = [0x4e2d, 0xd83d, 0xde00, 0xd800, 0x61, 0xdc00]
        for mask in 0..<(1 << (units.count - 1)) {
            let buffer = ClientTextBuffer()
            var start = 0
            for end in 1...units.count where end == units.count || mask & (1 << (end - 1)) != 0 {
                let snapshot = buffer.append(Array(units[start..<end]))
                XCTAssertEqual(Array(snapshot.utf16), Array(String(decoding: units[..<end], as: UTF16.self).utf16))
                XCTAssertEqual(buffer.append([]), snapshot)
                start = end
            }
        }
    }

    func testDisplaySnapshotsAndNewResponseBuffersStayIndependent() {
        let first = ClientTextBuffer("中文e\u{301}\r\n")
        let incomplete = first.append([0xd83d])
        XCTAssertEqual(Array(first.append([0xde00]).utf16), Array("中文e\u{301}\r\n😀".utf16))
        XCTAssertEqual(Array(incomplete.utf16), Array("中文e\u{301}\r\n�".utf16))
        XCTAssertEqual(ClientTextBuffer().append([0xde00]), "�")
    }
}
